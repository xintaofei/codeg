pub mod claude;
pub mod cline;
pub mod codebuddy;
pub mod codex;
pub mod cursor;
pub mod gemini;
pub mod grok;
pub mod hermes;
pub mod kimi_code;
pub mod openclaw;
pub mod opencode;
pub mod pi;
mod summary_cache;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

/// A root of external agent-CLI transcript data, archived under
/// `external/<agent>/` by the optional "include conversation content" toggle.
/// These paths are owned by the respective CLIs — codeg only reads them.
#[derive(Clone)]
pub struct ExternalSource {
    /// Stable directory name inside the archive (`external/<agent>/`).
    pub agent: &'static str,
    /// Live source path (a directory, or a single file when `is_file`).
    pub root: PathBuf,
    pub is_file: bool,
    /// When `Some`, only entries whose first path component (relative to
    /// `root`) is in this allowlist are archived. Used to keep the backup to
    /// transcript/session data and exclude sibling credential/config/cache
    /// files in shared base dirs (e.g. `~/.gemini/oauth_creds.json`). `None`
    /// means the whole root is already transcript-scoped.
    pub include_top: Option<&'static [&'static str]>,
}

impl ExternalSource {
    /// The base directory a `external/<agent>/<rest>` entry restores under.
    /// For file sources that is the file's parent; for dir sources, the root.
    pub fn restore_base(&self) -> PathBuf {
        if self.is_file {
            self.root
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.root.clone())
        } else {
            self.root.clone()
        }
    }
}

/// Enumerate every external transcript source, resolved against the current
/// environment (honoring `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, etc.). Sources
/// whose root does not exist are still listed; callers skip missing roots.
pub fn external_transcript_sources() -> Vec<ExternalSource> {
    let mut sources = vec![
        ExternalSource {
            agent: "claude",
            root: claude::resolve_claude_config_dir().join("projects"),
            is_file: false,
            include_top: None,
        },
        ExternalSource {
            agent: "codex",
            root: codex::resolve_codex_home_dir().join("sessions"),
            is_file: false,
            include_top: None,
        },
        ExternalSource {
            // Gemini's base dir mixes transcripts with credentials/config; only
            // pack the transcript/session subtrees, never `oauth_creds.json` etc.
            agent: "gemini",
            root: gemini::resolve_gemini_base_dir(),
            is_file: false,
            include_top: Some(&["tmp", "history", "projects.json"]),
        },
        ExternalSource {
            agent: "cline",
            root: cline::cline_data_dir(),
            is_file: false,
            include_top: None,
        },
        ExternalSource {
            agent: "opencode",
            root: opencode::resolve_opencode_base_dir().join("opencode.db"),
            is_file: true,
            include_top: None,
        },
        ExternalSource {
            // Hermes self-manages its session store at `~/.hermes/state.db`.
            // WAL caveat: `is_file` archives only the main DB file, not the
            // `-wal`/`-shm` sidecars, so a cold backup taken mid-write can miss
            // the newest un-checkpointed frames (same known limitation as
            // OpenCode). This does NOT affect live reads — the parser's `mode=ro`
            // connection sees committed WAL frames.
            agent: "hermes",
            root: hermes::resolve_hermes_home_dir().join("state.db"),
            is_file: true,
            include_top: None,
        },
        ExternalSource {
            // CodeBuddy stores its JSONL transcripts under
            // `~/.codebuddy/projects` — Claude Code's directory layout, but an
            // OpenAI Agents-SDK item record schema (see `parsers::codebuddy`).
            agent: "codebuddy",
            root: codebuddy::resolve_codebuddy_config_dir().join("projects"),
            is_file: false,
            include_top: None,
        },
        ExternalSource {
            // Kimi Code keeps a directory-per-session transcript store under
            // `~/.kimi-code/sessions/` plus a `session_index.jsonl` (the only
            // source of each session's working directory). Archive both, but
            // allowlist them so the sibling `config.toml` / `credentials/` /
            // `oauth/` are excluded (see `parsers::kimi_code`).
            agent: "kimi-code",
            root: kimi_code::resolve_kimi_code_home_dir(),
            is_file: false,
            include_top: Some(&["sessions", "session_index.jsonl"]),
        },
        ExternalSource {
            // Grok keeps a directory-per-session transcript store under
            // `~/.grok/sessions/<encoded-cwd>/<uuid>/` (relocatable via
            // `GROK_HOME`). `resolve_grok_home_dir()` points at `~/.grok`, so
            // scope the archive to the `sessions/` subtree — never the sibling
            // `auth.json` / `config.toml` / `bin/` under the same home.
            agent: "grok",
            root: grok::resolve_grok_home_dir().join("sessions"),
            is_file: false,
            include_top: None,
        },
        ExternalSource {
            // Cursor keeps a SQLite blob store per chat under
            // `~/.cursor/chats/<md5-of-cwd>/<uuid>/store.db`, and one per ACP
            // session under `~/.cursor/acp-sessions/<uuid>/store.db` (both
            // relocatable via `CURSOR_CONFIG_DIR`). Allowlist exactly those
            // two subtrees — never the sibling `cli-config.json` / `mcp.json`
            // / IDE state under the same home.
            agent: "cursor",
            root: cursor::resolve_cursor_config_dir(),
            is_file: false,
            include_top: Some(&["chats", "acp-sessions"]),
        },
        ExternalSource {
            // pi writes one JSONL per session under `~/.pi/agent/sessions/`
            // (relocatable via `PI_CODING_AGENT_SESSION_DIR` /
            // `PI_CODING_AGENT_DIR`). `resolve_pi_sessions_dir()` already points
            // at the `sessions/` subtree, so sibling credentials (`auth.json`,
            // `models.json`) under `~/.pi/agent` are never archived.
            agent: "pi",
            root: pi::resolve_pi_sessions_dir(),
            is_file: false,
            include_top: None,
        },
    ];
    if let Some(home) = dirs::home_dir() {
        sources.push(ExternalSource {
            agent: "openclaw",
            root: home.join(".openclaw").join("agents"),
            is_file: false,
            include_top: None,
        });
    }
    sources
}

