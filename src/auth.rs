use axum::{
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::dashboard;

// ─── Data Structures ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}

#[derive(Serialize, Deserialize, Clone)]
struct PanelConfig {
    username: String,
    password_hash: String,
    salt: String,
    must_change_password: bool,
}

#[derive(Serialize)]
struct LoginResponse {
    status: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    must_change_password: Option<bool>,
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

// ─── Path Helpers ────────────────────────────────────────────────────────────

fn config_path() -> Option<PathBuf> {
    dashboard::resolve_data_base_dir().map(|base| base.join("data").join("config.json"))
}

fn secret_key_path() -> Option<PathBuf> {
    dashboard::resolve_data_base_dir().map(|base| base.join("data").join("secret.key"))
}

// ─── Password Hashing ───────────────────────────────────────────────────────
// Uses multi-round SipHash with salt. Adequate for a local panel where the
// config file lives on the same machine. Upgrade to bcrypt/argon2 for
// public-facing deployments.

fn hash_password(password: &str, salt: &str) -> String {
    let combined = format!("minpanel:v1:{}:{}", salt, password);
    let mut h1 = DefaultHasher::new();
    combined.as_bytes().hash(&mut h1);
    let d1 = h1.finish();

    let round2 = format!("{d1:x}:{combined}:secure");
    let mut h2 = DefaultHasher::new();
    round2.as_bytes().hash(&mut h2);
    let d2 = h2.finish();

    let round3 = format!("{d1:x}{d2:x}:final:{salt}");
    let mut h3 = DefaultHasher::new();
    round3.as_bytes().hash(&mut h3);
    let d3 = h3.finish();

    format!("{d1:016x}{d2:016x}{d3:016x}")
}

// ─── Config Management ──────────────────────────────────────────────────────

fn generate_salt() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn generate_secret() -> String {
    format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4())
}

fn load_config() -> Option<PanelConfig> {
    let path = config_path()?;
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_config(config: &PanelConfig) -> Result<(), String> {
    let path = config_path().ok_or("Unable to resolve config path")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {e}"))?;
    }
    let content = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;
    fs::write(&path, content).map_err(|e| format!("Failed to write config: {e}"))
}

fn ensure_config() -> PanelConfig {
    if let Some(config) = load_config() {
        return config;
    }
    let salt = generate_salt();
    let config = PanelConfig {
        username: "admin".to_string(),
        password_hash: hash_password("admin", &salt),
        salt,
        must_change_password: true,
    };
    let _ = save_config(&config);
    config
}

fn load_jwt_secret() -> String {
    static SECRET: OnceLock<String> = OnceLock::new();
    SECRET
        .get_or_init(|| {
            let path = match secret_key_path() {
                Some(p) => p,
                None => return generate_secret(),
            };
            if let Ok(secret) = fs::read_to_string(&path) {
                let trimmed = secret.trim().to_string();
                if !trimmed.is_empty() {
                    return trimmed;
                }
            }
            let secret = generate_secret();
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&path, &secret);
            secret
        })
        .clone()
}

// ─── JWT Operations ─────────────────────────────────────────────────────────

fn create_token(username: &str) -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as usize;
    let claims = Claims {
        sub: username.to_string(),
        exp: now + 86400, // 24 hours
    };
    let secret = load_jwt_secret();
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| format!("Failed to create token: {e}"))
}

fn verify_token(token: &str) -> Result<Claims, String> {
    let secret = load_jwt_secret();
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|e| format!("Invalid token: {e}"))
}

// ─── Cookie Helpers ─────────────────────────────────────────────────────────

fn extract_session_token(headers: &HeaderMap) -> Option<String> {
    // Check Authorization header first
    if let Some(auth) = headers.get(header::AUTHORIZATION) {
        if let Ok(value) = auth.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                return Some(token.trim().to_string());
            }
        }
    }
    // Check cookie
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_header.split(';').find_map(|cookie| {
        cookie
            .trim()
            .strip_prefix("mp_session=")
            .map(|v| v.trim().to_string())
    })
}

fn session_cookie(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "mp_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=86400"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn clear_session_cookie() -> HeaderValue {
    HeaderValue::from_static("mp_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

// ─── Public: Check if request is authenticated ──────────────────────────────

pub fn is_authenticated(headers: &HeaderMap) -> bool {
    extract_session_token(headers)
        .and_then(|t| verify_token(&t).ok())
        .is_some()
}

// ─── Handlers ───────────────────────────────────────────────────────────────

pub async fn login(Json(payload): Json<LoginRequest>) -> impl IntoResponse {
    let config = ensure_config();
    let password_hash = hash_password(&payload.password, &config.salt);

    if payload.username != config.username || password_hash != config.password_hash {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::SET_COOKIE, clear_session_cookie())],
            Json(LoginResponse {
                status: false,
                message: "Invalid username or password".to_string(),
                must_change_password: None,
            }),
        );
    }

    match create_token(&payload.username) {
        Ok(token) => (
            StatusCode::OK,
            [(header::SET_COOKIE, session_cookie(&token))],
            Json(LoginResponse {
                status: true,
                message: "Login successful".to_string(),
                must_change_password: Some(config.must_change_password),
            }),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::SET_COOKIE, clear_session_cookie())],
            Json(LoginResponse {
                status: false,
                message: format!("Authentication error: {e}"),
                must_change_password: None,
            }),
        ),
    }
}

pub async fn change_password(Json(payload): Json<ChangePasswordRequest>) -> impl IntoResponse {
    let config = ensure_config();
    let current_hash = hash_password(&payload.current_password, &config.salt);

    if current_hash != config.password_hash {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "status": false,
                "message": "Current password is incorrect"
            })),
        );
    }
    if payload.new_password.len() < 6 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "status": false,
                "message": "New password must be at least 6 characters"
            })),
        );
    }

    let new_salt = generate_salt();
    let updated = PanelConfig {
        username: config.username,
        password_hash: hash_password(&payload.new_password, &new_salt),
        salt: new_salt,
        must_change_password: false,
    };
    if let Err(e) = save_config(&updated) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "status": false,
                "message": format!("Failed to save new password: {e}")
            })),
        );
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": true,
            "message": "Password changed successfully. Please login again."
        })),
    )
}

pub async fn logout() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::SET_COOKIE, clear_session_cookie())],
        Json(serde_json::json!({ "status": true, "message": "Logged out" })),
    )
}

// ─── Auth Middleware ─────────────────────────────────────────────────────────

/// Called once at startup to create config.json and secret.key if missing.
pub fn initialize() {
    ensure_config();
    let _ = load_jwt_secret();
}

/// Middleware for API routes: returns 401 if not authenticated.
pub async fn require_auth(request: Request<axum::body::Body>, next: Next) -> Response {
    if is_authenticated(request.headers()) {
        return next.run(request).await;
    }
    StatusCode::UNAUTHORIZED.into_response()
}

/// Middleware for page routes: redirects to /login if not authenticated.
pub async fn require_auth_page(request: Request<axum::body::Body>, next: Next) -> Response {
    if is_authenticated(request.headers()) {
        return next.run(request).await;
    }
    axum::response::Redirect::temporary("/login").into_response()
}

/// GET /login — serves the login page HTML.
pub async fn login_page() -> impl IntoResponse {
    match dashboard::load_template("login.html") {
        Ok(page) => (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            )],
            page,
        )
            .into_response(),
        Err(error) => dashboard::template_load_error_response(error),
    }
}
