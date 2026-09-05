//! DB access for collaboration sessions and turns (v2 design §4.2/§4.3).
//!
//! Every state transition the coordinator performs is a guarded UPDATE
//! (current state AND owning execution id in the WHERE clause) — a CAS, not a
//! read-modify-write. The single-active-turn invariant is additionally
//! enforced by the partial unique index, so concurrent admission fails at the
//! storage layer, not just by convention.

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};
use sea_orm::sea_query::{Expr, OnConflict};

use crate::db::entities::{collaboration_session, collaboration_turn};
use crate::db::error::DbError;

/// States that mark a turn as ACTIVE (not yet terminal). Kept in sync with
/// the partial unique index in the migration — a drift here would let two
/// active turns slip past the index.
pub const ACTIVE_TURN_STATES: [&str; 5] = [
    "accepted",
    "preparing",
    "dispatching",
    "running",
    "cancel_requested",
];

pub const TERMINAL_TURN_STATES: [&str; 5] = [
    "completed",
    "failed",
    "canceled",
    "interrupted",
    "outcome_unknown",
];

/// A new collaboration session row (created at first-turn acceptance).
pub struct NewSession {
    pub id: String,
    pub source_task_id: String,
    pub parent_conversation_id: i32,
    pub child_conversation_id: i32,
    pub resume_binding_json: String,
}

/// A new turn row (created at acceptance, before anything is dispatched).
pub struct NewTurn {
    pub id: String,
    pub session_id: String,
    pub ordinal: i32,
    pub request_id: String,
    pub message: String,
    pub initiator_parent_conversation_id: i32,
    pub initiator_tool_use_id: Option<String>,
}

pub type SessionRow = collaboration_session::Model;
pub type TurnRow = collaboration_turn::Model;

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// Create the session row unless one already exists for the source; either
/// way, return the row that now holds `source_task_id`. The UNIQUE
/// constraints make "one session per source / per child" a storage fact.
pub async fn upsert_session_once<C: ConnectionTrait>(
    conn: &C,
    new: NewSession,
) -> Result<SessionRow, DbError> {
    let active = collaboration_session::ActiveModel {
        id: Set(new.id),
        source_task_id: Set(new.source_task_id),
        parent_conversation_id: Set(new.parent_conversation_id),
        child_conversation_id: Set(new.child_conversation_id),
        state: Set("open".to_string()),
        resume_binding_json: Set(new.resume_binding_json),
        created_at: Set(now()),
        closed_at: Set(None),
    };
    let source_task_id = active.source_task_id.clone().unwrap();
    collaboration_session::Entity::insert(active)
        .on_conflict(
            OnConflict::column(collaboration_session::Column::SourceTaskId)
                .do_nothing()
                .to_owned(),
        )
        .do_nothing()
        .exec(conn)
        .await
        .map_err(DbError::from)?;
    find_session_by_source(conn, &source_task_id)
        .await?
        .ok_or_else(|| DbError::Conflict("collaboration_session vanished after upsert".into()))
}

pub async fn find_session_by_source<C: ConnectionTrait>(
    conn: &C,
    source_task_id: &str,
) -> Result<Option<SessionRow>, DbError> {
    collaboration_session::Entity::find()
        .filter(collaboration_session::Column::SourceTaskId.eq(source_task_id))
        .one(conn)
        .await
        .map_err(DbError::from)
}

pub async fn find_session_by_id<C: ConnectionTrait>(
    conn: &C,
    id: &str,
) -> Result<Option<SessionRow>, DbError> {
    collaboration_session::Entity::find_by_id(id)
        .one(conn)
        .await
        .map_err(DbError::from)
}

