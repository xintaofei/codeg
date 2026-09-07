//! ACP `session/fork` support via raw JSON-RPC messages.
//!
//! The `sacp` crate does not yet provide typed request/response types for
//! `session/fork`, so we use `UntypedMessage` (the same pattern used for
//! `session/set_config_option` in connection.rs).

use sacp::schema::{ForkSessionRequest, ForkSessionResponse, SessionId};
use sacp::{Agent, ConnectionTo, UntypedMessage};

use crate::acp::error::AcpError;
use crate::models::agent::AgentType;
use crate::models::message::{ContentBlock, MessageTurn, TurnRole};

/// Where in the history to fork, for the agents that can honour it.
///
/// Rides as `_meta.jetbrains.air.fork` on `session/fork`. Every adapter that
/// implements it reads the same block and they all fall back to forking at the
/// TAIL when it is absent, so omitting this is always the old behaviour.
///
/// They resolve it differently, which is why all three halves exist:
///
/// * **claude-agent-acp 0.75.1** resolves in three levels: `message_id` against
///   the live id map, then against `messageIdForGrouping` (the API message id,
///   else the record uuid) along the ACTIVE parentUuid chain, then — new in
///   0.75.1 — against the full persisted transcript INCLUDING abandoned
///   branches, where a failed id finally falls through to the fingerprint. Up
///   to 0.74.0 it ignored the fingerprint entirely, so codeg sent the id alone;
///   from 0.75.1 both halves are sent, which is what turns a fork point on an
///   abandoned branch from a silent tail-fork into an exact hit.
///   `crate::parsers::claude` derives the id into
///   [`crate::models::MessageTurn::agent_message_id`], and the hash side is
///   byte-compatible with what `fingerprint_agent_message` computes (a single
///   fingerprint match even wins regardless of occurrence).
/// * **codex-acp 1.8.0** first matches `message_id` against `items[].id`, then
///   falls back to hashing each agent message and taking the
///   `message_occurrence`-th match. Codex rollout files record NO item ids, so
///   codeg cannot produce one it would recognise — the fingerprint is the only
///   path that resolves there, and `message_id` is sent as codeg's own turn id
///   purely because the field is required.
/// * **deepseek-acp 0.8.0** is the only one that can use BOTH halves, so codeg
///   sends both. Its id side accepts either the wire id it stamps on message
///   chunks (`<turn>:<step>`) or the session log's own `message.id`, and
///   `crate::parsers::deepseek` records the latter. Its fingerprint side hashes
///   the history TWICE — once per assistant message, once per whole turn — and
///   refuses (`invalid_params`) when the two land on different turns; codeg
///   renders one bubble per log turn, so the per-turn reading is the one that
///   matches, and the id is what keeps the ambiguous case from ever being
///   reached.
///
/// All three strip a trailing `:segment:<n>` before matching, so ids must not
/// carry one.
#[derive(Debug, Clone)]
pub struct ForkPoint {
    pub message_id: String,
    pub message_fingerprint: Option<String>,
    /// 1-based index among agent messages sharing the same fingerprint.
    pub message_occurrence: Option<u32>,
}

