//! Durable ordinary-delegation admission and terminal-result ledger.
//!
//! This module intentionally owns only the durable facts needed by the
//! delegation runtime. It does not own connections, turns, retries, or
//! process cleanup. The runtime performs its busy/strict-resume checks first,
//! then calls [`admit`] immediately before sending the child prompt.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, Set,
};

use crate::acp::delegation::types::{DelegationTaskReport, TaskStatus};
use crate::db::entities::{conversation, delegation_task, folder};
use crate::db::error::DbError;
use crate::models::AgentType;

/// The exact agent/session/config identity used by a child execution.
///
/// This is serialized into the ledger as one JSON value so a future strict
/// resume can validate the complete binding before admission. `working_dir`
/// is the canonical directory actually used by the child, while
/// `requested_working_dir` on [`AdmissionInput`] preserves the caller's raw
/// request for retry correlation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResumeBinding {
    pub agent_type: AgentType,
    pub external_session_id: String,
    pub child_conversation_id: i32,
    pub working_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Effective value after the runtime applies its requested/default mode.
    /// The `preferred_` name matches the manager/spawner hand-off vocabulary.
    pub preferred_mode_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub preferred_config_values: BTreeMap<String, String>,
    pub config_fingerprint: String,
}

/// Prepared admission input. The runtime must supply the already-created
/// child row and the strict binding it intends to use; no prompt is sent by
/// this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionInput {
    pub task_id: String,
    pub parent_conversation_id: i32,
    pub child_conversation_id: i32,
    pub source_task_id: Option<String>,
    pub task: String,
    pub requested_working_dir: Option<String>,
    pub resume_binding: ResumeBinding,
}

/// Metadata plus the report visible to the broker. A terminal report is read
/// from the immutable JSON snapshot; a running row gets a synthesized running
/// report and never reads the child's mutable conversation status.
#[derive(Debug, Clone)]
pub struct TaskLedgerEntry {
    pub id: i32,
    pub task_id: String,
    pub parent_conversation_id: i32,
    pub child_conversation_id: i32,
    pub source_task_id: Option<String>,
    pub task: String,
    pub requested_working_dir: Option<String>,
    pub resume_binding: ResumeBinding,
    pub status: TaskStatus,
    pub released: bool,
    pub report: DelegationTaskReport,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Result of the atomic source-slot admission.
#[derive(Debug, Clone)]
pub enum AdmissionResult {
    New {
        entry: TaskLedgerEntry,
    },
    Existing {
        entry: TaskLedgerEntry,
    },
    Conflict {
        next_task_id: String,
        reason: String,
    },
}

/// Insert one durable execution record. A source slot is reserved by the
/// unique index on `source_task_id`; a loser of that race is reconciled with
/// the winner and never sends a second prompt for the same source.
pub async fn admit(
    conn: &DatabaseConnection,
    input: AdmissionInput,
) -> Result<AdmissionResult, DbError> {
    validate_input(&input)?;
    ensure_live_conversation(conn, input.parent_conversation_id, "parent").await?;
    ensure_live_conversation(conn, input.child_conversation_id, "child").await?;

    if let Some(source_task_id) = input.source_task_id.as_deref() {
        // Retries must resolve an already-admitted successor before checking
        // whether the source can be started again. This is important after a
        // process restart: the successor may still be running while the
        // caller repeats the same request.
        if let Some(winner) = delegation_task::Entity::find()
            .filter(delegation_task::Column::ParentConversationId.eq(input.parent_conversation_id))
            .filter(delegation_task::Column::SourceTaskId.eq(source_task_id))
            .one(conn)
            .await?
        {
            let Some(entry) =
                load_authorized(conn, input.parent_conversation_id, &winner.task_id).await?
            else {
                return Err(DbError::Conflict(format!(
                    "source task {source_task_id} is already reserved but is no longer queryable"
                )));
            };
            if same_admission_key(&winner, &input)? {
                return Ok(AdmissionResult::Existing { entry });
            }
            return Ok(AdmissionResult::Conflict {
                next_task_id: winner.task_id,
                reason: format!(
                    "source task {source_task_id} already has a successor with a different task, agent, or working directory"
                ),
            });
        }

        let source = find_raw_by_task_id(conn, source_task_id)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("source task {source_task_id}")))?;
        if source.parent_conversation_id != input.parent_conversation_id {
            return Err(DbError::NotFound(format!("source task {source_task_id}")));
        }
        if !is_terminal_status(&source.status) || !source.released {
            return Err(DbError::Validation(format!(
                "source task {source_task_id} is not eligible: it must be terminal and released"
            )));
        }
        let source_binding: ResumeBinding =
            serde_json::from_str(&source.resume_binding).map_err(|e| {
                DbError::Migration(format!(
                    "invalid resume binding for source {source_task_id}: {e}"
                ))
            })?;
        if source_binding != input.resume_binding {
            return Err(DbError::Conflict(format!(
                "source task {source_task_id} binding does not match the requested child session"
            )));
        }
        // This also checks the source child and its folder. A retained source
        // is not a usable resume anchor after either conversation is deleted.
        if load_authorized(conn, input.parent_conversation_id, source_task_id)
            .await?
            .is_none()
        {
            return Err(DbError::NotFound(format!("source task {source_task_id}")));
        }
    }

    let binding_json = serde_json::to_string(&input.resume_binding)
        .map_err(|e| DbError::Validation(format!("invalid resume binding: {e}")))?;
    let now = Utc::now();
    let active = delegation_task::ActiveModel {
        id: NotSet,
        task_id: Set(input.task_id.clone()),
        parent_conversation_id: Set(input.parent_conversation_id),
        child_conversation_id: Set(input.child_conversation_id),
        source_task_id: Set(input.source_task_id.clone()),
        task: Set(input.task.clone()),
        requested_working_dir: Set(input.requested_working_dir.clone()),
        status: Set(status_string(TaskStatus::Running)),
        terminal_report: Set(None),
        resume_binding: Set(binding_json),
        released: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    };

    match active.insert(conn).await {
        Ok(model) => Ok(AdmissionResult::New {
            entry: entry_from_model(model)?,
        }),
        Err(insert_error) => reconcile_insert_race(conn, &input, insert_error).await,
    }
}

