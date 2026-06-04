# Kryneth Gateway Architecture

## System Overview
Kryneth is an ultra-low latency L7 runtime control plane for autonomous AI agents. It sits between multi-turn agentic frameworks and LLM providers to provide real-time safety guardrails and monitoring.

## Architecture Layers

### 1. API Gateway (Axum)
- HTTP request handling with Tokio async runtime
- Multi-tenant request routing
- Request/response transformation
- Built on Axum web framework for performance

### 2. Authentication & Authorization
- Tenant ID validation from request context
- API key resolution from routing configuration
- Per-tenant rate limiting enforcement

### 3. Tool Extraction Engine
- **Universal Tool Call Parser**: Normalizes tool formats across providers
- Supports: OpenAI, Anthropic, Google Gemini, Cohere formats
- SIMD-accelerated JSON parsing via `simd-json`
- Returns standardized `UnifiedToolCall` structure

### 4. Safety Guards
- **Infinite Loop Detector**: Tracks tool signatures over 60s window
- **Tool Storm Guard**: Enforces max tool executions per session
- **Circuit Breaker**: Auto-failover on provider failures
- Memory-optimized via Moka cache and DashMap

### 5. Routing Engine
- Multi-target provider failover
- Priority-based routing configuration
- Schema format translation per provider
- Automatic retry on transient failures

### 6. L1 Cache (Lock-Free)
- In-memory state management (Moka + DashMap)
- Session tracking
- Tool signature cache
- Zero external dependencies (no Redis required for OSS)

## Core Components

### UnifiedToolCall
Normalized representation of tool calls across all providers:
```rust
pub struct UnifiedToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
    pub signature_hash: u64,  // For loop detection
    pub timestamp: u64,
}
```

### Infinite Loop Detector
```
Input: Tool call stream
↓
Signature Extraction (Name + Arguments)
↓
SIMD Hash Computation (ahash)
↓
60s Window Lookup
↓
Comparison against MAX_IDENTICAL_TOOL_CALLS threshold
↓
Action: Allow or Block
```

**Time Complexity:** O(1) lookup with hardware-accelerated hashing
**Space:** O(n) where n = calls in 60s window

### Tool Storm Guard
```
Input: Tool call request
↓
Session lookup (DashMap)
↓
Count check against MAX_SESSION_TOOL_CALLS
↓
Update counter
↓
Action: Allow or Reject (429)
```

**Enforcement:** Hard ceiling, instant kill on violation

### Circuit Breaker
Monitors provider health:
```
Request sent to Provider A
↓
Response received
↓
Status Check: 401, 503, 429?
↓
Yes → Open circuit, failover to Provider B (0.37ms)
No → Return response
```

**Failover Chain:** Defined by priority in `routing.yaml`

## Module Structure

```
src/
├── main.rs              # Application entry point
├── lib.rs               # Library exports
├── error.rs             # Error types and handling
│
├── api/                 # HTTP layer
│   ├── handlers.rs      # Request handlers
│   ├── routes.rs        # Route definitions
│   ├── middleware/      # Middleware stack
│   │   ├── auth.rs      # Tenant authentication
│   │   ├── rate_limit.rs # Rate limiting
│   │   ├── billing_guard.rs # Billing enforcement
│   │   └── trace_context.rs # Request tracing
│
├── domain/              # Business logic
│   ├── models.rs        # Core data structures
│   ├── ports.rs         # Interface definitions
│   ├── billing.rs       # Billing logic
│   └── compliance.rs    # Compliance rules
│
├── infrastructure/      # External integrations
│   ├── routing_strategy.rs # Provider routing
│   ├── cache_client.rs     # L1 cache operations
│   ├── llm_router.rs       # Provider routing logic
│   ├── mcp_client.rs       # MCP client integration
│   ├── mcp_registry.rs     # MCP server registry
│   ├── enterprise_adapters.rs # Enterprise features
│   ├── clickhouse_logger.rs   # Analytics logging
│   ├── clickhouse_repo.rs     # Data persistence
│   ├── redis_sync.rs         # Enterprise Redis
│   └── schema_mapper.rs       # Format translation
│
└── usecases/            # Orchestration
    ├── proxy.rs         # Request proxy logic
    ├── tool_router.rs   # Tool call routing
    ├── billing_engine.rs # Billing calculations
    ├── agentic_orchestrator.rs # Agent control flow
    ├── agentic_tracker.rs      # Session tracking
    ├── behavior_guard.rs       # Safety enforcement
    └── metrics_usecase.rs      # Metrics collection
```

## Data Flow

### Request Processing
```
1. HTTP POST /v1/chat/completions
   ↓
2. Middleware:
   - Auth validation
   - Rate limit check
   - Request tracing
   ↓
3. Handler:
   - Parse request body
   - Extract tenant ID
   ↓
4. Tool Extraction:
   - Detect provider format
   - Parse tool calls
   - Normalize to UnifiedToolCall
   ↓
5. Safety Checks:
   - Infinite loop detection
   - Tool storm enforcement
   - Behavior guards
   ↓
6. Routing:
   - Select provider from routing.yaml
   - Resolve API key from environment
   - Transform to provider format
   ↓
7. Provider Call:
   - Send to upstream LLM
   - Monitor response
   - Handle failures with failover
   ↓
8. Response Processing:
   - Parse provider response
   - Cache if configured
   - Add tracing metadata
   ↓
9. Return to Client
```

## Performance Characteristics

### End-to-End Latency
| Percentile | Latency | Components |
|-----------|---------|-----------|
| P50 | 1.4ms | Auth + parsing + cache lookup |
| P90 | 2.1ms | + SIMD-JSON extraction |
| P95 | 3.0ms | + Tool signature hashing |
| P99 | 4.5ms | + Large payload handling |
| Failover | 0.37ms | Circuit breaker activation |

### Throughput
- Single instance: ~5,000 req/s (P99 < 4.5ms)
- Scales linearly with Tokio worker threads
- No memory leaks with lock-free designs

## State Management

### OSS Edition
- **L1 Cache:** Moka (in-memory, lock-free)
- **Concurrent Map:** DashMap (sharded hash map)
- **Data:** Session state, tool signatures, rate limits
- **TTL:** Configurable, default 60s for loops

### Enterprise Edition
- **L1 Cache:** Same as OSS
- **L2 Store:** Centralized Redis cluster
- **Data:** Same + billing aggregates, compliance logs
- **Consistency:** Eventually consistent across nodes

## Configuration Sources

### Runtime Configuration
- Environment variables (highest priority)
- `routing.yaml` file
- Defaults in code

### Feature Flags
- `--no-default-features` for minimal OSS build
- `--features enterprise` for full capabilities

## Security Considerations

1. **Memory Safety:** Rust + tokio = no buffer overflows, thread-safe by default
2. **API Key Management:** Keys resolved from environment, never logged
3. **Request Validation:** All inputs validated before processing
4. **Rate Limiting:** Per-tenant, per-window enforcement
5. **Circuit Breaker:** Prevents cascading failures to upstream

## Deployment Targets
- Local development (cargo run)
- Docker containers
- Kubernetes (Enterprise)
- Serverless (AWS Lambda, Azure Functions)
