//! api/middleware/auth.rs — Authentication + RBAC middleware.
//!
//! ## Auth Flow
//! 1. CORS preflight → bypass
//! 2. Public paths (/health) → bypass
//! 3. Bearer header → JWT decode or API key DB lookup
//! 4. x-api-key header → API key DB lookup
//! 5. re_token cookie → JWT decode (CSRF guard on state-changing methods)
//!
//! ## RBAC
//! `Claims.role` is decoded from the JWT `"role"` field.
//! Missing field defaults to `Role::Developer` (zero-downtime migration, Option B).
//!
//! `require_admin` middleware enforces `Role::Admin` on all `/v1/admin/*` routes.

use crate::domain::models::{AccountType, AppState};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use axum::{
    body::Body,
    extract::{Extension, State},
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

// ── Role ─────────────────────────────────────────────────────────────────────

/// RBAC role embedded in JWT claims.
/// Missing `"role"` field in a JWT defaults to `Developer` (backward-compatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    #[serde(alias = "Admin", alias = "admin")]
    Admin,
    #[serde(alias = "Developer", alias = "developer")]
    Developer,
    #[serde(alias = "Viewer", alias = "viewer")]
    Viewer,
}

impl Default for Role {
    /// Option B: missing role field → Developer (zero-downtime JWT migration).
    fn default() -> Self {
        Role::Developer
    }
}

// ── Claims ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub tenant_id: String,
    pub exp: usize,
    /// RBAC role. Defaults to `Developer` when absent from the JWT payload
    /// so existing tokens without a "role" field continue to work.
    #[serde(default)]
    pub role: Role,
    /// The tenant's account type. Defaults to `Solo` if missing from older JWTs.
    #[serde(default)]
    pub account_type: AccountType,
}

// ── JWT source discriminant ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JwtSource {
    BearerHeader,
    Cookie,
}

// ── Primary auth middleware ───────────────────────────────────────────────────

pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // 1. CORS Preflight bypass
    if req.method() == axum::http::Method::OPTIONS {
        return Ok(next.run(req).await);
    }

    // 2. Public endpoint bypass
    let path = req.uri().path();
    if path == "/health" || path == "/v1/health" {
        return Ok(next.run(req).await);
    }

    // 2.5. OSS Localhost admin bypass check
    {
        let is_loopback = if let Some(axum::extract::ConnectInfo(addr)) =
            req.extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        {
            addr.ip().is_loopback()
        } else {
            false
        };

        let is_localhost_header =
            if let Some(host_val) = req.headers().get(axum::http::header::HOST) {
                if let Ok(host_str) = host_val.to_str() {
                    let host_part = host_str.split(':').next().unwrap_or(host_str);
                    host_part == "localhost"
                        || host_part == "127.0.0.1"
                        || host_part == "[::1]"
                        || host_part == "::1"
                } else {
                    false
                }
            } else {
                false
            };

        if is_loopback || is_localhost_header {
            let claims = Claims {
                sub: "localhost_admin".to_string(),
                tenant_id: "00000000-0000-0000-0000-000000000000".to_string(),
                exp: 0,
                role: Role::Admin,
                account_type: AccountType::Solo,
            };
            let mut req = req;
            req.headers_mut().insert(
                "x-tenant-id",
                axum::http::HeaderValue::from_static("00000000-0000-0000-0000-000000000000"),
            );
            req.extensions_mut().insert(claims);
            return Ok(next.run(req).await);
        }
    }

    // 3. Token extraction — precedence: Bearer > x-api-key > Cookie
    #[derive(Clone, Copy)]
    enum TokenKind {
        ApiKey,
        Jwt { source: JwtSource },
    }

    let mut token_opt: Option<(String, TokenKind)> = None;

    if let Some(auth_header) = req.headers().get(axum::http::header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                if token.starts_with("re_live_") {
                    token_opt = Some((token.to_string(), TokenKind::ApiKey));
                } else {
                    token_opt = Some((
                        token.to_string(),
                        TokenKind::Jwt {
                            source: JwtSource::BearerHeader,
                        },
                    ));
                }
            }
        }
    }

    if token_opt.is_none() {
        if let Some(key_header) = req.headers().get("x-api-key") {
            if let Ok(token) = key_header.to_str() {
                if token.starts_with("re_live_") {
                    token_opt = Some((token.to_string(), TokenKind::ApiKey));
                }
            }
        }
    }

    // Cookie fallback (CSRF guard applied inside handle_jwt)
    if token_opt.is_none() {
        if let Some(cookie_header) = req.headers().get(header::COOKIE) {
            if let Ok(cookie_str) = cookie_header.to_str() {
                for cookie in cookie_str.split(';') {
                    let cookie = cookie.trim();
                    if let Some(token) = cookie.strip_prefix("re_token=") {
                        if token.split('.').count() == 3 && !token.is_empty() {
                            token_opt = Some((
                                token.to_string(),
                                TokenKind::Jwt {
                                    source: JwtSource::Cookie,
                                },
                            ));
                            tracing::debug!("JWT extracted from re_token cookie");
                            break;
                        }
                    }
                }
            }
        }
    }

    match token_opt {
        Some((token, TokenKind::ApiKey)) => handle_api_key(&state, &token, req, next).await,
        Some((token, TokenKind::Jwt { source })) => handle_jwt(&token, source, req, next).await,
        None => {
            if path.starts_with("/v1/chat") {
                tracing::warn!("Missing credentials for {} {}", req.method(), path);
            } else {
                tracing::debug!("Unauthenticated request to {} {} — 401", req.method(), path);
            }
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

// ── RBAC: require_admin ───────────────────────────────────────────────────────

/// Axum middleware that enforces `Role::Admin`.
/// Applied to all `/v1/admin/*` routes in `routes.rs`.
/// Must be layered AFTER `auth_middleware` (so `Claims` is already injected).
pub async fn require_admin(
    Extension(claims): Extension<Claims>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if claims.role != Role::Admin {
        tracing::warn!(
            sub = %claims.sub,
            role = ?claims.role,
            path = %req.uri().path(),
            "Access denied: Admin role required"
        );
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(req).await)
}

// ── RBAC: require_team_plan ───────────────────────────────────────────────────

/// Axum middleware that restricts features to Team or Enterprise plans.
pub async fn require_team_plan(
    Extension(claims): Extension<Claims>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, Response> {
    if claims.account_type == AccountType::Solo {
        tracing::warn!(
            sub = %claims.sub,
            account_type = ?claims.account_type,
            path = %req.uri().path(),
            "Access denied: Team plan required"
        );
        let body = Json(serde_json::json!({
            "error": "Forbidden",
            "message": "Upgrade to a Team plan to access this feature"
        }));
        return Err((StatusCode::FORBIDDEN, body).into_response());
    }
    Ok(next.run(req).await)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

pub fn verify_jwt(token: &str) -> Result<Claims, StatusCode> {
    let secret = std::env::var("JWT_SECRET").map_err(|_| {
        tracing::error!("CRITICAL: JWT_SECRET environment variable is missing!");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_exp = true;

    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|e| {
        tracing::warn!("JWT verification failed: {}", e);
        StatusCode::UNAUTHORIZED
    })
}

async fn handle_jwt(
    token: &str,
    source: JwtSource,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // CSRF guard: cookie-authenticated state-changing requests require x-csrf-token.
    if source == JwtSource::Cookie {
        let m = req.method();
        let is_state_changing = m != axum::http::Method::GET
            && m != axum::http::Method::HEAD
            && m != axum::http::Method::OPTIONS;

        if is_state_changing {
            let csrf_present = req
                .headers()
                .get("x-csrf-token")
                .and_then(|v| v.to_str().ok())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);

            if !csrf_present {
                tracing::warn!(
                    method = %m,
                    path = %req.uri().path(),
                    "Cookie-auth rejected: missing x-csrf-token"
                );
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    let claims = verify_jwt(token)?;

    let tenant_id = Uuid::parse_str(&claims.tenant_id).map_err(|_| {
        tracing::warn!("Invalid UUID in tenant_id claim");
        StatusCode::UNAUTHORIZED
    })?;

    req.headers_mut().insert(
        "x-tenant-id",
        axum::http::HeaderValue::from_str(&tenant_id.to_string())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );

    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}

async fn handle_api_key(
    state: &Arc<AppState>,
    api_key: &str,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    let key_hash = hex::encode(hasher.finalize());

    let (tenant_id_str, account_type_str) = state
        .auth_resolver
        .resolve_api_key(&key_hash)
        .await
        .map_err(|e| {
            tracing::error!("Auth resolution error: {:?}", e);
            StatusCode::UNAUTHORIZED
        })?;

    let account_type = match account_type_str.as_str() {
        "team" => AccountType::Team,
        "enterprise" => AccountType::Enterprise,
        _ => AccountType::Solo,
    };

    req.headers_mut().insert(
        "x-tenant-id",
        axum::http::HeaderValue::from_str(&tenant_id_str)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );

    // API keys inject Developer role — they are programmatic, not admin sessions.
    req.extensions_mut().insert(Claims {
        sub: "api_key".to_string(),
        tenant_id: tenant_id_str,
        exp: 0,
        role: Role::Developer,
        account_type,
    });

    Ok(next.run(req).await)
}

// ── AES-256-GCM decryption helper ────────────────────────────────────────────

pub fn decrypt_api_key(encrypted_data: &[u8]) -> Result<String, &'static str> {
    let master_key = std::env::var("AES_MASTER_KEY").map_err(|_| "AES_MASTER_KEY missing")?;
    if master_key.len() != 32 || encrypted_data.len() < 12 {
        return Err("Invalid master key length or encrypted data too short");
    }

    let key = Key::<Aes256Gcm>::from_slice(master_key.as_bytes());
    let cipher = Aes256Gcm::new(key);

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Decryption failed")?;
    String::from_utf8(plaintext).map_err(|_| "Decrypted key is not valid UTF-8")
}
