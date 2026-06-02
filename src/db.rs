//! Pool de Postgres + auto-migración al arrancar.
//!
//! `connect_and_migrate()` corre todas las migraciones de `./migrations`
//! que aún no estén aplicadas. sqlx las embebe en el binario al compilar,
//! así que no hay que distribuir los .sql junto con la imagen.

use std::time::Duration;

use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("DATABASE_URL ausente")]
    MissingUrl,

    #[error("conexión a Postgres: {0}")]
    Connect(#[from] sqlx::Error),

    #[error("migración falló: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub async fn connect_and_migrate() -> Result<PgPool, DbError> {
    let url = std::env::var("DATABASE_URL").map_err(|_| DbError::MissingUrl)?;

    let opts: PgConnectOptions = url.parse()?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(opts)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("migraciones aplicadas");

    Ok(pool)
}
