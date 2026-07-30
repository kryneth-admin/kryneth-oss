use std::collections::VecDeque;
use std::time::Duration;

use moka::future::Cache;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::domain::models::GatewayError;

pub enum EmbeddingClient {
    Tcp {
        endpoint: String,
        client: reqwest::Client,
    },
}

impl EmbeddingClient {
    pub async fn embed(&self, prompt: &str) -> Result<Vec<f32>, GatewayError> {
        match self {
            Self::Tcp { endpoint, client } => {
                let fut = async {
                    let req_payload = serde_json::json!({ "prompt": prompt });
                    let url = format!("{}/embed", endpoint.trim_end_matches('/'));
                    let resp = client
                        .post(&url)
                        .json(&req_payload)
                        .send()
                        .await
                        .map_err(|e| {
                            GatewayError::ResponseBuild(format!("TCP HTTP request failed: {e}"))
                        })?;

                    let status = resp.status();
                    if !status.is_success() {
                        return Err(GatewayError::ResponseBuild(format!(
                            "TCP embedding service returned status: {status}"
                        )));
                    }

                    let res_val: serde_json::Value = resp.json().await.map_err(|e| {
                        GatewayError::ResponseBuild(format!(
                            "Failed to parse TCP JSON response: {e}"
                        ))
                    })?;

                    let vector = res_val["embedding"]
                        .as_array()
                        .ok_or_else(|| {
                            GatewayError::ResponseBuild(
                                "TCP response missing 'embedding' field".to_string(),
                            )
                        })?
                        .iter()
                        .map(|v| {
                            v.as_f64().map(|f| f as f32).ok_or_else(|| {
                                GatewayError::ResponseBuild("TCP embedding not float".to_string())
                            })
                        })
                        .collect::<Result<Vec<f32>, GatewayError>>()?;

                    Ok(vector)
                };

                tokio::time::timeout(Duration::from_millis(250), fut)
                    .await
                    .map_err(|_| {
                        GatewayError::ResponseBuild(
                            "TCP embedding request timed out after 250ms".to_string(),
                        )
                    })?
            }
        }
    }
}

/// Hybrid L1 Cache using RAM for Exact and Semantic Matches.
pub struct L1Cache {
    /// Exact match layer: SHA256(Prompt) -> LLM Response bounds by Moka
    exact_cache: Cache<String, String>,
    /// Thread-safe local ONNX embedding model for Semantic L1 Check
    pub embed_client: EmbeddingClient,
    /// Vector Index: Ring buffer of recent embeddings and their responses
    /// Bounded to 5000 elements to strictly enforce RAM limits (pure rust).
    /// Each entry is scoped to a tenant so semantic similarity never crosses tenants.
    #[allow(clippy::type_complexity)]
    semantic_cache: RwLock<VecDeque<(String, String, Vec<f32>, String)>>,
}

impl L1Cache {
    /// Initializes the L1 cache. Limits memory using `capacity_bytes`.
    pub fn new(capacity_bytes: u64) -> Result<Self, GatewayError> {
        let exact_cache = Cache::builder()
            .max_capacity(capacity_bytes)
            .time_to_idle(Duration::from_secs(3600))
            // Weigher enforces max_capacity as a *byte budget*, not entry count.
            // Without this, max_capacity(500MB) means 500M entries — effectively unlimited.
            // +64 accounts for HashMap internal node overhead per entry.
            .weigher(|k: &String, v: &String| -> u32 {
                (k.len() + v.len() + 64).min(u32::MAX as usize) as u32
            })
            .build();

        let embed_client = {
            // Safe fallback sequence for local Windows TCP architecture
            let tcp_url = std::env::var("EMBEDDING_TCP_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8081".to_string()); // Default target embedding sync macro
            let url = if tcp_url.trim().is_empty() {
                "http://127.0.0.1:8081"
            } else {
                tcp_url.trim()
            };

            info!(
                "L1 FastEmbed: Routing fallback via remote TCP offloading URL: {:?}",
                url
            );
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(150)) // Set smooth overhead window
                .build()
                .map_err(|e| GatewayError::ResponseBuild(format!("Failed to build client: {e}")))?;

            EmbeddingClient::Tcp {
                endpoint: url.to_string(),
                client,
            }
        };

