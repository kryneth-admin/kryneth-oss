//! tests/agent_db_reliability_test.rs — Agent Database Reliability & Adversarial Harness.
//!
//! Validates Kryneth execution integrity and reliability for AI Agents interacting
//! with databases and persistent downstream storage:
//! 1. Agent DB Mutation: Commit-then-timeout reconciled without double SQL mutation.
//! 2. Agent DB Rollback: Timeout before commit allows safe retry after reconciliation.
//! 3. High-Concurrency Thundering Herd: 10 concurrent agent requests for the same DB mutation.
//! 4. Flaky / Intermittent Transport: Multi-step network flap retains UNKNOWN until recovery.
//! 5. Policy Separation: Safe read-only queries vs. unsafe mutating statements.
//! 6. Agent Runaway Loop Detection: Guardian prevents infinite mutation tool loops.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use kryneth_gateway::domain::execution::{ExecutionState, ToolExecution};
use kryneth_gateway::domain::models::{AppState, RoutingState, TraceContext};
use kryneth_gateway::domain::ports::{Reconciler, ReconciliationResult, ToolTransport};
use kryneth_gateway::infrastructure::l1_cache::L1Cache;
use kryneth_gateway::infrastructure::mcp_client::{ToolCall, ToolResult};
use kryneth_gateway::infrastructure::oss_adapters::{
    MokaExecutionStore, OssAuth, OssBilling, OssRateLimit, OssRoutingConfig, OssSemanticCache,
    OssTelemetry,
};
use kryneth_gateway::usecases::behavior_guard::enforce_oss_agent_guardian;
use kryneth_gateway::usecases::execution_service::{resolve_tool_policy, ExecutionService};
use moka::future::Cache;
use serde_json::{json, Value};

// ── Mock Agent Database & Transaction Engine ────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DbTransactionRecord {
    pub tx_id: String,
    pub account_id: String,
    pub amount_delta: i64,
    pub status: String,
    pub committed_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockDbMode {
    NormalSuccess,
    CommitThenTimeout,
    RollbackOnTimeout,
    FlakyReconciliation,
    DelayedWrite { delay_ms: u64 },
}

pub struct MockAgentDatabase {
    pub accounts: Arc<Mutex<HashMap<String, i64>>>,
    pub transactions: Arc<Mutex<HashMap<String, DbTransactionRecord>>>,
    pub mutation_attempts: Arc<AtomicUsize>,
    pub commit_count: Arc<AtomicUsize>,
    pub rollback_count: Arc<AtomicUsize>,
    pub mode: Arc<Mutex<MockDbMode>>,
}

impl MockAgentDatabase {
    pub fn new(mode: MockDbMode, initial_balance: i64) -> Self {
        let mut accounts = HashMap::new();
        accounts.insert("acc_agent_main".to_string(), initial_balance);

        Self {
            accounts: Arc::new(Mutex::new(accounts)),
            transactions: Arc::new(Mutex::new(HashMap::new())),
            mutation_attempts: Arc::new(AtomicUsize::new(0)),
            commit_count: Arc::new(AtomicUsize::new(0)),
            rollback_count: Arc::new(AtomicUsize::new(0)),
            mode: Arc::new(Mutex::new(mode)),
        }
    }

    pub fn set_mode(&self, new_mode: MockDbMode) {
        let mut m = self.mode.lock().unwrap();
        *m = new_mode;
    }

    pub fn get_balance(&self, account_id: &str) -> i64 {
        let accs = self.accounts.lock().unwrap();
        *accs.get(account_id).unwrap_or(&0)
    }

    pub fn total_mutations(&self) -> usize {
        self.mutation_attempts.load(Ordering::SeqCst)
    }

    pub fn total_commits(&self) -> usize {
        self.commit_count.load(Ordering::SeqCst)
    }
}

// ── Database Transport Adapter ──────────────────────────────────────────────

pub struct MockDatabaseTransport {
    pub db: Arc<MockAgentDatabase>,
}

