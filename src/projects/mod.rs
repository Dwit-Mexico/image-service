pub mod api_key;
pub mod invalidator;
pub mod repo;
pub mod resolver;
pub mod storage_config;

use chrono::{DateTime, Utc};
use uuid::Uuid;

pub use api_key::{ApiKeyError, ApiKeyHash, GeneratedKey};
pub use resolver::{ProjectResolver, ResolveError};
pub use storage_config::StorageConfig;

/// Proyecto totalmente resuelto: lo que va al cache después del miss
/// (DB query + descifrado).
///
/// `storage_config` está en plano aquí — sus campos sensibles se zeroean
/// cuando el struct se dropea (cache eviction o invalidación).
#[derive(Debug, Clone)]
pub struct ResolvedProject {
    pub id: Uuid,
    pub name: String,
    pub cert_cn: String,
    pub api_key_hash: ApiKeyHash,
    pub storage_config: StorageConfig,
    pub default_container: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}
