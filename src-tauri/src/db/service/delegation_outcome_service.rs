//! Immutable storage for COMPLETED delegation results.
//!
//! Contract (v2 design §4.1):
//! * `insert_once` is first-writer-wins on the `task_id` primary key. A second
//!   write of the IDENTICAL success result is idempotent (`AlreadyIdentical`);
//!   a DIFFERENT success result never overwrites the winner (`Conflict`).
//!   There is deliberately no upsert/update path.
//! * Only success results exist here — the broker never hands us a canceled /
//!   failed terminal, and the table CHECK enforces `status = 'completed'`.
//! * `find_owned` is the ONLY read path: every lookup is scoped to the owning
//!   parent conversation id, so there is no unscoped "query by task id"
//!   endpoint to misuse. Query errors and "no row" are distinct results.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use sea_orm::sea_query::OnConflict;

use crate::db::entities::delegation_outcome;
use crate::db::error::DbError;

/// Bump only when the row's meaning changes in a way readers must detect.
pub const DELEGATION_OUTCOME_SCHEMA_VERSION: i32 = 1;

/// A successful delegation result to freeze. Constructed by the broker from the
/// outcome the current execution produced when it won the terminal race.
#[derive(Debug, Clone, PartialEq)]
pub struct DelegationOutcomeInsert {
    pub task_id: String,
    pub parent_conversation_id: i32,
    pub parent_tool_use_id: Option<String>,
    pub child_conversation_id: Option<i32>,
    /// Wire string of the target agent (e.g. `"claude_code"`).
    pub agent_type: String,
    /// Bounded result text (the broker already applied its UTF-8-safe cap).
    pub text: String,
    pub duration_ms: i64,
    pub text_truncated: bool,
    pub completed_at: chrono::DateTime<chrono::Utc>,
    /// Non-sensitive resume-binding JSON (external session id, canonical cwd,
    /// agent type, execution-config fingerprint) — `None` when the binding
    /// could not be verified at the success moment.
    pub resume_binding_json: Option<String>,
}

/// Result of a first-writer-wins insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeWriteResult {
    /// This call wrote the row — it is the winner.
    Inserted,
    /// A row already existed and carries the identical result.
    AlreadyIdentical,
    /// A row already existed with a DIFFERENT result; the original was kept.
    Conflict,
}

/// A frozen completed result as read back from the store.
#[derive(Debug, Clone, PartialEq)]
pub struct DelegationOutcomeRow {
    pub task_id: String,
    pub parent_conversation_id: i32,
    pub parent_tool_use_id: Option<String>,
    pub child_conversation_id: Option<i32>,
    pub agent_type: String,
    pub text: String,
    pub duration_ms: i64,
    pub text_truncated: bool,
    pub completed_at: chrono::DateTime<chrono::Utc>,
    pub schema_version: i32,
    pub resume_binding_json: Option<String>,
}

impl DelegationOutcomeRow {
    fn from_model(m: delegation_outcome::Model) -> Self {
        Self {
            task_id: m.task_id,
            parent_conversation_id: m.parent_conversation_id,
            parent_tool_use_id: m.parent_tool_use_id,
            child_conversation_id: m.child_conversation_id,
            agent_type: m.agent_type,
            text: m.text,
            duration_ms: m.duration_ms,
            text_truncated: m.text_truncated,
            completed_at: m.completed_at,
            schema_version: m.schema_version,
            resume_binding_json: m.resume_binding_json,
        }
    }
}

/// The result fields two writes of the SAME success must agree on. Metadata
/// (`completed_at`, `resume_binding_json`) is intentionally excluded: the
/// winner's values stand, and a later identical re-report with a differently
/// captured binding is still the same result.
fn same_result(row: &DelegationOutcomeRow, rec: &DelegationOutcomeInsert) -> bool {
    row.parent_conversation_id == rec.parent_conversation_id
        && row.parent_tool_use_id == rec.parent_tool_use_id
        && row.child_conversation_id == rec.child_conversation_id
        && row.agent_type == rec.agent_type
        && row.text == rec.text
        && row.duration_ms == rec.duration_ms
        && row.text_truncated == rec.text_truncated
}

