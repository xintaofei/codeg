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
//! After each child turn settles, the broker keeps the child process alive
//! (completed / failed) so `continue_with_session` can send follow-ups in the
//! SAME session with full prior context. `close_session` (or cancel / parent
//! teardown) permanently retires the child. Re-spawning with the child's
//! agent `external_id` covers the case where the process was reaped while the
//! conversation row still exists.

pub mod broker;
pub mod companion;
pub mod depth;
pub mod event_emitter;
pub mod listener;
pub mod live_reply;
pub mod meta_writer;
pub mod parent_watcher;
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
pub const CONTINUE_TOOL_REWRITE_TITLE: &str = "codeg-mcp__continue_with_session";
pub const CLOSE_TOOL_REWRITE_TITLE: &str = "codeg-mcp__close_session";
