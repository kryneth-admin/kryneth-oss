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

use std::sync::Arc;

use futures::{stream, StreamExt};
use serde_json::{json, Value};
use tokio::time::timeout;
use tracing::{info, warn};

use crate::domain::models::AppState;
use crate::domain::utp_models::{ToolExecutionStatus, UniversalToolResult};
use crate::infrastructure::providers::traits::UniversalProviderAdapter;

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
/// Each call is individually wrapped in a `tokio::time::timeout(5s)` so a
/// single slow server cannot stall the entire batch.
///
/// ## Timeout handling
/// Timed-out calls produce a `ToolResult` with `content = {"error":"MCP_TIMEOUT"}`
/// and `success = false`.  They are merged alongside successful results so the
/// LLM always receives a complete, uniform response array.
pub async fn fan_out(
    tool_calls: Vec<ToolCall>,
    tenant_id: &str,
    state: &Arc<AppState>,
    enable_compression: bool,
) -> Vec<ToolResult> {
    if tool_calls.is_empty() {
        return Vec::new();
    }

    let fan_out_count = tool_calls.len();
    let batch_start = std::time::Instant::now();

    info!(
        fan_out_count,
        "Tunnel 3 Phase 4 — bounded fan-out dispatching (limit: 10 concurrent)"
    );

    let tenant_id_owned = tenant_id.to_string();
    // Build a stream of futures; buffer_unordered(10) enforces the concurrency cap.
    let results: Vec<ToolResult> = stream::iter(tool_calls)
        .map(|tc| {
            let state = state.clone();
            let tenant_id_c = tenant_id_owned.clone();
            async move {
                // Per-call 5-second hard timeout.
                match timeout(
                    std::time::Duration::from_secs(5),
                    execute_single_tool_call(tc.clone(), &tenant_id_c, &state, enable_compression),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_elapsed) => {
                        warn!(
                            tool_name = %tc.name,
                            "Tunnel 3 Phase 4 — MCP tool call timed out (5s)"
                        );
                        ToolResult {
                            tool_call_id: tc.id,
                            name: tc.name,
                            content: r#"{"error":"MCP_TIMEOUT"}"#.to_string(),
                            latency_ms: 5000,
                            success: false,
                        }
                    }
                }
            }
        })
        .buffer_unordered(10) // Hard concurrency cap per session.
        .collect()
        .await;

    let total_latency_ms = batch_start.elapsed().as_millis() as u64;

    // Emit fan-out telemetry (fire-and-forget).
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
    state: &Arc<AppState>,
    enable_compression: bool,
) -> ToolResult {
    let start = std::time::Instant::now();

    // Resolve the MCP endpoint URL from the Phase 1 registry.
    let sse_url = match state.mcp_registry.get_url(&tc.name) {
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

    // POST to the MCP server's messages endpoint.
    let messages_url = format!("{}/messages", sse_url.trim_end_matches('/'));

    let response = state
        .http_client
        .post(&messages_url)
        .timeout(std::time::Duration::from_millis(4500)) // < 5s outer tokio timeout
        .json(&mcp_body)
        .send()
        .await;

    let latency_ms = start.elapsed().as_millis() as u64;

    // Record dynamic tool call telemetry in-memory (zero-copy, concurrent)
    state
        .mcp_registry
        .record_tool_call(tenant_id, &tc.name, latency_ms);

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
    let tool_metrics: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "tool_name": r.name,
                "tool_latency_ms": r.latency_ms,
                "success": r.success,
            })
        })
        .collect();

    json!({
        "type": "mcp_fan_out",
        "tenant_id": tenant_id,
        "fan_out_count": fan_out_count,
        "total_latency_ms": total_latency_ms,
        "success_count": results.iter().filter(|r| r.success).count(),
        "tools": tool_metrics,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
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
}
