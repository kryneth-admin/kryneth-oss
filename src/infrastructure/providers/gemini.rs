//! gemini.rs — UniversalProviderAdapter implementation for Google Gemini API.

use crate::domain::models::{
    KrynethContent, KrynethConversation, KrynethRole, StandardResponse, StandardStreamChunk,
};
use crate::domain::utp_models::{UniversalToolCall, UniversalToolResult};
use crate::error::GatewayError;
use crate::infrastructure::providers::traits::UniversalProviderAdapter;
use serde_json::json;
use simd_json::prelude::*;

/// Adapter plugin for translating Google Gemini format function calls and responses to/from UTP.
#[derive(Debug, Clone, Default)]
pub struct GeminiPlugin;

impl GeminiPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl UniversalProviderAdapter for GeminiPlugin {
    fn extract_calls(&self, raw_payload: &[u8]) -> Result<Vec<UniversalToolCall>, GatewayError> {
        let mut buffer = raw_payload.to_vec();
        let value = simd_json::to_borrowed_value(&mut buffer)
            .map_err(|e| GatewayError::InvalidJSON(e.to_string()))?;

        let mut tool_calls = Vec::new();

        if let Some(candidates) = value.get("candidates").and_then(|c| c.as_array()) {
            if let Some(first_cand) = candidates.first() {
                if let Some(parts) = first_cand
                    .get("content")
                    .and_then(|c| c.get("parts"))
                    .and_then(|p| p.as_array())
                {
                    for part in parts {
                        if let Some(call) = parse_gemini_part(part)? {
                            tool_calls.push(call);
                        }
                    }
                    return Ok(tool_calls);
                }
            }
        }

        if let Some(parts) = value
            .get("parts")
            .or_else(|| value.get("content").and_then(|c| c.get("parts")))
            .and_then(|p| p.as_array())
        {
            for part in parts {
                if let Some(call) = parse_gemini_part(part)? {
                    tool_calls.push(call);
                }
            }
            return Ok(tool_calls);
        }

        if let simd_json::BorrowedValue::Array(arr) = &value {
            for item in arr {
                if let Some(call) = parse_gemini_part(item)? {
                    tool_calls.push(call);
                }
            }
            return Ok(tool_calls);
        }

        if let Some(call) = parse_gemini_part(&value)? {
            tool_calls.push(call);
        }

        Ok(tool_calls)
    }

    fn format_results(
        &self,
        results: &[UniversalToolResult],
    ) -> Result<serde_json::Value, GatewayError> {
        let mut responses = Vec::with_capacity(results.len());

        for res in results {
            let part = json!({
                "functionResponse": {
                    "name": res.call_id,
                    "response": {
                        "name": res.call_id,
                        "content": res.content
                    }
                }
            });
            responses.push(part);
        }

        Ok(serde_json::Value::Array(responses))
    }

    fn to_universal(&self, _payload: &mut [u8]) -> Result<KrynethConversation, GatewayError> {
        Err(GatewayError::ResponseBuild(
            "Gemini to_universal not fully implemented".to_string(),
        ))
    }

    fn from_universal(
        &self,
        conv: &KrynethConversation,
    ) -> Result<serde_json::Value, GatewayError> {
        let mut contents = Vec::new();

        for msg in &conv.messages {
            let role = match msg.role {
                KrynethRole::System | KrynethRole::User => "user",
                KrynethRole::Assistant | KrynethRole::Tool => "model",
            };

            let mut parts = Vec::new();
            for c in &msg.content {
                if let KrynethContent::Text { text } = c {
                    parts.push(json!({ "text": text }));
                }
            }

            contents.push(json!({
                "role": role,
                "parts": parts
            }));
        }

        let mut req = json!({
            "contents": contents,
        });

        if let Some(sp) = &conv.system_prompt {
            req["systemInstruction"] = json!({
                "parts": [{ "text": sp }]
            });
        }

        let mut generation_config = serde_json::Map::new();
        if let Some(t) = conv.temperature {
            generation_config.insert("temperature".to_string(), json!(t));
        }
        if let Some(tp) = conv.top_p {
            generation_config.insert("topP".to_string(), json!(tp));
        }
        if let Some(mt) = conv.max_tokens {
            generation_config.insert("maxOutputTokens".to_string(), json!(mt));
        }
        if let Some(stop) = &conv.stop {
            generation_config.insert("stopSequences".to_string(), json!(stop));
        }

        if !generation_config.is_empty() {
            req["generationConfig"] = serde_json::Value::Object(generation_config);
        }

        Ok(req)
    }

    fn unify_response(&self, raw: serde_json::Value) -> Result<StandardResponse, GatewayError> {
        let text_content = raw
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
            .and_then(|arr| arr.first())
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        let mock_openai_resp = json!({
            "id": "chatcmpl-gemini",
            "object": "chat.completion",
            "created": 0,
            "model": "gemini",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": text_content
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 0,
                "completion_tokens": 0,
                "total_tokens": 0
            }
        });

        Ok(mock_openai_resp)
    }

    fn unify_stream_chunk(&self, chunk: String) -> Result<StandardStreamChunk, GatewayError> {
        if chunk.starts_with("data: ") {
            return Ok(chunk);
        }
        Ok("".to_string())
    }
}

fn parse_gemini_part(
    val: &simd_json::BorrowedValue<'_>,
) -> Result<Option<UniversalToolCall>, GatewayError> {
    let func_call = match val.get("functionCall") {
        Some(fc) => fc,
        None => return Ok(None),
    };

    let tool_name = func_call
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| {
            GatewayError::InvalidJSON("Missing 'name' field in Gemini functionCall".into())
        })?;

    let call_id = func_call
        .get("id")
        .and_then(|i| i.as_str())
        .or_else(|| val.get("id").and_then(|i| i.as_str()))
        .unwrap_or(tool_name);

    let arguments = match func_call.get("args") {
        Some(simd_json::BorrowedValue::String(s)) => s.to_string(),
        Some(other) => serde_json::to_string(other).map_err(|e| {
            GatewayError::InvalidJSON(format!("Failed to serialize Gemini args: {e}"))
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
    fn test_extract_calls_gemini_candidate() {
        let plugin = GeminiPlugin::new();
        let payload = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "name": "get_weather",
                                    "args": {
                                        "location": "Boston, MA"
                                    }
                                }
                            }
                        ]
                    }
                }
            ]
        });

        let raw_bytes = serde_json::to_vec(&payload).unwrap();
        let calls = plugin
            .extract_calls(&raw_bytes)
            .expect("Should extract functionCall");

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_name, "get_weather");
        assert_eq!(calls[0].call_id, "get_weather");

        let args: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["location"], "Boston, MA");
    }

    #[test]
    fn test_extract_calls_invalid_json() {
        let plugin = GeminiPlugin::new();
        let raw_bytes = b"bad payload";

        let result = plugin.extract_calls(raw_bytes);
        assert!(matches!(result, Err(GatewayError::InvalidJSON(_))));
    }

    #[test]
    fn test_format_results() {
        let plugin = GeminiPlugin::new();
        let results = vec![UniversalToolResult {
            call_id: "get_weather".into(),
            status: ToolExecutionStatus::Success,
            content: "Cloudy".into(),
            latency_ms: 100,
        }];

        let formatted = plugin
            .format_results(&results)
            .expect("Should format results");

        assert!(formatted.is_array());
        let arr = formatted.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["functionResponse"]["name"], "get_weather");
        assert_eq!(arr[0]["functionResponse"]["response"]["content"], "Cloudy");
    }
}
