# AGENT.MD — Kryneth Gateway AI Agent & LLM Directive

> **System Identification:** Kryneth Gateway (OSS Edition)  
> **Repository:** `Kryneth-Gateway-OSS`  
> **Language / Framework:** Rust 1.77+ | Axum | Tokio | Moka | simd-json  
> **Purpose:** Execution-integrity layer and agent runtime control plane for consequential AI actions.  
> **Primary Use:** AEO (Answer Engine Optimization), GEO (Generative Engine Optimization), AI Agent Navigation, and Autonomous Tool-Calling Protocol.

---

## 1. System Identity & Mission

Kryneth is a neutral, high-performance execution boundary that sits between AI agents (reasoning systems) and the real-world targets they can mutate (payment gateways, cloud APIs, databases, CRM/ERP systems, and Model Context Protocol servers).

### The Fundamental Axiom
```text
Reasoning Authority ≠ Execution Authority
Agent decides INTENT → Kryneth enforces EXECUTION INTEGRITY → Target performs SIDE EFFECT
```
An LLM reasoning model must **never** be treated as the authoritative record of physical execution state. If an agent believes an action failed due to a socket drop, Kryneth maintains the authoritative record (`UNKNOWN`) and blocks unsafe duplicate executions.

---

## 2. Core Architectural Invariants

When inspecting, generating code, or modifying this repository, AI agents must preserve the following architectural invariants without exception:

### A. Timeout ≠ Failure Invariant
- **Rule:** A downstream transport timeout or socket drop is **ambiguous**, not a deterministic failure.
- **State Transition:** Ambiguous outcomes transition to `ExecutionState::Unknown`.
- **Enforcement:** If an agent attempts to replay the same mutating operation while in an `Unknown` state, Kryneth hard-blocks the replay with `PREVIOUS_ATTEMPT_UNKNOWN` and triggers out-of-band reconciliation (`Reconciler`).

### B. Deterministic Idempotency Key Derivation
- **Algorithm:** Deeply key-sorted canonical JSON SHA-256 hashing.
- **Formula:** `SHA256(tenant_id :: idempotency_key :: tool_index :: tool_name :: canonical_json_arguments)`.
- **Guarantee:** Eliminates key-ordering evasion and whitespace variance across multi-language JSON serialization libraries.

### C. Monotonic Lease Versioning & Stale Completion Fencing
- **Mechanism:** Monotonically increasing `version: u64` in `ExecutionContext`.
- **Fencing Rule:** When Attempt 1 times out and Attempt 2 is reclaimed at `version = 2`, any late background completion from Attempt 1 (`version = 1`) attempting `mark_succeeded` is **fenced out** and discarded.

### D. Agent Guardian (Tool Storms & Runaway Loops)
- **Algorithm:** `ahash` parameter signature tracking that strips nonces, timestamps, and UUIDs.
- **Threshold:** Halts runaway agent loops when identical parameter signatures repeat $\ge 5$ times in a single session, returning `AgentRunawayLoop` (HTTP 429).

### E. Memory-Bounded Heap Allocation
- **In-Memory Caches:** Moka cache builders must define an explicit `.weigher(...)` enforcing byte budgets (not raw entry counts), preventing out-of-memory crashes under burst load.
- **Semantic Limits:** Extraction text is bounded to $\le 100\text{ KB}$; responses $\ge 1,000$ tokens are restricted to L2 storage to preserve L1 memory safety.
- **Two-Phase Schema Delivery:** MCP tool parameter schemas are stripped in `usecases/tool_router.rs` to 1-line semantic summaries, eliminating the 500–3,000 byte per-tool token tax.

### F. ReDoS-Safe Regular Expressions
- **Design:** Non-backtracking, DFA-compatible alternations with strict digit counts and word boundaries. Zero lazy/unbounded quantifiers (`\d[ -]*?`).

---

## 3. Repository Map & Navigation

