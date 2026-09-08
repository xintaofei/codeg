//! Live ACP session titles.
//!
//! Agents publish a session name through `session_info_update.title`. Codeg
//! used to ignore that field and only adopt a title the next time the
//! conversation was loaded from disk. These helpers extract a usable title
//! from the live notification so the lifecycle worker can write it immediately.
//!
//! Not every agent can push, and Claude Code could not until recently: through
//! claude-agent-acp 0.72.0 its adapter had no wire event for the generated
//! title — it read the name back out of the session file at turn-end — so
//! `acp::background_watch` reads the same `ai-title` / `custom-title` records
//! off the transcript it is already tailing and hands them to
//! [`publish_native_title`], which is the one place that decides whether a
//! title reaches the lifecycle worker.
//!
//! 0.73.0 adds the wire event (`session-titles.js`: Claude Code's own
//! auto-titling never arms under the Agent SDK, so the adapter now asks the CLI
//! via `generate_session_title` and publishes the result as
//! `session_info_update.title`). That makes the two producers below REAL for
//! Claude rather than theoretical, which is what the single critical section in
//! [`publish_native_title`] was already built for. The transcript path stays:
//! it also carries `custom-title` and pre-0.73 installs, and the adapter
//! persists its generated title into the same session file, so both producers
//! converge on one string and the skip-cache absorbs the duplicate.

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::acp::session_state::SessionState;
use crate::acp::types::AcpEvent;
use crate::web::event_bridge::{emit_with_state, EventEmitter};

/// Pull a usable session title out of ACP `session_info_update.title`.
///
/// `Undefined` (passed in as `None`) means the update did not touch the title
/// and is ignored. The schema also uses `Null` to mean "clear"; we treat that
/// the same as absent on purpose so an explicit clear cannot wipe the row
/// back to Untitled. Whitespace-only strings are ignored for the same reason.
pub(crate) fn native_title_from_session_info(title: Option<&str>) -> Option<String> {
    let t = title?.trim();
    if t.is_empty() {
        None
    } else {
        Some(crate::parsers::truncate_str(t, 100))
    }
}

