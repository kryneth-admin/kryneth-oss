//! usecases/agentic_orchestrator.rs — Tunnel 3 Phase 1 + Phase 2 orchestration.
//!
//! Extracted from `chat_completions` handler to keep the HTTP handler ≤ 40 lines.
//!
//! ## Phase 1: Speculative MCP Pre-Fetching
//! FSM byte-scan (zero-allocation, chunk-boundary-safe). Acquires a permit from a
//! 20-slot semaphore — if full, the warm-up is silently dropped (best-effort).
//!
//! ## Phase 2: Lazy Schema Loading (Bumpalo Arena)
//! A per-request arena bounds intermediate allocations. Freed in O(1) when the
//! scope ends. Returns the (possibly schema-stripped) body bytes for upstream use.
//!
//! ## Telemetry Backpressure
//! BOUNDED: `try_send()` is used instead of `send().await`. If the telemetry channel
//! is full (backpressure), the event is silently dropped — the hot path is NEVER blocked.
//! A counter warning is emitted at `debug` level.

use std::sync::Arc;

use tracing::{debug, info};

use crate::domain::models::{AppState, TraceContext};
use crate::error::GatewayError;

/// Output of the agentic orchestration phase, handed back to the HTTP handler.
pub struct AgenticContext {
    /// Loop-count to inject into the response header (None if non-agentic request).
    pub loop_count: Option<u64>,
    /// Body bytes for the upstream LLM call — may differ from the original if
    /// Phase 2 schema-stripped tool definitions (zero-copy pass-through otherwise).
    pub body_bytes: axum::body::Bytes,
}

