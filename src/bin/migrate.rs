//! Aplica las migraciones pendientes contra DATABASE_URL y termina.
//! Útil en local (`cargo run --bin migrate`) o como step previo al rollout.

use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt().init();

    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL ausente");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("conexión Postgres");

    match sqlx::migrate!("./migrations").run(&pool).await {
        Ok(_) => tracing::info!("migraciones aplicadas"),
        Err(e) => {
            tracing::error!("migración falló: {e}");
            std::process::exit(1);
        }
    }
}
