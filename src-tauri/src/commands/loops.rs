//! Loop engineering commands. `_core` functions hold the business logic shared
//! by the desktop (`#[tauri::command]`) and server (Axum handler) modes; every
//! successful write emits the coarse `loop://changed` event so all clients
//! refetch. M2.0 wires CRUD only — engine actions (trigger/pause/…) arrive in
//! M2.1+.

use sea_orm::DatabaseConnection;

use crate::app_error::AppCommandError;
use crate::db::entities::loop_artifact_revision::ActorKind;
use crate::db::entities::loop_inbox_item::{InboxKind, InboxStatus};
use crate::db::entities::loop_issue::{IssuePriority, IssueStatus};
use crate::db::entities::loop_memory::{MemoryKind, MemoryStatus};
use crate::db::service::folder_service;
use crate::db::service::loop_service::{
    artifact, inbox, issue, iteration, memory, space, validation,
};
use crate::loop_engine::transitions::cas_issue_status;
use crate::models::loops::{
    IssueConfig, LoopArtifactDetail, LoopArtifactRow, LoopChanged, LoopDagView, LoopInboxItemRow,
    LoopIssueDetail, LoopIterationRow, LoopMemoryRow, LoopSpaceSummary, LoopValidationRunRow,
    LOOP_CHANGED_EVENT,
};
use crate::loop_engine::LoopEngine;
use crate::web::event_bridge::{emit_event, EventEmitter};
use std::sync::Arc;

#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;

fn emit_loop_changed(
    emitter: &EventEmitter,
    space_id: i32,
    issue_id: Option<i32>,
    subject_kind: &str,
    subject_id: i32,
    kind: &str,
) {
    emit_event(
        emitter,
        LOOP_CHANGED_EVENT,
        LoopChanged {
            v: 1,
            space_id,
            issue_id,
            subject_kind: subject_kind.to_string(),
            subject_id,
            kind: kind.to_string(),
        },
    );
}

async fn folder_is_git_repo(path: &str) -> bool {
    tokio::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ─── Spaces ──────────────────────────────────────────────────────────────

pub async fn list_loop_spaces_core(
    conn: &DatabaseConnection,
) -> Result<Vec<LoopSpaceSummary>, AppCommandError> {
    Ok(space::list_spaces(conn).await?)
}

pub async fn create_loop_space_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    name: String,
    folder_id: i32,
) -> Result<LoopSpaceSummary, AppCommandError> {
    let folder = folder_service::get_folder_by_id(conn, folder_id)
        .await?
        .ok_or_else(|| AppCommandError::not_found("Folder not found"))?;
    if !folder_is_git_repo(&folder.path).await {
        return Err(AppCommandError::not_a_git_repository(
            "Loop space folder must be a git repository",
        ));
    }
    let created = space::create_space(conn, &name, folder_id).await?;
    emit_loop_changed(emitter, created.id, None, "space", created.id, "created");
    summary_for(conn, created.id).await
}

pub async fn update_loop_space_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    id: i32,
    name: String,
) -> Result<LoopSpaceSummary, AppCommandError> {
    space::update_space(conn, id, &name).await?;
    emit_loop_changed(emitter, id, None, "space", id, "updated");
    summary_for(conn, id).await
}

pub async fn set_loop_space_default_config_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    id: i32,
    config: Option<IssueConfig>,
) -> Result<(), AppCommandError> {
    space::set_default_config(conn, id, config.as_ref()).await?;
    emit_loop_changed(emitter, id, None, "space", id, "updated");
    Ok(())
}

pub async fn delete_loop_space_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    id: i32,
) -> Result<(), AppCommandError> {
    space::delete_space(conn, id).await?;
    emit_loop_changed(emitter, id, None, "space", id, "deleted");
    Ok(())
}

async fn summary_for(
    conn: &DatabaseConnection,
    id: i32,
) -> Result<LoopSpaceSummary, AppCommandError> {
    space::list_spaces(conn)
        .await?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| AppCommandError::not_found("Loop space not found"))
}

// ─── Issues ──────────────────────────────────────────────────────────────

pub async fn list_loop_issues_core(
    conn: &DatabaseConnection,
    space_id: i32,
    statuses: Option<Vec<IssueStatus>>,
) -> Result<Vec<crate::models::loops::LoopIssueRow>, AppCommandError> {
    Ok(issue::list_issues(conn, space_id, statuses).await?)
}