/// Orchestrate Tunnel 3 Phase 1 (MCP warm-up) and Phase 2 (lazy schema injection),
/// plus agentic loop-budget enforcement for agentic payloads.
///
/// # Arguments
/// * `body_bytes`      — raw, validated request body (zero-copy `axum::body::Bytes`)
/// * `tenant_id`       — tenant string slice (extracted from header, not owned)
/// * `api_key`         — API key string slice
/// * `session_id`      — optional client-provided session ID slice
/// * `idempotency_key` — optional idempotency key slice
/// * `is_agentic`      — pre-computed agentic flag (tools / tool_choice key detected)
#[allow(clippy::too_many_arguments)]
pub async fn orchestrate(
    state: &Arc<AppState>,
    body_bytes: axum::body::Bytes,
    tenant_id: &str,
    api_key: &str,
    session_id: Option<&str>,
    idempotency_key: Option<&str>,
    is_agentic: bool,
    trace_ctx: &TraceContext,
    enable_compression: bool,
) -> Result<AgenticContext, GatewayError> {
    let mut loop_count: Option<u64> = None;

    // ── Agentic Loop Budget Enforcement ──────────────────────────────────────
    if is_agentic {
        let resolved_session_id = crate::usecases::agentic_tracker::resolve_session_id(
            session_id,
            idempotency_key,
            tenant_id,
            api_key,
            &body_bytes,
        );

        let start_time = std::time::Instant::now();
        let budget_res = crate::usecases::behavior_guard::enforce_agentic_loop_budget(
            state,
            &resolved_session_id,
        )
        .await;
        let latency_ms = start_time.elapsed().as_millis() as u32;

        let count_val = match budget_res {
            Ok(c) => {
                // BOUNDED telemetry: try_send drops silently if channel is full.
                // Never blocks the hot path — OOM / channel backpressure is handled gracefully.
                let payload = serde_json::json!({
                    "type": "agentic_loop_detection",
                    "session_id": resolved_session_id,
                    "tenant_id": tenant_id,
                    "agentic_requests_total": 1,
                    "loop_limit_hits": 0u64,
                    "detection_latency_seconds": (latency_ms as f64) / 1000.0,
                    "timestamp": chrono::Utc::now().to_rfc3339()
                });
                state.telemetry.log_event(payload);
                c
            }
            Err(e) => {
                // Early loop budget rejection!
                // 1. Update Metrics
                state
                    .dashboard_metrics
                    .total_latency_ms
                    .fetch_add(latency_ms as usize, std::sync::atomic::Ordering::Relaxed);
                // Note: enforce_agentic_loop_budget already handles state.dashboard_metrics.blocked_agent_loops.

                // 2. Log dual payloads to ClickHouse/tracer
                let req_payload = serde_json::json!({
                    "id": uuid::Uuid::new_v4().to_string(),
                    "tenant_id": tenant_id,
                    "status": 429_u16,
                    "latency_ms": latency_ms,
                    "model": "__loop_blocked",
                    "tokens": 0_u32,
                    "requested_provider": "unknown",
                    "executed_provider": "",
                    "is_hot_swapped": 0_u8
                });
                state.telemetry.log_event(req_payload);

                let raw_prompt = std::str::from_utf8(&body_bytes).unwrap_or("");
                let trace_payload = serde_json::json!({
                    "trace_id":        trace_ctx.trace_id,
                    "session_id":      resolved_session_id,
                    "parent_trace_id": trace_ctx.parent_trace_id,
                    "tenant_id":       tenant_id,
                    "model":           "__loop_blocked",
                    "status":          429_u16,
                    "latency_ms":      latency_ms,
                    "total_tokens":    0_u32,
                    "cache_hit":       false,
                    "prompt_content":  raw_prompt,
                    "response_content": "",
                    "requested_provider": "unknown",
                    "executed_provider": "",
                    "is_hot_swapped": 0_u8,
                    "error":           e.to_string()
                });
                state.telemetry.log_event(trace_payload);

                return Err(e);
            }
        };

        loop_count = Some(count_val);
    }

    // ── Tunnel 3 Phase 1: Speculative MCP Pre-Fetching ────────────────────────
    // FSM-based byte scan — zero-allocation, chunk-boundary safe.
    // Semaphore: 20 in-flight permits. If full, drop silently (best-effort).
    if !state.mcp_registry.is_empty() {
        if let Some(tool_name) = state.mcp_registry.find_tool_hint(&body_bytes) {
            if let Some(sse_url) = state.mcp_registry.get_url(&tool_name) {
                match Arc::clone(&state.mcp_registry.prefetch_sem).try_acquire_owned() {
                    Ok(permit) => {
                        let registry = Arc::clone(&state.mcp_registry);
                        let http_client = state.http_client.clone();
                        let tid_owned = tenant_id.to_string();
                        tokio::spawn(async move {
                            crate::infrastructure::mcp_registry::McpConnectionRegistry::warm_connection(
                                registry,
                                http_client,
                                tid_owned,
                                tool_name,
                                sse_url,
                                permit,
                            )
                            .await;
                        });
                    }
                    Err(_) => {
                        debug!(
                            "Tunnel 3 Phase 1 — prefetch semaphore full (20 in-flight); \
                             dropping warm-up (best-effort)"
                        );
                    }
                }
            }
        }
    }

    // ── Tunnel 3 Phase 2: Lazy Schema Loading via Bumpalo Arena ──────────────
    // Arena is stack-allocated; holds intermediate byte slices during JSON
    // serialisation. Freed in O(1) when this block exits.
    //
    // `body_bytes` is shadowed: if schemas were stripped, the binding now
    // points to the modified bytes; otherwise it's a zero-copy pass-through.
    let body_bytes: axum::body::Bytes = if !state.tool_registry.is_empty() {
        let arena = bumpalo::Bump::new();
        match state
            .tool_registry
            .inject_lazy_summaries(&body_bytes, &arena, enable_compression)
        {
            Some(modified) => {
                info!(
                    original_len = body_bytes.len(),
                    modified_len = modified.len(),
                    "Tunnel 3 Phase 2 — schema-stripped body for upstream LLM call"
                );
                // Allocate the modified body on the heap — the arena is freed after this block.
                axum::body::Bytes::from(modified)
            }
            // No registered tools found or no schemas present — zero-copy pass-through.
            None => body_bytes,
        }
    } else {
        body_bytes
    };

    Ok(AgenticContext {
        loop_count,
        body_bytes,
    })
}
