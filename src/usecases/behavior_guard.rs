//! usecases/behavior_guard.rs — Agent loop detection circuit breaker.
//!
//! Detects recursive AI agent loops by SHA-256 hashing the request body.
//! Under OSS mode, it enforces runaway tool storm detection and identical tool signature loops.

use std::sync::Arc;
use tracing::{instrument, warn};

use crate::domain::models::{AppState, GatewayError};
use simd_json::prelude::*;

/// Checks whether the current request body forms a recursive loop within
/// the given session (no-op for purified OSS core).
#[instrument(skip(_state, _body), fields(session_id = %session_id))]
pub async fn enforce_loop_detection(
    _state: &Arc<AppState>,
    session_id: &str,
    _body: &[u8],
) -> Result<(), GatewayError> {
    Ok(())
}

fn extract_tool_calls_from_request<'a>(
    parsed: &simd_json::BorrowedValue<'a>,
) -> Vec<simd_json::BorrowedValue<'a>> {
    let mut extracted = Vec::new();

    // 1. OpenAI / Anthropic messages array
    if let Some(messages) = parsed.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            // OpenAI tool_calls
            if let Some(tc) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                for call in tc {
                    extracted.push(call.clone());
                }
            }
            // Anthropic tool_use inside content array or object
            if let Some(content) = msg.get("content") {
                if let Some(arr) = content.as_array() {
                    for block in arr {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            extracted.push(block.clone());
                        }
                    }
                } else if content.is_object()
                    && content.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                {
                    extracted.push(content.clone());
                }
            }
        }
    }

    // 2. Gemini contents array
    if let Some(contents) = parsed.get("contents").and_then(|c| c.as_array()) {
        for content in contents {
            if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                for part in parts {
                    if part.get("functionCall").is_some() {
                        extracted.push(part.clone());
                    }
                }
            }
        }
    }

    // 3. Fallback: Check root tool_calls (for compatibility/direct/tests)
    if let Some(tc) = parsed.get("tool_calls").and_then(|t| t.as_array()) {
        for call in tc {
            extracted.push(call.clone());
        }
    }

    // 4. Fallback: Check choices (OpenAI response format, if used in tests or legacy)
    if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
        for choice in choices {
            if let Some(msg) = choice.get("message") {
                if let Some(tc) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                    for call in tc {
                        extracted.push(call.clone());
                    }
                }
            } else if let Some(delta) = choice.get("delta") {
                if let Some(tc) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                    for call in tc {
                        extracted.push(call.clone());
                    }
                }
            }
        }
    }

    extracted
}

/// OSS runaway agent guardian for tool storms and loop detection.
#[instrument(skip(state, body_bytes), fields(session_id = %session_id))]
pub async fn enforce_oss_agent_guardian(
    state: &Arc<AppState>,
    tenant_id: &str,
    session_id: &str,
    body_bytes: &[u8],
) -> Result<(), GatewayError> {
    // Clone body bytes since simd_json mutates the buffer in-place
    let mut buffer = body_bytes.to_vec();

    let parsed = match simd_json::to_borrowed_value(&mut buffer) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let tool_calls = extract_tool_calls_from_request(&parsed);
    if tool_calls.is_empty() {
        return Ok(());
    }

    enforce_oss_tool_calls(state, tenant_id, session_id, &tool_calls).await
}

struct UnifiedToolCall {
    pub name: String,
    pub semantic_hash: u64,
}