pub async fn get_loop_issue_core(
    conn: &DatabaseConnection,
    id: i32,
) -> Result<Option<LoopIssueDetail>, AppCommandError> {
    Ok(issue::get_issue_detail(conn, id).await?)
}

pub async fn create_loop_issue_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    space_id: i32,
    title: String,
    description: String,
    priority: IssuePriority,
    config: Option<IssueConfig>,
) -> Result<LoopIssueDetail, AppCommandError> {
    // No explicit config → the issue inherits the space default (resolved at
    // read time); an explicit config makes it a custom issue.
    let inherits = config.is_none();
    let config = config.unwrap_or_default();
    let mut detail =
        issue::create_issue(conn, space_id, &title, &description, priority, &config).await?;
    if inherits {
        issue::set_config_inherits(conn, detail.row.id, true).await?;
        detail.config_inherits = true;
    }
    emit_loop_changed(emitter, space_id, Some(detail.row.id), "issue", detail.row.id, "created");
    Ok(detail)
}

pub async fn delete_loop_issue_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    id: i32,
) -> Result<(), AppCommandError> {
    let space_id = issue::get_issue(conn, id).await?.map(|i| i.space_id);
    issue::delete_issue(conn, id).await?;
    if let Some(space_id) = space_id {
        emit_loop_changed(emitter, space_id, Some(id), "issue", id, "deleted");
    }
    Ok(())
}

pub async fn update_loop_issue_config_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    id: i32,
    config: IssueConfig,
    token_budget: Option<i64>,
    config_inherits: bool,
) -> Result<(), AppCommandError> {
    let space_id = issue::get_issue(conn, id).await?.map(|i| i.space_id);
    issue::update_issue_config(conn, id, &config, token_budget, config_inherits).await?;
    if let Some(space_id) = space_id {
        emit_loop_changed(emitter, space_id, Some(id), "issue", id, "updated");
    }
    Ok(())
}

// ─── Engine actions (trigger / pause / resume / cancel) ─────────────────────

pub async fn trigger_loop_issue_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    engine: &Arc<LoopEngine>,
    id: i32,
) -> Result<(), AppCommandError> {
    let issue = issue::get_issue(conn, id)
        .await?
        .ok_or_else(|| AppCommandError::not_found("Issue not found"))?;
    engine.trigger_issue(id).await?;
    emit_loop_changed(emitter, issue.space_id, Some(id), "issue", id, "triggered");
    Ok(())
}

pub async fn pause_loop_issue_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    engine: &Arc<LoopEngine>,
    id: i32,
) -> Result<(), AppCommandError> {
    let issue = issue::get_issue(conn, id)
        .await?
        .ok_or_else(|| AppCommandError::not_found("Issue not found"))?;
    engine.pause_issue(id).await?;
    emit_loop_changed(emitter, issue.space_id, Some(id), "issue", id, "paused");
    Ok(())
}

pub async fn resume_loop_issue_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    engine: &Arc<LoopEngine>,
    id: i32,
) -> Result<(), AppCommandError> {
    let issue = issue::get_issue(conn, id)
        .await?
        .ok_or_else(|| AppCommandError::not_found("Issue not found"))?;
    engine.resume_issue(id).await?;
    emit_loop_changed(emitter, issue.space_id, Some(id), "issue", id, "resumed");
    Ok(())
}

pub async fn cancel_loop_issue_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    engine: &Arc<LoopEngine>,
    id: i32,
) -> Result<(), AppCommandError> {
    let issue = issue::get_issue(conn, id)
        .await?
        .ok_or_else(|| AppCommandError::not_found("Issue not found"))?;
    engine.cancel_issue(id).await?;
    emit_loop_changed(emitter, issue.space_id, Some(id), "issue", id, "cancelled");
    Ok(())
}

/// Retry a blocked issue (inbox escape hatch): the engine re-arms the blocked
/// tasks, marks the blocking cards handled, and resumes the issue — emitting the
/// change itself, so this wrapper is thin.
pub async fn retry_loop_issue_core(
    _conn: &DatabaseConnection,
    _emitter: &EventEmitter,
    engine: &Arc<LoopEngine>,
    id: i32,
) -> Result<(), AppCommandError> {
    engine.retry_issue(id).await?;
    Ok(())
}

/// Add `additional` tokens to a budget-paused issue's budget and resume it (the
/// engine emits the change).
pub async fn add_loop_issue_budget_core(
    _conn: &DatabaseConnection,
    _emitter: &EventEmitter,
    engine: &Arc<LoopEngine>,
    id: i32,
    additional: i64,
) -> Result<(), AppCommandError> {
    engine.add_budget(id, additional).await?;
    Ok(())
}

