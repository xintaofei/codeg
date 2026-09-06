//! HTTP handlers for the content-translation middleware — the web-mode mirror
//! of the Tauri commands in `commands::translation`. Both call the same
//! `_core` functions, so masking, validation, and cache behaviour cannot drift
//! between transports.

use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::translation::{
    translation_cache_stats_core, translation_clear_cache_core, translation_get_settings_core,
    translation_list_models_core, translation_pool_status_core, translation_test_core,
    translation_translate_core, translation_update_settings_core,
};
use crate::translation::pool::ProviderStatus;
use crate::translation::settings::TranslationSettings;
use crate::translation::{TranslationCacheStats, TranslationResult};

pub async fn translation_get_settings(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<TranslationSettings>, AppCommandError> {
    Ok(Json(translation_get_settings_core(&state.db.conn).await?))
}

#[derive(Deserialize)]
pub struct UpdateSettingsParams {
    pub settings: TranslationSettings,
}

pub async fn translation_update_settings(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateSettingsParams>,
) -> Result<Json<TranslationSettings>, AppCommandError> {
    Ok(Json(
        translation_update_settings_core(&state.db.conn, params.settings).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestParams {
    pub settings: TranslationSettings,
    #[serde(default = "default_locale")]
    pub ui_locale: String,
    /// Aims the test at one pool row (the settings-page row being edited).
    #[serde(default)]
    pub provider_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateParams {
    pub texts: Vec<String>,
    #[serde(default = "default_locale")]
    pub ui_locale: String,
    #[serde(default)]
    pub priority: bool,
    #[serde(default)]
    pub target_lang: Option<String>,
}

fn default_locale() -> String {
    "en".to_string()
}

pub async fn translation_test(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<TestParams>,
) -> Result<Json<String>, AppCommandError> {
    Ok(Json(
        translation_test_core(&state.db.conn, params.settings, &params.ui_locale, params.provider_id)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListModelsParams {
    pub settings: TranslationSettings,
    /// Aims the probe at one pool row (the settings-page row being edited).
    #[serde(default)]
    pub provider_id: Option<String>,
}

pub async fn translation_list_models(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ListModelsParams>,
) -> Result<Json<Vec<String>>, AppCommandError> {
    Ok(Json(
        translation_list_models_core(&state.db.conn, params.settings, params.provider_id).await?,
    ))
}

pub async fn translation_pool_status(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<Vec<ProviderStatus>> {
    Json(translation_pool_status_core(&state.db.conn).await)
}

pub async fn translation_metrics() -> Json<crate::translation::metrics::TranslationMetricsSnapshot>
{
    Json(crate::commands::translation::translation_metrics_core())
}

pub async fn translation_translate(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<TranslateParams>,
) -> Result<Json<Vec<TranslationResult>>, AppCommandError> {
    let priority = if params.priority {
        crate::translation::client::Priority::Priority
    } else {
        crate::translation::client::Priority::Background
    };
    Ok(Json(
        translation_translate_core(
            &state.db.conn,
            params.texts,
            &params.ui_locale,
            priority,
            params.target_lang,
        )
        .await?,
    ))
}

pub async fn translation_cache_stats() -> Json<TranslationCacheStats> {
    Json(translation_cache_stats_core())
}

pub async fn translation_clear_cache() -> Json<TranslationCacheStats> {
    Json(translation_clear_cache_core())
}
