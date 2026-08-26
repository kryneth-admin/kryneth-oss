//! providers module — UTP Provider Plugins

pub mod anthropic;
pub mod bedrock;
pub mod cohere;
pub mod gemini;
pub mod openai;
pub mod traits;

pub use anthropic::AnthropicPlugin;
pub use bedrock::BedrockPlugin;
pub use cohere::CoherePlugin;
pub use gemini::GeminiPlugin;
pub use openai::OpenAIPlugin;
pub use traits::UniversalProviderAdapter;

use crate::infrastructure::llm_router::SchemaFormat;

/// Factory function that returns a provider-specific UniversalProviderAdapter implementation
/// based on the target SchemaFormat.
pub fn get_utp_adapter(schema: &SchemaFormat) -> Box<dyn UniversalProviderAdapter> {
    match schema {
        SchemaFormat::OpenAI => Box::new(openai::OpenAIPlugin::new()),
        SchemaFormat::Anthropic => Box::new(anthropic::AnthropicPlugin::new()),
        SchemaFormat::Gemini => Box::new(gemini::GeminiPlugin::new()),
        SchemaFormat::Cohere => Box::new(cohere::CoherePlugin::new()),
    }
}