/// Lookup a task under parent authorization. Deleted parent/child/folder rows
/// are intentionally indistinguishable from an unknown task.
pub async fn lookup(
    conn: &DatabaseConnection,
    parent_conversation_id: i32,
    task_id: &str,
) -> Result<Option<TaskLedgerEntry>, DbError> {
    load_authorized(conn, parent_conversation_id, task_id).await
}

/// Return the one successor reserved by `source_task_id`, if it is visible to
/// the authorized parent.
pub async fn successor(
    conn: &DatabaseConnection,
    parent_conversation_id: i32,
    source_task_id: &str,
) -> Result<Option<TaskLedgerEntry>, DbError> {
    let row = delegation_task::Entity::find()
        .filter(delegation_task::Column::ParentConversationId.eq(parent_conversation_id))
        .filter(delegation_task::Column::SourceTaskId.eq(source_task_id))
        .one(conn)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if !conversations_are_live(conn, parent_conversation_id, row.child_conversation_id).await? {
        return Ok(None);
    }
    Ok(Some(entry_from_model(row)?))
}

/// Return the source id of an authorized task. The source row itself is not
/// required to be live here; only the task being inspected is authorized.
pub async fn source(
    conn: &DatabaseConnection,
    parent_conversation_id: i32,
    task_id: &str,
) -> Result<Option<String>, DbError> {
    Ok(lookup(conn, parent_conversation_id, task_id)
        .await?
        .and_then(|entry| entry.source_task_id))
}

