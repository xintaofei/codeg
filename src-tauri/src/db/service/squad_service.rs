use std::collections::BTreeMap;

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, Set,
};

use crate::acp::registry;
use crate::db::entities::{
    squad_artifact, squad_role_profile, squad_role_run, squad_run, squad_task,
};
use crate::db::error::DbError;
use crate::models::agent::AgentType;
use crate::models::squad::{
    SquadArtifactInfo, SquadArtifactType, SquadRoleKind, SquadRoleProfileInfo,
    SquadRoleProfilePatch, SquadRoleRunInfo, SquadRoleRunStatus, SquadRunInfo, SquadRunMode,
    SquadRunSnapshot, SquadRunStatus, SquadTaskInfo, SquadTaskStatus, SquadWorkspacePolicy,
};

fn to_json_string<T: serde::Serialize>(value: &T) -> Result<String, DbError> {
    serde_json::to_string(value).map_err(|e| DbError::Migration(e.to_string()))
}

fn from_json_string<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, DbError> {
    serde_json::from_str(value).map_err(|e| DbError::Migration(e.to_string()))
}

fn parse_or<T: serde::de::DeserializeOwned>(value: &str, fallback: T) -> T {
    serde_json::from_str(value).unwrap_or(fallback)
}

fn serialize_env_json(raw: Option<String>) -> Result<Option<String>, DbError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let env: BTreeMap<String, String> = serde_json::from_str(trimmed)
        .map_err(|e| DbError::Migration(format!("invalid env_json: {e}")))?;
    validate_env_map(&env)?;
    serde_json::to_string(&env)
        .map(Some)
        .map_err(|e| DbError::Migration(e.to_string()))
}

pub fn validate_env_map(env: &BTreeMap<String, String>) -> Result<(), DbError> {
    for (key, value) in env {
        if key.is_empty() || key.len() > 256 {
            return Err(DbError::Migration(
                "env key must be 1-256 characters".into(),
            ));
        }
        if value.len() > 8192 {
            return Err(DbError::Migration(format!("env value too long: {key}")));
        }
        if key.contains('\0') || value.contains('\0') {
            return Err(DbError::Migration(format!("env contains NUL byte: {key}")));
        }
        if key == "PATH"
            || key == "LD_PRELOAD"
            || key == "LD_LIBRARY_PATH"
            || key == "LD_AUDIT"
            || key.starts_with("DYLD_")
            || key.starts_with("CODEG_")
        {
            return Err(DbError::Migration(format!("env key is not allowed: {key}")));
        }
    }
    Ok(())
}

fn role_profile_info(row: squad_role_profile::Model) -> SquadRoleProfileInfo {
    SquadRoleProfileInfo {
        id: row.id,
        folder_id: row.folder_id,
        role_kind: parse_or(&row.role_kind, SquadRoleKind::Worker),
        enabled: row.enabled,
        agent_type: parse_or(&row.agent_type, AgentType::ClaudeCode),
        registry_id: row.registry_id,
        model_provider_id: row.model_provider_id,
        model_id: row.model_id,
        env_json: row.env_json,
        system_prompt: row.system_prompt,
        workspace_policy: parse_or(&row.workspace_policy, SquadWorkspacePolicy::ReadOnly),
        default_run_mode: parse_or(&row.default_run_mode, SquadRunMode::Manual),
        mode_id: row.mode_id,
        config_options_json: row.config_options_json,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    }
}

fn run_info(row: squad_run::Model) -> SquadRunInfo {
    SquadRunInfo {
        id: row.id,
        folder_id: row.folder_id,
        origin_conversation_id: row.origin_conversation_id,
        mode: parse_or(&row.mode, SquadRunMode::Manual),
        status: parse_or(&row.status, SquadRunStatus::Pending),
        goal_summary: row.goal_summary,
        base_branch: row.base_branch,
        isolation_mode: row.isolation_mode,
        started_with_dirty_base: row.started_with_dirty_base,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
        started_at: row.started_at.map(|v| v.to_rfc3339()),
        completed_at: row.completed_at.map(|v| v.to_rfc3339()),
        cancelled_at: row.cancelled_at.map(|v| v.to_rfc3339()),
        error_message: row.error_message,
    }
}

