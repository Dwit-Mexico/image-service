//! `POST /upload/file` — sube un archivo "tal cual" (sin procesar): documentos
//! como PDF, etc. Comparte auth, storage y modelo de proyecto con los demás
//! endpoints de upload, pero NO transcodea ni recomprime: el byte stream se
//! guarda íntegro (un PDF firmado no debe alterarse). Allowlist de tipos por
//! extensión/MIME para no convertirlo en un dropbox abierto.

use std::sync::Arc;

use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::AppError, projects::ResolvedProject, storage};

#[derive(Deserialize, Default)]
struct FileOptions {
    /// Carpeta opcional dentro del container (p.ej. "foundations/huellas/docs").
    folder: Option<String>,
}

#[derive(Serialize)]
pub struct FileUploadResponse {
    pub id: String,
    pub url: String,
    pub bytes: usize,
    pub content_type: String,
    pub format: String,
}

// (extensión, MIME) permitidos. Documentos primero; imágenes por si fotografían
// el acta. Nada ejecutable ni HTML (evita XSS si el container es público).
fn resolve_type(
    file_name: Option<&str>,
    declared: Option<&str>,
) -> Option<(&'static str, &'static str)> {
    let ext = file_name
        .and_then(|n| n.rsplit('.').next())
        .map(|e| e.to_ascii_lowercase());
    let by_ext = match ext.as_deref() {
        Some("pdf") => Some(("pdf", "application/pdf")),
        Some("png") => Some(("png", "image/png")),
        Some("jpg") | Some("jpeg") => Some(("jpg", "image/jpeg")),
        Some("webp") => Some(("webp", "image/webp")),
        _ => None,
    };
    if by_ext.is_some() {
        return by_ext;
    }
    // Fallback al MIME declarado cuando el nombre no trae extensión útil.
    match declared {
        Some(m) if m.starts_with("application/pdf") => Some(("pdf", "application/pdf")),
        Some(m) if m.starts_with("image/png") => Some(("png", "image/png")),
        Some(m) if m.starts_with("image/jpeg") => Some(("jpg", "image/jpeg")),
        Some(m) if m.starts_with("image/webp") => Some(("webp", "image/webp")),
        _ => None,
    }
}

pub async fn upload_file_handler(
    Extension(project): Extension<Arc<ResolvedProject>>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<FileUploadResponse>, AppError> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;
    let mut declared_type: Option<String> = None;
    let mut opts = FileOptions::default();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        match field.name() {
            Some("file") => {
                file_name = field.file_name().map(|s| s.to_string());
                declared_type = field.content_type().map(|s| s.to_string());
                file_bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::BadRequest(e.to_string()))?
                        .to_vec(),
                );
            }
            Some("options") => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                opts = serde_json::from_str(&text).unwrap_or(opts);
            }
            _ => {}
        }
    }

    let raw = file_bytes.ok_or_else(|| AppError::BadRequest("campo 'file' requerido".into()))?;
    if raw.is_empty() {
        return Err(AppError::BadRequest("archivo vacío".into()));
    }

    let (ext, content_type) = resolve_type(file_name.as_deref(), declared_type.as_deref())
        .ok_or_else(|| {
            AppError::BadRequest("tipo de archivo no permitido (pdf, png, jpg, webp)".into())
        })?;

    let container = project
        .default_container
        .clone()
        .unwrap_or_else(|| "files".to_string());

    let uuid = Uuid::new_v4();
    let key = match opts.folder.as_deref().map(|f| f.trim_matches('/')) {
        Some(f) if !f.is_empty() => format!("{f}/{uuid}.{ext}"),
        _ => format!("{uuid}.{ext}"),
    };

    let store = storage::build(&project.storage_config)
        .map_err(|e| AppError::Storage(anyhow::anyhow!("build storage: {e}")))?;

    let url = store
        .upload(&container, &key, raw.clone(), content_type)
        .await?;

    Ok(Json(FileUploadResponse {
        id: key,
        url,
        bytes: raw.len(),
        content_type: content_type.to_string(),
        format: ext.to_string(),
    }))
}
