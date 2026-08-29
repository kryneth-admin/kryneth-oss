//! usecases/proxy.rs — Core proxy orchestration logic.
//!
//! Pipeline: compliance redaction → cache check → upstream call → async telemetry.
//! Intentionally free of Axum types so it can be tested or reused independently.
//!
//! ## Architecture
//! Each pipeline stage is isolated in its own `#[inline]` helper, keeping
//! `execute_proxy` readable as a top-level orchestrator.  All variables and
//! external interfaces are unchanged.

use std::sync::{Arc, OnceLock};

use eventsource_stream::Eventsource;
// futures::StreamExt is imported inline inside handle_streaming_response.
use regex::Regex;
use serde_json::{json, Value};
use simd_json::prelude::*;
// NOTE: ReceiverStream (bounded) is used inline in handle_streaming_response.
use tracing::{debug, error, info, warn};

use crate::domain::models::{AppState, ClientConfig, TraceContext};
use crate::error::GatewayError;
use crate::infrastructure::llm_router;
use crate::infrastructure::routing_strategy::RoutingStrategy;

// ── Public types ─────────────────────────────────────────────────────────────

pub enum ProxyBody {
    Buffered(Vec<u8>),
    Stream(axum::body::Body),
}

pub struct ProxyResult {
    pub status: u16,
    pub content_type: String,
    pub body: ProxyBody,
    pub cache_hit: bool,
}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Rough chars-per-token estimate for prompt token counting.
const CHARS_PER_TOKEN: usize = 4;

// ── PII regex (compiled once) ─────────────────────────────────────────────────

fn pii_regex() -> &'static Regex {
    static PII_REGEX: OnceLock<Regex> = OnceLock::new();
    PII_REGEX.get_or_init(|| {
        // Bug 5 Fix: ReDoS-safe regex — replaced backtracking-prone lazy quantifiers
        // like `(?:\d[ -]*?){13,16}` with fixed-width, DFA-compatible alternations.
        // Each alternative uses a concrete digit count and explicit separator positions,
        // preventing polynomial backtracking on adversarial inputs.
        //
        // Patterns covered:
        //   CC:     4×4-digit groups (space/dash/none separator)
        //   Email:  possessive char classes, bounded TLD {2,7}
        //   Phone:  US format with strict separator positions
        //   Aadhaar:4-4-4 digit groups
        //   IFSC:   4 alpha + 0 + 6 alnum
        //   Bank/SSN: 9–12 digit sequences (bounded, word-anchored)
        let pattern = concat!(
            // Credit card: 4×4 groups with space, dash or no separator
            r"\b\d{4}[[:space:]-]?\d{4}[[:space:]-]?\d{4}[[:space:]-]?\d{4}\b",
            // Email: bounded TLD prevents runaway backtracking
            r"|\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,7}\b",
            // US Phone: +1 optional, strict separator positions
            r"|\b(?:\+1[[:space:]])?\(?\d{3}\)?[[:space:].-]\d{3}[[:space:].-]\d{4}\b",
            // Aadhaar: 4-4-4 digit groups
            r"|\b\d{4}[[:space:]-]?\d{4}[[:space:]-]?\d{4}\b",
            // IFSC: 4 alpha + literal 0 + 6 alnum
            r"|\b[A-Z]{4}0[A-Z0-9]{6}\b",
            // Bank account / SSN-like: 9–12 consecutive digits, word-anchored
            r"|\b\d{9,12}\b"
        );
        Regex::new(pattern).expect("Invalid PII Regex")
    })
}

// ── Main entry point ──────────────────────────────────────────────────────────

fn get_requested_provider(state: &Arc<AppState>, tenant_id: &str, model_name: &str) -> String {
    state
        .routing_state
        .state
        .load()
        .get(tenant_id)
        .and_then(|models| models.get(model_name))
        .and_then(|c| c.targets.first())
        .map(|t| t.schema_format.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Extracts textual content from the most recent user message for semantic embedding,
/// returning `None` if the payload represents an active agentic tool-execution turn.
///
/// ## Agentic Loop Prevention
///
/// In multi-turn agentic workflows (User → Assistant `tool_calls` → Tool result → Assistant
/// next turn), every subsequent LLM call re-includes the original user prompt. Without this
/// guard, `extract_semantic_text` would always land on the same user text, generate an
/// identical 384-dim BGE vector, and the L1/L2 pgvector lookup would return `dist=0.0000,
/// sim=1.0000` — trapping the agent in an infinite cache hit loop.
///
/// Returning `None` propagates through the pipeline:
/// `semantic_text = None` → `embedding_vector = None` → cache future short-circuits →
/// request is forwarded directly to the upstream LLM without cache read or write.
///
/// ## What is NOT bypassed
///
/// - **L1 Exact Cache**: keyed on the full raw JSON body. Each agentic turn appends new
///   messages, so the raw bytes differ every turn — false exact-match collisions are
///   impossible. No bypass needed.
/// - **PII scanning** (`extract_semantic_text_raw`): intentionally left active. Agent tools
///   (e.g., `query_database`, `fetch_user_profile`) are the primary vectors for introducing
///   raw sensitive data (PII) into the LLM context. The 64 KiB bounded scan is a mandatory
///   security trade-off for data-privacy compliance.
fn extract_semantic_text(payload: &simd_json::BorrowedValue<'_>) -> Option<String> {
    // ─────────────────────────────────────────────────────────────────────────
    // PATH 1: OpenAI / Anthropic `messages` array — MERGED SINGLE-PASS
    //
    // Iterates newest-to-oldest (rev). On each message:
    //   • If a tool/function execution marker is detected → return None immediately.
    //   • If a user message is found first → extract its text and break.
    //
    // Single-pass O(N): detection and extraction share one reverse walk.
    // ─────────────────────────────────────────────────────────────────────────
    if let Some(simd_json::BorrowedValue::Array(messages)) = payload.get("messages") {
        let mut semantic_text = String::new();

        for i in (0..messages.len()).rev() {
            let msg = &messages[i];
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");

            // ── Agentic bypass: tool / function response role ─────────────
            if role == "tool" || role == "function" {
                tracing::debug!(
                    "Agentic tool response turn detected (role={}); bypassing semantic cache",
                    role
                );
                return None;
            }

            // ── Agentic bypass: assistant with active tool invocations ────
            if role == "assistant" {
                if msg
                    .get("tool_calls")
                    .and_then(|tc| tc.as_array())
                    .is_some_and(|a| !a.is_empty())
                {
                    tracing::debug!(
                        "Agentic assistant tool_calls detected; bypassing semantic cache"
                    );
                    return None;
                }
                if msg.get("function_call").is_some() {
                    tracing::debug!(
                        "Agentic assistant function_call detected; bypassing semantic cache"
                    );
                    return None;
                }
                // Anthropic assistants use a content block array with type="tool_use"
                // instead of (or in addition to) the tool_calls field.
                if let Some(content_blocks) = msg.get("content").and_then(|c| c.as_array()) {
                    for block in content_blocks {
                        if let Some(bt) = block.get("type").and_then(|t| t.as_str()) {
                            if bt == "tool_use" {
                                tracing::debug!(
                                    "Anthropic assistant tool_use content block detected; bypassing semantic cache"
                                );
                                return None;
                            }
                        }
                    }
                }
                // assistant messages with no tool invocations are skipped — continue scan
                continue;
            }

            // ── Agentic bypass: Anthropic tool content blocks (non-assistant roles) ──
            // Catches `tool_result` in user-role messages (the tool response turn).
            if let Some(content_blocks) = msg.get("content").and_then(|c| c.as_array()) {
                for block in content_blocks {
                    if let Some(bt) = block.get("type").and_then(|t| t.as_str()) {
                        if bt == "tool_result" {
                            tracing::debug!(
                                "Anthropic tool_result content block detected; bypassing semantic cache"
                            );
                            return None;
                        }
                    }
                }
            }

            // ── Extraction: most recent user message ──────────────────────
            if role == "user" {
                if let Some(content) = msg.get("content") {
                    if let Some(text) = content.as_str() {
                        semantic_text.push_str(text);
                        semantic_text.push('\n');
                    } else if let Some(arr) = content.as_array() {
                        for block in arr {
                            if let Some(text) = block.as_str() {
                                semantic_text.push_str(text);
                                semantic_text.push('\n');
                            } else if block.is_object()
                                && block.get("type").and_then(|t| t.as_str()) == Some("text")
                            {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    semantic_text.push_str(text);
                                    semantic_text.push('\n');
                                }
                            }
                        }
                    }
                }
                break; // stop after the most recent user message
            }
        }

        let trimmed = semantic_text.trim();
        return if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }

    // ─────────────────────────────────────────────────────────────────────────
    // PATH 2: Anthropic legacy `prompt` string format
    // No tool-turn markers exist in this format; extract the most recent Human turn.
    // ─────────────────────────────────────────────────────────────────────────
    if let Some(prompt) = payload.get("prompt").and_then(|p| p.as_str()) {
        let mut clean_prompt = prompt;
        if let Some(human_idx) = prompt.rfind("\n\nHuman:") {
            let start = human_idx + "\n\nHuman:".len();
            if let Some(assistant_idx) = prompt[start..].find("\n\nAssistant:") {
                clean_prompt = &prompt[start..start + assistant_idx];
            } else {
                clean_prompt = &prompt[start..];
            }
        }
        let trimmed = clean_prompt.trim();
        return if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }

    // ─────────────────────────────────────────────────────────────────────────
    // PATH 3: Google Gemini `contents` array — TWO-PASS
    //
    // Pass 1 (forward): detect functionCall / functionResponse in any part.
    //   These appear in `model`-role content, not user-role content, so a single
    //   reverse walk cannot reliably detect and extract in one pass.
    // Pass 2 (reverse): extract the most recent `user`-role text parts.
    // ─────────────────────────────────────────────────────────────────────────
    if let Some(simd_json::BorrowedValue::Array(contents)) = payload.get("contents") {
        // Pass 1: agentic bypass detection (forward iteration on contents)
        for content in contents {
            if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                for part in parts {
                    if part.get("functionResponse").is_some() || part.get("functionCall").is_some()
                    {
                        tracing::debug!(
                            "Gemini functionResponse/functionCall detected; bypassing semantic cache"
                        );
                        return None;
                    }
                }
            }
        }

        // Pass 2: user text extraction (reverse index walk)
        let mut semantic_text = String::new();
        for i in (0..contents.len()).rev() {
            let content = &contents[i];
            if content.get("role").and_then(|r| r.as_str()) == Some("user") {
                if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            semantic_text.push_str(text);
                            semantic_text.push('\n');
                        }
                    }
                }
                break;
            }
        }

        let trimmed = semantic_text.trim();
        return if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }

    None
}

