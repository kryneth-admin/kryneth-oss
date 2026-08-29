use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExecutionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttemptId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkflowId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdempotencyKey(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionState {
    Pending,
    Claimed,
    Running,
    Succeeded,
    Failed,
    Unknown,
    Reconciling,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolExecution {
    pub execution_id: ExecutionId,
    pub operation_id: OperationId,
    pub workflow_id: Option<WorkflowId>,
    pub agent_id: Option<AgentId>,
    pub tenant_id: TenantId,
    pub session_id: Option<SessionId>,

    pub tool_name: String,
    pub arguments_hash: String,
    pub idempotency_key: IdempotencyKey,

    pub attempt: u32,
    pub state: ExecutionState,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub result_content: Option<String>,
    pub latency_ms: Option<u64>,
    pub error_message: Option<String>,
    #[serde(skip)]
    pub lease_until: Option<std::time::Instant>,
    pub version: u64,
}
