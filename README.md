# image-service

Servicio HTTP en Rust (Axum) para subir, comprimir y servir imágenes desde Azure Blob Storage o S3. Multi-tenant: cada proyecto cliente se identifica con cert CN + API key, y cada uno apunta a su propio storage backend. Incluye CLI y UI admin para gestionar proyectos.

## Cómo funciona

```
cliente ──(X-Client-Cert-CN + X-API-Key)──> Gateway API ──> image-service ──> Azure / S3
                                                                   │
                                                resolver ↔ Postgres (proyectos + creds cifradas)
                                                         ↔ Valkey   (pub/sub invalidación)
```

1. El Gateway termina TLS y reenvía el header `X-Forwarded-Client-Cert` con el CN (o el cliente manda `X-Client-Cert-CN` directo).
2. El middleware (`src/middleware/auth.rs`) extrae el CN y consulta al **resolver**: cache local (TTL 30 s) → en miss, query a Postgres + descifrado de la storage config con la KEK.
3. Verifica `X-API-Key` con HMAC-SHA256 (timing-safe).
4. Construye el `StorageProvider` específico del proyecto (Azure o S3) y sube la imagen procesada.

## Endpoints

### Públicos (auth por cert CN + API key)

| Método | Ruta             | Descripción                          |
|--------|------------------|--------------------------------------|
| GET    | `/health`        | Healthcheck (`{"status":"ok"}`)      |
| POST   | `/upload`        | Sube una sola imagen (multipart)     |
| POST   | `/upload/batch`  | Sube varias imágenes en paralelo     |

### Admin UI (auth por session cookie, montado bajo `/admin`)

| Método | Ruta                                              | Descripción                       |
|--------|---------------------------------------------------|-----------------------------------|
| GET    | `/admin/login`                                    | Form de login                     |
| POST   | `/admin/login`                                    | Crea sesión (rate-limit 5/5 min)  |
| POST   | `/admin/logout`                                   | Borra cookie                      |
| GET    | `/admin/`                                         | Dashboard con todos los proyectos |
| GET    | `/admin/projects/new?backend=azure\|s3`           | Form de creación                  |
| POST   | `/admin/projects`                                 | Crea, devuelve plaintext key una vez |
| GET    | `/admin/projects/:cert_cn`                        | Detalle + acciones                |
| POST   | `/admin/projects/:cert_cn/rotate-key`             | Rota API key                      |
| POST   | `/admin/projects/:cert_cn/rotate-storage`         | Rota creds Azure/S3               |
| POST   | `/admin/projects/:cert_cn/revoke`                 | Revoca                            |

La UI usa **htmx + Pico CSS** (server-rendered, sin SPA). Toda acción mutativa publica invalidación a Valkey si está configurado.

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

| Variable               | Requerida | Descripción                                                            |
|------------------------|-----------|------------------------------------------------------------------------|
| `DATABASE_URL`         | sí        | Postgres connection string                                             |
| `MASTER_KEY_V1`        | sí        | KEK base64 de 32 bytes (`openssl rand -base64 32`)                     |
| `VALKEY_SENTINEL_ADDR` | no        | `host:port` de Sentinel(s). Sin esto no hay pub/sub (cache TTL only). |
| `VALKEY_MASTER_NAME`   | no        | Default: `mymaster`                                                    |
| `VALKEY_PASSWORD`      | no        | Si Valkey está protegido con auth                                      |
| `ADMIN_USERNAME`       | no        | Default: `admin`                                                       |
| `ADMIN_PASSWORD_HASH`  | no        | Hash argon2id. Si no está, `/admin/*` se omite. En prod **no se setea a mano** — lo genera el workflow `rotate-admin-password.yml` desde el Secret `ADMIN_PASSWORD_PLAIN` |
| `LISTEN_ADDR`          | no        | Default: `0.0.0.0:8080`                                                |
| `RUST_LOG`             | no        | Default: `info`                                                        |

### Variables legacy (solo para seed inicial)

