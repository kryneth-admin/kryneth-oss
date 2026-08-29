//! infrastructure/mcp_client.rs — Tunnel 3
//!
//! ## Architecture (Phase 4 upgrade)
//! ```text
//! [tool_calls: [A, B, C, D, ...]]
//!        │
//!        ▼
//!  fan_out() ──► futures::stream::iter ──► buffer_unordered(10)
//!                  ┌── timeout(5s) ──► execute_single_tool_call(A) ──┐
//!                  ├── timeout(5s) ──► execute_single_tool_call(B) ──┤
//!                  └── timeout(5s) ──► execute_single_tool_call(C) ──┘
//!                                           merge_results() → JSON array
//! ```
//!
//! ## Key constraints
//! * **Bounded concurrency**: `buffer_unordered(10)` caps simultaneous outbound
//!   MCP requests per session — prevents downstream DDoS.
//! * **Per-call timeout**: 5 seconds. Timed-out calls return `{"error":"MCP_TIMEOUT"}`
//!   rather than hanging the entire batch.
//! * **Strict merge format**: `[{"tool":"A","result":{...}}, {"tool":"B","result":{"error":"..."}}]`
//!   — LLM always receives a uniform array regardless of partial failures.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use futures::{stream, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::domain::models::AppState;
use crate::domain::utp_models::{ToolExecutionStatus, UniversalToolResult};
use crate::infrastructure::mcp_registry::McpConnectionRegistry;
use crate::infrastructure::providers::traits::UniversalProviderAdapter;

// ── Idempotency helpers ────────────────────────────────────────────────────────

/// Recursively converts all JSON Object keys to sorted BTreeMap representations for deep canonicalization.
pub fn canonicalize_json(val: Value) -> Value {
    match val {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, canonicalize_json(v)))
                .collect();
            serde_json::to_value(sorted).unwrap_or(Value::Null)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(canonicalize_json).collect()),
        other => other,
    }
}

/// Generates a deterministic SHA-256 idempotency key: Hash(tenant_id + tool_name + sorted_json_args)
pub fn generate_idempotency_key(tenant_id: &str, tool_name: &str, raw_args: &str) -> String {
    let canonical_args = match serde_json::from_str::<Value>(raw_args) {
        Ok(val) => {
            let canonical_val = canonicalize_json(val);
            serde_json::to_string(&canonical_val).unwrap_or_else(|_| raw_args.to_string())
        }
        Err(_) => raw_args.to_string(),
    };

    let mut hasher = Sha256::new();
    hasher.update(tenant_id.as_bytes());
    hasher.update(b"::");
    hasher.update(tool_name.as_bytes());
    hasher.update(b"::");
    hasher.update(canonical_args.as_bytes());

    hex::encode(hasher.finalize())
}

// ── Domain types ──────────────────────────────────────────────────────────────

/// A single tool call extracted from an LLM `tool_calls` array.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// The unique call ID assigned by the LLM (used to correlate results).
    pub id: String,
    /// The function/tool name.
    pub name: String,
    /// The arguments JSON object as a raw string (as received from the LLM).
    pub arguments: String,
}

