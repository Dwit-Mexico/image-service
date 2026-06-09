//! Handlers de las rutas `/admin/*`. Cada uno renderiza un template de askama.

use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Form,
};
use axum_extra::extract::cookie::SignedCookieJar;
use base64::Engine as _;
use serde::Deserialize;
use std::net::SocketAddr;

use crate::admin::auth::{
    build_logout_cookie, build_session_cookie, session_user, verify_password,
};
use crate::admin::templates::{
    CreatedTpl, CreateTpl, DashboardRow, DashboardTpl, LoginTpl, ProjectTpl,
};
use crate::config::AppState;
use crate::crypto::seal;
use crate::projects::{api_key, invalidator, repo, storage_config, StorageConfig};
use crate::storage;

// ────────────────── login / logout ──────────────────

pub async fn login_get(jar: SignedCookieJar) -> Response {
    if session_user(&jar).is_some() {
        return Redirect::to("/admin").into_response();
    }
    LoginTpl { error: None }.into_response()
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

pub async fn login_post(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let Some(admin) = state.admin.as_ref() else {
        return (StatusCode::NOT_FOUND, "admin not configured").into_response();
    };

    // Rate limit por IP. Tomamos X-Forwarded-For si viene del gateway, sino la
    // de conexión directa.
    let ip = client_ip(&headers, remote);
    if !admin.login_rl.try_acquire(&ip) {
        return LoginTpl {
            error: Some("demasiados intentos — intenta de nuevo en 5 min".into()),
        }
        .into_response();
    }

    if form.username != admin.username || !verify_password(&form.password, &admin.password_hash) {
        return LoginTpl {
            error: Some("credenciales inválidas".into()),
        }
        .into_response();
    }

    let cookie = build_session_cookie(&form.username);
    let jar = jar.add(cookie);
    (jar, Redirect::to("/admin")).into_response()
}

pub async fn logout_post(jar: SignedCookieJar) -> Response {
    let jar = jar.add(build_logout_cookie());
    (jar, Redirect::to("/admin/login")).into_response()
}

fn client_ip(headers: &HeaderMap, remote: SocketAddr) -> String {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            return first.trim().to_string();
        }
    }
    remote.ip().to_string()
}

// ────────────────── dashboard ──────────────────

#[derive(Deserialize, Default)]
pub struct DashboardQuery {
    flash: Option<String>,
}

pub async fn dashboard(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Query(q): Query<DashboardQuery>,
) -> Response {
    let user = match session_user(&jar) {
        Some(u) => u,
        None => return Redirect::to("/admin/login").into_response(),
    };

    let rows = match repo::list_all(&state.resolver.pool()).await {
        Ok(r) => r,
        Err(e) => return server_error(format!("list_all: {e}")),
    };

    let projects = rows
        .into_iter()
        .map(|r| DashboardRow {
            id: r.id.to_string(),
            name: r.name,
            cert_cn: r.cert_cn,
            api_key_prefix: r.api_key_prefix,
            backend: r.storage_backend,
            container: r.default_container.unwrap_or_else(|| "—".into()),
            last_used: r
                .last_used_at
                .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "—".into()),
            status: if r.revoked_at.is_some() {
                "revoked"
            } else {
                "active"
            },
        })
        .collect();

    DashboardTpl {
        user,
        projects,
        flash: q.flash,
    }
    .into_response()
}

// ────────────────── project detail ──────────────────

#[derive(Deserialize, Default)]
pub struct ProjectQuery {
    flash: Option<String>,
    new_key: Option<String>,
}

pub async fn project_detail(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(cert_cn): Path<String>,
    Query(q): Query<ProjectQuery>,
) -> Response {
    let user = match session_user(&jar) {
        Some(u) => u,
        None => return Redirect::to("/admin/login").into_response(),
    };

    let row = match repo::find_by_cert_cn_any(&state.resolver.pool(), &cert_cn).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "proyecto no encontrado").into_response(),
        Err(e) => return server_error(format!("find: {e}")),
    };

    ProjectTpl {
        user,
        id: row.id.to_string(),
        name: row.name,
        cert_cn: row.cert_cn,
        api_key_prefix: row.api_key_prefix,
        backend: row.storage_backend,
        container: row.default_container.unwrap_or_else(|| "—".into()),
        created_at: row.created_at,
        last_used: row.last_used_at,
        revoked: row.revoked_at.is_some(),
        csrf_token: csrf_for(&jar),
        flash: q.flash,
        newly_generated_key: q.new_key,
    }
    .into_response()
}

// ────────────────── create ──────────────────

#[derive(Deserialize)]
pub struct CreateQuery {
    backend: Option<String>,
}

