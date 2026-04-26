use crate::acp::manager::ConnectionManager;
use crate::app_error::AppCommandError;
use crate::db::service::squad_service;
use crate::db::AppDatabase;
use crate::models::squad::{
    SquadArtifactInfo, SquadArtifactType, SquadRoleKind, SquadRoleProfileInfo,
    SquadRoleProfilePatch, SquadRoleRunInfo, SquadRunInfo, SquadRunMode, SquadRunSnapshot,
    SquadTaskInfo, SquadTaskStatus,
};
use crate::squad::dispatcher;
use crate::web::event_bridge::EventEmitter;

pub async fn squad_get_role_profiles_core(
    db: &AppDatabase,
    folder_id: i32,
) -> Result<Vec<SquadRoleProfileInfo>, AppCommandError> {
    squad_service::list_role_profiles(&db.conn, folder_id)
        .await
        .map_err(AppCommandError::from)
}

pub async fn squad_seed_role_profiles_core(
    db: &AppDatabase,
    folder_id: i32,
) -> Result<Vec<SquadRoleProfileInfo>, AppCommandError> {
    squad_service::seed_role_profiles(&db.conn, folder_id)
        .await
        .map_err(AppCommandError::from)
}

pub async fn squad_update_role_profile_core(
    db: &AppDatabase,
    folder_id: i32,
    role_kind: SquadRoleKind,
    patch: SquadRoleProfilePatch,
) -> Result<SquadRoleProfileInfo, AppCommandError> {
    squad_service::update_role_profile(&db.conn, folder_id, role_kind, patch)
        .await
        .map_err(AppCommandError::from)
}

pub async fn squad_reset_role_profile_core(
    db: &AppDatabase,
    folder_id: i32,
    role_kind: SquadRoleKind,
) -> Result<SquadRoleProfileInfo, AppCommandError> {
    squad_service::reset_role_profile(&db.conn, folder_id, role_kind)
        .await
        .map_err(AppCommandError::from)
}

pub async fn squad_create_run_core(
    db: &AppDatabase,
    folder_id: i32,
    origin_conversation_id: Option<i32>,
    mode: SquadRunMode,
    goal_summary: String,
) -> Result<SquadRunSnapshot, AppCommandError> {
    let snapshot = squad_service::create_run(
        &db.conn,
        folder_id,
        origin_conversation_id,
        mode,
        goal_summary,
    )
    .await
    .map_err(AppCommandError::from)?;
    Ok(snapshot)
}

pub async fn squad_get_run_core(
    db: &AppDatabase,
    squad_run_id: i32,
) -> Result<SquadRunSnapshot, AppCommandError> {
    squad_service::get_run(&db.conn, squad_run_id)
        .await
        .map_err(AppCommandError::from)
}

pub async fn squad_list_runs_core(
    db: &AppDatabase,
    folder_id: i32,
) -> Result<Vec<SquadRunInfo>, AppCommandError> {
    squad_service::list_runs(&db.conn, folder_id)
        .await
        .map_err(AppCommandError::from)
}

pub async fn squad_start_run_core(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    squad_run_id: i32,
    working_dir: Option<String>,
) -> Result<(), AppCommandError> {
    dispatcher::start_run(db, manager, emitter, squad_run_id, working_dir)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}

