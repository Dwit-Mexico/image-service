# image-service

Servicio HTTP en Rust (Axum) para subir, comprimir y servir imágenes desde Azure Blob Storage. Multi-tenant: cada proyecto cliente se identifica con mTLS + API key.

## Cómo funciona

```
cliente ──(mTLS + X-API-Key)──> Gateway API ──> image-service ──> Azure Blob
                                     │
                            termina TLS, valida cert,
                            reenvía X-Forwarded-Client-Cert
```

1. El Gateway termina mTLS y valida el certificado cliente contra la CA configurada (`k8s/mtls-policy.yaml`, vía `ClientTrafficPolicy`).
2. Reenvía el header `X-Forwarded-Client-Cert` con el `CN` del cert al backend.
3. El middleware de auth (`src/middleware/auth.rs`) extrae el CN, busca el proyecto registrado y compara el header `X-API-Key` con la key esperada en tiempo constante.
4. Si pasa, el handler decodifica la imagen, la redimensiona, la encoda al formato pedido y la sube al container de Azure.

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

Defaults: `quality=85`, `max_width=2048`, `format=webp`, `container=$DEFAULT_CONTAINER`, sin folder.

Respuesta:
```json
{
  "id": "users/123/avatars/<uuid>.webp",
  "url": "https://<account>.blob.core.windows.net/<container>/<id>",
  "original_bytes": 1234567,
  "compressed_bytes": 87654,
  "format": "webp"
}
```

### `POST /upload/batch`

Mismo formato, pero acepta múltiples campos `file`. Las `options` son compartidas para todo el batch. Procesa en paralelo y devuelve un resultado por imagen indexado por orden de envío.

## Configuración

Variables de entorno (ver `.env.example`):

| Variable                            | Requerida | Descripción                                          |
|-------------------------------------|-----------|------------------------------------------------------|
| `AZURE_STORAGE_CONNECTION_STRING`   | sí        | Connection string completa de la storage account     |
| `DEFAULT_CONTAINER`                 | no        | Container Azure por defecto (default: `images`)      |
| `PROJECT_*`                         | sí (≥1)   | Registro de proyectos — ver abajo                    |
| `LISTEN_ADDR`                       | no        | Default: `0.0.0.0:8080`                              |
| `RUST_LOG`                          | no        | Default: `info`                                      |

### Registro de proyectos

Cualquier env var con prefijo `PROJECT_` se interpreta como un proyecto. **El nombre de la variable es solo etiqueta**; lo que importa es el valor, en formato `<cert_cn>:<api_key>`:

```
PROJECT_VELVET=velvet:sk_live_xxxxxxxx
PROJECT_PIXEL_SOLIDARY=pixel-solidary:sk_live_yyyyyyyy
PROJECT_PIXEL_SOLIDARI_DASHBOARD=pixel-solidari-dashboard:sk_live_zzzzzzzz
```

Al hacer un request, el servicio matchea por:
- `cert_cn` = CN del certificado mTLS del cliente (case-insensitive)
- `api_key` = header `X-API-Key` (comparación timing-safe)

Si o el CN no está registrado, o la API key no coincide → `401 Unauthorized`.

## Desarrollo local

```bash
cp .env.example .env
# editar .env con tus credenciales
cargo run
```

El servicio escucha en `:8080`. En local no hay gateway de mTLS, así que para probar el auth puedes mandar el header `X-Client-Cert-CN` directamente (fallback que acepta el middleware):

```bash
curl -X POST http://localhost:8080/upload \
  -H "X-Client-Cert-CN: velvet" \
  -H "X-API-Key: sk_live_xxxxxxxx" \
  -F "file=@foto.jpg" \
  -F 'options={"quality":80,"max_width":1024,"folder":"test"}'
```

## Deploy

### Infraestructura (k8s + Gateway API)

Manifests en `k8s/`:
- `image-service/2-deployment.yaml` — Deployment (2 réplicas, envFrom el secret `image-service`)
- `image-service/3-service.yaml` — ClusterIP en :80 → :8080
- `http-route.yaml` — `HTTPRoute` que expone `image-service.dwitmexico.com` en el listener `https-mtls` del `Gateway` `gateway-api`
- `mtls-policy.yaml` — `ClientTrafficPolicy` que valida certs cliente contra `image-service-ca-secret`

### Workflows (CI/CD)

Dos workflows en `.github/workflows/`:

**`deploy.yml`** — corre en push a `main` o manual:
1. Build de la imagen Docker y push a `ghcr.io/dwit-mexico/image-service:<sha>`
2. SCP de los manifests al servidor
3. SSH al servidor → reescribe el secret `image-service` con `kubectl create secret --from-literal=... | kubectl apply -f -` → primer deploy aplica manifests, subsecuentes solo actualizan la imagen (`kubectl set image`)

**`update-secrets.yml`** — manual (`workflow_dispatch`):
- Reescribe solo el secret `image-service` y hace `rollout restart`. Úsalo cuando agregues/rotes un proyecto sin cambiar código.

> **Importante**: ambos workflows escriben el secret completo con `--from-literal=`. Si agregas un nuevo `PROJECT_*` en GitHub Secrets, tienes que **agregarlo también en los dos YAMLs** (sección `env:`, lista `envs:`, y la línea `--from-literal=`), sino la próxima corrida lo va a dejar fuera del secret de k8s.

### Manejo de secrets — gotchas

- **No pre-codifiques en base64**: k8s ya guarda los secrets en base64 internamente. Si los pegas pre-codificados en GitHub Secrets, kubectl los re-codifica → doble base64 y el pod recibe basura.
- **El nombre del secret en GitHub no afecta runtime**: el servicio solo lee el valor (`cn:api_key`). El nombre es para humanos / mapeo al env var del pod.

## Estructura del código

```
src/
├── main.rs              # bootstrap del server, rutas, middleware
├── config.rs            # AppState, parseo de PROJECT_* env vars
├── error.rs             # AppError + mapeo a respuesta HTTP
├── middleware/
│   └── auth.rs          # mTLS CN + X-API-Key
├── handlers/
│   ├── health.rs
│   ├── upload.rs        # /upload (single)
│   └── batch.rs         # /upload/batch (paralelo)
├── processing/
│   └── image.rs         # decode + resize + encode (webp/jpeg/png)
└── storage/
    ├── mod.rs           # trait StorageProvider
    └── azure.rs         # implementación Azure Blob
```

El `trait StorageProvider` está pensado para poder agregar otros backends (S3, GCS) sin tocar los handlers.
