//! tests/payment_execution_test.rs — Deterministic Payment Failure & Reconciliation Harness.
//!
//! Validates the core Kryneth execution boundary:
//! 1. An agent requests a payment operation.
//! 2. Kryneth forwards to mock payment service.
//! 3. Payment service commits the payment.
//! 4. The response times out / socket dropped.
//! 5. Kryneth marks outcome as UNKNOWN.
//! 6. Reconciliation queries the authoritative downstream ledger.
//! 7. Retry returns recovered result with ZERO duplicate payments committed.
//! 8. Concurrent duplicates are coordinated atomically.
//! 9. Stale background completions are fenced out.
//! 10. Canonical JSON hashing prevents key-ordering evasion.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use kryneth_gateway::domain::execution::{ExecutionState, IdempotencyKey, ToolExecution};
use kryneth_gateway::domain::models::{AppState, RoutingState, TraceContext};
use kryneth_gateway::domain::ports::{
    ExecutionStore, Reconciler, ReconciliationResult, ToolTransport,
};
use kryneth_gateway::infrastructure::l1_cache::L1Cache;
use kryneth_gateway::infrastructure::mcp_client::{ToolCall, ToolResult};
use kryneth_gateway::infrastructure::oss_adapters::{
    MokaExecutionStore, OssAuth, OssBilling, OssRateLimit, OssRoutingConfig, OssSemanticCache,
    OssTelemetry,
};
use kryneth_gateway::usecases::execution_service::ExecutionService;
use moka::future::Cache;
use serde_json::Value;

// ── Deterministic Mock Payment Service & Ledger ─────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PaymentRecord {
    pub payment_id: String,
    pub order_id: String,
    pub amount: u64,
    pub status: String,
    pub committed_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockPaymentMode {
    Success,
    CommitThenTimeout,
    TimeoutBeforeCommit,
    DelayedCompletion { delay_ms: u64 },
    AuthoritativeNotFound,
    InconclusiveReconciliation,
}

pub struct MockPaymentLedger {
    pub records: Arc<Mutex<HashMap<String, PaymentRecord>>>,
    pub mutation_attempts: Arc<AtomicUsize>,
    pub commit_count: Arc<AtomicUsize>,
    pub mode: Arc<Mutex<MockPaymentMode>>,
}

impl MockPaymentLedger {
    pub fn new(mode: MockPaymentMode) -> Self {
        Self {
            records: Arc::new(Mutex::new(HashMap::new())),
            mutation_attempts: Arc::new(AtomicUsize::new(0)),
            commit_count: Arc::new(AtomicUsize::new(0)),
            mode: Arc::new(Mutex::new(mode)),
        }
    }

    pub fn set_mode(&self, new_mode: MockPaymentMode) {
        let mut m = self.mode.lock().unwrap();
        *m = new_mode;
    }

    pub fn get_payment(&self, order_id: &str) -> Option<PaymentRecord> {
        let records = self.records.lock().unwrap();
        records.get(order_id).cloned()
    }

    pub fn total_mutations(&self) -> usize {
        self.mutation_attempts.load(Ordering::SeqCst)
    }

    pub fn total_commits(&self) -> usize {
        self.commit_count.load(Ordering::SeqCst)
    }
}

pub struct MockPaymentTransport {
    pub ledger: Arc<MockPaymentLedger>,
}