fn hash_borrowed_value(
    val: &simd_json::BorrowedValue<'_>,
    hasher: &mut ahash::AHasher,
    depth: usize,
) {
    use std::hash::Hasher;

    const MAX_DEPTH: usize = 16;
    if depth > MAX_DEPTH {
        let fallback_str = serde_json::to_string(val).unwrap_or_default();
        hasher.write(fallback_str.as_bytes());
        return;
    }

    #[allow(unreachable_patterns)]
    match val {
        simd_json::BorrowedValue::Static(node) => match node {
            simd_json::StaticNode::I64(n) => {
                hasher.write_u8(2);
                hasher.write_u64((*n as f64).to_bits());
            }
            simd_json::StaticNode::U64(n) => {
                hasher.write_u8(2);
                hasher.write_u64((*n as f64).to_bits());
            }
            simd_json::StaticNode::F64(n) => {
                hasher.write_u8(2);
                hasher.write_u64(n.to_bits());
            }
            simd_json::StaticNode::Bool(b) => {
                hasher.write_u8(1);
                hasher.write_u8(if *b { 1 } else { 0 });
            }
            simd_json::StaticNode::Null => {
                hasher.write_u8(0);
            }
        },
        simd_json::BorrowedValue::String(s) => {
            hasher.write_u8(5);
            hasher.write(s.as_bytes());
        }
        simd_json::BorrowedValue::Array(arr) => {
            hasher.write_u8(6);
            hasher.write_usize(arr.len());
            for item in arr {
                hash_borrowed_value(item, hasher, depth + 1);
            }
        }
        simd_json::BorrowedValue::Object(obj) => {
            hasher.write_u8(7);

            // Order-Independent Hashing (XOR Combination)
            let mut obj_hash = 0u64;
            for (k, v) in obj.iter() {
                let k_str: &str = k.as_ref();
                if !["timestamp", "nonce", "uuid", "stream", "session_id", "time"].contains(&k_str)
                {
                    let mut kv_hasher = ahash::AHasher::default();
                    kv_hasher.write(k_str.as_bytes());
                    hash_borrowed_value(v, &mut kv_hasher, depth + 1);
                    obj_hash ^= kv_hasher.finish();
                }
            }
            hasher.write_u64(obj_hash);
        }
        _ => {
            let fallback_str = serde_json::to_string(val).unwrap_or_default();
            hasher.write(fallback_str.as_bytes());
        }
    }
}

impl UnifiedToolCall {
    /// Unifies tool calls from top AI agent ecosystems into a standard format.
    fn from_borrowed_value(tc: &simd_json::BorrowedValue<'_>) -> Option<Self> {
        use simd_json::prelude::*;
        use std::hash::Hasher;

        if let Some(func) = tc.get("function") {
            // 1. OpenAI / Groq Format
            let name = func
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args_str = func.get("arguments").and_then(|a| a.as_str()).unwrap_or("");

            let arena = bumpalo::Bump::new();
            let mut_bytes = arena.alloc_slice_copy(args_str.as_bytes());
            let parsed_val = match simd_json::to_borrowed_value(mut_bytes) {
                Ok(val) => val,
                Err(_) => simd_json::BorrowedValue::Static(simd_json::StaticNode::Null),
            };

            let mut hasher = ahash::AHasher::default();
            hash_borrowed_value(&parsed_val, &mut hasher, 0);
            let semantic_hash = hasher.finish();

            Some(Self {
                name,
                semantic_hash,
            })
        } else if let Some(func_call) = tc.get("functionCall") {
            // 2. Google Gemini Format
            let name = func_call
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args = func_call.get("args");

            let mut hasher = ahash::AHasher::default();
            if let Some(val) = args {
                hash_borrowed_value(val, &mut hasher, 0);
            } else {
                hash_borrowed_value(
                    &simd_json::BorrowedValue::Static(simd_json::StaticNode::Null),
                    &mut hasher,
                    0,
                );
            }
            let semantic_hash = hasher.finish();

            Some(Self {
                name,
                semantic_hash,
            })
        } else if let Some(name_val) = tc.get("name") {
            // 3. Anthropic & Cohere Formats
            let name = name_val.as_str().unwrap_or("").to_string();
            let args = tc.get("input").or_else(|| tc.get("parameters"));

            let mut hasher = ahash::AHasher::default();
            if let Some(val) = args {
                hash_borrowed_value(val, &mut hasher, 0);
            } else {
                hash_borrowed_value(
                    &simd_json::BorrowedValue::Static(simd_json::StaticNode::Null),
                    &mut hasher,
                    0,
                );
            }
            let semantic_hash = hasher.finish();

            Some(Self {
                name,
                semantic_hash,
            })
        } else {
            None // Unsupported format
        }
    }
}