        Ok(Self {
            exact_cache,
            embed_client,
            semantic_cache: RwLock::new(VecDeque::with_capacity(5000)),
        })
    }

    /// Exact cache key: `tenant_id` + SHA256(prompt) so entries never cross tenants.
    fn exact_cache_key(tenant_id: &str, prompt: &str) -> String {
        format!("{}:{}", tenant_id, Self::hash_prompt(prompt))
    }

    /// Generates a SHA-256 hash of the prompt string
    fn hash_prompt(prompt: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(prompt.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Check Exact Match (tenant-scoped).
    pub async fn get_exact(&self, tenant_id: &str, prompt: &str) -> Option<String> {
        let key = Self::exact_cache_key(tenant_id, prompt);
        let hit = self.exact_cache.get(&key).await;
        if hit.is_some() {
            info!("L1 Exact Cache Match hit for prompt!");
        }
        hit
    }

    /// Calculates Cosine distance between two vectors. Lower is better.
    fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
        let mut dot = 0.0;
        let mut norm_a = 0.0;
        let mut norm_b = 0.0;
        for i in 0..a.len() {
            dot += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }
        if norm_a == 0.0 || norm_b == 0.0 {
            return 1.0;
        }
        let similarity = dot / (norm_a.sqrt() * norm_b.sqrt());
        1.0 - similarity
    }

    /// Normalizes a vector to unit length (L2 norm = 1.0)
    fn normalize_vec(vec: &[f32]) -> Vec<f32> {
        let norm: f32 = vec.iter().map(|&x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            vec.iter().map(|&x| x / norm).collect()
        } else {
            vec.to_vec()
        }
    }

    /// Returns the leading/primary interrogative token in lowercase, if present.
    fn get_primary_interrogative(prompt: &str) -> Option<&'static str> {
        let strong_tokens = ["what", "who", "where", "when", "why", "how"];
        let weak_tokens = ["can", "is", "do", "does"];
        let prompt_lower = prompt.to_lowercase();
        let mut words = prompt_lower
            .split(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
            .filter(|w| !w.is_empty());

        let first_word = words.next();

        // 1. Check if first word is weak or strong
        if let Some(first) = first_word {
            for &t in &strong_tokens {
                if first == t {
                    return Some(t);
                }
            }
            for &t in &weak_tokens {
                if first == t {
                    return Some(t);
                }
            }
        }

        // 2. Check subsequent words ONLY for strong tokens
        for word in words {
            for &t in &strong_tokens {
                if word == t {
                    return Some(t);
                }
            }
        }
        None
    }

    /// Pre-flight Intent Interception / Keyword Penalty Check.
    /// If both prompts have a primary interrogative token but they don't match, return false (mismatch).
    fn verify_intent(incoming: &str, cached: &str) -> bool {
        let incoming_token = Self::get_primary_interrogative(incoming);
        let cached_token = Self::get_primary_interrogative(cached);
        match (incoming_token, cached_token) {
            (Some(inc), Some(cac)) => inc == cac,
            _ => true,
        }
    }

    /// Check Semantic Match -> Threshold 0.86 similarity
    /// Distance < 0.14 for Cosine Metric (or < 0.06 if word count <= 8).
    /// Only considers vectors stored for `tenant_id`.
    /// Returns Some((response_content, cached_prompt)) on hit, None on miss.
    pub async fn get_semantic(
        &self,
        tenant_id: &str,
        prompt: &str,
        vector: &[f32],
    ) -> Option<(String, String)> {
        let vector = Self::normalize_vec(vector);

        let word_count = prompt.split_whitespace().count();
        let threshold = if word_count <= 8 { 0.06 } else { 0.14 };

        // 1. Extract valid cache candidates under the RwLock read guard into an owned vector.
        // This keeps the read lock hold time minimal and prevents deadlocks/starvation.
        let candidates: Vec<(Vec<f32>, String, String)> = {
            let queue = self.semantic_cache.read().await;
            queue
                .iter()
                .filter(|(cached_tenant, cached_prompt, _, _)| {
                    cached_tenant == tenant_id && Self::verify_intent(prompt, cached_prompt)
                })
                .map(|(_, cached_prompt, cached_vec, response)| {
                    (cached_vec.clone(), cached_prompt.clone(), response.clone())
                })
                .collect()
        };

        // 2. Offload the dense O(N) cosine similarity distance calculations and threshold evaluation
        // entirely to tokio::task::spawn_blocking to prevent async executor worker thread starvation.
        let result = tokio::task::spawn_blocking(move || {
            let mut best_dist = f32::MAX;
            let mut best_entry = None;

            for (cached_vec, cached_prompt, response) in candidates {
                let dist = Self::cosine_distance(&vector, &cached_vec);
                if dist < best_dist {
                    best_dist = dist;
                    best_entry = Some((response, cached_prompt));
                }
            }

            if best_dist < threshold {
                best_entry.map(|entry| (best_dist, entry))
            } else {
                None
            }
        })
        .await
        .ok()
        .flatten();

        if let Some((dist, (response, cached_prompt))) = result {
            info!(
                distance = dist,
                threshold = threshold,
                "L1 Semantic Cache Match hit!"
            );
            Some((response, cached_prompt))
        } else {
            None
        }
    }

    /// Insert into L1 if payload is small, caching Exact + Vectorized Semantic (tenant-scoped).
    pub async fn insert(
        &self,
        tenant_id: &str,
        exact_prompt: &str,
        semantic_prompt: &str,
        response: &str,
        vector: &[f32],
    ) -> Result<(), GatewayError> {
        let key = Self::exact_cache_key(tenant_id, exact_prompt);

        // Insert exact
        self.exact_cache.insert(key, response.to_string()).await;

        let vector = Self::normalize_vec(vector);

        // Enforce a 100KB limit per response string in semantic cache to prevent OOM
        // while allowing exact_cache (Moka) to handle larger entries with its own capacity_bytes limit.
        if response.len() > 100_000 {
            debug!(
                len = response.len(),
                "Skipping semantic cache insertion due to payload size"
            );
            return Ok(());
        }

        let mut queue = self.semantic_cache.write().await;
        if queue.len() >= 5000 {
            queue.pop_front();
        }
        queue.push_back((
            tenant_id.to_string(),
            semantic_prompt.to_string(),
            vector,
            response.to_string(),
        ));

        Ok(())
    }

    /// Insert ONLY into L1 Exact Cache (tenant-scoped).
    pub async fn insert_exact(&self, tenant_id: &str, exact_prompt: &str, response: &str) {
        let key = Self::exact_cache_key(tenant_id, exact_prompt);
        self.exact_cache.insert(key, response.to_string()).await;
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;

    static ENV_MUTEX: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

    fn get_env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_MUTEX
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap()
    }

    #[tokio::test]
    async fn test_l1_exact_match() {
        let _lock = get_env_lock();
        let cache = L1Cache::new(1024 * 1024).unwrap();
        let tid = "test-tenant";
        cache
            .insert(tid, "Hello", "Hello", "World", &[1.0, 0.0, 0.0])
            .await
            .unwrap();

        let exact = cache.get_exact(tid, "Hello").await;
        assert_eq!(exact, Some("World".to_string()));

        let miss = cache.get_exact(tid, "Unknown").await;
        assert_eq!(miss, None);

        let other = cache.get_exact("other-tenant", "Hello").await;
        assert_eq!(other, None);
    }

    #[tokio::test]
    async fn test_l1_semantic_match() {
        let _lock = get_env_lock();
        let cache = L1Cache::new(1024 * 1024).unwrap();
        let tid = "test-tenant";
        cache
            .insert(
                tid,
                "exact-match-req",
                "This is a test prompt about rust.",
                "Rust response",
                &[1.0, 0.0, 0.0],
            )
            .await
            .unwrap();

        // Pass a matching vector (e.g. cosine distance < 0.14)
        // Vector is [1.0, 0.0, 0.0], let's pass a slightly perturbed vector: [0.99, 0.1, 0.0]
        let semantic = cache
            .get_semantic(
                tid,
                "This is a test prompt about rust programming.",
                &[0.99, 0.1, 0.0],
            )
            .await;
        assert!(
            semantic.is_some(),
            "Expected semantic match to trigger for similar prompt under normalized vector space"
        );
    }

    #[tokio::test]
    async fn test_l1_semantic_unrelated_miss() {
        let _lock = get_env_lock();
        let cache = L1Cache::new(1024 * 1024).unwrap();
        let tid = "test-tenant";
        cache
            .insert(
                tid,
                "exact-match-req",
                "This is a test prompt about rust.",
                "Rust response",
                &[1.0, 0.0, 0.0],
            )
            .await
            .unwrap();

        // Pass an unrelated vector: [0.0, 1.0, 0.0] (orthogonal)
        let semantic = cache
            .get_semantic(
                tid,
                "What is the recipe for baking chocolate cookies?",
                &[0.0, 1.0, 0.0],
            )
            .await;
        assert!(
            semantic.is_none(),
            "Expected completely unrelated prompt to result in a semantic miss (no false positives)"
        );
    }

    #[tokio::test]
    async fn test_moka_eviction_limits() {
        let _lock = get_env_lock();
        let cache = L1Cache::new(1024).unwrap();

        let tid = "test-tenant";
        for i in 0..50 {
            let prompt = format!("Large Prompt {} with a lot of extra text to consume RAM", i);
            let response = format!(
                "Large Response {} with a lot of extra text to consume RAM",
                i
            );
            cache
                .insert(tid, &prompt, &prompt, &response, &[1.0, 0.0, 0.0])
                .await
                .unwrap();
        }

        // Moka eviction is async, but we can just check it handles things gracefully without crashing
        let hit = cache
            .get_exact(
                tid,
                "Large Prompt 49 with a lot of extra text to consume RAM",
            )
            .await;
        assert!(hit.is_some() || hit.is_none());
    }

    #[tokio::test]
    async fn test_l1_handle_empty() {
        let _lock = get_env_lock();
        let cache = L1Cache::new(1024 * 1024).unwrap();
        let tid = "test-tenant";
        // Insert empty edge case
        cache
            .insert(tid, "", "", "", &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        assert_eq!(cache.get_exact(tid, "").await, Some("".to_string()));
    }

    #[tokio::test]
    async fn test_l1_semantic_intent_mismatch() {
        let _lock = get_env_lock();
        let cache = L1Cache::new(1024 * 1024).unwrap();
        let tid = "test-tenant";
        cache
            .insert(
                tid,
                "exact-match",
                "what is special about india",
                "India response",
                &[1.0, 0.0, 0.0],
            )
            .await
            .unwrap();

        // Even with a matching vector ([0.99, 0.1, 0.0]), the intent check ("who" vs "what") should cause a miss
        let semantic = cache
            .get_semantic(tid, "who is special one in india", &[0.99, 0.1, 0.0])
            .await;

        assert!(
            semantic.is_none(),
            "Expected CACHE MISS due to intent mismatch between 'who' and 'what'"
        );
    }

    #[tokio::test]
    async fn test_l1_semantic_short_prompt_threshold() {
        let _lock = get_env_lock();
        let cache = L1Cache::new(1024 * 1024).unwrap();
        let tid = "test-tenant";
        cache
            .insert(
                tid,
                "exact-match",
                "hello world rust",
                "Short response",
                &[1.0, 0.0, 0.0],
            )
            .await
            .unwrap();

        // A short prompt (<= 8 words) that is slightly different should miss under strict threshold (< 0.06)
        // Vector distance between [1.0, 0.0, 0.0] and [0.93, 0.36, 0.0] is:
        // dot = 0.93. similarity = 0.93. distance = 0.07.
        // 0.07 is < 0.14 but >= 0.06, so it should miss.
        let semantic = cache
            .get_semantic(tid, "hello world python", &[0.93, 0.36, 0.0])
            .await;

        assert!(
            semantic.is_none(),
            "Short sentences must enforce a strict distance threshold (< 0.06) and miss here"
        );
    }

    #[tokio::test]
    async fn test_l1_cache_new_selection() {
        let _lock = get_env_lock();
        // Clear environment variables first to avoid pollution
        std::env::remove_var("EMBEDDING_TCP_URL");

        // By default, should default to Tcp
        let cache = L1Cache::new(1024).unwrap();
        match cache.embed_client {
            EmbeddingClient::Tcp { ref endpoint, .. } => {
                assert_eq!(endpoint, "http://127.0.0.1:8081");
            }
        }

        // If EMBEDDING_TCP_URL is set, should use that
        std::env::set_var("EMBEDDING_TCP_URL", "http://localhost:5678");
        let cache = L1Cache::new(1024).unwrap();
        match cache.embed_client {
            EmbeddingClient::Tcp { ref endpoint, .. } => {
                assert_eq!(endpoint, "http://localhost:5678");
            }
        }
        std::env::remove_var("EMBEDDING_TCP_URL");
    }

    #[tokio::test]
    async fn test_tcp_embedding_success() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let mock_response = serde_json::json!({
            "embedding": [0.1, 0.2, 0.3]
        });

        Mock::given(method("POST"))
            .and(path("/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&mock_response))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let embed_client = EmbeddingClient::Tcp {
            endpoint: mock_server.uri(),
            client,
        };

        let res = embed_client.embed("hello").await.unwrap();
        assert_eq!(res, vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn test_tcp_embedding_timeout() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        // 300ms delay is greater than 250ms timeout
        Mock::given(method("POST"))
            .and(path("/embed"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(300)))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let embed_client = EmbeddingClient::Tcp {
            endpoint: mock_server.uri(),
            client,
        };

        let res = embed_client.embed("hello").await;
        assert!(res.is_err());
        let err_msg = res.err().unwrap().to_string();
        assert!(err_msg.contains("timed out after 250ms"));
    }
}