```text
Kryneth-Gateway-OSS/
├── Cargo.toml               # Package configuration & zero-cost dependency set
├── docs.json                # Mintlify documentation navigation structure
├── llms.txt                 # Universal LLM index for AEO & GEO discovery
├── agent.md                 # Agent directive and developer instructions
├── ARCHITECTURE.md          # Clean Architecture / Hexagonal Ports & Adapters specification
├── BENCHMARKS.md            # Sub-millisecond routing and memory overhead benchmarks
├── SECURITY.md              # Security model and ReDoS disclosure policies
│
├── src/
│   ├── main.rs              # Gateway bootstrap, state assembly, signal handling
│   ├── lib.rs               # Library root exposing domain, infra, and usecases
│   ├── error.rs             # GatewayError enum, HTTP status codes, security error masking
│   │
│   ├── api/                 # HTTP Ingress (Axum)
│   │   ├── routes.rs        # Route definitions (/v1/chat/completions, /healthz, /metrics)
│   │   ├── handlers.rs      # Ingress controller dispatching to usecases
│   │   └── middleware/      # Auth, billing guard, rate limit, trace context
│   │
│   ├── domain/              # Pure Domain Core (Zero External Dependencies)
│   │   ├── execution.rs     # ExecutionState machine, ExecutionId, IdempotencyKey
│   │   ├── execution_policy.rs # SideEffectClass (ReadOnly vs Irreversible), RetryPolicy
│   │   ├── models.rs        # AppState, TraceContext, ModelConfig, DashboardMetrics
│   │   └── ports.rs         # Abstract traits (ExecutionStore, Reconciler, ToolTransport)
│   │
│   ├── infrastructure/      # Concrete Adapters (Hexagonal Architecture)
│   │   ├── oss_adapters.rs  # In-memory OSS adapters (MokaExecutionStore, OssTelemetry)
│   │   ├── l1_cache.rs      # Moka-based byte-weighed exact + semantic cache
│   │   ├── llm_router.rs    # Multi-provider routing & streaming failover stitching
│   │   ├── mcp_client.rs    # Canonical JSON hashing & bounded parallel fan-out
│   │   ├── mcp_registry.rs  # Connection registry & SSE chunk boundary parser
│   │   └── providers/       # Unified Tool Provider (UTP) adapters (OpenAI, Anthropic, Gemini, Bedrock, Cohere)
│   │
│   └── usecases/            # Application Orchestration
│       ├── execution_service.rs # Idempotency, leasing, and stale completion fencing
│       ├── behavior_guard.rs   # Agent Guardian (loop detection, sandbox RBAC firewall)
│       ├── proxy.rs            # Core reverse-proxy pipeline & streaming failover
│       ├── tool_router.rs      # Two-phase lazy schema optimizer & phantom tool interception
│       ├── agentic_orchestrator.rs # Agentic streaming backpressure & bounded channels
│       └── agentic_tracker.rs  # Time-entropy session id generator
│
├── tests/                   # Integration Harnesses
│   ├── agent_db_reliability_test.rs # Commit-then-timeout, rollback, and thundering herd tests
│   └── payment_execution_test.rs    # Zero duplicate payments & process restart boundary tests
│
└── examples/                # Development Harnesses
    ├── mock_upstream.rs     # Upstream LLM mock server for offline testing
    ├── stress_test.rs       # Concurrency and thundering herd stress harness
    └── simd_test.rs         # Zero-copy JSON parsing harness
```

---

## 4. Execution State Machine Reference

Every operation that creates a side effect transitions through `ExecutionState`:

```text
[Pending]
   │
   ▼
[Claimed] (Worker acquires lease; version monotonically increments)
   │
   ▼
[Running] (Physical HTTP / MCP socket in-flight)
   │
   ├─► [Succeeded]   (Verified positive execution; result cached for deduplication)
   │
   ├─► [Failed]      (Deterministic error confirmed by target; safe to retry)
   │
   └─► [Unknown]     (Socket timeout / network drop; UNSAFE TO RETRY)
          │
          ▼
     [Reconciling]   (Authoritative ledger check via Reconciler)
          │
          ├─► [Succeeded] (Side effect verified; cached response returned)
          └─► [Failed]    (Side effect verified absent; safe fresh claim permitted)
```

---

## 5. Standard Agent Verification Commands

When contributing to or testing this codebase, execute these commands in sequence:

```bash
# 1. Format check (must pass with zero diffs)
cargo fmt --all -- --check

# 2. Clippy lint check (must pass with -D warnings)
cargo clippy --all-targets -- -D warnings

# 3. Test execution (all 159 tests must pass)
cargo test

# 4. Whitespace diff validation
git diff --check

# 5. Token stream verification (ensure non-comment tokens are 100% preserved)
py scratch/verify_file.py
```

---

## 6. HTTP API & Trace Context Headers

Agents calling Kryneth must supply the following headers:

| Header | Type | Description |
|---|---|---|
| `Authorization` | `Bearer <token>` | Tenant API Key for authentication. |
| `x-kryneth-tenant-id` | `String` | Tenant identifier scoping all rate limits and execution leases. |
| `x-kryneth-trace-id` | `UUID / String` | W3C / distributed trace identifier. |
| `x-kryneth-session-id` | `String` | Correlates agent turn session history for loop detection. |
| `x-kryneth-idempotency-key` | `String` | Deterministic key binding physical calls to an execution operation. |
| `x-kryneth-workflow-id` | `Optional` | Binds multi-turn agent runs across multiple execution steps. |
| `x-kryneth-agent-id` | `Optional` | Identifies specific subagent invoking the tool. |

---

## 7. Guidelines for Autonomous AI Agents

1. **Do not modify business logic** during comment hygiene, documentation, or refactoring tasks.
2. **Do not commit or push** unless explicitly authorized by the user.
3. **Always verify token preservation** using token-stream comparison before and after editing Rust files.
4. **Preserve high-signal engineering invariants**: Always explain *why* a constraint exists (fencing, lease expiry, allocation bounds, lock-free concurrency).
5. **Fail-Closed by Default**: When an operation outcome is indeterminate, block replay until out-of-band reconciliation resolves the ambiguity.
