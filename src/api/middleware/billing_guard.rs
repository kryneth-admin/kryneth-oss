//! Middleware for tenant wallet balance extraction and injection into request extensions.
//! Final HTTP 402 rejection based on provider rates is delegated to the proxy execution stage.

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
    let tenant_id = match req.headers().get("x-tenant-id") {
        Some(value) => match value.to_str() {
            Ok(s) => Some(s.trim().to_string()),
            Err(_) => None,
        },
        None => None,
    };

    if let Some(tid) = tenant_id {
        if tid == "anonymous" || req.uri().path() == "/health" {
            return Ok(next.run(req).await);
        }

        let balance = state.billing.check_balance(&tid).await?;
        req.extensions_mut().insert(balance);
    }

    Ok(next.run(req).await)
}
