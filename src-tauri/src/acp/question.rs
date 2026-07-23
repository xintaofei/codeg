//! Interactive multiple-choice question ("ask the user") domain types.
//!
//! Mid-turn an agent can ask the user one or more multiple-choice questions and
//! BLOCK until the user answers — the `ask_user_question` MCP tool exposed by
//! `codeg-mcp`. Unlike live-feedback ([`crate::acp::feedback`]), which is a
//! non-blocking pull the user pushes into, a question PAUSES the agent's tool
//! call: the questions render as an interactive card above the conversation
//! input box (driven by [`crate::acp::session_state::SessionState`], in-memory
//! and turn-scoped — it is real-time steering, not durable history), and the
//! tool call returns only once the user submits their choices.
//!
//! This module holds the pieces shared across layers so the manager, the
//! delegation listener, the MCP companion plumbing, and the settings command
//! don't each grow their own copy:
//!   * [`QuestionSpec`] / [`QuestionOption`] — one question + its choices.
//!   * [`PendingQuestionState`] — the awaiting-answer set stored on the session.
//!   * [`QuestionAnswer`] / [`QuestionAnswerItem`] — the user's submission
//!     (frontend → backend).
//!   * [`QuestionOutcome`] / [`QuestionAnsweredItem`] — the self-describing
//!     result handed back to the blocked tool (so the companion can render it
//!     without re-holding the questions).
//!   * [`SessionQuestionAccess`] — the listener-facing trait the production
//!     `ConnectionManager` implements (kept here so the listener can be unit
//!     tested with an in-memory stub, mirroring `SessionFeedbackAccess`).
//!   * [`QuestionRuntimeConfig`] — the hot-swappable "is the feature on?" flag,
//!     read at MCP injection time (mirrors [`crate::acp::feedback`]).

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{oneshot, RwLock};

/// Max questions per `ask_user_question` call. Matches Claude Code's
/// `AskUserQuestion` contract; the JSON schema advertises the same `maxItems`.
pub const MAX_QUESTIONS: usize = 4;
/// Min / max selectable options per question. Fewer than two options is not a
/// meaningful choice; more than four overwhelms the card. Matches Claude Code.
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 4;
/// Max characters for a question's short `header` chip.
pub const MAX_HEADER_CHARS: usize = 12;
/// Per-field sanity bound (characters) for every agent/user-supplied free-text
/// field: the question text, each option label + description, and the free-text
/// "Other" answer. The full text rides in the broadcast event, the snapshot, and
/// the agent-facing tool result, so this caps the blast radius of a pathological
/// field — whether from a malformed agent (`parse_questions`) or a hand-rolled
/// client hitting `acp_answer_question` directly (`build_outcome`). The UI can't
/// produce anything this long.
pub const MAX_QUESTION_TEXT_CHARS: usize = 4096;

/// One selectable choice in a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    /// Concise display text. A recommended option puts itself first and ends
    /// its label with "(Recommended)" (a string convention, like Claude Code).
    pub label: String,
    /// What this choice means / its trade-off. May be empty.
    pub description: String,
}

/// A single multiple-choice question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionSpec {
    /// Backend-minted stable id. Used as the answer correlation key instead of
    /// the question text (which Claude Code keys on) so duplicate question
    /// strings or reordering can't collide.
    pub id: String,
    /// The full question shown to the user.
    pub question: String,
    /// Short category label (≤ [`MAX_HEADER_CHARS`]) rendered as a chip.
    pub header: String,
    /// When true the user may select multiple options.
    pub multi_select: bool,
    /// The choices ([`MIN_OPTIONS`]..=[`MAX_OPTIONS`]).
    pub options: Vec<QuestionOption>,
}

/// The pending (awaiting-answer) question set stored on
/// `SessionState.pending_question` and carried on `to_snapshot()` so a client
/// attaching mid-turn (cold attach, reconnect, another window) re-renders the
/// card even though the one-shot `QuestionRequest` event won't replay for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingQuestionState {
    pub question_id: String,
    pub questions: Vec<QuestionSpec>,
    pub created_at: DateTime<Utc>,
}

/// One question's answer (frontend → backend). `labels` carries the selected
/// option labels (and any free-text "Other" the user typed, which the host UI
/// always offers); single-select submits exactly one label. camelCase on the
/// wire — this is constructed by the frontend, not read from an event payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAnswerItem {
    pub question_id: String,
    pub labels: Vec<String>,
}

/// The user's full submission for a pending question set (frontend → backend →
/// the blocked tool). `declined` is set when the user dismissed the card
/// without choosing — the agent then proceeds with its own judgment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    #[serde(default)]
    pub answers: Vec<QuestionAnswerItem>,
    #[serde(default)]
    pub declined: bool,
}

/// One answered question, joined with its prompt text so the result is
/// self-describing (the companion renders it without holding the questions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnsweredItem {
    pub question: String,
    pub header: String,
    pub multi_select: bool,
    /// The labels the user chose (or typed via "Other").
    pub selected: Vec<String>,
}

/// The resolved outcome delivered over the broker socket to the blocked tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOutcome {
    #[serde(default)]
    pub answers: Vec<QuestionAnsweredItem>,
    #[serde(default)]
    pub declined: bool,
}

/// What [`SessionQuestionAccess::register_question`] hands back to the listener:
/// the new question id plus the receiver to await the user's answer on.
pub struct RegisteredQuestion {
    pub question_id: String,
    pub answer_rx: oneshot::Receiver<QuestionOutcome>,
}

