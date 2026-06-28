# Endpoints de media (video, audio, file)

Spec de uso para `POST /upload/video`, `POST /upload/audio` y `POST /upload/file`. Comparten auth, storage backend y modelo de proyecto con los endpoints de imagen — sin impacto en `/upload` o `/upload/batch`.

## Auth (igual que image)

Mismo middleware que `/upload`. Headers requeridos en cada request:

```
X-Client-Cert-CN: <cert_cn>   # o vía mTLS pass-through del gateway
X-API-Key: <plain_api_key>
```

Sin estos → `401 Unauthorized`. CN o key inválidos → `401`.

---

## `POST /upload/video`

### Request

Multipart:

| Campo     | Requerido | Tipo            | Descripción |
|-----------|-----------|-----------------|-------------|
| `file`    | sí        | bytes del video | Cualquier formato que ffmpeg pueda decodificar (mp4, mov, webm, mkv, avi…) |
| `options` | no        | JSON string     | Ver tabla de opciones abajo |

**Body máximo: 300 MB.** El input puede ser pesado (sin comprimir); la salida será mucho menor.

### Options

| Campo                  | Tipo  | Default | Rango / Notas |
|------------------------|-------|---------|---------------|
| `max_height`           | u32   | 720     | Altura máxima en px. Si el input es menor, se conserva. Ancho se calcula preservando aspect ratio. |
| `crf`                  | u8    | 24      | Calidad H.264. `0` lossless, `51` peor. Sweet spot web 22-28. |
| `audio_bitrate_k`      | u32   | 128     | kbps del audio AAC en el output. |
| `max_duration_seconds` | f32   | 120     | Tope absoluto. El servicio rechaza con 400 si excede. **No puede pasar de 120** sin cambiar código — es defensa contra uploads de horas. |
| `folder`               | str   | (raíz)  | Prefijo de Key dentro del container (sin slash inicial/final). |

### Procesamiento

1. `ffprobe` extrae duración → si excede `max_duration_seconds`, retorna 400.
2. `ffmpeg` transcoda a **MP4 H.264 (libx264, preset fast) + AAC**, escala a `max_height` preservando aspect ratio, agrega `+faststart` para streaming progresivo.
3. Segundo pase de `ffmpeg` extrae **thumbnail WebP** del segundo 1 (o frame 0 si el video dura < 1s), escalado igual.
4. Sube video y thumbnail al `default_container` del proyecto (Azure o S3, según el backend).

### Response (200 OK)

```json
{
  "id": "tickets/123/9a3f-…-mp4-uuid.mp4",
  "url": "https://<host>/<container>/<id>",
  "thumbnail_id": "tickets/123/9a3f-…-mp4-uuid-thumb.webp",
  "thumbnail_url": "https://<host>/<container>/<thumbnail_id>",
  "original_bytes": 25600000,
  "compressed_bytes": 4800000,
  "thumbnail_bytes": 18432,
  "duration_seconds": 47.3,
  "format": "mp4"
}
```

### Invariante crítico para clientes

**`id` y `thumbnail_id` son la Key real del objeto en el bucket/container del proyecto**, sin transformación. El cliente puede hacer:

```ts
// AWS SDK
new GetObjectCommand({ Bucket: <project_bucket>, Key: <id> })

// Azure Blob
containerClient.getBlobClient(<id>).download()
```

Y recupera el objeto exacto. Lo mismo aplica para `thumbnail_id`.

> Si el `id` no funciona en tu GetObject, el bug está en este servicio — no normalices/transformes la Key del lado del cliente. Reporta el caso.

### Errores

| Status | Body                                                                                  | Causa |
|--------|---------------------------------------------------------------------------------------|-------|
| 400    | `{"error":"video dura 240.5s, máximo permitido 120s"}`                                | Excede `max_duration_seconds` |
| 400    | `{"error":"campo 'file' requerido"}`                                                  | Multipart sin campo `file` |
| 400    | `{"error":"ffprobe rechazó el archivo: ..."}`                                          | Input corrupto o formato no decodificable |
| 401    | `{"error":"API key inválida"}` / `{"error":"proyecto 'x' no registrado"}`             | Auth |
| 413    | (cuerpo de tower)                                                                     | Body > 300 MB |
| 422    | `{"error":"ffmpeg falló: ..."}`                                                       | Falla de encoding (rara) |
| 500    | `{"error":"build storage: ..."}`                                                      | Misconfig del proyecto (endpoint inválido, creds malas) |

### Ejemplo curl

