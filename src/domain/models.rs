use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::ports::{
    AuthPort, BillingPort, RateLimitPort, RoutingConfigPort, SemanticCachePort, TelemetryPort,
};
use std::sync::Arc;

/// Application-wide shared state passed to every handler via Axum's `State` extractor.
#[derive(Clone)]
pub struct AppState {
    pub http_client: reqwest::Client,
    pub compliance_url: String,
    pub rate_limit_max: u32,
    pub rate_limit_window: u32,
    pub dashboard_url: String,
    pub llm_api_base_url: Option<String>,
    // ── Dependency-inverted ports (OSS ↔ Enterprise swappable) ────────────────
    pub telemetry: Arc<dyn TelemetryPort>,
    pub billing: Arc<dyn BillingPort>,
    pub auth_resolver: Arc<dyn AuthPort>,
    /// Phase 3: Distributed token-bucket rate limiter.
    /// Enterprise: syncs local `rate_limit_cache` counters to Redis periodically.
    /// OSS: process-local only — no Redis required.
    pub rate_limiter: Arc<dyn RateLimitPort>,
    /// Phase 3: Routing-configuration hot-reload via Redis Pub/Sub.
    /// Enterprise: subscribes to `kryneth:routing_updates` from `kryneth_config`.
    /// OSS: no-op — routing state is static after boot.
    pub routing_config: Arc<dyn RoutingConfigPort>,
    /// Phase 3: L2 semantic cache (gRPC to `kryneth_cache:50051`).
    /// Enterprise: dispatches real gRPC lookup/store RPCs.
    /// OSS: always returns None (cache miss) — no tonic dependency.
    pub semantic_cache: Arc<dyn SemanticCachePort>,
    // ── Process-local, always-present infrastructure ───────────────────────────
    pub rate_limit_cache: std::sync::Arc<dashmap::DashMap<String, std::sync::Arc<LocalBucket>>>,
    pub l1_cache: std::sync::Arc<crate::infrastructure::l1_cache::L1Cache>,
    pub routing_state: std::sync::Arc<RoutingState>,
    /// Proactive circuit breaker: maps a failed key alias to () with a 60-second TTL.
    /// MEMORY BOUND: weigher enforces byte budget, not entry count.
    pub circuit_breaker: moka::future::Cache<String, ()>,
    /// Fallback L1 cache for agentic loop tracking when Redis is unavailable.
    /// MEMORY BOUND: weigher enforces byte budget.
    pub loop_fallback_cache:
        moka::future::Cache<String, std::sync::Arc<std::sync::atomic::AtomicU64>>,
    /// Tunnel 3 Phase 1: Lock-free MCP tool registry.
    pub mcp_registry: std::sync::Arc<crate::infrastructure::mcp_registry::McpConnectionRegistry>,
    /// Tunnel 3 Phase 2: Immutable MCP tool schema registry.
    pub tool_registry: std::sync::Arc<crate::usecases::tool_router::ToolRegistry>,
    /// OSS Agent Guardian cache for Tool Storms and Runaway Loops.
    pub agent_guardian_cache: moka::future::Cache<String, u32>,
    /// Ephemeral Admin Dashboard in-memory metrics.
    pub dashboard_metrics: std::sync::Arc<DashboardMetrics>,
    /// Dynamic pricing map for dynamic token & tool pricing rules
    pub pricing_map: std::sync::Arc<arc_swap::ArcSwap<std::collections::HashMap<String, crate::domain::billing::PrecautionaryRate>>>,
    /// Dynamic budgets mapped by scope identifier
    pub budget_map: std::sync::Arc<arc_swap::ArcSwap<std::collections::HashMap<String, ScopedBudget>>>,
}

#[derive(Debug)]
pub struct DashboardMetrics {
    pub total_requests: std::sync::atomic::AtomicUsize,
    pub total_latency_ms: std::sync::atomic::AtomicUsize,
    pub total_tokens: std::sync::atomic::AtomicUsize,
    pub blocked_agent_loops: std::sync::atomic::AtomicUsize,
}

