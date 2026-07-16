---
icon: "shield-check"
---
# Security Model & Trust Boundaries

At Kryneth, security is the foundational purpose of the product. Kryneth acts as a **Runtime Control Plane and Firewall** to protect enterprise infrastructure, API budgets, and data privacy from non-deterministic autonomous agents.

Autonomous AI agents (via LangGraph, CrewAI, MCP, etc.) introduce a new class of cybersecurity risks that traditional API gateways cannot mitigate. The Kryneth Agent Control Plane establishes a Zero-Trust architecture between your internal tools, external LLMs, and the autonomous agents traversing them.

---

## 1. Agentic Firewall Security (Behavior Guard)

Traditional security models focus on securing ingress from external malicious actors. Kryneth focuses on securing egress and intra-session runaways triggered by autonomous LLMs.

### OSS Core Security
-   **Tool Storm Mitigation**: Agents occasionally enter degenerate states where they spawn thousands of distinct, parallel tool calls in a single frame. The OSS Core tracks these calls via `MAX_SESSION_TOOL_CALLS` and preemptively cuts the connection, preventing Denial of Wallet (DoW) attacks.
-   **Runaway Loop Trapping**: We actively trap recursive loop behaviors. By employing zero-allocation SHA-256 signature hashing on tool names and arguments, Kryneth detects when an agent stubbornly repeats the exact same failing tool invocation and intercepts the traffic before it reaches upstream providers.
-   **OPA Sandbox Firewall (Fail-Closed)**: Kryneth intercepts outbound tool execution requests. It consults an Open Policy Agent (OPA) sidecar over HTTP to validate RBAC policies (e.g., "Can Tenant X execute `drop_table`?"). If OPA is unreachable, the system defaults to Fail-Closed.
-   **Soft-Steering Mutation**: When OPA denies a tool call, Kryneth does not simply return an HTTP 500. Instead, it mutates the request inline, injecting a compact soft-steer response back into the LLM context (e.g., *"Kryneth Guard: Policy Denied. Use read-only tools."*). This safely guides the agent away from destructive behaviors without crashing the orchestrator.

### Enterprise Engine Security
-   **Financial Security (Spend & Burn Rate)**: Extends the OSS logic to enforce strict token budgets across tenancies. If an agent's intra-session burn-rate exceeds the predefined velocity limits, the Kryneth Enterprise proxy rejects subsequent calls, ensuring financial safety.

---

## 2. Data Exfiltration & Privacy (PII Protection)

Data leaks often occur implicitly when LLMs include sensitive information in context. 
-   **Pre-Flight Redaction**: Kryneth's pipeline inspects payloads at the edge, actively identifying and redacting Personally Identifiable Information (PII) like PAN cards, SSNs, and credit cards *before* they leave your infrastructure and reach public LLMs.

---

## 3. MCP Governance (Model Context Protocol)

The Model Context Protocol (MCP) enables LLMs to dynamically discover and invoke local tools. Kryneth governs this securely:
-   **Strict Schema Parsing**: Employs rigorous parsing on upstream tool definitions.
-   **Bounded Allocations**: All mutation operations (like stripping heavy parameter schemas) use bounded arena allocators (e.g., `bumpalo`), preventing memory-exhaustion DoS attacks.
-   **Zero-Copy Validations**: Fast-path detection utilizes byte-level scanning (e.g., `simd-json`) on incoming payloads. If no `tools` array is detected, Kryneth passes the bytes zero-copy, ensuring maximum throughput without exposing parsing attack vectors.

---

## 4. Authentication & Authorization (Enterprise Engine)

The Enterprise Engine overlays strict IAM controls across the proxy:
-   **JWT & API Key Validation**: Every request is intercepted by the `AuthMW` middleware which performs fast tenant-level token validations against a high-availability PostgreSQL backend.
-   **RBAC**: Admin endpoints and specific LLM routes are guarded by strict Role-Based Access Controls to prevent privilege escalation.

---

## 5. Memory Safety by Default

The entire Kryneth control plane is written in safe Rust. This design choice fundamentally eliminates entire classes of memory vulnerabilities (buffer overflows, use-after-free, double frees) that are endemic in traditional C/C++ reverse proxies.
