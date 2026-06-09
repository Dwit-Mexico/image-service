pub mod auth;
pub mod handlers;
pub mod ratelimit;
pub mod templates;

pub use auth::AdminState;

use axum::{
    middleware as axum_middleware,
    routing::{get, post},
    Router,
};

use crate::config::AppState;

/// Construye el sub-router `/admin`. Las rutas internas comparten un middleware
/// que exige sesión válida (excepto `/admin/login` y `/admin/logout`).
pub fn router(state: AppState) -> Router<AppState> {
    let public = Router::new()
        .route("/login", get(handlers::login_get).post(handlers::login_post))
        .route("/logout", post(handlers::logout_post));

    let private = Router::new()
        .route("/", get(handlers::dashboard))
        .route(
            "/projects/new",
            get(handlers::create_get),
        )
        .route("/projects", post(handlers::create_post))
        .route(
            "/projects/:cert_cn",
            get(handlers::project_detail),
        )
        .route(
            "/projects/:cert_cn/revoke",
            post(handlers::revoke_post),
        )
        .route(
            "/projects/:cert_cn/rotate-key",
            post(handlers::rotate_key_post),
        )
        .route(
            "/projects/:cert_cn/rotate-storage",
            post(handlers::rotate_storage_post),
        )
        .layer(axum_middleware::from_fn_with_state(
            state,
            auth::admin_session_required,
        ));

    Router::new().merge(public).merge(private)
}

// Re-export para que `axum_extra::extract::cookie::SignedCookieJar` use la
// signing key vía `FromRef<AppState>`.
use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state
            .admin
            .as_ref()
            .expect("admin not configured")
            .cookie_key
            .clone()
    }
}
