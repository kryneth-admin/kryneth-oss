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
    /// Flat fee charged per MCP tool call.
    pub mcp_flat_fee: f64,
    /// Markup multiplier applied to total cost when agent loop detected.
    pub agent_loop_multiplier: f64,
}

/// Core function matching incoming provider & target model pairs to their official 2026 rates.
///
/// This is the OSS static pricing table — the authoritative fallback that is used when:
/// - No dynamic `pricing_map` has been loaded from the admin DB / Redis yet (cold-start).
/// - The Enterprise config subscriber has not yet performed its first hot-swap.
/// - Running in OSS-only mode without a connected pricing database.
///
/// At proxy time, `AppState.pricing_map` takes priority over this function.
/// The dynamic engine calls this only when a lookup miss occurs in the live map.
///
/// ## Samalikkira Buffer (Defensive Ceiling)
/// Any zero-day or unknown model/provider defaults to the **Premier Ceiling**
/// ($20.00 input / $60.00 output per 1M) — i.e., we always over-estimate rather
/// than under-bill, preventing unintended free-riding by unlisted models.
pub fn lookup_precautionary_rate(provider: &str, target_model: &str) -> PrecautionaryRate {
    let p = provider.to_lowercase();
    let m = target_model.to_lowercase();

    // ── OpenAI ───────────────────────────────────────────────────────────────
    if p.contains("openai") {
        if m.contains("o1-mini") {
            return PrecautionaryRate {
                pricing_type: PricingType::TokenBased,
                input_cost_per_1m: 15.00,
                output_cost_per_1m: 60.00,
                per_request_buffer: 0.0,
                mcp_flat_fee: 0.0,
                agent_loop_multiplier: 1.0,
            };
        }
        if m.contains("o1") {
            return PrecautionaryRate {
                pricing_type: PricingType::TokenBased,
                input_cost_per_1m: 15.00,
                output_cost_per_1m: 60.00,
                per_request_buffer: 0.0,
                mcp_flat_fee: 0.0,
                agent_loop_multiplier: 1.0,
            };
        }
        if m.contains("gpt-4o-mini") {
            return PrecautionaryRate {
                pricing_type: PricingType::TokenBased,
                input_cost_per_1m: 0.15,
                output_cost_per_1m: 0.60,
                per_request_buffer: 0.0,
                mcp_flat_fee: 0.0,
                agent_loop_multiplier: 1.0,
            };
        }
        if m.contains("gpt-4o") || m.contains("gpt-4-turbo") {
            return PrecautionaryRate {
                pricing_type: PricingType::TokenBased,
                input_cost_per_1m: 5.00,
                output_cost_per_1m: 15.00,
                per_request_buffer: 0.0,
                mcp_flat_fee: 0.0,
                agent_loop_multiplier: 1.0,
            };
        }
    }

    // ── Anthropic ────────────────────────────────────────────────────────────
    if p.contains("anthropic") {
        if m.contains("claude-3-5-sonnet") || m.contains("claude-3.5-sonnet") {
            return PrecautionaryRate {
                pricing_type: PricingType::TokenBased,
                input_cost_per_1m: 3.00,
                output_cost_per_1m: 15.00,
                per_request_buffer: 0.0,
                mcp_flat_fee: 0.0,
                agent_loop_multiplier: 1.0,
            };
        }
        if m.contains("claude-3-opus") {
            return PrecautionaryRate {
                pricing_type: PricingType::TokenBased,
                input_cost_per_1m: 15.00,
                output_cost_per_1m: 75.00,
                per_request_buffer: 0.0,
                mcp_flat_fee: 0.0,
                agent_loop_multiplier: 1.0,
            };
        }
        if m.contains("claude-3-haiku") {
            return PrecautionaryRate {
                pricing_type: PricingType::TokenBased,
                input_cost_per_1m: 0.25,
                output_cost_per_1m: 1.25,
                per_request_buffer: 0.0,
                mcp_flat_fee: 0.0,
                agent_loop_multiplier: 1.0,
            };
        }
    }

    // ── Google / Gemini ───────────────────────────────────────────────────────
    if p.contains("google") || p.contains("gemini") {
        if m.contains("gemini-1.5-pro") {
            return PrecautionaryRate {
                pricing_type: PricingType::TokenBased,
                input_cost_per_1m: 3.50,
                output_cost_per_1m: 10.50,
                per_request_buffer: 0.0,
                mcp_flat_fee: 0.0,
                agent_loop_multiplier: 1.0,
            };
        }
        if m.contains("gemini-1.5-flash") || m.contains("gemini-flash") {
            return PrecautionaryRate {
                pricing_type: PricingType::TokenBased,
                input_cost_per_1m: 0.075,
                output_cost_per_1m: 0.30,
                per_request_buffer: 0.0,
                mcp_flat_fee: 0.0,
                agent_loop_multiplier: 1.0,
            };
        }
    }

    // ── DeepSeek ─────────────────────────────────────────────────────────────
    if p.contains("deepseek") {
        if m.contains("deepseek-r1") {
            return PrecautionaryRate {
                pricing_type: PricingType::TokenBased,
                input_cost_per_1m: 0.55,
                output_cost_per_1m: 2.19,
                per_request_buffer: 0.0,
                mcp_flat_fee: 0.0,
                agent_loop_multiplier: 1.0,
            };
        }
        // deepseek-chat / v3
        return PrecautionaryRate {
            pricing_type: PricingType::TokenBased,
            input_cost_per_1m: 0.14,
            output_cost_per_1m: 0.28,
            per_request_buffer: 0.0,
            mcp_flat_fee: 0.0,
            agent_loop_multiplier: 1.0,
        };
    }

    // ── Groq (Developer Free Tier) ────────────────────────────────────────────
    if p.contains("groq") {
        return PrecautionaryRate {
            pricing_type: PricingType::TokenBased,
            input_cost_per_1m: 0.0,
            output_cost_per_1m: 0.0,
            per_request_buffer: 0.0,
            mcp_flat_fee: 0.0,
            agent_loop_multiplier: 1.0,
        };
    }

    // ── Open-Source / Self-Hosted Buffer Ceiling ──────────────────────────────
    // Catches qwen, llama, mistral, mixtral, phi, falcon, etc.
    let is_open_source = m.contains("llama")
        || m.contains("mistral")
        || m.contains("mixtral")
        || m.contains("qwen")
        || m.contains("phi")
        || m.contains("falcon")
        || m.contains("gemma");

    if is_open_source {
        return PrecautionaryRate {
            pricing_type: PricingType::TokenBased,
            input_cost_per_1m: 0.50,
            output_cost_per_1m: 1.50,
            per_request_buffer: 0.0,
            mcp_flat_fee: 0.0,
            agent_loop_multiplier: 1.0,
        };
    }

    // ── Samalikkira Buffer: Premier Defensive Ceiling ─────────────────────────
    // Any unlisted/zero-day model defaults to the protective premier ceiling.
    // We always over-estimate cost to prevent unintended free usage of unknown models.
    let _ = (provider, target_model); // suppress unused warnings
    PrecautionaryRate {
        pricing_type: PricingType::TokenBased,
        input_cost_per_1m: 20.00,
        output_cost_per_1m: 60.00,
        per_request_buffer: 0.0,
        mcp_flat_fee: 0.0,
        agent_loop_multiplier: 1.0,
    }
}