fn role_run_info(row: squad_role_run::Model) -> SquadRoleRunInfo {
    SquadRoleRunInfo {
        id: row.id,
        squad_run_id: row.squad_run_id,
        role_kind: parse_or(&row.role_kind, SquadRoleKind::Worker),
        role_profile_snapshot_json: row.role_profile_snapshot_json,
        connection_id: row.connection_id,
        session_id: row.session_id,
        conversation_id: row.conversation_id,
        workspace_path: row.workspace_path,
        branch_name: row.branch_name,
        status: parse_or(&row.status, SquadRoleRunStatus::Pending),
        last_event_at: row.last_event_at.map(|v| v.to_rfc3339()),
        budget_state_json: row.budget_state_json,
        error_message: row.error_message,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    }
}

fn task_info(row: squad_task::Model) -> SquadTaskInfo {
    SquadTaskInfo {
        id: row.id,
        squad_run_id: row.squad_run_id,
        assigned_role_kind: parse_or(&row.assigned_role_kind, SquadRoleKind::Worker),
        title: row.title,
        description: row.description,
        input_summary: row.input_summary,
        status: parse_or(&row.status, SquadTaskStatus::Pending),
        depends_on_json: row.depends_on_json,
        priority: row.priority,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
        completed_at: row.completed_at.map(|v| v.to_rfc3339()),
        error_message: row.error_message,
    }
}

fn artifact_info(row: squad_artifact::Model) -> SquadArtifactInfo {
    SquadArtifactInfo {
        id: row.id,
        squad_run_id: row.squad_run_id,
        squad_role_run_id: row.squad_role_run_id,
        task_id: row.task_id,
        role_kind: row
            .role_kind
            .as_deref()
            .map(|v| parse_or(v, SquadRoleKind::Worker)),
        artifact_type: parse_or(&row.artifact_type, SquadArtifactType::Log),
        title: row.title,
        content_json: row.content_json,
        created_at: row.created_at.to_rfc3339(),
    }
}

pub async fn seed_role_profiles(
    conn: &DatabaseConnection,
    folder_id: i32,
) -> Result<Vec<SquadRoleProfileInfo>, DbError> {
    for (index, role_kind) in SquadRoleKind::all().into_iter().enumerate() {
        let role_kind_json = to_json_string(&role_kind)?;
        let existing = squad_role_profile::Entity::find()
            .filter(squad_role_profile::Column::FolderId.eq(folder_id))
            .filter(squad_role_profile::Column::RoleKind.eq(role_kind_json.clone()))
            .one(conn)
            .await?;
        if existing.is_some() {
            continue;
        }

        let agent_type = AgentType::ClaudeCode;
        let now = Utc::now();
        let active = squad_role_profile::ActiveModel {
            id: NotSet,
            folder_id: Set(folder_id),
            role_kind: Set(role_kind_json),
            enabled: Set(true),
            agent_type: Set(to_json_string(&agent_type)?),
            registry_id: Set(registry::registry_id_for(agent_type).to_string()),
            model_provider_id: Set(None),
            model_id: Set(None),
            env_json: Set(None),
            system_prompt: Set(role_kind.default_prompt().to_string()),
            workspace_policy: Set(to_json_string(&role_kind.default_workspace_policy())?),
            default_run_mode: Set(to_json_string(&if index == 0 {
                SquadRunMode::ConductorDispatch
            } else {
                SquadRunMode::Manual
            })?),
            mode_id: Set(None),
            config_options_json: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        };
        active.insert(conn).await?;
    }
    list_role_profiles(conn, folder_id).await
}

pub async fn list_role_profiles(
    conn: &DatabaseConnection,
    folder_id: i32,
) -> Result<Vec<SquadRoleProfileInfo>, DbError> {
    let rows = squad_role_profile::Entity::find()
        .filter(squad_role_profile::Column::FolderId.eq(folder_id))
        .order_by_asc(squad_role_profile::Column::Id)
        .all(conn)
        .await?;
    Ok(rows.into_iter().map(role_profile_info).collect())
}

