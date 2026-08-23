use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::{
    AgentType, ContentBlock, ConversationDetail, ConversationSummary, MessageTurn, TurnRole,
    TurnUsage,
};
use crate::parsers::expand_home_prefix;
use crate::parsers::{
    backfill_turn_durations, compute_session_stats, folder_name_from_path,
    infer_context_window_max_tokens, merge_context_window_stats, relocate_orphaned_tool_results,
    resolve_patch_line_numbers, structurize_read_tool_output, title_from_user_text, truncate_str,
    AgentParser, ParseError,
};

/// Resolve the DeepSeek Harness home the way `@deepseek-ai/dsh-home-paths`'
/// `resolveDshHome` does: `DSH_HOME` wins, else `~/.dsh`.
///
/// Two details are upstream's, not conveniences: a WHITESPACE-only `DSH_HOME`
/// counts as unset (`fromEnv.trim().length > 0`), so a blank override never
/// resolves the home to a whitespace path; and a leading `~` is expanded
/// (`expandHomePath`) before use. The value itself is otherwise passed through
/// untrimmed, again like upstream. Upstream additionally absolutizes a
/// relative value against the harness process's cwd — deliberately NOT
/// mirrored, because that cwd is the session workspace, not codeg's, so
/// resolving it here would name a directory the agent never uses.
fn resolve_dsh_home_from(dsh_home_env: Option<OsString>, home_dir: Option<PathBuf>) -> PathBuf {
    dsh_home_env
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .map(|value| expand_home_prefix(&value.to_string_lossy(), home_dir.as_ref()))
        .unwrap_or_else(|| home_dir.unwrap_or_default().join(".dsh"))
}

/// The DeepSeek Harness home (`DSH_HOME`, default `~/.dsh`) — the root of the
/// skills store, the credentials document, and (unless relocated) the session
/// logs.
pub(crate) fn resolve_dsh_home_dir() -> PathBuf {
    resolve_dsh_home_from(std::env::var_os("DSH_HOME"), dirs::home_dir())
}

/// Resolve the shared cross-agent home the way `dsh-skill-filesystem` does:
/// `DSH_AGENTS_HOME` wins, else `~/.agents`. Its `skills/` subdirectory is the
/// same store Codex, pi, Cursor and OpenCode read, so a skill linked there is
/// visible to DeepSeek too.
pub(crate) fn resolve_dsh_agents_home_dir() -> PathBuf {
    resolve_dsh_agents_home_from(std::env::var_os("DSH_AGENTS_HOME"), dirs::home_dir())
}

fn resolve_dsh_agents_home_from(
    agents_home_env: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    agents_home_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.unwrap_or_default().join(".agents"))
}

/// Resolve the session-log root the way deepseek-acp's `sessionsRoot` does:
/// `DEEPSEEK_ACP_SESSIONS_ROOT` wins, else `$DSH_HOME/sessions`, else
/// `~/.dsh/sessions`.
pub(crate) fn resolve_deepseek_sessions_root() -> PathBuf {
    resolve_deepseek_sessions_root_from(
        std::env::var_os("DEEPSEEK_ACP_SESSIONS_ROOT"),
        std::env::var_os("DSH_HOME"),
        dirs::home_dir(),
    )
}

fn resolve_deepseek_sessions_root_from(
    sessions_env: Option<OsString>,
    dsh_home_env: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    sessions_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| resolve_dsh_home_from(dsh_home_env, home_dir).join("sessions"))
}

/// Resolve the image attachment store the way `dsh-attachment-local` does:
/// always `$DSH_HOME/attachments/v1`, with no override of its own.
///
/// Deliberately NOT derived from the sessions root:
/// `DEEPSEEK_ACP_SESSIONS_ROOT` relocates the logs alone, so a deployment that
/// uses it keeps its attachments under `$DSH_HOME` all the same.
pub(crate) fn resolve_deepseek_attachments_root() -> PathBuf {
    resolve_dsh_home_dir().join("attachments").join("v1")
}

/// DeepSeek Harness (driven through the `deepseek-acp` editor bridge) persists
/// each session as an **append-only event log**:
///
/// ```text
/// $DSH_HOME/sessions/               (default ~/.dsh/sessions; whole root
/// └── <munged cwd>/                  relocatable via DEEPSEEK_ACP_SESSIONS_ROOT)
///     └── <session uuid>/
///         └── session.jsonl.zstd    # or session.jsonl when compression=none
/// ```
///
/// The `.zstd` file is a sequence of complete Zstandard frames (one appended
/// per write batch); a standard streaming decode walks them all. Each line is
/// `{type, seq, time, data}` (`time` in epoch ms). The first line is the
/// session header (`type: "session"`, carrying `id` / `createdAt` / `cwd` /
/// `delegationDepth`). Content comes from:
///
/// - `user/message` — `data.source.kind` separates a real human prompt
///   (`"user"`) from plugin-injected runtime context (`"plugin"`, skipped).
///   Since 0.6.0 its `content[]` may hold `image` blocks alongside the text;
///   a compaction's replacement message rides the same `"plugin"` skip
///   (`{kind: "plugin", plugin: "compact"}`), so the summary text never shows
///   up as something the user said.
/// - `assistant/message` — the ASSEMBLED assistant message for one step
///   (`content[]` of `text` / `reasoning` / `tool-call {id, name, arguments}`
///   blocks) plus that step's `usage` (`inputTokens` / `outputTokens` /
///   `cacheReadTokens` / `reasoningTokens`). The raw stream duplicates it as
///   `assistant/chunk` / `*-chunks` rows, which are skipped.
/// - `tool/result` — the paired result (`message.content[0]` is a
///   `tool-result` with `toolCallId` / `content[]` / `isError`).
/// - `turn/start` / `turn/end` — authoritative turn boundaries; `turn/end`
///   carries the end reason (`completed` / `aborted` / `error` / …) and its
///   `time` is the turn's completion clock. A turn missing its `turn/end`
///   (killed mid-flight) keeps honest `None` clocks.
/// - `request/header` — per-request `config.model`; `request/context` — the
///   advertised `contextWindow`; `session/title` — the running title
///   (last one wins).
/// - `compaction/start` → `compaction/summary` → `compaction/end` (0.6.0) —
///   one context compaction, rendered as the shared context-compaction marker.
///
/// Images (0.6.0): an `image` content block carries only an
/// `ImageAttachmentRef`; the bytes live in the content-addressed store at
/// `$DSH_HOME/attachments/v1/objects/<xx>/<sha256>`. The detail path inlines
/// them, the list path does not — see [`attachment_image_block`].
///
/// Usage semantics: per-step records are SUMMED into the turn (mirroring the
/// Kimi parser); the context-window occupancy is the LATEST step's input side
/// (`inputTokens + cacheReadTokens`) — never the per-turn sum, which re-counts
/// the cached prefix once per step.
pub struct DeepSeekParser {
    base_dir: PathBuf,
    /// Root of the content-addressed attachment store (`$DSH_HOME/attachments/
    /// v1`), where the pixels behind an `image` content block live. Resolved
    /// separately from `base_dir`: `DEEPSEEK_ACP_SESSIONS_ROOT` moves the logs
    /// without moving the store.
    attachments_root: PathBuf,
}

impl DeepSeekParser {
    pub fn new() -> Self {
        Self {
            base_dir: resolve_deepseek_sessions_root(),
            attachments_root: resolve_deepseek_attachments_root(),
        }
    }

