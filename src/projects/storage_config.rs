//! Configuración de storage por proyecto.
//!
//! Se serializa a JSON para alimentar a `crypto::seal()`, luego se persiste
//! como ciphertext en la DB. En memoria los campos sensibles se zeroean al
//! drop y se redactan en `Debug`.

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Discriminado por `backend` en el JSON, p.ej.:
///   `{"backend":"azure","connection_string":"..."}`
///   `{"backend":"s3","access_key_id":"...","secret_access_key":"...","region":"..."}`
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "lowercase")]
pub enum StorageConfig {
    Azure {
        connection_string: String,
    },
    S3 {
        access_key_id: String,
        secret_access_key: String,
        region: String,
        bucket: String,
        /// Para S3-compatible (MinIO, Cloudflare R2, etc.)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
    },
}

impl StorageConfig {
    pub fn backend_label(&self) -> &'static str {
        match self {
            StorageConfig::Azure { .. } => "azure",
            StorageConfig::S3 { .. } => "s3",
        }
    }
}

impl Drop for StorageConfig {
    fn drop(&mut self) {
        match self {
            StorageConfig::Azure { connection_string } => connection_string.zeroize(),
            StorageConfig::S3 {
                access_key_id,
                secret_access_key,
                ..
            } => {
                access_key_id.zeroize();
                secret_access_key.zeroize();
            }
        }
    }
}

impl std::fmt::Debug for StorageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageConfig::Azure { .. } => f
                .debug_struct("StorageConfig::Azure")
                .field("connection_string", &"[REDACTED]")
                .finish(),
            StorageConfig::S3 {
                region,
                bucket,
                endpoint,
                ..
            } => f
                .debug_struct("StorageConfig::S3")
                .field("access_key_id", &"[REDACTED]")
                .field("secret_access_key", &"[REDACTED]")
                .field("region", region)
                .field("bucket", bucket)
                .field("endpoint", endpoint)
                .finish(),
        }
    }
}

/// Serializa a JSON. Solo se usa como input a `crypto::seal()`.
pub fn to_json(cfg: &StorageConfig) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(cfg)
}

/// Deserializa desde JSON descifrado.
pub fn from_json(bytes: &[u8]) -> Result<StorageConfig, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azure_roundtrip() {
        let cfg = StorageConfig::Azure {
            connection_string: "DefaultEndpoints=...;AccountKey=secret".into(),
        };
        let bytes = to_json(&cfg).unwrap();
        let back = from_json(&bytes).unwrap();
        match &back {
            StorageConfig::Azure { connection_string } => {
                assert_eq!(connection_string, "DefaultEndpoints=...;AccountKey=secret");
            }
            other => panic!("backend incorrecto: {other:?}"),
        }
    }

    #[test]
    fn s3_roundtrip() {
        let cfg = StorageConfig::S3 {
            access_key_id: "AKIA...".into(),
            secret_access_key: "supersecret".into(),
            region: "us-east-1".into(),
            bucket: "my-bucket".into(),
            endpoint: Some("https://r2.cloudflarestorage.com".into()),
        };
        let bytes = to_json(&cfg).unwrap();
        let back = from_json(&bytes).unwrap();
        if let StorageConfig::S3 {
            access_key_id,
            secret_access_key,
            region,
            bucket,
            endpoint,
        } = &back
        {
            assert_eq!(access_key_id, "AKIA...");
            assert_eq!(secret_access_key, "supersecret");
            assert_eq!(region, "us-east-1");
            assert_eq!(bucket, "my-bucket");
            assert_eq!(endpoint.as_deref(), Some("https://r2.cloudflarestorage.com"));
        } else {
            panic!("backend incorrecto");
        }
    }

    #[test]
    fn s3_without_endpoint_omits_field() {
        let cfg = StorageConfig::S3 {
            access_key_id: "AKIA...".into(),
            secret_access_key: "s".into(),
            region: "us-east-1".into(),
            bucket: "b".into(),
            endpoint: None,
        };
        let s = String::from_utf8(to_json(&cfg).unwrap()).unwrap();
        assert!(!s.contains("endpoint"), "endpoint=None debe omitirse: {s}");
    }

    #[test]
    fn backend_discriminator_present() {
        let cfg = StorageConfig::Azure {
            connection_string: "x".into(),
        };
        let s = String::from_utf8(to_json(&cfg).unwrap()).unwrap();
        assert!(s.contains("\"backend\":\"azure\""));
    }

    #[test]
    fn debug_redacts_secrets() {
        let cfg = StorageConfig::Azure {
            connection_string: "super-secret".into(),
        };
        let s = format!("{cfg:?}");
        assert!(!s.contains("super-secret"));
        assert!(s.contains("REDACTED"));
    }

    #[test]
    fn debug_s3_redacts_keys_but_keeps_region() {
        let cfg = StorageConfig::S3 {
            access_key_id: "AKIAEXAMPLE".into(),
            secret_access_key: "do-not-look".into(),
            region: "us-east-1".into(),
            bucket: "my-bucket".into(),
            endpoint: None,
        };
        let s = format!("{cfg:?}");
        assert!(!s.contains("AKIAEXAMPLE"));
        assert!(!s.contains("do-not-look"));
        assert!(s.contains("us-east-1"));
        assert!(s.contains("my-bucket"));
    }
}
