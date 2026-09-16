//! Mid-conversation agent handoff: switch an existing conversation to a
//! different agent in place, carrying its context over.
//!
//! Two paths, decided by [`plan_path`]:
//!
//! * **Native** (same family, Claude today): the agent's own transcript is
//!   copied into the target's home ([`copy_claude_session`]) and the target
//!   `session/load`s it under the SAME session id. Nothing is summarized; the
//!   target continues with the full context the source had.
//! * **Summary** (everything else): the target starts a fresh session seeded
//!   with a briefing ([`build_briefing`]) built from the conversation: a digest
//!   of every earlier turn, the last few turns in full, and the user's note.
//!
//! Either way the conversation ROW keeps its id: `conversation_service::
//! rebind_for_handoff` moves it to the new agent + session, and a
//! `conversation_handoff` record keeps the earlier segment reachable so the
//! detail read ([`splice_handoffs`]) renders the whole history with a divider
//! where the switch happened.
//!
//! Everything here is pure or filesystem-only so it can be unit tested; the
//! connection choreography lives in `commands::handoff`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::acp::registry::{self, AgentDistribution};
use crate::db::entities::conversation_handoff;
use crate::models::{AgentType, ContentBlock, MessageTurn, TurnRole};

/// First line of every seeded briefing. An HTML comment renders as nothing
/// wherever the prompt is shown as Markdown, and the detail read folds the
/// turn that starts with it into the handoff divider (`splice_handoffs`).
pub const BRIEFING_MARKER: &str = "<!-- codeg:handoff-briefing -->";

/// Upper bound on a seeded briefing, in characters. Generous for a long
/// conversation, and bounded so a pathological one cannot exhaust the target
/// agent's first turn.
pub const DEFAULT_BRIEFING_BUDGET: usize = 60_000;

/// `_meta` key on the divider tool call. Recognized by the frontend the same
/// way `contextCompaction` is: by key, never by agent.
pub const HANDOFF_META_KEY: &str = "codeg.handoff";

/// How many trailing turns the briefing carries in full before shrinking.
const VERBATIM_TURNS: usize = 6;
/// Fewest trailing turns the briefing keeps in full while shrinking.
const MIN_VERBATIM_TURNS: usize = 2;
/// Per-turn character caps for the digest, tried in order while shrinking.
const DIGEST_CAPS: [usize; 3] = [600, 300, 150];
/// Cap on one tool input/output preview inside the verbatim tail.
const TOOL_PREVIEW_CAP: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffPath {
    Native,
    Summary,
}

impl HandoffPath {
    pub fn as_str(self) -> &'static str {
        match self {
            HandoffPath::Native => "native",
            HandoffPath::Summary => "summary",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "native" => HandoffPath::Native,
            _ => HandoffPath::Summary,
        }
    }
}

/// Agent families whose sessions codeg can move between homes losslessly.
///
/// Claude only, deliberately: `claude-agent-acp`'s `session/load` resolves a
/// session id by scanning `<CLAUDE_CONFIG_DIR>/projects/*/<id>.jsonl`, so a
/// transcript copied into another home loads there under the same id
/// (verified against the real adapter). Codex and Grok keep session stores
/// too, but their load paths have not been verified with a copied file, and
/// advertising a native transfer that silently starts an empty session would
/// be worse than the summary path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFamily {
    Claude,
}

/// npm packages that wrap Claude Code as an ACP agent. An extra isolated
/// Claude account in codeg is a custom agent running one of these with its own
/// `CLAUDE_CONFIG_DIR`, so its sessions are Claude sessions.
pub fn is_claude_adapter_package(package: &str) -> bool {
    // `@scope/name@1.2.3` → `@scope/name`; an unversioned `@scope/name` has its
    // only `@` at index 0, which `rsplit_once` would misread as a version split.
    let name = package
        .rsplit_once('@')
        .map(|(name, _)| name)
        .filter(|name| !name.is_empty())
        .unwrap_or(package);
    matches!(
        name,
        "@agentclientprotocol/claude-agent-acp"
            | "@zed-industries/claude-agent-acp"
            | "@zed-industries/claude-code-acp"
    )
}

/// [`native_family`] against an explicit distribution, so the decision can be
/// tested without the process-global custom registry.
pub fn native_family_of(agent_type: AgentType, distribution: &AgentDistribution) -> Option<NativeFamily> {
    match agent_type {
        AgentType::ClaudeCode => Some(NativeFamily::Claude),
        AgentType::Custom(_) => match distribution {
            AgentDistribution::Npx { package, .. } if is_claude_adapter_package(package) => {
                Some(NativeFamily::Claude)
            }
            _ => None,
        },
        _ => None,
    }
}

pub fn native_family(agent_type: AgentType) -> Option<NativeFamily> {
    let meta = registry::get_agent_meta(agent_type);
    native_family_of(agent_type, &meta.distribution)
}

/// Which path a handoff between two agents takes. Same family on both sides
/// means the transcript can move; anything else is briefed.
pub fn plan_path(source: AgentType, target: AgentType) -> HandoffPath {
    plan_path_of(native_family(source), native_family(target))
}

pub fn plan_path_of(source: Option<NativeFamily>, target: Option<NativeFamily>) -> HandoffPath {
    match (source, target) {
        (Some(a), Some(b)) if a == b => HandoffPath::Native,
        _ => HandoffPath::Summary,
    }
}

/// The static env a distribution launches with (`merge_agent_env` applies it
/// under the per-agent runtime env).
pub fn distribution_env(distribution: &AgentDistribution) -> &'static [(&'static str, &'static str)] {
    match distribution {
        AgentDistribution::Npx { env, .. }
        | AgentDistribution::Binary { env, .. }
        | AgentDistribution::Uvx { env, .. } => env,
    }
}