impl ToolTransport for MockDatabaseTransport {
    fn execute_tool<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        arguments: &'a str,
        _tenant_id: &'a str,
        _enable_compression: bool,
        _test_scenario: Option<&'a str>,
    ) -> Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        let db = self.db.clone();
        let cid = tool_call_id.to_string();
        let name = tool_name.to_string();
        let raw_args = arguments.to_string();

        Box::pin(async move {
            let parsed: Value = serde_json::from_str(&raw_args).unwrap_or(json!({}));
            let account_id = parsed["account_id"]
                .as_str()
                .unwrap_or("acc_agent_main")
                .to_string();
            let amount_deduct = parsed["amount"].as_i64().unwrap_or(100);

            // Read-only queries do not mutate state
            if name.starts_with("query") || name.starts_with("select") || name.starts_with("read") {
                let current_balance = db.get_balance(&account_id);
                return ToolResult {
                    tool_call_id: cid,
                    name,
                    content: format!(
                        r#"{{"account_id":"{}","balance":{}}}"#,
                        account_id, current_balance
                    ),
                    latency_ms: 5,
                    success: true,
                };
            }

            // Mutating database tool
            db.mutation_attempts.fetch_add(1, Ordering::SeqCst);
            let mode = *db.mode.lock().unwrap();

            match mode {
                MockDbMode::NormalSuccess => {
                    let tx_id = format!("tx_{}", uuid::Uuid::new_v4().simple());
                    // Apply DB balance mutation
                    {
                        let mut accs = db.accounts.lock().unwrap();
                        let bal = accs.entry(account_id.clone()).or_insert(1000);
                        *bal -= amount_deduct;
                    }
                    // Insert into transaction log
                    db.transactions.lock().unwrap().insert(
                        tx_id.clone(),
                        DbTransactionRecord {
                            tx_id: tx_id.clone(),
                            account_id: account_id.clone(),
                            amount_delta: -amount_deduct,
                            status: "COMMITTED".to_string(),
                            committed_at: Utc::now(),
                        },
                    );
                    db.commit_count.fetch_add(1, Ordering::SeqCst);

                    let new_balance = db.get_balance(&account_id);
                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: format!(
                            r#"{{"tx_id":"{}","account_id":"{}","new_balance":{},"status":"COMMITTED"}}"#,
                            tx_id, account_id, new_balance
                        ),
                        latency_ms: 12,
                        success: true,
                    }
                }
                MockDbMode::CommitThenTimeout => {
                    // Database commits transaction, but network socket drops on return path
                    let tx_id = format!("tx_{}", uuid::Uuid::new_v4().simple());
                    {
                        let mut accs = db.accounts.lock().unwrap();
                        let bal = accs.entry(account_id.clone()).or_insert(1000);
                        *bal -= amount_deduct;
                    }
                    db.transactions.lock().unwrap().insert(
                        tx_id.clone(),
                        DbTransactionRecord {
                            tx_id: tx_id.clone(),
                            account_id: account_id.clone(),
                            amount_delta: -amount_deduct,
                            status: "COMMITTED".to_string(),
                            committed_at: Utc::now(),
                        },
                    );
                    db.commit_count.fetch_add(1, Ordering::SeqCst);

                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: r#"{"error":"MCP_TIMEOUT","message":"Database socket timeout after commit"}"#.to_string(),
                        latency_ms: 5000,
                        success: false,
                    }
                }
                MockDbMode::RollbackOnTimeout => {
                    // DB transaction rolls back before commit
                    db.rollback_count.fetch_add(1, Ordering::SeqCst);
                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: r#"{"error":"MCP_TIMEOUT","message":"Transaction rolled back before commit"}"#.to_string(),
                        latency_ms: 5000,
                        success: false,
                    }
                }
                MockDbMode::FlakyReconciliation => {
                    // Simulates intermittent failure
                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: r#"{"error":"MCP_TIMEOUT","message":"Transient database connection timeout"}"#.to_string(),
                        latency_ms: 5000,
                        success: false,
                    }
                }
                MockDbMode::DelayedWrite { delay_ms } => {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    let tx_id = format!("tx_{}", uuid::Uuid::new_v4().simple());
                    {
                        let mut accs = db.accounts.lock().unwrap();
                        let bal = accs.entry(account_id.clone()).or_insert(1000);
                        *bal -= amount_deduct;
                    }
                    db.transactions.lock().unwrap().insert(
                        tx_id.clone(),
                        DbTransactionRecord {
                            tx_id: tx_id.clone(),
                            account_id: account_id.clone(),
                            amount_delta: -amount_deduct,
                            status: "COMMITTED".to_string(),
                            committed_at: Utc::now(),
                        },
                    );
                    db.commit_count.fetch_add(1, Ordering::SeqCst);

                    let new_balance = db.get_balance(&account_id);
                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: format!(
                            r#"{{"tx_id":"{}","account_id":"{}","new_balance":{},"status":"COMMITTED"}}"#,
                            tx_id, account_id, new_balance
                        ),
                        latency_ms: delay_ms,
                        success: true,
                    }
                }
            }
        })
    }
}

