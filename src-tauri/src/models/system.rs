use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemProxySettings {
    pub enabled: bool,
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppLocale {
    #[default]
    En,
    ZhCn,
    ZhTw,
    Ja,
    Ko,
    Es,
    De,
    Fr,
    Pt,
    Ar,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LanguageMode {
    #[default]
    System,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SystemLanguageSettings {
    pub mode: LanguageMode,
    pub language: AppLocale,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SystemOpenTarget {
    Vscode,
    #[default]
    FileManager,
    Terminal,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SystemWebFileOpenMethod {
    #[default]
    Browser,
    Editor,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SystemOpenTargetSettings {
    pub target: SystemOpenTarget,
    pub web_file_open_method: SystemWebFileOpenMethod,
}

#[cfg(feature = "tauri-runtime")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SystemRenderingSettings {
    pub disable_hardware_acceleration: bool,
}

// --- Version Control ---

/// Explicit credentials for a single git remote operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCredentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDetectResult {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GitSettings {
    pub custom_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubAccount {
    pub id: String,
    pub server_url: String,
    pub username: String,
    pub scopes: Vec<String>,
    pub avatar_url: Option<String>,
    pub is_default: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GitHubAccountsSettings {
    pub accounts: Vec<GitHubAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubTokenValidation {
    pub success: bool,
    pub username: Option<String>,
    pub scopes: Vec<String>,
    pub avatar_url: Option<String>,
    pub message: Option<String>,
}
