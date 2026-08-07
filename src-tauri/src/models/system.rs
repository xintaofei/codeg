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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SystemTerminalSettings {
    pub default_shell: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CloseAction {
    /// Hide to tray when a tray is available; otherwise exit.
    #[default]
    HideToTray,
    /// Always exit when the main window is closed.
    Exit,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct SystemCloseSettings {
    pub action: CloseAction,
}

/// One row in the "default shell" picker. Backend owns the option list so the
/// frontend doesn't have to know which shells are available on which platform.
/// Labels are not localized server-side: `label_key` points at a frontend i18n
/// key under `GeneralSettings.*`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalShellOption {
    /// Stable identifier the dropdown uses as its <option value>.
    pub id: String,
    /// i18n key resolved by the frontend (`GeneralSettings.<label_key>`).
    pub label_key: String,
    /// Concrete value persisted into `SystemTerminalSettings.default_shell`.
    /// `None` for `system` (use `resolve_shell()`) and `custom` (user supplies path).
    pub value: Option<String>,
    /// Whether this shell is currently resolvable on the host. `false` lets
    /// the UI mark the option as "not installed" without preventing selection.
    pub exists: bool,
    /// True for the `custom` row — the UI should render a path input next to
    /// the dropdown when this option is selected.
    pub accepts_custom_path: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableTerminalShells {
    pub options: Vec<TerminalShellOption>,
    /// What `resolve_shell()` would currently fall back to. Surfaced read-only
    /// in the UI so users can see what "system default" actually maps to.
    pub resolved_shell: String,
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
