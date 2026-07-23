use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::web::tailscale::{
    set_funnel_enabled_core, OpenTailscaleLoginResult, TailscaleFunnelStatus,
};

pub async fn get_status(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<TailscaleFunnelStatus>, AppCommandError> {
    Ok(Json(state.tailscale.status().await))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetEnabledParams {
    pub enabled: bool,
}

pub async fn set_enabled(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SetEnabledParams>,
) -> Result<Json<TailscaleFunnelStatus>, AppCommandError> {
    set_funnel_enabled_core(state, params.enabled)
        .await
        .map(Json)
}

pub async fn open_login(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<OpenTailscaleLoginResult>, AppCommandError> {
    let login_url = state.tailscale.open_login_hint().await?;
    Ok(Json(OpenTailscaleLoginResult { login_url }))
}