`seed-from-env` migra estos a la tabla `projects` una sola vez; después se pueden eliminar del entorno:
- `AZURE_STORAGE_CONNECTION_STRING` — storage compartido para todos los proyectos legacy
- `DEFAULT_CONTAINER` — container Azure por defecto
- `PROJECT_*` (formato `cn:api_key`) — un proyecto por variable

## Schema y migraciones

Las tablas se crean **automáticamente** la primera vez que el servicio (o `cargo run --bin migrate`) se conecta a la DB. No hay que correr DDL a mano.

- Los archivos `.sql` viven en `migrations/`
- sqlx los **embebe en el binario** al compilar (`sqlx::migrate!("./migrations")`)
- Al arrancar, sqlx lee la tabla `_sqlx_migrations` y solo aplica las pendientes
- Es idempotente — restarts no rompen nada
- Cada `.sql` corre en una transacción

| Contexto         | Cuándo se aplican                                                    |
|------------------|----------------------------------------------------------------------|
| Local dev        | Al arrancar el servicio o explícito (`cargo run --bin migrate`)      |
| Producción (k8s) | initContainer `migrate` corre antes que la app, en cada rollout      |
| CI/CD            | Cubierto por el initContainer — no necesitas step extra              |

## Gestión de proyectos

Tres caminos para administrar la tabla `projects`:

### 1. UI admin (`/admin`)

La más cómoda. Login con password, dashboard con tabla, formularios para crear/rotar/revocar. Cada acción mutativa publica invalidación a Valkey.

### 2. CLI `project-admin`

Mismas operaciones, accesibles desde un pod productivo o local. Útil para automatización.

```bash
# Local
cargo run --bin project-admin list
cargo run --bin project-admin show project-velvet

# En cluster
kubectl -n production exec -it deploy/image-service -c image-service -- \
  /app/project-admin list
```

Subcomandos:

| Subcomando | Uso |
|---|---|
| `list` | Listar proyectos |
| `show <cert_cn>` | Detalle de un proyecto |
| `create-azure <name> <cert_cn> <conn> [container]` | Crear con Azure |
| `create-s3 <name> <cert_cn> <access> <secret> <region> <bucket> [endpoint]` | Crear con S3/MinIO/R2 |
| `rotate-storage-azure <cert_cn> <conn> [container]` | Rotar creds Azure |
| `rotate-storage-s3 <cert_cn> <access> <secret> <region> <bucket> [endpoint]` | Rotar creds S3 |
| `rotate-key <cert_cn>` | Genera nueva API key |
| `revoke <cert_cn>` | Marca `revoked_at` |
| `admin-hash` | Lee password de stdin, imprime hash argon2id |

### 3. SQL directo

Para inspección o cosas que el CLI no cubre. Las credenciales de storage están cifradas, no se ven:

```sql
SELECT name, cert_cn, api_key_prefix, storage_backend, default_container, created_at
FROM projects
WHERE revoked_at IS NULL;
```

### Migrar clientes existentes sin romper nada

`seed-from-env` lee las env vars `PROJECT_*` legacy y las inserta en la tabla `projects` con la **misma API key plaintext** (hasheada con salt nuevo). El cliente sigue usando la misma key, no necesita reconfigurar nada. Es idempotente.

## Desarrollo local

```bash
# 1. Levantar Postgres (puerto 5433 para no chocar con un Postgres del sistema en 5432)
docker compose up -d postgres

# 2. Configurar .env
cp .env.example .env
# editar — al menos DATABASE_URL y MASTER_KEY_V1 (`openssl rand -base64 32`)

# 3. Si tienes env vars PROJECT_* legacy, seedearlas a la DB
cargo run --bin seed-from-env

# 4. (Opcional) Generar password para la UI admin
cargo run --example gen_admin_hash -- mipassword
# pegar el ADMIN_PASSWORD_HASH=... en .env (con comillas simples — dotenv expande $)

# 5. Arrancar
cargo run --bin image-service
```

