//! infrastructure/llm_router.rs — Dynamic Multi-LLM Router with Automated Fallback

use std::sync::Arc;
use tracing::{error, info, warn};

use crate::domain::models::{AppState, UpstreamTarget};
use crate::error::GatewayError;
use crate::infrastructure::routing_strategy::RoutingStrategy;

#[derive(Debug, Clone, PartialEq)]
pub enum AuthScheme {
    Bearer,
    XApiKey,
    GoogleApiKey,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SchemaFormat {
    OpenAI,
    Anthropic,
    Gemini,
    Cohere,
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub id: String,
    pub base_url: String, // Kept for backwards compatibility but we use model_config.base_url
    pub auth_scheme: AuthScheme,
    pub schema_format: SchemaFormat,
}

pub fn get_provider_config(provider_name: &str) -> ProviderConfig {
    let provider = provider_name.to_lowercase();

    // Exact match on the canonical schema_format strings stored in the DB.
    // "google" is an internal alias for Gemini — never exposed to the UI dropdown.
    // Any unrecognised value defaults to OpenAI-compatible behaviour.
    let (schema, auth) = match provider.as_str() {
        "anthropic" => (SchemaFormat::Anthropic, AuthScheme::XApiKey),
        "gemini" | "google" => (SchemaFormat::Gemini, AuthScheme::GoogleApiKey),
        "cohere" => (SchemaFormat::Cohere, AuthScheme::Bearer),
        _ => (SchemaFormat::OpenAI, AuthScheme::Bearer),
    };

    ProviderConfig {
        id: provider.clone(),
        base_url: String::new(), // Dynamic base_url used directly from model config
        auth_scheme: auth,
        schema_format: schema,
    }
}

pub fn request_has_tools(body_bytes: &[u8]) -> bool {
    let scan_limit = body_bytes.len().min(16384);
    let slice = &body_bytes[..scan_limit];
    slice.windows(7).any(|w| w == b"\"tools\"") || slice.windows(12).any(|w| w == b"\"tool_calls\"")
}

fn is_retryable_error(status: u16) -> bool {
    matches!(status, 401 | 403 | 404 | 408 | 429 | 500 | 502 | 503 | 504)
}

/// Circuit-breaker entries are scoped per tenant so aliases never collide across tenants.
#[inline]
fn circuit_breaker_key(tenant_id: &str, key_alias: &str) -> String {
    format!("{}::{}", tenant_id, key_alias)
}

pub struct PreparedUpstreamRequest {
    pub tenant_id: String,
    pub primary_request: Option<reqwest::RequestBuilder>,
    pub primary_target: UpstreamTarget,
    pub fallback_targets: Vec<UpstreamTarget>,
    pub provider_config: ProviderConfig,
    pub model: String,
    pub requested_provider: String,
    pub executed_provider: String,
    pub is_hot_swapped: u8,
    pub accept_header: String,
}

pub fn extract_model_fast(body_bytes: &[u8]) -> Result<String, GatewayError> {
    // Scan first 8KB or so
    let scan_limit = body_bytes.len().min(8192);
    let slice = &body_bytes[..scan_limit];
    if let Some(pos) = slice.windows(7).position(|w| w == b"\"model\"") {
        let after_key = &slice[pos + 7..];
        let mut i = 0;
        while i < after_key.len() && (after_key[i].is_ascii_whitespace() || after_key[i] == b':') {
            i += 1;
        }
        if i < after_key.len() && after_key[i] == b'"' {
            i += 1;
            let start = i;
            while i < after_key.len() && after_key[i] != b'"' {
                if after_key[i] == b'\\' {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < after_key.len() {
                let model_str = std::str::from_utf8(&after_key[start..i]).map_err(|_| {
                    GatewayError::InvalidJSON("Invalid UTF-8 in model name".to_string())
                })?;
                return Ok(model_str.trim().to_string());
            }
        }
    }

    if body_bytes.len() > 5 * 1024 * 1024 {
        // High load pass-through: if > 5MB and model key not found in prefix, return error
        return Err(GatewayError::MissingModel);
    }

    // Fallback for smaller cases
    #[derive(serde::Deserialize)]
    struct ExtractModel<'a> {
        #[serde(borrow)]
        model: Option<std::borrow::Cow<'a, str>>,
    }
    let extracted: ExtractModel<'_> =
        serde_json::from_slice(body_bytes).map_err(|e| GatewayError::InvalidJSON(e.to_string()))?;
    let model_owned = extracted
        .model
        .as_deref()
        .ok_or(GatewayError::MissingModel)?
        .trim()
        .to_string();
    Ok(model_owned)
}

pub async fn prep_upstream_request(
    state: &Arc<AppState>,
    tenant_id: &str,
    body_bytes: &mut [u8],
    accept_header: &str,
    _strategy: RoutingStrategy,
    test_scenario: Option<&str>,
) -> Result<PreparedUpstreamRequest, GatewayError> {
    // 1. Extract the model from the incoming request using the fast-path scanner
    let model_owned = extract_model_fast(body_bytes)?;
    let model = model_owned.as_str();

    // 2. Load the lock-free state
    let routing_guard = state.routing_state.state.load();

    // 3. Lookup the model for the specific tenant
    tracing::debug!("LOOKUP: Tenant: '{}', Model: '{}'", tenant_id, model);

    let tenant_models = routing_guard
        .get(tenant_id)
        .or_else(|| routing_guard.get("00000000-0000-0000-0000-000000000000"))
        .ok_or_else(|| {
            GatewayError::ModelNotConfigured(format!("No routing config for tenant '{}'", tenant_id))
        })?;

    let model_config = tenant_models
        .get(model)
        .or_else(|| {
            let m = model.trim().to_lowercase();
            if m.starts_with("gpt-") {
                tenant_models.get("gpt-4o")
            } else if m.starts_with("claude-") {
                tenant_models.get("claude-3-5-sonnet")
            } else if m.starts_with("gemini-") {
                tenant_models.get("gemini-1.5-pro")
            } else if m.starts_with("deepseek-") {
                tenant_models.get("deepseek-r1")
            } else if m.starts_with("cohere-") || m.starts_with("command-") {
                tenant_models.get("cohere-command-r")
            } else if m.starts_with("llama-") {
                tenant_models.get("llama-3.3-70b-versatile")
            } else {
                None
            }
        })
        .ok_or_else(|| {
            GatewayError::ModelNotConfigured(format!(
                "Model '{}' not configured for tenant '{}'",
                model, tenant_id
            ))
        })?;

    if model_config.targets.is_empty() {
        warn!("No targets configured for model {}", model);
        return Err(GatewayError::NoActiveKeys);
    }

    // 4. Sort and filter out circuit-breaker-blacklisted targets.
    let mut sorted_targets = model_config.targets.clone();

    sorted_targets.sort_by_key(|t| t.priority);

    // Drain targets that are currently open-circuited, collecting healthy ones.
    let mut healthy_targets: Vec<UpstreamTarget> = Vec::with_capacity(sorted_targets.len());
    for target in sorted_targets {
        let cb_key = circuit_breaker_key(tenant_id, &target.api_key_alias);
        if state.circuit_breaker.get(&cb_key).await.is_some() {
            warn!(
                model = %model,
                key_alias = %target.api_key_alias,
                "Circuit breaker OPEN for target — skipping"
            );
        } else {
            healthy_targets.push(target);
        }
    }

    if healthy_targets.is_empty() {
        error!(model = %model, "All targets for model are circuit-breaker-blacklisted");
        return Err(GatewayError::NoActiveKeys);
    }

    // The first healthy target is our primary target; the rest are ordered fallbacks.
    let mut healthy_targets_iter = healthy_targets.into_iter();
    // Safe: we already checked healthy_targets is non-empty above.
    let primary_target = healthy_targets_iter
        .next()
        .ok_or(GatewayError::NoActiveKeys)?;
    let fallback_targets: Vec<UpstreamTarget> = healthy_targets_iter.collect();

    let provider_id = primary_target.schema_format.as_str();
    let config = get_provider_config(provider_id);

    let requested_provider = config.id.clone();
    let executed_provider = config.id.clone();

    let primary_request = build_provider_request(
        &state.http_client,
        &config,
        &primary_target.base_url,
        &primary_target.api_key,
        model,
        &primary_target.target_model,
        body_bytes,
        accept_header,
        test_scenario,
    )?;

    Ok(PreparedUpstreamRequest {
        tenant_id: tenant_id.to_string(),
        primary_request: Some(primary_request),
        primary_target,
        fallback_targets,
        provider_config: config,
        model: model_owned,
        requested_provider,
        executed_provider,
        is_hot_swapped: 0,
        accept_header: accept_header.to_string(),
    })
}

/// Dynamic Routing Helper with Circuit-Breaker Fallbacks.
/// Executes the pre-built primary request, and if it fails with a retryable HTTP status code,
/// iterates through fallback targets in priority order, building and attempting each one.
pub async fn execute_upstream_request(
    state: &Arc<AppState>,
    mut prep: PreparedUpstreamRequest,
    body_bytes: &mut [u8],
) -> Result<(reqwest::Response, String, u8, PreparedUpstreamRequest), GatewayError> {
    info!(
        model = %prep.model,
        "Attempting primary upstream request"
    );

    let mut last_error;

    let req = prep
        .primary_request
        .take()
        .expect("primary request already consumed");
    match req.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            if !is_retryable_error(status) {
                // Primary key succeeded — is_hot_swapped = 0
                return Ok((
                    response,
                    prep.primary_target.api_key_alias.clone(),
                    0u8,
                    prep,
                ));
            }
            warn!(
                model = %prep.model,
                key_alias = %prep.primary_target.api_key_alias,
                status = status,
                "Primary target returned retryable status code — triggering circuit breaker"
            );
            // Open circuit breaker for 30s
            let cb_key = circuit_breaker_key(&prep.tenant_id, &prep.primary_target.api_key_alias);
            state
                .circuit_breaker
                .insert(cb_key, ())
                .await;

            last_error = GatewayError::ResponseBuild(format!("Provider returned {}", status));
        }
        Err(e) => {
            warn!(
                model = %prep.model,
                key_alias = %prep.primary_target.api_key_alias,
                error = %e,
                "Primary target network request failed — triggering circuit breaker"
            );
            let cb_key = circuit_breaker_key(&prep.tenant_id, &prep.primary_target.api_key_alias);
            state
                .circuit_breaker
                .insert(cb_key, ())
                .await;

            last_error = GatewayError::UpstreamUnreachable(e);
        }
    }

