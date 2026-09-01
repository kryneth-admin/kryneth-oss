use crate::domain::execution::{
    AgentId, ExecutionId, ExecutionState, IdempotencyKey, OperationId, SessionId, TenantId,
    ToolExecution, WorkflowId,
};
use crate::domain::execution_policy::{
    IdempotencyRequirement, RetryPolicy, SideEffectClass, ToolExecutionPolicy,
};
use crate::domain::models::{AppState, TraceContext};
use crate::domain::ports::ReconciliationResult;
use crate::infrastructure::mcp_client::{ToolCall, ToolResult};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

pub fn resolve_tool_policy(tool_name: &str) -> ToolExecutionPolicy {
    let lower = tool_name.to_lowercase();
    if lower.starts_with("get")
        || lower.starts_with("list")
        || lower.starts_with("read")
        || lower.starts_with("describe")
        || lower.starts_with("search")
        || lower.starts_with("query")
        || lower.starts_with("show")
    {
        ToolExecutionPolicy {
            retry_policy: RetryPolicy::Safe,
            side_effect_class: SideEffectClass::ReadOnly,
            idempotency_requirement: IdempotencyRequirement::None,
            timeout_seconds: 5,
        }
    } else {
        ToolExecutionPolicy {
            retry_policy: RetryPolicy::Unsafe,
            side_effect_class: SideEffectClass::Irreversible,
            idempotency_requirement: IdempotencyRequirement::Required,
            timeout_seconds: 5,
        }
    }
}

pub struct ExecutionService;

