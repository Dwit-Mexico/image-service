use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use subtle::ConstantTimeEq;

use crate::{config::AppState, error::AppError};

/// Doble verificación: cert CN (via mTLS forward header) + API key por proyecto.
///
/// El gateway (Istio / Envoy Gateway) termina mTLS y reenvía la cadena
/// `X-Forwarded-Client-Cert` con el CN del certificado cliente.
/// Formato Envoy: `By=...;Hash=...;Subject="CN=project-alpha,..."`
///
/// El header `X-API-Key` debe coincidir exactamente con la key del proyecto.
pub async fn auth_middleware(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    // — 1. Extraer CN del cert cliente —
    let cert_cn = extract_cert_cn(&headers)
        .ok_or_else(|| AppError::Unauthorized("certificado cliente ausente o inválido".into()))?;

    // — 2. Buscar la key esperada para ese proyecto —
    let expected_key = state
        .projects
        .get(&cert_cn.to_lowercase())
        .ok_or_else(|| AppError::Unauthorized(format!("proyecto '{cert_cn}' no registrado")))?;

    // — 3. Verificar API key (timing-safe) —
    let provided_key = headers
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("header X-API-Key ausente".into()))?;

    let valid: bool = expected_key
        .as_bytes()
        .ct_eq(provided_key.as_bytes())
        .into();

    if !valid {
        return Err(AppError::Unauthorized("API key inválida".into()));
    }

    Ok(next.run(request).await)
}

/// Extrae el CN del header `X-Forwarded-Client-Cert` que Envoy/Istio inyecta.
///
/// El valor tiene la forma:
///   `By=spiffe://...;Hash=...;Subject="CN=project-alpha,O=Acme"`
///
/// Fallback: header `X-Client-Cert-CN` para gateways que ya lo parsean.
fn extract_cert_cn(headers: &axum::http::HeaderMap) -> Option<String> {
    // Fallback simple: algunos gateways ya extraen solo el CN
    if let Some(cn) = headers
        .get("X-Client-Cert-CN")
        .and_then(|v| v.to_str().ok())
    {
        return Some(cn.to_string());
    }

    // Parseo de X-Forwarded-Client-Cert (formato Envoy)
    let xfcc = headers
        .get("X-Forwarded-Client-Cert")
        .and_then(|v| v.to_str().ok())?;

    // Busca Subject="CN=<valor>,..."
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
