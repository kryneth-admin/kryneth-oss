use serde_json::{json, Value};

/// Maps an OpenAI format tools array to an Anthropic tools array.
/// Returns None if the input array is empty or if no valid tools were mapped.
pub fn map_openai_tools_to_anthropic(openai_tools: Option<Vec<Value>>) -> Option<Vec<Value>> {
    let tools = openai_tools?;
    let mut mapped_tools = Vec::with_capacity(tools.len());

    for tool in tools {
        // Only process tools of type "function"
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

/// Maps an Anthropic message content blocks array (containing tool uses)
/// into an OpenAI format tool_calls array.
/// Returns None if no `tool_use` blocks are found.
pub fn map_anthropic_tool_use_to_openai_calls(
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
                // Must stringify the input object into the OpenAI arguments field
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_map_openai_tools_to_anthropic() {
        let openai_tools = vec![json!({
            "type": "function",
            "function": {
                "name": "get_current_weather",
                "description": "Get the current weather in a given location",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {
                            "type": "string",
                            "description": "The city and state, e.g. San Francisco, CA"
                        }
                    },
                    "required": ["location"]
                }
            }
        })];

        let result =
            map_openai_tools_to_anthropic(Some(openai_tools)).expect("Should return mapped tools");
        assert_eq!(result.len(), 1);

        let mapped = &result[0];
        assert_eq!(mapped["name"], "get_current_weather");
        assert_eq!(
            mapped["description"],
            "Get the current weather in a given location"
        );
        assert_eq!(mapped["input_schema"]["type"], "object");
        assert_eq!(mapped["input_schema"]["required"][0], "location");
    }

    #[test]
    fn test_map_openai_tools_to_anthropic_skips_invalid() {
        let openai_tools = vec![
            json!({"type": "other"}), // Should skip
            json!({
                "type": "function",
                "function": {} // Missing name, should skip
            }),
        ];

        let result = map_openai_tools_to_anthropic(Some(openai_tools));
        assert!(result.is_none());
    }

    #[test]
    fn test_map_anthropic_tool_use_to_openai_calls() {
        let anthropic_blocks = vec![
            json!({
                "type": "text",
                "text": "Sure, I can help with that."
            }),
            json!({
                "type": "tool_use",
                "id": "toolu_123",
                "name": "get_current_weather",
                "input": {
                    "location": "Boston, MA"
                }
            }),
        ];

        let result = map_anthropic_tool_use_to_openai_calls(anthropic_blocks)
            .expect("Should return valid OpenAI tool calls");
        assert_eq!(result.len(), 1);

        let mapped = &result[0];
        assert_eq!(mapped["id"], "toolu_123");
        assert_eq!(mapped["type"], "function");
        assert_eq!(mapped["function"]["name"], "get_current_weather");

        let args_str = mapped["function"]["arguments"]
            .as_str()
            .expect("Arguments should be a string");
        let parsed_args: serde_json::Value =
            serde_json::from_str(args_str).expect("Should form valid inner JSON");
        assert_eq!(parsed_args["location"], "Boston, MA");
    }

    #[test]
    fn test_map_anthropic_tool_use_returns_none_if_no_tools() {
        let anthropic_blocks = vec![json!({
            "type": "text",
            "text": "Just plain text."
        })];

        let result = map_anthropic_tool_use_to_openai_calls(anthropic_blocks);
        assert!(result.is_none());
    }
}
