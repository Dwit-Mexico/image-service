use std::sync::Arc;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use crate::{
    config::AppState,
    error::AppError,
    projects::{api_key, ResolveError, ResolvedProject},
};

/// Doble verificación: cert CN (vía mTLS forward header) + API key por proyecto.
///
/// El Gateway termina mTLS y reenvía la cadena `X-Forwarded-Client-Cert` con
/// el CN del certificado cliente. El header `X-API-Key` debe coincidir con
/// la key registrada para ese cert_cn.
///
/// El `ResolvedProject` resultante se inyecta en `request.extensions()` para
/// que los handlers lo extraigan con `Extension<Arc<ResolvedProject>>`.
pub async fn auth_middleware(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let cert_cn = extract_cert_cn(&headers).ok_or_else(|| {
        AppError::Unauthorized("certificado cliente ausente o inválido".into())
    })?;

    let resolved = state
        .resolver
        .resolve(&cert_cn.to_lowercase())
        .await
        .map_err(|e| match e {
            ResolveError::NotFound => {
                AppError::Unauthorized(format!("proyecto '{cert_cn}' no registrado"))
            }
            other => {
                tracing::error!("resolver error para cn={cert_cn}: {other}");
                AppError::Unauthorized("auth no disponible".into())
            }
        })?;

    let provided_key = headers
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("header X-API-Key ausente".into()))?;

    if !api_key::verify(provided_key, &resolved.api_key_hash) {
        return Err(AppError::Unauthorized("API key inválida".into()));
    }

    state.resolver.record_use(resolved.id);
    request.extensions_mut().insert(Arc::clone(&resolved));
    Ok(next.run(request).await)
}

/// Extrae el CN del header `X-Forwarded-Client-Cert` que el Gateway inyecta,
/// o del fallback `X-Client-Cert-CN` si el Gateway ya lo parseó.
///
/// Formato XFCC: `By=...;Hash=...;Subject="CN=project-alpha,O=Acme"`
fn extract_cert_cn(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(cn) = headers
        .get("X-Client-Cert-CN")
        .and_then(|v| v.to_str().ok())
    {
        return Some(cn.to_string());
    }

    let xfcc = headers
        .get("X-Forwarded-Client-Cert")
        .and_then(|v| v.to_str().ok())?;

    for part in xfcc.split(';') {
        let part = part.trim();
        if let Some(subject) = part.strip_prefix("Subject=\"") {
            let subject = subject.trim_end_matches('"');
            for field in subject.split(',') {
                if let Some(cn) = field.trim().strip_prefix("CN=") {
                    return Some(cn.to_string());
                }
            }
        }
    }

    None
}

// Silencia warning de import si en algún punto no se usa Arc directamente
#[allow(dead_code)]
fn _project_type_anchor(_: Arc<ResolvedProject>) {}
