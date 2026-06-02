//! One-shot: lee proyectos de env vars PROJECT_* (formato legacy
//! `cert_cn:api_key`) y los inserta en Postgres apuntando todos al mismo
//! storage backend que define `AZURE_STORAGE_CONNECTION_STRING`.
//!
//! Idempotente: si un cert_cn ya existe en la DB, lo salta. Las API keys
//! se preservan (`api_key::import`) — los clientes existentes siguen
//! funcionando sin cambiar nada en su lado.
//!
//! Uso:
//!   cargo run --bin seed-from-env
//!
//! Requiere DATABASE_URL + MASTER_KEY_V1 + AZURE_STORAGE_CONNECTION_STRING
//! + las PROJECT_* en el entorno.

use std::env;

use image_service::{
    crypto::{seal, Kek},
    projects::{api_key, repo, storage_config, StorageConfig},
};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt().init();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL missing");
    let default_container = env::var("DEFAULT_CONTAINER").ok();
    let azure_conn = env::var("AZURE_STORAGE_CONNECTION_STRING")
        .expect("AZURE_STORAGE_CONNECTION_STRING missing");
    let kek = Kek::from_env().expect("MASTER_KEY_V1 missing or invalid");

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("pg connect");

    let projects = collect_project_envs();
    if projects.is_empty() {
        tracing::warn!("no PROJECT_* env vars found — nothing to seed");
        return;
    }

    let storage_cfg = StorageConfig::Azure {
        connection_string: azure_conn,
    };
    let storage_json = storage_config::to_json(&storage_cfg).expect("serialize storage cfg");

    let mut inserted = 0usize;
    let mut skipped = 0usize;

    for (env_name, cert_cn, api_key_plain) in projects {
        let existing = repo::find_active_by_cert_cn(&pool, &cert_cn)
            .await
            .expect("lookup");

        if existing.is_some() {
            tracing::info!("skip {env_name} ({cert_cn}): already exists");
            skipped += 1;
            continue;
        }

        let api_key_hash = api_key::import(&api_key_plain);
        let blob = seal(&kek, &storage_json).expect("seal storage cfg");

        let id = repo::insert(
            &pool,
            repo::NewProject {
                name: &env_name,
                cert_cn: &cert_cn,
                api_key_hash: &api_key_hash,
                storage_backend: "azure",
                storage_blob: &blob,
                default_container: default_container.as_deref(),
            },
        )
        .await
        .expect("insert project");

        tracing::info!(
            "inserted {env_name} (id={id}, cert_cn={cert_cn}, prefix={})",
            api_key_hash.prefix
        );
        inserted += 1;
    }

    tracing::info!("done — inserted={inserted} skipped={skipped}");
}

/// Lee env vars con prefijo `PROJECT_` y devuelve (nombre_var, cert_cn, api_key).
fn collect_project_envs() -> Vec<(String, String, String)> {
    env::vars()
        .filter(|(k, _)| k.starts_with("PROJECT_"))
        .filter_map(|(k, v)| {
            let (cn, key) = v.split_once(':')?;
            Some((k, cn.trim().to_lowercase(), key.trim().to_string()))
        })
        .collect()
}