```bash
curl -X POST https://image-service.dwitmexico.com/upload/video \
  -H "X-Client-Cert-CN: project-velvet" \
  -H "X-API-Key: sk_live_..." \
  -F "file=@clip.mov" \
  -F 'options={"max_height":720,"crf":24,"folder":"tickets/123","max_duration_seconds":120}'
```

---

## `POST /upload/audio`

### Request

Multipart:

| Campo     | Requerido | Tipo            | Descripción |
|-----------|-----------|-----------------|-------------|
| `file`    | sí        | bytes del audio | Cualquier formato que ffmpeg pueda decodificar (mp3, m4a, wav, flac, ogg, opus, aac…) |
| `options` | no        | JSON string     | Ver tabla abajo |

**Body máximo: 30 MB.** Suficiente para 3 min de WAV sin comprimir o 30+ min de mp3.

### Options

| Campo                  | Tipo | Default | Rango / Notas |
|------------------------|------|---------|---------------|
| `bitrate_k`            | u32  | 128     | kbps del output MP3. 96-192 razonable. |
| `max_duration_seconds` | f32  | 180     | Tope absoluto. El servicio rechaza con 400 si excede. |
| `folder`               | str  | (raíz)  | Prefijo de Key. |

### Procesamiento

1. `ffprobe` extrae duración → 400 si excede.
2. `ffmpeg` transcoda a **MP3** (`libmp3lame`) con `-vn` (descarta video si el input lo trae embebido).
3. Sube al `default_container` del proyecto.

### Response (200 OK)

```json
{
  "id": "voice-notes/u42/9a3f-…-uuid.mp3",
  "url": "https://<host>/<container>/<id>",
  "original_bytes": 4096000,
  "compressed_bytes": 2880000,
  "duration_seconds": 174.2,
  "format": "mp3"
}
```

`id` = Key real en S3/Azure, mismo invariante que video e image.

### Errores

| Status | Body | Causa |
|--------|------|-------|
| 400    | `{"error":"audio dura 240.5s, máximo permitido 180s"}` | Excede `max_duration_seconds` |
| 400    | `{"error":"campo 'file' requerido"}` | Multipart sin file |
| 400    | `{"error":"ffprobe rechazó el archivo: ..."}` | Input corrupto |
| 401    | auth | Mismo que video |
| 413    | (tower) | Body > 30 MB |
| 422    | `{"error":"ffmpeg falló: ..."}` | Encoding rara |
| 500    | `{"error":"build storage: ..."}` | Misconfig proyecto |

### Ejemplo curl

```bash
curl -X POST https://image-service.dwitmexico.com/upload/audio \
  -H "X-Client-Cert-CN: project-velvet" \
  -H "X-API-Key: sk_live_..." \
  -F "file=@note.m4a" \
  -F 'options={"bitrate_k":128,"folder":"voice-notes/u42"}'
```

---

## `POST /upload/file`

Passthrough crudo — **no transcodea, no recomprime**. Guarda el byte stream íntegro. Pensado para documentos firmados (PDF) donde alterar un solo byte invalida la firma, o para imágenes que ya vienen optimizadas y no quieres re-encodear.

### Request

Multipart:

| Campo     | Requerido | Tipo               | Descripción |
|-----------|-----------|--------------------|-------------|
| `file`    | sí        | bytes del archivo  | Su `filename` y `Content-Type` se usan para detectar el tipo |
| `options` | no        | JSON string        | `{ "folder": "..." }` opcional |

**Body máximo: 30 MB.**

### Allowlist de tipos

Solo se aceptan estos (por extensión del filename o por MIME declarado):

| Extensión | MIME              |
|-----------|-------------------|
| `pdf`     | `application/pdf` |
| `png`     | `image/png`       |
| `jpg/jpeg`| `image/jpeg`      |
| `webp`    | `image/webp`      |

Cualquier otro tipo → `400 Bad Request: "tipo de archivo no permitido (pdf, png, jpg, webp)"`.

> **Por qué tan corta la lista**: HTML/SVG/SWF/etc. pueden contener scripts, y si el container de destino es público (o accesible directo por URL) se vuelven vector XSS. Si necesitas otro tipo seguro, agrégalo en `src/handlers/file.rs::resolve_type`.

### Procesamiento

1. Detecta tipo: primero extensión del filename, fallback al `Content-Type` declarado en el multipart.
2. Valida contra allowlist.
3. Sube bytes íntegros al `default_container` del proyecto (fallback `"files"` si no está definido).

**No hay validación de contenido** — no se parsea el PDF ni se inspeccionan los píxeles. Confías en el cliente sobre lo que sube.

### Response (200 OK)

