-- Gateway test schema — single migration run by setup_test_environment().
--
-- This is NOT a production migration. It is a consolidated DDL snapshot that
-- the gateway integration tests apply to ephemeral testcontainers Postgres
-- instances.  It mirrors the subset of tables the gateway reads from the
-- shared DB (owned by kryneth_auth in production), ensuring tests always run
-- against a correctly-shaped schema without touching any persistent database.
--
-- To add a column or table that the gateway needs in tests:
--   1. Add it here (with IF NOT EXISTS guards).
--   2. Never hand-craft CREATE TABLE / ALTER TABLE in test setup functions.

-- Required extension for gen_random_uuid().
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ── Core tenant identity ──────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS tenants (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_name   TEXT NOT NULL DEFAULT 'test-workspace',
    onboarding_status BOOLEAN NOT NULL DEFAULT FALSE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Virtual API keys issued to gateway callers ────────────────────────────────
CREATE TABLE IF NOT EXISTS api_keys (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id  UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    key_hash   TEXT NOT NULL,
    is_active  BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── LLM provider keys (encrypted) ─────────────────────────────────────────────
-- Mirrors the schema produced by kryneth_auth's migration chain up to
-- 20260501000001_add_weight_to_provider_keys.sql.
CREATE TABLE IF NOT EXISTS provider_keys (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    provider_name TEXT NOT NULL,
    key_alias     VARCHAR NOT NULL DEFAULT 'default',
    encrypted_key BYTEA,
    priority      INT NOT NULL DEFAULT 1,
    weight        INT NOT NULL DEFAULT 1,
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT unique_tenant_provider_alias
        UNIQUE (tenant_id, provider_name, key_alias)
);

-- Index for fast per-tenant key lookups (mirrors production index).
CREATE INDEX IF NOT EXISTS idx_provider_keys_tenant_id
    ON provider_keys (tenant_id);
