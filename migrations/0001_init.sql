-- Schema inicial para image-service: registro de proyectos con auth mTLS+API key
-- y credenciales de storage cifradas con envelope encryption.

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE projects (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL UNIQUE,
    cert_cn         TEXT NOT NULL UNIQUE,

    -- API key: HMAC-SHA256(salt, key_plain). Salt único por proyecto.
    -- api_key_prefix = primeros 12 chars de la key en plano, para identificarla
    -- en logs/UI sin exponerla completa.
    api_key_hash    BYTEA NOT NULL,
    api_key_salt    BYTEA NOT NULL,
    api_key_prefix  TEXT  NOT NULL,

    -- Storage backend: 'azure' | 's3'. Determina cómo deserializar storage_config
    -- una vez descifrado.
    storage_backend TEXT NOT NULL CHECK (storage_backend IN ('azure', 's3')),

    -- Envelope encryption (ver src/crypto.rs):
    --   storage_ciphertext = AES-256-GCM(DEK, JSON(StorageConfig))
    --   dek_ciphertext     = AES-256-GCM(KEK, DEK)
    storage_ciphertext  BYTEA   NOT NULL,
    storage_nonce       BYTEA   NOT NULL,
    dek_ciphertext      BYTEA   NOT NULL,
    dek_nonce           BYTEA   NOT NULL,
    kek_version         INTEGER NOT NULL,

    -- Container/bucket por defecto cuando el cliente no especifica uno.
    -- En plano: no es sensible.
    default_container TEXT,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at    TIMESTAMPTZ,
    revoked_at      TIMESTAMPTZ
);

-- Lookup por cert_cn es el path caliente del middleware de auth.
-- Filtramos en el índice para que las filas revocadas no inflen el btree.
CREATE INDEX idx_projects_cert_cn_active
    ON projects(cert_cn)
    WHERE revoked_at IS NULL;
