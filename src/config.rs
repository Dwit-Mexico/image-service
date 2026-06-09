use std::sync::Arc;

use crate::admin::AdminState;
use crate::crypto::Kek;
use crate::projects::ProjectResolver;

/// Estado compartido por todos los handlers.
#[derive(Clone)]
pub struct AppState {
    pub resolver: Arc<ProjectResolver>,
    /// La KEK también vive aquí para que las rutas admin puedan sellar
    /// nuevas credenciales. El resolver tiene su propia copia para descifrar.
    pub kek: Option<Arc<Kek>>,
    /// `None` si `ADMIN_PASSWORD_HASH` no está seteado → `/admin/*` se omite.
    pub admin: Option<AdminState>,
}
