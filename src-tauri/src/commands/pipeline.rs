use std::collections::BTreeMap;

use chrono::Utc;

use crate::app_error::AppCommandError;
use crate::db::error::DbError;
use crate::db::service::pipeline_service;
use crate::db::AppDatabase;
use crate::models::{PipelineDraft, PipelineInfo, PipelineRunInfo, PipelineRunRequest};
use crate::pipeline::presets;
use crate::web::event_bridge::{emit_event, EventEmitter, PipelineChange, PIPELINE_CHANGED_EVENT};

fn map_db(error: DbError) -> AppCommandError {
    match error {
        DbError::NotFound(message) => AppCommandError::not_found(message),
        DbError::Validation(message) => AppCommandError::invalid_input(message),
        DbError::Conflict(message) => AppCommandError::already_exists(message),
        other => AppCommandError::db(other),
    }
}

fn engine_unavailable() -> AppCommandError {
    AppCommandError::dependency_missing("pipeline engine not running")
        .with_i18n("Pipeline.engineUnavailable", BTreeMap::new())
}

pub async fn pipeline_list_core(
    db: &AppDatabase,
    folder_id: Option<i32>,
) -> Result<Vec<PipelineInfo>, AppCommandError> {
    pipeline_service::list(&db.conn, folder_id)
        .await
        .map_err(map_db)
}

pub async fn pipeline_get_core(db: &AppDatabase, id: i32) -> Result<PipelineInfo, AppCommandError> {
    pipeline_service::get(&db.conn, id).await.map_err(map_db)
}

pub async fn pipeline_save_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    id: Option<i32>,
    draft: PipelineDraft,
) -> Result<PipelineInfo, AppCommandError> {
    let info = pipeline_service::save(&db.conn, id, draft)
        .await
        .map_err(|error| match error {
            DbError::Validation(message) => AppCommandError::configuration_invalid(message),
            other => map_db(other),
        })?;
    emit_event(
        emitter,
        PIPELINE_CHANGED_EVENT,
        PipelineChange::Upsert { id: info.id },
    );
    Ok(info)
}

pub async fn pipeline_delete_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    id: i32,
) -> Result<(), AppCommandError> {
    pipeline_service::delete(&db.conn, id)
        .await
        .map_err(map_db)?;
    emit_event(
        emitter,
        PIPELINE_CHANGED_EVENT,
        PipelineChange::Deleted { id },
    );
    Ok(())
}

pub async fn pipeline_presets_core() -> Vec<PipelineInfo> {
    let now = Utc::now();
    presets::builtin_presets(None)
        .into_iter()
        .map(|(key, name, graph)| PipelineInfo {
            id: 0,
            name: name.to_string(),
            preset_key: Some(key.to_string()),
            folder_id: None,
            graph,
            isolation: crate::models::PipelineIsolation::WorktreePerRun,
            created_at: now,
            updated_at: now,
        })
        .collect()
}


/// Map an engine error to a client-facing error.
///
/// Only a missing engine is a dependency problem; everything else (an already
/// running pipeline, an invalid graph) is the caller's input and must keep its
/// own message, otherwise every failure reads as "the engine is not running".
fn map_engine_error(message: String) -> AppCommandError {
    AppCommandError::invalid_input(message)
}

pub async fn pipeline_run_core(
    _emitter: &EventEmitter,
    _db: &AppDatabase,
    request: PipelineRunRequest,
) -> Result<PipelineRunInfo, AppCommandError> {
    let engine = crate::pipeline::engine::engine().ok_or_else(engine_unavailable)?;
    engine.start(request).await.map_err(map_engine_error)
}

pub async fn pipeline_cancel_core(run_id: i32) -> Result<(), AppCommandError> {
    let engine = crate::pipeline::engine::engine().ok_or_else(engine_unavailable)?;
    engine.cancel(run_id).await.map_err(map_engine_error)
}

pub async fn pipeline_run_status_core(
    db: &AppDatabase,
    run_id: i32,
) -> Result<PipelineRunInfo, AppCommandError> {
    pipeline_service::get_run_info(&db.conn, run_id)
        .await
        .map_err(map_db)
}

pub async fn pipeline_runs_core(
    db: &AppDatabase,
    folder_id: i32,
    limit: u64,
) -> Result<Vec<PipelineRunInfo>, AppCommandError> {
    pipeline_service::list_runs(&db.conn, folder_id, limit)
        .await
        .map_err(map_db)
}

