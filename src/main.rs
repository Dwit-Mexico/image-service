use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    middleware as axum_middleware,
    routing::{get, post},
    Router,
};
use image_service::{
    admin,
    config::AppState,
    crypto::Kek,
    db,
    handlers::{
        audio::upload_audio_handler, batch::batch_upload_handler, health::health_handler,
        upload::upload_handler, video::upload_video_handler,
    },
    middleware::auth_middleware,
    projects::{invalidator, ProjectResolver},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    let kek = Arc::new(Kek::from_env().expect("MASTER_KEY_V1 missing or invalid"));
    let pool = db::connect_and_migrate()
        .await
        .expect("postgres connect/migrate failed");
    let resolver = Arc::new(ProjectResolver::new(pool, Arc::clone(&kek)));

    if let Some(cfg) = invalidator::ValkeyConfig::from_env() {
        let resolver_for_sub = Arc::clone(&resolver);
        tokio::spawn(invalidator::run_subscriber(resolver_for_sub, cfg));
    } else {
        tracing::warn!("VALKEY_SENTINEL_ADDR no seteado — sin invalidación distribuida");
    }

    let admin_state = admin::AdminState::from_env(&kek);
    if admin_state.is_some() {
        tracing::info!("admin UI montada en /admin");
    } else {
        tracing::info!("admin UI deshabilitada (sin ADMIN_PASSWORD_HASH)");
    }

    let state = AppState {
        resolver,
        kek: Some(kek),
        admin: admin_state,
    };

    // Body limits per endpoint:
    //   - images / batch / audio: 30 MB  (images small, audio ≤3 min input)
    //   - video: 300 MB                  (input pre-compression can be hefty)
    const MAX_BODY_DEFAULT: usize = 30 * 1024 * 1024;
    const MAX_BODY_VIDEO: usize = 300 * 1024 * 1024;

    let image_and_audio = Router::new()
        .route("/upload", post(upload_handler))
        .route("/upload/batch", post(batch_upload_handler))
        .route("/upload/audio", post(upload_audio_handler))
        .layer(DefaultBodyLimit::max(MAX_BODY_DEFAULT));

    let video = Router::new()
        .route("/upload/video", post(upload_video_handler))
        .layer(DefaultBodyLimit::max(MAX_BODY_VIDEO));

    let protected = image_and_audio.merge(video).layer(
        axum_middleware::from_fn_with_state(state.clone(), auth_middleware),
    );

    let admin_router = if state.admin.is_some() {
        Some(admin::router(state.clone()))
    } else {
        None
    };

    let mut app = Router::new()
        .route("/health", get(health_handler))
        .merge(protected);
    if let Some(adm) = admin_router {
        app = app.nest("/admin", adm);
    }
    let app = app.with_state(state);

    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("listening on {addr}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();
}
