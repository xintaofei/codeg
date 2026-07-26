//! HTTP handlers for delegation settings — the web-mode mirror of the
//! Tauri commands in `commands::delegation`.
//!
//! Both endpoints share the same core helpers (`load_delegation_settings`,
//! `set_delegation_settings_core`) so the clamp + persist + broker
//! re-apply behavior stays identical across transports.

use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::acp::delegation::broker::ContinuationAvailability;
use crate::acp::delegation::types::DelegationTaskReport;
use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::delegation::{
    close_delegation_session_core, continue_delegation_core, get_continuation_availability_core,
    load_delegation_settings, set_delegation_settings_core, DelegationSettings,
};

pub async fn get_delegation_settings(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<DelegationSettings>, AppCommandError> {
    Ok(Json(load_delegation_settings(&state.db.conn).await))
}

#[derive(Deserialize)]
pub struct SetDelegationSettingsParams {
    pub settings: DelegationSettings,
}

pub async fn set_delegation_settings(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SetDelegationSettingsParams>,
) -> Result<Json<DelegationSettings>, AppCommandError> {
    let saved =
        set_delegation_settings_core(&state.db.conn, &state.delegation_broker, params.settings)
            .await?;
    Ok(Json(saved))
}

// -------- User-side continuation entry (Task 5) ------------------------------
//
// The web-mode mirrors of the `continue_delegation` /
// `close_delegation_session` / `get_continuation_availability` Tauri
// commands. All three address the child session by its CONVERSATION id (D5)
// and share the `_core` helpers, so target resolution, refusal shapes, and
// broker dispatch stay identical across transports. Rejections ride the
// `DelegationTaskReport` shape (stable `error_code`), never an HTTP error —
// only a real infrastructure fault (DB down) surfaces as one.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueDelegationParams {
    pub child_conversation_id: i32,
    pub message: String,
    /// Caller-minted idempotency key — the frontend generates one per
    /// submission and REUSES it on retry (Requirement 2.13).
    pub continuation_id: String,
}

