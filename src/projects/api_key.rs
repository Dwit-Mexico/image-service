//! Generación y verificación de API keys.
//!
//! Decisión de diseño: usamos HMAC-SHA256 con salt único por proyecto, NO
//! argon2/bcrypt. Las API keys son tokens de **alta entropía** (256 bits) —
//! el bruteforce es infeasible independientemente del hash, así que pagar
//! el coste de argon2 en cada request es regalar latencia sin ganar nada.
//! Este es el mismo enfoque que usan GitHub PATs y Stripe.
//!
//! Lo importante: hash irreversible + salt único (frena rainbow tables a
//! futuro si el algoritmo se debilitara).
//!
//! Formato de la key emitida: `sk_live_<32 chars base32>` (40 chars total).

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

const KEY_PREFIX: &str = "sk_live_";
const RANDOM_BYTES: usize = 24; // 24 bytes → 32 chars base64url no-pad
const SALT_BYTES: usize = 32;
const HASH_BYTES: usize = 32; // SHA-256 output

#[derive(Debug, thiserror::Error)]
pub enum ApiKeyError {
    #[error("formato de API key inválido")]
    InvalidFormat,
    #[error("longitud de salt o hash incorrecta")]
    InvalidStored,
}

/// Material persistido en la DB para verificar una key.
#[derive(Clone)]
pub struct ApiKeyHash {
    pub hash: [u8; HASH_BYTES],
    pub salt: [u8; SALT_BYTES],
    /// Primeros 12 chars de la key en plano (`sk_live_a3f2`) — sirve para
    /// identificarla en logs/UI sin exponer la key completa.
    pub prefix: String,
}

impl ApiKeyHash {
    pub fn from_stored(
        hash: &[u8],
        salt: &[u8],
        prefix: String,
    ) -> Result<Self, ApiKeyError> {
        if hash.len() != HASH_BYTES || salt.len() != SALT_BYTES {
            return Err(ApiKeyError::InvalidStored);
        }
        let mut h = [0u8; HASH_BYTES];
        let mut s = [0u8; SALT_BYTES];
        h.copy_from_slice(hash);
        s.copy_from_slice(salt);
        Ok(Self {
            hash: h,
            salt: s,
            prefix,
        })
    }
}

impl std::fmt::Debug for ApiKeyHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeyHash")
            .field("prefix", &self.prefix)
            .field("hash", &"[REDACTED]")
            .field("salt", &"[REDACTED]")
            .finish()
    }
}

/// Resultado de generar una key nueva. El plaintext SOLO existe aquí,
/// pensado para mostrarse una sola vez al admin y luego descartarse.
pub struct GeneratedKey {
    /// La key en plano — `sk_live_...`. NUNCA se guarda; el caller la entrega
    /// al usuario y la deja caer.
    pub plaintext: String,
    pub hash: ApiKeyHash,
}

/// Genera una API key nueva con salt random y devuelve el ciphertext + plain
/// para mostrar una sola vez.
pub fn generate() -> GeneratedKey {
    let mut random = [0u8; RANDOM_BYTES];
    OsRng.fill_bytes(&mut random);
    let suffix = URL_SAFE_NO_PAD.encode(random);
    let plaintext = format!("{KEY_PREFIX}{suffix}");
    let hash = hash_with_new_salt(&plaintext);
    GeneratedKey { plaintext, hash }
}

/// Importa una key ya existente (caso de migración o cliente ya configurado).
/// El caller pasa el plaintext que ya conoce el cliente y obtiene el hash
/// listo para persistir. Acepta cualquier formato — no asume el prefijo
/// `sk_live_` para tolerar keys legadas con otra forma.
pub fn import(plaintext: &str) -> ApiKeyHash {
    hash_with_new_salt(plaintext)
}