En local Valkey no está disponible, así que el servicio arranca sin pub/sub. Para probar el auth sin gateway mTLS, manda `X-Client-Cert-CN` directo:

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
| `image-service`   | Servidor HTTP principal                                                |
| `migrate`         | Aplica migraciones pendientes y termina (init container o pre-deploy)  |
| `seed-from-env`   | Migra `PROJECT_*` legacy a la tabla `projects`. Idempotente.           |
| `project-admin`   | CLI para gestionar proyectos (ver tabla arriba)                        |

## Deploy

### Infraestructura asumida

- **Postgres** en el cluster (typ. namespace `data`, junto con Valkey). No incluido en este repo.
- **Valkey** ya desplegada (ver `infra-valkey-k8s`). NetworkPolicy permite tráfico desde `production`.
- Secret `valkey-auth` **replicado al namespace `production`** (el playbook de Valkey lo hace si `apps_namespace: production`).
- **Cluster k3s** con Gateway API. El listener compartido (`gateway-api/https`) acepta routes del namespace `default`.

### Manifests (`k8s/`)

| Archivo                                  | Namespace    | Qué hace                                                                |
|------------------------------------------|--------------|-------------------------------------------------------------------------|
| `image-service/2-deployment.yaml`        | `production` | Deployment con 2 initContainers (`migrate` + `seed-from-env`) y la app  |
| `image-service/3-service.yaml`           | `production` | ClusterIP `:80 → :8080`                                                 |
| `http-route.yaml`                        | `default`    | HTTPRoute en `default` + ReferenceGrant para cross-ns backend → `production` |
| `mtls-policy.yaml`                       | `default`    | `ClientTrafficPolicy` (legacy, mTLS no activo en el listener actual)    |

### Workflows (`.github/workflows/`)

**`deploy.yml`** — push a `main` o manual:
1. Build & push de imagen a `ghcr.io`
2. SCP de manifests
3. SSH → reescribe el secret `image-service` **preservando `ADMIN_PASSWORD_HASH`**, crea namespace si no existe, aplica manifests si es primer deploy, hace `set image` para subsecuentes

**`update-secrets.yml`** — manual:
- Reescribe solo el secret (preservando `ADMIN_PASSWORD_HASH`) y hace `rollout restart`. Úsalo cuando rotes credenciales legacy sin cambiar código.

**`rotate-admin-password.yml`** — manual:
- Lee el password del Secret de GitHub **`ADMIN_PASSWORD_PLAIN`** (sin inputs en el form de "Run workflow"), lo hashea con argon2id en el runner, **patchea solo la key `ADMIN_PASSWORD_HASH`** del secret de k8s y reinicia el deployment. El plaintext nunca aparece en logs ni job summary (GitHub enmascara automáticamente valores de `secrets.*`). Para rotar: edita el Secret y re-ejecuta el workflow.

### GitHub Secrets y Variables que necesita el repo

> Settings → Secrets and variables → Actions

**Secrets** (cifrados, enmascarados en logs):

| Nombre                            | Quién lo usa                    | Notas |
|-----------------------------------|---------------------------------|-------|
| `SSH_HOST`, `SSH_USER`, `SSH_KEY`, `SSH_PORT` | deploy / update-secrets / rotate-admin | acceso al nodo k3s |
| `DATABASE_URL`                    | deploy / update-secrets         | `postgres://user:pwd@postgres.data.svc.cluster.local:5432/image_service` |
| `MASTER_KEY_V1`                   | deploy / update-secrets         | base64 de 32 bytes — `openssl rand -base64 32` |
| `VALKEY_PASSWORD`                 | (no se usa; el deployment monta `valkey-auth` directo) | dejar vacío o quitar |
| `ADMIN_PASSWORD_PLAIN`            | rotate-admin-password           | password en plano para el admin |
| `AZURE_STORAGE_CONNECTION_STRING` | deploy / update-secrets (legacy) | solo necesario mientras seedeen proyectos vía `seed-from-env` |
| `DEFAULT_CONTAINER`               | deploy / update-secrets (legacy) | idem |
| `PROJECT_VELVET`, `PROJECT_PIXEL_*`, ... | deploy / update-secrets (legacy) | un secret por proyecto legacy |
| `TLS_*`                           | (legacy del setup viejo)        | si ya no se usan, eliminar |