    /// Construct a parser pointed at an explicit sessions directory (test
    /// fixtures).
    ///
    /// The attachment store is placed at `<base_dir>/attachments/v1` rather
    /// than resolved from the environment: a fixture that stages no image bytes
    /// must not fall through to the developer's real `~/.dsh/attachments`. The
    /// real layout puts the two beside each other under `$DSH_HOME` instead,
    /// which is why production resolves them separately — the sessions root is
    /// independently relocatable via `DEEPSEEK_ACP_SESSIONS_ROOT`.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        let attachments_root = base_dir.join("attachments").join("v1");
        Self {
            base_dir,
            attachments_root,
        }
    }

    fn build_summary(&self, session_dir: &Path, session_id: &str) -> Option<ConversationSummary> {
        // No attachment root: a summary renders no blocks, so inlining image
        // bytes here would read (and base64) every attachment in every listed
        // session for a payload nobody looks at.
        let parsed = parse_session_log(session_dir, None)?;
        // Sub-agent sessions (spawned by an upstream harness delegation) are
        // not editor conversations; only the top-level depth is listed.
        if parsed.delegation_depth > 0 {
            return None;
        }
        // A session that never produced a user/assistant/tool event (only the
        // header + config records) is treated as empty, matching the
        // "metadata-only is not listed" rule of the other parsers.
        if parsed.content_events == 0 {
            return None;
        }
        let started_at = parsed.created_at.or(parsed.first_ts)?;

        Some(ConversationSummary {
            id: session_id.to_string(),
            agent_type: AgentType::DeepSeek,
            folder_name: parsed.cwd.as_deref().map(folder_name_from_path),
            folder_path: parsed.cwd,
            title: parsed.title.or(parsed.first_user_text),
            started_at,
            ended_at: parsed.last_ts,
            message_count: parsed.message_count,
            model: parsed.model,
            git_branch: None,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        })
    }

    fn build_detail(&self, session_dir: &Path, conversation_id: &str) -> ConversationDetail {
        let parsed =
            parse_session_log(session_dir, Some(&self.attachments_root)).unwrap_or_default();

        let mut turns = parsed.turns;
        relocate_orphaned_tool_results(&mut turns);
        structurize_read_tool_output(&mut turns);
        resolve_patch_line_numbers(&mut turns, parsed.cwd.as_deref());
        backfill_turn_durations(&mut turns, &[]);

        // Context-window occupancy is the LATEST step's input side; capacity
        // comes from the log's own `request/context.contextWindow` (falling
        // back to model-name inference for logs that never recorded one).
        let max_tokens = parsed
            .context_window
            .or_else(|| infer_context_window_max_tokens(parsed.model.as_deref()));
        let session_stats = merge_context_window_stats(
            compute_session_stats(&turns),
            parsed.last_step_input_side,
            max_tokens,
        );

        let summary = ConversationSummary {
            id: conversation_id.to_string(),
            agent_type: AgentType::DeepSeek,
            folder_name: parsed.cwd.as_deref().map(folder_name_from_path),
            folder_path: parsed.cwd,
            title: parsed.title.or(parsed.first_user_text),
            started_at: parsed
                .created_at
                .or(parsed.first_ts)
                .unwrap_or_else(Utc::now),
            ended_at: parsed.last_ts,
            message_count: parsed.message_count,
            model: parsed.model,
            git_branch: None,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        };

        ConversationDetail {
            summary,
            turns,
            session_stats,
            transcript_watermark: None,
        }
    }

    /// Locate the `<session uuid>` directory matching `conversation_id` across
    /// the `base_dir/<munged cwd>/` buckets (two shallow levels).
    fn find_session_dir(&self, conversation_id: &str) -> Option<PathBuf> {
        for bucket in read_subdirs(&self.base_dir) {
            let candidate = bucket.join(conversation_id);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        None
    }
}

impl Default for DeepSeekParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentParser for DeepSeekParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let mut conversations = Vec::new();
        if !self.base_dir.is_dir() {
            return Ok(conversations);
        }

        for bucket in read_subdirs(&self.base_dir) {
            for session_dir in read_subdirs(&bucket) {
                let Some(session_id) = session_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                else {
                    continue;
                };
                if let Some(summary) = self.build_summary(&session_dir, &session_id) {
                    conversations.push(summary);
                }
            }
        }

        conversations.sort_by_key(|c| std::cmp::Reverse(c.started_at));
        Ok(conversations)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        let Some(session_dir) = self.find_session_dir(conversation_id) else {
            return Err(ParseError::ConversationNotFound(
                conversation_id.to_string(),
            ));
        };
        Ok(self.build_detail(&session_dir, conversation_id))
    }
}

/// The accumulated result of scanning one session log.
#[derive(Default)]
struct SessionParse {
    turns: Vec<MessageTurn>,
    cwd: Option<String>,
    created_at: Option<DateTime<Utc>>,
    delegation_depth: u64,
    /// Latest `session/title` (the title service re-emits as it improves).
    title: Option<String>,
    /// First user prompt, already truncated for use as a fallback title.
    first_user_text: Option<String>,
    /// Latest `request/header` model (fallback: the assistant message source).
    model: Option<String>,
    /// Latest `request/context.contextWindow`.
    context_window: Option<u64>,
    /// The latest single step's input side (`inputTokens + cacheReadTokens`) —
    /// the context-window occupancy. NOT the per-turn sum (see the type doc).
    last_step_input_side: Option<u64>,
    first_ts: Option<DateTime<Utc>>,
    last_ts: Option<DateTime<Utc>>,
    /// User prompts + assistant text messages, the list view's activity count.
    message_count: u32,
    /// Content-bearing records — whether the session is worth listing at all.
    content_events: u32,
}

/// Read a session's log text: the Zstandard file when present, else the
/// plaintext `session.jsonl` written by a `compression: "none"` deployment.
fn read_session_log_text(session_dir: &Path) -> Option<String> {
    let zstd_path = session_dir.join("session.jsonl.zstd");
    match fs::read(&zstd_path) {
        Ok(bytes) => decode_zstd_frames_prefix(&bytes),
        Err(_) => fs::read_to_string(session_dir.join("session.jsonl")).ok(),
    }
}

/// Decode every complete Zstandard frame, KEEPING the prefix when the stream
/// errors partway. The writer appends one frame per batch, so a concurrent
/// read can catch the final frame half-written — failing the whole decode
/// there would make a LIVE session vanish from the list until the next flush.
/// The JSONL walk upstream skips whatever broken tail line the cut leaves.
fn decode_zstd_frames_prefix(bytes: &[u8]) -> Option<String> {
    use std::io::Read as _;
    let mut decoder = zstd::stream::read::Decoder::with_buffer(bytes).ok()?;
    let mut decoded = Vec::new();
    // On Err, `read_to_end` has already appended every byte that decoded
    // cleanly before the failure — exactly the prefix to keep.
    let _ = decoder.read_to_end(&mut decoded);
    // Lossy: one bad byte must not drop the whole session; the JSON lines
    // that decode cleanly still parse.
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

fn parse_session_log(session_dir: &Path, attachments: Option<&Path>) -> Option<SessionParse> {
    Some(parse_session_events(
        &read_session_log_text(session_dir)?,
        attachments,
    ))
}

/// Byte ceiling for inlining one attachment into a history turn.
///
/// Under upstream's stock policy this never fires: the store holds NORMALIZED
/// bytes, and normalization re-encodes (and downscales, repeatedly) until the
/// object fits `normalizedImageMaxBytes` / `normalizedImageMaxDimension` —
/// 4 MiB and 2048px by default, refusing the write outright if it cannot. The
/// far larger `maxImageBytes` (20 MB) and `maxImageDimension` (8192) are
/// INPUT-admission limits, i.e. what a prompt may hand in, not what lands on
/// disk.
///
/// It exists because both of those are deployment-configurable and because
/// base64 inflates whatever it does find by ~4/3 into a single conversation
/// payload. Over the ceiling the block degrades to a text note rather than
/// disappearing — the same trade `attachment_placeholder` makes for a missing
/// object.
const DEEPSEEK_MAX_INLINE_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

/// One `image` content block → the block history should show.
///
/// The log stores only an `ImageAttachmentRef` (`{attachmentId, mediaType,
/// bytes, width, height, name?}`); the pixels live in the content-addressed
/// store at `$DSH_HOME/attachments/v1/objects/<first two hex>/<sha256 hex>`,
/// where `attachmentId` is the string `sha256:<hex>`.
///
/// `attachments` is `None` on the LIST path: building a summary never renders
/// blocks, so reading (and base64-ing) every image in every session just to
/// count turns would be pure cost. Both paths still produce a block, because
/// the alternative is worse than an unrendered image — an image-only prompt
/// ("what is wrong with this screenshot?") carries no text at all, and dropping
/// its block would drop the whole user turn, leaving an answer to a question
/// that is not there.
fn attachment_image_block(attachment: Option<&Value>, attachments: Option<&Path>) -> ContentBlock {
    let Some(attachment) = attachment else {
        return attachment_placeholder(None, None);
    };
    let media_type = attachment
        .get("mediaType")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty());
    let name = attachment
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty());
    let placeholder = || attachment_placeholder(name, media_type);

    let (Some(mime), Some(root)) = (media_type, attachments) else {
        return placeholder();
    };
    let Some(hex) = attachment
        .get("attachmentId")
        .and_then(Value::as_str)
        .and_then(attachment_object_hex)
    else {
        return placeholder();
    };
    if attachment
        .get("bytes")
        .and_then(Value::as_u64)
        .is_some_and(|bytes| bytes > DEEPSEEK_MAX_INLINE_IMAGE_BYTES)
    {
        return placeholder();
    }
    let object = root.join("objects").join(&hex[..2]).join(&hex);
    // Size the file on disk BEFORE reading it. The log's own `bytes` field is
    // checked above, but it is optional and it is the log's claim — a truncated
    // or corrupt object would otherwise be read into memory in full and only
    // then rejected, which is the one allocation the ceiling is meant to stop.
    let Ok(meta) = fs::metadata(&object) else {
        return placeholder();
    };
    if meta.len() == 0 || meta.len() > DEEPSEEK_MAX_INLINE_IMAGE_BYTES {
        return placeholder();
    }
    let Ok(bytes) = fs::read(&object) else {
        return placeholder();
    };
    // The file can grow between the stat and the read; the store is
    // content-addressed and its writes are atomic renames, so this is
    // belt-and-braces rather than an expected path.
    if bytes.is_empty() || bytes.len() as u64 > DEEPSEEK_MAX_INLINE_IMAGE_BYTES {
        return placeholder();
    }
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    ContentBlock::Image {
        data: STANDARD.encode(&bytes),
        mime_type: mime.to_string(),
        // The store is content-addressed, so the id is a digest, not a name.
        // Handing it over as the `uri` would label the thumbnail with 64 hex
        // characters; `None` lets the frontend derive `image.<ext>` from the
        // mime instead. A ref that kept the user's own file name uses that.
        uri: name.map(String::from),
    }
}

