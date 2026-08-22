use axum::Json;
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::acp::types::AgentSkillScope;
use crate::commands::experts::{ExpertInstallStatus, LinkOp, LinkOpResult};
use crate::commands::science as science_commands;
use crate::commands::science::ScienceListItem;
use crate::models::agent::AgentType;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScienceIdParams {
    pub skill_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScienceAgentParams {
    pub skill_id: String,
    pub agent_type: AgentType,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyLinksParams {
    pub ops: Vec<LinkOp>,
    pub scope: AgentSkillScope,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeParams {
    pub scope: AgentSkillScope,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

pub async fn science_list() -> Result<Json<Vec<ScienceListItem>>, AppCommandError> {
    let result = science_commands::science_list()
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn science_get_install_status(
    Json(params): Json<ScienceIdParams>,
) -> Result<Json<Vec<ExpertInstallStatus>>, AppCommandError> {
    let result = science_commands::science_get_install_status(params.skill_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn science_list_all_install_statuses(
    Json(params): Json<ScopeParams>,
) -> Result<Json<Vec<ExpertInstallStatus>>, AppCommandError> {
    let result = science_commands::science_list_all_install_statuses(
        params.scope,
        params.workspace_path,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn science_link_to_agent(
    Json(params): Json<ScienceAgentParams>,
) -> Result<Json<ExpertInstallStatus>, AppCommandError> {
    let result = science_commands::science_link_to_agent(params.skill_id, params.agent_type)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn science_apply_links(
    Json(params): Json<ApplyLinksParams>,
) -> Result<Json<Vec<LinkOpResult>>, AppCommandError> {
    let result = science_commands::science_apply_links(
        params.ops,
        params.scope,
        params.workspace_path,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn science_unlink_from_agent(
    Json(params): Json<ScienceAgentParams>,
) -> Result<Json<()>, AppCommandError> {
    science_commands::science_unlink_from_agent(params.skill_id, params.agent_type)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn science_read_content(
    Json(params): Json<ScienceIdParams>,
) -> Result<Json<String>, AppCommandError> {
    let result = science_commands::science_read_content(params.skill_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn science_open_central_dir() -> Result<Json<String>, AppCommandError> {
    let result = science_commands::science_open_central_dir()
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}
