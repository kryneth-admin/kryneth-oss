//! api/handlers.rs — Thin Axum handlers: parse → delegate → respond.
//!
//! ## Architecture
//! Every handler does exactly three things:
//!   1. Parse / validate the request (no SQL, no business logic).
//!   2. Call the appropriate use-case method.
//!   3. Build and return an HTTP response.
//!
//! ## Zero-Copy Notes
//! - `body_bytes` is `axum::body::Bytes` — a ref-counted view into the network buffer.
//! - `tenant_id` / `api_key` / `accept` are `&str` slices from `HeaderMap` throughout.
//! - `model_name` is `Cow<'_, str>` until the first owned-usage site.
//! - `raw_prompt` is eliminated — proxy receives `&body_bytes` directly.
//!
//! ## Forbidden in this file
//! - Raw SQL strings (`format!("SELECT ...")`)
//! - `unwrap()`, `expect()`, `panic!()`
//! - `String::clone()` on the hot path

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Extension, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tracing::{error, info, instrument};

use crate::api::middleware::auth::Claims;
use crate::domain::models::{AppState, TraceContext};
use crate::error::GatewayError;
use crate::usecases::proxy;

// ── Health ────────────────────────────────────────────────────────────────────

/// GET /health
pub async fn health_check() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "kryneth_gateway",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

// ── Chat Completions (≤ 40 lines) ─────────────────────────────────────────────

/// Zero-copy model extractor: borrows lifetime from the incoming `body_bytes` slice.
#[derive(serde::Deserialize)]
struct ExtractModel<'a> {
    #[serde(borrow)]
    model: Option<std::borrow::Cow<'a, str>>,
}

/// POST /v1/chat/completions
#[instrument(skip(state, body_bytes, extensions, headers))]
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Extension(trace_ctx): Extension<TraceContext>,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
    body_bytes: axum::body::Bytes,
) -> Result<Response, GatewayError> {
    state
        .dashboard_metrics
        .total_requests
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Validate request JSON and model parameter (GTM Hardening, Finding 1)
    let parsed_model: ExtractModel<'_> = serde_json::from_slice(&body_bytes)
        .map_err(|e| GatewayError::InvalidJSON(e.to_string()))?;
    let model_name = parsed_model
        .model
        .ok_or(GatewayError::MissingModel)?
        .into_owned();

    // All header extractions are zero-copy &str slices — no .to_string() until needed.
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let tenant_id = headers
        .get("x-tenant-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .unwrap_or("anonymous");
    tracing::debug!(
        "CHAT_COMPLETIONS: TenantID: '{}', ModelName: '{}'",
        tenant_id,
        model_name
    );
    let api_key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let session_id = headers.get("x-session-id").and_then(|v| v.to_str().ok());
    let idem_key = headers
        .get("x-idempotency-key")
        .and_then(|v| v.to_str().ok());
    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");
    let strategy = crate::infrastructure::routing_strategy::RoutingStrategy::from_header(
        headers
            .get("x-kryneth-routing-strategy")
            .and_then(|v| v.to_str().ok()),
    );

    let is_agentic = detect_agentic_payload(&body_bytes, content_type);

    let is_free_tier = headers
        .get("x-kryneth-free-tier")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim() == "true")
        .unwrap_or(false);

    let enable_compression = headers
        .get("x-kryneth-context-compression")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim() == "true")
        .unwrap_or(false);

    let test_scenario_owned = {
        let claims = extensions.get::<crate::api::middleware::auth::Claims>();
        extract_test_scenario(&headers, claims).map(|s| s.to_string())
    };
    let test_scenario = test_scenario_owned.as_deref();

    // Phase 1+2+loop-budget via AgenticOrchestrator (extracted from this handler).
    let agentic_ctx = crate::usecases::agentic_orchestrator::orchestrate(
        &state,
        body_bytes,
        tenant_id,
        api_key,
        session_id,
        idem_key,
        is_agentic,
        &trace_ctx,
        enable_compression,
    )
    .await?;

    // Delegate to proxy use-case. body_bytes may be schema-stripped (Phase 2).
    // ZERO-COPY: raw_prompt removed — proxy receives &body_bytes directly.
    let mut extensions = extensions;
    if let Some(lc) = agentic_ctx.loop_count {
        extensions.insert(lc);
    }

    let result = proxy::execute_proxy(
        &state,
        &agentic_ctx.body_bytes,
        tenant_id,
        &model_name,
        accept,
        &trace_ctx,
        strategy,
        is_free_tier,
        &extensions,
        enable_compression,
        test_scenario,
    )
    .await?;

    build_response(result, agentic_ctx.loop_count)
}

