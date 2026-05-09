//! Background task that aggregates ACP events into a single `PetState`
//! stream consumed by the desktop pet renderer.
//!
//! Subscribes to the same broadcaster the lifecycle subscriber uses
//! (`acp://event` channel), maintains a small in-memory aggregate of
//! cross-connection signals, and pushes `pet://state` whenever the
//! computed state actually changes.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::acp::types::{AcpEvent, ConnectionStatus, EventEnvelope};
use crate::db::entities::conversation::ConversationStatus;
use crate::models::pet::PetState;
use crate::web::event_bridge::{emit_event, EventEmitter, WebEvent, WebEventBroadcaster};

/// Aggregate snapshot of cross-connection ACP signals, derived from the
/// stream of `AcpEvent`s. Pure data — `compute_pet_state` is the sole
/// source of truth for translating it into a `PetState`.
#[derive(Debug, Clone, Default)]
pub struct PetGlobalState {
    /// Connections currently in `Prompting` (an in-flight prompt is streaming).
    prompting: HashSet<String>,
    /// Connections currently in `Connected` (idle but reachable).
    connected: HashSet<String>,
    /// Connections in a terminal `Error` state. We treat any error event as
    /// authoritative even if a later `StatusChanged` clears it — Codex's
    /// `failed` row should briefly play, then the next event will reset it.
    erroring: HashSet<String>,
    /// Outstanding permission requests (request_id → connection_id). The
    /// presence of *any* outstanding permission flips the state to Review.
    pending_permissions: HashMap<String, String>,
    /// Conversations in `PendingReview`. A turn ended with output the user
    /// hasn't acknowledged.
    pending_reviews: HashSet<i32>,
}

impl PetGlobalState {
    pub fn apply(&mut self, env: &EventEnvelope) {
        let conn = &env.connection_id;
        match &env.payload {
            AcpEvent::StatusChanged { status } => match status {
                ConnectionStatus::Prompting => {
                    self.prompting.insert(conn.clone());
                    self.connected.insert(conn.clone());
                    self.erroring.remove(conn);
                }
                ConnectionStatus::Connected | ConnectionStatus::Connecting => {
                    self.prompting.remove(conn);
                    self.connected.insert(conn.clone());
                    self.erroring.remove(conn);
                }
                ConnectionStatus::Error => {
                    self.erroring.insert(conn.clone());
                    self.prompting.remove(conn);
                }
                ConnectionStatus::Disconnected => {
                    self.prompting.remove(conn);
                    self.connected.remove(conn);
                    self.erroring.remove(conn);
                    self.pending_permissions.retain(|_, cid| cid != conn);
                }
            },
            AcpEvent::Error { .. } => {
                self.erroring.insert(conn.clone());
            }
            AcpEvent::PermissionRequest { request_id, .. } => {
                self.pending_permissions
                    .insert(request_id.clone(), conn.clone());
            }
            AcpEvent::TurnComplete { .. } => {
                self.prompting.remove(conn);
                // PermissionRequest entries with stale request_ids are not
                // cleared here — the lifecycle of a single permission is
                // closed by the agent re-sending or the user responding,
                // both of which surface as ConversationStatusChanged or a
                // status flip. Leaving stale entries around briefly is
                // safer than clearing too eagerly.
            }
            AcpEvent::ConversationStatusChanged {
                conversation_id,
                status,
            } => match status {
                ConversationStatus::PendingReview => {
                    self.pending_reviews.insert(*conversation_id);
                }
                ConversationStatus::InProgress
                | ConversationStatus::Completed
                | ConversationStatus::Cancelled => {
                    self.pending_reviews.remove(conversation_id);
                }
            },
            _ => {}
        }
    }
}

/// Pure function: aggregate → state. Order of checks defines priority.
pub fn compute_pet_state(snapshot: &PetGlobalState) -> PetState {
    if !snapshot.erroring.is_empty() {
        return PetState::Failed;
    }
    if !snapshot.pending_permissions.is_empty() || !snapshot.pending_reviews.is_empty() {
        return PetState::Review;
    }
    if !snapshot.prompting.is_empty() {
        return PetState::Running;
    }
    if !snapshot.connected.is_empty() {
        return PetState::Waiting;
    }
    PetState::Idle
}