// ─── Merge gate (approve / reject the result) ───────────────────────────────

/// Approve a finalized issue's merge: the engine lands its loop branch on the
/// base branch (under a per-repo lock, with the stale-base check) and closes the
/// issue, or blocks it with an inbox card on any fault. The engine emits the
/// `loop://changed` event itself, covering both this path and auto-merge.
pub async fn approve_loop_merge_core(
    _conn: &DatabaseConnection,
    _emitter: &EventEmitter,
    engine: &Arc<LoopEngine>,
    id: i32,
) -> Result<(), AppCommandError> {
    engine.merge_issue(id).await?;
    Ok(())
}

/// Reject a finalized issue's merge: the work does not land. The issue is blocked
/// for human follow-up (cancel, or adjust and retrigger) with a card carrying the
/// reviewer's comment; any pending merge-approval card is marked handled.
pub async fn reject_loop_merge_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    engine: &Arc<LoopEngine>,
    id: i32,
    comment: Option<String>,
) -> Result<(), AppCommandError> {
    let issue = issue::get_issue(conn, id)
        .await?
        .ok_or_else(|| AppCommandError::not_found("Issue not found"))?;
    // Clear a pending merge-approval card if the approval gate filed one.
    let pending = inbox::list_inbox(conn, issue.space_id, Some(InboxStatus::Pending)).await?;
    if let Some(card) = pending
        .into_iter()
        .find(|c| c.kind == InboxKind::Approval && c.subject_key == format!("merge:{id}"))
    {
        inbox::handle_inbox(
            conn,
            card.id,
            serde_json::json!({ "action": "reject", "comment": comment }),
        )
        .await?;
    }
    if !cas_issue_status(conn, id, IssueStatus::Running, IssueStatus::Blocked).await? {
        return Err(crate::loop_engine::LoopError::Conflict.into());
    }
    inbox::upsert_inbox(
        conn,
        issue.space_id,
        id,
        None,
        InboxKind::Blocked,
        &format!("merge_rejected:{id}"),
        serde_json::json!({ "reason": "merge_rejected", "comment": comment }),
    )
    .await?;
    // Wake the parked driver so it re-ticks, sees the non-running status, and exits.
    engine.wake(id).await;
    emit_loop_changed(emitter, issue.space_id, Some(id), "issue", id, "merge_rejected");
    Ok(())
}

// ─── Design approval gate (route=full) ──────────────────────────────────────

/// Approve the design gate: the engine marks the design done and advances the
/// issue to planning (and emits the change event).
pub async fn approve_loop_design_core(
    _conn: &DatabaseConnection,
    _emitter: &EventEmitter,
    engine: &Arc<LoopEngine>,
    id: i32,
) -> Result<(), AppCommandError> {
    engine.approve_design(id).await?;
    Ok(())
}

/// Reject the design gate with a comment: the engine supersedes the design and
/// re-runs design with the feedback (and emits the change event).
pub async fn reject_loop_design_core(
    _conn: &DatabaseConnection,
    _emitter: &EventEmitter,
    engine: &Arc<LoopEngine>,
    id: i32,
    comment: Option<String>,
) -> Result<(), AppCommandError> {
    engine.reject_design(id, comment).await?;
    Ok(())
}

// ─── Artifacts / DAG ───────────────────────────────────────────────────────

pub async fn get_loop_dag_core(
    conn: &DatabaseConnection,
    issue_id: i32,
) -> Result<LoopDagView, AppCommandError> {
    Ok(artifact::list_dag(conn, issue_id).await?)
}

pub async fn list_loop_artifacts_core(
    conn: &DatabaseConnection,
    space_id: i32,
) -> Result<Vec<LoopArtifactRow>, AppCommandError> {
    Ok(artifact::list_artifacts_for_space(conn, space_id).await?)
}

pub async fn get_loop_artifact_core(
    conn: &DatabaseConnection,
    id: i32,
) -> Result<Option<LoopArtifactDetail>, AppCommandError> {
    Ok(artifact::get_artifact_detail(conn, id).await?)
}

// ─── Iterations ────────────────────────────────────────────────────────────

pub async fn list_loop_iterations_core(
    conn: &DatabaseConnection,
    space_id: i32,
    issue_id: Option<i32>,
) -> Result<Vec<LoopIterationRow>, AppCommandError> {
    Ok(match issue_id {
        Some(issue_id) => iteration::list_iterations(conn, issue_id).await?,
        None => iteration::list_iterations_for_space(conn, space_id).await?,
    })
}

