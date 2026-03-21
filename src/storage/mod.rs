pub mod azure;

pub use async_trait::async_trait;

#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Sube bytes al contenedor/bucket y devuelve la URL pública.
    async fn upload(
        &self,
        container: &str,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> anyhow::Result<String>;
}

pub fn from_env() -> std::sync::Arc<dyn StorageProvider> {
    std::sync::Arc::new(azure::AzureStorage::from_env())
}