impl Default for DashboardMetrics {
    fn default() -> Self {
        Self {
            total_requests: std::sync::atomic::AtomicUsize::new(0),
            total_latency_ms: std::sync::atomic::AtomicUsize::new(0),
            total_tokens: std::sync::atomic::AtomicUsize::new(0),
            blocked_agent_loops: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl DashboardMetrics {
    pub fn new() -> Self {
        Self::default()
    }
}

pub struct LocalBucket {
    pub consumed: std::sync::atomic::AtomicU32,
    pub is_blocked: std::sync::atomic::AtomicBool,
    pub capacity_rpm: std::sync::atomic::AtomicU32,
}

/// Trace context propagated through every request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    pub trace_id: String,
    pub session_id: String,
    pub parent_trace_id: Option<String>,
}

/// Strongly-typed payload for telemetry ingestion to guarantee valid JSON serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogPayload {
    pub id: String,
    pub trace_id: String,
    pub session_id: String,
    pub parent_trace_id: Option<String>,
    pub tenant_id: String,
    pub model: String,
    pub status: u16,
    pub latency_ms: u32,
    pub tokens: u32,
    pub total_tokens: u32,
    pub cache_hit: bool,
    pub prompt_content: String,
    pub response_content: String,
    pub error_message: String,
    pub requested_provider: String,
    pub executed_provider: String,
    pub is_hot_swapped: u8,
}

pub use crate::error::GatewayError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamTarget {
    pub priority: i32,
    pub weight: i32,
    pub api_key_alias: String,
    pub api_key: String,
    pub provider_name: String,
    pub base_url: String,
    pub target_model: String,
    pub schema_format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelConfig {
    pub targets: Vec<UpstreamTarget>,
    /// Per-route burst-capable rate limit (requests per minute).
    /// Published by `kryneth_config` in the Redis Pub/Sub payload.
    /// Falls back to `AppState::rate_limit_max` when `None` (legacy configs).
    #[serde(default)]
    pub rate_limit_rpm: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub tenant_id: String,
    pub pii_masking_enabled: bool,
    #[serde(alias = "semantic_caching_enabled")]
    pub semantic_cache_enabled: bool,
    pub routing_fallback_enabled: bool,
    pub rate_limit_rpm: Option<i32>,
    pub preferred_model: Option<String>,
    pub fallback_timeout_ms: i32,
    pub semantic_cache_threshold: f64,
    pub max_agent_loops: i32,
    pub max_identical_tool_calls: i32,
    pub context_window_budget: i32,
    pub burn_rate_limit: f64,
}

#[derive(Default)]
pub struct RoutingState {
    pub state: arc_swap::ArcSwap<
        std::collections::HashMap<String, std::collections::HashMap<String, ModelConfig>>,
    >,
    pub client_configs: arc_swap::ArcSwap<std::collections::HashMap<String, ClientConfig>>,
}

impl RoutingState {
    pub fn new() -> Self {
        Self {
            state: arc_swap::ArcSwap::from_pointee(std::collections::HashMap::new()),
            client_configs: arc_swap::ArcSwap::from_pointee(std::collections::HashMap::new()),
        }
    }
}

/// The role of the message sender in the Universal Middleman Schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KrynethRole {
    System,
    User,
    Assistant,
    Tool,
}

/// The content block within a message, supporting text, images, and tool invocations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KrynethContent {
    Text {
        text: String,
    },
    ImageUrl {
        url: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        tool_id: String,
        content: String,
    },
}

/// A specific message in the conversation history, associating a role with a list of content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KrynethMessage {
    pub role: KrynethRole,
    pub content: Vec<KrynethContent>,
}

/// The Universal Middleman Schema encompassing the whole conversation state for parsing on-the-fly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KrynethConversation {
    pub system_prompt: Option<String>,
    pub messages: Vec<KrynethMessage>,
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