/// Freeze one terminal report. The conditional UPDATE makes finish idempotent
/// and prevents a late old connection from changing a newer terminal result.
/// Returns `true` only when this call won the terminal write.
pub async fn finish(
    conn: &DatabaseConnection,
    parent_conversation_id: i32,
    task_id: &str,
    report: &DelegationTaskReport,
) -> Result<bool, DbError> {
    let Some(row) = load_authorized(conn, parent_conversation_id, task_id).await? else {
        return Err(DbError::NotFound(format!("delegation task {task_id}")));
    };
    validate_terminal_report(task_id, report, &row)?;
    let terminal_report = serde_json::to_string(report)
        .map_err(|e| DbError::Validation(format!("cannot serialize terminal report: {e}")))?;
    let result = delegation_task::Entity::update_many()
        .col_expr(
            delegation_task::Column::Status,
            sea_orm::sea_query::Expr::value(status_string(report.status)),
        )
        .col_expr(
            delegation_task::Column::TerminalReport,
            sea_orm::sea_query::Expr::value(terminal_report),
        )
        .col_expr(
            delegation_task::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(Utc::now()),
        )
        .filter(delegation_task::Column::Id.eq(row.id))
        .filter(delegation_task::Column::Status.eq(status_string(TaskStatus::Running)))
        .filter(delegation_task::Column::TerminalReport.is_null())
        .exec(conn)
        .await?;
    Ok(result.rows_affected == 1)
}

/// Mark process release independently from finish. This is deliberately a
/// one-way CAS: finishing a task never resets a release acknowledgement.
pub async fn mark_released(
    conn: &DatabaseConnection,
    parent_conversation_id: i32,
    task_id: &str,
) -> Result<bool, DbError> {
    let Some(row) = load_authorized(conn, parent_conversation_id, task_id).await? else {
        return Err(DbError::NotFound(format!("delegation task {task_id}")));
    };
    let result = delegation_task::Entity::update_many()
        .col_expr(
            delegation_task::Column::Released,
            sea_orm::sea_query::Expr::value(true),
        )
        .col_expr(
            delegation_task::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(Utc::now()),
        )
        .filter(delegation_task::Column::Id.eq(row.id))
        .filter(delegation_task::Column::Released.eq(false))
        .exec(conn)
        .await?;
    Ok(result.rows_affected == 1)
}

async fn reconcile_insert_race(
    conn: &DatabaseConnection,
    input: &AdmissionInput,
    insert_error: sea_orm::DbErr,
) -> Result<AdmissionResult, DbError> {
    let Some(source_task_id) = input.source_task_id.as_deref() else {
        return Err(insert_error.into());
    };
    let Some(winner) = delegation_task::Entity::find()
        .filter(delegation_task::Column::ParentConversationId.eq(input.parent_conversation_id))
        .filter(delegation_task::Column::SourceTaskId.eq(source_task_id))
        .one(conn)
        .await?
    else {
        return Err(insert_error.into());
    };
    let Some(entry) = load_authorized(conn, input.parent_conversation_id, &winner.task_id).await?
    else {
        return Err(DbError::Conflict(format!(
            "source task {source_task_id} is already reserved but is no longer queryable"
        )));
    };
    if same_admission_key(&winner, input)? {
        return Ok(AdmissionResult::Existing { entry });
    }
    Ok(AdmissionResult::Conflict {
        next_task_id: winner.task_id,
        reason: format!(
                    "source task {source_task_id} already has a successor with a different task, agent, or working directory"
        ),
    })
}

async fn load_authorized(
    conn: &DatabaseConnection,
    parent_conversation_id: i32,
    task_id: &str,
) -> Result<Option<TaskLedgerEntry>, DbError> {
    let Some(row) = delegation_task::Entity::find()
        .filter(delegation_task::Column::TaskId.eq(task_id))
        .filter(delegation_task::Column::ParentConversationId.eq(parent_conversation_id))
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    if !conversations_are_live(conn, parent_conversation_id, row.child_conversation_id).await? {
        return Ok(None);
    }
    Ok(Some(entry_from_model(row)?))
}

async fn find_raw_by_task_id(
    conn: &DatabaseConnection,
    task_id: &str,
) -> Result<Option<delegation_task::Model>, DbError> {
    Ok(delegation_task::Entity::find()
        .filter(delegation_task::Column::TaskId.eq(task_id))
        .one(conn)
        .await?)
}

