//! Askama templates para `/admin/*`. Compile-time embed, no runtime IO.

use askama::Template;
use chrono::{DateTime, Utc};

#[derive(Template)]
#[template(path = "admin/login.html")]
pub struct LoginTpl {
    pub error: Option<String>,
}

pub struct DashboardRow {
    pub id: String,
    pub name: String,
    pub cert_cn: String,
    pub api_key_prefix: String,
    pub backend: String,
    pub container: String,
    pub last_used: String,
    pub status: &'static str,
}

#[derive(Template)]
#[template(path = "admin/dashboard.html")]
pub struct DashboardTpl {
    pub user: String,
    pub projects: Vec<DashboardRow>,
    pub flash: Option<String>,
}

#[derive(Template)]
#[template(path = "admin/project.html")]
pub struct ProjectTpl {
    pub user: String,
    pub id: String,
    pub name: String,
    pub cert_cn: String,
    pub api_key_prefix: String,
    pub backend: String,
    pub container: String,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
    pub revoked: bool,
    pub csrf_token: String,
    pub flash: Option<String>,
    pub newly_generated_key: Option<String>,
}

#[derive(Template)]
#[template(path = "admin/create.html")]
pub struct CreateTpl {
    pub user: String,
    pub backend: String, // "azure" or "s3"
    pub error: Option<String>,
    pub csrf_token: String,
}

#[derive(Template)]
#[template(path = "admin/created.html")]
pub struct CreatedTpl {
    pub user: String,
    pub cert_cn: String,
    pub plaintext_key: String,
}
