use std::sync::Arc;

use axum::{Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::acp::types::AgentSkillScope;
use crate::app_state::AppState;
use crate::commands::experts::{ExpertInstallStatus, LinkOp, LinkOpResult};
use crate::commands::office_tools as ot;
use crate::commands::office_tools::{OfficecliInfo, OfficecliSkill, SkillSyncReport};
use crate::models::agent::AgentType;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillIdParams {
    pub skill_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillAgentParams {
    pub skill_id: String,
    pub agent_type: AgentType,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderHtmlParams {
    pub root_path: String,
    pub path: String,
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

pub async fn officecli_detect() -> Result<Json<OfficecliInfo>, AppCommandError> {
    let result = ot::officecli_detect().await;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficecliInstallParams {
    pub task_id: String,
}

pub async fn officecli_install(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<OfficecliInstallParams>,
) -> Result<Json<OfficecliInfo>, AppCommandError> {
    let emitter = state.emitter.clone();
    let result = ot::officecli_install_core(params.task_id, &emitter)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn officecli_uninstall() -> Result<Json<OfficecliInfo>, AppCommandError> {
    let result = ot::officecli_uninstall()
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn officecli_list_skills() -> Result<Json<Vec<OfficecliSkill>>, AppCommandError> {
    let result = ot::officecli_list_skills().await;
    Ok(Json(result))
}

pub async fn officecli_sync_skills() -> Result<Json<SkillSyncReport>, AppCommandError> {
    let result = ot::officecli_sync_skills()
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn officecli_skill_link_to_agent(
    Json(params): Json<SkillAgentParams>,
) -> Result<Json<ExpertInstallStatus>, AppCommandError> {
    let result = ot::officecli_skill_link_to_agent(params.skill_id, params.agent_type)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn officecli_skill_unlink_from_agent(
    Json(params): Json<SkillAgentParams>,
) -> Result<Json<()>, AppCommandError> {
    ot::officecli_skill_unlink_from_agent(params.skill_id, params.agent_type)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn officecli_skill_get_install_status(
    Json(params): Json<SkillIdParams>,
) -> Result<Json<Vec<ExpertInstallStatus>>, AppCommandError> {
    let result = ot::officecli_skill_get_install_status(params.skill_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn officecli_skill_list_all_install_statuses(
    Json(params): Json<ScopeParams>,
) -> Result<Json<Vec<ExpertInstallStatus>>, AppCommandError> {
    let result = ot::officecli_skill_list_all_install_statuses(
        params.scope,
        params.workspace_path,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn officecli_skill_apply_links(
    Json(params): Json<ApplyLinksParams>,
) -> Result<Json<Vec<LinkOpResult>>, AppCommandError> {
    let result = ot::officecli_skill_apply_links(
        params.ops,
        params.scope,
        params.workspace_path,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn officecli_skill_read_content(
    Json(params): Json<SkillIdParams>,
) -> Result<Json<String>, AppCommandError> {
    let result = ot::officecli_skill_read_content(params.skill_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn officecli_render_html(
    Json(params): Json<RenderHtmlParams>,
) -> Result<Json<String>, AppCommandError> {
    let result = ot::officecli_render_html(params.root_path, params.path)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeWatchParams {
    pub root_path: String,
    pub path: String,
}

pub async fn start_office_watch(
    Json(params): Json<OfficeWatchParams>,
) -> Result<Json<crate::office_watch::OfficeWatchStarted>, AppCommandError> {
    // `?` converts WatchError → AppCommandError, carrying the machine code.
    let result =
        crate::office_watch::start_office_watch_core(params.root_path, params.path).await?;
    Ok(Json(result))
}

pub async fn stop_office_watch(
    Json(params): Json<OfficeWatchParams>,
) -> Result<Json<()>, AppCommandError> {
    crate::office_watch::stop_office_watch_core(params.root_path, params.path).await?;
    Ok(Json(()))
}