pub type StandardResponse = serde_json::Value;
pub type StandardStreamChunk = String;

/// Scopes under which budgets can be configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetScopeType {
    Global,
    Tenant,
    Team,
    ApiKey,
    Route,
}

/// Dynamic hierarchical scoped budget definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedBudget {
    pub scope_type: BudgetScopeType,
    pub scope_id: String,
    pub limit_amount: f64,
    pub alert_80_enabled: bool,
    pub alert_100_projected_enabled: bool,
    pub emergency_kill_switch: bool,
}

// ==============================================================================
// Virtual API Key Phase 1: Multi-LLM Architecture Models
// ==============================================================================

/// Account type for tenant workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccountType {
    #[serde(alias = "individual", alias = "Solo", alias = "solo")]
    // Catch-all for legacy and cross-service payloads
    #[default]
    Solo,
    #[serde(alias = "Team", alias = "team")]
    Team,
    #[serde(alias = "Enterprise", alias = "enterprise")]
    Enterprise,
}

/// A tenant represents an organization or individual workspace.
/// All resources are scoped to a tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub is_active: bool,
    pub onboarding_status: bool,
    /// Account type: 'individual' or 'team'
    pub account_type: AccountType,
}

/// A virtual API key issued to tenant applications for gateway authentication.
/// The raw key is never stored; only a SHA-256 hash is persisted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: Uuid,
    pub tenant_id: Uuid,
    /// SHA-256 hash of the raw key (hex-encoded)
    pub key_hash: String,
    /// Human-readable name for the key (e.g., "Default Project", "Dev Key")
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub is_active: bool,
}

/// Supported LLM providers for the provider_keys table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderName {
    OpenAI,
    Anthropic,
    Gemini,
    Groq,
}

/// An encrypted upstream LLM provider API key.
/// Each tenant can store multiple provider keys for multi-LLM support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderKey {
    pub id: Uuid,
    pub tenant_id: Uuid,
    /// The LLM provider (e.g., 'openai', 'anthropic')
    pub provider_name: ProviderName,
    /// AES-256-GCM encrypted provider API key
    #[serde(skip_serializing)]
    pub encrypted_key: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_universal_schema_serialization() {
        let json_str = r#"{
            "system_prompt": "You are a helpful assistant.",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "Hello, what's in this image?"
                        },
                        {
                            "type": "image_url",
                            "url": "https://example.com/image.jpg"
                        }
                    ]
                },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_call",
                            "id": "call_123",
                            "name": "get_weather",
                            "arguments": { "location": "San Francisco" }
                        }
                    ]
                },
                {
                    "role": "tool",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_id": "call_123",
                            "content": "Sunny and 70 degrees"
                        }
                    ]
                }
            ],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "description": "Get current weather"
                    }
                }
            ]
        }"#;

        // Verify Deserialization from JSON String
        let conversation: KrynethConversation =
            serde_json::from_str(json_str).expect("Failed to deserialize Universal Schema JSON");

        assert_eq!(
            conversation.system_prompt.as_deref(),
            Some("You are a helpful assistant.")
        );
        assert_eq!(conversation.messages.len(), 3);
        assert_eq!(conversation.messages[0].role, KrynethRole::User);
        assert_eq!(conversation.messages[1].role, KrynethRole::Assistant);
        assert_eq!(conversation.messages[2].role, KrynethRole::Tool);

        // Verify Serialization back to JSON with no data loss
        let serialized_str =
            serde_json::to_string(&conversation).expect("Failed to serialize back to json");

        // Parse both as serde_json::Value to ignore whitespace formatting differences
        let original_val: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let serialized_val: serde_json::Value = serde_json::from_str(&serialized_str).unwrap();

        assert_eq!(
            original_val, serialized_val,
            "Serialized JSON did not match original cleanly"
        );
    }
}
