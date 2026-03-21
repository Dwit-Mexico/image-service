use axum::{http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("no autorizado: {0}")]
    Unauthorized(String),

    #[error("solicitud inválida: {0}")]
    BadRequest(String),

    #[error("error procesando imagen: {0}")]
    Processing(String),

    #[error("error de almacenamiento: {0}")]
    Storage(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            AppError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m.clone()),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            AppError::Processing(m) => (StatusCode::UNPROCESSABLE_ENTITY, m.clone()),
            AppError::Storage(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