/// `sha256:<64 lowercase hex>` of `text`, the shape codex-acp compares against.
pub fn fingerprint_agent_message(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

/// This turn's agent text, as one string — what codex fingerprints.
fn turn_text(turn: &MessageTurn) -> String {
    turn.blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// 1-based index of `turns[idx]` among the assistant turns sharing its
/// fingerprint — what both fingerprint-matching adapters count, so an answer
/// repeated verbatim earlier in the session still forks at the right one.
///
/// Assistant turns only: both hash agent messages, so a user turn with
/// identical text must not shift the count.
fn fingerprint_occurrence(turns: &[MessageTurn], idx: usize, fingerprint: &str) -> usize {
    turns[..idx]
        .iter()
        .filter(|t| {
            matches!(t.role, TurnRole::Assistant)
                && fingerprint_agent_message(&turn_text(t)) == fingerprint
        })
        .count()
        + 1
}

/// Build the fork point naming `turn_id`, or `None` when this agent/turn cannot
/// be named — in which case the caller forks at the tail, as before.
///
/// Only ASSISTANT turns are fork points: every adapter resolves the point
/// against an agent message, and "continue from my own prompt" is already what
/// a plain fork-send does.
pub fn resolve_fork_point(
    turns: &[MessageTurn],
    turn_id: &str,
    agent_type: AgentType,
) -> Option<ForkPoint> {
    let idx = turns
        .iter()
        .position(|t| t.id == turn_id && matches!(t.role, TurnRole::Assistant))?;
    let turn = &turns[idx];

    match agent_type {
        // Codex rollouts carry no item ids, so the id can never match and the
        // fingerprint is the only thing that resolves. `message_id` is still
        // required by the wire contract, so it carries codeg's own turn id —
        // deliberately something codex will not find, which is exactly what
        // makes it fall through to the fingerprint branch.
        AgentType::Codex => {
            let text = turn_text(turn);
            if text.trim().is_empty() {
                return None;
            }
            let fingerprint = fingerprint_agent_message(&text);
            let occurrence = fingerprint_occurrence(turns, idx, &fingerprint);
            Some(ForkPoint {
                message_id: turn_id.to_string(),
                message_fingerprint: Some(fingerprint),
                message_occurrence: u32::try_from(occurrence).ok(),
            })
        }
        // DeepSeek and Claude 0.75.1 both read the two halves in the same
        // order, so both are sent. The id resolves on its own whenever the
        // transcript named the message, and the fingerprint is what still
        // resolves when it did not — a DeepSeek log written without an `id` or
        // a parse that began mid-log; on Claude, a fork point that has drifted
        // off the active parentUuid chain onto an abandoned branch. Sending the
        // fingerprint alongside an id costs nothing: both adapters stop at the
        // first id that matches and never look at it.
        //
        // The `text.trim().is_empty()` guard below is load-bearing on Claude,
        // not just tidiness. Every turn `parsers::claude` leaves unnamed is one
        // codeg SYNTHESIZED with no text of its own (a `/goal` marker, a bare
        // top-level `tool_use`, a bare `tool_result`); `fingerprint("")` would
        // match every text-free grouping on the agent's side at once and then
        // pick between them by occurrence, forking somewhere arbitrary. A tail
        // fork is the honest answer for those.
        AgentType::ClaudeCode | AgentType::DeepSeek => {
            let text = turn_text(turn);
            let fingerprint = (!text.trim().is_empty()).then(|| fingerprint_agent_message(&text));
            // Neither half can name this turn — an assistant bubble opened by a
            // tool result alone, with no message of its own to point at.
            if turn.agent_message_id.is_none() && fingerprint.is_none() {
                return None;
            }
            let occurrence = fingerprint
                .as_ref()
                .map(|fp| fingerprint_occurrence(turns, idx, fp));
            Some(ForkPoint {
                // Same reasoning as codex when the transcript named nothing:
                // the field is required, and codeg's own turn id (`turn-<n>`,
                // a position, never a record uuid) is deliberately something
                // neither adapter will find, which is what makes it fall
                // through to the fingerprint.
                message_id: turn
                    .agent_message_id
                    .clone()
                    .unwrap_or_else(|| turn_id.to_string()),
                message_fingerprint: fingerprint,
                message_occurrence: occurrence.and_then(|n| u32::try_from(n).ok()),
            })
        }
        // Every other agent either has no `session/fork` or no fork point in
        // it; forking at the tail is the honest fallback.
        _ => None,
    }
}

impl ForkPoint {
    fn to_meta(&self) -> serde_json::Value {
        let mut fork = serde_json::Map::new();
        fork.insert("version".into(), serde_json::json!(1));
        fork.insert("messageId".into(), serde_json::json!(self.message_id));
        if let Some(fp) = &self.message_fingerprint {
            fork.insert("messageFingerprint".into(), serde_json::json!(fp));
        }
        if let Some(n) = self.message_occurrence {
            fork.insert("messageOccurrence".into(), serde_json::json!(n));
        }
        serde_json::json!({ "jetbrains": { "air": { "fork": fork } } })
    }
}

/// Send a `session/fork` request over an existing ACP connection.
///
/// Returns the full `ForkSessionResponse` so the caller can attach directly
/// without a separate `session/load` round-trip, plus the raw top-level `models`
/// value (captured before the typed deserialize drops it) so the Grok path can
/// parse per-model reasoning-effort data. `None` when the response has no
/// `models` field.
///
/// `fork_point` forks at a chosen message instead of the tail; see [`ForkPoint`].
/// An agent that does not implement it ignores the unknown `_meta` key, so this
/// is inert rather than an error wherever it isn't understood. An agent that
/// DOES implement it but cannot resolve the point answers `invalid_params`,
/// which surfaces as a fork failure the user can retry from a different turn.
pub async fn fork_session(
    cx: &ConnectionTo<Agent>,
    session_id: &SessionId,
    cwd: &str,
    fork_point: Option<&ForkPoint>,
) -> Result<(ForkSessionResponse, Option<serde_json::Value>), AcpError> {
    let req = ForkSessionRequest::new(session_id.clone(), cwd);
    // Built through a Value so the AIR block can be attached: `_meta` is not a
    // settable field on the typed request, and the bytes are otherwise
    // identical to the typed send.
    let mut req = serde_json::to_value(&req)
        .map_err(|e| AcpError::protocol(format!("Failed to build fork request: {e}")))?;
    if let (Some(point), Some(obj)) = (fork_point, req.as_object_mut()) {
        obj.insert("_meta".into(), point.to_meta());
    }
    let untyped_req = UntypedMessage::new("session/fork", &req)
        .map_err(|e| AcpError::protocol(format!("Failed to build fork request: {e}")))?;

    let raw_response: serde_json::Value = cx
        .send_request_to(Agent, untyped_req)
        .block_task()
        .await
        .map_err(|e| AcpError::protocol(format!("session/fork failed: {e}")))?;

    let models = raw_response.get("models").cloned();
    let response: ForkSessionResponse = serde_json::from_value(raw_response)
        .map_err(|e| AcpError::protocol(format!("Failed to parse fork response: {e}")))?;

    Ok((response, models))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn turn(id: &str, role: TurnRole, text: &str, agent_message_id: Option<&str>) -> MessageTurn {
        MessageTurn {
            id: id.into(),
            role,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            timestamp: Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: agent_message_id.map(str::to_string),
        }
    }

    /// The shape both adapters parse: version 1, and the id under
    /// `jetbrains.air.fork`.
    #[test]
    fn meta_matches_the_air_fork_block() {
        let meta = ForkPoint {
            message_id: "msg_01".into(),
            message_fingerprint: Some("sha256:ab".into()),
            message_occurrence: Some(2),
        }
        .to_meta();
        assert_eq!(
            meta,
            serde_json::json!({"jetbrains": {"air": {"fork": {
                "version": 1,
                "messageId": "msg_01",
                "messageFingerprint": "sha256:ab",
                "messageOccurrence": 2,
            }}}})
        );
    }

    /// The optional halves stay absent rather than null — every adapter
    /// validates the fingerprint's shape when present, so sending `null` would
    /// be worse than sending nothing.
    #[test]
    fn meta_omits_absent_fingerprint_and_occurrence() {
        let meta = ForkPoint {
            message_id: "msg_01".into(),
            message_fingerprint: None,
            message_occurrence: None,
        }
        .to_meta();
        let fork = &meta["jetbrains"]["air"]["fork"];
        assert!(fork.get("messageFingerprint").is_none());
        assert!(fork.get("messageOccurrence").is_none());
    }

    /// The id the parser derived leads, and 0.75.1 also reads the fingerprint,
    /// so both go out: the id resolves on the active chain, the fingerprint is
    /// what still resolves once the point has drifted onto an abandoned branch.
    #[test]
    fn claude_sends_the_derived_id_and_the_fingerprint_together() {
        let turns = vec![
            turn("turn-0", TurnRole::User, "hi", None),
            turn("turn-1", TurnRole::Assistant, "hello", Some("msg_01")),
        ];
        let point = resolve_fork_point(&turns, "turn-1", AgentType::ClaudeCode).unwrap();
        assert_eq!(point.message_id, "msg_01");
        assert_eq!(
            point.message_fingerprint.as_deref(),
            Some(fingerprint_agent_message("hello").as_str())
        );
        // Claude needs BOTH halves to use the fingerprint at all, so an
        // occurrence must ride along with it.
        assert_eq!(point.message_occurrence, Some(1));
    }

    /// A turn Claude never named still forks by content — 0.75.1 falls through
    /// to the fingerprint when the id misses, and `turn-<n>` is a position that
    /// can never collide with a record uuid.
    #[test]
    fn claude_falls_back_to_the_fingerprint_with_no_agent_id() {
        let turns = vec![turn("turn-1", TurnRole::Assistant, "hello", None)];
        let point = resolve_fork_point(&turns, "turn-1", AgentType::ClaudeCode).unwrap();
        assert_eq!(point.message_id, "turn-1");
        assert_eq!(
            point.message_fingerprint.as_deref(),
            Some(fingerprint_agent_message("hello").as_str())
        );
    }

    /// The one case that must stay a tail fork: a turn codeg synthesized with
    /// no text and no id. `fingerprint("")` would match every text-free
    /// grouping on Claude's side, so guessing between them by occurrence is
    /// strictly worse than not naming a point at all.
    #[test]
    fn claude_declines_a_synthesized_turn_with_neither_id_nor_text() {
        let mut t = turn("turn-1", TurnRole::Assistant, "", None);
        t.blocks = vec![ContentBlock::ToolUse {
            tool_use_id: Some("tl-tool-0".into()),
            tool_name: "Bash".into(),
            input_preview: None,
            status: None,
            meta: None,
        }];
        assert!(resolve_fork_point(&[t], "turn-1", AgentType::ClaudeCode).is_none());
    }

    /// A textless turn Claude DID name is still a fork point by id alone.
    #[test]
    fn claude_forks_a_textless_named_turn_by_its_id_alone() {
        let mut t = turn("turn-1", TurnRole::Assistant, "", Some("msg_01"));
        t.blocks = Vec::new();
        let point = resolve_fork_point(&[t], "turn-1", AgentType::ClaudeCode).unwrap();
        assert_eq!(point.message_id, "msg_01");
        assert!(point.message_fingerprint.is_none());
        assert!(point.message_occurrence.is_none());
    }

    #[test]
    fn codex_forks_by_content_fingerprint() {
        let turns = vec![
            turn("turn-0", TurnRole::User, "hi", None),
            turn("turn-1", TurnRole::Assistant, "hello", None),
        ];
        let point = resolve_fork_point(&turns, "turn-1", AgentType::Codex).unwrap();
        assert_eq!(
            point.message_fingerprint.as_deref(),
            Some(fingerprint_agent_message("hello").as_str())
        );
        assert_eq!(point.message_occurrence, Some(1));
    }

    /// An answer repeated verbatim must still fork at the one the user clicked,
    /// which is the only thing `messageOccurrence` is for.
    #[test]
    fn codex_counts_repeated_answers() {
        let turns = vec![
            turn("turn-0", TurnRole::Assistant, "same", None),
            turn("turn-1", TurnRole::User, "again", None),
            turn("turn-2", TurnRole::Assistant, "same", None),
        ];
        assert_eq!(
            resolve_fork_point(&turns, "turn-0", AgentType::Codex)
                .unwrap()
                .message_occurrence,
            Some(1)
        );
        assert_eq!(
            resolve_fork_point(&turns, "turn-2", AgentType::Codex)
                .unwrap()
                .message_occurrence,
            Some(2)
        );
    }

    /// Codex hashes agent messages only, so a user turn with identical text
    /// must not shift the count.
    #[test]
    fn codex_occurrence_ignores_user_turns() {
        let turns = vec![
            turn("turn-0", TurnRole::User, "same", None),
            turn("turn-1", TurnRole::Assistant, "same", None),
        ];
        assert_eq!(
            resolve_fork_point(&turns, "turn-1", AgentType::Codex)
                .unwrap()
                .message_occurrence,
            Some(1)
        );
    }

    /// The fingerprint is the only thing that can resolve on codex, so a turn
    /// with no text to hash is not a fork point.
    #[test]
    fn codex_declines_a_turn_with_no_text() {
        let mut t = turn("turn-1", TurnRole::Assistant, "", None);
        t.blocks = vec![ContentBlock::Text { text: "   ".into() }];
        assert!(resolve_fork_point(&[t], "turn-1", AgentType::Codex).is_none());
    }

    /// DeepSeek reads BOTH halves, so both are sent — unlike codex, whose id
    /// half can never resolve.
    #[test]
    fn deepseek_sends_the_log_id_and_the_fingerprint_together() {
        let turns = vec![
            turn("turn-0", TurnRole::User, "hi", None),
            turn("turn-1", TurnRole::Assistant, "hello", Some("uuid-a2")),
        ];
        let point = resolve_fork_point(&turns, "turn-1", AgentType::DeepSeek).unwrap();
        assert_eq!(point.message_id, "uuid-a2");
        assert_eq!(
            point.message_fingerprint.as_deref(),
            Some(fingerprint_agent_message("hello").as_str())
        );
        assert_eq!(point.message_occurrence, Some(1));
    }

    /// A log that named nothing still forks by content, the codex shape: the id
    /// is codeg's own turn id, which DeepSeek cannot match, so it falls through
    /// to the fingerprint instead of failing.
    #[test]
    fn deepseek_falls_back_to_the_fingerprint_with_no_log_id() {
        let turns = vec![turn("turn-1", TurnRole::Assistant, "hello", None)];
        let point = resolve_fork_point(&turns, "turn-1", AgentType::DeepSeek).unwrap();
        assert_eq!(point.message_id, "turn-1");
        assert_eq!(
            point.message_fingerprint.as_deref(),
            Some(fingerprint_agent_message("hello").as_str())
        );
    }

    /// An id with no text is still a fork point on DeepSeek — a bubble whose
    /// whole turn was tool calls names a message the adapter can look up, which
    /// is exactly what codex cannot do.
    #[test]
    fn deepseek_forks_a_textless_turn_by_its_id_alone() {
        let mut t = turn("turn-1", TurnRole::Assistant, "", Some("uuid-a1"));
        t.blocks = Vec::new();
        let point = resolve_fork_point(&[t], "turn-1", AgentType::DeepSeek).unwrap();
        assert_eq!(point.message_id, "uuid-a1");
        // Nothing to hash, so no fingerprint — and none to count occurrences of.
        assert!(point.message_fingerprint.is_none());
        assert!(point.message_occurrence.is_none());
    }

    /// Neither half available (a bubble opened by a tool result alone) is not a
    /// fork point; the caller degrades to the tail.
    #[test]
    fn deepseek_declines_a_turn_with_neither_id_nor_text() {
        let mut t = turn("turn-1", TurnRole::Assistant, "", None);
        t.blocks = vec![ContentBlock::Text { text: "   ".into() }];
        assert!(resolve_fork_point(&[t], "turn-1", AgentType::DeepSeek).is_none());
    }

    /// The fingerprint half counts the same way codex's does, so a repeated
    /// answer in a log that named nothing still forks at the one clicked.
    #[test]
    fn deepseek_counts_repeated_answers() {
        let turns = vec![
            turn("turn-0", TurnRole::Assistant, "same", None),
            turn("turn-1", TurnRole::User, "again", None),
            turn("turn-2", TurnRole::Assistant, "same", None),
        ];
        assert_eq!(
            resolve_fork_point(&turns, "turn-2", AgentType::DeepSeek)
                .unwrap()
                .message_occurrence,
            Some(2)
        );
    }

    /// Forking "up to" a user turn is what a plain fork-send already does, and
    /// no adapter resolves an id against a user message.
    #[test]
    fn user_turns_are_not_fork_points() {
        let turns = vec![turn("turn-0", TurnRole::User, "hi", Some("msg_01"))];
        assert!(resolve_fork_point(&turns, "turn-0", AgentType::ClaudeCode).is_none());
        assert!(resolve_fork_point(&turns, "turn-0", AgentType::Codex).is_none());
        assert!(resolve_fork_point(&turns, "turn-0", AgentType::DeepSeek).is_none());
    }

    /// Every other agent forks at the tail — advertising a fork point they do
    /// not implement would silently change what their fork means.
    #[test]
    fn other_agents_have_no_fork_point() {
        let turns = vec![turn("turn-1", TurnRole::Assistant, "hello", Some("msg_01"))];
        for agent in [AgentType::Gemini, AgentType::Grok, AgentType::Custom("acme")] {
            assert!(resolve_fork_point(&turns, "turn-1", agent).is_none());
        }
    }

    #[test]
    fn unknown_turn_id_is_not_a_fork_point() {
        let turns = vec![turn("turn-1", TurnRole::Assistant, "hello", Some("msg_01"))];
        assert!(resolve_fork_point(&turns, "turn-9", AgentType::ClaudeCode).is_none());
    }

    /// Pinned against a digest computed outside this crate, so a change of hash
    /// or encoding cannot pass by agreeing with itself.
    #[test]
    fn fingerprint_is_sha256_hex() {
        assert_eq!(
            fingerprint_agent_message("abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