use regex::Regex;

use crate::models::{
    ContentBlock, ConversationDetail, ConversationSummary, MessageTurn, SessionStats, TurnUsage,
};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("Conversation not found: {0}")]
    ConversationNotFound(String),
    #[error("Invalid data: {0}")]
    InvalidData(String),
}

pub trait AgentParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError>;
    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError>;
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
pub fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}

/// True when `id` is safe to embed as a single filename component beneath a
/// session's `subagents/` directory (Claude Code's and CodeBuddy's sub-agent
/// transcript layout). The id is read straight from transcript JSON
/// (`agentId` / `subAgent.sessionId`), so a corrupted or hostile transcript
/// could otherwise smuggle a path that escapes the directory once it is joined
/// and a file is opened.
///
/// Rejects: empty, a path separator (`/` or `\`), a parent ref (`..`), a colon
/// (Windows drive prefix `C:` / NTFS alternate-data-stream), or a NUL. The
/// checks are conservative and platform-independent — we reject `:` and `\`
/// even on Unix (where they are legal filename chars) so the same id can never
/// escape if the transcript is later read on Windows, where `Path::join("C:x")`
/// silently replaces the whole base path.
pub fn is_safe_subagent_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains("..")
        && !id.contains(':')
        && !id.contains('\0')
}

/// Punctuation the serializer escapes with a leading backslash inside a
/// reference label (mirrors `escapeMarkdownText` in `src/lib/reference-text.ts`
/// and the class in the frontend `unescapeReferenceLabel`).
fn is_escapable_reference_punct(c: char) -> bool {
    matches!(
        c,
        '\\' | '`' | '*' | '_' | '~' | '[' | ']' | '(' | ')' | '<' | '>'
    )
}

/// Reverse the serializer's label escaping: drop the backslash from each escaped
/// inline-significant punctuation char so the recovered label reads literally.
/// Mirrors the frontend `unescapeReferenceLabel`.
fn unescape_reference_label(label: &[char]) -> String {
    let mut out = String::with_capacity(label.len());
    let mut i = 0;
    while i < label.len() {
        if label[i] == '\\' && i + 1 < label.len() && is_escapable_reference_punct(label[i + 1]) {
            out.push(label[i + 1]);
            i += 2;
        } else {
            out.push(label[i]);
            i += 1;
        }
    }
    out
}

/// Mirror ECMAScript's `/\s/` — the whitespace class the frontend
/// `foldReferenceLinks` (`src/lib/reference-link.ts`) scans destinations with —
/// so this port stays in step with it. It deliberately differs from Rust's
/// `char::is_whitespace()` in exactly two code points: `U+FEFF` (BOM) is
/// whitespace to JS but not to Rust, and `U+0085` (NEL) is whitespace to Rust
/// but not to JS. The set is ECMAScript WhiteSpace + LineTerminator.
fn is_markdown_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'..='\u{000D}'      // tab, LF, VT, FF, CR
            | '\u{0020}'             // space
            | '\u{00A0}'             // no-break space
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'             // line separator
            | '\u{2029}'             // paragraph separator
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'             // zero-width no-break space (BOM)
    )
}

/// Whether the backslash at `k` escapes the next character. CommonMark never
/// lets a backslash escape whitespace, so `\` + whitespace ENDS (not extends) a
/// label/destination scan — only `\` + a non-whitespace char is a real escape.
fn reference_escapes_next(chars: &[char], k: usize) -> bool {
    chars.get(k) == Some(&'\\') && chars.get(k + 1).is_some_and(|c| !is_markdown_whitespace(*c))
}

/// If a well-formed `(destination)` begins at `start`, return the index just
/// past its closing `)`; otherwise `None`. Mirrors the frontend `destinationEnd`
/// and the serializer's two forms: a `<…>`-wrapped destination (interior `\`,
/// `<`, `>` backslash-escaped) or a bare run with no `(`, `)`, whitespace, `<` or
/// `>`.
fn reference_destination_end(chars: &[char], start: usize) -> Option<usize> {
    let n = chars.len();
    if start >= n || chars[start] != '(' {
        return None;
    }
    let mut k = start + 1;
    if chars.get(k) == Some(&'<') {
        k += 1;
        while k < n {
            if reference_escapes_next(chars, k) {
                k += 2;
                continue;
            }
            match chars[k] {
                '>' => {
                    return if chars.get(k + 1) == Some(&')') {
                        Some(k + 2)
                    } else {
                        None
                    };
                }
                // An unescaped `<` or a line break is forbidden inside `<…>`;
                // bailing here also bounds the scan so a missing `>` stops at the
                // next `<` instead of running to EOF (keeps adversarial input
                // linear).
                '<' | '\n' | '\r' => return None,
                _ => k += 1,
            }
        }
        return None;
    }
    while k < n {
        if reference_escapes_next(chars, k) {
            k += 2;
            continue;
        }
        let c = chars[k];
        if c == ')' {
            return Some(k + 1);
        }
        if c == '(' || c == '<' || c == '>' || is_markdown_whitespace(c) {
            return None;
        }
        k += 1;
    }
    None
}