pub async fn update_role_profile(
    conn: &DatabaseConnection,
    folder_id: i32,
    role_kind: SquadRoleKind,
    patch: SquadRoleProfilePatch,
) -> Result<SquadRoleProfileInfo, DbError> {
    let role_kind_json = to_json_string(&role_kind)?;
    let row = squad_role_profile::Entity::find()
        .filter(squad_role_profile::Column::FolderId.eq(folder_id))
        .filter(squad_role_profile::Column::RoleKind.eq(role_kind_json))
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration("squad role profile not found".into()))?;
    let mut active = row.into_active_model();

    if let Some(v) = patch.enabled {
        active.enabled = Set(v);
    }
    if let Some(v) = patch.agent_type {
        active.agent_type = Set(to_json_string(&v)?);
    }
    if let Some(v) = patch.registry_id {
        active.registry_id = Set(v);
    }
    if let Some(v) = patch.model_provider_id {
        active.model_provider_id = Set(Some(v));
    }
    if let Some(v) = patch.model_id {
        active.model_id = Set(if v.trim().is_empty() { None } else { Some(v) });
    }
    if patch.env_json.is_some() {
        active.env_json = Set(serialize_env_json(patch.env_json)?);
    }
    if let Some(v) = patch.system_prompt {
        active.system_prompt = Set(v);
    }
    if let Some(v) = patch.workspace_policy {
        active.workspace_policy = Set(to_json_string(&v)?);
    }
    if let Some(v) = patch.default_run_mode {
        active.default_run_mode = Set(to_json_string(&v)?);
    }
    if let Some(v) = patch.mode_id {
        active.mode_id = Set(if v.trim().is_empty() { None } else { Some(v) });
    }
    if let Some(v) = patch.config_options_json {
        active.config_options_json = Set(if v.trim().is_empty() { None } else { Some(v) });
    }
    active.updated_at = Set(Utc::now());
    Ok(role_profile_info(active.update(conn).await?))
}

pub async fn reset_role_profile(
    conn: &DatabaseConnection,
    folder_id: i32,
    role_kind: SquadRoleKind,
) -> Result<SquadRoleProfileInfo, DbError> {
    let role_kind_json = to_json_string(&role_kind)?;
    if let Some(row) = squad_role_profile::Entity::find()
        .filter(squad_role_profile::Column::FolderId.eq(folder_id))
        .filter(squad_role_profile::Column::RoleKind.eq(role_kind_json))
        .one(conn)
        .await?
    {
        squad_role_profile::Entity::delete_by_id(row.id)
            .exec(conn)
            .await?;
    }
    seed_role_profiles(conn, folder_id).await?;
    let profiles = list_role_profiles(conn, folder_id).await?;
    profiles
        .into_iter()
        .find(|profile| profile.role_kind == role_kind)
        .ok_or_else(|| DbError::Migration("squad role profile reset failed".into()))
}

pub async fn create_run(
    conn: &DatabaseConnection,
    folder_id: i32,
    origin_conversation_id: Option<i32>,
    mode: SquadRunMode,
    goal_summary: String,
) -> Result<SquadRunSnapshot, DbError> {
    let profiles = seed_role_profiles(conn, folder_id).await?;
    let now = Utc::now();
    let run = squad_run::ActiveModel {
        id: NotSet,
        folder_id: Set(folder_id),
        origin_conversation_id: Set(origin_conversation_id),
        mode: Set(to_json_string(&mode)?),
        status: Set(to_json_string(&SquadRunStatus::Pending)?),
        goal_summary: Set(goal_summary),
        base_branch: Set(None),
        isolation_mode: Set("workspace_policy".to_string()),
        started_with_dirty_base: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
        started_at: Set(None),
        completed_at: Set(None),
        cancelled_at: Set(None),
        error_message: Set(None),
    }
    .insert(conn)
    .await?;

    for profile in profiles.into_iter().filter(|profile| profile.enabled) {
        let snapshot =
            serde_json::to_string(&profile).map_err(|e| DbError::Migration(e.to_string()))?;
        squad_role_run::ActiveModel {
            id: NotSet,
            squad_run_id: Set(run.id),
            role_kind: Set(to_json_string(&profile.role_kind)?),
            role_profile_snapshot_json: Set(snapshot),
            connection_id: Set(None),
            session_id: Set(None),
            conversation_id: Set(None),
            workspace_path: Set(None),
            branch_name: Set(None),
            status: Set(to_json_string(&SquadRoleRunStatus::Pending)?),
            last_event_at: Set(None),
            budget_state_json: Set(None),
            error_message: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(conn)
        .await?;
    }
    get_run(conn, run.id).await
}

pub async fn get_run(conn: &DatabaseConnection, run_id: i32) -> Result<SquadRunSnapshot, DbError> {
    let run = squad_run::Entity::find_by_id(run_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("squad run not found: {run_id}")))?;
    let roles = squad_role_run::Entity::find()
        .filter(squad_role_run::Column::SquadRunId.eq(run_id))
        .order_by_asc(squad_role_run::Column::Id)
        .all(conn)
        .await?;
    let tasks = squad_task::Entity::find()
        .filter(squad_task::Column::SquadRunId.eq(run_id))
        .order_by_asc(squad_task::Column::Id)
        .all(conn)
        .await?;
    let artifacts = squad_artifact::Entity::find()
        .filter(squad_artifact::Column::SquadRunId.eq(run_id))
        .order_by_asc(squad_artifact::Column::Id)
        .all(conn)
        .await?;
    Ok(SquadRunSnapshot {
        run: run_info(run),
        roles: roles.into_iter().map(role_run_info).collect(),
        tasks: tasks.into_iter().map(task_info).collect(),
        artifacts: artifacts.into_iter().map(artifact_info).collect(),
    })
}

