//! error.rs — Structured, typed gateway errors.
//!
//! ## Design Principles
//! - Every variant maps to exactly one HTTP status code via `IntoResponse`.
//! - Internal details (DB errors, SQL snippets) are logged server-side but
//!   NEVER surfaced in the client JSON response.
//! - `GatewayError::DatabaseError` absorbs all ClickHouse / Postgres failures.
//! - `GatewayError::TenantNotFound` maps to 404 — clients can distinguish
//!   "you don't exist" from "you lack permission" (403 Forbidden).

use thiserror::Error;

/// Comprehensive, structured error enum for the Kryneth Gateway.
/// All variants are `#[error(...)]` attributed for display; `IntoResponse`
/// maps each to a safe HTTP status + opaque error code.
#[derive(Debug, Error)]
pub enum GatewayError {
    // ── Auth & Access ────────────────────────────────────────────────────────
    #[error("Missing or invalid API key")]
    Unauthorized,

    /// Returned when a JWT-authenticated user calls an Admin-only endpoint.
    #[error("Insufficient permissions")]
    Forbidden,

    // ── Resource Errors ──────────────────────────────────────────────────────
    /// Tenant UUID was validated but no matching tenant row exists.
    #[error("Tenant not found")]
    TenantNotFound,

    /// Tenant ID string is not a valid UUID.
    #[error("Invalid tenant ID format")]
    InvalidTenantId,

    // ── Infrastructure ───────────────────────────────────────────────────────
    /// ClickHouse or Postgres query failure. Internal details are logged;
    /// the client receives only the opaque `DATABASE_ERROR` code.
    #[error("Database error: {0}")]
    DatabaseError(String),

    /// HTTP response construction failure (header value encoding, etc.).
    #[error("Failed to build proxy request/response: {0}")]
    ResponseBuild(String),

    #[error("Invalid JSON payload: {0}")]
    InvalidJSON(String),

    #[error("Missing model parameter in request")]
    MissingModel,

    /// reqwest error with timeout discrimination.
    #[error("LLM Provider unreachable: {0}")]
    UpstreamUnreachable(reqwest::Error),

    /// Generic reqwest error for non-upstream calls (ClickHouse, compliance).
    #[error("Gateway internal error: {0}")]
    Proxy(reqwest::Error),

    // ── Business Rules ───────────────────────────────────────────────────────
    #[error("Compliance block: {0}")]
    ComplianceFailure(String),

    #[error("Security timeout: {0}")]
    SecurityTimeout(String),

    #[error("Rate Limit Exceeded: {0}")]
    RateLimitExceeded(String),

    #[error("Agent loop detected: {0}")]
    LoopDetected(String),

    #[error("Agent runaway loop detected: {0}")]
    AgentRunawayLoop(String),

    #[error("Agent tool storm detected: {0}")]
    AgentToolStorm(String),

    #[error("Agent loop budget exceeded: {0}")]
    AgentLoopBudgetExceeded(String),

    #[error("Burn rate exceeded: {0}")]
    BurnRateExceeded(String),

    #[error("Model not configured: {0}")]
    ModelNotConfigured(String),

    #[error("Routing state missing")]
    RoutingStateMissing,

    #[error("No active keys available for routing")]
    NoActiveKeys,

    #[error("Insufficient funds in wallet")]
    InsufficientFunds,

    #[error("Billing calculation anomaly: {0}")]
    BillingAnomaly(String),

    #[error("Billing system is temporarily unavailable: {0}")]
    BillingUnavailable(String),

    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Internal gateway error: {0}")]
    InternalError(String),
}