/// Listener-facing access to register / cancel a pending question on a parent
/// connection. The production impl (`ConnectionManagerQuestionLookup`) wraps the
/// `ConnectionManager`; tests use an in-memory stub. Mirrors
/// [`crate::acp::feedback::SessionFeedbackAccess`] and
/// `crate::acp::delegation::listener::ParentSessionLookup`.
#[async_trait]
pub trait SessionQuestionAccess: Send + Sync {
    /// Register a question set on the parent connection (resolved from the
    /// per-launch token), broadcast it to every attached client, and return a
    /// receiver that resolves when the user answers (or the question is
    /// canceled). `None` when the connection is gone — nothing to ask.
    async fn register_question(
        &self,
        parent_connection_id: &str,
        questions: Vec<QuestionSpec>,
    ) -> Option<RegisteredQuestion>;

    /// Cancel a pending question — the companion's tool call was canceled
    /// (peer-close) or the connection is tearing down. Removes it and clears
    /// the card on every client. No-op if it was already answered / gone.
    async fn cancel_question(&self, parent_connection_id: &str, question_id: &str);

    /// Cancel every pending question parked on a connection that is tearing
    /// down. Called from the `run_connection` cleanup guard (alongside the
    /// delegation `cancel_by_parent` cascade) so a question entry — and the
    /// listener task parked on it — is reclaimed synchronously on disconnect,
    /// rather than lingering until the companion's ask socket happens to close.
    /// No-op when the connection has no pending ask.
    async fn cancel_questions_by_parent(&self, parent_connection_id: &str);
}

/// Validate + parse the MCP `ask_user_question` arguments into typed
/// [`QuestionSpec`]s, minting a stable id per question. Enforces the contract
/// (1..=[`MAX_QUESTIONS`] questions, each with a non-empty question + header
/// ≤ [`MAX_HEADER_CHARS`] and [`MIN_OPTIONS`]..=[`MAX_OPTIONS`] labeled options)
/// so a malformed call is rejected synchronously with a helpful message the LLM
/// can fix, rather than round-tripping bad data. `multiSelect` defaults to
/// false; an option `description` defaults to empty (lenient).
pub fn parse_questions(arguments: &Value) -> Result<Vec<QuestionSpec>, String> {
    let arr = arguments
        .get("questions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "ask_user_question requires a `questions` array".to_string())?;
    if arr.is_empty() {
        return Err("ask_user_question requires at least one question".to_string());
    }
    if arr.len() > MAX_QUESTIONS {
        return Err(format!(
            "ask_user_question supports at most {MAX_QUESTIONS} questions per call"
        ));
    }
    let mut out = Vec::with_capacity(arr.len());
    for (qi, q) in arr.iter().enumerate() {
        let question = q
            .get("question")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("questions[{qi}] is missing a non-empty `question`"))?;
        if question.chars().count() > MAX_QUESTION_TEXT_CHARS {
            return Err(format!(
                "questions[{qi}] `question` exceeds {MAX_QUESTION_TEXT_CHARS} characters"
            ));
        }
        let header = q
            .get("header")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("questions[{qi}] is missing a non-empty `header`"))?;
        if header.chars().count() > MAX_HEADER_CHARS {
            return Err(format!(
                "questions[{qi}] `header` exceeds {MAX_HEADER_CHARS} characters"
            ));
        }
        let multi_select = q
            .get("multiSelect")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let opts = q
            .get("options")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("questions[{qi}] is missing an `options` array"))?;
        if opts.len() < MIN_OPTIONS || opts.len() > MAX_OPTIONS {
            return Err(format!(
                "questions[{qi}] must have between {MIN_OPTIONS} and {MAX_OPTIONS} options"
            ));
        }
        let mut options = Vec::with_capacity(opts.len());
        for (oi, o) in opts.iter().enumerate() {
            let label = o
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    format!("questions[{qi}].options[{oi}] is missing a non-empty `label`")
                })?;
            if label.chars().count() > MAX_QUESTION_TEXT_CHARS {
                return Err(format!(
                    "questions[{qi}].options[{oi}] `label` exceeds {MAX_QUESTION_TEXT_CHARS} characters"
                ));
            }
            let description = o
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if description.chars().count() > MAX_QUESTION_TEXT_CHARS {
                return Err(format!(
                    "questions[{qi}].options[{oi}] `description` exceeds {MAX_QUESTION_TEXT_CHARS} characters"
                ));
            }
            options.push(QuestionOption {
                label: label.to_string(),
                description,
            });
        }
        // Reject duplicate option labels within a question: the UI uses the
        // label as both the React key and the selection identity, and the
        // answer is submitted by label — duplicates would be ambiguous (select
        // one, select both) and collide on the key.
        let mut seen_labels = std::collections::HashSet::new();
        for o in &options {
            if !seen_labels.insert(o.label.as_str()) {
                return Err(format!(
                    "questions[{qi}] has duplicate option label {:?}",
                    o.label
                ));
            }
        }
        out.push(QuestionSpec {
            id: uuid::Uuid::new_v4().to_string(),
            question: question.to_string(),
            header: header.to_string(),
            multi_select,
            options,
        });
    }
    Ok(out)
}

