# Kryneth Gateway Architecture

## Overview
Kryneth is a blazingly fast, ultra-low latency Rust service built with Axum. It evolved from a simple reverse proxy into an **Agent Runtime Control Plane**. It sits between your AI Agents and upstream LLM providers to orchestrate security, loop prevention, semantic caching, and strict budget enforcement at the edge.

Kryneth operates on a **Dual Architecture**:
- **OSS Edition**: Runs purely in-memory using lock-free local caches (`Moka` & `DashMap`) and SIMD-accelerated JSON parsing for zero-dependency deployments.
- **Enterprise Edition**: Seamlessly transitions to distributed state management using Redis, ClickHouse, and PostgreSQL for multi-node Kubernetes deployments.

## High-Level Design

```mermaid
graph TD
    Client((AI Agent)) --> Ingress[Ingress & Middleware]
    
    subgraph Kryneth Agent Control Plane
        Ingress --> AgentGuard[Agent Guardian: Loop & Storm Guard]
        AgentGuard -->|429 Block| Reject((Reject Runaway))
        
        AgentGuard --> PII[Pre-Flight Redaction]
        PII --> L1[L1 Semantic/Exact Cache]
        
        L1 -->|Cache Hit| Outgress[Response Formatter]
        
        L1 -->|Cache Miss| Router[LLM Router & Circuit Breaker]
        Router -->|Fallback| Router
        
        Router --> Upstream((OpenAI / Anthropic / Gemini))
        
        Upstream --> MCPSandbox[MCP Sandbox & Tool Firewall]
        MCPSandbox --> Outgress
    end
    
    Outgress --> Client
    Outgress -.-> Telemetry[Async Telemetry Worker]
    
    subgraph Dual Infrastructure
        Telemetry -->|OSS Edition| Local[Local Logs & DashMap]
        Telemetry -->|Enterprise| ClickHouse[(ClickHouse & Redis)]
    end

## Technical Stack
- **Framework**: Axum (Tokio-based)
