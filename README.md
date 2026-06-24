# Kryneth: The Production Reliability Layer for AI Agents

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)](https://github.com/kryneth-admin/kryneth-oss)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Rust](https://img.shields.io/badge/Rust-1.77%2B-orange.svg)](https://www.rust-lang.org/)
[![Docker](https://img.shields.io/badge/Docker-supported-blue.svg)](https://www.docker.com/)
[![Discord](https://img.shields.io/discord/1234567890?color=5865F2&logo=discord&logoColor=white&label=Discord)](https://discord.gg/uurgj9fMy8)

> **Agents don't fail loudly. They fail silently, burn money, and nobody notices until production breaks.**
>
> Kryneth is the runtime control plane that stops runaway loops, unsafe tool execution, and uncontrolled AI spend before they hit production. 
> Built for LangGraph, CrewAI, AutoGen, Claude Code, OpenAI Agents SDK, and MCP workflows.

<div align="center">
  <img src="./docs/quickstart_demo.gif" alt="Kryneth Gateway Zero-to-Hero Setup" width="100%">
</div>

## ⚡ Zero-to-Hero Quick Start

Drop Kryneth into your infrastructure in under 60 seconds.

```bash
# 1. Boot the gateway
docker run -d -p 8080:8080 crossroot/kryneth-gateway:latest

# 2. Point your existing OpenAI SDK to localhost
export OPENAI_BASE_URL="http://localhost:8080/v1"
```

---

## 🚫 Why a Runtime Control Plane?

Existing LLM API gateways solve yesterday's problems (Routing, API Keys, Rate Limits). Traditional gateways manage *requests*. Kryneth manages *agent behavior*.

Gateways answer: **"Where should this request go?"**<br>
Kryneth answers: **"Should this action happen at all?"**

Production AI teams deploying autonomous agents are currently defenseless against:
**✗ Silent agent failures**<br>
**✗ Infinite reasoning loops**<br>
**✗ Tool storms**<br>
**✗ MCP over-permission**<br>
**✗ Cost explosions**

---

## 🚨 Real Production Incidents We Stop

If you are building autonomous agents, you have likely experienced:
* An agent calling the same tool 500 times in a row.
* An agent consuming $120 overnight on a background task.
* An MCP server returning malformed data that crashes the reasoning engine.
* An OpenAI outage causing an entire multi-agent workflow to fail.

Kryneth was designed specifically to stop these exact classes of incidents.

---

## 👥 Who Uses Kryneth

* **AI SaaS Teams:** Protecting production margins and uptime.
* **LangGraph & CrewAI Developers:** Governing multi-agent workflows.
* **MCP Builders:** Securing tool execution and schemas.
* **Claude Code Users:** Safely integrating local coding agents.
* **AI Automation Agencies:** Guaranteeing client budget limits.
* **Enterprise AI Teams:** Enforcing PII compliance and audit trails.

---

## 🛡️ Production Failures Kryneth Prevents

### Scenario 1: The Infinite Reasoning Loop
*An agent gets stuck retrying the same MCP tool 50 times.*
* **Without Kryneth:** $300 bill and a dead workflow.
* **With Kryneth:** Blocked instantly after 5 identical parameter signatures.

### Scenario 2: Data Exfiltration
*A user prompt or agent context contains a PAN or Aadhaar card.*
* **Without Kryneth:** PII leakage to public LLMs.
* **With Kryneth:** The compliance engine strips the PII locally before it leaves your infrastructure.

### Scenario 3: Provider Outage
*OpenAI returns a `429 Rate Limit` or `500 Error`.*
* **Without Kryneth:** Application fails and user sees an error.
* **With Kryneth:** Fallback route (e.g., Anthropic) executes automatically and transparently.

---

## 🎯 Features: Outcomes Over Infrastructure

| Production Problem | Kryneth Solution |
| :--- | :--- |
| **Agent loops burn money** | Runaway Loop Protection |
| **Tool storms** | Tool Governance |
| **MCP over-permission** | Policy Enforcement |
| **PII leakage** | PII Protection |
| **Cost explosions** | Budget Controls |
| **Silent failures** | Audit + Trace Foundation |

---

## 🎥 Live Terminal Demos

### 1. Agentic Runaway Loop Trap
Watch Kryneth block an agent stuck in a recursive loop executing the same tool over and over:

![Agent Loop Blocked Demo](docs/agent_loop_blocked.gif)

### 2. Circuit Breaker & Automatic LLM Hot-Swaps
Watch Kryneth's circuit breaker catch a 429 Rate Limit and instantly hot-swap to Anthropic:

![LLM Hot-Swap Demo](docs/llm_hotswap.gif)

---

## 🏗️ Architecture & Traffic Flow

```mermaid
graph TD
    %% Custom styling definition
    classDef framework fill:#1E293B,stroke:#3B82F6,stroke-width:2px,color:#F8FAFC
    classDef ingress fill:#0F172A,stroke:#64748B,stroke-width:2px,color:#F8FAFC
    classDef security fill:#450A0A,stroke:#EF4444,stroke-width:2px,color:#F8FAFC
    classDef routing fill:#064E3B,stroke:#10B981,stroke-width:2px,color:#F8FAFC
    classDef upstream fill:#78350F,stroke:#F59E0B,stroke-width:2px,color:#F8FAFC
    
    subgraph ClientLayer ["Agentic Client Layer"]
        Agent["AI Agent Frameworks<br>(LangGraph, CrewAI, AutoGen, MCP)"]:::framework
    end
    
    subgraph Firewall ["Kryneth Runtime Control Plane"]
        Ingress["Kryneth Ingress<br/>(Axum HTTP & SSE Stream Router)"]:::ingress
        
        subgraph Guardian ["Reliability & Protection Layer"]
            LoopTrap["Runaway Loop Protection<br/>(ahash signature trap)"]:::security
            PII["PII Protection<br/>(Regex Entity Filters)"]:::security
        end
        
        subgraph RouteLayer ["Routing Layer (DX & Efficiency)"]
            L1Cache["L1 FastEmbed Cache<br/>(In-Memory Moka / DashMap)"]:::routing
            CircuitBreaker["Circuit Breaker & Fallbacks<br/>(Automatic hot-swaps)"]:::routing
            MCPTunnel["MCP Server Tunnel<br/>(SSE Tool Routing)"]:::routing
        end
    end
    
    subgraph Providers ["Upstream LLM Layer"]
        OpenAI["OpenAI / Anthropic / Gemini / Groq"]:::upstream
    end

    subgraph External ["Local / Remote Resources"]
        MCPServer["MCP Servers<br/>(Databases, Internal APIs)"]:::upstream
    end

    %% Flow connections
    Agent -->|HTTP Requests / SSE Streams| Ingress
    Ingress --> LoopTrap
    LoopTrap -->|Pass| PII
    PII --> L1Cache
    
    L1Cache -->|Cache Miss| CircuitBreaker
    CircuitBreaker -->|Route Call| OpenAI
    MCPTunnel -->|Execute Tool| MCPServer
    MCPServer -->|Return Data| MCPTunnel
    
    %% Fast responses
    L1Cache -->|Cache Hit - Fast Path 1.4ms| Ingress
    LoopTrap -.->|Block Loop - 429 Response| Ingress
    
    %% Output
    OpenAI --> Ingress
    MCPTunnel --> Ingress
    Ingress --> Agent
```

---

## ⚡ Quickstart in 60 Seconds

> 🏎️ **Performance Note:** Kryneth is built entirely in memory-safe Rust with `bumpalo` and `simd-json`. It adds virtually zero latency overhead (**P90 latency ~2.1ms**) between your agent and the LLM.

### One-Command Setup

**Linux/macOS:**
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

> ✨ The setup script automatically:
> - ✓ Copies `.env` and `routing.yaml` from templates
> - ✓ Validates prerequisites (Docker, Rust, Git)
> - ✓ Checks your API key configuration
> - ✓ Shows deployment options

---

### Step 1: Docker Compose (Recommended)
Run Kryneth on port `8080` instantly:

```bash
docker-compose up -d --build
```

**Available at:** `http://localhost:8080`

> [!NOTE]
> Alternative deployment options:
> 
> **Option A: Local Development**
> ```bash
> cargo run --release
> ```
>
> **Option B: Raw Docker**
> ```bash
> docker build -t kryneth-gateway:latest .
> docker run -d --name kryneth-gateway \
>   -p 8080:8080 \
>   -v $(pwd)/routing.yaml:/app/routing.yaml \
>   --env-file .env \
>   kryneth-gateway:latest
> ```
>
> **Option C: Docker Hub (Production)**
> ```bash
> docker pull krynethgw/kryneth-gateway:latest
> docker run -d --name kryneth-gateway \
>   -p 8080:8080 \
>   -v $(pwd)/routing.yaml:/app/routing.yaml \
>   --env-file .env \
>   krynethgw/kryneth-gateway:latest
> ```
> See [Docker Hub Deployment Guide](./docs/DOCKER_HUB.md) for details.

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

## 🏛️ Open-Core Transparency

Kryneth uses a strict open-core model. Users don't buy infrastructure components—they buy outcomes.

| Capability | Open-Source (OSS) | Enterprise Edition |
| :--- | :--- | :--- |
| **Agent Loops** | Loop Detection | Incident Replay & Diagnostics |
| **Tool Execution** | Tool Storm Detection | Policy Studio & Fleet Governance |
| **MCP Security** | Basic MCP Governance | MCP Compliance Packs |
| **Visibility** | Basic Audit & JSON Logs | Reliability Analytics & Dashboards |
| **Budgeting** | Basic Cost Tracking | Budget Kill-Switches & Quotas |

> **OSS Choice:** Best suited for local agent setups, single-node proxies, and developer environments.
> **Enterprise Choice:** Tailored for production scale and compliance-sensitive enterprises requiring real-time analytical dashboards.

---

## 📚 Documentation & Resources

Complete guides for every use case:

| Guide | Purpose | Best For |
|-------|---------|----------|
| **[Getting Started](./docs/GETTING_STARTED.mdx)** | Installation & first request | New users, developers |
| **[Configuration Reference](./docs/CONFIGURATION.md)** | All settings explained | Advanced setup, tuning |
| **[Docker Hub Deployment](./docs/DOCKER_HUB.md)** | Cloud & production deployments | AWS, GCP, Kubernetes, ECS |
| **[API Reference](./docs/API.md)** | Full API endpoint documentation | Integration & SDK building |
| **[Troubleshooting](./docs/TROUBLESHOOTING.md)** | Common issues & solutions | Debugging problems |
| **[Architecture Deep Dive](./docs/ARCHITECTURE.md)** | System design & internals | Contributors, advanced users |

### Setup Helpers

- **[setup.sh](./setup.sh)** - Automated setup for Linux/macOS
- **[setup.ps1](./setup.ps1)** - Automated setup for Windows PowerShell
- **[.env.example](./env.example)** - Heavily commented configuration template
- **[routing.yaml.example](./routing.yaml.example)** - Provider routing examples

### Quick Reference

```bash
# One-command setup (Linux/macOS)
bash setup.sh

# One-command setup (Windows)
.\setup.ps1

# Docker Compose (recommended)
docker-compose up -d --build

# Local development
cargo run --release

# Docker Hub (production)
docker run -p 8080:8080 --env-file .env -v $(pwd)/routing.yaml:/app/routing.yaml krynethgw/kryneth-gateway:latest
```

---

## 🤝 Community & Support

Building autonomous agents is hard. Let's figure it out together.

* **[Join our Discord](https://discord.gg/uurgj9fMy8)** to chat with other engineers building production AI, share MCP tool ideas, and get direct help from the maintainers.
* **GitHub Issues:** For bug reports and feature requests.
* **GitHub Discussions:** For architectural questions and Q&A.

---

## 📄 License

This project is licensed under the Apache License 2.0. See the [LICENSE](LICENSE) file for details.