impl ToolTransport for MockPaymentTransport {
    fn execute_tool<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        arguments: &'a str,
        _tenant_id: &'a str,
        _enable_compression: bool,
        _test_scenario: Option<&'a str>,
    ) -> Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        self.ledger.mutation_attempts.fetch_add(1, Ordering::SeqCst);
        let mode = *self.ledger.mode.lock().unwrap();
        let ledger = self.ledger.clone();
        let cid = tool_call_id.to_string();
        let name = tool_name.to_string();
        let raw_args = arguments.to_string();

        Box::pin(async move {
            let parsed: Value = serde_json::from_str(&raw_args).unwrap_or(serde_json::json!({}));
            let order_id = parsed["order_id"]
                .as_str()
                .unwrap_or("order_default")
                .to_string();
            let amount = parsed["amount"].as_u64().unwrap_or(100);

            match mode {
                MockPaymentMode::Success => {
                    let payment_id = format!("pay_{}", uuid::Uuid::new_v4().simple());
                    let record = PaymentRecord {
                        payment_id: payment_id.clone(),
                        order_id: order_id.clone(),
                        amount,
                        status: "COMMITTED".to_string(),
                        committed_at: Utc::now(),
                    };
                    ledger.records.lock().unwrap().insert(order_id, record);
                    ledger.commit_count.fetch_add(1, Ordering::SeqCst);

                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: format!(
                            r#"{{"payment_id":"{}","status":"COMMITTED","amount":{}}}"#,
                            payment_id, amount
                        ),
                        latency_ms: 10,
                        success: true,
                    }
                }
                MockPaymentMode::CommitThenTimeout => {
                    // CRITICAL SCENARIO: Payment committed in downstream ledger, but socket drops
                    let payment_id = format!("pay_{}", uuid::Uuid::new_v4().simple());
                    let record = PaymentRecord {
                        payment_id: payment_id.clone(),
                        order_id: order_id.clone(),
                        amount,
                        status: "COMMITTED".to_string(),
                        committed_at: Utc::now(),
                    };
                    ledger.records.lock().unwrap().insert(order_id, record);
                    ledger.commit_count.fetch_add(1, Ordering::SeqCst);

                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: r#"{"error":"MCP_TIMEOUT","message":"Downstream connection timed out after commit"}"#.to_string(),
                        latency_ms: 5000,
                        success: false,
                    }
                }
                MockPaymentMode::TimeoutBeforeCommit => {
                    // Gateway timed out before payment could commit
                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: r#"{"error":"MCP_TIMEOUT","message":"Gateway timeout before payment commit"}"#.to_string(),
                        latency_ms: 5000,
                        success: false,
                    }
                }
                MockPaymentMode::DelayedCompletion { delay_ms } => {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    let payment_id = format!("pay_{}", uuid::Uuid::new_v4().simple());
                    let record = PaymentRecord {
                        payment_id: payment_id.clone(),
                        order_id: order_id.clone(),
                        amount,
                        status: "COMMITTED".to_string(),
                        committed_at: Utc::now(),
                    };
                    ledger.records.lock().unwrap().insert(order_id, record);
                    ledger.commit_count.fetch_add(1, Ordering::SeqCst);

                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: format!(
                            r#"{{"payment_id":"{}","status":"COMMITTED","amount":{}}}"#,
                            payment_id, amount
                        ),
                        latency_ms: delay_ms,
                        success: true,
                    }
                }
                MockPaymentMode::AuthoritativeNotFound => ToolResult {
                    tool_call_id: cid,
                    name,
                    content: r#"{"error":"PAYMENT_REJECTED","message":"Card declined"}"#
                        .to_string(),
                    latency_ms: 20,
                    success: false,
                },
                MockPaymentMode::InconclusiveReconciliation => ToolResult {
                    tool_call_id: cid,
                    name,
                    content: r#"{"error":"MCP_TIMEOUT"}"#.to_string(),
                    latency_ms: 5000,
                    success: false,
                },
            }
        })
    }
}

pub struct LedgerReconciler {
    pub ledger: Arc<MockPaymentLedger>,
}

