# Configuration Guide

Kryneth Gateway can be configured through environment variables, `routing.yaml`, and feature flags.

## Environment Variables

### Server Configuration
```bash
# Server port
KRYNETH_PORT=8080

# Server host
KRYNETH_HOST=0.0.0.0

# Logging level (trace, debug, info, warn, error)
RUST_LOG=info
```

### Rate Limiting
Controls request rate limiting per tenant per window.

```bash
# Maximum requests allowed per window
RATE_LIMIT_MAX_REQUESTS=60

# Time window in seconds
RATE_LIMIT_WINDOW_SECS=60

# Example: 60 requests per 60 seconds = 1 req/sec sustained
```

### Agent Safety Guards
Prevents runaway agents and tool explosions.

```bash
# Maximum tool call executions within 60-second window
# Prevents "tool storms" - cascading tool calls that drain budgets
MAX_SESSION_TOOL_CALLS=20

# Maximum identical tool signatures within 60-second window
# Prevents infinite loops where agent repeats same tool with same arguments
MAX_IDENTICAL_TOOL_CALLS=5

# Example flow:
# 1. Agent calls get_weather(location="NYC") - Count: 1
# 2. Agent calls get_weather(location="NYC") - Count: 2
# 3. Agent calls get_weather(location="NYC") - Count: 3
# ...
# 6. Agent calls get_weather(location="NYC") - Count: 6 → BLOCKED (exceeds limit)
```

### Enterprise Features
```bash
# Enable enterprise capabilities (Redis, billing, compliance)
ENABLE_ENTERPRISE=false

# Redis connection string (for enterprise clustering)
REDIS_URL=redis://localhost:6379

# ClickHouse logging endpoint (for enterprise observability)
CLICKHOUSE_URL=http://localhost:8123

# Billing configuration
BILLING_ENABLED=false
BILLING_API_URL=https://billing.example.com
```

### Performance Tuning
```bash
# Tokio worker threads (defaults to CPU count)
TOKIO_WORKER_THREADS=8

# Max concurrent connections
MAX_CONNECTIONS=1000

# Cache expiration in seconds
CACHE_TTL_SECS=60

# L1 cache size (number of entries)
L1_CACHE_SIZE=10000
```

## routing.yaml Configuration

### File Location
Place `routing.yaml` at the project root:
```
Kryneth-Gateway-OSS/
├── routing.yaml      ← Configuration file
├── src/
├── Cargo.toml
└── ...
```

### Basic Structure
```yaml
# Top-level: Tenant ID (UUID format)
"00000000-0000-0000-0000-000000000000":
  
  # Model level: Virtual model name (what your app requests)
  "llama-3.3-70b-versatile":
    
    # Targets: Provider failover chain
    targets:
      # Primary provider
      - priority: 1
        provider_name: "groq"
        api_key_alias: "GROQ_API_KEY"
        base_url: "https://api.groq.com/openai/v1"
        target_model: "llama-3.3-70b-versatile"
        schema_format: "openai"
      
      # Secondary provider (fallback)
      - priority: 2
        provider_name: "cohere"
        api_key_alias: "COHERE_API_KEY"
        base_url: "https://api.cohere.com/v1"
        target_model: "command-r-plus-08-2024"
        schema_format: "openai"
```

### Complete Example with Multiple Tenants

```yaml
# Production tenant
"prod-tenant-12345":
  "gpt-4-turbo":
    targets:
      - priority: 1
        provider_name: "openai"
        api_key_alias: "OPENAI_API_KEY"
        base_url: "https://api.openai.com/v1"
        target_model: "gpt-4-turbo"
        schema_format: "openai"
  
  "claude-3-opus":
    targets:
      - priority: 1
        provider_name: "anthropic"
        api_key_alias: "ANTHROPIC_API_KEY"
        base_url: "https://api.anthropic.com"
        target_model: "claude-3-opus-20240229"
        schema_format: "anthropic"

# Development tenant
"dev-tenant-67890":
  "local-testing":
    targets:
      - priority: 1
        provider_name: "groq"
        api_key_alias: "GROQ_API_KEY"
        base_url: "https://api.groq.com/openai/v1"
        target_model: "llama-3.3-70b-versatile"
        schema_format: "openai"
      
      - priority: 2
        provider_name: "cohere"
        api_key_alias: "COHERE_API_KEY"
        base_url: "https://api.cohere.com/v1"
        target_model: "command-r-plus-08-2024"
        schema_format: "openai"
```

### Provider Configuration