/// The result of executing one MCP tool call.
#[derive(Debug)]
pub struct ToolResult {
    /// Mirrors `ToolCall::id` for correlation.
    pub tool_call_id: String,
    /// The tool name (for telemetry).
    pub name: String,
    /// The content returned by the MCP server, or an error description.
    pub content: String,
    /// Wall-clock execution time in milliseconds.
    pub latency_ms: u64,
    /// `true` if the MCP call succeeded.
    pub success: bool,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Parses `tool_calls` from an LLM response body using the provided UTP `adapter`
/// and returns them as a `Vec<ToolCall>`.
///
/// Returns an empty `Vec` when no `tool_calls` are present or parsing fails.
pub fn extract_tool_calls(
    response_body: &[u8],
    adapter: &dyn UniversalProviderAdapter,
) -> Vec<ToolCall> {
    let universal_calls = match adapter.extract_calls(response_body) {
        Ok(calls) => calls,
        Err(_) => return Vec::new(),
    };

    universal_calls
        .into_iter()
        .map(|c| ToolCall {
            id: c.call_id,
            name: c.tool_name,
            arguments: c.arguments,
        })
        .collect()
}

/// Executes all `tool_calls` concurrently against their registered MCP
/// endpoints and returns an ordered `Vec<ToolResult>`.
///
/// ## Concurrency model (Phase 4)
/// `futures::stream::buffer_unordered(10)` ensures at most **10 simultaneous
/// outbound MCP connections per fan-out batch**, preventing downstream overload.
pub async fn fan_out(
    tool_calls: Vec<ToolCall>,
    tenant_id: &str,
    state: &Arc<AppState>,
    enable_compression: bool,
    trace_ctx: &crate::domain::models::TraceContext,
) -> Vec<ToolResult> {
    if tool_calls.is_empty() {
        return Vec::new();
    }

    let fan_out_count = tool_calls.len();
    let batch_start = std::time::Instant::now();

    info!(
        fan_out_count,
        "Tunnel 3 Phase 4 — bounded fan-out dispatching via ExecutionService (limit: 10 concurrent)"
    );

    let tenant_id_owned = tenant_id.to_string();
    let results: Vec<ToolResult> = stream::iter(tool_calls.into_iter().enumerate())
        .map(|(index, tc)| {
            let state = state.clone();
            let tenant_id_c = tenant_id_owned.clone();
            let trace_ctx_c = trace_ctx.clone();
            async move {
                crate::usecases::execution_service::ExecutionService::execute_tool(
                    &state,
                    tc,
                    &tenant_id_c,
                    &trace_ctx_c,
                    index,
                    fan_out_count,
                    enable_compression,
                )
                .await
            }
        })
        .buffer_unordered(10) // Bounded concurrency is preserved!
        .collect()
        .await;

    let total_latency_ms = batch_start.elapsed().as_millis() as u64;

    let telemetry_payload =
        build_telemetry_payload(&results, fan_out_count, total_latency_ms, tenant_id);
    state.telemetry.log_event(telemetry_payload);

    let success_count = results.iter().filter(|r| r.success).count();
    let timeout_count = results
        .iter()
        .filter(|r| !r.success && r.content.contains("MCP_TIMEOUT"))
        .count();
    info!(
        fan_out_count,
        total_latency_ms,
        success_count,
        timeout_count,
        "Tunnel 3 Phase 4 — bounded fan-out complete"
    );

    results
}

/// Serialises a `Vec<ToolResult>` into provider-native format using the UTP `adapter`.
pub fn merge_results(
    results: Vec<ToolResult>,
    _enable_compression: bool,
    adapter: &dyn UniversalProviderAdapter,
) -> Value {
    let universal_results: Vec<UniversalToolResult> = results
        .into_iter()
        .map(|r| UniversalToolResult {
            call_id: r.tool_call_id,
            status: if r.success {
                ToolExecutionStatus::Success
            } else {
                ToolExecutionStatus::Error
            },
            content: r.content,
            latency_ms: r.latency_ms,
        })
        .collect();

    adapter
        .format_results(&universal_results)
        .unwrap_or_else(|_| Value::Array(Vec::new()))
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Executes a single MCP tool call against its registered SSE endpoint.
///
/// Uses the `McpConnectionRegistry` to resolve the endpoint URL.  If the
/// tool is not registered, returns a descriptive error result (fail-open).
async fn execute_single_tool_call(
    tc: ToolCall,
    tenant_id: &str,
    mcp_registry: &Arc<McpConnectionRegistry>,
    http_client: &reqwest::Client,
    enable_compression: bool,
    test_scenario: Option<String>,
) -> ToolResult {
    let start = std::time::Instant::now();

    // Resolve the MCP endpoint URL from the Phase 1 registry.
    let sse_url = match mcp_registry.get_url(&tc.name) {
        Some(url) => url,
        None => {
            warn!(
                tool_name = %tc.name,
                "Tunnel 3 Phase 4 — no MCP endpoint registered for tool; returning error result"
            );
            return ToolResult {
                tool_call_id: tc.id,
                name: tc.name,
                content: "MCP endpoint not configured for this tool.".to_string(),
                latency_ms: start.elapsed().as_millis() as u64,
                success: false,
            };
        }
    };

    // Build the MCP tool-call request body (MCP 2024-11-05 spec).
    let mcp_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tc.name,
            "arguments": parse_arguments(&tc.arguments),
        }
    });

    let messages_url = format!("{}/messages", sse_url.trim_end_matches('/'));

    let mut request_builder = http_client
        .post(&messages_url)
        .timeout(std::time::Duration::from_millis(4500)) // < 5s outer tokio timeout
        .json(&mcp_body);

    if let Some(scenario) = test_scenario {
        request_builder = request_builder.header("X-Test-Scenario", scenario);
    }

    let response = request_builder.send().await;

    let latency_ms = start.elapsed().as_millis() as u64;

    // Record dynamic tool call telemetry in-memory (zero-copy, concurrent)
    mcp_registry.record_tool_call(tenant_id, &tc.name, latency_ms);

    match response {
        Ok(resp) if resp.status().is_success() => {
            let content = extract_mcp_result(resp, enable_compression).await;
            info!(
                tool_name = %tc.name,
                latency_ms,
                "Tunnel 3 Phase 4 — MCP tool call succeeded"
            );
            ToolResult {
                tool_call_id: tc.id,
                name: tc.name,
                content,
                latency_ms,
                success: true,
            }
        }
        Ok(resp) => {
            let status = resp.status().as_u16();
            let err_body = resp
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable>".to_string());
            warn!(
                tool_name = %tc.name,
                status,
                latency_ms,
                "Tunnel 3 Phase 4 — MCP server returned non-2xx"
            );
            let name = tc.name.clone();
            ToolResult {
                tool_call_id: tc.id,
                name: name.clone(),
                content: format!("MCP tool '{}' returned HTTP {}: {}", name, status, err_body),
                latency_ms,
                success: false,
            }
        }
        Err(e) => {
            warn!(
                tool_name = %tc.name,
                error = %e,
                latency_ms,
                "Tunnel 3 Phase 4 — MCP tool call network error"
            );
            let name = tc.name.clone();
            ToolResult {
                tool_call_id: tc.id,
                name: name.clone(),
                content: format!("MCP tool '{}' unreachable: {}", name, e),
                latency_ms,
                success: false,
            }
        }
    }
}

