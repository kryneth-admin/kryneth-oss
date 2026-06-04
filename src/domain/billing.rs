//! src/domain/billing.rs — 2026 Official Provider Pricing Registry & Models.
//!
//! Exposes pricing types and standard rates for premier and open-source models,
//! incorporating defensive fallback structures (Samalikkira Buffer).

use serde::{Deserialize, Serialize};

/// Identifies the pricing model utilized by a specific upstream LLM route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingType {
    /// Purely token volume based pricing (e.g., input/output counts).
    TokenBased,
    /// Perplexity-style: input/output token volume pricing plus a flat per-request buffer.
    Hybrid,
    /// Fixed cost per single invocation regardless of token length.
    FixedRequest,
}

/// Represents the unit pricing structure for an LLM provider model.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PrecautionaryRate {
    pub pricing_type: PricingType,
    /// Precautionary input token cost per 1,000,000 (1M) tokens.
    pub input_cost_per_1m: f64,
    /// Precautionary output token cost per 1,000,000 (1M) tokens.
    pub output_cost_per_1m: f64,
    /// A flat buffer fee added per request (for Hybrid systems).
    pub per_request_buffer: f64,
}

/// Core function matching incoming provider & target model pairs to their official 2026 rates.
/// Uses robust matching to support sub-strings, wildcard scenarios, and zero-day unlisted models.
pub fn lookup_precautionary_rate(provider: &str, target_model: &str) -> PrecautionaryRate {
    let provider_lower = provider.to_lowercase();
    let model_lower = target_model.to_lowercase();

    match provider_lower.as_str() {
        "openai" => {
            if model_lower.contains("gpt-4o-mini") {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 0.150,
                    output_cost_per_1m: 0.600,
                    per_request_buffer: 0.0,
                }
            } else if model_lower.contains("gpt-4o") {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 5.00,
                    output_cost_per_1m: 15.00,
                    per_request_buffer: 0.0,
                }
            } else if model_lower.contains("o1") {
                // OpenAI o1 and o1-mini: reasoning tokens billed as output tokens.
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 15.00,
                    output_cost_per_1m: 60.00,
                    per_request_buffer: 0.0,
                }
            } else {
                // Defensive ceiling for premier unlisted variants
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 20.00,
                    output_cost_per_1m: 60.00,
                    per_request_buffer: 0.0,
                }
            }
        }
        "anthropic" => {
            if model_lower.contains("claude-3-5-sonnet") || model_lower.contains("3.5-sonnet") {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 3.00,
                    output_cost_per_1m: 15.00,
                    per_request_buffer: 0.0,
                }
            } else if model_lower.contains("claude-3-5-haiku") || model_lower.contains("3.5-haiku")
            {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 0.25,
                    output_cost_per_1m: 1.25,
                    per_request_buffer: 0.0,
                }
            } else if model_lower.contains("claude-3-opus") || model_lower.contains("opus") {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 15.00,
                    output_cost_per_1m: 75.00,
                    per_request_buffer: 0.0,
                }
            } else {
                // Defensive ceiling for premier unlisted variants
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 20.00,
                    output_cost_per_1m: 60.00,
                    per_request_buffer: 0.0,
                }
            }
        }
        "deepseek" => {
            if model_lower.contains("deepseek-v3") || model_lower.contains("deepseek-chat") {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 0.14,
                    output_cost_per_1m: 0.28,
                    per_request_buffer: 0.0,
                }
            } else if model_lower.contains("deepseek-r1") {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 0.55,
                    output_cost_per_1m: 2.19,
                    per_request_buffer: 0.0,
                }
            } else {
                // Default fallback for unlisted DeepSeek variants (using open-source host buffer)
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 0.50,
                    output_cost_per_1m: 1.50,
                    per_request_buffer: 0.0,
                }
            }
        }
        "grok" | "xai" => {
            if model_lower.contains("grok-2") || model_lower.contains("grok-beta") {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 2.00,
                    output_cost_per_1m: 10.00,
                    per_request_buffer: 0.0,
                }
            } else {
                // Defensive ceiling for premier unlisted variants
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 20.00,
                    output_cost_per_1m: 60.00,
                    per_request_buffer: 0.0,
                }
            }
        }
        "perplexity" => {
            #[allow(clippy::if_same_then_else)]
            if model_lower.contains("sonar-reasoning")
                || model_lower.contains("sonar-deep-research")
            {
                PrecautionaryRate {
                    pricing_type: PricingType::Hybrid,
                    input_cost_per_1m: 5.00,
                    output_cost_per_1m: 15.00,
                    per_request_buffer: 0.05,
                }
            } else {
                PrecautionaryRate {
                    pricing_type: PricingType::Hybrid,
                    input_cost_per_1m: 5.00,
                    output_cost_per_1m: 15.00,
                    per_request_buffer: 0.05,
                }
            }
        }
        "google" | "gemini" => {
            if model_lower.contains("gemini-1.5-flash") {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 0.075,
                    output_cost_per_1m: 0.30,
                    per_request_buffer: 0.0,
                }
            } else if model_lower.contains("gemini-1.5-pro") {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 1.25,
                    output_cost_per_1m: 5.00,
                    per_request_buffer: 0.0,
                }
            } else {
                // Defensive ceiling for premier unlisted variants
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 20.00,
                    output_cost_per_1m: 60.00,
                    per_request_buffer: 0.0,
                }
            }
        }
        "groq" => {
            if model_lower.contains("llama")
                || model_lower.contains("mixtral")
                || model_lower.contains("gemma")
            {
                // Standard developer free tier rates when not bypassed
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 0.0,
                    output_cost_per_1m: 0.0,
                    per_request_buffer: 0.0,
                }
            } else {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 0.50,
                    output_cost_per_1m: 1.50,
                    per_request_buffer: 0.0,
                }
            }
        }
        _ => {
            // Defensive Fallback System (Samalikkira Buffer)
            if provider_lower.contains("groq")
                || provider_lower.contains("qwen")
                || provider_lower.contains("sarvam")
            {
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 0.50,
                    output_cost_per_1m: 1.50,
                    per_request_buffer: 0.0,
                }
            } else {
                // Premier unlisted variant ceiling
                PrecautionaryRate {
                    pricing_type: PricingType::TokenBased,
                    input_cost_per_1m: 20.00,
                    output_cost_per_1m: 60.00,
                    per_request_buffer: 0.0,
                }
            }
        }
    }
}
