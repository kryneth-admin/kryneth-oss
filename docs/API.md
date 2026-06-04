# Kryneth Gateway API Documentation

## Overview
Kryneth Gateway intercepts AI agent requests and provides real-time control, monitoring, and safety guardrails for autonomous agents.

## Base URL
```
http://0.0.0.0:8080
```

## Core Endpoints

### POST /v1/chat/completions
Routes LLM requests through the control plane with agent guardrails.

**Request:**
```json
{
  "model": "llama-3.3-70b-versatile",
  "messages": [
    {
      "role": "user",
      "content": "What is the weather?"
    }
  ],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "get_weather",
        "description": "Get current weather",
        "parameters": {
          "type": "object",
          "properties": {
            "location": {"type": "string"}
          }
        }
      }
    }
  ]
}
```

**Response:**
```json
{
  "id": "chatcmpl-xxxxx",
  "object": "chat.completion",
  "created": 1234567890,
  "model": "llama-3.3-70b-versatile",
  "choices": [...],
  "usage": {
    "prompt_tokens": 100,
    "completion_tokens": 50,
    "total_tokens": 150
  }
}
```

## Safety Features

### Tool Storm Guard
Enforces maximum concurrent tool executions per session.

**Configuration:**
```
MAX_SESSION_TOOL_CALLS=20     # Max tool calls within 60s
MAX_IDENTICAL_TOOL_CALLS=5    # Max identical tool signatures
```

**Behavior:**
- Request blocked with 429 status if limits exceeded
- Prevents cascading tool call explosions
- Protects against runaway recursive patterns

### Infinite Loop Detection
Detects and blocks tool call patterns that repeat within a 60-second window.

**How it works:**
- Tracks tool call signatures (name + arguments)
- Uses SIMD-accelerated hashing for fast comparison
- Compares against 60s sliding window
- Triggers circuit breaker on threshold violation

### Circuit Breaker
Automatic failover to secondary provider on upstream failures.

**Triggered by:**
- 401 Unauthorized (invalid API key)
- 503 Service Unavailable
- 429 Rate Limit Exceeded

**Failover time:** 0.37ms average

## Provider Support

### Supported Providers
- OpenAI (GPT-4, GPT-3.5)
- Anthropic (Claude)
- Google Gemini
- Groq (Llama, Mixtral)
- Cohere (Command-R)
- Open AI Compactible Models

### Tool Format Normalization
Kryneth automatically extracts and validates tools from:
- OpenAI format
- Anthropic format
- Google Gemini format
- Cohere format

## Error Responses

### 429 Too Many Requests
```json
{
  "error": {
    "message": "Tool storm threshold exceeded",
    "type": "tool_limit_error",
    "code": "MAX_TOOL_CALLS_EXCEEDED"
  }
}
```

### 503 Service Unavailable
```json
{
  "error": {
    "message": "All upstream providers failed",
    "type": "provider_error",
    "code": "CIRCUIT_BREAKER_OPEN"
  }
}
```

### 400 Bad Request
```json
{
  "error": {
    "message": "Invalid tool signature",
    "type": "validation_error",
    "code": "INVALID_TOOL_FORMAT"
  }
}
```

## Performance Targets
| Metric | Target | Description |
|--------|--------|-------------|
| P50 | 1.4ms | Standard auth, parsing, L1 cache |
| P90 | 2.1ms | SIMD-JSON extraction, tool hashing |
| P95 | 3.0ms | Heavy concurrent tool matching |
| P99 | 4.5ms | Large token payloads |
| Failover | 0.37ms | Circuit breaker trigger |

## Rate Limiting

**Configuration:**
```
RATE_LIMIT_MAX_REQUESTS=60
RATE_LIMIT_WINDOW_SECS=60
```

**Per tenant, per window:**
- 60 requests per 60 seconds (default)
- Returns 429 if exceeded
- Configurable per environment