impl ExecutionService {
    pub async fn execute_tool(
        state: &Arc<AppState>,
        tool_call: ToolCall,
        tenant_id: &str,
        trace_ctx: &TraceContext,
        tool_index: usize,
        tool_calls_len: usize,
        enable_compression: bool,
    ) -> ToolResult {
        let tool_call_id = tool_call.id.clone();
        let tool_name = tool_call.name.clone();
        let start = std::time::Instant::now();
        let policy = resolve_tool_policy(&tool_name);

        // 1. Resolve execution identity
        let exec_id = ExecutionId(
            trace_ctx
                .execution_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
        );
        let wf_id = trace_ctx.workflow_id.clone().map(WorkflowId);
        let ag_id = trace_ctx.agent_id.clone().map(AgentId);
        let t_id = TenantId(tenant_id.to_string());
        let sess_id = Some(SessionId(trace_ctx.session_id.clone()));

        let operation_id = if let (1, Some(ref op_id)) = (tool_calls_len, &trace_ctx.operation_id) {
            OperationId(op_id.clone())
        } else {
            let mut hasher = Sha256::new();
            hasher.update(exec_id.0.as_bytes());
            hasher.update(b"::");
            hasher.update(tool_call.id.as_bytes());
            OperationId(hex::encode(hasher.finalize()))
        };

        let is_explicit_idem = trace_ctx.idempotency_key.is_some();
        let idempotency_key = if let Some(ref key) = trace_ctx.idempotency_key {
            let mut hasher = Sha256::new();
            hasher.update(tenant_id.as_bytes());
            hasher.update(b"::");
            hasher.update(key.as_bytes());
            hasher.update(b"::");
            hasher.update(tool_index.to_string().as_bytes());
            hasher.update(b"::");
            hasher.update(tool_call.name.as_bytes());
            hasher.update(b"::");
            hasher.update(tool_call.arguments.as_bytes());
            IdempotencyKey(hex::encode(hasher.finalize()))
        } else {
            let mut hasher = Sha256::new();
            hasher.update(tenant_id.as_bytes());
            hasher.update(b"::");
            hasher.update(exec_id.0.as_bytes());
            hasher.update(b"::");
            hasher.update(tool_call.id.as_bytes());
            IdempotencyKey(hex::encode(hasher.finalize()))
        };

        // 2. Load or create the claim
        let initial_execution = ToolExecution {
            execution_id: exec_id.clone(),
            operation_id: operation_id.clone(),
            workflow_id: wf_id.clone(),
            agent_id: ag_id.clone(),
            tenant_id: t_id.clone(),
            session_id: sess_id.clone(),
            tool_name: tool_call.name.clone(),
            arguments_hash: idempotency_key.0.clone(),
            idempotency_key: idempotency_key.clone(),
            attempt: 1,
            state: ExecutionState::Pending,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let lease_duration = std::time::Duration::from_secs(10);
        let claim_res = state
            .execution_store
            .create_or_claim(initial_execution.clone(), lease_duration)
            .await;

        let (claim_state, mut claim_ctx) = match claim_res {
            Ok(pair) => pair,
            Err(e) => {
                return ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    name: tool_name.clone(),
                    content: format!(r#"{{"error":"CLAIM_FAILED","message":"{:?}"}}"#, e),
                    latency_ms: start.elapsed().as_millis() as u64,
                    success: false,
                };
            }
        };

        // 3. Reason about existing state / auto-retry block
        if claim_state == ExecutionState::Succeeded {
            let content = claim_ctx.result_content.clone().unwrap_or_default();
            let latency_ms = claim_ctx.latency_ms.unwrap_or(0);
            return ToolResult {
                tool_call_id: tool_call_id.clone(),
                name: tool_name.clone(),
                content,
                latency_ms,
                success: true,
            };
        }

        if claim_state == ExecutionState::Running {
            // Already in flight
            state
                .dashboard_metrics
                .mcp_already_in_flight_blocked
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return ToolResult {
                tool_call_id: tool_call_id.clone(),
                name: tool_name.clone(),
                content: r#"{"error":"ALREADY_IN_FLIGHT"}"#.to_string(),
                latency_ms: 0,
                success: false,
            };
        }

        if claim_state == ExecutionState::Unknown {
            // Unsafe retry check
            if policy.retry_policy != RetryPolicy::Safe && is_explicit_idem {
                // Mutating tool + explicit idempotency key + UNKNOWN = Block retry, try reconciliation
                let mut resolved_via_reconciliation = false;
                let mut content_out = String::new();
                let mut latency_out = 0u64;

                // Try reconciliation
                let recon_res = state.reconciler.reconcile(&initial_execution).await;
                match recon_res {
                    Ok(ReconciliationResult::Succeeded { content }) => {
                        let latency = start.elapsed().as_millis() as u64;
                        let _ = state
                            .execution_store
                            .mark_succeeded(
                                &idempotency_key,
                                claim_ctx.version,
                                content.clone(),
                                latency,
                            )
                            .await;
                        content_out = content;
                        latency_out = latency;
                        resolved_via_reconciliation = true;
                    }
                    Ok(ReconciliationResult::Failed { reason }) => {
                        let _ = state
                            .execution_store
                            .mark_failed(&idempotency_key, claim_ctx.version, reason.clone())
                            .await;
                        return ToolResult {
                            tool_call_id: tool_call_id.clone(),
                            name: tool_name.clone(),
                            content: format!(
                                r#"{{"error":"RECONCILIATION_FAILED","reason":"{}"}}"#,
                                reason
                            ),
                            latency_ms: start.elapsed().as_millis() as u64,
                            success: false,
                        };
                    }
                    _ => {} // Still unknown, block retry below
                }

                if resolved_via_reconciliation {
                    return ToolResult {
                        tool_call_id: tool_call_id.clone(),
                        name: tool_name.clone(),
                        content: content_out,
                        latency_ms: latency_out,
                        success: true,
                    };
                }

                state
                    .dashboard_metrics
                    .mcp_previous_attempt_unknown_blocked
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    name: tool_name.clone(),
                    content: r#"{"error":"PREVIOUS_ATTEMPT_UNKNOWN"}"#.to_string(),
                    latency_ms: 0,
                    success: false,
                };
            }

            // Otherwise, we either allow retry (if read-only) or we have no explicit idempotency key
            // (meaning it has a request-scoped key anyway). We reclaim and increment version.
            let reclaim_res = state
                .execution_store
                .create_or_claim(
                    ToolExecution {
                        attempt: initial_execution.attempt + 1,
                        ..initial_execution.clone()
                    },
                    lease_duration,
                )
                .await;

            if let Ok((_new_state, new_ctx)) = reclaim_res {
                claim_ctx = new_ctx;
            } else {
                return ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    name: tool_name.clone(),
                    content: r#"{"error":"RECLAIM_FAILED"}"#.to_string(),
                    latency_ms: 0,
                    success: false,
                };
            }
        }

        // 4. Mark running and execute tool call
        let version = claim_ctx.version;
        let _ = state
            .execution_store
            .mark_running(&idempotency_key, version)
            .await;