pub async fn squad_stop_run_core(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    squad_run_id: i32,
) -> Result<(), AppCommandError> {
    dispatcher::stop_run(db, manager, emitter, squad_run_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}

pub async fn squad_connect_role_core(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    squad_run_id: i32,
    role_kind: SquadRoleKind,
    working_dir: Option<String>,
) -> Result<SquadRoleRunInfo, AppCommandError> {
    dispatcher::connect_role(db, manager, emitter, squad_run_id, role_kind, working_dir)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}

pub async fn squad_prompt_role_core(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    squad_run_id: i32,
    role_kind: SquadRoleKind,
    task_id: Option<i32>,
) -> Result<SquadRoleRunInfo, AppCommandError> {
    let task = match task_id {
        Some(task_id) => Some(
            squad_service::get_task_for_run(&db.conn, squad_run_id, task_id)
                .await
                .map_err(AppCommandError::from)?,
        ),
        None => None,
    };
    dispatcher::prompt_role(db, manager, emitter, squad_run_id, role_kind, task)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}

pub async fn squad_create_task_core(
    db: &AppDatabase,
    squad_run_id: i32,
    assigned_role_kind: SquadRoleKind,
    title: String,
    description: String,
) -> Result<SquadTaskInfo, AppCommandError> {
    squad_service::create_task(
        &db.conn,
        squad_run_id,
        assigned_role_kind,
        title,
        description,
    )
    .await
    .map_err(AppCommandError::from)
}

pub async fn squad_update_task_status_core(
    db: &AppDatabase,
    task_id: i32,
    status: SquadTaskStatus,
) -> Result<SquadTaskInfo, AppCommandError> {
    squad_service::update_task_status(&db.conn, task_id, status)
        .await
        .map_err(AppCommandError::from)
}

pub async fn squad_list_tasks_core(
    db: &AppDatabase,
    squad_run_id: i32,
) -> Result<Vec<SquadTaskInfo>, AppCommandError> {
    squad_service::list_tasks(&db.conn, squad_run_id)
        .await
        .map_err(AppCommandError::from)
}

#[allow(clippy::too_many_arguments)]
pub async fn squad_create_artifact_core(
    db: &AppDatabase,
    emitter: &EventEmitter,
    squad_run_id: i32,
    role_kind: Option<SquadRoleKind>,
    task_id: Option<i32>,
    artifact_type: SquadArtifactType,
    title: String,
    content_json: String,
) -> Result<SquadArtifactInfo, AppCommandError> {
    dispatcher::record_role_artifact(
        db,
        emitter,
        squad_run_id,
        role_kind,
        task_id,
        artifact_type,
        title,
        content_json,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}

pub async fn squad_list_artifacts_core(
    db: &AppDatabase,
    squad_run_id: i32,
) -> Result<Vec<SquadArtifactInfo>, AppCommandError> {
    squad_service::list_artifacts(&db.conn, squad_run_id)
        .await
        .map_err(AppCommandError::from)
}

pub async fn squad_apply_conductor_output_core(
    db: &AppDatabase,
    emitter: &EventEmitter,
    squad_run_id: i32,
    raw_text: String,
) -> Result<dispatcher::ApplyConductorOutputResult, AppCommandError> {
    dispatcher::apply_conductor_output(db, emitter, squad_run_id, &raw_text)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}

pub async fn squad_dispatch_pending_tasks_core(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    squad_run_id: i32,
) -> Result<dispatcher::DispatchPendingTasksResult, AppCommandError> {
    dispatcher::dispatch_pending_tasks(db, manager, emitter, squad_run_id)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}

pub async fn squad_record_turn_artifacts_core(
    db: &AppDatabase,
    emitter: &EventEmitter,
    squad_run_id: i32,
    role_kind: SquadRoleKind,
    task_id: Option<i32>,
    agent_text: String,
    plan_json: Option<String>,
) -> Result<dispatcher::TurnArtifactsResult, AppCommandError> {
    dispatcher::record_turn_artifacts(
        db,
        emitter,
        squad_run_id,
        role_kind,
        task_id,
        agent_text,
        plan_json,
    )
    .await
    .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_get_role_profiles(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<Vec<SquadRoleProfileInfo>, AppCommandError> {
    squad_get_role_profiles_core(&db, folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_seed_role_profiles(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<Vec<SquadRoleProfileInfo>, AppCommandError> {
    squad_seed_role_profiles_core(&db, folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_update_role_profile(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    role_kind: SquadRoleKind,
    patch: SquadRoleProfilePatch,
) -> Result<SquadRoleProfileInfo, AppCommandError> {
    squad_update_role_profile_core(&db, folder_id, role_kind, patch).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_reset_role_profile(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    role_kind: SquadRoleKind,
) -> Result<SquadRoleProfileInfo, AppCommandError> {
    squad_reset_role_profile_core(&db, folder_id, role_kind).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_create_run(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    origin_conversation_id: Option<i32>,
    mode: SquadRunMode,
    goal_summary: String,
) -> Result<SquadRunSnapshot, AppCommandError> {
    squad_create_run_core(&db, folder_id, origin_conversation_id, mode, goal_summary).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_get_run(
    db: tauri::State<'_, AppDatabase>,
    squad_run_id: i32,
) -> Result<SquadRunSnapshot, AppCommandError> {
    squad_get_run_core(&db, squad_run_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_list_runs(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<Vec<SquadRunInfo>, AppCommandError> {
    squad_list_runs_core(&db, folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_start_run(
    db: tauri::State<'_, AppDatabase>,
    manager: tauri::State<'_, ConnectionManager>,
    app: tauri::AppHandle,
    squad_run_id: i32,
    working_dir: Option<String>,
) -> Result<(), AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    squad_start_run_core(&db, &manager, &emitter, squad_run_id, working_dir).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_stop_run(
    db: tauri::State<'_, AppDatabase>,
    manager: tauri::State<'_, ConnectionManager>,
    app: tauri::AppHandle,
    squad_run_id: i32,
) -> Result<(), AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    squad_stop_run_core(&db, &manager, &emitter, squad_run_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_connect_role(
    db: tauri::State<'_, AppDatabase>,
    manager: tauri::State<'_, ConnectionManager>,
    app: tauri::AppHandle,
    squad_run_id: i32,
    role_kind: SquadRoleKind,
    working_dir: Option<String>,
) -> Result<SquadRoleRunInfo, AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    squad_connect_role_core(
        &db,
        &manager,
        &emitter,
        squad_run_id,
        role_kind,
        working_dir,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_prompt_role(
    db: tauri::State<'_, AppDatabase>,
    manager: tauri::State<'_, ConnectionManager>,
    app: tauri::AppHandle,
    squad_run_id: i32,
    role_kind: SquadRoleKind,
    task_id: Option<i32>,
) -> Result<SquadRoleRunInfo, AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    squad_prompt_role_core(&db, &manager, &emitter, squad_run_id, role_kind, task_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_create_task(
    db: tauri::State<'_, AppDatabase>,
    squad_run_id: i32,
    assigned_role_kind: SquadRoleKind,
    title: String,
    description: String,
) -> Result<SquadTaskInfo, AppCommandError> {
    squad_create_task_core(&db, squad_run_id, assigned_role_kind, title, description).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_update_task_status(
    db: tauri::State<'_, AppDatabase>,
    task_id: i32,
    status: SquadTaskStatus,
) -> Result<SquadTaskInfo, AppCommandError> {
    squad_update_task_status_core(&db, task_id, status).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_list_tasks(
    db: tauri::State<'_, AppDatabase>,
    squad_run_id: i32,
) -> Result<Vec<SquadTaskInfo>, AppCommandError> {
    squad_list_tasks_core(&db, squad_run_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command surface mirrors the underlying _core fn.
pub async fn squad_create_artifact(
    db: tauri::State<'_, AppDatabase>,
    app: tauri::AppHandle,
    squad_run_id: i32,
    role_kind: Option<SquadRoleKind>,
    task_id: Option<i32>,
    artifact_type: SquadArtifactType,
    title: String,
    content_json: String,
) -> Result<SquadArtifactInfo, AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    squad_create_artifact_core(
        &db,
        &emitter,
        squad_run_id,
        role_kind,
        task_id,
        artifact_type,
        title,
        content_json,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_list_artifacts(
    db: tauri::State<'_, AppDatabase>,
    squad_run_id: i32,
) -> Result<Vec<SquadArtifactInfo>, AppCommandError> {
    squad_list_artifacts_core(&db, squad_run_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_apply_conductor_output(
    db: tauri::State<'_, AppDatabase>,
    app: tauri::AppHandle,
    squad_run_id: i32,
    raw_text: String,
) -> Result<dispatcher::ApplyConductorOutputResult, AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    squad_apply_conductor_output_core(&db, &emitter, squad_run_id, raw_text).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_dispatch_pending_tasks(
    db: tauri::State<'_, AppDatabase>,
    manager: tauri::State<'_, ConnectionManager>,
    app: tauri::AppHandle,
    squad_run_id: i32,
) -> Result<dispatcher::DispatchPendingTasksResult, AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    squad_dispatch_pending_tasks_core(&db, &manager, &emitter, squad_run_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn squad_record_turn_artifacts(
    db: tauri::State<'_, AppDatabase>,
    app: tauri::AppHandle,
    squad_run_id: i32,
    role_kind: SquadRoleKind,
    task_id: Option<i32>,
    agent_text: String,
    plan_json: Option<String>,
) -> Result<dispatcher::TurnArtifactsResult, AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    squad_record_turn_artifacts_core(
        &db,
        &emitter,
        squad_run_id,
        role_kind,
        task_id,
        agent_text,
        plan_json,
    )
    .await
}
