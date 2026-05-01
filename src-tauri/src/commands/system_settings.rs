#[cfg(feature = "tauri-runtime")]
use std::{collections::BTreeMap, sync::OnceLock};

use sea_orm::DatabaseConnection;
#[cfg(feature = "tauri-runtime")]
use tauri::State;

use crate::app_error::AppCommandError;
use crate::db::service::app_metadata_service;
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;
use crate::models::{
    SystemFontFamily, SystemFontFamilyList, SystemFontFamilySource, SystemFontSettings,
    SystemLanguageSettings, SystemProxySettings,
};
#[cfg(feature = "tauri-runtime")]
use crate::models::SystemRenderingSettings;
#[cfg(feature = "tauri-runtime")]
use crate::network::proxy;
#[cfg(feature = "tauri-runtime")]
use crate::preferences;

const SYSTEM_PROXY_SETTINGS_KEY: &str = "system_proxy_settings";
const SYSTEM_LANGUAGE_SETTINGS_KEY: &str = "system_language_settings";
const APPEARANCE_FONT_SETTINGS_KEY: &str = "appearance_font_settings";
#[cfg(feature = "tauri-runtime")]
const LANGUAGE_SETTINGS_UPDATED_EVENT: &str = "app://language-settings-updated";
const MAX_FONT_FAMILY_LENGTH: usize = 128;
#[cfg(feature = "tauri-runtime")]
const MAX_FONT_FAMILIES: usize = 512;
const FALLBACK_FONT_FAMILIES: [(&str, bool); 10] = [
    ("system-ui", false),
    ("ui-sans-serif", false),
    ("Arial", false),
    ("Helvetica", false),
    ("sans-serif", false),
    ("ui-monospace", true),
    ("Menlo", true),
    ("Monaco", true),
    ("Courier New", true),
    ("monospace", true),
];

#[cfg(feature = "tauri-runtime")]
static SYSTEM_FONT_FAMILY_CACHE: OnceLock<SystemFontFamilyList> = OnceLock::new();

#[cfg(feature = "tauri-runtime")]
fn sanitize_font_family_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('.')
        || trimmed.chars().count() > MAX_FONT_FAMILY_LENGTH
        || trimmed.chars().any(char::is_control)
    {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(feature = "tauri-runtime")]
fn insert_font_family(
    families: &mut BTreeMap<String, SystemFontFamily>,
    family: String,
    monospace: bool,
) {
    let key = family.to_lowercase();
    families
        .entry(key)
        .and_modify(|existing| {
            existing.monospace = existing.monospace || monospace;
        })
        .or_insert(SystemFontFamily { family, monospace });
}

pub(crate) fn fallback_system_font_families() -> SystemFontFamilyList {
    let families = FALLBACK_FONT_FAMILIES
        .iter()
        .map(|(family, monospace)| SystemFontFamily {
            family: (*family).to_string(),
            monospace: *monospace,
        })
        .collect();

    SystemFontFamilyList {
        families,
        source: SystemFontFamilySource::Fallback,
    }
}

#[cfg(feature = "tauri-runtime")]
pub(crate) fn list_system_font_families_core() -> SystemFontFamilyList {
    SYSTEM_FONT_FAMILY_CACHE
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();

            let mut families = BTreeMap::new();
            for face in db.faces() {
                for (family, _language) in &face.families {
                    if let Some(safe_family) = sanitize_font_family_name(family) {
                        insert_font_family(&mut families, safe_family, face.monospaced);
                    }
                }
            }

            let families = families
                .into_values()
                .take(MAX_FONT_FAMILIES)
                .collect::<Vec<_>>();

            if families.is_empty() {
                fallback_system_font_families()
            } else {
                SystemFontFamilyList {
                    families,
                    source: SystemFontFamilySource::System,
                }
            }
        })
        .clone()
}

fn normalize_proxy_settings(
    settings: SystemProxySettings,
) -> Result<SystemProxySettings, AppCommandError> {
    if !settings.enabled {
        let proxy_url = settings
            .proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        return Ok(SystemProxySettings {
            enabled: false,
            proxy_url,
        });
    }

    let proxy_url = settings
        .proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppCommandError::configuration_missing("Proxy URL is required when proxy is enabled")
        })?;

    reqwest::Proxy::all(proxy_url).map_err(|e| {
        AppCommandError::configuration_invalid("Invalid proxy URL").with_detail(e.to_string())
    })?;

    Ok(SystemProxySettings {
        enabled: true,
        proxy_url: Some(proxy_url.to_string()),
    })
}