#### Groq
```yaml
- priority: 1
  provider_name: "groq"
  api_key_alias: "GROQ_API_KEY"
  base_url: "https://api.groq.com/openai/v1"
  target_model: "llama-3.3-70b-versatile"  # or mixtral-8x7b-32768
  schema_format: "openai"
```

#### OpenAI
```yaml
- priority: 1
  provider_name: "openai"
  api_key_alias: "OPENAI_API_KEY"
  base_url: "https://api.openai.com/v1"
  target_model: "gpt-4-turbo"  # or gpt-3.5-turbo
  schema_format: "openai"
```

#### Anthropic
```yaml
- priority: 1
  provider_name: "anthropic"
  api_key_alias: "ANTHROPIC_API_KEY"
  base_url: "https://api.anthropic.com"
  target_model: "claude-3-opus-20240229"  # or claude-3-sonnet
  schema_format: "anthropic"
```

#### Google Gemini
```yaml
- priority: 1
  provider_name: "gemini"
  api_key_alias: "GEMINI_API_KEY"
  base_url: "https://generativelanguage.googleapis.com"
  target_model: "gemini-pro"
  schema_format: "gemini"
```

#### Cohere
```yaml
- priority: 1
  provider_name: "cohere"
  api_key_alias: "COHERE_API_KEY"
  base_url: "https://api.cohere.com/v1"
  target_model: "command-r-plus-08-2024"
  schema_format: "openai"
```

### Configuration Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `priority` | Integer | Yes | Lower number = higher priority in failover chain |
| `provider_name` | String | Yes | Provider identifier: groq, openai, anthropic, gemini, cohere |
| `api_key_alias` | String | Yes | Environment variable name containing API key |
| `base_url` | String | Yes | Provider API base URL |
| `target_model` | String | Yes | Model name at the target provider |
| `schema_format` | String | Yes | Response format: openai, anthropic, gemini, cohere |

## Feature Flags

### OSS Build (Minimal)
```bash
cargo build --release --no-default-features
```

**Includes:**
- Core routing engine
- Tool extraction
- Loop detection
- Tool storm guard
- In-memory cache (Moka + DashMap)

**Size:** ~35MB Docker image

### Full Build (All Features)
```bash
cargo build --release
```

**Adds to OSS:**
- Enterprise mode support
- Redis integration
- ClickHouse logging
- Compliance features
- Observability tools

## Configuration Precedence

When configuring Kryneth, values are loaded in this order (highest to lowest priority):

1. **Environment Variables** - Override everything
2. **routing.yaml** - Per-tenant provider config
3. **Defaults in Code** - Fallback values

### Example
```bash
# This environment variable overrides routing.yaml
export MAX_SESSION_TOOL_CALLS=50

# routing.yaml can then be simpler
# Kryneth will use env variable value: 50
```

## .env File Support

Create a `.env` file for local development:

```bash
# .env
GROQ_API_KEY=gsk_your_key_here
COHERE_API_KEY=h28_your_key_here
RUST_LOG=debug
MAX_SESSION_TOOL_CALLS=20
MAX_IDENTICAL_TOOL_CALLS=5
RATE_LIMIT_MAX_REQUESTS=100
RATE_LIMIT_WINDOW_SECS=60
```

Load with:
```bash
set -a
source .env
set +a
cargo run --release
```

Or with Docker:
```bash
docker run -p 8080:8080 --env-file .env kryneth-gateway:latest
```

## Validation

Kryneth validates configuration on startup:

```bash
# Missing required field in routing.yaml
Error: target_model is required in routing.yaml

# Invalid provider name
Error: unknown provider_name "llama-api"

# API key not found
Error: environment variable GROQ_API_KEY not set
```

Fix validation errors before the gateway starts.

## Performance Tuning

### High-Throughput Scenario
```bash
export RATE_LIMIT_MAX_REQUESTS=1000
export RATE_LIMIT_WINDOW_SECS=60
export TOKIO_WORKER_THREADS=16
export L1_CACHE_SIZE=50000
export MAX_CONNECTIONS=5000
```

### Low-Latency Scenario
```bash
export MAX_SESSION_TOOL_CALLS=10
export MAX_IDENTICAL_TOOL_CALLS=3
export CACHE_TTL_SECS=30
export TOKIO_WORKER_THREADS=4
```

### Memory-Constrained Environment
```bash
export L1_CACHE_SIZE=1000
export MAX_CONNECTIONS=100
export RATE_LIMIT_MAX_REQUESTS=10
```