/// Re-assert the [`parse_questions`] count + size bounds on already-typed specs.
/// The companion validates before sending, but the broker socket is only
/// token-gated, so a hand-rolled client could bypass that path and ride
/// oversized or malformed specs straight into the broadcast `QuestionRequest`
/// event and the `pending_question` snapshot. The listener registers through
/// this, declining the ask on `Err` rather than trusting unbounded input — the
/// authoritative answer-side bounds already live in [`build_outcome`], so this
/// closes the matching gap on the request side. Bounds mirror `parse_questions`.
pub fn validate_specs(specs: &[QuestionSpec]) -> Result<(), String> {
    if specs.is_empty() || specs.len() > MAX_QUESTIONS {
        return Err(format!(
            "expected 1..={MAX_QUESTIONS} questions, got {}",
            specs.len()
        ));
    }
    let mut seen_ids = std::collections::HashSet::new();
    for (qi, q) in specs.iter().enumerate() {
        // `parse_questions` mints a fresh uuid per question; a hand-rolled client
        // could send empty / colliding ids, and the answer routing + UI state map
        // key on `id`, so duplicates would misroute or collide.
        if q.id.trim().is_empty() {
            return Err(format!("questions[{qi}] has an empty `id`"));
        }
        if !seen_ids.insert(q.id.as_str()) {
            return Err(format!("questions[{qi}] has a duplicate `id` {:?}", q.id));
        }
        if q.question.trim().is_empty() {
            return Err(format!("questions[{qi}] has an empty `question`"));
        }
        if q.question.chars().count() > MAX_QUESTION_TEXT_CHARS {
            return Err(format!(
                "questions[{qi}] `question` exceeds {MAX_QUESTION_TEXT_CHARS} characters"
            ));
        }
        if q.header.trim().is_empty() {
            return Err(format!("questions[{qi}] has an empty `header`"));
        }
        if q.header.chars().count() > MAX_HEADER_CHARS {
            return Err(format!(
                "questions[{qi}] `header` exceeds {MAX_HEADER_CHARS} characters"
            ));
        }
        if q.options.len() < MIN_OPTIONS || q.options.len() > MAX_OPTIONS {
            return Err(format!(
                "questions[{qi}] must have between {MIN_OPTIONS} and {MAX_OPTIONS} options"
            ));
        }
        let mut seen_labels = std::collections::HashSet::new();
        for (oi, o) in q.options.iter().enumerate() {
            if o.label.trim().is_empty() {
                return Err(format!("questions[{qi}].options[{oi}] has an empty `label`"));
            }
            if o.label.chars().count() > MAX_QUESTION_TEXT_CHARS {
                return Err(format!(
                    "questions[{qi}].options[{oi}] `label` exceeds {MAX_QUESTION_TEXT_CHARS} characters"
                ));
            }
            // Mirror parse_questions: labels are the React key + selection identity
            // and answers are submitted by label, so duplicates (trimmed) are
            // ambiguous.
            if !seen_labels.insert(o.label.trim()) {
                return Err(format!(
                    "questions[{qi}] has a duplicate option label {:?}",
                    o.label
                ));
            }
            if o.description.chars().count() > MAX_QUESTION_TEXT_CHARS {
                return Err(format!(
                    "questions[{qi}].options[{oi}] `description` exceeds {MAX_QUESTION_TEXT_CHARS} characters"
                ));
            }
        }
    }
    Ok(())
}

/// Join the user's submission with the original questions into a self-describing
/// [`QuestionOutcome`], normalizing + validating against the stored specs. The
/// UI enforces these rules, but `acp_answer_question` is a plain API a stale or
/// hand-rolled client can hit directly, so the authoritative checks live here.
///
/// Iterates the TRUSTED `questions` (≤ [`MAX_QUESTIONS`]), not the client's
/// `answers`, so a flood of unknown / duplicate answer items can neither grow an
/// intermediate set nor bloat the output — extra items are simply never looked
/// up. For each spec question it takes the first matching answer (dedup) and:
///   * trims each label, drops empties, bounds each to [`MAX_QUESTION_TEXT_CHARS`];
///   * caps the count — single-select keeps 1, multi-select keeps at most every
///     real option plus one free-text "Other" (`options.len() + 1`);
///   * drops a question left with no usable label.
///
/// Output is therefore bounded by the question set's own size, in asked order.
/// A declined submission yields an empty, `declined: true` outcome.
pub fn build_outcome(questions: &[QuestionSpec], answer: &QuestionAnswer) -> QuestionOutcome {
    if answer.declined {
        return QuestionOutcome {
            answers: Vec::new(),
            declined: true,
        };
    }
    let answers = questions
        .iter()
        .filter_map(|spec| {
            let a = answer.answers.iter().find(|a| a.question_id == spec.id)?;
            // Cap selections to the question's own size: single-select → 1;
            // multi-select → every real option plus one "Other". Enforce the cap
            // DURING iteration (early break, allocate only kept labels) so a
            // pathological `labels` array can't do unbounded intermediate work.
            let cap = if spec.multi_select {
                spec.options.len() + 1
            } else {
                1
            };
            let mut labels: Vec<String> = Vec::with_capacity(cap);
            for l in &a.labels {
                if labels.len() == cap {
                    break;
                }
                let trimmed = l.trim();
                if trimmed.is_empty() {
                    continue;
                }
                labels.push(trimmed.chars().take(MAX_QUESTION_TEXT_CHARS).collect());
            }
            if labels.is_empty() {
                return None;
            }
            Some(QuestionAnsweredItem {
                question: spec.question.clone(),
                header: spec.header.clone(),
                multi_select: spec.multi_select,
                selected: labels,
            })
        })
        .collect();
    QuestionOutcome {
        answers,
        declined: false,
    }
}

