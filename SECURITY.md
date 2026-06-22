# Security Policy

At Kryneth, security is not an afterthought—it is the foundational purpose of the product. Kryneth acts as a **Runtime Control Plane and Firewall** to protect enterprise infrastructure, API budgets, and data privacy from non-deterministic autonomous agents.

## Our Security Purpose

Autonomous AI agents (via LangGraph, CrewAI, MCP, etc.) introduce a new class of cybersecurity risks that traditional API gateways cannot mitigate. Kryneth is specifically designed to enforce security boundaries around agentic workflows:

1. **Agentic Runaway Loop Protection:** We actively trap recursive loop behaviors (tool storms) using zero-allocation signature hashing to prevent Denial of Wallet (DoW) and accidental self-inflicted DDoS attacks against upstream providers.
2. **Data Exfiltration & PII Protection:** Our pipeline inspects payloads at the edge, actively identifying and redacting Personally Identifiable Information (PII) like PAN cards, SSNs, and credit cards *before* they leave your infrastructure and reach public LLMs.
3. **MCP Governance (Model Context Protocol):** We enforce strict schema validation, ensuring agents cannot exploit over-permissioned tools or inject malicious payloads into local MCP servers.
4. **Memory Safety by Default:** The entire Kryneth control plane is written in Rust, eliminating entire classes of memory vulnerabilities (buffer overflows, use-after-free) present in C/C++ proxies.

## Supported Versions

We apply critical security patches to the following versions of Kryneth. Please ensure you are running a supported version in production.

| Version | Supported          |
| ------- | ------------------ |
| 1.0.x   | :white_check_mark: |
| < 1.0   | :x:                |

## Reporting a Vulnerability

We take all security vulnerabilities seriously. If you discover a vulnerability in Kryneth, we ask that you report it to us privately so we can patch it before it is exploited.

**Do not open a public GitHub issue for security vulnerabilities.**

### What to include in your report:
* Description of the vulnerability.
* Steps to reproduce the issue (including any malicious payload or `.tape` files).
* Potential impact on the Kryneth gateway or upstream LLMs.
* Your contact information.

### Response Timeline
* We will acknowledge receipt of your vulnerability report within **48 hours**.
* We will provide a status update and an estimated patch timeline within **7 days**.
* Once the patch is merged, we will issue a security advisory and notify you.

## Best Practices for Production

To maximize the security of your Kryneth deployment, we recommend the following:
* **Never commit `.env` or `routing.yaml` files** containing real API keys or JWT secrets.
* **Keep `RATE_LIMIT_MAX_REQUESTS` strictly configured** to prevent unexpected traffic spikes from rogue client scripts.
* **Always run Kryneth behind a TLS-terminating reverse proxy** (e.g., Nginx, Traefik, or AWS ALB) when exposing it to the public internet, as Kryneth OSS HTTP handles traffic in plaintext.
* **Restrict local auth bypass:** Ensure the zero-config local development bypass is completely disabled or firewalled in production environments.
