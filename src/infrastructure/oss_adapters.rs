//! infrastructure/oss_adapters.rs — Concrete OSS-default implementations (Adapters) of domain ports.
//!
//! These adapters provide clean no-ops or process-local fallbacks for the gateway ports,
//! keeping compile-time dependencies to a minimum in OSS builds.

use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::domain::ports::{
    AuthPort, BillingPort, RateLimitPort, RoutingConfigPort, SemanticCachePort, TelemetryPort,
};
use crate::error::GatewayError;

// ── OssTelemetry ─────────────────────────────────────────────────────────────

pub struct OssTelemetry {
    pub trace_store: Arc<dashmap::DashMap<String, serde_json::Value>>,
}

impl OssTelemetry {
    pub fn new(trace_store: Arc<dashmap::DashMap<String, serde_json::Value>>) -> Self {
        Self { trace_store }
    }
}

impl TelemetryPort for OssTelemetry {
    fn log_event(&self, event: serde_json::Value) {
        tracing::info!(telemetry_event = ?event, "OSS Telemetry Log Event");
        if let Some(trace_id) = event
            .get("trace_id")
            .or_else(|| event.get("id"))
            .and_then(|v| v.as_str())
        {
            self.trace_store.insert(trace_id.to_string(), event);
        }
    }
}

// ── OssBilling ────────────────────────────────────────────────────────────────

pub struct OssBilling;

impl BillingPort for OssBilling {
    fn check_balance<'a>(
        &'a self,
        _tenant_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<f64, GatewayError>> + Send + 'a>> {
        Box::pin(async {
            // OSS mode is unlimited balance
            Ok(f64::MAX)
        })
    }

    fn enforce_scoped_budget<'a>(
        &'a self,
        _tenant_id: &'a str,
        _team_id: Option<&'a str>,
        _key_alias: Option<&'a str>,
        _model: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), GatewayError>> + Send + 'a>> {
        Box::pin(async {
            // OSS mode does not enforce budgets
            Ok(())
        })
    }

    fn process_billing_telemetry<'a>(
        &'a self,
        _trace_id: &'a str,
        _tenant_id: &'a str,
        _team_id: Option<&'a str>,
        _api_key_alias: Option<&'a str>,
        _provider: &'a str,
        _target_model: &'a str,
        _prompt_tokens: u64,
        _completion_tokens: u64,
        _mcp_calls: u32,
        _agent_loops: u32,
        _cache_hit: bool,
        _is_free_tier: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), GatewayError>> + Send + 'a>> {
        Box::pin(async {
            // OSS mode billing analytics are no-op
            Ok(())
        })
    }
}

// ── OssAuth ───────────────────────────────────────────────────────────────────

pub struct OssAuth;

impl AuthPort for OssAuth {
    fn resolve_api_key<'a>(
        &'a self,
        key_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(String, String), GatewayError>> + Send + 'a>> {
        Box::pin(async move {
            let valid_keys_env = std::env::var("KRYNETH_VALID_KEYS").unwrap_or_default();
            for raw_key in valid_keys_env.split(',') {
                let trimmed = raw_key.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let mut hasher = Sha256::new();
                hasher.update(trimmed.as_bytes());
                let candidate_hash = hex::encode(hasher.finalize());

                if candidate_hash == key_hash {
                    // Authenticated successfully! Return a default tenant ID and account type.
                    return Ok((
                        "00000000-0000-0000-0000-000000000000".to_string(),
                        "solo".to_string(),
                    ));
                }
            }

            tracing::warn!("API key hash mismatch against KRYNETH_VALID_KEYS");
            Err(GatewayError::Unauthorized)
        })
    }
}

// ── OssRateLimit ─────────────────────────────────────────────────────────────

pub struct OssRateLimit;

impl RateLimitPort for OssRateLimit {
    fn start_sync_worker(
        &self,
        cache: Arc<dashmap::DashMap<String, Arc<crate::domain::models::LocalBucket>>>,
    ) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            // First tick fires immediately; we skip it to sleep first
            interval.tick().await;

            loop {
                interval.tick().await;
                tracing::debug!("Resetting process-local rate limit buckets");
                for entry in cache.iter() {
                    let bucket = entry.value();
                    bucket
                        .consumed
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                    bucket
                        .is_blocked
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }
        });
    }
}

// ── OssRoutingConfig ─────────────────────────────────────────────────────────

pub struct OssRoutingConfig;