pub async fn create_get(jar: SignedCookieJar, Query(q): Query<CreateQuery>) -> Response {
    let user = match session_user(&jar) {
        Some(u) => u,
        None => return Redirect::to("/admin/login").into_response(),
    };
    let backend = q.backend.unwrap_or_else(|| "azure".into());
    if backend != "azure" && backend != "s3" {
        return (StatusCode::BAD_REQUEST, "backend inválido").into_response();
    }
    CreateTpl {
        user,
        backend,
        error: None,
        csrf_token: csrf_for(&jar),
    }
    .into_response()
}

#[derive(Deserialize)]
pub struct CreateForm {
    csrf: String,
    backend: String,
    name: String,
    cert_cn: String,
    // azure
    connection_string: Option<String>,
    default_container: Option<String>,
    // s3
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    region: Option<String>,
    bucket: Option<String>,
    endpoint: Option<String>,
}

pub async fn create_post(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Form(form): Form<CreateForm>,
) -> Response {
    let user = match session_user(&jar) {
        Some(u) => u,
        None => return Redirect::to("/admin/login").into_response(),
    };
    if !csrf_valid(&jar, &form.csrf) {
        return (StatusCode::FORBIDDEN, "csrf inválido").into_response();
    }

    let cert_cn = form.cert_cn.trim().to_lowercase();
    let (cfg, default_container) = match form.backend.as_str() {
        "azure" => {
            let conn = match form.connection_string.as_deref().filter(|s| !s.is_empty()) {
                Some(c) => c.to_string(),
                None => {
                    return CreateTpl {
                        user,
                        backend: form.backend,
                        error: Some("connection_string es requerido".into()),
                        csrf_token: csrf_for(&jar),
                    }
                    .into_response();
                }
            };
            let cfg = StorageConfig::Azure {
                connection_string: conn,
            };
            (cfg, form.default_container.filter(|s| !s.is_empty()))
        }
        "s3" => {
            let access = form.access_key_id.clone().unwrap_or_default();
            let secret = form.secret_access_key.clone().unwrap_or_default();
            let region = form.region.clone().unwrap_or_default();
            let bucket = form.bucket.clone().unwrap_or_default();
            if access.is_empty() || secret.is_empty() || region.is_empty() || bucket.is_empty() {
                return CreateTpl {
                    user,
                    backend: form.backend,
                    error: Some("access_key, secret_key, region y bucket requeridos".into()),
                    csrf_token: csrf_for(&jar),
                }
                .into_response();
            }
            let cfg = StorageConfig::S3 {
                access_key_id: access,
                secret_access_key: secret,
                region,
                bucket,
                endpoint: form.endpoint.clone().filter(|s| !s.is_empty()),
            };
            (cfg, None)
        }
        _ => return (StatusCode::BAD_REQUEST, "backend inválido").into_response(),
    };

    if let Err(msg) = storage::validate(&cfg) {
        return CreateTpl {
            user,
            backend: form.backend,
            error: Some(msg),
            csrf_token: csrf_for(&jar),
        }
        .into_response();
    }

    let key = api_key::generate();
    let storage_json = match storage_config::to_json(&cfg) {
        Ok(j) => j,
        Err(e) => return server_error(format!("serialize: {e}")),
    };
    let kek = match &state.kek {
        Some(k) => k,
        None => return server_error("KEK ausente".to_string()),
    };
    let blob = match seal(kek, &storage_json) {
        Ok(b) => b,
        Err(e) => return server_error(format!("seal: {e}")),
    };

    let insert_res = repo::insert(
        &state.resolver.pool(),
        repo::NewProject {
            name: &form.name,
            cert_cn: &cert_cn,
            api_key_hash: &key.hash,
            storage_backend: &form.backend,
            storage_blob: &blob,
            default_container: default_container.as_deref(),
        },
    )
    .await;

    if let Err(e) = insert_res {
        return CreateTpl {
            user,
            backend: form.backend,
            error: Some(format!("insert falló: {e}")),
            csrf_token: csrf_for(&jar),
        }
        .into_response();
    }

    CreatedTpl {
        user,
        cert_cn,
        plaintext_key: key.plaintext,
    }
    .into_response()
}

// ────────────────── revoke ──────────────────

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

pub async fn revoke_post(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(cert_cn): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    if session_user(&jar).is_none() {
        return Redirect::to("/admin/login").into_response();
    }
    if !csrf_valid(&jar, &f.csrf) {
        return (StatusCode::FORBIDDEN, "csrf inválido").into_response();
    }
    let row = match repo::find_active_by_cert_cn(&state.resolver.pool(), &cert_cn).await {
        Ok(Some(r)) => r,
        _ => return Redirect::to("/admin").into_response(),
    };
    let _ = repo::revoke(&state.resolver.pool(), row.id).await;
    publish_invalidation(&cert_cn).await;
    Redirect::to(&format!("/admin?flash=revocado: {cert_cn}")).into_response()
}

// ────────────────── rotate key ──────────────────