/// Whether the child conversation is currently RESERVED by an open/blocked
/// collaboration session — the reservation marker ordinary write paths must
/// check before sending anything to the child.
pub async fn child_is_reserved<C: ConnectionTrait>(
    conn: &C,
    child_conversation_id: i32,
) -> Result<bool, DbError> {
    let open = collaboration_session::Entity::find()
        .filter(collaboration_session::Column::ChildConversationId.eq(child_conversation_id))
        .filter(collaboration_session::Column::State.is_in(["open", "blocked"]))
        .count(conn)
        .await
        .map_err(DbError::from)?;
    Ok(open > 0)
}

/// Transition the session state (`open` ↔ `blocked` → `closed`). Sets
/// `closed_at` when closing. Idempotent for repeated closes of the same
/// target state.
pub async fn set_session_state<C: ConnectionTrait>(
    conn: &C,
    session_id: &str,
    state: &str,
) -> Result<(), DbError> {
    let mut active = collaboration_session::ActiveModel {
        id: Set(session_id.to_string()),
        state: Set(state.to_string()),
        ..Default::default()
    };
    if state == "closed" {
        active.closed_at = Set(Some(now()));
    }
    active.update(conn).await.map_err(DbError::from)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Turns
// ---------------------------------------------------------------------------

/// The next ordinal for a session (max + 1), or 1 for the first turn.
/// Sessions hold a handful of turns, so loading them is cheaper than fighting
/// SQL NULL decoding on an aggregate over an empty set.
pub async fn next_ordinal<C: ConnectionTrait>(conn: &C, session_id: &str) -> Result<i32, DbError> {
    let rows = collaboration_turn::Entity::find()
        .filter(collaboration_turn::Column::SessionId.eq(session_id))
        .all(conn)
        .await
        .map_err(DbError::from)?;
    Ok(rows.iter().map(|t| t.ordinal).max().unwrap_or(0) + 1)
}

/// Insert the accepted turn. Unique violations (same request_id or ordinal —
/// i.e. a concurrent admission that won the race) surface as a raw DbErr for
/// the coordinator to translate into domain errors.
pub async fn insert_turn<C: ConnectionTrait>(
    conn: &C,
    new: NewTurn,
) -> Result<TurnRow, DbError> {
    let active = collaboration_turn::ActiveModel {
        id: Set(new.id),
        session_id: Set(new.session_id),
        ordinal: Set(new.ordinal),
        request_id: Set(new.request_id),
        message: Set(new.message),
        initiator_kind: Set("parent_agent".to_string()),
        initiator_parent_conversation_id: Set(new.initiator_parent_conversation_id),
        initiator_tool_use_id: Set(new.initiator_tool_use_id),
        state: Set("accepted".to_string()),
        execution_id: Set(uuid::Uuid::new_v4().to_string()),
        connection_id: Set(None),
        result_text: Set(None),
        text_truncated: Set(false),
        error_code: Set(None),
        error_message: Set(None),
        created_at: Set(now()),
        started_at: Set(None),
        finished_at: Set(None),
        version: Set(1),
    };
    active.insert(conn).await.map_err(DbError::from)
}

pub async fn find_turn<C: ConnectionTrait>(conn: &C, turn_id: &str) -> Result<Option<TurnRow>, DbError> {
    collaboration_turn::Entity::find_by_id(turn_id)
        .one(conn)
        .await
        .map_err(DbError::from)
}

pub async fn find_turn_by_request<C: ConnectionTrait>(
    conn: &C,
    session_id: &str,
    request_id: &str,
) -> Result<Option<TurnRow>, DbError> {
    collaboration_turn::Entity::find()
        .filter(collaboration_turn::Column::SessionId.eq(session_id))
        .filter(collaboration_turn::Column::RequestId.eq(request_id))
        .one(conn)
        .await
        .map_err(DbError::from)
}

/// Turns of a session ordered by ordinal, for the read-only UI projection.
pub async fn list_turns<C: ConnectionTrait>(
    conn: &C,
    session_id: &str,
    after_ordinal: i32,
    limit: u64,
) -> Result<Vec<TurnRow>, DbError> {
    collaboration_turn::Entity::find()
        .filter(collaboration_turn::Column::SessionId.eq(session_id))
        .filter(collaboration_turn::Column::Ordinal.gt(after_ordinal))
        .order_by_asc(collaboration_turn::Column::Ordinal)
        .limit(limit)
        .all(conn)
        .await
        .map_err(DbError::from)
}

/// The session's active turn, if any (there can be at most one).
pub async fn active_turn<C: ConnectionTrait>(
    conn: &C,
    session_id: &str,
) -> Result<Option<TurnRow>, DbError> {
    collaboration_turn::Entity::find()
        .filter(collaboration_turn::Column::SessionId.eq(session_id))
        .filter(collaboration_turn::Column::State.is_in(ACTIVE_TURN_STATES))
        .one(conn)
        .await
        .map_err(DbError::from)
}

/// Record the diagnostic connection id on the turn (set after a successful
/// strict attach). Guarded by the execution id like every other write.
pub async fn set_turn_connection<C: ConnectionTrait>(
    conn: &C,
    turn_id: &str,
    execution_id: &str,
    connection_id: &str,
) -> Result<bool, DbError> {
    let update = collaboration_turn::Entity::update_many()
        .col_expr(
            collaboration_turn::Column::ConnectionId,
            Expr::value(connection_id.to_string()),
        )
        .filter(collaboration_turn::Column::Id.eq(turn_id))
        .filter(collaboration_turn::Column::ExecutionId.eq(execution_id))
        .exec(conn)
        .await
        .map_err(DbError::from)?;
    Ok(update.rows_affected > 0)
}

/// CAS the turn state forward. The update applies ONLY when the row is
/// currently in one of `from_states` AND (when `execution_id` is `Some`)
/// carries that execution identity. Returns `true` when this caller won the
/// transition; `version` is bumped on every applied write.
pub async fn cas_turn_state<C: ConnectionTrait>(
    conn: &C,
    turn_id: &str,
    execution_id: Option<&str>,
    from_states: &[&str],
    to_state: &str,
    started: bool,
) -> Result<bool, DbError> {
    let mut update = collaboration_turn::Entity::update_many()
        .col_expr(
            collaboration_turn::Column::State,
            Expr::value(to_state.to_string()),
        )
        .col_expr(
            collaboration_turn::Column::Version,
            Expr::col(collaboration_turn::Column::Version).add(1),
        )
        .filter(collaboration_turn::Column::Id.eq(turn_id))
        .filter(collaboration_turn::Column::State.is_in(from_states.to_vec()));
    if let Some(exec) = execution_id {
        update = update.filter(collaboration_turn::Column::ExecutionId.eq(exec));
    }
    if started {
        update = update.col_expr(
            collaboration_turn::Column::StartedAt,
            Expr::value(now()),
        );
    }
    let result = update.exec(conn).await.map_err(DbError::from)?;
    Ok(result.rows_affected > 0)
}

/// The full terminal write: CAS out of an active state into `to_state` and
/// store the outcome payload atomically. The FIRST caller to win fixes the
/// terminal state and result; later (stale) terminals return `false` and
/// never overwrite the winner.
pub struct SettlePayload {
    pub to_state: &'static str,
    pub result_text: Option<String>,
    pub text_truncated: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

pub async fn settle_turn<C: ConnectionTrait + TransactionTrait>(
    conn: &C,
    turn_id: &str,
    execution_id: &str,
    payload: SettlePayload,
) -> Result<bool, DbError> {
    let txn = conn.begin().await.map_err(DbError::from)?;
    let update = collaboration_turn::Entity::update_many()
        .col_expr(
            collaboration_turn::Column::State,
            Expr::value(payload.to_state.to_string()),
        )
        .col_expr(
            collaboration_turn::Column::ResultText,
            Expr::value(payload.result_text),
        )
        .col_expr(
            collaboration_turn::Column::TextTruncated,
            Expr::value(payload.text_truncated),
        )
        .col_expr(
            collaboration_turn::Column::ErrorCode,
            Expr::value(payload.error_code),
        )
        .col_expr(
            collaboration_turn::Column::ErrorMessage,
            Expr::value(payload.error_message),
        )
        .col_expr(
            collaboration_turn::Column::FinishedAt,
            Expr::value(now()),
        )
        .col_expr(
            collaboration_turn::Column::Version,
            Expr::col(collaboration_turn::Column::Version).add(1),
        )
        .filter(collaboration_turn::Column::Id.eq(turn_id))
        .filter(collaboration_turn::Column::ExecutionId.eq(execution_id))
        .filter(collaboration_turn::Column::State.is_in(ACTIVE_TURN_STATES))
        .exec(&txn)
        .await
        .map_err(DbError::from)?;
    let applied = update.rows_affected > 0;
    txn.commit().await.map_err(DbError::from)?;
    Ok(applied)
}

/// Startup recovery scan (v2 design §5.2). Returns the recovered turns:
/// * accepted / preparing → `interrupted` (host restarted before dispatch),
/// * dispatching / running / cancel_requested → `outcome_unknown` AND the
///   session becomes `blocked` (a send MAY have happened; the host can't
///   prove what the agent did, so nothing is re-sent and the session stops
///   accepting new turns until a human closes it).
///
/// Never re-sends anything; never invents outcomes.
#[derive(Debug)]
pub struct RecoverySummary {
    pub interrupted: Vec<String>,
    pub outcome_unknown: Vec<String>,
    pub blocked_sessions: Vec<String>,
}

pub async fn recover_on_startup<C: ConnectionTrait + TransactionTrait>(
    conn: &C,
) -> Result<RecoverySummary, DbError> {
    let txn = conn.begin().await.map_err(DbError::from)?;
    let mut summary = RecoverySummary {
        interrupted: Vec::new(),
        outcome_unknown: Vec::new(),
        blocked_sessions: Vec::new(),
    };

    let pre_dispatch = collaboration_turn::Entity::find()
        .filter(collaboration_turn::Column::State.is_in(["accepted", "preparing"]))
        .all(&txn)
        .await
        .map_err(DbError::from)?;
    for turn in pre_dispatch {
        let applied = cas_turn_state(
            &txn,
            &turn.id,
            None,
            &["accepted", "preparing"],
            "interrupted",
            false,
        )
        .await?;
        if applied {
            summary.interrupted.push(turn.id);
        }
    }

    let mid_flight = collaboration_turn::Entity::find()
        .filter(
            collaboration_turn::Column::State
                .is_in(["dispatching", "running", "cancel_requested"]),
        )
        .all(&txn)
        .await
        .map_err(DbError::from)?;
    for turn in mid_flight {
        let applied = cas_turn_state(
            &txn,
            &turn.id,
            None,
            &["dispatching", "running", "cancel_requested"],
            "outcome_unknown",
            false,
        )
        .await?;
        if applied {
            summary.outcome_unknown.push(turn.id);
            if !summary.blocked_sessions.contains(&turn.session_id) {
                summary.blocked_sessions.push(turn.session_id.clone());
            }
        }
    }

    for session_id in &summary.blocked_sessions {
        // Only block sessions that are not already closed.
        if let Some(session) =
            collaboration_session::Entity::find_by_id(session_id).one(&txn).await.map_err(DbError::from)?
        {
            if session.state != "closed" {
                set_session_state(&txn, session_id, "blocked").await?;
            }
        }
    }

    txn.commit().await.map_err(DbError::from)?;
    Ok(summary)
}

/// Whether the child conversation row still exists and is not soft-deleted.
pub async fn child_conversation_alive<C: ConnectionTrait>(
    conn: &C,
    child_conversation_id: i32,
) -> Result<bool, DbError> {
    use crate::db::entities::conversation;
    let alive = conversation::Entity::find_by_id(child_conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .count(conn)
        .await
        .map_err(DbError::from)?;
    Ok(alive > 0)
}