/// Emit `title` as this connection's live session title, unless it is a repeat
/// of the last one emitted here.
///
/// Test and set under ONE write lock, because there are genuinely two
/// concurrent producers: the connection's notification loop (which handles a
/// session's `session_info_update`s serially) and — for Claude — the transcript
/// watcher's own task, which reads the same name off `ai-title` /
/// `custom-title` records on its own poll cadence. Splitting the test from the
/// set would let both admit the same string and write it twice; the single
/// critical section makes "only a CHANGED title gets through" hold by
/// construction rather than by the two callers happening not to overlap.
///
/// What the critical section does NOT order is the `emit` that follows it: two
/// producers admitting DIFFERENT strings can broadcast in the opposite order,
/// leaving the older name written last. Note what does NOT repair that: the
/// transcript re-emitting the same `ai-title` record, because the watcher has
/// already folded that value into its own slots and will not re-queue it. What
/// does is a later detail fetch (it re-resolves the whole file) or the next
/// genuinely different title from either producer — including the adapter's own
/// turn-end pull, whose `lastTitle` now differs from what the file holds. So the
/// window is recoverable auto-title staleness, not a stuck name, and not worth
/// serializing the whole publish behind the state lock.
///
/// The third writer of `last_native_title` is the `ConversationLinked` arm,
/// which is emitted ONLY while the row is still unbound and therefore can never
/// race a title this admits.
///
/// A title published before the first prompt binds the row has nowhere to land
/// and is dropped WITHOUT being remembered, so the same string is still
/// accepted once the row exists.
///
/// The emitted `NativeSessionTitle` reaches `acp::lifecycle`, whose
/// `refresh_auto_title` write is itself a no-op on a user-renamed
/// (`title_locked`) row and on an unchanged value — so repeats that do get
/// past the skip-cache still cost nothing and can never overwrite a name the
/// user chose.
pub(crate) async fn publish_native_title(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    title: String,
) {
    let admit = {
        let mut s = state.write().await;
        let admit =
            s.conversation_id.is_some() && s.last_native_title.as_deref() != Some(title.as_str());
        if admit {
            s.last_native_title = Some(title.clone());
        }
        admit
    };
    if admit {
        emit_with_state(state, emitter, AcpEvent::NativeSessionTitle { title }).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{native_title_from_session_info, publish_native_title};
    use crate::acp::session_state::SessionState;
    use crate::acp::types::AcpEvent;
    use crate::models::agent::AgentType;
    use crate::web::event_bridge::EventEmitter;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn title_state(conversation_id: Option<i32>) -> Arc<RwLock<SessionState>> {
        let mut st = SessionState::new(
            "conn-title".to_string(),
            AgentType::ClaudeCode,
            None,
            "win".to_string(),
            None,
        );
        st.conversation_id = conversation_id;
        Arc::new(RwLock::new(st))
    }

    /// Titles emitted on this connection so far, oldest first.
    async fn emitted(state: &Arc<RwLock<SessionState>>) -> Vec<String> {
        state
            .read()
            .await
            .recent_events_after(0)
            .unwrap_or_default()
            .iter()
            .filter_map(|e| match &e.payload {
                AcpEvent::NativeSessionTitle { title } => Some(title.clone()),
                _ => None,
            })
            .collect()
    }

    /// The skip-cache is what keeps the two producers cheap: Claude Code
    /// re-emits its `ai-title` record throughout a session (228 identical
    /// copies in one observed transcript) and CodeBuddy re-pushes its fallback
    /// after every prompt. Only a CHANGED name may reach the lifecycle worker.
    #[tokio::test]
    async fn only_a_changed_title_is_emitted() {
        let state = title_state(Some(7));

        publish_native_title(&state, &EventEmitter::Noop, "Fix the login flow".into()).await;
        publish_native_title(&state, &EventEmitter::Noop, "Fix the login flow".into()).await;
        publish_native_title(&state, &EventEmitter::Noop, "Fix the signup flow".into()).await;

        assert_eq!(
            emitted(&state).await,
            vec![
                "Fix the login flow".to_string(),
                "Fix the signup flow".to_string()
            ]
        );
    }

    /// A title published before the first prompt binds the row has nowhere to
    /// land, so it is dropped — and deliberately NOT remembered, or the retry
    /// that follows `ConversationLinked` would be swallowed and the row would
    /// keep its fallback name for the rest of the connection.
    #[tokio::test]
    async fn a_title_dropped_while_unbound_does_not_poison_the_cache() {
        let state = title_state(None);

        publish_native_title(&state, &EventEmitter::Noop, "Fix the login flow".into()).await;
        assert!(emitted(&state).await.is_empty(), "no row to write to yet");
        assert!(
            state.read().await.last_native_title.is_none(),
            "a dropped title must not poison the skip-cache"
        );

        state
            .write()
            .await
            .apply_event(&AcpEvent::ConversationLinked {
                conversation_id: 7,
                folder_id: 1,
                parent_conversation_id: None,
                parent_tool_use_id: None,
            });

        publish_native_title(&state, &EventEmitter::Noop, "Fix the login flow".into()).await;
        assert_eq!(
            emitted(&state).await,
            vec!["Fix the login flow".to_string()],
            "the same title must be accepted once the row exists"
        );
    }

    #[test]
    fn rejects_missing_and_blank() {
        assert_eq!(native_title_from_session_info(None), None);
        assert_eq!(native_title_from_session_info(Some("")), None);
        assert_eq!(native_title_from_session_info(Some("   ")), None);
        assert_eq!(native_title_from_session_info(Some("\n\t")), None);
    }

    #[test]
    fn trims_and_keeps_a_real_title() {
        assert_eq!(
            native_title_from_session_info(Some("  Fix login flow  ")).as_deref(),
            Some("Fix login flow")
        );
    }

    #[test]
    fn caps_at_parser_title_length() {
        let long = "a".repeat(150);
        let got = native_title_from_session_info(Some(&long)).unwrap();
        assert_eq!(got, crate::parsers::truncate_str(&long, 100));
        assert!(got.ends_with("..."));
    }

    /// The two Claude producers must NORMALIZE identically, or they stop being
    /// repeats of each other.
    ///
    /// claude-agent-acp 0.73.0 publishes its generated title over the wire
    /// (here) AND persists it to the session file, where the transcript watcher
    /// and the detail-fetch backfill read it back through
    /// `parsers::claude::capture_title_record`. Both then race into
    /// `publish_native_title`, whose skip-cache only collapses IDENTICAL
    /// strings — and whose doc notes that two producers admitting DIFFERENT
    /// strings can broadcast out of order. So a divergence in trimming or in
    /// the length cap would not merely differ cosmetically: it would turn every
    /// long title into a permanent two-writer ping-pong on the sidebar row.
    ///
    /// Asserting against the transcript helper rather than a literal is the
    /// point — a future change to either cap has to change both.
    #[test]
    fn agrees_with_the_transcript_producer_on_normalization() {
        for raw in [
            "  Fix login flow  ",
            "a",
            &"b".repeat(100),
            &"c".repeat(101),
            &format!("  {}  ", "d".repeat(150)),
        ] {
            let record = serde_json::json!({ "type": "ai-title", "aiTitle": raw });
            let (mut custom, mut ai) = (None, None);
            crate::parsers::claude::capture_title_record(&record, "ai-title", &mut custom, &mut ai);
            assert_eq!(
                native_title_from_session_info(Some(raw)),
                ai,
                "wire and transcript producers disagree on {raw:?}"
            );
        }
    }
}
