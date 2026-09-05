//! Read-only collaboration-session query (v2 design §6): the web-mode mirror
//! of the Tauri `get_collaboration_session` command. Both wrap the SAME
//! coordinator core (`commands::collaboration`), so pagination, parent
//! scoping, and DTO semantics cannot drift across transports.
//!
//! Auth note: the web service runs behind the global bearer-token middleware
//! (`auth::require_token`), the same single-operator trust model every other
//! HTTP handler here uses (mirroring `list_child_conversations`, which also
//! takes an explicit parent id over the authenticated channel). There is
//! deliberately NO unauthenticated write path — continuation writes exist
//! only via the MCP listener's per-launch token.

use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::collaboration::{self, CollaborationSnapshot};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCollaborationSessionParams {
    pub source_task_id: String,
    #[serde(default)]
    pub after_ordinal: i32,
    pub limit: Option<u32>,
    /// The parent conversation that owns the source. Resolved over the
    /// bearer-authenticated channel; the coordinator re-scopes every read by
    /// it, so a wrong id yields the same empty snapshot as an unknown source.
    pub parent_conversation_id: i32,
}

pub async fn get_collaboration_session(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<GetCollaborationSessionParams>,
) -> Result<Json<CollaborationSnapshot>, AppCommandError> {
    let snapshot = collaboration::get_collaboration_session_core(
        &state.continuation_coordinator,
        params.parent_conversation_id,
        &params.source_task_id,
        params.after_ordinal,
        params.limit,
    )
    .await?;
    Ok(Json(snapshot))
}
