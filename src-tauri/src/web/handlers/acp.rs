use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::acp::preflight::PreflightResult;
use crate::acp::registry;
use crate::acp::types::{
    AcpAgentInfo, AcpAgentStatus, AgentSkillContent, AgentSkillLayout, AgentSkillScope,
    AgentSkillsListResult, ConnectionInfo, ForkResultInfo,
};
use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::acp as acp_commands;
use crate::db::service::agent_setting_service;
use crate::models::agent::AgentType;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTypeParams {
    pub agent_type: AgentType,
}

pub async fn acp_get_agent_status(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AgentTypeParams>,
) -> Result<Json<AcpAgentStatus>, AppCommandError> {
    let db = &state.db;
    let result = acp_commands::acp_get_agent_status_core(params.agent_type, db)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn acp_list_agents(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<AcpAgentInfo>>, AppCommandError> {
    let db = &state.db;
    let result = acp_commands::acp_list_agents_core(db)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConnectParams {
    pub agent_type: AgentType,
    pub working_dir: Option<String>,
    pub session_id: Option<String>,
}

pub async fn acp_connect(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpConnectParams>,
) -> Result<Json<String>, AppCommandError> {
    let db = &state.db;
    let manager = &state.connection_manager;
    let meta = registry::get_agent_meta(params.agent_type);

    let setting = agent_setting_service::get_by_agent_type(&db.conn, params.agent_type)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    let disabled = setting
        .as_ref()
        .map(|model| !model.enabled)
        .unwrap_or(false);
    if disabled {
        return Err(AppCommandError::task_execution_failed(format!(
            "{} is disabled in settings",
            params.agent_type
        )));
    }

    let local_config_json = acp_commands::load_agent_local_config_json(params.agent_type);
    let mut runtime_env = acp_commands::build_runtime_env_from_setting(
        params.agent_type,
        setting.as_ref(),
        local_config_json.as_deref(),
    );

    if params.agent_type == AgentType::OpenClaw && params.session_id.is_none() {
        runtime_env.insert("OPENCLAW_RESET_SESSION".into(), "1".into());
    }

    if let registry::AgentDistribution::Npx { cmd, .. } = meta.distribution {
        if !acp_commands::is_cmd_available(cmd) {
            return Err(AppCommandError::task_execution_failed(format!(
                "{} SDK is not installed. Please install it in Agent Settings.",
                meta.name
            )));
        }
    }

    let emitter = state.emitter.clone();
    let connection_id = manager
        .spawn_agent(
            params.agent_type,
            params.working_dir,
            params.session_id,
            runtime_env,
            "web".to_string(),
            emitter,
        )
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;

    Ok(Json(connection_id))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpDisconnectParams {
    pub connection_id: String,
}

pub async fn acp_disconnect(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpDisconnectParams>,
) -> Result<Json<()>, AppCommandError> {
    let manager = &state.connection_manager;
    manager
        .disconnect(&params.connection_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptParams {
    pub connection_id: String,
    pub blocks: Vec<crate::acp::types::PromptInputBlock>,
}

pub async fn acp_prompt(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpPromptParams>,
) -> Result<Json<()>, AppCommandError> {
    let manager = &state.connection_manager;
    manager
        .send_prompt(&params.connection_id, params.blocks)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

// --- Pattern A: Pure function handlers ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPreflightParams {
    pub agent_type: AgentType,
    pub force_refresh: Option<bool>,
}

pub async fn acp_preflight(
    Json(params): Json<AcpPreflightParams>,
) -> Result<Json<PreflightResult>, AppCommandError> {
    let result = acp_commands::acp_preflight(params.agent_type, params.force_refresh)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn acp_clear_binary_cache(
    Json(params): Json<AgentTypeParams>,
) -> Result<Json<()>, AppCommandError> {
    acp_commands::acp_clear_binary_cache(params.agent_type)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpListAgentSkillsParams {
    pub agent_type: AgentType,
    pub workspace_path: Option<String>,
}

pub async fn acp_list_agent_skills(
    Json(params): Json<AcpListAgentSkillsParams>,
) -> Result<Json<AgentSkillsListResult>, AppCommandError> {
    let result =
        acp_commands::acp_list_agent_skills(params.agent_type, params.workspace_path)
            .await
            .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpReadAgentSkillParams {
    pub agent_type: AgentType,
    pub scope: AgentSkillScope,
    pub skill_id: String,
    pub workspace_path: Option<String>,
}

pub async fn acp_read_agent_skill(
    Json(params): Json<AcpReadAgentSkillParams>,
) -> Result<Json<AgentSkillContent>, AppCommandError> {
    let result = acp_commands::acp_read_agent_skill(
        params.agent_type,
        params.scope,
        params.skill_id,
        params.workspace_path,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSaveAgentSkillParams {
    pub agent_type: AgentType,
    pub scope: AgentSkillScope,
    pub skill_id: String,
    pub content: String,
    pub workspace_path: Option<String>,
    pub layout: Option<AgentSkillLayout>,
}

pub async fn acp_save_agent_skill(
    Json(params): Json<AcpSaveAgentSkillParams>,
) -> Result<Json<()>, AppCommandError> {
    acp_commands::acp_save_agent_skill(
        params.agent_type,
        params.scope,
        params.skill_id,
        params.content,
        params.workspace_path,
        params.layout,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpDeleteAgentSkillParams {
    pub agent_type: AgentType,
    pub scope: AgentSkillScope,
    pub skill_id: String,
    pub workspace_path: Option<String>,
}

pub async fn acp_delete_agent_skill(
    Json(params): Json<AcpDeleteAgentSkillParams>,
) -> Result<Json<()>, AppCommandError> {
    acp_commands::acp_delete_agent_skill(
        params.agent_type,
        params.scope,
        params.skill_id,
        params.workspace_path,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

// --- Pattern C: ConnectionManager handlers ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConnectionIdParams {
    pub connection_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSetModeParams {
    pub connection_id: String,
    pub mode_id: String,
}

pub async fn acp_set_mode(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpSetModeParams>,
) -> Result<Json<()>, AppCommandError> {
    let manager = &state.connection_manager;
    manager
        .set_mode(&params.connection_id, params.mode_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSetConfigOptionParams {
    pub connection_id: String,
    pub config_id: String,
    pub value_id: String,
}

pub async fn acp_set_config_option(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpSetConfigOptionParams>,
) -> Result<Json<()>, AppCommandError> {
    let manager = &state.connection_manager;
    manager
        .set_config_option(&params.connection_id, params.config_id, params.value_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn acp_cancel(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpConnectionIdParams>,
) -> Result<Json<()>, AppCommandError> {
    let manager = &state.connection_manager;
    manager
        .cancel(&params.connection_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn acp_fork(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpConnectionIdParams>,
) -> Result<Json<ForkResultInfo>, AppCommandError> {
    let manager = &state.connection_manager;
    let result = manager
        .fork_session(&params.connection_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRespondPermissionParams {
    pub connection_id: String,
    pub request_id: String,
    pub option_id: String,
}

pub async fn acp_respond_permission(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpRespondPermissionParams>,
) -> Result<Json<()>, AppCommandError> {
    let manager = &state.connection_manager;
    manager
        .respond_permission(&params.connection_id, &params.request_id, &params.option_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn acp_list_connections(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<ConnectionInfo>>, AppCommandError> {
    let manager = &state.connection_manager;
    let result = manager.list_connections().await;
    Ok(Json(result))
}

// --- Pattern B+: Core function handlers ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUpdateAgentPreferencesParams {
    pub agent_type: AgentType,
    pub enabled: bool,
    pub env: BTreeMap<String, String>,
    pub config_json: Option<String>,
    pub opencode_auth_json: Option<String>,
    pub codex_auth_json: Option<String>,
    pub codex_config_toml: Option<String>,
}

pub async fn acp_update_agent_preferences(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpUpdateAgentPreferencesParams>,
) -> Result<Json<()>, AppCommandError> {
    let db = &state.db;
    let emitter = state.emitter.clone();
    acp_commands::acp_update_agent_preferences_core(
        params.agent_type,
        params.enabled,
        params.env,
        params.config_json,
        params.opencode_auth_json,
        params.codex_auth_json,
        params.codex_config_toml,
        db,
        &emitter,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn acp_download_agent_binary(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AgentTypeParams>,
) -> Result<Json<()>, AppCommandError> {
    let emitter = state.emitter.clone();
    acp_commands::acp_download_agent_binary_core(params.agent_type, &emitter)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn acp_detect_agent_local_version(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AgentTypeParams>,
) -> Result<Json<Option<String>>, AppCommandError> {
    let db = &state.db;
    let result =
        acp_commands::acp_detect_agent_local_version_core(params.agent_type, &db.conn)
            .await
            .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPrepareNpxAgentParams {
    pub agent_type: AgentType,
    pub registry_version: Option<String>,
}

pub async fn acp_prepare_npx_agent(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpPrepareNpxAgentParams>,
) -> Result<Json<String>, AppCommandError> {
    let db = &state.db;
    let emitter = state.emitter.clone();
    let result = acp_commands::acp_prepare_npx_agent_core(
        params.agent_type,
        params.registry_version,
        db,
        &emitter,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn acp_uninstall_agent(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AgentTypeParams>,
) -> Result<Json<()>, AppCommandError> {
    let db = &state.db;
    let emitter = state.emitter.clone();
    acp_commands::acp_uninstall_agent_core(params.agent_type, db, &emitter)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpReorderAgentsParams {
    pub agent_types: Vec<AgentType>,
}

pub async fn acp_reorder_agents(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpReorderAgentsParams>,
) -> Result<Json<()>, AppCommandError> {
    let db = &state.db;
    let emitter = state.emitter.clone();
    acp_commands::acp_reorder_agents_core(&params.agent_types, db, &emitter)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}
