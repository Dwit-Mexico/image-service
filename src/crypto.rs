//! Envelope encryption para credenciales de storage.
//!
//! Esquema:
//!   - KEK (Key Encryption Key): 32 bytes, viene de env var `MASTER_KEY_V1`
//!   - DEK (Data Encryption Key): 32 bytes, random por cada `seal()`
//!   - Cipher: AES-256-GCM para ambos niveles
//!
//! Lo que se persiste en DB es `EncryptedBlob` — incluye la DEK cifrada con la
//! KEK. La KEK nunca toca la DB; vive solo en el k8s Secret y en memoria del
//! proceso, redactada en `Debug` y zeroeada al drop.

use std::env;

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    AeadCore, Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("variable de entorno {0} ausente")]
    MissingEnv(&'static str),

    #[error("master key inválida: {0}")]
    InvalidKey(&'static str),

    /// Genérico a propósito: no leakeamos qué parte falló (auth tag, padding, etc.)
    /// para evitar oráculos de descifrado.
    #[error("fallo de cifrado/descifrado")]
    AeadFailure,

    #[error("KEK version mismatch: blob v{found}, runtime v{expected}")]
    KekVersionMismatch { expected: u32, found: u32 },
}

pub struct Kek {
    version: u32,
    key: [u8; 32],
}

impl Drop for Kek {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

impl std::fmt::Debug for Kek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kek")
            .field("version", &self.version)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl Kek {
    /// Carga `MASTER_KEY_V1` desde el entorno (base64 de 32 bytes).
    pub fn from_env() -> Result<Self, CryptoError> {
        let raw =
            env::var("MASTER_KEY_V1").map_err(|_| CryptoError::MissingEnv("MASTER_KEY_V1"))?;
        Self::from_base64(1, &raw)
    }

    pub fn from_base64(version: u32, b64: &str) -> Result<Self, CryptoError> {
        let bytes = STANDARD
            .decode(b64.trim())
            .map_err(|_| CryptoError::InvalidKey("debe ser base64 estándar"))?;
        if bytes.len() != 32 {
            return Err(CryptoError::InvalidKey("debe decodear a 32 bytes"));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(Self { version, key })
    }

    pub fn version(&self) -> u32 {
        self.version
    }
}

#[derive(Debug, Clone)]
pub struct EncryptedBlob {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub dek_ciphertext: Vec<u8>,
    pub dek_nonce: Vec<u8>,
    pub kek_version: u32,
}

/// Genera DEK random, cifra `plaintext` con DEK y cifra la DEK con la KEK.
pub fn seal(kek: &Kek, plaintext: &[u8]) -> Result<EncryptedBlob, CryptoError> {
    let dek = Aes256Gcm::generate_key(&mut OsRng);
    let dek_cipher = Aes256Gcm::new(&dek);

    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = dek_cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| CryptoError::AeadFailure)?;

    let kek_cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&kek.key));
    let dek_nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let dek_ciphertext = kek_cipher
        .encrypt(&dek_nonce, dek.as_slice())
        .map_err(|_| CryptoError::AeadFailure)?;

    Ok(EncryptedBlob {
        ciphertext,
        nonce: nonce.to_vec(),
        dek_ciphertext,
        dek_nonce: dek_nonce.to_vec(),
        kek_version: kek.version,
    })
}

/// Descifra la DEK con la KEK y luego el plaintext con la DEK.
/// El resultado se zeroea al drop.
pub fn open(kek: &Kek, blob: &EncryptedBlob) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if blob.kek_version != kek.version {
        return Err(CryptoError::KekVersionMismatch {
            expected: kek.version,
            found: blob.kek_version,
        });
    }
    if blob.nonce.len() != 12 || blob.dek_nonce.len() != 12 {
        return Err(CryptoError::AeadFailure);
    }

    let kek_cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&kek.key));
    let dek_nonce = Nonce::from_slice(&blob.dek_nonce);
    let dek_bytes = Zeroizing::new(
        kek_cipher
            .decrypt(dek_nonce, blob.dek_ciphertext.as_ref())
            .map_err(|_| CryptoError::AeadFailure)?,
    );

    if dek_bytes.len() != 32 {
        return Err(CryptoError::AeadFailure);
    }

    let dek_cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&dek_bytes));
    let nonce = Nonce::from_slice(&blob.nonce);
    let plaintext = dek_cipher
        .decrypt(nonce, blob.ciphertext.as_ref())
        .map_err(|_| CryptoError::AeadFailure)?;

    Ok(Zeroizing::new(plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_kek(version: u32) -> Kek {
        let mut key = [0u8; 32];
        key[0] = version as u8;
        Kek { version, key }
    }

    #[test]
    fn roundtrip() {
        let kek = test_kek(1);
        let pt = b"hola mundo, soy un secreto";
        let blob = seal(&kek, pt).unwrap();
        let out = open(&kek, &blob).unwrap();
        assert_eq!(out.as_slice(), pt);
    }

    #[test]
    fn each_seal_uses_different_dek_and_nonce() {
        let kek = test_kek(1);
        let pt = b"same payload";
        let a = seal(&kek, pt).unwrap();
        let b = seal(&kek, pt).unwrap();
        assert_ne!(a.ciphertext, b.ciphertext);
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.dek_ciphertext, b.dek_ciphertext);
        assert_ne!(a.dek_nonce, b.dek_nonce);
    }

    #[test]
    fn wrong_kek_fails() {
        let kek_a = test_kek(1);
        let mut other = [9u8; 32];
        other[0] = 1;
        let kek_b = Kek {
            version: 1,
            key: other,
        };
        let blob = seal(&kek_a, b"secret").unwrap();
        assert!(matches!(open(&kek_b, &blob), Err(CryptoError::AeadFailure)));
    }

    #[test]
    fn version_mismatch_rejected() {
        let kek_v1 = test_kek(1);
        let kek_v2 = test_kek(2);
        let blob = seal(&kek_v1, b"secret").unwrap();
        assert!(matches!(
            open(&kek_v2, &blob),
            Err(CryptoError::KekVersionMismatch {
                expected: 2,
                found: 1
            })
        ));
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let kek = test_kek(1);
        let mut blob = seal(&kek, b"secret payload").unwrap();
        blob.ciphertext[0] ^= 0x01;
        assert!(matches!(open(&kek, &blob), Err(CryptoError::AeadFailure)));
    }

    #[test]
    fn tampered_dek_ciphertext_fails() {
        let kek = test_kek(1);
        let mut blob = seal(&kek, b"secret payload").unwrap();
        blob.dek_ciphertext[0] ^= 0x01;
        assert!(matches!(open(&kek, &blob), Err(CryptoError::AeadFailure)));
    }

    #[test]
    fn invalid_nonce_size_rejected() {
        let kek = test_kek(1);
        let mut blob = seal(&kek, b"secret payload").unwrap();
        blob.nonce.push(0);
        assert!(matches!(open(&kek, &blob), Err(CryptoError::AeadFailure)));
    }

    #[test]
    fn from_base64_validates_length() {
        let short = STANDARD.encode([0u8; 16]);
        assert!(matches!(
            Kek::from_base64(1, &short),
            Err(CryptoError::InvalidKey(_))
        ));
    }

    #[test]
    fn from_base64_rejects_invalid() {
        assert!(matches!(
            Kek::from_base64(1, "not!!base64!!"),
            Err(CryptoError::InvalidKey(_))
        ));
    }

    #[test]
    fn debug_redacts_the_key() {
        let kek = test_kek(1);
        let s = format!("{kek:?}");
        assert!(s.contains("REDACTED"));
        assert!(!s.contains("[0, 0"));
    }
}