pub async fn continue_delegation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ContinueDelegationParams>,
) -> Result<Json<DelegationTaskReport>, AppCommandError> {
    let report = continue_delegation_core(
        &state.db.conn,
        &state.delegation_broker,
        params.child_conversation_id,
        params.message,
        params.continuation_id,
    )
    .await?;
    Ok(Json(report))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseDelegationSessionParams {
    pub child_conversation_id: i32,
    pub continuation_id: String,
}

pub async fn close_delegation_session(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CloseDelegationSessionParams>,
) -> Result<Json<DelegationTaskReport>, AppCommandError> {
    let report = close_delegation_session_core(
        &state.db.conn,
        &state.delegation_broker,
        params.child_conversation_id,
        params.continuation_id,
    )
    .await?;
    Ok(Json(report))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContinuationAvailabilityParams {
    pub child_conversation_id: i32,
}

pub async fn get_continuation_availability(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<GetContinuationAvailabilityParams>,
) -> Result<Json<ContinuationAvailability>, AppCommandError> {
    let availability = get_continuation_availability_core(
        &state.db.conn,
        &state.delegation_broker,
        params.child_conversation_id,
    )
    .await?;
    Ok(Json(availability))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::delegation::broker::ContinuationAvailability;
    use crate::acp::delegation::types::TaskStatus;
    use crate::db::service::{conversation_service, folder_service};
    use crate::models::AgentType;

    async fn state_for_test() -> (Arc<crate::app_state::AppState>, tempfile::TempDir) {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let state = crate::app_state::AppState::new_for_test(db, dir.path().to_path_buf());
        (Arc::new(state), dir)
    }

    /// Seed a REGULAR root conversation row (`parent_id` null, kind
    /// `Regular`) — the shape the user-side entry must refuse to drive
    /// through the delegation broker.
    async fn seed_root_conversation(state: &crate::app_state::AppState) -> i32 {
        let folder = folder_service::add_folder(&state.db.conn, "/w6-root")
            .await
            .expect("folder");
        conversation_service::create(
            &state.db.conn,
            folder.id,
            AgentType::ClaudeCode,
            Some("plain root".into()),
            None,
        )
        .await
        .expect("conversation")
        .id
    }

    /// 5.1 red · Requirement 4.6 / design acceptance negative ①: continuing a
    /// child conversation id that does not exist must answer status `Unknown`
    /// — the same verdict a foreign parent's task id gets — so the endpoint
    /// never discloses whether a conversation row exists.
    #[tokio::test]
    async fn continue_on_unknown_child_conversation_reports_unknown() {
        let (state, _dir) = state_for_test().await;
        let Json(report) = continue_delegation(
            Extension(state),
            Json(ContinueDelegationParams {
                child_conversation_id: 424_242,
                message: "hello again".into(),
                continuation_id: "cid-unknown-1".into(),
            }),
        )
        .await
        .expect("handler must not surface an HTTP error for an unknown id");
        assert_eq!(report.status, TaskStatus::Unknown);
        assert!(
            report.error_code.is_none(),
            "unknown ids answer with the Unknown status shape, not an error code"
        );
    }

    /// 5.1 red · design acceptance negative ②: a regular root conversation
    /// (`parent_id` null) is not a delegation subsession — continuing it must
    /// be refused with the stable `not_continuable` code.
    #[tokio::test]
    async fn continue_on_root_conversation_is_rejected() {
        let (state, _dir) = state_for_test().await;
        let root_id = seed_root_conversation(&state).await;
        let Json(report) = continue_delegation(
            Extension(state),
            Json(ContinueDelegationParams {
                child_conversation_id: root_id,
                message: "you are not a subagent".into(),
                continuation_id: "cid-root-1".into(),
            }),
        )
        .await
        .expect("rejection rides the report shape, not an HTTP error");
        assert_eq!(report.error_code.as_deref(), Some("not_continuable"));
        assert_eq!(report.status, TaskStatus::Failed);
    }

    /// 5.1 red: closing an unknown child conversation id gets the same
    /// non-disclosing `Unknown` verdict as continue.
    #[tokio::test]
    async fn close_on_unknown_child_conversation_reports_unknown() {
        let (state, _dir) = state_for_test().await;
        let Json(report) = close_delegation_session(
            Extension(state),
            Json(CloseDelegationSessionParams {
                child_conversation_id: 424_242,
                continuation_id: "cid-close-1".into(),
            }),
        )
        .await
        .expect("handler must not surface an HTTP error for an unknown id");
        assert_eq!(report.status, TaskStatus::Unknown);
    }

    /// 5.1 red: availability for an unknown id folds into `NotContinuable` —
    /// one verdict for "does not exist" and "cannot continue", so the query
    /// leaks nothing about row existence.
    #[tokio::test]
    async fn availability_on_unknown_child_conversation_is_not_continuable() {
        let (state, _dir) = state_for_test().await;
        let Json(availability) = get_continuation_availability(
            Extension(state),
            Json(GetContinuationAvailabilityParams {
                child_conversation_id: 424_242,
            }),
        )
        .await
        .expect("availability queries never surface an HTTP error for bad ids");
        assert_eq!(availability, ContinuationAvailability::NotContinuable);
    }

    /// 5.1 red: a regular root conversation has no continuation availability.
    #[tokio::test]
    async fn availability_on_root_conversation_is_not_continuable() {
        let (state, _dir) = state_for_test().await;
        let root_id = seed_root_conversation(&state).await;
        let Json(availability) = get_continuation_availability(
            Extension(state),
            Json(GetContinuationAvailabilityParams {
                child_conversation_id: root_id,
            }),
        )
        .await
        .expect("availability queries never surface an HTTP error");
        assert_eq!(availability, ContinuationAvailability::NotContinuable);
    }
}

/// Happy-path coverage for the HTTP handler seam — split from the refusal
/// tests above because it wires a full (mock-spawner) broker + real DB rows.
#[cfg(test)]
mod happy_path_tests {
    use std::sync::Arc;

    use super::*;
    use crate::acp::delegation::broker::{
        DbChildStatusLookup, DbDepthLookup, DelegationBroker, DelegationConfig,
    };
    use crate::acp::delegation::spawner::{mock::MockSpawner, ConnectionSpawner, DelegationLink};
    use crate::acp::delegation::types::{
        DelegationOutcome, DelegationRequest, DelegationSuccess, TaskStatus,
    };
    use crate::db::service::{conversation_service, folder_service};
    use crate::models::AgentType;
    use sea_orm::{ActiveModelTrait, Set};

    /// Happy path through the HTTP handler seam: a LIVE delegated child
    /// (settled task, kept-alive connection) is continued via the endpoint;
    /// the follow-up really reaches the (mock) agent connection, and the JSON
    /// response's field names + wire enum casing match the frontend TS mirror
    /// (`src/lib/api.ts` `interface DelegationTaskReport`) — the
    /// serialization seam broker-level tests never cross.
    #[tokio::test]
    async fn continue_on_live_session_happy_path_matches_ts_mirror_shape() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let conn = db.conn.clone();
        let db_arc = Arc::new(crate::db::AppDatabase { conn: conn.clone() });

        // Real DB rows: folder + root parent + delegate child. The child's
        // `delegation_call_id` is bound to the broker-minted task id below.
        let folder = folder_service::add_folder(&conn, "/w6-happy")
            .await
            .expect("folder");
        let parent_id = conversation_service::create(
            &conn,
            folder.id,
            AgentType::ClaudeCode,
            Some("parent".into()),
            None,
        )
        .await
        .expect("parent")
        .id;
        let child = conversation_service::create_with_delegation(
            &conn,
            folder.id,
            AgentType::ClaudeCode,
            Some("delegated child".into()),
            None,
            Some(DelegationLink {
                parent_conversation_id: parent_id,
                parent_tool_use_id: "pt-happy".into(),
                delegation_call_id: "pending-task-id".into(),
            }),
        )
        .await
        .expect("child");
        let child_id = child.id;

        // Broker over a mock spawner + the REAL DB-backed lookups (the same
        // wiring shape production uses).
        let mock = Arc::new(MockSpawner::new());
        let broker = Arc::new(
            DelegationBroker::new(
                mock.clone() as Arc<dyn ConnectionSpawner>,
                Arc::new(DbDepthLookup { db: db_arc.clone() }),
            )
            .with_status_lookup(Arc::new(DbChildStatusLookup { db: db_arc })),
        );
        broker
            .set_config(DelegationConfig {
                enabled: true,
                ..DelegationConfig::default()
            })
            .await;

        // Settle one full delegation → a live kept-alive child connection.
        mock.queue_spawn(Ok("child-conn-live".into())).await;
        mock.queue_send(Ok(child_id)).await;
        let ack = broker
            .start_delegation(DelegationRequest {
                parent_connection_id: "parent-conn".into(),
                parent_conversation_id: parent_id,
                parent_tool_use_id: "pt-happy".into(),
                agent_type: AgentType::ClaudeCode,
                task: "do x".into(),
                working_dir: None,
                requested_working_dir: None,
                external_handle: None,
            })
            .await;
        let task_id = ack.task_id.expect("running task carries an id");
        broker
            .complete_call(
                &task_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: child_id,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        // Bind the child row to the broker-minted task id (the D5 join key
        // `resolve_delegation_target` reads).
        let mut row: crate::db::entities::conversation::ActiveModel = child.into();
        row.delegation_call_id = Set(Some(task_id.clone()));
        row.update(&conn).await.expect("bind the call id");

        // AppState whose broker is the seeded one.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = crate::app_state::AppState::new_for_test(db, dir.path().to_path_buf());
        state.delegation_broker = broker;
        let state = Arc::new(state);

        mock.queue_followup(Ok(())).await;
        let Json(report) = continue_delegation(
            Extension(state),
            Json(ContinueDelegationParams {
                child_conversation_id: child_id,
                message: "next step".into(),
                continuation_id: "cid-happy-1".into(),
            }),
        )
        .await
        .expect("the happy path never surfaces an HTTP error");

        assert_eq!(report.status, TaskStatus::Running);
        assert_eq!(report.task_id.as_deref(), Some(task_id.as_str()));
        assert_eq!(report.child_conversation_id, Some(child_id));
        // The follow-up really reached the live agent connection with the
        // adopted child row + folder.
        let followups = mock.followups.lock().await;
        assert_eq!(followups.len(), 1, "exactly one follow-up dispatched");
        assert_eq!(followups[0].conn_id, "child-conn-live");
        assert_eq!(followups[0].message, "next step");
        assert_eq!(followups[0].conversation_id, child_id);
        assert_eq!(followups[0].folder_id, folder.id);
        drop(followups);

        // Serialization seam: every emitted key must exist in the TS mirror
        // (`src/lib/api.ts` interface DelegationTaskReport), and enums ride
        // the wire snake_case the mirror declares.
        let value = serde_json::to_value(&report).expect("serialize the report");
        let obj = value.as_object().expect("a JSON object");
        const TS_MIRROR_FIELDS: [&str; 8] = [
            "task_id",
            "status",
            "child_conversation_id",
            "agent_type",
            "text",
            "error_code",
            "message",
            "duration_ms",
        ];
        for key in obj.keys() {
            assert!(
                TS_MIRROR_FIELDS.contains(&key.as_str()),
                "serialized field `{key}` is missing from the src/lib/api.ts \
                 DelegationTaskReport mirror"
            );
        }
        assert_eq!(
            obj.get("status").and_then(|v| v.as_str()),
            Some("running"),
            "TaskStatus must serialize as the snake_case wire string"
        );
        assert!(obj.get("task_id").is_some_and(|v| v.is_string()));
        assert!(obj.get("child_conversation_id").is_some_and(|v| v.is_i64()));
    }
}