/// Grok's native `ask_user_question` tool has NO `header` (the short category
/// chip codeg renders); synthesize one from the leading characters of the
/// question text, bounded to [`MAX_HEADER_CHARS`]. Always returns a non-empty,
/// in-bounds string so [`validate_specs`] accepts it.
fn synthesize_grok_header(question: &str) -> String {
    let header: String = question.trim().chars().take(MAX_HEADER_CHARS).collect();
    let header = header.trim();
    if header.is_empty() {
        "Ask".to_string()
    } else {
        header.to_string()
    }
}

/// Convert grok's native `_x.ai/ask_user_question` ext-request params into codeg
/// [`QuestionSpec`]s, so grok's own questions render in the SAME interactive card
/// as the `codeg-mcp` ask tool (grok emits an ACP ext request and blocks on the
/// reply; if codeg doesn't answer it, grok falls back to inert fire-and-forget
/// rendering — see the connection handler). Grok's wire shape per question is
/// `{question, options:[{label, description}], multiSelect}` — no `header`, which
/// we synthesize. Counts are clamped to codeg's bounds because
/// [`crate::acp::manager::ConnectionManager::register_question`] re-runs
/// [`validate_specs`] and would otherwise decline the whole ask: more than
/// [`MAX_QUESTIONS`] questions or [`MAX_OPTIONS`] options are truncated (logged,
/// never silently dropped), and duplicate option labels — which `validate_specs`
/// rejects as ambiguous — are deduped. A question left with fewer than
/// [`MIN_OPTIONS`] usable options is not a real choice and fails the request
/// (grok then falls back — no worse than before the bridge existed).
pub fn parse_grok_ext_questions(params: &Value) -> Result<Vec<QuestionSpec>, String> {
    let arr = params
        .get("questions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "ask_user_question ext request missing `questions` array".to_string())?;
    if arr.is_empty() {
        return Err("ask_user_question ext request has no questions".to_string());
    }
    if arr.len() > MAX_QUESTIONS {
        tracing::warn!(
            "[grok ask] dropping {} question(s) past the max of {MAX_QUESTIONS}",
            arr.len() - MAX_QUESTIONS
        );
    }
    let mut out = Vec::with_capacity(arr.len().min(MAX_QUESTIONS));
    for (qi, q) in arr.iter().take(MAX_QUESTIONS).enumerate() {
        let question = q
            .get("question")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("questions[{qi}] is missing a non-empty `question`"))?;
        let multi_select = q
            .get("multiSelect")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let opts = q
            .get("options")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("questions[{qi}] is missing an `options` array"))?;
        if opts.len() > MAX_OPTIONS {
            tracing::warn!(
                "[grok ask] questions[{qi}] has {} options; truncating to {MAX_OPTIONS}",
                opts.len()
            );
        }
        let mut options = Vec::with_capacity(opts.len().min(MAX_OPTIONS));
        let mut seen_labels = std::collections::HashSet::new();
        for o in opts {
            if options.len() == MAX_OPTIONS {
                break;
            }
            let label = o
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let Some(label) = label else { continue };
            // `validate_specs` rejects duplicate labels (the label is the card's
            // React key + selection identity); dedup rather than fail the ask.
            if !seen_labels.insert(label.to_string()) {
                continue;
            }
            let description: String = o
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .chars()
                .take(MAX_QUESTION_TEXT_CHARS)
                .collect();
            options.push(QuestionOption {
                label: label.chars().take(MAX_QUESTION_TEXT_CHARS).collect(),
                description,
            });
        }
        if options.len() < MIN_OPTIONS {
            return Err(format!(
                "questions[{qi}] has fewer than {MIN_OPTIONS} usable options"
            ));
        }
        out.push(QuestionSpec {
            id: uuid::Uuid::new_v4().to_string(),
            question: question.chars().take(MAX_QUESTION_TEXT_CHARS).collect(),
            header: synthesize_grok_header(question),
            multi_select,
            options,
        });
    }
    Ok(out)
}

/// Serialize a resolved [`QuestionOutcome`] into grok's `AskUserQuestionExtResponse`
/// — the reply to a `_x.ai/ask_user_question` ext request. Verified against grok
/// 0.2.101 on a real run: the response is internally tagged by `outcome`; the
/// `accepted` variant carries an `answers` map keyed by the QUESTION TEXT (grok's
/// questions have no id and it correlates the answer by text) whose value is a
/// bare string for single-select or an array for multi-select (grok's
/// `StringOrVec`), plus an empty `partial_answers`. A declined card maps to
/// `skip_interview` (a variant grok's enum accepts). A wrong shape only makes grok
/// fall back to inert rendering, so this can never regress the pre-bridge state.
pub fn build_grok_ext_response(outcome: &QuestionOutcome) -> Value {
    if outcome.declined {
        return grok_ext_skip_response();
    }
    let mut answers = serde_json::Map::new();
    for item in &outcome.answers {
        let value = if item.multi_select {
            Value::Array(item.selected.iter().cloned().map(Value::String).collect())
        } else {
            // Single-select keeps exactly one label (capped in `build_outcome`);
            // grok's `StringOrVec` wants a bare string here, not a 1-element array.
            match item.selected.first() {
                Some(label) => Value::String(label.clone()),
                None => continue,
            }
        };
        answers.insert(item.question.clone(), value);
    }
    serde_json::json!({
        "outcome": "accepted",
        "answers": Value::Object(answers),
        "partial_answers": Value::Object(serde_json::Map::new()),
    })
}

