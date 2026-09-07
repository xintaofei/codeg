use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::delegation::{
    load_continuation_enabled, set_continuation_enabled_core, ContinuationSettings,
};

/// Web-mode mirror of `get_continuation_settings`.
pub async fn get_continuation_settings(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<ContinuationSettings>, AppCommandError> {
    Ok(Json(ContinuationSettings {
        continuable_delegation_enabled: load_continuation_enabled(&state.db.conn).await,
    }))
}

#[derive(Deserialize)]
pub struct SetContinuationSettingsParams {
    pub settings: ContinuationSettings,
}

/// Web-mode mirror of `set_continuation_settings`: persist + apply to the
/// live coordinator.
pub async fn set_continuation_settings(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SetContinuationSettingsParams>,
) -> Result<Json<ContinuationSettings>, AppCommandError> {
    let enabled = set_continuation_enabled_core(
        &state.db.conn,
        &state.continuation_coordinator,
        params.settings.continuable_delegation_enabled,
    )
    .await?;
    Ok(Json(ContinuationSettings {
        continuable_delegation_enabled: enabled,
    }))
}
