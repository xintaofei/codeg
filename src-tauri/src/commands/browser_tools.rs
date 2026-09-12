//! The `browser_tools.enabled` setting — whether an agent may see the built-in
//! browser at all.
//!
//! Separate from `commands::browser`, which is the browser itself and exists
//! only in the desktop build: this switch is read by the shared codeg-mcp
//! plumbing (injection, the service-status popover), so it has to compile in
//! server mode too — where it is simply always answered "no" at the point of
//! use, there being no native tabs there.
//!
//! **Off by default**, unlike the other two read-only tool groups. Those hand
//! an agent codeg's own state; this one hands it a listing of the sites the
//! user has open right now, which is the sort of thing that should be a
//! decision rather than a default. Sharing an individual page is a second,
//! per-tab decision on top of it (`crate::browser::agent`) — this switch only
//! decides whether the tools exist.

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::acp::browser_tools::{BrowserToolsConfig, BrowserToolsRuntimeConfig};
use crate::app_error::AppCommandError;
use crate::db::service::app_metadata_service;
use crate::web::event_bridge::{emit_event, EventEmitter, BROWSER_TOOLS_SETTINGS_CHANGED_EVENT};

pub const KEY_BROWSER_TOOLS_ENABLED: &str = "browser_tools.enabled";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserToolsSettings {
    pub enabled: bool,
}

impl BrowserToolsSettings {
    fn into_runtime_config(self) -> BrowserToolsConfig {
        BrowserToolsConfig {
            enabled: self.enabled,
        }
    }
}

/// Read the persisted key from `app_metadata`, falling back to the default
/// (off) for a missing or malformed value. Never errors hard.
pub async fn load_browser_tools_settings(conn: &DatabaseConnection) -> BrowserToolsSettings {
    let mut settings = BrowserToolsSettings::default();
    if let Ok(Some(raw)) = app_metadata_service::get_value(conn, KEY_BROWSER_TOOLS_ENABLED).await {
        if let Ok(v) = raw.parse::<bool>() {
            settings.enabled = v;
        }
    }
    settings
}

/// Pull settings from the DB and push the resulting [`BrowserToolsConfig`] onto
/// the shared runtime handle. Idempotent — safe on startup or after any save.
pub async fn apply_persisted_browser_tools_config(
    conn: &DatabaseConnection,
    config: &BrowserToolsRuntimeConfig,
) {
    let settings = load_browser_tools_settings(conn).await;
    config.set(settings.into_runtime_config()).await;
}

/// Persist + apply + broadcast. Shared by the Tauri command and the HTTP
/// handler so the write + re-apply + notify chain lives in one place.
///
/// The apply is what makes turning this off take effect on sessions that are
/// already running: the access impl reads the same handle on every call.
pub async fn set_browser_tools_settings_core(
    conn: &DatabaseConnection,
    config: &BrowserToolsRuntimeConfig,
    emitter: &EventEmitter,
    desired: BrowserToolsSettings,
) -> Result<BrowserToolsSettings, AppCommandError> {
    app_metadata_service::upsert_value(
        conn,
        KEY_BROWSER_TOOLS_ENABLED,
        &desired.enabled.to_string(),
    )
    .await
    .map_err(AppCommandError::from)?;
    config.set(desired.clone().into_runtime_config()).await;
    emit_event(emitter, BROWSER_TOOLS_SETTINGS_CHANGED_EVENT, &desired);
    Ok(desired)
}

// -------- Tauri commands -----------------------------------------------------

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_browser_tools_settings(
    #[cfg(feature = "tauri-runtime")] db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<BrowserToolsSettings, AppCommandError> {
    #[cfg(feature = "tauri-runtime")]
    {
        Ok(load_browser_tools_settings(&db.conn).await)
    }
    #[cfg(not(feature = "tauri-runtime"))]
    {
        Err(AppCommandError::configuration_invalid("tauri-only command"))
    }
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn set_browser_tools_settings(
    #[cfg(feature = "tauri-runtime")] app: tauri::AppHandle,
    #[cfg(feature = "tauri-runtime")] db: tauri::State<'_, crate::db::AppDatabase>,
    #[cfg(feature = "tauri-runtime")] config: tauri::State<'_, BrowserToolsRuntimeConfig>,
    settings: BrowserToolsSettings,
) -> Result<BrowserToolsSettings, AppCommandError> {
    #[cfg(feature = "tauri-runtime")]
    {
        let emitter = EventEmitter::Tauri(app);
        set_browser_tools_settings_core(&db.conn, &config, &emitter, settings).await
    }
    #[cfg(not(feature = "tauri-runtime"))]
    {
        let _ = settings;
        Err(AppCommandError::configuration_invalid("tauri-only command"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default is the one thing about this setting worth pinning: a user
    /// who never opens the switch has not handed anyone a list of the sites
    /// they have open.
    #[test]
    fn agents_cannot_see_the_browser_until_someone_says_so() {
        assert!(!BrowserToolsSettings::default().enabled);
    }
}
