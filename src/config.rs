use std::sync::Arc;

use crate::projects::ProjectResolver;

/// Estado compartido por todos los handlers. El resolver dueña la pool de
/// Postgres, la KEK y el cache; los handlers solo lo consultan.
#[derive(Clone)]
pub struct AppState {
    pub resolver: Arc<ProjectResolver>,
}
