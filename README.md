# image-service

Servicio HTTP en Rust (Axum) para subir, comprimir y servir imágenes desde Azure Blob Storage o S3. Multi-tenant: cada proyecto cliente se identifica con mTLS + API key, y cada uno apunta a su propio storage backend.

## Cómo funciona

```
cliente ──(mTLS + X-API-Key)──> Gateway API ──> image-service ──> Azure / S3
                                     │                  │
                                termina TLS,         resolver ↔ Postgres (proyectos + creds cifradas)
                                reenvía CN                   ↔ Valkey   (pub/sub invalidación)
```

1. El Gateway termina mTLS y valida el certificado cliente contra la CA configurada (`k8s/mtls-policy.yaml`).
2. Reenvía el header `X-Forwarded-Client-Cert` con el `CN` del cert al backend.
3. El middleware (`src/middleware/auth.rs`) extrae el CN y consulta al **resolver**: cache local (TTL 30s) → en miss, query a Postgres + descifrado de la storage config con la KEK.
4. Verifica el header `X-API-Key` con HMAC-SHA256 (timing-safe).
5. Construye el `StorageProvider` específico del proyecto (Azure o S3) y sube la imagen procesada.

## Endpoints

| Método | Ruta             | Auth | Descripción                          |
|--------|------------------|------|--------------------------------------|
| GET    | `/health`        | no   | Healthcheck (`{"status":"ok"}`)      |
| POST   | `/upload`        | sí   | Sube una sola imagen (multipart)     |
| POST   | `/upload/batch`  | sí   | Sube varias imágenes en paralelo     |

### `POST /upload`

Campos multipart:
- `file` (requerido): bytes de la imagen
- `options` (opcional): JSON con opciones de procesamiento

```json
{
  "quality": 85,
  "max_width": 2048,
  "format": "webp",
  "container": "images",
  "folder": "users/123/avatars"
}
```

Defaults: `quality=85`, `max_width=2048`, `format=webp`, `container` = `default_container` del proyecto (o `images`), sin folder.

Respuesta:
```json
{
  "id": "users/123/avatars/<uuid>.webp",
  "url": "https://<host>/<container>/<id>",
  "original_bytes": 1234567,
  "compressed_bytes": 87654,
  "format": "webp"
}
```

### `POST /upload/batch`

Mismo formato, acepta múltiples campos `file`. Las `options` son compartidas para todo el batch. Procesa en paralelo y devuelve un resultado por imagen indexado por orden de envío.

## Configuración

| Variable                          | Requerida | Descripción                                                            |
|-----------------------------------|-----------|------------------------------------------------------------------------|
| `DATABASE_URL`                    | sí        | Postgres connection string                                             |
| `MASTER_KEY_V1`                   | sí        | KEK base64 de 32 bytes (`openssl rand -base64 32`)                     |
| `VALKEY_SENTINEL_ADDR`            | no        | `host:port` de Sentinel(s). Sin esto, no hay pub/sub (cache TTL only). |
| `VALKEY_MASTER_NAME`              | no        | Default: `mymaster`                                                    |
| `VALKEY_PASSWORD`                 | no        | Si Valkey está protegido con auth                                      |
| `LISTEN_ADDR`                     | no        | Default: `0.0.0.0:8080`                                                |
| `RUST_LOG`                        | no        | Default: `info`                                                        |

### Variables legacy (transición)

Mientras los proyectos aún se seedean desde env vars, el binario `seed-from-env` lee:
- `AZURE_STORAGE_CONNECTION_STRING` — storage compartido para todos los proyectos legacy
- `DEFAULT_CONTAINER` — container Azure por defecto
- `PROJECT_*` (formato `cn:api_key`) — un proyecto por variable

Una vez seedeados a Postgres, estas variables se pueden eliminar del secret.

## Registro de proyectos

Cada proyecto vive en la tabla `projects` con:
- `cert_cn` (único): CN del certificado mTLS del cliente
- `api_key_hash` + `api_key_salt`: HMAC-SHA256 (la key plaintext nunca se almacena)
- `storage_backend`: `azure` o `s3`
- `storage_ciphertext` + envelope: connection string Azure o creds S3 cifradas con la KEK
- `default_container`: container/bucket por defecto

### Agregar un proyecto nuevo

Por ahora vía SQL directo o `seed-from-env`. UI de admin pendiente.

```sql
-- Ejemplo de inspección (las credenciales están cifradas, no se ven):
SELECT name, cert_cn, api_key_prefix, storage_backend, default_container, created_at
FROM projects
WHERE revoked_at IS NULL;
```

### Migrar clientes existentes sin romper nada

El binario `seed-from-env` lee las env vars `PROJECT_*` legacy y las inserta en la tabla `projects` con la misma API key plaintext (hasheada con salt nuevo). El cliente sigue usando la misma key, no necesita reconfigurar nada.

Es idempotente: re-corrérlo solo inserta los proyectos que aún no existen.

## Desarrollo local

```bash
# 1. Levantar Postgres
docker compose up -d postgres

# 2. Configurar .env
cp .env.example .env
# editar — al menos DATABASE_URL y MASTER_KEY_V1 (generar con openssl rand -base64 32)

# 3. Si tienes env vars PROJECT_* legacy, seedearlas a la DB
cargo run --bin seed-from-env

# 4. Arrancar el servicio
cargo run --bin image-service
```

