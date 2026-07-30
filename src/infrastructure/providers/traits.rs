//! traits.rs — Core Universal Provider Adapter interface.

use crate::domain::models::{KrynethConversation, StandardResponse, StandardStreamChunk};
use crate::domain::utp_models::{UniversalToolCall, UniversalToolResult};
use crate::error::GatewayError;

/// Trait defining the contract for provider-specific tool adapters and payload translators.
pub trait UniversalProviderAdapter: Send + Sync {
    // ── Tool Protocol ────────────────────────────────────────────────────────
    /// Extracts UniversalToolCall items from a provider's raw response payload.
    fn extract_calls(&self, raw_payload: &[u8]) -> Result<Vec<UniversalToolCall>, GatewayError>;

    /// Formats UniversalToolResult items into provider-specific JSON content blocks.
    fn format_results(
        &self,
        results: &[UniversalToolResult],
    ) -> Result<serde_json::Value, GatewayError>;

    // ── Core Translation Protocol ────────────────────────────────────────────
    /// Converts a provider-specific request payload into a universal KrynethConversation.
    fn to_universal(&self, payload: &mut [u8]) -> Result<KrynethConversation, GatewayError>;

    /// Serializes a universal KrynethConversation into a provider-native JSON payload.
    #[allow(clippy::wrong_self_convention)]
    fn from_universal(&self, conv: &KrynethConversation)
        -> Result<serde_json::Value, GatewayError>;

    /// Unifies a provider-native response payload into standard OpenAI-compatible format.
    fn unify_response(&self, raw: serde_json::Value) -> Result<StandardResponse, GatewayError>;

    /// Unifies a provider-native SSE stream chunk into standard OpenAI-compatible format.
    fn unify_stream_chunk(&self, chunk: String) -> Result<StandardStreamChunk, GatewayError>;
}