/// The Claude config dir an agent's process actually uses, resolved the way
/// the spawn layer resolves it: the per-agent runtime env wins over the
/// distribution's static env, which wins over codeg's own process env, which
/// falls back to `~/.claude`. An explicitly EMPTY runtime value is what the
/// spawn layer `env_remove`s, so it means the default, not "".
pub fn claude_config_dir_for(agent_type: AgentType, runtime_env: &BTreeMap<String, String>) -> PathBuf {
    let meta = registry::get_agent_meta(agent_type);
    claude_config_dir_from(
        runtime_env,
        distribution_env(&meta.distribution),
        std::env::var_os("CLAUDE_CONFIG_DIR"),
        dirs::home_dir(),
    )
}

pub fn claude_config_dir_from(
    runtime_env: &BTreeMap<String, String>,
    distribution_env: &[(&str, &str)],
    process_env: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    if let Some(value) = runtime_env.get("CLAUDE_CONFIG_DIR") {
        if !value.is_empty() {
            return PathBuf::from(value);
        }
        return crate::parsers::claude::resolve_claude_config_dir_from(None, home_dir);
    }
    if let Some((_, value)) = distribution_env
        .iter()
        .find(|(key, value)| *key == "CLAUDE_CONFIG_DIR" && !value.is_empty())
    {
        return PathBuf::from(value);
    }
    crate::parsers::claude::resolve_claude_config_dir_from(process_env, home_dir)
}

#[derive(Debug, thiserror::Error)]
pub enum HandoffError {
    #[error("session id {0:?} is not a valid transcript name")]
    InvalidSessionId(String),
    #[error("no transcript for session {0} under {dir}", dir = .1.display())]
    SessionNotFound(String, PathBuf),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// What [`copy_claude_session`] wrote, so a failed load can take it back.
#[derive(Debug, Clone)]
pub struct CopiedSession {
    pub transcript: PathBuf,
    /// `<project>/<session-id>/` (tool results, sub-agent transcripts) when the
    /// source had one.
    pub sidecar: Option<PathBuf>,
}

/// A session id is embedded in a file name; refuse anything that could leave
/// the project directory (`parsers::claude::find_session_file_in` applies the
/// same rule when looking the file up).
fn safe_session_id(session_id: &str) -> bool {
    !session_id.is_empty() && crate::parsers::is_safe_subagent_id(session_id)
}

/// Copy `<src_home>/projects/<project>/<session_id>.jsonl` (plus its sidecar
/// directory) into the same project directory under `dst_home`.
///
/// The project directory keeps its name: Claude derives it from the working
/// directory, which does not change with the home, so the target finds the
/// file exactly where its own resolver looks. An existing copy is replaced;
/// the source is the authority at handoff time (a round trip A→B→A must bring
/// B's newer turns back over A's stale file). The source is never touched.
///
/// The transcript lands through a temp file + rename so a partial copy can
/// never be mistaken for a whole session.
pub fn copy_claude_session(
    src_home: &Path,
    dst_home: &Path,
    session_id: &str,
) -> Result<CopiedSession, HandoffError> {
    if !safe_session_id(session_id) {
        return Err(HandoffError::InvalidSessionId(session_id.to_string()));
    }
    let projects = src_home.join("projects");
    let src = crate::parsers::claude::find_session_file_in(&projects, session_id)
        .ok_or_else(|| HandoffError::SessionNotFound(session_id.to_string(), projects.clone()))?;
    let project_dir_name = src
        .parent()
        .and_then(Path::file_name)
        .ok_or_else(|| HandoffError::SessionNotFound(session_id.to_string(), projects.clone()))?
        .to_os_string();
    let dst_dir = dst_home.join("projects").join(project_dir_name);
    fs::create_dir_all(&dst_dir)?;

    let dst = dst_dir.join(format!("{session_id}.jsonl"));
    let tmp = dst_dir.join(format!("{session_id}.jsonl.handoff-tmp"));
    fs::copy(&src, &tmp)?;
    if let Err(e) = fs::rename(&tmp, &dst) {
        let _ = fs::remove_file(tmp);
        return Err(e.into());
    }

    let src_sidecar = src.with_file_name(session_id);
    let sidecar = if src_sidecar.is_dir() {
        let dst_sidecar = dst_dir.join(session_id);
        copy_dir_recursive(&src_sidecar, &dst_sidecar)?;
        Some(dst_sidecar)
    } else {
        None
    };
    Ok(CopiedSession {
        transcript: dst,
        sidecar,
    })
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Undo [`copy_claude_session`] after the target failed to load it. Best
/// effort: the copy is inert either way, so a leftover costs nothing but disk.
pub fn remove_copied_session(copied: &CopiedSession) {
    let _ = fs::remove_file(&copied.transcript);
    if let Some(sidecar) = &copied.sidecar {
        let _ = fs::remove_dir_all(sidecar);
    }
}

// ─── Briefing ──────────────────────────────────────────────────────────────

pub struct BriefingInput<'a> {
    pub source_label: &'a str,
    pub target_label: &'a str,
    pub working_dir: Option<&'a str>,
    pub title: Option<&'a str>,
    pub turns: &'a [MessageTurn],
    pub note: Option<&'a str>,
    pub budget: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Briefing {
    pub text: String,
    /// Earlier turns had to be dropped (or the text cut) to fit the budget.
    pub truncated: bool,
    /// Trailing turns carried in full.
    pub verbatim_turns: usize,
    /// Earlier turns carried as a digest.
    pub digest_turns: usize,
    /// Earlier turns that did not fit at all.
    pub omitted_turns: usize,
}

/// True for a prompt this module seeded (the folded-away turn).
pub fn is_briefing_text(text: &str) -> bool {
    text.trim_start().starts_with(BRIEFING_MARKER)
}

fn turn_text(turn: &MessageTurn) -> String {
    let mut out = String::new();
    for block in &turn.blocks {
        match block {
            ContentBlock::Text { text } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
            ContentBlock::Image { .. } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str("[image]");
            }
            _ => {}
        }
    }
    out
}

fn tool_names(turn: &MessageTurn) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for block in &turn.blocks {
        if let ContentBlock::ToolUse { tool_name, .. } = block {
            if !names.contains(&tool_name.as_str()) {
                names.push(tool_name);
            }
        }
    }
    names
}

