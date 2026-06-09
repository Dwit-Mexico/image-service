//! Resolver de proyectos con cache local + invalidación opcional vía Valkey.
//!
//! Hot path: `resolve(cert_cn)` → cache hit lookup en memoria. En cache miss:
//! 1. Query a Postgres (`repo::find_active_by_cert_cn`)
//! 2. Envelope decrypt del `storage_config` con la KEK
//! 3. Construye `ResolvedProject` y lo mete al cache
//!
//! Invalidación:
//! - TTL local (30s) — red de seguridad si pub/sub se pierde
//! - Pub/sub Valkey en canal `projects:invalidate` (opcional) —
//!   coordinación entre pods cuando admin edita/revoca

use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use sqlx::PgPool;
use uuid::Uuid;

use crate::crypto::Kek;
use crate::projects::{
    api_key::ApiKeyHash, repo, storage_config, ResolvedProject, StorageConfig,
};

const CACHE_TTL: Duration = Duration::from_secs(30);
const CACHE_CAPACITY: u64 = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("proyecto no encontrado")]
    NotFound,

    #[error("error de DB: {0}")]
    Db(#[from] sqlx::Error),

    #[error("error de descifrado: {0}")]
    Crypto(#[from] crate::crypto::CryptoError),

    #[error("storage_config inválido: {0}")]
    InvalidConfig(#[from] serde_json::Error),

    #[error("api_key_hash en DB corrupto: {0}")]
    Stored(#[from] crate::projects::api_key::ApiKeyError),
}

pub struct ProjectResolver {
    pool: PgPool,
    kek: Arc<Kek>,
    cache: Cache<String, Arc<ResolvedProject>>,
}

impl ProjectResolver {
    /// Acceso a la pool para queries auxiliares (admin UI, CLI).
    pub fn pool(&self) -> PgPool {
        self.pool.clone()
    }

    pub fn new(pool: PgPool, kek: Arc<Kek>) -> Self {
        let cache = Cache::builder()
            .max_capacity(CACHE_CAPACITY)
            .time_to_live(CACHE_TTL)
            .build();
        Self { pool, kek, cache }
    }

    /// Resuelve por cert_cn. Devuelve `NotFound` si no existe o está revocado.
    pub async fn resolve(&self, cert_cn: &str) -> Result<Arc<ResolvedProject>, ResolveError> {
        // moka's get_with deduplica concurrencia: si 100 requests llegan al
        // mismo tiempo con un miss, solo uno hace la carga; el resto espera.
        let key = cert_cn.to_string();
        let result = self
            .cache
            .try_get_with(key, async {
                self.load_from_db(cert_cn)
                    .await
                    .map(Arc::new)
            })
            .await;

        match result {
            Ok(p) => Ok(p),
            Err(arc_err) => Err(unwrap_arc_err(arc_err)),
        }
    }

    /// Invalidar explícitamente (admin update/revoke o pub/sub).
    pub async fn invalidate(&self, cert_cn: &str) {
        self.cache.invalidate(cert_cn).await;
    }

    /// Vaciar todo el cache (e.g. al recibir un señal de reload).
    pub async fn invalidate_all(&self) {
        self.cache.invalidate_all();
    }

    /// Anuncia el `last_used_at` sin bloquear el request.
    /// Errores solo se loggean — no afectan al caller.
    pub fn record_use(&self, id: Uuid) {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            if let Err(e) = repo::touch_last_used(&pool, id).await {
                tracing::warn!("touch_last_used falló para {id}: {e}");
            }
        });
    }

    async fn load_from_db(&self, cert_cn: &str) -> Result<ResolvedProject, ResolveError> {
        let row = repo::find_active_by_cert_cn(&self.pool, cert_cn)
            .await?
            .ok_or(ResolveError::NotFound)?;

        let blob = row.encrypted_blob();
        let plaintext = crate::crypto::open(&self.kek, &blob)?;
        let storage_config: StorageConfig = storage_config::from_json(&plaintext)?;

        let api_key_hash = ApiKeyHash::from_stored(
            &row.api_key_hash,
            &row.api_key_salt,
            row.api_key_prefix.clone(),
        )?;

        Ok(ResolvedProject {
            id: row.id,
            name: row.name,
            cert_cn: row.cert_cn,
            api_key_hash,
            storage_config,
            default_container: row.default_container,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
        })
    }
}

/// moka `try_get_with` envuelve los errores en `Arc<E>` porque puede haber
/// múltiples callers esperando el mismo intento. Convertimos a un error
/// owned para el caller.
fn unwrap_arc_err(e: Arc<ResolveError>) -> ResolveError {
    // Si somos el único holder, hacemos try_unwrap. Si no, replicamos según variante.
    Arc::try_unwrap(e).unwrap_or_else(|arc| match arc.as_ref() {
        ResolveError::NotFound => ResolveError::NotFound,
        other => ResolveError::Db(sqlx::Error::Protocol(other.to_string())),
    })
}
