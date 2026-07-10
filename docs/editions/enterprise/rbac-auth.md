# Authentication & RBAC Isolation

Kryneth Enterprise implements multi-tenant security boundaries to isolate client workspaces, models, rate limits, and analytics.

---

## 1. Authentication Middleware

The gateway's `AuthMW` middleware intercepts incoming client connections:
-   **Token Verification**: Validates JWT tokens or system API keys against the high-availability PostgreSQL database backend.
-   **Identity Extraction**: Resolves the client identity into tenant UUIDs (`x-tenant-id`) and user roles, injecting them into Axum's connection extensions.
-   **Local Caching**: Recent tokens are cached in the gateway's L1 memory, avoiding database lookup overhead for every incoming request.

---

## 2. Role-Based Access Control (RBAC)

Kryneth enforces strict role definitions to control which capabilities are exposed to clients:

-   **Admin**: Full system management (updating routing configs, altering tenant balances, auditing trace logs).
-   **Developer**: Can register custom MCP tools and inspect local debugging traces.
-   **Agent**: Restricted to executing designated virtual models. Cannot update settings or access administrative telemetry.

---

## 3. Configuration Reference

Configure these environment variables to enable RBAC:

| Env Variable | Requirement | Default | Description |
| :--- | :--- | :--- | :--- |
| `DATABASE_URL` | **Required** | *Empty* | PostgreSQL database connection string used for tenant validation. |
| `JWT_SECRET` | **Required** | *Empty* | Secret key used to sign and verify client session tokens. |
