//! Multi-agent delegation: the parent agent's LLM can call the built-in MCP
//! tool `delegate_to_agent` to spawn a fresh ACP session of any (possibly
//! different) agent type, wait for its first turn to finish, and receive the
//! sub-agent's final assistant text as the MCP tool_result.
//!
//! The high-level wiring is:
//!
//! ```text
//!   parent LLM ─┐
//!               │ ToolUse(delegate_to_agent, ...)
//!               ▼
//!   parent CLI ──stdio──► codeg-mcp (per-launch companion binary)
//!                                 │
//!                                 │ UDS / named pipe (token-authed)
//!                                 ▼
//!                       DelegationBroker (this module)
//!                                 │
//!                                 │ ConnectionSpawner trait
//!                                 ▼
//!                       ConnectionManager.spawn_agent / send_prompt_linked
//!                                 │
//!                                 ▼
//!                       child ACP session  ── TurnComplete ──┐
//!                                                            │
//!   parent LLM ◄── MCP tool_result ◄── DelegationOutcome ◄───┘
//! ```
//!
//! v1 is one-shot (function-call semantics): after the child's first
//! `TurnComplete`, the broker resolves the pending call, sends `disconnect`
//! to the child, and returns. v2 will introduce `continue_with_session` /
//! `close_session` tools without protocol breakage.
//!
//! One deliberate exception to one-shot: `resume_delegation` revives a task
//! that was INTERRUPTED (canceled, or stranded by a crash) by re-spawning its
//! child connection with the recorded agent session id and re-arming the same
//! `delegation_call_id` routing — strictly a continuation of the original
//! task, never a second iteration on it (the tool takes no task text).

pub mod broker;
pub mod companion;
pub mod depth;
pub mod event_emitter;
pub mod listener;
pub mod live_reply;
pub mod meta_writer;
pub mod parent_watcher;
pub mod service;
pub mod spawner;
pub mod transport;
pub mod types;

/// Canonical titles written onto a parent tool call that was announced
/// identity-less (Cursor's `"MCP: tool"` — see
/// `acp::lifecycle::CURSOR_IDENTITYLESS_MCP_TITLE`) once the companion
/// round-trip reveals which codeg-mcp tool it actually is. Two writers must
/// agree on these strings: the broker/listener call-time rewrite
/// (`DelegationBroker::rewrite_identityless_tool_call`) and the
/// completion-time result sniff in `acp::connection`
/// (`cursor_companion_title_from_content`). The `codeg-mcp__<tool>` shape is
/// what the frontend's tool-name normalizer already resolves to the dedicated
/// delegation / status cards.
pub const DELEGATE_TOOL_REWRITE_TITLE: &str = "codeg-mcp__delegate_to_agent";
pub const STATUS_TOOL_REWRITE_TITLE: &str = "codeg-mcp__get_delegation_status";
pub const CANCEL_TOOL_REWRITE_TITLE: &str = "codeg-mcp__cancel_delegation";
pub const RESUME_TOOL_REWRITE_TITLE: &str = "codeg-mcp__resume_delegation";

/// Byte cap shared by every fallback task label written to delegation-card
/// metadata. The full task remains available in the child session.
pub(crate) const TASK_PREVIEW_CAP: usize = 2 * 1024;

/// Return a bounded task label without splitting a UTF-8 code point.
pub(crate) fn task_preview(task: &str) -> String {
    if task.len() <= TASK_PREVIEW_CAP {
        return task.to_string();
    }
    const ELLIPSIS: &str = "…";
    let mut end = TASK_PREVIEW_CAP.saturating_sub(ELLIPSIS.len());
    while end > 0 && !task.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &task[..end], ELLIPSIS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_preview_is_utf8_safe_and_within_cap() {
        let preview = task_preview(&"界".repeat(TASK_PREVIEW_CAP));
        assert!(preview.len() <= TASK_PREVIEW_CAP);
        assert!(preview.ends_with('…'));
    }
}
