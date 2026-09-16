use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::handoff::{
    handoff_core, handoff_plan_core, HandoffPlan, HandoffRequest, HandoffResult,
};
use crate::models::AgentType;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpHandoffPlanParams {
    pub conversation_id: i32,
    pub target_agent_type: AgentType,
}

pub async fn acp_handoff_plan(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpHandoffPlanParams>,
) -> Result<Json<HandoffPlan>, AppCommandError> {
    Ok(Json(
        handoff_plan_core(
            &state.db,
            &state.data_dir,
            params.conversation_id,
            params.target_agent_type,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpHandoffParams {
    pub conversation_id: i32,
    pub target_agent_type: AgentType,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub preferred_mode_id: Option<String>,
    #[serde(default)]
    pub preferred_config_values: Option<BTreeMap<String, String>>,
}

pub async fn acp_handoff(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpHandoffParams>,
) -> Result<Json<HandoffResult>, AppCommandError> {
    Ok(Json(
        handoff_core(
            &state.db,
            &state.connection_manager,
            &state.emitter,
            &state.data_dir,
            "web".to_string(),
            HandoffRequest {
                conversation_id: params.conversation_id,
                target_agent_type: params.target_agent_type,
                note: params.note,
                preferred_mode_id: params.preferred_mode_id,
                preferred_config_values: params.preferred_config_values.unwrap_or_default(),
            },
        )
        .await?,
    ))
}