pub async fn list_loop_validations_core(
    conn: &DatabaseConnection,
    space_id: i32,
) -> Result<Vec<LoopValidationRunRow>, AppCommandError> {
    Ok(validation::list_for_space(conn, space_id).await?)
}

// ─── Inbox ─────────────────────────────────────────────────────────────────

pub async fn list_loop_inbox_core(
    conn: &DatabaseConnection,
    space_id: i32,
    status: Option<InboxStatus>,
) -> Result<Vec<LoopInboxItemRow>, AppCommandError> {
    Ok(inbox::list_inbox(conn, space_id, status).await?)
}

// ─── Memory ────────────────────────────────────────────────────────────────

pub async fn list_loop_memory_core(
    conn: &DatabaseConnection,
    space_id: i32,
) -> Result<Vec<LoopMemoryRow>, AppCommandError> {
    Ok(memory::list_memory(conn, space_id).await?)
}

pub async fn create_loop_memory_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    space_id: i32,
    kind: MemoryKind,
    title: String,
    content: String,
) -> Result<LoopMemoryRow, AppCommandError> {
    let m = memory::create_memory(conn, space_id, kind, ActorKind::Human, &title, &content).await?;
    emit_loop_changed(emitter, space_id, None, "memory", m.id, "created");
    Ok(memory::to_row(m))
}

pub async fn update_loop_memory_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    space_id: i32,
    id: i32,
    title: String,
    content: String,
    status: MemoryStatus,
) -> Result<(), AppCommandError> {
    memory::update_memory(conn, id, &title, &content, status).await?;
    emit_loop_changed(emitter, space_id, None, "memory", id, "updated");
    Ok(())
}

pub async fn delete_loop_memory_core(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
    space_id: i32,
    id: i32,
) -> Result<(), AppCommandError> {
    memory::delete_memory(conn, id).await?;
    emit_loop_changed(emitter, space_id, None, "memory", id, "deleted");
    Ok(())
}