/// The `_x.ai/ask_user_question` reply for a declined card or an ask that was
/// canceled/torn down before the user submitted (the answer one-shot dropped).
/// Grok's `skip_interview` outcome — chosen over `cancelled` because the ACP
/// response enum only exposes `Accepted`/`ChatAboutThis`/`SkipInterview`.
pub fn grok_ext_skip_response() -> Value {
    serde_json::json!({ "outcome": "skip_interview" })
}

/// Build the `raw_input` (questions) for the in-stream `AskQuestionResultCard`
/// codeg synthesizes for a grok native ask. Grok never emits a *completed* tool
/// result into the ACP `updates.jsonl` stream (it resolves the answer over the
/// `_x.ai/ask_user_question` ext round-trip instead), so the connection handler
/// emits this itself once the user submits — see `handle_grok_ask_user_question`.
///
/// Deliberately omits `header`: grok's native questions have none (the chip
/// header we synthesize for the interactive card is an internal detail), so the
/// frontend parses `header:""` here. The paired [`grok_result_card_output`] uses
/// the SAME empty header, and the history parser (`grok.rs`) emits `header:""`
/// too, so the card's answer↔question match key (`header + question`) aligns and
/// a conversation renders identically live and after reload. Built from the
/// already-clamped [`QuestionSpec`]s so input and output stay in lockstep.
pub fn grok_result_card_input(specs: &[QuestionSpec]) -> Value {
    let questions: Vec<Value> = specs
        .iter()
        .map(|s| {
            serde_json::json!({
                "question": s.question,
                "multiSelect": s.multi_select,
                "options": s.options,
            })
        })
        .collect();
    serde_json::json!({ "questions": questions })
}

/// Build the `raw_output` (`{answers, declined}` envelope) for the in-stream
/// `AskQuestionResultCard` — the codeg-mcp `structuredContent` shape the frontend
/// already parses (`parseAskQuestionOutcome`). Emits `header:""` to match
/// [`grok_result_card_input`] (see its docs). Companion to [`grok_result_card_input`].
pub fn grok_result_card_output(outcome: &QuestionOutcome) -> Value {
    let answers: Vec<Value> = outcome
        .answers
        .iter()
        .map(|a| {
            serde_json::json!({
                "header": "",
                "question": a.question,
                "multi_select": a.multi_select,
                "selected": a.selected,
            })
        })
        .collect();
    serde_json::json!({ "answers": answers, "declined": outcome.declined })
}

/// The hot-swappable feature config read at MCP injection time. Kept tiny and
/// separate from `FeedbackConfig` / `DelegationConfig` so the three features
/// toggle independently — `codeg-mcp` is injected when ANY is enabled, and each
/// tool is listed only when its own feature is on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuestionConfig {
    pub enabled: bool,
}

/// Shared, hot-swappable handle to [`QuestionConfig`]. Cloned into
/// `DelegationInjection` (read at injection) and `AppState` (updated on save).
/// Mirrors [`crate::acp::feedback::FeedbackRuntimeConfig`].
#[derive(Clone, Default)]
pub struct QuestionRuntimeConfig {
    inner: Arc<RwLock<QuestionConfig>>,
}

