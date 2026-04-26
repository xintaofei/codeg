//! Subscribes to ACP events on the broadcast bus and turns them into
//! squad pipeline actions.
//!
//! Why not call dispatcher directly from `acp::connection`? The ACP
//! conversation loop has no notion of squad roles — all it knows is a
//! `connection_id`. Wiring squad-specific bookkeeping into that loop
//! would couple unrelated subsystems on a hot path. Instead we
//! eavesdrop on the existing event broadcast and react out-of-band:
//!
//!   - `content_delta` / `thinking` text is accumulated per connection.
//!   - `plan_update` snapshots the latest plan per connection.
//!   - `turn_complete` / `turn_idle_timeout` resolves the connection
//!     to a squad role; if it matches one, fan out to:
//!       * `dispatcher::record_turn_artifacts` (Summary + Plan)
//!       * for the Conductor role: `apply_conductor_output`, then
//!         `dispatch_pending_tasks` to fan tasks out to other roles
//!       * for any other role: `dispatch_pending_tasks` is still
//!         re-run because a worker may have just unblocked dependents.
//!
//! Failure modes are logged but never propagated — losing a single
//! turn capture is not worth taking down the listener task.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::acp::manager::ConnectionManager;
use crate::db::service::squad_service;
use crate::db::AppDatabase;
use crate::models::squad::SquadRoleKind;
use crate::squad::dispatcher;
use crate::web::event_bridge::{EventEmitter, WebEventBroadcaster};

/// Buffer of streaming text + plan accumulated for a single ACP turn,
/// keyed by `connection_id`.
#[derive(Default)]
struct TurnBuffer {
    /// Concatenated agent text content (`content_delta` events).
    text: String,
    /// Latest plan snapshot, serialized as JSON. We replace rather than
    /// append because `plan_update` events carry the full plan each
    /// time, not deltas.
    plan_json: Option<String>,
}

#[derive(Default)]
struct BufferStore {
    buffers: HashMap<String, TurnBuffer>,
}

impl BufferStore {
    fn append_text(&mut self, conn_id: &str, text: &str) {
        let entry = self.buffers.entry(conn_id.to_string()).or_default();
        entry.text.push_str(text);
    }

    fn set_plan(&mut self, conn_id: &str, plan_json: String) {
        let entry = self.buffers.entry(conn_id.to_string()).or_default();
        entry.plan_json = Some(plan_json);
    }

    fn take(&mut self, conn_id: &str) -> Option<TurnBuffer> {
        self.buffers.remove(conn_id)
    }
}

/// Spawn the long-running ACP→squad bridge. Idempotent if you only
/// call it once per `AppState`. Returns immediately; the listener
/// runs detached on a Tokio task.
pub fn spawn(
    db: AppDatabase,
    manager: ConnectionManager,
    emitter: EventEmitter,
    broadcaster: Arc<WebEventBroadcaster>,
) {
    let store: Arc<Mutex<BufferStore>> = Arc::new(Mutex::new(BufferStore::default()));
    let mut rx = broadcaster.subscribe();

    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!(
                        "[squad/turn_listener] broadcast lag: dropped {n} event(s); continuing"
                    );
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    eprintln!("[squad/turn_listener] broadcast closed; exiting");
                    break;
                }
            };

            // Only ACP events carry per-connection turn signals.
            if event.channel != "acp://event" {
                continue;
            }
            handle_acp_event(&db, &manager, &emitter, &store, &event.payload).await;
        }
    });
}

async fn handle_acp_event(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    store: &Arc<Mutex<BufferStore>>,
    payload: &serde_json::Value,
) {
    let Some(event_type) = payload.get("type").and_then(|v| v.as_str()) else {
        return;
    };
    let Some(conn_id) = payload.get("connection_id").and_then(|v| v.as_str()) else {
        return;
    };

    match event_type {
        "content_delta" | "thinking" => {
            if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
                store.lock().await.append_text(conn_id, text);
            }
        }
        "plan_update" => {
            if let Some(entries) = payload.get("entries") {
                if let Ok(plan_json) = serde_json::to_string(entries) {
                    store.lock().await.set_plan(conn_id, plan_json);
                }
            }
        }
        "turn_complete" | "turn_idle_timeout" => {
            // Drain the buffer regardless of whether this connection
            // turns out to belong to a squad — keeping it around would
            // leak memory across long-lived chats.
            let buffer = store.lock().await.take(conn_id).unwrap_or_default();
            on_turn_finished(db, manager, emitter, conn_id, buffer).await;
        }
        _ => {}
    }
}

