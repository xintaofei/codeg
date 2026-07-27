//! Grok plan-mode approval ("exit plan mode") domain types.
//!
//! When a Grok session is in **plan mode** and the agent finishes planning, it
//! calls its native `exit_plan_mode` tool. That tool reads `plan.md` from disk
//! and issues a BLOCKING ACP ext request (`_x.ai/exit_plan_mode`,
//! `ExitPlanModeExtRequest { sessionId, toolCallId, planContent }`) to the client
//! to get the user's approval before leaving plan mode. Grok waits for the reply.
//!
//! Codeg bridges that ext request into an interactive **plan-approval card**
//! rendered above the composer — the SAME shape as the `ask_user_question` bridge
//! ([`crate::acp::question`]) and the permission dialog: a pending request is
//! captured onto [`crate::acp::session_state::SessionState`] (in-memory, turn
//! scoped, carried on `to_snapshot()` so a mid-turn attach re-renders the card),
//! and the blocked ext request is answered once the user picks an action.
//!
//! Three outcomes exist (per Grok's binary + `~/.grok/docs/user-guide/19-plan-mode.md`):
//!   * **Approve** — leave plan mode and start implementing.
//!   * **Request changes** — freeform revision notes; the agent revises and plan
//!     mode stays active (the user can iterate).
//!   * **Abandon** — abandon the plan and turn plan mode off.
//!
//! ## Wire-format (confirmed against Grok 0.2.111, live ACP capture)
//!
//! Request `ExitPlanModeExtRequest = { sessionId, toolCallId, planContent }`.
//! Note `planContent` is often `null` — Grok does NOT embed the plan body in the
//! request; after approval it reads `plan.md` itself, so the card's plan preview
//! can be empty here.
//!
//! Reply `ExitPlanModeExtResponse = { "outcome": <string>, "feedback": <string> }`
//! (the field is `outcome`, NOT `decision`). Only two outcome values are
//! recognized; every other value (and a missing field) falls through Grok's
//! `#[serde(other)]` catch-all to keep-planning, and the reply `feedback` is then
//! discarded:
//!   * `"approved"`  — leave plan mode and start implementing. A non-empty
//!     `feedback` becomes "approve with review comments".
//!   * `"abandoned"` — abandon the plan and turn plan mode off.
//!   * anything else (we send `"keep_planning"`) — stay in plan mode. Grok drops
//!     the reply `feedback`, so the user's revision notes must be delivered
//!     out-of-band as a follow-up prompt (mirroring Grok's own TUI, where
//!     "request changes" moves focus to the prompt).
//!
//! An earlier inferred shape (`{ decision: "approve"/... }`) was wrong: Grok
//! silently defaulted it to keep-planning, so "Approve" read as "request changes".

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;

/// Per-field sanity bound (characters) for the plan markdown carried in the
/// request event + snapshot. The plan rides the broadcast `PlanApprovalRequest`
/// and every snapshot, so this caps the blast radius of a pathological payload.
/// Generous — a real `plan.md` is far smaller.
pub const MAX_PLAN_MARKDOWN_CHARS: usize = 262_144;
/// Per-field sanity bound (characters) for the user's freeform "request changes"
/// feedback, echoed back to Grok. Generous enough for real revision notes.
pub const MAX_FEEDBACK_CHARS: usize = 16_384;

/// The pending (awaiting-decision) plan approval stored on
/// `SessionState.pending_plan_approval` and carried on `to_snapshot()` so a
/// client attaching mid-turn (cold attach, reconnect, another window) re-renders
/// the approval card even though the one-shot `PlanApprovalRequest` event won't
/// replay for it. At most one is pending per connection (the agent is blocked in
/// its `exit_plan_mode` tool call).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPlanApprovalState {
    /// Backend-minted correlation key for the answer (NOT Grok's toolCallId).
    pub approval_id: String,
    /// Grok's `toolCallId` for the `exit_plan_mode` call, so the synthesized
    /// in-stream result card can key on the same id as the (suppressed) native
    /// tool call. Empty when the request omitted it.
    pub tool_call_id: String,
    /// The plan markdown read from `plan.md` (Grok's `planContent`). May be empty
    /// — an empty/missing plan still opens the approval surface with an
    /// empty-state notice (per Grok's plan-mode doc).
    pub plan_markdown: String,
    pub created_at: DateTime<Utc>,
}

/// Which action the user took on the plan-approval card. `snake_case` on the
/// wire (`approve` / `request_changes` / `abandon`) — constructed by the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanApprovalDecision {
    /// Approve the plan → Grok leaves plan mode and starts implementing.
    Approve,
    /// Request changes → Grok revises the plan; plan mode stays active.
    RequestChanges,
    /// Abandon the plan → plan mode is turned off.
    Abandon,
}

