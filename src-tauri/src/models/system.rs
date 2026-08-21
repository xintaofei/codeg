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

/// "Launch at login". The OS registration itself is the source of truth
/// (registry Run value / LaunchAgent plist / XDG autostart entry), so there is
/// no mirrored copy in the database — the toggle always reflects what the
/// system would actually do, including changes made outside the app (e.g.
/// Windows Task Manager's Startup tab).
///
/// Known limitation, inherited from `auto-launch` 0.5: on macOS and Linux
/// `is_enabled()` only asks whether the file exists. macOS Ventura's Login
/// Items and GNOME both disable an entry *in place*, leaving the file behind,
/// so after one of those the toggle reads on while login will not start the
/// app. Turning it off and on again in this UI rewrites the entry and restores
/// agreement. Windows does not have the gap — its `is_enabled()` also consults
/// the StartupApproved key that Task Manager writes.
#[cfg(feature = "tauri-runtime")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SystemAutostartSettings {
    pub enabled: bool,
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
    /// Which forge this account signs in to: `"github"` | `"gitlab"`, or
    /// absent. Absent is what every account stored before GitLab support
    /// existed looks like, and it keeps meaning what it always meant — a
    /// credential for this HOST, whichever forge lives there. Set, it also
    /// says which API the token is for, which is the only reliable signal for
    /// a self-hosted instance whose hostname gives nothing away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
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