async fn on_turn_finished(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    conn_id: &str,
    buffer: TurnBuffer,
) {
    // Quick reject: is this connection bound to a squad role at all?
    let role_run = match squad_service::find_role_run_by_connection_id(&db.conn, conn_id).await {
        Ok(Some(role)) => role,
        Ok(None) => return, // Not a squad connection. Common case.
        Err(err) => {
            eprintln!("[squad/turn_listener] DB lookup failed for {conn_id}: {err}");
            return;
        }
    };

    let run_id = role_run.squad_run_id;
    let role_kind = role_run.role_kind;
    let TurnBuffer { text, plan_json } = buffer;

    // Persist the turn artifacts (Summary + Plan). Always attempt this
    // first so a partial failure of the conductor pipeline below
    // doesn't drop the captured text.
    if !text.trim().is_empty() || plan_json.is_some() {
        if let Err(err) =
            dispatcher::record_turn_artifacts(db, emitter, run_id, role_kind, None, text.clone(), plan_json)
                .await
        {
            eprintln!(
                "[squad/turn_listener] record_turn_artifacts failed run={run_id} role={role_kind:?}: {err}"
            );
        }
    }

    // Conductor turns may carry a fresh task plan; parse + persist.
    if matches!(role_kind, SquadRoleKind::Conductor) && !text.trim().is_empty() {
        match dispatcher::apply_conductor_output(db, emitter, run_id, &text).await {
            Ok(result) => {
                if !result.skipped_reasons.is_empty() {
                    eprintln!(
                        "[squad/turn_listener] conductor plan applied run={run_id} created={} skipped={:?}",
                        result.created_tasks.len(),
                        result.skipped_reasons
                    );
                }
            }
            Err(err) => {
                eprintln!(
                    "[squad/turn_listener] apply_conductor_output failed run={run_id}: {err}"
                );
            }
        }
    }

    // Any role completing a turn may have unblocked dependents — try
    // to dispatch the next round. Idempotent: tasks already in flight
    // are reported as `AlreadyInFlight` and not re-prompted.
    if let Err(err) = dispatcher::dispatch_pending_tasks(db, manager, emitter, run_id).await {
        eprintln!(
            "[squad/turn_listener] dispatch_pending_tasks failed run={run_id}: {err}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_appends_text_and_replaces_plan() {
        let mut store = BufferStore::default();
        store.append_text("conn-1", "hello ");
        store.append_text("conn-1", "world");
        store.set_plan("conn-1", r#"[{"id":1}]"#.into());
        store.set_plan("conn-1", r#"[{"id":2}]"#.into());
        let buf = store.take("conn-1").unwrap();
        assert_eq!(buf.text, "hello world");
        assert_eq!(buf.plan_json.as_deref(), Some(r#"[{"id":2}]"#));
    }

    #[test]
    fn buffer_take_clears_entry() {
        let mut store = BufferStore::default();
        store.append_text("conn-1", "x");
        assert!(store.take("conn-1").is_some());
        assert!(store.take("conn-1").is_none());
    }

    #[test]
    fn buffer_per_connection_isolation() {
        let mut store = BufferStore::default();
        store.append_text("a", "alpha");
        store.append_text("b", "beta");
        let a = store.take("a").unwrap();
        let b = store.take("b").unwrap();
        assert_eq!(a.text, "alpha");
        assert_eq!(b.text, "beta");
    }

    #[test]
    fn buffer_take_missing_returns_none() {
        let mut store = BufferStore::default();
        assert!(store.take("never-seen").is_none());
    }
}