/// The stand-in for an image whose pixels are not being inlined — the list
/// path, an object that is gone (the store is prunable independently of the
/// logs), or one too large to inline. Names whatever the ref knew so the turn
/// still says what was attached.
fn attachment_placeholder(name: Option<&str>, media_type: Option<&str>) -> ContentBlock {
    let detail = match (name, media_type) {
        (Some(name), _) => format!(" {name}"),
        (None, Some(media_type)) => format!(" {media_type}"),
        (None, None) => String::new(),
    };
    ContentBlock::Text {
        text: format!("[image{detail}]"),
    }
}

/// The object's sha256 hex, or `None` when the id is not the `sha256:<64 hex>`
/// form the local store mints. Validated rather than split because the hex goes
/// straight into a path: a `../…` id must not become a directory traversal.
fn attachment_object_hex(attachment_id: &str) -> Option<String> {
    let hex = attachment_id.strip_prefix("sha256:")?;
    // Lowercase only: upstream formats the digest with `toString('hex')`, and
    // accepting other spellings would invent ids the store never wrote.
    (hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| hex.to_string())
}

/// Tool-call arguments arrive as the model's raw JSON string. Store it
/// verbatim when small; over the cap, re-serialize with string VALUES capped so
/// the stored preview stays VALID JSON (the delegation card `JSON.parse`s it to
/// recover `task` / `agent_type` — an opaque byte truncation would corrupt it).
/// Unparseable arguments (a model that emitted broken JSON) fall back to the
/// plain truncation, which cannot make them any less parseable.
const DEEPSEEK_TOOL_INPUT_CAP: usize = 8_000;

fn deepseek_tool_input_preview(arguments: &str) -> Option<String> {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() <= DEEPSEEK_TOOL_INPUT_CAP {
        return Some(trimmed.to_string());
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => crate::parsers::grok::cap_json_to_budget(&value, DEEPSEEK_TOOL_INPUT_CAP),
        Err(_) => Some(truncate_str(trimmed, DEEPSEEK_TOOL_INPUT_CAP)),
    }
}

fn event_millis(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(value.get("time")?.as_i64()?)
}

/// Render every `{type:"image"}` part of a message `content[]` array, in wire
/// order. Empty when the message carries none, which is every message written
/// before 0.6.0 and most written after it.
fn user_image_blocks(content: Option<&Value>, attachments: Option<&Path>) -> Vec<ContentBlock> {
    let Some(items) = content.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("image"))
        .map(|item| attachment_image_block(item.get("attachment"), attachments))
        .collect()
}

/// Concatenate the `text` of every `{type:"text"}` part in a message
/// `content[]` array.
fn collect_text_parts(content: Option<&Value>) -> String {
    let mut out = String::new();
    if let Some(items) = content.and_then(Value::as_array) {
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(text);
                }
            }
        }
    }
    out
}

fn add_usage(a: TurnUsage, b: TurnUsage) -> TurnUsage {
    TurnUsage {
        input_tokens: a.input_tokens.saturating_add(b.input_tokens),
        output_tokens: a.output_tokens.saturating_add(b.output_tokens),
        cache_creation_input_tokens: a
            .cache_creation_input_tokens
            .saturating_add(b.cache_creation_input_tokens),
        cache_read_input_tokens: a
            .cache_read_input_tokens
            .saturating_add(b.cache_read_input_tokens),
    }
}

/// Map one step's `usage` object onto `TurnUsage`; `None` when all counters
/// are absent or zero. `reasoningTokens` is a subset of `outputTokens` (the
/// adapter reports both), so it is deliberately not added anywhere.
fn usage_from_step(usage: Option<&Value>) -> Option<TurnUsage> {
    let usage = usage?;
    let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let input = field("inputTokens");
    let output = field("outputTokens");
    let cache_read = field("cacheReadTokens");
    if input == 0 && output == 0 && cache_read == 0 {
        return None;
    }
    Some(TurnUsage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read,
    })
}

/// What a compaction's opening and summary events told us, held until its
/// `compaction/end` closes the transaction and the marker can be emitted.
#[derive(Default)]
struct OpenCompaction {
    started_at: Option<DateTime<Utc>>,
    /// `/compact` rather than the automatic window-pressure trigger.
    manual: bool,
    /// Heuristic token price of the range the summary replaced.
    pre_tokens: Option<u64>,
    /// Tokens the summarization call emitted, i.e. the replacement's size.
    post_tokens: Option<u64>,
}

