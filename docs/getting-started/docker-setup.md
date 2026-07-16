---
icon: "box"
---
# Docker & Container Deployment

Kryneth Gateway is packaged as a lightweight container available on Docker Hub for rapid testing and production-grade deployments.

---

## 📦 Container Registry Specifications

| Image | Tag | Description | Size |
| :--- | :--- | :--- | :--- |
| `krynethgw/kryneth-gateway` | `latest` | Multi-architecture production image | ~45MB |
| `krynethgw/kryneth-gateway` | `latest-slim` | Stripped Alpine binary | ~35MB |
| `krynethgw/kryneth-gateway` | `v0.1.0` | Pinned immutable version | ~45MB |

### Supported Architectures
-   `linux/amd64` (Intel/AMD x86_64)
-   `linux/arm64` (Apple Silicon, AWS Graviton)

---

## 🚀 Running the Container

### 1. Configure the Environment File
Create a `.env` file to govern the gateway's behavior and inject LLM credentials securely.

```ini
# .env
GATEWAY_PORT=8080
RUST_LOG=info
KRYNETH_VALID_KEYS=dev_secret_123

# Provider Keys
GROQ_API_KEY=gsk_...
OPENAI_API_KEY=sk_...
```

### 2. Configure model routing
Create a `routing.yaml` file mapping models to failover priorities:

```yaml
# routing.yaml
"00000000-0000-0000-0000-000000000000":
  "llama-3.3-70b-versatile":
    targets:
      - priority: 1
        provider_name: "groq"
        api_key_alias: "GROQ_API_KEY"
        base_url: "https://api.groq.com/openai/v1"
        target_model: "llama-3.3-70b-versatile"
        schema_format: "openai"
```

### 3. Run the Container
Mount your `routing.yaml` file into the container's execution context:

```bash
docker run -d \
  --name kryneth-gateway \
  -p 8080:8080 \
  --env-file .env \
  -v $(pwd)/routing.yaml:/app/routing.yaml:ro \
  krynethgw/kryneth-gateway:latest
```

> [!IMPORTANT]
> The `:ro` flag mounts the configuration file as read-only. This prevents the container process from mutating local filesystem assets.

---

## 🐳 Docker Compose Integration

Deploy Kryneth along with your application stack using Docker Compose.

```yaml
# docker-compose.yml
version: '3.8'

services:
  kryneth-gateway:
    image: krynethgw/kryneth-gateway:latest
    container_name: kryneth-gateway
    ports:
      - "8080:8080"
    environment:
      - GATEWAY_PORT=8080
      - RUST_LOG=info
      - KRYNETH_VALID_KEYS=${KRYNETH_VALID_KEYS}
      - GROQ_API_KEY=${GROQ_API_KEY}
    volumes:
      - ./routing.yaml:/app/routing.yaml:ro
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
      interval: 30s
      timeout: 10s
      retries: 3
    restart: unless-stopped
```

Deploy with:
```bash
docker-compose up -d
```