fn role_label(role: &TurnRole) -> &'static str {
    match role {
        TurnRole::User => "User",
        TurnRole::Assistant => "Assistant",
        TurnRole::System => "System",
    }
}

/// Collapse runs of whitespace and cut at `cap` characters (never inside one).
fn clip(text: &str, cap: usize) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= cap {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(cap).collect();
    cut.push('…');
    cut
}

fn is_edit_tool(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "edit" | "write" | "multiedit" | "notebookedit" | "apply_patch" | "create_file"
    ) || lower.contains("edit_file")
        || lower.contains("write_file")
        || lower.contains("str_replace")
}

/// Paths named by editing tools, in first-seen order. Heuristic on the input
/// preview (a path-shaped token), which is all the parsers keep.
fn touched_files(turns: &[MessageTurn]) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for turn in turns {
        for block in &turn.blocks {
            let ContentBlock::ToolUse {
                tool_name,
                input_preview: Some(preview),
                ..
            } = block
            else {
                continue;
            };
            if !is_edit_tool(tool_name) {
                continue;
            }
            for token in preview.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '{' | '}')) {
                let token = token.trim_matches(|c: char| matches!(c, ':' | '[' | ']' | '(' | ')'));
                if token.len() < 3 || token.len() > 260 {
                    continue;
                }
                if !(token.contains('/') || token.contains('\\')) || token.starts_with("http") {
                    continue;
                }
                if !files.iter().any(|f| f == token) {
                    files.push(token.to_string());
                }
                break;
            }
        }
    }
    files
}

fn render_verbatim(turn: &MessageTurn, index: usize) -> String {
    let mut out = format!("### {} (turn {})\n", role_label(&turn.role), index + 1);
    let mut wrote = false;
    for block in &turn.blocks {
        match block {
            ContentBlock::Text { text } => {
                out.push_str(text.trim_end());
                out.push('\n');
                wrote = true;
            }
            ContentBlock::Image { .. } => {
                out.push_str("[image]\n");
                wrote = true;
            }
            ContentBlock::ToolUse {
                tool_name,
                input_preview,
                ..
            } => {
                out.push_str(&format!("[tool: {tool_name}]"));
                if let Some(preview) = input_preview {
                    out.push(' ');
                    out.push_str(&clip(preview, TOOL_PREVIEW_CAP));
                }
                out.push('\n');
                wrote = true;
            }
            ContentBlock::ToolResult {
                output_preview: Some(preview),
                is_error,
                ..
            } => {
                out.push_str(if *is_error { "[error] " } else { "[result] " });
                out.push_str(&clip(preview, TOOL_PREVIEW_CAP));
                out.push('\n');
                wrote = true;
            }
            _ => {}
        }
    }
    if !wrote {
        out.push_str("(no text)\n");
    }
    out
}

fn render_digest(turn: &MessageTurn, index: usize, cap: usize) -> String {
    let text = turn_text(turn);
    let mut line = format!("- {} (turn {}): ", role_label(&turn.role), index + 1);
    let clipped = clip(&text, cap);
    if clipped.is_empty() {
        line.push_str("(no text)");
    } else {
        line.push_str(&clipped);
    }
    let tools = tool_names(turn);
    if !tools.is_empty() {
        line.push_str(&format!(" [tools: {}]", tools.iter().take(8).cloned().collect::<Vec<_>>().join(", ")));
    }
    line.push('\n');
    line
}

struct Shape {
    verbatim: usize,
    cap: usize,
    omitted: usize,
}

fn render(input: &BriefingInput<'_>, shape: &Shape) -> String {
    let turns = input.turns;
    let n = turns.len();
    let verbatim_from = n.saturating_sub(shape.verbatim);
    let digest_from = shape.omitted.min(verbatim_from);

    let mut out = String::new();
    out.push_str(BRIEFING_MARKER);
    out.push('\n');
    out.push_str(&format!(
        "This conversation was handed off to you ({}) from {} inside Codeg. You are continuing it in place: \
         the user sees the earlier turns above this message, so pick up the work without re-introducing yourself.\n",
        input.target_label, input.source_label
    ));
    if let Some(dir) = input.working_dir.filter(|d| !d.trim().is_empty()) {
        out.push_str(&format!("Working directory: {dir}\n"));
    }
    if let Some(title) = input.title.filter(|t| !t.trim().is_empty()) {
        out.push_str(&format!("Conversation title: {title}\n"));
    }
    if let Some(note) = input.note.map(str::trim).filter(|n| !n.is_empty()) {
        out.push_str("\n## What the user wants you to focus on\n");
        out.push_str(note);
        out.push('\n');
    }

    out.push_str(&format!("\n## Conversation so far ({n} turns)\n"));
    if digest_from > 0 {
        out.push_str(&format!(
            "({digest_from} earlier turn{} omitted to fit this briefing)\n",
            if digest_from == 1 { "" } else { "s" }
        ));
    }
    if digest_from < verbatim_from {
        out.push_str("Digest of earlier turns:\n");
        for (i, turn) in turns.iter().enumerate().take(verbatim_from).skip(digest_from) {
            out.push_str(&render_digest(turn, i, shape.cap));
        }
    }
    let files = touched_files(&turns[..verbatim_from]);
    if !files.is_empty() {
        out.push_str("\nFiles edited earlier:\n");
        for file in files.iter().take(40) {
            out.push_str(&format!("- {file}\n"));
        }
    }
    if verbatim_from < n {
        out.push_str(&format!(
            "\n## Last {} turn{} in full (tool previews shortened)\n",
            n - verbatim_from,
            if n - verbatim_from == 1 { "" } else { "s" }
        ));
        for (i, turn) in turns.iter().enumerate().skip(verbatim_from) {
            out.push_str(&render_verbatim(turn, i));
        }
    }
    out.push_str("\nContinue from here.\n");
    out
}