pub async fn list_runs(
    conn: &DatabaseConnection,
    folder_id: i32,
) -> Result<Vec<SquadRunInfo>, DbError> {
    let rows = squad_run::Entity::find()
        .filter(squad_run::Column::FolderId.eq(folder_id))
        .order_by_desc(squad_run::Column::UpdatedAt)
        .all(conn)
        .await?;
    Ok(rows.into_iter().map(run_info).collect())
}

pub async fn set_run_status(
    conn: &DatabaseConnection,
    run_id: i32,
    status: SquadRunStatus,
    error_message: Option<String>,
) -> Result<SquadRunInfo, DbError> {
    let row = squad_run::Entity::find_by_id(run_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("squad run not found: {run_id}")))?;
    let now = Utc::now();
    let mut active = row.into_active_model();
    active.status = Set(to_json_string(&status)?);
    active.updated_at = Set(now);
    if matches!(status, SquadRunStatus::Running) {
        active.started_at = Set(Some(now));
    }
    if matches!(status, SquadRunStatus::Completed | SquadRunStatus::Failed) {
        active.completed_at = Set(Some(now));
    }
    if matches!(status, SquadRunStatus::Cancelled) {
        active.cancelled_at = Set(Some(now));
    }
    active.error_message = Set(error_message);
    Ok(run_info(active.update(conn).await?))
}

pub async fn list_role_runs(
    conn: &DatabaseConnection,
    run_id: i32,
) -> Result<Vec<SquadRoleRunInfo>, DbError> {
    let rows = squad_role_run::Entity::find()
        .filter(squad_role_run::Column::SquadRunId.eq(run_id))
        .order_by_asc(squad_role_run::Column::Id)
        .all(conn)
        .await?;
    Ok(rows.into_iter().map(role_run_info).collect())
}

pub async fn get_role_run(
    conn: &DatabaseConnection,
    run_id: i32,
    role_kind: SquadRoleKind,
) -> Result<SquadRoleRunInfo, DbError> {
    let role_kind_json = to_json_string(&role_kind)?;
    let row = squad_role_run::Entity::find()
        .filter(squad_role_run::Column::SquadRunId.eq(run_id))
        .filter(squad_role_run::Column::RoleKind.eq(role_kind_json))
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration("squad role run not found".into()))?;
    Ok(role_run_info(row))
}

pub async fn update_role_workspace(
    conn: &DatabaseConnection,
    role_run_id: i32,
    workspace_path: Option<String>,
    branch_name: Option<String>,
) -> Result<SquadRoleRunInfo, DbError> {
    let row = squad_role_run::Entity::find_by_id(role_run_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("squad role run not found: {role_run_id}")))?;
    let mut active = row.into_active_model();
    active.workspace_path = Set(workspace_path);
    active.branch_name = Set(branch_name);
    active.updated_at = Set(Utc::now());
    Ok(role_run_info(active.update(conn).await?))
}

