//! Capa de acceso a Postgres para `projects`.
//!
//! Las funciones devuelven structs "crudos" (`ProjectRow`) con el ciphertext
//! intacto. El descifrado y la conversión a `ResolvedProject` viven en el
//! resolver — esta capa solo habla SQL.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::crypto::EncryptedBlob;
use crate::projects::api_key::ApiKeyHash;

#[derive(Debug)]
pub struct ProjectRow {
    pub id: Uuid,
    pub name: String,
    pub cert_cn: String,

    pub api_key_hash: Vec<u8>,
    pub api_key_salt: Vec<u8>,
    pub api_key_prefix: String,

    pub storage_backend: String,
    pub storage_ciphertext: Vec<u8>,
    pub storage_nonce: Vec<u8>,
    pub dek_ciphertext: Vec<u8>,
    pub dek_nonce: Vec<u8>,
    pub kek_version: i32,

    pub default_container: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl ProjectRow {
    pub fn encrypted_blob(&self) -> EncryptedBlob {
        EncryptedBlob {
            ciphertext: self.storage_ciphertext.clone(),
            nonce: self.storage_nonce.clone(),
            dek_ciphertext: self.dek_ciphertext.clone(),
            dek_nonce: self.dek_nonce.clone(),
            kek_version: self.kek_version as u32,
        }
    }
}

/// Datos para crear un proyecto nuevo. El caller ya generó la API key
/// (con `api_key::generate()`) y cifró el storage_config (con `crypto::seal()`).
#[derive(Debug)]
pub struct NewProject<'a> {
    pub name: &'a str,
    pub cert_cn: &'a str,
    pub api_key_hash: &'a ApiKeyHash,
    pub storage_backend: &'a str,
    pub storage_blob: &'a EncryptedBlob,
    pub default_container: Option<&'a str>,
}

#[derive(Debug)]
pub struct ProjectSummary {
    pub id: Uuid,
    pub name: String,
    pub cert_cn: String,
    pub api_key_prefix: String,
    pub storage_backend: String,
    pub default_container: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Hot path: lookup en cada request (vía cache miss).
pub async fn find_active_by_cert_cn(
    pool: &PgPool,
    cert_cn: &str,
) -> Result<Option<ProjectRow>, sqlx::Error> {
    sqlx::query_as!(
        ProjectRow,
        r#"
        SELECT
            id, name, cert_cn,
            api_key_hash, api_key_salt, api_key_prefix,
            storage_backend, storage_ciphertext, storage_nonce,
            dek_ciphertext, dek_nonce, kek_version,
            default_container, created_at, last_used_at
        FROM projects
        WHERE cert_cn = $1 AND revoked_at IS NULL
        "#,
        cert_cn
    )
    .fetch_optional(pool)
    .await
}

/// Best-effort: actualiza `last_used_at`. Errores no se propagan al request.
pub async fn touch_last_used(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE projects SET last_used_at = now() WHERE id = $1",
        id
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert(pool: &PgPool, new: NewProject<'_>) -> Result<Uuid, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO projects (
            name, cert_cn,
            api_key_hash, api_key_salt, api_key_prefix,
            storage_backend, storage_ciphertext, storage_nonce,
            dek_ciphertext, dek_nonce, kek_version,
            default_container
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        RETURNING id
        "#,
        new.name,
        new.cert_cn,
        &new.api_key_hash.hash[..],
        &new.api_key_hash.salt[..],
        new.api_key_hash.prefix,
        new.storage_backend,
        new.storage_blob.ciphertext,
        new.storage_blob.nonce,
        new.storage_blob.dek_ciphertext,
        new.storage_blob.dek_nonce,
        new.storage_blob.kek_version as i32,
        new.default_container,
    )
    .fetch_one(pool)
    .await?;
    Ok(row.id)
}

pub async fn list_all(pool: &PgPool) -> Result<Vec<ProjectSummary>, sqlx::Error> {
    sqlx::query_as!(
        ProjectSummary,
        r#"
        SELECT
            id, name, cert_cn, api_key_prefix, storage_backend,
            default_container, created_at, last_used_at, revoked_at
        FROM projects
        ORDER BY created_at DESC
        "#
    )
    .fetch_all(pool)
    .await
}

pub async fn revoke(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let res = sqlx::query!(
        "UPDATE projects SET revoked_at = now(), updated_at = now()
         WHERE id = $1 AND revoked_at IS NULL",
        id
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Re-cifra storage_config con un blob nuevo (para rotación de credenciales).
/// Si `default_container` es `Some`, también lo actualiza; si es `None`, deja
/// el existente (COALESCE).
pub async fn rotate_storage(
    pool: &PgPool,
    id: Uuid,
    backend: &str,
    blob: &EncryptedBlob,
    default_container: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query!(
        r#"
        UPDATE projects SET
            storage_backend    = $2,
            storage_ciphertext = $3,
            storage_nonce      = $4,
            dek_ciphertext     = $5,
            dek_nonce          = $6,
            kek_version        = $7,
            default_container  = COALESCE($8, default_container),
            updated_at         = now()
        WHERE id = $1 AND revoked_at IS NULL
        "#,
        id,
        backend,
        blob.ciphertext,
        blob.nonce,
        blob.dek_ciphertext,
        blob.dek_nonce,
        blob.kek_version as i32,
        default_container,
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}