impl RoutingConfigPort for OssRoutingConfig {
    fn start_subscriber(
        &self,
        _routing_state: Arc<crate::domain::models::RoutingState>,
        _mcp_registry: Arc<crate::infrastructure::mcp_registry::McpConnectionRegistry>,
    ) {
        // OSS is static and loaded from file at boot time
    }
}

// ── OssSemanticCache ─────────────────────────────────────────────────────────

pub struct OssSemanticCache;

impl SemanticCachePort for OssSemanticCache {
    fn is_enabled(&self) -> bool {
        false
    }

    fn lookup<'a>(
        &'a self,
        _tenant_id: &'a str,
        _model: &'a str,
        _raw_prompt: &'a str,
        _trace_ctx: &'a crate::domain::models::TraceContext,
        _vector: &'a [f32],
    ) -> Pin<Box<dyn Future<Output = Option<(String, String)>> + Send + 'a>> {
        Box::pin(async {
            // OSS mode is always a cache miss (L2 Semantic Cache is disabled)
            None
        })
    }

    fn store<'a>(
        &'a self,
        _tenant_id: &'a str,
        _model: &'a str,
        _raw_prompt: &'a str,
        _response_content: &'a str,
        _trace_ctx: &'a crate::domain::models::TraceContext,
        _vector: &'a [f32],
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {
            // OSS mode does not store in L2 Semantic Cache
        })
    }
}

// ── MokaExecutionStore ────────────────────────────────────────────────────────

use crate::domain::execution::{ExecutionContext, ExecutionState, IdempotencyKey, ToolExecution};
use crate::domain::ports::{ExecutionStore, Reconciler, ReconciliationResult};
use chrono::Utc;

pub struct MokaExecutionStore {
    pub cache: moka::future::Cache<String, crate::domain::models::OperationCacheEntry>,
}

impl MokaExecutionStore {
    pub fn new(
        cache: moka::future::Cache<String, crate::domain::models::OperationCacheEntry>,
    ) -> Self {
        Self { cache }
    }
}

impl ExecutionStore for MokaExecutionStore {
    fn create_or_claim<'a>(
        &'a self,
        execution: ToolExecution,
        lease_duration: std::time::Duration,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<(ExecutionState, ExecutionContext), GatewayError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let key = execution.idempotency_key.0.clone();
            let now = Utc::now();
            let lease_until = std::time::Instant::now() + lease_duration;

            if let Some(mut existing) = self.cache.get(&key).await {
                match existing.state {
                    ExecutionState::Succeeded => {
                        return Ok((existing.state, existing.context));
                    }
                    ExecutionState::Claimed | ExecutionState::Running => {
                        let is_lease_expired = existing
                            .context
                            .lease_until
                            .map(|t| std::time::Instant::now() > t)
                            .unwrap_or(true);

                        if !is_lease_expired {
                            return Ok((existing.state, existing.context));
                        }

                        existing.state = ExecutionState::Claimed;
                        existing.context.version += 1;
                        existing.context.lease_until = Some(lease_until);
                        existing.context.error_message = Some("Lease expired".to_string());
                        existing.execution.attempt += 1;
                        existing.execution.state = ExecutionState::Claimed;
                        existing.execution.updated_at = now;

                        self.cache.insert(key, existing.clone()).await;
                        return Ok((existing.state, existing.context));
                    }
                    ExecutionState::Failed => {
                        existing.state = ExecutionState::Claimed;
                        existing.context.version += 1;
                        existing.context.lease_until = Some(lease_until);
                        existing.execution.attempt += 1;
                        existing.execution.state = ExecutionState::Claimed;
                        existing.execution.updated_at = now;

                        self.cache.insert(key, existing.clone()).await;
                        return Ok((existing.state, existing.context));
                    }
                    ExecutionState::Unknown => {
                        return Ok((existing.state, existing.context));
                    }
                    ExecutionState::Pending | ExecutionState::Reconciling => {
                        return Ok((existing.state, existing.context));
                    }
                }
            }

            let context = ExecutionContext {
                result_content: None,
                latency_ms: None,
                error_message: None,
                lease_until: Some(lease_until),
                version: 1,
            };

            let mut exec = execution;
            exec.state = ExecutionState::Claimed;
            exec.created_at = now;
            exec.updated_at = now;

            let new_entry = crate::domain::models::OperationCacheEntry {
                state: ExecutionState::Claimed,
                context: context.clone(),
                execution: exec,
            };

