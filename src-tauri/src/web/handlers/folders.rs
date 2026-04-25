use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::folders as folder_commands;
use crate::db::service::folder_service;
use crate::models::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderIdParams {
    pub folder_id: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFolderWorkspacePresetParams {
    pub folder_id: i32,
    pub workspace_preset: Option<WorkspacePreset>,
}

pub async fn load_folder_history(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<FolderHistoryEntry>>, AppCommandError> {
    let db = &state.db;
    let result = folder_service::list_folders(&db.conn)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

pub async fn list_open_folders(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<FolderHistoryEntry>>, AppCommandError> {
    let db = &state.db;
    let result = folder_service::list_open_folders(&db.conn)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

pub async fn get_folder(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<FolderIdParams>,
) -> Result<Json<FolderDetail>, AppCommandError> {
    let db = &state.db;
    let folder = folder_service::get_folder_by_id(&db.conn, params.folder_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Folder not found"))?;
    Ok(Json(folder))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddFolderParams {
    pub path: String,
}

/// Add the folder to the workspace (upsert + set is_open=true) and return its full detail.
/// Previously this spawned a new window; the new single-window workspace model
/// simply returns the folder info so the client can update its local state.
pub async fn open_folder(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AddFolderParams>,
) -> Result<Json<FolderDetail>, AppCommandError> {
    let db = &state.db;
    let entry = folder_service::add_folder(&db.conn, &params.path)
        .await
        .map_err(AppCommandError::from)?;
    let folder = folder_service::get_folder_by_id(&db.conn, entry.id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Folder not found after add"))?;
    Ok(Json(folder))
}

// --- New workspace handlers ---

pub async fn list_open_folder_details(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<FolderDetail>>, AppCommandError> {
    let db = &state.db;
    let result = folder_service::list_open_folder_details(&db.conn)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

pub async fn list_all_folder_details(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<FolderDetail>>, AppCommandError> {
    let db = &state.db;
    let result = folder_service::list_all_folder_details(&db.conn)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

pub async fn open_folder_by_id(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<FolderIdParams>,
) -> Result<Json<FolderDetail>, AppCommandError> {
    let db = &state.db;
    folder_service::set_folder_open(&db.conn, params.folder_id, true)
        .await
        .map_err(AppCommandError::from)?;
    let folder = folder_service::get_folder_by_id(&db.conn, params.folder_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Folder not found"))?;
    Ok(Json(folder))
}

pub async fn remove_folder_from_workspace(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<FolderIdParams>,
) -> Result<Json<()>, AppCommandError> {
    use crate::db::service::tab_service;
    let db = &state.db;
    tab_service::delete_tabs_for_folder(&db.conn, params.folder_id)
        .await
        .map_err(AppCommandError::from)?;
    folder_service::set_folder_open(&db.conn, params.folder_id, false)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(()))
}

pub async fn update_folder_workspace_preset(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateFolderWorkspacePresetParams>,
) -> Result<Json<FolderDetail>, AppCommandError> {
    let db = &state.db;
    let result = folder_service::update_workspace_preset(
        &db.conn,
        params.folder_id,
        params.workspace_preset,
    )
    .await
    .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderFoldersParams {
    pub ids: Vec<i32>,
}

pub async fn reorder_folders(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ReorderFoldersParams>,
) -> Result<Json<()>, AppCommandError> {
    let db = &state.db;
    folder_service::reorder_folders(&db.conn, params.ids)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathParams {
    pub path: String,
}

pub async fn get_git_branch(
    Json(params): Json<PathParams>,
) -> Result<Json<Option<String>>, AppCommandError> {
    let result = folder_commands::get_git_branch(params.path).await?;
    Ok(Json(result))
}

pub async fn get_home_directory() -> Result<Json<String>, AppCommandError> {
    let result = folder_commands::get_home_directory().await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDirectoryEntriesParams {
    pub path: String,
}

pub async fn list_directory_entries(
    Json(params): Json<ListDirectoryEntriesParams>,
) -> Result<Json<Vec<folder_commands::DirectoryEntry>>, AppCommandError> {
    let result = folder_commands::list_directory_entries(params.path).await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetFileTreeParams {
    pub path: String,
    pub max_depth: Option<usize>,
}

pub async fn get_file_tree(
    Json(params): Json<GetFileTreeParams>,
) -> Result<Json<Vec<folder_commands::FileTreeNode>>, AppCommandError> {
    let result = folder_commands::get_file_tree(params.path, params.max_depth).await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSettingsWindowParams {
    pub section: Option<String>,
    pub agent_type: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsNavigationResult {
    pub path: String,
}

/// Web equivalent of `open_settings_window`: returns the target navigation path.
/// The web client handles the actual navigation.
pub async fn open_settings_window(
    Json(params): Json<OpenSettingsWindowParams>,
) -> Result<Json<SettingsNavigationResult>, AppCommandError> {
    let route = match params.section.as_deref() {
        Some("appearance") => "settings/appearance",
        Some("agents") => "settings/agents",
        Some("mcp") => "settings/mcp",
        Some("skills") => "settings/skills",
        Some("shortcuts") => "settings/shortcuts",
        Some("system") => "settings/system",
        _ => "settings/appearance",
    };

    let path = if route == "settings/agents" {
        if let Some(ref agent) = params.agent_type {
            let trimmed = agent.trim();
            if !trimmed.is_empty()
                && trimmed
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
            {
                format!("/{route}?agent={trimmed}")
            } else {
                format!("/{route}")
            }
        } else {
            format!("/{route}")
        }
    } else {
        format!("/{route}")
    };

    Ok(Json(SettingsNavigationResult { path }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCommitWindowParams {
    pub folder_id: i32,
}

/// Web equivalent of `open_commit_window`: returns the navigation path.
pub async fn open_commit_window(
    Json(params): Json<OpenCommitWindowParams>,
) -> Result<Json<SettingsNavigationResult>, AppCommandError> {
    Ok(Json(SettingsNavigationResult {
        path: format!("/commit?folderId={}", params.folder_id),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenMergeWindowParams {
    pub folder_id: i32,
    pub operation: Option<String>,
    pub upstream_commit: Option<String>,
}

pub async fn open_merge_window(
    Json(params): Json<OpenMergeWindowParams>,
) -> Result<Json<SettingsNavigationResult>, AppCommandError> {
    let mut path = format!("/merge?folderId={}", params.folder_id);
    if let Some(op) = &params.operation {
        path.push_str(&format!("&operation={op}"));
    }
    if let Some(uc) = &params.upstream_commit {
        path.push_str(&format!("&upstreamCommit={uc}"));
    }
    Ok(Json(SettingsNavigationResult { path }))
}

pub async fn open_stash_window(
    Json(params): Json<OpenCommitWindowParams>,
) -> Result<Json<SettingsNavigationResult>, AppCommandError> {
    Ok(Json(SettingsNavigationResult {
        path: format!("/stash?folderId={}", params.folder_id),
    }))
}

pub async fn open_push_window(
    Json(params): Json<OpenCommitWindowParams>,
) -> Result<Json<SettingsNavigationResult>, AppCommandError> {
    Ok(Json(SettingsNavigationResult {
        path: format!("/push?folderId={}", params.folder_id),
    }))
}

pub async fn add_folder_to_history(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AddFolderParams>,
) -> Result<Json<FolderHistoryEntry>, AppCommandError> {
    let db = &state.db;
    let result = folder_service::add_folder(&db.conn, &params.path)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

pub async fn remove_folder_from_history(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<AddFolderParams>,
) -> Result<Json<()>, AppCommandError> {
    let db = &state.db;
    folder_service::remove_folder(&db.conn, &params.path)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(()))
}

pub async fn create_folder_directory(
    Json(params): Json<AddFolderParams>,
) -> Result<Json<()>, AppCommandError> {
    folder_commands::create_folder_directory(params.path).await?;
    Ok(Json(()))
}
