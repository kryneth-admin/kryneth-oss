//! anthropic.rs — UniversalProviderAdapter implementation for Anthropic Claude API.

use crate::domain::models::{
    KrynethContent, KrynethConversation, KrynethRole, StandardResponse, StandardStreamChunk,
};
use crate::domain::utp_models::{ToolExecutionStatus, UniversalToolCall, UniversalToolResult};
use crate::error::GatewayError;
use crate::infrastructure::providers::traits::UniversalProviderAdapter;
use serde::Serialize;
use serde_json::{json, Value};
use simd_json::prelude::*;

#[derive(Debug, Serialize, PartialEq)]
pub struct AnthropicRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: Vec<AnthropicContent>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicContent {
    Text {
        text: String,
    },
    Image {
        source: AnthropicImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize, PartialEq)]
pub struct AnthropicImageSource {
    pub r#type: String,
    pub media_type: String,
    pub data: String,
}

/// Adapter plugin for translating Anthropic tool calls, tool results, and payloads to/from UTP.
#[derive(Debug, Clone, Default)]
pub struct AnthropicPlugin;

impl AnthropicPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl UniversalProviderAdapter for AnthropicPlugin {
    fn extract_calls(&self, raw_payload: &[u8]) -> Result<Vec<UniversalToolCall>, GatewayError> {
        let mut buffer = raw_payload.to_vec();
        let value = simd_json::to_borrowed_value(&mut buffer)
            .map_err(|e| GatewayError::InvalidJSON(e.to_string()))?;

        let mut tool_calls = Vec::new();

        match &value {
            simd_json::BorrowedValue::Object(map) => {
                if let Some(simd_json::BorrowedValue::Array(content_blocks)) = map.get("content") {
                    for block in content_blocks {
                        if let Some(call) = parse_anthropic_block(block)? {
                            tool_calls.push(call);
                        }
                    }
                } else if map.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    if let Some(call) = parse_anthropic_block(&value)? {
                        tool_calls.push(call);
                    }
                }
            }
            simd_json::BorrowedValue::Array(blocks) => {
                for block in blocks {
                    if let Some(call) = parse_anthropic_block(block)? {
                        tool_calls.push(call);
                    }
                }
            }
            _ => {
                return Err(GatewayError::InvalidJSON(
                    "Unexpected JSON root type".into(),
                ))
            }
        }

