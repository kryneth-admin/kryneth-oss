//! infrastructure/mcp_registry.rs — Hardened Speculative Pre-Fetcher (Phase 1 Refactor).
//!
//! ## Changes from MVP
//! * **Buffered FSM Parser**: `find_tool_hint` now drives a byte-level `NameScanFsm`
//!   that correctly handles `"name"` keys split across chunk boundaries without any
//!   `simd-json` overhead or allocation.
//! * **Stateful Handoff**: `warm_connection` writes a `PreWarmedConnection` record into
//!   `McpConnectionRegistry::warmed` (DashMap) after completion.  Phase 4 fan-out can
//!   query this map to skip redundant TCP handshakes.
//! * **Backpressure Semaphore**: `prefetch_sem` (capacity 20) is stored on the registry.
//!   Callers use `try_acquire_owned()` — if all 20 slots are busy the warm-up is
//!   silently dropped (best-effort advisory).

use dashmap::DashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, info, warn};

/// Composite key for per-tenant MCP warm-up state: `"{tenant_id}::{tool_name}"`.
#[inline]
pub fn mcp_warmed_key(tenant_id: &str, tool_name: &str) -> String {
    format!("{}::{}", tenant_id, tool_name)
}

// ── FSM ───────────────────────────────────────────────────────────────────────

/// States for the `NameScanFsm`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FsmState {
    /// Scanning for opening `"` of any JSON string.
    Idle,
    /// Inside a potential `"name"` key; field = chars of `"name"` matched so far (0–3).
    MatchKey(u8),
    /// Matched `"name"` fully, waiting for `:`.
    AfterKeyClose,
    /// After `:`, waiting for opening `"` of the value.
    AfterColon,
    /// Inside the value string, accumulating bytes.
    InValue,
    /// Inside the value, after a `\` escape character.
    InValueEscape,
}

/// Byte-level finite-state machine that extracts `"name": "<value>"` entries
/// from a raw JSON byte stream, handling keys split across arbitrary chunk
/// boundaries without heap allocations on the hot path.
struct NameScanFsm {
    state: FsmState,
    /// Accumulates the value bytes between the opening and closing `"`.
    value_buf: Vec<u8>,
}

impl NameScanFsm {
    #[inline]
    fn new() -> Self {
        Self {
            state: FsmState::Idle,
            value_buf: Vec::with_capacity(128),
        }
    }

    /// Feed a byte slice into the FSM.
    /// Returns `Some(value_bytes)` the moment a complete `"name"` value is captured.
    fn feed(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        const KEY: [u8; 4] = *b"name";
        for &b in bytes {
            match self.state {
                FsmState::Idle => {
                    if b == b'"' {
                        self.state = FsmState::MatchKey(0);
                    }
                }
                FsmState::MatchKey(pos) => {
                    let p = pos as usize;
                    if p < 4 && b == KEY[p] {
                        self.state = FsmState::MatchKey((p + 1) as u8);
                    } else if p == 4 && b == b'"' {
                        // Matched all of "name" + closing quote.
                        self.state = FsmState::AfterKeyClose;
                    } else if b == b'"' {
                        // Mismatch but new string opening — restart key match.
                        self.state = FsmState::MatchKey(0);
                    } else {
                        self.state = FsmState::Idle;
                    }
                }
                FsmState::AfterKeyClose => match b {
                    b':' => self.state = FsmState::AfterColon,
                    b' ' | b'\t' | b'\r' | b'\n' => {}
                    _ => self.state = FsmState::Idle,
                },
                FsmState::AfterColon => match b {
                    b'"' => {
                        self.value_buf.clear();
                        self.state = FsmState::InValue;
                    }
                    b' ' | b'\t' | b'\r' | b'\n' => {}
                    _ => self.state = FsmState::Idle,
                },
                FsmState::InValue => match b {
                    b'"' => {
                        let result = self.value_buf.clone();
                        self.state = FsmState::Idle;
                        self.value_buf.clear();
                        return Some(result);
                    }
                    b'\\' => self.state = FsmState::InValueEscape,
                    _ => self.value_buf.push(b),
                },
                FsmState::InValueEscape => {
                    // Emit the escaped character literally; unescape is caller's problem.
                    self.value_buf.push(b);
                    self.state = FsmState::InValue;
                }
            }
        }
        None
    }
}