impl Reconciler for LedgerReconciler {
    fn reconcile<'a>(
        &'a self,
        _operation: &'a ToolExecution,
    ) -> Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<ReconciliationResult, kryneth_gateway::error::GatewayError>,
                > + Send
                + 'a,
        >,
    > {
        let ledger = self.ledger.clone();
        let mode = *ledger.mode.lock().unwrap();

        Box::pin(async move {
            if mode == MockPaymentMode::InconclusiveReconciliation {
                return Ok(ReconciliationResult::StillUnknown);
            }

            let records = ledger.records.lock().unwrap();
            if let Some(record) = records.values().next() {
                Ok(ReconciliationResult::Succeeded {
                    content: format!(
                        r#"{{"payment_id":"{}","status":"{}","amount":{}}}"#,
                        record.payment_id, record.status, record.amount
                    ),
                })
            } else if mode == MockPaymentMode::TimeoutBeforeCommit
                || mode == MockPaymentMode::AuthoritativeNotFound
            {
                Ok(ReconciliationResult::Failed {
                    reason: "Payment verified absent in downstream ledger".to_string(),
                })
            } else {
                Ok(ReconciliationResult::StillUnknown)
            }
        })
    }
}

fn setup_payment_test_app(ledger: Arc<MockPaymentLedger>) -> Arc<AppState> {
    let operation_cache = Cache::builder().max_capacity(100).build();
    let trace_store = Arc::new(dashmap::DashMap::new());

    Arc::new(AppState {
        http_client: reqwest::Client::new(),
        compliance_url: String::new(),
        rate_limit_max: 60,
        rate_limit_window: 60,
        dashboard_url: String::new(),
        llm_api_base_url: None,
        redis_client: None,
        telemetry: Arc::new(OssTelemetry::new(trace_store.clone())),
        billing: Arc::new(OssBilling),
        auth_resolver: Arc::new(OssAuth),
        rate_limiter: Arc::new(OssRateLimit),
        routing_config: Arc::new(OssRoutingConfig),
        semantic_cache: Arc::new(OssSemanticCache),
        execution_store: Arc::new(MokaExecutionStore::new(operation_cache.clone())),
        reconciler: Arc::new(LedgerReconciler {
            ledger: ledger.clone(),
        }),
        tool_transport: Arc::new(MockPaymentTransport {
            ledger: ledger.clone(),
        }),
        rate_limit_cache: Arc::new(dashmap::DashMap::new()),
        l1_cache: Arc::new(L1Cache::new(1024).unwrap()),
        routing_state: Arc::new(RoutingState::new()),
        circuit_breaker: Cache::builder().build(),
        loop_fallback_cache: Cache::builder().build(),
        mcp_registry: kryneth_gateway::infrastructure::mcp_registry::McpConnectionRegistry::empty(),
        tool_registry: kryneth_gateway::usecases::tool_router::ToolRegistry::empty(),
        agent_guardian_cache: Cache::builder().build(),
        operation_cache,
        dashboard_metrics: Arc::new(kryneth_gateway::domain::models::DashboardMetrics::new()),
        pricing_map: Arc::new(arc_swap::ArcSwap::from_pointee(HashMap::new())),
        trace_store,
        budget_map: Arc::new(arc_swap::ArcSwap::from_pointee(HashMap::new())),
    })
}

// ── ACCEPTANCE TESTS ────────────────────────────────────────────────────────

/// 1. Happy path: Normal payment success creates exactly one payment.
#[tokio::test]
async fn test_normal_payment_success_creates_one_payment() {
    let ledger = Arc::new(MockPaymentLedger::new(MockPaymentMode::Success));
    let app_state = setup_payment_test_app(ledger.clone());

    let trace_ctx = TraceContext {
        trace_id: "t_success".to_string(),
        session_id: "s1".to_string(),
        parent_trace_id: None,
        workflow_id: None,
        agent_id: None,
        execution_id: Some("exec_succ".to_string()),
        operation_id: Some("op_succ".to_string()),
        idempotency_key: Some("idem_succ".to_string()),
        test_scenario: None,
    };

    let tool_call = ToolCall {
        id: "call_succ".to_string(),
        name: "charge_payment".to_string(),
        arguments: r#"{"order_id":"ord_101","amount":100}"#.to_string(),
    };

    let res =
        ExecutionService::execute_tool(&app_state, tool_call, "tenant_1", &trace_ctx, 0, 1, false)
            .await;

    assert!(res.success);
    assert_eq!(ledger.total_mutations(), 1);
    assert_eq!(ledger.total_commits(), 1);
    assert!(ledger.get_payment("ord_101").is_some());
}