        Ok(tool_calls)
    }

    fn format_results(
        &self,
        results: &[UniversalToolResult],
    ) -> Result<serde_json::Value, GatewayError> {
        let mut blocks = Vec::with_capacity(results.len());

        for res in results {
            let is_error = res.status == ToolExecutionStatus::Error;
            let block = json!({
                "type": "tool_result",
                "tool_use_id": res.call_id,
                "content": res.content,
                "is_error": is_error
            });
            blocks.push(block);
        }

        Ok(serde_json::Value::Array(blocks))
    }

    fn to_universal(&self, _payload: &mut [u8]) -> Result<KrynethConversation, GatewayError> {
        Err(GatewayError::ResponseBuild(
            "Anthropic to_universal not fully implemented".to_string(),
        ))
    }

    fn from_universal(&self, conv: &KrynethConversation) -> Result<Value, GatewayError> {
        let mut anthropic_messages: Vec<AnthropicMessage> = Vec::with_capacity(conv.messages.len());

        for msg in &conv.messages {
            let anthropic_role = match msg.role {
                KrynethRole::System => continue,
                KrynethRole::User | KrynethRole::Tool => "user".to_string(),
                KrynethRole::Assistant => "assistant".to_string(),
            };

            if let Some(last_msg) = anthropic_messages.last() {
                if last_msg.role == anthropic_role {
                    let dummy_role = if anthropic_role == "user" {
                        "assistant"
                    } else {
                        "user"
                    };
                    anthropic_messages.push(AnthropicMessage {
                        role: dummy_role.to_string(),
                        content: vec![AnthropicContent::Text {
                            text: "<dummy>".to_string(),
                        }],
                    });
                }
            }

            let mut anthropic_content = Vec::with_capacity(msg.content.len());
            for c in &msg.content {
                match c {
                    KrynethContent::Text { text } => {
                        anthropic_content.push(AnthropicContent::Text { text: text.clone() });
                    }
                    KrynethContent::ImageUrl { url } => {
                        anthropic_content.push(AnthropicContent::Image {
                            source: AnthropicImageSource {
                                r#type: "url".to_string(),
                                media_type: "image/jpeg".to_string(),
                                data: url.clone(),
                            },
                        });
                    }
                    KrynethContent::ToolCall {
                        id,
                        name,
                        arguments,
                    } => {
                        anthropic_content.push(AnthropicContent::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: arguments.clone(),
                        });
                    }
                    KrynethContent::ToolResult { tool_id, content } => {
                        anthropic_content.push(AnthropicContent::ToolResult {
                            tool_use_id: tool_id.clone(),
                            content: content.clone(),
                        });
                    }
                }
            }

            anthropic_messages.push(AnthropicMessage {
                role: anthropic_role,
                content: anthropic_content,
            });
        }

        let mapped_tools = map_openai_tools_to_anthropic(conv.tools.clone());

        let req = AnthropicRequest {
            system: conv.system_prompt.clone(),
            messages: anthropic_messages,
            tools: mapped_tools,
            temperature: conv.temperature,
            top_p: conv.top_p,
            max_tokens: conv.max_tokens.unwrap_or(4096),
            stop_sequences: conv.stop.clone(),
            stream: conv.stream,
            model: conv.model.clone(),
        };

        serde_json::to_value(req)
            .map_err(|e| GatewayError::ResponseBuild(format!("Anthropic serialize error: {e}")))
    }

    fn unify_response(&self, raw: Value) -> Result<StandardResponse, GatewayError> {
        let id = raw
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("chatcmpl-fallback");
        let model = raw
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("claude");

        let mapped_tool_calls = raw
            .get("content")
            .and_then(|c| c.as_array())
            .cloned()
            .and_then(map_anthropic_tool_use_to_openai_calls);

        let text_content = raw
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            })
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        let input_tokens = raw
            .get("usage")
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let output_tokens = raw
            .get("usage")
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        let finish_reason = if mapped_tool_calls.is_some() {
            "tool_calls"
        } else {
            "stop"
        };

        let mut message_obj = json!({
            "role": "assistant",
            "content": text_content
        });

        if let Some(tool_calls) = mapped_tool_calls {
            message_obj["tool_calls"] = json!(tool_calls);
        }

        let mock_openai_resp = json!({
            "id": id,
            "object": "chat.completion",
            "created": 0,
            "model": model,
            "choices": [{
                "index": 0,
                "message": message_obj,
                "finish_reason": finish_reason
            }],
            "usage": {
                "prompt_tokens": input_tokens,
                "completion_tokens": output_tokens,
                "total_tokens": input_tokens + output_tokens
            }
        });

        Ok(mock_openai_resp)
    }

    fn unify_stream_chunk(&self, chunk: String) -> Result<StandardStreamChunk, GatewayError> {
        if !chunk.starts_with("data: ") || chunk == "data: [DONE]" {
            return Ok(chunk);
        }

        let json_str = chunk.trim_start_matches("data: ").trim();
        if json_str.is_empty() {
            return Ok(chunk);
        }

        let val: Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => return Ok("".to_string()),
        };

        let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if event_type == "content_block_delta" {
            let text = val
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let mock_openai_chunk = json!({
                "id": "chatcmpl-stream",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "claude",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "content": text
                    },
                    "finish_reason": Value::Null
                }]
            });
            let chunk_str = serde_json::to_string(&mock_openai_chunk).map_err(|e| {
                GatewayError::ResponseBuild(format!("Failed to serialize chunk: {e}"))
            })?;
            return Ok(format!("data: {chunk_str}"));
        }

        if event_type == "message_stop" {
            return Ok("data: [DONE]".to_string());
        }

        Ok("".to_string())
    }
}

/// Absorbed helper: Maps OpenAI format tools to Anthropic format tools.
fn map_openai_tools_to_anthropic(openai_tools: Option<Vec<Value>>) -> Option<Vec<Value>> {
    let tools = openai_tools?;
    let mut mapped_tools = Vec::with_capacity(tools.len());

    for tool in tools {
        if tool.get("type").and_then(|t| t.as_str()) != Some("function") {
            continue;
        }

        if let Some(function) = tool.get("function").and_then(|f| f.as_object()) {
            if let Some(name) = function.get("name").and_then(|n| n.as_str()) {
                let description = function
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or_default();
                let parameters = match function.get("parameters") {
                    Some(p) => p.clone(),
                    None => json!({"type": "object", "properties": {}}),
                };

                mapped_tools.push(json!({
                    "name": name,
                    "description": description,
                    "input_schema": parameters
                }));
            }
        }
    }

    if mapped_tools.is_empty() {
        None
    } else {
        Some(mapped_tools)
    }
}

