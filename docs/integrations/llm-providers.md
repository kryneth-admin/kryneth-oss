# Upstream LLM Providers

Kryneth abstracts upstream LLM providers behind a virtual model translation layer. This allows you to hot-swap models and implement automatic failovers transparently.

---

## 1. Provider Target Mapping

Configuring targets within `routing.yaml` specifies how virtual model requests map to upstream APIs:

```yaml
# routing.yaml
"00000000-0000-0000-0000-000000000000": # Tenant UUID
  "llama-3.3-70b-versatile":
    targets:
      - priority: 1
        provider_name: "groq"
        api_key_alias: "GROQ_API_KEY"
        base_url: "https://api.groq.com/openai/v1"
        target_model: "llama-3.3-70b-versatile"
        schema_format: "openai"
```

### Supported Providers
-   **Groq (`groq`)**: Low-latency execution. Uses `openai` format.
-   **OpenAI (`openai`)**: Industry standard. Uses `openai` format.
-   **Anthropic (`anthropic`)**: High reasoning capacity. Uses native `anthropic` messages format.
-   **Google Gemini (`gemini`)**: Multi-modal capability. Uses `gemini` structured format.
-   **Cohere (`cohere`)**: Failover translation targets. Uses `openai` or `cohere` formats.

---

## 2. API Key Management

The gateway dynamically resolves authentication tokens from active environment variables using the `api_key_alias` string in the target configuration.

> [!WARNING]
> **Environment Security:**
> Never commit API keys directly inside `routing.yaml`. Always map them to environment variable aliases (e.g. `api_key_alias: "GROQ_API_KEY"`) and inject the real keys via your system environment variables or docker `.env` settings.
