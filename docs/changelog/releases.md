---
icon: "sparkles"
---
# Version Release History

Track additions, changes, and safety updates introduced across Kryneth Gateway releases.

---

## [v0.1.1] — Multi-Architecture & Automation Update

### Added
-   **Automated Multi-Arch Builds**: Fully automated GitHub Actions CI/CD pipeline to deploy Docker images for `linux/amd64` and `linux/arm64`.
-   **Docker Hub Integration**: Verified, production-ready images now continuously published to `kryenthhq/krynethgw:latest`.
-   **Improved Deployment Docs**: Clarified Docker configuration instructions to point to the new automated builds.

## [v0.1.0] — Initial Release

### Added
-   **Stateless L7 HTTP Proxying**: Complete translation support across OpenAI, Anthropic, and Google Gemini schema message structures.
-   **Agentic Behavior Guard**: First-generation hashing-based signature traps to block infinite agent tool loops, and session tool counts.
-   **Hybrid Vector Cache**: L1 in-memory cache indexing powered by `Moka` and `DashMap`.
-   **SSE Model Context Protocol Tunneling**: Tunnel 3 routing, mapping LLM tools to Server-Sent Event (SSE) servers.
-   **Multi-tenant JWT Auth**: Isolated routing, budgets, and rate-limiting mappings.
-   **Asynchronous ClickHouse Telemetry**: Decoupled non-blocking worker threads flushing traces asynchronously.
