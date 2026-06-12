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

pub struct OssTelemetry;

impl TelemetryPort for OssTelemetry {
    fn log_event(&self, event: serde_json::Value) {
        tracing::info!(telemetry_event = ?event, "OSS Telemetry Log Event");
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

    fn process_billing_telemetry<'a>(
        &'a self,
        _trace_id: &'a str,
        _tenant_id: &'a str,
        _provider: &'a str,
        _target_model: &'a str,
        _prompt_tokens: u64,
        _completion_tokens: u64,
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
    fn start_subscriber(&self, _routing_state: Arc<crate::domain::models::RoutingState>) {
        // OSS is static and loaded from file at boot time
    }
}

// ── OssSemanticCache ─────────────────────────────────────────────────────────

pub struct OssSemanticCache;

impl SemanticCachePort for OssSemanticCache {
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
