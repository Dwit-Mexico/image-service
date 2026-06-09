//! Auth para `/admin/*`: password (argon2id) + cookie de sesión firmada.
//!
//! La signing key de la cookie se deriva de `MASTER_KEY_V1` con un contexto
//! distinto al de envelope encryption, así no compartimos la KEK directamente.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, Key, SameSite, SignedCookieJar};
use rand::rngs::OsRng;

use crate::config::AppState;
use crate::crypto::Kek;

pub const SESSION_COOKIE: &str = "image_admin_session";
pub const SESSION_TTL_SECS: u64 = 4 * 60 * 60;

#[derive(Clone)]
pub struct AdminState {
    pub username: String,
    pub password_hash: String,
    pub cookie_key: Key,
    pub login_rl: Arc<crate::admin::ratelimit::LoginRateLimit>,
}

impl AdminState {
    /// Construye desde env vars. Devuelve `None` si admin no está configurado
    /// (sin `ADMIN_PASSWORD_HASH`), en cuyo caso `/admin/*` no se monta.
    pub fn from_env(kek: &Kek) -> Option<Self> {
        let password_hash = std::env::var("ADMIN_PASSWORD_HASH").ok()?;
        let username = std::env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
        // Deriva la signing key de la cookie del MASTER_KEY con un contexto
        // distinto (HMAC funciona como KDF en este caso simple).
        let derived = derive_cookie_key(kek);
        Some(Self {
            username,
            password_hash,
            cookie_key: Key::from(&derived),
            login_rl: Arc::new(crate::admin::ratelimit::LoginRateLimit::new()),
        })
    }
}

fn derive_cookie_key(kek: &Kek) -> [u8; 64] {
    // `Key::from` requiere ≥ 64 bytes. Dos bloques HMAC concatenados.
    let mac1 = kek.derive_subkey(b"admin_cookie_signing_v1");
    let mac2 = kek.derive_subkey(b"admin_cookie_signing_v2");
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(&mac1);
    out[32..].copy_from_slice(&mac2);
    out
}

/// Hash de un password (argon2id) con params razonables para login interactivo.
/// Tarda ~50-100ms — suficientemente lento contra bruteforce, no insoportable
/// para un humano logueándose.
pub fn hash_password(plain: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("argon2 hash: {e}"))
}

pub fn verify_password(plain: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok()
}

/// Construye la cookie de sesión firmada con expiración a `SESSION_TTL_SECS`.
pub fn build_session_cookie(username: &str) -> Cookie<'static> {
    let exp = now_secs() + SESSION_TTL_SECS;
    let value = format!("{username}|{exp}");
    Cookie::build((SESSION_COOKIE, value))
        .path("/admin")
        .secure(true)
        .http_only(true)
        .same_site(SameSite::Strict)
        .max_age(time::Duration::seconds(SESSION_TTL_SECS as i64))
        .build()
}

pub fn build_logout_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, ""))
        .path("/admin")
        .secure(true)
        .http_only(true)
        .same_site(SameSite::Strict)
        .max_age(time::Duration::seconds(0))
        .build()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Si la sesión es válida, devuelve el username. Sino, `None`.
pub fn session_user(jar: &SignedCookieJar) -> Option<String> {
    let cookie = jar.get(SESSION_COOKIE)?;
    let value = cookie.value();
    let (user, exp_str) = value.split_once('|')?;
    let exp: u64 = exp_str.parse().ok()?;
    if now_secs() > exp {
        return None;
    }
    Some(user.to_string())
}

/// Middleware: bloquea `/admin/*` (excepto `/admin/login`) si no hay sesión.
pub async fn admin_session_required(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    request: Request,
    next: Next,
) -> Response {
    let admin = match &state.admin {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "admin not configured").into_response(),
    };
    let _ = admin; // jar uses key via state below, no need here
    if session_user(&jar).is_some() {
        next.run(request).await
    } else {
        Redirect::to("/admin/login").into_response()
    }
}