/// The `compactionId` correlating a compaction's three events.
fn compaction_id(data: &Value) -> Option<String> {
    data.get("compactionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(String::from)
}

fn parse_session_events(text: &str, attachments: Option<&Path>) -> SessionParse {
    let mut sp = SessionParse::default();

    // The open assistant turn for the CURRENT log turn: index into `sp.turns`,
    // the accumulated per-step usage, and the `turn/start` clock.
    let mut open_assistant: Option<usize> = None;
    let mut pending_usage: Option<TurnUsage> = None;
    let mut turn_started_at: Option<DateTime<Utc>> = None;
    // Compactions between their `start` and `end` markers, by `compactionId`.
    // A map rather than a single slot because the markers bracket a model call:
    // context injected while the summary runs may sit between the pair, and
    // nothing in the log format promises the brackets never interleave.
    let mut open_compactions: HashMap<String, OpenCompaction> = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        // Raw stream chunks duplicate `assistant/message` content (and the
        // compacted `*-chunks` rows carry `seq0`/`time0` instead of `time`).
        if matches!(
            event_type,
            "assistant/chunk" | "tool-call-chunks" | "reasoning-chunks" | "text-chunks"
        ) {
            continue;
        }
        let ts = event_millis(&value);
        if let Some(ts) = ts {
            sp.first_ts.get_or_insert(ts);
            sp.last_ts = Some(ts);
        }
        let data = value.get("data");

        match event_type {
            "session" => {
                sp.cwd = value
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from);
                sp.created_at = value
                    .get("createdAt")
                    .and_then(Value::as_i64)
                    .and_then(DateTime::from_timestamp_millis);
                sp.delegation_depth = value
                    .get("delegationDepth")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
            }
            "turn/start" => {
                // Defensive: a dangling assistant turn from a log missing its
                // `turn/end` must not absorb the next turn's steps.
                finalize_assistant(
                    &mut sp.turns,
                    &mut open_assistant,
                    &mut pending_usage,
                    None,
                    turn_started_at,
                );
                turn_started_at = ts;
            }
            "turn/end" => {
                finalize_assistant(
                    &mut sp.turns,
                    &mut open_assistant,
                    &mut pending_usage,
                    ts,
                    turn_started_at,
                );
                turn_started_at = None;
            }
            "user/message" => {
                let Some(data) = data else { continue };
                // Plugin-injected runtime-context snapshots (sandbox/approval
                // policy, file-change notices, …) are model plumbing, not
                // conversation content.
                let kind = data
                    .pointer("/source/kind")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if kind != "user" {
                    continue;
                }
                let text = collect_text_parts(data.get("content"));
                // Since 0.6.0 a prompt may be images only ("what is wrong with
                // this screenshot?"), so empty text no longer means an empty
                // turn — check for renderable blocks instead. Skipping on text
                // alone would delete the question and leave its answer.
                let images = user_image_blocks(data.get("content"), attachments);
                if text.trim().is_empty() && images.is_empty() {
                    continue;
                }
                if sp.first_user_text.is_none() && !text.trim().is_empty() {
                    sp.first_user_text = Some(title_from_user_text(text.trim()));
                }
                sp.content_events += 1;
                sp.message_count += 1;
                let ts = ts.or(sp.last_ts).unwrap_or_else(Utc::now);
                // Text first, then the images: upstream keeps the two
                // interleaved in wire order for the model, but a history turn
                // renders attachments as a strip beside the prose, so the
                // ordering within the block list is not what the reader sees.
                let mut blocks = Vec::with_capacity(images.len() + 1);
                if !text.trim().is_empty() {
                    blocks.push(ContentBlock::Text { text });
                }
                blocks.extend(images);
                sp.turns.push(MessageTurn {
                    id: format!("turn-{}", sp.turns.len()),
                    role: TurnRole::User,
                    blocks,
                    timestamp: ts,
                    usage: None,
                    duration_ms: None,
                    model: None,
                    completed_at: Some(ts),
                });
            }
            "request/header" => {
                if let Some(model) = data
                    .and_then(|d| d.pointer("/header/config/model"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    sp.model = Some(model.to_string());
                }
            }
            "request/context" => {
                if let Some(window) = data
                    .and_then(|d| d.get("contextWindow"))
                    .and_then(Value::as_u64)
                    .filter(|w| *w > 0)
                {
                    sp.context_window = Some(window);
                }
            }
            "session/title" => {
                if let Some(title) = data
                    .and_then(|d| d.get("title"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    sp.title = Some(title.to_string());
                }
            }
            "assistant/message" => {
                let Some(data) = data else { continue };
                let step_usage = usage_from_step(data.get("usage"));
                if let Some(usage) = &step_usage {
                    sp.last_step_input_side = Some(
                        usage
                            .input_tokens
                            .saturating_add(usage.cache_read_input_tokens),
                    );
                    pending_usage = Some(match pending_usage.take() {
                        Some(prev) => add_usage(prev, usage.clone()),
                        None => usage.clone(),
                    });
                }
                let message = data.get("message");
                let source_model = message
                    .and_then(|m| m.pointer("/source/model"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from);
                if sp.model.is_none() {
                    sp.model.clone_from(&source_model);
                }
                let Some(blocks) = message
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                // The list-view activity count grows once per assistant
                // MESSAGE with text, not once per text block inside it.
                let mut counted_text = false;
                for block in blocks {
                    let rendered = match block.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text" => {
                            let text = block
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if text.trim().is_empty() {
                                continue;
                            }
                            if !counted_text {
                                sp.message_count += 1;
                                counted_text = true;
                            }
                            ContentBlock::Text {
                                text: text.to_string(),
                            }
                        }
                        "reasoning" => {
                            let text = block
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if text.trim().is_empty() {
                                continue;
                            }
                            ContentBlock::Thinking {
                                text: text.to_string(),
                            }
                        }
                        "tool-call" => ContentBlock::ToolUse {
                            tool_use_id: block.get("id").and_then(Value::as_str).map(String::from),
                            tool_name: block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
                            input_preview: block
                                .get("arguments")
                                .and_then(Value::as_str)
                                .and_then(deepseek_tool_input_preview),
                            status: None,
                            meta: None,
                        },
                        // Upstream declares `image` role-neutral and says
                        // assistant-side rendering is forward compatibility —
                        // today's adapters are all text-only on output, so no
                        // real log reaches this arm.
                        //
                        // RECORDED, not yet rendered: the frontend surfaces
                        // images from USER turns only (`ai-elements-adapter`
                        // extracts them into `userImages`; an assistant-turn
                        // image block falls to its `default: return null`). So
                        // this arm's job is to get the block into the stored
                        // turn rather than drop it on the floor — the day an
                        // adapter emits one, the data is already there and only
                        // the renderer has to learn it.
                        "image" => attachment_image_block(block.get("attachment"), attachments),
                        _ => continue,
                    };
                    sp.content_events += 1;
                    let ts = ts.or(sp.last_ts).unwrap_or_else(Utc::now);
                    let turn = ensure_assistant(
                        &mut sp.turns,
                        &mut open_assistant,
                        ts,
                        sp.model.clone().or(source_model.clone()),
                    );
                    turn.blocks.push(rendered);
                }
            }
            "tool/result" => {
                let Some(data) = data else { continue };
                let result = data
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .and_then(|items| {
                        items
                            .iter()
                            .find(|i| i.get("type").and_then(Value::as_str) == Some("tool-result"))
                    });
                let tool_use_id = result
                    .and_then(|r| r.get("toolCallId"))
                    .and_then(Value::as_str)
                    .or_else(|| {
                        data.pointer("/message/source/callId")
                            .and_then(Value::as_str)
                    })
                    .map(String::from);
                let output = result
                    .map(|r| collect_text_parts(r.get("content")))
                    .unwrap_or_default();
                // `data.error` is the tool's internal failure identity; the
                // model-facing `isError` flag is the primary signal.
                let is_error = result
                    .and_then(|r| r.get("isError"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    || data.get("error").is_some_and(|e| !e.is_null());
                sp.content_events += 1;
                let ts = ts.or(sp.last_ts).unwrap_or_else(Utc::now);
                let model = sp.model.clone();
                let turn = ensure_assistant(&mut sp.turns, &mut open_assistant, ts, model);
                turn.blocks.push(ContentBlock::ToolResult {
                    tool_use_id,
                    output_preview: (!output.trim().is_empty()).then_some(output),
                    is_error,
                    agent_stats: None,
                    images: Vec::new(),
                });
            }
            // ── Context compaction (0.6.0) ───────────────────────────────
            //
            // Upstream writes one compaction as three events: `start` takes the
            // lock, `summary` records the summary and what it replaced, `end`
            // releases the lock (carrying `error` when the attempt failed).
            //
            // The LOG is the only place codeg can see this. On the wire the
            // matching `compaction_update` / `compaction_summary_chunk` updates
            // are gated behind a `clientCapabilities.session.compaction` that
            // the pinned ACP schema crate cannot advertise, so the agent
            // correctly never sends them — see the DeepSeek entry in
            // `acp::registry`. Nothing is lost from the transcript either way:
            // the log is append-only, so the compacted turns are all still here
            // and history shows them. What the marker adds is WHY the model
            // stopped remembering them.
            "compaction/start" => {
                let Some(data) = data else { continue };
                let Some(id) = compaction_id(data) else { continue };
                let entry = open_compactions.entry(id).or_default();
                entry.started_at = ts;
                entry.manual = data.get("sourceCommandId").is_some();
            }
            "compaction/summary" => {
                let Some(data) = data else { continue };
                let Some(id) = compaction_id(data) else { continue };
                // `or_default` rather than a lookup: a log can begin AFTER the
                // opening marker (a fork seed, or a truncated prefix from a
                // half-written final frame), and a compaction with no visible
                // `start` still deserves its marker.
                let entry = open_compactions.entry(id).or_default();
                if data.get("sourceCommandId").is_some() {
                    entry.manual = true;
                }
                // What the replaced span cost, and what replaced it. `usage` is
                // the SUMMARIZATION request's own accounting, so its output side
                // is the summary's size — the two together are the reduction
                // this compaction bought.
                entry.pre_tokens = data.get("shadowedTokenCount").and_then(Value::as_u64);
                entry.post_tokens = data.pointer("/usage/outputTokens").and_then(Value::as_u64);
            }
            "compaction/end" => {
                let Some(data) = data else { continue };
                let Some(id) = compaction_id(data) else { continue };
                let entry = open_compactions.remove(&id).unwrap_or_default();
                let error = data
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|e| !e.is_empty());
                let mut marker = serde_json::Map::new();
                marker.insert("version".to_string(), Value::from(1));
                // `trigger` is adapter-defined vocabulary the card surfaces as a
                // tooltip; upstream distinguishes the two by whether a command
                // initiated the transaction. All THREE events repeat
                // `sourceCommandId`, and this is the only one guaranteed to be
                // in the log — reading it from the opening marker alone would
                // relabel a `/compact` as automatic whenever its `start` fell
                // outside a fork seed or a truncated prefix.
                let manual = entry.manual || data.get("sourceCommandId").is_some();
                marker.insert(
                    "trigger".to_string(),
                    Value::from(if manual { "manual" } else { "auto" }),
                );
                // Counts only for a SUCCESSFUL compaction: a failed attempt
                // replaced nothing, so reporting a reduction it did not make
                // would be a lie told in the failure's own label.
                if error.is_none() {
                    if let Some(pre) = entry.pre_tokens {
                        marker.insert("preTokens".to_string(), Value::from(pre));
                    }
                    if let Some(post) = entry.post_tokens {
                        marker.insert("postTokens".to_string(), Value::from(post));
                    }
                }
                if let (Some(started), Some(ended)) = (entry.started_at, ts) {
                    let elapsed = (ended - started).num_milliseconds();
                    if elapsed > 0 {
                        marker.insert("durationMs".to_string(), Value::from(elapsed));
                    }
                }
                if let Some(error) = error {
                    marker.insert("error".to_string(), Value::from(error));
                }

                sp.content_events += 1;
                let ts = ts.or(sp.last_ts).unwrap_or_else(Utc::now);
                let model = sp.model.clone();
                let turn = ensure_assistant(&mut sp.turns, &mut open_assistant, ts, model);
                // The compaction id is the log's own identity for the
                // transaction, so re-parsing the same log yields the same
                // tool_use_id. The pair is self-contained — a ToolUse with no
                // ToolResult would read as a call still running.
                turn.blocks.push(ContentBlock::ToolUse {
                    tool_use_id: Some(id.clone()),
                    tool_name: "context_compaction".to_string(),
                    input_preview: None,
                    status: None,
                    meta: Some(Value::Object(
                        [("contextCompaction".to_string(), Value::Object(marker))]
                            .into_iter()
                            .collect(),
                    )),
                });
                turn.blocks.push(ContentBlock::ToolResult {
                    tool_use_id: Some(id),
                    output_preview: None,
                    is_error: error.is_some(),
                    agent_stats: None,
                    images: Vec::new(),
                });
            }
            // `agent/inbox/spliced` (queue bookkeeping), `step/start`/`step/end`
            // (sub-turn markers), `sandbox/mode`, `todo/write` (the todo_write
            // TOOL call already renders), and `session/end-seed` (resume seed
            // boundary) carry no conversation content. `compaction/prune` is
            // the model-free prune variant's metering record — it has no
            // start/end bracket of its own, and the backend this bridge
            // composes (`dsh-compaction-basic`) only ever summarizes, so no log
            // codeg reads contains one.
            _ => {}
        }
    }

    finalize_assistant(
        &mut sp.turns,
        &mut open_assistant,
        &mut pending_usage,
        None,
        turn_started_at,
    );
    sp
}

/// The open assistant turn for the current log turn, created on first use.
fn ensure_assistant<'a>(
    turns: &'a mut Vec<MessageTurn>,
    open_assistant: &mut Option<usize>,
    ts: DateTime<Utc>,
    model: Option<String>,
) -> &'a mut MessageTurn {
    let idx = match open_assistant {
        Some(idx) => *idx,
        None => {
            turns.push(MessageTurn {
                id: format!("turn-{}", turns.len()),
                role: TurnRole::Assistant,
                blocks: Vec::new(),
                timestamp: ts,
                usage: None,
                duration_ms: None,
                model,
                completed_at: None,
            });
            let idx = turns.len() - 1;
            *open_assistant = Some(idx);
            idx
        }
    };
    &mut turns[idx]
}