async fn ensure_live_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
    label: &str,
) -> Result<(), DbError> {
    if conversations_are_live(conn, conversation_id, conversation_id).await? {
        return Ok(());
    }
    Err(DbError::NotFound(format!(
        "{label} conversation {conversation_id}"
    )))
}

async fn conversations_are_live(
    conn: &DatabaseConnection,
    parent_conversation_id: i32,
    child_conversation_id: i32,
) -> Result<bool, DbError> {
    let Some(parent) = conversation::Entity::find_by_id(parent_conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    let Some(child) = conversation::Entity::find_by_id(child_conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    if parent.deleted_at.is_some() || child.deleted_at.is_some() {
        return Ok(false);
    }
    let Some(parent_folder) = folder::Entity::find_by_id(parent.folder_id)
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    let Some(child_folder) = folder::Entity::find_by_id(child.folder_id)
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    Ok(parent_folder.deleted_at.is_none() && child_folder.deleted_at.is_none())
}

fn entry_from_model(model: delegation_task::Model) -> Result<TaskLedgerEntry, DbError> {
    let status = parse_status(&model.status)?;
    let binding: ResumeBinding = serde_json::from_str(&model.resume_binding).map_err(|e| {
        DbError::Migration(format!("invalid resume binding for {}: {e}", model.task_id))
    })?;
    let report = match model.terminal_report.as_deref() {
        Some(raw) => serde_json::from_str(raw).map_err(|e| {
            DbError::Migration(format!(
                "invalid terminal report for {}: {e}",
                model.task_id
            ))
        })?,
        None => running_report(&model, status, binding.agent_type),
    };
    Ok(TaskLedgerEntry {
        id: model.id,
        task_id: model.task_id,
        parent_conversation_id: model.parent_conversation_id,
        child_conversation_id: model.child_conversation_id,
        source_task_id: model.source_task_id,
        task: model.task,
        requested_working_dir: model.requested_working_dir,
        resume_binding: binding,
        status,
        released: model.released,
        report,
        created_at: model.created_at,
        updated_at: model.updated_at,
    })
}

fn running_report(
    model: &delegation_task::Model,
    status: TaskStatus,
    agent_type: AgentType,
) -> DelegationTaskReport {
    DelegationTaskReport {
        task_id: Some(model.task_id.clone()),
        status,
        child_conversation_id: Some(model.child_conversation_id),
        agent_type: Some(agent_type),
        text: None,
        error_code: None,
        message: None,
        duration_ms: None,
        blocked_on: None,
    }
}

fn validate_input(input: &AdmissionInput) -> Result<(), DbError> {
    if input.task_id.trim().is_empty() {
        return Err(DbError::Validation("task id must not be empty".into()));
    }
    if input.task.trim().is_empty() {
        return Err(DbError::Validation("task must not be empty".into()));
    }
    if input.resume_binding.child_conversation_id != input.child_conversation_id {
        return Err(DbError::Validation(
            "resume binding child conversation does not match admission".into(),
        ));
    }
    if input.resume_binding.external_session_id.trim().is_empty()
        || input.resume_binding.working_dir.trim().is_empty()
        || input.resume_binding.config_fingerprint.trim().is_empty()
    {
        return Err(DbError::Validation(
            "resume binding requires session id, working directory, and config fingerprint".into(),
        ));
    }
    Ok(())
}

fn validate_terminal_report(
    task_id: &str,
    report: &DelegationTaskReport,
    entry: &TaskLedgerEntry,
) -> Result<(), DbError> {
    if !is_terminal(report.status) {
        return Err(DbError::Validation(
            "only completed, failed, or canceled reports can finish a task".into(),
        ));
    }
    if report.task_id.as_deref() != Some(task_id) {
        return Err(DbError::Validation(
            "terminal report must carry the admitted task id".into(),
        ));
    }
    if report.child_conversation_id != Some(entry.child_conversation_id) {
        return Err(DbError::Validation(
            "terminal report child conversation does not match admission".into(),
        ));
    }
    if report.agent_type != Some(entry.resume_binding.agent_type) {
        return Err(DbError::Validation(
            "terminal report agent does not match admission".into(),
        ));
    }
    Ok(())
}

fn same_admission_key(
    row: &delegation_task::Model,
    input: &AdmissionInput,
) -> Result<bool, DbError> {
    let binding: ResumeBinding = serde_json::from_str(&row.resume_binding).map_err(|e| {
        DbError::Migration(format!("invalid resume binding for {}: {e}", row.task_id))
    })?;
    Ok(row.task == input.task
        && row.requested_working_dir == input.requested_working_dir
        && binding == input.resume_binding)
}

fn is_terminal(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Canceled
    )
}

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "canceled")
}

