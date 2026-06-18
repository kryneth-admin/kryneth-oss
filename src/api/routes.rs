//! api/routes.rs — Single source of truth for all route registrations.

use std::sync::Arc;

use axum::{
    http::header::{AUTHORIZATION, CONTENT_TYPE},
    routing::{get, post},
    Router,
};
use tower_http::cors::CorsLayer;

use crate::api::handlers;
use crate::api::middleware::auth::{auth_middleware, require_admin};
use crate::domain::models::AppState;

pub fn create_router(state: Arc<AppState>) -> Router {
    // ── LLM proxy: rate limit + auth ─────────────────────────────────────────
    let proxy_routes = Router::new()
        .route("/chat/completions", post(handlers::chat_completions))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::api::middleware::rate_limit::rate_limit_middleware,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::api::middleware::billing_guard::billing_guard_middleware,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // ── Admin: auth → require_admin (RBAC) ───────────────────────────────────
    // `require_admin` is inner (runs first on the way in, after auth injects Claims).
    // Order: auth_middleware → require_admin → handler.
    let admin_routes = Router::new()
        .route("/dashboard", get(handlers::get_dashboard))
        .route("/metrics/live", get(handlers::get_live_metrics))
        .route("/routing-state", get(handlers::get_routing_state))
        // RBAC: Admin role required for ALL /v1/admin/* routes.
        // Applied before auth so Claims is already in extensions when it runs.
        .route_layer(axum::middleware::from_fn(require_admin))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // ── MCP: auth only (no admin requirement) ────────────────────────────────
    let mcp_routes = Router::new()
        .route("/registry", get(handlers::get_mcp_registry))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let cors = create_cors_layer();

    Router::new()
        .nest("/v1", proxy_routes)
        .nest("/v1/admin", admin_routes)
        .nest("/v1/mcp", mcp_routes)
        .route("/health", get(handlers::health_check))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::api::middleware::trace_context::trace_context_middleware,
        ))
        .layer(cors)
        .layer(axum::extract::DefaultBodyLimit::max(50 * 1024 * 1024))
        .with_state(state)
}

fn create_cors_layer() -> CorsLayer {
    let mut origins = Vec::new();

    if let Ok(url) = std::env::var("DASHBOARD_URL") {
        if let Ok(v) = url.parse() {
            origins.push(v);
        } else {
            tracing::error!(%url, "Invalid DASHBOARD_URL; no CORS origins configured");
        }
    } else {
        if let Ok(v) = "http://localhost:5173".parse() {
            origins.push(v);
        }
        if let Ok(v) = "http://localhost:3000".parse() {
            origins.push(v);
        }
    }

    CorsLayer::new()
        .allow_origin(origins)
        .allow_credentials(true)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            axum::http::HeaderName::from_static("x-csrf-token"),
            axum::http::HeaderName::from_static("x-tenant-id"),
            axum::http::HeaderName::from_static("x-kryneth-routing-strategy"),
            axum::http::HeaderName::from_static("x-kryneth-loop-count"),
            axum::http::HeaderName::from_static("x-kryneth-context-compression"),
        ])
        .expose_headers([
            axum::http::HeaderName::from_static("x-cache"),
            axum::http::HeaderName::from_static("x-trace-id"),
            axum::http::HeaderName::from_static("x-kryneth-loop-count"),
        ])
}
