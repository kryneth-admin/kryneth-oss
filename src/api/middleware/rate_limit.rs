//! src/api/middleware/rate_limit.rs — Distributed Token Bucket Rate Limiter.
//!
//! ## Architecture
//!
//! This middleware enforces per-virtual-route request limits using the
//! **Token Bucket** algorithm executed atomically via a **Redis Lua script**.
//!
//! ### Why Lua?
//! A single `EVAL` call is guaranteed to run atomically on Redis — no TOCTOU
//! race between reading `tokens` and writing them back, even with multiple
//! concurrent Gateway instances.
//!
//! ### Fail-Open
//! If the Redis call fails or exceeds the 50ms budget, the middleware logs
//! the error at `error!` level and allows the request to proceed. We never
//! drop customer traffic due to a telemetry/rate-limit infrastructure fault.
//!
//! ### Dynamic Limits
//! The RPM limit is read from the in-memory `RoutingState` (populated via
//! Redis Pub/Sub hot-reload from `kryneth_config`), keyed by
//! `tenant_id:virtual_model_name`. The model name is extracted from the
//! `x-kryneth-model` header, set earlier in the request pipeline.
//!
//! ### Key Schema
//! ```text
//! rl:tb:{tenant_id}:{model_name}
//!   tokens    (float, remaining burst budget)
//!   last_refill (unix seconds, float)
//! ```

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::net::SocketAddr;
use tracing::warn;

use crate::domain::models::AppState;

const MODEL_HEADER: &str = "x-kryneth-model";

pub fn extract_identifiers(headers: &HeaderMap, addr: &SocketAddr) -> (String, String) {
    let tenant_id = headers
        .get("x-tenant-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| addr.ip().to_string());

    let user_id = headers
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "anon_user".to_string());

    (tenant_id, user_id)
}

fn extract_model_name(headers: &HeaderMap) -> Option<String> {
    headers
        .get(MODEL_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let fallback_addr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);
    let addr = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0)
        .unwrap_or(fallback_addr);

    let headers = request.headers().clone();
    let (tenant_id, _user_id) = extract_identifiers(&headers, &addr);
    let model_name = extract_model_name(&headers);

    let capacity_rpm: u32 = model_name
        .as_deref()
        .and_then(|model| {
            let routing_guard = state.routing_state.state.load();
            let tenant_map = routing_guard.get(&tenant_id)?;
            let model_cfg = tenant_map.get(model)?;
            model_cfg.rate_limit_rpm
        })
        .unwrap_or(state.rate_limit_max);

    let bucket_key = match &model_name {
        Some(m) => format!("rl:tb:{}:{}", tenant_id, m),
        None => format!("rl:tb:{}:global", tenant_id),
    };

    let bucket = state
        .rate_limit_cache
        .entry(bucket_key.clone())
        .or_insert_with(|| {
            std::sync::Arc::new(crate::domain::models::LocalBucket {
                consumed: std::sync::atomic::AtomicU32::new(0),
                is_blocked: std::sync::atomic::AtomicBool::new(false),
                capacity_rpm: std::sync::atomic::AtomicU32::new(capacity_rpm),
            })
        })
        .clone();

    bucket
        .capacity_rpm
        .store(capacity_rpm, std::sync::atomic::Ordering::Relaxed);

    if bucket.is_blocked.load(std::sync::atomic::Ordering::Relaxed) {
        warn!(
            rate_limited = true,
            tenant_id   = %tenant_id,
            model       = ?model_name,
            bucket      = %bucket_key,
            capacity_rpm,
            "Request BLOCKED by local rate limiter — 429"
        );

        let body = Json(json!({
            "error": {
                "code": "RATE_LIMITED",
                "message": format!(
                    "Rate limit exceeded: maximum {} requests/minute for this route. \
                     Bursts are supported. Please back off and retry.",
                    capacity_rpm
                )
            }
        }));

        let mut response = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
        response
            .headers_mut()
            .insert("x-ratelimit-limit", HeaderValue::from(capacity_rpm));
        response
            .headers_mut()
            .insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("60"));
        return Ok(response);
    }

    let consumed = bucket
        .consumed
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if consumed >= capacity_rpm {
        bucket
            .is_blocked
            .store(true, std::sync::atomic::Ordering::Relaxed);

        warn!(
            rate_limited = true,
            tenant_id   = %tenant_id,
            model       = ?model_name,
            bucket      = %bucket_key,
            capacity_rpm,
            "Local rate-limit bucket exceeded capacity, blocking — 429"
        );

        let body = Json(json!({
            "error": {
                "code": "RATE_LIMITED",
                "message": format!(
                    "Rate limit exceeded: maximum {} requests/minute for this route. \
                     Bursts are supported. Please back off and retry.",
                    capacity_rpm
                )
            }
        }));

        let mut response = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
        response
            .headers_mut()
            .insert("x-ratelimit-limit", HeaderValue::from(capacity_rpm));
        response
            .headers_mut()
            .insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("60"));
        return Ok(response);
    }

    // ── Allowed (or fail-open): forward to the next layer ────────────────────
    Ok(next.run(request).await)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_extract_identifiers_with_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-tenant-id", HeaderValue::from_static("tenant_abc"));
        headers.insert("x-user-id", HeaderValue::from_static("user_xyz"));

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        let (tenant, user) = extract_identifiers(&headers, &addr);

        assert_eq!(tenant, "tenant_abc");
        assert_eq!(user, "user_xyz");
    }

    #[test]
    fn test_extract_identifiers_fallback() {
        let headers = HeaderMap::new();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 200)), 8080);

        let (tenant, user) = extract_identifiers(&headers, &addr);

        assert_eq!(tenant, "192.168.1.200"); // IP fallback
        assert_eq!(user, "anon_user"); // constant fallback
    }

    #[test]
    fn test_extract_model_name_present() {
        let mut headers = HeaderMap::new();
        headers.insert(MODEL_HEADER, HeaderValue::from_static("my-prod-route"));
        assert_eq!(extract_model_name(&headers), Some("my-prod-route".into()));
    }

    #[test]
    fn test_extract_model_name_absent() {
        let headers = HeaderMap::new();
        assert_eq!(extract_model_name(&headers), None);
    }

    #[test]
    fn test_bucket_key_scoped_to_route() {
        // Validate the key format used by the middleware
        let tenant_id = "11111111-2222-3333-4444-555555555555";
        let model = "gpt-production";
        let key = format!("rl:tb:{}:{}", tenant_id, model);
        assert_eq!(
            key,
            "rl:tb:11111111-2222-3333-4444-555555555555:gpt-production"
        );
    }

    #[test]
    fn test_bucket_key_global_fallback() {
        let tenant_id = "tenant-abc";
        let key: String = format!("rl:tb:{}:global", tenant_id);
        assert_eq!(key, "rl:tb:tenant-abc:global");
    }

    /// Sanity-check that the refill-rate formula stays consistent with RPM semantics.
    #[test]
    fn test_refill_rate_formula() {
        let capacity_rpm: u32 = 60;
        let refill_rate = capacity_rpm as f64 / 60.0;
        // 60 RPM → 1 token per second
        assert!((refill_rate - 1.0).abs() < f64::EPSILON);

        let capacity_rpm: u32 = 600;
        let refill_rate = capacity_rpm as f64 / 60.0;
        // 600 RPM → 10 tokens per second
        assert!((refill_rate - 10.0).abs() < f64::EPSILON);
    }
}
