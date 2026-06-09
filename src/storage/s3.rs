use object_store::{aws::AmazonS3Builder, path::Path, ObjectStore};

use super::async_trait;
use super::{is_aws_virtual_hosted_endpoint, StorageError, StorageProvider};

/// Almacena bytes en S3 o S3-compatible (MinIO, Cloudflare R2, etc.).
///
/// **Reglas de configuración** (validadas en `new`):
///   - AWS S3: NO se especifica `endpoint`. object_store usa virtual-hosted
///     style automáticamente con `region + bucket`. URL: `https://<bucket>.s3.<region>.amazonaws.com/<key>`.
///   - S3-compatible (MinIO/R2/etc.): `endpoint` debe ser la URL base del
///     servicio sin el bucket en el subdominio. object_store usa path-style.
///     URL: `<endpoint>/<bucket>/<key>`.
///
/// Si se pasa una URL virtual-hosted de AWS como `endpoint` (p.ej.
/// `https://<bucket>.s3.<region>.amazonaws.com`), AWS interpreta el subdominio
/// como bucket Y el primer segmento del path-style como Key — el objeto
/// termina bajo Key `<bucket>/<key>` y el contrato `id == Key` se rompe.
pub struct S3Storage {
    region: String,
    bucket: String,
    access_key_id: String,
    secret_access_key: String,
    /// None → AWS S3 estándar (virtual-hosted). Some → S3-compatible (path-style).
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
        if let Some(ep) = &endpoint {
            if is_aws_virtual_hosted_endpoint(ep) {
                return Err(StorageError::Config(format!(
                    "endpoint '{ep}' es virtual-hosted de AWS S3. Para AWS no se \
                     especifica endpoint: el SDK lo construye con region + bucket. \
                     Para S3-compatible (MinIO/R2) usa la URL base del servicio."
                )));
            }
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

    /// URL pública construida con las MISMAS reglas que usa object_store para
    /// ubicar el objeto en S3. Garantía: si esta URL apunta a `<bucket>/<key>`,
    /// el objeto está realmente bajo Key `<key>` (NO `<bucket>/<key>`) y por
    /// tanto el `id` que devolvemos al cliente coincide con la Key en S3.
    pub fn public_url(&self, key: &str) -> String {
        match &self.endpoint {
            Some(ep) => format!("{}/{}/{}", ep.trim_end_matches('/'), self.bucket, key),
            None => format!(
                "https://{}.s3.{}.amazonaws.com/{}",
                self.bucket, self.region, key
            ),
        }
    }
}

#[async_trait]
impl StorageProvider for S3Storage {
    async fn upload(
        &self,
        // En S3, `container` se ignora — el bucket es fijo por proyecto.
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
        Ok(self.public_url(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aws(bucket: &str, region: &str) -> S3Storage {
        S3Storage::new(
            "AKIA".into(),
            "secret".into(),
            region.into(),
            bucket.into(),
            None,
        )
        .unwrap()
    }

    fn minio(bucket: &str, endpoint: &str) -> S3Storage {
        S3Storage::new(
            "AKIA".into(),
            "secret".into(),
            "us-east-1".into(),
            bucket.into(),
            Some(endpoint.into()),
        )
        .unwrap()
    }

    #[test]
    fn aws_url_has_no_double_bucket() {
        // AWS sin endpoint → virtual-hosted. Bucket en host, no en path.
        let s = aws("portento", "mx-central-1");
        let key = "_diag/abc.webp";
        let url = s.public_url(key);
        assert_eq!(url, "https://portento.s3.mx-central-1.amazonaws.com/_diag/abc.webp");
        // El bucket aparece exactamente una vez (en el host).
        assert_eq!(url.matches("portento").count(), 1);
    }

    #[test]
    fn aws_id_equals_key() {
        // El "id" que devuelve el handler es la `key` pasada a upload(),
        // y la URL apunta exactamente a esa key bajo el host del bucket.
        // Por construcción de public_url, el path es `/<key>`, no `/<bucket>/<key>`.
        let s = aws("mybucket", "us-east-1");
        let key = "users/42/avatar.webp";
        let url = s.public_url(key);
        let path_after_host = url
            .splitn(4, '/')
            .nth(3)
            .expect("URL has path component");
        assert_eq!(path_after_host, key, "id (key) debe coincidir con el path");
    }

    #[test]
    fn minio_url_path_style_with_bucket_prefix() {
        // MinIO/R2 → path-style. Bucket va en el path porque el host no
        // identifica el bucket.
        let s = minio("data", "https://minio.local:9000");
        let key = "imgs/x.webp";
        let url = s.public_url(key);
        assert_eq!(url, "https://minio.local:9000/data/imgs/x.webp");
    }

    #[test]
    fn minio_url_trims_trailing_slash_in_endpoint() {
        let s = minio("data", "https://minio.local:9000/");
        assert_eq!(s.public_url("x.webp"), "https://minio.local:9000/data/x.webp");
    }

    #[test]
    fn rejects_aws_virtual_hosted_endpoint_at_new() {
        let res = S3Storage::new(
            "AKIA".into(),
            "secret".into(),
            "mx-central-1".into(),
            "portento".into(),
            Some("https://portento.s3.mx-central-1.amazonaws.com".into()),
        );
        let msg = match res {
            Ok(_) => panic!("debió rechazar el endpoint virtual-hosted"),
            Err(e) => e.to_string(),
        };
        assert!(msg.contains("virtual-hosted"), "msg: {msg}");
    }

    #[test]
    fn rejects_missing_bucket() {
        let res = S3Storage::new(
            "AKIA".into(),
            "secret".into(),
            "us-east-1".into(),
            "".into(),
            None,
        );
        assert!(res.is_err());
    }

    #[test]
    fn rejects_missing_region() {
        let res = S3Storage::new(
            "AKIA".into(),
            "secret".into(),
            "".into(),
            "mybucket".into(),
            None,
        );
        assert!(res.is_err());
    }
}