pub async fn rotate_key_post(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(cert_cn): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    if session_user(&jar).is_none() {
        return Redirect::to("/admin/login").into_response();
    }
    if !csrf_valid(&jar, &f.csrf) {
        return (StatusCode::FORBIDDEN, "csrf inválido").into_response();
    }
    let row = match repo::find_active_by_cert_cn(&state.resolver.pool(), &cert_cn).await {
        Ok(Some(r)) => r,
        _ => return Redirect::to("/admin").into_response(),
    };
    let new = api_key::generate();
    let res = sqlx::query!(
        "UPDATE projects SET api_key_hash = $2, api_key_salt = $3, api_key_prefix = $4, updated_at = now() WHERE id = $1",
        row.id,
        &new.hash.hash[..],
        &new.hash.salt[..],
        new.hash.prefix,
    )
    .execute(&state.resolver.pool())
    .await;
    if let Err(e) = res {
        return server_error(format!("rotate-key: {e}"));
    }
    publish_invalidation(&cert_cn).await;
    Redirect::to(&format!(
        "/admin/projects/{cert_cn}?new_key={}",
        urlencoding::encode(&new.plaintext)
    ))
    .into_response()
}

// ────────────────── rotate storage ──────────────────

#[derive(Deserialize)]
pub struct RotateStorageForm {
    csrf: String,
    backend: String,
    connection_string: Option<String>,
    default_container: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    region: Option<String>,
    bucket: Option<String>,
    endpoint: Option<String>,
}

pub async fn rotate_storage_post(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(cert_cn): Path<String>,
    Form(f): Form<RotateStorageForm>,
) -> Response {
    if session_user(&jar).is_none() {
        return Redirect::to("/admin/login").into_response();
    }
    if !csrf_valid(&jar, &f.csrf) {
        return (StatusCode::FORBIDDEN, "csrf inválido").into_response();
    }
    let row = match repo::find_active_by_cert_cn(&state.resolver.pool(), &cert_cn).await {
        Ok(Some(r)) => r,
        _ => return Redirect::to("/admin").into_response(),
    };

    let cfg = match f.backend.as_str() {
        "azure" => {
            let Some(conn) = f.connection_string.filter(|s| !s.is_empty()) else {
                return (StatusCode::BAD_REQUEST, "connection_string requerido").into_response();
            };
            StorageConfig::Azure {
                connection_string: conn,
            }
        }
        "s3" => StorageConfig::S3 {
            access_key_id: f.access_key_id.unwrap_or_default(),
            secret_access_key: f.secret_access_key.unwrap_or_default(),
            region: f.region.unwrap_or_default(),
            bucket: f.bucket.unwrap_or_default(),
            endpoint: f.endpoint.filter(|s| !s.is_empty()),
        },
        _ => return (StatusCode::BAD_REQUEST, "backend inválido").into_response(),
    };

    if let Err(msg) = storage::validate(&cfg) {
        return Redirect::to(&format!(
            "/admin/projects/{cert_cn}?flash={}",
            urlencoding::encode(&msg)
        ))
        .into_response();
    }

    let kek = match &state.kek {
        Some(k) => k,
        None => return server_error("KEK ausente".to_string()),
    };
    let storage_json = match storage_config::to_json(&cfg) {
        Ok(j) => j,
        Err(e) => return server_error(format!("serialize: {e}")),
    };
    let blob = match seal(kek, &storage_json) {
        Ok(b) => b,
        Err(e) => return server_error(format!("seal: {e}")),
    };
    let new_container = f.default_container.filter(|s| !s.is_empty());

    let res = repo::rotate_storage(
        &state.resolver.pool(),
        row.id,
        &f.backend,
        &blob,
        new_container.as_deref(),
    )
    .await;
    if let Err(e) = res {
        return server_error(format!("rotate-storage: {e}"));
    }
    publish_invalidation(&cert_cn).await;
    Redirect::to(&format!(
        "/admin/projects/{cert_cn}?flash=storage rotado"
    ))
    .into_response()
}

// ────────────────── helpers ──────────────────

/// CSRF token: derivamos del valor de la cookie de sesión. Solo válido para
/// usuarios autenticados (la cookie misma ya está firmada).
fn csrf_for(jar: &SignedCookieJar) -> String {
    use sha2::{Digest, Sha256};
    let value = jar
        .get(crate::admin::auth::SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .unwrap_or_default();
    let digest = Sha256::digest(value.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn csrf_valid(jar: &SignedCookieJar, token: &str) -> bool {
    use subtle::ConstantTimeEq;
    let expected = csrf_for(jar);
    expected.as_bytes().ct_eq(token.as_bytes()).into()
}

async fn publish_invalidation(cert_cn: &str) {
    if let Some(cfg) = invalidator::ValkeyConfig::from_env() {
        let _ = invalidator::publish_invalidation(&cfg, cert_cn).await;
    }
}

fn server_error(msg: String) -> Response {
    tracing::error!("admin: {msg}");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}

