use std::{collections::HashMap, env, sync::Arc};

use crate::storage::StorageProvider;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<dyn StorageProvider>,
    /// project_cn (cert CN) → api_key
    pub projects: Arc<HashMap<String, String>>,
    /// default Azure container cuando el cliente no especifica
    pub default_container: String,
}

/// Carga proyectos desde variables de entorno con el prefijo `PROJECT_`:
///   PROJECT_ALPHA=cn=project-alpha:sk_live_xxxx
///   PROJECT_BETA=cn=project-beta:sk_live_yyyy
///
/// Formato: `<cert_cn>:<api_key>`
pub fn load_projects() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (key, val) in env::vars() {
        if key.starts_with("PROJECT_") {
            if let Some((cn, api_key)) = val.split_once(':') {
                map.insert(cn.trim().to_lowercase(), api_key.trim().to_string());
            } else {
                tracing::warn!("Variable {key} ignorada: formato inválido (esperado cn:api_key)");
            }
        }
    }
    if map.is_empty() {
        tracing::warn!("No se encontraron proyectos configurados (PROJECT_* vars)");
    }
    map
}

pub fn default_container() -> String {
    env::var("DEFAULT_CONTAINER").unwrap_or_else(|_| "images".to_string())
}
