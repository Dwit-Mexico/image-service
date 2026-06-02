use object_store::{aws::AmazonS3Builder, path::Path, ObjectStore};

use super::async_trait;
use super::{StorageError, StorageProvider};

/// Almacena bytes en S3 o S3-compatible (MinIO, Cloudflare R2, etc.).
pub struct S3Storage {
    region: String,
    bucket: String,
    access_key_id: String,
    secret_access_key: String,
    /// Para S3-compatible. None → AWS S3 estándar.
    endpoint: Option<String>,
}

impl S3Storage {
    pub fn new(
        access_key_id: String,
        secret_access_key: String,
        region: String,
        bucket: String,
        endpoint: Option<String>,
    ) -> Result<Self, StorageError> {
        if region.is_empty() {
            return Err(StorageError::Config("region missing".into()));
        }
        if bucket.is_empty() {
            return Err(StorageError::Config("bucket missing".into()));
        }
        Ok(Self {
            region,
            bucket,
            access_key_id,
            secret_access_key,
            endpoint,
        })
    }

    fn store(&self) -> anyhow::Result<impl ObjectStore> {
        let mut builder = AmazonS3Builder::new()
            .with_region(&self.region)
            .with_bucket_name(&self.bucket)
            .with_access_key_id(&self.access_key_id)
            .with_secret_access_key(&self.secret_access_key);
        if let Some(ep) = &self.endpoint {
            builder = builder.with_endpoint(ep).with_allow_http(true);
        }
        Ok(builder.build()?)
    }
}

#[async_trait]
impl StorageProvider for S3Storage {
    async fn upload(
        &self,
        // En S3, `container` se ignora — el bucket es fijo por proyecto.
        // (El parámetro existe en el trait por compatibilidad con Azure.)
        _container: &str,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> anyhow::Result<String> {
        let store = self.store()?;
        let path = Path::from(key);

        let payload = object_store::PutPayload::from(data);
        let mut put_opts = object_store::PutOptions::default();
        put_opts.attributes.insert(
            object_store::Attribute::ContentType,
            content_type.to_string().into(),
        );

        store.put_opts(&path, payload, put_opts).await?;

        // Si hay endpoint custom, lo respetamos; sino la URL S3 estándar.
        let url = match &self.endpoint {
            Some(ep) => format!("{}/{}/{}", ep.trim_end_matches('/'), self.bucket, key),
            None => format!(
                "https://{}.s3.{}.amazonaws.com/{}",
                self.bucket, self.region, key
            ),
        };
        Ok(url)
    }
}
