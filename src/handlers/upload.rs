use axum::{extract::State, Json};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    config::AppState,
    error::AppError,
    processing::{process_image, ProcessOptions},
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
    State(state): State<AppState>,
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
    process_and_upload(raw, opts, &state).await
}

pub(crate) async fn process_and_upload(
    raw: Vec<u8>,
    opts: ProcessOptions,
    state: &AppState,
) -> Result<Json<UploadResponse>, AppError> {
    let original_bytes = raw.len();
    let container = opts
        .container
        .clone()
        .unwrap_or_else(|| state.default_container.clone());

    let (compressed, format) = tokio::task::spawn_blocking({
        let raw = raw.clone();
        let opts = opts.clone();
        move || process_image(&raw, &opts)
    })
    .await
    .map_err(|e| AppError::Processing(e.to_string()))??;

    let key = format!("{}.{}", Uuid::new_v4(), format.extension());

    let url = state
        .storage
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
