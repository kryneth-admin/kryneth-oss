//! src/api/middleware/billing_guard.rs — Balance Enforcement Pre-flight Middleware.
//!
//! Intercepts all billing-scoped proxy requests, checks the tenant's wallet balance
//! in Redis asynchronously, and rejects depleted accounts with HTTP 402 Payment Required.

use crate::domain::models::AppState;
use crate::error::GatewayError;
use axum::{
    body::Body,
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

/// Axum-native middleware that intercepts proxy requests to enforce credit bounds.
/// Executes after `auth_middleware` ensures a verified `x-tenant-id` header is attached.
pub async fn billing_guard_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, GatewayError> {
    // 1. Extract the verified x-tenant-id header injected by auth_middleware
    let tenant_id = match req.headers().get("x-tenant-id") {
        Some(value) => match value.to_str() {
            Ok(s) => Some(s.trim().to_string()),
            Err(_) => None,
        },
        None => None,
    };

    if let Some(tid) = tenant_id {
        // Bypass checks for non-route sentinel requests or health checks
        if tid == "anonymous" || req.uri().path() == "/health" {
            return Ok(next.run(req).await);
        }

        // 2. Lookup balance via BillingPort
        let balance = state.billing.check_balance(&tid).await?;
        req.extensions_mut().insert(balance);
    }

    // 4. Wallet verified; forward to the next handler/middleware in Tower chain
    Ok(next.run(req).await)
}