impl axum::response::IntoResponse for GatewayError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        use axum::Json;

        // ── Internal-only: AgentLoopBudgetExceeded returns OpenAI-spec body ──
        if let GatewayError::AgentLoopBudgetExceeded(_) = &self {
            let body = Json(serde_json::json!({
                "error": {
                    "message": "Kryneth Guard: Agent loop budget exceeded. This session has been paused.",
                    "type": "kryneth_loop_limit",
                    "code": "agent_loop_exceeded",
                    "param": null
                }
            }));
            let mut res = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
            res.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("300"),
            );
            return res;
        }

        // ── Map variant → (status, opaque_code, safe_message) ────────────────
        // SECURITY: `message` is what the client sees. Internal details (DB
        // query strings, stack traces) are logged separately below.
        let (status, code, message) = match &self {
            GatewayError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "Missing or invalid API key.",
            ),
            GatewayError::Forbidden => (
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
                "Insufficient permissions for this resource.",
            ),
            GatewayError::TenantNotFound => (
                StatusCode::NOT_FOUND,
                "TENANT_NOT_FOUND",
                "The requested tenant was not found.",
            ),
            GatewayError::InvalidTenantId => (
                StatusCode::BAD_REQUEST,
                "INVALID_TENANT_ID",
                "Tenant ID is not a valid UUID.",
            ),
            // DatabaseError: 500, safe message — detail is logged not exposed
            GatewayError::DatabaseError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "DATABASE_ERROR",
                "A database error occurred. Please try again later.",
            ),
            GatewayError::ResponseBuild(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "An internal error occurred while building the response.",
            ),
            GatewayError::InvalidJSON(_) => (
                StatusCode::BAD_REQUEST,
                "INVALID_JSON",
                "Malformed or invalid JSON payload.",
            ),
            GatewayError::MissingModel => (
                StatusCode::BAD_REQUEST,
                "MISSING_MODEL",
                "Missing 'model' parameter in request.",
            ),
            GatewayError::UpstreamUnreachable(e) => {
                if e.is_timeout() {
                    (
                        StatusCode::GATEWAY_TIMEOUT,
                        "GATEWAY_TIMEOUT",
                        "The upstream request timed out.",
                    )
                } else {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "UPSTREAM_ERROR",
                        "The upstream LLM service is currently unreachable.",
                    )
                }
            }
            GatewayError::ComplianceFailure(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "COMPLIANCE_ERROR",
                "Request blocked: compliance service rejected this payload.",
            ),
            GatewayError::SecurityTimeout(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "SECURITY_TIMEOUT",
                "Gateway Security Timeout: A required security or compliance service timed out.",
            ),
            GatewayError::RateLimitExceeded(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "RATE_LIMITED",
                "Rate limit exceeded. Please try again later.",
            ),
            GatewayError::LoopDetected(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "AGENT_LOOP_DETECTED",
                "Agent recursive loop detected. This session has been blocked.",
            ),
            GatewayError::AgentRunawayLoop(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "AGENT_RUNAWAY_LOOP",
                "Kryneth Guard: Agent infinite loop detected for identical tool signature. Request blocked.",
            ),
            GatewayError::AgentToolStorm(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "AGENT_TOOL_STORM",
                "Kryneth Guard: Tool storm detected (too many distinct tool calls). Request blocked.",
            ),
            GatewayError::AgentLoopBudgetExceeded(_) => unreachable!("handled above"),
            GatewayError::BurnRateExceeded(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "BURN_RATE_EXCEEDED",
                "Session burn rate exceeded. Spending has been paused.",
            ),
            GatewayError::Proxy(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "An internal error occurred while communicating with backend services.",
            ),
            GatewayError::ModelNotConfigured(_) => (
                StatusCode::BAD_REQUEST,
                "MODEL_NOT_CONFIGURED",
                "The requested model is not configured for this tenant.",
            ),
            GatewayError::RoutingStateMissing => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "ROUTING_STATE_MISSING",
                "Routing state is missing or unavailable.",
            ),
            GatewayError::NoActiveKeys => (
                StatusCode::SERVICE_UNAVAILABLE,
                "ALL_KEYS_EXHAUSTED",
                "All configured keys exhausted.",
            ),
            GatewayError::InsufficientFunds => (
                StatusCode::PAYMENT_REQUIRED,
                "INSUFFICIENT_FUNDS",
                "Insufficient wallet balance. Please top up.",
            ),
            GatewayError::BillingAnomaly(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "BILLING_ANOMALY",
                "Billing anomaly detected.",
            ),
            GatewayError::BillingUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "BILLING_UNAVAILABLE",
                "The billing service is temporarily unavailable. Please try again later.",
            ),
            GatewayError::ConfigurationError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "CONFIGURATION_ERROR",
                "A configuration error occurred.",
            ),
            GatewayError::InternalError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "An internal gateway error occurred.",
            ),
        };

        // ── Structured server-side logging — full internal detail ─────────────
        match &self {
            GatewayError::DatabaseError(_)
            | GatewayError::ResponseBuild(_)
            | GatewayError::Proxy(_)
            | GatewayError::RoutingStateMissing
            | GatewayError::BillingAnomaly(_)
            | GatewayError::BillingUnavailable(_)
            | GatewayError::ConfigurationError(_)
            | GatewayError::InternalError(_) => {
                tracing::error!(
                    error_code = %code,
                    status = %status.as_u16(),
                    internal_details = %self,
                    "Internal gateway error"
                );
            }
            _ => {
                tracing::warn!(
                    error_code = %code,
                    status = %status.as_u16(),
                    "Gateway client error"
                );
            }
        }

        let body = Json(serde_json::json!({
            "error": {
                "code": code,
                "message": message,
            }
        }));

        (status, body).into_response()
    }
}