pub async fn update_role_connection(
    conn: &DatabaseConnection,
    role_run_id: i32,
    connection_id: Option<String>,
    session_id: Option<String>,
    status: SquadRoleRunStatus,
    error_message: Option<String>,
) -> Result<SquadRoleRunInfo, DbError> {
    let row = squad_role_run::Entity::find_by_id(role_run_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("squad role run not found: {role_run_id}")))?;
    let now = Utc::now();
    let mut active = row.into_active_model();
    active.connection_id = Set(connection_id);
    active.session_id = Set(session_id);
    active.status = Set(to_json_string(&status)?);
    active.last_event_at = Set(Some(now));
    active.error_message = Set(error_message);
    active.updated_at = Set(now);
    Ok(role_run_info(active.update(conn).await?))
}

pub async fn create_task(
    conn: &DatabaseConnection,
    run_id: i32,
    assigned_role_kind: SquadRoleKind,
    title: String,
    description: String,
) -> Result<SquadTaskInfo, DbError> {
    let now = Utc::now();
    let row = squad_task::ActiveModel {
        id: NotSet,
        squad_run_id: Set(run_id),
        assigned_role_kind: Set(to_json_string(&assigned_role_kind)?),
        title: Set(title),
        description: Set(description),
        input_summary: Set(None),
        status: Set(to_json_string(&SquadTaskStatus::Pending)?),
        depends_on_json: Set(None),
        priority: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        completed_at: Set(None),
        error_message: Set(None),
    }
    .insert(conn)
    .await?;
    Ok(task_info(row))
}

pub async fn list_tasks(
    conn: &DatabaseConnection,
    run_id: i32,
) -> Result<Vec<SquadTaskInfo>, DbError> {
    let rows = squad_task::Entity::find()
        .filter(squad_task::Column::SquadRunId.eq(run_id))
        .order_by_asc(squad_task::Column::Id)
        .all(conn)
        .await?;
    Ok(rows.into_iter().map(task_info).collect())
}

pub async fn get_task_for_run(
    conn: &DatabaseConnection,
    run_id: i32,
    task_id: i32,
) -> Result<SquadTaskInfo, DbError> {
    let row = squad_task::Entity::find()
        .filter(squad_task::Column::Id.eq(task_id))
        .filter(squad_task::Column::SquadRunId.eq(run_id))
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("squad task not found: {task_id}")))?;
    Ok(task_info(row))
}

pub async fn update_task_status(
    conn: &DatabaseConnection,
    task_id: i32,
    status: SquadTaskStatus,
) -> Result<SquadTaskInfo, DbError> {
    let row = squad_task::Entity::find_by_id(task_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("squad task not found: {task_id}")))?;
    let now = Utc::now();
    let mut active = row.into_active_model();
    active.status = Set(to_json_string(&status)?);
    active.updated_at = Set(now);
    if matches!(status, SquadTaskStatus::Completed) {
        active.completed_at = Set(Some(now));
    }
    Ok(task_info(active.update(conn).await?))
}

pub async fn create_artifact(
    conn: &DatabaseConnection,
    run_id: i32,
    role_kind: Option<SquadRoleKind>,
    task_id: Option<i32>,
    artifact_type: SquadArtifactType,
    title: String,
    content_json: String,
) -> Result<SquadArtifactInfo, DbError> {
    let _: serde_json::Value = from_json_string(&content_json)?;
    let row = squad_artifact::ActiveModel {
        id: NotSet,
        squad_run_id: Set(run_id),
        squad_role_run_id: Set(None),
        task_id: Set(task_id),
        role_kind: Set(role_kind.map(|role| to_json_string(&role)).transpose()?),
        artifact_type: Set(to_json_string(&artifact_type)?),
        title: Set(title),
        content_json: Set(content_json),
        created_at: Set(Utc::now()),
    }
    .insert(conn)
    .await?;
    Ok(artifact_info(row))
}

pub async fn list_artifacts(
    conn: &DatabaseConnection,
    run_id: i32,
) -> Result<Vec<SquadArtifactInfo>, DbError> {
    let rows = squad_artifact::Entity::find()
        .filter(squad_artifact::Column::SquadRunId.eq(run_id))
        .order_by_asc(squad_artifact::Column::Id)
        .all(conn)
        .await?;
    Ok(rows.into_iter().map(artifact_info).collect())
}

pub fn role_profile_from_snapshot(snapshot: &str) -> Result<SquadRoleProfileInfo, DbError> {
    from_json_string(snapshot)
}
