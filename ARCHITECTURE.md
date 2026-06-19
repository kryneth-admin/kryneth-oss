# Kryneth Gateway Architecture

## Overview
Kryneth is a blazingly fast, ultra-low latency Rust service built with Axum. It evolved from a simple reverse proxy into an **Agent Runtime Control Plane**. It sits between your AI Agents and upstream LLM providers to orchestrate security, loop prevention, semantic caching, and strict budget enforcement at the edge.

## Ports & Adapters (Clean Architecture)

The codebase strictly adheres to the **Ports & Adapters** (Hexagonal Architecture) pattern. Core domain logic and use cases (e.g., `behavior_guard`, `tool_router`) rely entirely on abstract traits ("Ports").
Concrete implementations ("Adapters") are injected at runtime, enabling Kryneth to operate in two distinct modes via an Open-Core strategy.

This open-source repository uses strictly in-memory adapters (`OssAuth`, `OssRateLimit`) to guarantee frictionless local development without external database dependencies.

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