fn normalize_font_family_preference(value: Option<String>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('.')
        || trimmed.chars().count() > MAX_FONT_FAMILY_LENGTH
        || trimmed.chars().any(char::is_control)
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn normalize_font_settings(settings: SystemFontSettings) -> SystemFontSettings {
    SystemFontSettings {
        ui_font_family: normalize_font_family_preference(settings.ui_font_family),
        code_font_family: normalize_font_family_preference(settings.code_font_family),
    }
}

pub(crate) async fn load_system_proxy_settings(
    conn: &DatabaseConnection,
) -> Result<SystemProxySettings, AppCommandError> {
    let raw = app_metadata_service::get_value(conn, SYSTEM_PROXY_SETTINGS_KEY)
        .await
        .map_err(AppCommandError::from)?;

    let Some(raw) = raw else {
        return Ok(SystemProxySettings::default());
    };

    let parsed = serde_json::from_str::<SystemProxySettings>(&raw).map_err(|e| {
        AppCommandError::configuration_invalid("Failed to parse stored proxy settings")
            .with_detail(e.to_string())
    })?;
    normalize_proxy_settings(parsed)
}

pub(crate) async fn load_system_language_settings(
    conn: &DatabaseConnection,
) -> Result<SystemLanguageSettings, AppCommandError> {
    let raw = app_metadata_service::get_value(conn, SYSTEM_LANGUAGE_SETTINGS_KEY)
        .await
        .map_err(AppCommandError::from)?;

    let Some(raw) = raw else {
        return Ok(SystemLanguageSettings::default());
    };

    serde_json::from_str::<SystemLanguageSettings>(&raw).map_err(|e| {
        AppCommandError::configuration_invalid("Failed to parse stored language settings")
            .with_detail(e.to_string())
    })
}

pub(crate) async fn load_system_font_settings(
    conn: &DatabaseConnection,
) -> Result<SystemFontSettings, AppCommandError> {
    let raw = app_metadata_service::get_value(conn, APPEARANCE_FONT_SETTINGS_KEY)
        .await
        .map_err(AppCommandError::from)?;

    let Some(raw) = raw else {
        return Ok(SystemFontSettings::default());
    };

    let parsed = serde_json::from_str::<SystemFontSettings>(&raw).map_err(|e| {
        AppCommandError::configuration_invalid("Failed to parse stored font settings")
            .with_detail(e.to_string())
    })?;
    Ok(normalize_font_settings(parsed))
}

pub(crate) async fn update_system_font_settings_core(
    conn: &DatabaseConnection,
    settings: SystemFontSettings,
) -> Result<SystemFontSettings, AppCommandError> {
    let normalized = normalize_font_settings(settings);
    let serialized = serde_json::to_string(&normalized).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize font settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(conn, APPEARANCE_FONT_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    Ok(normalized)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_system_font_families() -> Result<SystemFontFamilyList, AppCommandError> {
    Ok(list_system_font_families_core())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_proxy_settings(
    db: State<'_, AppDatabase>,
) -> Result<SystemProxySettings, AppCommandError> {
    load_system_proxy_settings(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_proxy_settings(
    settings: SystemProxySettings,
    db: State<'_, AppDatabase>,
) -> Result<SystemProxySettings, AppCommandError> {
    let normalized = normalize_proxy_settings(settings)?;
    let serialized = serde_json::to_string(&normalized).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize proxy settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(&db.conn, SYSTEM_PROXY_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    proxy::apply_system_proxy_settings(&normalized)?;
    Ok(normalized)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_language_settings(
    db: State<'_, AppDatabase>,
) -> Result<SystemLanguageSettings, AppCommandError> {
    load_system_language_settings(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_font_settings(
    db: State<'_, AppDatabase>,
) -> Result<SystemFontSettings, AppCommandError> {
    load_system_font_settings(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_font_settings(
    settings: SystemFontSettings,
    db: State<'_, AppDatabase>,
) -> Result<SystemFontSettings, AppCommandError> {
    update_system_font_settings_core(&db.conn, settings).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_language_settings(
    settings: SystemLanguageSettings,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<SystemLanguageSettings, AppCommandError> {
    let serialized = serde_json::to_string(&settings).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize language settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(&db.conn, SYSTEM_LANGUAGE_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    let emitter = crate::web::event_bridge::EventEmitter::Tauri(app);
    crate::web::event_bridge::emit_event(
        &emitter,
        LANGUAGE_SETTINGS_UPDATED_EVENT,
        settings.clone(),
    );

    Ok(settings)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_rendering_settings() -> Result<SystemRenderingSettings, AppCommandError> {
    let prefs = preferences::load();
    Ok(SystemRenderingSettings {
        disable_hardware_acceleration: prefs.disable_hardware_acceleration,
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_rendering_settings(
    settings: SystemRenderingSettings,
) -> Result<SystemRenderingSettings, AppCommandError> {
    let mut prefs = preferences::load();
    prefs.disable_hardware_acceleration = settings.disable_hardware_acceleration;
    preferences::save(&prefs).map_err(|err| {
        AppCommandError::io_error("Failed to persist rendering settings")
            .with_detail(err.to_string())
    })?;
    Ok(settings)
}
