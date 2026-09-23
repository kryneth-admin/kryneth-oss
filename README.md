# Kryneth

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)](https://github.com/kryneth-admin/kryneth-oss)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Rust](https://img.shields.io/badge/Rust-1.77%2B-orange.svg)](https://www.rust-lang.org/)
[![Docker](https://img.shields.io/badge/Docker-supported-blue.svg)](https://www.docker.com/)
[![Discord](https://img.shields.io/discord/1234567890?color=5865F2&logo=discord&logoColor=white&label=Discord)](https://discord.gg/uurgj9fMy8)
[![Rust CI](https://github.com/kryneth-admin/kryneth-oss/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/kryneth-admin/kryneth-oss/actions/workflows/rust-ci.yml)

> **Kryneth is the execution-integrity layer for AI systems that can take consequential actions.**
>
> Kryneth provides a neutral execution boundary between AI-generated actions and the real systems they can change. It enforces execution identity, deterministic idempotency, lease versioning, stale completion fencing, and explicit `UNKNOWN` outcome state handling—preventing unsafe side-effect replay when networks time out or agents fail.

<div align="center">
  <img src="./docs/quickstart_demo.gif" alt="Kryneth Gateway Setup Demo" width="100%">
</div>

---

## 📐 The Core Invariant

Every autonomous AI application eventually crosses a fundamental boundary:

```text
AI-generated action
        ↓
Kryneth execution boundary
        ↓
real-world side effect
```

Kryneth exists at the exact point where an AI system moves from **reasoning and intent** to an **actual operation against an external system**. 

Consequential actions governed by this boundary include:
* Financial transactions & invoice processing
* Production cloud infrastructure mutations
* Database writes & customer record updates
* Enterprise ERP & CRM updates
* Automated communications (email, Slack, SMS)
* Autonomous browser actions & web mutations
* Model Context Protocol (MCP) tool invocations

---

## ⚡ Quick Start

Drop Kryneth in front of your existing AI requests in under 60 seconds:

```bash
# 1. Boot the gateway
docker run -d -p 8080:8080 kryenthhq/krynethgw:latest

# 2. Point your existing OpenAI SDK or agent client to localhost
export OPENAI_BASE_URL="http://localhost:8080/v1"
```

---

## 🎯 Why This Layer Exists: The Execution Problem

Existing infrastructure tools address different parts of the AI stack:
* **LLM Gateways** manage traffic (routing, rate limits, token caching).
* **Agent Frameworks** manage reasoning, planning, and orchestration.
* **Tool Transports (MCP, OpenAPI)** expose system capabilities.
* **Observability Systems** record telemetry after the fact.

**Kryneth focuses on the execution boundary where an AI-generated action becomes a real-world side effect.**

```text
LLM gateways manage AI traffic.
Agent frameworks manage reasoning and orchestration.
Tool systems expose capabilities.
Observability systems show what happened.

Kryneth controls the execution boundary where an AI action mutates a real system.
```

### The Failure Scenario: `timeout ≠ failure`

Consider what happens when an AI agent requests a mutating operation against an external system:

```text
Agent / LLM
     │
     │ "Charge customer $500"
     ▼
  Kryneth (Execution E123, Operation O456, Attempt A1)
     │
     │ Payment Request
     ▼
External Payment Provider
     │
     ❌ (Network Timeout / Socket Drop after 5s)
```

In a standard agent setup, a network timeout forces a dilemma. The downstream provider may have:
1. Processed the charge successfully before the socket dropped,
2. Failed before initiating any side effect, or
3. Entered an ambiguous partial execution state.

Without an execution-integrity layer, an agent or runner interprets a timeout as `Failure` and blindly retries:

```text
Unsafe System:
timeout → failure → blind retry → duplicated side effect ($1,000 charged)
```

### The Kryneth Execution Semantics

Kryneth introduces explicit execution state machine semantics so that an ambiguous downstream outcome becomes **`UNKNOWN`** rather than being blindly converted into failure + retry:

```text
Kryneth Model:
timeout → UNKNOWN state → block unsafe retry → attempt reconciliation → inform caller/workflow
```

If the agent tries to execute the exact same mutating operation while in an `UNKNOWN` state, Kryneth blocks the unsafe retry (`PREVIOUS_ATTEMPT_UNKNOWN`) and attempts out-of-band reconciliation before allowing any side effect to replay.

---

## 🆔 Execution Model & Identity Hierarchy

Kryneth organizes execution tracking around an explicit identity hierarchy implemented across the domain model:

```text
Tenant
  ↓
Session
  ↓
Workflow
  ↓
Execution
  ↓
Operation
  ↓
Attempt
```

### Strongly-Typed Identifiers

* `TenantId`: Scopes all executions, rate limits, and configuration to an organization or workspace.
* `SessionId`: Correlates conversation history or agent session context.
* `WorkflowId`: Optional identifier binding multi-step agent runs across multiple execution steps.
* `AgentId`: Identifies the specific agent or subagent invoking the tool.
* `ExecutionId`: Unique identifier for an execution run.
* `OperationId`: Uniquely identifies a single business operation.
* `AttemptId` / `Attempt`: Tracks physical execution attempts (Attempt 1, Attempt 2, ...).
* `IdempotencyKey`: Deterministic hash derived from `tenant_id :: key :: tool_index :: tool_name :: canonical_arguments`.

### ⚠️ Idempotency Key vs. Behavioral Semantic Hash

Kryneth makes a strict technical distinction between business idempotency and behavioral loop protection:

* **Business Idempotency (`IdempotencyKey`)**: Derived via deterministic SHA-256 canonical JSON hashing (deeply key-sorted). Binds physical attempts to an authoritative business operation state.
* **Behavioral Semantic Hash (`ahash`)**: Used by Kryneth's Agent Guardian to detect runaway loops and tool storms by hashing parameter signatures (ignoring ephemeral nonces, timestamps, and UUIDs). This is a **behavior-protection mechanism**, not authoritative business-operation identity.

---

## 🔄 Execution States

Kryneth tracks every side-effect operation through a strict state machine (`ExecutionState`):

| State | Operational Meaning |
| :--- | :--- |
| **`Pending`** | Execution created, awaiting worker lease. |
| **`Claimed`** | Lease acquired by an execution worker with an active time duration. |
| **`Running`** | Physical attempt actively in flight against downstream target/MCP server. |
| **`Succeeded`** | Downstream side effect completed cleanly. Result payload cached for deduplication. |
| **`Failed`** | Deterministic failure confirmed by downstream target (e.g., HTTP 400 Bad Request). Safe to retry. |
| **`Unknown`** | Outcome ambiguous (network timeout, socket drop). Unsafe retries blocked. |
| **`Reconciling`** | System attempting out-of-band status query to determine true downstream state. |

### `Failed` vs. `Unknown`

* **`Failed`**: Kryneth has verifiable proof that the downstream action did not succeed. Safe retries are permitted.
* **`Unknown`**: Kryneth cannot safely prove whether the side effect occurred. Replaying mutating side effects is blocked until reconciled.
* **`Reconciling`**: A reconciliation provider is actively querying the external system to transition `Unknown` into `Succeeded` or `Failed`.

---

## 🛡️ Stale Completion Fencing & Lease Versioning

When a network request times out, the physical attempt might still be running in the background on the downstream server. If a secondary attempt is launched later, the first attempt completing late must not corrupt the new execution state.

Kryneth enforces **monotonic lease versioning** (`version: u64` in `ExecutionContext`):

```text
Attempt A1 (version 1)
   ↓
(Timeout occurs → state becomes UNKNOWN)
   ↓
Attempt A2 (reclaimed, version incremented to 2)
   ↓
Attempt A1 finishes late in background
   ↓
Kryneth checks version: store version (2) != attempt version (1)
   ↓
Attempt A1 result is FENCED OUT and ignored.
```

`mark_succeeded`, `mark_failed`, and `mark_unknown` verify version match prior to committing state transitions.

---

## 🔀 The `A ↔ KRYNETH ↔ B` Integration Model

Kryneth does not sit merely as a passive proxy (`A → Kryneth → B`). It operates as an execution boundary between Agent `A` and Target System `B`:

```text
       Agent / LLM Framework (A)
                │
                │ 1. Mutating Tool Request (TraceContext, IdempotencyKey)
                ▼
    ╔═════════════════════════════════════════╗
    ║            KRYNETH GATEWAY              ║
    ║        Execution Integrity Boundary     ║
    ║                                         ║
    ║  • Idempotency Check    • State Machine ║
    ║  • Policy Guard         • Version Lease ║
    ║  • Fencing Guard        • Audit Trace   ║
    ╚════════════════╤════════════════════════╝
                     │
                     │ 2. Tool Execution (Attempt 1)
                     ▼
       Real External System / MCP Server (B)
                     │
                     │ 3. Response / Network Timeout / Error
                     ▼
    ╔═════════════════════════════════════════╗
    ║            KRYNETH GATEWAY              ║
    ║  Classifies outcome (Succeeded/Unknown) ║
    ║  Applies stale-completion version fence ║
    ╚════════════════╤════════════════════════╝
                     │
                     │ 4. Trustworthy Execution State
                     ▼
       Agent / LLM Framework (A)
```

### Upstream Responsibility (to Agent / Workflow)
Reports authoritative, trustworthy execution state:
* Accepted & Executed (`Succeeded`)
* Deterministic Error (`Failed`)
* In Flight (`ALREADY_IN_FLIGHT`)
* Ambiguous Timeout (`PREVIOUS_ATTEMPT_UNKNOWN`)
* Policy Denied / Blocked (`AgentToolStorm`, `AgentRunawayLoop`)

### Downstream Responsibility (to Target System)
Interacts with external APIs and MCP servers:
* Bounded concurrent execution (capped fan-out streams)
* Classifies transport timeouts vs business responses
* Fences out late or detached completions
* Prevents duplicate execution replay

---

## 🧠 Reasoning Authority vs. Execution Authority

A fundamental principle of Kryneth’s architecture:

```text
Agent / LLM
= Decides intent ("I want to process a refund for Order #99").

Kryneth
= Controls and records execution ("Operation O-99 state is UNKNOWN; replay blocked").

External System
= Performs the real side effect.
```

The LLM's reasoning engine is **never treated as the authoritative source of execution state**. If an LLM assumes an action failed because of a socket timeout, Kryneth's execution state layer retains the authoritative record (`UNKNOWN`) and blocks unsafe duplicate side effects regardless of LLM prompt hallucinatory retries.

---

## 🏛️ Architecture & Supporting Gateway Capabilities

Kryneth positions traditional gateway functionality as **supporting infrastructure around the execution boundary**:

```text
Kryneth Architecture
├── Execution Integrity (Core Boundary)
│   ├── Execution Identity & Identity Hierarchy
│   ├── Deterministic Idempotency Engine (Canonical SHA-256)
│   ├── Monotonic Version Leasing & Stale Completion Fencing
│   ├── Explicit UNKNOWN State Machine
│   └── Reconciliation Hooks & Adapters
│
└── Supporting Gateway & Runtime Protections
    ├── OpenAI-Compatible Ingress (/v1/chat/completions)
    ├── Upstream Multi-LLM Routing & Failover (Groq, Cohere, OpenAI, Anthropic, Gemini)
    ├── Bounded MCP Tool Execution (Capped concurrent fan-out stream)
    ├── Agent Guardian (Runaway Loop & Tool Storm Protection via ahash)
    ├── PII Masking & Local Compliance Sandbox Engine
    └── Audit & Ephemeral Trace Foundation
```

### Flow Architecture

```mermaid
graph TD
    classDef framework fill:#1E293B,stroke:#3B82F6,stroke-width:2px,color:#F8FAFC
    classDef ingress fill:#0F172A,stroke:#64748B,stroke-width:2px,color:#F8FAFC
    classDef execution fill:#450A0A,stroke:#EF4444,stroke-width:2px,color:#F8FAFC
    classDef routing fill:#064E3B,stroke:#10B981,stroke-width:2px,color:#F8FAFC
    classDef upstream fill:#78350F,stroke:#F59E0B,stroke-width:2px,color:#F8FAFC

    subgraph ClientLayer ["Agentic Client Layer"]
        Agent["AI Agent / Workflow<br>(LangGraph, CrewAI, AutoGen, Claude, Custom)"]:::framework
    end

    subgraph Firewall ["Kryneth Execution Integrity Layer"]
        Ingress["Kryneth Ingress<br/>(Axum HTTP & SSE Router)"]:::ingress

        subgraph ExecEngine ["Execution Engine"]
            Idempotency["Idempotency & Claim Store<br/>(Moka ExecutionStore)"]:::execution
            Guardian["Agent Guardian<br/>(Runaway Loops & Tool Storms)"]:::execution
            StateEngine["Execution State Machine<br/>(Pending / Claimed / Succeeded / Unknown)"]:::execution
        end

        subgraph RouteLayer ["Supporting Gateway Services"]
            CircuitBreaker["Provider Failover & Routing"]:::routing
            MCPTransport["MCP Tool Transport<br/>(Bounded Fan-Out Stream)"]:::routing
        end
    end

    subgraph Upstream ["External Layer"]
        LLMs["Model Providers<br>(OpenAI, Anthropic, Gemini, Groq)"]:::upstream
        MCPServers["MCP Tool Servers & External APIs<br>(Databases, Payment Gateways, Cloud)"]:::upstream
    end

    Agent -->|1. Request / Tool Calls| Ingress
    Ingress --> Guardian
    Guardian -->|Pass| Idempotency
    Idempotency -->|Check State| StateEngine
    StateEngine -->|New / Retry Claim| CircuitBreaker
    StateEngine -->|Direct MCP Tool Call| MCPTransport

    CircuitBreaker -->|Route Request| LLMs
    MCPTransport -->|Execute Side Effect| MCPServers

    MCPServers -->|Result / Timeout| StateEngine
    LLMs -->|Response| Ingress
    StateEngine -->|Cached Result / UNKNOWN Error| Ingress
    Ingress -->|Trustworthy Outcome| Agent
```

---

## 🚫 What Kryneth Is Not

To maintain architectural clarity, Kryneth explicitly defines what it is **not**:

* ❌ **Not an Agent Framework**: Kryneth does not build agents, manage prompts, or execute agent planning loops (use LangGraph, CrewAI, AutoGen, or OpenAI Agents SDK).
* ❌ **Not an LLM Provider**: Kryneth does not host or train models.
* ❌ **Not a Model-Routing-Only Gateway**: Upstream provider routing is supporting infrastructure, not the core product category.
* ❌ **Not a Workflow Builder**: Kryneth does not replace visual DAG or process workflow engines.
* ❌ **Not an MCP Marketplace**: Kryneth connects tool execution to registered SSE endpoints without operating a public registry.
* ❌ **Not an Agent Memory Store**: Kryneth does not provide long-term vector/rag memory.
* ❌ **Not a General-Purpose BPM Engine**: Kryneth sits specifically at the execution boundary of AI actions.

Kryneth sits **underneath or alongside** these systems to provide execution integrity at the side-effect boundary.

---

## 🌐 Ecosystem Integration Story

Kryneth is designed to integrate into existing AI application stacks without requiring developers to rewrite their agents or abandon their choice of framework:

```text
Existing Agent (LangGraph, CrewAI, AutoGen, OpenAI Agents SDK, Claude)
  +
Existing Model Providers (OpenAI, Anthropic, Gemini, Groq)
  +
Existing Tooling / MCP Infrastructure
  ↓
Kryneth Execution Integrity Layer
```

You do not need a "Kryneth Agent Framework". You add Kryneth as the execution boundary sitting between your framework and external mutating APIs.

---

## 💻 Infrastructure vs. Optional Optimizations

AI applications utilize two distinct categories of infrastructure:

| Infrastructure Category | Examples | Role |
| :--- | :--- | :--- |
| **Optimization & Routing** | Model routing, MCP discovery, semantic caching, prompt compression, cost tracking | Optional performance, latency, and cost enhancements. |
| **Execution Integrity** | Execution identity, idempotency keys, state versioning, `UNKNOWN` handling, fencing | Mandatory boundary protection for consequential actions. |

Kryneth is intended for AI systems whose actions create **consequential external side effects** where uncoordinated retries or silent execution failures cause real-world damage.

---

## 🛠️ Open-Source Scope & Architecture Realities

The Kryneth open-source repository provides a complete process-local implementation of the execution integrity architecture:

* **Process-Local Execution Store (`MokaExecutionStore`)**: Implements `ExecutionStore` using an in-memory `moka::future::Cache` with byte-budget memory bounds and TTL expiration.
* **Port / Adapter Architecture**: Core domain traits (`ExecutionStore`, `Reconciler`, `ToolTransport`, `TelemetryPort`, `BillingPort`, `AuthPort`, `RateLimitPort`, `RoutingConfigPort`, `SemanticCachePort`) decouple execution logic from infrastructure backends.
* **OSS Defaults**: Ships with `MokaExecutionStore`, `OssReconciler`, `OssTelemetry`, and `McpToolTransport`. Enterprise backends (such as Redis cluster execution persistence or ClickHouse telemetry) implement these exact port interfaces.

> ℹ️ *Note on OSS Scope:* The OSS in-memory execution store provides process-local state tracking, lease versioning, idempotency checking, and fencing for single-node deployments. Distributed durable execution across multi-node clusters requires backends implementing the `ExecutionStore` port interface.

---

## ⚙️ Quick Start & Setup

### Prerequisites
* **Rust** 1.77+ (for local builds)
* **Docker** & **Docker Compose** (for containerized setup)

### 1. Automated Setup Script

**Linux / macOS:**
```bash
git clone https://github.com/kryneth-admin/kryneth-oss.git && cd kryneth-oss
bash setup.sh
```

**Windows (PowerShell):**
```powershell
git clone https://github.com/kryneth-admin/kryneth-oss.git
cd kryneth-oss
.\setup.ps1
```

### 2. Docker Deployment

```bash
# Build and launch with Docker Compose
docker-compose up -d --build
```

Server starts listening on `http://localhost:8080`.

### 3. Local Development Build

```bash
cargo run --release
```

### 4. Basic Configuration

**`.env` file configuration:**
```ini
GATEWAY_PORT=8080
RUST_LOG=info
KRYNETH_VALID_KEYS=re_live_local_dev_123
GROQ_API_KEY=your_groq_api_key
COHERE_API_KEY=your_cohere_api_key
```

**`routing.yaml` model routing configuration:**
```yaml
"00000000-0000-0000-0000-000000000000":
  "llama-3.3-70b-versatile":
    targets:
      - priority: 1
        weight: 100
        api_key_alias: "GROQ_API_KEY"
        provider_name: "groq"
        base_url: "https://api.groq.com/openai/v1"
        target_model: "llama-3.3-70b-versatile"
        schema_format: "openai"
      - priority: 2
        weight: 100
        api_key_alias: "COHERE_API_KEY"
        provider_name: "cohere"
        base_url: "https://api.cohere.ai/compatibility/v1"
        target_model: "command-r-plus-08-2024"
        schema_format: "openai"
    rate_limit_rpm: 60
```

### 5. Verified First Request

Point your client or `curl` to the local gateway:

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer re_live_local_dev_123" \
  -d '{
    "model": "llama-3.3-70b-versatile",
    "messages": [
      {
        "role": "user",
        "content": "Verify execution state integrity."
      }
    ]
  }'
```

---

## 🏛️ Open-Core Scope Overview

| Capability | Open-Source (OSS) | Enterprise Architecture |
| :--- | :--- | :--- |
| **Execution Identity** | Complete identity hierarchy (`ExecutionId`, `OperationId`, `IdempotencyKey`) | Multi-tenant tenant/workspace RBAC resolution |
| **Execution State Store** | Process-local in-memory store (`MokaExecutionStore`) | Distributed durable state backend (Redis / DB) |
| **Fencing & Versioning** | Monotonic lease versioning & stale completion fencing | Distributed lock leases |
| **Idempotency** | Canonical JSON SHA-256 idempotency key calculation | Shared multi-node idempotency store |
| **Behavior Protection** | Runaway loop & tool storm detection (`Agent Guardian`) | Fleet policy governance & incident replay |
| **MCP Execution** | Bounded SSE MCP tool transport (`buffer_unordered(10)`) | Distributed MCP proxy & connection pool |
| **Telemetry & Traces** | Ephemeral in-memory audit trace store | Analytical telemetry pipeline (ClickHouse / WAL) |

---

## 📚 Documentation

* **[Architecture Deep Dive](./docs/overview/architecture.md)**
* **[Getting Started Guide](./docs/getting-started/quickstart.mdx)**
* **[Operational & Configuration Guide](./docs/getting-started/operational-guide.md)**
* **[API Reference](./docs/api-reference/endpoints.md)**
* **[Docker Deployment Guide](./docs/getting-started/docker-setup.md)**

---

## 🤝 Community & Support

* **Discord**: [Join our Discord Community](https://discord.gg/uurgj9fMy8)
* **GitHub Issues**: Bug reports and feature tracking
* **GitHub Discussions**: Architecture Q&A and technical discussions

---

## 📄 License

This repository is licensed under the Apache License 2.0. See the [LICENSE](LICENSE) file for details.
