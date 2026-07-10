# Operational Guide

This guide provides operators and DevOps engineers with instructions to deploy, configure, monitor, and troubleshoot the Kryneth AI Gateway. Because Kryneth is built using a Hexagonal Architecture (Ports & Adapters), operational requirements change depending on whether you deploy the **OSS Gateway** (memory-bound) or the **Enterprise Engine** (database-bound).

---

## 1. Deployment Topologies

### OSS Gateway (Standalone)
The OSS Edition focuses on developer friction-less onboarding. It runs entirely in memory without requiring external databases or services.
-   **Ideal for**: Local development, small-scale deployments, open-source testing.
-   **Infrastructure Requirements**: None. Simply run `cargo run` or use the provided standalone Docker container. 
-   **State**: Transient. Restarts clear L1 semantic caches and rate limit counters.

### Enterprise Full-Stack
The Enterprise Edition relies on concrete adapters (PostgreSQL, Redis, ClickHouse) for horizontal scalability, high-availability, and distributed state management.
-   **Ideal for**: Production API gateways, Multi-tenant SaaS, High-traffic routing.
-   **Infrastructure Requirements**: 
    -   PostgreSQL (for tenant configuration, billing ledgers, RBAC)
    -   Redis (for distributed rate-limiting and session tracking)
    -   ClickHouse (for high-volume async telemetry processing)
    -   gRPC Server (for L2 Distributed Cache)

---

## 2. Configuration & Environment Variables

Kryneth uses environment variables for configuration. Do **NOT** commit `.env` or `routing.yaml` files containing real API keys or secrets into version control.

### Core Gateway Flags (Applies to both OSS & Enterprise)
-   `RUST_LOG`: Controls logging verbosity via `tracing` (e.g. `RUST_LOG=info`).
-   `PORT`: HTTP port to bind to (Default: `8080`).
-   `RATE_LIMIT_MAX_REQUESTS`: Strict limit to prevent unexpected traffic spikes.
-   `MAX_SESSION_TOOL_CALLS`: Triggers tool storm mitigation if agents exceed this limit.
-   `MAX_IDENTICAL_TOOL_CALLS`: Triggers infinite loop interception.
-   `SANDBOX_FALLBACK_MODE`: Controls OPA Sandbox failure behavior (Default: `closed`).

### Enterprise Services Configuration
-   `DATABASE_URL`: PostgreSQL connection string.
-   `REDIS_URL`: Redis connection string.
-   `CLICKHOUSE_URL`: ClickHouse HTTP endpoint for telemetry.

### Enterprise-Specific Storage & WAL Configuration
-   **`REDB_TELEMETRY_WAL_PATH`** (Optional, Default: `./data/telemetry_wal.redb`):
    The filepath for the embedded Write-Ahead Log (WAL) database managed by `redb`.
    
    > [!IMPORTANT]
    > **Container Volume Mounting:**
    > To prevent telemetry loss during unexpected container restarts or gateway failures, you **MUST** map a persistent Docker volume to the folder hosting the `telemetry_wal.redb` database (e.g., mounting a volume to `/app/data/` if using the default path).

-   **`DASHBOARD_URL`** (Optional, Default: `http://localhost:5173`):
    Origin used to validate CORS policies for admin calls.

---

## 3. Distributed State Channels & Budgets

In Enterprise clusters, gateway instances coordinate configuration updates and budget thresholds through Redis-backed event loops:

### Dynamic Pricing & Config Sync
When model pricing or route priorities change in the control plane, `kryneth_config` publishes updates to the Redis channels:
-   **`kryneth:billing_updates`**: Pushes dynamic completions pricing tables and tenant budgets.
-   **`kryneth:routing_updates`**: Pushes dynamic upstream target priorities.
The gateway instances subscribe to these channels and atomically reload state via `ArcSwap` pointers in under **1ms**, without restarting.

### Emergency Kill Switch
Admins can trigger an emergency kill switch via the billing dashboard preferences. This writes a budget value of `0.0` directly to the Redis key:
```text
billing:tenant:<tenant_id>:balance
```
All connected gateway instances instantly identify the zero-balance state on subsequent auth/routing checks and drop incoming traffic, protecting the tenant from runaway agent expenses.

---

## 4. Port Allocation Map

Ensure your local firewall or Kubernetes network policies allow traffic routing across the following ports:

| Service Name | Port | Protocol | Default URL | Description |
| :--- | :--- | :--- | :--- | :--- |
| **Kryneth Gateway** | `8080` | HTTP | `http://localhost:8080` | L7 API proxy & routing engine |
| **Config Service** | `8085` | HTTP | `http://localhost:8085` | Control plane dynamic routing APIs |
| **Cache Service REST** | `8081` | HTTP | `http://localhost:8081` | Cache lookup, store & manual embeddings |
| **Cache Service gRPC** | `50051` | gRPC/H2 | `http://localhost:50051` | High-speed L2 semantic Cache database |
| **Compliance Service** | `8083` | HTTP | `http://localhost:8083` | PII scanning, geo-routing & OPA checks |
| **Auth Service** | `8084` | HTTP | `http://localhost:8084` | Tenant and API Key Management |
| **Tracer Service** | `8082` | HTTP | `http://localhost:8082` | ClickHouse telemetry interface |
| **React Dashboard** | `5173` | HTTP | `http://localhost:5173` | UI Management Console |

---

## 5. Troubleshooting Common Scenarios

### Diagnosing 429 Blocks (Runaway Agent Loops)
**Symptom**: Requests are blocked returning `429 Too Many Requests` indicating a Runaway Loop or Tool Storm.
**Action**:
1.  Inspect the logs for `Tool storm detected` or `Runaway loop detected`.
2.  Identify the offending Agent Session ID. If this is a false positive, adjust `MAX_SESSION_TOOL_CALLS` or `MAX_IDENTICAL_TOOL_CALLS`.
3.  In Enterprise deployments, check if the tenant has breached their predefined `enforce_agentic_loop_budget`.

### MCP Registry Connectivity & Schema Failures
**Symptom**: Agents receive full schemas instead of lazy semantic summaries, causing high token usage.
**Action**:
1.  Check if `MCP_TOOL_SCHEMA_REGISTRY` is correctly loaded. If Kryneth fails to parse this at startup, it fails-open.
2.  Verify that `SANDBOX_FALLBACK_MODE` isn't misconfigured.
3.  Check telemetry logs for `Tunnel 3 Phase 2 — lazy schema injection` messages.