pub async fn pipeline_request_changes_core(
    run_id: i32,
    notes: String,
) -> Result<(), AppCommandError> {
    let engine = crate::pipeline::engine::engine().ok_or_else(engine_unavailable)?;
    engine.request_changes(run_id, notes).await.map_err(map_engine_error)
}

pub async fn pipeline_stop_manual_core(run_id: i32) -> Result<(), AppCommandError> {
    let engine = crate::pipeline::engine::engine().ok_or_else(engine_unavailable)?;
    engine.stop_for_manual_fix(run_id).await.map_err(map_engine_error)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineDiffFile {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineDiff {
    pub files: Vec<PipelineDiffFile>,
    pub patch: String,
    pub truncated: bool,
}

pub async fn pipeline_run_diff_core(
    db: &AppDatabase,
    run_id: i32,
) -> Result<PipelineDiff, AppCommandError> {
    let run = pipeline_service::get_run_raw(&db.conn, run_id)
        .await
        .map_err(map_db)?;
    let target_folder_id = run.worktree_folder_id.unwrap_or(run.folder_id);
    let folder = crate::commands::folders::get_folder_core(db, target_folder_id)
        .await
        .map_err(AppCommandError::from)?;

    // Get statuses (A/M/D/R) from git diff --name-status
    let statuses = crate::pipeline::git_status::get_file_statuses(&folder.path, "HEAD")
        .await
        .unwrap_or_default();

    // Get line counts from git diff --numstat
    let numstat = crate::work_task::git::diff_numstat_with_untracked(&folder.path, "HEAD")
        .await
        .unwrap_or_default();

    // Build a map of file paths to line counts
    let mut numstat_map = std::collections::HashMap::new();
    for f in numstat {
        numstat_map.insert(f.file, (f.additions, f.deletions));
    }

    let mut files = Vec::new();
    for status in statuses {
        let (additions, deletions) = numstat_map
            .get(&status.file)
            .copied()
            .unwrap_or((0, 0));

        files.push(PipelineDiffFile {
            path: status.file,
            status: status.status,
            additions: additions as u32,
            deletions: deletions as u32,
        });
    }

    let patch = crate::work_task::git::diff_patch_with_untracked(&folder.path, "HEAD", None)
        .await
        .unwrap_or_default();

    const MAX_PATCH_BYTES: usize = 2 * 1024 * 1024;
    let (final_patch, truncated) = if patch.len() > MAX_PATCH_BYTES {
        (patch[..MAX_PATCH_BYTES].to_string(), true)
    } else {
        (patch, false)
    };

    Ok(PipelineDiff {
        files,
        patch: final_patch,
        truncated,
    })
}

pub async fn pipeline_run_apply_core(run_id: i32, strategy: String) -> Result<(), AppCommandError> {
    let engine = crate::pipeline::engine::engine().ok_or_else(engine_unavailable)?;
    let run = pipeline_service::get_run_raw(&engine.db().conn, run_id)
        .await
        .map_err(map_db)?;

    // A run that shared the root folder has nothing to merge: its changes are
    // already in the working tree the user is looking at.
    let Some(wt_id) = run.worktree_folder_id else {
        return Ok(());
    };

    let strategy = match strategy.as_str() {
        "squash" => "squash",
        "no_ff" => "no_ff",
        other => {
            return Err(AppCommandError::invalid_input(format!(
                "unknown merge strategy: {other}"
            )))
        }
    };

    let worktree = crate::commands::folders::get_folder_core(engine.db(), wt_id)
        .await
        .map_err(AppCommandError::from)?;
    let root_folder = crate::commands::folders::get_folder_core(engine.db(), run.folder_id)
        .await
        .map_err(AppCommandError::from)?;

    // Agents leave their work uncommitted, so commit it before merging:
    // without this the merge below finds nothing to bring over and silently
    // reports success.
    crate::work_task::git::commit_all(
        &engine.db().conn,
        &worktree.path,
        &format!("Pipeline run #{run_id}"),
    )
    .await?;

    // Read the branch from the worktree itself: a name collision at creation
    // time makes the engine fall back to a suffixed branch, so a name rebuilt
    // from the run id would miss it.
    let head = crate::process::tokio_command("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&worktree.path)
        .output()
        .await
        .map_err(|e| AppCommandError::io_error(format!("git rev-parse failed: {e}")))?;
    if !head.status.success() {
        return Err(AppCommandError::io_error(
            "failed to read the run branch".to_string(),
        ));
    }
    let branch = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        return Err(AppCommandError::io_error(
            "run worktree has no branch to merge".to_string(),
        ));
    }

    // A squash merge only STAGES, and the commit below takes whatever is in
    // the index. Work the user had staged themselves would be swallowed by a
    // commit they did not write, so refuse rather than mix the two.
    if strategy == "squash" && !crate::work_task::git::staged_clean(&root_folder.path).await? {
        return Err(AppCommandError::invalid_input(
            "the project has staged changes; commit or unstage them before applying a run"
                .to_string(),
        ));
    }

    let message = format!("Pipeline run #{run_id}");
    let merged = match strategy {
        "squash" => crate::work_task::git::merge_squash(&root_folder.path, &branch).await,
        _ => crate::work_task::git::merge_no_ff(&root_folder.path, &branch, &message).await,
    };
    if let Err(e) = merged {
        // Leave the root repository usable rather than parked mid-merge.
        let _ = crate::work_task::git::reset_merge(&root_folder.path).await;
        return Err(e);
    }

    // `merge --squash` stages and stops; `merge --no-ff` already committed.
    // Without this the run's only record would be the branch the cleanup
    // below is about to delete.
    if strategy == "squash" {
        if let Err(e) =
            crate::work_task::git::commit_staged(&engine.db().conn, &root_folder.path, &message)
                .await
        {
            let _ = crate::work_task::git::reset_merge(&root_folder.path).await;
            return Err(e);
        }
    }

    // The run is landed, so its worktree and branch are dead weight; a failure
    // here must not fail the apply the user just confirmed.
    let _ = crate::commands::folders::git_remove_worktree_core(
        engine.emitter(),
        engine.db(),
        root_folder.path.clone(),
        branch,
        run.folder_id,
        true,
        false,
    )
    .await;

    Ok(())
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineListParams {
    pub folder_id: Option<i32>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineIdParams {
    pub id: i32,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineSaveParams {
    pub id: Option<i32>,
    pub draft: PipelineDraft,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRunParams {
    pub request: PipelineRunRequest,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRunsParams {
    pub folder_id: i32,
    #[serde(default = "default_run_limit")]
    pub limit: u64,
}

fn default_run_limit() -> u64 {
    20
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRunIdParams {
    pub run_id: i32,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRequestChangesParams {
    pub run_id: i32,
    pub notes: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineApplyParams {
    pub run_id: i32,
    pub strategy: String,
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_list(
    db: tauri::State<'_, AppDatabase>,
    folder_id: Option<i32>,
) -> Result<Vec<PipelineInfo>, AppCommandError> {
    pipeline_list_core(&db, folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_get(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<PipelineInfo, AppCommandError> {
    pipeline_get_core(&db, id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_save(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: Option<i32>,
    draft: PipelineDraft,
) -> Result<PipelineInfo, AppCommandError> {
    pipeline_save_core(&EventEmitter::Tauri(app), &db, id, draft).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_delete(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<(), AppCommandError> {
    pipeline_delete_core(&EventEmitter::Tauri(app), &db, id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_presets() -> Result<Vec<PipelineInfo>, AppCommandError> {
    Ok(pipeline_presets_core().await)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_run(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    request: PipelineRunRequest,
) -> Result<PipelineRunInfo, AppCommandError> {
    pipeline_run_core(&EventEmitter::Tauri(app), &db, request).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_cancel(run_id: i32) -> Result<(), AppCommandError> {
    pipeline_cancel_core(run_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_run_status(
    db: tauri::State<'_, AppDatabase>,
    run_id: i32,
) -> Result<PipelineRunInfo, AppCommandError> {
    pipeline_run_status_core(&db, run_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_runs(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    limit: Option<u64>,
) -> Result<Vec<PipelineRunInfo>, AppCommandError> {
    pipeline_runs_core(&db, folder_id, limit.unwrap_or(20)).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_request_changes(run_id: i32, notes: String) -> Result<(), AppCommandError> {
    pipeline_request_changes_core(run_id, notes).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_stop_manual(run_id: i32) -> Result<(), AppCommandError> {
    pipeline_stop_manual_core(run_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_run_diff(
    db: tauri::State<'_, AppDatabase>,
    run_id: i32,
) -> Result<PipelineDiff, AppCommandError> {
    pipeline_run_diff_core(&db, run_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pipeline_run_apply(run_id: i32, strategy: String) -> Result<(), AppCommandError> {
    pipeline_run_apply_core(run_id, strategy).await
}
