use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetryPolicy {
    Safe,
    Unsafe,
    Conditional,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SideEffectClass {
    ReadOnly,
    Reversible,
    Irreversible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdempotencyRequirement {
    None,
    Recommended,
    Required,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolExecutionPolicy {
    pub retry_policy: RetryPolicy,
    pub side_effect_class: SideEffectClass,
    pub idempotency_requirement: IdempotencyRequirement,
    pub timeout_seconds: u64,
}
