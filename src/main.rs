mod config;
mod error;
mod handlers;
mod middleware;
mod processing;
mod storage;

use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    middleware as axum_middleware,
    routing::{get, post},
    Router,
};
use config::{default_container, load_projects, AppState};
use handlers::{batch::batch_upload_handler, health::health_handler, upload::upload_handler};
use middleware::auth_middleware;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() {
    // Cargar .env en desarrollo
    let _ = dotenvy::dotenv();

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    let state = AppState {
        storage: storage::from_env(),
        projects: Arc::new(load_projects()),
        default_container: default_container(),
    };

    // 30 MB máximo por request (cubre batch de varias imágenes)
    const MAX_BODY: usize = 30 * 1024 * 1024;

    // Rutas protegidas (auth middleware)
    let protected = Router::new()
        .route("/upload", post(upload_handler))
        .route("/upload/batch", post(batch_upload_handler))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let app = Router::new()
        .route("/health", get(health_handler))
        .merge(protected)
        .with_state(state);

    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("Escuchando en {addr}");
    axum::serve(listener, app).await.unwrap();
}
