use std::env;

use object_store::{azure::MicrosoftAzureBuilder, path::Path, ObjectStore};

use super::async_trait;
use super::StorageProvider;

/// Guarda las credenciales y crea un store por container en cada upload.
/// Así soportamos containers dinámicos sin requerir un store por container al inicio.
pub struct AzureStorage {
    account: String,
    access_key: String,
}

impl AzureStorage {
    pub fn from_env() -> Self {
        let conn = env::var("AZURE_STORAGE_CONNECTION_STRING")
            .expect("AZURE_STORAGE_CONNECTION_STRING requerido");

        // Parsear "Key=Val;Key=Val;..." — nota: AccountKey puede contener '='
        let mut account = String::new();
        let mut access_key = String::new();

        for part in conn.split(';') {
            if let Some(v) = part.strip_prefix("AccountName=") {
                account = v.to_string();
            } else if let Some(v) = part.strip_prefix("AccountKey=") {
                access_key = v.to_string();
            }
        }

        assert!(
            !account.is_empty(),
            "AccountName no encontrado en connection string"
        );
        assert!(
            !access_key.is_empty(),
            "AccountKey no encontrado en connection string"
        );

        Self {
            account,
            access_key,
        }
    }

    fn store_for(&self, container: &str) -> anyhow::Result<impl ObjectStore> {
        let store = MicrosoftAzureBuilder::new()
            .with_account(&self.account)
            .with_access_key(&self.access_key)
            .with_container_name(container)
            .build()?;
        Ok(store)
    }
}

#[async_trait]
impl StorageProvider for AzureStorage {
    async fn upload(
        &self,
        container: &str,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> anyhow::Result<String> {
        let store = self.store_for(container)?;
        let path = Path::from(key);

        let payload = object_store::PutPayload::from(data);
        let mut put_opts = object_store::PutOptions::default();
        put_opts.attributes.insert(
            object_store::Attribute::ContentType,
            content_type.to_string().into(),
        );

        store.put_opts(&path, payload, put_opts).await?;

        let url = format!(
            "https://{}.blob.core.windows.net/{}/{}",
            self.account, container, key
        );
        Ok(url)
    }
}