    // Try fallbacks in priority order
    for (i, fallback) in prep.fallback_targets.clone().into_iter().enumerate() {
        let cb_key = circuit_breaker_key(&prep.tenant_id, &fallback.api_key_alias);
        if state.circuit_breaker.get(&cb_key).await.is_some() {
            warn!(
                model = %prep.model,
                key_alias = %fallback.api_key_alias,
                "Fallback target circuit breaker OPEN — skipping"
            );
            continue;
        }

        info!(
            model = %prep.model,
            key_alias = %fallback.api_key_alias,
            fallback_index = i + 1,
            "Attempting fallback target request"
        );

        let fallback_config = get_provider_config(fallback.schema_format.as_str());

        let fallback_req = build_provider_request(
            &state.http_client,
            &fallback_config,
            &fallback.base_url,
            &fallback.api_key,
            &prep.model,
            &fallback.target_model,
            body_bytes,
            "application/json",
            None,
        );

        let req_builder = match fallback_req {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    model = %prep.model,
                    key_alias = %fallback.api_key_alias,
                    error = %e,
                    "Failed to build fallback request — skipping"
                );
                continue;
            }
        };

        match req_builder.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                if !is_retryable_error(status) {
                    info!(
                        model = %prep.model,
                        key_alias = %fallback.api_key_alias,
                        "Fallback target succeeded — hot-swap complete"
                    );
                    prep.executed_provider = fallback_config.id.clone();
                    prep.is_hot_swapped = 1;
                    return Ok((response, fallback.api_key_alias.clone(), 1u8, prep));
                }
                warn!(
                    model = %prep.model,
                    key_alias = %fallback.api_key_alias,
                    status = status,
                    "Fallback target returned retryable status code — triggering circuit breaker"
                );
                state
                    .circuit_breaker
                    .insert(cb_key, ())
                    .await;

                last_error = GatewayError::ResponseBuild(format!(
                    "Fallback target {} HTTP {}",
                    fallback.api_key_alias, status
                ));
            }
            Err(e) => {
                warn!(
                    model = %prep.model,
                    key_alias = %fallback.api_key_alias,
                    error = %e,
                    "Fallback target network request failed — triggering circuit breaker"
                );
                state
                    .circuit_breaker
                    .insert(cb_key, ())
                    .await;

                last_error = GatewayError::UpstreamUnreachable(e);
            }
        }
    }

    error!(
        model = %prep.model,
        "All upstream targets failed (primary + all fallbacks)"
    );
    Err(last_error)
}

