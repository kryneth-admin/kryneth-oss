# Agentic Firewall (Behavior Guard)

To prevent autonomous agents from running out of bounds, repeating failing actions, or triggering costly "tool storms," Kryneth acts as a behavior-aware firewall. It actively inspects outgoing tool executions and request velocities in real time.

---

## 1. Runaway Loop Interception

If an agent encounters a parsing error or database timeout, it may enter an infinite loop, repeating the exact same execution parameters and draining your API budget.

### Hashing Signature Trap
-   For every outgoing tool execution, Kryneth generates a deterministic hash signature of the tool name and argument JSON string using the fast `ahash` algorithm.
-   If the same signature repeat count exceeds the threshold (default: `5`) within a 60-second sliding window, Kryneth intercepts the request locally.
-   It returns an HTTP `429 Too Many Requests` directly, preventing the transaction from consuming upstream LLM tokens.

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