impl QuestionRuntimeConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn snapshot(&self) -> QuestionConfig {
        self.inner.read().await.clone()
    }

    pub async fn set(&self, cfg: QuestionConfig) {
        *self.inner.write().await = cfg;
    }

    /// Convenience read used at MCP injection time.
    pub async fn is_enabled(&self) -> bool {
        self.inner.read().await.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_args() -> Value {
        json!({
            "questions": [{
                "question": "Which approach?",
                "header": "Approach",
                "multiSelect": false,
                "options": [
                    { "label": "Incremental", "description": "smaller diffs" },
                    { "label": "Rewrite", "description": "clean slate" }
                ]
            }]
        })
    }

    #[test]
    fn parse_questions_happy_path_mints_ids() {
        let qs = parse_questions(&valid_args()).unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].question, "Which approach?");
        assert_eq!(qs[0].header, "Approach");
        assert!(!qs[0].multi_select);
        assert_eq!(qs[0].options.len(), 2);
        assert!(!qs[0].id.is_empty());
    }

    #[test]
    fn validate_specs_accepts_well_formed_and_rejects_malformed() {
        // What parse_questions mints passes the request-side re-check.
        let good = parse_questions(&valid_args()).unwrap();
        assert!(validate_specs(&good).is_ok());

        // Build a spec with a tunable question, option count, and first-label
        // length so each bound can be tripped independently.
        let spec = |question: &str, options: usize, first_label_len: usize| QuestionSpec {
            id: "q".into(),
            question: question.into(),
            header: "H".into(),
            multi_select: false,
            options: (0..options)
                .map(|i| QuestionOption {
                    label: if first_label_len > 0 && i == 0 {
                        "x".repeat(first_label_len)
                    } else {
                        format!("opt{i}")
                    },
                    description: String::new(),
                })
                .collect(),
        };

        assert!(validate_specs(&[]).is_err(), "empty set");
        assert!(
            validate_specs(&[spec(&"q".repeat(MAX_QUESTION_TEXT_CHARS + 1), 2, 0)]).is_err(),
            "oversized question text"
        );
        assert!(
            validate_specs(&[spec("ok", MIN_OPTIONS - 1, 0)]).is_err(),
            "too few options"
        );
        assert!(
            validate_specs(&[spec("ok", MAX_OPTIONS + 1, 0)]).is_err(),
            "too many options"
        );
        assert!(
            validate_specs(&[spec("ok", 2, MAX_QUESTION_TEXT_CHARS + 1)]).is_err(),
            "oversized option label"
        );
        assert!(validate_specs(&[spec("   ", 2, 0)]).is_err(), "blank question");

        // Duplicate question id across the set (spec() hardcodes id "q") — answer
        // routing + UI state key on id, so duplicates must be rejected.
        assert!(
            validate_specs(&[spec("a", 2, 0), spec("b", 2, 0)]).is_err(),
            "duplicate question id"
        );
        let blank_id = QuestionSpec {
            id: "  ".into(),
            question: "ok".into(),
            header: "H".into(),
            multi_select: false,
            options: vec![
                QuestionOption {
                    label: "a".into(),
                    description: String::new(),
                },
                QuestionOption {
                    label: "b".into(),
                    description: String::new(),
                },
            ],
        };
        assert!(validate_specs(&[blank_id]).is_err(), "blank id");
        // Duplicate option label within one question (parse_questions rejects it).
        let dup_label = QuestionSpec {
            id: "q".into(),
            question: "ok".into(),
            header: "H".into(),
            multi_select: false,
            options: vec![
                QuestionOption {
                    label: "same".into(),
                    description: String::new(),
                },
                QuestionOption {
                    label: " same ".into(),
                    description: String::new(),
                },
            ],
        };
        assert!(
            validate_specs(&[dup_label]).is_err(),
            "duplicate option label (trimmed)"
        );
    }

    #[test]
    fn parse_questions_rejects_empty_and_overlong_sets() {
        assert!(parse_questions(&json!({ "questions": [] })).is_err());
        assert!(parse_questions(&json!({})).is_err());
        let mut many = Vec::new();
        for _ in 0..(MAX_QUESTIONS + 1) {
            many.push(json!({
                "question": "q", "header": "h", "multiSelect": false,
                "options": [{ "label": "a", "description": "" }, { "label": "b", "description": "" }]
            }));
        }
        assert!(parse_questions(&json!({ "questions": many })).is_err());
    }

    #[test]
    fn parse_questions_enforces_option_count_and_header_len() {
        // One option is not a choice.
        let one_opt = json!({ "questions": [{
            "question": "q", "header": "h", "multiSelect": false,
            "options": [{ "label": "only", "description": "" }]
        }] });
        assert!(parse_questions(&one_opt).is_err());
        // Header too long.
        let long_header = json!({ "questions": [{
            "question": "q", "header": "this-header-is-way-too-long", "multiSelect": false,
            "options": [{ "label": "a", "description": "" }, { "label": "b", "description": "" }]
        }] });
        assert!(parse_questions(&long_header).is_err());
    }

    #[test]
    fn build_outcome_maps_labels_by_id_and_drops_unknown() {
        let qs = parse_questions(&valid_args()).unwrap();
        let qid = qs[0].id.clone();
        let answer = QuestionAnswer {
            answers: vec![
                QuestionAnswerItem {
                    question_id: qid,
                    labels: vec!["Incremental".into()],
                },
                QuestionAnswerItem {
                    question_id: "does-not-exist".into(),
                    labels: vec!["ghost".into()],
                },
            ],
            declined: false,
        };
        let outcome = build_outcome(&qs, &answer);
        assert!(!outcome.declined);
        assert_eq!(outcome.answers.len(), 1);
        assert_eq!(outcome.answers[0].question, "Which approach?");
        assert_eq!(outcome.answers[0].selected, vec!["Incremental".to_string()]);
    }

    #[test]
    fn parse_questions_rejects_overlong_option_label() {
        let huge = "x".repeat(MAX_QUESTION_TEXT_CHARS + 1);
        let bad = json!({ "questions": [{
            "question": "q", "header": "h", "multiSelect": false,
            "options": [
                { "label": huge, "description": "" },
                { "label": "B", "description": "" }
            ]
        }] });
        let err = parse_questions(&bad).unwrap_err();
        assert!(err.contains("exceeds"));
    }

    #[test]
    fn build_outcome_caps_multiselect_labels_and_ignores_unknown() {
        // A multi-select question with 2 options: a flood of submitted labels
        // plus a flood of unknown answer items must NOT bloat the outcome.
        let args = json!({ "questions": [{
            "question": "Which modules?", "header": "Scope", "multiSelect": true,
            "options": [
                { "label": "auth", "description": "" },
                { "label": "billing", "description": "" }
            ]
        }] });
        let qs = parse_questions(&args).unwrap();
        let qid = qs[0].id.clone();
        let mut items = vec![QuestionAnswerItem {
            question_id: qid,
            labels: (0..1000).map(|i| format!("l{i}")).collect(),
        }];
        // 10k unknown answer items — must be ignored without growth.
        for i in 0..10_000 {
            items.push(QuestionAnswerItem {
                question_id: format!("ghost-{i}"),
                labels: vec!["x".into()],
            });
        }
        let outcome = build_outcome(&qs, &QuestionAnswer { answers: items, declined: false });
        assert_eq!(outcome.answers.len(), 1);
        // Cap = options.len() + 1 = 3 (every real option plus one "Other"); the
        // FIRST three are kept (early break — labels past the cap and the 10k
        // unknown items are never processed/retained).
        assert_eq!(
            outcome.answers[0].selected,
            vec!["l0".to_string(), "l1".to_string(), "l2".to_string()]
        );
    }

    #[test]
    fn parse_questions_rejects_duplicate_option_labels() {
        let dup = json!({ "questions": [{
            "question": "q", "header": "h", "multiSelect": false,
            "options": [
                { "label": "Same", "description": "a" },
                { "label": "Same", "description": "b" }
            ]
        }] });
        let err = parse_questions(&dup).unwrap_err();
        assert!(err.contains("duplicate"));
    }

    #[test]
    fn build_outcome_normalizes_malformed_answer() {
        // Single-select with two labels + an empty + an oversize one, plus a
        // duplicate answer item and an unknown question id. The endpoint must
        // not trust this; build_outcome sanitizes it.
        let qs = parse_questions(&valid_args()).unwrap();
        let qid = qs[0].id.clone();
        let huge = "x".repeat(MAX_QUESTION_TEXT_CHARS + 50);
        let answer = QuestionAnswer {
            answers: vec![
                QuestionAnswerItem {
                    question_id: qid.clone(),
                    labels: vec!["  ".into(), "Incremental".into(), huge.clone()],
                },
                // Duplicate item for the same question — must be deduped (first wins).
                QuestionAnswerItem {
                    question_id: qid,
                    labels: vec!["Rewrite".into()],
                },
                // Unknown question — dropped.
                QuestionAnswerItem {
                    question_id: "ghost".into(),
                    labels: vec!["x".into()],
                },
            ],
            declined: false,
        };
        let outcome = build_outcome(&qs, &answer);
        assert_eq!(outcome.answers.len(), 1, "deduped + unknown dropped");
        // Single-select: empty trimmed away, then truncated to one → "Incremental".
        assert_eq!(outcome.answers[0].selected, vec!["Incremental".to_string()]);
    }

    #[test]
    fn build_outcome_drops_question_with_only_empty_labels() {
        let qs = parse_questions(&valid_args()).unwrap();
        let outcome = build_outcome(
            &qs,
            &QuestionAnswer {
                answers: vec![QuestionAnswerItem {
                    question_id: qs[0].id.clone(),
                    labels: vec!["   ".into(), "".into()],
                }],
                declined: false,
            },
        );
        assert!(outcome.answers.is_empty());
    }

    #[test]
    fn build_outcome_declined_is_empty() {
        let qs = parse_questions(&valid_args()).unwrap();
        let outcome = build_outcome(
            &qs,
            &QuestionAnswer {
                answers: vec![],
                declined: true,
            },
        );
        assert!(outcome.declined);
        assert!(outcome.answers.is_empty());
    }

    #[tokio::test]
    async fn runtime_config_hot_swaps() {
        let cfg = QuestionRuntimeConfig::new();
        assert!(!cfg.is_enabled().await);
        cfg.set(QuestionConfig { enabled: true }).await;
        assert!(cfg.is_enabled().await);
    }

    fn grok_params(questions: Value) -> Value {
        json!({
            "sessionId": "s-1",
            "toolCallId": "call-1",
            "questions": questions,
            "mode": "default",
        })
    }

    #[test]
    fn parse_grok_ext_maps_shape_and_synthesizes_header() {
        // Grok's wire shape: no `header`, camelCase `multiSelect`.
        let specs = parse_grok_ext_questions(&grok_params(json!([{
            "question": "What is your favorite color?",
            "options": [
                { "label": "Red", "description": "warm" },
                { "label": "Blue", "description": "cool" }
            ],
            "multiSelect": false
        }])))
        .unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].question, "What is your favorite color?");
        assert!(!specs[0].multi_select);
        assert_eq!(specs[0].options.len(), 2);
        // Header is synthesized, non-empty, and within the chip bound.
        assert!(!specs[0].header.is_empty());
        assert!(specs[0].header.chars().count() <= MAX_HEADER_CHARS);
        // Whatever we synthesize MUST satisfy the register-time re-validation,
        // else `register_question` silently declines the whole ask.
        validate_specs(&specs).expect("synthesized specs must pass validate_specs");
    }

    #[test]
    fn parse_grok_ext_clamps_questions_and_options_to_bounds() {
        // 6 questions, each with 6 options — both past codeg's maxima.
        let many: Vec<Value> = (0..6)
            .map(|qi| {
                let opts: Vec<Value> = (0..6)
                    .map(|oi| json!({ "label": format!("q{qi}o{oi}"), "description": "" }))
                    .collect();
                json!({ "question": format!("Question {qi}?"), "options": opts })
            })
            .collect();
        let specs = parse_grok_ext_questions(&grok_params(json!(many))).unwrap();
        assert_eq!(specs.len(), MAX_QUESTIONS, "questions clamped");
        assert!(specs.iter().all(|s| s.options.len() == MAX_OPTIONS), "options clamped");
        validate_specs(&specs).unwrap();
    }

    #[test]
    fn parse_grok_ext_dedups_option_labels() {
        // Duplicate labels would fail validate_specs; we dedup, keeping enough.
        let specs = parse_grok_ext_questions(&grok_params(json!([{
            "question": "Pick one",
            "options": [
                { "label": "A", "description": "" },
                { "label": "A", "description": "dup" },
                { "label": "B", "description": "" }
            ]
        }])))
        .unwrap();
        assert_eq!(specs[0].options.len(), 2);
        validate_specs(&specs).unwrap();
    }

    #[test]
    fn parse_grok_ext_rejects_degenerate_and_malformed() {
        // Fewer than two usable options is not a real choice → fail (grok falls back).
        assert!(parse_grok_ext_questions(&grok_params(json!([{
            "question": "Only one",
            "options": [{ "label": "Solo", "description": "" }]
        }])))
        .is_err());
        // Missing questions array.
        assert!(parse_grok_ext_questions(&json!({ "mode": "default" })).is_err());
        // Empty questions.
        assert!(parse_grok_ext_questions(&grok_params(json!([]))).is_err());
    }

    #[test]
    fn build_grok_ext_response_single_select_is_bare_string() {
        let outcome = QuestionOutcome {
            answers: vec![QuestionAnsweredItem {
                question: "What is your favorite color?".into(),
                header: "What is your".into(),
                multi_select: false,
                selected: vec!["Red".into()],
            }],
            declined: false,
        };
        let v = build_grok_ext_response(&outcome);
        assert_eq!(v["outcome"], "accepted");
        // Keyed by question text; single-select value is a bare string.
        assert_eq!(v["answers"]["What is your favorite color?"], json!("Red"));
        assert!(v["partial_answers"].as_object().unwrap().is_empty());
    }

    #[test]
    fn build_grok_ext_response_multi_select_is_array() {
        let outcome = QuestionOutcome {
            answers: vec![QuestionAnsweredItem {
                question: "Which modules?".into(),
                header: "Which module".into(),
                multi_select: true,
                selected: vec!["auth".into(), "billing".into()],
            }],
            declined: false,
        };
        let v = build_grok_ext_response(&outcome);
        assert_eq!(v["outcome"], "accepted");
        assert_eq!(v["answers"]["Which modules?"], json!(["auth", "billing"]));
    }

    #[test]
    fn build_grok_ext_response_declined_is_skip_interview() {
        let outcome = QuestionOutcome {
            answers: vec![],
            declined: true,
        };
        assert_eq!(build_grok_ext_response(&outcome), json!({ "outcome": "skip_interview" }));
        assert_eq!(grok_ext_skip_response(), json!({ "outcome": "skip_interview" }));
    }

    #[test]
    fn grok_result_card_input_omits_header_and_keeps_shape() {
        // Built from parsed specs (which DO carry a synthesized header) — the card
        // input must nonetheless drop it, so the frontend parses `header:""`.
        let specs = parse_grok_ext_questions(&grok_params(json!([{
            "question": "Which colors?",
            "options": [
                { "label": "Red", "description": "warm" },
                { "label": "Green", "description": "cool" }
            ],
            "multiSelect": true
        }])))
        .unwrap();
        let input = grok_result_card_input(&specs);
        let q = &input["questions"][0];
        assert_eq!(q["question"], "Which colors?");
        assert_eq!(q["multiSelect"], true);
        assert_eq!(q["options"][0]["label"], "Red");
        // No header field is serialized (the frontend reads it as "").
        assert!(q.get("header").is_none(), "input must not carry a header");
    }

    #[test]
    fn grok_result_card_output_single_select_empty_header() {
        let outcome = QuestionOutcome {
            answers: vec![QuestionAnsweredItem {
                question: "你更喜欢哪种演示方式？".into(),
                header: "你更喜欢哪种".into(), // synthesized upstream; must be dropped here
                multi_select: false,
                selected: vec!["随便看看".into()],
            }],
            declined: false,
        };
        let out = grok_result_card_output(&outcome);
        assert_eq!(out["declined"], false);
        let a = &out["answers"][0];
        // Header is forced empty to align with the header-less card input.
        assert_eq!(a["header"], "");
        assert_eq!(a["question"], "你更喜欢哪种演示方式？");
        assert_eq!(a["selected"], json!(["随便看看"]));
    }

    #[test]
    fn grok_result_card_output_multi_and_declined() {
        let multi = QuestionOutcome {
            answers: vec![QuestionAnsweredItem {
                question: "Which colors?".into(),
                header: "Which colors".into(),
                multi_select: true,
                selected: vec!["Red".into(), "Green".into()],
            }],
            declined: false,
        };
        assert_eq!(
            grok_result_card_output(&multi)["answers"][0]["selected"],
            json!(["Red", "Green"])
        );
        let declined = QuestionOutcome {
            answers: vec![],
            declined: true,
        };
        let out = grok_result_card_output(&declined);
        assert_eq!(out["declined"], true);
        assert_eq!(out["answers"], json!([]));
    }

    #[test]
    fn grok_result_card_input_output_question_texts_align() {
        // The frontend matches answers to questions by (header, question); with
        // header empty on both sides, the question text must match verbatim so the
        // capsule shows the pick rather than "未选择". `build_outcome` copies the
        // spec's question into the answer, so a full round trip stays aligned.
        let specs = parse_grok_ext_questions(&grok_params(json!([{
            "question": "Pick one",
            "options": [
                { "label": "A", "description": "" },
                { "label": "B", "description": "" }
            ]
        }])))
        .unwrap();
        let outcome = build_outcome(
            &specs,
            &QuestionAnswer {
                answers: vec![QuestionAnswerItem {
                    question_id: specs[0].id.clone(),
                    labels: vec!["A".into()],
                }],
                declined: false,
            },
        );
        let input = grok_result_card_input(&specs);
        let output = grok_result_card_output(&outcome);
        assert_eq!(input["questions"][0]["question"], output["answers"][0]["question"]);
        assert_eq!(output["answers"][0]["header"], "");
    }
}
