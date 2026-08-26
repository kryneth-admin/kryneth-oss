//! bedrock.rs — UniversalProviderAdapter implementation for AWS Bedrock Converse API.

use crate::domain::models::{KrynethConversation, StandardResponse, StandardStreamChunk};
use crate::domain::utp_models::{ToolExecutionStatus, UniversalToolCall, UniversalToolResult};
use crate::error::GatewayError;
use crate::infrastructure::providers::traits::UniversalProviderAdapter;
use serde_json::json;
use simd_json::prelude::*;

/// Adapter plugin for translating AWS Bedrock Converse API toolUse and toolResult blocks to/from UTP.
#[derive(Debug, Clone, Default)]
pub struct BedrockPlugin;

impl BedrockPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl UniversalProviderAdapter for BedrockPlugin {
    fn extract_calls(&self, raw_payload: &[u8]) -> Result<Vec<UniversalToolCall>, GatewayError> {
        let mut buffer = raw_payload.to_vec();
        let value = simd_json::to_borrowed_value(&mut buffer)
            .map_err(|e| GatewayError::InvalidJSON(e.to_string()))?;

        let mut tool_calls = Vec::new();

        if let Some(content) = value
            .get("output")
            .and_then(|o| o.get("message"))
            .and_then(|m| m.get("content"))
            .or_else(|| value.get("message").and_then(|m| m.get("content")))
            .or_else(|| value.get("content"))
            .and_then(|c| c.as_array())
        {
            for block in content {
                if let Some(call) = parse_bedrock_block(block)? {
                    tool_calls.push(call);
                }
            }
            return Ok(tool_calls);
        }

        if let simd_json::BorrowedValue::Array(arr) = &value {
            for block in arr {
                if let Some(call) = parse_bedrock_block(block)? {
                    tool_calls.push(call);
                }
            }
            return Ok(tool_calls);
        }

        if let Some(call) = parse_bedrock_block(&value)? {
            tool_calls.push(call);
        }

        Ok(tool_calls)
    }

    fn format_results(
        &self,
        results: &[UniversalToolResult],
    ) -> Result<serde_json::Value, GatewayError> {
        let mut blocks = Vec::with_capacity(results.len());

        for res in results {
            let status = match res.status {
                ToolExecutionStatus::Success => "success",
                ToolExecutionStatus::Error => "error",
            };

            let block = json!({
                "toolResult": {
                    "toolUseId": res.call_id,
                    "content": [
                        { "text": res.content }
                    ],
                    "status": status
                }
            });
            blocks.push(block);
        }

        Ok(serde_json::Value::Array(blocks))
    }

    fn to_universal(&self, _payload: &mut [u8]) -> Result<KrynethConversation, GatewayError> {
        Err(GatewayError::ResponseBuild(
            "Bedrock to_universal not implemented".to_string(),
        ))
    }

    fn from_universal(
        &self,
        _conv: &KrynethConversation,
    ) -> Result<serde_json::Value, GatewayError> {
        Err(GatewayError::ResponseBuild(
            "Bedrock from_universal not implemented".to_string(),
        ))
    }

    fn unify_response(&self, raw: serde_json::Value) -> Result<StandardResponse, GatewayError> {
        Ok(raw)
    }

    fn unify_stream_chunk(&self, chunk: String) -> Result<StandardStreamChunk, GatewayError> {
        Ok(chunk)
    }
}

fn parse_bedrock_block(
    val: &simd_json::BorrowedValue<'_>,
) -> Result<Option<UniversalToolCall>, GatewayError> {
    let tool_use = match val.get("toolUse") {
        Some(tu) => tu,
        None => return Ok(None),
    };

    let call_id = tool_use
        .get("toolUseId")
        .and_then(|id| id.as_str())
        .ok_or_else(|| {
            GatewayError::InvalidJSON("Missing 'toolUseId' in Bedrock toolUse block".into())
        })?;

    let tool_name = tool_use
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| {
            GatewayError::InvalidJSON("Missing 'name' in Bedrock toolUse block".into())
        })?;

    let input = tool_use.get("input").ok_or_else(|| {
        GatewayError::InvalidJSON("Missing 'input' in Bedrock toolUse block".into())
    })?;

    let arguments = match input {
        simd_json::BorrowedValue::String(s) => s.to_string(),
        other => serde_json::to_string(other).map_err(|e| {
            GatewayError::InvalidJSON(format!("Failed to serialize Bedrock input: {e}"))
        })?,
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
    use serde_json::json;

    #[test]
    fn test_extract_calls_bedrock() {
        let plugin = BedrockPlugin::new();
        let payload = json!({
            "output": {
                "message": {
                    "content": [
                        {
                            "toolUse": {
                                "toolUseId": "tooluse_bedrock_1",
                                "name": "get_stock_price",
                                "input": {
                                    "symbol": "AAPL"
                                }
                            }
                        }
                    ]
                }
            }
        });

        let raw_bytes = serde_json::to_vec(&payload).unwrap();
        let calls = plugin
            .extract_calls(&raw_bytes)
            .expect("Should extract toolUse");

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "tooluse_bedrock_1");
        assert_eq!(calls[0].tool_name, "get_stock_price");

        let args: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["symbol"], "AAPL");
    }

    #[test]
    fn test_extract_calls_invalid_json() {
        let plugin = BedrockPlugin::new();
        let result = plugin.extract_calls(b"malformed");
        assert!(matches!(result, Err(GatewayError::InvalidJSON(_))));
    }

    #[test]
    fn test_format_results() {
        let plugin = BedrockPlugin::new();
        let results = vec![UniversalToolResult {
            call_id: "tooluse_bedrock_1".into(),
            status: ToolExecutionStatus::Success,
            content: "150.00 USD".into(),
            latency_ms: 80,
        }];

        let formatted = plugin
            .format_results(&results)
            .expect("Should format results");

        assert!(formatted.is_array());
        let arr = formatted.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["toolResult"]["toolUseId"], "tooluse_bedrock_1");
        assert_eq!(arr[0]["toolResult"]["status"], "success");
        assert_eq!(arr[0]["toolResult"]["content"][0]["text"], "150.00 USD");
    }
}
