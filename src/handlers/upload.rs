use std::sync::Arc;

use axum::{Extension, Json};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    error::AppError,
    processing::{process_image, ProcessOptions},
    projects::ResolvedProject,
    storage,
};

#[derive(Serialize)]
pub struct UploadResponse {
    pub id: String,
    pub url: String,
    pub original_bytes: usize,
    pub compressed_bytes: usize,
    pub format: String,
}

pub async fn upload_handler(
    Extension(project): Extension<Arc<ResolvedProject>>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<UploadResponse>, AppError> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut opts = ProcessOptions::default();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        match field.name() {
            Some("file") => {
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
    process_and_upload(raw, opts, &project).await
}

pub(crate) async fn process_and_upload(
    raw: Vec<u8>,
    opts: ProcessOptions,
    project: &ResolvedProject,
) -> Result<Json<UploadResponse>, AppError> {
    let original_bytes = raw.len();

    let container = opts
        .container
        .clone()
        .or_else(|| project.default_container.clone())
        .unwrap_or_else(|| "images".to_string());

    let (compressed, format) = tokio::task::spawn_blocking({
        let raw = raw.clone();
        let opts = opts.clone();
        move || process_image(&raw, &opts)
    })
    .await
    .map_err(|e| AppError::Processing(e.to_string()))??;

    let key = match &opts.folder {
        Some(folder) => format!(
            "{}/{}.{}",
            folder.trim_matches('/'),
            Uuid::new_v4(),
            format.extension()
        ),
        None => format!("{}.{}", Uuid::new_v4(), format.extension()),
    };

    let store = storage::build(&project.storage_config)
        .map_err(|e| AppError::Storage(anyhow::anyhow!("build storage: {e}")))?;
    let url = store
        .upload(&container, &key, compressed.clone(), format.mime())
        .await?;

    Ok(Json(UploadResponse {
        id: key,
        url,
        original_bytes,
        compressed_bytes: compressed.len(),
        format: format.extension().to_string(),
    }))
}
