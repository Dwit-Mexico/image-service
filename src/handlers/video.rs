//! `POST /upload/video` — sube un video, lo transcodea a MP4 (H.264 + AAC) y
//! genera un thumbnail WebP. Sube ambos al storage del proyecto.
//!
//! Response: `id` y `url` del video, más `thumbnail_id` y `thumbnail_url` —
//! pensados para almacenarse del lado del cliente para reconstruir GetObject.

use std::sync::Arc;

use axum::{Extension, Json};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    error::AppError,
    processing::{process_video, VideoOptions},
    projects::ResolvedProject,
    storage,
};

#[derive(Serialize)]
pub struct VideoUploadResponse {
    pub id: String,
    pub url: String,
    pub thumbnail_id: String,
    pub thumbnail_url: String,
    pub original_bytes: usize,
    pub compressed_bytes: usize,
    pub thumbnail_bytes: usize,
    pub duration_seconds: f32,
    pub format: String,
}

pub async fn upload_video_handler(
    Extension(project): Extension<Arc<ResolvedProject>>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<VideoUploadResponse>, AppError> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut opts = VideoOptions::default();

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

    let result = process_video(&raw, &opts).await?;

    let container = project
        .default_container
        .clone()
        .unwrap_or_else(|| "videos".to_string());

    let uuid = Uuid::new_v4();
    let folder = opts.folder.as_deref().map(|f| f.trim_matches('/'));
    let video_key = match folder {
        Some(f) if !f.is_empty() => format!("{f}/{uuid}.mp4"),
        _ => format!("{uuid}.mp4"),
    };
    let thumbnail_key = match folder {
        Some(f) if !f.is_empty() => format!("{f}/{uuid}-thumb.webp"),
        _ => format!("{uuid}-thumb.webp"),
    };

    let store = storage::build(&project.storage_config)
        .map_err(|e| AppError::Storage(anyhow::anyhow!("build storage: {e}")))?;

    let video_url = store
        .upload(&container, &video_key, result.video_bytes.clone(), "video/mp4")
        .await?;
    let thumbnail_url = store
        .upload(
            &container,
            &thumbnail_key,
            result.thumbnail_bytes.clone(),
            "image/webp",
        )
        .await?;

    Ok(Json(VideoUploadResponse {
        id: video_key,
        url: video_url,
        thumbnail_id: thumbnail_key,
        thumbnail_url,
        original_bytes,
        compressed_bytes: result.video_bytes.len(),
        thumbnail_bytes: result.thumbnail_bytes.len(),
        duration_seconds: result.duration_seconds,
        format: "mp4".to_string(),
    }))
}
