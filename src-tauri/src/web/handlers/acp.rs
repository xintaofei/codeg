use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::acp::error::AcpError;
use crate::acp::opencode_plugins::PluginCheckSummary;
use crate::acp::preflight::PreflightResult;
use crate::acp::types::{
    AcpAgentInfo, AcpAgentStatus, AgentSkillContent, AgentSkillLayout, AgentSkillScope,
    AgentSkillsListResult, ConnectionInfo, ForkResultInfo,
};
use crate::app_error::{AppCommandError, AppErrorCode};
use crate::app_state::AppState;
use crate::commands::acp as acp_commands;
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
    #[serde(default)]
    pub preferred_mode_id: Option<String>,
    #[serde(default)]
    pub preferred_config_values: Option<BTreeMap<String, String>>,
}

pub async fn acp_connect(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpConnectParams>,
) -> Result<Json<String>, AppCommandError> {
    let db = &state.db;
    let manager = &state.connection_manager;

    let runtime_env = acp_commands::build_session_runtime_env(
        db,
        params.agent_type,
        params.session_id.as_deref(),
        &state.data_dir,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;

    // Guard: the session page must never trigger a download or install.
    // If the agent isn't ready, return SdkNotInstalled here so the frontend
    // can prompt the user to install it from Agent Settings.
    acp_commands::verify_agent_installed(params.agent_type)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;

    let emitter = state.emitter.clone();
    let connection_id = manager
        .spawn_agent(
            params.agent_type,
            params.working_dir,
            params.session_id,
            runtime_env,
            "web".to_string(),
            emitter,
            params.preferred_mode_id,
            params.preferred_config_values.unwrap_or_default(),
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
pub struct AcpTouchConnectionParams {
    pub connection_id: String,
}

pub async fn acp_touch_connection(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpTouchConnectionParams>,
) -> Result<Json<bool>, AppCommandError> {
    let touched = state.connection_manager.touch(&params.connection_id).await;
    Ok(Json(touched))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptParams {
    pub connection_id: String,
    pub blocks: Vec<crate::acp::types::PromptInputBlock>,
    pub folder_id: Option<i32>,
    pub conversation_id: Option<i32>,
    #[serde(default)]
    pub client_message_id: Option<String>,
}

pub async fn acp_prompt(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpPromptParams>,
) -> Result<Json<()>, AppCommandError> {
    state
        .connection_manager
        .send_prompt_linked_with_message_id(
            &state.db,
            &params.connection_id,
            params.blocks,
            params.folder_id,
            params.conversation_id,
            None,
            params.client_message_id,
        )
        .await
        .map_err(|e| {
            let message = e.to_string();
            // A concurrent send while a turn is in flight is an expected,
            // recoverable condition (409), not a server fault (500). The
            // frontend re-queues the draft. Other errors stay 500.
            match e {
                AcpError::TurnInProgress => {
                    AppCommandError::new(AppErrorCode::TurnInProgress, message)
                }
                _ => AppCommandError::task_execution_failed(message),
            }
        })?;
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
    let result = acp_commands::acp_list_agent_skills(params.agent_type, params.workspace_path)
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpDescribeAgentOptionsParams {
    pub agent_type: crate::models::AgentType,
    #[serde(default)]
    pub working_dir: Option<String>,
}

pub async fn acp_describe_agent_options(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpDescribeAgentOptionsParams>,
) -> Result<Json<crate::acp::types::AgentOptionsSnapshot>, AppCommandError> {
    let snapshot = crate::commands::acp::acp_describe_agent_options_core(
        &state.connection_manager,
        &state.db,
        &state.data_dir,
        params.agent_type,
        params.working_dir,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(snapshot))
}

pub async fn acp_cancel(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpConnectionIdParams>,
) -> Result<Json<()>, AppCommandError> {
    let manager = &state.connection_manager;
    manager
        .cancel(&state.db.conn, &params.connection_id)
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
        .fork_session(&state.db, &params.connection_id)
        .await
        .map_err(|e| {
            let message = e.to_string();
            // A fork requested while a turn is in flight is an expected,
            // recoverable condition (409) — the frontend re-queues — not a
            // server fault (500). Mirror `acp_prompt`. Other errors stay 500.
            match e {
                AcpError::TurnInProgress => {
                    AppCommandError::new(AppErrorCode::TurnInProgress, message)
                }
                _ => AppCommandError::task_execution_failed(message),
            }
        })?;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAnswerQuestionParams {
    pub connection_id: String,
    pub question_id: String,
    pub answer: crate::acp::question::QuestionAnswer,
}

pub async fn acp_answer_question(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpAnswerQuestionParams>,
) -> Result<Json<()>, AppCommandError> {
    let manager = &state.connection_manager;
    manager
        .answer_question(&params.connection_id, &params.question_id, params.answer)
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpGetSessionSnapshotParams {
    pub connection_id: String,
}

pub async fn acp_get_session_snapshot(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpGetSessionSnapshotParams>,
) -> Result<Json<Option<crate::acp::LiveSessionSnapshot>>, AppCommandError> {
    let snap = acp_commands::acp_get_session_snapshot_core(
        &state.connection_manager,
        &params.connection_id,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(snap))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpGetSessionSnapshotByConversationParams {
    pub conversation_id: i32,
}

pub async fn acp_get_session_snapshot_by_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpGetSessionSnapshotByConversationParams>,
) -> Result<Json<Option<crate::acp::LiveSessionSnapshot>>, AppCommandError> {
    let snap = acp_commands::acp_get_session_snapshot_by_conversation_core(
        &state.connection_manager,
        params.conversation_id,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(snap))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpFindConnectionForConversationParams {
    pub conversation_id: i32,
    /// Optional session id (`external_id`) fallback, matched (with `agent_type`)
    /// when no live connection is bound to `conversation_id` yet (pre-first-
    /// prompt window).
    #[serde(default)]
    pub session_id: Option<String>,
    pub agent_type: AgentType,
}

pub async fn acp_find_connection_for_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpFindConnectionForConversationParams>,
) -> Result<Json<Option<crate::acp::ConversationConnectionInfo>>, AppCommandError> {
    let info = acp_commands::acp_find_connection_for_conversation_core(
        &state.connection_manager,
        params.conversation_id,
        params.session_id.as_deref(),
        params.agent_type,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(info))
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
) -> Result<Json<usize>, AppCommandError> {
    let db = &state.db;
    let emitter = state.emitter.clone();
    let affected = acp_commands::acp_update_agent_preferences_and_refresh(
        params.agent_type,
        params.enabled,
        params.env,
        params.config_json,
        params.opencode_auth_json,
        params.codex_auth_json,
        params.codex_config_toml,
        db,
        &state.connection_manager,
        &state.data_dir,
        &emitter,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(affected))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUpdateAgentEnvParams {
    pub agent_type: AgentType,
    pub enabled: bool,
    pub env: BTreeMap<String, String>,
    pub model_provider_id: Option<i32>,
}

pub async fn acp_update_agent_env(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpUpdateAgentEnvParams>,
) -> Result<Json<usize>, AppCommandError> {
    let db = &state.db;
    let emitter = state.emitter.clone();
    let affected = acp_commands::acp_update_agent_env_and_refresh(
        params.agent_type,
        params.enabled,
        params.env,
        params.model_provider_id,
        db,
        &state.connection_manager,
        &state.data_dir,
        &emitter,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(affected))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUpdateAgentConfigParams {
    pub agent_type: AgentType,
    pub config_json: Option<String>,
    pub opencode_auth_json: Option<String>,
    pub codex_auth_json: Option<String>,
    pub codex_config_toml: Option<String>,
}

pub async fn acp_update_agent_config(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpUpdateAgentConfigParams>,
) -> Result<Json<usize>, AppCommandError> {
    let emitter = state.emitter.clone();
    let affected = acp_commands::acp_update_agent_config_and_refresh(
        params.agent_type,
        params.config_json,
        params.opencode_auth_json,
        params.codex_auth_json,
        params.codex_config_toml,
        &state.db,
        &state.connection_manager,
        &state.data_dir,
        &emitter,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(affected))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUpdateHermesConfigParams {
    pub provider: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub raw_config_yaml: Option<String>,
}

pub async fn acp_update_hermes_config(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpUpdateHermesConfigParams>,
) -> Result<Json<()>, AppCommandError> {
    let emitter = state.emitter.clone();
    acp_commands::acp_update_hermes_config_core(
        acp_commands::HermesConfigUpdate {
            provider: params.provider,
            api_key: params.api_key,
            model: params.model,
            base_url: params.base_url,
            raw_config_yaml: params.raw_config_yaml,
        },
        &emitter,
    )
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUpdateKimiCodeConfigParams {
    pub mode: String,
    #[serde(default)]
    pub interface_type: Option<String>,
    #[serde(default)]
    pub auth_type: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_context_size: Option<i64>,
    #[serde(default)]
    pub vertex_project: Option<String>,
    #[serde(default)]
    pub vertex_location: Option<String>,
    #[serde(default)]
    pub raw_config_toml: Option<String>,
}

pub async fn acp_update_kimi_code_config(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpUpdateKimiCodeConfigParams>,
) -> Result<Json<usize>, AppCommandError> {
    let emitter = state.emitter.clone();
    let affected = acp_commands::acp_update_kimi_code_config_and_refresh(
        acp_commands::KimiCodeConfigUpdate {
            mode: params.mode,
            interface_type: params.interface_type,
            auth_type: params.auth_type,
            base_url: params.base_url,
            api_key: params.api_key,
            model: params.model,
            max_context_size: params.max_context_size,
            vertex_project: params.vertex_project,
            vertex_location: params.vertex_location,
            raw_config_toml: params.raw_config_toml,
        },
        &state.db,
        &state.connection_manager,
        &state.data_dir,
        &emitter,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(affected))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpFetchKimiModelsParams {
    pub base_url: String,
    pub api_key: String,
}

pub async fn acp_fetch_kimi_models(
    Json(params): Json<AcpFetchKimiModelsParams>,
) -> Result<Json<Vec<String>>, AppCommandError> {
    let models = acp_commands::acp_fetch_kimi_models_core(&params.base_url, &params.api_key)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(models))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUpdatePiConfigParams {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub thinking_level: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub custom_base_url: Option<String>,
    #[serde(default)]
    pub custom_api: Option<String>,
}

pub async fn acp_update_pi_config(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpUpdatePiConfigParams>,
) -> Result<Json<()>, AppCommandError> {
    let emitter = state.emitter.clone();
    acp_commands::acp_update_pi_config_core(
        acp_commands::PiConfigUpdate {
            provider: params.provider,
            model: params.model,
            thinking_level: params.thinking_level,
            api_key: params.api_key,
            custom_base_url: params.custom_base_url,
            custom_api: params.custom_api,
        },
        &state.db,
        &emitter,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn acp_load_pi_config(
) -> Result<Json<acp_commands::PiConfigProjection>, AppCommandError> {
    Ok(Json(acp_commands::load_pi_config_core()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpValidatePiCommandParams {
    pub command: String,
}

pub async fn acp_validate_pi_command(
    Json(params): Json<AcpValidatePiCommandParams>,
) -> Result<Json<acp_commands::PiCommandValidation>, AppCommandError> {
    Ok(Json(acp_commands::acp_validate_pi_command_core(
        params.command,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpDownloadAgentBinaryParams {
    pub agent_type: AgentType,
    #[serde(default)]
    pub version: Option<String>,
    pub task_id: String,
}

pub async fn acp_download_agent_binary(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpDownloadAgentBinaryParams>,
) -> Result<Json<()>, AppCommandError> {
    let emitter = state.emitter.clone();
    acp_commands::acp_download_agent_binary_core(
        params.agent_type,
        params.version,
        params.task_id,
        &emitter,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpInstallUvToolParams {
    pub task_id: String,
}

pub async fn acp_install_uv_tool(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpInstallUvToolParams>,
) -> Result<Json<()>, AppCommandError> {
    let emitter = state.emitter.clone();
    acp_commands::acp_install_uv_tool_core(params.task_id, &emitter)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn acp_detect_agent_local_version(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AgentTypeParams>,
) -> Result<Json<Option<String>>, AppCommandError> {
    let db = &state.db;
    let result = acp_commands::acp_detect_agent_local_version_core(params.agent_type, &db.conn)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPrepareNpxAgentParams {
    pub agent_type: AgentType,
    pub registry_version: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub clean_first: bool,
    pub task_id: String,
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
        params.version,
        params.clean_first,
        params.task_id,
        db,
        &emitter,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUninstallAgentParams {
    pub agent_type: AgentType,
    pub task_id: String,
}

pub async fn acp_uninstall_agent(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpUninstallAgentParams>,
) -> Result<Json<()>, AppCommandError> {
    let db = &state.db;
    let emitter = state.emitter.clone();
    acp_commands::acp_uninstall_agent_core(params.agent_type, params.task_id, db, &emitter)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPiBinaryParams {
    pub task_id: String,
}

pub async fn acp_install_pi_binary(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpPiBinaryParams>,
) -> Result<Json<()>, AppCommandError> {
    let emitter = state.emitter.clone();
    acp_commands::acp_install_pi_binary_core(params.task_id, &emitter)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

pub async fn acp_uninstall_pi_binary(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AcpPiBinaryParams>,
) -> Result<Json<()>, AppCommandError> {
    let emitter = state.emitter.clone();
    acp_commands::acp_uninstall_pi_binary_core(params.task_id, &emitter)
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

pub async fn opencode_list_plugins() -> Result<Json<PluginCheckSummary>, AppCommandError> {
    let result = acp_commands::opencode_list_plugins_core()
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeProviderCatalogParams {
    #[serde(default)]
    pub force_refresh: Option<bool>,
}

pub async fn opencode_provider_catalog(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<OpencodeProviderCatalogParams>,
) -> Result<Json<Vec<crate::acp::opencode_catalog::CatalogProvider>>, AppCommandError> {
    let catalog = acp_commands::opencode_provider_catalog_core(
        &state.data_dir,
        params.force_refresh.unwrap_or(false),
    )
    .await;
    Ok(Json(catalog))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeInstallPluginsParams {
    pub names: Option<Vec<String>>,
    pub task_id: String,
}

pub async fn opencode_install_plugins(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<OpencodeInstallPluginsParams>,
) -> Result<Json<()>, AppCommandError> {
    let emitter = crate::web::event_bridge::EventEmitter::web_only(
        state.event_broadcaster.clone(),
        state.acp_event_bus.clone(),
    );
    acp_commands::opencode_install_plugins_core(params.names, params.task_id, &emitter)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeUninstallPluginParams {
    pub name: String,
}

pub async fn opencode_uninstall_plugin(
    Json(params): Json<OpencodeUninstallPluginParams>,
) -> Result<Json<PluginCheckSummary>, AppCommandError> {
    let result = acp_commands::opencode_uninstall_plugin_core(params.name)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

pub async fn codex_request_device_code(
) -> Result<Json<acp_commands::CodexDeviceCodeResponse>, AppCommandError> {
    let result = acp_commands::codex_request_device_code_core()
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexPollDeviceCodeParams {
    pub device_auth_id: String,
    pub user_code: String,
}

pub async fn codex_poll_device_code(
    Json(params): Json<CodexPollDeviceCodeParams>,
) -> Result<Json<acp_commands::CodexDeviceCodePollResult>, AppCommandError> {
    let result = acp_commands::codex_poll_device_code_core(params.device_auth_id, params.user_code)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(result))
}
