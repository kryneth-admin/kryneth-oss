# API Endpoints Catalog

Kryneth Gateway and its accompanying control plane services expose standard REST and gRPC interfaces for completions proxying, configuration, caching, and analytics tracking.

---

## 1. Chat Completions Proxy (`kryneth_gateway`)

### `POST /v1/chat/completions`
Intercepts completions requests, applies agent safety guards, performs cache lookups, and forwards requests to active upstream providers.

#### Request Headers
*   `Authorization`: `Bearer <token>` (**Required**)
*   `x-kryneth-model`: Virtual target model (e.g. `llama-3.3-70b-versatile`) (**Required**)
*   `x-tenant-id`: Tenant workspace isolation UUID (**Optional**)
*   `x-session-id`: Session identifier used for runaway loop tracking (**Optional**)

#### Request Payload Example
```json
{
  "model": "llama-3.3-70b-versatile",
  "messages": [
    {
      "role": "user",
      "content": "Perform the system run query."
    }
  ],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "query_db",
        "description": "Query database tables"
      }
    }
  ]
}
```

---

## 2. Configuration Control Plane (`kryneth_config`)

These endpoints manage the dynamic workspace, routing, and tool registrations for your gateway nodes.

| Method | Endpoint Path | Description | Access |
| :--- | :--- | :--- | :--- |
| **GET** | `/v1/config/:tenant_id` | Fetch the full active routing config. | Tenant Developer |
| **PUT** | `/v1/config/:tenant_id` | Upsert the full routing configuration. | Tenant Admin |
| **GET** | `/v1/config/:tenant_id/models` | List all registered virtual models. | Tenant Developer |
| **POST** | `/v1/config/:tenant_id/models` | Register or update a virtual model mapping. | Tenant Admin |
| **DELETE**| `/v1/config/:tenant_id/models/:name`| Remove a virtual model mapping. | Tenant Admin |
| **POST** | `/v1/config/:tenant_id/routing-mesh`| Force compile and sync the routing mesh. | Tenant Admin |
| **GET** | `/v1/config/:tenant_id/api-keys` | List active keys in this tenant space. | Tenant Admin |
| **DELETE**| `/v1/config/:tenant_id/api-keys/:id`| Revoke a tenant API key. | Tenant Admin |
| **GET** | `/v1/config/:tenant_id/mcp-servers`| List registered SSE MCP servers. | Tenant Developer |
| **PUT** | `/v1/config/:tenant_id/mcp-servers`| Register or update an SSE MCP server. | Tenant Admin |
| **DELETE**| `/v1/config/:tenant_id/mcp-servers/:name`| Unregister an MCP server. | Tenant Admin |
| **POST** | `/v1/config/:tenant_id/mcp-servers/test`| Send ping/schema queries to test an MCP server. | Tenant Developer |

---

## 3. Identity & Access Control (`kryneth_auth`)

Handles developer and admin authorization, OAuth logins, and securely stores third-party credentials.

### Public Auth Routes
*   `POST /v1/auth/signup` — Create a new developer account.
*   `POST /v1/auth/login` — Authenticate and receive a JWT session token.
*   `POST /v1/auth/logout` — Terminate active session.
*   `POST /v1/auth/refresh` — Refresh expired JWT token.
*   `POST /v1/auth/otp/request` — Initiate 2FA verification.
*   `POST /v1/auth/otp/verify` — Validate OTP token.
*   `GET /v1/auth/google/login` & `/google/callback` — Google OAuth handlers.
*   `GET /v1/auth/google/callback` — Google OAuth callback.
*   `GET /v1/auth/github/login` & `/github/callback` — GitHub OAuth handlers.
*   `GET /v1/auth/github/callback` — GitHub OAuth callback.

### Protected Auth Routes (Requires JWT bearer header)
*   `POST /v1/auth/onboard` — Bootstrap a new tenant organization.
*   `GET /v1/auth/api-keys` & `POST /v1/auth/api-keys` — Read/Generate user API keys.
*   `GET /v1/auth/provider-keys` — List encrypted upstream LLM keys.
*   `POST /v1/auth/provider-keys` — Store encrypted LLM API keys (e.g. OpenAI/Anthropic keys). Values are encrypted using AES-256-GCM.
*   `DELETE /v1/auth/provider-keys/:id` — Revoke a stored LLM key.
*   `POST /v1/auth/invite` — Invite a new member to the tenant team.
*   `GET /v1/auth/members` — List all members of the tenant team.
*   `PUT /v1/auth/members/:id` & `DELETE /v1/auth/members/:id` — Edit member roles or remove them.

---

## 4. Cache Administration (`kryneth_cache`)

These low-latency REST endpoints are exposed on port **8081** to allow direct interactions with the cache engine:
*   `POST /v1/cache/lookup` — Perform direct cache lookups using JSON prompt signatures.
*   `POST /v1/cache/store` — Manually write responses to the exact and semantic cache vectors.
*   `POST /embed` — Generates a local embedding vector (384-dimensions, BGE Small) for a given prompt without hitting remote APIs.

---

## 5. Enterprise Telemetry & Analytics (`kryneth_gateway` Admin)

Exposed under the `/v1/admin` namespace of the gateway service, these routes require **Admin** JWT authorization and pull telemetry from ClickHouse:

*   `GET /v1/admin/audit-logs` — Fetch compliance and firewall check logs.
*   `GET /v1/admin/metrics/hot-swaps` — Fetch target model failover and hot-swap frequency.
*   `GET /v1/admin/metrics/usage` — Fetch aggregate token count usage.
*   `GET /v1/admin/billing/breakdown` — Detailed cost calculations and credit spent logs.
*   `GET /v1/admin/billing/preferences` & `PUT /v1/admin/billing/preferences` — Get/Update tenant budgets and trigger alerts.
*   `GET /v1/admin/billing/pricing` & `PUT /v1/admin/billing/pricing` — Get/Update completions pricing rate maps.
*   `GET /v1/admin/traces` & `GET /v1/admin/traces/:trace_id` — Inspect individual transaction executions.
*   `GET /v1/admin/security/alerts` — Fetch prompt injection blocks and policy failures.
*   `GET /v1/admin/metrics/cache` — Fetch cache performance statistics.
*   `GET /v1/admin/metrics/compliance` — Fetch PII scan count statistics.
*   `GET /v1/admin/metrics/sankey` — Retrieve model flow layout diagrams for UI dashboard rendering.