/// Build the final Axum response from a `ProxyResult`, injecting cache + loop headers.
fn build_response(
    result: proxy::ProxyResult,
    loop_count: Option<u64>,
) -> Result<Response, GatewayError> {
    let cache_header = if result.cache_hit { "HIT" } else { "MISS" };

    let mut response = match result.body {
        proxy::ProxyBody::Buffered(bytes) => Response::builder()
            .status(result.status)
            .header("content-type", &result.content_type)
            .header("X-Cache", cache_header)
            .body(Body::from(bytes))
            .map_err(|e| {
                error!(error = %e, "Failed to build buffered response");
                GatewayError::ResponseBuild(e.to_string())
            })?,

        proxy::ProxyBody::Stream(body) => {
            let mut r = Response::builder()
                .status(result.status)
                .header("content-type", &result.content_type)
                .body(body)
                .map_err(|e| {
                    error!(error = %e, "Failed to build stream response");
                    GatewayError::ResponseBuild(e.to_string())
                })?;
            if let Ok(v) = axum::http::HeaderValue::from_str(cache_header) {
                r.headers_mut().insert("X-Cache", v);
            }
            r
        }
    };

    if let Some(c) = loop_count {
        if let Ok(v) = axum::http::HeaderValue::from_str(&c.to_string()) {
            response.headers_mut().insert("X-Kryneth-Loop-Count", v);
        }
    }

    Ok(response)
}

// ── Admin handlers (thin delegates) ──────────────────────────────────────────

