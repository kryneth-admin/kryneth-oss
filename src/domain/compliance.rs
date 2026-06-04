//! compliance.rs — Domain models for Phase 8: Autonomous Compliance Engine.
//!
//! Includes structs for Compliance Configuration, Data Residency Rules,
//! OPA (Open Policy Agent) payloads, and the Immutable Audit Log schema.

use serde::{Deserialize, Serialize};

// ── 1. Compliance Configuration ─────────────────────────────────────────────

/// Determines the strictness and active rules for PII redaction and policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompliancePolicy {
    /// E.g., "GDPR", "HIPAA", "DPDP"
    pub active_frameworks: Vec<String>,
    
    /// True if PII detection should run before forwarding requests
    pub enable_pii_redaction: bool,
    
    /// Specific PII entities to redact (e.g., ["CREDIT_CARD", "SSN"])
    pub target_entities: Vec<String>,
    
    /// If true, blocks requests when OPA or PII engine is unreachable (fail closed)
    pub fail_closed: bool,
}

// ── 2. Data Residency Routing Rules ─────────────────────────────────────────

/// Rules mapping regions/geographies to specific upstream LLM endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidencyRule {
    /// Client region or IP mask (e.g., "EU", "US")
    pub region: String,
    
    /// Base URL of the regionally-compliant LLM endpoint
    pub regional_endpoint: String,
    
    /// Whether data is allowed to leave this region
    pub strict_isolation: bool,
}

// ── 3. OPA Request/Response Payloads ────────────────────────────────────────

/// The payload sent to Open Policy Agent for evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpaRequestPayload {
    pub input: OpaInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpaInput {
    pub trace_id: String,
    pub tenant_id: String,
    pub user_region: String,
    pub model_requested: String,
    pub active_frameworks: Vec<String>,
}

/// The evaluation result returned by Open Policy Agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpaResponsePayload {
    pub result: OpaResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpaResult {
    /// If false, the request violates policy and must be blocked (HTTP 403)
    pub allow: bool,
    
    /// The reason for blocking, if `allow` is false
    pub block_reason: Option<String>,
}

// ── 4. Immutable Audit Log Schema ───────────────────────────────────────────

/// Represents a single append-only audit event for compliance telemetry.
/// PII is strictly excluded from this struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceAuditRecord {
    pub trace_id: String,
    pub tenant_id: String,
    
    /// The timestamp of the compliance check (ISO 8601)
    pub timestamp: String,
    
    /// Final policy decision (true = allowed, false = blocked)
    pub policy_result: bool,
    
    /// Number of distinct PII entities redacted from the prompt
    pub redacted_entity_count: u32,
    
    /// The resolved destination region based on ResidencyRule (e.g., "EU")
    pub destination_region: String,
    
    /// Optional field showing the reason for a block
    pub block_reason: Option<String>,
}