/// Replace every inline `[label](destination)` reference link in `text` with its
/// unescaped `label`, leaving all other prose (including malformed `[…]`/`(…)`
/// fragments and invocation tokens like `@Codex`) untouched.
///
/// This is the Rust counterpart of the frontend canonical fold
/// (`foldReferenceLinks` in `src/lib/reference-link.ts`) and MUST stay in step
/// with it: a single O(n) left-to-right scan over a stack of unmatched `[`
/// positions, matching each `]` against the most recent opener so a balanced
/// nested label closes at the right bracket, requiring a non-empty label and a
/// well-formed `(dest)` for a link, and recovering later links after a
/// stray/unbalanced `[`. Used to derive conversation titles from a user's first
/// message: folding BEFORE truncation means a long `file://` destination can
/// never be sliced mid-link into an unterminable `[label](file://…` fragment.
pub fn fold_reference_links(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    // Start of the pending prose run; flushed before each link and at the end.
    let mut text_start = 0usize;
    // Indices of `[` seen but not yet matched by a `]` (most recent on top).
    let mut openers: Vec<usize> = Vec::new();
    let mut i = 0usize;

    while i < n {
        if reference_escapes_next(&chars, i) {
            // `\[` / `\]` (and any `\x`) is literal — skip both chars.
            i += 2;
            continue;
        }
        match chars[i] {
            '[' => {
                openers.push(i);
                i += 1;
            }
            ']' if !openers.is_empty() => {
                let open = openers.pop().expect("openers is non-empty");
                match reference_destination_end(&chars, i + 1) {
                    // A link needs a well-formed `(dest)` right after `]` and a
                    // non-empty label between the brackets.
                    Some(end) if i > open + 1 => {
                        out.extend(chars[text_start..open].iter());
                        out.push_str(&unescape_reference_label(&chars[open + 1..i]));
                        // Everything up to `open` is committed, so any still-open
                        // outer `[` can no longer span a link.
                        openers.clear();
                        i = end;
                        text_start = end;
                    }
                    // Not a link: keep the brackets in the pending prose run and
                    // keep scanning so a later valid link is still found.
                    _ => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    out.extend(chars[text_start..n].iter());
    out
}

/// Derive a conversation title from a user's first message: fold inline
/// reference links to their labels, then cap the length. Folding first ensures a
/// `[name](file://<long path>)` mention becomes `name` instead of a raw — and,
/// once truncated, unterminable — Markdown link.
pub fn title_from_user_text(text: &str) -> String {
    truncate_str(&fold_reference_links(text), 100)
}

/// Aggregate turn-level usage and duration into a single `SessionStats`.
pub fn compute_session_stats(turns: &[MessageTurn]) -> Option<SessionStats> {
    let mut total_in = 0u64;
    let mut total_out = 0u64;
    let mut total_cache_create = 0u64;
    let mut total_cache_read = 0u64;
    let mut total_duration = 0u64;
    let mut has_data = false;

    for turn in turns {
        if let Some(ref u) = turn.usage {
            total_in += u.input_tokens;
            total_out += u.output_tokens;
            total_cache_create += u.cache_creation_input_tokens;
            total_cache_read += u.cache_read_input_tokens;
            has_data = true;
        }
        if let Some(d) = turn.duration_ms {
            total_duration += d;
        }
    }

    if !has_data {
        return None;
    }

    Some(SessionStats {
        total_usage: Some(TurnUsage {
            input_tokens: total_in,
            output_tokens: total_out,
            cache_creation_input_tokens: total_cache_create,
            cache_read_input_tokens: total_cache_read,
        }),
        total_tokens: Some(total_in + total_out + total_cache_create + total_cache_read),
        total_duration_ms: total_duration,
        context_window_used_tokens: None,
        context_window_max_tokens: None,
        context_window_usage_percent: None,
    })
}

fn model_capacity_suffix_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\[\s*([0-9]+(?:\.[0-9]+)?)\s*([km])\s*\]\s*$")
            .expect("valid model capacity regex")
    })
}

fn parse_model_capacity_suffix(model: &str) -> Option<u64> {
    let captures = model_capacity_suffix_regex().captures(model.trim())?;
    let value = captures.get(1)?.as_str().parse::<f64>().ok()?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }

    let unit = captures
        .get(2)
        .map(|m| m.as_str().to_ascii_lowercase())
        .unwrap_or_default();
    let multiplier = match unit.as_str() {
        "m" => 1_000_000.0,
        "k" => 1_000.0,
        _ => return None,
    };

    Some((value * multiplier) as u64)
}

