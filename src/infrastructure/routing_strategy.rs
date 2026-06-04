//! infrastructure/routing_strategy.rs — Advanced LLM routing strategy engine.
//!
//! Provides intelligent routing strategies.
//! Phase 2 supersedes some of these with strict lock-free priority loops,
//! but the strategy enum is preserved for API backwards compatibility.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStrategy {
    Default,
    LeastLatency,
    CostOptimized,
}

impl RoutingStrategy {
    pub fn from_header(value: Option<&str>) -> Self {
        match value {
            Some("least_latency") => Self::LeastLatency,
            Some("cost_optimized") => Self::CostOptimized,
            _ => Self::Default,
        }
    }
}