En local Valkey no está disponible (solo vive en la red interna de k8s), así que el servicio arranca sin pub/sub y depende del TTL del cache para invalidaciones. Eso es OK para una sola instancia.

Para probar el auth en local, manda el header `X-Client-Cert-CN` (fallback del middleware):

```bash
curl -X POST http://localhost:8080/upload \
  -H "X-Client-Cert-CN: project-velvet" \
  -H "X-API-Key: sk_live_..." \
  -F "file=@foto.jpg" \
  -F 'options={"quality":80,"max_width":1024,"folder":"test"}'
```

## Binarios

| Binario           | Propósito                                                              |
|-------------------|------------------------------------------------------------------------|
| `image-service`   | Servicio HTTP principal                                                |
| `migrate`         | Aplica migraciones pendientes y termina (init container o pre-deploy)  |
| `seed-from-env`   | Migra `PROJECT_*` legacy a la tabla `projects`. Idempotente.           |
| `project-admin`   | CLI: `list`, `show`, `create-azure`/`create-s3`, `revoke`, `rotate-key` |

### `project-admin` — ejemplos

```bash
# Listar proyectos
project-admin list

# Crear proyecto Azure
project-admin create-azure my-tenant my-tenant-cn \
  "DefaultEndpointsProtocol=https;AccountName=...;AccountKey=...;EndpointSuffix=core.windows.net" \
  my-container
# → imprime la API key plaintext UNA SOLA VEZ

# Crear proyecto S3 (también funciona con MinIO/R2 si pasas endpoint)
project-admin create-s3 my-tenant my-tenant-cn ACCESS_KEY SECRET_KEY us-east-1 my-bucket

# Rotar la API key (la vieja se invalida — publica a Valkey para corte inmediato)
project-admin rotate-key my-tenant-cn

# Revocar (no se borra; se marca revoked_at)
project-admin revoke my-tenant-cn
```

En producción: `kubectl -n production exec -it deploy/image-service -- /app/project-admin list`

## Deploy

### Infraestructura

- **Postgres** en el cluster (namespace `data`, opcionalmente compartido con Valkey). No incluido en este repo.
- **Valkey** ya desplegada (ver `infra-valkey-k8s`). NetworkPolicy permite tráfico desde `production`.
- **Cluster k3s** con Gateway API y mTLS configurado.

### Manifests (`k8s/`)

Todos viven en namespace `production`:
- `image-service/2-deployment.yaml` — Deployment con 2 initContainers (`migrate` + `seed-from-env`) y la app
- `image-service/3-service.yaml` — ClusterIP :80 → :8080
- `http-route.yaml` — `HTTPRoute` que monta `image-service.dwitmexico.com` en el listener mTLS
- `mtls-policy.yaml` — `ClientTrafficPolicy` que valida certs cliente contra `image-service-ca-secret`

### Workflows (`.github/workflows/`)

**`deploy.yml`** — push a `main` o manual:
1. Build & push de imagen Docker a `ghcr.io`
2. SCP de manifests
3. SSH → reescribe el secret `image-service` (todas las env vars en `--from-literal`), crea namespace si no existe, aplica manifests si es primer deploy, hace `set image` para subsecuentes

**`update-secrets.yml`** — manual:
- Reescribe solo el secret y hace `rollout restart`. Úsalo cuando rotes credenciales sin cambiar código.

### Migración de secrets — gotchas

- **No pre-codifiques en base64**: k8s ya guarda secrets en base64 internamente. Si los pegas pre-codificados en GitHub, kubectl los re-codifica → doble base64 y el pod recibe basura.
- **El nombre del secret en GitHub no afecta runtime**: los `PROJECT_*` legacy solo importan por su valor (`cn:api_key`); el nombre es para humanos.

## Estructura del código

```
src/
├── main.rs              # bootstrap: KEK + pool + resolver + subscriber + axum
├── lib.rs               # re-exporta módulos para los binarios
├── bin/
│   ├── migrate.rs       # corre migraciones y termina
│   └── seed_from_env.rs # migra PROJECT_* legacy a Postgres
├── config.rs            # AppState (resolver)
├── crypto.rs            # envelope encryption (KEK + DEK con AES-256-GCM)
├── db.rs                # PgPool + sqlx::migrate!
├── error.rs             # AppError → respuesta HTTP
├── middleware/
│   └── auth.rs          # mTLS CN + verify api_key + inyecta ResolvedProject
├── handlers/
│   ├── health.rs
│   ├── upload.rs        # /upload, construye storage por request
│   └── batch.rs         # /upload/batch
├── processing/
│   └── image.rs         # decode + resize + encode (webp/jpeg/png)
├── projects/
│   ├── api_key.rs       # HMAC-SHA256 + salt, generate/import/verify
│   ├── storage_config.rs# enum Azure/S3 + serde + zeroize
│   ├── repo.rs          # queries sqlx (validadas en compile)
│   ├── resolver.rs      # cache moka + load_from_db + record_use
│   └── invalidator.rs   # subscriber Valkey pub/sub (opcional)
└── storage/
    ├── mod.rs           # trait + factory build(&StorageConfig)
    ├── azure.rs         # Azure Blob via object_store
    └── s3.rs            # S3 / S3-compatible (MinIO, R2) via object_store
```
