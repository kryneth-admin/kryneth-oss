//! domain/ports.rs — Abstract interfaces (Ports) for Enterprise/OSS features.
//!
//! These traits define the contract that the OSS core depends on.
//! The concrete implementations (Adapters) live in `infrastructure/oss_adapters.rs`.
//! OSS builds receive default/mock adapters.

use crate::error::GatewayError;
use std::future::Future;
use std::pin::Pin;

/// Port for billing operations. The OSS no-op passes everything.
/// The Enterprise adapter reads/deducts from Redis wallet keys.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub trait BillingPort: Send + Sync {
    /// Returns the current wallet balance for a tenant.
    /// OSS no-op returns `f64::MAX` (unlimited).
    fn check_balance<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<f64, GatewayError>> + Send + 'a>>;

    /// Processes post-request billing deduction and analytical logging.
    /// OSS no-op does nothing and returns Ok(()).
    fn process_billing_telemetry<'a>(
        &'a self,
        trace_id: &'a str,
        tenant_id: &'a str,
        provider: &'a str,
        target_model: &'a str,
        prompt_tokens: u64,
        completion_tokens: u64,
        cache_hit: bool,
        is_free_tier: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), GatewayError>> + Send + 'a>>;
}

/// Port for async telemetry event dispatch.
/// The OSS no-op discards events silently.
/// The Enterprise adapter sends to the bounded flume channel → redb WAL → ClickHouse.
pub trait TelemetryPort: Send + Sync {
    /// Fire-and-forget: log a structured telemetry event.
    /// Must never block the caller (bounded channel, try_send semantics).
    fn log_event(&self, event: serde_json::Value);
}

/// Port for resolving API keys.
/// OSS version will resolve from a local Env/YAML map or defaults.
/// Enterprise version will query the Postgres database.
#[allow(clippy::type_complexity)]
pub trait AuthPort: Send + Sync {
    /// Resolves an API key hash to a Tenant ID and their RBAC plan.
    /// Returns a tuple of (tenant_id, account_type).
    fn resolve_api_key<'a>(
        &'a self,
        key_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(String, String), GatewayError>> + Send + 'a>>;
}

// ── Phase 3: Infrastructure Ports ─────────────────────────────────────────────

/// Port for the distributed, per-tenant token-bucket rate limiter.
///
/// **Enterprise adapter** (`EnterpriseRateLimit`): spawns the Tokio background
/// worker from `rate_limit::start_rate_limit_sync_worker`, which periodically
/// pushes the local `DashMap` counters back into Redis via the atomic Lua script
/// so that multi-instance deployments share the same quota.
///
/// **OSS adapter** (`OssRateLimit`): spawns nothing. The local `DashMap` counters
/// in `AppState::rate_limit_cache` still enforce the limit per process — valid
/// for single-node deployments. No `bb8` or `redis` dependency required at
/// compile time in OSS mode.
pub trait RateLimitPort: Send + Sync {
    /// Spawn the background Redis sync worker.
    ///
    /// Called once in `main.rs` immediately after `AppState` is constructed.
    /// The worker holds a clone of `cache` and runs for the process lifetime.
    /// OSS no-op: immediately returns without spawning anything.
    fn start_sync_worker(
        &self,
        cache: std::sync::Arc<
            dashmap::DashMap<String, std::sync::Arc<crate::domain::models::LocalBucket>>,
        >,
    );
}

/// Port for routing-configuration hot-reload via Redis Pub/Sub.
///
/// **Enterprise adapter** (`EnterpriseRoutingConfig`): wraps
/// `infrastructure::config_subscriber::spawn_config_subscriber`. Subscribes to
/// `kryneth:routing_updates` and atomically swaps `RoutingState` on every delta
/// pushed by `kryneth_config`.
///
/// **OSS adapter** (`OssRoutingConfig`): spawns nothing. Routing state was
/// pre-loaded from env/YAML at boot time and is static for the process lifetime.
pub trait RoutingConfigPort: Send + Sync {
    /// Spawn the Pub/Sub subscriber background task.
    ///
    /// Called once in `main.rs`. The task runs for the process lifetime.
    /// OSS no-op: immediately returns without spawning anything.
    fn start_subscriber(&self, routing_state: std::sync::Arc<crate::domain::models::RoutingState>);
}

/// Port for the L2 semantic cache (gRPC to `kryneth_cache:50051`).
///
/// **Enterprise adapter** (`EnterpriseSemanticCache`): dispatches real
/// `LookupCache` / `StoreCache` gRPC RPCs over the shared lazy
/// `tonic::transport::Channel`. Enforces a 2-second deadline on both paths
/// (fail-open: errors become `None` / unit).
///
/// **OSS adapter** (`OssSemanticCache`): always returns `None` on lookup and
/// silently ignores stores. Removes the `tonic` build-time dependency from the
/// OSS binary.
#[allow(clippy::type_complexity)]
pub trait SemanticCachePort: Send + Sync {
    /// Returns true if semantic caching is supported by this adapter.
    fn is_enabled(&self) -> bool { true }

    /// Look up a semantically similar prompt in the L2 cache.
    ///
    /// Returns `Some((response_content, original_prompt))` on a hit, or `None`
    /// on a miss or any transport error (fail-open semantics, never panics).
    fn lookup<'a>(
        &'a self,
        tenant_id: &'a str,
        model: &'a str,
        raw_prompt: &'a str,
        trace_ctx: &'a crate::domain::models::TraceContext,
        vector: &'a [f32],
    ) -> Pin<Box<dyn Future<Output = Option<(String, String)>> + Send + 'a>>;

    /// Store a prompt → response pair in the L2 cache.
    ///
    /// Fire-and-forget: enforces a 2-second internal deadline. Silently drops on
    /// timeout or transport error. Must never block the hot request path.
    fn store<'a>(
        &'a self,
        tenant_id: &'a str,
        model: &'a str,
        raw_prompt: &'a str,
        response_content: &'a str,
        trace_ctx: &'a crate::domain::models::TraceContext,
        vector: &'a [f32],
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