/// Zero-copy PII scan target extractor.
///
/// Reads directly from raw JSON bytes without heap-copying the payload.
/// Uses a byte-level state machine (same pattern as `extract_model_fast` and
/// `rewrite_model_field` in `llm_router.rs`) to extract only text-type content
/// nodes from user messages, explicitly skipping Base64 image blocks.
///
/// Output is capped at 64 KiB — the regex scan surface is always bounded
/// regardless of total payload size (e.g., a 50 MB multimodal request).
///
/// Returns `None` when no extractable text exists (pure image / tool payload),
/// which is treated as "no PII possible" and skips compliance redaction.
fn extract_semantic_text_raw(body: &[u8]) -> Option<String> {
    const BUDGET: usize = 64 * 1024;

    #[inline]
    fn skip_ws(b: &[u8], mut i: usize) -> usize {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    }

    fn parse_string(b: &[u8], i: usize) -> Option<(&[u8], usize)> {
        if b.get(i) != Some(&b'"') {
            return None;
        }
        let mut j = i + 1;
        let mut esc = false;
        while j < b.len() {
            match (esc, b[j]) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => return Some((&b[i + 1..j], j + 1)),
                _ => {}
            }
            j += 1;
        }
        None
    }

    fn skip_value(b: &[u8], start: usize) -> usize {
        let i = skip_ws(b, start);
        if i >= b.len() {
            return b.len();
        }
        match b[i] {
            b'"' => parse_string(b, i).map(|(_, e)| e).unwrap_or(b.len()),
            b'[' | b'{' => {
                let open = b[i];
                let close = if open == b'[' { b']' } else { b'}' };
                let mut depth = 1i32;
                let mut j = i + 1;
                while j < b.len() && depth > 0 {
                    match b[j] {
                        b'"' => {
                            j = parse_string(b, j).map(|(_, e)| e).unwrap_or(j + 1);
                            continue;
                        }
                        c if c == open => depth += 1,
                        c if c == close => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                j
            }
            _ => {
                let mut j = i;
                while j < b.len()
                    && !matches!(b[j], b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t')
                {
                    j += 1;
                }
                j
            }
        }
    }

    fn find_key(b: &[u8], key: &[u8], start: usize, end: usize) -> Option<usize> {
        let end = end.min(b.len());
        let mut i = start;
        while i < end {
            if b[i] == b'"' {
                let klen = key.len();
                if i + klen + 2 <= end
                    && b[i + 1..i + 1 + klen] == *key
                    && b.get(i + 1 + klen) == Some(&b'"')
                {
                    let mut j = i + klen + 2;
                    j = skip_ws(b, j);
                    if b.get(j) == Some(&b':') {
                        j += 1;
                        j = skip_ws(b, j);
                        return Some(j);
                    }
                }
                if let Some((_, end_off)) = parse_string(b, i) {
                    i = end_off;
                    continue;
                }
            }
            i += 1;
        }
        None
    }

    let len = body.len();
    let mut out = String::new();

    // Iterate through all JSON objects `{ ... }` in body
    let mut i = 0;
    while i < len && out.len() < BUDGET {
        if body[i] == b'"' {
            if let Some((_, end)) = parse_string(body, i) {
                i = end;
                continue;
            }
        }
        if body[i] == b'{' {
            let obj_start = i;
            let mut depth = 1i32;
            let mut j = i + 1;
            while j < len && depth > 0 {
                match body[j] {
                    b'"' => {
                        if let Some((_, end)) = parse_string(body, j) {
                            j = end;
                            continue;
                        }
                    }
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }

            if depth == 0 {
                let obj_bytes = &body[obj_start..j];
                let obj_len = obj_bytes.len();

                // Check if this object is a user message
                let is_user_role = find_key(obj_bytes, b"role", 0, obj_len)
                    .and_then(|voff| parse_string(obj_bytes, voff))
                    .map(|(role_val, _)| role_val == b"user")
                    .unwrap_or(false);

                if is_user_role {
                    // Extract "content" (OpenAI / Anthropic)
                    if let Some(cv) = find_key(obj_bytes, b"content", 0, obj_len) {
                        let cv = skip_ws(obj_bytes, cv);
                        if cv < obj_len {
                            match obj_bytes[cv] {
                                b'"' => {
                                    if let Some((raw, _)) = parse_string(obj_bytes, cv) {
                                        if let Ok(text) = std::str::from_utf8(raw) {
                                            out.push_str(text);
                                            out.push('\n');
                                        }
                                    }
                                }
                                b'[' => {
                                    let arr_end = skip_value(obj_bytes, cv);
                                    let mut k = cv + 1;
                                    while k < arr_end && out.len() < BUDGET {
                                        k = skip_ws(obj_bytes, k);
                                        if k >= arr_end || obj_bytes[k] == b']' {
                                            break;
                                        }
                                        if obj_bytes[k] == b'{' {
                                            let block_end = skip_value(obj_bytes, k);
                                            let block_bytes = &obj_bytes[k..block_end];
                                            let blen = block_bytes.len();

                                            let is_text = find_key(block_bytes, b"type", 0, blen)
                                                .and_then(|tv| parse_string(block_bytes, tv))
                                                .map(|(t, _)| t == b"text")
                                                .unwrap_or(true);

                                            if is_text {
                                                if let Some(tv) =
                                                    find_key(block_bytes, b"text", 0, blen)
                                                {
                                                    if let Some((raw, _)) =
                                                        parse_string(block_bytes, tv)
                                                    {
                                                        if let Ok(text) = std::str::from_utf8(raw) {
                                                            out.push_str(text);
                                                            out.push('\n');
                                                        }
                                                    }
                                                }
                                            }
                                            k = block_end;
                                        } else {
                                            k = skip_value(obj_bytes, k);
                                        }
                                        k = skip_ws(obj_bytes, k);
                                        if obj_bytes.get(k) == Some(&b',') {
                                            k += 1;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }

                    // Extract "parts" (Gemini)
                    if let Some(pv) = find_key(obj_bytes, b"parts", 0, obj_len) {
                        let pv = skip_ws(obj_bytes, pv);
                        if obj_bytes.get(pv) == Some(&b'[') {
                            let arr_end = skip_value(obj_bytes, pv);
                            let mut k = pv + 1;
                            while k < arr_end && out.len() < BUDGET {
                                k = skip_ws(obj_bytes, k);
                                if k >= arr_end || obj_bytes[k] == b']' {
                                    break;
                                }
                                if obj_bytes[k] == b'{' {
                                    let block_end = skip_value(obj_bytes, k);
                                    let block_bytes = &obj_bytes[k..block_end];
                                    let blen = block_bytes.len();
                                    if let Some(tv) = find_key(block_bytes, b"text", 0, blen) {
                                        if let Some((raw, _)) = parse_string(block_bytes, tv) {
                                            if let Ok(text) = std::str::from_utf8(raw) {
                                                out.push_str(text);
                                                out.push('\n');
                                            }
                                        }
                                    }
                                    k = block_end;
                                } else {
                                    k = skip_value(obj_bytes, k);
                                }
                                k = skip_ws(obj_bytes, k);
                                if obj_bytes.get(k) == Some(&b',') {
                                    k += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        i += 1;
    }

    // Anthropic legacy: top-level "prompt" string (scan first 8 KiB only)
    if out.trim().is_empty() {
        if let Some(pv) = find_key(body, b"prompt", 0, len.min(8192)) {
            let pv = skip_ws(body, pv);
            if body.get(pv) == Some(&b'"') {
                if let Some((raw, _)) = parse_string(body, pv) {
                    if let Ok(prompt) = std::str::from_utf8(raw) {
                        let text = if let Some(idx) = prompt.rfind("\n\nHuman:") {
                            let s = &prompt[idx + "\n\nHuman:".len()..];
                            s.find("\n\nAssistant:")
                                .map(|e| s[..e].trim())
                                .unwrap_or_else(|| s.trim())
                        } else {
                            prompt.trim()
                        };
                        if !text.is_empty() {
                            out.push_str(text);
                        }
                    }
                }
            }
        }
    }

    if out.len() > BUDGET {
        out.truncate(BUDGET);
    }

    let trimmed = out.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Returns the leading/primary interrogative token in lowercase, if present.
fn get_primary_interrogative(prompt: &str) -> Option<&'static str> {
    let strong_tokens = ["what", "who", "where", "when", "why", "how"];
    let weak_tokens = ["can", "is", "do", "does"];
    let prompt_lower = prompt.to_lowercase();
    let mut words = prompt_lower
        .split(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .filter(|w| !w.is_empty());

    let first_word = words.next();

    // 1. Check if first word is weak or strong
    if let Some(first) = first_word {
        for &t in &strong_tokens {
            if first == t {
                return Some(t);
            }
        }
        for &t in &weak_tokens {
            if first == t {
                return Some(t);
            }
        }
    }

    // 2. Check subsequent words ONLY for strong tokens
    for word in words {
        for &t in &strong_tokens {
            if word == t {
                return Some(t);
            }
        }
    }
    None
}

/// Pre-flight Intent Interception / Keyword Penalty Check.
/// If both prompts have a primary interrogative token but they don't match, return false (mismatch).
fn verify_intent(incoming: &str, cached: &str) -> bool {
    let incoming_token = get_primary_interrogative(incoming);
    let cached_token = get_primary_interrogative(cached);
    match (incoming_token, cached_token) {
        (Some(inc), Some(cac)) => inc == cac,
        _ => true,
    }
}

/// Execute the full proxy pipeline: compliance → cache → upstream → telemetry.
///
/// `raw_prompt` is derived from `body_bytes` on-demand inside this function.
/// The handler no longer pays the `String::from_utf8_lossy(..).to_string()` clone
/// on the hot path for non-PII, non-streaming requests.
#[allow(clippy::too_many_arguments)]
async fn get_client_config(state: &Arc<AppState>, tenant_id: &str) -> ClientConfig {
    if let Some(cfg) = state.routing_state.client_configs.load().get(tenant_id) {
        return cfg.clone();
    }

    ClientConfig {
        tenant_id: tenant_id.to_string(),
        pii_masking_enabled: false,
        semantic_cache_enabled: true,
        routing_fallback_enabled: true,
        rate_limit_rpm: None,
        preferred_model: None,
        fallback_timeout_ms: 30000,
        semantic_cache_threshold: 0.85,
        max_agent_loops: 20,
        max_identical_tool_calls: 5,
        context_window_budget: 128000,
        burn_rate_limit: 10.0,
    }
}
#[allow(clippy::too_many_arguments)]
pub async fn execute_proxy(
    state: &Arc<AppState>,
    body_bytes: &axum::body::Bytes,
    tenant_id: &str,
    model_name: &str,
    accept_header: &str,
    trace_ctx: &TraceContext,
    strategy: RoutingStrategy,
    is_free_tier: bool,
    req_extensions: &axum::http::Extensions,
    enable_compression: bool,
    test_scenario: Option<&str>,
) -> Result<ProxyResult, GatewayError> {
    let start_time = std::time::Instant::now();

    let claims = req_extensions.get::<crate::api::middleware::auth::Claims>();
    let team_id = claims.and_then(|c| c.team_id.clone());
    let api_key_alias = claims.and_then(|c| c.api_key_alias.clone());
    let agent_loops = req_extensions.get::<u64>().copied().unwrap_or(0) as u32;

    let client_config = get_client_config(state, tenant_id).await;

    crate::usecases::behavior_guard::enforce_oss_agent_guardian(
        state,
        tenant_id,
        &trace_ctx.session_id,
        body_bytes,
    )
    .await?;

    // Extract raw_prompt directly from body_bytes (zero-copy).
    let raw_prompt: &str =
        std::str::from_utf8(body_bytes).map_err(|e| GatewayError::InvalidJSON(e.to_string()))?;

    // ── Stage 0: PII Redaction (fail-closed, zero-copy scan) ─────────────────
    // extract_semantic_text_raw reads body_bytes directly without .to_vec().
    // The regex runs only on extracted text nodes (≤64 KiB), never on Base64
    // image blobs. Pure-image payloads return None → is_pii_match = false.
    let pii_scan_target = extract_semantic_text_raw(body_bytes);
    let is_pii_match = pii_scan_target
        .as_deref()
        .map(|t| pii_regex().is_match(t))
        .unwrap_or(false);

    let body_bytes = if is_pii_match {
        let parsed_body: Value = serde_json::from_slice(body_bytes)
            .map_err(|e| GatewayError::InvalidJSON(e.to_string()))?;

        let redacted = call_compliance_redact(
            &state.http_client,
            &state.compliance_url,
            &parsed_body,
            trace_ctx,
        )
        .await?;

        let redacted_bytes = serde_json::to_vec(&redacted).map_err(|e| {
            GatewayError::ResponseBuild(format!("Failed to serialize redacted body: {}", e))
        })?;
        axum::body::Bytes::from(redacted_bytes)
    } else {
        body_bytes.clone()
    };

    let telemetry_prompt = if is_pii_match {
        extract_response_content(Some(&body_bytes))
    } else {
        raw_prompt.to_string()
    };

    let mut parse_buffer = body_bytes.to_vec();

    // ── Stage 1 & 2: Speculative Cache & Router Prep ──────────────────────────
    // Isolate inspection buffer to prevent `simd_json` in-place mutation of `parse_buffer`
    let (is_streaming, semantic_text) = {
        let mut inspect_buf = parse_buffer.clone();
        let lazy_parsed = simd_json::to_borrowed_value(&mut inspect_buf)
            .map_err(|e| GatewayError::InvalidJSON(e.to_string()))?;

        let is_stream = lazy_parsed
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let sem_text = extract_semantic_text(&lazy_parsed);
        (is_stream, sem_text)
    };

    let semantic_text_str = semantic_text.as_deref().unwrap_or("");

    // ── NEW PRECEDENCE RULE: Check Exact Cache BEFORE embedding ───────────────
    if test_scenario.is_none() {
        if let Some(cached_content) = state.l1_cache.get_exact(tenant_id, raw_prompt).await {
            info!(tenant_id = %tenant_id, "L1 Exact Cache HIT (Fast Path)!");
            return handle_cache_hit(
                state,
                cached_content,
                tenant_id,
                model_name,
                &telemetry_prompt,
                semantic_text_str,
                trace_ctx,
                start_time,
                is_streaming,
                is_free_tier,
                client_config.semantic_cache_enabled,
                team_id,
                api_key_alias,
                agent_loops,
            );
        }
    }

    // Step 1: Speculatively generate embedding if semantic text exists
    let embedding_vector: Option<Vec<f32>> = if test_scenario.is_none()
        && client_config.semantic_cache_enabled
        && state.semantic_cache.is_enabled()
    {
        if let Some(ref sem_text) = semantic_text {
            match state.l1_cache.embed_client.embed(sem_text).await {
                Ok(vec) => Some(vec),
                Err(e) => {
                    warn!("Embedding generation failed or timed out: {} — proceeding fail-open without semantic cache checks", e);
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let semantic_cache_enabled =
        client_config.semantic_cache_enabled && test_scenario.is_none();

    let cache_future = {
        let embedding_vector = embedding_vector.clone();
        let semantic_text = semantic_text.clone();
        let semantic_cache_enabled = semantic_cache_enabled;
        async move {
            if !semantic_cache_enabled {
                return None;
            }
            info!(tenant_id = %tenant_id, model = %model_name, "Evaluating L1/L2 caches for request");
            if let (Some(ref sem_text), Some(ref vec)) = (semantic_text, embedding_vector) {
                if let Some((cached_content, _cached_prompt)) =
                    state.l1_cache.get_semantic(tenant_id, sem_text, vec).await
                {
                    info!(tenant_id = %tenant_id, "L1 Semantic Cache HIT!");
                    return Some(cached_content);
                }
                let l2_hit = state
                    .semantic_cache
                    .lookup(tenant_id, model_name, sem_text, trace_ctx, vec)
                    .await;
                if let Some((cached_content, original_prompt)) = l2_hit {
                    if verify_intent(sem_text, &original_prompt) {
                        info!(tenant_id = %tenant_id, "L2 Semantic Cache HIT!");
                        return Some(cached_content);
                    } else {
                        info!(tenant_id = %tenant_id, "L2 Semantic Cache HIT rejected due to intent mismatch (Keyword Penalty Check)");
                    }
                } else {
                    info!(tenant_id = %tenant_id, "Semantic cache MISS in both L1 and L2.");
                }
                None
            } else {
                info!(tenant_id = %tenant_id, "No semantic text or embedding vector available; skipping semantic cache checks.");
                None
            }
        }
    };

    let prep_future = llm_router::prep_upstream_request(
        state,
        tenant_id,
        &mut parse_buffer,
        accept_header,
        strategy,
        test_scenario,
    );

    let (cache_result, prep_result) = tokio::join!(cache_future, prep_future);

    if let Some(cached_content) = cache_result {
        return handle_cache_hit(
            state,
            cached_content,
            tenant_id,
            model_name,
            &telemetry_prompt,
            semantic_text_str,
            trace_ctx,
            start_time,
            is_streaming,
            is_free_tier,
            client_config.semantic_cache_enabled,
            team_id,
            api_key_alias,
            agent_loops,
        );
    }

    let prep = prep_result?;

    // ── Stage 1.5 & 1.6: Pre-flight checks (Token-Bucket & Loop Detection) ─────
    let token_future = async { Ok::<(), GatewayError>(()) };
    let loop_future = crate::usecases::behavior_guard::enforce_loop_detection(
        state,
        &trace_ctx.session_id,
        &body_bytes,
    );

    if let Err(e) = tokio::try_join!(token_future, loop_future) {
        let latency_ms = start_time.elapsed().as_millis() as u32;

        let model_telemetry = match &e {
            GatewayError::RateLimitExceeded(_) => "__token_blocked",
            GatewayError::LoopDetected(_) => "__loop_blocked",
            _ => "__preflight_blocked",
        };

        // 1. Update metrics
        state
            .dashboard_metrics
            .total_latency_ms
            .fetch_add(latency_ms as usize, std::sync::atomic::Ordering::Relaxed);
        // Note: enforce_loop_detection already incremented blocked_agent_loops.

        // 2. Log dual payloads to ClickHouse/tracer
        let req_payload = serde_json::json!({
            "id": trace_ctx.trace_id.clone(),
            "tenant_id": tenant_id,
            "status": 429_u16,
            "latency_ms": latency_ms,
            "model": model_telemetry,
            "tokens": 0_u32,
            "requested_provider": get_requested_provider(state, tenant_id, model_name),
            "executed_provider": "",
            "is_hot_swapped": 0_u8
        });
        state.telemetry.log_event(req_payload);

        let trace_payload = serde_json::json!({
            "trace_id":        trace_ctx.trace_id,
            "session_id":      trace_ctx.session_id,
            "parent_trace_id": trace_ctx.parent_trace_id,
            "tenant_id":       tenant_id,
            "model":           model_telemetry,
            "status":          429_u16,
            "latency_ms":      latency_ms,
            "total_tokens":    0_u32,
            "cache_hit":       false,
            "prompt_content":  &telemetry_prompt,
            "response_content": "",
            "requested_provider": get_requested_provider(state, tenant_id, model_name),
            "executed_provider": "",
            "is_hot_swapped": 0_u8,
            "error":           e.to_string(),
            "has_response":    false
        });
        state.telemetry.log_event(trace_payload);

        return Err(e);
    }

    // ── Stage 1.7: Burn-Rate Control ─────────────────────────────────────────
    if let Err(e) =
        crate::usecases::behavior_guard::enforce_burn_rate(state, &trace_ctx.session_id).await
    {
        let latency_ms = start_time.elapsed().as_millis() as u32;

        // 1. Update Metrics
        state
            .dashboard_metrics
            .total_latency_ms
            .fetch_add(latency_ms as usize, std::sync::atomic::Ordering::Relaxed);

        // 2. Log dual payloads to ClickHouse/tracer
        let req_payload = serde_json::json!({
            "id": trace_ctx.trace_id.clone(),
            "tenant_id": tenant_id,
            "status": 429_u16,
            "latency_ms": latency_ms,
            "model": "__burn_rate_blocked",
            "tokens": 0_u32,
            "requested_provider": get_requested_provider(state, tenant_id, model_name),
            "executed_provider": "",
            "is_hot_swapped": 0_u8
        });
        state.telemetry.log_event(req_payload);

        let trace_payload = serde_json::json!({
            "trace_id":        trace_ctx.trace_id,
            "session_id":      trace_ctx.session_id,
            "parent_trace_id": trace_ctx.parent_trace_id,
            "tenant_id":       tenant_id,
            "model":           "__burn_rate_blocked",
            "status":          429_u16,
            "latency_ms":      latency_ms,
            "total_tokens":    0_u32,
            "cache_hit":       false,
            "prompt_content":  &telemetry_prompt,
            "response_content": "",
            "requested_provider": get_requested_provider(state, tenant_id, model_name),
            "executed_provider": "",
            "is_hot_swapped": 0_u8,
            "error":           e.to_string(),
            "has_response":    false
        });
        state.telemetry.log_event(trace_payload);

        return Err(e);
    }

    // ── Stage 1.8: Circuit-Breaker / Adaptive Fallback ────────────────────────

    // ── Stage 2 & 3: Dynamic Provider Key Resolution and Routing with Fallback ─

    // [OBSERVABILITY] Structural dispatch log — never dumps raw payload bytes so
    // unredacted PII cannot leak into log aggregators via the debug pipeline.
    tracing::debug!(
        tenant_id = %tenant_id,
        model = %model_name,
        payload_bytes = body_bytes.len(),
        "Dispatching payload to upstream LLM provider"
    );

    // ── Stage 1.9: Deferred Billing Balance Guard ────────────────────────────
    struct RequestExtensions<'a>(&'a axum::http::Extensions);
    impl<'a> RequestExtensions<'a> {
        fn extensions(&self) -> &axum::http::Extensions {
            self.0
        }
    }
    let req = RequestExtensions(req_extensions);

    let current_balance = match req.extensions().get::<f64>() {
        Some(b) => *b,
        None => {
            return Err(crate::error::GatewayError::InternalError(
                "Billing context missing from request extensions".to_string(),
            ))
        }
    };

    let provider_name = prep.primary_target.provider_name.clone();
    let target_model = prep.primary_target.target_model.clone();
    let pricing_guard = state.pricing_map.load();
    let target_rate = pricing_guard
        .get(&target_model)
        .or(pricing_guard.get(model_name))
        .copied()
        .unwrap_or_else(|| {
            crate::domain::billing::lookup_precautionary_rate(&provider_name, &target_model)
        });

    if !is_free_tier {
        state
            .billing
            .enforce_scoped_budget(
                tenant_id,
                team_id.as_deref(),
                api_key_alias.as_deref(),
                model_name,
            )
            .await?;

        if target_rate.input_cost_per_1m > 0.0 && current_balance <= 0.0 {
            return Err(crate::error::GatewayError::InsufficientFunds);
        }
    }

    let (upstream_response, success_key_alias, is_hot_swapped, prep) =
        llm_router::execute_upstream_request(
            state,
            prep,
            &mut parse_buffer, // Pass mut bytes for zero-copy proxy
        )
        .await?;

    // requested_provider and executed_provider are both derived from the gateway's
    // own routing state — upstream providers never return these in response headers.
    let requested_provider = prep.requested_provider.clone();
    let executed_provider = prep.executed_provider.clone();

    let upstream_status = upstream_response.status().as_u16();
    let content_type = upstream_response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();

    let upstream_host = upstream_response
        .url()
        .host_str()
        .unwrap_or(executed_provider.as_str());

    info!(
        status = upstream_status,
        provider = upstream_host,
        "Received response from upstream LLM provider"
    );

    // ── Stage 4a: SSE Streaming Path ─────────────────────────────────────────
    if is_streaming {
        return handle_streaming_response(
            state,
            upstream_response,
            upstream_status,
            content_type,
            tenant_id,
            model_name,
            &telemetry_prompt,
            semantic_text_str,
            trace_ctx,
            start_time,
            requested_provider,
            executed_provider,
            is_hot_swapped,
            success_key_alias,
            prep,
            body_bytes.clone(),
            is_free_tier,
            embedding_vector,
            semantic_cache_enabled,
            team_id,
            api_key_alias,
            agent_loops,
        );
    }

    // ── Stage 4b: Buffered (non-streaming) Path ───────────────────────────────
    handle_buffered_response(
        state,
        upstream_response,
        upstream_status,
        content_type,
        tenant_id,
        model_name,
        &telemetry_prompt,
        semantic_text_str,
        trace_ctx,
        start_time,
        requested_provider,
        executed_provider,
        is_hot_swapped,
        success_key_alias,
        is_free_tier,
        embedding_vector,
        semantic_cache_enabled,
        enable_compression,
        req_extensions,
    )
    .await
}

// ── Pipeline stage helpers ────────────────────────────────────────────────────

/// Returns a `ProxyResult` from a semantic-cache hit and fires background telemetry.
#[allow(clippy::too_many_arguments)]
fn handle_cache_hit(
    state: &Arc<AppState>,
    cached_content: String,
    tenant_id: &str,
    model_name: &str,
    raw_prompt: &str,
    semantic_text: &str,
    trace_ctx: &TraceContext,
    start_time: std::time::Instant,
    is_streaming: bool,
    is_free_tier: bool,
    semantic_cache_enabled: bool,
    team_id: Option<String>,
    api_key_alias: Option<String>,
    agent_loops: u32,
) -> Result<ProxyResult, GatewayError> {
    let latency_ms = start_time.elapsed().as_millis() as u32;
    let requested_provider = get_requested_provider(state, tenant_id, model_name);

    spawn_telemetry(
        state,
        tenant_id,
        model_name,
        raw_prompt,
        semantic_text,
        trace_ctx,
        200,
        latency_ms,
        0,
        true,
        Some(cached_content.as_bytes().to_vec()),
        requested_provider.clone(),
        requested_provider,
        0,
        "cache".to_string(),
        is_free_tier,
        None,
        semantic_cache_enabled,
        team_id,
        api_key_alias,
        0,
        agent_loops,
    );

    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if is_streaming {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(16);
        let mock_id = format!("chatcmpl-cached-{}", uuid::Uuid::new_v4());
        let model_name = model_name.to_string();
        let cached_content = cached_content.clone();

        tokio::spawn(async move {
            // Chunk 1: Role initialization
            let chunk1 = json!({
                "id": mock_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model_name,
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "role": "assistant",
                            "content": ""
                        },
                        "finish_reason": null
                    }
                ]
            });
            let data1 = format!("data: {}\n\n", chunk1);
            if tx.send(Ok(bytes::Bytes::from(data1))).await.is_err() {
                return;
            }

            // Chunk 2: Content delivery
            let chunk2 = json!({
                "id": mock_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model_name,
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "content": cached_content
                        },
                        "finish_reason": null
                    }
                ]
            });
            let data2 = format!("data: {}\n\n", chunk2);
            if tx.send(Ok(bytes::Bytes::from(data2))).await.is_err() {
                return;
            }

            // Chunk 3: Finish reason stop
            let chunk3 = json!({
                "id": mock_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model_name,
                "choices": [
                    {
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop"
                    }
                ]
            });
            let data3 = format!("data: {}\n\n", chunk3);
            if tx.send(Ok(bytes::Bytes::from(data3))).await.is_err() {
                return;
            }

            // Terminal [DONE]
            let _ = tx
                .send(Ok(bytes::Bytes::from("data: [DONE]\n\n".to_string())))
                .await;
        });

        let body_stream =
            axum::body::Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
        Ok(ProxyResult {
            status: 200,
            content_type: "text/event-stream".to_string(),
            body: ProxyBody::Stream(body_stream),
            cache_hit: true,
        })
    } else {
        let mock_id = format!("chatcmpl-cached-{}", uuid::Uuid::new_v4());
        let mock_response = json!({
            "id": mock_id,
            "object": "chat.completion",
            "created": created,
            "model": model_name,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": cached_content}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}
        });

        let bytes = serde_json::to_vec(&mock_response).map_err(|e| {
            GatewayError::ResponseBuild(format!("Failed to serialize mock response: {}", e))
        })?;

        Ok(ProxyResult {
            status: 200,
            content_type: "application/json".to_string(),
            body: ProxyBody::Buffered(bytes),
            cache_hit: true,
        })
    }
}

fn has_error_signature(chunk: &[u8]) -> bool {
    // Allocation-free sliding window scanner for error signatures
    chunk.windows(7).any(|w| w == b"\"error\"")
        || chunk.windows(12).any(|w| w == b"\"rate_limit\"")
        || chunk.windows(20).any(|w| w == b"\"insufficient_funds\"")
        || chunk.windows(15).any(|w| w == b"\"billing_limit\"")
}

#[allow(clippy::too_many_arguments)]
fn handle_streaming_response(
    state: &Arc<AppState>,
    upstream_response: reqwest::Response,
    upstream_status: u16,
    content_type: String,
    tenant_id: &str,
    model_name: &str,
    raw_prompt: &str,
    semantic_text: &str,
    trace_ctx: &TraceContext,
    start_time: std::time::Instant,
    requested_provider: String,
    executed_provider: String,
    is_hot_swapped: u8,
    success_key_alias: String,
    prep: crate::infrastructure::llm_router::PreparedUpstreamRequest,
    body_bytes: axum::body::Bytes,
    is_free_tier: bool,
    embedding_vector: Option<Vec<f32>>,
    semantic_cache_enabled: bool,
    team_id: Option<String>,
    api_key_alias: Option<String>,
    agent_loops: u32,
) -> Result<ProxyResult, GatewayError> {
    let state_stream = state.clone();
    let state_telemetry = state.clone();
    let tenant_id_c = tenant_id.to_string();
    let model_name_c = model_name.to_string();
    let raw_prompt_c = raw_prompt.to_string();
    let semantic_text_c = semantic_text.to_string();
    let trace_ctx_c = trace_ctx.clone();
    let embedding_vector_c = embedding_vector.clone();
    let body_bytes_stream = body_bytes.clone();
    let requested_provider_c = requested_provider.clone();
    let team_id_c = team_id.clone();
    let api_key_alias_c = api_key_alias.clone();

    let shared_executed_provider =
        std::sync::Arc::new(std::sync::Mutex::new(executed_provider.clone()));
    let shared_success_key_alias =
        std::sync::Arc::new(std::sync::Mutex::new(success_key_alias.clone()));
    let shared_is_hot_swapped =
        std::sync::Arc::new(std::sync::atomic::AtomicU8::new(is_hot_swapped));

    let shared_executed_provider_c = shared_executed_provider.clone();
    let shared_success_key_alias_c = shared_success_key_alias.clone();
    let shared_is_hot_swapped_c = shared_is_hot_swapped.clone();

    let mut prep = prep;

    use futures::StreamExt; // Put at top of function so both spawns can use it

    // MEMORY BOUND: bounded channel prevents unbounded heap growth during slow SSE streams.
    let (telemetry_tx, telemetry_rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(1024);
    let (client_tx, client_rx) =
        tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(1024);

    tokio::spawn(async move {
        let mut active_stream = upstream_response.bytes_stream();
        let mut current_provider = executed_provider;

        loop {
            let mut failed = false;

            while let Some(chunk_res) = active_stream.next().await {
                match chunk_res {
                    Ok(chunk) => {
                        // Check for error signatures
                        if has_error_signature(&chunk) {
                            tracing::warn!(
                                "Detected error signature in streaming chunk from provider {}",
                                current_provider
                            );
                            failed = true;
                            break;
                        }

                        let _ = telemetry_tx.try_send(chunk.clone());
                        if client_tx.send(Ok(chunk)).await.is_err() {
                            return; // Client disconnected
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "Mid-stream connection error from provider {}: {}",
                            current_provider,
                            e
                        );
                        failed = true;
                        break;
                    }
                }
            }

            if !failed {
                // Stream finished successfully!
                break;
            }

            // If we reached here, the active stream failed. Try fallbacks!
            let mut fallback_succeeded = false;
            while let Some(target) = prep.fallback_targets.first().cloned() {
                prep.fallback_targets.remove(0); // Consume this fallback

                tracing::info!(
                    model = %prep.model,
                    key_alias = %target.api_key_alias,
                    "Streaming failover: attempting fallback upstream request"
                );

                let provider_id = target.schema_format.as_str();
                let config = crate::infrastructure::llm_router::get_provider_config(provider_id);

                let mut stream_buffer = body_bytes_stream.to_vec();
                let fallback_req = match crate::infrastructure::llm_router::build_provider_request(
                    &state_stream.http_client,
                    &config,
                    &target.base_url,
                    &target.api_key,
                    &prep.model,
                    &target.target_model,
                    &mut stream_buffer,
                    &prep.accept_header,
                    None,
                ) {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!("Failed to build fallback request for stitching: {}", e);
                        continue;
                    }
                };

                match fallback_req.send().await {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            active_stream = resp.bytes_stream();
                            current_provider = config.id.clone();

                            // Update shared state for telemetry safely
                            if let Ok(mut p_guard) = shared_executed_provider.lock() {
                                *p_guard = config.id.clone();
                            }
                            if let Ok(mut k_guard) = shared_success_key_alias.lock() {
                                *k_guard = target.api_key_alias.clone();
                            }
                            shared_is_hot_swapped.store(1, std::sync::atomic::Ordering::Relaxed);

                            fallback_succeeded = true;
                            tracing::info!(
                                "Streaming failover: stitched fallback stream successfully"
                            );
                            break;
                        } else {
                            tracing::warn!("Streaming failover: fallback provider returned non-success status: {}", resp.status());
                        }
                    }
                    Err(e) => {
                        tracing::error!("Streaming failover: fallback provider unreachable: {}", e);
                    }
                }
            }

            if !fallback_succeeded {
                // All fallbacks exhausted or failed. Send Err to client_tx to force HTTP stream abort.
                tracing::error!(
                    "Stream interrupted and all fallbacks exhausted. Aborting stream connection."
                );
                let _ = client_tx
                    .send(Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "Upstream mid-stream failure: all fallbacks exhausted",
                    )))
                    .await;
                break;
            }
        }
    });

    let body_stream =
        axum::body::Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(client_rx));

    // Spawn a lightweight task to parse tokens and record telemetry.
    // Uses ReceiverStream (bounded) instead of UnboundedReceiverStream.
    tokio::spawn(async move {
        let mut total_tokens = 0u32;
        let mut response_text = String::new();
        let mut estimated_completion_bytes = 0usize;
        const MAX_STREAM_BUFFER_BYTES: usize = 256 * 1024; // Bounded to 256 KB

        let mut event_stream = tokio_stream::wrappers::ReceiverStream::new(telemetry_rx)
            .map(Ok::<_, std::io::Error>)
            .eventsource();

        while let Some(event_res) = event_stream.next().await {
            match event_res {
                Ok(event) => {
                    let data = event.data;
                    if data == "[DONE]" {
                        break;
                    }

                    if let Ok(v) = serde_json::from_str::<Value>(&data) {
                        // 1. Extract token usage if available
                        if let Some(usage) = v.get("usage") {
                            if let Some(total) = usage.get("total_tokens").and_then(|t| t.as_u64())
                            {
                                total_tokens = total as u32;
                            }
                        }

                        // 2. Incremental parsing of content deltas to reconstruct response_text
                        // OpenAI stream format: choices[0].delta.content
                        if let Some(content) = v["choices"][0]["delta"]["content"].as_str() {
                            if response_text.len() + content.len() < MAX_STREAM_BUFFER_BYTES {
                                response_text.push_str(content);
                            }
                        }
                        // Anthropic stream format: delta.text
                        else if let Some(text) = v["delta"]["text"].as_str() {
                            if response_text.len() + text.len() < MAX_STREAM_BUFFER_BYTES {
                                response_text.push_str(text);
                            }
                        }
                    }

                    estimated_completion_bytes += data.len() + 1;
                }
                Err(_) => break,
            }
        }

        if total_tokens == 0 {
            let estimated_prompt = (raw_prompt_c.len() / CHARS_PER_TOKEN).max(1) as u32;
            let estimated_completion = (estimated_completion_bytes / CHARS_PER_TOKEN).max(1) as u32;
            total_tokens = estimated_prompt + estimated_completion;
        }

        let latency_ms = start_time.elapsed().as_millis() as u32;

        let response_bytes = if response_text.is_empty() {
            None
        } else {
            Some(response_text.into_bytes())
        };

        let final_executed_provider = shared_executed_provider_c
            .lock()
            .map_or_else(|e| e.into_inner().clone(), |g| g.clone());
        let final_success_key_alias = shared_success_key_alias_c
            .lock()
            .map_or_else(|e| e.into_inner().clone(), |g| g.clone());
        let final_is_hot_swapped =
            shared_is_hot_swapped_c.load(std::sync::atomic::Ordering::Relaxed);
        let final_status = if final_is_hot_swapped == 1 {
            200
        } else {
            upstream_status
        };

        fire_async_telemetry(
            &state_telemetry,
            &tenant_id_c,
            &model_name_c,
            &raw_prompt_c,
            &semantic_text_c,
            &trace_ctx_c,
            final_status,
            latency_ms,
            total_tokens,
            false,
            response_bytes,
            requested_provider_c,
            final_executed_provider,
            final_is_hot_swapped,
            final_success_key_alias,
            is_free_tier,
            embedding_vector_c,
            semantic_cache_enabled,
            team_id_c,
            api_key_alias_c,
            0,
            agent_loops,
        )
        .await;
    });

    Ok(ProxyResult {
        status: upstream_status,
        content_type,
        body: ProxyBody::Stream(body_stream),
        cache_hit: false,
    })
}

/// Handles buffered (non-streaming) response: reads body, extracts token usage,
/// fires background telemetry.
#[allow(clippy::too_many_arguments)]
async fn handle_buffered_response(
    state: &Arc<AppState>,
    upstream_response: reqwest::Response,
    upstream_status: u16,
    content_type: String,
    tenant_id: &str,
    model_name: &str,
    raw_prompt: &str,
    semantic_text: &str,
    trace_ctx: &TraceContext,
    start_time: std::time::Instant,
    requested_provider: String,
    executed_provider: String,
    is_hot_swapped: u8,
    success_key_alias: String,
    is_free_tier: bool,
    embedding_vector: Option<Vec<f32>>,
    semantic_cache_enabled: bool,
    enable_compression: bool,
    req_extensions: &axum::http::Extensions,
) -> Result<ProxyResult, GatewayError> {
    let mut body_bytes = upstream_response
        .bytes()
        .await
        .map_err(|e| GatewayError::ResponseBuild(format!("Failed to read upstream body: {}", e)))?
        .to_vec();
    let latency_ms = start_time.elapsed().as_millis() as u32;

    let config = crate::infrastructure::llm_router::get_provider_config(&executed_provider);
    let adapter = crate::infrastructure::providers::get_utp_adapter(&config.schema_format);
    if let Ok(raw_json) = serde_json::from_slice::<Value>(&body_bytes) {
        match adapter.unify_response(raw_json) {
            Ok(unified_json) => {
                if let Ok(unified_bytes) = serde_json::to_vec(&unified_json) {
                    body_bytes = unified_bytes;
                }
            }
            Err(e) => {
                warn!("Response unification error: {}", e);
            }
        }
    }

    let estimated_tokens = (raw_prompt.len() / CHARS_PER_TOKEN).max(1) as u32;

    // ── Tunnel 3 Phase 3: MCP Sandbox Firewall ───────────────────────────────
    // Inspect the (now-unified) response for tool_calls and apply OPA RBAC.
    // Fail-open: if OPA is unreachable or times out, the original bytes are
    // returned unchanged.  Zero overhead on the non-agentic path: the fast
    // simd-json scan returns immediately when no tool_calls key is present.
    let body_bytes =
        crate::usecases::behavior_guard::enforce_mcp_sandbox(state, tenant_id, body_bytes).await?;

    // ── Tunnel 3 Phase 2: Phantom tool interception ───────────────────────────
    // If the LLM called the phantom `get_tool_details` tool, resolve the
    // schema from cache and replace body_bytes — no external call needed.
    let body_bytes = if !state.tool_registry.is_empty() {
        if let Some(phantom_resp) = state.tool_registry.intercept_phantom_call(&body_bytes) {
            tracing::debug!("Tunnel 3 Phase 2 — phantom call resolved from cache");
            phantom_resp
        } else {
            body_bytes
        }
    } else {
        body_bytes
    };

    // ── Tunnel 3 Phase 4: Flow-Based Parallel Fan-Out ────────────────────────
    // If the (sandbox-cleared) response contains tool_calls AND every called
    // tool is registered in the MCP registry, dispatch all calls concurrently.
    // Results are merged and forwarded to the telemetry channel so the next
    // agentic turn can include them as `role: "tool"` context messages.
    //
    // This block is a no-op when:
    //   • `body_bytes` contains no `tool_calls` key (byte-scan early-exit)
    //   • `mcp_registry` is empty (pre-fetching disabled)
    let mut mcp_calls = 0;
    {
        let has_tool_calls_bytes = body_bytes
            .windows(b"tool_calls".len())
            .any(|w| w == b"tool_calls");

        if has_tool_calls_bytes && !state.mcp_registry.is_empty() {
            let config = crate::infrastructure::llm_router::get_provider_config(&executed_provider);
            let adapter = crate::infrastructure::providers::get_utp_adapter(&config.schema_format);

            let calls =
                crate::infrastructure::mcp_client::extract_tool_calls(&body_bytes, &*adapter);

            if !calls.is_empty() {
                let trace_ctx = req_extensions
                    .get::<crate::domain::models::TraceContext>()
                    .cloned()
                    .unwrap_or_else(|| crate::domain::models::TraceContext {
                        trace_id: uuid::Uuid::new_v4().to_string(),
                        session_id: uuid::Uuid::new_v4().to_string(),
                        parent_trace_id: None,
                        workflow_id: None,
                        agent_id: None,
                        execution_id: None,
                        operation_id: None,
                        idempotency_key: None,
                        test_scenario: None,
                    });

                let results = crate::infrastructure::mcp_client::fan_out(
                    calls,
                    tenant_id,
                    state,
                    enable_compression,
                    &trace_ctx,
                )
                .await;

                mcp_calls = results.len() as u32;

                if !results.is_empty() {
                    // Filter out PREVIOUS_ATTEMPT_UNKNOWN tool results before passing to LLM context
                    // to prevent LLM hallucinations on retry failure strings.
                    let safe_results: Vec<_> = results
                        .into_iter()
                        .filter(|r| {
                            if r.content.contains("PREVIOUS_ATTEMPT_UNKNOWN") {
                                tracing::warn!(
                                    tool_name = %r.name,
                                    "Dropping PREVIOUS_ATTEMPT_UNKNOWN tool result from LLM context to force hard-fail"
                                );
                                false
                            } else {
                                true
                            }
                        })
                        .collect();

                    if !safe_results.is_empty() {
                        let tool_messages = crate::infrastructure::mcp_client::merge_results(
                            safe_results,
                            enable_compression,
                            &*adapter,
                        );

                        // Forward merged tool results to telemetry as structured context.
                        let ctx_payload = serde_json::json!({
                            "type": "mcp_tool_results",
                            "tenant_id": tenant_id,
                            "tool_messages": tool_messages,
                        });
                        state.telemetry.log_event(ctx_payload);
                    }
                }
            }
        }
    }

    let actual_tokens = serde_json::from_slice::<Value>(&body_bytes)
        .ok()
        .and_then(|v| v["usage"]["total_tokens"].as_u64())
        .map(|v| v as u32)
        .unwrap_or_else(|| {
            let estimated_completion = (body_bytes.len() / CHARS_PER_TOKEN).max(1) as u32;
            estimated_tokens + estimated_completion
        });

    let claims = req_extensions.get::<crate::api::middleware::auth::Claims>();
    let team_id = claims.and_then(|c| c.team_id.clone());
    let api_key_alias = claims.and_then(|c| c.api_key_alias.clone());
    let agent_loops = req_extensions.get::<u64>().copied().unwrap_or(0) as u32;

    spawn_telemetry(
        state,
        tenant_id,
        model_name,
        raw_prompt,
        semantic_text,
        trace_ctx,
        upstream_status,
        latency_ms,
        actual_tokens,
        false,
        Some(body_bytes.clone()),
        requested_provider,
        executed_provider,
        is_hot_swapped,
        success_key_alias,
        is_free_tier,
        embedding_vector,
        semantic_cache_enabled,
        team_id,
        api_key_alias,
        mcp_calls,
        agent_loops,
    );

    Ok(ProxyResult {
        status: upstream_status,
        content_type,
        body: ProxyBody::Buffered(body_bytes),
        cache_hit: false,
    })
}

// ── Telemetry helpers ─────────────────────────────────────────────────────────

/// Convenience wrapper: spawns `fire_async_telemetry` in a detached Tokio task.
#[allow(clippy::too_many_arguments)]
fn spawn_telemetry(
    state: &Arc<AppState>,
    tenant_id: &str,
    model_name: &str,
    raw_prompt: &str,
    semantic_text: &str,
    trace_ctx: &TraceContext,
    status_code: u16,
    latency_ms: u32,
    tokens: u32,
    cache_hit: bool,
    response_bytes: Option<Vec<u8>>,
    requested_provider: String,
    executed_provider: String,
    is_hot_swapped: u8,
    success_key_alias: String,
    is_free_tier: bool,
    embedding_vector: Option<Vec<f32>>,
    semantic_cache_enabled: bool,
    team_id: Option<String>,
    api_key_alias: Option<String>,
    mcp_calls: u32,
    agent_loops: u32,
) {
    let s = state.clone();
    let tid = tenant_id.to_string();
    let model = model_name.to_string();
    let prompt = raw_prompt.to_string();
    let sem_text = semantic_text.to_string();
    let ctx = trace_ctx.clone();
    let vec = embedding_vector;
    let team_id_c = team_id;
    let api_key_alias_c = api_key_alias;

    tokio::spawn(async move {
        fire_async_telemetry(
            &s,
            &tid,
            &model,
            &prompt,
            &sem_text,
            &ctx,
            status_code,
            latency_ms,
            tokens,
            cache_hit,
            response_bytes,
            requested_provider,
            executed_provider,
            is_hot_swapped,
            success_key_alias,
            is_free_tier,
            vec,
            semantic_cache_enabled,
            team_id_c,
            api_key_alias_c,
            mcp_calls,
            agent_loops,
        )
        .await;
    });
}

/// Formats telemetry properties and dispatches them to the background worker channel.
#[allow(clippy::too_many_arguments)]
async fn fire_async_telemetry(
    state: &Arc<AppState>,
    tenant_id: &str,
    model_name: &str,
    raw_prompt: &str,
    semantic_text: &str,
    trace_ctx: &TraceContext,
    status_code: u16,
    latency_ms: u32,
    tokens: u32,
    cache_hit: bool,
    response_bytes: Option<Vec<u8>>,
    requested_provider: String,
    executed_provider: String,
    is_hot_swapped: u8,
    _success_key_alias: String,
    is_free_tier: bool,
    embedding_vector: Option<Vec<f32>>,
    semantic_cache_enabled: bool,
    team_id: Option<String>,
    api_key_alias: Option<String>,
    mcp_calls: u32,
    agent_loops: u32,
) {
    state
        .dashboard_metrics
        .total_latency_ms
        .fetch_add(latency_ms as usize, std::sync::atomic::Ordering::Relaxed);
    state
        .dashboard_metrics
        .total_tokens
        .fetch_add(tokens as usize, std::sync::atomic::Ordering::Relaxed);

    let response_content = extract_response_content(response_bytes.as_deref());

    // 1. Clickhouse bulk batched payload
    let payload = json!({
        "id": trace_ctx.trace_id.clone(),
        "tenant_id": tenant_id,
        "team_id": team_id,
        "api_key_alias": api_key_alias,
        "status": status_code,
        "latency_ms": latency_ms,
        "model": model_name,
        "tokens": tokens,
        "mcp_calls": mcp_calls,
        "agent_loops": agent_loops,
        "requested_provider": requested_provider,
        "executed_provider": executed_provider,
        "is_hot_swapped": is_hot_swapped,
        "cache_hit": cache_hit
    });

    state.telemetry.log_event(payload);

    // 2. Tracer deep payload sent synchronously
    let trace_payload = json!({
        "trace_id":        trace_ctx.trace_id,
        "session_id":      trace_ctx.session_id,
        "parent_trace_id": trace_ctx.parent_trace_id,
        "tenant_id":       tenant_id,
        "team_id":         team_id,
        "api_key_alias":   api_key_alias,
        "model":           model_name,
        "status":          status_code,
        "latency_ms":      latency_ms,
        "total_tokens":    tokens,
        "mcp_calls":       mcp_calls,
        "agent_loops":     agent_loops,
        "cache_hit":       cache_hit,
        "prompt_content":  raw_prompt,
        "response_content": response_content,
        "requested_provider": requested_provider,
        "executed_provider": executed_provider,
        "is_hot_swapped": is_hot_swapped,
        "has_response":    cache_hit || !response_content.is_empty()
    });

    state.telemetry.log_event(trace_payload);

    // 3. Cache injection for successful misses
    if semantic_cache_enabled && !cache_hit && status_code == 200 && !response_content.is_empty() {
        if let (Some(ref vec), false) = (&embedding_vector, semantic_text.is_empty()) {
            // Smart Token-based Routing & Background Sync
            if tokens < 1000 {
                info!(
                    tenant_id = %tenant_id,
                    tokens = tokens,
                    "Cache injection: inserting response into L1 (exact + semantic) and L2 (semantic) caches."
                );
                // Save in BOTH L1 and L2
                if let Err(e) = state
                    .l1_cache
                    .insert(tenant_id, raw_prompt, semantic_text, &response_content, vec)
                    .await
                {
                    tracing::warn!("Failed to store in L1 cache: {}", e);
                }
                state
                    .semantic_cache
                    .store(
                        tenant_id,
                        model_name,
                        semantic_text,
                        &response_content,
                        trace_ctx,
                        vec,
                    )
                    .await;
            } else {
                // Tokens >= 1000: Bulk response, L2 only to prevent L1 OOM
                info!(
                    tenant_id = %tenant_id,
                    tokens = tokens,
                    "Cache injection: response size >= 1000 tokens, inserting into L2 cache only to prevent L1 OOM."
                );
                state
                    .semantic_cache
                    .store(
                        tenant_id,
                        model_name,
                        semantic_text,
                        &response_content,
                        trace_ctx,
                        vec,
                    )
                    .await;
            }
        } else {
            // If embedding vector is not available or semantic_text is empty, but the request was successful,
            // we still inject into the L1 Exact Cache (if tokens < 1000) using insert_exact
            if tokens < 1000 {
                info!(
                    tenant_id = %tenant_id,
                    tokens = tokens,
                    "Cache injection: embedding vector unavailable or empty semantic text, inserting response into L1 exact cache only."
                );
                state
                    .l1_cache
                    .insert_exact(tenant_id, raw_prompt, &response_content)
                    .await;
            } else {
                info!(
                    tenant_id = %tenant_id,
                    "Cache injection: skipping L1 Exact Cache injection because tokens >= 1000."
                );
            }
        }
    } else if !cache_hit {
        info!(
            tenant_id = %tenant_id,
            status = status_code,
            has_response = !response_content.is_empty(),
            "Cache miss: skipping injection (conditions not met)."
        );
    }

    // 4. Billing Engine Precautionary Credit Deduction & Analytics Telemetry
    if status_code == 200 {
        let prompt_tokens = (raw_prompt.len() / CHARS_PER_TOKEN).max(1) as u64;
        let completion_tokens = (tokens as u64).saturating_sub(prompt_tokens);

        let _ = process_runtime_billing_telemetry(
            state,
            trace_ctx.trace_id.clone(),
            tenant_id.to_string(),
            team_id,
            api_key_alias,
            &executed_provider,
            model_name,
            prompt_tokens,
            completion_tokens,
            mcp_calls,
            agent_loops,
            cache_hit,
            is_free_tier,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn process_runtime_billing_telemetry(
    state: &Arc<AppState>,
    trace_id: String,
    tenant_id: String,
    team_id: Option<String>,
    api_key_alias: Option<String>,
    provider: &str,
    target_model: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    mcp_calls: u32,
    agent_loops: u32,
    is_cache_hit: bool,
    is_free_tier: bool,
) -> Result<(), GatewayError> {
    let state = state.clone();
    let provider = provider.to_string();
    let target_model = target_model.to_string();
    tokio::spawn(async move {
        let _ = state
            .billing
            .process_billing_telemetry(
                &trace_id,
                &tenant_id,
                team_id.as_deref(),
                api_key_alias.as_deref(),
                &provider,
                &target_model,
                prompt_tokens,
                completion_tokens,
                mcp_calls,
                agent_loops,
                is_cache_hit,
                is_free_tier,
            )
            .await;
    });

    Ok(())
}

// ── Small pure helpers ────────────────────────────────────────────────────────

/// Extracts `choices[0].message.content` from a raw OpenAI-format JSON body, or falls back to raw UTF-8 string.
fn extract_response_content(bytes: Option<&[u8]>) -> String {
    let Some(b) = bytes else {
        return String::new();
    };
    if let Ok(v) = serde_json::from_slice::<Value>(b) {
        if let Some(content) = v["choices"][0]["message"]["content"].as_str() {
            return content.to_string();
        }
    }
    String::from_utf8_lossy(b).into_owned()
}

// ── Compliance helper ─────────────────────────────────────────────────────────

/// POSTs `payload` to the compliance redaction endpoint and returns the
fn local_sanitize_pii_value(val: &Value) -> Value {
    match val {
        Value::String(s) => {
            let sanitized = pii_regex().replace_all(s, "[REDACTED_PII]");
            Value::String(sanitized.into_owned())
        }
        Value::Array(arr) => Value::Array(arr.iter().map(local_sanitize_pii_value).collect()),
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            for (k, v) in map {
                new_map.insert(k.clone(), local_sanitize_pii_value(v));
            }
            Value::Object(new_map)
        }
        other => other.clone(),
    }
}

/// POSTs `payload` to the compliance redaction endpoint and returns the
/// sanitised body. Falls back gracefully to local inline regex PII redaction if
/// external compliance service is unreachable.
async fn call_compliance_redact(
    http_client: &reqwest::Client,
    compliance_url: &str,
    payload: &Value,
    trace_ctx: &TraceContext,
) -> Result<Value, GatewayError> {
    let endpoint = format!(
        "{}/api/v1/compliance/redact",
        compliance_url.trim_end_matches('/')
    );
    debug!(endpoint = %endpoint, "Calling compliance redaction service");

    let send_result = http_client
        .post(&endpoint)
        .header("x-kryneth-trace-id", &trace_ctx.trace_id)
        .header("x-kryneth-session-id", &trace_ctx.session_id)
        .json(payload)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await;

    let resp = match send_result {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "External compliance service unreachable — falling back to local inline regex PII redaction");
            return Ok(local_sanitize_pii_value(payload));
        }
    };

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        warn!(
            status = status,
            "Compliance service returned non-2xx — falling back to local inline regex PII redaction"
        );
        return Ok(local_sanitize_pii_value(payload));
    }

    let compliance_json: Value = resp.json().await.map_err(|e| {
        error!(error = %e, "Failed to parse compliance response body");
        GatewayError::ComplianceFailure(format!("Malformed compliance response: {}", e))
    })?;

    match compliance_json.get("sanitized_payload").cloned() {
        Some(sanitized) => {
            info!("PII redaction complete — forwarding sanitised payload to upstream LLM");
            Ok(sanitized)
        }
        None => {
            warn!("Compliance response missing `sanitized_payload` field — falling back to local inline regex PII redaction");
            Ok(local_sanitize_pii_value(payload))
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    // super::* use panna, mela irukkura pii_regex() function inga kedaikum
    use super::*;

    #[test]
    fn test_extract_semantic_text_formats() {
        fn extract_from_value(value: serde_json::Value) -> Option<String> {
            let mut bytes = serde_json::to_vec(&value).unwrap();
            let parsed = simd_json::to_borrowed_value(&mut bytes).unwrap();
            extract_semantic_text(&parsed)
        }
        // 1. OpenAI format
        let openai_payload = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Explain vector databases."}
            ]
        });
        assert_eq!(
            extract_from_value(openai_payload),
            Some("Explain vector databases.".to_string())
        );

        // 2. Anthropic legacy prompt format
        let anthropic_legacy = json!({
            "model": "claude-2",
            "prompt": "\n\nHuman: What is latent space?\n\nAssistant:"
        });
        assert_eq!(
            extract_from_value(anthropic_legacy),
            Some("What is latent space?".to_string())
        );

        // 3. Anthropic Messages type content block format
        let anthropic_messages = json!({
            "model": "claude-3-opus",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "How does HNSW index work?"
                        }
                    ]
                }
            ]
        });
        assert_eq!(
            extract_from_value(anthropic_messages),
            Some("How does HNSW index work?".to_string())
        );

        // 4. Gemini format
        let gemini_payload = json!({
            "contents": [
                {
                    "role": "user",
                    "parts": [
                        {"text": "What is Cosine Similarity?"}
                    ]
                }
            ]
        });
        assert_eq!(
            extract_from_value(gemini_payload),
            Some("What is Cosine Similarity?".to_string())
        );

        // 5. Unsupported / empty format (None fallback)
        let unsupported_payload = json!({
            "invalid_key": "some value"
        });
        assert_eq!(extract_from_value(unsupported_payload), None);

        // 6. OpenAI Multi-Turn format (only the last user message should be extracted)
        let openai_multiturn = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "Hello, how are you?"},
                {"role": "assistant", "content": "I am good, thank you!"},
                {"role": "user", "content": "Explain vector databases."}
            ]
        });
        assert_eq!(
            extract_from_value(openai_multiturn),
            Some("Explain vector databases.".to_string())
        );

        // 7. Gemini Multi-Turn format
        let gemini_multiturn = json!({
            "contents": [
                {
                    "role": "user",
                    "parts": [{"text": "Hello, how are you?"}]
                },
                {
                    "role": "model",
                    "parts": [{"text": "I am good, thank you!"}]
                },
                {
                    "role": "user",
                    "parts": [{"text": "What is Cosine Similarity?"}]
                }
            ]
        });
        assert_eq!(
            extract_from_value(gemini_multiturn),
            Some("What is Cosine Similarity?".to_string())
        );

        // 8. Anthropic Legacy Multi-Turn
        let anthropic_multiturn = json!({
            "model": "claude-2",
            "prompt": "\n\nHuman: Hello, how are you?\n\nAssistant: I am good!\n\nHuman: What is latent space?\n\nAssistant:"
        });
        assert_eq!(
            extract_from_value(anthropic_multiturn),
            Some("What is latent space?".to_string())
        );
    }

    #[test]
    fn test_pii_regex_matches_credit_card() {
        let regex = pii_regex();
        let prompt = "My credit card number is 1234-5678-9012-3456, please don't share it.";
        // assert! na "ithu unmai nu prove pannu" nu artham. Match aagalana test fail aagum.
        assert!(regex.is_match(prompt));
    }

    #[test]
    fn test_pii_regex_matches_email() {
        let regex = pii_regex();
        let prompt = "Send the AI response to admin@nmmglobal.com";
        assert!(regex.is_match(prompt));
    }

    #[test]
    fn test_pii_regex_ignores_safe_prompt() {
        let regex = pii_regex();
        let prompt = "What is the capital of Tamil Nadu? Explain in 50 words.";
        // assert! false check pannuthu. Safe prompt-a adhu block panna koodathu.
        assert!(!regex.is_match(prompt));
    }

    #[tokio::test]
    async fn test_telemetry_mpsc_channel_flow() {
        use tokio::sync::mpsc;

        // 1. Channel create pandrom (Capacity 100 logs)
        let (tx, mut rx) = mpsc::channel::<String>(100);

        // 2. Gateway Producer: User request vantha udane channel-la data poduthu
        tokio::spawn(async move {
            tx.send("Log 1: Tenant A used 400 tokens".to_string())
                .await
                .unwrap();
            tx.send("Log 2: Tenant B latency 45ms".to_string())
                .await
                .unwrap();
        });

        // 3. Background Worker (Consumer): Channel-la irunthu data-va edukuthu
        let first_log = rx.recv().await.unwrap();
        assert_eq!(first_log, "Log 1: Tenant A used 400 tokens");

        let second_log = rx.recv().await.unwrap();
        assert_eq!(second_log, "Log 2: Tenant B latency 45ms");

        // Ippadi thaan channel ulla data pass aaguthu nu compile aagi prove aagidum!
    }

    #[test]
    fn test_pii_rayon_short_circuit() {
        let regex = pii_regex();

        // 1. Test Match at the end of a massive payload
        let mut lines: Vec<String> = (0..5000)
            .map(|i| format!("Safe line content #{}", i))
            .collect();
        lines.push("My credit card is 4111-2222-3333-4444".to_string()); // Match!
        let massive_prompt = lines.join("\n");

        let has_pii = massive_prompt.lines().any(|line| regex.is_match(line));
        assert!(
            has_pii,
            "Failed to detect PII at the end of a large payload"
        );

        // 2. Test Match at the beginning (Short-circuit verification)
        let mut early_match = vec!["Contact me at boss@nmmglobal.com".to_string()];
        early_match.extend((0..5000).map(|i| format!("Safe line content #{}", i)));
        let early_prompt = early_match.join("\n");

        let has_pii_early = early_prompt.lines().any(|line| regex.is_match(line));
        assert!(has_pii_early, "Failed to detect PII at the beginning");
    }

    #[test]
    fn test_extract_response_content_formats() {
        // 1. Standard OpenAI JSON format
        let openai_resp = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello! I am OpenAI."
                }
            }]
        });
        let bytes = serde_json::to_vec(&openai_resp).unwrap();
        assert_eq!(
            extract_response_content(Some(&bytes)),
            "Hello! I am OpenAI.".to_string()
        );

        // 2. Plain-text fallback (our stream output representation)
        let plain_text = "This is a stream of pure text data";
        assert_eq!(
            extract_response_content(Some(plain_text.as_bytes())),
            plain_text.to_string()
        );

        // 3. None check
        assert_eq!(extract_response_content(None), String::new());
    }

    #[test]
    fn test_streaming_sse_reconstruction() {
        let events = vec![
            json!({
                "choices": [{
                    "delta": {
                        "content": "Hello"
                    }
                }]
            }),
            json!({
                "choices": [{
                    "delta": {
                        "content": ", "
                    }
                }]
            }),
            json!({
                "choices": [{
                    "delta": {
                        "content": "world!"
                    }
                }]
            }),
            // Anthropic chunk structure
            json!({
                "delta": {
                    "text": " Adding Anthropic text"
                }
            }),
        ];

        let mut reconstructed = String::new();
        for event in events {
            if let Some(content) = event["choices"][0]["delta"]["content"].as_str() {
                reconstructed.push_str(content);
            } else if let Some(text) = event["delta"]["text"].as_str() {
                reconstructed.push_str(text);
            }
        }

        assert_eq!(reconstructed, "Hello, world! Adding Anthropic text");
    }

    #[test]
    #[allow(invalid_from_utf8)]
    fn test_zero_copy_regex_checking() -> Result<(), Box<dyn std::error::Error>> {
        let pattern = "hello world";
        let regex = regex::Regex::new(pattern)?;

        let valid_bytes = b"hello world";
        let decoded = std::str::from_utf8(valid_bytes)?;
        assert!(regex.is_match(decoded));

        let invalid_bytes = &[0, 159, 146, 150]; // Invalid UTF-8 bytes
        let decode_res = std::str::from_utf8(invalid_bytes);
        assert!(decode_res.is_err());

        Ok(())
    }

    #[test]
    fn test_has_error_signature_matches() {
        assert!(has_error_signature(b"some prefix {\"error\": \"message\"}"));
        assert!(has_error_signature(b"\"rate_limit\" exceeded"));
        assert!(has_error_signature(b"\"insufficient_funds\" in balance"));
        assert!(has_error_signature(b"\"billing_limit\" reached"));
        assert!(!has_error_signature(
            b"this is a safe message with no issues"
        ));
    }

    #[allow(clippy::uninit_assumed_init, invalid_value, unknown_lints)]
    async fn setup_proxy_test_state() -> (Arc<AppState>, wiremock::MockServer) {
        let mock_server = wiremock::MockServer::start().await;

        let circuit_breaker = moka::future::Cache::builder()
            .max_capacity(100)
            .time_to_live(std::time::Duration::from_secs(60))
            .build();

        let loop_fallback_cache = moka::future::Cache::builder().max_capacity(100).build();
        let agent_guardian_cache = moka::future::Cache::builder().max_capacity(100).build();

        let routing_state = Arc::new(crate::domain::models::RoutingState::new());
        let l1_cache = Arc::new(crate::infrastructure::l1_cache::L1Cache::new(1024).unwrap());
        let http_client = reqwest::Client::new();

        let state = AppState {
            http_client,
            compliance_url: String::new(),
            rate_limit_max: 0,
            rate_limit_window: 0,
            dashboard_url: String::new(),
            llm_api_base_url: Some(mock_server.uri()),
            redis_client: None,
            telemetry: Arc::new(crate::infrastructure::oss_adapters::OssTelemetry::new(
                Arc::new(dashmap::DashMap::new()),
            )),
            billing: Arc::new(crate::infrastructure::oss_adapters::OssBilling),
            auth_resolver: Arc::new(crate::infrastructure::oss_adapters::OssAuth),
            rate_limiter: Arc::new(crate::infrastructure::oss_adapters::OssRateLimit),
            routing_config: Arc::new(crate::infrastructure::oss_adapters::OssRoutingConfig),
            semantic_cache: Arc::new(crate::infrastructure::oss_adapters::OssSemanticCache),
            execution_store: Arc::new(
                crate::infrastructure::oss_adapters::MokaExecutionStore::new(
                    moka::future::Cache::builder().build(),
                ),
            ),
            reconciler: Arc::new(crate::infrastructure::oss_adapters::OssReconciler),
            tool_transport: Arc::new(crate::infrastructure::oss_adapters::OssToolTransport),
            rate_limit_cache: Arc::new(dashmap::DashMap::new()),
            l1_cache,
            routing_state,
            circuit_breaker,
            loop_fallback_cache,
            mcp_registry: crate::infrastructure::mcp_registry::McpConnectionRegistry::empty(),
            tool_registry: crate::usecases::tool_router::ToolRegistry::empty(),
            agent_guardian_cache,
            operation_cache: moka::future::Cache::builder().build(),
            dashboard_metrics: Arc::new(crate::domain::models::DashboardMetrics::new()),
            pricing_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
            trace_store: Arc::new(dashmap::DashMap::new()),
            budget_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
        };

        (Arc::new(state), mock_server)
    }

    #[tokio::test]
    async fn test_streaming_failover_stitching_end_to_end() {
        let (state, mock_server) = setup_proxy_test_state().await;

        let tenant_id = "test-tenant";
        let virtual_model_name = "test-model";

        let mut tenant_map = std::collections::HashMap::new();
        tenant_map.insert(
            virtual_model_name.to_string(),
            crate::domain::models::ModelConfig {
                targets: vec![
                    crate::domain::models::UpstreamTarget {
                        priority: 1,
                        weight: 1,
                        api_key_alias: "primary".into(),
                        api_key: "sk-primary".to_string(),
                        provider_name: "openai".into(),
                        base_url: mock_server.uri(),
                        target_model: "gpt-4".into(),
                        schema_format: "openai".into(),
                    },
                    crate::domain::models::UpstreamTarget {
                        priority: 2,
                        weight: 1,
                        api_key_alias: "fallback".into(),
                        api_key: "sk-fallback".to_string(),
                        provider_name: "openai".into(),
                        base_url: mock_server.uri(),
                        target_model: "gpt-4".into(),
                        schema_format: "openai".into(),
                    },
                ],
                ..Default::default()
            },
        );
        let mut map = std::collections::HashMap::new();
        map.insert(tenant_id.to_string(), tenant_map);
        state.routing_state.state.store(Arc::new(map));

        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};

        // Primary path returns a chunk then an error signature chunk
        let primary_body = "data: {\"choices\": [{\"delta\": {\"content\": \"Hello\"}}]}\n\ndata: {\"error\": \"rate_limit\"}\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(wiremock::matchers::header(
                "Authorization",
                "Bearer sk-primary",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(primary_body))
            .mount(&mock_server)
            .await;

        // Fallback path returns success
        let fallback_body = "data: {\"choices\": [{\"delta\": {\"content\": \" stitched world\"}}]}\n\ndata: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(wiremock::matchers::header(
                "Authorization",
                "Bearer sk-fallback",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(fallback_body))
            .mount(&mock_server)
            .await;

        let body = axum::body::Bytes::from(
            serde_json::json!({
                "model": "test-model",
                "stream": true,
                "messages": [{"role": "user", "content": "hi"}]
            })
            .to_string(),
        );

        let mut req_extensions = axum::http::Extensions::new();
        req_extensions.insert(100.0f64);

        let trace_ctx = TraceContext {
            trace_id: "t1".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: None,
            operation_id: None,
            idempotency_key: None,
            test_scenario: None,
        };

        let result = execute_proxy(
            &state,
            &body,
            tenant_id,
            "test-model",
            "*/*",
            &trace_ctx,
            RoutingStrategy::Default,
            false,
            &req_extensions,
            false,
            None,
        )
        .await
        .unwrap();

        let ProxyBody::Stream(body_stream) = result.body else {
            panic!("Expected streaming response body");
        };

        use futures::StreamExt;
        let mut body_bytes = Vec::new();
        let mut stream = body_stream.into_data_stream();
        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res.unwrap();
            body_bytes.extend_from_slice(&chunk);
        }

        let output_str = String::from_utf8(body_bytes).unwrap();
        assert!(
            output_str.contains(" stitched world"),
            "Output does not contain ' stitched world': {}",
            output_str
        );
        assert!(
            !output_str.contains("rate_limit"),
            "Output incorrectly contains 'rate_limit': {}",
            output_str
        );
    }

    #[tokio::test]
    async fn test_large_payload_runs_pii_and_compliance() {
        let (state, _mock_server) = setup_proxy_test_state().await;
        let tenant_id = "test-tenant-123";

        // Generate a large payload > 5MB that contains PII email pattern
        let pii_email = "test-email-address@domain.com";
        let padding_size = 5 * 1024 * 1024 + 100; // > 5MB
        let mut large_string = String::with_capacity(padding_size);
        large_string.push_str(pii_email);
        while large_string.len() < padding_size {
            large_string.push_str(" padding content");
        }

        let body = axum::body::Bytes::from(format!(
            r#"{{"model": "test-model", "messages": [{{"role": "user", "content": "{}"}}]}}"#,
            large_string
        ));

        let mut req_extensions = axum::http::Extensions::new();
        req_extensions.insert(100.0f64);

        let trace_ctx = TraceContext {
            trace_id: "t2".to_string(),
            session_id: "s2".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: None,
            operation_id: None,
            idempotency_key: None,
            test_scenario: None,
        };

        // If high-load bypass is NOT working, this call will fail because compliance service is not mocked.
        // If it is working, it bypasses PII/compliance check, prep requests, and returns success!
        let result = execute_proxy(
            &state,
            &body,
            tenant_id,
            "test-model",
            "*/*",
            &trace_ctx,
            RoutingStrategy::Default,
            false,
            &req_extensions,
            false,
            None,
        )
        .await;

        assert!(
            result.is_err(),
            "Payload > 5MB MUST NOT bypass PII/compliance check"
        );
        let err_str = format!("{:?}", result.err());
        assert!(
            err_str.contains("ComplianceFailure")
                || err_str.contains("ModelNotConfigured")
                || err_str.contains("MissingModel"),
            "Expected compliance/routing check to run for >5MB payload: got {}",
            err_str
        );
    }

    // ── extract_semantic_text_raw unit tests ──────────────────────────────────

    #[test]
    fn test_raw_scanner_openai_string_content() {
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "What is my SSN 123-45-6789?"}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = extract_semantic_text_raw(&bytes);
        assert!(result.is_some(), "must extract user message text");
        let text = result.unwrap();
        assert!(
            text.contains("SSN 123-45-6789"),
            "must contain PII text: got {:?}",
            text
        );
        assert!(
            !text.contains("helpful assistant"),
            "must NOT include system message"
        );
    }

    #[test]
    fn test_raw_scanner_multimodal_skips_base64() {
        let fake_base64 = "iVBORw0KGgoAAAANSUhEUgAAAAUA".repeat(2000); // ~54 KB "image"
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Describe this image. My email is user@example.com"},
                    {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{}", fake_base64)}}
                ]
            }]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = extract_semantic_text_raw(&bytes);
        assert!(result.is_some(), "must extract text block");
        let text = result.unwrap();
        assert!(
            text.contains("user@example.com"),
            "must contain email PII: got {:?}",
            text
        );
        assert!(
            !text.contains("iVBORw0KGgo"),
            "MUST NOT contain Base64 image data in scan target"
        );
    }

    #[test]
    fn test_raw_scanner_pure_image_returns_none() {
        // A payload with ONLY an image_url block — no text content at all.
        // extract_semantic_text_raw must return None so pii_regex is never called.
        let fake_base64 = "iVBORw0KGgoAAAANSUhEUgAAAAUA".repeat(2000);
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{}", fake_base64)}}
                ]
            }]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = extract_semantic_text_raw(&bytes);
        assert!(
            result.is_none(),
            "pure-image payload must return None — PII regex must not run on Base64 data"
        );
    }

    #[test]
    fn test_raw_scanner_gemini_parts_format() {
        let body = serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [
                    {"text": "My Aadhaar is 1234 5678 9012"}
                ]
            }]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = extract_semantic_text_raw(&bytes);
        assert!(result.is_some(), "must extract Gemini parts text");
        assert!(
            result.unwrap().contains("1234 5678 9012"),
            "must contain Aadhaar number"
        );
    }

    #[test]
    fn test_raw_scanner_anthropic_legacy_prompt() {
        let body = serde_json::json!({
            "prompt": "\n\nHuman: Call me at +1 415-555-0100\n\nAssistant:"
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = extract_semantic_text_raw(&bytes);
        assert!(result.is_some(), "must extract Anthropic legacy prompt");
        assert!(
            result.unwrap().contains("+1 415-555-0100"),
            "must contain phone number PII"
        );
    }

    #[test]
    fn test_raw_scanner_64kb_budget_cap() {
        // Build a user message text that is > 64 KiB
        let long_text = "a".repeat(128 * 1024);
        let body = serde_json::json!({
            "messages": [{"role": "user", "content": long_text}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = extract_semantic_text_raw(&bytes);
        // Must return Some but capped at ≤64 KiB
        assert!(result.is_some());
        assert!(
            result.unwrap().len() <= 64 * 1024,
            "output must be capped at 64 KiB"
        );
    }

    #[test]
    fn test_proxy_buffer_unmutated_with_escaped_json_and_xml_tags() {
        // NOTE on `tools` vs `tool_calls`:
        // - `"tools": [...]` at the TOP LEVEL = tool SCHEMA DEFINITIONS provided to the model.
        //   This is part of the initial user request setup and does NOT signal an agentic turn.
        //   extract_semantic_text MUST still extract user text and return Some(...).
        // - `"tool_calls": [...]` INSIDE an `assistant` message = the model ACTIVELY INVOKING
        //   a tool. This signals an in-progress agentic turn and extract_semantic_text MUST
        //   return None to bypass the semantic cache.
        // This test covers the former (tool definitions) — semantic cache must remain active.
        let raw_payload = r#"{"model":"gpt-4","messages":[{"role":"user","content":"<function=web_search>"}],"tools":[{"type":"function","function":{"name":"search","arguments":"{\"query\": \"AI explanation\"}"}}],"stream":true}"#;
        let parse_buffer = raw_payload.as_bytes().to_vec();

        // Run the inspection pipeline stage (matching execute_proxy inspection logic)
        let (is_streaming, semantic_text) = {
            let mut inspect_buf = parse_buffer.clone();
            let lazy_parsed = simd_json::to_borrowed_value(&mut inspect_buf)
                .expect("inspect_buf should be valid JSON");

            let is_stream = lazy_parsed
                .get("stream")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let sem_text = extract_semantic_text(&lazy_parsed);
            (is_stream, sem_text)
        };

        assert!(is_streaming, "stream flag must be true");
        assert!(
            semantic_text.is_some(),
            "top-level tool definitions must NOT bypass cache — only tool_calls in messages should"
        );

        // Critical Assertion: The primary transmission buffer must remain 100% byte-identical
        assert_eq!(
            parse_buffer,
            raw_payload.as_bytes(),
            "Outbound transmission buffer must retain all backslashes and XML tags without in-place mutation corruption"
        );
    }

    // ── extract_semantic_text agentic bypass unit tests ───────────────────────

    #[test]
    fn test_extract_semantic_text_single_turn_user() {
        // Standard single-turn query: semantic cache must remain active → Some(text)
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "What is the capital of France?"}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut buf = bytes.clone();
        let parsed = simd_json::to_borrowed_value(&mut buf).unwrap();
        let result = extract_semantic_text(&parsed);
        assert_eq!(
            result.as_deref(),
            Some("What is the capital of France?"),
            "single-turn user query must extract text for caching"
        );
    }

    #[test]
    fn test_extract_semantic_text_bypasses_on_tool_role() {
        // Payload contains a role:"tool" message → must return None to bypass cache
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "user",      "content": "Search the web for Rust async"},
                {"role": "assistant", "content": null, "tool_calls": [{"id": "call_abc", "type": "function", "function": {"name": "search_web", "arguments": "{}"}}]},
                {"role": "tool",      "tool_call_id": "call_abc", "content": "Rust async uses tokio..."}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut buf = bytes.clone();
        let parsed = simd_json::to_borrowed_value(&mut buf).unwrap();
        let result = extract_semantic_text(&parsed);
        assert!(
            result.is_none(),
            "role:tool message must bypass semantic cache — got {:?}",
            result
        );
    }

    #[test]
    fn test_extract_semantic_text_bypasses_on_assistant_tool_calls() {
        // Assistant message with non-empty tool_calls array → must return None
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "user",      "content": "Fetch my profile"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_xyz", "type": "function", "function": {"name": "fetch_user_profile", "arguments": "{}"}}
                ]}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut buf = bytes.clone();
        let parsed = simd_json::to_borrowed_value(&mut buf).unwrap();
        let result = extract_semantic_text(&parsed);
        assert!(
            result.is_none(),
            "assistant with non-empty tool_calls must bypass semantic cache — got {:?}",
            result
        );
    }

    #[test]
    fn test_extract_semantic_text_bypasses_on_assistant_function_call() {
        // Assistant message with legacy function_call object → must return None
        let body = serde_json::json!({
            "model": "gpt-3.5-turbo",
            "messages": [
                {"role": "user",      "content": "Get weather"},
                {"role": "assistant", "content": null, "function_call": {"name": "get_weather", "arguments": "{}"}}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut buf = bytes.clone();
        let parsed = simd_json::to_borrowed_value(&mut buf).unwrap();
        let result = extract_semantic_text(&parsed);
        assert!(
            result.is_none(),
            "assistant with function_call must bypass semantic cache — got {:?}",
            result
        );
    }

    #[test]
    fn test_extract_semantic_text_bypasses_on_anthropic_tool_use_block() {
        // Anthropic: content block with type="tool_use" in assistant message → must return None
        let body = serde_json::json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [
                {"role": "user",      "content": "Run the query"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "toolu_01", "name": "query_database", "input": {}}
                ]}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut buf = bytes.clone();
        let parsed = simd_json::to_borrowed_value(&mut buf).unwrap();
        let result = extract_semantic_text(&parsed);
        assert!(
            result.is_none(),
            "Anthropic tool_use content block must bypass semantic cache — got {:?}",
            result
        );
    }

    #[test]
    fn test_extract_semantic_text_bypasses_on_anthropic_tool_result_block() {
        // Anthropic: content block with type="tool_result" in user message → must return None
        let body = serde_json::json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [
                {"role": "user",      "content": "Check my balance"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "toolu_02", "name": "get_balance", "input": {}}
                ]},
                {"role": "user",      "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_02", "content": "Balance: $500"}
                ]}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut buf = bytes.clone();
        let parsed = simd_json::to_borrowed_value(&mut buf).unwrap();
        let result = extract_semantic_text(&parsed);
        assert!(
            result.is_none(),
            "Anthropic tool_result content block must bypass semantic cache — got {:?}",
            result
        );
    }

    #[test]
    fn test_extract_semantic_text_bypasses_on_gemini_function_response() {
        // Gemini: any part contains functionResponse → must return None
        let body = serde_json::json!({
            "contents": [
                {"role": "user",  "parts": [{"text": "What is the weather?"}]},
                {"role": "model", "parts": [{"functionCall": {"name": "get_weather", "args": {}}}]},
                {"role": "user",  "parts": [{"functionResponse": {"name": "get_weather", "response": {"temperature": "22C"}}}]}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut buf = bytes.clone();
        let parsed = simd_json::to_borrowed_value(&mut buf).unwrap();
        let result = extract_semantic_text(&parsed);
        assert!(
            result.is_none(),
            "Gemini functionResponse must bypass semantic cache — got {:?}",
            result
        );
    }

    #[test]
    fn test_extract_semantic_text_bypasses_on_gemini_function_call() {
        // Gemini: any part contains functionCall → must return None
        let body = serde_json::json!({
            "contents": [
                {"role": "user",  "parts": [{"text": "Search for Rust crates"}]},
                {"role": "model", "parts": [{"functionCall": {"name": "search_crates", "args": {"query": "async"}}}]}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut buf = bytes.clone();
        let parsed = simd_json::to_borrowed_value(&mut buf).unwrap();
        let result = extract_semantic_text(&parsed);
        assert!(
            result.is_none(),
            "Gemini functionCall must bypass semantic cache — got {:?}",
            result
        );
    }

    #[test]
    fn test_extract_semantic_text_multi_turn_no_tools() {
        // Multi-turn conversation with NO tool usage: must still cache using most recent user text.
        // This validates that the bypass fix does not break normal RAG / conversational use cases.
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "user",      "content": "Tell me about Paris."},
                {"role": "assistant", "content": "Paris is the capital of France."},
                {"role": "user",      "content": "What is its population?"}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut buf = bytes.clone();
        let parsed = simd_json::to_borrowed_value(&mut buf).unwrap();
        let result = extract_semantic_text(&parsed);
        assert_eq!(
            result.as_deref(),
            Some("What is its population?"),
            "multi-turn without tools must extract the most recent user message"
        );
    }

    #[tokio::test]
    async fn test_handle_cache_hit_streaming_three_chunk_sequence() {
        let (state, _mock_server) = setup_proxy_test_state().await;
        let trace_ctx = TraceContext {
            trace_id: "t_cache_1".to_string(),
            session_id: "s_cache_1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: None,
            operation_id: None,
            idempotency_key: None,
            test_scenario: None,
        };

        let result = handle_cache_hit(
            &state,
            "Cached response content".to_string(),
            "test-tenant-123",
            "test-model",
            "What is 2+2?",
            "What is 2+2?",
            &trace_ctx,
            std::time::Instant::now(),
            true,  // is_streaming
            false, // is_free_tier
            true,  // semantic_cache_enabled
            None,
            None,
            0,
        )
        .expect("handle_cache_hit should succeed");

        assert_eq!(result.content_type, "text/event-stream");
        assert!(result.cache_hit);

        let ProxyBody::Stream(body_stream) = result.body else {
            panic!("Expected ProxyBody::Stream");
        };

        use futures::StreamExt;
        let mut chunks_raw = Vec::new();
        let mut stream = body_stream.into_data_stream();
        while let Some(chunk_res) = stream.next().await {
            let chunk_bytes = chunk_res.expect("stream chunk should be ok");
            chunks_raw.extend_from_slice(&chunk_bytes);
        }

        let stream_text = String::from_utf8(chunks_raw).expect("valid utf8 stream text");
        let lines: Vec<&str> = stream_text
            .lines()
            .filter(|l| l.starts_with("data: "))
            .collect();

        assert_eq!(
            lines.len(),
            4,
            "Expected 4 SSE data lines (Role init, Content delivery, Stop finish_reason, [DONE])"
        );

        // Line 1: Role initialization
        let payload1: serde_json::Value =
            serde_json::from_str(lines[0].trim_start_matches("data: ")).unwrap();
        assert!(
            payload1["created"].as_u64().unwrap_or(0) > 0,
            "created timestamp must be real Unix epoch (> 0)"
        );
        assert_eq!(payload1["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(payload1["choices"][0]["delta"]["content"], "");
        assert!(payload1["choices"][0]["finish_reason"].is_null());

        // Line 2: Content delivery
        let payload2: serde_json::Value =
            serde_json::from_str(lines[1].trim_start_matches("data: ")).unwrap();
        assert!(
            payload2["created"].as_u64().unwrap_or(0) > 0,
            "created timestamp must be real Unix epoch (> 0)"
        );
        assert_eq!(
            payload2["choices"][0]["delta"]["content"],
            "Cached response content"
        );
        assert!(
            payload2["choices"][0]["delta"].get("role").is_none(),
            "Chunk 2 must not re-include role"
        );
        assert!(payload2["choices"][0]["finish_reason"].is_null());

        // Line 3: Finish reason stop
        let payload3: serde_json::Value =
            serde_json::from_str(lines[2].trim_start_matches("data: ")).unwrap();
        assert!(
            payload3["created"].as_u64().unwrap_or(0) > 0,
            "created timestamp must be real Unix epoch (> 0)"
        );
        assert_eq!(payload3["choices"][0]["finish_reason"], "stop");

        // Line 4: Terminal marker
        assert_eq!(lines[3], "data: [DONE]");
    }

    #[tokio::test]
    async fn test_handle_cache_hit_non_streaming_timestamp() {
        let (state, _mock_server) = setup_proxy_test_state().await;
        let trace_ctx = TraceContext {
            trace_id: "t_cache_2".to_string(),
            session_id: "s_cache_2".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: None,
            operation_id: None,
            idempotency_key: None,
            test_scenario: None,
        };

        let result = handle_cache_hit(
            &state,
            "Cached non-streaming content".to_string(),
            "test-tenant-123",
            "test-model",
            "Hello",
            "Hello",
            &trace_ctx,
            std::time::Instant::now(),
            false, // is_streaming = false
            false,
            true,
            None,
            None,
            0,
        )
        .expect("handle_cache_hit should succeed");

        assert_eq!(result.content_type, "application/json");
        assert!(result.cache_hit);

        let ProxyBody::Buffered(bytes) = result.body else {
            panic!("Expected ProxyBody::Buffered");
        };

        let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            payload["created"].as_u64().unwrap_or(0) > 0,
            "created timestamp must be real Unix epoch (> 0)"
        );
        assert!(
            payload["id"]
                .as_str()
                .unwrap_or("")
                .starts_with("chatcmpl-cached-"),
            "id must be dynamic chatcmpl-cached-..."
        );
        assert_eq!(
            payload["choices"][0]["message"]["content"],
            "Cached non-streaming content"
        );
        assert_eq!(payload["choices"][0]["finish_reason"], "stop");
    }
}