            let entry = self.cache.get_with(key, async { new_entry }).await;
            Ok((entry.state, entry.context))
        })
    }

    fn get<'a>(
        &'a self,
        idempotency_key: &'a IdempotencyKey,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Option<(ExecutionState, ExecutionContext, ToolExecution)>,
                        GatewayError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if let Some(entry) = self.cache.get(&idempotency_key.0).await {
                Ok(Some((entry.state, entry.context, entry.execution)))
            } else {
                Ok(None)
            }
        })
    }

    fn mark_running<'a>(
        &'a self,
        idempotency_key: &'a IdempotencyKey,
        version: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), GatewayError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(mut entry) = self.cache.get(&idempotency_key.0).await {
                if entry.context.version == version {
                    entry.state = ExecutionState::Running;
                    entry.execution.state = ExecutionState::Running;
                    entry.execution.updated_at = Utc::now();
                    self.cache.insert(idempotency_key.0.clone(), entry).await;
                }
            }
            Ok(())
        })
    }

    fn mark_succeeded<'a>(
        &'a self,
        idempotency_key: &'a IdempotencyKey,
        version: u64,
        content: String,
        latency_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), GatewayError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(mut entry) = self.cache.get(&idempotency_key.0).await {
                if entry.context.version == version {
                    entry.state = ExecutionState::Succeeded;
                    entry.context.result_content = Some(content);
                    entry.context.latency_ms = Some(latency_ms);
                    entry.execution.state = ExecutionState::Succeeded;
                    entry.execution.updated_at = Utc::now();
                    self.cache.insert(idempotency_key.0.clone(), entry).await;
                }
            }
            Ok(())
        })
    }

    fn mark_failed<'a>(
        &'a self,
        idempotency_key: &'a IdempotencyKey,
        version: u64,
        reason: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), GatewayError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(mut entry) = self.cache.get(&idempotency_key.0).await {
                if entry.context.version == version {
                    entry.state = ExecutionState::Failed;
                    entry.context.error_message = Some(reason);
                    entry.execution.state = ExecutionState::Failed;
                    entry.execution.updated_at = Utc::now();
                    self.cache.insert(idempotency_key.0.clone(), entry).await;
                }
            }
            Ok(())
        })
    }

    fn mark_unknown<'a>(
        &'a self,
        idempotency_key: &'a IdempotencyKey,
        version: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), GatewayError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(mut entry) = self.cache.get(&idempotency_key.0).await {
                if entry.context.version == version {
                    entry.state = ExecutionState::Unknown;
                    entry.execution.state = ExecutionState::Unknown;
                    entry.execution.updated_at = Utc::now();
                    self.cache.insert(idempotency_key.0.clone(), entry).await;
                }
            }
            Ok(())
        })
    }

    fn transition<'a>(
        &'a self,
        idempotency_key: &'a IdempotencyKey,
        from: ExecutionState,
        to: ExecutionState,
        version: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), GatewayError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(mut entry) = self.cache.get(&idempotency_key.0).await {
                if entry.state == from && entry.context.version == version {
                    entry.state = to;
                    entry.execution.state = to;
                    entry.execution.updated_at = Utc::now();
                    self.cache.insert(idempotency_key.0.clone(), entry).await;
                }
            }
            Ok(())
        })
    }
}

// ── OssReconciler ────────────────────────────────────────────────────────────

pub struct OssReconciler;

impl Reconciler for OssReconciler {
    fn reconcile<'a>(
        &'a self,
        _operation: &'a ToolExecution,
    ) -> Pin<Box<dyn Future<Output = Result<ReconciliationResult, GatewayError>> + Send + 'a>> {
        Box::pin(async { Ok(ReconciliationResult::StillUnknown) })
    }
}

// ── OssToolTransport ──────────────────────────────────────────────────────────

pub struct OssToolTransport;

impl crate::domain::ports::ToolTransport for OssToolTransport {
    fn execute_tool<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        _arguments: &'a str,
        _tenant_id: &'a str,
        _enable_compression: bool,
        _test_scenario: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = crate::infrastructure::mcp_client::ToolResult> + Send + 'a>>
    {
        let cid = tool_call_id.to_string();
        let name = tool_name.to_string();
        Box::pin(async move {
            crate::infrastructure::mcp_client::ToolResult {
                tool_call_id: cid,
                name,
                content: "mock result".to_string(),
                latency_ms: 0,
                success: true,
            }
        })
    }
}