pub async fn execute_upstream_request_simple(
    state: &Arc<AppState>,
    tenant_id: &str,
    body_bytes: &mut [u8],
    accept_header: &str,
    strategy: RoutingStrategy,
) -> Result<(reqwest::Response, String, u8), GatewayError> {
    let prep = prep_upstream_request(state, tenant_id, body_bytes, accept_header, strategy, None).await?;
    let (resp, key, hot_swapped, _) = execute_upstream_request(state, prep, body_bytes).await?;
    Ok((resp, key, hot_swapped))
}

pub async fn route_chat_completion_with_fallback(
    state: &Arc<AppState>,
    tenant_id: &str,
    body_bytes: &mut [u8],
    accept_header: &str,
    strategy: RoutingStrategy,
) -> Result<(reqwest::Response, String, u8), GatewayError> {
    let prep = prep_upstream_request(state, tenant_id, body_bytes, accept_header, strategy, None).await?;
    let (resp, key, hot_swapped, _) = execute_upstream_request(state, prep, body_bytes).await?;
    Ok((resp, key, hot_swapped))
}

#[allow(clippy::too_many_arguments, unused_variables)]
pub fn build_provider_request(
    client: &reqwest::Client,
    config: &ProviderConfig,
    base_url: &str,
    api_key: &str,
    model: &str,
    upstream_target_model: &str,
    body_bytes: &mut [u8],
    accept_header: &str,
    test_scenario: Option<&str>,
) -> Result<reqwest::RequestBuilder, GatewayError> {
    let endpoint = if config.schema_format == SchemaFormat::Anthropic {
        format!("{}/messages", base_url)
    } else if config.schema_format == SchemaFormat::Gemini {
        #[derive(serde::Deserialize)]
        struct ExtractStream {
            stream: Option<bool>,
        }
        let extracted_stream =
            serde_json::from_slice::<ExtractStream>(body_bytes).map_err(|e| {
                GatewayError::ResponseBuild(format!("Invalid JSON body for stream check: {}", e))
            })?;
        let is_stream = extracted_stream.stream.unwrap_or(false);
        if is_stream {
            format!(
                "{}/{}:streamGenerateContent?key={}",
                base_url, upstream_target_model, api_key
            )
        } else {
            format!(
                "{}/{}:generateContent?key={}",
                base_url, upstream_target_model, api_key
            )
        }
    } else {
        format!("{}/chat/completions", base_url)
    };

    let mut builder = client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", accept_header);

    if let Some(scen) = test_scenario {
        builder = builder.header("X-Test-Scenario", scen);
    }

    let request = match config.auth_scheme {
        AuthScheme::Bearer => builder.bearer_auth(api_key),
        AuthScheme::XApiKey => {
            if config.schema_format == SchemaFormat::Anthropic {
                builder
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01")
            } else {
                builder.header("x-api-key", api_key)
            }
        }
        AuthScheme::GoogleApiKey => builder, // Included in URL query
    };

    // 3. Conditional Translation (Zero-Copy proxy for OpenAI schemas)
    // If request payload > 5MB, bypass payload rewriting or translation entirely,
    // and build the request directly using body_bytes.clone().
    let request = if body_bytes.len() > 5 * 1024 * 1024 {
        request.body(body_bytes.to_vec())
    } else if config.schema_format != SchemaFormat::OpenAI {
        let source_adapter =
            crate::infrastructure::providers::get_utp_adapter(&SchemaFormat::OpenAI);

        let mut conv = source_adapter.to_universal(body_bytes).map_err(|e| {
            GatewayError::ResponseBuild(format!("Incoming translation failed: {e}"))
        })?;

        conv.model = Some(upstream_target_model.to_string());

        let target_adapter =
            crate::infrastructure::providers::get_utp_adapter(&config.schema_format);
        let final_body = target_adapter
            .from_universal(&conv)
            .map_err(|e| GatewayError::ResponseBuild(format!("Translation error: {e}")))?;

        request.json(&final_body)
    } else {
        // True Decoupling via Payload Rewriting (Phase 3)
        // Mutate the `model` field using simd-json in-place, then re-serialize using
        // simd_json::to_vec (NOT serde_json) to avoid OwnedValue → serde round-trip corruption.
        let rewritten_bytes = rewrite_model_field(body_bytes, upstream_target_model)
            .map_err(|e| GatewayError::ResponseBuild(format!("Payload rewrite failed: {}", e)))?;

        tracing::debug!(
            upstream_target_model = %upstream_target_model,
            payload = %String::from_utf8_lossy(&rewritten_bytes),
            "[SIMD] Mutated payload that will be sent upstream"
        );

        request.body(rewritten_bytes)
    };

    Ok(request)
}

