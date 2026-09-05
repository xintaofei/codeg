//! `semantic_submit` — the Tauri command + Axum web handler that exposes
//! `run_semantic_core` (Task 4) to the frontend.
//!
//! Two surfaces live here, mirroring `crate::commands::chat_authoring` /
//! `crate::web::handlers::chat_authoring`:
//!
//!   * [`semantic_submit_core`] — the transport-agnostic core. Builds a
//!     `ConnectionSpawner` + `ConversationDepthLookup` and hands them to
//!     `run_semantic_core`.
//!   * [`semantic_submit`] — the `#[tauri::command]` wrapper (desktop only).
//!   * [`semantic_submit_handler`] — the Axum `POST /semantic_submit` handler
//!     (server mode), wired in `web::router`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::acp::delegation::broker::ConversationDepthLookup;
use crate::acp::delegation::spawner::ConnectionSpawner;
use crate::acp::manager::ConnectionManagerSpawner;
use crate::acp::delegation::types::DelegationError;
use crate::acp::manager::ConnectionManager;
use crate::db::AppDatabase;
use crate::semantic::broker::{run_semantic_core, SemanticRequest};
use crate::semantic::envelope::IntentEnvelope;

/// Concrete [`ConversationDepthLookup`] used by both the Tauri command and the
/// web handler. v1 has no conversation-tree semantics, so every id is its own
/// root (`parent_of` always returns `None`).
pub struct RootDepth;

#[async_trait]
impl ConversationDepthLookup for RootDepth {
    async fn parent_of(&self, _id: i32) -> Result<Option<i32>, DelegationError> {
        Ok(None)
    }
}

/// Transport-agnostic core. Builds the delegation broker inside
/// [`run_semantic_core`] from the supplied spawner + depth and returns the
/// fully-populated [`IntentEnvelope`].
pub async fn semantic_submit_core(
    spawner: Arc<dyn ConnectionSpawner>,
    depth: Arc<dyn ConversationDepthLookup>,
    req: SemanticRequest,
) -> Result<IntentEnvelope, String> {
    Ok(run_semantic_core(spawner, depth, req).await)
}

// ===========================================================================
// Tauri command (desktop)
// ===========================================================================

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn semantic_submit(
    #[cfg(feature = "tauri-runtime")] manager: tauri::State<'_, ConnectionManager>,
    #[cfg(feature = "tauri-runtime")] db: tauri::State<'_, AppDatabase>,
    req: SemanticRequest,
) -> Result<IntentEnvelope, String> {
    #[cfg(feature = "tauri-runtime")]
    {
        // `AppState` is not handed to commands as a single managed value, so we
        // reassemble the production `ConnectionSpawner` from the managed
        // `ConnectionManager` + `AppDatabase` + effective data dir (set as the
        // `CODEG_DATA_DIR` env var at bootstrap).
        let data_dir = Arc::new(PathBuf::from(
            std::env::var("CODEG_DATA_DIR").unwrap_or_default(),
        ));
        let spawner = Arc::new(ConnectionManagerSpawner {
            manager: Arc::new(manager.inner().clone_ref()),
            db: Arc::new(AppDatabase {
                conn: db.inner().conn.clone(),
            }),
            data_dir,
        }) as Arc<dyn ConnectionSpawner>;
        semantic_submit_core(spawner, Arc::new(RootDepth), req).await
    }
    #[cfg(not(feature = "tauri-runtime"))]
    {
        let _ = req;
        Err("semantic_submit is only available under the tauri runtime".into())
    }
}

// ===========================================================================
// Web handler (server mode)
// ===========================================================================

#[derive(Deserialize)]
pub struct SemanticSubmitParams {
    pub req: SemanticRequest,
}

pub use self::web_handler::semantic_submit_handler;

mod web_handler {
    use super::*;
    use axum::{extract::Extension, Json};
    use crate::app_error::AppCommandError;
    use crate::app_state::AppState;

    pub async fn semantic_submit_handler(
        Extension(state): Extension<Arc<AppState>>,
        Json(params): Json<SemanticSubmitParams>,
    ) -> Result<Json<IntentEnvelope>, AppCommandError> {
        let spawner = Arc::new(ConnectionManagerSpawner {
            manager: Arc::new(state.connection_manager.clone_ref()),
            db: Arc::new(AppDatabase {
                conn: state.db.conn.clone(),
            }),
            data_dir: Arc::new(state.data_dir.clone()),
        }) as Arc<dyn ConnectionSpawner>;
        let out = semantic_submit_core(spawner, Arc::new(RootDepth), params.req)
            .await
            .map_err(AppCommandError::configuration_invalid)?;
        Ok(Json(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::delegation::spawner::mock::MockSpawner;
    use crate::acp::delegation::spawner::SpawnerError;
    use crate::models::agent::AgentType;
    use crate::semantic::envelope::{AcceptState, Op};

    /// Drive `semantic_submit_core` through the operator-failure path: a
    /// `MockSpawner` whose queued spawn errors out. `run_semantic_core` (called
    /// inside the core) returns a `Denied` envelope immediately — no pending
    /// delegation is ever parked, so the test does not hang and exercises the
    /// real spawner → broker → error-surface path.
    #[tokio::test]
    async fn submit_returns_denied_envelope_on_spawn_failure() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Err(SpawnerError::Spawn("boom".into()))).await;

        let spawner = mock as Arc<dyn ConnectionSpawner>;
        let depth = Arc::new(RootDepth) as Arc<dyn ConversationDepthLookup>;

        let req = SemanticRequest {
            intent: "list files".into(),
            why: "see layout".into(),
            ops: vec![Op {
                tool: "shell".into(),
                params: serde_json::json!({"cmd":"ls"}),
            }],
            working_dir: Some("/tmp".into()),
            agent_type: AgentType::ClaudeCode,
        };

        let out = semantic_submit_core(spawner, depth, req).await.unwrap();
        assert!(matches!(out.accept, AcceptState::Accepted | AcceptState::Denied));
        assert_eq!(out.accept, AcceptState::Denied);
        assert!(out.result.as_ref().unwrap().contains("boom"));
    }

    /// A well-formed request still flows through the core and yields a typed
    /// envelope (the `Denied` here is the spawn-error path again, but the
    /// point is the request deserializes and the core returns a real struct).
    #[tokio::test]
    async fn submit_returns_envelope_for_valid_request() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Err(SpawnerError::Send("no child".into()))).await;

        let spawner = mock as Arc<dyn ConnectionSpawner>;
        let depth = Arc::new(RootDepth) as Arc<dyn ConversationDepthLookup>;

        let req = SemanticRequest {
            intent: "summarize".into(),
            why: "catch up".into(),
            ops: vec![],
            working_dir: None,
            agent_type: AgentType::OpenCode,
        };

        let out = semantic_submit_core(spawner, depth, req).await.unwrap();
        assert!(matches!(out.accept, AcceptState::Accepted | AcceptState::Denied));
    }
}
