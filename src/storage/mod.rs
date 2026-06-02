pub mod azure;
pub mod s3;

use std::sync::Arc;

pub use async_trait::async_trait;
use thiserror::Error;

use crate::projects::StorageConfig;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage config inválido: {0}")]
    Config(String),
}

#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Sube bytes al backend y devuelve la URL pública (o accesible).
    async fn upload(
        &self,
        container: &str,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> anyhow::Result<String>;
}

/// Factory: construye el `StorageProvider` correcto según el `StorageConfig`
/// resuelto del proyecto.
pub fn build(cfg: &StorageConfig) -> Result<Arc<dyn StorageProvider>, StorageError> {
    match cfg {
        StorageConfig::Azure { connection_string } => {
            let s = azure::AzureStorage::from_connection_string(connection_string)?;
            Ok(Arc::new(s))
        }
        StorageConfig::S3 {
            access_key_id,
            secret_access_key,
            region,
            bucket,
            endpoint,
        } => {
            let s = s3::S3Storage::new(
                access_key_id.clone(),
                secret_access_key.clone(),
                region.clone(),
                bucket.clone(),
                endpoint.clone(),
            )?;
            Ok(Arc::new(s))
        }
    }
}