/// Close the open assistant turn: attach the summed step usage, and — when the
/// log recorded a `turn/end` — the completion clock and the `turn/start`-based
/// duration. A turn missing its `turn/end` (killed mid-flight) keeps honest
/// `None` clocks; `backfill_turn_durations` tiles an estimate later.
fn finalize_assistant(
    turns: &mut [MessageTurn],
    open_assistant: &mut Option<usize>,
    pending_usage: &mut Option<TurnUsage>,
    ended_at: Option<DateTime<Utc>>,
    started_at: Option<DateTime<Utc>>,
) {
    let Some(idx) = open_assistant.take() else {
        *pending_usage = None;
        return;
    };
    let Some(turn) = turns.get_mut(idx) else {
        *pending_usage = None;
        return;
    };
    if let Some(usage) = pending_usage.take() {
        turn.usage = Some(match turn.usage.take() {
            Some(existing) => add_usage(existing, usage),
            None => usage,
        });
    }
    if let Some(end) = ended_at {
        turn.completed_at = Some(end);
        if let Some(start) = started_at {
            let ms = (end - start).num_milliseconds();
            if ms > 0 {
                turn.duration_ms = Some(ms as u64);
            }
        }
    }
}

/// List immediate sub-directories of `dir` (empty when `dir` is missing or not
/// a directory). Shallow by design — the layout is exactly two levels deep.
fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_home_prefers_env_override() {
        assert_eq!(
            resolve_dsh_home_from(
                Some(OsString::from("/tmp/custom-dsh")),
                Some(PathBuf::from("/home/demo")),
            ),
            PathBuf::from("/tmp/custom-dsh")
        );
        assert_eq!(
            resolve_dsh_home_from(None, Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.dsh")
        );
        // An empty env value must not produce an empty base path.
        assert_eq!(
            resolve_dsh_home_from(Some(OsString::new()), Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.dsh")
        );
        // Upstream `resolveDshHome` treats a WHITESPACE-only override as unset
        // too ("a blank override never resolves the home to the cwd").
        assert_eq!(
            resolve_dsh_home_from(Some(OsString::from("   ")), Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.dsh")
        );
        // ... and expands a leading `~` (`expandHomePath`) before use, so a
        // `DSH_HOME=~/custom` names the same directory for codeg as for the
        // agent instead of a literal `./~/custom`.
        assert_eq!(
            resolve_dsh_home_from(
                Some(OsString::from("~/custom-dsh")),
                Some(PathBuf::from("/home/demo")),
            ),
            PathBuf::from("/home/demo/custom-dsh")
        );
        assert_eq!(
            resolve_dsh_home_from(Some(OsString::from("~")), Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo")
        );
        // `~user` is NOT a prefix upstream expands — kept verbatim.
        assert_eq!(
            resolve_dsh_home_from(Some(OsString::from("~root/x")), Some(PathBuf::from("/home/demo"))),
            PathBuf::from("~root/x")
        );
    }

    #[test]
    fn resolve_agents_home_prefers_env_override() {
        assert_eq!(
            resolve_dsh_agents_home_from(
                Some(OsString::from("/tmp/shared-agents")),
                Some(PathBuf::from("/home/demo")),
            ),
            PathBuf::from("/tmp/shared-agents")
        );
        assert_eq!(
            resolve_dsh_agents_home_from(None, Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.agents")
        );
        assert_eq!(
            resolve_dsh_agents_home_from(Some(OsString::new()), Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.agents")
        );
    }

    #[test]
    fn resolve_sessions_root_mirrors_adapter_chain() {
        // Explicit sessions root wins over everything.
        assert_eq!(
            resolve_deepseek_sessions_root_from(
                Some(OsString::from("/data/ds-sessions")),
                Some(OsString::from("/tmp/dsh")),
                Some(PathBuf::from("/home/demo")),
            ),
            PathBuf::from("/data/ds-sessions")
        );
        // Else $DSH_HOME/sessions.
        assert_eq!(
            resolve_deepseek_sessions_root_from(
                None,
                Some(OsString::from("/tmp/dsh")),
                Some(PathBuf::from("/home/demo")),
            ),
            PathBuf::from("/tmp/dsh/sessions")
        );
        // Else ~/.dsh/sessions.
        assert_eq!(
            resolve_deepseek_sessions_root_from(None, None, Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.dsh/sessions")
        );
    }

    fn header_line(cwd: &str) -> String {
        json!({
            "type": "session",
            "version": 0,
            "id": "0126397e-97b1-4420-a564-bffe4453915b",
            "createdAt": 1_786_708_736_990_i64,
            "cwd": cwd,
            "delegationDepth": 0
        })
        .to_string()
    }

    fn event(event_type: &str, seq: u64, time: i64, data: Value) -> String {
        json!({"type": event_type, "seq": seq, "time": time, "data": data}).to_string()
    }

    /// A faithful miniature of a real session: one turn with a user prompt, a
    /// plugin context snapshot, a reasoning + tool-call step, its result, and a
    /// closing text step.
    fn sample_log() -> String {
        let user = event(
            "user/message",
            4,
            1_786_708_737_018,
            json!({
                "content": [{"type": "text", "text": "读一下 /tmp/target.txt"}],
                "source": {"kind": "user"},
                "role": "user",
                "id": "u-1"
            }),
        );
        let plugin = event(
            "user/message",
            5,
            1_786_708_737_019,
            json!({
                "content": [{"type": "text", "text": "Current runtime context."}],
                "source": {"kind": "plugin", "plugin": "@deepseek-ai/dsh-system-prompt"},
                "role": "user",
                "id": "u-2"
            }),
        );
        [
            header_line("/Users/demo/project"),
            event("turn/start", 1, 1_786_708_737_006, json!({"turn": 1})),
            event("step/start", 3, 1_786_708_737_017, json!({"turn": 1, "step": 1})),
            user,
            plugin,
            event(
                "session/title",
                6,
                1_786_708_737_020,
                json!({"title": "读一下 /tmp/target.txt", "messageSeqs": [4], "source": {"kind": "fallback"}}),
            ),
            event(
                "request/header",
                7,
                1_786_708_737_022,
                json!({"header": {"config": {"provider": "deepseek-official", "model": "deepseek-v4-flash", "maxTokens": 256_000, "reasoningEffort": "high"}}, "reason": "initial"}),
            ),
            event(
                "request/context",
                8,
                1_786_708_737_023,
                json!({"provider": "deepseek-official", "model": "deepseek-v4-flash", "contextWindow": 1_000_000}),
            ),
            event(
                "assistant/message",
                29,
                1_786_708_738_169,
                json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"type": "reasoning", "text": "先读文件。"},
                            {"type": "tool-call", "id": "call_1", "name": "read", "arguments": "{\"file_path\": \"/tmp/target.txt\"}"}
                        ],
                        "source": {"kind": "model", "provider": "deepseek-official", "model": "deepseek-v4-flash"},
                        "id": "a-1"
                    },
                    "usage": {"inputTokens": 1514, "outputTokens": 49, "cacheReadTokens": 1792, "reasoningTokens": 12}
                }),
            ),
            event(
                "tool/call",
                30,
                1_786_708_738_172,
                json!({"turn": 1, "step": 1, "callId": "call_1", "name": "read", "arguments": "{\"file_path\": \"/tmp/target.txt\"}"}),
            ),
            event(
                "tool/result",
                31,
                1_786_708_738_183,
                json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "source": {"kind": "tool", "callId": "call_1"},
                        "content": [{
                            "type": "tool-result",
                            "toolCallId": "call_1",
                            "content": [{"type": "text", "text": "<content>hello</content>"}],
                            "isError": false
                        }],
                        "role": "user",
                        "id": "t-1"
                    },
                    "meta": {"path": "/tmp/target.txt"}
                }),
            ),
            event(
                "assistant/message",
                40,
                1_786_708_739_000,
                json!({
                    "turn": 1, "step": 2,
                    "message": {
                        "role": "assistant",
                        "content": [{"type": "text", "text": "文件内容是 hello。"}],
                        "source": {"kind": "model", "provider": "deepseek-official", "model": "deepseek-v4-flash"},
                        "id": "a-2"
                    },
                    "usage": {"inputTokens": 111, "outputTokens": 100, "cacheReadTokens": 5248, "reasoningTokens": 0}
                }),
            ),
            event("turn/end", 41, 1_786_708_739_100, json!({"turn": 1, "reason": {"kind": "completed"}})),
            event("session/end-seed", 42, 1_786_708_739_101, json!({})),
        ]
        .join("\n")
    }

    #[test]
    fn parses_a_full_turn_with_usage_duration_and_title() {
        let sp = parse_session_events(&sample_log(), None);

        assert_eq!(sp.cwd.as_deref(), Some("/Users/demo/project"));
        assert_eq!(sp.delegation_depth, 0);
        assert_eq!(sp.title.as_deref(), Some("读一下 /tmp/target.txt"));
        assert_eq!(sp.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(sp.context_window, Some(1_000_000));
        // Latest step's input side, never the per-turn sum.
        assert_eq!(sp.last_step_input_side, Some(111 + 5248));

        // One user turn + ONE assistant turn per log turn (the plugin context
        // snapshot is filtered).
        assert_eq!(sp.turns.len(), 2);
        assert!(matches!(sp.turns[0].role, TurnRole::User));
        assert_eq!(sp.turns[0].blocks.len(), 1);
        assert!(matches!(
            &sp.turns[0].blocks[0],
            ContentBlock::Text { text } if text == "读一下 /tmp/target.txt"
        ));

        let assistant = &sp.turns[1];
        assert!(matches!(assistant.role, TurnRole::Assistant));
        assert_eq!(assistant.model.as_deref(), Some("deepseek-v4-flash"));
        // reasoning → Thinking, tool-call → ToolUse, result → ToolResult,
        // closing text → Text, in stream order.
        assert_eq!(assistant.blocks.len(), 4);
        assert!(
            matches!(&assistant.blocks[0], ContentBlock::Thinking { text } if text == "先读文件。")
        );
        assert!(matches!(
            &assistant.blocks[1],
            ContentBlock::ToolUse { tool_use_id: Some(id), tool_name, input_preview: Some(input), .. }
                if id == "call_1" && tool_name == "read" && input.contains("/tmp/target.txt")
        ));
        assert!(matches!(
            &assistant.blocks[2],
            ContentBlock::ToolResult { tool_use_id: Some(id), output_preview: Some(out), is_error: false, .. }
                if id == "call_1" && out.contains("hello")
        ));
        assert!(
            matches!(&assistant.blocks[3], ContentBlock::Text { text } if text == "文件内容是 hello。")
        );

        // Steps SUMMED into the turn.
        let usage = assistant.usage.as_ref().expect("usage");
        assert_eq!(usage.input_tokens, 1514 + 111);
        assert_eq!(usage.output_tokens, 49 + 100);
        assert_eq!(usage.cache_read_input_tokens, 1792 + 5248);
        assert_eq!(usage.cache_creation_input_tokens, 0);

        // turn/start (…737_006) → turn/end (…739_100).
        assert_eq!(assistant.duration_ms, Some(2094));
        assert_eq!(
            assistant.completed_at,
            DateTime::from_timestamp_millis(1_786_708_739_100)
        );

        // user prompt + assistant text message.
        assert_eq!(sp.message_count, 2);
        assert_eq!(
            sp.created_at,
            DateTime::from_timestamp_millis(1_786_708_736_990)
        );
    }

    #[test]
    fn aborted_turn_without_end_keeps_honest_clocks() {
        let log = [
            header_line("/Users/demo/project"),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            event(
                "user/message",
                2,
                1_010,
                json!({"content": [{"type": "text", "text": "hi"}], "source": {"kind": "user"}, "role": "user", "id": "u"}),
            ),
            event(
                "assistant/message",
                3,
                1_500,
                json!({
                    "turn": 1, "step": 1,
                    "message": {"role": "assistant", "content": [{"type": "text", "text": "part"}], "source": {"kind": "model", "model": "deepseek-v4"}, "id": "a"},
                    "usage": {"inputTokens": 10, "outputTokens": 5, "cacheReadTokens": 0}
                }),
            ),
            // Killed mid-flight: no turn/end, no session/end-seed.
        ]
        .join("\n");

        let sp = parse_session_events(&log, None);
        assert_eq!(sp.turns.len(), 2);
        let assistant = &sp.turns[1];
        // Usage still flushed at EOF; clocks stay honest.
        assert_eq!(assistant.usage.as_ref().unwrap().input_tokens, 10);
        assert_eq!(assistant.completed_at, None);
        assert_eq!(assistant.duration_ms, None);
        // The header model fallback comes from the assistant message source.
        assert_eq!(sp.model.as_deref(), Some("deepseek-v4"));
    }

    #[test]
    fn tool_error_results_are_flagged() {
        let log = [
            header_line("/w"),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            event(
                "tool/result",
                2,
                1_100,
                json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "source": {"kind": "tool", "callId": "c1"},
                        "content": [{"type": "tool-result", "toolCallId": "c1", "content": [{"type": "text", "text": "Error: MCP error -32001: Request timed out"}], "isError": true}],
                        "role": "user", "id": "t"
                    }
                }),
            ),
        ]
        .join("\n");
        let sp = parse_session_events(&log, None);
        assert_eq!(sp.turns.len(), 1);
        assert!(matches!(
            &sp.turns[0].blocks[0],
            ContentBlock::ToolResult { is_error: true, output_preview: Some(out), .. }
                if out.contains("timed out")
        ));
    }

    #[test]
    fn oversized_tool_arguments_stay_valid_json() {
        let long_task = "长".repeat(DEEPSEEK_TOOL_INPUT_CAP);
        let arguments =
            json!({"agent_type": "codex", "task": long_task, "working_dir": "/w"}).to_string();
        assert!(arguments.len() > DEEPSEEK_TOOL_INPUT_CAP);

        let preview = deepseek_tool_input_preview(&arguments).expect("preview");
        assert!(preview.len() <= DEEPSEEK_TOOL_INPUT_CAP);
        let parsed: Value = serde_json::from_str(&preview).expect("valid JSON");
        assert_eq!(parsed["agent_type"], "codex");
        assert_eq!(parsed["working_dir"], "/w");

        // Small arguments pass through byte-identical.
        let small = "{\"file_path\": \"/tmp/a\"}";
        assert_eq!(deepseek_tool_input_preview(small).as_deref(), Some(small));
    }

    fn write_session(dir: &Path, bucket: &str, id: &str, log: &str, compressed: bool) {
        let session_dir = dir.join(bucket).join(id);
        fs::create_dir_all(&session_dir).expect("mkdir");
        if compressed {
            // Two separately-encoded frames concatenated — the exact shape the
            // append-only writer produces — must decode as one stream. The
            // split point is arbitrary (frames carry bytes, not lines).
            let raw = log.as_bytes();
            let split = raw.len() / 2;
            let mut bytes = zstd::stream::encode_all(&raw[..split], 0).expect("frame 1");
            bytes.extend(zstd::stream::encode_all(&raw[split..], 0).expect("frame 2"));
            fs::write(session_dir.join("session.jsonl.zstd"), bytes).expect("write zstd");
        } else {
            fs::write(session_dir.join("session.jsonl"), log).expect("write jsonl");
        }
    }

    #[test]
    fn lists_and_loads_a_zstd_multi_frame_session() {
        let dir = std::env::temp_dir().join(format!("deepseek-parser-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let id = "0126397e-97b1-4420-a564-bffe4453915b";
        write_session(&dir, "--Users-demo-project--", id, &sample_log(), true);

        let parser = DeepSeekParser::with_base_dir(dir.clone());
        let conversations = parser.list_conversations().expect("list");
        assert_eq!(conversations.len(), 1);
        let summary = &conversations[0];
        assert_eq!(summary.id, id);
        assert_eq!(summary.agent_type, AgentType::DeepSeek);
        assert_eq!(summary.folder_path.as_deref(), Some("/Users/demo/project"));
        assert_eq!(summary.folder_name.as_deref(), Some("project"));
        assert_eq!(summary.title.as_deref(), Some("读一下 /tmp/target.txt"));
        assert_eq!(summary.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(summary.message_count, 2);

        let detail = parser.get_conversation(id).expect("detail");
        assert_eq!(detail.turns.len(), 2);
        let stats = detail.session_stats.expect("stats");
        assert_eq!(stats.context_window_used_tokens, Some(111 + 5248));
        assert_eq!(stats.context_window_max_tokens, Some(1_000_000));
        let total = stats.total_usage.expect("usage");
        assert_eq!(total.input_tokens, 1514 + 111);
        assert_eq!(total.output_tokens, 49 + 100);

        let _ = fs::remove_dir_all(&dir);
    }

    // Mid-write race: the appender's final frame can be half-written when a
    // list refresh reads the file. The complete frames before it must still
    // parse — dropping them would make a LIVE session vanish from the list.
    #[test]
    fn torn_trailing_frame_keeps_the_decoded_prefix() {
        let mut bytes =
            zstd::stream::encode_all(sample_log().as_bytes(), 0).expect("complete frame");
        let torn = zstd::stream::encode_all(
            format!(
                "\n{}",
                event("turn/start", 50, 1_786_708_740_000, json!({"turn": 2}))
            )
            .as_bytes(),
            0,
        )
        .expect("tail frame");
        bytes.extend(&torn[..torn.len() / 2]);

        let text = decode_zstd_frames_prefix(&bytes).expect("prefix survives");
        let sp = parse_session_events(&text, None);
        assert_eq!(sp.turns.len(), 2);
        assert_eq!(sp.title.as_deref(), Some("读一下 /tmp/target.txt"));
    }

    #[test]
    fn assistant_message_with_many_text_blocks_counts_once() {
        let log = [
            header_line("/w"),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            event(
                "assistant/message",
                2,
                1_500,
                json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"type": "text", "text": "part one"},
                            {"type": "text", "text": "part two"}
                        ],
                        "source": {"kind": "model", "model": "deepseek-v4"},
                        "id": "a"
                    }
                }),
            ),
        ]
        .join("\n");
        let sp = parse_session_events(&log, None);
        assert_eq!(sp.message_count, 1);
        assert_eq!(sp.turns[0].blocks.len(), 2);
    }

    #[test]
    fn skips_empty_plaintext_and_subagent_sessions() {
        let dir = std::env::temp_dir().join(format!("deepseek-parser-skip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        // Header-only session: metadata is not a conversation.
        write_session(&dir, "--w--", "empty", &header_line("/w"), false);
        // A delegated sub-agent session is not an editor conversation.
        let child = [
            json!({"type": "session", "version": 0, "id": "child", "createdAt": 1_000, "cwd": "/w", "delegationDepth": 1}).to_string(),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            event(
                "user/message",
                2,
                1_010,
                json!({"content": [{"type": "text", "text": "sub"}], "source": {"kind": "user"}, "role": "user", "id": "u"}),
            ),
        ]
        .join("\n");
        write_session(&dir, "--w--", "child", &child, false);
        // A plaintext (compression: none) session IS listed.
        write_session(&dir, "--w--", "plain", &sample_log(), false);

        let parser = DeepSeekParser::with_base_dir(dir.clone());
        let conversations = parser.list_conversations().expect("list");
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].id, "plain");

        let _ = fs::remove_dir_all(&dir);
    }

    // ── 0.6.0: image prompts ────────────────────────────────────────────

    /// A `user/message` carrying one image attachment and the given text.
    fn image_user_event(text: Option<&str>, attachment: Value) -> String {
        let mut content = Vec::new();
        if let Some(text) = text {
            content.push(json!({"type": "text", "text": text}));
        }
        content.push(json!({"type": "image", "attachment": attachment}));
        event(
            "user/message",
            2,
            1_010,
            json!({"content": content, "source": {"kind": "user"}, "role": "user", "id": "u"}),
        )
    }

    fn image_log(text: Option<&str>, attachment: Value) -> String {
        [
            header_line("/w"),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            image_user_event(text, attachment),
        ]
        .join("\n")
    }

    /// The 1×1 PNG the fixtures stage, with its real sha256 (the store is
    /// content-addressed, so the id is the digest of these exact bytes).
    fn tiny_png() -> (Vec<u8>, String) {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let bytes = STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
            .expect("decode fixture png");
        let digest = {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(&bytes))
        };
        (bytes, digest)
    }

    fn stage_attachment(root: &Path, digest: &str, bytes: &[u8]) {
        let bucket = root.join("objects").join(&digest[..2]);
        fs::create_dir_all(&bucket).expect("create bucket");
        fs::write(bucket.join(digest), bytes).expect("write object");
    }

    // An image-only prompt ("what is wrong with this screenshot?") carries no
    // text at all. Before 0.6.0 empty text meant an empty turn, so keying the
    // skip on text alone now deletes the question and leaves its answer.
    #[test]
    fn image_only_prompt_keeps_its_user_turn() {
        let (_, digest) = tiny_png();
        let sp = parse_session_events(
            &image_log(
                None,
                json!({
                    "attachmentId": format!("sha256:{digest}"),
                    "mediaType": "image/png",
                    "bytes": 68, "width": 1, "height": 1, "name": "shot.png"
                }),
            ),
            None,
        );
        assert_eq!(sp.turns.len(), 1);
        assert!(matches!(sp.turns[0].role, TurnRole::User));
        assert_eq!(sp.message_count, 1);
        // Without an attachment root the pixels are not read, but the block
        // still names what was attached.
        assert!(
            matches!(&sp.turns[0].blocks[0], ContentBlock::Text { text } if text == "[image shot.png]"),
            "unexpected block: {:?}",
            sp.turns[0].blocks[0]
        );
    }

    #[test]
    fn detail_inlines_attachment_bytes_but_the_list_does_not() {
        let dir =
            std::env::temp_dir().join(format!("deepseek-parser-image-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (bytes, digest) = tiny_png();
        stage_attachment(&dir.join("attachments").join("v1"), &digest, &bytes);
        write_session(
            &dir,
            "--w--",
            "img",
            &image_log(
                Some("这张图里是什么？"),
                json!({
                    "attachmentId": format!("sha256:{digest}"),
                    "mediaType": "image/png",
                    "bytes": bytes.len(), "width": 1, "height": 1
                }),
            ),
            false,
        );

        let parser = DeepSeekParser::with_base_dir(dir.clone());
        // The list path never renders blocks, so it must not pay for the bytes
        // — it only has to agree that the session is worth listing.
        let conversations = parser.list_conversations().expect("list");
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].title.as_deref(), Some("这张图里是什么？"));

        let detail = parser.get_conversation("img").expect("detail");
        assert_eq!(detail.turns.len(), 1);
        let blocks = &detail.turns[0].blocks;
        assert_eq!(blocks.len(), 2, "text then image: {blocks:?}");
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "这张图里是什么？"));
        match &blocks[1] {
            ContentBlock::Image {
                data,
                mime_type,
                uri,
            } => {
                use base64::{engine::general_purpose::STANDARD, Engine as _};
                assert_eq!(STANDARD.decode(data).expect("valid base64"), bytes);
                assert_eq!(mime_type, "image/png");
                // No `name` on the ref, and the id is a digest rather than a
                // file name — the frontend derives `image.png` from the mime.
                assert_eq!(uri.as_deref(), None);
            }
            other => panic!("expected an inlined image, got {other:?}"),
        }

        let _ = fs::remove_dir_all(&dir);
    }

    // The attachment store is prunable independently of the logs, so a log can
    // outlive its pixels. The turn must survive that.
    #[test]
    fn a_missing_or_oversized_attachment_degrades_to_a_note() {
        let dir = std::env::temp_dir().join(format!("deepseek-parser-gone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (bytes, digest) = tiny_png();
        let root = dir.join("attachments").join("v1");

        // Object never written.
        let gone = attachment_image_block(
            Some(&json!({
                "attachmentId": format!("sha256:{digest}"),
                "mediaType": "image/png", "bytes": 68, "width": 1, "height": 1,
                "name": "shot.png"
            })),
            Some(&root),
        );
        assert!(matches!(&gone, ContentBlock::Text { text } if text == "[image shot.png]"));

        // Present, but the ref declares more bytes than we will inline — the
        // check reads the ref, so the file is never opened.
        stage_attachment(&root, &digest, &bytes);
        let huge = attachment_image_block(
            Some(&json!({
                "attachmentId": format!("sha256:{digest}"),
                "mediaType": "image/png",
                "bytes": DEEPSEEK_MAX_INLINE_IMAGE_BYTES + 1,
                "width": 20_000, "height": 20_000
            })),
            Some(&root),
        );
        assert!(matches!(&huge, ContentBlock::Text { text } if text == "[image image/png]"));

        let _ = fs::remove_dir_all(&dir);
    }

    // The hex goes straight into a path, so a hand-edited or malformed id must
    // not become a directory traversal.
    #[test]
    fn attachment_ids_that_are_not_a_sha256_digest_are_rejected() {
        assert_eq!(
            attachment_object_hex(&format!("sha256:{}", "a".repeat(64))),
            Some("a".repeat(64))
        );
        for bad in [
            "sha256:../../../../etc/passwd",
            "sha256:",
            "sha256:AABB",
            // Right length, but uppercase hex is not what the store mints.
            &format!("sha256:{}", "A".repeat(64)),
            // Right length and charset, wrong scheme.
            &format!("sha1:{}", "a".repeat(64)),
            &"a".repeat(64),
        ] {
            assert_eq!(attachment_object_hex(bad), None, "accepted {bad}");
        }
    }

    // ── 0.6.0: context compaction ───────────────────────────────────────

    fn compaction_log(end_data: Value, with_summary: bool) -> String {
        let mut lines = vec![
            header_line("/w"),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            event(
                "compaction/start",
                2,
                1_100,
                json!({"compactionId": "c-1", "turn": 1}),
            ),
        ];
        if with_summary {
            lines.push(event(
                "compaction/summary",
                3,
                1_800,
                json!({
                    "compactionId": "c-1",
                    "summary": [{"type": "text", "text": "早先的重构讨论。"}],
                    "shadowedRange": {"start": 4, "end": 40},
                    "shadowedSeqs": [4, 5],
                    "shadowedTokenCount": 51_777,
                    "provider": "deepseek-official",
                    "model": "deepseek-v4-flash",
                    "usage": {"inputTokens": 51_777, "outputTokens": 4_616}
                }),
            ));
            // The replacement rides the plugin skip, so the summary text must
            // never surface as something the user said.
            lines.push(event(
                "user/message",
                4,
                1_810,
                json!({
                    "content": [{"type": "text", "text": "早先的重构讨论。"}],
                    "source": {"kind": "plugin", "plugin": "compact", "compactionId": "c-1"},
                    "role": "user", "id": "u-c"
                }),
            ));
        }
        lines.push(event("compaction/end", 5, 2_400, end_data));
        lines.join("\n")
    }

    /// The `contextCompaction` marker off a turn's compaction ToolUse.
    fn compaction_marker(sp: &SessionParse) -> Value {
        let block = sp
            .turns
            .iter()
            .flat_map(|t| &t.blocks)
            .find(|b| matches!(b, ContentBlock::ToolUse { tool_name, .. } if tool_name == "context_compaction"))
            .expect("no compaction ToolUse");
        match block {
            ContentBlock::ToolUse { meta, .. } => meta
                .as_ref()
                .expect("meta")
                .get("contextCompaction")
                .expect("marker")
                .clone(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_completed_compaction_renders_the_shared_marker() {
        let sp = parse_session_events(
            &compaction_log(json!({"compactionId": "c-1", "turn": 1}), true),
            None,
        );
        let marker = compaction_marker(&sp);
        assert_eq!(marker["version"], json!(1));
        assert_eq!(marker["trigger"], json!("auto"));
        assert_eq!(marker["preTokens"], json!(51_777));
        assert_eq!(marker["postTokens"], json!(4_616));
        // `compaction/end` at 2400 closes a `compaction/start` from 1100.
        assert_eq!(marker["durationMs"], json!(1_300));
        assert!(marker.get("error").is_none());

        // The pair must be well-formed: an unpaired ToolUse reads as a call
        // still running.
        let blocks: Vec<_> = sp.turns.iter().flat_map(|t| &t.blocks).collect();
        assert!(blocks.iter().any(|b| matches!(
            b,
            ContentBlock::ToolResult { tool_use_id, is_error: false, .. }
                if tool_use_id.as_deref() == Some("c-1")
        )));
        // The compaction's replacement message is plugin-sourced, so no user
        // turn was invented out of the summary text.
        assert!(!sp
            .turns
            .iter()
            .any(|t| matches!(t.role, TurnRole::User)));
    }

    #[test]
    fn a_failed_compaction_reports_the_error_and_no_counts() {
        let sp = parse_session_events(
            &compaction_log(
                json!({"compactionId": "c-1", "turn": 1, "error": "summarization failed"}),
                true,
            ),
            None,
        );
        let marker = compaction_marker(&sp);
        assert_eq!(marker["error"], json!("summarization failed"));
        // A failed attempt replaced nothing — reporting a reduction it did not
        // make would be a lie told inside the failure's own label.
        assert!(marker.get("preTokens").is_none());
        assert!(marker.get("postTokens").is_none());

        assert!(sp.turns.iter().flat_map(|t| &t.blocks).any(|b| matches!(
            b,
            ContentBlock::ToolResult { is_error: true, .. }
        )));
    }

    // A log can begin after the opening marker (a fork seed, or a prefix left
    // by a half-written final frame). The compaction still gets its marker.
    #[test]
    fn a_compaction_whose_start_is_not_in_the_log_still_renders() {
        let log = [
            header_line("/w"),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            event(
                "compaction/end",
                2,
                2_400,
                json!({"compactionId": "c-9", "turn": null, "sourceCommandId": "cmd-compact"}),
            ),
        ]
        .join("\n");
        let sp = parse_session_events(&log, None);
        let marker = compaction_marker(&sp);
        // `sourceCommandId` is upstream's own manual/auto discriminator.
        assert_eq!(marker["trigger"], json!("manual"));
        // No `start` to measure against, and no `summary` to count.
        assert!(marker.get("durationMs").is_none());
        assert!(marker.get("preTokens").is_none());
    }
}
