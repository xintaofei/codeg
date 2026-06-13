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
use crate::db::service::loop_service::{artifact, inbox, issue, iteration, memory, space};
use crate::loop_engine::transitions::cas_issue_status;
use crate::models::loops::{
    IssueConfig, LoopArtifactDetail, LoopArtifactRow, LoopChanged, LoopDagView, LoopInboxItemRow,
    LoopIssueDetail, LoopIterationRow, LoopMemoryRow, LoopSpaceSummary, LOOP_CHANGED_EVENT,
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
    let config = config.unwrap_or_default();
    let detail = issue::create_issue(conn, space_id, &title, &description, priority, &config).await?;
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
) -> Result<(), AppCommandError> {
    let space_id = issue::get_issue(conn, id).await?.map(|i| i.space_id);
    issue::update_issue_config(conn, id, &config, token_budget).await?;
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
) -> Result<(), AppCommandError> {
    update_loop_issue_config_core(&db.conn, &EventEmitter::Tauri(app), id, config, token_budget)
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