/// Spawn-friendly subscriber loop. Mirrors `lifecycle_subscriber_task`'s
/// "subscribe synchronously, return future" shape so the broadcast buffer
/// covers the gap between `subscribe()` and the first `recv()`.
pub fn pet_state_subscriber_task(
    broadcaster: Arc<WebEventBroadcaster>,
    emitter: EventEmitter,
) -> impl Future<Output = ()> + Send + 'static {
    let mut rx = broadcaster.subscribe();
    async move {
        let mut snapshot = PetGlobalState::default();
        let mut last_state = PetState::Idle;
        // Push an initial "idle" snapshot so the renderer doesn't start blank.
        emit_event(&emitter, "pet://state", last_state);

        loop {
            match rx.recv().await {
                Ok(WebEvent { channel, payload }) => {
                    if channel != "acp://event" {
                        continue;
                    }
                    let envelope: EventEnvelope = match serde_json::from_value((*payload).clone()) {
                        Ok(env) => env,
                        Err(_) => continue,
                    };
                    snapshot.apply(&envelope);
                    let next = compute_pet_state(&snapshot);
                    if next != last_state {
                        last_state = next;
                        emit_event(&emitter, "pet://state", next);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // We can't reliably reconstruct state after lagging — but
                    // it is recoverable: the next StatusChanged event will
                    // reseed the relevant fields. Conservatively reset the
                    // snapshot to Idle so we don't surface phantom errors.
                    snapshot = PetGlobalState::default();
                    if last_state != PetState::Idle {
                        last_state = PetState::Idle;
                        emit_event(&emitter, "pet://state", last_state);
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(id: &str, payload: AcpEvent) -> EventEnvelope {
        EventEnvelope {
            seq: 0,
            connection_id: id.to_string(),
            payload,
        }
    }

    #[test]
    fn idle_when_empty() {
        let s = PetGlobalState::default();
        assert_eq!(compute_pet_state(&s), PetState::Idle);
    }

    #[test]
    fn waiting_when_connected_but_not_prompting() {
        let mut s = PetGlobalState::default();
        s.apply(&env(
            "c1",
            AcpEvent::StatusChanged {
                status: ConnectionStatus::Connected,
            },
        ));
        assert_eq!(compute_pet_state(&s), PetState::Waiting);
    }

    #[test]
    fn running_overrides_waiting() {
        let mut s = PetGlobalState::default();
        s.apply(&env(
            "c1",
            AcpEvent::StatusChanged {
                status: ConnectionStatus::Connected,
            },
        ));
        s.apply(&env(
            "c1",
            AcpEvent::StatusChanged {
                status: ConnectionStatus::Prompting,
            },
        ));
        assert_eq!(compute_pet_state(&s), PetState::Running);
    }

    #[test]
    fn permission_pending_yields_review() {
        let mut s = PetGlobalState::default();
        s.apply(&env(
            "c1",
            AcpEvent::PermissionRequest {
                request_id: "r1".into(),
                tool_call: serde_json::json!({}),
                options: vec![],
            },
        ));
        assert_eq!(compute_pet_state(&s), PetState::Review);
    }

    #[test]
    fn error_dominates_everything() {
        let mut s = PetGlobalState::default();
        s.apply(&env(
            "c1",
            AcpEvent::StatusChanged {
                status: ConnectionStatus::Prompting,
            },
        ));
        s.apply(&env(
            "c1",
            AcpEvent::Error {
                message: "boom".into(),
                agent_type: "claude_code".into(),
                code: None,
            },
        ));
        assert_eq!(compute_pet_state(&s), PetState::Failed);
    }

    #[test]
    fn disconnect_clears_state() {
        let mut s = PetGlobalState::default();
        s.apply(&env(
            "c1",
            AcpEvent::StatusChanged {
                status: ConnectionStatus::Connected,
            },
        ));
        s.apply(&env(
            "c1",
            AcpEvent::StatusChanged {
                status: ConnectionStatus::Disconnected,
            },
        ));
        assert_eq!(compute_pet_state(&s), PetState::Idle);
    }
}