pub fn infer_context_window_max_tokens(model: Option<&str>) -> Option<u64> {
    let raw = model?.trim();
    if raw.is_empty() {
        return None;
    }

    if let Some(suffixed_limit) = parse_model_capacity_suffix(raw) {
        return Some(suffixed_limit);
    }

    let normalized = raw
        .rsplit('/')
        .next()
        .unwrap_or(raw)
        .split(':')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();

    if normalized.starts_with("claude") {
        return Some(200_000);
    }
    if normalized.starts_with("gemini") {
        return Some(1_000_000);
    }
    if normalized.starts_with("kimi") {
        return Some(262_144);
    }
    if normalized.starts_with("grok") {
        // Context windows per x.ai docs (docs.x.ai/developers/models, 2026-07).
        // Grok's model names churn, so match the known families before the
        // generic fallback, most-specific first:
        //   grok-4.5              → 500K
        //   grok-4.3 / grok-4.20  → 1M
        //   grok-build-* / grok-code-fast-1 → 256K (coding models; the latter
        //                           is 256K despite the "fast" in its name)
        //   general -fast (grok-4-fast) → 2M
        // Default any unknown grok model to the conservative 256K rather than
        // guessing high.
        if normalized.contains("4.5") {
            return Some(500_000);
        }
        if normalized.contains("4.3") || normalized.contains("4.20") {
            return Some(1_000_000);
        }
        if normalized.contains("code") || normalized.contains("build") {
            return Some(256_000);
        }
        if normalized.contains("fast") {
            return Some(2_000_000);
        }
        return Some(256_000);
    }

    match normalized.as_str() {
        "gpt-5.2-codex" | "gpt-5.1-codex-max" | "gpt-5.1-codex-mini" | "gpt-5.2" => Some(258_000),
        "gpt-5.1" | "gpt-5.1-codex" | "gpt-4o" | "gpt-4o-mini" | "gpt-4-turbo" | "o1-mini"
        | "o1-preview" => Some(128_000),
        "gpt-4" => Some(8_192),
        "o3" | "o3-mini" | "o1" => Some(200_000),
        _ => {
            if normalized.starts_with("gpt-5") {
                Some(258_000)
            } else if normalized.starts_with("gpt-4o")
                || normalized.starts_with("gpt-4.1")
                || normalized.starts_with("gpt-4-turbo")
            {
                Some(128_000)
            } else if normalized.starts_with("o3") || normalized == "o1" {
                Some(200_000)
            } else if normalized.starts_with("o1-mini") || normalized.starts_with("o1-preview") {
                Some(128_000)
            } else {
                None
            }
        }
    }
}

pub fn latest_turn_total_usage_tokens(turns: &[MessageTurn]) -> Option<u64> {
    turns.iter().rev().find_map(|turn| {
        turn.usage.as_ref().map(|usage| {
            usage
                .input_tokens
                .saturating_add(usage.output_tokens)
                .saturating_add(usage.cache_creation_input_tokens)
                .saturating_add(usage.cache_read_input_tokens)
        })
    })
}

pub fn merge_context_window_stats(
    stats: Option<SessionStats>,
    used_tokens: Option<u64>,
    max_tokens: Option<u64>,
) -> Option<SessionStats> {
    if used_tokens.is_none() && max_tokens.is_none() {
        return stats;
    }

    let usage_percent = match (used_tokens, max_tokens) {
        (Some(used), Some(max)) if max > 0 => Some((used as f64 / max as f64) * 100.0),
        _ => None,
    };

    match stats {
        Some(mut s) => {
            s.context_window_used_tokens = used_tokens;
            s.context_window_max_tokens = max_tokens;
            s.context_window_usage_percent = usage_percent;
            Some(s)
        }
        None => Some(SessionStats {
            total_usage: None,
            total_tokens: None,
            total_duration_ms: 0,
            context_window_used_tokens: used_tokens,
            context_window_max_tokens: max_tokens,
            context_window_usage_percent: usage_percent,
        }),
    }
}

/// Relocate orphaned tool_result blocks to the turn that contains their matching tool_use.
///
/// After `group_into_turns` splits assistant rounds, async tool execution can cause
/// a tool_result to land in a later turn than its corresponding tool_use.
/// This post-processing step moves such orphaned results back.
pub fn relocate_orphaned_tool_results(turns: &mut Vec<MessageTurn>) {
    // Build map: tool_use_id → turn index
    let mut tool_use_turn: HashMap<String, usize> = HashMap::new();
    for (idx, turn) in turns.iter().enumerate() {
        for block in &turn.blocks {
            if let ContentBlock::ToolUse {
                tool_use_id: Some(ref id),
                ..
            } = block
            {
                tool_use_turn.insert(id.clone(), idx);
            }
        }
    }

    if tool_use_turn.is_empty() {
        return;
    }

    // Collect (source_turn, target_turn, block) for orphaned results
    let mut relocations: Vec<(usize, usize, ContentBlock)> = Vec::new();
    for (turn_idx, turn) in turns.iter().enumerate() {
        for block in &turn.blocks {
            if let ContentBlock::ToolResult {
                tool_use_id: Some(ref id),
                ..
            } = block
            {
                if let Some(&target) = tool_use_turn.get(id) {
                    if target != turn_idx {
                        relocations.push((turn_idx, target, block.clone()));
                    }
                }
            }
        }
    }

    if relocations.is_empty() {
        return;
    }

    // Build set of (turn_idx, tool_use_id) to remove
    let remove_set: HashMap<usize, Vec<String>> = {
        let mut map: HashMap<usize, Vec<String>> = HashMap::new();
        for (from, _, block) in &relocations {
            if let ContentBlock::ToolResult {
                tool_use_id: Some(ref id),
                ..
            } = block
            {
                map.entry(*from).or_default().push(id.clone());
            }
        }
        map
    };

    // Remove from source turns
    for (&turn_idx, ids) in &remove_set {
        turns[turn_idx].blocks.retain(|block| {
            if let ContentBlock::ToolResult {
                tool_use_id: Some(ref id),
                ..
            } = block
            {
                !ids.contains(id)
            } else {
                true
            }
        });
    }

    // Append to target turns
    for (_, target, block) in relocations {
        turns[target].blocks.push(block);
    }

    // Remove turns that became empty after relocation
    turns.retain(|turn| !turn.blocks.is_empty());
}

