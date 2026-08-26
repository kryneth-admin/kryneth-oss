//! cohere.rs — UniversalProviderAdapter implementation for Cohere API.

use crate::domain::models::{KrynethConversation, StandardResponse, StandardStreamChunk};
use crate::domain::utp_models::{UniversalToolCall, UniversalToolResult};
use crate::error::GatewayError;
use crate::infrastructure::providers::traits::UniversalProviderAdapter;
use serde_json::json;
use simd_json::prelude::*;

/// Adapter plugin for translating Cohere tool_calls and outputs to/from UTP.
#[derive(Debug, Clone, Default)]
pub struct CoherePlugin;

impl CoherePlugin {
    pub fn new() -> Self {
        Self
    }
}

impl UniversalProviderAdapter for CoherePlugin {
    fn extract_calls(&self, raw_payload: &[u8]) -> Result<Vec<UniversalToolCall>, GatewayError> {
        let mut buffer = raw_payload.to_vec();
        let value = simd_json::to_borrowed_value(&mut buffer)
            .map_err(|e| GatewayError::InvalidJSON(e.to_string()))?;

        let mut tool_calls = Vec::new();

        if let Some(tc_arr) = value
            .get("tool_calls")
            .or_else(|| value.get("response").and_then(|r| r.get("tool_calls")))
            .and_then(|t| t.as_array())
        {
            for tc in tc_arr {
                if let Some(call) = parse_cohere_call(tc)? {
                    tool_calls.push(call);
                }
            }
            return Ok(tool_calls);
        }

        if let simd_json::BorrowedValue::Array(arr) = &value {
            for item in arr {
                if let Some(call) = parse_cohere_call(item)? {
                    tool_calls.push(call);
                }
            }
            return Ok(tool_calls);
        }

        if let Some(call) = parse_cohere_call(&value)? {
            tool_calls.push(call);
        }

        Ok(tool_calls)
    }

    fn format_results(
        &self,
        results: &[UniversalToolResult],
    ) -> Result<serde_json::Value, GatewayError> {
        let mut outputs = Vec::with_capacity(results.len());

        for res in results {
            let item = json!({
                "call": {
                    "name": res.call_id,
                    "parameters": {}
                },
                "outputs": [
                    { "result": res.content }
                ]
            });
            outputs.push(item);
        }

        Ok(serde_json::Value::Array(outputs))
    }

    fn to_universal(&self, _payload: &mut [u8]) -> Result<KrynethConversation, GatewayError> {
        Err(GatewayError::ResponseBuild(
            "Cohere to_universal not implemented".to_string(),
        ))
    }

    fn from_universal(
        &self,
        _conv: &KrynethConversation,
    ) -> Result<serde_json::Value, GatewayError> {
        Err(GatewayError::ResponseBuild(
            "Cohere from_universal not implemented".to_string(),
        ))
    }

    fn unify_response(&self, raw: serde_json::Value) -> Result<StandardResponse, GatewayError> {
        Ok(raw)
    }

    fn unify_stream_chunk(&self, chunk: String) -> Result<StandardStreamChunk, GatewayError> {
        Ok(chunk)
    }
}

fn parse_cohere_call(
    val: &simd_json::BorrowedValue<'_>,
) -> Result<Option<UniversalToolCall>, GatewayError> {
    let tool_name = match val.get("name").and_then(|n| n.as_str()) {
        Some(name) => name,
        None => return Ok(None),
    };

    let call_id = val
        .get("id")
        .and_then(|id| id.as_str())
        .unwrap_or(tool_name);

    let arguments = match val.get("parameters").or_else(|| val.get("arguments")) {
        Some(simd_json::BorrowedValue::String(s)) => s.to_string(),
        Some(other) => serde_json::to_string(other).map_err(|e| {
            GatewayError::InvalidJSON(format!("Failed to serialize Cohere parameters: {e}"))
        })?,
        None => "{}".to_string(),
    };

    Ok(Some(UniversalToolCall {
        call_id: call_id.to_string(),
        tool_name: tool_name.to_string(),
        arguments,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::utp_models::ToolExecutionStatus;
    use serde_json::json;

    #[test]
    fn test_extract_calls_cohere() {
        let plugin = CoherePlugin::new();
        let payload = json!({
            "response_id": "cohere_123",
            "tool_calls": [
                {
                    "name": "lookup_user",
                    "parameters": {
                        "user_id": 42
                    }
                }
            ]
        });

        let raw_bytes = serde_json::to_vec(&payload).unwrap();
        let calls = plugin
            .extract_calls(&raw_bytes)
            .expect("Should extract Cohere tool_calls");

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_name, "lookup_user");
        assert_eq!(calls[0].call_id, "lookup_user");

        let args: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["user_id"], 42);
    }

    #[test]
    fn test_extract_calls_invalid_json() {
        let plugin = CoherePlugin::new();
        let result = plugin.extract_calls(b"bad cohere json");
        assert!(matches!(result, Err(GatewayError::InvalidJSON(_))));
    }

    #[test]
    fn test_format_results() {
        let plugin = CoherePlugin::new();
        let results = vec![UniversalToolResult {
            call_id: "lookup_user".into(),
            status: ToolExecutionStatus::Success,
            content: "User 42 found".into(),
            latency_ms: 30,
        }];

        let formatted = plugin
            .format_results(&results)
            .expect("Should format results");

        assert!(formatted.is_array());
        let arr = formatted.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["call"]["name"], "lookup_user");
        assert_eq!(arr[0]["outputs"][0]["result"], "User 42 found");
    }
}