/// Absorbed helper: Maps Anthropic message content blocks into OpenAI tool_calls.
fn map_anthropic_tool_use_to_openai_calls(
    anthropic_content_blocks: Vec<Value>,
) -> Option<Vec<Value>> {
    let mut openai_calls = Vec::new();

    for block in anthropic_content_blocks {
        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
            if let (Some(id), Some(name), Some(input)) = (
                block.get("id").and_then(|i| i.as_str()),
                block.get("name").and_then(|n| n.as_str()),
                block.get("input").and_then(|i| i.as_object()),
            ) {
                if let Ok(arguments_json) = serde_json::to_string(input) {
                    openai_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": arguments_json
                        }
                    }));
                }
            }
        }
    }

    if openai_calls.is_empty() {
        None
    } else {
        Some(openai_calls)
    }
}

fn parse_anthropic_block(
    block: &simd_json::BorrowedValue<'_>,
) -> Result<Option<UniversalToolCall>, GatewayError> {
    let block_type = block.get("type").and_then(|t| t.as_str());
    if block_type != Some("tool_use") {
        return Ok(None);
    }

    let call_id = block.get("id").and_then(|id| id.as_str()).ok_or_else(|| {
        GatewayError::InvalidJSON("Missing 'id' field in Anthropic tool_use block".into())
    })?;

    let tool_name = block.get("name").and_then(|n| n.as_str()).ok_or_else(|| {
        GatewayError::InvalidJSON("Missing 'name' field in Anthropic tool_use block".into())
    })?;

    let input = block.get("input").ok_or_else(|| {
        GatewayError::InvalidJSON("Missing 'input' field in Anthropic tool_use block".into())
    })?;

    let arguments = match input {
        simd_json::BorrowedValue::String(s) => s.to_string(),
        _ => serde_json::to_string(input).map_err(|e| {
            GatewayError::InvalidJSON(format!("Failed to serialize tool input: {e}"))
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
    fn test_extract_calls_valid_anthropic_message() {
        let plugin = AnthropicPlugin::new();
        let payload = json!({
            "id": "msg_01123",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "Checking the weather..."
                },
                {
                    "type": "tool_use",
                    "id": "toolu_01A09q9065609",
                    "name": "get_weather",
                    "input": {
                        "location": "San Francisco, CA"
                    }
                }
            ]
        });

        let raw_bytes = serde_json::to_vec(&payload).unwrap();
        let calls = plugin
            .extract_calls(&raw_bytes)
            .expect("Should extract tool calls");

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "toolu_01A09q9065609");
        assert_eq!(calls[0].tool_name, "get_weather");

        let args: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["location"], "San Francisco, CA");
    }

    #[test]
    fn test_extract_calls_array_of_blocks() {
        let plugin = AnthropicPlugin::new();
        let payload = json!([
            {
                "type": "tool_use",
                "id": "toolu_999",
                "name": "search_db",
                "input": { "query": "rust" }
            }
        ]);

        let raw_bytes = serde_json::to_vec(&payload).unwrap();
        let calls = plugin
            .extract_calls(&raw_bytes)
            .expect("Should extract tool calls");

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "toolu_999");
        assert_eq!(calls[0].tool_name, "search_db");
    }

    #[test]
    fn test_extract_calls_invalid_json() {
        let plugin = AnthropicPlugin::new();
        let raw_bytes = b"not a json";

        let result = plugin.extract_calls(raw_bytes);
        assert!(matches!(result, Err(GatewayError::InvalidJSON(_))));
    }

    #[test]
    fn test_extract_calls_missing_required_fields() {
        let plugin = AnthropicPlugin::new();
        let payload = json!({
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_123"
                }
            ]
        });

        let raw_bytes = serde_json::to_vec(&payload).unwrap();
        let result = plugin.extract_calls(&raw_bytes);
        assert!(matches!(result, Err(GatewayError::InvalidJSON(_))));
    }

    #[test]
    fn test_format_results_success_and_error() {
        let plugin = AnthropicPlugin::new();
        let results = vec![
            UniversalToolResult {
                call_id: "toolu_01".into(),
                status: ToolExecutionStatus::Success,
                content: "72F and sunny".into(),
                latency_ms: 120,
            },
            UniversalToolResult {
                call_id: "toolu_02".into(),
                status: ToolExecutionStatus::Error,
                content: "Connection timeout".into(),
                latency_ms: 5000,
            },
        ];

        let formatted = plugin
            .format_results(&results)
            .expect("Should format results");

        assert!(formatted.is_array());
        let arr = formatted.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        assert_eq!(arr[0]["type"], "tool_result");
        assert_eq!(arr[0]["tool_use_id"], "toolu_01");
        assert_eq!(arr[0]["content"], "72F and sunny");
        assert_eq!(arr[0]["is_error"], false);

        assert_eq!(arr[1]["type"], "tool_result");
        assert_eq!(arr[1]["tool_use_id"], "toolu_02");
        assert_eq!(arr[1]["content"], "Connection timeout");
        assert_eq!(arr[1]["is_error"], true);
    }
}