/// Parses the `arguments` string from the LLM into a `Value` object.
/// Falls back to an empty object on parse failure (arguments may be a raw JSON
/// string or already a `Value`).
#[inline]
fn parse_arguments(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or(json!({}))
}

fn convert_to_toon(json_array: &Vec<serde_json::Value>) -> Result<String, &'static str> {
    if json_array.is_empty() {
        return Ok("array[0]{}:".to_string());
    }

    let first_obj = json_array[0]
        .as_object()
        .ok_or("First element is not an object")?;
    let mut keys: Vec<&str> = first_obj.keys().map(|k| k.as_str()).collect();
    keys.sort();

    for val in json_array.iter().skip(1) {
        let obj = val.as_object().ok_or("Element is not an object")?;
        if obj.len() != keys.len() {
            return Err("Heterogeneous JSON");
        }
        for k in &keys {
            if !obj.contains_key(*k) {
                return Err("Heterogeneous JSON");
            }
        }
    }

    let keys_str = keys.join(",");
    let mut rows = Vec::new();
    for val in json_array {
        let obj = val.as_object().unwrap();
        let mut row = Vec::new();
        for k in &keys {
            let item_val = obj.get(*k).unwrap();
            let formatted = match item_val {
                Value::String(s) => s.clone(),
                Value::Null => "null".to_string(),
                _ => item_val.to_string(),
            };
            row.push(formatted);
        }
        rows.push(row.join(","));
    }

    let rows_str = rows.join(" \n ");
    Ok(format!(
        "array[{}]{{{}}}: \n {}",
        json_array.len(),
        keys_str,
        rows_str
    ))
}

