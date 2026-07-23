//! `DelegationMetaWriter` — broker capability that attaches the live
//! delegation state onto the parent's active `delegate_to_agent`
//! tool-call. The shape written under `meta["codeg.delegation"]`
//! follows the convention documented at
//! [`crate::acp::session_state::ToolCallState::meta`].
//!
//! The broker calls this at three lifecycle points:
//!
//! 1. After `send_prompt_linked_for_delegation` returns Ok — sets
//!    `status: "running"` with the child's connection / conversation ids.
//! 2. In `complete_call` — sets `status: "completed"` (ok branch) or
//!    `status: "failed"` + `error_code` (err branch).
//! 3. In `cancel_by_parent` / `cancel_by_child_connection` — sets
//!    `status: "failed"` + `error_code: "canceled"`.
//!
//! Writes are skipped when the broker is operating on a synthetic
//! `parent_tool_use_id` (the `"delegation-*"` UUID fallback) because
//! there's no matching ACP `tool_call_id` to attach meta to. The
//! frontend's snapshot path will still recover via `parseInput(input)`.

use async_trait::async_trait;
use std::sync::Arc;

use crate::acp::manager::ConnectionManager;
use crate::acp::types::AcpEvent;
use crate::web::event_bridge::emit_with_state;

/// Top-level key under which delegation state lives on a tool call's
/// `meta` object. Single source of truth — both the writer and the
/// frontend reader must spell it the same way.
pub const DELEGATION_META_KEY: &str = "codeg.delegation";

/// Capability the broker uses to patch `meta["codeg.delegation"]` on
/// the parent connection's active `delegate_to_agent` tool call.
///
/// Errors are swallowed at the impl boundary: a missing parent
/// connection (e.g. user disconnected mid-delegation) or a stale
/// tool_use_id (e.g. parent turn already wrapped up) must not derail
/// the rest of the broker lifecycle, which still has to disconnect the
/// child and resolve the pending call.
#[async_trait]
pub trait DelegationMetaWriter: Send + Sync {
    async fn write_meta(
        &self,
        parent_connection_id: &str,
        parent_tool_use_id: &str,
        meta: serde_json::Value,
    );

    /// Restore a tool call's lost identity: rewrite its `title` and
    /// `raw_input` on the live `ToolCallState`. Used for calls the host
    /// announced identity-less (Cursor's `"MCP: tool"` with an empty input —
    /// the wire never re-sends title/arguments), once the companion
    /// round-trip reveals which codeg-mcp tool the call actually is and with
    /// what arguments. Default no-op so `NoopMetaWriter` and mocks that don't
    /// observe identity writes stay unchanged.
    async fn write_tool_call_identity(
        &self,
        _parent_connection_id: &str,
        _tool_call_id: &str,
        _title: &str,
        _raw_input: serde_json::Value,
    ) {
    }
}

/// Default writer used when the broker is constructed via the
/// short-form `DelegationBroker::new` (most test callsites). Silently
/// drops every write — the broker's correctness is observable through
/// its outcomes and pending-call accounting, not through meta emits.
#[derive(Default, Clone)]
pub struct NoopMetaWriter;

#[async_trait]
impl DelegationMetaWriter for NoopMetaWriter {
    async fn write_meta(
        &self,
        _parent_connection_id: &str,
        _parent_tool_use_id: &str,
        _meta: serde_json::Value,
    ) {
    }
}

/// Production impl backed by `ConnectionManager`. Emits an
/// `AcpEvent::ToolCallUpdate` carrying only the `meta` field so the
/// existing `apply_tool_call_update` merge path (partial-update
/// preservation of locations / images / content / etc.) is reused
/// without duplicating the patch logic.
#[derive(Clone)]
pub struct ConnectionManagerMetaWriter {
    pub manager: Arc<ConnectionManager>,
}

#[async_trait]
impl DelegationMetaWriter for ConnectionManagerMetaWriter {
    async fn write_meta(
        &self,
        parent_connection_id: &str,
        parent_tool_use_id: &str,
        meta: serde_json::Value,
    ) {
        let Some((state_arc, emitter)) = self
            .manager
            .get_state_and_emitter(parent_connection_id)
            .await
        else {
            return;
        };
        emit_with_state(
            &state_arc,
            &emitter,
            AcpEvent::ToolCallUpdate {
                tool_call_id: parent_tool_use_id.to_string(),
                title: None,
                status: None,
                content: None,
                raw_input: None,
                raw_output: None,
                raw_output_append: None,
                locations: None,
                meta: Some(meta),
                images: None,
            },
        )
        .await;
    }