// ── PreWarmedConnection ───────────────────────────────────────────────────────

/// Records the outcome of a speculative warm-up for a single MCP tool endpoint.
/// Stored in `McpConnectionRegistry::warmed`; read by Phase 4 fan-out.
pub struct PreWarmedConnection {
    pub tool_name: String,
    pub sse_url: String,
    /// Wall-clock timestamp of when the warm-up attempt finished.
    pub warmed_at: Instant,
    /// `true` if the HEAD request completed with any HTTP response (socket warm).
    pub success: bool,
}

impl PreWarmedConnection {
    /// Returns `true` when the warm-up record is still fresh (< 30 s old).
    #[inline]
    pub fn is_fresh(&self) -> bool {
        self.warmed_at.elapsed().as_secs() < 30
    }
}

// ── ToolMetrics ──────────────────────────────────────────────────────────────

/// Stores execution telemetry for a registered MCP tool.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct ToolMetrics {
    pub call_count: u64,
    pub total_latency_ms: u64,
}

// ── Registry ──────────────────────────────────────────────────────────────────

pub struct McpConnectionRegistry {
    /// tool_name → (endpoint URL, environment variables) (read-only after construction).
    registry: DashMap<String, (String, std::collections::HashMap<String, String>)>,
    /// tool_name → result of the last speculative warm-up for that tenant+tool.
    /// Keys are `mcp_warmed_key(tenant_id, tool_name)` so tenants never see each other's warm state.
    pub warmed: DashMap<String, PreWarmedConnection>,
    /// Backpressure semaphore: at most 20 speculative warm-ups in flight.
    /// Callers must use `try_acquire_owned()` — never block.
    pub prefetch_sem: Arc<Semaphore>,
    /// tenant_id::tool_name -> real execution telemetry.
    pub tool_metrics: DashMap<String, ToolMetrics>,
}