**Variables** (texto plano, visibles):

| Nombre                | Quién lo usa                    | Valor típico |
|-----------------------|---------------------------------|--------------|
| `VALKEY_SENTINEL_ADDR`| deploy / update-secrets         | `valkey.data.svc.cluster.local:26379` |
| `VALKEY_MASTER_NAME`  | deploy / update-secrets         | `myprimary` (depende del chart de Valkey) |
| `ADMIN_USERNAME`      | deploy / update-secrets         | `admin` (default si no se setea) |

### Acceso a la UI admin desde browser

`image-service.dwitmexico.com` está pensado para clientes; para entrar al admin lo más práctico es port-forward:

```bash
ssh dwit_kb -L 8080:localhost:8080
# en el servidor:
kubectl -n production port-forward deploy/image-service 8080:8080
# browser: http://localhost:8080/admin/login
```

(El cookie tiene `Secure`, así que via HTTP funciona porque `http://localhost` es excepción del browser; otras URLs HTTP no aceptan la cookie.)

### Migración de secrets — gotchas

- **No pre-codifiques en base64** los valores en GitHub Secrets. k8s ya guarda los secrets en base64 internamente; si los pegas pre-codificados, kubectl los re-codifica → doble base64 y el pod recibe basura.
- **Dotenv expande `$`**: si pegas `ADMIN_PASSWORD_HASH=$argon2id$...` en `.env`, dotenv interpreta `$argon2id` como variable. Usa comillas simples: `ADMIN_PASSWORD_HASH='$argon2id$...'`.
- **El nombre del secret legacy en GitHub no afecta runtime**: los `PROJECT_*` solo importan por su valor (`cn:api_key`).

## Estructura del código

```
src/
├── main.rs              # bootstrap: KEK + pool + resolver + subscriber + admin + axum
├── lib.rs               # re-exporta módulos para los binarios
├── bin/
│   ├── migrate.rs       # corre migraciones y termina
│   ├── seed_from_env.rs # migra PROJECT_* legacy a Postgres
│   └── project_admin.rs # CLI multi-subcomando
├── config.rs            # AppState (resolver + kek + admin)
├── crypto.rs            # envelope encryption (KEK + DEK con AES-256-GCM)
├── db.rs                # PgPool + sqlx::migrate!
├── error.rs             # AppError → respuesta HTTP
├── middleware/
│   └── auth.rs          # extrae cert CN + verify api_key + inyecta ResolvedProject
├── handlers/
│   ├── health.rs
│   ├── upload.rs        # /upload, construye storage por request
│   └── batch.rs         # /upload/batch
├── processing/
│   └── image.rs         # decode + resize + encode (webp/jpeg/png)
├── projects/
│   ├── api_key.rs       # HMAC-SHA256 + salt, generate/import/verify
│   ├── storage_config.rs# enum Azure/S3 + serde + zeroize + redacción
│   ├── repo.rs          # queries sqlx (validadas en compile)
│   ├── resolver.rs      # cache moka + load_from_db + record_use
│   └── invalidator.rs   # subscriber Valkey pub/sub (opcional)
├── storage/
│   ├── mod.rs           # trait + factory build(&StorageConfig)
│   ├── azure.rs         # Azure Blob via object_store
│   └── s3.rs            # S3 / S3-compatible (MinIO, R2) via object_store
└── admin/
    ├── mod.rs           # router /admin
    ├── auth.rs          # argon2id + signed session cookie
    ├── handlers.rs      # rutas (login, dashboard, CRUD)
    ├── templates.rs     # askama Template structs
    └── ratelimit.rs     # bucket en memoria (5/5min) para login

templates/admin/         # askama HTML — htmx + Pico CSS, embebido al compilar
migrations/              # .sql numerados, aplicados por sqlx::migrate!
.sqlx/                   # metadata cacheada para builds offline (commit)
examples/
└── gen_admin_hash.rs    # helper local para generar ADMIN_PASSWORD_HASH
```