        let state_task = state.clone();
        let tool_call_task = tool_call; // transfer ownership of tool_call here
        let tenant_id_task = tenant_id.to_string();
        let idempotency_key_task = idempotency_key.clone();
        let test_scenario_task = trace_ctx.test_scenario.clone();

        // Spawn detached task so it survives client disconnects (supports late completion fencing)
        let exec_handle = tokio::spawn(async move {
            let res = state_task
                .tool_transport
                .execute_tool(
                    &tool_call_task.id,
                    &tool_call_task.name,
                    &tool_call_task.arguments,
                    &tenant_id_task,
                    enable_compression,
                    test_scenario_task.as_deref(),
                )
                .await;

            if res.success {
                let _ = state_task
                    .execution_store
                    .mark_succeeded(
                        &idempotency_key_task,
                        version,
                        res.content.clone(),
                        res.latency_ms,
                    )
                    .await;
            } else {
                let is_timeout = res.content.contains("MCP_TIMEOUT")
                    || res.content.contains("unreachable")
                    || res.content.contains("not configured")
                    || res.content.contains("timed out")
                    || res.content.contains("timeout");
                if is_timeout {
                    let _ = state_task
                        .execution_store
                        .mark_unknown(&idempotency_key_task, version)
                        .await;
                } else {
                    let _ = state_task
                        .execution_store
                        .mark_failed(&idempotency_key_task, version, res.content.clone())
                        .await;
                }
            }
            res
        });

        // Wait for execution with policy timeout
        let exec_timeout = std::time::Duration::from_secs(policy.timeout_seconds);
        let exec_res = tokio::time::timeout(exec_timeout, exec_handle).await;

        let result = match exec_res {
            Ok(Ok(result)) => result,
            Ok(Err(join_err)) => {
                let _ = state
                    .execution_store
                    .mark_unknown(&idempotency_key, version)
                    .await;
                ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    name: tool_name.clone(),
                    content: format!(
                        r#"{{"error":"MCP_TASK_FAILED","message":"{:?}"}}"#,
                        join_err
                    ),
                    latency_ms: start.elapsed().as_millis() as u64,
                    success: false,
                }
            }
            Err(_elapsed) => {
                let _ = state
                    .execution_store
                    .mark_unknown(&idempotency_key, version)
                    .await;
                ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    name: tool_name.clone(),
                    content: r#"{"error":"MCP_TIMEOUT"}"#.to_string(),
                    latency_ms: start.elapsed().as_millis() as u64,
                    success: false,
                }
            }
        };

        // 5. Emit Telemetry
        let latency_ms = start.elapsed().as_millis() as u64;
        let final_state = if result.success {
            ExecutionState::Succeeded
        } else {
            if result.content.contains("MCP_TIMEOUT")
                || result.content.contains("unreachable")
                || result.content.contains("not configured")
                || result.content.contains("timed out")
                || result.content.contains("timeout")
            {
                ExecutionState::Unknown
            } else {
                ExecutionState::Failed
            }
        };
        let telemetry_payload = serde_json::json!({
            "type": "tool_execution_event",
            "execution_id": exec_id.0,
            "operation_id": operation_id.0,
            "tenant_id": tenant_id,
            "workflow_id": wf_id.as_ref().map(|w| &w.0),
            "agent_id": ag_id.as_ref().map(|a| &a.0),
            "tool_name": tool_name,
            "attempt": initial_execution.attempt,
            "state": format!("{:?}", final_state),
            "idempotency_key_hash": idempotency_key.0,
            "policy": {
                "retry_policy": format!("{:?}", policy.retry_policy),
                "side_effect_class": format!("{:?}", policy.side_effect_class),
                "idempotency_requirement": format!("{:?}", policy.idempotency_requirement),
            },
            "started_at": initial_execution.created_at.to_rfc3339(),
            "completed_at": Utc::now().to_rfc3339(),
            "latency_ms": latency_ms,
            "error_category": if result.success { None } else { Some(if result.content.contains("TIMEOUT") { "timeout" } else { "execution_error" }) },
            "reconciliation_state": format!("{:?}", if final_state == ExecutionState::Unknown { ReconciliationResult::StillUnknown } else { ReconciliationResult::Succeeded { content: String::new() } }),
        });
        state.telemetry.log_event(telemetry_payload);

        result
    }
}