/// Convert Read tool output from numbered-line format to `{"start_line":N,"content":"..."}`.
///
/// Claude Code embeds line numbers in Read output like `   115→content`.
/// This splits on the `→` delimiter (or tab for older `cat -n` format),
/// extracts the starting line number, and returns clean content.
pub fn structurize_read_tool_output(turns: &mut [MessageTurn]) {
    let mut read_tool_ids: HashSet<String> = HashSet::new();
    for turn in turns.iter() {
        for block in &turn.blocks {
            if let ContentBlock::ToolUse {
                tool_use_id: Some(ref id),
                ref tool_name,
                ..
            } = block
            {
                let name = tool_name.to_lowercase();
                if matches!(
                    name.as_str(),
                    "read" | "read_file" | "readfile" | "read file" | "cat" | "view"
                ) {
                    read_tool_ids.insert(id.clone());
                }
            }
        }
    }

    for turn in turns.iter_mut() {
        for block in turn.blocks.iter_mut() {
            let is_read_result = matches!(
                block,
                ContentBlock::ToolResult { tool_use_id: Some(ref id), .. }
                if read_tool_ids.contains(id)
            );
            if !is_read_result {
                continue;
            }
            if let ContentBlock::ToolResult {
                ref mut output_preview,
                ..
            } = block
            {
                if let Some(ref text) = output_preview {
                    if let Some(json) = strip_numbered_lines(text) {
                        *output_preview = Some(json);
                    }
                }
            }
        }
    }
}

/// Known delimiters between line number and content.
const LINE_NUM_DELIMITERS: &[&str] = &["→", "\t"];

/// Try to split a line at a known delimiter, returning (line_number, content).
fn split_line_number(line: &str) -> Option<(u64, &str)> {
    for delim in LINE_NUM_DELIMITERS {
        if let Some(pos) = line.find(delim) {
            let prefix = line[..pos].trim();
            if let Ok(num) = prefix.parse::<u64>() {
                let content_start = pos + delim.len();
                return Some((num, &line[content_start..]));
            }
        }
    }
    None
}

/// If most lines have a recognized line-number prefix, strip them all
/// and return `{"start_line":N,"content":"clean text"}`.
pub fn strip_numbered_lines(text: &str) -> Option<String> {
    let raw_lines: Vec<&str> = text.lines().collect();
    if raw_lines.len() < 2 {
        return None;
    }

    let matched = raw_lines
        .iter()
        .filter(|l| l.is_empty() || split_line_number(l).is_some())
        .count();
    if matched < raw_lines.len() * 4 / 5 {
        return None;
    }

    let mut start_line: u64 = 1;
    let mut first = true;
    let stripped: Vec<&str> = raw_lines
        .iter()
        .map(|line| {
            if let Some((num, content)) = split_line_number(line) {
                if first {
                    start_line = num;
                    first = false;
                }
                content
            } else {
                first = false;
                *line
            }
        })
        .collect();

    Some(
        serde_json::json!({
            "start_line": start_line,
            "content": stripped.join("\n")
        })
        .to_string(),
    )
}

/// Resolve line numbers for `*** Update File` / `*** Add File` style patches.
///
/// When a hunk header is just `@@` without `-N,M +N,M`, this reads the actual
/// file from disk and matches the context lines to calculate real line numbers.
/// Falls back gracefully if the file doesn't exist or context doesn't match.
pub fn resolve_patch_line_numbers(turns: &mut [MessageTurn], cwd: Option<&str>) {
    for turn in turns.iter_mut() {
        for block in turn.blocks.iter_mut() {
            if let ContentBlock::ToolUse {
                ref tool_name,
                ref mut input_preview,
                ..
            } = block
            {
                let name = tool_name.to_lowercase();
                if !matches!(
                    name.as_str(),
                    "apply_patch" | "edit" | "patch" | "applypatch"
                ) {
                    continue;
                }
                if let Some(ref text) = input_preview {
                    if text.contains("@@\n") || text.contains("@@\r\n") {
                        if let Some(resolved) = resolve_patch_text(text, cwd) {
                            *input_preview = Some(resolved);
                        }
                    }
                }
            }
        }
    }
}

