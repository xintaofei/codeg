use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::squad as squad_commands;
use crate::models::squad::{
    SquadArtifactInfo, SquadArtifactType, SquadRoleKind, SquadRoleProfileInfo,
    SquadRoleProfilePatch, SquadRoleRunInfo, SquadRunInfo, SquadRunMode, SquadRunSnapshot,
    SquadTaskInfo, SquadTaskStatus,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderIdParams {
    pub folder_id: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleProfileParams {
    pub folder_id: i32,
    pub role_kind: SquadRoleKind,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRoleProfileParams {
    pub folder_id: i32,
    pub role_kind: SquadRoleKind,
    pub patch: SquadRoleProfilePatch,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunParams {
    pub folder_id: i32,
    pub origin_conversation_id: Option<i32>,
    pub mode: SquadRunMode,
    pub goal_summary: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunIdParams {
    pub squad_run_id: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunParams {
    pub squad_run_id: i32,
    pub working_dir: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleRuntimeParams {
    pub squad_run_id: i32,
    pub role_kind: SquadRoleKind,
    pub working_dir: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRoleParams {
    pub squad_run_id: i32,
    pub role_kind: SquadRoleKind,
    pub task_id: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskParams {
    pub squad_run_id: i32,
    pub assigned_role_kind: SquadRoleKind,
    pub title: String,
    pub description: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTaskStatusParams {
    pub task_id: i32,
    pub status: SquadTaskStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateArtifactParams {
    pub squad_run_id: i32,
    pub role_kind: Option<SquadRoleKind>,
    pub task_id: Option<i32>,
    pub artifact_type: SquadArtifactType,
    pub title: String,
    pub content_json: String,
}

pub async fn squad_get_role_profiles(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<FolderIdParams>,
) -> Result<Json<Vec<SquadRoleProfileInfo>>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_get_role_profiles_core(&state.db, params.folder_id).await?,
    ))
}

pub async fn squad_seed_role_profiles(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<FolderIdParams>,
) -> Result<Json<Vec<SquadRoleProfileInfo>>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_seed_role_profiles_core(&state.db, params.folder_id).await?,
    ))
}

pub async fn squad_update_role_profile(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateRoleProfileParams>,
) -> Result<Json<SquadRoleProfileInfo>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_update_role_profile_core(
            &state.db,
            params.folder_id,
            params.role_kind,
            params.patch,
        )
        .await?,
    ))
}

pub async fn squad_reset_role_profile(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<RoleProfileParams>,
) -> Result<Json<SquadRoleProfileInfo>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_reset_role_profile_core(
            &state.db,
            params.folder_id,
            params.role_kind,
        )
        .await?,
    ))
}

pub async fn squad_create_run(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateRunParams>,
) -> Result<Json<SquadRunSnapshot>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_create_run_core(
            &state.db,
            params.folder_id,
            params.origin_conversation_id,
            params.mode,
            params.goal_summary,
        )
        .await?,
    ))
}

pub async fn squad_get_run(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<RunIdParams>,
) -> Result<Json<SquadRunSnapshot>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_get_run_core(&state.db, params.squad_run_id).await?,
    ))
}

pub async fn squad_list_runs(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<FolderIdParams>,
) -> Result<Json<Vec<SquadRunInfo>>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_list_runs_core(&state.db, params.folder_id).await?,
    ))
}

pub async fn squad_start_run(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<StartRunParams>,
) -> Result<Json<()>, AppCommandError> {
    squad_commands::squad_start_run_core(
        &state.db,
        &state.connection_manager,
        &state.emitter,
        params.squad_run_id,
        params.working_dir,
    )
    .await?;
    Ok(Json(()))
}

pub async fn squad_stop_run(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<RunIdParams>,
) -> Result<Json<()>, AppCommandError> {
    squad_commands::squad_stop_run_core(
        &state.db,
        &state.connection_manager,
        &state.emitter,
        params.squad_run_id,
    )
    .await?;
    Ok(Json(()))
}

pub async fn squad_connect_role(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<RoleRuntimeParams>,
) -> Result<Json<SquadRoleRunInfo>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_connect_role_core(
            &state.db,
            &state.connection_manager,
            &state.emitter,
            params.squad_run_id,
            params.role_kind,
            params.working_dir,
        )
        .await?,
    ))
}

pub async fn squad_prompt_role(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<PromptRoleParams>,
) -> Result<Json<SquadRoleRunInfo>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_prompt_role_core(
            &state.db,
            &state.connection_manager,
            &state.emitter,
            params.squad_run_id,
            params.role_kind,
            params.task_id,
        )
        .await?,
    ))
}

pub async fn squad_create_task(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateTaskParams>,
) -> Result<Json<SquadTaskInfo>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_create_task_core(
            &state.db,
            params.squad_run_id,
            params.assigned_role_kind,
            params.title,
            params.description,
        )
        .await?,
    ))
}

pub async fn squad_update_task_status(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateTaskStatusParams>,
) -> Result<Json<SquadTaskInfo>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_update_task_status_core(&state.db, params.task_id, params.status)
            .await?,
    ))
}

pub async fn squad_list_tasks(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<RunIdParams>,
) -> Result<Json<Vec<SquadTaskInfo>>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_list_tasks_core(&state.db, params.squad_run_id).await?,
    ))
}

pub async fn squad_create_artifact(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateArtifactParams>,
) -> Result<Json<SquadArtifactInfo>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_create_artifact_core(
            &state.db,
            params.squad_run_id,
            params.role_kind,
            params.task_id,
            params.artifact_type,
            params.title,
            params.content_json,
        )
        .await?,
    ))
}

pub async fn squad_list_artifacts(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<RunIdParams>,
) -> Result<Json<Vec<SquadArtifactInfo>>, AppCommandError> {
    Ok(Json(
        squad_commands::squad_list_artifacts_core(&state.db, params.squad_run_id).await?,
    ))
}