/// The user's submission for a pending plan approval (frontend → backend → the
/// blocked ext request). `feedback` carries the freeform revision notes for
/// `RequestChanges` (ignored / empty for the other decisions). camelCase on the
/// wire — this is constructed by the frontend, not read from an event payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanApprovalAnswer {
    pub decision: PlanApprovalDecision,
    #[serde(default)]
    pub feedback: Option<String>,
}

impl PlanApprovalAnswer {
    /// The trimmed, length-bounded feedback text (empty when none / whitespace).
    /// Applied on the answer side so a hand-rolled client hitting the plain
    /// `acp_answer_plan_approval` endpoint directly can't ride an unbounded blob
    /// back to Grok.
    pub fn normalized_feedback(&self) -> String {
        self.feedback
            .as_deref()
            .unwrap_or("")
            .trim()
            .chars()
            .take(MAX_FEEDBACK_CHARS)
            .collect()
    }
}

/// What [`SessionPlanApprovalAccess::register_plan_approval`] hands back to the
/// connection's `exit_plan_mode` ext handler: the new approval id plus the
/// receiver to await the user's decision on. Mirrors
/// [`crate::acp::question::RegisteredQuestion`].
pub struct RegisteredPlanApproval {
    pub approval_id: String,
    pub answer_rx: oneshot::Receiver<PlanApprovalAnswer>,
}

/// Connection-facing access to register / cancel a pending plan approval on a
/// parent connection. The production impl
/// (`crate::acp::manager::ConnectionManagerPlanApprovalLookup`) wraps the
/// `ConnectionManager`; tests use an in-memory stub. Mirrors
/// [`crate::acp::question::SessionQuestionAccess`] — kept in this neutral module
/// so `connection.rs` depends only on the trait, not on `manager.rs`.
#[async_trait]
pub trait SessionPlanApprovalAccess: Send + Sync {
    /// Register a pending plan approval on the parent connection (resolved from
    /// the connection id), broadcast `PlanApprovalRequest` to every attached
    /// client, and return a receiver that resolves when the user picks an action
    /// (or the approval is canceled). `None` when the connection is gone or one
    /// is already pending on it — the ext handler then declines to Grok, which
    /// keeps plan mode active.
    async fn register_plan_approval(
        &self,
        parent_connection_id: &str,
        tool_call_id: String,
        plan_markdown: String,
    ) -> Option<RegisteredPlanApproval>;

    /// Cancel every pending plan approval parked on a connection that is tearing
    /// down. Called from the `run_connection` cleanup guard so a pending approval
    /// — and the ext handler task parked on it — is reclaimed synchronously on
    /// disconnect. Dropping the sender resolves the handler's await as a
    /// disconnect (Grok keeps plan mode active, re-surfaces on reconnect).
    async fn cancel_plan_approvals_by_parent(&self, parent_connection_id: &str);
}

