use axum::Json;
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::acp::types::AgentSkillScope;
use crate::commands::experts as experts_commands;
use crate::commands::experts::{ExpertInstallStatus, ExpertListItem, LinkOp, LinkOpResult};
use crate::models::agent::AgentType;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertIdParams {
    pub expert_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertAgentParams {
    pub expert_id: String,
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

pub async fn experts_list() -> Result<Json<Vec<ExpertListItem>>, AppCommandError> {
    let result = experts_commands::experts_list()
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn experts_get_install_status(
    Json(params): Json<ExpertIdParams>,
) -> Result<Json<Vec<ExpertInstallStatus>>, AppCommandError> {
    let result = experts_commands::experts_get_install_status(params.expert_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn experts_list_all_install_statuses(
    Json(params): Json<ScopeParams>,
) -> Result<Json<Vec<ExpertInstallStatus>>, AppCommandError> {
    let result = experts_commands::experts_list_all_install_statuses(
        params.scope,
        params.workspace_path,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn experts_link_to_agent(
    Json(params): Json<ExpertAgentParams>,
) -> Result<Json<ExpertInstallStatus>, AppCommandError> {
    let result = experts_commands::experts_link_to_agent(params.expert_id, params.agent_type)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn experts_apply_links(
    Json(params): Json<ApplyLinksParams>,
) -> Result<Json<Vec<LinkOpResult>>, AppCommandError> {
    let result = experts_commands::experts_apply_links(
        params.ops,
        params.scope,
        params.workspace_path,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn experts_unlink_from_agent(
    Json(params): Json<ExpertAgentParams>,
) -> Result<Json<()>, AppCommandError> {
    experts_commands::experts_unlink_from_agent(params.expert_id, params.agent_type)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn experts_read_content(
    Json(params): Json<ExpertIdParams>,
) -> Result<Json<String>, AppCommandError> {
    let result = experts_commands::experts_read_content(params.expert_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn experts_open_central_dir() -> Result<Json<String>, AppCommandError> {
    let result = experts_commands::experts_open_central_dir()
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}