fn active_model(rec: &DelegationOutcomeInsert) -> delegation_outcome::ActiveModel {
    delegation_outcome::ActiveModel {
        task_id: Set(rec.task_id.clone()),
        parent_conversation_id: Set(rec.parent_conversation_id),
        parent_tool_use_id: Set(rec.parent_tool_use_id.clone()),
        child_conversation_id: Set(rec.child_conversation_id),
        agent_type: Set(rec.agent_type.clone()),
        status: Set("completed".to_string()),
        text: Set(rec.text.clone()),
        duration_ms: Set(rec.duration_ms),
        text_truncated: Set(rec.text_truncated),
        completed_at: Set(rec.completed_at),
        schema_version: Set(DELEGATION_OUTCOME_SCHEMA_VERSION),
        resume_binding_json: Set(rec.resume_binding_json.clone()),
    }
}

/// Read the stored winner for `task_id` (unscoped — internal; every public
/// read path goes through [`find_owned`]).
async fn find_row(
    conn: &DatabaseConnection,
    task_id: &str,
) -> Result<Option<DelegationOutcomeRow>, DbError> {
    Ok(delegation_outcome::Entity::find_by_id(task_id)
        .one(conn)
        .await?
        .map(DelegationOutcomeRow::from_model))
}

/// First-writer-wins insert of a completed result. The DB primary key is the
/// only concurrency guard: on a conflict the stored winner is read back and
/// compared — never overwritten.
pub async fn insert_once(
    conn: &DatabaseConnection,
    rec: DelegationOutcomeInsert,
) -> Result<OutcomeWriteResult, DbError> {
    // The insert-level `.do_nothing()` wraps the result in `TryInsertResult`:
    // a fired conflict surfaces as `Conflicted` instead of an error, which is
    // exactly the "row already exists" signal we compare on. The primary key
    // alone decides the winner — no upsert, no update.
    let attempt = delegation_outcome::Entity::insert(active_model(&rec))
        .on_conflict(
            OnConflict::column(delegation_outcome::Column::TaskId)
                .do_nothing()
                .to_owned(),
        )
        .do_nothing()
        .exec(conn)
        .await;
    match attempt {
        // SeaORM reports a fired `DO NOTHING` conflict as `Conflicted` (and
        // `Empty` for a no-op statement) — either way a row already exists.
        Ok(
            sea_orm::TryInsertResult::Conflicted | sea_orm::TryInsertResult::Empty,
        ) => {
            let winner = find_row(conn, &rec.task_id)
                .await?
                .ok_or_else(|| {
                    DbError::Conflict(format!(
                        "delegation_outcome: conflict reported but no row for task {}",
                        rec.task_id
                    ))
                })?;
            if same_result(&winner, &rec) {
                Ok(OutcomeWriteResult::AlreadyIdentical)
            } else {
                Ok(OutcomeWriteResult::Conflict)
            }
        }
        Ok(sea_orm::TryInsertResult::Inserted(_)) => Ok(OutcomeWriteResult::Inserted),
        Err(e) => Err(e.into()),
    }
}

/// The frozen result for `task_id` IF it belongs to `parent_conversation_id`.
/// `Ok(None)` covers both "never frozen" and "frozen under a different parent"
/// — the latter must stay indistinguishable from the former to a non-owner.
pub async fn find_owned(
    conn: &DatabaseConnection,
    parent_conversation_id: i32,
    task_id: &str,
) -> Result<Option<DelegationOutcomeRow>, DbError> {
    Ok(delegation_outcome::Entity::find()
        .filter(delegation_outcome::Column::TaskId.eq(task_id))
        .filter(delegation_outcome::Column::ParentConversationId.eq(parent_conversation_id))
        .one(conn)
        .await?
        .map(DelegationOutcomeRow::from_model))
}