```json
{
  "id": "foundations/huellas/docs/<uuid>.pdf",
  "url": "https://<host>/<container>/<id>",
  "bytes": 184320,
  "content_type": "application/pdf",
  "format": "pdf"
}
```

`id` = Key real en S3/Azure, mismo invariante. Note que **no hay `original_bytes`/`compressed_bytes`** — el archivo no se modifica, así que solo `bytes` (= tamaño original = tamaño almacenado).

### Errores

| Status | Body                                                                | Causa |
|--------|---------------------------------------------------------------------|-------|
| 400    | `{"error":"campo 'file' requerido"}`                                | Multipart sin campo `file` |
| 400    | `{"error":"archivo vacío"}`                                          | Body de 0 bytes |
| 400    | `{"error":"tipo de archivo no permitido (pdf, png, jpg, webp)"}`     | Ext/MIME fuera de la allowlist |
| 401    | auth                                                                | Mismo que video/audio |
| 413    | (tower)                                                             | Body > 30 MB |
| 500    | `{"error":"build storage: ..."}`                                    | Misconfig proyecto |

### Ejemplo curl

```bash
curl -X POST https://image-service.dwitmexico.com/upload/file \
  -H "X-Client-Cert-CN: project-velvet" \
  -H "X-API-Key: sk_live_..." \
  -F "file=@acta.pdf" \
  -F 'options={"folder":"foundations/huellas/docs"}'
```

---

## Consideraciones para el cliente

### Qué almacenar en tu DB

Para cada upload de video, guarda mínimo:

```
video_key       = response.id
thumbnail_key   = response.thumbnail_id   # solo video
duration        = response.duration_seconds
size_bytes      = response.compressed_bytes
```

Para audio:

```
audio_key       = response.id
duration        = response.duration_seconds
size_bytes      = response.compressed_bytes
```

Para file (PDF/imagen passthrough):

```
file_key        = response.id
size_bytes      = response.bytes
content_type    = response.content_type
```

**No guardes `url`** — es derivable y puede cambiar si rotas el storage backend (AWS → R2, p.ej.). La Key sí es estable.

### Reproductor en la web

- **Video**: `<video src="..." poster="<thumbnail_url>" controls>`. El MP4 con `+faststart` permite playback progresivo sin esperar al download completo. H.264 + AAC funciona en Safari/iOS sin extras.
- **Audio**: `<audio src="..." controls>`. MP3 reproduce en todos los browsers sin transcoding del lado del cliente.

Si tu storage es **privado**, el cliente debe firmar URLs (presigned) o proxy-stream. La `url` que devuelve el servicio asume bucket público; ignórala si tu setup es privado y construye GetObject desde la `id`.

### Reintentos

- `400` (duración excedida, archivo corrupto): **no reintentes**. Pide al usuario un archivo más corto / arregla el origen.
- `401`: **no reintentes**. Rota credenciales si crees que están comprometidas.
- `413` (body > 300 MB para video, > 30 MB para audio): comprime del lado del cliente antes de enviar.
- `5xx`, timeouts, connection reset: reintenta con backoff exponencial. El servicio es idempotente solo en lectura — un retry de upload genera un nuevo `uuid`, así que asegúrate de manejar dedup del lado del cliente si te importa.

---

## Cambios respecto a `/upload` (image)

Si vienes del endpoint de imagen, lo nuevo:

- **Body limit diferente** para video (300 MB vs 30 MB)
- **Validación de duración** con 400 antes de transcoding
- **Thumbnail extra** para video, con su propio `id`/`url` en la respuesta
- **Sin opciones de formato output** — siempre MP4/H.264+AAC para video, MP3 para audio
- **Sin parámetro `container`** en options (image lo soportaba para override) — usa el `default_container` del proyecto

Si necesitas un container distinto para videos/audios, configúralo en el proyecto vía `project-admin` o la UI admin (`default_container`).

---

## Restricciones del servicio

| Aspecto | Video | Audio | File |
|---------|-------|-------|------|
| Duración máxima | 120s (2 min) | 180s (3 min) | n/a |
| Body máximo | 300 MB | 30 MB | 30 MB |
| Output codec | H.264 (libx264) + AAC | MP3 (libmp3lame) | passthrough |
| Output container | MP4 (`+faststart`) | MP3 | original |
| Resolución | escala a 720p si excede | — | original |
| Thumbnail | sí (WebP del seg. 1) | no | no |
| Formatos input | cualquiera que ffmpeg decodifique | cualquiera que ffmpeg decodifique | pdf, png, jpg, webp |
| Modifica el archivo | sí (transcoda) | sí (transcoda) | **no** |