// ── Unit Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::execution::ExecutionContext;
    use crate::domain::models::RoutingState;
    use crate::domain::ports::ExecutionStore;
    use crate::infrastructure::l1_cache::L1Cache;
    use moka::future::Cache;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct TestTransport {
        call_count: Arc<AtomicUsize>,
        should_timeout: Arc<AtomicBool>,
        delay_ms: u64,
    }

    impl crate::domain::ports::ToolTransport for TestTransport {
        fn execute_tool<'a>(
            &'a self,
            tool_call_id: &'a str,
            tool_name: &'a str,
            _arguments: &'a str,
            _tenant_id: &'a str,
            _enable_compression: bool,
            _test_scenario: Option<&'a str>,
        ) -> Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let should_timeout = self.should_timeout.load(Ordering::SeqCst);
            let delay = self.delay_ms;
            let name = tool_name.to_string();
            let cid = tool_call_id.to_string();
            Box::pin(async move {
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                if should_timeout {
                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: "error: MCP_TIMEOUT".to_string(),
                        latency_ms: delay,
                        success: false,
                    }
                } else {
                    ToolResult {
                        tool_call_id: cid,
                        name,
                        content: r#"{"status":"ok"}"#.to_string(),
                        latency_ms: delay,
                        success: true,
                    }
                }
            })
        }
    }

    struct TestReconciler {
        result: std::sync::Mutex<ReconciliationResult>,
    }

    impl crate::domain::ports::Reconciler for TestReconciler {
        fn reconcile<'a>(
            &'a self,
            _operation: &'a ToolExecution,
        ) -> Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<ReconciliationResult, crate::error::GatewayError>,
                    > + Send
                    + 'a,
            >,
        > {
            let r = self.result.lock().unwrap().clone();
            Box::pin(async move { Ok(r) })
        }
    }

    fn setup_test_context(
        transport_call_count: Arc<AtomicUsize>,
        transport_should_timeout: Arc<AtomicBool>,
        transport_delay_ms: u64,
        reconciliation_result: ReconciliationResult,
    ) -> Arc<AppState> {
        let operation_cache = Cache::builder().max_capacity(100).build();

        let state = AppState {
            http_client: reqwest::Client::new(),
            compliance_url: String::new(),
            rate_limit_max: 60,
            rate_limit_window: 60,
            dashboard_url: String::new(),
            llm_api_base_url: None,
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
                    operation_cache.clone(),
                ),
            ),
            reconciler: Arc::new(TestReconciler {
                result: std::sync::Mutex::new(reconciliation_result),
            }),
            tool_transport: Arc::new(TestTransport {
                call_count: transport_call_count,
                should_timeout: transport_should_timeout,
                delay_ms: transport_delay_ms,
            }),
            rate_limit_cache: Arc::new(dashmap::DashMap::new()),
            l1_cache: Arc::new(L1Cache::new(1024).unwrap()),
            routing_state: Arc::new(RoutingState::new()),
            circuit_breaker: moka::future::Cache::builder().build(),
            loop_fallback_cache: moka::future::Cache::builder().build(),
            mcp_registry: crate::infrastructure::mcp_registry::McpConnectionRegistry::empty(),
            tool_registry: crate::usecases::tool_router::ToolRegistry::empty(),
            agent_guardian_cache: moka::future::Cache::builder().build(),
            operation_cache,
            dashboard_metrics: Arc::new(crate::domain::models::DashboardMetrics::new()),
            pricing_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
            trace_store: Arc::new(dashmap::DashMap::new()),
            budget_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
        };

        Arc::new(state)
    }

    // A. Same business operation
    #[tokio::test]
    async fn test_same_business_operation_deduplication() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let should_timeout = Arc::new(AtomicBool::new(false));
        let state = setup_test_context(
            call_count.clone(),
            should_timeout,
            0,
            ReconciliationResult::StillUnknown,
        );

        let trace_ctx = TraceContext {
            trace_id: "t1".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec1".to_string()),
            operation_id: None,
            idempotency_key: Some("idem_charge_100".to_string()),
            test_scenario: None,
        };

        let tc = ToolCall {
            id: "call_123".to_string(),
            name: "charge_card".to_string(),
            arguments: r#"{"amount":100}"#.to_string(),
        };

        // First attempt
        let res1 =
            ExecutionService::execute_tool(&state, tc.clone(), "tenant1", &trace_ctx, 0, 1, false)
                .await;
        assert!(res1.success);
        assert_eq!(res1.content, r#"{"status":"ok"}"#);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Second attempt
        let res2 =
            ExecutionService::execute_tool(&state, tc.clone(), "tenant1", &trace_ctx, 0, 1, false)
                .await;
        assert!(res2.success);
        assert_eq!(res2.content, r#"{"status":"ok"}"#);
        assert_eq!(call_count.load(Ordering::SeqCst), 1); // execution count stays 1!
    }

    // B. Same arguments, different business operation
    #[tokio::test]
    async fn test_same_arguments_different_operation() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let should_timeout = Arc::new(AtomicBool::new(false));
        let state = setup_test_context(
            call_count.clone(),
            should_timeout,
            0,
            ReconciliationResult::StillUnknown,
        );

        let tc = ToolCall {
            id: "call_123".to_string(),
            name: "charge_card".to_string(),
            arguments: r#"{"amount":100}"#.to_string(),
        };

        // Operation A
        let trace_ctx_a = TraceContext {
            trace_id: "t1".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec1".to_string()),
            operation_id: None,
            idempotency_key: Some("idem_a".to_string()),
            test_scenario: None,
        };
        let res_a = ExecutionService::execute_tool(
            &state,
            tc.clone(),
            "tenant1",
            &trace_ctx_a,
            0,
            1,
            false,
        )
        .await;
        assert!(res_a.success);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Operation B (same args, different idem key)
        let trace_ctx_b = TraceContext {
            trace_id: "t2".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec1".to_string()),
            operation_id: None,
            idempotency_key: Some("idem_b".to_string()),
            test_scenario: None,
        };
        let res_b = ExecutionService::execute_tool(
            &state,
            tc.clone(),
            "tenant1",
            &trace_ctx_b,
            0,
            1,
            false,
        )
        .await;
        assert!(res_b.success);
        assert_eq!(call_count.load(Ordering::SeqCst), 2); // Executed independently!
    }

    // C. Concurrent duplicates
    #[tokio::test]
    async fn test_concurrent_duplicates() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let should_timeout = Arc::new(AtomicBool::new(false));
        // Add a slight delay to allow overlap
        let state = setup_test_context(
            call_count.clone(),
            should_timeout,
            50,
            ReconciliationResult::StillUnknown,
        );

        let trace_ctx = TraceContext {
            trace_id: "t1".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec1".to_string()),
            operation_id: None,
            idempotency_key: Some("idem_concurrent".to_string()),
            test_scenario: None,
        };

        let tc = ToolCall {
            id: "call_123".to_string(),
            name: "charge_card".to_string(),
            arguments: r#"{"amount":100}"#.to_string(),
        };

        let state_c1 = state.clone();
        let state_c2 = state.clone();
        let tc_c1 = tc.clone();
        let tc_c2 = tc.clone();
        let trace_c1 = trace_ctx.clone();
        let trace_c2 = trace_ctx.clone();

        let h1 = tokio::spawn(async move {
            ExecutionService::execute_tool(&state_c1, tc_c1, "tenant1", &trace_c1, 0, 1, false)
                .await
        });
        let h2 = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            ExecutionService::execute_tool(&state_c2, tc_c2, "tenant1", &trace_c2, 0, 1, false)
                .await
        });

        let r1 = h1.await.unwrap();
        let r2 = h2.await.unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 1); // Only 1 downstream execution!
        assert!(r1.success || r2.success);
    }

    // D. Timeout ambiguity
    #[tokio::test]
    async fn test_timeout_ambiguity() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let should_timeout = Arc::new(AtomicBool::new(true)); // simulates timeout
        let state = setup_test_context(
            call_count.clone(),
            should_timeout,
            0,
            ReconciliationResult::StillUnknown,
        );

        let trace_ctx = TraceContext {
            trace_id: "t1".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec1".to_string()),
            operation_id: None,
            idempotency_key: Some("idem_timeout".to_string()),
            test_scenario: None,
        };

        let tc = ToolCall {
            id: "call_123".to_string(),
            name: "charge_card".to_string(),
            arguments: r#"{"amount":100}"#.to_string(),
        };

        let res =
            ExecutionService::execute_tool(&state, tc, "tenant1", &trace_ctx, 0, 1, false).await;
        assert!(!res.success);

        // State must become Unknown
        let op_key = if let Some(ref key) = trace_ctx.idempotency_key {
            let mut hasher = Sha256::new();
            hasher.update("tenant1".as_bytes());
            hasher.update(b"::");
            hasher.update(key.as_bytes());
            hasher.update(b"::0::charge_card::{\"amount\":100}");
            IdempotencyKey(hex::encode(hasher.finalize()))
        } else {
            unreachable!()
        };

        let stored = state.execution_store.get(&op_key).await.unwrap().unwrap();
        assert_eq!(stored.0, ExecutionState::Unknown);
    }

    // E. Unsafe retry
    #[tokio::test]
    async fn test_unsafe_retry_blocked() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let should_timeout = Arc::new(AtomicBool::new(false));
        let state = setup_test_context(
            call_count.clone(),
            should_timeout,
            0,
            ReconciliationResult::StillUnknown,
        );

        let trace_ctx = TraceContext {
            trace_id: "t1".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec1".to_string()),
            operation_id: None,
            idempotency_key: Some("idem_unsafe".to_string()),
            test_scenario: None,
        };

        let tc = ToolCall {
            id: "call_123".to_string(),
            name: "charge_card".to_string(),
            arguments: r#"{"amount":100}"#.to_string(),
        };

        let op_key = if let Some(ref key) = trace_ctx.idempotency_key {
            let mut hasher = Sha256::new();
            hasher.update("tenant1".as_bytes());
            hasher.update(b"::");
            hasher.update(key.as_bytes());
            hasher.update(b"::0::charge_card::{\"amount\":100}");
            IdempotencyKey(hex::encode(hasher.finalize()))
        } else {
            unreachable!()
        };

        // Force cache into Unknown state
        let exec = ToolExecution {
            execution_id: ExecutionId("exec1".to_string()),
            operation_id: OperationId("op1".to_string()),
            workflow_id: None,
            agent_id: None,
            tenant_id: TenantId("tenant1".to_string()),
            session_id: Some(SessionId("s1".to_string())),
            tool_name: "charge_card".to_string(),
            arguments_hash: op_key.0.clone(),
            idempotency_key: op_key.clone(),
            attempt: 1,
            state: ExecutionState::Unknown,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let ctx = ExecutionContext {
            result_content: None,
            latency_ms: None,
            error_message: Some("timeout".to_string()),
            lease_until: None,
            version: 1,
        };
        state
            .operation_cache
            .insert(
                op_key.0,
                crate::domain::models::OperationCacheEntry {
                    state: ExecutionState::Unknown,
                    context: ctx,
                    execution: exec,
                },
            )
            .await;

        // Try executing retry - must be blocked
        let res =
            ExecutionService::execute_tool(&state, tc, "tenant1", &trace_ctx, 0, 1, false).await;
        assert!(!res.success);
        assert_eq!(res.content, r#"{"error":"PREVIOUS_ATTEMPT_UNKNOWN"}"#);
    }

    // F. Reconciliation
    #[tokio::test]
    async fn test_reconciliation_resolves_unknown() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let should_timeout = Arc::new(AtomicBool::new(false));
        // Reconciliation result is Succeeded
        let state = setup_test_context(
            call_count.clone(),
            should_timeout,
            0,
            ReconciliationResult::Succeeded {
                content: r#"{"status":"recovered"}"#.to_string(),
            },
        );

        let trace_ctx = TraceContext {
            trace_id: "t1".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec1".to_string()),
            operation_id: None,
            idempotency_key: Some("idem_reconciliation".to_string()),
            test_scenario: None,
        };

        let tc = ToolCall {
            id: "call_123".to_string(),
            name: "charge_card".to_string(),
            arguments: r#"{"amount":100}"#.to_string(),
        };

        let op_key = if let Some(ref key) = trace_ctx.idempotency_key {
            let mut hasher = Sha256::new();
            hasher.update("tenant1".as_bytes());
            hasher.update(b"::");
            hasher.update(key.as_bytes());
            hasher.update(b"::0::charge_card::{\"amount\":100}");
            IdempotencyKey(hex::encode(hasher.finalize()))
        } else {
            unreachable!()
        };

        // Force cache into Unknown state
        let exec = ToolExecution {
            execution_id: ExecutionId("exec1".to_string()),
            operation_id: OperationId("op1".to_string()),
            workflow_id: None,
            agent_id: None,
            tenant_id: TenantId("tenant1".to_string()),
            session_id: Some(SessionId("s1".to_string())),
            tool_name: "charge_card".to_string(),
            arguments_hash: op_key.0.clone(),
            idempotency_key: op_key.clone(),
            attempt: 1,
            state: ExecutionState::Unknown,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let ctx = ExecutionContext {
            result_content: None,
            latency_ms: None,
            error_message: Some("timeout".to_string()),
            lease_until: None,
            version: 1,
        };
        state
            .operation_cache
            .insert(
                op_key.0.clone(),
                crate::domain::models::OperationCacheEntry {
                    state: ExecutionState::Unknown,
                    context: ctx,
                    execution: exec,
                },
            )
            .await;

        // Try executing retry - should trigger reconciliation and succeed
        let res =
            ExecutionService::execute_tool(&state, tc, "tenant1", &trace_ctx, 0, 1, false).await;
        assert!(res.success);
        assert_eq!(res.content, r#"{"status":"recovered"}"#);
        assert_eq!(call_count.load(Ordering::SeqCst), 0); // No downstream replay!

        // State must become Succeeded
        let stored = state.execution_store.get(&op_key).await.unwrap().unwrap();
        assert_eq!(stored.0, ExecutionState::Succeeded);
    }

    // G. Late completion
    #[tokio::test]
    async fn test_late_completion_fencing() {
        let operation_cache = Cache::builder().max_capacity(100).build();

        let store = Arc::new(
            crate::infrastructure::oss_adapters::MokaExecutionStore::new(operation_cache.clone()),
        );

        let op_key = IdempotencyKey("idem_late".to_string());

        // Setup state at version 2 (simulating that a new attempt/claim was made after a timeout)
        let exec = ToolExecution {
            execution_id: ExecutionId("exec1".to_string()),
            operation_id: OperationId("op1".to_string()),
            workflow_id: None,
            agent_id: None,
            tenant_id: TenantId("tenant1".to_string()),
            session_id: Some(SessionId("s1".to_string())),
            tool_name: "charge_card".to_string(),
            arguments_hash: op_key.0.clone(),
            idempotency_key: op_key.clone(),
            attempt: 2,
            state: ExecutionState::Claimed,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let ctx = ExecutionContext {
            result_content: None,
            latency_ms: None,
            error_message: Some("timeout".to_string()),
            lease_until: None,
            version: 2,
        };
        operation_cache
            .insert(
                op_key.0.clone(),
                crate::domain::models::OperationCacheEntry {
                    state: ExecutionState::Claimed,
                    context: ctx,
                    execution: exec,
                },
            )
            .await;

        // Simulate version 1 task completing late - it must not overwrite the cache
        let mark_res = store
            .mark_succeeded(&op_key, 1, r#"{"status":"late"}"#.to_string(), 120)
            .await;
        assert!(mark_res.is_ok());

        // Verify the cache remains in version 2 and Claimed state
        let stored = store.get(&op_key).await.unwrap().unwrap();
        assert_eq!(stored.0, ExecutionState::Claimed);
        assert_eq!(stored.1.version, 2);
        assert!(stored.1.result_content.is_none());
    }

    // H. tool_call_id distinction
    #[tokio::test]
    async fn test_tool_call_id_distinction() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let should_timeout = Arc::new(AtomicBool::new(false));
        let state = setup_test_context(
            call_count.clone(),
            should_timeout,
            0,
            ReconciliationResult::StillUnknown,
        );

        let trace_ctx = TraceContext {
            trace_id: "t1".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec1".to_string()),
            operation_id: None,
            idempotency_key: None, // absent key means request-scoped
            test_scenario: None,
        };

        // Identical tools and args but different tool_call_id
        let tc1 = ToolCall {
            id: "call_abc".to_string(),
            name: "send_email".to_string(),
            arguments: r#"{"to":"test"}"#.to_string(),
        };
        let tc2 = ToolCall {
            id: "call_def".to_string(),
            name: "send_email".to_string(),
            arguments: r#"{"to":"test"}"#.to_string(),
        };

        let res1 =
            ExecutionService::execute_tool(&state, tc1, "tenant1", &trace_ctx, 0, 2, false).await;
        let res2 =
            ExecutionService::execute_tool(&state, tc2, "tenant1", &trace_ctx, 1, 2, false).await;

        assert!(res1.success);
        assert!(res2.success);
        assert_eq!(call_count.load(Ordering::SeqCst), 2); // executed independently!
    }

    // I. same idempotency key, different tool
    #[tokio::test]
    async fn test_same_idempotency_key_different_tool() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let should_timeout = Arc::new(AtomicBool::new(false));
        let state = setup_test_context(
            call_count.clone(),
            should_timeout,
            0,
            ReconciliationResult::StillUnknown,
        );

        let trace_ctx = TraceContext {
            trace_id: "t1".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec1".to_string()),
            operation_id: None,
            idempotency_key: Some("idem_shared".to_string()),
            test_scenario: None,
        };

        let tc1 = ToolCall {
            id: "call_1".to_string(),
            name: "refund".to_string(),
            arguments: r#"{"id":1}"#.to_string(),
        };
        let tc2 = ToolCall {
            id: "call_2".to_string(),
            name: "send_email".to_string(),
            arguments: r#"{"id":1}"#.to_string(),
        };

        let res1 =
            ExecutionService::execute_tool(&state, tc1, "tenant1", &trace_ctx, 0, 2, false).await;
        let res2 =
            ExecutionService::execute_tool(&state, tc2, "tenant1", &trace_ctx, 1, 2, false).await;

        assert!(res1.success);
        assert!(res2.success);
        assert_eq!(call_count.load(Ordering::SeqCst), 2); // No collision!
    }

    // J. TTL semantics
    #[tokio::test]
    async fn test_ttl_expiry_allows_re_execution() {
        let operation_cache = Cache::builder()
            .max_capacity(100)
            .time_to_live(std::time::Duration::from_millis(50)) // tiny TTL
            .build();

        let call_count = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(AppState {
            http_client: reqwest::Client::new(),
            compliance_url: String::new(),
            rate_limit_max: 60,
            rate_limit_window: 60,
            dashboard_url: String::new(),
            llm_api_base_url: None,
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
                    operation_cache.clone(),
                ),
            ),
            reconciler: Arc::new(crate::infrastructure::oss_adapters::OssReconciler),
            tool_transport: Arc::new(TestTransport {
                call_count: call_count.clone(),
                should_timeout: Arc::new(AtomicBool::new(false)),
                delay_ms: 0,
            }),
            rate_limit_cache: Arc::new(dashmap::DashMap::new()),
            l1_cache: Arc::new(L1Cache::new(1024).unwrap()),
            routing_state: Arc::new(RoutingState::new()),
            circuit_breaker: moka::future::Cache::builder().build(),
            loop_fallback_cache: moka::future::Cache::builder().build(),
            mcp_registry: crate::infrastructure::mcp_registry::McpConnectionRegistry::empty(),
            tool_registry: crate::usecases::tool_router::ToolRegistry::empty(),
            agent_guardian_cache: moka::future::Cache::builder().build(),
            operation_cache,
            dashboard_metrics: Arc::new(crate::domain::models::DashboardMetrics::new()),
            pricing_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
            trace_store: Arc::new(dashmap::DashMap::new()),
            budget_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
        });

        let trace_ctx = TraceContext {
            trace_id: "t1".to_string(),
            session_id: "s1".to_string(),
            parent_trace_id: None,
            workflow_id: None,
            agent_id: None,
            execution_id: Some("exec1".to_string()),
            operation_id: None,
            idempotency_key: Some("idem_ttl".to_string()),
            test_scenario: None,
        };

        let tc = ToolCall {
            id: "call_123".to_string(),
            name: "charge_card".to_string(),
            arguments: r#"{"amount":100}"#.to_string(),
        };

        let res1 =
            ExecutionService::execute_tool(&state, tc.clone(), "tenant1", &trace_ctx, 0, 1, false)
                .await;
        assert!(res1.success);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Sleep to let entry expire in Cache
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let res2 =
            ExecutionService::execute_tool(&state, tc.clone(), "tenant1", &trace_ctx, 0, 1, false)
                .await;
        assert!(res2.success);
        assert_eq!(call_count.load(Ordering::SeqCst), 2); // Run again because cache entry expired!
    }
}
