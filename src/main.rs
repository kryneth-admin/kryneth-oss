//! main.rs — Kryneth Gateway entry point (Clean Architecture).
//!
//! Bootstrap only: load config, init tracing, build state, start server.

use std::{net::SocketAddr, sync::Arc};

use axum::Router;
use dotenvy::dotenv;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use kryneth_gateway::api;
use kryneth_gateway::domain;
use kryneth_gateway::infrastructure;

use domain::models::{AppState, RoutingState};

// ── Config defaults ───────────────────────────────────────────────────────────

const DEFAULT_PORT: &str = "8080";
const DEFAULT_COMPLIANCE_URL: &str = "http://localhost:8083";
const DEFAULT_RATE_LIMIT_MAX: &str = "60";
const DEFAULT_RATE_LIMIT_WINDOW: &str = "60";
const HTTP_TIMEOUT_SECS: u64 = 120;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Reads a required environment variable, panicking with a clear message on absence.
fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{} must be set", key))
}

/// Reads an optional environment variable, returning `default` when absent.
fn optional_env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Parses a numeric environment variable, panicking with a clear message on failure.
fn parse_env<T: std::str::FromStr>(key: &str, default: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    optional_env(key, default)
        .parse::<T>()
        .unwrap_or_else(|_| panic!("{} must be a valid number", key))
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    dotenv().ok();

    let jwt_secret_was_missing = if std::env::var("JWT_SECRET").is_err() {
        let generated = uuid::Uuid::new_v4().to_string();
        std::env::set_var("JWT_SECRET", &generated);
        true
    } else {
        false
    };

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    if jwt_secret_was_missing {
        tracing::warn!("JWT_SECRET not found in env. Auto-generated a secure session key for local development.");
    }

    // ── Config ────────────────────────────────────────────────────────────────
    let port: u16 = parse_env("GATEWAY_PORT", DEFAULT_PORT);
    let _jwt_secret = require_env("JWT_SECRET"); // strictly enforces presence at boot
    let compliance_url = optional_env("COMPLIANCE_URL", DEFAULT_COMPLIANCE_URL);
    let dashboard_url = optional_env("DASHBOARD_URL", "http://localhost:5173");
    let rate_limit_max: u32 = parse_env("RATE_LIMIT_MAX_REQUESTS", DEFAULT_RATE_LIMIT_MAX);
    let rate_limit_window: u32 = parse_env("RATE_LIMIT_WINDOW_SECS", DEFAULT_RATE_LIMIT_WINDOW);

    // ── Infrastructure clients ────────────────────────────────────────────────
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .expect("Failed to construct reqwest HTTP client");

    // ── App state ─────────────────────────────────────────────────────────────
    let rate_limit_cache = Arc::new(dashmap::DashMap::new());
    let dashboard_metrics = Arc::new(domain::models::DashboardMetrics::new());

    let l1_cache = match infrastructure::l1_cache::L1Cache::new(500 * 1024 * 1024) {
        Ok(cache) => Arc::new(cache),
        Err(e) => {
            error!("Failed to initialize Hybrid L1 Cache: {}", e);
            std::process::exit(1);
        }
    };

    // MEMORY BOUND: max_capacity is a byte budget (not entry count) enforced by
    // the weigher. Without a weigher, max_capacity(10_000) means 10K entries,
    // which could exhaust RAM under a burst. With a weigher, it means 10 MB total.
    let circuit_breaker = moka::future::Cache::builder()
        .max_capacity(10 * 1024 * 1024) // 10 MB ceiling
        .weigher(|k: &String, _v: &()| -> u32 {
            // key length + 64 bytes for HashMap node overhead
            (k.len() + 64).min(u32::MAX as usize) as u32
        })
        .time_to_live(std::time::Duration::from_secs(60))
        .build();

    // MEMORY BOUND: loop-fallback cache. Each AtomicU64 value is 8 bytes;
    // the key is the session_id string. 50 MB ceiling handles ~650K sessions.
    let loop_fallback_cache = moka::future::Cache::builder()
        .max_capacity(50 * 1024 * 1024) // 50 MB ceiling
        .weigher(
            |k: &String, _v: &std::sync::Arc<std::sync::atomic::AtomicU64>| -> u32 {
                // key + Arc overhead + AtomicU64 size
                (k.len() + 64 + 8).min(u32::MAX as usize) as u32
            },
        )
        .time_to_live(std::time::Duration::from_secs(300))
        .build();

    let agent_guardian_cache = moka::future::Cache::builder()
        .max_capacity(10 * 1024 * 1024)
        .weigher(|k: &String, _v: &u32| -> u32 { (k.len() + 64 + 4).min(u32::MAX as usize) as u32 })
        .time_to_live(std::time::Duration::from_secs(60))
        .build();

    // ── MCP Connection Registry ─────────────────────────────────────────────
    let mcp_registry = infrastructure::mcp_registry::McpConnectionRegistry::from_env();

    // ── MCP Tool Schema Registry ────────────────────────────────────────────
    let tool_registry = kryneth_gateway::usecases::tool_router::ToolRegistry::from_env();

    let initial_config = {
        let config_path =
            std::env::var("ROUTING_CONFIG_PATH").unwrap_or_else(|_| "routing.yaml".to_string());
        // Try current directory first, then fallback to kryneth_gateway/routing.yaml
        let mut file_content = std::fs::read_to_string(&config_path);
        if file_content.is_err() && config_path == "routing.yaml" {
            if let Ok(content) = std::fs::read_to_string("kryneth_gateway/routing.yaml") {
                file_content = Ok(content);
            }
        }

        match file_content {
            Ok(content) => {
                match serde_yaml::from_str::<
                    std::collections::HashMap<
                        String,
                        std::collections::HashMap<String, domain::models::ModelConfig>,
                    >,
                >(&content)
                {
                    Ok(mut parsed) => {
                        info!(path = %config_path, "Loaded OSS routing configuration successfully");
                        // Resolve API keys from env if they are empty
                        for tenant_models in parsed.values_mut() {
                            for model_cfg in tenant_models.values_mut() {
                                for target in &mut model_cfg.targets {
                                    if target.api_key.is_empty() {
                                        if let Ok(val) = std::env::var(&target.api_key_alias) {
                                            target.api_key = val;
                                        } else {
                                            tracing::warn!(
                                                alias = %target.api_key_alias,
                                                "API key environment variable not set for target"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        parsed
                    }
                    Err(e) => {
                        error!(path = %config_path, error = %e, "Failed to parse OSS routing configuration YAML. Falling back to empty routing.");
                        std::collections::HashMap::new()
                    }
                }
            }
            Err(e) => {
                tracing::warn!(path = %config_path, error = %e, "Could not read OSS routing configuration file. Falling back to empty routing.");
                std::collections::HashMap::new()
            }
        }
    };

    let initial_client_configs = std::collections::HashMap::new();

    let routing_state = Arc::new(RoutingState::new());
    routing_state.state.store(Arc::new(initial_config));
    routing_state
        .client_configs
        .store(Arc::new(initial_client_configs));

    // ── Telemetry / Billing / Auth Adapters ────────────────────────────────────
    let telemetry: Arc<dyn crate::domain::ports::TelemetryPort> =
        Arc::new(infrastructure::oss_adapters::OssTelemetry);

    let billing: Arc<dyn crate::domain::ports::BillingPort> =
        Arc::new(infrastructure::oss_adapters::OssBilling);

    let auth_resolver: Arc<dyn crate::domain::ports::AuthPort> =
        Arc::new(infrastructure::oss_adapters::OssAuth);

    let rate_limiter: Arc<dyn crate::domain::ports::RateLimitPort> =
        Arc::new(infrastructure::oss_adapters::OssRateLimit);

    let routing_config: Arc<dyn crate::domain::ports::RoutingConfigPort> =
        Arc::new(infrastructure::oss_adapters::OssRoutingConfig);

    let semantic_cache: Arc<dyn crate::domain::ports::SemanticCachePort> =
        Arc::new(infrastructure::oss_adapters::OssSemanticCache);

    let state = Arc::new(AppState {
        http_client: http_client.clone(),
        compliance_url,
        rate_limit_max,
        rate_limit_window,
        dashboard_url,
        llm_api_base_url: None,
        telemetry,
        billing,
        auth_resolver,
        rate_limiter,
        routing_config,
        semantic_cache,
        rate_limit_cache,
        l1_cache,
        routing_state: routing_state.clone(),
        circuit_breaker,
        loop_fallback_cache,
        mcp_registry,
        tool_registry,
        agent_guardian_cache,
        dashboard_metrics,
        pricing_map: Arc::new(arc_swap::ArcSwap::from_pointee(std::collections::HashMap::new())),
        budget_map: Arc::new(arc_swap::ArcSwap::from_pointee(std::collections::HashMap::new())),
    });

    // ── Start Port-based background workers ───────────────────────────────────
    state
        .rate_limiter
        .start_sync_worker(state.rate_limit_cache.clone());

    state
        .routing_config
        .start_subscriber(state.routing_state.clone(), state.mcp_registry.clone());

    // ── Server ────────────────────────────────────────────────────────────────
    let app: Router = api::routes::create_router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    info!("🚀 Kryneth Gateway listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind TCP listener");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("Axum server encountered a fatal error");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("signal received, starting graceful shutdown");
}
