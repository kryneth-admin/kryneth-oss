---
icon: "shield-alert"
---
# Agentic Firewall (Behavior Guard)

To prevent autonomous agents from running out of bounds, repeating failing actions, or triggering costly "tool storms," Kryneth acts as a behavior-aware firewall. It actively inspects outgoing tool executions and request velocities in real time.

---

## 1. Runaway Loop Interception & Ephemeral Evasion Trap

If an agent encounters a parsing error, database timeout, or unhandled exception, it may enter an infinite loop, repeating identical tool execution parameters and draining your API budget.

### Unified Tool Call Abstraction (`UnifiedToolCall`)
To maintain universal loop protection regardless of which LLM provider or SDK powers the agent, Kryneth normalizes heterogeneous tool payloads into a unified internal representation (`UnifiedToolCall` in `usecases/behavior_guard.rs`):

```rust
struct UnifiedToolCall {
    pub name: String,
    pub semantic_hash: u64,
}
```

* **Multi-Provider Normalization**: Seamlessly ingests OpenAI/Groq `function` objects (`name` + `arguments`), Google Gemini `functionCall` nodes (`name` + `args`), and Anthropic/Cohere `input`/`parameters` structures into a standardized evaluation pipeline.

### Ephemeral Key Evasion Trap (`hash_borrowed_value`)
Rogue or looping LLM agents occasionally attempt to evade string-hashing loop detectors by injecting dynamic, non-functional properties into their tool argument payloads (e.g., adding `timestamp: "17:31:00"`, `nonce: 12345`, or dynamic `uuid` fields) or reordering JSON keys.

Kryneth neutralizes these evasion tactics through the **Ephemeral Key Evasion Trap** (`hash_borrowed_value`):

1. **Ephemeral Key Blacklisting**: During zero-allocation JSON traversal (`simd_json::BorrowedValue`), Kryneth explicitly strips blacklisted ephemeral keys:
   ```rust
   if !["timestamp", "nonce", "uuid", "stream", "session_id", "time"].contains(&k_str) {
       // hash key-value pair
   }
   ```
   *Note: Core functional identifiers like `id` (e.g. `customer_id` or `ticket_id`) are preserved to avoid false loop triggers on distinct entity queries.*

2. **Order-Independent XOR Combination**: Object key-value hashes are combined using bitwise XOR operations (`obj_hash ^= kv_hasher.finish()`). Reordering JSON keys produces an identical `semantic_hash`.

3. **Numeric Coercion**: Integer and floating-point representations (e.g., `5` vs `5.0`) are coerced into identical 64-bit IEEE 754 bit representations (`(*n as f64).to_bits()`), preventing precision format changes from evading hash checks.

### Hashing Signature Trap & Enforcement
- For every outgoing tool call, Kryneth calculates a composite loop key:
  $$\text{LoopKey} = \text{AHasher}(\text{session\_id} \parallel \text{tool\_name} \parallel \text{semantic\_hash})$$
- If the repeat count for a specific `LoopKey` exceeds `MAX_IDENTICAL_TOOL_CALLS` (default: `5`) within a 60-second sliding window stored in `AppState::agent_guardian_cache` (powered by `Moka`), Kryneth intercepts the call locally.
- It returns a structured `GatewayError::AgentRunawayLoop` (`HTTP 429 Too Many Requests`), terminating the infinite loop before it consumes upstream tokens.

---

## 2. Tool Storm Mitigation

Multi-agent frameworks occasionally initiate degenerate recursive states where they trigger hundreds of parallel tool calls in rapid succession.

-   **Session Budgets**: Kryneth monitors the total count of distinct tool executions triggered by an active session ID.
-   **Mitigation**: If the number of invocations breaches the defined threshold (default: `20` calls per 60s), the gateway blocks additional calls, safeguarding backend resources and database systems.

---

## 3. Open Policy Agent (OPA) Firewall

Kryneth intercepts tool call targets and validates them against policies loaded in an external Open Policy Agent (OPA) container.

-   **Fail-Closed State**: If the OPA service is unreachable, Kryneth defaults to blocking the request for security.
-   **Soft-Steering Mutation**: Instead of failing the application with an HTTP 500 when OPA blocks a tool, Kryneth mutates the request payload inline to guide the agent:
    ```json
    "Kryneth Guard: Policy Denied. Use read-only tools instead."
    ```
    This guides the reasoning engine to alternative strategies without terminating the agent runner session.

---

## 4. Configuration Reference

Configure these environment variables in your deployment setup:

| Env Variable | Requirement | Default | Description |
| :--- | :--- | :--- | :--- |
| `MAX_IDENTICAL_TOOL_CALLS` | **Optional** | `5` | Maximum number of tool calls with identical signatures allowed per 60-second sliding window. |
| `MAX_SESSION_TOOL_CALLS` | **Optional** | `20` | Maximum absolute tool calls allowed per session per 60-second window. |
| `SANDBOX_FALLBACK_MODE` | **Optional** | `closed` | Security boundary policy when OPA is down. Options: `open` (Fail-Open) or `closed` (Fail-Closed). |
