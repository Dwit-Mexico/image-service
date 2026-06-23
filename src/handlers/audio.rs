//! `POST /upload/audio` — sube un audio, lo transcodea a MP3 y lo sube al
//! storage del proyecto.

use std::sync::Arc;

use axum::{Extension, Json};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    error::AppError,
    processing::{process_audio, AudioOptions},
    projects::ResolvedProject,
    storage,
};

#[derive(Serialize)]
pub struct AudioUploadResponse {
    pub id: String,
    pub url: String,
    pub original_bytes: usize,
    pub compressed_bytes: usize,
    pub duration_seconds: f32,
    pub format: String,
}

pub async fn upload_audio_handler(
    Extension(project): Extension<Arc<ResolvedProject>>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<AudioUploadResponse>, AppError> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut opts = AudioOptions::default();

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
    let original_bytes = raw.len();

    let result = process_audio(&raw, &opts).await?;

    let container = project
        .default_container
        .clone()
        .unwrap_or_else(|| "audios".to_string());

    let uuid = Uuid::new_v4();
    let key = match opts.folder.as_deref().map(|f| f.trim_matches('/')) {
        Some(f) if !f.is_empty() => format!("{f}/{uuid}.mp3"),
        _ => format!("{uuid}.mp3"),
    };

    let store = storage::build(&project.storage_config)
        .map_err(|e| AppError::Storage(anyhow::anyhow!("build storage: {e}")))?;

    let url = store
        .upload(&container, &key, result.bytes.clone(), "audio/mpeg")
        .await?;

    Ok(Json(AudioUploadResponse {
        id: key,
        url,
        original_bytes,
        compressed_bytes: result.bytes.len(),
        duration_seconds: result.duration_seconds,
        format: "mp3".to_string(),
    }))
}