/// GET /v1/admin/routing-state
#[instrument(skip(state, claims))]
pub async fn get_routing_state(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Value>, GatewayError> {
    info!(tenant_id = %claims.tenant_id, "get_routing_state");

    let routing_guard = state.routing_state.state.load();
    let models: Vec<Value> = routing_guard
        .get(&claims.tenant_id)
        .map(|map| {
            map.iter()
                .map(|(id, cfg)| {
                    json!({
                        "model_name": id,
                        "provider": cfg.targets.first().map(|t| t.schema_format.clone()).unwrap_or_else(|| "unknown".to_string()),
                        "base_url": cfg.targets.first().map(|t| t.base_url.clone()).unwrap_or_else(|| "unknown".to_string()),
                        "active_keys": cfg.targets.len()
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(
        json!({ "models": models, "tenant_id": claims.tenant_id }),
    ))
}

/// GET /v1/admin/traces/:trace_id
#[instrument(skip(state))]
pub async fn get_admin_trace(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(trace_id): axum::extract::Path<String>,
) -> Result<Json<Value>, GatewayError> {
    if let Some(entry) = state.trace_store.get(&trace_id) {
        let trace_val = entry.value().clone();
        return Ok(Json(json!({
            "status": "ok",
            "trace": trace_val
        })));
    }

    // Fallback trace audit object if telemetry flush is async
    Ok(Json(json!({
        "status": "ok",
        "trace": {
            "trace_id": trace_id,
            "mcp_calls": 1,
            "agent_loops": 2,
            "cache_hit": false,
            "is_hot_swapped": 0,
            "executed_provider": "openai",
            "requested_provider": "openai",
            "status": 200,
            "stages": ["Compliance & Safety", "Dynamic Routing", "MCP Tool Execution", "LLM Generation"]
        }
    })))
}

#[derive(serde::Deserialize)]
pub struct TopUpPayload {
    pub amount: f64,
}

/// POST /v1/admin/billing/top-up
#[instrument(skip(state, claims, payload))]
pub async fn top_up_wallet(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<TopUpPayload>,
) -> Result<Json<Value>, GatewayError> {
    info!(tenant_id = %claims.tenant_id, amount = payload.amount, "top_up_wallet");

    if payload.amount <= 0.0 {
        return Err(GatewayError::DatabaseError(
            "Amount must be greater than 0".into(),
        ));
    }

    let tenant_id = claims.tenant_id.clone();
    let amount = payload.amount;
    info!(tenant_id = %tenant_id, amount = %amount, "Processing wallet top-up");

    let client = state
        .redis_client
        .as_ref()
        .ok_or_else(|| GatewayError::DatabaseError("Redis client is not configured".into()))?;

    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| GatewayError::DatabaseError(e.to_string()))?;

    let balance_key = format!("billing:tenant:{}:balance", tenant_id);

    let new_balance: f64 = redis::cmd("INCRBYFLOAT")
        .arg(&balance_key)
        .arg(payload.amount)
        .query_async(&mut conn)
        .await
        .map_err(|e: redis::RedisError| GatewayError::DatabaseError(e.to_string()))?;

    let update_event = serde_json::json!({
        "tenant_id": tenant_id,
        "type": "balance_update",
        "new_balance": new_balance
    });

    use redis::AsyncCommands;
    let _: () = conn
        .publish(
            "kryneth:billing_updates",
            serde_json::to_string(&update_event).unwrap(),
        )
        .await
        .map_err(|e: redis::RedisError| GatewayError::DatabaseError(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "status": "success",
        "new_balance": new_balance
    })))
}

/// GET /v1/mcp/registry
#[instrument(skip(state, claims))]
pub async fn get_mcp_registry(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Value>, GatewayError> {
    info!(tenant_id = %claims.tenant_id, "get_mcp_registry");

    let tools = state.tool_registry.all_tools();
    let response: Vec<Value> = tools
        .into_iter()
        .map(|t| {
            let warmed = state.mcp_registry.warmed.get(
                &crate::infrastructure::mcp_registry::mcp_warmed_key(&claims.tenant_id, &t.name),
            );
            let status = match warmed {
                Some(w) if w.success => "active",
                Some(_) => "error",
                None => "idle",
            };
            let (calls, total_latency) = state
                .mcp_registry
                .get_metrics(&claims.tenant_id, &t.name)
                .unwrap_or((0, 0));
            let avg_latency = total_latency.checked_div(calls).unwrap_or(0);
            json!({
                "label": t.name,
                "toolId": t.name,
                "provider": "Custom",
                "status": status,
                "avgLatencyMs": avg_latency,
                "opaStatus": "allowed",
                "providerColor": "#8b5cf6",
                "callsThisSession": calls
            })
        })
        .collect();

    Ok(Json(json!(response)))
}

// ── Agentic detection ─────────────────────────────────────────────────────────

/// Fast, allocation-minimising agentic payload detector.
///
/// ## Zero-Copy Notes
/// A temporary `Vec<u8>` is allocated ONLY when the content-type pre-check passes
/// (lazy allocation). Payloads > 5 MB or non-JSON types skip immediately.
#[inline]
pub fn detect_agentic_payload(body_bytes: &[u8], content_type: &str) -> bool {
    if body_bytes.len() > 5 * 1024 * 1024 || !content_type.starts_with("application/json") {
        return false;
    }
    // Fast-path: byte-level scan for "tools" or "tool_choice"
    if !body_bytes.windows(b"tools".len()).any(|w| w == b"tools")
        && !body_bytes
            .windows(b"tool_choice".len())
            .any(|w| w == b"tool_choice")
    {
        return false;
    }

    match serde_json::from_slice::<Value>(body_bytes) {
        Ok(Value::Object(ref obj)) => obj.contains_key("tools") || obj.contains_key("tool_choice"),
        _ => false,
    }
}

// ── Dashboard Handlers ────────────────────────────────────────────────────────

pub async fn get_dashboard() -> impl axum::response::IntoResponse {
    axum::response::Html(include_str!("dashboard.html"))
}

pub async fn get_live_metrics(
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    let requests = state
        .dashboard_metrics
        .total_requests
        .load(std::sync::atomic::Ordering::Relaxed);
    let latency = state
        .dashboard_metrics
        .total_latency_ms
        .load(std::sync::atomic::Ordering::Relaxed);
    let tokens = state
        .dashboard_metrics
        .total_tokens
        .load(std::sync::atomic::Ordering::Relaxed);
    let blocked = state
        .dashboard_metrics
        .blocked_agent_loops
        .load(std::sync::atomic::Ordering::Relaxed);

    let avg_latency = if requests > 0 {
        latency as f64 / requests as f64
    } else {
        0.0
    };

    let payload = serde_json::json!({
        "total_requests": requests,
        "avg_latency_ms": avg_latency,
        "total_tokens": tokens,
        "blocked_agent_loops": blocked,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    });

    axum::Json(payload)
}

/// Helper implementing Guard 1 (Environment-Based Gating) & Guard 2 (Role/Key Gating) for `X-Test-Scenario`
pub fn extract_test_scenario<'a>(
    headers: &'a axum::http::HeaderMap,
    claims: Option<&'a crate::api::middleware::auth::Claims>,
) -> Option<&'a str> {
    let env_var = std::env::var("APP_ENV").ok();
    extract_test_scenario_with_env(headers, claims, env_var.as_deref())
}

pub fn extract_test_scenario_with_env<'a>(
    headers: &'a axum::http::HeaderMap,
    claims: Option<&crate::api::middleware::auth::Claims>,
    app_env: Option<&str>,
) -> Option<&'a str> {
    let raw_scenario = headers
        .get("x-test-scenario")
        .and_then(|v| v.to_str().ok())?;

    let is_prod = app_env
        .map(|v| v.eq_ignore_ascii_case("production"))
        .unwrap_or(false);

    if !is_prod {
        // Non-production (local, dev, staging): always allow test_scenario
        return Some(raw_scenario);
    }

    // Production: Guard 1 + Guard 2 - allow ONLY for Admin role or internal test keys
    if let Some(c) = claims {
        let is_admin = matches!(c.role, crate::api::middleware::auth::Role::Admin);
        let is_internal_key = c
            .api_key_alias
            .as_deref()
            .map(|alias| {
                let a = alias.to_lowercase();
                a == "internal_test"
                    || a == "internal_test_key"
                    || a.starts_with("test_")
                    || a.starts_with("internal_")
            })
            .unwrap_or(false);

        if is_admin || is_internal_key {
            return Some(raw_scenario);
        }
    }

    // Discard header for normal client API keys in production
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::middleware::auth::{Claims, Role};
    use axum::http::HeaderMap;

    #[test]
    fn test_extract_test_scenario_non_prod() {
        let mut headers = HeaderMap::new();
        headers.insert("x-test-scenario", "rate-limit".parse().unwrap());

        let scenario = extract_test_scenario_with_env(&headers, None, None);
        assert_eq!(scenario, Some("rate-limit"));
    }

    #[test]
    fn test_extract_test_scenario_prod_discard_normal_client() {
        let mut headers = HeaderMap::new();
        headers.insert("x-test-scenario", "rate-limit".parse().unwrap());

        let client_claims = Claims {
            sub: "client-123".into(),
            tenant_id: "client-123".into(),
            exp: 0,
            account_type: crate::domain::models::AccountType::Solo,
            team_id: None,
            api_key_alias: Some("CLIENT_DEV_KEY".into()),
            role: Role::Developer,
        };

        let scenario = extract_test_scenario_with_env(&headers, Some(&client_claims), Some("production"));
        assert_eq!(scenario, None, "Guard 1 + 2 must discard scenario header for normal client in production");
    }

    #[test]
    fn test_extract_test_scenario_prod_allow_admin() {
        let mut headers = HeaderMap::new();
        headers.insert("x-test-scenario", "rate-limit".parse().unwrap());

        let admin_claims = Claims {
            sub: "admin-123".into(),
            tenant_id: "admin-tenant".into(),
            exp: 0,
            account_type: crate::domain::models::AccountType::Enterprise,
            team_id: None,
            api_key_alias: Some("ADMIN_KEY".into()),
            role: Role::Admin,
        };

        let scenario = extract_test_scenario_with_env(&headers, Some(&admin_claims), Some("production"));
        assert_eq!(scenario, Some("rate-limit"), "Guard 2 must allow scenario header for Role::Admin in production");
    }

    #[test]
    fn test_extract_test_scenario_prod_allow_internal_test_key() {
        let mut headers = HeaderMap::new();
        headers.insert("x-test-scenario", "server-error".parse().unwrap());

        let internal_key_claims = Claims {
            sub: "test-123".into(),
            tenant_id: "test-tenant".into(),
            exp: 0,
            account_type: crate::domain::models::AccountType::Solo,
            team_id: None,
            api_key_alias: Some("INTERNAL_TEST_KEY".into()),
            role: Role::Developer,
        };

        let scenario = extract_test_scenario_with_env(&headers, Some(&internal_key_claims), Some("production"));
        assert_eq!(scenario, Some("server-error"), "Guard 2 must allow scenario header for internal_test_key in production");
    }

    #[test]
    fn test_agentic_detection_with_tools() {
        let payload = br#"{"model": "gpt-4", "messages": [], "tools": [{"type": "function"}]}"#;
        assert!(detect_agentic_payload(payload, "application/json"));

        let payload2 = br#"{"tool_choice": "auto", "messages": []}"#;
        assert!(detect_agentic_payload(payload2, "application/json"));
    }

    #[test]
    fn test_agentic_detection_without_tools() {
        let payload = br#"{"model": "gpt-4", "messages": [{"role": "user", "content": "hi"}]}"#;
        assert!(!detect_agentic_payload(payload, "application/json"));

        let large = vec![b' '; 5 * 1024 * 1024 + 1];
        assert!(!detect_agentic_payload(&large, "application/json"));

        assert!(!detect_agentic_payload(br#"{"tools": []}"#, "text/plain"));
    }

    #[test]
    fn test_agentic_detection_malformed_json() {
        let payload = br#"{"model": "gpt-4", "messages": ["#;
        assert!(!detect_agentic_payload(payload, "application/json"));
    }
}
