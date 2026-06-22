use crate::domain::models::{
    KrynethContent, KrynethConversation, KrynethMessage, KrynethRole, StandardResponse,
    StandardStreamChunk,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Translation error: {0}")]
    TranslationError(String),
}

pub trait BaseTranslator: Send + Sync {
    fn to_universal(&self, payload: &mut [u8]) -> Result<KrynethConversation, AppError>;
    #[allow(clippy::wrong_self_convention)]
    fn from_universal(&self, conv: &KrynethConversation) -> Result<Value, AppError>;
    fn unify_response(&self, raw: Value) -> Result<StandardResponse, AppError>;
    fn unify_stream_chunk(&self, chunk: String) -> Result<StandardStreamChunk, AppError>;
}

// ----------------------------------------------------------------------------
// OpenAI Structs
// ----------------------------------------------------------------------------
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
    pub r#type: String, // typically "function"
    pub function: OpenAIFunctionCall,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String, // usually a JSON string
}

pub struct OpenAiTranslator;

impl BaseTranslator for OpenAiTranslator {
    fn to_universal(&self, payload: &mut [u8]) -> Result<KrynethConversation, AppError> {
        let req: OpenAIChatRequest = simd_json::from_slice(payload)
            .map_err(|e| AppError::TranslationError(format!("OpenAI parse error: {}", e)))?;

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
                    return Err(AppError::TranslationError(format!(
                        "Unknown OpenAI role: {}",
                        other
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
                        AppError::TranslationError("Invalid tool arguments JSON".to_string())
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

    fn from_universal(&self, conv: &KrynethConversation) -> Result<Value, AppError> {
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
                    KrynethContent::ImageUrl { url: _ } => {
                        // Normally this would be a content array in OpenAI format
                    }
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
            .map_err(|e| AppError::TranslationError(format!("OpenAI serialize error: {}", e)))
    }

    fn unify_response(&self, raw: Value) -> Result<StandardResponse, AppError> {
        Ok(raw) // Already OpenAI compatible
    }

    fn unify_stream_chunk(&self, chunk: String) -> Result<StandardStreamChunk, AppError> {
        Ok(chunk) // Already OpenAI compatible
    }
}

// ----------------------------------------------------------------------------
// Anthropic Structs
// ----------------------------------------------------------------------------
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
    pub role: String, // "user" or "assistant"
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

pub struct AnthropicTranslator;

impl BaseTranslator for AnthropicTranslator {
    fn to_universal(&self, _payload: &mut [u8]) -> Result<KrynethConversation, AppError> {
        Err(AppError::TranslationError(
            "Anthropic to_universal not fully implemented".to_string(),
        ))
    }

    fn from_universal(&self, conv: &KrynethConversation) -> Result<Value, AppError> {
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

        let mapped_tools =
            crate::infrastructure::schema_mapper::map_openai_tools_to_anthropic(conv.tools.clone());

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
            .map_err(|e| AppError::TranslationError(format!("Anthropic serialize error: {}", e)))
    }

    fn unify_response(&self, raw: Value) -> Result<StandardResponse, AppError> {
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
            .and_then(|arr| {
                crate::infrastructure::schema_mapper::map_anthropic_tool_use_to_openai_calls(arr)
            });

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

        let mut message_obj = serde_json::json!({
            "role": "assistant",
            "content": text_content
        });

        if let Some(tool_calls) = mapped_tool_calls {
            message_obj["tool_calls"] = serde_json::json!(tool_calls);
        }

        let mock_openai_resp = serde_json::json!({
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

    fn unify_stream_chunk(&self, chunk: String) -> Result<StandardStreamChunk, AppError> {
        if !chunk.starts_with("data: ") || chunk == "data: [DONE]" {
            return Ok(chunk);
        }

        let json_str = chunk.trim_start_matches("data: ").trim();
        if json_str.is_empty() {
            return Ok(chunk);
        }

        let val: Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => return Ok("".to_string()), // Ignore malformed chunks
        };

        let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if event_type == "content_block_delta" {
            let text = val
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let mock_openai_chunk = serde_json::json!({
                "id": "chatcmpl-stream",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "claude",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "content": text
                    },
                    "finish_reason": serde_json::Value::Null
                }]
            });
            let chunk_str = serde_json::to_string(&mock_openai_chunk).map_err(|e| {
                AppError::TranslationError(format!("Failed to serialize chunk: {}", e))
            })?;
            return Ok(format!("data: {}", chunk_str));
        }

        if event_type == "message_stop" {
            return Ok("data: [DONE]".to_string());
        }

        Ok("".to_string())
    }
}

pub struct GeminiTranslator;

impl BaseTranslator for GeminiTranslator {
    fn to_universal(&self, _payload: &mut [u8]) -> Result<KrynethConversation, AppError> {
        Err(AppError::TranslationError(
            "Gemini to_universal not fully implemented".to_string(),
        ))
    }

    fn from_universal(&self, conv: &KrynethConversation) -> Result<Value, AppError> {
        let mut contents = Vec::new();

        for msg in &conv.messages {
            let role = match msg.role {
                KrynethRole::System | KrynethRole::User => "user",
                KrynethRole::Assistant | KrynethRole::Tool => "model",
            };

            let mut parts = Vec::new();
            for c in &msg.content {
                if let KrynethContent::Text { text } = c {
                    parts.push(serde_json::json!({ "text": text }));
                }
            }

            contents.push(serde_json::json!({
                "role": role,
                "parts": parts
            }));
        }

        let mut req = serde_json::json!({
            "contents": contents,
        });

        if let Some(sp) = &conv.system_prompt {
            req["systemInstruction"] = serde_json::json!({
                "parts": [{ "text": sp }]
            });
        }

        let mut generation_config = serde_json::Map::new();
        if let Some(t) = conv.temperature {
            generation_config.insert("temperature".to_string(), serde_json::json!(t));
        }
        if let Some(tp) = conv.top_p {
            generation_config.insert("topP".to_string(), serde_json::json!(tp));
        }
        if let Some(mt) = conv.max_tokens {
            generation_config.insert("maxOutputTokens".to_string(), serde_json::json!(mt));
        }
        if let Some(stop) = &conv.stop {
            generation_config.insert("stopSequences".to_string(), serde_json::json!(stop));
        }

        if !generation_config.is_empty() {
            req["generationConfig"] = serde_json::Value::Object(generation_config);
        }

        Ok(req)
    }

    fn unify_response(&self, raw: Value) -> Result<StandardResponse, AppError> {
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

        let mock_openai_resp = serde_json::json!({
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

    fn unify_stream_chunk(&self, chunk: String) -> Result<StandardStreamChunk, AppError> {
        if chunk.starts_with("data: ") {
            return Ok(chunk);
        }
        Ok("".to_string())
    }
}