/// Build the prompt that seeds the target's fresh session.
///
/// Shrinks in a fixed order until it fits `budget`: the digest's per-turn cap
/// first, then the number of verbatim trailing turns (never below two), then
/// the oldest digest turns are dropped, and as a last resort the text is cut.
/// Anything past the first step is reported as `truncated` so the UI can say
/// the briefing is not the whole story.
pub fn build_briefing(input: &BriefingInput<'_>) -> Briefing {
    let n = input.turns.len();
    let budget = input.budget.max(BRIEFING_MARKER.len() + 64);
    let mut shape = Shape {
        verbatim: VERBATIM_TURNS.min(n),
        cap: DIGEST_CAPS[0],
        omitted: 0,
    };
    let mut cap_index = 0;
    let mut truncated = false;
    loop {
        let text = render(input, &shape);
        if text.chars().count() <= budget {
            return Briefing {
                text,
                truncated,
                verbatim_turns: shape.verbatim,
                digest_turns: n.saturating_sub(shape.verbatim).saturating_sub(shape.omitted),
                omitted_turns: shape.omitted,
            };
        }
        if cap_index + 1 < DIGEST_CAPS.len() {
            cap_index += 1;
            shape.cap = DIGEST_CAPS[cap_index];
            continue;
        }
        if shape.verbatim > MIN_VERBATIM_TURNS.min(n) {
            shape.verbatim -= 1;
            truncated = true;
            continue;
        }
        let digest_turns = n.saturating_sub(shape.verbatim);
        if shape.omitted < digest_turns {
            // Drop the oldest quarter at a time so a very long conversation
            // converges in a few rounds instead of one per turn.
            let step = ((digest_turns - shape.omitted) / 4).max(1);
            shape.omitted += step;
            truncated = true;
            continue;
        }
        // Nothing left to drop: cut the text itself, keeping the marker line.
        let suffix = "\n[briefing cut to fit the prompt budget]\n";
        let keep = budget.saturating_sub(suffix.chars().count());
        let mut cut: String = text.chars().take(keep).collect();
        cut.push_str(suffix);
        return Briefing {
            text: cut,
            truncated: true,
            verbatim_turns: shape.verbatim,
            digest_turns: 0,
            omitted_turns: shape.omitted,
        };
    }
}

// ─── Divider + splice ───────────────────────────────────────────────────────

/// One handoff in a conversation's chain, as the detail read consumes it.
#[derive(Debug, Clone, PartialEq)]
pub struct HandoffLink {
    pub from_agent_type: AgentType,
    pub from_external_id: Option<String>,
    pub to_agent_type: AgentType,
    pub to_external_id: String,
    pub path: HandoffPath,
    pub carried: bool,
    pub user_turns_before: usize,
    pub note: Option<String>,
    pub briefing: Option<String>,
    pub truncated: bool,
    pub at: DateTime<Utc>,
}

impl HandoffLink {
    /// `None` when either agent's wire name no longer parses (a row written by
    /// a newer codeg); such a link is skipped rather than rendered wrongly.
    pub fn from_row(row: &conversation_handoff::Model) -> Option<Self> {
        Some(Self {
            from_agent_type: AgentType::from_wire(&row.from_agent_type)?,
            from_external_id: row.from_external_id.clone(),
            to_agent_type: AgentType::from_wire(&row.to_agent_type)?,
            to_external_id: row.to_external_id.clone(),
            path: HandoffPath::parse(&row.path),
            carried: row.carried,
            user_turns_before: usize::try_from(row.user_turns_before).unwrap_or(0),
            note: row.note.clone(),
            briefing: row.briefing.clone(),
            truncated: row.truncated,
            at: row.created_at,
        })
    }
}

/// The divider between two segments: a paired `ToolUse`/`ToolResult` carrying
/// `_meta["codeg.handoff"]`, the same shape the context-compaction divider
/// takes so the frontend hoists it to a standalone timeline item.
pub fn divider_turn(link: &HandoffLink, index: usize) -> MessageTurn {
    let tool_use_id = format!("handoff-{index}");
    let mut marker = serde_json::Map::new();
    marker.insert("version".into(), serde_json::json!(1));
    marker.insert("from".into(), serde_json::json!(link.from_agent_type.as_wire()));
    marker.insert("to".into(), serde_json::json!(link.to_agent_type.as_wire()));
    marker.insert("path".into(), serde_json::json!(link.path.as_str()));
    marker.insert("carried".into(), serde_json::json!(link.carried));
    marker.insert("truncated".into(), serde_json::json!(link.truncated));
    marker.insert("at".into(), serde_json::json!(link.at.to_rfc3339()));
    if let Some(note) = link.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        marker.insert("note".into(), serde_json::json!(note));
    }
    if let Some(briefing) = &link.briefing {
        marker.insert("briefing".into(), serde_json::json!(briefing));
    }
    let meta = serde_json::Value::Object(
        [(HANDOFF_META_KEY.to_string(), serde_json::Value::Object(marker))]
            .into_iter()
            .collect(),
    );
    MessageTurn {
        id: tool_use_id.clone(),
        role: TurnRole::Assistant,
        blocks: vec![
            ContentBlock::ToolUse {
                tool_use_id: Some(tool_use_id.clone()),
                tool_name: "agent_handoff".to_string(),
                input_preview: None,
                status: None,
                meta: Some(meta),
            },
            ContentBlock::ToolResult {
                tool_use_id: Some(tool_use_id),
                output_preview: None,
                is_error: false,
                agent_stats: None,
                images: Vec::new(),
            },
        ],
        timestamp: link.at,
        usage: None,
        duration_ms: None,
        model: None,
        completed_at: Some(link.at),
        agent_message_id: None,
    }
}

