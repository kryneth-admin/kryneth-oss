//! utp_models.rs — Language-agnostic Universal Tool Protocol (UTP) domain models.

use serde::{Deserialize, Serialize};

/// Definition of a tool registered in the Universal Tool Protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UniversalToolDef {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub schema: serde_json::Value,
    pub strict_mode: bool,
}

/// Representation of an extracted tool call originating from an LLM response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UniversalToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: String,
}

/// Status of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolExecutionStatus {
    Success,
    Error,
}

/// Representation of a tool execution result to be formatted back to an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UniversalToolResult {
    pub call_id: String,
    pub status: ToolExecutionStatus,
    pub content: String,
    pub latency_ms: u64,
}