/// Rewrites the `model` field in a JSON byte payload using simd-json.
///
/// SIMD-json parses in-place (mutating `body_bytes`), builds an `OwnedValue`,
/// swaps the `model` key, then re-serializes using `simd_json::to_vec` — which
/// understands its own type system and produces correct JSON.  Using
/// `serde_json::to_vec` on an `OwnedValue` previously caused numeric/null
/// corruption because the two crates' Serialize impls are not equivalent.
pub(crate) fn rewrite_model_field(
    body_bytes: &[u8],
    target_model: &str,
) -> Result<Vec<u8>, String> {
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut depth = 0;

    while i < body_bytes.len() {
        let b = body_bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => {
                    in_string = true;
                    // Check if this is the "model" key at depth 1
                    if depth == 1 && body_bytes[i..].starts_with(b"\"model\"") {
                        let mut j = i + b"\"model\"".len();
                        // Skip whitespace
                        while j < body_bytes.len() && body_bytes[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        if j < body_bytes.len() && body_bytes[j] == b':' {
                            j += 1;
                            while j < body_bytes.len() && body_bytes[j].is_ascii_whitespace() {
                                j += 1;
                            }
                            if j < body_bytes.len() && body_bytes[j] == b'"' {
                                let start_quote = j;
                                j += 1;
                                while j < body_bytes.len() {
                                    if body_bytes[j] == b'\\' {
                                        j += 2;
                                        continue;
                                    }
                                    if body_bytes[j] == b'"' {
                                        let end_quote = j;
                                        let mut result = Vec::with_capacity(
                                            body_bytes.len() + target_model.len(),
                                        );
                                        result.extend_from_slice(&body_bytes[..start_quote + 1]);
                                        result.extend_from_slice(target_model.as_bytes());
                                        result.extend_from_slice(&body_bytes[end_quote..]);
                                        return Ok(result);
                                    }
                                    j += 1;
                                }
                            }
                        }
                    }
                }
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
    }

    Err("Could not find \"model\" field for zero-copy rewrite".to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::{AppState, ModelConfig, RoutingState, UpstreamTarget};
    use crate::infrastructure::l1_cache::L1Cache;
    use moka::future::Cache;
    use std::collections::HashMap;
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ── SIMD Payload Mutation Tests ───────────────────────────────────────────

    #[test]
    fn test_rewrite_model_field_produces_valid_json() {
        let input =
            r#"{"model": "my-prod-route", "messages": [{"role": "user", "content": "Hello"}]}"#;
        let bytes = input.as_bytes().to_vec();

        let result = rewrite_model_field(&bytes, "llama3-8b-8192");
        assert!(
            result.is_ok(),
            "rewrite_model_field must not return an error: {:?}",
            result.err()
        );

        let output = result.unwrap();

        // Assert the output is valid UTF-8
        let output_str = std::str::from_utf8(&output).expect("output must be valid UTF-8");

        // Assert it round-trips via standard serde_json (proves no structural corruption)
        let parsed: serde_json::Value =
            serde_json::from_str(output_str).expect("output must be valid JSON");

        // Assert the model field was correctly swapped
        assert_eq!(
            parsed["model"].as_str().unwrap(),
            "llama3-8b-8192",
            "model field must be rewritten to the upstream target model"
        );

        // Assert the messages array was preserved intact
        assert_eq!(
            parsed["messages"][0]["role"].as_str().unwrap(),
            "user",
            "messages array must survive the rewrite round-trip"
        );
        assert_eq!(
            parsed["messages"][0]["content"].as_str().unwrap(),
            "Hello",
            "message content must survive the rewrite round-trip"
        );
    }

    #[test]
    fn test_rewrite_model_field_preserves_numerics_and_booleans() {
        // This is the regression test for the original serde_json::to_vec bug which
        // could corrupt numbers, booleans, and null values through simd-json's OwnedValue.
        let input = r#"{"model": "my-route", "temperature": 0.7, "max_tokens": 1024, "stream": true, "top_p": null}"#;
        let bytes = input.as_bytes().to_vec();

        let output = rewrite_model_field(&bytes, "llama3-70b-8192").unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(parsed["model"].as_str().unwrap(), "llama3-70b-8192");
        // Verify numerics are NOT corrupted
        assert!(
            (parsed["temperature"].as_f64().unwrap() - 0.7).abs() < 1e-9,
            "temperature float must not be corrupted: got {:?}",
            parsed["temperature"]
        );
        assert_eq!(
            parsed["max_tokens"].as_i64().unwrap(),
            1024,
            "max_tokens integer must not be corrupted"
        );
        assert!(
            parsed["stream"].as_bool().unwrap(),
            "stream boolean must not be corrupted"
        );
        assert!(
            parsed["top_p"].is_null(),
            "top_p null must survive the round-trip"
        );
    }

    #[test]
    fn test_rewrite_model_field_short_to_long_model_name() {
        // The critical case: target model is LONGER than the virtual name.
        // A naive byte-splice approach would produce invalid JSON here.
        let input = r#"{"model": "gpt-4", "messages": []}"#;
        let bytes = input.as_bytes().to_vec();

        let output = rewrite_model_field(&bytes, "llama3-70b-8192-tool-use-preview").unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(
            parsed["model"].as_str().unwrap(),
            "llama3-70b-8192-tool-use-preview",
            "long model names must be written correctly without truncation"
        );
    }

    #[test]
    fn test_rewrite_model_field_long_to_short_model_name() {
        // The reverse: target model is SHORTER than the virtual name.
        let input = r#"{"model": "my-very-long-virtual-route-name", "messages": []}"#;
        let bytes = input.as_bytes().to_vec();

        let output = rewrite_model_field(&bytes, "gpt-4o").unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(
            parsed["model"].as_str().unwrap(),
            "gpt-4o",
            "short model names must not leave trailing bytes from the longer source"
        );
    }

    #[test]
    fn test_rewrite_model_field_invalid_json_returns_error() {
        let bytes = b"not valid json {{{".to_vec();
        let result = rewrite_model_field(&bytes, "some-model");
        assert!(
            result.is_err(),
            "rewrite_model_field must propagate simd-json parse errors"
        );
    }

    #[allow(clippy::uninit_assumed_init, invalid_value, unknown_lints)]
    async fn setup_test_state() -> (Arc<AppState>, MockServer) {
        let mock_server = MockServer::start().await;

        let circuit_breaker = Cache::builder()
            .max_capacity(100)
            .time_to_live(std::time::Duration::from_secs(60))
            .build();

        let loop_fallback_cache = Cache::builder().max_capacity(100).build();
        let agent_guardian_cache = Cache::builder().max_capacity(100).build();

        let routing_state = Arc::new(RoutingState::new());
        let l1_cache = Arc::new(L1Cache::new(1024).unwrap());
        let http_client = reqwest::Client::new();

        let state = AppState {
            http_client,
            compliance_url: String::new(),
            rate_limit_max: 0,
            rate_limit_window: 0,
            dashboard_url: String::new(),
            llm_api_base_url: Some(mock_server.uri()),
            redis_client: None,
            telemetry: Arc::new(crate::infrastructure::oss_adapters::OssTelemetry::new(Arc::new(dashmap::DashMap::new()))),
            billing: Arc::new(crate::infrastructure::oss_adapters::OssBilling),
            auth_resolver: Arc::new(crate::infrastructure::oss_adapters::OssAuth),
            rate_limiter: Arc::new(crate::infrastructure::oss_adapters::OssRateLimit),
            routing_config: Arc::new(crate::infrastructure::oss_adapters::OssRoutingConfig),
            semantic_cache: Arc::new(crate::infrastructure::oss_adapters::OssSemanticCache),
            rate_limit_cache: Arc::new(dashmap::DashMap::new()),
            l1_cache,
            routing_state,
            circuit_breaker,
            loop_fallback_cache,
            mcp_registry: crate::infrastructure::mcp_registry::McpConnectionRegistry::empty(),
            tool_registry: crate::usecases::tool_router::ToolRegistry::empty(),
            agent_guardian_cache,
            operation_cache: Cache::builder().build(),
            dashboard_metrics: Arc::new(crate::domain::models::DashboardMetrics::new()),
            pricing_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
            trace_store: Arc::new(dashmap::DashMap::new()),
            budget_map: Arc::new(arc_swap::ArcSwap::from_pointee(
                std::collections::HashMap::new(),
            )),
        };

        (Arc::new(state), mock_server)
    }

    // ── PIPELINE CONTRACT TEST 3: Router virtual-model lookup ─────────────
    //
    // Constructs a fake RoutingState with "my-prod-route" as the virtual model
    // name, injects a JSON body {"model": "my-prod-route"}, and asserts that
    // prep_upstream_request:
    //   1. Does NOT return GatewayError::ModelNotConfigured.
    //   2. Returns an upstream_target_model of "gpt-4o" (the physical name).
    //   3. Returns the virtual name "my-prod-route" in the PreparedUpstreamRequest.model.
    //
    // This test acts as the definitive proof that the router correctly separates
    // the virtual routing key (what the client sends) from the physical model name
    // (what OpenAI receives). If this fails, the decoupling is broken at the router.
    #[tokio::test]
    async fn test_router_finds_virtual_model() {
        let (state, mock_server) = setup_test_state().await;

        // Seed the RoutingState with a virtual model mapping.
        // The outer key is a UUID string (matching what auth_middleware injects).
        // The inner key is the virtual model name the client will send.
        let tenant_id = "11111111-2222-3333-4444-555555555555";
        let virtual_model_name = "my-prod-route";
        let physical_model_name = "gpt-4o";

        let mut tenant_map = HashMap::new();
        tenant_map.insert(
            virtual_model_name.to_string(),
            ModelConfig {
                targets: vec![UpstreamTarget {
                    priority: 1,
                    weight: 100,
                    api_key_alias: "primary".into(),
                    api_key: "sk-test-key".to_string(),
                    provider_name: "openai".into(),
                    base_url: mock_server.uri(),
                    target_model: physical_model_name.to_string(),
                    schema_format: "openai".into(),
                }],
                ..Default::default()
            },
        );
        let mut map = HashMap::new();
        map.insert(tenant_id.to_string(), tenant_map);
        state.routing_state.state.store(Arc::new(map));

        // Simulate the exact client request body.
        let body = axum::body::Bytes::from(
            serde_json::json!({
                "model": "my-prod-route",
                "messages": [{"role": "user", "content": "Hello"}]
            })
            .to_string(),
        );

        let mut body_mut = body.to_vec();
        let result = prep_upstream_request(
            &state,
            tenant_id,
            &mut body_mut,
            "*/*",
            RoutingStrategy::Default,
            None,
        )
        .await;

        assert!(
            result.is_ok(),
            "prep_upstream_request MUST NOT return MODEL_NOT_CONFIGURED for a correctly seeded RoutingState. Error: {:?}",
            result.err()
        );

        let prep = result.unwrap();

        // The `model` field in PreparedUpstreamRequest holds the VIRTUAL name
        // (used for logging / telemetry — what the client requested).
        assert_eq!(
            prep.model, virtual_model_name,
            "prep.model must be the virtual name '{}' for telemetry",
            virtual_model_name
        );

        // The `upstream_target_model` is the PHYSICAL name sent to OpenAI.
        assert_eq!(
            prep.primary_target.target_model, physical_model_name,
            "prep.primary_target.target_model must be the physical name '{}' sent upstream",
            physical_model_name
        );

        // The primary key must be selected.
        assert_eq!(prep.primary_target.api_key_alias, "primary");
        assert!(
            prep.fallback_targets.is_empty(),
            "No fallback targets expected"
        );

        std::mem::forget(state);
    }

    // ── PIPELINE CONTRACT TEST 3b: Trimming guard ─────────────────────────
    //
    // Verifies that a client sending {"model": "my-prod-route "} (trailing space)
    // is still matched correctly to "my-prod-route" in the routing table.
    // Regression test for the trim() fix applied to prep_upstream_request.
    #[tokio::test]
    async fn test_router_trims_whitespace_in_model_name() {
        let (state, mock_server) = setup_test_state().await;

        let tenant_id = "22222222-3333-4444-5555-666666666666";
        let mut tenant_map = HashMap::new();
        tenant_map.insert(
            "clean-route".to_string(),
            ModelConfig {
                targets: vec![UpstreamTarget {
                    priority: 1,
                    weight: 1,
                    api_key_alias: "key-a".into(),
                    api_key: "sk-abc".to_string(),
                    provider_name: "openai".into(),
                    base_url: mock_server.uri(),
                    target_model: "gpt-4o-mini".into(),
                    schema_format: "openai".into(),
                }],
                ..Default::default()
            },
        );
        let mut map = HashMap::new();
        map.insert(tenant_id.to_string(), tenant_map);
        state.routing_state.state.store(Arc::new(map));

        // Inject a model name with leading/trailing whitespace.
        let body = axum::body::Bytes::from(
            r#"{"model": "  clean-route  ", "messages": [{"role": "user", "content": "hi"}]}"#,
        );

        let mut body_mut = body.to_vec();
        let result = prep_upstream_request(
            &state,
            tenant_id,
            &mut body_mut,
            "*/*",
            RoutingStrategy::Default,
            None,
        )
        .await;

        assert!(
            result.is_ok(),
            "Whitespace-padded model name must be trimmed and matched. Error: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap().model, "clean-route");

        std::mem::forget(state);
    }

    #[tokio::test]
    async fn test_circuit_breaker_blacklist_enforcement() {
        let (state, _) = setup_test_state().await;

        // 1. Setup: 2 targets, target-1 is primary (priority 1)
        let mut tenant_map = HashMap::new();
        tenant_map.insert(
            "gpt-4".into(),
            ModelConfig {
                targets: vec![
                    UpstreamTarget {
                        priority: 1,
                        weight: 1,
                        api_key_alias: "key-1".into(),
                        api_key: "sk-1".into(),
                        provider_name: "openai".into(),
                        base_url: "http://localhost".into(),
                        target_model: "gpt-4".into(),
                        schema_format: "openai".into(),
                    },
                    UpstreamTarget {
                        priority: 2,
                        weight: 1,
                        api_key_alias: "key-2".into(),
                        api_key: "sk-2".into(),
                        provider_name: "openai".into(),
                        base_url: "http://localhost".into(),
                        target_model: "gpt-4".into(),
                        schema_format: "openai".into(),
                    },
                ],
                ..Default::default()
            },
        );
        let mut map = HashMap::new();
        map.insert("t1".into(), tenant_map);
        state.routing_state.state.store(Arc::new(map));

        // 2. Blacklist key-1 (tenant-scoped key as used by the router)
        state
            .circuit_breaker
            .insert(circuit_breaker_key("t1", "key-1"), ())
            .await;

        // 3. Prep request
        let body = axum::body::Bytes::from(r#"{"model": "gpt-4"}"#);
        let mut body_mut = body.to_vec();
        let prep =
            prep_upstream_request(&state, "t1", &mut body_mut, "*/*", RoutingStrategy::Default, None)
                .await
                .unwrap();

        // 4. Verify: key-2 should have been selected as primary (prep.fallback_targets should be empty)
        assert_eq!(
            prep.fallback_targets.len(),
            0,
            "key-2 should have been promoted to primary, leaving no fallbacks"
        );

        // We leak the Arc to prevent it from trying to drop uninitialized handles on DB/Redis
        std::mem::forget(state);
    }

    #[tokio::test]
    async fn test_circuit_breaker_tripping() {
        let (state, mock_server) = setup_test_state().await;

        // 1. Setup: Model with a failing target
        let mut tenant_map = HashMap::new();
        tenant_map.insert(
            "gpt-4".into(),
            ModelConfig {
                targets: vec![UpstreamTarget {
                    priority: 1,
                    weight: 1,
                    api_key_alias: "fail-key".into(),
                    api_key: "sk-1".into(),
                    provider_name: "openai".into(),
                    base_url: mock_server.uri(),
                    target_model: "gpt-4".into(),
                    schema_format: "openai".into(),
                }],
                ..Default::default()
            },
        );
        let mut map = HashMap::new();
        map.insert("t1".into(), tenant_map);
        state.routing_state.state.store(Arc::new(map));

        // 2. Mock: Provider returns 429
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&mock_server)
            .await;

        // 3. Prep request
        let body = axum::body::Bytes::from(r#"{"model": "gpt-4"}"#);
        let mut body_mut = body.to_vec();
        let prep =
            prep_upstream_request(&state, "t1", &mut body_mut, "*/*", RoutingStrategy::Default, None)
                .await
                .unwrap();
        let _ = execute_upstream_request(&state, prep, &mut body_mut).await;

        // 4. Verify: Breaker is now open for "fail-key" under tenant t1
        let is_blacklisted = state
            .circuit_breaker
            .get(&circuit_breaker_key("t1", "fail-key"))
            .await
            .is_some();
        assert!(
            is_blacklisted,
            "Circuit breaker should have tripped on 429 response"
        );

        std::mem::forget(state);
    }

    #[test]
    fn test_is_retryable_error_includes_401_403() {
        assert!(is_retryable_error(401), "401 should be retryable");
        assert!(is_retryable_error(403), "403 should be retryable");
        assert!(is_retryable_error(429), "429 should be retryable");
        assert!(is_retryable_error(500), "500 should be retryable");
        assert!(!is_retryable_error(200), "200 should not be retryable");
        assert!(!is_retryable_error(400), "400 should not be retryable");
        assert!(is_retryable_error(404), "404 should be retryable");
    }

    #[test]
    fn test_gemini_url_appends_api_key() {
        let client = reqwest::Client::new();
        let config = ProviderConfig {
            id: "gemini".to_string(),
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            auth_scheme: AuthScheme::GoogleApiKey,
            schema_format: SchemaFormat::Gemini,
        };

        // Case 1: Stream
        let stream_body = axum::body::Bytes::from(
            r#"{"model": "gemini-1.5-pro", "stream": true, "messages": [{"role": "user", "content": "hello"}]}"#,
        );
        let mut stream_body_mut = stream_body.to_vec();
        let builder_stream = build_provider_request(
            &client,
            &config,
            "https://generativelanguage.googleapis.com/v1beta/models",
            "test-gemini-key",
            "gemini-1.5-pro",
            "gemini-1.5-pro",
            &mut stream_body_mut,
            "*/*",
            None,
        )
        .unwrap();
        let request_stream = builder_stream.build().unwrap();
        assert_eq!(
            request_stream.url().as_str(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro:streamGenerateContent?key=test-gemini-key"
        );

        // Case 2: Non-stream
        let non_stream_body = axum::body::Bytes::from(
            r#"{"model": "gemini-1.5-pro", "stream": false, "messages": [{"role": "user", "content": "hello"}]}"#,
        );
        let mut non_stream_body_mut = non_stream_body.to_vec();
        let builder_non_stream = build_provider_request(
            &client,
            &config,
            "https://generativelanguage.googleapis.com/v1beta/models",
            "test-gemini-key",
            "gemini-1.5-pro",
            "gemini-1.5-pro",
            &mut non_stream_body_mut,
            "*/*",
            None,
        )
        .unwrap();
        let request_non_stream = builder_non_stream.build().unwrap();
        assert_eq!(
            request_non_stream.url().as_str(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro:generateContent?key=test-gemini-key"
        );
    }

    #[tokio::test]
    async fn test_execute_upstream_request_falls_back_on_404() {
        let (state, primary_server) = setup_test_state().await;
        let fallback_server = MockServer::start().await;

        // Primary returns 404 Not Found
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&primary_server)
            .await;

        // Fallback returns 200 OK
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "chatcmpl-fallback",
                "object": "chat.completion",
                "choices": [{"message": {"role": "assistant", "content": "Fallback response"}}]
            })))
            .mount(&fallback_server)
            .await;

        let tenant_id = "test-tenant-404";
        let mut tenant_map = HashMap::new();
        tenant_map.insert(
            "gemini-1.5-flash".to_string(),
            ModelConfig {
                targets: vec![
                    UpstreamTarget {
                        priority: 1,
                        weight: 100,
                        api_key_alias: "primary_key".into(),
                        api_key: "primary_secret".to_string(),
                        provider_name: "gemini".into(),
                        base_url: primary_server.uri(),
                        target_model: "gemini-1.5-flash".to_string(),
                        schema_format: "gemini".into(),
                    },
                    UpstreamTarget {
                        priority: 2,
                        weight: 100,
                        api_key_alias: "GROQ_API_KEY".into(),
                        api_key: "groq_secret".to_string(),
                        provider_name: "openai".into(),
                        base_url: fallback_server.uri(),
                        target_model: "gemini-1.5-flash".to_string(),
                        schema_format: "openai".into(),
                    },
                ],
                ..Default::default()
            },
        );
        let mut map = HashMap::new();
        map.insert(tenant_id.to_string(), tenant_map);
        state.routing_state.state.store(Arc::new(map));

        let body = axum::body::Bytes::from(
            serde_json::json!({
                "model": "gemini-1.5-flash",
                "messages": [{"role": "user", "content": "hello"}]
            })
            .to_string(),
        );

        let mut body_mut = body.to_vec();
        let prep = prep_upstream_request(
            &state,
            tenant_id,
            &mut body_mut,
            "application/json",
            RoutingStrategy::Default,
            None,
        )
        .await
        .unwrap();

        let (resp, success_key, hot_swapped, updated_prep) =
            execute_upstream_request(&state, prep, &mut body_mut)
                .await
                .unwrap();

        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(success_key, "GROQ_API_KEY");
        assert_eq!(hot_swapped, 1);
        assert_eq!(updated_prep.executed_provider, "openai");
    }

    #[test]
    fn test_request_has_tools_detection() {
        let payload_with_tools =
            r#"{"model": "gpt-4o", "messages": [], "tools": [{"type": "function"}]}"#;
        assert!(request_has_tools(payload_with_tools.as_bytes()));

        let payload_with_tool_calls =
            r#"{"model": "gpt-4o", "messages": [{"role": "assistant", "tool_calls": []}]}"#;
        assert!(request_has_tools(payload_with_tool_calls.as_bytes()));

        let payload_plain =
            r#"{"model": "gpt-4o", "messages": [{"role": "user", "content": "hello"}]}"#;
        assert!(!request_has_tools(payload_plain.as_bytes()));
    }

    #[tokio::test]
    async fn test_execute_upstream_request_skips_cohere_fallback_on_tools() {
        let (state, primary_server) = setup_test_state().await;
        let cohere_server = MockServer::start().await;
        let final_server = MockServer::start().await;

        // Primary returns 500 Server Error (retryable)
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&primary_server)
            .await;

        // Cohere returns 422 Unprocessable Entity if called (should be skipped!)
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_string("Cohere 422 invalid schema"))
            .mount(&cohere_server)
            .await;

        // Final OpenAI target returns 200 OK
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "chatcmpl-ok",
                "object": "chat.completion",
                "choices": [{"message": {"role": "assistant", "content": "Success"}}]
            })))
            .mount(&final_server)
            .await;

        let tenant_id = "test-tenant-cohere-skip";
        let mut tenant_map = HashMap::new();
        tenant_map.insert(
            "agentic-model".to_string(),
            ModelConfig {
                targets: vec![
                    UpstreamTarget {
                        priority: 1,
                        weight: 100,
                        api_key_alias: "primary_key".into(),
                        api_key: "primary_secret".to_string(),
                        provider_name: "openai".into(),
                        base_url: primary_server.uri(),
                        target_model: "gpt-4o".to_string(),
                        schema_format: "openai".into(),
                    },
                    UpstreamTarget {
                        priority: 2,
                        weight: 100,
                        api_key_alias: "COHERE_API_KEY".into(),
                        api_key: "cohere_secret".to_string(),
                        provider_name: "cohere".into(),
                        base_url: cohere_server.uri(),
                        target_model: "command-r-plus".to_string(),
                        schema_format: "cohere".into(),
                    },
                    UpstreamTarget {
                        priority: 3,
                        weight: 100,
                        api_key_alias: "BACKUP_OPENAI_KEY".into(),
                        api_key: "backup_secret".to_string(),
                        provider_name: "openai".into(),
                        base_url: final_server.uri(),
                        target_model: "gpt-4o-mini".to_string(),
                        schema_format: "openai".into(),
                    },
                ],
                ..Default::default()
            },
        );
        let mut map = HashMap::new();
        map.insert(tenant_id.to_string(), tenant_map);
        state.routing_state.state.store(Arc::new(map));

        let body = axum::body::Bytes::from(
            serde_json::json!({
                "model": "agentic-model",
                "messages": [{"role": "user", "content": "use a tool"}],
                "tools": [{"type": "function", "function": {"name": "test"}}]
            })
            .to_string(),
        );

        let mut body_mut = body.to_vec();
        let prep = prep_upstream_request(
            &state,
            tenant_id,
            &mut body_mut,
            "application/json",
            RoutingStrategy::Default,
            None,
        )
        .await
        .unwrap();

        let (resp, success_key, hot_swapped, updated_prep) =
            execute_upstream_request(&state, prep, &mut body_mut)
                .await
                .unwrap();

        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(success_key, "BACKUP_OPENAI_KEY");
        assert_eq!(hot_swapped, 1);
        assert_eq!(updated_prep.executed_provider, "openai");
    }
}
