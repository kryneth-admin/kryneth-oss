use reqwest::Client;
use serde_json::json;
use std::sync::Arc;
use tokio::time::Instant;

/// Kryneth Gateway Rust Stress Test
///
/// RUN COMMAND:
/// GATEWAY_TOKEN="your_jwt" cargo run --example stress_test
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(Client::new());
    let url = "http://localhost:8084/v1/chat/completions";
    let token = std::env::var("GATEWAY_TOKEN").unwrap_or_else(|_| "test_token".to_string());

    println!("\u{1f680} Starting Kryneth Gateway Performance Benchmark...");

    // --- Scenario A: Cache Hits (1000 Concurrent Requests) ---
    println!("\n[Scenario A] Testing Cache Throughput (1000 requests)...");
    let start = Instant::now();
    let mut handles = vec![];

    for _ in 0..1000 {
        let c = client.clone();
        let u = url.to_string();
        let t = token.clone();
        handles.push(tokio::spawn(async move {
            let res = c
                .post(&u)
                .header("Authorization", format!("Bearer {}", t))
                .json(&json!({
                    "model": "gpt-4o",
                    "messages": [{"role": "user", "content": "Keep it cached"}]
                }))
                .send()
                .await;

            match res {
                Ok(resp) => resp.status().is_success(),
                Err(_) => false,
            }
        }));
    }

    let results = futures::future::join_all(handles).await;
    let successes = results
        .into_iter()
        .filter(|r| *r.as_ref().unwrap_or(&false))
        .count();
    let duration = start.elapsed();

    println!("  - Success Rate: {}/1000", successes);
    println!("  - Total Time: {:?}", duration);
    println!("  - Requests/sec: {:.2}", 1000.0 / duration.as_secs_f64());

    // --- Scenario B: Heavy PII (Rayon Parallelism) ---
    println!("\n[Scenario B] Testing Heavy PII (50k tokens)...");
    let heavy_payload =
        "Safe text. ".repeat(10000) + "email@example.com" + &"Safe text. ".repeat(10000);

    let start = Instant::now();
    let res = client
        .post(url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": heavy_payload}]
        }))
        .send()
        .await?;

    println!("  - Status: {}", res.status());
    println!("  - PII Scan + Roundtrip Time: {:?}", start.elapsed());

    Ok(())
}