fn status_string(status: TaskStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn parse_status(status: &str) -> Result<TaskStatus, DbError> {
    serde_json::from_value(serde_json::Value::String(status.to_owned()))
        .map_err(|e| DbError::Migration(format!("invalid delegation task status {status:?}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::{conversation_service, folder_service};
    use crate::db::test_helpers::{fresh_disk_db, fresh_in_memory_db};
    use sea_orm::Database;

    fn binding(child: i32) -> ResumeBinding {
        ResumeBinding {
            agent_type: AgentType::Codex,
            external_session_id: format!("session-{child}"),
            child_conversation_id: child,
            working_dir: "/workspace/project".into(),
            preferred_mode_id: Some("default".into()),
            preferred_config_values: BTreeMap::from([(String::from("model"), String::from("o3"))]),
            config_fingerprint: "fingerprint-1".into(),
        }
    }

    fn input(
        task_id: &str,
        parent: i32,
        child: i32,
        source: Option<&str>,
        task: &str,
    ) -> AdmissionInput {
        AdmissionInput {
            task_id: task_id.into(),
            parent_conversation_id: parent,
            child_conversation_id: child,
            source_task_id: source.map(str::to_owned),
            task: task.into(),
            requested_working_dir: Some("/workspace/project".into()),
            resume_binding: binding(child),
        }
    }

    async fn conversations(db: &crate::db::AppDatabase) -> (i32, i32) {
        let folder = folder_service::add_folder(&db.conn, "/workspace/project")
            .await
            .expect("folder")
            .id;
        let parent =
            conversation_service::create(&db.conn, folder, AgentType::ClaudeCode, None, None)
                .await
                .expect("parent")
                .id;
        let child = conversation_service::create(&db.conn, folder, AgentType::Codex, None, None)
            .await
            .expect("child")
            .id;
        (parent, child)
    }

    fn report(task_id: &str, child: i32, text: &str, status: TaskStatus) -> DelegationTaskReport {
        DelegationTaskReport {
            task_id: Some(task_id.into()),
            status,
            child_conversation_id: Some(child),
            agent_type: Some(AgentType::Codex),
            text: Some(text.into()),
            error_code: None,
            message: None,
            duration_ms: Some(12),
            blocked_on: None,
        }
    }

    #[tokio::test]
    async fn terminal_reports_survive_disk_reopen_and_child_status_drift() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = fresh_disk_db(dir.path()).await;
        let (parent, child) = conversations(&db).await;
        let result = admit(&db.conn, input("t0", parent, child, None, "first"))
            .await
            .expect("admit");
        assert!(matches!(result, AdmissionResult::New { .. }));

        let first = report("t0", child, "original", TaskStatus::Completed);
        assert!(finish(&db.conn, parent, "t0", &first)
            .await
            .expect("finish"));
        mark_released(&db.conn, parent, "t0")
            .await
            .expect("release t0");
        admit(
            &db.conn,
            input("t1", parent, child, Some("t0"), "follow-up"),
        )
        .await
        .expect("admit t1");
        assert!(finish(
            &db.conn,
            parent,
            "t1",
            &report("t1", child, "follow-up", TaskStatus::Completed),
        )
        .await
        .expect("finish t1"));
        assert!(!finish(
            &db.conn,
            parent,
            "t0",
            &report("t0", child, "late overwrite", TaskStatus::Failed),
        )
        .await
        .expect("idempotent finish"));
        conversation_service::update_status(
            &db.conn,
            child,
            conversation::ConversationStatus::Cancelled,
        )
        .await
        .expect("child status");
        db.conn.close().await.expect("close disk db");

        let path = dir.path().join("source.db");
        let reopened = Database::connect(format!("sqlite:{}?mode=rwc", path.to_string_lossy()))
            .await
            .expect("reopen disk db");

        let entry = lookup(&reopened, parent, "t0")
            .await
            .expect("lookup")
            .expect("entry");
        assert_eq!(
            serde_json::to_value(&entry.report).expect("stored report"),
            serde_json::to_value(&first).expect("expected report"),
        );
        assert_eq!(entry.status, TaskStatus::Completed);
        let successor_entry = lookup(&reopened, parent, "t1")
            .await
            .expect("lookup t1")
            .expect("t1 entry");
        assert_eq!(successor_entry.report.text.as_deref(), Some("follow-up"));
        reopened.close().await.expect("close reopened db");
    }

    #[tokio::test]
    async fn lookup_requires_parent_and_live_rows() {
        let db = fresh_in_memory_db().await;
        let (parent, child) = conversations(&db).await;
        admit(&db.conn, input("t0", parent, child, None, "first"))
            .await
            .expect("admit");
        let other_parent = {
            let folder = folder_service::add_folder(&db.conn, "/workspace/other")
                .await
                .expect("folder")
                .id;
            conversation_service::create(&db.conn, folder, AgentType::ClaudeCode, None, None)
                .await
                .expect("other parent")
                .id
        };
        assert!(lookup(&db.conn, other_parent, "t0")
            .await
            .expect("auth lookup")
            .is_none());
        conversation_service::soft_delete(&db.conn, child)
            .await
            .expect("delete child");
        assert!(lookup(&db.conn, parent, "t0")
            .await
            .expect("deleted lookup")
            .is_none());

        let db = fresh_in_memory_db().await;
        let (parent, child) = conversations(&db).await;
        admit(&db.conn, input("folder-task", parent, child, None, "task"))
            .await
            .expect("admit folder task");
        let folder_id = conversation::Entity::find_by_id(parent)
            .one(&db.conn)
            .await
            .expect("parent row")
            .expect("parent")
            .folder_id;
        folder_service::soft_delete_folder(&db.conn, folder_id)
            .await
            .expect("delete folder");
        assert!(lookup(&db.conn, parent, "folder-task")
            .await
            .expect("folder-deleted lookup")
            .is_none());

        let db = fresh_in_memory_db().await;
        let (parent, child) = conversations(&db).await;
        admit(&db.conn, input("parent-task", parent, child, None, "task"))
            .await
            .expect("admit parent task");
        conversation_service::soft_delete(&db.conn, parent)
            .await
            .expect("delete parent");
        assert!(lookup(&db.conn, parent, "parent-task")
            .await
            .expect("parent-deleted lookup")
            .is_none());
    }

    #[tokio::test]
    async fn source_slot_race_returns_one_existing_successor() {
        let db = fresh_in_memory_db().await;
        let (parent, child) = conversations(&db).await;
        admit(&db.conn, input("t0", parent, child, None, "first"))
            .await
            .expect("admit source");
        let done = report("t0", child, "done", TaskStatus::Completed);
        finish(&db.conn, parent, "t0", &done).await.expect("finish");
        mark_released(&db.conn, parent, "t0")
            .await
            .expect("release");
        let left = input("t1-left", parent, child, Some("t0"), "next");
        let mut right = left.clone();
        right.task_id = "t1-right".into();
        let (left, right) = tokio::join!(admit(&db.conn, left), admit(&db.conn, right));
        let mut new_count = 0;
        let mut existing_count = 0;
        let ids = [left, right]
            .into_iter()
            .map(|result| match result.expect("admission") {
                AdmissionResult::New { entry } => {
                    new_count += 1;
                    entry.task_id
                }
                AdmissionResult::Existing { entry } => {
                    existing_count += 1;
                    entry.task_id
                }
                AdmissionResult::Conflict { .. } => panic!("same key must be idempotent"),
            })
            .collect::<Vec<_>>();
        assert_eq!(new_count, 1);
        assert_eq!(existing_count, 1);
        assert_eq!(ids[0], ids[1]);
        assert!(successor(&db.conn, parent, "t0")
            .await
            .expect("successor")
            .is_some());
    }

    #[tokio::test]
    async fn different_successor_task_is_a_conflict_with_next_id() {
        let db = fresh_in_memory_db().await;
        let (parent, child) = conversations(&db).await;
        admit(&db.conn, input("t0", parent, child, None, "first"))
            .await
            .expect("admit source");
        finish(
            &db.conn,
            parent,
            "t0",
            &report("t0", child, "done", TaskStatus::Completed),
        )
        .await
        .expect("finish");
        mark_released(&db.conn, parent, "t0")
            .await
            .expect("release");
        admit(&db.conn, input("t1", parent, child, Some("t0"), "next"))
            .await
            .expect("first successor");
        let different = input("t2", parent, child, Some("t0"), "different");
        let result = admit(&db.conn, different).await.expect("conflict");
        assert!(matches!(
            result,
            AdmissionResult::Conflict { next_task_id, .. } if next_task_id == "t1"
        ));
    }

    #[tokio::test]
    async fn physical_child_delete_cascades_ledger_row() {
        let db = fresh_in_memory_db().await;
        let (parent, child) = conversations(&db).await;
        admit(&db.conn, input("t0", parent, child, None, "first"))
            .await
            .expect("admit");
        assert!(delegation_task::Entity::find()
            .filter(delegation_task::Column::TaskId.eq("t0"))
            .one(&db.conn)
            .await
            .expect("ledger lookup")
            .is_some());

        conversation::Entity::delete_by_id(child)
            .exec(&db.conn)
            .await
            .expect("physical child delete");
        assert!(delegation_task::Entity::find()
            .filter(delegation_task::Column::TaskId.eq("t0"))
            .one(&db.conn)
            .await
            .expect("ledger lookup after cascade")
            .is_none());
    }

    #[tokio::test]
    async fn continuation_must_keep_the_source_session_binding() {
        let db = fresh_in_memory_db().await;
        let (parent, child) = conversations(&db).await;
        admit(&db.conn, input("t0", parent, child, None, "first"))
            .await
            .expect("admit source");
        finish(
            &db.conn,
            parent,
            "t0",
            &report("t0", child, "done", TaskStatus::Completed),
        )
        .await
        .expect("finish");
        mark_released(&db.conn, parent, "t0")
            .await
            .expect("release");
        let mut different_session = input("t1", parent, child, Some("t0"), "next");
        different_session.resume_binding.external_session_id = "other".into();
        let error = admit(&db.conn, different_session)
            .await
            .expect_err("different session must be refused");
        assert!(error.to_string().contains("binding"));
        assert!(successor(&db.conn, parent, "t0")
            .await
            .expect("successor lookup")
            .is_none());
    }

    #[tokio::test]
    async fn release_and_finish_are_independent_in_both_orders() {
        let db = fresh_in_memory_db().await;
        let (parent, child) = conversations(&db).await;
        for (task_id, finish_first) in [("t0", true), ("t1", false)] {
            admit(&db.conn, input(task_id, parent, child, None, "task"))
                .await
                .expect("admit");
            if finish_first {
                finish(
                    &db.conn,
                    parent,
                    task_id,
                    &report(task_id, child, "done", TaskStatus::Completed),
                )
                .await
                .expect("finish");
                mark_released(&db.conn, parent, task_id)
                    .await
                    .expect("release");
            } else {
                mark_released(&db.conn, parent, task_id)
                    .await
                    .expect("release");
                finish(
                    &db.conn,
                    parent,
                    task_id,
                    &report(task_id, child, "done", TaskStatus::Completed),
                )
                .await
                .expect("finish");
            }
            let entry = lookup(&db.conn, parent, task_id)
                .await
                .expect("lookup")
                .expect("entry");
            assert!(entry.released);
            assert_eq!(entry.status, TaskStatus::Completed);
        }
    }
}