// ── Database Reconciler ─────────────────────────────────────────────────────

pub struct DatabaseReconciler {
    pub db: Arc<MockAgentDatabase>,
}

impl Reconciler for DatabaseReconciler {
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
        let db = self.db.clone();
        let mode = *db.mode.lock().unwrap();

        Box::pin(async move {
            if mode == MockDbMode::FlakyReconciliation {
                return Ok(ReconciliationResult::StillUnknown);
            }

            let txs = db.transactions.lock().unwrap();
            if let Some(record) = txs.values().next() {
                Ok(ReconciliationResult::Succeeded {
                    content: format!(
                        r#"{{"tx_id":"{}","account_id":"{}","status":"COMMITTED"}}"#,
                        record.tx_id, record.account_id
                    ),
                })
            } else if mode == MockDbMode::RollbackOnTimeout {
                Ok(ReconciliationResult::Failed {
                    reason: "Transaction was rolled back; verified absent in DB transaction log"
                        .to_string(),
                })
            } else {
                Ok(ReconciliationResult::StillUnknown)
            }
        })
    }
}

fn setup_agent_db_test_app(db: Arc<MockAgentDatabase>) -> Arc<AppState> {
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
        reconciler: Arc::new(DatabaseReconciler { db: db.clone() }),
        tool_transport: Arc::new(MockDatabaseTransport { db: db.clone() }),
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

// ── AGENT DATABASE RELIABILITY TESTS ────────────────────────────────────────

/// 1. Agent DB Mutation: Commit-then-timeout reconciles without double SQL debit.
#[tokio::test]
async fn test_agent_db_mutation_timeout_reconciled_zero_double_debit() {
    // Initial balance: 1,000
    let db = Arc::new(MockAgentDatabase::new(MockDbMode::CommitThenTimeout, 1000));
    let app = setup_agent_db_test_app(db.clone());

    let trace_ctx = TraceContext {
        trace_id: "t_db_1".to_string(),
        session_id: "agent_session_10".to_string(),
        parent_trace_id: None,
        workflow_id: Some("wf_db_debit".to_string()),
        agent_id: Some("agent_sql_runner".to_string()),
        execution_id: Some("exec_db_1".to_string()),
        operation_id: Some("op_db_debit_100".to_string()),
        idempotency_key: Some("idem_db_transfer_100".to_string()),
        test_scenario: None,
    };

    let tool_call = ToolCall {
        id: "call_sql_1".to_string(),
        name: "update_database_record".to_string(),
        arguments: r#"{"account_id":"acc_agent_main","amount":100}"#.to_string(),
    };

    // Step 1: Agent fires DB mutation. DB commits balance: 1,000 -> 900, but socket drops.
    let res1 = ExecutionService::execute_tool(
        &app,
        tool_call.clone(),
        "tenant_db",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;

    assert!(!res1.success);
    assert!(res1.content.contains("MCP_TIMEOUT"));
    assert_eq!(
        db.get_balance("acc_agent_main"),
        900,
        "Database balance debited once to 900"
    );
    assert_eq!(db.total_commits(), 1);
    assert_eq!(db.total_mutations(), 1);

    // Kryneth operation state must be UNKNOWN
    let cached_entries: Vec<_> = app.operation_cache.iter().collect();
    assert!(
        !cached_entries.is_empty(),
        "Cache must contain operation entry"
    );
    assert_eq!(
        cached_entries[0].1.state,
        ExecutionState::Unknown,
        "Kryneth state must be UNKNOWN"
    );

    // Step 2: Agent blindly retries the exact same mutation.
    // Kryneth intercepts UNKNOWN -> Reconciler checks DB transaction log -> Recovers transaction!
    let res2 = ExecutionService::execute_tool(
        &app,
        tool_call.clone(),
        "tenant_db",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;

    assert!(res2.success, "Retry must succeed through reconciliation");
    assert!(res2.content.contains("COMMITTED"));

    // CRITICAL RELIABILITY INVARIANT:
    // Balance MUST REMAIN 900! It must NOT be deducted twice (which would be 800)!
    assert_eq!(
        db.get_balance("acc_agent_main"),
        900,
        "Account balance MUST remain 900! Zero duplicate database deductions!"
    );
    assert_eq!(
        db.total_mutations(),
        1,
        "Downstream DB mutation statements executed must remain 1"
    );
    assert_eq!(
        db.total_commits(),
        1,
        "Downstream DB committed transactions must remain 1"
    );
}

/// 2. Agent DB Rollback on Timeout: Safe retry executed after authoritative reconciliation.
#[tokio::test]
async fn test_agent_db_rollback_on_timeout_allows_safe_retry() {
    let db = Arc::new(MockAgentDatabase::new(MockDbMode::RollbackOnTimeout, 1000));
    let app = setup_agent_db_test_app(db.clone());

    let trace_ctx = TraceContext {
        trace_id: "t_db_rollback".to_string(),
        session_id: "s1".to_string(),
        parent_trace_id: None,
        workflow_id: None,
        agent_id: None,
        execution_id: Some("exec_rb".to_string()),
        operation_id: Some("op_rb".to_string()),
        idempotency_key: Some("idem_rb".to_string()),
        test_scenario: None,
    };

    let tool_call = ToolCall {
        id: "call_rb".to_string(),
        name: "update_database_record".to_string(),
        arguments: r#"{"account_id":"acc_agent_main","amount":150}"#.to_string(),
    };

    // Attempt 1: DB transaction aborts & rolls back
    let res1 = ExecutionService::execute_tool(
        &app,
        tool_call.clone(),
        "tenant_db",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(!res1.success);
    assert_eq!(
        db.get_balance("acc_agent_main"),
        1000,
        "Balance unaffected on rollback"
    );

    // Attempt 2 (retry): Kryneth reconciles -> DB confirms transaction aborted/absent -> marks Failed
    let res2 = ExecutionService::execute_tool(
        &app,
        tool_call.clone(),
        "tenant_db",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(!res2.success);
    assert!(res2.content.contains("RECONCILIATION_FAILED"));

    // DB recovers to normal mode
    db.set_mode(MockDbMode::NormalSuccess);

    // Attempt 3: Because state became Failed, Kryneth safely permits fresh claim & commit
    let res3 = ExecutionService::execute_tool(
        &app,
        tool_call.clone(),
        "tenant_db",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(res3.success);
    assert_eq!(
        db.get_balance("acc_agent_main"),
        850,
        "Balance debited exactly once by 150 to 850"
    );
    assert_eq!(db.total_commits(), 1);
}

/// 3. Thundering Herd: 10 concurrent agent workers attempting the same DB mutation.
#[tokio::test]
async fn test_agent_db_burst_concurrency_thundering_herd() {
    let db = Arc::new(MockAgentDatabase::new(MockDbMode::NormalSuccess, 1000));
    let app = setup_agent_db_test_app(db.clone());

    let trace_ctx = TraceContext {
        trace_id: "t_thundering".to_string(),
        session_id: "session_herd".to_string(),
        parent_trace_id: None,
        workflow_id: None,
        agent_id: None,
        execution_id: Some("exec_herd".to_string()),
        operation_id: Some("op_herd".to_string()),
        idempotency_key: Some("idem_herd_debit".to_string()),
        test_scenario: None,
    };

    let tool_call = ToolCall {
        id: "call_herd".to_string(),
        name: "update_database_record".to_string(),
        arguments: r#"{"account_id":"acc_agent_main","amount":200}"#.to_string(),
    };

    // Spawn 10 concurrent agent tasks simultaneously
    let mut handles = Vec::new();
    for _ in 0..10 {
        let app_c = app.clone();
        let tc_c = tool_call.clone();
        let ctx_c = trace_ctx.clone();
        handles.push(tokio::spawn(async move {
            ExecutionService::execute_tool(&app_c, tc_c, "tenant_db", &ctx_c, 0, 1, false).await
        }));
    }

    let mut success_count = 0;
    let mut in_flight_count = 0;

    for h in handles {
        let res = h.await.unwrap();
        if res.success {
            success_count += 1;
        } else if res.content.contains("ALREADY_IN_FLIGHT") {
            in_flight_count += 1;
        }
    }

    // Exactly 1 winner executes the DB mutation
    assert_eq!(
        success_count, 1,
        "Exactly 1 concurrent request must succeed"
    );
    assert_eq!(
        in_flight_count, 9,
        "The other 9 concurrent requests must be rejected with ALREADY_IN_FLIGHT"
    );

    // Downstream DB mutation count and commits must be EXACTLY 1
    assert_eq!(db.total_mutations(), 1, "DB mutations must be exactly 1");
    assert_eq!(db.total_commits(), 1, "DB commits must be exactly 1");
    assert_eq!(
        db.get_balance("acc_agent_main"),
        800,
        "Account balance must be decremented by 200 exactly once (1000 - 200 = 800)"
    );
}

/// 4. Flaky / Intermittent Transport: Network flap maintains UNKNOWN until recovery.
#[tokio::test]
async fn test_agent_db_intermittent_flaky_reconciliation() {
    let db = Arc::new(MockAgentDatabase::new(MockDbMode::CommitThenTimeout, 1000));
    let app = setup_agent_db_test_app(db.clone());

    let trace_ctx = TraceContext {
        trace_id: "t_flaky".to_string(),
        session_id: "s1".to_string(),
        parent_trace_id: None,
        workflow_id: None,
        agent_id: None,
        execution_id: Some("exec_flaky".to_string()),
        operation_id: Some("op_flaky".to_string()),
        idempotency_key: Some("idem_flaky".to_string()),
        test_scenario: None,
    };

    let tool_call = ToolCall {
        id: "call_flaky".to_string(),
        name: "update_database_record".to_string(),
        arguments: r#"{"account_id":"acc_agent_main","amount":50}"#.to_string(),
    };

    // Step 1: Initial mutation times out after commit
    let res1 = ExecutionService::execute_tool(
        &app,
        tool_call.clone(),
        "tenant_db",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    assert!(!res1.success);
    assert_eq!(db.get_balance("acc_agent_main"), 950);

    // Step 2: Reconciliation network is down / flaky
    db.set_mode(MockDbMode::FlakyReconciliation);
    let res2 = ExecutionService::execute_tool(
        &app,
        tool_call.clone(),
        "tenant_db",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    // Fails closed! Unsafe replay blocked!
    assert!(!res2.success);
    assert_eq!(res2.content, r#"{"error":"PREVIOUS_ATTEMPT_UNKNOWN"}"#);
    assert_eq!(
        db.total_mutations(),
        1,
        "No duplicate mutation while outcome is unknown"
    );

    // Step 3: Downstream network recovers
    db.set_mode(MockDbMode::NormalSuccess);
    let res3 = ExecutionService::execute_tool(
        &app,
        tool_call.clone(),
        "tenant_db",
        &trace_ctx,
        0,
        1,
        false,
    )
    .await;
    // Reconciler recovers committed transaction!
    assert!(res3.success);
    assert!(res3.content.contains("COMMITTED"));
    assert_eq!(db.get_balance("acc_agent_main"), 950);
    assert_eq!(db.total_mutations(), 1);
}

/// 5. Policy Separation: Safe read-only queries vs. mutating statements.
#[tokio::test]
async fn test_agent_db_read_only_vs_mutating_policy_classification() {
    let policy_read = resolve_tool_policy("query_database");
    assert_eq!(
        policy_read.retry_policy,
        kryneth_gateway::domain::execution_policy::RetryPolicy::Safe
    );
    assert_eq!(
        policy_read.side_effect_class,
        kryneth_gateway::domain::execution_policy::SideEffectClass::ReadOnly
    );

    let policy_select = resolve_tool_policy("select_customer_records");
    assert_eq!(
        policy_select.retry_policy,
        kryneth_gateway::domain::execution_policy::RetryPolicy::Safe
    );

    let policy_mutate = resolve_tool_policy("update_database_record");
    assert_eq!(
        policy_mutate.retry_policy,
        kryneth_gateway::domain::execution_policy::RetryPolicy::Unsafe
    );
    assert_eq!(
        policy_mutate.side_effect_class,
        kryneth_gateway::domain::execution_policy::SideEffectClass::Irreversible
    );

    let policy_composite = resolve_tool_policy("get_or_create_account");
    assert_eq!(
        policy_composite.retry_policy,
        kryneth_gateway::domain::execution_policy::RetryPolicy::Unsafe
    );
    assert_eq!(
        policy_composite.side_effect_class,
        kryneth_gateway::domain::execution_policy::SideEffectClass::Irreversible
    );
}

/// 6. Agent Runaway Loop Detection: Guardian halts infinite DB mutation tool loops.
#[tokio::test]
async fn test_agent_runaway_loop_on_database_mutations_blocked_by_guardian() {
    let db = Arc::new(MockAgentDatabase::new(MockDbMode::NormalSuccess, 1000));
    let app = setup_agent_db_test_app(db.clone());
    let session_id = "agent_loop_session_42";

    // Simulate agent emitting the exact same database update tool call in a loop
    let request_body = json!({
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [
                    {
                        "id": "call_loop_1",
                        "type": "function",
                        "function": {
                            "name": "update_database_record",
                            "arguments": "{\"account_id\":\"acc_agent_main\",\"amount\":10}"
                        }
                    }
                ]
            }
        ]
    });
    let body_bytes = serde_json::to_vec(&request_body).unwrap();

    // In Kryneth, DEFAULT_MAX_IDENTICAL_TOOL_CALLS is 5.
    // Iterations 1..=5 pass through:
    for i in 1..=5 {
        let res = enforce_oss_agent_guardian(&app, "tenant_db", session_id, &body_bytes).await;
        assert!(res.is_ok(), "Call {} within threshold should pass", i);
    }

    // Call 6: Exceeds threshold -> Agent Guardian intercepts and halts runaway loop!
    let blocked_res = enforce_oss_agent_guardian(&app, "tenant_db", session_id, &body_bytes).await;
    assert!(
        blocked_res.is_err(),
        "Agent loop must be blocked on exceeding repetition threshold"
    );

    match blocked_res.unwrap_err() {
        kryneth_gateway::domain::models::GatewayError::AgentRunawayLoop(msg) => {
            assert!(msg.contains("infinite loop"));
        }
        other => panic!("Expected AgentRunawayLoop, got {:?}", other),
    }

    // Metric is incremented
    assert!(
        app.dashboard_metrics
            .blocked_agent_loops
            .load(Ordering::Relaxed)
            > 0,
        "Dashboard metrics must record blocked agent runaway loop"
    );
}
