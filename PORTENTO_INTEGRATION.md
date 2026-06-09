# Integración Portento (cliente `external_pt`) — solo S3

Doc para configurar el image-service de modo que **portento** pueda subir imágenes
(empezando por los adjuntos de *bug reports*). Enfocado exclusivamente en backend **S3**.

## 0. Plan por fases (decisión actual)

| Fase | Bucket | Protección | Trabajo en portento |
|------|--------|------------|---------------------|
| **Hoy** | el bucket **público** existente `portento` | obscuridad por UUID en la key + chequeo de rol del proxy de portento | **cero** (la subida ya migró al servicio; el serving sigue por el proxy existente) |
| **Fin de semana** | bucket **privado** nuevo | real | **cero código** — solo repuntar `external_pt` al bucket privado; el proxy funciona igual |

Clave: el proxy de portento lee el objeto de S3 con creds AWS, así que **funciona igual
sobre bucket público o privado**. Por eso no se cambia código entre fases: el upgrade a
protección real es puramente infra (crear bucket privado + repuntar el proyecto). La `url`
pública que devuelve el servicio se ignora en ambas fases; portento usa solo el `id` (key).

> ⚠️ Mientras el bucket sea público, el objeto es alcanzable por URL directa si se conoce
> la key (la key lleva UUID, por eso es "obscuridad", no control de acceso real). La
> protección real llega en la fase de fin de semana con el bucket privado.

---

## 1. Contexto y decisión de arquitectura

El image-service es **solo de subida**: expone `POST /upload` y `POST /upload/batch`,
pero **no tiene endpoint para servir/descargar** imágenes ni para borrarlas. La `url`
que devuelve es la URL plana del objeto en S3.

Portento sirve las imágenes de bug-tickets por un **proxy autenticado** en la propia app
de Next.js (`GET /bug-tickets/[id]/images/[imageID]`, gateado por rol y con
`Cache-Control: private, no-store`). Ese proxy lee el objeto de S3 con creds AWS, así que
**funciona igual sea el bucket público o privado** — de ahí el plan por fases (§0).

> Nota de estado real: el bucket `portento` **es público hoy** (otros flujos sirven por
> URL directa). Por eso en la fase actual el proxy aporta el chequeo de rol pero el objeto
> sigue alcanzable por URL si se conoce la key. La protección real llega al mover el
> contenido sensible a un bucket **privado** (fin de semana).

La decisión de integración (válida en ambas fases):

> **El proyecto `external_pt` escribe en el bucket S3 que el proxy de portento lee.**
> Hoy = el bucket público `portento`. Fin de semana = un bucket privado nuevo (solo se
> repunta el proyecto; el proxy no cambia).

Así el objeto comprimido cae exactamente en la key que el proxy de portento ya lee. La
`url` pública que devuelve el servicio **se ignora**; portento guarda y usa solo el `id`
(la key).

```
portento ──(POST /upload, X-API-Key + X-Client-Cert-CN: external_pt)──> image-service
                                                                              │
                                                          comprime (webp) y PUT a S3
                                                                              ▼
                                  bucket S3 que lee portento  ◀── hoy: público `portento`
                                                              ◀── finde: privado nuevo
                                                                              ▲
portento (proxy autenticado, lee la key) ─────────────────────────────────┘
```

> Si `external_pt` apuntara a un bucket **propio** del image-service (que portento no
> puede leer con sus creds), el proxy no funcionaría y solo quedaría la URL pública. Por
> eso el bucket debe ser uno que portento pueda leer.

---

## 2. Acción requerida en el servicio: crear el proyecto `external_pt`

Crear el proyecto con backend S3 apuntando al bucket de portento. Datos de portento:

| Campo      | Valor                     |
|------------|---------------------------|
| `cert_cn`  | `external_pt`             |
| `region`   | `mx-central-1`            |
| `bucket`   | `portento`                |
| `access` / `secret` | las **mismas** creds AWS que ya usa portento (sus secrets `DEPLOY_AWS_ACCESS_KEY_ID` / `DEPLOY_AWS_SECRET_ACCESS_KEY`) |

Comando (CLI `project-admin`, orden exacto verificado en `src/bin/project_admin.rs`):

```bash
# create-s3 <name> <cert_cn> <access_key> <secret_key> <region> <bucket> [endpoint]
project-admin create-s3 "Portento" external_pt "<AWS_ACCESS_KEY_ID>" "<AWS_SECRET_ACCESS_KEY>" mx-central-1 portento
```

Esto imprime la **API key en plano una sola vez**. Guardarla (ver §3). Sin `endpoint`
(es S3 de AWS, no MinIO/R2).

> Alternativa: crearlo desde la UI admin (`/admin/projects/new?backend=s3`). Mismo
> resultado; la key plaintext se muestra una vez.

