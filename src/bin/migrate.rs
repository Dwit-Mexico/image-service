//! Aplica las migraciones pendientes contra DATABASE_URL y termina.
//! Útil en local (`cargo run --bin migrate`) o como step previo al rollout.

use image_service::db;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt().init();

    match db::connect_and_migrate().await {
        Ok(_) => tracing::info!("ok"),
        Err(e) => {
            tracing::error!("migration failed: {e}");
            std::process::exit(1);
        }
    }
}