/// Resolve a single patch text, replacing bare `@@` with `@@ -N,M +N,M @@`.
pub fn resolve_patch_text(patch: &str, cwd: Option<&str>) -> Option<String> {
    let mut output = String::with_capacity(patch.len() + 256);
    let mut current_file_path: Option<String> = None;
    let mut file_lines: Option<Vec<String>> = None;
    let mut any_resolved = false;

    let lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Detect file markers
        if line.starts_with("*** Update File: ") || line.starts_with("*** Add File: ") {
            let marker_end = if line.starts_with("*** Update File: ") {
                17
            } else {
                14
            };
            let path = line[marker_end..].trim();
            current_file_path = Some(path.to_string());
            file_lines = load_file_lines(path, cwd);
            output.push_str(line);
            output.push('\n');
            i += 1;
            continue;
        }

        // Detect bare @@ hunk header (no line numbers)
        if line == "@@" {
            if let (Some(ref fl), true) = (&file_lines, current_file_path.is_some()) {
                // Collect context lines from this hunk to find match position
                let hunk_lines = collect_hunk_lines(&lines, i + 1);
                if let Some((old_start, old_count, new_count)) = find_hunk_position(fl, &hunk_lines)
                {
                    let new_start = old_start; // same start for context-based patches
                    output.push_str(&format!(
                        "@@ -{},{} +{},{} @@\n",
                        old_start, old_count, new_start, new_count
                    ));
                    any_resolved = true;
                    i += 1;
                    continue;
                }
            }
            // Fallback: keep bare @@
            output.push_str(line);
            output.push('\n');
            i += 1;
            continue;
        }

        output.push_str(line);
        output.push('\n');
        i += 1;
    }

    if any_resolved {
        Some(output)
    } else {
        None
    }
}

/// Load file lines from disk, trying both absolute path and cwd-relative.
pub fn load_file_lines(path: &str, cwd: Option<&str>) -> Option<Vec<String>> {
    use std::fs;
    use std::path::Path;

    let p = Path::new(path);
    if p.is_absolute() {
        if let Ok(content) = fs::read_to_string(p) {
            return Some(content.lines().map(|l| l.to_string()).collect());
        }
    }
    if let Some(base) = cwd {
        let full = Path::new(base).join(path);
        if let Ok(content) = fs::read_to_string(&full) {
            return Some(content.lines().map(|l| l.to_string()).collect());
        }
    }
    None
}

