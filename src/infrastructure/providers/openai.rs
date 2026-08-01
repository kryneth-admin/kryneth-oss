//! openai.rs — UniversalProviderAdapter implementation for OpenAI, Groq, Mistral, DeepSeek, and Azure.

use crate::domain::models::{
    KrynethContent, KrynethConversation, KrynethMessage, KrynethRole, StandardResponse,
    StandardStreamChunk,
};
use crate::domain::utp_models::{UniversalToolCall, UniversalToolResult};
use crate::error::GatewayError;
use crate::infrastructure::providers::traits::UniversalProviderAdapter;
use serde::{Deserialize, Serialize};
use serde_json::json;
use simd_json::prelude::*;

#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIChatRequest {
    pub messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIToolCall {
    pub id: String,
    pub r#type: String,
    pub function: OpenAIFunctionCall,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Adapter plugin for translating OpenAI format tool calls and payloads to/from UTP.
#[derive(Debug, Clone, Default)]
pub struct OpenAIPlugin;

impl OpenAIPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl UniversalProviderAdapter for OpenAIPlugin {
    fn extract_calls(&self, raw_payload: &[u8]) -> Result<Vec<UniversalToolCall>, GatewayError> {
        let mut buffer = raw_payload.to_vec();
        let value = simd_json::to_borrowed_value(&mut buffer)
            .map_err(|e| GatewayError::InvalidJSON(e.to_string()))?;

        let mut tool_calls = Vec::new();

        if let Some(choices) = value.get("choices").and_then(|c| c.as_array()) {
            if let Some(first_choice) = choices.first() {
                if let Some(msg) = first_choice
                    .get("message")
                    .or_else(|| first_choice.get("delta"))
                {
                    if let Some(tc_arr) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tc_arr {
                            if let Some(call) = parse_openai_tool_call(tc)? {
                                tool_calls.push(call);
                            }
                        }
                        return Ok(tool_calls);
                    }
                }
            }
        }

        if let Some(tc_arr) = value
            .get("tool_calls")
            .or_else(|| value.get("message").and_then(|m| m.get("tool_calls")))
            .and_then(|t| t.as_array())
        {
            for tc in tc_arr {
                if let Some(call) = parse_openai_tool_call(tc)? {
                    tool_calls.push(call);
                }
            }
            return Ok(tool_calls);
        }

        if let simd_json::BorrowedValue::Array(arr) = &value {
            for item in arr {
                if let Some(call) = parse_openai_tool_call(item)? {
                    tool_calls.push(call);
                }
            }
            return Ok(tool_calls);
        }

        if let Some(call) = parse_openai_tool_call(&value)? {
            tool_calls.push(call);
        }

        Ok(tool_calls)
    }

    fn format_results(
        &self,
        results: &[UniversalToolResult],
    ) -> Result<serde_json::Value, GatewayError> {
        let mut messages = Vec::with_capacity(results.len());

        for res in results {
            let msg = json!({
                "role": "tool",
                "tool_call_id": res.call_id,
                "content": res.content
            });
            messages.push(msg);
        }

        Ok(serde_json::Value::Array(messages))
    }

    fn to_universal(&self, payload: &mut [u8]) -> Result<KrynethConversation, GatewayError> {
        let req: OpenAIChatRequest = serde_json::from_slice(payload)
            .map_err(|e| GatewayError::InvalidJSON(format!("OpenAI parse error: {e}")))?;

        let mut system_prompt = None;
        let mut kryneth_messages = Vec::with_capacity(req.messages.len());

        for msg in req.messages {
            let role = match msg.role.as_str() {
                "system" => {
                    if let Some(content) = msg.content {
                        if let Some(s) = content.as_str() {
                            system_prompt = Some(s.to_string());
                        } else if let Some(arr) = content.as_array() {
                            for part in arr {
                                if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                        system_prompt = Some(text.to_string());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
                "user" => KrynethRole::User,
                "assistant" => KrynethRole::Assistant,
                "tool" | "function" => KrynethRole::Tool,
                other => {
                    return Err(GatewayError::InvalidJSON(format!(
                        "Unknown OpenAI role: {other}"
                    )))
                }
            };

            let mut contents = Vec::new();

            if let Some(content) = msg.content {
                if let Some(s) = content.as_str() {
                    if !s.is_empty() {
                        contents.push(KrynethContent::Text {
                            text: s.to_string(),
                        });
                    }
                } else if let Some(arr) = content.as_array() {
                    for part in arr {
                        if let Some(type_str) = part.get("type").and_then(|v| v.as_str()) {
                            if type_str == "text" {
                                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                    contents.push(KrynethContent::Text {
                                        text: text.to_string(),
                                    });
                                }
                            } else if type_str == "image_url" {
                                if let Some(url_obj) = part.get("image_url") {
                                    if let Some(url_str) =
                                        url_obj.get("url").and_then(|v| v.as_str())
                                    {
                                        contents.push(KrynethContent::ImageUrl {
                                            url: url_str.to_string(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(tool_calls) = msg.tool_calls {
                for tc in tool_calls {
                    let args = serde_json::from_str(&tc.function.arguments).map_err(|_| {
                        GatewayError::InvalidJSON("Invalid tool arguments JSON".to_string())
                    })?;
                    contents.push(KrynethContent::ToolCall {
                        id: tc.id,
                        name: tc.function.name,
                        arguments: args,
                    });
                }
            }

            if role == KrynethRole::Tool {
                if let Some(tool_id) = msg.tool_call_id {
                    let mut result_text = String::new();
                    for c in &contents {
                        if let KrynethContent::Text { text } = c {
                            result_text.push_str(text);
                        }
                    }
                    contents.clear();
                    contents.push(KrynethContent::ToolResult {
                        tool_id,
                        content: result_text,
                    });
                }
            }

            if !contents.is_empty() {
                kryneth_messages.push(KrynethMessage {
                    role,
                    content: contents,
                });
            }
        }

        Ok(KrynethConversation {
            system_prompt,
            messages: kryneth_messages,
            tools: req.tools,
            temperature: req.temperature,
            top_p: req.top_p,
            max_tokens: req.max_tokens,
            stop: req.stop,
            stream: req.stream,
            model: req.model,
        })
    }

    fn from_universal(
        &self,
        conv: &KrynethConversation,
    ) -> Result<serde_json::Value, GatewayError> {
        let mut messages = Vec::new();

        if let Some(ref sp) = conv.system_prompt {
            messages.push(OpenAIMessage {
                role: "system".to_string(),
                content: Some(serde_json::Value::String(sp.clone())),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        for msg in &conv.messages {
            let role_str = match msg.role {
                KrynethRole::System => "system",
                KrynethRole::User => "user",
                KrynethRole::Assistant => "assistant",
                KrynethRole::Tool => "tool",
            };

            let mut text_content = String::new();
            let mut tool_calls = Vec::new();
            let mut tool_call_id = None;

            for content in &msg.content {
                match content {
                    KrynethContent::Text { text } => {
                        if !text_content.is_empty() {
                            text_content.push('\n');
                        }
                        text_content.push_str(text);
                    }
                    KrynethContent::ImageUrl { url: _ } => {}
                    KrynethContent::ToolCall {
                        id,
                        name,
                        arguments,
                    } => {
                        tool_calls.push(OpenAIToolCall {
                            id: id.clone(),
                            r#type: "function".to_string(),
                            function: OpenAIFunctionCall {
                                name: name.clone(),
                                arguments: arguments.to_string(),
                            },
                        });
                    }
                    KrynethContent::ToolResult {
                        tool_id,
                        content: res_content,
                    } => {
                        tool_call_id = Some(tool_id.clone());
                        if !text_content.is_empty() {
                            text_content.push('\n');
                        }
                        text_content.push_str(res_content);
                    }
                }
            }

            let final_content = if !text_content.is_empty() {
                Some(serde_json::Value::String(text_content))
            } else {
                None
            };

            messages.push(OpenAIMessage {
                role: role_str.to_string(),
                content: final_content,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
                tool_call_id,
            });
        }

        let req = OpenAIChatRequest {
            messages,
            tools: conv.tools.clone(),
            temperature: conv.temperature,
            top_p: conv.top_p,
            max_tokens: conv.max_tokens,
            stop: conv.stop.clone(),
            stream: conv.stream,
            model: conv.model.clone(),
        };

        serde_json::to_value(req)
            .map_err(|e| GatewayError::ResponseBuild(format!("OpenAI serialize error: {e}")))
    }

    fn unify_response(&self, raw: serde_json::Value) -> Result<StandardResponse, GatewayError> {
        Ok(raw)
    }

    fn unify_stream_chunk(&self, chunk: String) -> Result<StandardStreamChunk, GatewayError> {
        Ok(chunk)
    }
}

fn parse_openai_tool_call(
    val: &simd_json::BorrowedValue<'_>,
) -> Result<Option<UniversalToolCall>, GatewayError> {
    let call_id = match val.get("id").and_then(|i| i.as_str()) {
        Some(id) => id,
        None => return Ok(None),
    };

    let func = match val.get("function") {
        Some(f) => f,
        None => {
            return Err(GatewayError::InvalidJSON(
                "Missing 'function' object in tool call".into(),
            ))
        }
    };

    let tool_name = func.get("name").and_then(|n| n.as_str()).ok_or_else(|| {
        GatewayError::InvalidJSON("Missing 'name' in tool call function object".into())
    })?;

    let arguments = match func.get("arguments") {
        Some(simd_json::BorrowedValue::String(s)) => s.to_string(),
        Some(other) => serde_json::to_string(other).map_err(|e| {
            GatewayError::InvalidJSON(format!("Failed to serialize arguments: {e}"))
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
    fn test_extract_calls_openai_standard_response() {
        let plugin = OpenAIPlugin::new();
        let payload = json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_abc123",
                                "type": "function",
                                "function": {
                                    "name": "get_current_weather",
                                    "arguments": "{\"location\": \"San Francisco, CA\"}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        });

        let raw_bytes = serde_json::to_vec(&payload).unwrap();
        let calls = plugin
            .extract_calls(&raw_bytes)
            .expect("Should extract tool calls");

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "call_abc123");
        assert_eq!(calls[0].tool_name, "get_current_weather");
        assert_eq!(calls[0].arguments, "{\"location\": \"San Francisco, CA\"}");
    }

    #[test]
    fn test_extract_calls_direct_array() {
        let plugin = OpenAIPlugin::new();
        let payload = json!([
            {
                "id": "call_999",
                "type": "function",
                "function": {
                    "name": "calculate",
                    "arguments": "{\"expr\": \"2+2\"}"
                }
            }
        ]);

        let raw_bytes = serde_json::to_vec(&payload).unwrap();
        let calls = plugin
            .extract_calls(&raw_bytes)
            .expect("Should extract tool calls");

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "call_999");
        assert_eq!(calls[0].tool_name, "calculate");
    }

    #[test]
    fn test_extract_calls_invalid_json() {
        let plugin = OpenAIPlugin::new();
        let raw_bytes = b"invalid payload";

        let result = plugin.extract_calls(raw_bytes);
        assert!(matches!(result, Err(GatewayError::InvalidJSON(_))));
    }

    #[test]
    fn test_extract_calls_missing_function_name() {
        let plugin = OpenAIPlugin::new();
        let payload = json!({
            "tool_calls": [
                {
                    "id": "call_123",
                    "function": {}
                }
            ]
        });

        let raw_bytes = serde_json::to_vec(&payload).unwrap();
        let result = plugin.extract_calls(&raw_bytes);
        assert!(matches!(result, Err(GatewayError::InvalidJSON(_))));
    }

    #[test]
    fn test_format_results() {
        let plugin = OpenAIPlugin::new();
        let results = vec![UniversalToolResult {
            call_id: "call_abc123".into(),
            status: ToolExecutionStatus::Success,
            content: "72F".into(),
            latency_ms: 50,
        }];

        let formatted = plugin
            .format_results(&results)
            .expect("Should format results");

        assert!(formatted.is_array());
        let arr = formatted.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["role"], "tool");
        assert_eq!(arr[0]["tool_call_id"], "call_abc123");
        assert_eq!(arr[0]["content"], "72F");
    }
}