---

## 3. Handoff de credenciales hacia portento

Del paso anterior salen dos valores que portento necesita:

| Valor              | De dónde sale          | Dónde lo pone portento |
|--------------------|------------------------|------------------------|
| **API key** (plain)| output de `create-s3`  | 1Password `op://Private/portento/image-service/api_key` **y** GitHub secret `IMAGE_SERVICE_API_KEY` |
| **cert CN**        | `external_pt` (fijo)   | 1Password `op://Private/portento/image-service/cert_cn` **y** GitHub secret `IMAGE_SERVICE_CERT_CN` |

Portento manda en cada request:
- `X-API-Key: <api key>`
- `X-Client-Cert-CN: external_pt`

(Host del servicio que usa portento: `https://image-service.dwitmexico.com`.)

---

## 4. Contrato que consume portento (lo que NO debe cambiar)

Portento depende de este comportamiento del servicio. Si algo de esto cambia, se rompe
la integración.

### `POST /upload` (multipart)
- Campo `file`: bytes de la imagen.
- Campo `options` (JSON), portento envía:
  ```json
  { "folder": "bug-tickets/<ticketID>", "format": "webp", "quality": 85, "max_width": 2048 }
  ```
  (hoy portento solo fuerza `folder`; el resto usa los defaults del servicio).

### Respuesta — portento usa **`id`**, ignora `url`
```json
{
  "id": "bug-tickets/123/<uuid>.webp",   ← se guarda en DB como key S3 del objeto
  "url": "https://portento.s3.mx-central-1.amazonaws.com/bug-tickets/123/<uuid>.webp",  ← ignorado
  "original_bytes": 1234567,
  "compressed_bytes": 87654,
  "format": "webp"
}
```

### Invariantes que el servicio debe garantizar (S3)
1. **`id` == key real del objeto en S3.** Confirmado en `src/storage/s3.rs`: la key es
   `"{folder}/{uuid}.{ext}"` y es lo que se devuelve como `id`. Portento usa ese `id`
   tal cual como `Key` en su `GetObjectCommand`. No anteponer prefijos ocultos ni
   transformar la key.
2. **El objeto se escribe en el bucket configurado del proyecto** (el de portento), no
   en otro. Confirmado: en S3 el `container` se ignora y el bucket es fijo por proyecto
   (`s3.rs`).
3. **Content-Type correcto** en el PUT (p.ej. `image/webp`). Ya se hace
   (`PutOptions` con `Attribute::ContentType`).
4. **No requerir que el bucket sea público.** El PUT no debe depender de ACL public-read.
   (Mantener el bucket privado es responsabilidad de portento, pero el servicio no debe
   forzar lo contrario.)

---

## 5. Gaps conocidos — qué NO se necesita ahora (y qué flaggear a futuro)

| Capacidad           | ¿Necesaria para bug reports? | Nota |
|---------------------|------------------------------|------|
| Endpoint GET/serve  | **No**                       | portento sirve por su proxy autenticado leyendo la key del bucket compartido. |
| Endpoint de delete  | **No**                       | bug-tickets hace **soft-delete en DB**; el objeto S3 no se borra. |

> ⚠️ **A futuro**: otras secciones de portento (p.ej. documentos de gastos) sí borran del
> storage (`deleteBlob`). Cuando lleguemos a esas, o portento borra directo en S3 con sus
> propias creds (posible, porque es el mismo bucket), o el image-service necesitaría
> exponer un `DELETE`. Para bug reports no aplica.

---

## 6. Smoke test (sin gateway mTLS, header directo)

Una vez creado `external_pt`, validar contra el servicio:

```bash
curl -X POST https://image-service.dwitmexico.com/upload \
  -H "X-Client-Cert-CN: external_pt" \
  -H "X-API-Key: <api key plain>" \
  -F "file=@captura.png" \
  -F 'options={"folder":"bug-tickets/0","format":"webp"}'
```

Verificar:
- [ ] Respuesta `200` con `id = bug-tickets/0/<uuid>.webp`.
- [ ] El objeto existe en `s3://portento/bug-tickets/0/<uuid>.webp` (mismo bucket de portento).
- [ ] El bucket `portento` sigue **privado** (la URL pública no descarga sin firma).
- [ ] Portento puede leer esa key vía su proxy autenticado.

---

## 7. Resumen — qué necesito confirmado del lado del servicio

1. Proyecto **`external_pt`** creado con `create-s3` apuntando a **bucket `portento`,
   región `mx-central-1`**, con las creds AWS de portento.
2. **API key** entregada a portento (1Password + GH secret).
3. Invariantes de §4 garantizadas (id == key, mismo bucket, content-type, no exige bucket público).
4. Confirmar que para bug reports **no** se requiere serve ni delete en el servicio (§5).