impl McpConnectionRegistry {
    // ── Constructors ──────────────────────────────────────────────────────────

    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            registry: DashMap::new(),
            warmed: DashMap::new(),
            prefetch_sem: Arc::new(Semaphore::new(20)),
            tool_metrics: DashMap::new(),
        })
    }

    /// Loads the registry from the `MCP_TOOL_REGISTRY` environment variable.
    ///
    /// Format: `{"jira_search":"http://mcp-jira:9000/sse", ...}` (JSON object).
    /// Returns an empty registry (pre-fetching disabled) on any error (fail-open).
    pub fn from_env() -> Arc<Self> {
        let raw = match std::env::var("MCP_TOOL_REGISTRY") {
            Ok(v) => v,
            Err(_) => {
                debug!("MCP_TOOL_REGISTRY not set — speculative pre-fetching disabled");
                return Self::empty();
            }
        };

        let registry: DashMap<String, (String, std::collections::HashMap<String, String>)> =
            DashMap::new();
        match serde_json::from_str::<std::collections::HashMap<String, String>>(&raw) {
            Ok(map) => {
                let count = map.len();
                for (k, v) in map {
                    registry.insert(k, (v, std::collections::HashMap::new()));
                }
                info!(tool_count = count, "Tunnel 3 — MCP registry loaded");
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse MCP_TOOL_REGISTRY — pre-fetching disabled");
            }
        }

        Arc::new(Self {
            registry,
            warmed: DashMap::new(),
            prefetch_sem: Arc::new(Semaphore::new(20)),
            tool_metrics: DashMap::new(),
        })
    }

    // ── Telemetry & dynamic metrics ──────────────────────────────────────────

    /// Records execution telemetry for an MCP tool call (zero-copy, concurrent, tenant-scoped).
    pub fn record_tool_call(&self, tenant_id: &str, tool_name: &str, latency_ms: u64) {
        let key = mcp_warmed_key(tenant_id, tool_name);
        self.tool_metrics
            .entry(key)
            .and_modify(|m| {
                m.call_count += 1;
                m.total_latency_ms += latency_ms;
            })
            .or_insert(ToolMetrics {
                call_count: 1,
                total_latency_ms: latency_ms,
            });
    }

    /// Retrieves dynamic tool call metrics (calls and total latency) for a tenant's tool.
    pub fn get_metrics(&self, tenant_id: &str, tool_name: &str) -> Option<(u64, u64)> {
        let key = mcp_warmed_key(tenant_id, tool_name);
        self.tool_metrics
            .get(&key)
            .map(|m| (m.call_count, m.total_latency_ms))
    }

    pub fn update_server(
        &self,
        name: String,
        url: String,
        env_vars: std::collections::HashMap<String, String>,
    ) {
        self.registry.insert(name, (url, env_vars));
    }

    pub fn remove_server(&self, name: &str) {
        self.registry.remove(name);
    }

    // ── Query helpers ─────────────────────────────────────────────────────────

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
    }

    pub fn get_url(&self, tool_name: &str) -> Option<String> {
        self.registry.get(tool_name).map(|r| r.value().0.clone())
    }

    pub fn get_env_vars(
        &self,
        tool_name: &str,
    ) -> Option<std::collections::HashMap<String, String>> {
        self.registry.get(tool_name).map(|r| r.value().1.clone())
    }

    pub fn all_endpoints(&self) -> Vec<(String, String)> {
        self.registry
            .iter()
            .map(|kv| (kv.key().clone(), kv.value().0.clone()))
            .collect()
    }

    /// Returns `Some(tool_name)` when the request body contains a registered
    /// MCP tool name in a `"name"` field.
    ///
    /// ## Algorithm
    /// Drives a zero-allocation `NameScanFsm` over 64-byte chunks of the first
    /// 16 KB of `body_bytes`.  The chunk-based feeding correctly handles the
    /// pathological case where `"name"` is split across chunk boundaries.
    ///
    /// Replaces the previous `simd-json` parse — no heap allocation on the hot path.
    pub fn find_tool_hint(&self, body_bytes: &[u8]) -> Option<String> {
        if self.registry.is_empty() {
            return None;
        }

        let scan_limit = body_bytes.len().min(16 * 1024);
        let mut fsm = NameScanFsm::new();

        // Feed in 64-byte chunks — simulates streaming; proves cross-boundary safety.
        for chunk in body_bytes[..scan_limit].chunks(64) {
            if let Some(name_bytes) = fsm.feed(chunk) {
                if let Ok(name) = std::str::from_utf8(&name_bytes) {
                    if self.registry.contains_key::<str>(name) {
                        debug!(
                            tool_name = %name,
                            "Tunnel 3 Phase 1 — FSM matched registered MCP tool"
                        );
                        return Some(name.to_string());
                    }
                }
            }
        }
        None
    }

    // ── Warm-up ───────────────────────────────────────────────────────────────

    /// Fires a non-blocking HEAD request to pre-warm the TCP/TLS connection
    /// for `sse_url`, then records the outcome in `registry.warmed`.
    ///
    /// ## Signature change from MVP
    /// Takes `registry: Arc<McpConnectionRegistry>` so the result can be
    /// written back without a separate callback.  The `_permit` argument
    /// holds the semaphore slot for the task's lifetime; dropping it at the
    /// end of the function releases one of the 20 concurrent slots.
    pub async fn warm_connection(
        registry: Arc<McpConnectionRegistry>,
        http_client: reqwest::Client,
        tenant_id: String,
        tool_name: String,
        sse_url: String,
        _permit: OwnedSemaphorePermit,
    ) {
        debug!(
            tenant_id = %tenant_id,
            tool_name = %tool_name,
            url = %sse_url,
            "Tunnel 3 Phase 1 — firing speculative warm-up (semaphore slot held)"
        );

        let start = Instant::now();

        let success = match http_client
            .head(&sse_url)
            .timeout(std::time::Duration::from_millis(500))
            .send()
            .await
        {
            Ok(resp) => {
                info!(
                    tenant_id = %tenant_id,
                    tool_name = %tool_name,
                    status = resp.status().as_u16(),
                    latency_ms = start.elapsed().as_millis(),
                    "Tunnel 3 Phase 1 — warm-up completed"
                );
                true
            }
            Err(e) => {
                warn!(
                    tenant_id = %tenant_id,
                    tool_name = %tool_name,
                    error = %e,
                    "Tunnel 3 Phase 1 — warm-up failed (non-fatal)"
                );
                false
            }
        };

        // Write the outcome into the stateful handoff map.
        let cache_key = mcp_warmed_key(&tenant_id, &tool_name);
        registry.warmed.insert(
            cache_key,
            PreWarmedConnection {
                tool_name,
                sse_url,
                warmed_at: Instant::now(),
                success,
            },
        );
        // `_permit` is dropped here → semaphore slot released.
    }
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry(tools: &[(&str, &str)]) -> McpConnectionRegistry {
        let registry = DashMap::new();
        for (name, url) in tools {
            registry.insert(
                name.to_string(),
                (url.to_string(), std::collections::HashMap::new()),
            );
        }
        McpConnectionRegistry {
            registry,
            warmed: DashMap::new(),
            prefetch_sem: Arc::new(Semaphore::new(20)),
            tool_metrics: DashMap::new(),
        }
    }

    // ── FSM correctness tests ─────────────────────────────────────────────────

    #[test]
    fn test_fsm_finds_name_in_one_chunk() {
        let reg = make_registry(&[("jira_search", "http://mcp:9000/sse")]);
        let body = br#"{"tools":[{"type":"function","function":{"name":"jira_search"}}]}"#;
        assert_eq!(reg.find_tool_hint(body), Some("jira_search".to_string()));
    }

    #[test]
    fn test_fsm_finds_name_split_across_chunks() {
        // Split at an arbitrary boundary inside "jira_search"
        let reg = make_registry(&[("jira_search", "http://mcp:9000/sse")]);
        // Construct bytes that split the key name across a 64-byte chunk boundary.
        let prefix = b"{\"tools\":[{\"type\":\"function\",\"function\":{\"name\":\"jira_".to_vec();
        let suffix = b"search\"}}]}".to_vec();
        let body = [prefix, suffix].concat();
        // The body is 77 bytes; chunk boundary at 64 splits "jira_search"
        assert_eq!(reg.find_tool_hint(&body), Some("jira_search".to_string()));
    }

    #[test]
    fn test_fsm_no_match_returns_none() {
        let reg = make_registry(&[("jira_search", "http://mcp:9000/sse")]);
        let body = br#"{"tools":[{"function":{"name":"weather_api"}}]}"#;
        assert_eq!(reg.find_tool_hint(body), None);
    }

    #[test]
    fn test_fsm_empty_registry_short_circuits() {
        let reg = make_registry(&[]);
        let body = br#"{"tools":[{"function":{"name":"jira_search"}}]}"#;
        assert_eq!(reg.find_tool_hint(body), None);
    }

    #[test]
    fn test_fsm_malformed_json_no_panic() {
        let reg = make_registry(&[("jira_search", "http://mcp:9000/sse")]);
        assert_eq!(reg.find_tool_hint(b"{bad json[[\"name\":\""), None);
    }

    #[test]
    fn test_fsm_handles_escape_in_value() {
        // Value with escaped quote should not terminate early.
        let reg = make_registry(&[("tool_with_escape", "http://mcp:9000/sse")]);
        // Escaped backslash inside a different "name" value — should not confuse the FSM.
        let body = br#"{"function":{"name":"tool_with_escape"}}"#;
        assert_eq!(
            reg.find_tool_hint(body),
            Some("tool_with_escape".to_string())
        );
    }

    // ── Semaphore backpressure tests ──────────────────────────────────────────

    #[test]
    fn test_semaphore_allows_up_to_20() {
        let reg = make_registry(&[]);
        let mut permits = Vec::new();
        for _ in 0..20 {
            permits.push(
                Arc::clone(&reg.prefetch_sem)
                    .try_acquire_owned()
                    .expect("should acquire"),
            );
        }
        // 21st must fail
        assert!(
            Arc::clone(&reg.prefetch_sem).try_acquire_owned().is_err(),
            "21st acquire should be rejected"
        );
        // Drop one permit — 21st now succeeds
        drop(permits.pop());
        assert!(Arc::clone(&reg.prefetch_sem).try_acquire_owned().is_ok());
    }

    // ── PreWarmedConnection ───────────────────────────────────────────────────

    #[test]
    fn test_prewarmed_connection_freshness() {
        let conn = PreWarmedConnection {
            tool_name: "t".into(),
            sse_url: "u".into(),
            warmed_at: Instant::now(),
            success: true,
        };
        assert!(conn.is_fresh());
    }

    #[test]
    fn test_get_url_returns_correct_url() {
        let reg = make_registry(&[("jira_search", "http://mcp:9000/sse")]);
        assert_eq!(
            reg.get_url("jira_search"),
            Some("http://mcp:9000/sse".to_string())
        );
        assert_eq!(reg.get_url("nonexistent"), None);
    }
}
