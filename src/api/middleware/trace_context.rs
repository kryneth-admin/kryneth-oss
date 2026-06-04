//! Trace context propagation middleware.
//!
//! Extracts or generates `X-Session-ID`, `X-Trace-ID`, and `X-Parent-Trace-ID`
//! headers for every request. Injects them into Axum extensions and response headers.

use axum::{
    body::Body,
    extract::{Request, State},
    http::HeaderValue,
    middleware::Next,
    response::Response,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::domain::models::{AppState, TraceContext};

pub async fn trace_context_middleware(
    State(_state): State<Arc<AppState>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let headers = request.headers();

    let session_id = headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let trace_id = Uuid::new_v4().to_string();

    let parent_trace_id = headers
        .get("x-parent-trace-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let ctx = TraceContext {
        trace_id: trace_id.clone(),
        session_id: session_id.clone(),
        parent_trace_id,
    };

    // Inject into request extensions so handlers can access it
    request.extensions_mut().insert(ctx);

    // Call the next handler
    let mut response = next.run(request).await;

    // Inject trace IDs into response headers for client correlation
    if let Ok(val) = HeaderValue::from_str(&trace_id) {
        response.headers_mut().insert("x-trace-id", val);
    }
    if let Ok(val) = HeaderValue::from_str(&session_id) {
        response.headers_mut().insert("x-session-id", val);
    }

    response
}
