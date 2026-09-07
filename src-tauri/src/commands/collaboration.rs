//! Read-only collaboration-session snapshot (v2 design §6): shared core for
//! the Tauri command and the HTTP handler. UI reads NEVER operate on the
//! agent — they only project stored turns through the coordinator's scoped
//! queries.

use crate::acp::delegation::continuation::{
    ContinuationCoordinator, SchemaVersion1, SessionSummary, TurnReport,
};
use crate::app_error::AppCommandError;

/// Pagination caps (v2 design §6): default 20, hard ceiling 100.
pub const COLLAB_SNAPSHOT_DEFAULT_LIMIT: u32 = 20;
pub const COLLAB_SNAPSHOT_MAX_LIMIT: u32 = 100;

/// One page of the collaboration session for a source task. `session` is
/// `None` when the source has no collaboration relationship (the UI renders
/// "no rework rounds yet"), or when the caller is not the owning parent —
/// the two are indistinguishable by design (no existence leak).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CollaborationSnapshot {
    pub schema_version: SchemaVersion1,
    pub session: Option<SessionSummary>,
    /// Turns ordered by ordinal. ALWAYS includes the session's active turn
    /// (never hidden behind pagination) so a poll never leaves it stuck.
    pub turns: Vec<TurnReport>,
    /// `Some(after)` when more history pages exist (feed back as
    /// `after_ordinal`); `None` when the caller has reached the oldest turn.
    pub next_after_ordinal: Option<i32>,
}

/// The shared read path. `parent_conversation_id` scopes EVERY query — a
/// foreign parent gets the same empty snapshot as "no session".
pub async fn get_collaboration_session_core(
    coordinator: &ContinuationCoordinator,
    parent_conversation_id: i32,
    source_task_id: &str,
    after_ordinal: i32,
    limit: Option<u32>,
) -> Result<CollaborationSnapshot, AppCommandError> {
    let limit = limit.unwrap_or(COLLAB_SNAPSHOT_DEFAULT_LIMIT).min(COLLAB_SNAPSHOT_MAX_LIMIT);
    let parent = crate::acp::delegation::continuation::VerifiedParent {
        conversation_id: parent_conversation_id,
    };

    // The session summary for the source (ownership-checked inside).
    let session = coordinator
        .session_summary_for_source(parent, source_task_id)
        .await
        .map_err(app_error)?;

    let Some(session) = session else {
        return Ok(CollaborationSnapshot {
            schema_version: SchemaVersion1,
            session: None,
            turns: Vec::new(),
            next_after_ordinal: None,
        });
    };

    // Load one page after `after_ordinal`.
        let turns_page = coordinator
        .list_turns_for_session(parent, &session.session_id, after_ordinal, limit as u64)
        .await
        .map_err(app_error)?
        .unwrap_or_default();

    // The history cursor is derived from the HISTORICAL page ALONE
    // (acceptance F10): appending the active round below must not change
    // whether more history exists, or pages 21..N become unreachable.
    let next_after_ordinal = if turns_page.len() as u64 == limit as u64 {
        turns_page.last().map(|t| t.ordinal)
    } else {
        None
    };

    let mut turns = turns_page;
    // The active turn must never be paginated away: if it exists and falls
    // outside this page, fetch it separately and append (it does not affect
    // the cursor).
    let active_missing = coordinator
        .active_turn_of_session(&session.session_id)
        .await
        .map_err(app_error)?
        .filter(|active| !turns.iter().any(|t| t.turn_id == active.turn_id));
    if let Some(active) = active_missing {
        // The projection carries an empty source id (the caller joins via the
        // session summary); keep the page rows' value for consistency.
        let mut active = active;
        active.source_task_id = session.source_task_id.clone();
        turns.push(active);
        turns.sort_by_key(|t| t.ordinal);
    }

    Ok(CollaborationSnapshot {
        schema_version: SchemaVersion1,
        session: Some(session),
        turns,
        next_after_ordinal,
    })
}

fn app_error(e: crate::acp::delegation::continuation::ContinuationError) -> AppCommandError {
    AppCommandError::task_execution_failed(e.to_string())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
#[allow(unused_variables)]
pub async fn get_collaboration_session(
    #[cfg(feature = "tauri-runtime")] coordinator: tauri::State<
        '_,
        std::sync::Arc<ContinuationCoordinator>,
    >,
    parent_conversation_id: i32,
    source_task_id: String,
    after_ordinal: Option<i32>,
    limit: Option<u32>,
) -> Result<CollaborationSnapshot, AppCommandError> {
    #[cfg(feature = "tauri-runtime")]
    {
        get_collaboration_session_core(
            coordinator.inner(),
            parent_conversation_id,
            &source_task_id,
            after_ordinal.unwrap_or(0),
            limit,
        )
        .await
    }
    #[cfg(not(feature = "tauri-runtime"))]
    {
        let _ = (parent_conversation_id, source_task_id, after_ordinal, limit);
        Err(AppCommandError::configuration_invalid("tauri-only command"))
    }
}
