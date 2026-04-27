use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::system_settings as settings_commands;
use crate::db::service::app_metadata_service;
use crate::models::*;
use crate::network::proxy;

const SYSTEM_PROXY_SETTINGS_KEY: &str = "system_proxy_settings";
const SYSTEM_LANGUAGE_SETTINGS_KEY: &str = "system_language_settings";
const LANGUAGE_SETTINGS_UPDATED_EVENT: &str = "app://language-settings-updated";

// Wrapper structs to match Tauri's named parameter convention.
// Frontend sends `{ settings: <T> }` which Tauri `invoke()` unwraps automatically,
// but in web mode the entire JSON body arrives as-is.

#[derive(Deserialize)]
pub struct UpdateProxySettingsParams {
    pub settings: SystemProxySettings,
}

#[derive(Deserialize)]
pub struct UpdateLanguageSettingsParams {
    pub settings: SystemLanguageSettings,
}

// ---------------------------------------------------------------------------
// Read handlers
// ---------------------------------------------------------------------------

pub async fn list_system_font_families() -> Result<Json<SystemFontFamilyList>, AppCommandError> {
    Ok(Json(settings_commands::fallback_system_font_families()))
}

pub async fn get_system_proxy_settings(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<SystemProxySettings>, AppCommandError> {
    let db = &state.db;
    let settings = settings_commands::load_system_proxy_settings(&db.conn).await?;
    Ok(Json(settings))
}

pub async fn get_system_language_settings(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<SystemLanguageSettings>, AppCommandError> {
    let db = &state.db;
    let settings = settings_commands::load_system_language_settings(&db.conn).await?;
    Ok(Json(settings))
}

// ---------------------------------------------------------------------------
// Update handlers
// ---------------------------------------------------------------------------

pub async fn update_system_proxy_settings(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateProxySettingsParams>,
) -> Result<Json<SystemProxySettings>, AppCommandError> {
    let settings = params.settings;
    let db = &state.db;

    // TODO: call normalize_proxy_settings once it is made pub(crate) in
    // commands/system_settings.rs. For now the frontend validates the URL.
    let serialized = serde_json::to_string(&settings).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize proxy settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(&db.conn, SYSTEM_PROXY_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    proxy::apply_system_proxy_settings(&settings)?;
    Ok(Json(settings))
}

pub async fn update_system_language_settings(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateLanguageSettingsParams>,
) -> Result<Json<SystemLanguageSettings>, AppCommandError> {
    let settings = params.settings;
    let db = &state.db;

    let serialized = serde_json::to_string(&settings).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize language settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(&db.conn, SYSTEM_LANGUAGE_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    crate::web::event_bridge::emit_event(
        &state.emitter,
        LANGUAGE_SETTINGS_UPDATED_EVENT,
        settings.clone(),
    );

    Ok(Json(settings))
}
