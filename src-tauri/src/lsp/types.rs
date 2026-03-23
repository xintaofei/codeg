use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct LspServerInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub language: String,
    pub distribution_type: String,
    pub registry_version: Option<String>,
    pub enabled: bool,
    pub sort_order: i32,
    pub installed_version: Option<String>,
    pub config_json: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LspServerStatus {
    pub id: String,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub update_available: bool,
}
