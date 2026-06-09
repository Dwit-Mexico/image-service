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

/// Pre-flight validation que pueden llamar la UI admin y el CLI antes de
/// persistir un proyecto. Evita que una misconfig se siente en la DB y
/// luego falle silenciosamente en cada upload.
pub fn validate(cfg: &StorageConfig) -> Result<(), String> {
    if let StorageConfig::S3 {
        endpoint: Some(ep),
        ..
    } = cfg
    {
        if is_aws_virtual_hosted_endpoint(ep) {
            return Err(format!(
                "endpoint '{ep}' parece una URL virtual-hosted de AWS S3 \
                 (formato '<bucket>.s3.*.amazonaws.com'). Para AWS S3 NO se especifica \
                 endpoint: el SDK usa virtual-hosted style automáticamente con region + \
                 bucket. Si apuntas a MinIO/R2/otro S3-compatible, usa la URL base del \
                 servicio sin el bucket en el subdominio (p.ej. \
                 'https://<account>.r2.cloudflarestorage.com')."
            ));
        }
    }
    Ok(())
}

/// `true` si el host del endpoint corresponde a la forma virtual-hosted de
/// AWS S3 (`<bucket>.s3<.opt>.amazonaws.com`). NO match para:
///   - endpoints S3-compatible (R2, MinIO, etc.)
///   - endpoints AWS path-style sin bucket en subdominio (`s3.<region>.amazonaws.com`)
pub fn is_aws_virtual_hosted_endpoint(endpoint: &str) -> bool {
    let host = endpoint
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    if !host.ends_with(".amazonaws.com") {
        return false;
    }
    let stripped = &host[..host.len() - ".amazonaws.com".len()];

    // Buscar `.s3`, `.s3-…`, o `.s3.…` con algo antes.
    if let Some(idx) = stripped.rfind(".s3") {
        let after = &stripped[idx + 3..];
        let before = &stripped[..idx];
        let s3_suffix_ok =
            after.is_empty() || after.starts_with('.') || after.starts_with('-');
        if s3_suffix_ok && !before.is_empty() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_virtual_hosted_regional() {
        assert!(is_aws_virtual_hosted_endpoint(
            "https://portento.s3.mx-central-1.amazonaws.com"
        ));
    }

    #[test]
    fn detects_virtual_hosted_legacy() {
        assert!(is_aws_virtual_hosted_endpoint(
            "https://mybucket.s3.amazonaws.com"
        ));
    }

    #[test]
    fn detects_virtual_hosted_accelerated() {
        assert!(is_aws_virtual_hosted_endpoint(
            "https://mybucket.s3-accelerate.amazonaws.com"
        ));
    }

    #[test]
    fn detects_virtual_hosted_with_trailing_slash() {
        assert!(is_aws_virtual_hosted_endpoint(
            "https://portento.s3.mx-central-1.amazonaws.com/"
        ));
    }

    #[test]
    fn detects_virtual_hosted_with_port() {
        assert!(is_aws_virtual_hosted_endpoint(
            "https://portento.s3.us-east-1.amazonaws.com:443"
        ));
    }

    #[test]
    fn does_not_match_aws_path_style_endpoint() {
        // No bucket en subdominio → path-style, válido (aunque redundante)
        assert!(!is_aws_virtual_hosted_endpoint(
            "https://s3.us-east-1.amazonaws.com"
        ));
        assert!(!is_aws_virtual_hosted_endpoint("https://s3.amazonaws.com"));
    }

    #[test]
    fn does_not_match_r2() {
        assert!(!is_aws_virtual_hosted_endpoint(
            "https://abc123.r2.cloudflarestorage.com"
        ));
    }

    #[test]
    fn does_not_match_minio() {
        assert!(!is_aws_virtual_hosted_endpoint("https://minio.local:9000"));
        assert!(!is_aws_virtual_hosted_endpoint("http://minio.example.com"));
    }

    #[test]
    fn does_not_match_empty() {
        assert!(!is_aws_virtual_hosted_endpoint(""));
    }

    #[test]
    fn validate_rejects_aws_vh_endpoint() {
        let cfg = StorageConfig::S3 {
            access_key_id: "AKIA".into(),
            secret_access_key: "secret".into(),
            region: "mx-central-1".into(),
            bucket: "portento".into(),
            endpoint: Some("https://portento.s3.mx-central-1.amazonaws.com".into()),
        };
        let err = validate(&cfg).unwrap_err();
        assert!(err.contains("virtual-hosted"));
    }

    #[test]
    fn validate_accepts_no_endpoint() {
        let cfg = StorageConfig::S3 {
            access_key_id: "AKIA".into(),
            secret_access_key: "secret".into(),
            region: "us-east-1".into(),
            bucket: "mybucket".into(),
            endpoint: None,
        };
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn validate_accepts_minio_endpoint() {
        let cfg = StorageConfig::S3 {
            access_key_id: "AKIA".into(),
            secret_access_key: "secret".into(),
            region: "us-east-1".into(),
            bucket: "mybucket".into(),
            endpoint: Some("https://minio.local:9000".into()),
        };
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn validate_accepts_r2_endpoint() {
        let cfg = StorageConfig::S3 {
            access_key_id: "AKIA".into(),
            secret_access_key: "secret".into(),
            region: "auto".into(),
            bucket: "mybucket".into(),
            endpoint: Some("https://abc123.r2.cloudflarestorage.com".into()),
        };
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn validate_passes_through_azure() {
        let cfg = StorageConfig::Azure {
            connection_string: "DefaultEndpointsProtocol=https;AccountName=x;AccountKey=y".into(),
        };
        assert!(validate(&cfg).is_ok());
    }
}
