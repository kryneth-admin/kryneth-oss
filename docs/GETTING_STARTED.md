# Getting Started with Kryneth Gateway

## Prerequisites

- **Rust:** 1.70 or later ([Install Rust](https://rustup.rs/))
- **Cargo:** Included with Rust
- **API Keys:** 
  - Groq API key (https://console.groq.com)
  - Cohere API key (https://dashboard.cohere.com) - for failover
  - Or other provider keys (OpenAI, Anthropic, Gemini)
- **Git:** For cloning the repository

## Installation

### 1. Clone the Repository
```bash
git clone https://github.com/kryneth/Kryneth-Gateway-OSS.git
cd Kryneth-Gateway-OSS
```

### 2. Configure Routing
Edit `routing.yaml` at the project root with your tenant ID and provider configuration:

```yaml
"00000000-0000-0000-0000-000000000000":  # Your tenant ID
  "llama-3.3-70b-versatile":              # Virtual model alias
    targets:
      - priority: 1
        api_key_alias: "GROQ_API_KEY"
        provider_name: "groq"
        base_url: "https://api.groq.com/openai/v1"
        target_model: "llama-3.3-70b-versatile"
        schema_format: "openai"
      
      - priority: 2
        api_key_alias: "COHERE_API_KEY"
        provider_name: "cohere"
        base_url: "https://api.cohere.com/v1"
        target_model: "command-r-plus-08-2024"
        schema_format: "openai"
```

### 3. Set Environment Variables
Create a `.env` file or export the following variables.

```bash
# Gateway Auth
export KRYNETH_VALID_KEYS="dev_secret_123,another_secret_456"
export JWT_SECRET="dummy-secret-for-local-dev"

# MCP Integrations
export MCP_TOOL_REGISTRY='{"jira_search": "http://localhost:9000/sse"}'
export MCP_TOOL_SCHEMA_REGISTRY='[{"name":"jira_search","summary":"Search Jira tickets"}]'

# Upstream API credentials
export GROQ_API_KEY="gsk_your_groq_key_here"
export COHERE_API_KEY="h28_your_cohere_key_here"

# Configure rate limiting (optional)
export RATE_LIMIT_MAX_REQUESTS=60
export RATE_LIMIT_WINDOW_SECS=60

# Configure agent safety guards (optional)
export MAX_SESSION_TOOL_CALLS=20       # Max tools per 60s
export MAX_IDENTICAL_TOOL_CALLS=5      # Max identical signatures
```

### 4. Run Locally (Development)
```bash
# Standard build with all features
cargo run --release

```

**Output:**
```
2026-05-31T10:30:45.123Z INFO  Kryneth Gateway starting...
2026-05-31T10:30:45.456Z INFO  Listening on http://0.0.0.0:8080
```

Gateway is now accessible at `http://localhost:8080`

## Docker Deployment

### Build Image
```bash
docker build -t kryneth-gateway:latest .
```

### Run Container
```bash
docker run -p 8080:8080 \
  -e GROQ_API_KEY="gsk_..." \
  -e COHERE_API_KEY="h28_..." \
  -e MAX_SESSION_TOOL_CALLS=20 \
  -e MAX_IDENTICAL_TOOL_CALLS=5 \
  -v $(pwd)/routing.yaml:/app/routing.yaml \
  kryneth-gateway:latest
```

### Run with .env File
```bash
docker run -p 8080:8080 \
  --env-file .env \
  -v $(pwd)/routing.yaml:/app/routing.yaml \
  kryneth-gateway:latest
```

## Quick Test

### 1. Create a Test Request
```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "llama-3.3-70b-versatile",
    "messages": [
      {
        "role": "user",
        "content": "What is 2+2?"
      }
    ]
  }'
```

### 2. Expected Response
```json
{
  "id": "chatcmpl-xxxxx",
  "object": "chat.completion",
  "created": 1234567890,
  "model": "llama-3.3-70b-versatile",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "2 + 2 = 4"
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 10,
    "completion_tokens": 5,
    "total_tokens": 15
  }
}
```

## Testing Suite

### Run All Tests
```bash
cargo test --all
```

### Run Integration Tests
```bash
cargo test --test '*' -- --nocapture
```

### Run Specific Test Suite
```bash
# API integration tests
cargo test --test api_integration_test

# Billing engine tests
cargo test --test billing_engine_test

# Redis integration
cargo test --test integration_redis

# Chaos testing
cargo test --test chaos_test
```

### Run Examples
```bash
# Stress test (high concurrency)
cargo run --example stress_test

# SIMD performance benchmark
cargo run --example simd_test
```

## Project Structure

```
Kryneth-Gateway-OSS/
├── src/                    # Rust source code
│   ├── main.rs            # Entry point
│   ├── api/               # HTTP handlers
│   ├── domain/            # Business logic
│   ├── infrastructure/    # External integrations
│   └── usecases/          # Orchestration
├── tests/                 # Integration tests
├── examples/              # Example programs
├── migrations/            # Database migrations
├── docs/                  # Documentation
├── routing.yaml           # Provider routing config
├── Cargo.toml            # Dependency manifest
├── Dockerfile            # Container image
└── README.md             # Project overview
```

## Configuration Reference

### Core Environment Variables
```bash
# Logging
RUST_LOG=info              # Log level: trace, debug, info, warn, error

# Server
GATEWAY_PORT=8080          # Port to listen on

# Security & Auth
KRYNETH_VALID_KEYS=secret  # Comma-separated list of valid API keys for OSS auth
JWT_SECRET=secret          # JWT secret used for session tokens

# MCP Integration
MCP_TOOL_REGISTRY='{"tool":"http://mcp/sse"}' # JSON object mapping tool names to SSE endpoints
MCP_TOOL_SCHEMA_REGISTRY='[{"name":"tool"}]'  # JSON array of tool descriptors for Lazy Schema

# Rate Limiting
RATE_LIMIT_MAX_REQUESTS=60 # Requests per window
RATE_LIMIT_WINDOW_SECS=60  # Window duration in seconds

# Agent Safety
MAX_SESSION_TOOL_CALLS=20      # Max tools per 60s window
MAX_IDENTICAL_TOOL_CALLS=5     # Max identical tool signatures
SANDBOX_FALLBACK_MODE=closed   # Set to 'open' to fail-open on OPA downtime
```

## Troubleshooting

### Port Already in Use
```bash
# Find process using port 8080
lsof -i :8080
# Kill the process
kill -9 <PID>
```

### API Key Errors
```bash
# Verify environment variables are set
echo $GROQ_API_KEY
echo $COHERE_API_KEY

# Check routing.yaml has correct api_key_alias
cat routing.yaml
```

### Build Errors
```bash
# Update Rust
rustup update

# Clean build
cargo clean
cargo build --release

# Check dependencies
cargo tree
```

### High Latency
```bash
# Enable debug logging
RUST_LOG=debug cargo run --release

# Check system resources
top  # or Task Manager on Windows

# Run benchmark
cargo run --example stress_test
```

## Next Steps

1. **Read the Architecture Guide** → [ARCHITECTURE.md](ARCHITECTURE.md)
2. **API Documentation** → [API.md](API.md)
3. **Configuration Reference** → [CONFIGURATION.md](CONFIGURATION.md)
4. **Troubleshooting** → [TROUBLESHOOTING.md](TROUBLESHOOTING.md)

## Community & Support

- **Issues:** GitHub Issues for bug reports
- **Discussions:** GitHub Discussions for questions
- **Docs:** Check [docs/](docs/) for comprehensive guides
