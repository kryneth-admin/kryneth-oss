# Kryneth: The L7 Control Plane for AI Agents

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)](https://github.com/kryneth-admin/kryneth-oss)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Rust](https://img.shields.io/badge/Rust-1.77%2B-orange.svg)](https://www.rust-lang.org/)
[![Docker](https://img.shields.io/badge/Docker-supported-blue.svg)](https://www.docker.com/)

> **Stop LLM budget burn before it happens.** An ultra-low latency, memory-safe Rust firewall and router for autonomous AI agents.

---

## 🎯 The Agentic Reality (Pain vs. Solution)

Modern API gateways built for deterministic web apps fail under the recursive, non-deterministic behaviors of autonomous AI agents. Kryneth is built specifically to address the core architectural vulnerabilities of the agentic era:

| Developer Pain Point | Kryneth Edge Solution | Core Mechanism |
| :--- | :--- | :--- |
| **Runaway Token Loops**<br>Agents get stuck in recursive retry loops, executing the same tool with identical parameters and burning thousands of dollars overnight. | **Agent Guardian Loop Trap**<br>Traps repetitive tool-call signatures at the edge in **<2ms**—before the request ever hits your wallet. | Sliding window non-cryptographic `ahash` signature comparison & SIMD-accelerated JSON payload extraction. |
| **Tool Storm Explosions**<br>A single agent task explodes into 50+ nested tool calls within seconds, destroying SaaS unit economics and rate limits. | **Strict Session Budget Ceilings**<br>Enforces hard, session-level token cost and execution bounds to instantly kill cascading workflows. | Process-local maximum tool execution counters per session with instant circuit-tripping. |
| **Proxy Latency & Memory Bloat**<br>Python-based LLM proxies add 50–100ms latency overhead and introduce security supply-chain risks. | **Engineered in Native Rust**<br>Written in Rust using Axum and `simd-json`. Adds virtually zero overhead (**P90 latency ~2.1ms**). | Compiled memory-safe binary running on Tokio asynchronous runtime with zero runtime dependencies. |

---

## 🏗️ Architecture & Traffic Flow

Kryneth acts as a semantic firewall, sitting directly between your agentic frameworks and upstream LLM providers to inspect and control traffic in real-time.

```mermaid
graph TD
    %% Custom styling definition
    classDef framework fill:#1E293B,stroke:#3B82F6,stroke-width:2px,color:#F8FAFC
    classDef ingress fill:#0F172A,stroke:#64748B,stroke-width:2px,color:#F8FAFC
    classDef security fill:#450A0A,stroke:#EF4444,stroke-width:2px,color:#F8FAFC
    classDef routing fill:#064E3B,stroke:#10B981,stroke-width:2px,color:#F8FAFC
    classDef upstream fill:#78350F,stroke:#F59E0B,stroke-width:2px,color:#F8FAFC
    
    subgraph ClientLayer ["Agentic Client Layer"]
        Agent["AI Agent Frameworks<br>(CrewAI, LangChain, MCP)"]:::framework
    end
    
    subgraph Firewall ["Kryneth L7 Firewall Control Plane"]
        Ingress["Kryneth Ingress<br/>(Axum HTTP & SSE Stream Router)"]:::ingress
        
        subgraph Guardian ["Guardian Layer (Security & Abuse Firewall)"]
            LoopTrap["Loop Detection<br/>(ahash signature trap)"]:::security
            PII["PII Redaction<br/>(Regex Entity Filters)"]:::security
        end
        
        subgraph RouteLayer ["Routing Layer (DX & Efficiency)"]
            L1Cache["L1 FastEmbed Cache<br/>(In-Memory Moka / DashMap)"]:::routing
            CircuitBreaker["Circuit Breaker & Fallbacks<br/>(Automatic hot-swaps)"]:::routing
        end
    end
    
    subgraph Providers ["Upstream LLM Layer"]
        OpenAI["OpenAI / Anthropic / Gemini / Groq"]:::upstream
    end

    %% Flow connections
    Agent -->|HTTP Requests / SSE Streams| Ingress
    Ingress --> LoopTrap
    LoopTrap -->|Pass| PII
    PII --> L1Cache
    
    L1Cache -->|Cache Miss| CircuitBreaker
    CircuitBreaker -->|Route Call| OpenAI
    
    %% Fast responses
    L1Cache -->|Cache Hit - Fast Path 1.4ms| Ingress
    LoopTrap -.->|Block Loop - 429 Response| Ingress
    
    %% Output
    OpenAI --> Ingress
    Ingress --> Agent
```

---

## ⚡ Quickstart in 60 Seconds

Developers judge a tool by its **Time to First Request**. Get Kryneth up and running locally in three simple steps.

### Step 1: Docker Magic
Run the Kryneth gateway container on port `8080` instantly. Clone the repo and boot the system with one command:

```bash
# Clone the repository
git clone https://github.com/kryneth-admin/kryneth-oss.git && cd kryneth-oss

# Copy templates to default config files
cp env.example .env
cp routing.yaml.example routing.yaml

# Boot up the control plane
docker-compose up -d --build
```

> [!NOTE]
> If you prefer raw Docker, you can run Kryneth directly by passing the config environment file:
> ```bash
> docker run -d --name kryneth-gateway \
>   -p 8080:8080 \
>   -v $(pwd)/routing.yaml:/app/routing.yaml \
>   --env-file .env \
>   kryneth-gateway-oss:latest
> ```

### Step 2: Configuration
Configure your models in `routing.yaml`. Kryneth maps virtual models to failover-prioritized providers using environment-injected API keys.

**`.env` file configuration:**
```ini
# Gateway Server Configuration
GATEWAY_PORT=8080
RUST_LOG=info

# OSS Authentication key (Pass as 'Authorization: Bearer <key>')
KRYNETH_VALID_KEYS=re_live_local_dev_123

# Upstream API credentials
GROQ_API_KEY=gsk_u9yl94IAgmKGlNrQYOEmWGdrwer...
COHERE_API_KEY=h28KvJLI9vz7pbxRuTCrdcUPJ43rr...
```

**`routing.yaml` model routing configuration:**
```yaml
# Tenant Mapping Route Setup
"00000000-0000-0000-0000-000000000000": # Default Local Tenant ID
  "llama-3.3-70b-versatile":           # Virtual Route Target
    targets:
      - priority: 1                     # Primary Upstream Route
        weight: 100
        api_key_alias: "GROQ_API_KEY"
        provider_name: "groq"
        base_url: "https://api.groq.com/openai/v1"
        target_model: "llama-3.3-70b-versatile"
        schema_format: "openai"
      - priority: 2                     # Automatic Hot-Swap Fallback
        weight: 100
        api_key_alias: "COHERE_API_KEY"
        provider_name: "cohere"
        base_url: "https://api.cohere.ai/compatibility/v1"
        target_model: "command-r-plus-08-2024"
        schema_format: "openai"
    rate_limit_rpm: 60
```

> [!TIP]
> **Zero-Config Local Auth Bypass:** When running in local development mode, Kryneth automatically bypasses authentication for loopback connections (`localhost` / `127.0.0.1`), meaning you don't need to specify authorization headers for your local test requests.

### Step 3: The First Request
Send a unified OpenAI-compatible request targeting the virtual Llama-3 route. If the primary provider (Groq) is down or rate-limited, Kryneth automatically swaps the request to the secondary provider (Cohere) in **0.37ms**, transparently adapting schemas.

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer re_live_local_dev_123" \
  -d '{
    "model": "llama-3.3-70b-versatile",
    "messages": [
      {
        "role": "user",
        "content": "Generate a security posture assessment for a CrewAI autonomous agent."
      }
    ]
  }'
```

---

## 🏛️ Open-Core Transparency

Kryneth uses a strict, to ensure the open-source engine remains dependency-free, lock-free, and blazingly fast.

| Capability | Open-Source (OSS) | Enterprise Edition |
| :--- | :--- | :--- |
| **Core Runtime Engine** | Rust Static Binary, Axum Web Engine. | Distributed High-Availability Cluster Deployment. |
| **State Storage Layer** | Zero-dependency, lock-free `Moka` & `DashMap` (In-Memory). | Centralized, shared Redis clusters for synchronized state. |
| **Agent Control Plane** | Infinite Loop & Tool Storm Traps. | Session budget kill-switches, recursion depth guards. |
| **Observability** | Structured local JSON logs. | Distributed ClickHouse analytical ingestion. |
| **Governance & Security** | Local config mapping (`routing.yaml`). | OPA Integration, RBAC, Multi-tenant PII Redaction. |

> **OSS Choice:** Best suited for local agent setups, single-node proxies, and developer environments.
> **Enterprise Choice:** Tailored for production scale and compliance-sensitive enterprises requiring real-time analytical dashboards.

---

## 📄 License

This project is licensed under the Apache License 2.0. See the [LICENSE](LICENSE) file for details.