// ─── Tauri command wrappers (desktop) ──────────────────────────────────────

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_loop_spaces(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<LoopSpaceSummary>, AppCommandError> {
    list_loop_spaces_core(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_loop_space(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    name: String,
    folder_id: i32,
) -> Result<LoopSpaceSummary, AppCommandError> {
    create_loop_space_core(&db.conn, &EventEmitter::Tauri(app), name, folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_loop_space(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    name: String,
) -> Result<LoopSpaceSummary, AppCommandError> {
    update_loop_space_core(&db.conn, &EventEmitter::Tauri(app), id, name).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn set_loop_space_default_config(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    config: Option<IssueConfig>,
) -> Result<(), AppCommandError> {
    set_loop_space_default_config_core(&db.conn, &EventEmitter::Tauri(app), id, config).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn delete_loop_space(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<(), AppCommandError> {
    delete_loop_space_core(&db.conn, &EventEmitter::Tauri(app), id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_loop_issues(
    db: tauri::State<'_, AppDatabase>,
    space_id: i32,
    statuses: Option<Vec<IssueStatus>>,
) -> Result<Vec<crate::models::loops::LoopIssueRow>, AppCommandError> {
    list_loop_issues_core(&db.conn, space_id, statuses).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_loop_issue(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<Option<LoopIssueDetail>, AppCommandError> {
    get_loop_issue_core(&db.conn, id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_loop_issue(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    space_id: i32,
    title: String,
    description: String,
    priority: IssuePriority,
    config: Option<IssueConfig>,
) -> Result<LoopIssueDetail, AppCommandError> {
    create_loop_issue_core(
        &db.conn,
        &EventEmitter::Tauri(app),
        space_id,
        title,
        description,
        priority,
        config,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn delete_loop_issue(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<(), AppCommandError> {
    delete_loop_issue_core(&db.conn, &EventEmitter::Tauri(app), id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_loop_issue_config(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    config: IssueConfig,
    token_budget: Option<i64>,
    config_inherits: bool,
) -> Result<(), AppCommandError> {
    update_loop_issue_config_core(
        &db.conn,
        &EventEmitter::Tauri(app),
        id,
        config,
        token_budget,
        config_inherits,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn trigger_loop_issue(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    engine: tauri::State<'_, Arc<LoopEngine>>,
    id: i32,
) -> Result<(), AppCommandError> {
    trigger_loop_issue_core(&db.conn, &EventEmitter::Tauri(app), engine.inner(), id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pause_loop_issue(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    engine: tauri::State<'_, Arc<LoopEngine>>,
    id: i32,
) -> Result<(), AppCommandError> {
    pause_loop_issue_core(&db.conn, &EventEmitter::Tauri(app), engine.inner(), id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn resume_loop_issue(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    engine: tauri::State<'_, Arc<LoopEngine>>,
    id: i32,
) -> Result<(), AppCommandError> {
    resume_loop_issue_core(&db.conn, &EventEmitter::Tauri(app), engine.inner(), id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn cancel_loop_issue(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    engine: tauri::State<'_, Arc<LoopEngine>>,
    id: i32,
) -> Result<(), AppCommandError> {
    cancel_loop_issue_core(&db.conn, &EventEmitter::Tauri(app), engine.inner(), id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn retry_loop_issue(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    engine: tauri::State<'_, Arc<LoopEngine>>,
    id: i32,
) -> Result<(), AppCommandError> {
    retry_loop_issue_core(&db.conn, &EventEmitter::Tauri(app), engine.inner(), id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn add_loop_issue_budget(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    engine: tauri::State<'_, Arc<LoopEngine>>,
    id: i32,
    additional: i64,
) -> Result<(), AppCommandError> {
    add_loop_issue_budget_core(&db.conn, &EventEmitter::Tauri(app), engine.inner(), id, additional)
        .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn approve_loop_merge(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    engine: tauri::State<'_, Arc<LoopEngine>>,
    id: i32,
) -> Result<(), AppCommandError> {
    approve_loop_merge_core(&db.conn, &EventEmitter::Tauri(app), engine.inner(), id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn reject_loop_merge(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    engine: tauri::State<'_, Arc<LoopEngine>>,
    id: i32,
    comment: Option<String>,
) -> Result<(), AppCommandError> {
    reject_loop_merge_core(&db.conn, &EventEmitter::Tauri(app), engine.inner(), id, comment).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn approve_loop_design(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    engine: tauri::State<'_, Arc<LoopEngine>>,
    id: i32,
) -> Result<(), AppCommandError> {
    approve_loop_design_core(&db.conn, &EventEmitter::Tauri(app), engine.inner(), id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn reject_loop_design(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    engine: tauri::State<'_, Arc<LoopEngine>>,
    id: i32,
    comment: Option<String>,
) -> Result<(), AppCommandError> {
    reject_loop_design_core(&db.conn, &EventEmitter::Tauri(app), engine.inner(), id, comment).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_loop_dag(
    db: tauri::State<'_, AppDatabase>,
    issue_id: i32,
) -> Result<LoopDagView, AppCommandError> {
    get_loop_dag_core(&db.conn, issue_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_loop_artifacts(
    db: tauri::State<'_, AppDatabase>,
    space_id: i32,
) -> Result<Vec<LoopArtifactRow>, AppCommandError> {
    list_loop_artifacts_core(&db.conn, space_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_loop_artifact(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<Option<LoopArtifactDetail>, AppCommandError> {
    get_loop_artifact_core(&db.conn, id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_loop_iterations(
    db: tauri::State<'_, AppDatabase>,
    space_id: i32,
    issue_id: Option<i32>,
) -> Result<Vec<LoopIterationRow>, AppCommandError> {
    list_loop_iterations_core(&db.conn, space_id, issue_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_loop_validations(
    db: tauri::State<'_, AppDatabase>,
    space_id: i32,
) -> Result<Vec<LoopValidationRunRow>, AppCommandError> {
    list_loop_validations_core(&db.conn, space_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_loop_inbox(
    db: tauri::State<'_, AppDatabase>,
    space_id: i32,
    status: Option<InboxStatus>,
) -> Result<Vec<LoopInboxItemRow>, AppCommandError> {
    list_loop_inbox_core(&db.conn, space_id, status).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_loop_memory(
    db: tauri::State<'_, AppDatabase>,
    space_id: i32,
) -> Result<Vec<LoopMemoryRow>, AppCommandError> {
    list_loop_memory_core(&db.conn, space_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_loop_memory(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    space_id: i32,
    kind: MemoryKind,
    title: String,
    content: String,
) -> Result<LoopMemoryRow, AppCommandError> {
    create_loop_memory_core(&db.conn, &EventEmitter::Tauri(app), space_id, kind, title, content)
        .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_loop_memory(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    space_id: i32,
    id: i32,
    title: String,
    content: String,
    status: MemoryStatus,
) -> Result<(), AppCommandError> {
    update_loop_memory_core(
        &db.conn,
        &EventEmitter::Tauri(app),
        space_id,
        id,
        title,
        content,
        status,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn delete_loop_memory(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    space_id: i32,
    id: i32,
) -> Result<(), AppCommandError> {
    delete_loop_memory_core(&db.conn, &EventEmitter::Tauri(app), space_id, id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::loop_issue;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use sea_orm::EntityTrait;

    async fn seed_space(db: &crate::db::AppDatabase) -> i32 {
        let folder_id = seed_folder(db, "/tmp/loop-cmd").await;
        space::create_space(&db.conn, "S", folder_id)
            .await
            .unwrap()
            .id
    }

    async fn issue_row(db: &crate::db::AppDatabase, id: i32) -> loop_issue::Model {
        loop_issue::Entity::find_by_id(id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn create_without_config_inherits_space_default() {
        let db = fresh_in_memory_db().await;
        let space_id = seed_space(&db).await;
        let detail = create_loop_issue_core(
            &db.conn,
            &EventEmitter::Noop,
            space_id,
            "Issue".into(),
            "body".into(),
            IssuePriority::Medium,
            None,
        )
        .await
        .unwrap();
        assert!(detail.config_inherits, "no explicit config → inherits");
        assert!(issue_row(&db, detail.row.id).await.config_inherits);
    }

    #[tokio::test]
    async fn create_with_config_is_custom() {
        let db = fresh_in_memory_db().await;
        let space_id = seed_space(&db).await;
        let detail = create_loop_issue_core(
            &db.conn,
            &EventEmitter::Noop,
            space_id,
            "Issue".into(),
            "body".into(),
            IssuePriority::Medium,
            Some(IssueConfig::default()),
        )
        .await
        .unwrap();
        assert!(!detail.config_inherits, "explicit config → custom");
        assert!(!issue_row(&db, detail.row.id).await.config_inherits);
    }

    #[tokio::test]
    async fn update_config_toggles_inherit_and_preserves_custom() {
        let db = fresh_in_memory_db().await;
        let space_id = seed_space(&db).await;
        let detail = create_loop_issue_core(
            &db.conn,
            &EventEmitter::Noop,
            space_id,
            "Issue".into(),
            "body".into(),
            IssuePriority::Medium,
            Some(IssueConfig {
                max_attempts: 42,
                ..IssueConfig::default()
            }),
        )
        .await
        .unwrap();
        let id = detail.row.id;

        // Switch to inherit: stored config is preserved, only the flag flips.
        update_loop_issue_config_core(
            &db.conn,
            &EventEmitter::Noop,
            id,
            IssueConfig::default(),
            None,
            true,
        )
        .await
        .unwrap();
        let row = issue_row(&db, id).await;
        assert!(row.config_inherits);
        let preserved: IssueConfig = serde_json::from_str(&row.config).unwrap();
        assert_eq!(
            preserved.max_attempts, 42,
            "custom config preserved while inheriting"
        );

        // Switch back to custom with a new config.
        update_loop_issue_config_core(
            &db.conn,
            &EventEmitter::Noop,
            id,
            IssueConfig {
                max_attempts: 7,
                ..IssueConfig::default()
            },
            None,
            false,
        )
        .await
        .unwrap();
        let row = issue_row(&db, id).await;
        assert!(!row.config_inherits);
        let cfg: IssueConfig = serde_json::from_str(&row.config).unwrap();
        assert_eq!(cfg.max_attempts, 7);
    }

    #[tokio::test]
    async fn set_and_clear_space_default_config() {
        let db = fresh_in_memory_db().await;
        let space_id = seed_space(&db).await;

        set_loop_space_default_config_core(
            &db.conn,
            &EventEmitter::Noop,
            space_id,
            Some(IssueConfig {
                max_attempts: 13,
                ..IssueConfig::default()
            }),
        )
        .await
        .unwrap();
        let summary = summary_for(&db.conn, space_id).await.unwrap();
        assert_eq!(summary.default_config.unwrap().max_attempts, 13);

        set_loop_space_default_config_core(&db.conn, &EventEmitter::Noop, space_id, None)
            .await
            .unwrap();
        let summary = summary_for(&db.conn, space_id).await.unwrap();
        assert!(summary.default_config.is_none());
    }
}