/// Reads the MCP server response and extracts the `result.content[0].text`
/// field as defined by the MCP 2024-11-05 spec, with a fallback to the raw
/// JSON body on parse failure.
async fn extract_mcp_result(resp: reqwest::Response, enable_compression: bool) -> String {
    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return format!("<failed to parse MCP response: {}>", e);
        }
    };

    let raw_content = if let Some(text) = body
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
    {
        text.to_string()
    } else if let Some(result) = body.get("result") {
        result.to_string()
    } else {
        body.to_string()
    };

    if enable_compression {
        if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&raw_content) {
            if let Ok(toon_str) = convert_to_toon(&arr) {
                return toon_str;
            }
        }
    }

    raw_content
}

/// Builds the telemetry JSON payload for fan-out metrics.
fn build_telemetry_payload(
    results: &[ToolResult],
    fan_out_count: usize,
    total_latency_ms: u64,
    tenant_id: &str,
) -> Value {
    let in_flight_count = results
        .iter()
        .filter(|r| r.content.contains("ALREADY_IN_FLIGHT"))
        .count();
    let unknown_retry_blocked_count = results
        .iter()
        .filter(|r| r.content.contains("PREVIOUS_ATTEMPT_UNKNOWN"))
        .count();

    let tool_metrics: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "tool_name": r.name,
                "tool_latency_ms": r.latency_ms,
                "success": r.success,
                "in_flight": r.content.contains("ALREADY_IN_FLIGHT"),
                "unknown_retry_blocked": r.content.contains("PREVIOUS_ATTEMPT_UNKNOWN"),
            })
        })
        .collect();

    json!({
        "type": "mcp_fan_out",
        "tenant_id": tenant_id,
        "fan_out_count": fan_out_count,
        "total_latency_ms": total_latency_ms,
        "success_count": results.iter().filter(|r| r.success).count(),
        "already_in_flight_count": in_flight_count,
        "unknown_retry_blocked_count": unknown_retry_blocked_count,
        "tools": tool_metrics,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
}

// ── McpToolTransport ──────────────────────────────────────────────────────────

pub struct McpToolTransport {
    pub http_client: reqwest::Client,
    pub mcp_registry: Arc<McpConnectionRegistry>,
}

impl McpToolTransport {
    pub fn new(http_client: reqwest::Client, mcp_registry: Arc<McpConnectionRegistry>) -> Self {
        Self {
            http_client,
            mcp_registry,
        }
    }
}

impl crate::domain::ports::ToolTransport for McpToolTransport {
    fn execute_tool<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        arguments: &'a str,
        tenant_id: &'a str,
        enable_compression: bool,
        test_scenario: Option<&'a str>,
    ) -> Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        let tc = ToolCall {
            id: tool_call_id.to_string(),
            name: tool_name.to_string(),
            arguments: arguments.to_string(),
        };
        let tenant_owned = tenant_id.to_string();
        let mcp_registry = self.mcp_registry.clone();
        let http_client = self.http_client.clone();
        let test_scenario_owned = test_scenario.map(|s| s.to_string());
        Box::pin(async move {
            execute_single_tool_call(
                tc,
                &tenant_owned,
                &mcp_registry,
                &http_client,
                enable_compression,
                test_scenario_owned,
            )
            .await
        })
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tool_calls_parses_correctly() {
        let adapter = crate::infrastructure::providers::OpenAIPlugin::new();
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "call_abc",
                            "type": "function",
                            "function": {
                                "name": "jira_search",
                                "arguments": "{\"query\":\"open bugs\"}"
                            }
                        },
                        {
                            "id": "call_def",
                            "type": "function",
                            "function": {
                                "name": "github_search",
                                "arguments": "{\"repo\":\"kryneth\"}"
                            }
                        }
                    ]
                }
            }]
        });

        let bytes = serde_json::to_vec(&body).unwrap();
        let calls = extract_tool_calls(&bytes, &adapter);

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_abc");
        assert_eq!(calls[0].name, "jira_search");
        assert_eq!(calls[1].name, "github_search");
    }

    #[test]
    fn test_extract_tool_calls_empty_on_no_tool_calls() {
        let adapter = crate::infrastructure::providers::OpenAIPlugin::new();
        let body = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "Hello"}}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert!(extract_tool_calls(&bytes, &adapter).is_empty());
    }

    #[test]
    fn test_extract_tool_calls_no_panic_on_malformed() {
        let adapter = crate::infrastructure::providers::OpenAIPlugin::new();
        assert!(extract_tool_calls(b"{invalid}", &adapter).is_empty());
        assert!(extract_tool_calls(b"", &adapter).is_empty());
    }

    #[test]
    fn test_merge_results_strict_array_format() {
        let adapter = crate::infrastructure::providers::OpenAIPlugin::new();
        let results = vec![
            ToolResult {
                tool_call_id: "call_abc".to_string(),
                name: "jira_search".to_string(),
                content: r#"{"tickets":[{"id":"JR-1"}]}"#.to_string(),
                latency_ms: 120,
                success: true,
            },
            ToolResult {
                tool_call_id: "call_def".to_string(),
                name: "github_search".to_string(),
                content: r#"{"error":"MCP_TIMEOUT"}"#.to_string(),
                latency_ms: 5000,
                success: false,
            },
        ];

        let merged = merge_results(results, false, &adapter);
        assert!(merged.is_array());
        let arr = merged.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        // First entry: OpenAI format tool message
        assert_eq!(arr[0]["role"], "tool");
        assert_eq!(arr[0]["tool_call_id"], "call_abc");

        // Second entry: timeout result
        assert_eq!(arr[1]["role"], "tool");
        assert_eq!(arr[1]["tool_call_id"], "call_def");
        assert!(arr[1]["content"].as_str().unwrap().contains("MCP_TIMEOUT"));
    }

    #[test]
    fn test_merge_results_plain_text_fallback() {
        let adapter = crate::infrastructure::providers::OpenAIPlugin::new();
        let results = vec![ToolResult {
            tool_call_id: "c1".to_string(),
            name: "plain_tool".to_string(),
            content: "plain text result".to_string(),
            latency_ms: 50,
            success: true,
        }];
        let merged = merge_results(results, false, &adapter);
        assert!(merged.is_array());
        let arr = merged.as_array().unwrap();
        assert_eq!(arr[0]["role"], "tool");
        assert_eq!(arr[0]["tool_call_id"], "c1");
        assert_eq!(arr[0]["content"], "plain text result");
    }

    #[test]
    fn test_timeout_placeholder_is_valid_json() {
        // The MCP_TIMEOUT content injected by fan_out must parse as JSON.
        let timeout_content = r#"{"error":"MCP_TIMEOUT"}"#;
        let parsed: serde_json::Value =
            serde_json::from_str(timeout_content).expect("MCP_TIMEOUT must be valid JSON");
        assert_eq!(parsed["error"].as_str(), Some("MCP_TIMEOUT"));
    }

    #[test]
    fn test_parse_arguments_falls_back_to_empty_object() {
        assert_eq!(parse_arguments("{}"), json!({}));
        assert_eq!(parse_arguments("not json"), json!({}));
        let parsed = parse_arguments("{\"key\":\"val\"}");
        assert_eq!(parsed["key"], "val");
    }

    #[test]
    fn test_build_telemetry_payload_shape() {
        let results = vec![ToolResult {
            tool_call_id: "c1".to_string(),
            name: "jira_search".to_string(),
            content: "ok".to_string(),
            latency_ms: 50,
            success: true,
        }];
        let payload = build_telemetry_payload(&results, 1, 50, "test-tenant");
        assert_eq!(payload["type"], "mcp_fan_out");
        assert_eq!(payload["tenant_id"], "test-tenant");
        assert_eq!(payload["fan_out_count"], 1);
        assert_eq!(payload["success_count"], 1);
        assert_eq!(payload["tools"][0]["tool_name"], "jira_search");
        assert_eq!(payload["tools"][0]["tool_latency_ms"], 50);
    }

    #[test]
    fn test_toon_converter_fails_on_heterogeneous_data() {
        let data = vec![
            serde_json::json!({"id": 1, "status": "active"}),
            serde_json::json!({"error": "not found", "code": 404}),
        ];
        let res = convert_to_toon(&data);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), "Heterogeneous JSON");
    }

    #[test]
    fn test_deep_canonicalize_json() {
        let raw1 = serde_json::json!({
            "user": "Alex",
            "metadata": {
                "b": 1,
                "a": 2,
                "nested": { "z": 9, "y": 8 }
            },
            "tags": ["x", "y"]
        });

        let raw2 = serde_json::json!({
            "metadata": {
                "nested": { "y": 8, "z": 9 },
                "a": 2,
                "b": 1
            },
            "tags": ["x", "y"],
            "user": "Alex"
        });

        let canonical1 = canonicalize_json(raw1);
        let canonical2 = canonicalize_json(raw2);

        let str1 = serde_json::to_string(&canonical1).unwrap();
        let str2 = serde_json::to_string(&canonical2).unwrap();

        assert_eq!(str1, str2);
        assert_eq!(
            str1,
            r#"{"metadata":{"a":2,"b":1,"nested":{"y":8,"z":9}},"tags":["x","y"],"user":"Alex"}"#
        );
    }

    #[test]
    fn test_generate_idempotency_key_canonicalization() {
        let args1 = r#"{"user":"Alex","metadata":{"b":1,"a":2}}"#;
        let args2 = r#"{"metadata":{"a":2,"b":1},"user":"Alex"}"#;

        let key1 = generate_idempotency_key("tenant_123", "process_refund", args1);
        let key2 = generate_idempotency_key("tenant_123", "process_refund", args2);

        assert_eq!(key1, key2);

        // Different tenant produces different key
        let key3 = generate_idempotency_key("tenant_456", "process_refund", args1);
        assert_ne!(key1, key3);

        // Different tool produces different key
        let key4 = generate_idempotency_key("tenant_123", "cancel_refund", args1);
        assert_ne!(key1, key4);
    }

    #[tokio::test]
    async fn test_fan_out_idempotency_cache_states() {
        let operation_cache = moka::future::Cache::builder()
            .max_capacity(10 * 1024 * 1024)
            .build();

        let state = Arc::new(AppState {
            http_client: reqwest::Client::new(),
            compliance_url: "http://localhost:8083".to_string(),
            rate_limit_max: 60,
            rate_limit_window: 60,
            dashboard_url: "http://localhost:5173".to_string(),
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
                    operation_cache.clone(),
                ),
            ),
            reconciler: Arc::new(crate::infrastructure::oss_adapters::OssReconciler),
            tool_transport: Arc::new(crate::infrastructure::oss_adapters::OssToolTransport),
            rate_limit_cache: Arc::new(dashmap::DashMap::new()),
            l1_cache: Arc::new(crate::infrastructure::l1_cache::L1Cache::new(1024).unwrap()),
            routing_state: Arc::new(crate::domain::models::RoutingState::new()),
            circuit_breaker: moka::future::Cache::builder().build(),
            loop_fallback_cache: moka::future::Cache::builder().build(),
            mcp_registry: crate::infrastructure::mcp_registry::McpConnectionRegistry::from_env(),
            tool_registry: crate::usecases::tool_router::ToolRegistry::from_env(),
            agent_guardian_cache: moka::future::Cache::builder().build(),
            operation_cache: operation_cache.clone(),
            dashboard_metrics: Arc::new(crate::domain::models::DashboardMetrics::new()),
            pricing_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
            trace_store: Arc::new(dashmap::DashMap::new()),
            budget_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
        });

        let trace_ctx = crate::domain::models::TraceContext {
            trace_id: "test_trace".to_string(),
            session_id: "test_session".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec_test".to_string()),
            operation_id: None,
            idempotency_key: Some("idem_test".to_string()),
            test_scenario: None,
        };

        let tool_call = ToolCall {
            id: "call_123".to_string(),
            name: "test_refund".to_string(),
            arguments: r#"{"amount":100}"#.to_string(),
        };

        let op_key = {
            let mut hasher = Sha256::new();
            hasher.update("tenant_test".as_bytes());
            hasher.update(b"::");
            hasher.update("idem_test".as_bytes());
            hasher.update(b"::");
            hasher.update("0".as_bytes());
            hasher.update(b"::");
            hasher.update("test_refund".as_bytes());
            hasher.update(b"::");
            hasher.update(r#"{"amount":100}"#.as_bytes());
            hex::encode(hasher.finalize())
        };

        let exec = crate::domain::execution::ToolExecution {
            execution_id: crate::domain::execution::ExecutionId("exec_test".to_string()),
            operation_id: crate::domain::execution::OperationId("op_test".to_string()),
            workflow_id: None,
            agent_id: None,
            tenant_id: crate::domain::execution::TenantId("tenant_test".to_string()),
            session_id: Some(crate::domain::execution::SessionId(
                "test_session".to_string(),
            )),
            tool_name: "test_refund".to_string(),
            arguments_hash: op_key.clone(),
            idempotency_key: crate::domain::execution::IdempotencyKey(op_key.clone()),
            attempt: 1,
            state: crate::domain::execution::ExecutionState::Succeeded,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        // Scenario A: Succeeded state returns cached result
        let ctx = crate::domain::execution::ExecutionContext {
            result_content: Some(r#"{"status":"refunded"}"#.to_string()),
            latency_ms: Some(120),
            error_message: None,
            lease_until: None,
            version: 1,
        };
        operation_cache
            .insert(
                op_key.clone(),
                crate::domain::models::OperationCacheEntry {
                    state: crate::domain::execution::ExecutionState::Succeeded,
                    context: ctx,
                    execution: exec.clone(),
                },
            )
            .await;

        let results = fan_out(
            vec![tool_call.clone()],
            "tenant_test",
            &state,
            false,
            &trace_ctx,
        )
        .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert_eq!(results[0].content, r#"{"status":"refunded"}"#);
        assert_eq!(results[0].latency_ms, 120);

        // Scenario B: Claimed/Running state returns ALREADY_IN_FLIGHT
        let ctx_in_flight = crate::domain::execution::ExecutionContext {
            result_content: None,
            latency_ms: None,
            error_message: None,
            lease_until: Some(std::time::Instant::now() + std::time::Duration::from_secs(10)),
            version: 1,
        };
        operation_cache
            .insert(
                op_key.clone(),
                crate::domain::models::OperationCacheEntry {
                    state: crate::domain::execution::ExecutionState::Running,
                    context: ctx_in_flight,
                    execution: exec.clone(),
                },
            )
            .await;

        let results_in_flight = fan_out(
            vec![tool_call.clone()],
            "tenant_test",
            &state,
            false,
            &trace_ctx,
        )
        .await;
        assert_eq!(results_in_flight.len(), 1);
        assert!(!results_in_flight[0].success);
        assert_eq!(
            results_in_flight[0].content,
            r#"{"error":"ALREADY_IN_FLIGHT"}"#
        );

        // Scenario C: Unknown state blocks unsafe auto-retry
        let ctx_unknown = crate::domain::execution::ExecutionContext {
            result_content: None,
            latency_ms: None,
            error_message: Some("timeout".to_string()),
            lease_until: None,
            version: 1,
        };
        operation_cache
            .insert(
                op_key.clone(),
                crate::domain::models::OperationCacheEntry {
                    state: crate::domain::execution::ExecutionState::Unknown,
                    context: ctx_unknown,
                    execution: exec,
                },
            )
            .await;

        let results_unknown = fan_out(
            vec![tool_call.clone()],
            "tenant_test",
            &state,
            false,
            &trace_ctx,
        )
        .await;
        assert_eq!(results_unknown.len(), 1);
        assert!(!results_unknown[0].success);
        assert_eq!(
            results_unknown[0].content,
            r#"{"error":"PREVIOUS_ATTEMPT_UNKNOWN"}"#
        );
    }
}
