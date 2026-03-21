use axum::{extract::State, Json};
use futures::future::join_all;
use serde::Serialize;

use crate::{config::AppState, error::AppError, processing::ProcessOptions};

use super::upload::{process_and_upload, UploadResponse};

#[derive(Serialize)]
pub struct BatchUploadResponse {
    pub results: Vec<BatchItem>,
    pub total: usize,
    pub ok: usize,
    pub failed: usize,
}

#[derive(Serialize)]
pub struct BatchItem {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<UploadResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Acepta multipart con múltiples campos `file` y un campo opcional `options`
/// (JSON compartido para todas las imágenes del batch).
pub async fn batch_upload_handler(
    State(state): State<AppState>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<BatchUploadResponse>, AppError> {
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut opts = ProcessOptions::default();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        match field.name() {
            Some("file") => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?
                    .to_vec();
                files.push(bytes);
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

    if files.is_empty() {
        return Err(AppError::BadRequest(
            "al menos un campo 'file' requerido".into(),
        ));
    }

    // Procesar en paralelo
    let tasks: Vec<_> = files
        .into_iter()
        .enumerate()
        .map(|(i, raw)| {
            let opts = opts.clone();
            let state = state.clone();
            async move {
                let res = process_and_upload(raw, opts, &state).await;
                (i, res)
            }
        })
        .collect();

    let outcomes = join_all(tasks).await;
    let total = outcomes.len();
    let mut results = Vec::with_capacity(total);
    let mut ok = 0usize;
    let mut failed = 0usize;

    for (index, outcome) in outcomes {
        match outcome {
            Ok(Json(r)) => {
                ok += 1;
                results.push(BatchItem {
                    index,
                    result: Some(r),
                    error: None,
                });
            }
            Err(e) => {
                failed += 1;
                results.push(BatchItem {
                    index,
                    result: None,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    // Ordenar por índice original
    results.sort_by_key(|r| r.index);

    Ok(Json(BatchUploadResponse {
        results,
        total,
        ok,
        failed,
    }))
}