fn hash_with_new_salt(plaintext: &str) -> ApiKeyHash {
    let mut salt = [0u8; SALT_BYTES];
    OsRng.fill_bytes(&mut salt);
    let hash = hmac_key(&salt, plaintext.as_bytes());
    ApiKeyHash {
        hash,
        salt,
        prefix: prefix_of(plaintext),
    }
}

/// Verifica una key contra su hash almacenado. Comparación constant-time.
pub fn verify(provided: &str, stored: &ApiKeyHash) -> bool {
    let computed = hmac_key(&stored.salt, provided.as_bytes());
    computed.ct_eq(&stored.hash).into()
}

fn hmac_key(salt: &[u8], key: &[u8]) -> [u8; HASH_BYTES] {
    let mut mac = HmacSha256::new_from_slice(salt).expect("HMAC acepta cualquier longitud");
    mac.update(key);
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; HASH_BYTES];
    out.copy_from_slice(&result);
    out
}

fn prefix_of(key: &str) -> String {
    // 12 chars = "sk_live_" (8) + 4 chars del suffix random, o equivalente
    // para keys legadas con otro formato.
    key.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_verify_roundtrip() {
        let gen = generate();
        assert!(gen.plaintext.starts_with("sk_live_"));
        assert!(verify(&gen.plaintext, &gen.hash));
    }

    #[test]
    fn wrong_key_fails() {
        let gen = generate();
        assert!(!verify("sk_live_some_other_key_here", &gen.hash));
    }

    #[test]
    fn two_generates_produce_different_keys() {
        let a = generate();
        let b = generate();
        assert_ne!(a.plaintext, b.plaintext);
        assert_ne!(a.hash.hash, b.hash.hash);
        assert_ne!(a.hash.salt, b.hash.salt);
    }

    #[test]
    fn same_key_with_different_salt_produces_different_hash() {
        let a = generate();
        let b = generate();
        // mismo plaintext, salt distinto
        let hash_a = hmac_key(&a.hash.salt, a.plaintext.as_bytes());
        let hash_b = hmac_key(&b.hash.salt, a.plaintext.as_bytes());
        assert_ne!(hash_a, hash_b);
    }

    #[test]
    fn prefix_is_first_12_chars() {
        let gen = generate();
        assert_eq!(gen.hash.prefix.len(), 12);
        assert!(gen.plaintext.starts_with(&gen.hash.prefix));
    }

    #[test]
    fn debug_redacts_hash_and_salt() {
        let gen = generate();
        let s = format!("{:?}", gen.hash);
        assert!(s.contains(&gen.hash.prefix));
        assert!(s.contains("REDACTED"));
    }

    #[test]
    fn from_stored_rejects_invalid_lengths() {
        assert!(matches!(
            ApiKeyHash::from_stored(&[0u8; 10], &[0u8; 32], "x".into()),
            Err(ApiKeyError::InvalidStored)
        ));
        assert!(matches!(
            ApiKeyHash::from_stored(&[0u8; 32], &[0u8; 10], "x".into()),
            Err(ApiKeyError::InvalidStored)
        ));
    }

    #[test]
    fn minimum_key_entropy() {
        let gen = generate();
        // 24 random bytes → ~32 chars base64url. Más "sk_live_" → ≥40
        assert!(gen.plaintext.len() >= 40, "len = {}", gen.plaintext.len());
    }

    #[test]
    fn import_accepts_existing_key_and_verifies() {
        // Valor fake — el formato es lo que importa, no el contenido.
        let legacy = "sk_live_FAKE0000000000000000000000000000FAKE";
        let hash = import(legacy);
        assert!(verify(legacy, &hash));
        assert!(!verify("sk_live_some_other_distinct_key", &hash));
    }

    #[test]
    fn import_does_not_require_sk_live_prefix() {
        // Tolera keys legadas con cualquier formato.
        let exotic = "AKID_LEGACY_TOKEN_xyz";
        let hash = import(exotic);
        assert!(verify(exotic, &hash));
        assert_eq!(hash.prefix, "AKID_LEGACY_");
    }
}