/// Parse Grok's `_x.ai/exit_plan_mode` ext-request params into the plan markdown
/// and `toolCallId` codeg needs to render the approval card. Grok's wire shape is
/// `{ sessionId, toolCallId, planContent }`. Lenient: an empty / missing
/// `planContent` is valid (the empty-plan approval surface), so the only hard
/// error is a non-object payload. The markdown is length-bounded to
/// [`MAX_PLAN_MARKDOWN_CHARS`].
pub fn parse_grok_exit_plan_request(params: &Value) -> Result<(String, String), String> {
    let obj = params
        .as_object()
        .ok_or_else(|| "exit_plan_mode ext request params is not an object".to_string())?;
    let plan_markdown: String = obj
        .get("planContent")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(MAX_PLAN_MARKDOWN_CHARS)
        .collect();
    let tool_call_id = obj
        .get("toolCallId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok((plan_markdown, tool_call_id))
}

/// Serialize the user's decision into Grok's `ExitPlanModeExtResponse` — the
/// reply to a blocked `_x.ai/exit_plan_mode` ext request. See the module-level
/// wire-format note; the encoding is confirmed against Grok 0.2.111.
///
/// `RequestChanges` maps to `"keep_planning"` (any non-approve/non-abandon value
/// keeps Grok in plan mode via its `#[serde(other)]` catch-all). Grok discards the
/// reply `feedback` on that path, so the revision notes are ALSO delivered as a
/// follow-up prompt by the frontend — this field is carried for future-proofing
/// and is harmless if ignored.
pub fn build_grok_exit_plan_response(answer: &PlanApprovalAnswer) -> Value {
    let outcome = match answer.decision {
        PlanApprovalDecision::Approve => "approved",
        PlanApprovalDecision::RequestChanges => "keep_planning",
        PlanApprovalDecision::Abandon => "abandoned",
    };
    serde_json::json!({
        "outcome": outcome,
        "feedback": answer.normalized_feedback(),
    })
}

/// The `_x.ai/exit_plan_mode` reply for a connection tearing down mid-approval
/// (the parked responder is drained without a user decision). Mirrors Grok's own
/// "client disconnected mid-approval; plan mode stays active" behavior: reply with
/// the keep-planning outcome so Grok keeps plan mode active and re-surfaces the
/// approval on reconnect, rather than silently proceeding as if approved. Must be
/// neither `"approved"` nor `"abandoned"` (both leave plan mode).
pub fn grok_exit_plan_disconnect_response() -> Value {
    serde_json::json!({ "outcome": "keep_planning", "feedback": "" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn grok_params(plan: &str) -> Value {
        json!({
            "sessionId": "s-1",
            "toolCallId": "call-42",
            "planContent": plan,
        })
    }

    #[test]
    fn parse_reads_plan_and_tool_call_id() {
        let (plan, tc) = parse_grok_exit_plan_request(&grok_params("# Plan\n- step")).unwrap();
        assert_eq!(plan, "# Plan\n- step");
        assert_eq!(tc, "call-42");
    }

    #[test]
    fn parse_accepts_empty_or_missing_plan() {
        let (plan, tc) = parse_grok_exit_plan_request(&grok_params("")).unwrap();
        assert_eq!(plan, "");
        assert_eq!(tc, "call-42");
        // Missing planContent + toolCallId → empty strings, still Ok.
        let (plan, tc) =
            parse_grok_exit_plan_request(&json!({ "sessionId": "s-1" })).unwrap();
        assert!(plan.is_empty());
        assert!(tc.is_empty());
    }

    #[test]
    fn parse_rejects_non_object() {
        assert!(parse_grok_exit_plan_request(&json!("nope")).is_err());
        assert!(parse_grok_exit_plan_request(&json!([1, 2, 3])).is_err());
    }

    #[test]
    fn parse_bounds_plan_markdown() {
        let huge = "x".repeat(MAX_PLAN_MARKDOWN_CHARS + 500);
        let (plan, _) = parse_grok_exit_plan_request(&grok_params(&huge)).unwrap();
        assert_eq!(plan.chars().count(), MAX_PLAN_MARKDOWN_CHARS);
    }

    #[test]
    fn build_response_maps_each_decision() {
        // Confirmed against Grok 0.2.111: field is `outcome`; approve/abandon are
        // past-tense; request-changes uses the keep-planning catch-all value.
        let approve = PlanApprovalAnswer {
            decision: PlanApprovalDecision::Approve,
            feedback: None,
        };
        assert_eq!(build_grok_exit_plan_response(&approve)["outcome"], "approved");
        assert_eq!(build_grok_exit_plan_response(&approve)["feedback"], "");
        // The obsolete `decision` field must be gone (it was silently defaulted).
        assert!(build_grok_exit_plan_response(&approve)
            .get("decision")
            .is_none());

        let changes = PlanApprovalAnswer {
            decision: PlanApprovalDecision::RequestChanges,
            feedback: Some("  use SSE instead  ".into()),
        };
        let v = build_grok_exit_plan_response(&changes);
        assert_eq!(v["outcome"], "keep_planning");
        // Feedback is trimmed (carried for future-proofing; Grok ignores it here).
        assert_eq!(v["feedback"], "use SSE instead");

        let abandon = PlanApprovalAnswer {
            decision: PlanApprovalDecision::Abandon,
            feedback: None,
        };
        assert_eq!(
            build_grok_exit_plan_response(&abandon)["outcome"],
            "abandoned"
        );
    }

    #[test]
    fn feedback_is_bounded() {
        let huge = "y".repeat(MAX_FEEDBACK_CHARS + 100);
        let answer = PlanApprovalAnswer {
            decision: PlanApprovalDecision::RequestChanges,
            feedback: Some(huge),
        };
        assert_eq!(
            answer.normalized_feedback().chars().count(),
            MAX_FEEDBACK_CHARS
        );
    }

    #[test]
    fn answer_deserializes_from_camel_case_wire() {
        let a: PlanApprovalAnswer =
            serde_json::from_value(json!({ "decision": "request_changes", "feedback": "x" }))
                .unwrap();
        assert_eq!(a.decision, PlanApprovalDecision::RequestChanges);
        assert_eq!(a.feedback.as_deref(), Some("x"));
        // feedback optional.
        let a: PlanApprovalAnswer =
            serde_json::from_value(json!({ "decision": "approve" })).unwrap();
        assert_eq!(a.decision, PlanApprovalDecision::Approve);
        assert!(a.feedback.is_none());
    }

    #[test]
    fn disconnect_response_keeps_planning() {
        // On disconnect Grok must stay in plan mode: neither of the two
        // plan-mode-leaving outcomes.
        let v = grok_exit_plan_disconnect_response();
        assert_eq!(v["outcome"], "keep_planning");
        assert_ne!(v["outcome"], "approved");
        assert_ne!(v["outcome"], "abandoned");
    }
}