/// 2, 3, 4. Critical Sequence:
/// Payment committed → response lost → Kryneth marks UNKNOWN →
/// reconciliation finds payment → retry does not create a second payment.
#[tokio::test]
async fn test_payment_commit_then_timeout_reconciled_zero_duplicate_payments() {
    let ledger = Arc::new(MockPaymentLedger::new(MockPaymentMode::CommitThenTimeout));
    let app_state = setup_payment_test_app(ledger.clone());

    let trace_ctx = TraceContext {
        trace_id: "trace_payment_001".to_string(),
        session_id: "session_agent_1".to_string(),
        parent_trace_id: None,
        workflow_id: Some("wf_checkout".to_string()),
        agent_id: Some("agent_finance".to_string()),
        execution_id: Some("exec_001".to_string()),
        operation_id: Some("op_charge_order_99".to_string()),
        idempotency_key: Some("idem_order_99".to_string()),
        test_scenario: None,
    };

    let tool_call = ToolCall {
        id: "call_charge_1".to_string(),
        name: "process_payment".to_string(),
        arguments: r#"{"order_id":"ord_99","amount":500}"#.to_string(),
    };

    // Step 1: Initial attempt executes against payment service.
    // The payment commits in downstream ledger, but response times out!
    let res1 = ExecutionService::execute_tool(
        &app_state,
        tool_call.clone(),
        "tenant_acme",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;

    assert!(!res1.success);
    assert!(res1.content.contains("MCP_TIMEOUT"));
    assert_eq!(ledger.total_mutations(), 1);
    assert_eq!(ledger.total_commits(), 1);

    // Kryneth operation state must be marked UNKNOWN
    let cached_entries: Vec<_> = app_state.operation_cache.iter().collect();
    assert!(!cached_entries.is_empty());
    assert_eq!(cached_entries[0].1.state, ExecutionState::Unknown);

    // Step 2: Agent retries the exact same logical operation
    // Kryneth intercepts UNKNOWN mutating state -> reconciles with ledger -> recovers payment!
    let res2 = ExecutionService::execute_tool(
        &app_state,
        tool_call.clone(),
        "tenant_acme",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;

    assert!(res2.success);
    assert!(res2.content.contains("COMMITTED"));
    assert!(res2.content.contains("pay_"));

    // CRITICAL: ZERO duplicate mutations or commits downstream!
    assert_eq!(
        ledger.total_mutations(),
        1,
        "Downstream mutations must remain 1"
    );
    assert_eq!(
        ledger.total_commits(),
        1,
        "Downstream commits must remain 1"
    );

    // Step 3: Replay returns cached result
    let res3 = ExecutionService::execute_tool(
        &app_state,
        tool_call.clone(),
        "tenant_acme",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(res3.success);
    assert_eq!(res3.content, res2.content);
    assert_eq!(ledger.total_mutations(), 1);
    assert_eq!(ledger.total_commits(), 1);
}

/// 5. Authoritative confirmation that no mutation occurred allows safe retry.
#[tokio::test]
async fn test_timeout_before_commit_reconciliation_authoritative_failure_allows_safe_retry() {
    let ledger = Arc::new(MockPaymentLedger::new(MockPaymentMode::TimeoutBeforeCommit));
    let app_state = setup_payment_test_app(ledger.clone());

    let trace_ctx = TraceContext {
        trace_id: "t_no_commit".to_string(),
        session_id: "s1".to_string(),
        parent_trace_id: None,
        workflow_id: None,
        agent_id: None,
        execution_id: Some("exec_nc".to_string()),
        operation_id: Some("op_nc".to_string()),
        idempotency_key: Some("idem_nc".to_string()),
        test_scenario: None,
    };

    let tool_call = ToolCall {
        id: "call_nc".to_string(),
        name: "charge_payment".to_string(),
        arguments: r#"{"order_id":"ord_nc","amount":250}"#.to_string(),
    };

    // Attempt 1: times out before commit
    let res1 = ExecutionService::execute_tool(
        &app_state,
        tool_call.clone(),
        "tenant_1",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(!res1.success);
    assert_eq!(ledger.total_mutations(), 1);
    assert_eq!(ledger.total_commits(), 0);

    // Attempt 2 (retry): Kryneth reconciles -> reconciler proves NO payment exists in ledger!
    // Transitions to Failed
    let res2 = ExecutionService::execute_tool(
        &app_state,
        tool_call.clone(),
        "tenant_1",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(!res2.success);
    assert!(res2.content.contains("RECONCILIATION_FAILED"));

    // Now mock payment service is healthy
    ledger.set_mode(MockPaymentMode::Success);

    // Attempt 3: Because state became Failed, Kryneth safely allows re-claim and execution!
    let res3 = ExecutionService::execute_tool(
        &app_state,
        tool_call.clone(),
        "tenant_1",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(res3.success);
    assert_eq!(
        ledger.total_commits(),
        1,
        "Payment is committed exactly once!"
    );
}

/// 6. If reconciliation cannot determine outcome, Kryneth remains fail-closed for unsafe replay.
#[tokio::test]
async fn test_inconclusive_reconciliation_fails_closed() {
    let ledger = Arc::new(MockPaymentLedger::new(MockPaymentMode::CommitThenTimeout));
    let app_state = setup_payment_test_app(ledger.clone());

    let trace_ctx = TraceContext {
        trace_id: "t_inconclusive".to_string(),
        session_id: "s1".to_string(),
        parent_trace_id: None,
        workflow_id: None,
        agent_id: None,
        execution_id: Some("exec_inc".to_string()),
        operation_id: Some("op_inc".to_string()),
        idempotency_key: Some("idem_inc".to_string()),
        test_scenario: None,
    };

    let tool_call = ToolCall {
        id: "call_inc".to_string(),
        name: "charge_payment".to_string(),
        arguments: r#"{"order_id":"ord_inc","amount":300}"#.to_string(),
    };

    // Attempt 1: times out
    let res1 = ExecutionService::execute_tool(
        &app_state,
        tool_call.clone(),
        "tenant_1",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(!res1.success);

    // Set reconciler to inconclusive (downstream query times out / unreachable)
    ledger.set_mode(MockPaymentMode::InconclusiveReconciliation);

    // Attempt 2: Reconciliation returns StillUnknown -> unsafe replay MUST be blocked!
    let res2 = ExecutionService::execute_tool(
        &app_state,
        tool_call.clone(),
        "tenant_1",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(!res2.success);
    assert_eq!(res2.content, r#"{"error":"PREVIOUS_ATTEMPT_UNKNOWN"}"#);
    assert_eq!(
        ledger.total_mutations(),
        1,
        "No second downstream attempt allowed while outcome is unknown!"
    );
}

/// 8. Concurrent duplicates do not create duplicate mutations.
#[tokio::test]
async fn test_concurrent_duplicate_requests_zero_duplicate_mutations() {
    let ledger = Arc::new(MockPaymentLedger::new(MockPaymentMode::Success));
    let app_state = setup_payment_test_app(ledger.clone());

    let trace_ctx = TraceContext {
        trace_id: "t_concurrent".to_string(),
        session_id: "s1".to_string(),
        parent_trace_id: None,
        workflow_id: None,
        agent_id: None,
        execution_id: Some("exec_conc".to_string()),
        operation_id: Some("op_conc".to_string()),
        idempotency_key: Some("idem_concurrent_charge".to_string()),
        test_scenario: None,
    };

    let tool_call = ToolCall {
        id: "call_conc".to_string(),
        name: "charge_payment".to_string(),
        arguments: r#"{"order_id":"ord_conc","amount":750}"#.to_string(),
    };

    let state1 = app_state.clone();
    let state2 = app_state.clone();
    let tc1 = tool_call.clone();
    let tc2 = tool_call.clone();
    let ctx1 = trace_ctx.clone();
    let ctx2 = trace_ctx.clone();

    // Spawn 2 truly concurrent requests without artificial sleeps
    let h1 = tokio::spawn(async move {
        ExecutionService::execute_tool(&state1, tc1, "tenant_1", &ctx1, 0, 1, false).await
    });
    let h2 = tokio::spawn(async move {
        ExecutionService::execute_tool(&state2, tc2, "tenant_1", &ctx2, 0, 1, false).await
    });

    let (r1, r2) = tokio::join!(h1, h2);
    let res1 = r1.unwrap();
    let res2 = r2.unwrap();

    // Exactly one caller wins the claim and executes
    assert_eq!(
        ledger.total_mutations(),
        1,
        "Exactly 1 downstream mutation attempt allowed!"
    );
    assert_eq!(
        ledger.total_commits(),
        1,
        "Exactly 1 payment committed downstream!"
    );

    // One succeeds, the competing caller is rejected with ALREADY_IN_FLIGHT
    let one_succeeded = res1.success || res2.success;
    let one_in_flight =
        res1.content.contains("ALREADY_IN_FLIGHT") || res2.content.contains("ALREADY_IN_FLIGHT");
    assert!(one_succeeded, "At least one call must succeed");
    assert!(
        one_in_flight,
        "The competing concurrent call must be blocked with ALREADY_IN_FLIGHT"
    );
}

/// 9. A delayed stale attempt cannot overwrite newer Kryneth state.
#[tokio::test]
async fn test_stale_delayed_attempt_cannot_overwrite_newer_state() {
    let operation_cache = Cache::builder().max_capacity(100).build();
    let store = Arc::new(MokaExecutionStore::new(operation_cache.clone()));
    let op_key = IdempotencyKey("idem_version_fence".to_string());

    // Setup state at version 2 (simulating a reclaimed/newer attempt)
    let exec = ToolExecution {
        execution_id: kryneth_gateway::domain::execution::ExecutionId("exec_1".to_string()),
        operation_id: kryneth_gateway::domain::execution::OperationId("op_1".to_string()),
        workflow_id: None,
        agent_id: None,
        tenant_id: kryneth_gateway::domain::execution::TenantId("tenant_1".to_string()),
        session_id: None,
        tool_name: "charge_payment".to_string(),
        arguments_hash: op_key.0.clone(),
        idempotency_key: op_key.clone(),
        attempt: 2,
        state: ExecutionState::Claimed,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let ctx = kryneth_gateway::domain::execution::ExecutionContext {
        result_content: None,
        latency_ms: None,
        error_message: None,
        lease_until: None,
        version: 2,
    };
    operation_cache
        .insert(
            op_key.0.clone(),
            kryneth_gateway::domain::models::OperationCacheEntry {
                state: ExecutionState::Claimed,
                context: ctx,
                execution: exec,
            },
        )
        .await;

    // Simulate stale Attempt 1 (version 1) completing late and calling mark_succeeded with version 1
    let mark_res = store
        .mark_succeeded(&op_key, 1, r#"{"status":"stale"}"#.to_string(), 100)
        .await;
    assert!(mark_res.is_ok());

    // State MUST remain at version 2 in Claimed state (stale attempt fenced out!)
    let (state, ctx, _) = store.get(&op_key).await.unwrap().unwrap();
    assert_eq!(state, ExecutionState::Claimed);
    assert_eq!(ctx.version, 2);
    assert!(
        ctx.result_content.is_none(),
        "Stale content must NOT overwrite state!"
    );
}

/// 11. Identity tests: JSON key ordering produces identical IdempotencyKey and deduplicates.
#[tokio::test]
async fn test_json_key_ordering_canonicalization_deduplication() {
    let ledger = Arc::new(MockPaymentLedger::new(MockPaymentMode::Success));
    let app_state = setup_payment_test_app(ledger.clone());

    let trace_ctx = TraceContext {
        trace_id: "t_canon".to_string(),
        session_id: "s1".to_string(),
        parent_trace_id: None,
        workflow_id: None,
        agent_id: None,
        execution_id: Some("exec_canon".to_string()),
        operation_id: Some("op_canon".to_string()),
        idempotency_key: Some("idem_canon_order".to_string()),
        test_scenario: None,
    };

    // Attempt 1: order: {amount, currency, order_id}
    let tc1 = ToolCall {
        id: "call_1".to_string(),
        name: "charge_payment".to_string(),
        arguments: r#"{"amount":100,"currency":"USD","order_id":"ord_canon"}"#.to_string(),
    };

    let res1 =
        ExecutionService::execute_tool(&app_state, tc1, "tenant_1", &trace_ctx, 0, 1, false).await;
    assert!(res1.success);
    assert_eq!(ledger.total_mutations(), 1);

    // Attempt 2: Reordered JSON keys: {order_id, currency, amount}
    let tc2 = ToolCall {
        id: "call_2".to_string(), // New tool call ID!
        name: "charge_payment".to_string(),
        arguments: r#"{"order_id":"ord_canon","currency":"USD","amount":100}"#.to_string(),
    };

    let res2 =
        ExecutionService::execute_tool(&app_state, tc2, "tenant_1", &trace_ctx, 0, 1, false).await;
    assert!(res2.success);
    assert_eq!(
        res2.content, res1.content,
        "Must return cached response from attempt 1"
    );

    // Downstream mutations MUST REMAIN 1! Canonicalization prevented duplicate execution!
    assert_eq!(
        ledger.total_mutations(),
        1,
        "Reordered JSON keys must NOT cause a second downstream mutation!"
    );
}

/// 10. Process restart test: Documents what happens to operation state with in-memory adapter.
#[tokio::test]
async fn test_in_memory_process_restart_boundary_documented() {
    let ledger = Arc::new(MockPaymentLedger::new(MockPaymentMode::Success));

    // Gateway instance 1
    let app_state_1 = setup_payment_test_app(ledger.clone());
    let trace_ctx = TraceContext {
        trace_id: "t_restart".to_string(),
        session_id: "s1".to_string(),
        parent_trace_id: None,
        workflow_id: None,
        agent_id: None,
        execution_id: Some("exec_r".to_string()),
        operation_id: Some("op_r".to_string()),
        idempotency_key: Some("idem_restart".to_string()),
        test_scenario: None,
    };
    let tc = ToolCall {
        id: "call_r".to_string(),
        name: "charge_payment".to_string(),
        arguments: r#"{"order_id":"ord_restart","amount":100}"#.to_string(),
    };

    let res1 = ExecutionService::execute_tool(
        &app_state_1,
        tc.clone(),
        "tenant_1",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(res1.success);
    assert_eq!(ledger.total_mutations(), 1);

    // Simulate Gateway Process Restart (creates new AppState / in-memory store)
    let app_state_2 = setup_payment_test_app(ledger.clone());

    // Because MokaExecutionStore is strictly process-local in RAM, the new process does not have the previous cache entry
    let res2 =
        ExecutionService::execute_tool(&app_state_2, tc, "tenant_1", &trace_ctx, 0, 1, false).await;
    assert!(res2.success);

    // This documents the exact architectural boundary: in-memory store executes again across restarts
    assert_eq!(
        ledger.total_mutations(),
        2,
        "Documented limitation: in-memory storage adapter loses idempotency state across process restarts"
    );
}