/// Collect lines belonging to a hunk (until next `@@` or `*** ` marker or end).
fn collect_hunk_lines<'a>(lines: &'a [&'a str], start: usize) -> Vec<&'a str> {
    let mut result = Vec::new();
    for &line in &lines[start..] {
        if line == "@@" || line.starts_with("*** ") {
            break;
        }
        result.push(line);
    }
    result
}

/// Find where a hunk's context lines match in the file, returning (start_line, old_count, new_count).
/// `start_line` is 1-based.
///
/// The file on disk may be in either pre-patch or post-patch state, and may
/// have been further modified. We try three strategies in order:
/// 1. Contiguous match of context+added lines (post-patch file, no further edits)
/// 2. Contiguous match of context+deleted lines (pre-patch file)
/// 3. Subsequence match of context-only lines (file has been further modified)
fn find_hunk_position(file_lines: &[String], hunk_lines: &[&str]) -> Option<(usize, usize, usize)> {
    let mut old_count = 0usize;
    let mut new_count = 0usize;
    for hl in hunk_lines {
        if hl.starts_with(' ') {
            old_count += 1;
            new_count += 1;
        } else if hl.starts_with('-') {
            old_count += 1;
        } else if hl.starts_with('+') {
            new_count += 1;
        }
    }

    // Strategy 1: contiguous match of context+added (post-patch)
    let new_view: Vec<&str> = hunk_lines
        .iter()
        .filter(|l| l.starts_with(' ') || l.starts_with('+'))
        .map(|l| &l[1..])
        .collect();
    if let Some(pos) = find_contiguous(file_lines, &new_view) {
        return Some((pos + 1, old_count, new_count));
    }

    // Strategy 2: contiguous match of context+deleted (pre-patch)
    let old_view: Vec<&str> = hunk_lines
        .iter()
        .filter(|l| l.starts_with(' ') || l.starts_with('-'))
        .map(|l| &l[1..])
        .collect();
    if let Some(pos) = find_contiguous(file_lines, &old_view) {
        return Some((pos + 1, old_count, new_count));
    }

    // Strategy 3: subsequence match of context-only lines (file further modified)
    let ctx_only: Vec<&str> = hunk_lines
        .iter()
        .filter(|l| l.starts_with(' '))
        .map(|l| &l[1..])
        .collect();
    if let Some(pos) = find_subsequence(file_lines, &ctx_only) {
        return Some((pos + 1, old_count, new_count));
    }

    None
}

/// Find contiguous `view` lines in `file_lines`. Returns 0-based start index.
fn find_contiguous(file_lines: &[String], view: &[&str]) -> Option<usize> {
    if view.is_empty() || view.len() > file_lines.len() {
        return None;
    }
    let first = view[0];
    for i in 0..=(file_lines.len() - view.len()) {
        if file_lines[i].as_str() != first {
            continue;
        }
        if view
            .iter()
            .enumerate()
            .all(|(j, v)| file_lines[i + j].as_str() == *v)
        {
            return Some(i);
        }
    }
    None
}

/// Find `needles` as an ordered subsequence in `file_lines` within a small window.
/// Returns 0-based index of the first needle's position.
fn find_subsequence(file_lines: &[String], needles: &[&str]) -> Option<usize> {
    if needles.is_empty() {
        return None;
    }
    let first = needles[0];
    for start in 0..file_lines.len() {
        if file_lines[start].as_str() != first {
            continue;
        }
        let mut cursor = start + 1;
        let mut all_found = true;
        for &needle in &needles[1..] {
            // Allow up to 10 lines gap between consecutive context lines
            let limit = std::cmp::min(cursor + 10, file_lines.len());
            match file_lines[cursor..limit]
                .iter()
                .position(|fl| fl.as_str() == needle)
            {
                Some(offset) => cursor = cursor + offset + 1,
                None => {
                    all_found = false;
                    break;
                }
            }
        }
        if all_found {
            return Some(start);
        }
    }
    None
}

/// Extract the last path component as the folder name.
pub fn folder_name_from_path(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// Normalize a filesystem path string for tolerant cross-platform comparison.
/// This intentionally does not hit the filesystem (no canonicalize), and only
/// normalizes separators/casing differences that commonly break exact matching.
pub fn normalize_path_for_matching(path: &str) -> String {
    let mut normalized = path.trim().replace('\\', "/");

    #[cfg(target_os = "windows")]
    {
        if let Some(stripped) = normalized.strip_prefix("//?/") {
            normalized = stripped.to_string();
        }
        normalized = normalized.to_ascii_lowercase();
    }

    while normalized.ends_with('/') {
        if normalized == "/" {
            break;
        }
        // Keep Windows drive root such as "c:/" intact.
        if normalized.len() == 3
            && normalized.as_bytes().get(1) == Some(&b':')
            && normalized.as_bytes().get(2) == Some(&b'/')
        {
            break;
        }
        normalized.pop();
    }

    normalized
}

pub fn path_eq_for_matching(left: &str, right: &str) -> bool {
    normalize_path_for_matching(left) == normalize_path_for_matching(right)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{
        fold_reference_links, infer_context_window_max_tokens, is_safe_subagent_id,
        latest_turn_total_usage_tokens, merge_context_window_stats, path_eq_for_matching,
        title_from_user_text,
    };
    use crate::models::{MessageTurn, SessionStats, TurnRole, TurnUsage};

    #[test]
    fn safe_subagent_id_accepts_plain_ids_and_rejects_traversal() {
        // Real CodeBuddy / Claude sub-agent ids are plain tokens.
        assert!(is_safe_subagent_id("agent-cdd7c1ea"));
        assert!(is_safe_subagent_id("agent-test01"));
        // Every escape vector is rejected — including the Windows-only drive
        // colon that the old `/ \\ ..`-only guard let through.
        for hostile in [
            "",
            "..",
            "../../etc/passwd",
            "a/b",
            "a\\b",
            "C:evil",
            "C:\\Windows\\System32",
            "a\0b",
        ] {
            assert!(
                !is_safe_subagent_id(hostile),
                "expected rejection for {hostile:?}"
            );
        }
    }

    #[test]
    fn fold_reference_links_reduces_links_to_labels() {
        // Plain prose is untouched.
        assert_eq!(fold_reference_links("hello world"), "hello world");
        // A file link folds to its label; surrounding text is preserved.
        assert_eq!(
            fold_reference_links("看看 [README.md](file:///Users/x/README.md) 这是什么"),
            "看看 README.md 这是什么"
        );
        // codeg:// links fold too; an agent mention keeps its `@`.
        assert_eq!(
            fold_reference_links("调用 [@Codex CLI](codeg://agent/codex) 执行"),
            "调用 @Codex CLI 执行"
        );
        // Multiple links in one string.
        assert_eq!(
            fold_reference_links("compare [a.ts](file:///a.ts) and [b.ts](file:///b.ts)"),
            "compare a.ts and b.ts"
        );
    }

    #[test]
    fn fold_reference_links_handles_escapes_and_angle_destinations() {
        // A `<…>`-wrapped destination (spaces/parens in the path) still folds.
        assert_eq!(
            fold_reference_links("[report (1).pdf](<file:///tmp/report (1).pdf>)"),
            "report (1).pdf"
        );
        // Escaped punctuation in the label is unescaped.
        assert_eq!(fold_reference_links("[a\\]b\\(c](file:///x)"), "a]b(c");
        // A balanced nested-bracket label closes at the outer `]`.
        assert_eq!(fold_reference_links("[a [b]](https://x)"), "a [b]");
        // A later link is recovered after a stray/unbalanced `[`.
        assert_eq!(fold_reference_links("[a [b](url)"), "[a b");
    }

    #[test]
    fn fold_reference_links_matches_js_whitespace_class() {
        // Parity with the frontend `foldReferenceLinks`, whose destination scan
        // uses ECMAScript `/\s/` rather than Rust's `char::is_whitespace()`. The
        // two classes differ on exactly these code points (verified against the
        // TS module): U+FEFF (BOM) and U+00A0 (NBSP) ARE JS whitespace, so a bare
        // destination containing them is malformed and the text stays raw…
        assert_eq!(
            fold_reference_links("[a](foo\u{FEFF}bar)"),
            "[a](foo\u{FEFF}bar)"
        );
        assert_eq!(
            fold_reference_links("[a](foo\u{00A0}bar)"),
            "[a](foo\u{00A0}bar)"
        );
        // …while U+0085 (NEL) is NOT JS whitespace, so it is an ordinary
        // destination char and the link folds (Rust's is_whitespace would have
        // wrongly rejected it).
        assert_eq!(fold_reference_links("[a](foo\u{0085}bar)"), "a");
    }

    #[test]
    fn fold_reference_links_leaves_malformed_fragments_raw() {
        // An unterminated link (no closing `)`) is left verbatim — exactly the
        // truncated-title shape this fix keeps from ever being stored.
        assert_eq!(
            fold_reference_links("[oops no close](file:///x"),
            "[oops no close](file:///x"
        );
        // An empty-label `[](x)` is not a link.
        assert_eq!(fold_reference_links("[](x)"), "[](x)");
        // A bare destination with an unescaped space is malformed.
        assert_eq!(fold_reference_links("[a](foo bar)"), "[a](foo bar)");
    }

    #[test]
    fn title_from_user_text_folds_before_truncating() {
        // The regression: a long percent-encoded file mention used to be
        // truncated mid-destination into an unterminable `[label](file://…`
        // fragment. Folding first yields the short, clean filename.
        let long_path = "%E5%85%A8".repeat(40); // > 100 chars when raw
        let raw = format!("[全天候运维.xlsx](file:///Users/xggz/Desktop/{long_path}.xlsx)");
        assert!(raw.chars().count() > 100, "fixture must exceed the cap");
        assert_eq!(title_from_user_text(&raw), "全天候运维.xlsx");
    }

    #[test]
    fn title_from_user_text_still_caps_plain_prose() {
        let long = "x".repeat(250);
        let title = title_from_user_text(&long);
        assert_eq!(title.chars().count(), 103); // 100 + "..."
        assert!(title.ends_with("..."));
    }

    #[test]
    fn infers_model_context_limits() {
        assert_eq!(
            infer_context_window_max_tokens(Some("claude-sonnet-4-6")),
            Some(200_000)
        );
        assert_eq!(
            infer_context_window_max_tokens(Some("gemini-2.5-pro")),
            Some(1_000_000)
        );
        assert_eq!(
            infer_context_window_max_tokens(Some("claude-sonnet-4-6 [1.5M]")),
            Some(1_500_000)
        );
        // Grok context windows per x.ai docs: grok-4.5 = 500K, grok-4.3 /
        // grok-4.20 = 1M, the coding/build models = 256K (grok-code-fast-1
        // despite "fast"), the general -fast variants = 2M, and any unknown
        // grok model falls back to the conservative 256K.
        assert_eq!(
            infer_context_window_max_tokens(Some("grok-4.5")),
            Some(500_000)
        );
        assert_eq!(
            infer_context_window_max_tokens(Some("grok-4.3")),
            Some(1_000_000)
        );
        assert_eq!(
            infer_context_window_max_tokens(Some("grok-4.20-0309-reasoning")),
            Some(1_000_000)
        );
        assert_eq!(
            infer_context_window_max_tokens(Some("grok-build-0.1")),
            Some(256_000)
        );
        assert_eq!(
            infer_context_window_max_tokens(Some("grok-code-fast-1")),
            Some(256_000)
        );
        assert_eq!(
            infer_context_window_max_tokens(Some("grok-4-fast")),
            Some(2_000_000)
        );
        assert_eq!(
            infer_context_window_max_tokens(Some("grok-7-experimental")),
            Some(256_000)
        );
        assert_eq!(infer_context_window_max_tokens(Some("unknown-model")), None);
    }

    #[test]
    fn picks_latest_turn_usage_total_tokens() {
        let timestamp = Utc::now();
        let turns = vec![
            MessageTurn {
                id: "turn-0".to_string(),
                role: TurnRole::Assistant,
                blocks: vec![],
                timestamp,
                usage: Some(TurnUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                    cache_creation_input_tokens: 30,
                    cache_read_input_tokens: 40,
                }),
                duration_ms: None,
                model: None,
                completed_at: None,
            },
            MessageTurn {
                id: "turn-1".to_string(),
                role: TurnRole::Assistant,
                blocks: vec![],
                timestamp,
                usage: Some(TurnUsage {
                    input_tokens: 11,
                    output_tokens: 21,
                    cache_creation_input_tokens: 31,
                    cache_read_input_tokens: 41,
                }),
                duration_ms: None,
                model: None,
                completed_at: None,
            },
        ];

        assert_eq!(latest_turn_total_usage_tokens(&turns), Some(104));
    }

    #[test]
    fn merges_context_window_stats() {
        let merged = merge_context_window_stats(None, Some(1500), Some(3000))
            .expect("context stats should exist");
        assert_eq!(merged.context_window_used_tokens, Some(1500));
        assert_eq!(merged.context_window_max_tokens, Some(3000));
        assert!(merged.total_usage.is_none());
        let percent = merged
            .context_window_usage_percent
            .expect("usage percent should exist");
        assert!((percent - 50.0).abs() < f64::EPSILON);

        let existing = Some(SessionStats {
            total_usage: Some(TurnUsage {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_input_tokens: 3,
                cache_read_input_tokens: 4,
            }),
            total_tokens: Some(10),
            total_duration_ms: 100,
            context_window_used_tokens: None,
            context_window_max_tokens: None,
            context_window_usage_percent: None,
        });
        let merged_existing =
            merge_context_window_stats(existing, Some(200), Some(1000)).expect("merged");
        assert_eq!(merged_existing.total_tokens, Some(10));
        assert_eq!(merged_existing.context_window_used_tokens, Some(200));
        assert_eq!(merged_existing.context_window_max_tokens, Some(1000));
    }

    #[test]
    fn path_matching_handles_separator_differences() {
        assert!(path_eq_for_matching(
            "/Users/demo/workspace/codeg",
            "/Users/demo/workspace/codeg/"
        ));
        assert!(path_eq_for_matching(
            "C:\\Users\\demo\\workspace\\codeg",
            "C:/Users/demo/workspace/codeg"
        ));
    }
}