    async fn write_tool_call_identity(
        &self,
        parent_connection_id: &str,
        tool_call_id: &str,
        title: &str,
        raw_input: serde_json::Value,
    ) {
        let Some((state_arc, emitter)) = self
            .manager
            .get_state_and_emitter(parent_connection_id)
            .await
        else {
            return;
        };
        // `raw_input` rides as serialized JSON text: `upsert_tool_call` pushes
        // it as the latest chunk and re-parses it, so the full arguments
        // replace the announcement's empty `{}` on the live state (and on the
        // frontend, whose adapter applies the same latest-parseable-chunk
        // rule).
        emit_with_state(
            &state_arc,
            &emitter,
            AcpEvent::ToolCallUpdate {
                tool_call_id: tool_call_id.to_string(),
                title: Some(title.to_string()),
                status: None,
                content: None,
                raw_input: Some(raw_input.to_string()),
                raw_output: None,
                raw_output_append: None,
                locations: None,
                meta: None,
                images: None,
            },
        )
        .await;
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod mock {
    use super::*;
    use tokio::sync::Mutex;

    /// Records every call so broker tests can assert the meta lifecycle
    /// (running → completed/failed) was driven correctly. No-op on the
    /// emit side — the broker is the unit under test, not the event
    /// fanout.
    #[derive(Default)]
    pub struct MockMetaWriter {
        pub calls: Mutex<Vec<MetaWriteCall>>,
        pub identity_calls: Mutex<Vec<IdentityWriteCall>>,
    }

    #[derive(Debug, Clone)]
    pub struct MetaWriteCall {
        pub parent_connection_id: String,
        pub parent_tool_use_id: String,
        pub meta: serde_json::Value,
    }

    #[derive(Debug, Clone)]
    pub struct IdentityWriteCall {
        pub parent_connection_id: String,
        pub tool_call_id: String,
        pub title: String,
        pub raw_input: serde_json::Value,
    }

    impl MockMetaWriter {
        pub fn new() -> Self {
            Self::default()
        }

        pub async fn snapshot(&self) -> Vec<MetaWriteCall> {
            self.calls.lock().await.clone()
        }

        pub async fn identity_snapshot(&self) -> Vec<IdentityWriteCall> {
            self.identity_calls.lock().await.clone()
        }
    }

    #[async_trait]
    impl DelegationMetaWriter for MockMetaWriter {
        async fn write_meta(
            &self,
            parent_connection_id: &str,
            parent_tool_use_id: &str,
            meta: serde_json::Value,
        ) {
            self.calls.lock().await.push(MetaWriteCall {
                parent_connection_id: parent_connection_id.to_string(),
                parent_tool_use_id: parent_tool_use_id.to_string(),
                meta,
            });
        }

        async fn write_tool_call_identity(
            &self,
            parent_connection_id: &str,
            tool_call_id: &str,
            title: &str,
            raw_input: serde_json::Value,
        ) {
            self.identity_calls.lock().await.push(IdentityWriteCall {
                parent_connection_id: parent_connection_id.to_string(),
                tool_call_id: tool_call_id.to_string(),
                title: title.to_string(),
                raw_input,
            });
        }
    }
}

/// Helper to construct the canonical `meta["codeg.delegation"]` value.
/// Keeps the schema in one place so the writer impls and the broker
/// callsites can't drift apart on field naming.
#[allow(clippy::too_many_arguments)]
pub fn build_delegation_meta(
    status: &str,
    child_connection_id: Option<&str>,
    child_conversation_id: Option<i32>,
    error_code: Option<&str>,
    text_preview: Option<&str>,
    duration_ms: Option<u64>,
    task_preview: Option<&str>,
    task_id: Option<&str>,
) -> serde_json::Value {
    let mut inner = serde_json::Map::new();
    inner.insert(
        "status".to_string(),
        serde_json::Value::String(status.to_string()),
    );
    if let Some(id) = child_connection_id {
        inner.insert(
            "child_connection_id".to_string(),
            serde_json::Value::String(id.to_string()),
        );
    }
    if let Some(id) = child_conversation_id {
        inner.insert(
            "child_conversation_id".to_string(),
            serde_json::Value::Number(serde_json::Number::from(id)),
        );
    }
    if let Some(code) = error_code {
        inner.insert(
            "error_code".to_string(),
            serde_json::Value::String(code.to_string()),
        );
    }
    // Inline result preview so a parent-side snapshot replay after a refresh can
    // render the completed result without the live `delegation_completed` event
    // (which carries the same preview). Only set on the terminal `completed`
    // write; `None` everywhere else.
    if let Some(preview) = text_preview {
        inner.insert(
            "text_preview".to_string(),
            serde_json::Value::String(preview.to_string()),
        );
    }
    // Carry the broker-measured elapsed time so a parent-side snapshot replay
    // after a refresh shows the execution duration without the live
    // `delegation_completed` event. Set on the terminal writes (completed /
    // failed / canceled); `None` for the running write — same survival semantics
    // as `text_preview` above. NOTE: the live event only carries duration on its
    // `Ok` summary, so for failed/canceled the duration is meta-only (the live
    // card shows none until refresh, when this meta supplies it).
    if let Some(ms) = duration_ms {
        inner.insert(
            "duration_ms".to_string(),
            serde_json::Value::Number(serde_json::Number::from(ms)),
        );
    }
    // Task text preview + broker task id. The frontend card falls back to
    // these when the tool call's `raw_input` never carried the arguments
    // (Cursor's identity-less MCP announcements) and the live binding is gone
    // (page refresh, persisted transcript). Carried on EVERY write — meta is
    // replace-wholesale on the ToolCallState (`upsert_tool_call`), so a
    // terminal write that omitted them would erase what the running write
    // supplied.
    if let Some(task) = task_preview {
        inner.insert(
            "task_preview".to_string(),
            serde_json::Value::String(task.to_string()),
        );
    }
    if let Some(id) = task_id {
        inner.insert(
            "task_id".to_string(),
            serde_json::Value::String(id.to_string()),
        );
    }
    let mut outer = serde_json::Map::new();
    outer.insert(
        DELEGATION_META_KEY.to_string(),
        serde_json::Value::Object(inner),
    );
    serde_json::Value::Object(outer)
}

/// True when the broker handed out a synthetic placeholder
/// `parent_tool_use_id` (no matching ACP tool_call_id exists). Skipping
/// meta writes for these avoids spamming `ToolCallUpdate` events with a
/// tool_call_id that no live `ToolCallState` will ever match.
pub fn is_synthetic_parent_tool_use_id(id: &str) -> bool {
    id.starts_with("delegation-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_meta_includes_provided_fields() {
        let v = build_delegation_meta(
            "running",
            Some("conn-1"),
            Some(42),
            None,
            None,
            None,
            Some("run the tests"),
            Some("task-abc"),
        );
        let inner = v.get(DELEGATION_META_KEY).unwrap().as_object().unwrap();
        assert_eq!(inner.get("status").unwrap().as_str().unwrap(), "running");
        assert_eq!(
            inner.get("child_connection_id").unwrap().as_str().unwrap(),
            "conn-1"
        );
        assert_eq!(
            inner
                .get("child_conversation_id")
                .unwrap()
                .as_i64()
                .unwrap(),
            42
        );
        assert!(inner.get("error_code").is_none());
        // No duration on the running write.
        assert!(inner.get("duration_ms").is_none());
        assert_eq!(
            inner.get("task_preview").unwrap().as_str().unwrap(),
            "run the tests"
        );
        assert_eq!(inner.get("task_id").unwrap().as_str().unwrap(), "task-abc");
    }

    #[test]
    fn build_meta_with_error_code() {
        let v = build_delegation_meta("failed", None, Some(7), Some("timeout"), None, None, None, None);
        let inner = v.get(DELEGATION_META_KEY).unwrap().as_object().unwrap();
        assert_eq!(inner.get("status").unwrap().as_str().unwrap(), "failed");
        assert_eq!(
            inner.get("error_code").unwrap().as_str().unwrap(),
            "timeout"
        );
        assert!(inner.get("child_connection_id").is_none());
        assert!(inner.get("task_preview").is_none());
        assert!(inner.get("task_id").is_none());
    }

    #[test]
    fn build_meta_includes_duration_on_terminal_write() {
        let v = build_delegation_meta(
            "completed",
            Some("conn-1"),
            Some(42),
            None,
            None,
            Some(1234),
            None,
            None,
        );
        let inner = v.get(DELEGATION_META_KEY).unwrap().as_object().unwrap();
        assert_eq!(inner.get("duration_ms").unwrap().as_u64().unwrap(), 1234);
    }

    #[test]
    fn synthetic_id_detection() {
        assert!(is_synthetic_parent_tool_use_id(
            "delegation-3b4a5c6d-7e8f-90ab-cdef-1234567890ab"
        ));
        assert!(!is_synthetic_parent_tool_use_id("tu_real_acp_id"));
        assert!(!is_synthetic_parent_tool_use_id(""));
    }
}