pub fn count_user_turns(turns: &[MessageTurn]) -> usize {
    turns
        .iter()
        .filter(|t| matches!(t.role, TurnRole::User))
        .count()
}

/// True for a turn [`divider_turn`] produced: bookkeeping, not conversation,
/// so a briefing built from a spliced timeline leaves it out.
pub fn is_divider_turn(turn: &MessageTurn) -> bool {
    turn.blocks.iter().any(|b| {
        matches!(
            b,
            ContentBlock::ToolUse { meta: Some(meta), .. } if meta.get(HANDOFF_META_KEY).is_some()
        )
    })
}

fn is_briefing_turn(turn: &MessageTurn) -> bool {
    matches!(turn.role, TurnRole::User)
        && turn
            .blocks
            .iter()
            .find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .is_some_and(is_briefing_text)
}

/// Assemble one timeline out of a conversation's handoff chain.
///
/// `segments[i]` is the history read from `links[i]`'s source (empty for a
/// carried link, whose target already holds it). Uncarried segments come first,
/// each followed by its divider; then the current session. A carried link's
/// divider is placed by the user-turn count recorded at handoff time, right
/// before the next user prompt, since its store holds both halves and nothing
/// else marks the seam. The prompt that seeded a summary handoff is folded
/// away: its text already rides on the divider, and a screen-long briefing is
/// not something the user wrote.
pub fn splice_handoffs(
    links: &[HandoffLink],
    segments: Vec<Vec<MessageTurn>>,
    current: Vec<MessageTurn>,
) -> Vec<MessageTurn> {
    if links.is_empty() {
        return current;
    }
    let mut current = current;
    if let Some(pos) = current.iter().position(is_briefing_turn) {
        current.remove(pos);
    }

    let mut out: Vec<MessageTurn> = Vec::new();
    let mut segments = segments.into_iter();
    let mut carried: Vec<usize> = Vec::new();
    for (i, link) in links.iter().enumerate() {
        let segment = segments.next().unwrap_or_default();
        if link.carried {
            carried.push(i);
            continue;
        }
        out.extend(segment);
        out.push(divider_turn(link, i));
    }
    out.extend(current);

    for i in carried {
        let link = &links[i];
        let mut seen = 0usize;
        let mut pos = out.len();
        for (idx, turn) in out.iter().enumerate() {
            if matches!(turn.role, TurnRole::User) {
                if seen == link.user_turns_before {
                    pos = idx;
                    break;
                }
                seen += 1;
            }
        }
        out.insert(pos, divider_turn(link, i));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_turn(id: &str, role: TurnRole, text: &str) -> MessageTurn {
        MessageTurn {
            id: id.into(),
            role,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            timestamp: Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        }
    }

    fn npx(package: &'static str) -> AgentDistribution {
        AgentDistribution::Npx {
            version: "1",
            package,
            cmd: "x",
            args: &[],
            env: &[],
            node_required: None,
        }
    }

    fn link(carried: bool, user_turns_before: usize) -> HandoffLink {
        HandoffLink {
            from_agent_type: AgentType::ClaudeCode,
            from_external_id: Some("S1".into()),
            to_agent_type: AgentType::Codex,
            to_external_id: "S2".into(),
            path: if carried {
                HandoffPath::Native
            } else {
                HandoffPath::Summary
            },
            carried,
            user_turns_before,
            note: Some("finish it".into()),
            briefing: Some(format!("{BRIEFING_MARKER}\nbrief")),
            truncated: false,
            at: Utc::now(),
        }
    }

    #[test]
    fn claude_adapter_packages_are_recognized_with_or_without_a_version() {
        assert!(is_claude_adapter_package("@agentclientprotocol/claude-agent-acp@0.65.0"));
        assert!(is_claude_adapter_package("@agentclientprotocol/claude-agent-acp"));
        assert!(is_claude_adapter_package("@zed-industries/claude-code-acp@0.10.0"));
        assert!(!is_claude_adapter_package("@agentclientprotocol/codex-acp@1.1.9"));
        assert!(!is_claude_adapter_package("@vibe-kit/grok-cli@1.0.5"));
        assert!(!is_claude_adapter_package(""));
    }

    #[test]
    fn same_family_is_native_and_everything_else_is_summary() {
        let claude_slot = AgentType::custom("claude-code-2").unwrap();
        let codex_slot = AgentType::custom("codex-2").unwrap();
        let claude_dist = npx("@agentclientprotocol/claude-agent-acp@0.65.0");
        let codex_dist = npx("@agentclientprotocol/codex-acp@1.1.9");

        let builtin = native_family_of(AgentType::ClaudeCode, &npx("unused"));
        let slot = native_family_of(claude_slot, &claude_dist);
        assert_eq!(builtin, Some(NativeFamily::Claude));
        assert_eq!(slot, Some(NativeFamily::Claude));
        assert_eq!(plan_path_of(builtin, slot), HandoffPath::Native);
        assert_eq!(plan_path_of(slot, builtin), HandoffPath::Native);

        // Codex and Grok have stores too, but no verified load-from-copy path:
        // they stay on the summary side until that is proven.
        assert_eq!(native_family_of(AgentType::Codex, &codex_dist), None);
        assert_eq!(native_family_of(codex_slot, &codex_dist), None);
        assert_eq!(native_family_of(AgentType::Grok, &npx("@vibe-kit/grok-cli@1")), None);
        assert_eq!(plan_path_of(builtin, None), HandoffPath::Summary);
        assert_eq!(plan_path_of(None, builtin), HandoffPath::Summary);
        assert_eq!(plan_path_of(None, None), HandoffPath::Summary);
        // A custom agent that is NOT a Claude wrapper never counts as Claude,
        // whatever its id says.
        assert_eq!(
            native_family_of(AgentType::custom("claude-ish").unwrap(), &codex_dist),
            None
        );
    }

    #[test]
    fn config_dir_precedence_matches_the_spawn_layer() {
        let home = Some(PathBuf::from("/home/u"));
        let dist: &[(&str, &str)] = &[("CLAUDE_CONFIG_DIR", "/dist/home")];
        let mut runtime = BTreeMap::new();

        // Nothing set anywhere: the agent's own default.
        assert_eq!(
            claude_config_dir_from(&runtime, &[], None, home.clone()),
            PathBuf::from("/home/u/.claude")
        );
        // codeg's process env reaches the child when nothing overrides it.
        assert_eq!(
            claude_config_dir_from(&runtime, &[], Some("/proc/home".into()), home.clone()),
            PathBuf::from("/proc/home")
        );
        // The distribution's static env (an isolated custom slot) beats the
        // process env.
        assert_eq!(
            claude_config_dir_from(&runtime, dist, Some("/proc/home".into()), home.clone()),
            PathBuf::from("/dist/home")
        );
        // The per-agent runtime env beats both.
        runtime.insert("CLAUDE_CONFIG_DIR".into(), "/runtime/home".into());
        assert_eq!(
            claude_config_dir_from(&runtime, dist, Some("/proc/home".into()), home.clone()),
            PathBuf::from("/runtime/home")
        );
        // An explicitly empty runtime value is what the spawn layer removes, so
        // the child falls back to its default, not to the distribution value.
        runtime.insert("CLAUDE_CONFIG_DIR".into(), String::new());
        assert_eq!(
            claude_config_dir_from(&runtime, dist, Some("/proc/home".into()), home),
            PathBuf::from("/home/u/.claude")
        );
    }

    #[test]
    fn copy_moves_the_transcript_and_sidecar_into_the_same_project_dir() {
        let root = tempfile::tempdir().unwrap();
        let src_home = root.path().join("src");
        let dst_home = root.path().join("dst");
        let sid = "49e95410-6304-4967-980b-c94986d39913";
        let project = src_home.join("projects").join("C--work-repo");
        fs::create_dir_all(project.join(sid).join("tool-results")).unwrap();
        fs::write(project.join(format!("{sid}.jsonl")), "{\"type\":\"user\"}\n").unwrap();
        fs::write(project.join(sid).join("tool-results").join("a.txt"), "out").unwrap();

        let copied = copy_claude_session(&src_home, &dst_home, sid).unwrap();
        let dst_project = dst_home.join("projects").join("C--work-repo");
        assert_eq!(copied.transcript, dst_project.join(format!("{sid}.jsonl")));
        assert_eq!(
            fs::read_to_string(&copied.transcript).unwrap(),
            "{\"type\":\"user\"}\n"
        );
        assert_eq!(
            fs::read_to_string(dst_project.join(sid).join("tool-results").join("a.txt")).unwrap(),
            "out"
        );
        assert!(!dst_project.join(format!("{sid}.jsonl.handoff-tmp")).exists());
        // The source is untouched.
        assert!(project.join(format!("{sid}.jsonl")).exists());

        // A newer source replaces a stale copy (the A→B→A round trip).
        fs::write(project.join(format!("{sid}.jsonl")), "{\"type\":\"user\"}\n{\"type\":\"assistant\"}\n").unwrap();
        let copied = copy_claude_session(&src_home, &dst_home, sid).unwrap();
        assert_eq!(
            fs::read_to_string(&copied.transcript).unwrap().lines().count(),
            2
        );

        remove_copied_session(&copied);
        assert!(!copied.transcript.exists());
        assert!(!dst_project.join(sid).exists());
    }

    #[test]
    fn copy_refuses_missing_and_unsafe_sessions() {
        let root = tempfile::tempdir().unwrap();
        let src_home = root.path().join("src");
        let dst_home = root.path().join("dst");
        fs::create_dir_all(src_home.join("projects").join("p")).unwrap();
        assert!(matches!(
            copy_claude_session(&src_home, &dst_home, "missing-id"),
            Err(HandoffError::SessionNotFound(..))
        ));
        assert!(matches!(
            copy_claude_session(&src_home, &dst_home, "../escape"),
            Err(HandoffError::InvalidSessionId(..))
        ));
        assert!(matches!(
            copy_claude_session(&src_home, &dst_home, ""),
            Err(HandoffError::InvalidSessionId(..))
        ));
        assert!(!dst_home.exists(), "a refused copy writes nothing");
    }

    #[test]
    fn briefing_carries_the_marker_note_digest_and_verbatim_tail() {
        let turns: Vec<MessageTurn> = (0..10)
            .map(|i| {
                if i % 2 == 0 {
                    text_turn(&format!("t{i}"), TurnRole::User, &format!("user prompt {i}"))
                } else {
                    text_turn(&format!("t{i}"), TurnRole::Assistant, &format!("assistant reply {i}"))
                }
            })
            .collect();
        let briefing = build_briefing(&BriefingInput {
            source_label: "Claude Code",
            target_label: "Codex CLI",
            working_dir: Some("/work/repo"),
            title: Some("Fix the flaky test"),
            turns: &turns,
            note: Some("  focus on the retry loop  "),
            budget: DEFAULT_BRIEFING_BUDGET,
        });
        assert!(briefing.text.starts_with(BRIEFING_MARKER));
        assert!(is_briefing_text(&briefing.text));
        assert!(briefing.text.contains("from Claude Code"));
        assert!(briefing.text.contains("(Codex CLI)"));
        assert!(briefing.text.contains("Working directory: /work/repo"));
        assert!(briefing.text.contains("Conversation title: Fix the flaky test"));
        assert!(briefing.text.contains("focus on the retry loop"));
        assert!(!briefing.truncated);
        assert_eq!(briefing.verbatim_turns, 6);
        assert_eq!(briefing.digest_turns, 4);
        assert_eq!(briefing.omitted_turns, 0);
        // Digest lines for the first four, full sections for the last six.
        assert!(briefing.text.contains("- User (turn 1): user prompt 0"));
        assert!(briefing.text.contains("### User (turn 5)\nuser prompt 4"));
        assert!(briefing.text.contains("### Assistant (turn 10)\nassistant reply 9"));
        assert!(briefing.text.ends_with("Continue from here.\n"));
    }

    #[test]
    fn briefing_shrinks_to_the_budget_and_says_so() {
        let turns: Vec<MessageTurn> = (0..40)
            .map(|i| {
                let role = if i % 2 == 0 {
                    TurnRole::User
                } else {
                    TurnRole::Assistant
                };
                text_turn(&format!("t{i}"), role, &"lorem ipsum ".repeat(200))
            })
            .collect();
        let big = build_briefing(&BriefingInput {
            source_label: "A",
            target_label: "B",
            working_dir: None,
            title: None,
            turns: &turns,
            note: None,
            budget: DEFAULT_BRIEFING_BUDGET,
        });
        assert!(big.text.chars().count() <= DEFAULT_BRIEFING_BUDGET);

        let small = build_briefing(&BriefingInput {
            source_label: "A",
            target_label: "B",
            working_dir: None,
            title: None,
            turns: &turns,
            note: None,
            budget: 3_000,
        });
        assert!(small.text.chars().count() <= 3_000, "{}", small.text.chars().count());
        assert!(small.truncated);
        assert!(small.text.starts_with(BRIEFING_MARKER));
        assert!(small.verbatim_turns >= MIN_VERBATIM_TURNS);

        // Even an absurd budget keeps the marker and reports the cut.
        let tiny = build_briefing(&BriefingInput {
            source_label: "A",
            target_label: "B",
            working_dir: None,
            title: None,
            turns: &turns,
            note: None,
            budget: 10,
        });
        assert!(tiny.truncated);
        assert!(tiny.text.starts_with(BRIEFING_MARKER));
        assert!(tiny.text.contains("[briefing cut to fit the prompt budget]"));
    }

    #[test]
    fn briefing_of_an_empty_conversation_is_still_well_formed() {
        let briefing = build_briefing(&BriefingInput {
            source_label: "A",
            target_label: "B",
            working_dir: None,
            title: None,
            turns: &[],
            note: None,
            budget: DEFAULT_BRIEFING_BUDGET,
        });
        assert!(briefing.text.starts_with(BRIEFING_MARKER));
        assert!(briefing.text.contains("(0 turns)"));
        assert_eq!(briefing.verbatim_turns, 0);
        assert!(!briefing.truncated);
    }

    #[test]
    fn briefing_lists_files_edited_earlier_and_images() {
        let mut edit = text_turn("a", TurnRole::Assistant, "editing");
        edit.blocks.push(ContentBlock::ToolUse {
            tool_use_id: Some("tu1".into()),
            tool_name: "Edit".into(),
            input_preview: Some("file_path: src/lib/retry.ts, old_string: x".into()),
            status: None,
            meta: None,
        });
        let mut shot = text_turn("b", TurnRole::User, "look");
        shot.blocks.push(ContentBlock::Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
            uri: None,
        });
        let turns = vec![
            edit,
            shot,
            text_turn("c", TurnRole::Assistant, "ok"),
            text_turn("d", TurnRole::User, "next"),
            text_turn("e", TurnRole::Assistant, "done"),
            text_turn("f", TurnRole::User, "thanks"),
            text_turn("g", TurnRole::Assistant, "np"),
            text_turn("h", TurnRole::User, "more"),
        ];
        let briefing = build_briefing(&BriefingInput {
            source_label: "A",
            target_label: "B",
            working_dir: None,
            title: None,
            turns: &turns,
            note: None,
            budget: DEFAULT_BRIEFING_BUDGET,
        });
        assert!(briefing.text.contains("Files edited earlier:\n- src/lib/retry.ts"));
        assert!(briefing.text.contains("[tools: Edit]"));
        assert!(briefing.text.contains("[image]"));
    }

    #[test]
    fn divider_carries_the_handoff_meta_the_frontend_hoists() {
        let l = link(false, 3);
        let turn = divider_turn(&l, 2);
        assert_eq!(turn.id, "handoff-2");
        assert!(matches!(turn.role, TurnRole::Assistant));
        let ContentBlock::ToolUse {
            tool_use_id,
            tool_name,
            meta: Some(meta),
            ..
        } = &turn.blocks[0]
        else {
            panic!("expected a ToolUse first");
        };
        assert_eq!(tool_use_id.as_deref(), Some("handoff-2"));
        assert_eq!(tool_name, "agent_handoff");
        let marker = meta.get(HANDOFF_META_KEY).expect("meta key");
        assert_eq!(marker["version"], 1);
        assert_eq!(marker["from"], "claude_code");
        assert_eq!(marker["to"], "codex");
        assert_eq!(marker["path"], "summary");
        assert_eq!(marker["carried"], false);
        assert_eq!(marker["note"], "finish it");
        assert!(marker["briefing"].as_str().unwrap().starts_with(BRIEFING_MARKER));
        assert!(marker["at"].is_string());
        let ContentBlock::ToolResult { tool_use_id, .. } = &turn.blocks[1] else {
            panic!("expected the paired ToolResult");
        };
        assert_eq!(tool_use_id.as_deref(), Some("handoff-2"));
        assert!(is_divider_turn(&turn));
        assert!(!is_divider_turn(&text_turn("t", TurnRole::Assistant, "plain")));
    }

    #[test]
    fn divider_omits_a_blank_note() {
        let mut l = link(false, 0);
        l.note = Some("   ".into());
        l.briefing = None;
        let turn = divider_turn(&l, 0);
        let ContentBlock::ToolUse { meta: Some(meta), .. } = &turn.blocks[0] else {
            panic!("expected a ToolUse");
        };
        let marker = &meta[HANDOFF_META_KEY];
        assert!(marker.get("note").is_none());
        assert!(marker.get("briefing").is_none());
    }

    #[test]
    fn summary_segment_renders_before_its_divider_and_the_briefing_folds_away() {
        let segment = vec![
            text_turn("s1", TurnRole::User, "old prompt"),
            text_turn("s2", TurnRole::Assistant, "old reply"),
        ];
        let current = vec![
            text_turn("c1", TurnRole::User, &format!("{BRIEFING_MARKER}\nseeded briefing")),
            text_turn("c2", TurnRole::Assistant, "new agent reply"),
            text_turn("c3", TurnRole::User, "follow-up"),
        ];
        let out = splice_handoffs(&[link(false, 1)], vec![segment], current);
        let ids: Vec<&str> = out.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["s1", "s2", "handoff-0", "c2", "c3"]);
        assert_eq!(count_user_turns(&out), 2);
    }

    #[test]
    fn carried_divider_lands_before_the_next_user_prompt() {
        // The native target holds both halves; the divider goes after the
        // reply to the last prompt the source answered (2 user turns before).
        let current = vec![
            text_turn("u1", TurnRole::User, "one"),
            text_turn("a1", TurnRole::Assistant, "reply one"),
            text_turn("u2", TurnRole::User, "two"),
            text_turn("a2", TurnRole::Assistant, "reply two"),
            text_turn("u3", TurnRole::User, "three (asked after the handoff)"),
            text_turn("a3", TurnRole::Assistant, "reply three"),
        ];
        let out = splice_handoffs(&[link(true, 2)], vec![Vec::new()], current.clone());
        let ids: Vec<&str> = out.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["u1", "a1", "u2", "a2", "handoff-0", "u3", "a3"]);

        // Handed off after the last reply: nothing follows, so the divider is
        // the tail. A count past the end lands there too.
        let out = splice_handoffs(&[link(true, 3)], vec![Vec::new()], current.clone());
        assert_eq!(out.last().unwrap().id, "handoff-0");
        let out = splice_handoffs(&[link(true, 99)], vec![Vec::new()], current);
        assert_eq!(out.last().unwrap().id, "handoff-0");
    }

    #[test]
    fn mixed_chain_keeps_every_segment_in_order() {
        // A → B (summary), then B → B2 (native, carried): the A segment first,
        // then B2's own store holding B's turns, with the second divider placed
        // by the user-turn count measured on the combined rendering.
        let a_segment = vec![
            text_turn("a-u1", TurnRole::User, "start"),
            text_turn("a-a1", TurnRole::Assistant, "hi"),
        ];
        let b2_store = vec![
            text_turn("b-u1", TurnRole::User, "under B"),
            text_turn("b-a1", TurnRole::Assistant, "B reply"),
            text_turn("b2-u1", TurnRole::User, "under B2"),
            text_turn("b2-a1", TurnRole::Assistant, "B2 reply"),
        ];
        // At the second handoff the rendering showed a-u1 and b-u1: 2 user turns.
        let links = vec![link(false, 1), link(true, 2)];
        let out = splice_handoffs(&links, vec![a_segment, Vec::new()], b2_store);
        let ids: Vec<&str> = out.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["a-u1", "a-a1", "handoff-0", "b-u1", "b-a1", "handoff-1", "b2-u1", "b2-a1"]
        );
    }

    #[test]
    fn no_links_means_no_change() {
        let current = vec![text_turn("u1", TurnRole::User, "hi")];
        let out = splice_handoffs(&[], Vec::new(), current.clone());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "u1");
    }

    #[test]
    fn links_round_trip_through_rows() {
        let row = conversation_handoff::Model {
            id: 1,
            conversation_id: 7,
            seq: 0,
            from_agent_type: "claude_code".into(),
            from_external_id: Some("S1".into()),
            to_agent_type: "custom:claude-code-2".into(),
            to_external_id: "S1".into(),
            path: "native".into(),
            carried: true,
            user_turns_before: 4,
            note: None,
            briefing: None,
            truncated: false,
            created_at: Utc::now(),
        };
        let link = HandoffLink::from_row(&row).expect("parses");
        assert_eq!(link.from_agent_type, AgentType::ClaudeCode);
        assert_eq!(
            link.to_agent_type,
            AgentType::custom("claude-code-2").unwrap()
        );
        assert_eq!(link.path, HandoffPath::Native);
        assert!(link.carried);
        assert_eq!(link.user_turns_before, 4);

        let mut unknown = row;
        unknown.to_agent_type = "not_an_agent".into();
        assert!(HandoffLink::from_row(&unknown).is_none());
    }
}