async fn enforce_oss_tool_calls(
    state: &Arc<AppState>,
    tenant_id: &str,
    session_id: &str,
    tool_calls: &[simd_json::BorrowedValue<'_>],
) -> Result<(), GatewayError> {
    use std::hash::Hasher;

    let mut max_session_tool_calls = 20;
    let mut max_identical_tool_calls = 5;

    if let Some(cfg) = state.routing_state.client_configs.load().get(tenant_id) {
        max_session_tool_calls = cfg.max_agent_loops as u32;
        max_identical_tool_calls = cfg.max_identical_tool_calls as u32;
    } else {
        if let Some(env_max) = std::env::var("MAX_SESSION_TOOL_CALLS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
        {
            max_session_tool_calls = env_max;
        }
        if let Some(env_ident) = std::env::var("MAX_IDENTICAL_TOOL_CALLS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
        {
            max_identical_tool_calls = env_ident;
        }
    }

    let mut storm_hasher = ahash::AHasher::default();
    storm_hasher.write(session_id.as_bytes());
    storm_hasher.write(b"_STORM");
    let storm_key = storm_hasher.finish().to_string();

    // Async await retained for Moka Cache future
    let mut storm_count = state
        .agent_guardian_cache
        .get(&storm_key)
        .await
        .unwrap_or(0);
    storm_count += tool_calls.len() as u32;

    if storm_count > max_session_tool_calls {
        tracing::warn!(session_id = %session_id, storm_count, "Tool storm detected");
        state
            .dashboard_metrics
            .blocked_agent_loops
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Err(GatewayError::AgentToolStorm(
            "Too many distinct tool calls in this session. Request blocked.".into(),
        ));
    }
    state
        .agent_guardian_cache
        .insert(storm_key, storm_count)
        .await;

    // Process using the Unified Abstraction
    for tc in tool_calls {
        let unified_tool = match UnifiedToolCall::from_borrowed_value(tc) {
            Some(tool) => tool,
            None => continue,
        };

        let mut loop_hasher = ahash::AHasher::default();
        loop_hasher.write(session_id.as_bytes());
        loop_hasher.write(unified_tool.name.as_bytes());
        loop_hasher.write_u64(unified_tool.semantic_hash);
        let loop_key = loop_hasher.finish().to_string();

        let count = state.agent_guardian_cache.get(&loop_key).await.unwrap_or(0) + 1;

        if count > max_identical_tool_calls {
            tracing::warn!(session_id = %session_id, tool_name = %unified_tool.name, "Runaway loop detected");
            state
                .dashboard_metrics
                .blocked_agent_loops
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(GatewayError::AgentRunawayLoop(
                "Agent infinite loop detected for identical tool signature. Request blocked."
                    .into(),
            ));
        }
        state.agent_guardian_cache.insert(loop_key, count).await;
    }

    Ok(())
}

/// Enforces the agentic loop budget (no-op for purified OSS core, always returns Ok(0)).
#[instrument(skip(_state), fields(session_id = %session_id))]
pub async fn enforce_agentic_loop_budget(
    _state: &Arc<AppState>,
    session_id: &str,
) -> Result<u64, GatewayError> {
    Ok(0)
}

/// Pre-LLM guard (no-op for purified OSS core).
#[instrument(skip(_state), fields(session_id = %session_id))]
pub async fn enforce_burn_rate(
    _state: &Arc<AppState>,
    session_id: &str,
) -> Result<(), GatewayError> {
    Ok(())
}

/// Post-LLM spend recorder (no-op for purified OSS core).
#[instrument(skip(_state), fields(session_id = %session_id, cost))]
pub async fn record_session_spend(_state: &Arc<AppState>, session_id: &str, _cost: f64) {}

// ── Tunnel 3 Phase 3: MCP Sandbox Firewall (Fail-Closed) ────────────────────

/// Returns `true` when the sandbox should fail-open (legacy / non-production).
///
/// Reads `SANDBOX_FALLBACK_MODE` env var once per request.
/// Default (unset) = fail-closed: OPA downtime **blocks** execution.
/// Set `SANDBOX_FALLBACK_MODE=open` to revert to fail-open behaviour.
#[inline]
fn sandbox_is_fail_open() -> bool {
    // In OSS environments, if the compliance service is not expected to run,
    // we default to fail-open to allow agent tool calls to proceed without 503 timeouts.
    std::env::var("SANDBOX_FALLBACK_MODE")
        .map(|v| v.eq_ignore_ascii_case("open") || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Compact deny payload — single JSON object, minimal token cost.
const DENY_CONTENT: &str = r#"{"error":"Kryneth Guard: Policy Denied. Use read-only tools."}"#;

/// Inspects a buffered LLM response body for `tool_calls` and enforces
/// the MCP sandbox policy via an OPA RBAC check.
pub async fn enforce_mcp_sandbox(
    state: &Arc<AppState>,
    tenant_id: &str,
    response_body: Vec<u8>,
) -> Result<Vec<u8>, GatewayError> {
    // ── Fast-path: simd-json scan for tool_calls ─────────────────────────────
    let has_tool_calls = response_body
        .windows(b"tool_calls".len())
        .any(|w| w == b"tool_calls");

    if !has_tool_calls {
        return Ok(response_body);
    }

    // ── Parse full body to extract tool_calls ────────────────────────────────
    let mut body_val: serde_json::Value = match serde_json::from_slice(&response_body) {
        Ok(v) => v,
        Err(_) => return Ok(response_body),
    };

    let tool_calls = body_val
        .pointer("/choices/0/message/tool_calls")
        .and_then(|v| v.as_array());

    let tool_names: Vec<String> = if let Some(arr) = tool_calls {
        let mut names = Vec::new();
        for tc in arr {
            if let Some(name) = tc.pointer("/function/name").and_then(|n| n.as_str()) {
                names.push(name.to_string());
            }
        }
        if names.is_empty() {
            vec!["unknown_tool".to_string()]
        } else {
            names
        }
    } else {
        vec!["unknown_tool".to_string()]
    };

    // ── Non-blocking OPA RBAC check ──────────────────────────────────────────
    let fail_open = sandbox_is_fail_open();
    let mut check_futures = Vec::new();

    for tool_name in &tool_names {
        let opa_url = format!(
            "{}/v1/data/kryneth/mcp/allow",
            state.compliance_url.trim_end_matches('/')
        );
        let opa_payload = serde_json::json!({
            "input": {
                "tenant_id": tenant_id,
                "tool_name": tool_name.clone(),
                "action": "execute"
            }
        });
        let client = state.http_client.clone();
        let tool_name_clone = tool_name.clone();

        check_futures.push(async move {
            let res = client
                .post(&opa_url)
                .timeout(std::time::Duration::from_millis(200))
                .json(&opa_payload)
                .send()
                .await;
            (tool_name_clone, res)
        });
    }

    let results = futures::future::join_all(check_futures).await;
    let mut denied = false;
    let mut denied_tool = None;

    for (tool_name, opa_result) in results {
        let tool_denied = match opa_result {
            Ok(resp) if resp.status().is_success() => {
                let allow: bool = resp
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|v| v["result"].as_bool())
                    .unwrap_or(fail_open);
                !allow
            }
            Ok(resp) => {
                warn!(
                    status = resp.status().as_u16(),
                    tool_name = %tool_name,
                    fail_open,
                    "MCP sandbox: OPA returned non-2xx — applying fallback mode"
                );
                !fail_open
            }
            Err(e) => {
                warn!(
                    error = %e,
                    tool_name = %tool_name,
                    fail_open,
                    "MCP sandbox: OPA unreachable — applying fallback mode"
                );
                if !fail_open && e.is_timeout() {
                    return Err(GatewayError::SecurityTimeout(
                        "OPA Sandbox Validation timed out".into(),
                    ));
                }
                !fail_open
            }
        };

        if tool_denied {
            denied = true;
            denied_tool = Some(tool_name);
            break;
        }
    }

    if !denied {
        return Ok(response_body);
    }

    // ── Compact Soft-Steering Mutation ────────────────────────────────────────
    let logged_tool = denied_tool.unwrap_or_else(|| "unknown_tool".to_string());
    tracing::warn!(
        tenant_id = %tenant_id,
        tool_name = %logged_tool,
        "Tunnel 3 Phase 3 — OPA denied; injecting compact soft-steer response"
    );

    if let Some(choices) = body_val.get_mut("choices").and_then(|c| c.as_array_mut()) {
        if let Some(first_choice) = choices.first_mut() {
            if let Some(message) = first_choice.get_mut("message") {
                *message = serde_json::json!({
                    "role": "tool",
                    "content": DENY_CONTENT
                });
            }
            if let Some(obj) = first_choice.as_object_mut() {
                obj.remove("tool_calls");
                obj.insert("finish_reason".to_string(), serde_json::json!("stop"));
            }
        }
    }

    Ok(serde_json::to_vec(&body_val).unwrap_or(response_body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::RoutingState;
    use crate::infrastructure::l1_cache::L1Cache;
    use std::sync::Arc;

    #[test]
    fn test_deny_content_is_compact_json() {
        let parsed: serde_json::Value =
            serde_json::from_str(DENY_CONTENT).expect("DENY_CONTENT must be valid JSON");
        assert_eq!(
            parsed["error"].as_str(),
            Some("Kryneth Guard: Policy Denied. Use read-only tools."),
            "DENY_CONTENT error message mismatch"
        );
        assert!(
            DENY_CONTENT.len() < 80,
            "DENY_CONTENT must be compact (< 80 bytes), got {}",
            DENY_CONTENT.len()
        );
    }

    #[test]
    fn test_sandbox_fail_open_env_flag() {
        std::env::remove_var("SANDBOX_FALLBACK_MODE");
        assert!(
            !sandbox_is_fail_open(),
            "Default must be fail-closed in OSS mode"
        );

        std::env::set_var("SANDBOX_FALLBACK_MODE", "open");
        assert!(
            sandbox_is_fail_open(),
            "SANDBOX_FALLBACK_MODE=open must be fail-open"
        );

        std::env::set_var("SANDBOX_FALLBACK_MODE", "OPEN");
        assert!(
            sandbox_is_fail_open(),
            "SANDBOX_FALLBACK_MODE=OPEN must be fail-open"
        );

        std::env::set_var("SANDBOX_FALLBACK_MODE", "true");
        assert!(
            sandbox_is_fail_open(),
            "SANDBOX_FALLBACK_MODE=true must be fail-open"
        );

        std::env::set_var("SANDBOX_FALLBACK_MODE", "closed");
        assert!(
            !sandbox_is_fail_open(),
            "SANDBOX_FALLBACK_MODE=closed must be fail-closed"
        );

        std::env::remove_var("SANDBOX_FALLBACK_MODE");
    }

    #[test]
    fn test_soft_steer_content_is_compact_deny() {
        let message = serde_json::json!({
            "role": "tool",
            "content": DENY_CONTENT
        });
        assert_eq!(message["role"].as_str(), Some("tool"));
        let content = message["content"].as_str().unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(content).expect("Soft-steer content must be valid JSON");
        assert!(
            parsed.get("error").is_some(),
            "Soft-steer content must contain 'error' key"
        );
    }

    #[tokio::test]
    async fn test_oss_agent_guardian() {
        let agent_guardian_cache = moka::future::Cache::builder()
            .max_capacity(10 * 1024 * 1024)
            .time_to_live(std::time::Duration::from_secs(1))
            .build();

        let state = Arc::new(AppState {
            http_client: reqwest::Client::new(),
            compliance_url: String::new(),
            rate_limit_max: 60,
            rate_limit_window: 60,
            dashboard_url: String::new(),
            llm_api_base_url: None,
            redis_client: None,
            telemetry: Arc::new(crate::infrastructure::oss_adapters::OssTelemetry::new(
                Arc::new(dashmap::DashMap::new()),
            )),
            billing: Arc::new(crate::infrastructure::oss_adapters::OssBilling),
            auth_resolver: Arc::new(crate::infrastructure::oss_adapters::OssAuth),
            rate_limiter: Arc::new(crate::infrastructure::oss_adapters::OssRateLimit),
            routing_config: Arc::new(crate::infrastructure::oss_adapters::OssRoutingConfig),
            semantic_cache: Arc::new(crate::infrastructure::oss_adapters::OssSemanticCache),
            execution_store: Arc::new(
                crate::infrastructure::oss_adapters::MokaExecutionStore::new(
                    moka::future::Cache::builder().build(),
                ),
            ),
            reconciler: Arc::new(crate::infrastructure::oss_adapters::OssReconciler),
            tool_transport: Arc::new(crate::infrastructure::oss_adapters::OssToolTransport),
            rate_limit_cache: Arc::new(dashmap::DashMap::new()),
            l1_cache: Arc::new(L1Cache::new(1024).unwrap()),
            routing_state: Arc::new(RoutingState::new()),
            circuit_breaker: moka::future::Cache::builder().build(),
            loop_fallback_cache: moka::future::Cache::builder().build(),
            mcp_registry: crate::infrastructure::mcp_registry::McpConnectionRegistry::empty(),
            tool_registry: crate::usecases::tool_router::ToolRegistry::empty(),
            agent_guardian_cache,
            operation_cache: moka::future::Cache::builder().build(),
            dashboard_metrics: Arc::new(crate::domain::models::DashboardMetrics::new()),
            pricing_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
            trace_store: Arc::new(dashmap::DashMap::new()),
            budget_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
        });

        let session_id = "test-session-oss";

        // Test 1 (Valid Progression)
        for i in 1..=3 {
            let body = serde_json::json!({
                "messages": [{
                    "role": "assistant",
                    "tool_calls": [{
                        "function": {
                            "name": "search",
                            "arguments": format!("{{\"query\": \"A{}\"}}", i)
                        }
                    }]
                }]
            });
            let bytes = serde_json::to_vec(&body).unwrap();
            let res = enforce_oss_agent_guardian(&state, "test-tenant", session_id, &bytes).await;
            assert!(res.is_ok(), "Valid progression failed on iteration {}", i);
        }

        // Test 2 (Runaway Loop)
        let loop_body = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "tool_calls": [{
                    "function": {
                        "name": "fetch",
                        "arguments": "{\"id\": 1}"
                    }
                }]
            }]
        });
        let loop_bytes = serde_json::to_vec(&loop_body).unwrap();

        for _ in 1..=5 {
            let res =
                enforce_oss_agent_guardian(&state, "test-tenant", session_id, &loop_bytes).await;
            assert!(res.is_ok());
        }
        let res = enforce_oss_agent_guardian(&state, "test-tenant", session_id, &loop_bytes).await;
        assert!(matches!(res, Err(GatewayError::AgentRunawayLoop(_))));

        // Test 3 (Tool Storm)
        let session_id_storm = "test-session-storm";
        let mut tools = Vec::new();
        for i in 1..=21 {
            tools.push(serde_json::json!({
                "function": {
                    "name": format!("tool_{}", i),
                    "arguments": "{}"
                }
            }));
        }
        let storm_body = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "tool_calls": tools
            }]
        });
        let storm_bytes = serde_json::to_vec(&storm_body).unwrap();
        let res =
            enforce_oss_agent_guardian(&state, "test-tenant", session_id_storm, &storm_bytes).await;
        assert!(matches!(res, Err(GatewayError::AgentToolStorm(_))));

        // Test 4 (Time Window Decay)
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let res = enforce_oss_agent_guardian(&state, "test-tenant", session_id, &loop_bytes).await;
        assert!(res.is_ok(), "Cache did not expire");
    }

    #[test]
    fn test_ephemeral_hash_evasion() {
        let payload_base = serde_json::json!({
            "function": {
                "name": "get_weather",
                "arguments": "{\"location\": \"Paris\", \"timestamp\": \"2026-07-16T17:31:00Z\", \"nonce\": \"12345\"}"
            }
        });

        let payload_mutated = serde_json::json!({
            "function": {
                "name": "get_weather",
                "arguments": "{\"timestamp\": \"2026-07-16T17:35:00Z\", \"location\": \"Paris\", \"nonce\": \"67890\"}"
            }
        });

        let mut bytes_base = serde_json::to_vec(&payload_base).unwrap();
        let parsed_base = simd_json::to_borrowed_value(&mut bytes_base).unwrap();
        let tc_base = UnifiedToolCall::from_borrowed_value(&parsed_base).unwrap();

        let mut bytes_mutated = serde_json::to_vec(&payload_mutated).unwrap();
        let parsed_mutated = simd_json::to_borrowed_value(&mut bytes_mutated).unwrap();
        let tc_mutated = UnifiedToolCall::from_borrowed_value(&parsed_mutated).unwrap();

        assert_eq!(
            tc_base.semantic_hash, tc_mutated.semantic_hash,
            "Semantic hash must be identical when only ephemeral keys and ordering change"
        );

        // Mutating a stable key should yield a different hash
        let payload_different = serde_json::json!({
            "function": {
                "name": "get_weather",
                "arguments": "{\"location\": \"Berlin\", \"timestamp\": \"2026-07-16T17:31:00Z\", \"nonce\": \"12345\"}"
            }
        });
        let mut bytes_diff = serde_json::to_vec(&payload_different).unwrap();
        let parsed_diff = simd_json::to_borrowed_value(&mut bytes_diff).unwrap();
        let tc_diff = UnifiedToolCall::from_borrowed_value(&parsed_diff).unwrap();
        assert_ne!(
            tc_base.semantic_hash, tc_diff.semantic_hash,
            "Semantic hash must change when a stable key value changes"
        );

        // Numeric coercion verification (e.g. integer vs float)
        let payload_int = serde_json::json!({
            "function": {
                "name": "set_threshold",
                "arguments": "{\"value\": 5}"
            }
        });
        let payload_float = serde_json::json!({
            "function": {
                "name": "set_threshold",
                "arguments": "{\"value\": 5.0}"
            }
        });
        let mut bytes_int = serde_json::to_vec(&payload_int).unwrap();
        let parsed_int = simd_json::to_borrowed_value(&mut bytes_int).unwrap();
        let tc_int = UnifiedToolCall::from_borrowed_value(&parsed_int).unwrap();

        let mut bytes_float = serde_json::to_vec(&payload_float).unwrap();
        let parsed_float = simd_json::to_borrowed_value(&mut bytes_float).unwrap();
        let tc_float = UnifiedToolCall::from_borrowed_value(&parsed_float).unwrap();

        assert_eq!(
            tc_int.semantic_hash, tc_float.semantic_hash,
            "Numeric values (5 vs 5.0) must coerce to equivalent hashes"
        );

        // Confirm "id" is NOT in the ephemeral blacklist
        let payload_id1 = serde_json::json!({
            "function": {
                "name": "fetch",
                "arguments": "{\"id\": 1}"
            }
        });
        let payload_id2 = serde_json::json!({
            "function": {
                "name": "fetch",
                "arguments": "{\"id\": 2}"
            }
        });
        let mut bytes_id1 = serde_json::to_vec(&payload_id1).unwrap();
        let parsed_id1 = simd_json::to_borrowed_value(&mut bytes_id1).unwrap();
        let tc_id1 = UnifiedToolCall::from_borrowed_value(&parsed_id1).unwrap();

        let mut bytes_id2 = serde_json::to_vec(&payload_id2).unwrap();
        let parsed_id2 = simd_json::to_borrowed_value(&mut bytes_id2).unwrap();
        let tc_id2 = UnifiedToolCall::from_borrowed_value(&parsed_id2).unwrap();

        assert_ne!(
            tc_id1.semantic_hash, tc_id2.semantic_hash,
            "The 'id' field must not be stripped from hash payload"
        );
    }

    #[tokio::test]
    async fn test_extract_tool_calls_from_request_formats() {
        // OpenAI Format
        let openai_body = serde_json::json!({
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "openai_tool",
                                "arguments": "{}"
                            }
                        }
                    ]
                }
            ]
        });
        let mut bytes = serde_json::to_vec(&openai_body).unwrap();
        let parsed = simd_json::to_borrowed_value(&mut bytes).unwrap();
        let extracted = extract_tool_calls_from_request(&parsed);
        assert_eq!(extracted.len(), 1);
        assert_eq!(
            extracted[0]["function"]["name"].as_str(),
            Some("openai_tool")
        );

        // Anthropic Format
        let anthropic_body = serde_json::json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "call_2",
                            "name": "anthropic_tool",
                            "input": {}
                        }
                    ]
                }
            ]
        });
        let mut bytes = serde_json::to_vec(&anthropic_body).unwrap();
        let parsed = simd_json::to_borrowed_value(&mut bytes).unwrap();
        let extracted = extract_tool_calls_from_request(&parsed);
        assert_eq!(extracted.len(), 1);
        assert_eq!(extracted[0]["name"].as_str(), Some("anthropic_tool"));

        // Gemini Format
        let gemini_body = serde_json::json!({
            "contents": [
                {
                    "role": "model",
                    "parts": [
                        {
                            "functionCall": {
                                "name": "gemini_tool",
                                "args": {}
                            }
                        }
                    ]
                }
            ]
        });
        let mut bytes = serde_json::to_vec(&gemini_body).unwrap();
        let parsed = simd_json::to_borrowed_value(&mut bytes).unwrap();
        let extracted = extract_tool_calls_from_request(&parsed);
        assert_eq!(extracted.len(), 1);
        assert_eq!(
            extracted[0]["functionCall"]["name"].as_str(),
            Some("gemini_tool")
        );
    }

    #[tokio::test]
    async fn test_mcp_sandbox_multi_tool_denied() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        // Mock tool_1 allowed
        Mock::given(method("POST"))
            .and(path("/v1/data/kryneth/mcp/allow"))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "input": {
                    "tenant_id": "test-tenant",
                    "tool_name": "tool_1",
                    "action": "execute"
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": true
            })))
            .mount(&mock_server)
            .await;

        // Mock tool_2 denied
        Mock::given(method("POST"))
            .and(path("/v1/data/kryneth/mcp/allow"))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "input": {
                    "tenant_id": "test-tenant",
                    "tool_name": "tool_2",
                    "action": "execute"
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": false
            })))
            .mount(&mock_server)
            .await;

        let state = Arc::new(AppState {
            http_client: reqwest::Client::new(),
            compliance_url: mock_server.uri(),
            rate_limit_max: 60,
            rate_limit_window: 60,
            dashboard_url: String::new(),
            llm_api_base_url: None,
            redis_client: None,
            telemetry: Arc::new(crate::infrastructure::oss_adapters::OssTelemetry::new(
                Arc::new(dashmap::DashMap::new()),
            )),
            billing: Arc::new(crate::infrastructure::oss_adapters::OssBilling),
            auth_resolver: Arc::new(crate::infrastructure::oss_adapters::OssAuth),
            rate_limiter: Arc::new(crate::infrastructure::oss_adapters::OssRateLimit),
            routing_config: Arc::new(crate::infrastructure::oss_adapters::OssRoutingConfig),
            semantic_cache: Arc::new(crate::infrastructure::oss_adapters::OssSemanticCache),
            execution_store: Arc::new(
                crate::infrastructure::oss_adapters::MokaExecutionStore::new(
                    moka::future::Cache::builder().build(),
                ),
            ),
            reconciler: Arc::new(crate::infrastructure::oss_adapters::OssReconciler),
            tool_transport: Arc::new(crate::infrastructure::oss_adapters::OssToolTransport),
            rate_limit_cache: Arc::new(dashmap::DashMap::new()),
            l1_cache: Arc::new(L1Cache::new(1024).unwrap()),
            routing_state: Arc::new(RoutingState::new()),
            circuit_breaker: moka::future::Cache::builder().build(),
            loop_fallback_cache: moka::future::Cache::builder().build(),
            mcp_registry: crate::infrastructure::mcp_registry::McpConnectionRegistry::empty(),
            tool_registry: crate::usecases::tool_router::ToolRegistry::empty(),
            agent_guardian_cache: moka::future::Cache::builder().build(),
            operation_cache: moka::future::Cache::builder().build(),
            dashboard_metrics: Arc::new(crate::domain::models::DashboardMetrics::new()),
            pricing_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
            trace_store: Arc::new(dashmap::DashMap::new()),
            budget_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
        });

        // Response with multiple tool calls
        let response_body_json = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "tool_1"
                                }
                            },
                            {
                                "id": "call_2",
                                "type": "function",
                                "function": {
                                    "name": "tool_2"
                                }
                            }
                        ]
                    }
                }
            ]
        });

        let response_bytes = serde_json::to_vec(&response_body_json).unwrap();
        let res_bytes = enforce_mcp_sandbox(&state, "test-tenant", response_bytes)
            .await
            .unwrap();

        let res_json: serde_json::Value = serde_json::from_slice(&res_bytes).unwrap();
        assert_eq!(
            res_json["choices"][0]["message"]["role"].as_str(),
            Some("tool")
        );
        assert!(res_json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .contains("Policy Denied"));
        assert!(res_json["choices"][0]["message"]["tool_calls"].is_null());
        assert_eq!(
            res_json["choices"][0]["finish_reason"].as_str(),
            Some("stop")
        );
    }
}
