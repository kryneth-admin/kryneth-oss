//! examples/mock_upstream.rs — High-performance mock LLM server for benchmarking.
//!
//! Listens on http://localhost:8090 and handles:
//!   - /v1/chat/completions (JSON & SSE stream responses)
//!   - /api/v1/compliance/redact (Mock compliance redaction endpoint)

use axum::{
    http::{header::CONTENT_TYPE, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::time::SystemTime;

#[derive(Debug, Deserialize)]
struct ChatRequest {
    model: String,
    #[allow(dead_code)]
    messages: Vec<serde_json::Value>,
    #[serde(default)]
    stream: bool,
}

#[tokio::main]
async fn main() {
    // Build router
    let app = Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/api/v1/compliance/redact", post(handle_compliance_redact))
        .route("/health", axum::routing::get(|| async { "OK" }));

    let addr = SocketAddr::from(([127, 0, 0, 1], 8090));
    println!(
        "🤖 Mock LLM & Compliance Upstream Server running at http://{}",
        addr
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind TCP listener");

    axum::serve(listener, app)
        .await
        .expect("Failed to run Axum server");
}

async fn handle_compliance_redact(Json(payload): Json<serde_json::Value>) -> impl IntoResponse {
    // Return the payload back as-is to simulate successful redaction
    Json(payload)
}

async fn handle_chat_completions(Json(payload): Json<ChatRequest>) -> impl IntoResponse {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let request_id = format!("chatcmpl-mock-{}", uuid::Uuid::new_v4());
    let model = payload.model;

    if payload.stream {
        // Stream response via SSE
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(10);

        let request_id_clone = request_id.clone();
        let model_clone = model.clone();

        tokio::spawn(async move {
            let words = vec![
                "This",
                " is",
                " a",
                " mock",
                " streamed",
                " response",
                " from",
                " the",
                " local",
                " upstream",
                " LLM",
                " server.",
            ];

            for word in words {
                let chunk = json!({
                    "id": request_id_clone,
                    "object": "chat.completion.chunk",
                    "created": now,
                    "model": model_clone,
                    "choices": [
                        {
                            "index": 0,
                            "delta": {
                                "content": word
                            },
                            "finish_reason": serde_json::Value::Null
                        }
                    ]
                });

                let data = format!("data: {}\n\n", serde_json::to_string(&chunk).unwrap());
                if tx.send(Ok(bytes::Bytes::from(data))).await.is_err() {
                    return;
                }
                tokio::task::yield_now().await;
            }

            // Finish reason stop chunk
            let stop_chunk = json!({
                "id": request_id_clone,
                "object": "chat.completion.chunk",
                "created": now,
                "model": model_clone,
                "choices": [
                    {
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop"
                    }
                ]
            });
            let data = format!("data: {}\n\n", serde_json::to_string(&stop_chunk).unwrap());
            let _ = tx.send(Ok(bytes::Bytes::from(data))).await;

            // Send DONE
            let _ = tx.send(Ok(bytes::Bytes::from("data: [DONE]\n\n"))).await;
        });

        let body = axum::body::Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));

        Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"))
            .body(body)
            .unwrap()
    } else {
        // Return standard JSON
        let response = json!({
            "id": request_id,
            "object": "chat.completion",
            "created": now,
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "This is a mock response from the local upstream LLM server."
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 12,
                "total_tokens": 22
            }
        });

        Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .body(axum::body::Body::from(
                serde_json::to_vec(&response).unwrap(),
            ))
            .unwrap()
    }
}
