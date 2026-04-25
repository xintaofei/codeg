use std::sync::atomic::{AtomicI64, Ordering};

use chrono::Utc;
use serde::Serialize;

use crate::models::squad::{SquadEvent, SquadRoleKind};
use crate::web::event_bridge::{emit_event, EventEmitter};

pub const SQUAD_EVENT_CHANNEL: &str = "squad://event";

static SEQ: AtomicI64 = AtomicI64::new(1);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleConnectionAttachedPayload {
    pub connection_id: String,
    pub agent_type: crate::models::agent::AgentType,
    pub working_dir: Option<String>,
    pub session_id: Option<String>,
}

pub fn emit_squad_event(
    emitter: &EventEmitter,
    event_type: impl Into<String>,
    squad_run_id: i32,
    role_kind: Option<SquadRoleKind>,
    payload: Option<serde_json::Value>,
) {
    emit_event(
        emitter,
        SQUAD_EVENT_CHANNEL,
        SquadEvent {
            event_type: event_type.into(),
            squad_run_id,
            seq: SEQ.fetch_add(1, Ordering::Relaxed),
            at: Utc::now(),
            role_kind,
            payload,
        },
    );
}

pub fn emit_payload<T: Serialize>(
    emitter: &EventEmitter,
    event_type: impl Into<String>,
    squad_run_id: i32,
    role_kind: Option<SquadRoleKind>,
    payload: &T,
) {
    let value = serde_json::to_value(payload).ok();
    emit_squad_event(emitter, event_type, squad_run_id, role_kind, value);
}
