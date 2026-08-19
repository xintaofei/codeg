use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sacp::schema::{
    BlobResourceContents, CancelNotification, ClientCapabilities, ContentBlock, ContentChunk,
    CreateTerminalRequest, CreateTerminalResponse, ElicitationCapabilities,
    ElicitationFormCapabilities, EmbeddedResource, EmbeddedResourceResource,
    FileSystemCapabilities, ImageContent, InitializeRequest, KillTerminalRequest,
    KillTerminalResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
    NewSessionResponse, PermissionOptionKind, Plan, PlanEntryPriority, PlanEntryStatus,
    PromptRequest, ProtocolVersion, ReadTextFileRequest, ReadTextFileResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ResourceLink, ResumeSessionRequest,
    ResumeSessionResponse, SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelectGroup,
    SessionConfigSelectOption, SessionConfigSelectOptions, SessionId, SessionModeState,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, StopReason, TerminalExitStatus,
    TerminalOutputRequest, TerminalOutputResponse, TextContent, TextResourceContents,
    ToolCallContent, WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use sacp::schema::{HttpHeader, McpServer, McpServerHttp, McpServerSse, McpServerStdio};
use sacp::util::MatchDispatch;
use sacp::{
    on_receive_request, Agent, Client, ConnectionTo, Dispatch, JsonRpcRequest, Responder,
    SessionMessage, UntypedMessage,
};
use sacp_tokio::AcpAgent;
use tokio::sync::{mpsc, RwLock};

use crate::acp::background_watch;
use crate::acp::error::AcpError;
use crate::acp::file_system_runtime::{
    FileSystemRuntime, FileSystemRuntimeError, FsAccessPolicy, FS_POLICY_ENV,
};
use crate::acp::host_tools_policy::{HostToolsPolicy, HOST_TOOLS_ENV};
use crate::acp::registry::{self, AgentDistribution};
use crate::acp::session_state::SessionState;
use crate::acp::stderr_tail::{summarize_parser_error, StderrTail, TailScope};
use crate::acp::terminal_runtime::{
    TerminalRuntime, TerminalRuntimeError, TerminalShellRuntimeConfig,
};
use crate::acp::types::{
    AcpEvent, AvailableCommandInfo, ConnectionInfo, ConnectionStatus, GrokModelSpec,
    PermissionOptionInfo, PlanEntryInfo, PromptCapabilitiesInfo, PromptInputBlock,
    SessionConfigBooleanInfo, SessionConfigKindInfo, SessionConfigOptionInfo,
    SessionConfigSelectGroupInfo, SessionConfigSelectInfo, SessionConfigSelectOptionInfo,
    SessionFailureRecord, SessionModeInfo, SessionModeStateInfo, ToolCallImageInfo,
    UserMessageBlock,
};
use crate::logging::throttle::LeadingEdgeThrottle;
use crate::models::agent::AgentType;
use crate::network::proxy;
use crate::web::event_bridge::{emit_with_state, EventEmitter};

const DEFAULT_COMMAND_COLOR_ENV: [(&str, &str); 1] = [("CLICOLOR_FORCE", "1")];

fn merge_agent_env(
    env: &[(&'static str, &'static str)],
    runtime_env: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    // Env var order is not semantically meaningful; use map overwrite semantics
    // to keep precedence while avoiding repeated O(n) scans.
    let mut merged = BTreeMap::<String, String>::new();

    for (key, value) in DEFAULT_COMMAND_COLOR_ENV {
        merged.insert(key.to_string(), value.to_string());
    }

    for (key, value) in env {
        merged.insert((*key).to_string(), (*value).to_string());
    }

    for (key, value) in runtime_env {
        merged.insert(key.clone(), value.clone());
    }

    for (key, value) in proxy::current_proxy_env_vars() {
        merged.insert(key, value);
    }

    // Ensure agent-invoked `officecli …` (from an enabled office skill) resolves
    // even when codeg installed the binary outside the user's shell PATH — the
    // Windows self-managed dir, or `~/.local/bin` under a GUI launch.
    prepend_officecli_path(&mut merged);

    merged.into_iter().collect()
}

/// Cursor subscription-mode launch policy. When the user picked the official
/// subscription (browser login), guarantee the launched CLI sees NONE of the
/// custom-endpoint credentials — not even a stale `CURSOR_API_KEY` /
/// `CURSOR_API_BASE_URL` inherited from this process's environment (e.g. a dev
/// shell export). cursor-agent would otherwise validate that leaked key and
/// refuse to fall back to the login credential. An empty value tells the spawn
/// layer (vendored sacp-tokio) to `env_remove` the inherited var.
///
/// Gated on the explicit `CURSOR_AUTH_MODE` knob (written by the Cursor panel),
/// so legacy rows and operator-provided container env are left untouched. In
/// custom mode the credentials are present and non-empty, so nothing is cleared.
fn apply_cursor_env_policy(merged: &mut Vec<(String, String)>, runtime_env: &BTreeMap<String, String>) {
    if runtime_env.get("CURSOR_AUTH_MODE").map(String::as_str) != Some("subscription") {
        return;
    }
    for key in ["CURSOR_API_KEY", "CURSOR_API_BASE_URL"] {
        let already_set = merged
            .iter()
            .any(|(k, v)| k == key && !v.trim().is_empty());
        if !already_set {
            merged.retain(|(k, _)| k != key);
            merged.push((key.to_string(), String::new()));
        }
    }
}

/// Grok's launch-time credential policy, mirroring [`apply_cursor_env_policy`].
/// When the user picked the `grok login` subscription (recorded as
/// `GROK_AUTH_MODE=subscription` by the Grok settings panel), scrub any
/// `XAI_API_KEY` inherited from this process's environment so the CLI falls back
/// to the browser-login credential in `~/.grok/auth.json` rather than a leaked
/// shell/container export. An empty value tells the spawn layer (vendored
/// sacp-tokio) to `env_remove` the inherited var. In api_key mode the key is
/// present and non-empty, so nothing is cleared; legacy/no-mode rows are left
/// untouched.
fn apply_grok_env_policy(merged: &mut Vec<(String, String)>, runtime_env: &BTreeMap<String, String>) {
    if runtime_env.get("GROK_AUTH_MODE").map(String::as_str) != Some("subscription") {
        return;
    }
    let key = "XAI_API_KEY";
    let already_set = merged
        .iter()
        .any(|(k, v)| k == key && !v.trim().is_empty());
    if !already_set {
        merged.retain(|(k, _)| k != key);
        merged.push((key.to_string(), String::new()));
    }
}

/// Codex-only launch policy: force codex-acp's MCP name-conflict de-duplication
/// OFF. codeg injects its companion server (`codeg-mcp`) over ACP
/// `session/new.mcpServers`; codex-acp otherwise drops any ACP-passed server
/// whose name collides with a `config.toml` entry — global *or* project layer
/// (the check was widened to project `.codex/config.toml` in codex-acp #322) —
/// silently stripping codeg-mcp and with it ask_user_question / delegation /
/// feedback / session_info. The late `retain` + `push` makes the override win
/// over any user `runtime_env` twin, so the injection is guaranteed to survive.
/// Codex launch env policy. `initial_agent_mode` is the preset resolved from
/// `~/.codex/config.toml` by `codex_launch_initial_agent_mode` — passed in rather
/// than read here so this stays pure (and so its tests don't depend on whatever
/// `~/.codex/config.toml` the developer happens to have).
fn apply_codex_env_policy(
    agent_type: AgentType,
    merged: &mut Vec<(String, String)>,
    initial_agent_mode: Option<&str>,
) {
    if agent_type != AgentType::Codex {
        return;
    }
    let key = "DISABLE_MCP_CONFIG_FILTERING";
    merged.retain(|(k, _)| k != key);
    merged.push((key.to_string(), "true".to_string()));

    // Make `~/.codex/config.toml`'s sandbox/approval choice mean something.
    // codex-acp re-sends its own approvalPolicy + sandboxPolicy every turn from
    // an `AgentMode` seeded once per session, so without this the user's config
    // is silently overridden by the adapter's default `agent` preset (#442).
    // Same shape as Grok's `--permission-mode` launch flag.
    //
    // A pre-existing value in the runtime env WINS: that is an explicit
    // user-set key, a stronger signal than a config-file inference. A
    // `preferred_mode_id` / `config_values["mode"]` still overrides this after
    // connect via `set_config_option` — explicit choice > config inference.
    let mode_key = "INITIAL_AGENT_MODE";
    if merged
        .iter()
        .any(|(k, v)| k == mode_key && !v.trim().is_empty())
    {
        return;
    }
    if let Some(mode) = initial_agent_mode {
        merged.retain(|(k, _)| k != mode_key);
        merged.push((mode_key.to_string(), mode.to_string()));
    }
}

/// Prepend `dir` to the PATH entry of `env`, seeding from `fallback_path` when
/// `env` has no PATH key of its own. Removes any pre-existing PATH key first
/// (case-insensitively when `windows`, since Windows env keys are
/// case-insensitive) so the result has exactly one PATH entry — otherwise a
/// differently-cased duplicate (e.g. an inherited `Path` plus an inserted
/// `PATH`) could clobber the injected value when the child `Command` applies
/// them. Pure (no env/fs access) so it is unit-tested for both platforms.
fn prepend_dir_to_path_env(
    env: &mut BTreeMap<String, String>,
    dir: &str,
    fallback_path: &str,
    windows: bool,
) {
    let sep = if windows { ';' } else { ':' };
    // Collect every PATH-ish key. `BTreeMap` iterates sorted, so when several
    // differently-cased keys exist (e.g. both `Path` and `PATH`), the last is
    // the one the child `Command` applies last — i.e. the effective value under
    // Windows' case-insensitive env. Remove all of them so exactly one PATH
    // entry remains; a stale duplicate could otherwise overwrite the injected
    // value when the child applies them in order.
    let matching: Vec<String> = env
        .keys()
        .filter(|k| {
            if windows {
                k.eq_ignore_ascii_case("PATH")
            } else {
                k.as_str() == "PATH"
            }
        })
        .cloned()
        .collect();
    let mut existing_val: Option<String> = None;
    for k in &matching {
        existing_val = env.remove(k);
    }
    let existing_val = existing_val.unwrap_or_else(|| fallback_path.to_string());
    let new_path = if existing_val.is_empty() {
        dir.to_string()
    } else {
        format!("{dir}{sep}{existing_val}")
    };
    // Reuse the effective (last-sorted) key's casing when present; otherwise
    // default to the platform-conventional name (`Path` on Windows, `PATH` on Unix).
    let key = matching
        .into_iter()
        .next_back()
        .unwrap_or_else(|| if windows { "Path" } else { "PATH" }.to_string());
    env.insert(key, new_path);
}

/// Prepend codeg's known OfficeCLI install dir to `env`'s PATH when officecli is
/// installed there but not yet on the live PATH (see
/// `office_tools::officecli_agent_path_dir`). Applied to both the agent process
/// env (`merge_agent_env`) and the ACP terminal runtime's base env, so an
/// agent-invoked `officecli` resolves whether the agent execs it directly or
/// runs it through the client `terminal/create` tool. PATH-only: never forwards
/// model/API secrets.
fn prepend_officecli_path(env: &mut BTreeMap<String, String>) {
    if let Some(dir) = crate::commands::office_tools::officecli_agent_path_dir() {
        let fallback = std::env::var("PATH").unwrap_or_default();
        prepend_dir_to_path_env(env, &dir.to_string_lossy(), &fallback, cfg!(windows));
    }
}

/// The two actions codex's bespoke `_codex/session/goal_control` request
/// accepts (codex-acp #293, v1.1.4). Start / resume / re-objective are NOT part
/// of this method — those go through the `/goal` prompt (a real slash command;
/// only `/plan`, a config-option state toggle, is suppressed). Serializes to the
/// lowercase wire value codex expects (`"pause"` / `"clear"`) and deserializes
/// from the same string coming off the tauri command / HTTP endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalControlAction {
    Pause,
    Clear,
}

/// How the adapter disposed of a `_session/steering` message — the wire
/// `outcome` string, parsed by [`parse_steer_outcome`]. The distinction that
/// matters to callers is CONSUMPTION: `Injected` and `StartedNewTurn` mean the
/// adapter took the content (record it delivered, never resend);
/// `PromptRequired` means it did not (safe to resubmit as a normal prompt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerOutcome {
    /// Pushed into the RUNNING turn's input — consumed.
    Injected,
    /// The turn settled first and the adapter honored the
    /// `idleBehavior = "promptRequired"` opt-in — NOT consumed, still
    /// host-owned. The caller reroutes it through `session/prompt`.
    PromptRequired,
    /// The adapter ignored the opt-in (pre-0.64 claude adapter, codex-acp)
    /// and spun up a detached turn no host request owns — consumed. The
    /// manager records it delivered, warns, and downgrades
    /// `native_steering_available` for the rest of the session.
    StartedNewTurn,
}

/// Commands sent from Tauri command handlers to the ACP connection loop.
pub enum ConnectionCommand {
    Prompt {
        blocks: Vec<PromptInputBlock>,
        /// Pre-projected cross-client user-message broadcast (`message_id` +
        /// user blocks), computed by the manager under the prompt lock. The
        /// loop emits it as `AcpEvent::UserMessage` right before issuing the
        /// agent request, so its seq strictly precedes the turn's assistant /
        /// status events (viewers apply in seq order) and it only fires for a
        /// prompt actually being processed. `None` for delegation children,
        /// empty prompts, unbound conversations, and non-linked senders.
        user_message: Option<(String, Vec<UserMessageBlock>)>,
    },
    SetMode {
        mode_id: String,
    },
    SetConfigOption {
        config_id: String,
        value_id: String,
    },
    GoalControl {
        action: GoalControlAction,
        /// Did the goal-control request LAND? Attached only by a caller that
        /// intends to follow a successful control with an interrupt
        /// (`ConnectionManager::goal_control`), because aborting the turn is
        /// destructive and must not happen on the strength of a request the
        /// agent rejected. `None` = fire-and-forget, nobody is listening.
        reply: Option<tokio::sync::oneshot::Sender<bool>>,
    },
    Cancel,
    RespondPermission {
        request_id: String,
        option_id: String,
    },
    Fork {
        reply:
            tokio::sync::oneshot::Sender<Result<crate::acp::types::ForkProtocolResult, AcpError>>,
    },
    /// Inject a live-feedback note into the RUNNING turn over the ACP
    /// `_session/steering` extension (native push channel — see
    /// `manager::submit_feedback`). The loop does the protocol round-trip
    /// only and replies the parsed outcome; recording the note + the
    /// `FeedbackSubmitted` broadcast happen in the manager's
    /// cancellation-shielded task, mirroring Fork's protocol/persistence
    /// split. The idle arm replies `Err(NoActiveTurn)` so the oneshot can
    /// never hang.
    Steer {
        text: String,
        reply: tokio::sync::oneshot::Sender<Result<SteerOutcome, AcpError>>,
    },
    Disconnect,
}

/// Sentinel string embedded in a `sacp::Error` when the Initialize
/// handshake times out. Converted back to `AcpError::InitializeTimeout`
/// by the outer `.map_err(...)` in `run_connection`.
const INIT_TIMEOUT_SENTINEL: &str = "__codeg_init_timeout__";

/// Sentinel appended to a `session/new` failure when codeg had just forwarded
/// MCP servers to a *custom* agent, so the outer `.map_err(...)` can raise
/// `AcpError::McpRejectedByAgent` and point the user at the `supports_mcp`
/// switch. Same trick as [`INIT_TIMEOUT_SENTINEL`] — the inner future is typed
/// to `sacp::Error`, which has nowhere to carry a codeg error kind.
const MCP_SUSPECT_SENTINEL: &str = "__codeg_mcp_suspect__";

/// Mark a `session/new` failure as possibly caused by the MCP servers codeg put
/// on the wire.
///
/// Restricted to custom agents on purpose. A built-in's `supports_mcp` is a
/// repository constant already verified against the real agent, so blaming MCP
/// there would send the user chasing a switch that isn't wrong (and that they
/// cannot flip). A custom agent's is a user declaration about a binary codeg
/// knows nothing about — the case this hint is for. An empty list rules MCP out
/// entirely, since then nothing was forwarded to reject.
fn tag_mcp_suspect(
    err: sacp::Error,
    agent_type: AgentType,
    mcp_servers: &[McpServer],
) -> sacp::Error {
    if mcp_servers.is_empty() || !matches!(agent_type, AgentType::Custom(_)) {
        return err;
    }
    // Also log it. The coded error reaches the desktop webview through the
    // global `acp://event` emit, but a web/remote client that has not attached
    // yet never sees it — the connection is removed from the manager map as
    // soon as this task unwinds, so there is no state left to snapshot. That
    // loss is not specific to this hint (it costs `initialize_timeout`,
    // `spawn_failed` and every protocol error just the same), and repairing it
    // means changing how long terminal connections are retained. Until then the
    // log is the one channel that survives on every transport.
    tracing::warn!(
        "[ACP][{}] session/new failed with {} MCP server(s) attached; if this agent \
         does not accept MCP, turn off \"MCP support\" for it in settings: {}",
        agent_type,
        mcp_servers.len(),
        err
    );
    sacp::util::internal_error(format!("{err}{MCP_SUSPECT_SENTINEL}"))
}

/// RAII guard that removes the `AgentConnection` entry from the manager
/// map when dropped. Runs on both normal task exit AND task panic, so a
/// panic inside `run_connection` can't leak a stale map entry.
///
/// The `Mutex` is async, so we take two paths:
/// - If the lock is immediately available (`try_lock` succeeds), remove
///   the entry synchronously in the current context.
/// - Otherwise, spawn a short-lived cleanup task to acquire the lock
///   and remove the entry asynchronously. The guard must hold owned
///   `Arc<Mutex<_>>` and `String` so the spawned task has `'static`
///   captures.
struct ConnectionCleanupGuard {
    connections: Arc<tokio::sync::Mutex<HashMap<String, AgentConnection>>>,
    connection_id: String,
}

impl Drop for ConnectionCleanupGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.connections.try_lock() {
            guard.remove(&self.connection_id);
            return;
        }
        let connections = self.connections.clone();
        let connection_id = std::mem::take(&mut self.connection_id);
        tokio::spawn(async move {
            connections.lock().await.remove(&connection_id);
        });
    }
}

/// Represents a single active ACP agent connection.
pub struct AgentConnection {
    pub id: String,
    pub agent_type: AgentType,
    pub status: ConnectionStatus,
    pub owner_window_label: String,
    pub cmd_tx: mpsc::Sender<ConnectionCommand>,
    /// 后端权威的会话状态。所有 `emit_with_state` 写入此状态并自增 seq。
    /// 使用 `Arc<RwLock<_>>` 让 spawn 出的连接 task 与外部 snapshot 读取共享。
    pub state: Arc<RwLock<SessionState>>,
    /// 出口侧的事件发射器；管理器层（如 `send_prompt_linked`）需要直接发射
    /// `ConversationLinked` 等带 SessionState 写入的事件。
    pub emitter: EventEmitter,
    /// Serializes prompt sends per connection. Held across the
    /// link-check + DB write + emit + cmd_tx.send sequence so two
    /// concurrent prompts (multiple browser tabs of the same conversation,
    /// chat-channel + UI overlap) can't interleave and produce duplicate
    /// conversation rows or a confused agent that received two prompts
    /// in the same turn.
    pub prompt_lock: Arc<tokio::sync::Mutex<()>>,

    /// Canonical fingerprint of the agent's effective config (env vars + model
    /// provider creds + native config file content) captured at spawn. The
    /// running process is locked to THIS config; comparing it against a freshly
    /// recomputed fingerprint after a settings save tells us whether the session
    /// has drifted onto stale config. Immutable for the connection's lifetime.
    pub config_fingerprint: String,
    /// The most recent fingerprint seen by `refresh_connection_staleness`.
    /// Tracks "did anything change since we last looked" so a second settings
    /// save re-emits `SessionConfigStale` (re-showing a dismissed banner) while a
    /// no-op save (identical values) stays silent. Starts equal to
    /// `config_fingerprint`.
    pub last_observed_fingerprint: String,
    /// OS process id of the spawned agent subprocess, published by the
    /// vendored `sacp-tokio` `on_spawn` callback. `0` until the process has
    /// launched (or if the pid was never observed). Used only as a shutdown
    /// backstop: `disconnect_all` kills this pid's whole process tree
    /// synchronously after the graceful-disconnect grace window, so agents
    /// (and their own child processes, e.g. MCP servers) never leak as orphans
    /// when the host process exits before `ChildGuard::drop` can run on the
    /// connection driver thread.
    ///
    /// Reset to `0` by the paired `on_exit` callback the moment the process is
    /// *reaped* — the only moment its pid stops naming our child and becomes
    /// reassignable. That reset is what keeps the backstop from ever aiming at
    /// a pid the OS has since handed to an unrelated process. Notably it does
    /// NOT fire merely because the connection ended: `ChildGuard::drop` signals
    /// the tree without waiting, so the agent may still be alive and still
    /// needs the backstop.
    pub child_pid: Arc<std::sync::atomic::AtomicU32>,
}

impl AgentConnection {
    pub fn info(&self) -> ConnectionInfo {
        ConnectionInfo {
            id: self.id.clone(),
            agent_type: self.agent_type,
            status: self.status.clone(),
        }
    }
}

/// Build an AcpAgent from registry metadata.
/// Directory handed to codex-acp via `APP_SERVER_LOGS` so its adapter-side
/// (ACP ↔ Codex app-server translation) logs land on disk for support.
///
/// Roots under the same `<cache>/app.codeg` tree as
/// [`binary_cache::cache_dir`] for consistency. Returns `None` — and the
/// caller injects nothing — when the system cache dir is unknown or the
/// directory can't be created: diagnostics must never block a connection.
fn codex_app_server_log_dir() -> Option<String> {
    let dir = dirs::cache_dir()?
        .join("app.codeg")
        .join("acp-logs")
        .join("codex-acp");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.to_string_lossy().into_owned())
}

/// Pi runs through pi-acp, which spawns the actual `pi` binary at runtime. If
/// `pi` (or the BYO-pi `PI_ACP_PI_COMMAND` override) isn't resolvable, pi-acp
/// dies mid-connection with a raw ENOENT. This preflight resolves the effective
/// command up front against the same `PATH` the child inherits and returns a
/// clear message when it can't be found; `None` means launch may proceed.
///
/// The message contains the literal substring "is not installed", which the
/// frontend matches to show the localized SDK-missing prompt with an "Open Agent
/// Settings" action (see `src/contexts/acp-connections-context.tsx`). Do not
/// change that substring.
fn pi_launch_preflight(runtime_env: &BTreeMap<String, String>) -> Option<String> {
    let custom = runtime_env
        .get("PI_ACP_PI_COMMAND")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let command = custom.unwrap_or("pi");
    if crate::commands::acp::resolve_pi_command_path(command).is_some() {
        return None;
    }
    Some(match custom {
        Some(cmd) => format!(
            "Pi is not installed: the custom pi command \"{cmd}\" was not found. \
             Update it in Agent Settings → Pi → Runtime."
        ),
        None => "Pi is not installed. Install it with: \
                 npm install -g @earendil-works/pi-coding-agent \
                 (or set a custom pi command in Agent Settings → Pi → Runtime)."
            .to_string(),
    })
}

/// Transcript directory for an agent that codeg must record itself, or `None`
/// for agents with their own store parser.
///
/// Only custom ACP agents are recorded: every built-in has a dedicated parser
/// reading the agent's native transcript, and recording those too would double
/// the storage while risking two disagreeing histories.
fn transcript_dir_for(agent_type: AgentType) -> Option<&'static str> {
    agent_type
        .custom_id()
        .map(|_| registry::registry_id_for(agent_type))
}

/// Ensure a custom agent's transcript file exists with its header. No-op for
/// built-ins, and idempotent per session (a reconnect keeps the original
/// header, so the session's original cwd/start time survive).
fn record_transcript_header(agent_type: AgentType, session_id: &str, cwd: &str) {
    drop(queue_transcript_header(agent_type, session_id, cwd, None));
}

/// [`record_transcript_header`] for a session that carries an existing
/// conversation forward.
///
/// `continues_from` is set when `session/load` failed and codeg opened a fresh
/// agent session for the same conversation: the earlier turns stay where they
/// are and this header links back to them, so the reader still sees one
/// history. See [`crate::acp_transcript::TranscriptHeader::continues_from`].
async fn record_transcript_header_continuing(
    agent_type: AgentType,
    session_id: &str,
    cwd: &str,
    continues_from: Option<&str>,
) {
    let Some(ack) = queue_transcript_header(agent_type, session_id, cwd, continues_from) else {
        return;
    };
    // Wait for the link to be DURABLE before returning, so it is on disk before
    // the caller emits `SessionStarted`. Same shape and same reason as
    // `record_prompt` below: the writer is a background thread, and readers go
    // to the file.
    //
    // The specific reader here is `acp::continued_session_ids`, which the
    // session-binding guard consults to tell "this conversation continues under
    // a new agent session" from "an unrelated session landed on this row". Emit
    // first and a subscriber can read an empty chain, conclude the sessions are
    // unrelated, and split the conversation in two — and nothing removes that
    // row once the header lands, so the duplicate is permanent.
    //
    // Costs one disk write per continuation, i.e. per restart of a conversation
    // whose agent forgot it. `record_prompt` accepts the same cost per TURN.
    // A stalled writer falls back to the old racy behaviour after the timeout
    // rather than blocking the session: the failure mode there is a duplicate
    // row, never lost history.
    if continues_from.is_some() {
        let _ = tokio::time::timeout(std::time::Duration::from_millis(2000), ack).await;
    }
}

/// Build and enqueue the header write. `None` for agents with their own store
/// (nothing is recorded); otherwise the writer's completion ack.
fn queue_transcript_header(
    agent_type: AgentType,
    session_id: &str,
    cwd: &str,
    continues_from: Option<&str>,
) -> Option<tokio::sync::oneshot::Receiver<()>> {
    let dir = transcript_dir_for(agent_type)?;
    let mut header = crate::acp_transcript::TranscriptHeader::new(
        &agent_type.as_wire(),
        session_id,
        cwd,
        crate::acp_transcript::now_epoch_ms(),
    );
    if let Some(previous) = continues_from.filter(|p| !p.is_empty() && *p != session_id) {
        header = header.continuing(previous);
    }
    Some(crate::acp_transcript::record_header(dir, &header))
}

/// Record an outgoing prompt for a custom agent, and wait (briefly) for it to
/// land. No-op for agents with their own store.
///
/// Bound-waited like [`record_turn_end`], but for a sharper reason. The gate
/// that decides whether a later `session/load` replay may be recorded is
/// `acp_transcript::has_entries`, and it reads the FILE — a queued prompt is
/// invisible to it. Returning before the prompt is durable therefore leaves a
/// window in which a reconnect concludes "this conversation has no transcript",
/// records the agent's replay, and ends up with two copies of the same history.
///
/// The window is small but reachable (the writer can be behind on a slow disk,
/// and a conversation can be torn down between its first prompt and its turn
/// end, which is the other place codeg waits). A prompt happens once per turn,
/// so closing it costs one disk write per turn — nothing the user can perceive,
/// against a failure that is permanent and silent.
async fn record_prompt(agent_type: AgentType, session_id: &str, blocks: &[ContentBlock]) {
    let Some(dir) = transcript_dir_for(agent_type) else {
        return;
    };
    let Ok(payload) = serde_json::to_value(blocks) else {
        return;
    };
    let ack = crate::acp_transcript::record_entry(
        dir,
        session_id,
        crate::acp_transcript::EntryKind::Prompt,
        payload,
    );
    let _ = tokio::time::timeout(std::time::Duration::from_millis(2000), ack).await;
}

/// Record a turn's completion for a custom agent, and wait (briefly) for it to
/// land. No-op for agents with their own store.
///
/// The bounded wait exists because the frontend refetches conversation detail
/// right after `TurnComplete`; without it, a reopened conversation could be
/// read before the final lines were flushed. The bound means a stalled writer
/// delays nothing more than this.
async fn record_turn_end(
    agent_type: AgentType,
    session_id: &str,
    stop_reason: &str,
    started_at_ms: u64,
    model: Option<String>,
) {
    let Some(dir) = transcript_dir_for(agent_type) else {
        return;
    };
    let now = crate::acp_transcript::now_epoch_ms();
    let mut payload = serde_json::json!({
        "stopReason": stop_reason,
        "durationMs": now.saturating_sub(started_at_ms),
    });
    // ACP puts no model on the prompt response, so the session's model selector
    // is the only honest answer at turn end — and it is the same value the
    // composer showed while the turn ran. Recorded per turn rather than once in
    // the header because a mid-conversation model switch must not retroactively
    // relabel the turns that ran before it.
    if let (Some(obj), Some(model)) = (payload.as_object_mut(), model.filter(|m| !m.is_empty())) {
        obj.insert("model".to_string(), serde_json::Value::String(model));
    }
    let ack = crate::acp_transcript::record_entry(
        dir,
        session_id,
        crate::acp_transcript::EntryKind::TurnEnd,
        payload,
    );
    let _ = tokio::time::timeout(std::time::Duration::from_millis(2000), ack).await;
}

/// The model id a session's selectors currently report. Agent-agnostic: the
/// ACP `category: "model"` selector is the one channel every agent that has a
/// model at all publishes it on. `None` when the agent exposes no model
/// selector — most custom agents don't, and a fabricated label would be worse
/// than an empty field.
fn current_model_id_from_opts(opts: &[SessionConfigOptionInfo]) -> Option<String> {
    opts.iter()
        .find(|o| o.category.as_deref() == Some("model"))
        .and_then(|o| {
            // A model selector is always a `select`; any other kind carries no
            // model id to report.
            let SessionConfigKindInfo::Select(sel) = &o.kind else {
                return None;
            };
            Some(sel.current_value.clone())
        })
        .filter(|m| !m.is_empty())
}

/// [`current_model_id_from_opts`] against the authoritative `SessionState`
/// snapshot.
async fn current_session_model_id(state: &Arc<RwLock<SessionState>>) -> Option<String> {
    let opts = state.read().await.config_options.clone()?;
    current_model_id_from_opts(&opts)
}

/// Queue one raw `session/update` for a custom agent, handing back the ack so
/// the caller decides whether landing it matters.
///
/// `None` when nothing was queued: not a custom agent, an update the history
/// projection never reads back (see
/// [`crate::parsers::acp_native::is_recorded_update`], which owns that call so
/// the filter cannot drift from the reader it exists to serve), or an
/// unserializable payload.
fn queue_transcript_update(
    agent_type: AgentType,
    session_id: &str,
    update: &SessionUpdate,
) -> Option<tokio::sync::oneshot::Receiver<()>> {
    let dir = transcript_dir_for(agent_type)?;
    if !crate::parsers::acp_native::is_recorded_update(update) {
        return None;
    }
    let payload = serde_json::to_value(update).ok()?;
    Some(crate::acp_transcript::record_entry(
        dir,
        session_id,
        crate::acp_transcript::EntryKind::Update,
        payload,
    ))
}

/// Record one raw `session/update` for a custom agent, fire and forget.
///
/// The ack is dropped: streamed chunks must never make the live read loop wait.
/// Turn boundaries are the only place the live path bound-waits.
fn record_transcript_update(agent_type: AgentType, session_id: &str, update: &SessionUpdate) {
    drop(queue_transcript_update(agent_type, session_id, update));
}

/// How long one hydrated line may take to land before hydration gives up on
/// recording. Only a wedged filesystem can reach it (a line costs tens of
/// microseconds), so it is not a throughput bound — it is the difference
/// between "the conversation opens with a truncated history and a warning" and
/// "opening the conversation hangs forever".
const HYDRATION_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// [`record_transcript_update`] **with backpressure**, for the `session/load`
/// hydration drain. Returns false once the writer has stopped keeping up, after
/// which the caller must stop recording.
///
/// The live path can afford to drop the ack because a lost chunk costs one
/// chunk. Hydration cannot: the replay it is draining is the ONLY copy of that
/// history, and it arrives as fast as it parses while the writer runs at disk
/// speed. Fire-and-forget there fills the bounded queue and then discards from
/// the MIDDLE of the history — silently, leaving a transcript with holes that
/// the `has_entries` gate will never let a later replay repair.
///
/// Awaiting each ack is async-native backpressure (no worker thread is blocked,
/// and one outstanding line cannot overflow a queue of thousands), and it turns
/// the pathological case from "history with random holes" into "history that
/// stops cleanly at a point" — which is what a prefix-honest reader can work
/// with.
async fn record_hydrated_update(
    agent_type: AgentType,
    session_id: &str,
    update: &SessionUpdate,
) -> bool {
    let Some(ack) = queue_transcript_update(agent_type, session_id, update) else {
        return true;
    };
    match tokio::time::timeout(HYDRATION_ACK_TIMEOUT, ack).await {
        // `Err(RecvError)` means the writer thread is gone; there is nothing
        // left to wait for and nothing more will land either.
        Ok(res) => res.is_ok(),
        Err(_) => {
            tracing::warn!(
                "[ACP] transcript writer stalled while hydrating {session_id}; \
                 stopping recording so the replay lands as a clean prefix"
            );
            false
        }
    }
}

/// Build the `with_debug` callback for a spawned agent.
///
/// stderr does double duty: it is tee'd to `tracing::debug!` (historical
/// behavior, invisible at the default `Info` level) AND pushed into
/// `stderr_tail`, an in-memory ring buffer that survives regardless of the log
/// level. The buffer is what a silent-`EndTurn` diagnosis reads from — see
/// [`crate::acp::stderr_tail`]. stdin/stdout are NEVER buffered: they carry
/// JSON-RPC traffic including prompt text and file contents.
fn agent_debug_callback(
    agent_name: String,
    stderr_tail: Arc<StderrTail>,
    stdio_debug_enabled: bool,
) -> impl Fn(&str, sacp_tokio::LineDirection) + Send + Sync + 'static {
    move |line, dir| {
        let (tag, enabled) = match dir {
            sacp_tokio::LineDirection::Stderr => {
                stderr_tail.push(line);
                ("stderr", true)
            }
            sacp_tokio::LineDirection::Stdout => ("stdout", stdio_debug_enabled),
            sacp_tokio::LineDirection::Stdin => ("stdin", stdio_debug_enabled),
        };
        if !enabled {
            return;
        }
        const MAX: usize = 256;
        if line.len() > MAX {
            let head = line
                .char_indices()
                .take_while(|(i, _)| *i < MAX)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(MAX);
            tracing::debug!(
                "[ACP][{agent_name}][{tag}] {}... <truncated {} bytes>",
                &line[..head],
                line.len() - head
            );
        } else {
            tracing::debug!("[ACP][{agent_name}][{tag}] {line}");
        }
    }
}

async fn build_agent(
    agent_type: AgentType,
    runtime_env: &BTreeMap<String, String>,
    cwd: &Path,
    stderr_tail: &Arc<StderrTail>,
) -> Result<AcpAgent, AcpError> {
    // A conversation can outlive the custom-agent definition it was started
    // with (the user deleted it in settings). `get_agent_meta` cannot report
    // that — it is infallible — so it hands back a placeholder with an empty
    // command. Catch it here, before we try to spawn nothing and surface an
    // opaque ENOENT.
    if let Some(id) = agent_type.custom_id() {
        if !crate::acp::custom_registry::is_registered(id) {
            return Err(AcpError::SdkNotInstalled(format!(
                "The custom agent \"{id}\" is no longer registered. Re-add it in Settings → Agents to use this conversation."
            )));
        }
    }
    let meta = registry::get_agent_meta(agent_type);
    debug_assert_eq!(meta.agent_type, agent_type);

    let agent = match meta.distribution {
        AgentDistribution::Npx { cmd, args, env, .. } => {
            // pi-acp spawns the real `pi` binary; fail fast with a clear,
            // install-prompt-routable error if it (or a BYO-pi override) isn't
            // resolvable, rather than letting pi-acp die mid-connection on a raw
            // ENOENT that surfaces as an opaque protocol error.
            if agent_type == AgentType::Pi {
                if let Some(message) = pi_launch_preflight(runtime_env) {
                    return Err(AcpError::SdkNotInstalled(message));
                }
                // NOTE: codeg deliberately does NOT touch pi's `trust.json` here.
                // It used to mark this workspace trusted on every launch, which
                // made pi load the repo's own `.pi/*` — including `.pi/extensions`,
                // JS/TS modules whose top level executes at pi startup with the
                // user's permissions, before any prompt is sent. Because pi-acp
                // spawns `pi --mode rpc` (no UI), pi's own default is to skip those
                // resources, so the seeding was the sole reason they ran. Trust is
                // now an explicit per-workspace decision surfaced in the UI
                // (`acp_pi_set_project_trust`).
                //
                // Dropping the seeding does NOT retract the grants it already
                // wrote: they persist in pi's user-level store and are inherited
                // by every subdirectory, so a repo cloned into a folder some
                // earlier session trusted is trusted before it is ever opened.
                // Since pi resolves trust at startup and runs extensions
                // immediately, an unconfirmed grant has to stop the launch here —
                // a notice shown once the connection is up comes after the code it
                // warns about has already run.
                if let Some(message) =
                    crate::commands::acp::pi_project_trust_launch_block(cwd, runtime_env)
                {
                    return Err(AcpError::PiProjectTrustRequired(message));
                }
            }
            let mut merged_env = merge_agent_env(env, runtime_env);
            // Resolve the config-derived preset HERE (like Grok's
            // `grok_launch_permission_mode` below) so the policy helper stays a
            // pure function over the env list.
            let codex_initial_mode = if agent_type == AgentType::Codex {
                crate::commands::acp::codex_launch_initial_agent_mode()
            } else {
                None
            };
            apply_codex_env_policy(agent_type, &mut merged_env, codex_initial_mode.as_deref());
            // codex-acp 1.0.0 honors APP_SERVER_LOGS as a directory for its
            // adapter-side logs. Surface it only under CODEG_ACP_DEBUG so
            // default runs are unchanged; a directory-creation failure silently
            // skips injection (diagnostics must never block a connect).
            let want_codex_logs = agent_type == AgentType::Codex
                && std::env::var("CODEG_ACP_DEBUG")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
            if want_codex_logs {
                if let Some(dir) = codex_app_server_log_dir() {
                    merged_env.push(("APP_SERVER_LOGS".to_string(), dir));
                }
            }
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in &merged_env {
                parts.push(format!("{k}={v}"));
            }
            parts.push(
                crate::commands::acp::resolve_npx_command(cmd)
                    .await
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| {
                        crate::process::normalized_program(cmd)
                            .to_string_lossy()
                            .to_string()
                    }),
            );
            // Grok's root-level launch flags go BEFORE its `agent stdio`
            // subcommand (which rejects them):
            //  - `--no-auto-update`: codeg owns the pinned version, so suppress the
            //    CLI's background self-update (it would drift off the pin and can
            //    break the ACP contract). Config twin: `[cli].auto_update = false`.
            //  - `--permission-mode <value>`: grok's real permission enum
            //    (default/acceptEdits/auto/dontAsk/bypassPermissions/plan), read
            //    from the Grok panel's `[ui].permission_mode`. Only passed for a
            //    non-`default` mode; `default`/unset leaves it off so ACP
            //    permission requests still reach codeg's UI. (Grok exposes no ACP
            //    `modes` channel for permission — verified against 0.2.99 — so this
            //    launch flag, not a live `session/set_mode`, is the control point.)
            if agent_type == AgentType::Grok {
                parts.push("--no-auto-update".into());
                if let Some(mode) = crate::commands::acp::grok_launch_permission_mode() {
                    parts.push("--permission-mode".into());
                    parts.push(mode);
                }
            }
            for a in args {
                parts.push((*a).into());
            }
            // Translate OpenClaw-specific env vars to CLI flags
            if agent_type == AgentType::OpenClaw {
                if let Some(url) = runtime_env
                    .get("OPENCLAW_GATEWAY_URL")
                    .filter(|v| !v.is_empty())
                {
                    parts.push("--url".into());
                    parts.push(url.clone());
                }
                if let Some(key) = runtime_env
                    .get("OPENCLAW_SESSION_KEY")
                    .filter(|v| !v.is_empty())
                {
                    parts.push("--session".into());
                    parts.push(key.clone());
                }
                // When creating a new conversation (no session_id to resume),
                // pass --reset-session so OpenClaw mints a fresh transcript
                // instead of appending to the previous one.
                if runtime_env
                    .get("OPENCLAW_RESET_SESSION")
                    .is_some_and(|v| v == "1")
                {
                    parts.push("--reset-session".into());
                }
            }
            let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
            let agent_name = meta.name.to_string();
            let tail = Arc::clone(stderr_tail);
            AcpAgent::from_args(&refs)
                .map(|a| {
                    // `false`: this branch never logged stdin/stdout, and the
                    // shared callback must not quietly widen that. Only the
                    // Binary branch opts into the CODEG_ACP_DEBUG stdio dump.
                    a.with_debug(agent_debug_callback(agent_name, tail, false))
                })
                .map_err(|e| AcpError::SpawnFailed(e.to_string()))
        }
        AgentDistribution::Binary {
            version: registry_version,
            cmd,
            args,
            env,
            platforms,
            ..
        } => {
            let platform = registry::current_platform();
            let _ = platforms
                .iter()
                .find(|p| p.platform == platform)
                .ok_or_else(|| {
                    AcpError::PlatformNotSupported(format!(
                        "{} is not available on {platform}",
                        meta.name
                    ))
                })?;

            // Session-page connect must never trigger a download. Use
            // the best cached version available (tolerates users on
            // older-but-still-working binaries); return SdkNotInstalled
            // only when nothing is cached, so the frontend can prompt
            // the user to install it from the Agent Settings page.
            //
            // With nothing cached, every binary agent falls back to a
            // user-installed CLI on PATH (e.g. `cursor-agent` from the
            // official install script, a brew `opencode`, or the user's
            // own install of a custom tool) before giving up — mirroring
            // the Uvx `system_cmd` fallback.
            //
            // INVARIANT: the substring "is not installed" is matched
            // verbatim by the frontend catch block in
            // `src/contexts/acp-connections-context.tsx` to surface a
            // localized install prompt. Do not change the wording.
            let cached =
                crate::acp::binary_cache::find_best_cached_binary_for_agent(agent_type, cmd)?;
            let binary_path = match cached {
                Some((path, cached_version)) => {
                    if cached_version == registry_version {
                        tracing::info!(
                            "[ACP][{}] Using cached binary {cached_version}",
                            meta.name
                        );
                    } else {
                        tracing::info!(
                            "[ACP][{}] Using cached binary {cached_version} (registry recommends {registry_version})",
                            meta.name
                        );
                    }
                    path
                }
                None => {
                    let system = crate::commands::acp::resolve_system_agent_binary(cmd)
                        .ok_or_else(|| {
                            AcpError::SdkNotInstalled(format!(
                                "{} is not installed. Please install it in Agent Settings.",
                                meta.name
                            ))
                        })?;
                    tracing::info!(
                        "[ACP][{}] No cached binary; using system {} from PATH",
                        meta.name,
                        system.display()
                    );
                    system
                }
            };

            let binary_str = binary_path.to_string_lossy().to_string();
            let binary_size = std::fs::metadata(&binary_path)
                .map(|m| m.len())
                .unwrap_or(0);
            let mut server = McpServerStdio::new(meta.name, &binary_str);
            let mut cmd_args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
            // Cursor's ROOT-level `--model <id>` flag precedes the `acp`
            // subcommand and sets the session's default model. Sourced from
            // the Cursor panel's default-model control (env_json key
            // CURSOR_MODEL — a codeg-side launch knob; the CLI itself reads
            // no model env var).
            if agent_type == AgentType::Cursor {
                if let Some(model) = runtime_env
                    .get("CURSOR_MODEL")
                    .map(|v| v.trim())
                    .filter(|v| !v.is_empty())
                {
                    cmd_args.insert(0, "--model".to_string());
                    cmd_args.insert(1, model.to_string());
                }
                // Root `--force` = Run Everything: the ACP session swaps its
                // permission prompter for an auto-allow one, so tool calls
                // never reach session/request_permission (deny rules still
                // apply, and an org policy can downgrade it to rule-based
                // approval). Sourced from the panel's permission-mode
                // control (env_json key CURSOR_FORCE — codeg-side knob; the
                // CLI reads no such env var).
                if runtime_env
                    .get("CURSOR_FORCE")
                    .map(|v| v.trim())
                    .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                {
                    cmd_args.insert(0, "--force".to_string());
                }
            }
            let cmd_args_for_log = cmd_args.clone();
            if !cmd_args.is_empty() {
                server = server.args(cmd_args);
            }
            let mut merged_env = merge_agent_env(env, runtime_env);
            if agent_type == AgentType::Cursor {
                apply_cursor_env_policy(&mut merged_env, runtime_env);
            } else if agent_type == AgentType::Grok {
                apply_grok_env_policy(&mut merged_env, runtime_env);
            }
            let env_key_list: Vec<&str> = merged_env.iter().map(|(k, _)| k.as_str()).collect();
            if !merged_env.is_empty() {
                let env_vars: Vec<sacp::schema::EnvVariable> = merged_env
                    .iter()
                    .map(|(k, v)| sacp::schema::EnvVariable::new(k, v))
                    .collect();
                server = server.env(env_vars);
            }
            // Spawn-time diagnostic dump: binary identity, args, and env
            // key list (values omitted — they may contain API keys). If
            // the connection hangs later, these lines pin down exactly
            // which binary was invoked and how.
            tracing::info!(
                "[ACP][{}] binary_path={} size={} platform={} args={:?} env_keys={:?}",
                meta.name,
                binary_str,
                binary_size,
                registry::current_platform(),
                cmd_args_for_log,
                env_key_list
            );

            // Stdio logging policy:
            // - stderr is always on: it's the agent's own diagnostic
            //   output (ANSI log lines) and does not contain user data.
            // - stdin / stdout carry JSON-RPC traffic that includes
            //   prompt text, tool-call arguments, file read/write
            //   contents, and permission-response payloads — all of
            //   which may contain API keys pasted by users or file
            //   contents the agent is editing. They are gated behind
            //   the `CODEG_ACP_DEBUG=1` env var so production builds
            //   don't persist user content into OS-level log files
            //   (Console.app on macOS, journald on Linux).
            // - Max line length is kept short so what does get logged
            //   captures the JSON-RPC envelope (method, id) rather
            //   than large payload bodies.
            let stdio_debug_enabled = std::env::var("CODEG_ACP_DEBUG")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            let agent_name = meta.name.to_string();
            let tail = Arc::clone(stderr_tail);
            Ok(
                AcpAgent::new(sacp::schema::McpServer::Stdio(server)).with_debug(
                    agent_debug_callback(agent_name, tail, stdio_debug_enabled),
                ),
            )
        }
        AgentDistribution::Uvx {
            package,
            cmd,
            args,
            env,
            python,
            system_cmd,
            ..
        } => {
            let merged_env = merge_agent_env(env, runtime_env);
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in &merged_env {
                parts.push(format!("{k}={v}"));
            }
            if let Some(uvx_path) = crate::commands::acp::resolve_uvx_command() {
                // Primary: `uvx [--python <ver>] --from <pinned package> <entry
                // script>`. uvx fetches + caches the pinned package on first use;
                // the `--python` pin keeps it on an interpreter the agent
                // supports (see the registry `python` field).
                parts.push(uvx_path.to_string_lossy().to_string());
                parts.extend(crate::commands::acp::uvx_python_args(python));
                parts.push("--from".into());
                parts.push(package.to_string());
                parts.push(cmd.to_string());
                for a in args {
                    parts.push((*a).into());
                }
            } else if let Some((sys_path, sys_args)) = system_cmd.and_then(|(c, a)| {
                crate::commands::acp::resolve_command_on_path(c).map(|path| (path, a))
            }) {
                // Fallback: the agent's own CLI is already on PATH, installed
                // via pipx / `uv tool install` / an official installer rather
                // than provisioned through uvx.
                tracing::warn!(
                    "[ACP][{}] uvx unavailable; falling back to system command {:?}",
                    meta.name, sys_path
                );
                // `system_cmd` is a complete launch recipe for the PATH binary;
                // the uvx entry-script `args` don't necessarily apply to it.
                parts.push(sys_path.to_string_lossy().to_string());
                for a in sys_args {
                    parts.push((*a).into());
                }
            } else {
                // INVARIANT: the substring "is not installed" is matched
                // verbatim by the frontend catch block in
                // `src/contexts/acp-connections-context.tsx` to surface a
                // localized install prompt. Do not change the wording.
                return Err(AcpError::SdkNotInstalled(format!(
                    "{} is not installed. Please install it in Agent Settings.",
                    meta.name
                )));
            }
            let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
            let agent_name = meta.name.to_string();
            let tail = Arc::clone(stderr_tail);
            AcpAgent::from_args(&refs)
                .map(|a| {
                    // `false` for the same reason as the Npx branch above.
                    a.with_debug(agent_debug_callback(agent_name, tail, false))
                })
                .map_err(|e| AcpError::SpawnFailed(e.to_string()))
        }
    }?;

    // Run the agent subprocess in the session's working directory rather than
    // codeg's own process cwd (a desktop app launched from the Dock often
    // inherits "/"). A coding agent belongs in its project root. This is
    // required for Hermes, whose local terminal backend force-exports
    // TERMINAL_CWD = os.getcwd() at import (clobbering any inherited value)
    // and reports that as the agent's "Current working directory" in its
    // system prompt — without pinning it would believe it lives in "/". For
    // agents that already use the ACP session/new cwd this is a harmless
    // alignment (process cwd == session cwd). Guard on an existing directory
    // so a not-yet-created working_dir (e.g. a worktree path) can't make the
    // spawn fail.
    Ok(if cwd.is_dir() {
        agent.with_current_dir(cwd)
    } else {
        agent
    })
}

/// Stack size for the dedicated OS thread that drives each ACP connection's
/// `run_connection` future (see the spawn site below). `run_connection` is one
/// colossal async state machine — the full per-connection message loop plus
/// every registered client-request handler — and in DEBUG builds the compiler
/// does not pack async locals, so a single poll of the connection closure needs
/// far more than a default ~2 MiB Tokio worker-thread stack. Left on the worker
/// pool it overflowed the stack and aborted the whole process under `tauri dev`
/// (release builds pack the frame small enough to fit, which is why only debug
/// crashed). 8 MiB matches the macOS main-thread stack — 4x the default that
/// overflowed, generous headroom for the debug frame as ACP features accrete.
/// The stack is reserved address space, lazily committed, so it costs no
/// physical memory beyond the pages actually touched (~the real 2-3 MiB the
/// frame uses), regardless of this cap — so the larger reservation is free. If a
/// future ACP feature ever grows the loop past even this, split `run_connection`
/// into boxed sub-futures rather than raising it further.
const ACP_CONNECTION_STACK_SIZE: usize = 8 * 1024 * 1024;

/// Spawn an ACP agent process and run the connection loop in a background task.
///
/// On success, the newly created `AgentConnection` is inserted into
/// `connections` before this function returns. The background task
/// automatically removes the entry from `connections` once `run_connection`
/// exits (timeout, error, or clean disconnect), so the manager never
/// leaks stale entries after a connection tears down.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_agent_connection(
    connection_id: String,
    agent_type: AgentType,
    working_dir: Option<String>,
    session_id: Option<String>,
    runtime_env: BTreeMap<String, String>,
    owner_window_label: String,
    emitter: EventEmitter,
    connections: Arc<tokio::sync::Mutex<HashMap<String, AgentConnection>>>,
    preferred_mode_id: Option<String>,
    preferred_config_values: BTreeMap<String, String>,
    delegation_injection: Option<DelegationInjection>,
    terminal_shell_config: TerminalShellRuntimeConfig,
) -> Result<tokio::sync::oneshot::Receiver<()>, AcpError> {
    // Create the authoritative session state up front. Subsequent emit_with_state
    // calls write through this state and increment its seq counter so the first
    // event the frontend sees has seq=1, not the placeholder 0 from Phase 0.
    let mut initial_state = SessionState::new(
        connection_id.clone(),
        agent_type,
        working_dir.clone().map(PathBuf::from),
        owner_window_label.clone(),
        None, // folder_id 由后续 prompt handler 在首次 send 时绑定 (Phase 2)
    );

    // Install the SessionStarted dedup signal BEFORE wrapping into Arc so the
    // first event (StatusChanged{Connecting} below) doesn't race with the
    // installer. The receiver is returned to `spawn_agent`, which holds the
    // per-session dedup lock until this rx fires (or times out / aborts).
    let session_started_rx = initial_state.install_session_started_signal();

    let session_state = Arc::new(RwLock::new(initial_state));

    emit_with_state(
        &session_state,
        &emitter,
        AcpEvent::StatusChanged {
            status: ConnectionStatus::Connecting,
        },
    )
    .await;

    // Align ~/.hermes/.env's base-URL var with config.yaml's model.base_url so
    // Hermes' auxiliary tasks (title generation, compression, …) resolve the
    // same endpoint as the main conversation. Best-effort; never blocks launch.
    if agent_type == AgentType::Hermes {
        crate::commands::acp::reconcile_hermes_runtime_env(&runtime_env);
    }

    // Resolve the launch cwd from the same `working_dir` (via the same helper)
    // that run_connection uses for the session/new request, so the process
    // cwd, the ACP session cwd, and any os.getcwd()-derived agent state all
    // agree. Computed here because `working_dir` is moved into run_connection
    // below.
    let launch_cwd = resolve_working_dir(working_dir.as_deref());
    // Shared cell that receives the agent process's OS pid the instant it
    // spawns (via `on_spawn` below). Stored on the `AgentConnection` so the
    // shutdown path can `kill_tree` the process tree synchronously as a
    // backstop when the connection driver thread is torn down by process exit
    // before `ChildGuard::drop` can run. 0 = not spawned yet / unknown.
    let child_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
    // Connection-scoped ring buffer of the agent's stderr, populated by the
    // `with_debug` callback `build_agent` installs and read at turn end when a
    // turn is diagnosed as silently empty. Created here so both the spawn side
    // and the conversation loop share the same buffer.
    let stderr_tail = Arc::new(StderrTail::new());
    let agent = build_agent(agent_type, &runtime_env, &launch_cwd, &stderr_tail)
        .await?
        .on_spawn({
            let child_pid = Arc::clone(&child_pid);
            move |pid| child_pid.store(pid, std::sync::atomic::Ordering::SeqCst)
        })
        // Paired with `on_spawn`: publish 0 again once the process has been
        // reaped, so the shutdown backstop can never `kill_tree` a pid the OS
        // has already handed to someone else. Fires ONLY on a real reap — a
        // connection that merely ended keeps its pid published, because the
        // vendored `ChildGuard` signals the tree without waiting and the agent
        // may still be running.
        .on_exit({
            let child_pid = Arc::clone(&child_pid);
            move || child_pid.store(0, std::sync::atomic::Ordering::SeqCst)
        });

    // Path policy for the ACP `fs/*` channel. Built HERE rather than inside
    // `run_connection` because it needs the full `runtime_env` (only the git
    // credential keys survive into `terminal_base_env` below), and a per-agent
    // relocation like `GROK_HOME` must move the allowed root along with the
    // agent's state. Uses the same `launch_cwd` the process and ACP session get.
    let fs_policy = FsAccessPolicy::from_env(&launch_cwd, agent_type, &runtime_env);

    // Whether codeg hosts the `fs/*` + `terminal/*` channels at all, or hands
    // them back to the agent so the agent's OWN sandbox covers them (#436).
    // Resolved here for the same reason as `fs_policy`: it reads the full
    // per-agent `runtime_env`, which does not survive into `run_connection`.
    let host_tools = HostToolsPolicy::from_env(&runtime_env);

    // Forward only the codeg git credential helper keys into the terminal
    // runtime — not the agent's API tokens or model provider credentials.
    // This makes `git fetch`/`git push` issued through the ACP
    // `terminal/create` tool authenticate via the same helper path the
    // agent process uses, while keeping unrelated secrets scoped to the
    // agent and out of arbitrary shell commands it runs.
    let mut terminal_base_env: BTreeMap<String, String> = runtime_env
        .iter()
        .filter(|(k, _)| k.starts_with("GIT_CONFIG_"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    // Also surface a codeg-installed OfficeCLI on the terminal's PATH: agents run
    // office skills' `officecli …` through this `terminal/create` tool, not as a
    // child of the agent process, so the agent-env injection alone wouldn't reach
    // them right after install (before install.ps1's User-PATH change lands).
    prepend_officecli_path(&mut terminal_base_env);

    let (cmd_tx, cmd_rx) = mpsc::channel::<ConnectionCommand>(32);
    let conn_id = connection_id.clone();
    let emitter_clone = emitter.clone();
    let cleanup_connections = connections.clone();
    let cleanup_connection_id = connection_id.clone();
    let state_clone = Arc::clone(&session_state);

    // Canonical config fingerprint of what this process is launching with.
    // Derived from the same `runtime_env` we hand the agent (minus per-launch
    // volatile keys) plus the agent's native config file content, so a later
    // settings save can be compared against it to detect a stale running session.
    let config_fingerprint =
        crate::commands::acp::fingerprint_config(agent_type, &runtime_env);

    // Insert the entry BEFORE spawning the background task so that a
    // fast-failing `run_connection` can never remove it before it was
    // inserted (would otherwise leak the entry).
    connections.lock().await.insert(
        connection_id.clone(),
        AgentConnection {
            id: connection_id,
            agent_type,
            status: ConnectionStatus::Connecting,
            owner_window_label,
            cmd_tx,
            state: Arc::clone(&session_state),
            emitter: emitter.clone(),
            prompt_lock: Arc::new(tokio::sync::Mutex::new(())),
            last_observed_fingerprint: config_fingerprint.clone(),
            config_fingerprint,
            child_pid,
        },
    );

    // Drive `run_connection` on a dedicated, large-stack thread (see
    // ACP_CONNECTION_STACK_SIZE) rather than a Tokio worker task: its debug
    // poll frame is too big for a default ~2 MiB worker stack and was aborting
    // the process under `tauri dev`. `Handle::block_on` runs it on the SAME
    // shared runtime, so `tokio::spawn`/timers/IO inside `run_connection` still
    // use the pool — only the giant top-level frame moves to the roomy stack.
    // The connection is fire-and-forget (torn down from within via `cmd_rx` /
    // process exit; no JoinHandle is awaited), so a thread is behaviorally
    // equivalent to the previous task.
    let connection_rt = tokio::runtime::Handle::current();
    // RAII guard built OUTSIDE the thread body and moved in: on a normal exit
    // or panic unwind its Drop removes the manager map entry, AND if the thread
    // fails to spawn the dropped closure runs the same Drop — so the entry is
    // never leaked.
    let cleanup_guard = ConnectionCleanupGuard {
        connections: cleanup_connections,
        connection_id: cleanup_connection_id,
    };
    let connection_thread = std::thread::Builder::new()
        .name(format!("acp-conn-{conn_id}"))
        .stack_size(ACP_CONNECTION_STACK_SIZE)
        .spawn(move || {
            let _cleanup = cleanup_guard;
            connection_rt.block_on(async move {
        let delegation_for_cleanup = delegation_injection.clone();
        let result = run_connection(
            agent,
            conn_id.clone(),
            agent_type,
            working_dir,
            session_id,
            cmd_rx,
            emitter_clone.clone(),
            Arc::clone(&state_clone),
            terminal_base_env,
            terminal_shell_config,
            preferred_mode_id,
            preferred_config_values,
            delegation_injection,
            fs_policy,
            host_tools,
            stderr_tail,
        )
        .await;

        // Revoke the per-launch token + cascade cancel any still-pending
        // delegations AND questions owned by this parent connection. All are
        // best-effort: a missing token entry is a no-op, and both
        // `cancel_by_parent` calls are safe on an empty pending map.
        if let Some(inj) = delegation_for_cleanup {
            let token = {
                let snap = state_clone.read().await;
                snap.delegation_token.clone()
            };
            if let Some(tok) = token {
                inj.tokens.revoke(&tok).await;
            }
            inj.broker.cancel_by_parent(&conn_id).await;
            // Reclaim a parked `ask_user_question` instead of waiting for the
            // companion's ask socket to close (which a reparented/hard-killed
            // agent may never do); the dropped sender declines the tool cleanly.
            inj.questions.cancel_questions_by_parent(&conn_id).await;
            // Likewise reclaim a parked Grok `exit_plan_mode` approval; the
            // dropped sender replies disconnect so grok keeps plan mode active.
            inj.plan_approvals
                .cancel_plan_approvals_by_parent(&conn_id)
                .await;
        }

        if let Err(e) = result {
            let code = e.code().map(String::from);
            emit_with_state(
                &state_clone,
                &emitter_clone,
                AcpEvent::Error {
                    message: e.to_string(),
                    agent_type: agent_type.to_string(),
                    code,
                    details: None,
                    // The only genuinely terminal emit site: `run_connection`
                    // is unwinding and the next event is `Disconnected`.
                    // The lifecycle worker uses this flag to decide whether
                    // to flip the conversation row to Cancelled and to
                    // buffer the detail for the broker's cancel reason.
                    terminal: true,
                },
            )
            .await;
            // Drive the state machine through `Error` before `Disconnected`
            // so the frontend's error-handling effect (cancelled-on-error)
            // engages — without this hop the connection would jump straight
            // to Disconnected and look like a clean shutdown.
            emit_with_state(
                &state_clone,
                &emitter_clone,
                AcpEvent::StatusChanged {
                    status: ConnectionStatus::Error,
                },
            )
            .await;
        }

        emit_with_state(
            &state_clone,
            &emitter_clone,
            AcpEvent::StatusChanged {
                status: ConnectionStatus::Disconnected,
            },
        )
        .await;
                // Connection loop ended; `block_on` returns and `_cleanup`
                // (bound at the top of the thread body) drops next, removing
                // the manager map entry — same as on a panic unwind.
            });
        });
    if let Err(e) = connection_thread {
        // Thread creation only fails on OS resource exhaustion. Dropping the
        // un-spawned closure already ran `cleanup_guard`'s Drop (removing the
        // map entry), so just surface the failure and let the caller abort.
        tracing::error!("[ACP] failed to spawn connection driver thread: {e}");
        return Err(AcpError::SpawnFailed(format!(
            "connection driver thread: {e}"
        )));
    }

    Ok(session_started_rx)
}

/// A pending permission-card responder. `Acp` is a real ACP
/// `session/request_permission`; `CodexElicitation` is a codex approval-style
/// `elicitation/create` (MCP tool-call approval / message-only confirm) routed
/// through the SAME permission card so approvals look exactly like they did
/// before codeg advertised `elicitation.form` — its chosen option answers the
/// blocked elicitation request instead (see `handle_elicitation_request`).
enum PendingPermission {
    Acp(Responder<RequestPermissionResponse>),
    CodexElicitation {
        responder: Responder<serde_json::Value>,
        approval: crate::acp::question::ElicitationApproval,
    },
}

/// What [`PermissionQueue`] needs from a parked responder. Abstracted into a
/// trait ONLY so the queue stays a pure state machine that unit tests can drive:
/// sacp's `Responder` has private fields and no public constructor, so a real
/// [`PendingPermission`] cannot be built outside a live ACP connection.
trait PermissionResponder {
    /// Resolve with the user's chosen option id.
    fn respond_selected(self, option_id: String);
    /// Resolve as cancelled — the turn ended / connection tore down before the
    /// user chose.
    fn respond_cancelled(self);
}

impl PermissionResponder for PendingPermission {
    fn respond_selected(self, option_id: String) {
        match self {
            PendingPermission::Acp(responder) => {
                let outcome =
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id));
                let _ = responder.respond(RequestPermissionResponse::new(outcome));
            }
            PendingPermission::CodexElicitation {
                responder,
                approval,
            } => {
                let response = crate::acp::question::build_elicitation_approval_response(
                    &approval, &option_id,
                );
                let _ = responder.respond(serde_json::to_value(response).unwrap_or_default());
            }
        }
    }

    fn respond_cancelled(self) {
        match self {
            PendingPermission::Acp(responder) => {
                let _ = responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ));
            }
            PendingPermission::CodexElicitation { responder, .. } => {
                let _ = responder.respond(
                    serde_json::to_value(crate::acp::question::elicitation_cancel_response())
                        .unwrap_or_default(),
                );
            }
        }
    }
}

/// A permission card that is waiting for its turn on screen. Holds exactly the
/// three fields `AcpEvent::PermissionRequest` carries, so promoting a queued
/// entry is a move, not a rebuild.
struct QueuedPermission {
    request_id: String,
    tool_call: serde_json::Value,
    options: Vec<PermissionOptionInfo>,
}

/// What [`PermissionQueue::resolve`] decided.
struct ResolvedPermission {
    /// `false` when `request_id` was not ours (already answered, drained, or a
    /// stale client) — the caller must then emit nothing at all.
    answered: bool,
    /// The card promoted onto the screen by this answer, if any.
    next: Option<QueuedPermission>,
}

/// Per-connection permission state: the blocked responders AND the display
/// queue, under ONE lock.
///
/// Why they must share a lock (#442): the responder map used to be the only
/// state, and every `session/request_permission` emitted its card immediately.
/// But a card is a SINGLE slot both in the snapshot
/// (`SessionState.pending_permission`) and in the frontend reducer
/// (`conn.pendingPermission`), so N concurrent requests collapsed to the last
/// one — the earlier `request_id`s were no longer referenced by any client, so
/// no `RespondPermission` could ever arrive for them and their responders sat
/// parked until teardown, blocking the agent's tool calls forever (a work task
/// then sat at `awaiting_input` because its outstanding-request set kept the
/// orphan keys). Codex issues these concurrently: codex-acp forwards each
/// `item/commandExecution/requestApproval` to its own `session/request_permission`
/// and serializes only its notification queue, not approvals.
///
/// So: queue instead of overwrite — one card at a time, FIFO, nothing lost.
/// Splitting the responder map from the queue would leave a real interleaving
/// (the request handler runs on sacp's dispatch task, `Cancel` on the
/// conversation task, so they interleave at every `.await`):
///
/// ```text
/// handler: insert responder for P
/// cancel : drain responders; clear queue      <- P's responder is gone
/// handler: enqueue P; emit PermissionRequest(P)   <- clickable but inert card,
///                                                   and `showing = P` wedges
///                                                   every later request
/// ```
///
/// Re-checking "is P still in the responder map" before emitting does NOT close
/// that window (a drain can still land between the check and the publish). One
/// lock covering both, with the emit performed INSIDE it (see
/// [`admit_permission`]), makes "responder exists" and "card is on screen"
/// atomic with respect to a drain.
///
/// Invariant, upheld by all three methods: every id in `showing`/`waiting` has a
/// live entry in `responders`, and every `responders` key is either `showing` or
/// in `waiting`. That makes "queued card whose responder is gone"
/// unrepresentable, so promotion never has to skip dead entries.
///
/// LOCK ORDER: this mutex is always acquired BEFORE `SessionState`'s `RwLock`,
/// never after. `emit_with_state` takes the state lock internally, so publishing
/// while holding this mutex is the sanctioned direction; nothing may acquire
/// this mutex while already holding a `SessionState` guard.
struct PermissionQueue<R = PendingPermission> {
    responders: HashMap<String, R>,
    /// The card currently published to clients. `None` = nothing on screen.
    showing: Option<String>,
    waiting: VecDeque<QueuedPermission>,
}

// Hand-written rather than derived: `#[derive(Default)]` would demand
// `R: Default`, which a responder never is.
impl<R> Default for PermissionQueue<R> {
    fn default() -> Self {
        Self {
            responders: HashMap::new(),
            showing: None,
            waiting: VecDeque::new(),
        }
    }
}

impl<R: PermissionResponder> PermissionQueue<R> {
    /// Register a blocked responder and its card. Returns the card to publish
    /// NOW — the newcomer itself when the screen was free, otherwise `None`
    /// (it waits its turn).
    fn admit(&mut self, responder: R, card: QueuedPermission) -> Option<QueuedPermission> {
        self.responders.insert(card.request_id.clone(), responder);
        if self.showing.is_none() {
            self.showing = Some(card.request_id.clone());
            Some(card)
        } else {
            self.waiting.push_back(card);
            None
        }
    }

    /// Answer `request_id` and advance the screen.
    ///
    /// Unknown / already-answered ids are an idempotent no-op (`answered:
    /// false`) — two clients racing the same card must not double-respond to a
    /// responder that has already been consumed.
    fn resolve(&mut self, request_id: &str, option_id: String) -> ResolvedPermission {
        let Some(pending) = self.responders.remove(request_id) else {
            return ResolvedPermission {
                answered: false,
                next: None,
            };
        };
        pending.respond_selected(option_id);
        if self.showing.as_deref() == Some(request_id) {
            let next = self.waiting.pop_front();
            self.showing = next.as_ref().map(|c| c.request_id.clone());
            ResolvedPermission {
                answered: true,
                next,
            }
        } else {
            // Defensive: a stale client answered a card that never reached the
            // screen. Drop its queue entry too, or promoting it later would
            // surface a card with no responder — the state the invariant above
            // exists to forbid.
            self.waiting.retain(|c| c.request_id != request_id);
            ResolvedPermission {
                answered: true,
                next: None,
            }
        }
    }

    /// Cancel every blocked responder and clear the queue. Returns the id of the
    /// card that was on screen, which the caller MUST follow with a compensating
    /// `PermissionResolved` — otherwise that card stays up on every client with
    /// no live responder behind it, and a work task keeps its outstanding-request
    /// key forever (the pre-existing idle-`Cancel` ghost, #442).
    ///
    /// Queued cards need no compensation: they were never published, so no
    /// client rendered them and `track_request` never counted them.
    fn drain(&mut self) -> Option<String> {
        for (_, pending) in self.responders.drain() {
            pending.respond_cancelled();
        }
        self.waiting.clear();
        self.showing.take()
    }

    /// How many cards are waiting BEHIND the one on screen.
    fn waiting_len(&self) -> usize {
        self.waiting.len()
    }
}

/// Shared per-connection permission state (responders + display queue).
type PendingPermissions = Arc<tokio::sync::Mutex<PermissionQueue>>;

/// `conn=<id> conv=<id>` for the permission log lines.
///
/// Whose permission a log line is about is not answerable from the request id
/// alone, and the answer matters most exactly when it is a delegation
/// sub-agent's — that connection has no tab, so a stall there is the one an
/// operator is least able to see (#447). With the connection id here and
/// `child_connection_id` on the `delegation_task` span, an external watchdog can
/// join the two. `conv` is `-` until the row is bound.
///
/// Takes the `SessionState` read lock, so callers must hold the permission queue
/// mutex — never the reverse (see [`PermissionQueue`]'s LOCK ORDER note).
async fn permission_log_scope(state: &Arc<RwLock<SessionState>>) -> String {
    let s = state.read().await;
    match s.conversation_id {
        Some(cid) => format!("conn={} conv={cid}", s.connection_id),
        None => format!("conn={} conv=-", s.connection_id),
    }
}

/// Register a blocked permission responder and publish its card if the screen is
/// free. The emit happens INSIDE the queue lock — see [`PermissionQueue`] for why
/// that is load-bearing rather than incidental.
async fn admit_permission(
    perms: &PendingPermissions,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    responder: PendingPermission,
    card: QueuedPermission,
) {
    let mut queue = perms.lock().await;
    let scope = permission_log_scope(state).await;
    let request_id = card.request_id.clone();
    match queue.admit(responder, card) {
        Some(card) => {
            tracing::info!(
                "[ACP] permission {} shown {scope} (waiting={})",
                card.request_id,
                queue.waiting_len()
            );
            // `queued` is 0 by construction here: a card is only published when
            // the screen was free, which means nothing was waiting.
            emit_with_state(
                state,
                emitter,
                AcpEvent::PermissionRequest {
                    request_id: card.request_id,
                    tool_call: card.tool_call,
                    options: card.options,
                    queued: 0,
                },
            )
            .await;
        }
        None => {
            let depth = queue.waiting_len();
            tracing::info!(
                "[ACP] permission {request_id} queued {scope} behind {:?} (waiting={depth})",
                queue.showing,
            );
            // The visible card did not change, so nothing republishes it — send
            // a depth-only update or its "N more waiting" hint goes stale.
            emit_with_state(
                state,
                emitter,
                AcpEvent::PermissionQueueDepth {
                    depth: depth as u32,
                },
            )
            .await;
        }
    }
}

/// Answer a permission card and promote the next one.
///
/// Publish order is deliberate: `PermissionRequest(next)` goes out BEFORE
/// `PermissionResolved(request_id)`. Both clears are id-checked
/// (`SessionState::apply_event`, and the frontend's `PERMISSION_CLEARED`), so the
/// trailing resolve is a no-op against the newly-shown card rather than wiping
/// it. It also keeps a work task's outstanding-request set from transiently
/// emptying, which would otherwise flip the row `awaiting_input → running →
/// awaiting_input` on every single approval.
async fn resolve_permission(
    perms: &PendingPermissions,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    request_id: String,
    option_id: String,
) {
    let mut queue = perms.lock().await;
    let resolved = queue.resolve(&request_id, option_id);
    if !resolved.answered {
        return;
    }
    // Pairs with the `shown` line: a watchdog reading only the raise side can't
    // tell a permission the user answered in seconds from one that has been
    // blocking the agent since. Resolved AFTER the idempotent no-op check, so a
    // stale/duplicate answer costs no state read and logs nothing.
    let scope = permission_log_scope(state).await;
    tracing::info!("[ACP] permission {request_id} answered {scope}");
    if let Some(card) = resolved.next {
        let depth = queue.waiting_len();
        tracing::info!(
            "[ACP] permission {} promoted {scope} after {request_id} (waiting={depth})",
            card.request_id,
        );
        emit_with_state(
            state,
            emitter,
            AcpEvent::PermissionRequest {
                request_id: card.request_id,
                tool_call: card.tool_call,
                options: card.options,
                // Unlike a fresh admit this can be non-zero: promotion happens
                // with the rest of the queue still behind it.
                queued: depth as u32,
            },
        )
        .await;
    }
    emit_with_state(state, emitter, AcpEvent::PermissionResolved { request_id }).await;
}

/// Cancel every pending permission on this connection and clear the on-screen
/// card via a compensating `PermissionResolved`.
///
/// Mirrors `ConnectionManager::compensate_if_question_drained`, which solves the
/// same "broadcast card outlives its backend waiter" problem for questions.
async fn drain_permissions(
    perms: &PendingPermissions,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
) {
    let mut queue = perms.lock().await;
    drain_permissions_locked(&mut queue, state, emitter).await;
}

/// Drain, then publish `follow_up`, WITHOUT releasing the queue lock in between.
///
/// The two must be atomic whenever `follow_up` is `TurnComplete`, because
/// `SessionState::apply_event` nulls `pending_permission` on that event
/// unconditionally. With a plain drain followed by a separate emit, an
/// `admit_permission` landing in the gap (the request handler runs on sacp's
/// dispatch task, this on the conversation task) would publish its card and set
/// `showing`, and then `TurnComplete` would silently un-display it on every
/// client — leaving a live responder behind an id nobody can answer, and wedging
/// every LATER permission behind it for the life of the connection. That is the
/// exact failure mode this queue exists to prevent.
///
/// Holding the lock across both makes an in-flight admit wait until the turn has
/// ended; its card then lands on an empty queue and displays normally as the
/// out-of-turn request it now is.
async fn drain_permissions_then_emit(
    perms: &PendingPermissions,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    follow_up: AcpEvent,
) {
    let mut queue = perms.lock().await;
    drain_permissions_locked(&mut queue, state, emitter).await;
    emit_with_state(state, emitter, follow_up).await;
}

/// Shared body of the two drains above. Emits the compensating
/// `PermissionResolved` for whatever was on screen; queued cards were never
/// published, so they need none.
async fn drain_permissions_locked(
    queue: &mut PermissionQueue,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
) {
    if let Some(request_id) = queue.drain() {
        tracing::info!("[ACP] permission {request_id} cancelled by drain");
        emit_with_state(state, emitter, AcpEvent::PermissionResolved { request_id }).await;
    }
}

fn map_session_modes(mode_state: &SessionModeState) -> SessionModeStateInfo {
    SessionModeStateInfo {
        current_mode_id: mode_state.current_mode_id.to_string(),
        available_modes: mode_state
            .available_modes
            .iter()
            .map(|mode| SessionModeInfo {
                id: mode.id.to_string(),
                name: mode.name.clone(),
                description: mode.description.clone(),
            })
            .collect(),
    }
}

async fn emit_session_modes(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    modes: &Option<SessionModeState>,
) {
    if let Some(mode_state) = modes {
        emit_with_state(
            state,
            emitter,
            AcpEvent::SessionModes {
                modes: map_session_modes(mode_state),
            },
        )
        .await;
    }
}

fn map_session_config_category(category: &SessionConfigOptionCategory) -> String {
    match category {
        SessionConfigOptionCategory::Mode => "mode".to_string(),
        SessionConfigOptionCategory::Model => "model".to_string(),
        SessionConfigOptionCategory::ThoughtLevel => "thought_level".to_string(),
        SessionConfigOptionCategory::Other(value) => value.clone(),
        _ => "unknown".to_string(),
    }
}

fn map_session_config_select_option(
    option: &SessionConfigSelectOption,
) -> SessionConfigSelectOptionInfo {
    SessionConfigSelectOptionInfo {
        value: option.value.to_string(),
        name: option.name.clone(),
        description: option.description.clone(),
    }
}

fn map_session_config_select_group(
    group: &SessionConfigSelectGroup,
) -> SessionConfigSelectGroupInfo {
    SessionConfigSelectGroupInfo {
        group: group.group.to_string(),
        name: group.name.clone(),
        options: group
            .options
            .iter()
            .map(map_session_config_select_option)
            .collect(),
    }
}

fn map_session_config_option(option: &SessionConfigOption) -> Option<SessionConfigOptionInfo> {
    match &option.kind {
        SessionConfigKind::Select(select) => {
            let (flat_options, groups) = match &select.options {
                SessionConfigSelectOptions::Ungrouped(options) => (
                    options
                        .iter()
                        .map(map_session_config_select_option)
                        .collect::<Vec<_>>(),
                    Vec::new(),
                ),
                SessionConfigSelectOptions::Grouped(grouped) => (
                    grouped
                        .iter()
                        .flat_map(|group| {
                            group.options.iter().map(map_session_config_select_option)
                        })
                        .collect::<Vec<_>>(),
                    grouped
                        .iter()
                        .map(map_session_config_select_group)
                        .collect::<Vec<_>>(),
                ),
                _ => (Vec::new(), Vec::new()),
            };

            Some(SessionConfigOptionInfo {
                id: option.id.to_string(),
                name: option.name.clone(),
                description: option.description.clone(),
                category: option.category.as_ref().map(map_session_config_category),
                kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                    current_value: select.current_value.to_string(),
                    options: flat_options,
                    groups,
                }),
            })
        }
        SessionConfigKind::Boolean(toggle) => Some(SessionConfigOptionInfo {
            id: option.id.to_string(),
            name: option.name.clone(),
            description: option.description.clone(),
            category: option.category.as_ref().map(map_session_config_category),
            kind: SessionConfigKindInfo::Boolean(SessionConfigBooleanInfo {
                current_value: toggle.current_value,
            }),
        }),
        _ => None,
    }
}

/// The `type` discriminators codeg's schema build can decode. Anything else on
/// the wire is stripped by [`strip_unknown_config_options`] before the typed
/// parse, because `SessionConfigOption::kind` is a required flattened field:
/// one unrecognized option would otherwise fail the WHOLE response and take the
/// agent down with it (exactly what cline 3.0.50's `type: "boolean"` did before
/// `unstable_boolean_config` was enabled).
const KNOWN_CONFIG_OPTION_KINDS: &[&str] = &["select", "boolean"];

/// Drop `configOptions[]` entries whose `type` this build cannot decode, in
/// place, on a raw session response.
///
/// ACP's `SessionConfigKind` is a `#[serde(tag = "type")]` enum with no
/// catch-all variant, so an option kind added upstream after codeg's schema pin
/// is a hard deserialization failure rather than an ignorable unknown. Stripping
/// unknown kinds here downgrades "this agent is completely unusable" to "this
/// one selector is missing", which is the correct failure mode for a selector.
///
/// Entries missing a `type`, or shaped unexpectedly, are left untouched — serde
/// gives a better error for those than a silent drop would.
fn strip_unknown_config_options(raw: &mut serde_json::Value, method: &str) {
    let Some(options) = raw
        .get_mut("configOptions")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    options.retain(|option| {
        let Some(kind) = option.get("type").and_then(serde_json::Value::as_str) else {
            return true;
        };
        if KNOWN_CONFIG_OPTION_KINDS.contains(&kind) {
            return true;
        }
        tracing::warn!(
            "[ACP] {method}: dropping config option '{}' — unsupported kind '{kind}'; \
             codeg's ACP schema knows {:?}. The selector will not be shown.",
            option
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<no id>"),
            KNOWN_CONFIG_OPTION_KINDS,
        );
        false
    });
}

fn map_session_config_options(
    config_options: &[SessionConfigOption],
) -> Vec<SessionConfigOptionInfo> {
    config_options
        .iter()
        .filter_map(map_session_config_option)
        .collect()
}

/// Defensive fallback for Codex's approval-preset selector.
///
/// codex-acp 1.0.0 advertises its modes through *both* standard ACP
/// `SessionModes` and an `id = "mode"` config option (see `AgentMode.ts`'s
/// `toSessionModeState()` + `toConfigOption()`), so this synthesizer is
/// normally a no-op — the early return fires because the agent already
/// surfaced "mode". We keep it only as a safety net: if a future build ever
/// omits the "mode" config option (older 0.16.0 did this when the sandbox
/// policy didn't match a preset, e.g. after `writable_roots` injection), the
/// user would otherwise lose the preset picker entirely, because the composer
/// hides the standard mode selector whenever any config option exists. Codex's
/// `set_config_option` handler accepts `config_id = "mode"` regardless of
/// whether it was advertised.
///
/// The preset ids/names/descriptions below MUST match the live adapter
/// vocabulary (`read-only` / `agent` / `agent-full-access`, default `agent`);
/// the legacy 0.16.0 ids (`auto` / `full-access`) are no longer accepted.
fn ensure_codex_mode_option(options: &mut Vec<SessionConfigOptionInfo>) {
    if options.iter().any(|o| o.id == "mode") {
        return;
    }
    options.insert(
        0,
        SessionConfigOptionInfo {
            id: "mode".to_string(),
            name: "Approval Preset".to_string(),
            description: Some(
                "Choose an approval and sandboxing preset for your session".to_string(),
            ),
            category: Some("mode".to_string()),
            kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                current_value: "agent".to_string(),
                options: vec![
                    SessionConfigSelectOptionInfo {
                        value: "read-only".to_string(),
                        name: "Read-only".to_string(),
                        description: Some(
                            "Requires approval to edit files and run commands.".to_string(),
                        ),
                    },
                    SessionConfigSelectOptionInfo {
                        value: "agent".to_string(),
                        name: "Agent".to_string(),
                        description: Some("Read and edit files, and run commands.".to_string()),
                    },
                    SessionConfigSelectOptionInfo {
                        value: "agent-full-access".to_string(),
                        name: "Agent (full access)".to_string(),
                        description: Some(
                            "Codex can edit files outside this workspace and run commands with \
                             network access."
                                .to_string(),
                        ),
                    },
                ],
                groups: vec![],
            }),
        },
    );
}

async fn emit_session_config_options_values(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    agent_type: AgentType,
    config_options: Vec<SessionConfigOption>,
) {
    let mut mapped = map_session_config_options(&config_options);
    if agent_type == AgentType::Codex {
        ensure_codex_mode_option(&mut mapped);
    }
    emit_with_state(
        state,
        emitter,
        AcpEvent::SessionConfigOptions {
            config_options: mapped,
        },
    )
    .await;
}

async fn emit_selectors_ready(state: &Arc<RwLock<SessionState>>, emitter: &EventEmitter) {
    emit_with_state(state, emitter, AcpEvent::SelectorsReady).await;
}

/// Synthesized config-option id for Grok's model picker (drives the composer's
/// grouped model selector via the frontend's `isModelConfigOption`).
const GROK_MODEL_OPTION_ID: &str = "model";

/// Synthesized config-option id for Grok's per-session reasoning-effort selector.
/// Grok ships effort choices in `x.ai/sessionConfig` under `category:"mode"`
/// (ids `low`/`medium`/`high`), and applies a live override via the
/// `session/set_model` request's `_meta.reasoningEffort` — so effort is a live
/// composer control, not just a global config.toml default.
const GROK_EFFORT_OPTION_ID: &str = "reasoning_effort";

/// Stable `AcpEvent::Error` code the frontend localizes when a Grok model switch
/// is rejected because the conversation is already bound to a different agent
/// type (see `is_grok_incompatible_agent_switch`). Recoverable, not terminal.
const GROK_INCOMPATIBLE_AGENT_ERROR_CODE: &str = "grok_model_switch_incompatible_agent";

/// Grok partitions its models by `agentType` (e.g. `grok-4.5` → `grok-build-plan`,
/// `grok-composer-2.5-fast` → `cursor`). A session may switch models freely until
/// its first turn, after which it is locked to the agent type it started with;
/// a later cross-agent-type `session/set_model` is then rejected with a stable
/// `data.code` of `MODEL_SWITCH_INCOMPATIBLE_AGENT` (`suggestion: start_new_session`).
/// Grok's own `x.ai/sessionConfig` still lists every model regardless of type, so
/// the composer offers them all and we detect this specific rejection to handle
/// it gracefully rather than leaking a raw JSON-RPC error.
fn is_grok_incompatible_agent_switch(e: &sacp::Error) -> bool {
    e.data
        .as_ref()
        .and_then(|d| d.get("code"))
        .and_then(|c| c.as_str())
        == Some("MODEL_SWITCH_INCOMPATIBLE_AGENT")
}

/// Canonical, composer-facing label for a Grok reasoning-effort tier id. Aligns
/// the composer with the settings panel's `grok.effort*` wording
/// (Low/Medium/High/Max); unknown ids fall back to the id itself. Grok's own
/// richer per-tier text (e.g. "Highest implementation quality…") is kept as the
/// option *description*, not the name.
fn grok_effort_label(id: &str) -> &str {
    match id {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Max",
        other => other,
    }
}

/// Canonical composer-facing *description* (sub-text) for a Grok reasoning-effort
/// tier. Grok ships its own per-tier `description` only for the models switchable
/// `reasoningEfforts`; the model default that lives OUTSIDE that list — grok-4.5's
/// `xhigh`/Max — carries none, so the front-injected option would otherwise be the
/// only tier with no sub-text. This supplies a fitting one (and doubles as a
/// fallback if grok ever omits a switchable tier's description). Unknown ids get
/// `None`. Grok's own, more specific text always takes precedence over this.
fn grok_effort_description(id: &str) -> Option<&'static str> {
    match id {
        "low" => Some("Quick, fast responses"),
        "medium" => Some("Balanced speed and quality"),
        "high" => Some("Extensive reasoning for high quality"),
        "xhigh" => Some("Maximum reasoning for the most complex tasks"),
        _ => None,
    }
}

/// Parse Grok's raw top-level `models` (from a session-establishment response)
/// into a per-`modelId` spec map: reasoning-effort capability plus the model's
/// own context window. Absent `models` / `availableModels` → empty map (caller
/// falls back to the flat `x.ai/sessionConfig` effort list). Missing `_meta`
/// fields degrade gracefully (`supports=false` / `default=None` / `options=[]` /
/// `context_window=None`).
fn parse_grok_model_specs(models: Option<&serde_json::Value>) -> HashMap<String, GrokModelSpec> {
    let mut out = HashMap::new();
    let Some(list) = models
        .and_then(|m| m.get("availableModels"))
        .and_then(|v| v.as_array())
    else {
        return out;
    };
    for m in list {
        let Some(model_id) = m.get("modelId").and_then(|v| v.as_str()) else {
            continue;
        };
        let meta = m.get("_meta");
        let supports = meta
            .and_then(|x| x.get("supportsReasoningEffort"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let default = meta
            .and_then(|x| x.get("reasoningEffort"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let mut options = Vec::new();
        if let Some(efforts) = meta
            .and_then(|x| x.get("reasoningEfforts"))
            .and_then(|v| v.as_array())
        {
            for e in efforts {
                let Some(id) = e.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let label = e
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or(id)
                    .to_string();
                let description = e
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                options.push((id.to_string(), label, description));
            }
        }
        out.insert(
            model_id.to_string(),
            GrokModelSpec {
                options,
                default,
                supports,
                context_window: meta
                    .and_then(|x| x.get("totalContextTokens"))
                    .and_then(|v| v.as_u64())
                    .filter(|window| *window > 0),
            },
        );
    }
    out
}

/// Build the reasoning-effort selector for `model_id` from the per-model spec
/// map, or `None` if the model is absent from the map or does not support
/// effort. Options are the model's switchable `reasoningEfforts` (relabeled via
/// [`grok_effort_label`], keeping grok's own copy as the description); the model
/// default is injected at the FRONT when it isn't already listed, so a default
/// that lives OUTSIDE the switchable set — grok-4.5's `xhigh` — stays selectable
/// and the current value is always representable. `current_value` = the model
/// default (or the first option).
fn build_grok_effort_option(
    model_id: &str,
    specs: &HashMap<String, GrokModelSpec>,
) -> Option<SessionConfigOptionInfo> {
    let spec = specs.get(model_id)?;
    if !spec.supports {
        return None;
    }
    let mut options: Vec<SessionConfigSelectOptionInfo> = spec
        .options
        .iter()
        .map(|(id, _grok_label, desc)| SessionConfigSelectOptionInfo {
            value: id.clone(),
            name: grok_effort_label(id).to_string(),
            // Grok's own per-tier text wins; canonical fallback fills any gap.
            description: desc
                .clone()
                .or_else(|| grok_effort_description(id).map(str::to_string)),
        })
        .collect();
    if let Some(def) = &spec.default {
        if !options.iter().any(|o| &o.value == def) {
            options.insert(
                0,
                SessionConfigSelectOptionInfo {
                    value: def.clone(),
                    name: grok_effort_label(def).to_string(),
                    // The injected default (grok-4.5's `xhigh`) is absent from grok's
                    // switchable list, so it has no grok description — supply ours.
                    description: grok_effort_description(def).map(str::to_string),
                },
            );
        }
    }
    if options.is_empty() {
        return None;
    }
    let current_value = spec
        .default
        .clone()
        .unwrap_or_else(|| options[0].value.clone());
    Some(SessionConfigOptionInfo {
        id: GROK_EFFORT_OPTION_ID.to_string(),
        name: "Reasoning effort".to_string(),
        description: None,
        category: Some("mode".to_string()),
        kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
            current_value,
            options,
            groups: Vec::new(),
        }),
    })
}

/// Re-point the effort selector in `opts` at `model_id`: drop any existing
/// effort selector, then append a freshly-built one iff the model supports
/// effort. The model selector is untouched and effort stays LAST (matching
/// `synthesize_grok_config_options`' ordering). Used on a mid-session model
/// switch, where grok never re-sends per-model effort data.
fn set_grok_effort_selector_for_model(
    opts: &mut Vec<SessionConfigOptionInfo>,
    model_id: &str,
    specs: &HashMap<String, GrokModelSpec>,
) {
    opts.retain(|o| o.id != GROK_EFFORT_OPTION_ID);
    if let Some(effort) = build_grok_effort_option(model_id, specs) {
        opts.push(effort);
    }
}

/// Grok does not emit the standard ACP `config_options` / `modes` channels that
/// codeg's generic composer-selector pipeline reads (which is why the composer
/// showed no selectors for Grok). Instead it ships its selectors in a
/// non-standard `_meta["x.ai/sessionConfig"].options` list — a flat array of
/// `{id, category, label, description?, selected}` covering both model choices
/// (`category:"model"`) and reasoning-effort choices (`category:"mode"`). Fold
/// that list into the same `SessionConfigOptionInfo` shape every other agent's
/// selectors flow through, so Grok reaches selector parity with zero new
/// frontend code. Returns `None` when there is no usable sessionConfig.
fn synthesize_grok_config_options(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
    specs: &HashMap<String, GrokModelSpec>,
) -> Option<Vec<SessionConfigOptionInfo>> {
    let options = meta?
        .get("x.ai/sessionConfig")?
        .get("options")?
        .as_array()?;

    let mut model_opts: Vec<SessionConfigSelectOptionInfo> = Vec::new();
    let mut model_current: Option<String> = None;
    let mut effort_opts: Vec<SessionConfigSelectOptionInfo> = Vec::new();
    let mut effort_current: Option<String> = None;

    for opt in options {
        let Some(id) = opt.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        // Grok ships two composer selectors here: the MODEL list
        // (`category:"model"`) and the reasoning-EFFORT list (`category:"mode"`,
        // ids low/medium/high). Both are live over ACP — model via
        // `session/set_model`, effort via that request's `_meta.reasoningEffort`
        // (see `set_grok_model` / `set_grok_config_option`). Effort options only
        // appear when the current model advertises `supportsReasoningEffort`, so
        // the selector self-gates. Anything else is ignored.
        let (opts_vec, current) = match opt.get("category").and_then(|v| v.as_str()) {
            Some("model") => (&mut model_opts, &mut model_current),
            Some("mode") => (&mut effort_opts, &mut effort_current),
            _ => continue,
        };
        let label = opt.get("label").and_then(|v| v.as_str()).unwrap_or(id);
        if opt
            .get("selected")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            *current = Some(id.to_string());
        }
        opts_vec.push(SessionConfigSelectOptionInfo {
            value: id.to_string(),
            name: label.to_string(),
            description: opt
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }

    let mut result: Vec<SessionConfigOptionInfo> = Vec::new();
    // Current model id (the `selected` one, else the first) — needed both for
    // the model selector's `current_value` and to pick the per-model effort spec.
    let current_model = model_current
        .clone()
        .or_else(|| model_opts.first().map(|o| o.value.clone()));
    if !model_opts.is_empty() {
        let current = current_model
            .clone()
            .unwrap_or_else(|| model_opts[0].value.clone());
        result.push(SessionConfigOptionInfo {
            id: GROK_MODEL_OPTION_ID.to_string(),
            name: "Model".to_string(),
            description: None,
            category: Some("model".to_string()),
            kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                current_value: current,
                options: model_opts,
                groups: Vec::new(),
            }),
        });
    }
    // Effort selector. With per-model `specs` (parsed from the response's
    // top-level `models`), it follows the CURRENT model's advertised capability
    // — present/absent, its option set, and an `xhigh`-style out-of-list default
    // (see `build_grok_effort_option`). Without specs (no `models` in the
    // response) fall back to today's flat `x.ai/sessionConfig` "mode" list so
    // nothing regresses.
    if !specs.is_empty() {
        if let Some(effort) = current_model
            .as_deref()
            .and_then(|m| build_grok_effort_option(m, specs))
        {
            result.push(effort);
        }
    } else if !effort_opts.is_empty() {
        let current = effort_current.unwrap_or_else(|| effort_opts[0].value.clone());
        result.push(SessionConfigOptionInfo {
            id: GROK_EFFORT_OPTION_ID.to_string(),
            name: "Reasoning effort".to_string(),
            description: None,
            category: Some("mode".to_string()),
            kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                current_value: current,
                options: effort_opts,
                groups: Vec::new(),
            }),
        });
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Emit an already-mapped `SessionConfigOptionInfo` list (used by the Grok path,
/// which synthesizes `Info` directly rather than mapping sacp `SessionConfigOption`s).
async fn emit_session_config_options_info(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    config_options: Vec<SessionConfigOptionInfo>,
) {
    emit_with_state(
        state,
        emitter,
        AcpEvent::SessionConfigOptions { config_options },
    )
    .await;
}

/// Switch Grok's active model — and, optionally, its reasoning effort — via the
/// standard ACP `session/set_model`. Sent as an `UntypedMessage` for the same
/// reason as `session/resume` / `session/set_config_option`: sacp 11.0.0's typed
/// request is gated behind the `unstable_session_model` feature (not enabled),
/// and the orphan rule blocks a local `JsonRpcRequest` impl.
///
/// Reasoning effort IS live-settable (verified against grok 0.2.99): a
/// `reasoning_effort` value carried in the request's `_meta.reasoningEffort`
/// (string `low`/`medium`/`high`) is applied on top of the model — grok logs
/// `applying reasoning_effort override from meta` and emits a `model_changed`
/// session notification echoing the effort. Passing `None` leaves the current
/// effort untouched (e.g. a pure model switch). The `~/.grok/config.toml`
/// `default_reasoning_effort` remains the at-birth global default this overrides.
async fn set_grok_model(
    cx: &ConnectionTo<Agent>,
    session_id: &SessionId,
    model_id: String,
    reasoning_effort: Option<String>,
) -> Result<(), sacp::Error> {
    let params = build_grok_set_model_params(
        session_id.0.as_ref(),
        &model_id,
        reasoning_effort.as_deref(),
    );
    let untyped_req = UntypedMessage::new("session/set_model", params).map_err(|e| {
        sacp::util::internal_error(format!("Failed to build set_model request: {e}"))
    })?;
    cx.send_request_to(Agent, untyped_req).block_task().await?;
    Ok(())
}

/// Build the `session/set_model` params. A reasoning-effort override rides in
/// `_meta.reasoningEffort` (the exact key grok's sampling layer reads — verified
/// against 0.2.99); `None` omits `_meta` for a pure model switch.
fn build_grok_set_model_params(
    session_id: &str,
    model_id: &str,
    reasoning_effort: Option<&str>,
) -> serde_json::Value {
    let mut params = serde_json::json!({
        "sessionId": session_id,
        "modelId": model_id,
    });
    if let Some(effort) = reasoning_effort {
        params["_meta"] = serde_json::json!({ "reasoningEffort": effort });
    }
    params
}

/// Send `_session/steering` (the ACP steering extension) to inject a message
/// into the RUNNING turn. Untyped like `session/resume` — an extension method
/// the schema has no typed request for. Always opts into the 0.64.0
/// `promptRequired` idle contract; codeg only enables native steering for
/// adapters proven to honor it AND to keep the owning prompt in flight across
/// the steered work (claude-agent-acp 0.65.0 / #958 — see
/// [`synthesize_native_steering`] and `registry::steering_prompt_required_min_version`),
/// but the caller still handles every outcome in case the proof was wrong.
async fn send_steer_request(
    cx: &ConnectionTo<Agent>,
    session_id: &SessionId,
    text: &str,
) -> Result<SteerOutcome, AcpError> {
    let params = build_steer_params(session_id.0.as_ref(), text);
    let untyped_req = UntypedMessage::new("_session/steering", params).map_err(|e| {
        AcpError::protocol(format!("Failed to build steering request: {e}"))
    })?;
    let raw = cx
        .send_request_to(Agent, untyped_req)
        .block_task()
        .await
        .map_err(|e| AcpError::protocol(format!("Steering request failed: {e}")))?;
    parse_steer_outcome(&raw)
}

/// Build the `_session/steering` params. The prompt is a single text block
/// (codeg steering is text-only), and `_meta.steering.idleBehavior =
/// "promptRequired"` opts into the turn-end-race contract: a turn that
/// settled first yields `{outcome:"promptRequired"}` WITHOUT consuming the
/// content, so the host resubmits it through a normal `session/prompt`.
fn build_steer_params(session_id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "sessionId": session_id,
        "prompt": [{ "type": "text", "text": text }],
        "_meta": { "steering": { "idleBehavior": "promptRequired" } },
    })
}

/// Parse a `_session/steering` response's top-level `outcome`. Strict on
/// unknowns: a missing or unrecognized outcome is a protocol error, NOT a
/// silent success — the caller must know whether the content was consumed
/// before it decides between "record delivered" and "safe to resend".
fn parse_steer_outcome(raw: &serde_json::Value) -> Result<SteerOutcome, AcpError> {
    match raw.get("outcome").and_then(serde_json::Value::as_str) {
        Some("injected") => Ok(SteerOutcome::Injected),
        Some("promptRequired") => Ok(SteerOutcome::PromptRequired),
        Some("startedNewTurn") => Ok(SteerOutcome::StartedNewTurn),
        other => Err(AcpError::protocol(format!(
            "unexpected _session/steering outcome: {other:?}"
        ))),
    }
}

/// On reconnect, re-apply the user's last-picked Grok model AND reasoning effort
/// (both saved per agent by the frontend and shipped back as preferred config
/// values), reflecting each in its selector's `current_value`. Model is applied
/// first (a pure switch, effort untouched); effort is then re-applied on top of
/// the now-current model via `set_model`'s `_meta.reasoningEffort`.
async fn apply_grok_preferred_options(
    cx: &ConnectionTo<Agent>,
    session_id: &SessionId,
    opts: &mut Vec<SessionConfigOptionInfo>,
    preferred_config_values: &BTreeMap<String, String>,
    specs: &HashMap<String, GrokModelSpec>,
) {
    // Model preference — a pure `set_model` (no effort override). On success we
    // also re-point the effort selector at the newly-preferred model (grok ships
    // per-model effort only at birth, never on set_model).
    if let Some(pref) = preferred_config_values.get(GROK_MODEL_OPTION_ID).cloned() {
        // Split the eligibility read (immutable) from the rebuild (mutable) so we
        // never hold a `&mut opts` borrow across `set_grok_effort_selector_for_model`.
        let eligible = opts
            .iter()
            .find(|o| o.id == GROK_MODEL_OPTION_ID)
            .is_some_and(|o| {
                // Grok's selectors are synthesized as selects; any other kind
                // has no model value to compare against.
                let SessionConfigKindInfo::Select(sel) = &o.kind else {
                    return false;
                };
                // Skip if already current, or the saved model is no longer offered.
                sel.current_value != pref && sel.options.iter().any(|x| x.value == pref)
            });
        if eligible {
            match set_grok_model(cx, session_id, pref.clone(), None).await {
                Ok(()) => {
                    if let Some(SessionConfigKindInfo::Select(sel)) = opts
                        .iter_mut()
                        .find(|o| o.id == GROK_MODEL_OPTION_ID)
                        .map(|o| &mut o.kind)
                    {
                        sel.current_value = pref.clone();
                    }
                    if !specs.is_empty() {
                        set_grok_effort_selector_for_model(opts, &pref, specs);
                    }
                }
                Err(e) => tracing::error!(
                    "[ACP] failed to apply preferred grok model '{pref}' on connect: {e}"
                ),
            }
        }
    }
    // Effort preference — re-applied on top of the (possibly just-switched)
    // current model. The effort selector was rebuilt above for that model, so an
    // unsupported model (no selector) or an unoffered value is skipped here.
    if let Some(pref) = preferred_config_values.get(GROK_EFFORT_OPTION_ID) {
        let model_id = current_grok_model_id_from_opts(opts);
        if let Some(SessionConfigKindInfo::Select(sel)) = opts
            .iter_mut()
            .find(|o| o.id == GROK_EFFORT_OPTION_ID)
            .map(|o| &mut o.kind)
        {
            if &sel.current_value != pref && sel.options.iter().any(|o| &o.value == pref) {
                if let Some(model_id) = model_id {
                    match set_grok_model(cx, session_id, model_id, Some(pref.clone())).await {
                        Ok(()) => sel.current_value = pref.clone(),
                        Err(e) => tracing::error!(
                            "[ACP] failed to apply preferred grok effort '{pref}' on connect: {e}"
                        ),
                    }
                }
            }
        }
    }
}

/// The Grok model selector's current value, read from an in-memory options list.
fn current_grok_model_id_from_opts(opts: &[SessionConfigOptionInfo]) -> Option<String> {
    opts.iter()
        .find(|o| o.id == GROK_MODEL_OPTION_ID)
        .and_then(|o| {
            let SessionConfigKindInfo::Select(sel) = &o.kind else {
                return None;
            };
            Some(sel.current_value.clone())
        })
}

/// The Grok model selector's current value, read from the authoritative
/// `SessionState.config_options` snapshot — needed to carry a reasoning-effort
/// override on `session/set_model` (effort is applied relative to a model).
async fn current_grok_model_id(state: &Arc<RwLock<SessionState>>) -> Option<String> {
    let opts = state.read().await.config_options.clone()?;
    current_grok_model_id_from_opts(&opts)
}

/// Context window for the session's CURRENT Grok model. One state read, done at
/// turn start — the answer only changes on a model switch, which cannot happen
/// mid-turn.
///
/// Resolution order mirrors `parsers::grok::build_detail` so the live ring and
/// the re-parsed history never disagree about the denominator: what Grok
/// reported for the model at session establishment, then its on-disk catalog
/// (which is where a BYO endpoint's declared window lives — that one is often
/// absent from the wire), then the id-shaped heuristic.
///
/// `None` is a real outcome, not just "no model selector": a BYO endpoint is
/// keyed by whatever id the user typed (`[model.<id>]` holds the upstream
/// model's name), so an id like `deepseek-chat` can miss the wire, the catalog,
/// and every heuristic family at once. [`grok_window_change_usage`] is what
/// keeps that from stranding a previous model's window on screen.
async fn grok_current_model_context_window(state: &Arc<RwLock<SessionState>>) -> Option<u64> {
    let (model, reported) = {
        let st = state.read().await;
        let model = current_grok_model_id_from_opts(st.config_options.as_deref()?)?;
        let reported = st
            .grok_model_specs
            .as_ref()
            .and_then(|specs| specs.get(&model))
            .and_then(|spec| spec.context_window);
        (model, reported)
    };
    // Lock released: the catalog lookup below touches the filesystem.
    reported.or_else(|| {
        grok_offline_context_window(&model, &crate::parsers::grok::resolve_grok_home_dir())
    })
}

/// The window for `model` when Grok didn't report one on the wire: its on-disk
/// catalog first, then the id-shaped heuristic. Split out of
/// [`grok_current_model_context_window`] so the resolution order is testable
/// against a fixture home instead of the host's real `~/.grok`.
fn grok_offline_context_window(model: &str, grok_home: &Path) -> Option<u64> {
    crate::parsers::grok::grok_catalog_context_window(grok_home, model)
        .or_else(|| crate::parsers::infer_context_window_max_tokens(Some(model)))
}

/// The live pair to re-emit at turn start because the DENOMINATOR moved — the
/// user switched model between turns — or `None` when nothing needs re-keying.
///
/// Two cases, and the second is why this exists at all:
///   * the new model has a window → carry the cumulative count forward onto it,
///     so the ring doesn't keep dividing by the previous model's window until
///     this turn's first token-bearing update;
///   * the new model has NO resolvable window (a BYO id that the wire, the
///     catalog and the heuristic all decline to size) → hand the denominator
///     back with `size: 0`, the frontend's own "unknown window" sentinel
///     (`rawLiveSize > 0 ? … : null`), which drops the ring to the re-parsed
///     session stats. This is what stops the previous model's window from
///     surviving indefinitely on a turn that never reports a token count.
///
/// Note `size: 0` and not `used: 0` — the frontend's `USAGE_UPDATE` reducer
/// *drops* a zero-`used` update whenever it already holds a positive one (a rule
/// that exists for Claude Code's synthetic `/context` responses), so clearing
/// via zeros would strand every attached client on the stale value while the
/// backend snapshot moved on.
fn grok_window_change_usage(window: Option<u64>, last: Option<(u64, u64)>) -> Option<(u64, u64)> {
    let (used, last_size) = last?;
    let size = window.unwrap_or(0);
    (size != last_size).then_some((used, size))
}

/// The `(used, size)` a Grok update moves the context ring to, or `None` when
/// there is nothing new to report.
///
/// Grok emits no ACP `usage_update` at all; it reports a CUMULATIVE token count
/// for the session in the OUTER `params._meta.totalTokens` of ordinary
/// `session/update` notifications (the same number
/// `parsers::grok::GrokTurnMeta` folds into each turn's `usage.input_tokens`).
/// That count IS the context "used", so pairing it with the model's window fills
/// the composer ring live — previously it only moved once a turn had been
/// written to disk and re-parsed.
///
/// `last` suppresses the repeats: the count rides nearly every chunk, but steps
/// only a handful of times per turn. It keys on the WINDOW too, so a model
/// switch still re-emits while the count sits still.
///
/// An unresolvable window reports `size: 0` rather than going silent — the
/// frontend reads that as "unknown window" and sizes the ring from the re-parsed
/// session stats, while the live count still tracks. Going silent instead would
/// freeze whatever pair was last emitted (see [`grok_window_change_usage`]).
fn grok_live_usage_step(
    dispatch: &Dispatch,
    agent_type: AgentType,
    context_window: Option<u64>,
    last: Option<(u64, u64)>,
) -> Option<(u64, u64)> {
    if agent_type != AgentType::Grok {
        return None;
    }
    let size = context_window.unwrap_or(0);
    let Dispatch::Notification(notification) = dispatch else {
        return None;
    };
    let used = notification
        .params
        .get("_meta")?
        .get("totalTokens")?
        .as_u64()
        .filter(|used| *used > 0)?;
    (Some((used, size)) != last).then_some((used, size))
}

/// Route a composer config-option change for Grok. Both live selectors go
/// through `session/set_model`: the model selector switches the model, and the
/// reasoning-effort selector re-sends the current model with an
/// `_meta.reasoningEffort` override (the `~/.grok/config.toml`
/// `default_reasoning_effort` stays the at-birth global default). Re-emits the
/// options with the new `current_value` so the backend snapshot stays authoritative.
///
/// A cross-agent-type switch rejected on an established conversation
/// (`is_grok_incompatible_agent_switch`) is handled in-band: re-emit the
/// authoritative options to revert the composer's optimistic pick and surface a
/// friendly, recoverable `AcpEvent::Error` (localized by the frontend via
/// `GROK_INCOMPATIBLE_AGENT_ERROR_CODE`), returning `Ok` so the caller does not
/// also emit the raw JSON-RPC error. The saved model preference is left intact,
/// so the suggested "start a new session" actually lands on the picked model
/// (a fresh session applies the preference pre-turn, where the switch succeeds).
async fn set_grok_config_option(
    cx: &ConnectionTo<Agent>,
    session_id: &SessionId,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    config_id: String,
    value_id: String,
) -> Result<(), sacp::Error> {
    // Resolve the `set_model` args for whichever selector changed. A model pick
    // is the model itself (no effort override); an effort pick re-sends the
    // current model carrying the new `_meta.reasoningEffort`. Any other id is a
    // no-op (defensive — the composer only offers these two).
    let (model_id, effort) = if config_id == GROK_MODEL_OPTION_ID {
        (value_id.clone(), None)
    } else if config_id == GROK_EFFORT_OPTION_ID {
        match current_grok_model_id(state).await {
            Some(model_id) => (model_id, Some(value_id.clone())),
            // No model known yet — nothing to carry the effort override on.
            None => return Ok(()),
        }
    } else {
        return Ok(());
    };
    match set_grok_model(cx, session_id, model_id, effort).await {
        Ok(()) => {
            let (current, specs) = {
                let g = state.read().await;
                (g.config_options.clone(), g.grok_model_specs.clone())
            };
            if let Some(mut opts) = current {
                if let Some(SessionConfigKindInfo::Select(sel)) = opts
                    .iter_mut()
                    .find(|o| o.id == config_id)
                    .map(|o| &mut o.kind)
                {
                    sel.current_value = value_id.clone();
                }
                // A MODEL switch must re-point the effort selector at the new
                // model — grok never re-sends per-model effort data on
                // set_model. An EFFORT change leaves the list shape alone; no
                // specs ⇒ leave as-is (flat-fallback session).
                if config_id == GROK_MODEL_OPTION_ID {
                    if let Some(specs) = &specs {
                        set_grok_effort_selector_for_model(&mut opts, &value_id, specs);
                    }
                }
                emit_session_config_options_info(state, emitter, opts).await;
            }
            Ok(())
        }
        Err(e) if is_grok_incompatible_agent_switch(&e) => {
            emit_grok_incompatible_agent_switch(state, emitter).await;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Recover from a Grok cross-agent-type model-switch rejection: revert the
/// composer's optimistic selection by re-emitting the authoritative (unchanged)
/// options, then surface a friendly, recoverable error the frontend localizes
/// via `GROK_INCOMPATIBLE_AGENT_ERROR_CODE`. Split out of `set_grok_config_option`
/// so it can be unit-tested without a live ACP connection.
async fn emit_grok_incompatible_agent_switch(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
) {
    // Clone the options out of the read guard into a local BEFORE emitting: the
    // `emit_*` helpers re-acquire this same state's WRITE lock, and an `if let`
    // scrutinee keeps its temporary (the read guard) alive across the whole body
    // in Rust 2021 — so reading inline would deadlock. `current_value` is
    // unchanged because the switch never took effect.
    let current = state.read().await.config_options.clone();
    if let Some(opts) = current {
        emit_session_config_options_info(state, emitter, opts).await;
    }
    emit_with_state(
        state,
        emitter,
        AcpEvent::Error {
            message: "Cannot switch to that model in an existing conversation. \
                      Start a new session to use it."
                .to_string(),
            agent_type: AgentType::Grok.to_string(),
            code: Some(GROK_INCOMPATIBLE_AGENT_ERROR_CODE.to_string()),
            details: None,
            // Recoverable: the conversation continues on its current model.
            terminal: false,
        },
    )
    .await;
}

/// Emit the composer's session config-option selectors. For Grok this reads the
/// synthesized `x.ai/sessionConfig` (parity path); for every other agent it runs
/// the standard preference-application + sacp-mapping pipeline unchanged.
#[allow(clippy::too_many_arguments)]
async fn apply_and_emit_session_config_options(
    cx: &ConnectionTo<Agent>,
    session: &mut sacp::ActiveSession<'_, Agent>,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    agent_type: AgentType,
    grok_meta: Option<&serde_json::Map<String, serde_json::Value>>,
    grok_model_specs: Option<&HashMap<String, GrokModelSpec>>,
    preferred_mode_id: Option<&str>,
    preferred_config_values: &BTreeMap<String, String>,
    initial_config_options: Vec<SessionConfigOption>,
) {
    if agent_type == AgentType::Grok {
        let specs = grok_model_specs.cloned().unwrap_or_default();
        if let Some(mut opts) = synthesize_grok_config_options(grok_meta, &specs) {
            // Cache the per-model effort map so a later model switch can rebuild
            // the effort selector for the target model (grok ships it only at
            // session birth). `None` when empty keeps the switch path on the
            // flat-fallback branch.
            state.write().await.grok_model_specs = (!specs.is_empty()).then(|| specs.clone());
            let session_id = session.session_id().clone();
            apply_grok_preferred_options(
                cx,
                &session_id,
                &mut opts,
                preferred_config_values,
                &specs,
            )
            .await;
            emit_session_config_options_info(state, emitter, opts).await;
            return;
        }
        // No x.ai/sessionConfig (unexpected): fall through to the standard path,
        // which for Grok emits an empty list (no selectors) — same as before.
    }
    let updated = apply_preferred_session_options(
        cx,
        session,
        state,
        emitter,
        preferred_mode_id,
        preferred_config_values,
        initial_config_options,
    )
    .await;
    emit_session_config_options_values(state, emitter, agent_type, updated).await;
}

/// Grok's initialize still advertises `image: false` — the coding model
/// cannot see pixels. Native ACP `image` blocks nevertheless run its
/// image-describe sidecar. An image-mime `resource` blob does not: grok dumps
/// it as `<file_contents type="binary">` and the model only gets a path.
/// Advertise `image: true` so the composer sends Image blocks.
///
/// Measured live against 0.2.112, 1.0.0 and 1.0.3: every one of them still
/// advertises `image: false`, accepts a native `image` block anyway, and answers
/// correctly about the pixels — while the same bytes as a resource blob make the
/// model invent an answer. The advertisement has simply been wrong for as long
/// as codeg has supported grok, across every version tested, so this is
/// deliberately NOT version-gated (and stays correct if grok ever starts
/// advertising the truth: `image` is already true then). Deliberately no pinned
/// version named here either — `registry.rs` moves on its own schedule, and a
/// claim about "the pinned version" rots at the next bump.
///
/// This bit says only that grok takes image blocks AT ALL — it decodes just some
/// formats, which is a per-mime question a single capability cannot answer, so
/// [`normalize_grok_image_blocks`] settles that separately at dispatch.
fn effective_prompt_capabilities(
    agent_type: AgentType,
    capabilities: &sacp::schema::PromptCapabilities,
) -> PromptCapabilitiesInfo {
    PromptCapabilitiesInfo {
        image: capabilities.image || agent_type == AgentType::Grok,
        audio: capabilities.audio,
        embedded_context: capabilities.embedded_context,
    }
}

async fn emit_prompt_capabilities(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    capabilities: &sacp::schema::PromptCapabilities,
    agent_type: AgentType,
) {
    emit_with_state(
        state,
        emitter,
        AcpEvent::PromptCapabilities {
            prompt_capabilities: effective_prompt_capabilities(agent_type, capabilities),
        },
    )
    .await;
}

fn resolve_working_dir(working_dir: Option<&str>) -> PathBuf {
    match working_dir {
        Some(dir) => {
            let path = PathBuf::from(dir);
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            }
        }
        None => std::env::current_dir()
            .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))),
    }
}

fn claude_raw_sdk_session_meta(
    agent_type: AgentType,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    if agent_type != AgentType::ClaudeCode {
        return None;
    }

    let mut claude_code = serde_json::Map::new();
    claude_code.insert(
        "emitRawSDKMessages".to_string(),
        serde_json::Value::Bool(true),
    );

    let mut meta = serde_json::Map::new();
    meta.insert(
        "claudeCode".to_string(),
        serde_json::Value::Object(claude_code),
    );
    Some(meta)
}

/// The client capabilities codeg advertises on Initialize, with per-agent
/// gates. Extracted for testability — each gate is a documented product
/// decision:
///
/// - Everyone, unless `host_tools` says otherwise: filesystem read/write +
///   terminal, for ACP tool execution. Under
///   [`HostToolsPolicy::Agent`] BOTH are withheld, which is not a narrowing of
///   what the agent may do — it moves the doing back inside the agent's own
///   process, where the agent's own OS sandbox and permission rules already
///   apply (#436). Withholding one without the other buys nothing: the agent
///   just reaches the same file through the channel it kept.
/// - Codex only: form elicitation, so codex's native Plan-mode
///   `request_user_input` is delivered as `elicitation/create` (handled by
///   `handle_elicitation_request`) instead of being silently answered `{}`.
///   NOTE this reroutes codex's WHOLE form-elicitation surface — MCP
///   tool-call approvals and MCP-server forms included — so the handler must
///   cover every shape (`classify_elicitation`). URL elicitation is
///   deliberately NOT advertised: codex-acp then falls back to
///   `session/request_permission`, which codeg already handles. Scoped to
///   Codex to keep the blast radius off other agents (e.g. Claude's native
///   AskUserQuestion, which would otherwise un-gate and duplicate the
///   codeg-mcp ask tool).
/// - Claude Code only: `_meta["subagent-transcript"] = true` — opt into
///   claude-agent-acp ≥0.63's subagent transcript forwarding (#881, SDK
///   `forwardSubagentText`). Subagent text/thought chunks then stream with
///   update-level `_meta.claudeCode.parentToolUseId` instead of being
///   filtered; codeg routes them into the live Agent capsule (see
///   `claude_chunk_parent_tool_use_id`). The adapter checks strictly
///   `=== true`, and a pre-0.63 binary ignores the unknown key, so this is
///   inert everywhere it isn't understood.
fn build_client_capabilities(
    agent_type: AgentType,
    host_tools: HostToolsPolicy,
) -> ClientCapabilities {
    let mut client_capabilities = ClientCapabilities::new();
    if host_tools.hosts_channels() {
        client_capabilities = client_capabilities.terminal(true).fs(
            FileSystemCapabilities::new()
                .read_text_file(true)
                .write_text_file(true),
        );
    }
    // Form elicitation is advertised only to agents that are KNOWN to send
    // spec-conformant `elicitation/create` forms `classify_elicitation` can
    // bridge: codex-acp (Plan-mode `request_user_input`, MCP forms/approvals)
    // and deepseek-acp (its `ask_user_question` + plan-review both build
    // standard oneOf/anyOf forms, and decode a free-text answer that is not in
    // the option set as a custom answer — the card's "Other" input round-trips
    // cleanly). Agents without the bit fall back to their own
    // `request_permission` button path.
    if matches!(agent_type, AgentType::Codex | AgentType::DeepSeek) {
        client_capabilities = client_capabilities
            .elicitation(ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()));
    }
    // Client `_meta` extensions, per agent. `jetbrains.air` opts codeg into
    // JetBrains AIR typed session-failure records (claude-agent-acp 0.67+,
    // codex-acp 1.2+) — both adapters gate publication on EXACTLY this
    // advertisement (integer version >= 1 and "sessionFailure" in the
    // capabilities array). Advertising REPLACES codex's legacy failure
    // surfaces for the connection (`_meta.codex.error` → TurnRetrying,
    // warning/config-warning text chunks), so this ships together with the
    // `SessionFailure` consumer (`air_session_failure` + the frontend
    // banner). Only the two known AIR speakers get it: the capability-gate
    // convention is to advertise nothing an agent hasn't implemented.
    let mut meta = serde_json::Map::new();
    if agent_type == AgentType::ClaudeCode {
        meta.insert("subagent-transcript".to_string(), serde_json::Value::Bool(true));
    }
    // The capabilities array is deliberately "sessionFailure" ONLY.
    // claude-agent-acp 0.69.0 and codex-acp 1.4.0 added a second AIR
    // capability, "agentFileChangeReport": advertise it and every prompt may
    // carry `_meta.jetbrains.air.agentFileChangeReportRequest = {version: 1,
    // requestId}`, after which the agent runs an EXTRA model round-trip at the
    // end of the turn (claude: a Stop hook plus a hidden continuation calling
    // `mcp__claude_agent_acp__report_changed_files`; codex: an ephemeral
    // read-only `thread/fork`) and answers on
    // `session_info_update._meta.jetbrains.air.agentFileChangeReport`.
    //
    // codeg does not ask for it, and the reason is not cost alone: both
    // adapters CLAMP the reported paths to `cwd` + `additionalDirectories`
    // (anything outside a root is dropped as truncated), which is exactly the
    // tree `workspace_state` already watches recursively via `notify`. So the
    // report can only ever name a SUBSET of what the watcher sees, less
    // reliably — it is a model self-report that declares `complete: false`
    // when unsure and truncates at 1024 paths / 256KB. It exists for clients
    // with no filesystem watcher; codeg is not one. Nothing else in either
    // release depends on it, and both adapters no-op without the
    // advertisement, so staying out costs us nothing.
    if matches!(agent_type, AgentType::ClaudeCode | AgentType::Codex) {
        meta.insert(
            "jetbrains".to_string(),
            serde_json::json!({
                "air": { "version": 1, "capabilities": ["sessionFailure"] }
            }),
        );
    }
    if !meta.is_empty() {
        client_capabilities = client_capabilities.meta(meta);
    }
    client_capabilities
}

fn build_new_session_request(
    agent_type: AgentType,
    cwd: &Path,
    mcp_servers: Vec<McpServer>,
) -> NewSessionRequest {
    let mut req = NewSessionRequest::new(cwd.to_path_buf());
    if let Some(meta) = claude_raw_sdk_session_meta(agent_type) {
        req = req.meta(meta);
    }
    if !mcp_servers.is_empty() {
        req = req.mcp_servers(mcp_servers);
    }
    req
}

fn build_load_session_request(
    agent_type: AgentType,
    session_id: SessionId,
    cwd: &Path,
    mcp_servers: Vec<McpServer>,
) -> LoadSessionRequest {
    let mut req = LoadSessionRequest::new(session_id, cwd.to_path_buf());
    if let Some(meta) = claude_raw_sdk_session_meta(agent_type) {
        req = req.meta(meta);
    }
    if !mcp_servers.is_empty() {
        req = req.mcp_servers(mcp_servers);
    }
    req
}

/// Build a `session/resume` request. Mirrors `build_load_session_request`
/// (same fields + ClaudeCode raw-SDK meta + non-empty mcp_servers); the only
/// wire difference is that `ResumeSessionRequest.mcp_servers` is
/// `skip_serializing_if = Vec::is_empty`, so an empty list is omitted from the
/// payload rather than emitted as `[]`.
fn build_resume_session_request(
    agent_type: AgentType,
    session_id: SessionId,
    cwd: &Path,
    mcp_servers: Vec<McpServer>,
) -> ResumeSessionRequest {
    let mut req = ResumeSessionRequest::new(session_id, cwd.to_path_buf());
    if let Some(meta) = claude_raw_sdk_session_meta(agent_type) {
        req = req.meta(meta);
    }
    if !mcp_servers.is_empty() {
        req = req.mcp_servers(mcp_servers);
    }
    req
}

/// Wire-level half of `session/resume`: send the request and deserialize the
/// reply into `ResumeSessionResponse`.
///
/// `sacp` 11.0.0 ships no `JsonRpcRequest` impl for `ResumeSessionRequest`, and
/// the orphan rule blocks codeg from adding one, so we send via `UntypedMessage`
/// — the same in-tree pattern `set_session_config_option_inner` already uses for
/// `session/set_config_option`. On a JSON-RPC error the agent returns,
/// `block_task()` yields `Err(sacp::Error)` with `.code` / `.to_string()`
/// intact, so the caller's error ladder reads identically to the
/// `session/load` arm.
async fn send_resume_session(
    cx: &ConnectionTo<Agent>,
    req: ResumeSessionRequest,
) -> Result<(ResumeSessionResponse, Option<serde_json::Value>), sacp::Error> {
    let untyped_req = UntypedMessage::new("session/resume", req).map_err(|e| {
        sacp::util::internal_error(format!("Failed to build resume request: {e}"))
    })?;

    let mut raw_response = cx.send_request_to(Agent, untyped_req).block_task().await?;
    // Capture the raw top-level `models` (per-model reasoning-effort data) BEFORE
    // deserializing into the typed response, which drops it (Grok only — the
    // field survives serde as an ignored unknown for other agents).
    let models = raw_response.get("models").cloned();
    strip_unknown_config_options(&mut raw_response, "session/resume");
    let resp = serde_json::from_value(raw_response).map_err(|e| {
        sacp::util::internal_error(format!("Failed to parse resume response: {e}"))
    })?;
    Ok((resp, models))
}

/// Send `session/new` UNTYPED, so the raw response can be inspected before it is
/// deserialized.
///
/// Two things need the raw JSON. For Grok, the top-level `models` (per-model
/// reasoning-effort data) is dropped by the typed `NewSessionResponse` because
/// the `unstable_session_model` feature is off, so it is captured here and
/// returned; every other agent gets `None`. For *all* agents,
/// [`strip_unknown_config_options`] runs first so an option kind newer than
/// codeg's schema pin cannot fail the whole response.
///
/// The request bytes are identical to the typed send — `UntypedMessage::new`
/// serializes the very same `NewSessionRequest` — and `attach_session` only
/// consumes `session_id` / `modes` / `meta` off the result. Literal method
/// string because the schema's `SESSION_NEW_METHOD_NAME` is `pub(crate)` and
/// sacp ships no `JsonRpcRequest` for a raw new-session; this mirrors the
/// `session/resume` / `session/fork` untyped sends.
async fn send_new_session_capturing_models(
    cx: &ConnectionTo<Agent>,
    agent_type: AgentType,
    req: NewSessionRequest,
) -> Result<(NewSessionResponse, Option<serde_json::Value>), sacp::Error> {
    let untyped_req = UntypedMessage::new("session/new", req).map_err(|e| {
        sacp::util::internal_error(format!("Failed to build new_session request: {e}"))
    })?;
    let mut raw_response = cx.send_request_to(Agent, untyped_req).block_task().await?;
    let models = (agent_type == AgentType::Grok)
        .then(|| raw_response.get("models").cloned())
        .flatten();
    strip_unknown_config_options(&mut raw_response, "session/new");
    let resp = serde_json::from_value(raw_response).map_err(|e| {
        sacp::util::internal_error(format!("Failed to parse new_session response: {e}"))
    })?;
    Ok((resp, models))
}

/// Send `session/load` UNTYPED for the same reason as
/// [`send_new_session_capturing_models`]: the raw response must pass through
/// [`strip_unknown_config_options`] before the typed parse. Request bytes and
/// the `Err(sacp::Error)` a JSON-RPC failure yields (`.code` / `.to_string()`
/// intact) are unchanged, so the caller's error ladder reads identically.
async fn send_load_session(
    cx: &ConnectionTo<Agent>,
    req: LoadSessionRequest,
) -> Result<LoadSessionResponse, sacp::Error> {
    let untyped_req = UntypedMessage::new("session/load", req).map_err(|e| {
        sacp::util::internal_error(format!("Failed to build load_session request: {e}"))
    })?;
    let mut raw_response = cx.send_request_to(Agent, untyped_req).block_task().await?;
    strip_unknown_config_options(&mut raw_response, "session/load");
    serde_json::from_value(raw_response).map_err(|e| {
        sacp::util::internal_error(format!("Failed to parse load_session response: {e}"))
    })
}

/// Whether MCP servers forwarded over the ACP wire (`session/new.mcpServers`)
/// actually reach the agent's model. Almost all adapters deliver them; pi-acp
/// (0.0.31) accepts the `mcpServers` field but DROPS it — it never forwards MCP
/// to the inner `pi --mode rpc` process, and pi has no native MCP. So forwarding
/// either user servers or the built-in codeg-mcp companion to pi is futile, and
/// injecting codeg-mcp would falsely mark delegation/feedback/ask as available
/// (`feedback_tool_available`, a registered delegation token pi can never use).
/// `supports_mcp` stays `true` for pi (session/new tolerates the field), so this
/// is a separate, narrower gate. Gate codeg-mcp injection on it.
fn agent_delivers_wire_mcp(agent_type: AgentType) -> bool {
    !matches!(agent_type, AgentType::Pi)
}

/// Load MCP servers configured for `agent_type` and convert them into the
/// ACP wire format. Errors and unsupported entries are logged and skipped so
/// a single malformed entry never blocks a session from starting.
fn load_mcp_servers_for_agent(agent_type: AgentType) -> Vec<McpServer> {
    // Hermes, Kimi Code, Grok, and Cursor each read their own native MCP
    // config at launch — Hermes from `~/.hermes/config.yaml` (`mcp_servers`,
    // registered as `mcp-<name>` toolsets), Kimi Code from
    // `~/.kimi-code/mcp.json` (`mcpServers`), Grok from `~/.grok/config.toml`
    // (`[mcp_servers.<name>]`), Cursor from `~/.cursor/mcp.json`
    // (`mcpServers`, shared with the IDE). codeg manages those files directly
    // via the MCP settings UI, so forwarding the same servers over the ACP
    // wire here would double-register them — skip it. (The built-in
    // `codeg-mcp` companion is injected separately by `inject_codeg_mcp`, so
    // it still reaches them.)
    //
    // DeepSeek is deliberately NOT in this set: deepseek-acp reads no MCP file
    // at all, so `$DSH_HOME/mcp.json` (codeg's own store) reaches it ONLY
    // through the wire — skipping it would silently drop every user server.
    //
    // Qoder joins the skip set: the CLI reads `mcpServers` out of its own
    // `~/.qoder/settings.json` (gemini-schema settings file) at startup, which
    // codeg's MCP settings UI manages directly — forwarding the same servers
    // over the wire would double-mount them.
    if matches!(
        agent_type,
        AgentType::Hermes
            | AgentType::KimiCode
            | AgentType::Grok
            | AgentType::Cursor
            | AgentType::Qoder
    ) {
        return Vec::new();
    }
    let entries = match crate::commands::mcp::read_servers_for_agent_type(agent_type) {
        Ok(map) => map,
        Err(err) => {
            tracing::error!(
                "[ACP][{}] failed to read MCP servers from local config: {err}",
                agent_type
            );
            return Vec::new();
        }
    };

    let mut out = Vec::with_capacity(entries.len());
    for (name, spec) in entries {
        match canonical_spec_to_mcp_server(&name, &spec) {
            Ok(server) => out.push(server),
            Err(err) => {
                tracing::warn!(
                    "[ACP][{}] skip MCP server '{name}' (cannot map to ACP schema): {err}",
                    agent_type
                );
            }
        }
    }
    out
}

/// Context the connection layer needs to inject the built-in `codeg-mcp`
/// MCP entry. Built once per `run_connection` from the live AppState pieces
/// (broker config, token registry, UDS path) and passed through.
///
/// Optional because some test paths spin up `run_connection` without a
/// full delegation stack — those just skip injection.
/// Injection-time lookup of which agents the user has disabled in settings.
///
/// `delegate_to_agent`'s advertised targets must track the live toggle: a
/// disabled agent cannot spawn anyway (`build_session_runtime_env` rejects it
/// inside the delegation spawner), so listing it would only invite doomed
/// calls. Read fresh on every injection — sessions launched before a toggle
/// flip keep their launch-time list, and the spawn-time check stays the hard
/// gate for those.
#[async_trait::async_trait]
pub trait AgentAvailabilityLookup: Send + Sync {
    /// Wire slugs (`AgentType::as_wire`) of the agents disabled in settings.
    async fn disabled_agent_wire_slugs(&self) -> Vec<String>;
}

/// [`AgentAvailabilityLookup`] over the live `AppDatabase`: `agent_setting`
/// rows with `enabled = false`. An absent row means enabled (the settings
/// default). A read error fails OPEN — the enum then lists everything rather
/// than taking the whole companion injection down, and the spawn-time
/// disabled check still enforces.
pub struct DbAgentAvailabilityLookup {
    pub db: Arc<crate::db::AppDatabase>,
}

#[async_trait::async_trait]
impl AgentAvailabilityLookup for DbAgentAvailabilityLookup {
    async fn disabled_agent_wire_slugs(&self) -> Vec<String> {
        match crate::db::service::agent_setting_service::list(&self.db.conn).await {
            Ok(rows) => rows
                .into_iter()
                .filter(|row| !row.enabled)
                .filter_map(|row| serde_json::from_str::<AgentType>(&row.agent_type).ok())
                .map(|agent_type| agent_type.as_wire().into_owned())
                .collect(),
            Err(e) => {
                tracing::warn!(
                    "[delegation] reading agent settings failed ({e}); \
                     delegate targets will not be filtered this launch"
                );
                Vec::new()
            }
        }
    }
}

#[derive(Clone)]
pub struct DelegationInjection {
    pub broker: Arc<crate::acp::delegation::broker::DelegationBroker>,
    pub tokens: Arc<crate::acp::delegation::listener::TokenRegistry>,
    pub socket_path: PathBuf,
    /// Which agents are currently disabled in settings, read at injection
    /// time so `delegate_to_agent` only advertises launchable targets.
    pub agent_availability: Arc<dyn AgentAvailabilityLookup>,
    /// Hot-swappable "is live-feedback enabled?" flag. Read at injection time
    /// alongside the broker's delegation flag so `codeg-mcp` is injected when
    /// EITHER feature is on, and the companion is told which tool groups to
    /// expose. Shares the same `tokens` registry and UDS socket as delegation.
    pub feedback: crate::acp::feedback::FeedbackRuntimeConfig,
    /// Hot-swappable "is ask-user-question enabled?" flag. Read at injection
    /// time alongside delegation + feedback so `codeg-mcp` is injected when ANY
    /// of the three is on, and the companion's `--features` lists `ask` to expose
    /// the `ask_user_question` tool.
    pub ask: crate::acp::question::QuestionRuntimeConfig,
    /// Hot-swappable "is get-session-info enabled?" flag. Read at injection time
    /// alongside the other three so `codeg-mcp` is injected when ANY of the four
    /// is on, and the companion's `--features` lists `sessions` to expose the
    /// `get_session_info` tool. No teardown handle (the lookup is stateless).
    pub sessions: crate::acp::session_info::SessionInfoRuntimeConfig,
    /// Hot-swappable chat-authoring flags (`create_automation` /
    /// `create_work_task`). Read at injection time like the others so the
    /// companion's `--features` lists `automations` / `taskboard`. Unlike the
    /// read-only groups these are ALSO re-read at call time by the authoring
    /// access impl — see [`crate::acp::chat_authoring::ChatAuthoringRuntimeConfig`].
    pub authoring: crate::acp::chat_authoring::ChatAuthoringRuntimeConfig,
    /// Question registry handle for the teardown cascade. The `run_connection`
    /// cleanup guard calls `cancel_questions_by_parent` through this so a pending
    /// `ask_user_question` is reclaimed synchronously on disconnect, mirroring
    /// the delegation `broker.cancel_by_parent` cleanup. Shares the same backing
    /// `ConnectionManager` as the listener's question lookup.
    pub questions: Arc<dyn crate::acp::question::SessionQuestionAccess>,
    /// Plan-approval registry handle for Grok's `exit_plan_mode` ext bridge.
    /// Unlike delegation / ask / feedback this is NOT a codeg-mcp feature — it is
    /// Grok's native plan mode — so it has no runtime on/off flag and is always
    /// wired. Shares the same backing `ConnectionManager` as the question lookup.
    /// The `run_connection` handler registers approvals + parks on the reply
    /// through this, and the cleanup guard calls `cancel_plan_approvals_by_parent`
    /// on disconnect (mirroring the question teardown cascade).
    pub plan_approvals: Arc<dyn crate::acp::plan_approval::SessionPlanApprovalAccess>,
}

/// Locate the `codeg-mcp` companion binary across the supported deployment
/// shapes:
///
/// 1. `CODEG_MCP_BIN` env override — explicit absolute path. Lets dev shells,
///    custom installs, and integration tests point at a freshly compiled
///    binary without touching the install layout.
/// 2. Sibling of the running executable — the production layout for every
///    shipping target. Tauri sidecar (`Contents/MacOS/codeg-mcp` on macOS,
///    next to `codeg.exe` on Windows, next to the unix binary on Linux
///    deb/rpm), `install.sh`/`install.ps1` (drops `codeg-mcp` next to
///    `codeg-server`), Docker image (`/usr/local/bin/codeg-mcp` next to
///    `codeg-server`), and `cargo build` dev output
///    (`target/<profile>/codeg-mcp`).
/// 3. `PATH` lookup — last-resort for atypical layouts where ops moved the
///    two binaries apart but kept both reachable on `PATH`.
///
/// Returns `None` when no candidate is an executable file. Callers MUST
/// treat `None` as "delegation is unavailable at this site" and skip
/// injection — never paper over with a phantom path, because that fails
/// inside the agent's MCP spawn loop and may take the entire ACP session
/// down on stricter agents.
fn locate_codeg_mcp_binary() -> Option<PathBuf> {
    let filename = if cfg!(windows) {
        "codeg-mcp.exe"
    } else {
        "codeg-mcp"
    };

    if let Some(raw) = std::env::var_os("CODEG_MCP_BIN") {
        let candidate = PathBuf::from(raw);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }

    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        let candidate = dir.join(filename);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }

    which::which(filename)
        .ok()
        .filter(|p| is_executable_file(p))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

/// Append the built-in `codeg-mcp` MCP entry if delegation is enabled
/// AND the companion binary is present on disk. Returns the per-launch token
/// that was registered, or `None` when injection was skipped (disabled by
/// config, or binary missing).
///
/// When the binary is missing we log a single-line warning and skip
/// injection rather than register the token + emit a phantom McpServerStdio
/// pointing at a non-existent path. Phantom injection would have made every
/// new ACP session ship a guaranteed-to-fail MCP server entry: stricter
/// agents (Claude Code) refuse the whole session; lax agents lose the
/// delegate tool silently. Skipping leaves the agent fully functional minus
/// `delegate_to_agent`, which is the right degradation when codeg-mcp didn't
/// make it into the install.
/// Which tool groups a companion launch should expose. A struct rather than a
/// positional bool list: the groups keep growing and seven adjacent `bool`s at a
/// call site is a silent argument-swap waiting to happen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CompanionFeatureFlags {
    delegation: bool,
    feedback: bool,
    ask: bool,
    sessions: bool,
    /// Per-spawn (task-engine launches only), not a settings toggle — on its own
    /// it still injects the companion so a task session always has its reporting
    /// tools.
    tasks: bool,
    /// `create_automation`, gated by the chat-authoring setting.
    automations: bool,
    /// `create_work_task`, gated by the chat-authoring setting.
    taskboard: bool,
}

/// The `--features` value for a companion launch, or `None` when no group is
/// enabled (the companion isn't injected at all). Pulled out as a pure function
/// so the inject/skip decision is unit-testable without a real binary on disk or
/// a live broker. The order here is the order the companion's
/// `CompanionFeatures::parse` recognizes.
fn companion_features_arg(flags: CompanionFeatureFlags) -> Option<String> {
    let mut features: Vec<&str> = Vec::new();
    if flags.delegation {
        features.push("delegation");
    }
    if flags.feedback {
        features.push("feedback");
    }
    if flags.ask {
        features.push("ask");
    }
    if flags.sessions {
        features.push("sessions");
    }
    if flags.tasks {
        features.push("tasks");
    }
    if flags.automations {
        features.push("automations");
    }
    if flags.taskboard {
        features.push("taskboard");
    }
    if features.is_empty() {
        return None;
    }
    Some(features.join(","))
}

/// Outcome of injecting the `codeg-mcp` companion: the per-launch token to
/// stash for revocation, plus whether the `check_user_feedback` tool was exposed
/// to this agent (so the session can gate submit + UI on its real capability).
struct CompanionInjection {
    token: String,
    feedback_available: bool,
}

async fn inject_codeg_mcp(
    servers: &mut Vec<McpServer>,
    injection: &DelegationInjection,
    parent_connection_id: &str,
    working_dir: &Path,
    tasks_enabled: bool,
    host_tools: HostToolsPolicy,
) -> Option<CompanionInjection> {
    // codeg-mcp carries BOTH the delegation tools and the live-feedback tool.
    // Inject it when EITHER feature is enabled; the `--features` arg tells the
    // companion which tool groups to expose so a disabled feature's tools never
    // surface to the LLM. (Historically this was gated on delegation alone.)
    // `tasks_enabled` is per-spawn: true only for task-engine launches, which
    // must get their reporting tools regardless of the settings toggles.
    let feedback_enabled = injection.feedback.is_enabled().await;
    let authoring = injection.authoring.snapshot().await;
    // Delegation is a THIRD door into the same room as `fs/*` and `terminal/*`:
    // `delegate_to_agent` has codeg spawn a second agent — in codeg's process
    // tree, under ITS own (by default `Default`) policy — and hand its output
    // back. A sandboxed agent that cannot read `.env` itself would just ask a
    // sibling to read it. That defeats the boundary this switch advertises, for
    // the same reason withholding only one of fs/terminal would, so `agent`
    // withholds this group too. The other groups stay: they surface codeg's own
    // state (feedback, ask, session info, task reporting), not arbitrary file
    // or command execution on the user's machine.
    let delegation_configured = injection.broker.config_snapshot().await.enabled;
    let delegation_enabled = delegation_configured && host_tools.hosts_channels();
    if delegation_configured && !delegation_enabled {
        // The one combination that looks like a bug from the settings UI: the
        // multi-agent switch reads "on" and the tools are still absent. Say so
        // once per launch, naming the knob, so the log answers the question the
        // user actually asks ("why did delegate_to_agent disappear?").
        tracing::info!(
            "[delegation] multi-agent delegation is enabled in settings but withheld from \
             connection {parent_connection_id}: {HOST_TOOLS_ENV}=agent hands fs/terminal back \
             to this agent, and delegate_to_agent would route the same work through codeg \
             anyway. Turn that per-agent switch off to restore the delegation tools."
        );
    }
    let flags = CompanionFeatureFlags {
        delegation: delegation_enabled,
        feedback: feedback_enabled,
        ask: injection.ask.is_enabled().await,
        sessions: injection.sessions.is_enabled().await,
        tasks: tasks_enabled,
        automations: authoring.automations_enabled,
        taskboard: authoring.work_tasks_enabled,
    };
    // `None` (no feature enabled) short-circuits the whole injection.
    let features_arg = companion_features_arg(flags)?;
    let Some(binary_path) = locate_codeg_mcp_binary() else {
        tracing::warn!(
            "[delegation][WARN] codeg-mcp companion binary not found (checked CODEG_MCP_BIN, \
             exe sibling, and PATH); skipping delegate_to_agent / check_user_feedback / \
             ask_user_question / get_session_info tool injection for connection \
             {parent_connection_id}. Reinstall codeg or set CODEG_MCP_BIN to fix."
        );
        return None;
    };
    let token = uuid::Uuid::new_v4().to_string();
    injection
        .tokens
        .register(
            token.clone(),
            crate::acp::delegation::listener::TokenEntry {
                parent_connection_id: parent_connection_id.to_string(),
                working_dir: working_dir.to_path_buf(),
            },
        )
        .await;
    let mut server = McpServerStdio::new("codeg-mcp", binary_path);
    let mut args = vec![
        "--parent-connection-id".to_string(),
        parent_connection_id.to_string(),
        "--socket-path".to_string(),
        injection.socket_path.to_string_lossy().to_string(),
        "--token".to_string(),
        token.clone(),
        // Self-cleanup watchdog: codeg-mcp exits when this PID is gone so
        // orphaned companions can't keep the binary file locked across an
        // installer upgrade (Windows) or hold a stale broker connection
        // (any platform).
        "--parent-pid".to_string(),
        std::process::id().to_string(),
        // Tool groups to expose this launch (see `CompanionFeatureFlags`).
        "--features".to_string(),
        features_arg,
    ];
    // Advertised delegate targets track the user's enable toggles, read
    // fresh at injection time. Registered-and-enabled custom agents become
    // extra `delegate_to_agent` targets; disabled BUILT-INS are subtracted
    // companion-side (`--disabled-agents`) so the embedded schema stays the
    // single source of truth for the builtin list and its order. Either flag
    // is omitted when empty: the companion then serves its embedded
    // builtin-only schema unchanged, and an older codeg-mcp binary (which
    // rejects unknown flags at startup) keeps working for every installation
    // that needs neither.
    let disabled = injection
        .agent_availability
        .disabled_agent_wire_slugs()
        .await;
    let (custom_slugs, disabled_builtins) = delegate_target_args(&disabled);
    if !custom_slugs.is_empty() {
        args.push("--custom-agents".to_string());
        args.push(custom_slugs.join(","));
    }
    if !disabled_builtins.is_empty() {
        args.push("--disabled-agents".to_string());
        args.push(disabled_builtins.join(","));
    }
    server = server.args(args);
    servers.push(McpServer::Stdio(server));
    Some(CompanionInjection {
        token,
        feedback_available: feedback_enabled,
    })
}

/// Split the delegate-target adjustments into the two companion flags:
/// custom agents to APPEND to `delegate_to_agent`'s enum (the registered set
/// minus the disabled ones), and disabled built-ins for the companion to
/// SUBTRACT from its embedded list. Disabled customs need no subtraction
/// entry — they are simply never appended. The subtraction list is sorted so
/// the arg string is deterministic regardless of settings-row order.
fn delegate_target_args(disabled_wire_slugs: &[String]) -> (Vec<String>, Vec<String>) {
    let disabled: HashSet<&str> = disabled_wire_slugs.iter().map(String::as_str).collect();
    let custom_slugs: Vec<String> = crate::acp::custom_registry::all()
        .iter()
        .map(|a| a.as_wire().into_owned())
        .filter(|slug| !disabled.contains(slug.as_str()))
        .collect();
    let mut disabled_builtins: Vec<String> = disabled_wire_slugs
        .iter()
        .filter(|slug| !slug.starts_with(crate::models::agent::CUSTOM_AGENT_WIRE_PREFIX))
        .cloned()
        .collect();
    disabled_builtins.sort();
    disabled_builtins.dedup();
    (custom_slugs, disabled_builtins)
}

/// Resolve an MCP server `command` to an absolute path.
///
/// The ACP spec requires `McpServerStdio.command` to be an absolute path.
/// Users typically configure bare names like `npx` / `node` / `bunx`; if we
/// forwarded those verbatim, agents would fail to spawn the server. We try
/// `which` first, fall back to the platform-normalized form (which adds
/// `.exe`/`.cmd` on Windows), and finally to the raw input as last resort.
fn resolve_mcp_command(command: &str) -> PathBuf {
    let path = Path::new(command);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Ok(found) = which::which(command) {
        return found;
    }
    PathBuf::from(crate::process::normalized_program(command))
}

fn canonical_spec_to_mcp_server(name: &str, spec: &serde_json::Value) -> Result<McpServer, String> {
    let obj = spec
        .as_object()
        .ok_or_else(|| "spec must be a JSON object".to_string())?;
    let typ = obj
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("stdio");

    match typ {
        "stdio" => {
            let command = obj
                .get("command")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| "stdio MCP entry missing 'command'".to_string())?;
            // ACP spec requires an absolute path. If users wrote a bare
            // command (e.g. "npx"), resolve it via PATH so the agent can
            // actually spawn the server. Fall back to the raw value when
            // resolution fails — the agent will surface a clearer error.
            let command_path = resolve_mcp_command(command);
            let mut server = McpServerStdio::new(name, command_path);
            if let Some(args) = obj.get("args").and_then(serde_json::Value::as_array) {
                let args: Vec<String> = args
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect();
                if !args.is_empty() {
                    server = server.args(args);
                }
            }
            if let Some(env_obj) = obj.get("env").and_then(serde_json::Value::as_object) {
                let env_vars: Vec<sacp::schema::EnvVariable> = env_obj
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| sacp::schema::EnvVariable::new(k, s)))
                    .collect();
                if !env_vars.is_empty() {
                    server = server.env(env_vars);
                }
            }
            Ok(McpServer::Stdio(server))
        }
        "http" | "sse" => {
            let url = obj
                .get("url")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| "remote MCP entry missing 'url'".to_string())?;
            let headers: Vec<HttpHeader> = obj
                .get("headers")
                .and_then(serde_json::Value::as_object)
                .map(|map| {
                    map.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| HttpHeader::new(k, s)))
                        .collect()
                })
                .unwrap_or_default();
            if typ == "http" {
                let mut server = McpServerHttp::new(name, url);
                if !headers.is_empty() {
                    server = server.headers(headers);
                }
                Ok(McpServer::Http(server))
            } else {
                let mut server = McpServerSse::new(name, url);
                if !headers.is_empty() {
                    server = server.headers(headers);
                }
                Ok(McpServer::Sse(server))
            }
        }
        other => Err(format!("unsupported MCP transport type '{other}'")),
    }
}

/// The main ACP connection loop.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    name = "connection",
    skip_all,
    fields(
        connection_id = %connection_id,
        agent_type = ?agent_type,
        working_dir = ?working_dir,
        session_id = ?session_id,
    )
)]
async fn run_connection(
    agent: AcpAgent,
    connection_id: String,
    agent_type: AgentType,
    working_dir: Option<String>,
    session_id: Option<String>,
    mut cmd_rx: mpsc::Receiver<ConnectionCommand>,
    emitter: EventEmitter,
    state: Arc<RwLock<SessionState>>,
    terminal_base_env: BTreeMap<String, String>,
    terminal_shell_config: TerminalShellRuntimeConfig,
    preferred_mode_id: Option<String>,
    preferred_config_values: BTreeMap<String, String>,
    delegation_injection: Option<DelegationInjection>,
    fs_policy: FsAccessPolicy,
    host_tools: HostToolsPolicy,
    // Connection-scoped agent stderr buffer, shared with the `with_debug`
    // callback installed by `build_agent`. Read only when a turn ends without
    // agent output, to attach evidence to the synthesized error.
    stderr_tail: Arc<StderrTail>,
) -> Result<(), AcpError> {
    let pending_perms: PendingPermissions =
        Arc::new(tokio::sync::Mutex::new(PermissionQueue::default()));
    // `terminal_base_env` already filtered to just the credential helper
    // keys upstream — see `spawn_agent_connection` for the rationale and
    // why we don't forward the full agent runtime_env here.
    let cwd = resolve_working_dir(working_dir.as_deref());
    // Default terminals to the session working directory so an agent that calls
    // `terminal/create` without a `cwd` (e.g. CodeBuddy) runs in the folder the
    // conversation runs in rather than codeg's own process cwd.
    let terminal_runtime = Arc::new(
        TerminalRuntime::with_base_env(terminal_base_env)
            .with_default_cwd(Some(cwd.clone()))
            .with_default_shell_config(terminal_shell_config),
    );
    let cwd_string = cwd.to_string_lossy().to_string();
    // The connection's security posture in one place, so what a live session
    // actually enforces is readable from the log rather than inferred.
    tracing::info!(
        "[ACP] fs policy {} | host tools {}",
        fs_policy.describe(),
        host_tools.describe()
    );
    // `strict` reads as a containment boundary and is not one while codeg also
    // advertises `terminal`: an agent refused a read just `cat`s the file
    // through the shell codeg runs for it (empirically what grok does). Say so
    // rather than letting the knob's name do the promising.
    if fs_policy.confines_reads() && host_tools.hosts_channels() {
        tracing::warn!(
            "[ACP] {FS_POLICY_ENV} confines the fs channel but codeg still advertises \
             `terminal`, so an agent reaches the same paths through a shell — this is \
             not a containment boundary. Set {HOST_TOOLS_ENV}=agent to hand file access \
             and commands back to the agent, where its own sandbox applies."
        );
    }
    let file_system_runtime = Arc::new(FileSystemRuntime::with_policy(fs_policy));

    let conn_id = connection_id.clone();
    let emitter_clone = emitter.clone();
    let perms = pending_perms.clone();
    let state_outer = Arc::clone(&state);

    // Grok's native `ask_user_question` (verified against 0.2.101) arrives as an
    // `_x.ai/ask_user_question` ACP ext request that BLOCKS on the reply — rather
    // than the codeg-mcp tool. Capture the shared question access + feature toggle
    // (both live on the delegation injection) so the ext handler can register the
    // questions through the SAME interactive-card pipeline and answer grok once the
    // user submits. `None` when the companion isn't injected — the handler then
    // lets grok fall back to its inert rendering.
    let grok_ask_access = delegation_injection
        .as_ref()
        .map(|inj| (Arc::clone(&inj.questions), inj.ask.clone()));
    let grok_ask_conn_id = connection_id.clone();
    // Grok `exit_plan_mode` bridge access — always wired in production (native
    // plan mode, no feature flag). `None` only on the test paths that spin up
    // `run_connection` without a delegation stack; the handler then replies
    // disconnect and grok keeps plan mode active.
    let grok_plan_access = delegation_injection
        .as_ref()
        .map(|inj| Arc::clone(&inj.plan_approvals));
    let grok_plan_conn_id = connection_id.clone();
    // The ext handler emits the answered in-stream card (`AskQuestionResultCard`)
    // itself once the user submits — grok never emits a completed tool result into
    // the ACP stream — so it needs this connection's session state + emitter.
    let grok_ask_state = Arc::clone(&state);
    let grok_ask_emitter = emitter.clone();

    // Claude-only: tail this connection's session transcript for OUT-OF-TURN
    // activity (async sub-agent / background-shell completions, the agent's
    // continued work after them, cron//loop autonomous turns — none of which
    // the wire reliably represents) and surface it as `BackgroundActivity`
    // events; also feeds the keep-alive accounting that exempts the
    // connection from the idle sweeps while such work is pending. Created
    // HERE — per CONNECTION, not per conversation loop — so ONE watcher (and
    // one prompt ledger) spans fork restarts: `run_watch` observes the
    // session-id change and re-arms in place, carrying still-outstanding
    // tasks and settled ids across the fork (a post-fork `SendMessage`
    // resume must re-arm the keep-alive). The guard aborts the watcher when
    // this connection ends. Its spawn epoch (captured before the session
    // exists) is what lets the first arm process records written before the
    // transcript file is discovered.
    let prompt_ledger = background_watch::PromptLedger::shared();
    let _bg_watch = background_watch::spawn_if_claude(
        &connection_id,
        agent_type,
        Arc::clone(&state),
        emitter.clone(),
        cwd_string.clone(),
        Arc::clone(&prompt_ledger),
    );

    Client
        .builder()
        .name("codeg")
        .on_receive_request(
            {
                let emitter_inner = emitter_clone.clone();
                let perms = perms.clone();
                let perm_cwd = cwd_string.clone();
                let state_inner = Arc::clone(&state);
                async move |req: RequestPermissionRequest,
                            responder: Responder<RequestPermissionResponse>,
                            _cx: ConnectionTo<Agent>| {
                    handle_permission_request(
                        &state_inner,
                        &emitter_inner,
                        &perms,
                        &perm_cwd,
                        agent_type,
                        req,
                        responder,
                    )
                    .await;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let runtime = file_system_runtime.clone();
                async move |req: ReadTextFileRequest,
                            responder: Responder<ReadTextFileResponse>,
                            _cx: ConnectionTo<Agent>| {
                    if !host_tools.hosts_channels() {
                        return refuse_unadvertised_channel(responder, "fs/read_text_file");
                    }
                    respond_file_system_request(responder, runtime.read_text_file(req).await)?;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let runtime = file_system_runtime.clone();
                async move |req: WriteTextFileRequest,
                            responder: Responder<WriteTextFileResponse>,
                            _cx: ConnectionTo<Agent>| {
                    if !host_tools.hosts_channels() {
                        return refuse_unadvertised_channel(responder, "fs/write_text_file");
                    }
                    respond_file_system_request(responder, runtime.write_text_file(req).await)?;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let runtime = terminal_runtime.clone();
                async move |req: CreateTerminalRequest,
                            responder: Responder<CreateTerminalResponse>,
                            _cx: ConnectionTo<Agent>| {
                    if !host_tools.hosts_channels() {
                        return refuse_unadvertised_channel(responder, "terminal/create");
                    }
                    respond_terminal_request(responder, runtime.create_terminal(req).await)?;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let runtime = terminal_runtime.clone();
                async move |req: TerminalOutputRequest,
                            responder: Responder<TerminalOutputResponse>,
                            _cx: ConnectionTo<Agent>| {
                    if !host_tools.hosts_channels() {
                        return refuse_unadvertised_channel(responder, "terminal/output");
                    }
                    respond_terminal_request(responder, runtime.terminal_output(req).await)?;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let runtime = terminal_runtime.clone();
                async move |req: WaitForTerminalExitRequest,
                            responder: Responder<WaitForTerminalExitResponse>,
                            cx: ConnectionTo<Agent>| {
                    if !host_tools.hosts_channels() {
                        // Refuse INLINE, before the spawn below: there is no
                        // terminal to wait on, so answering immediately is
                        // correct and keeps the refusal off the spawn path.
                        return refuse_unadvertised_channel(responder, "terminal/wait_for_exit");
                    }
                    // `terminal/wait_for_exit` blocks until the command exits,
                    // and sacp awaits request handlers INSIDE its single
                    // dispatch loop ("the loop awaits the handler to completion
                    // before processing the next message"). Answering inline
                    // therefore freezes the ENTIRE connection for a command
                    // that never exits — an agent that backgrounds a dev server
                    // and then monitors it (grok does exactly this) would stall
                    // the turn forever, with every later session/update stuck
                    // unprocessed in the transport queue.
                    //
                    // Answer from a spawned task instead — sacp's own sanctioned
                    // escape hatch. `cx.spawn` rather than `tokio::spawn` so the
                    // wait is connection-scoped and torn down with it.
                    let runtime = runtime.clone();
                    cx.spawn(async move {
                        let result = runtime.wait_for_terminal_exit(req).await;
                        if let Err(err) = respond_terminal_request(responder, result) {
                            // Propagating this would tear down the whole
                            // connection, and a failed send only means the peer
                            // is already gone.
                            tracing::warn!(
                                "[ACP] failed to answer terminal/wait_for_exit: {err}"
                            );
                        }
                        Ok(())
                    })?;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let runtime = terminal_runtime.clone();
                async move |req: KillTerminalRequest,
                            responder: Responder<KillTerminalResponse>,
                            _cx: ConnectionTo<Agent>| {
                    if !host_tools.hosts_channels() {
                        return refuse_unadvertised_channel(responder, "terminal/kill");
                    }
                    respond_terminal_request(responder, runtime.kill_terminal(req).await)?;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let runtime = terminal_runtime.clone();
                async move |req: ReleaseTerminalRequest,
                            responder: Responder<ReleaseTerminalResponse>,
                            _cx: ConnectionTo<Agent>| {
                    if !host_tools.hosts_channels() {
                        return refuse_unadvertised_channel(responder, "terminal/release");
                    }
                    respond_terminal_request(responder, runtime.release_terminal(req).await)?;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let access = grok_ask_access.clone();
                let conn_id = grok_ask_conn_id.clone();
                let card_state = Arc::clone(&grok_ask_state);
                let card_emitter = grok_ask_emitter.clone();
                async move |req: GrokAskUserQuestionRequest,
                            responder: Responder<serde_json::Value>,
                            _cx: ConnectionTo<Agent>| {
                    handle_grok_ask_user_question(
                        &access,
                        &conn_id,
                        &card_state,
                        &card_emitter,
                        req,
                        responder,
                    )
                    .await;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let access = grok_plan_access.clone();
                let conn_id = grok_plan_conn_id.clone();
                async move |req: GrokExitPlanModeRequest,
                            responder: Responder<serde_json::Value>,
                            _cx: ConnectionTo<Agent>| {
                    handle_grok_exit_plan_mode(&access, &conn_id, req, responder).await;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                // Codex `elicitation/create`: question-style requests (Plan
                // mode `request_user_input`, generic MCP forms) bridge into
                // the same ask card as the codeg-mcp ask tool (reusing the ask
                // access + kill switch); approval-style requests (MCP
                // tool-call approvals, message-only confirms) route through
                // the permission card via `pending_perms`.
                let access = grok_ask_access.clone();
                let conn_id = grok_ask_conn_id.clone();
                let perms = perms.clone();
                let state_inner = Arc::clone(&state);
                let emitter_inner = emitter_clone.clone();
                async move |req: CodexElicitationRequest,
                            responder: Responder<serde_json::Value>,
                            _cx: ConnectionTo<Agent>| {
                    handle_elicitation_request(
                        &access,
                        &perms,
                        &state_inner,
                        &emitter_inner,
                        &conn_id,
                        req,
                        responder,
                    )
                    .await;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .connect_with(agent, async move |cx| -> Result<(), sacp::Error> {
            let state = state_outer;
            let agent_name_for_log = registry::get_agent_meta(agent_type).name;

            let init_request = InitializeRequest::new(ProtocolVersion::LATEST)
                .client_capabilities(build_client_capabilities(agent_type, host_tools));
            // Bound the Initialize handshake so an outdated / incompatible
            // cached binary that never responds can't leave the frontend
            // stuck on "Connecting...". A healthy agent answers in <1s; we
            // give 60s headroom for cold process startup on slow machines.
            //
            // We cannot carry a structured error code through sacp's Error
            // type, so we tag the timeout with `INIT_TIMEOUT_SENTINEL` and
            // convert it back to `AcpError::InitializeTimeout` in the
            // outer `.map_err(...)` below. The outer layer attaches a
            // stable `code` to the frontend event so it can be localized.
            tracing::info!(
                "[ACP][{agent_name_for_log}] Sending Initialize (protocol={}, timeout=60s)",
                ProtocolVersion::LATEST
            );
            let init_started = std::time::Instant::now();
            let init_resp = match tokio::time::timeout(
                std::time::Duration::from_secs(60),
                cx.send_request_to(Agent, init_request).block_task(),
            )
            .await
            {
                Ok(Ok(resp)) => {
                    tracing::info!(
                        "[ACP][{agent_name_for_log}] Initialize responded in {:?}",
                        init_started.elapsed()
                    );
                    resp
                }
                Ok(Err(e)) => {
                    tracing::error!(
                        "[ACP][{agent_name_for_log}] Initialize failed in {:?}: {e}",
                        init_started.elapsed()
                    );
                    return Err(e);
                }
                Err(_) => {
                    tracing::error!(
                        "[ACP][{agent_name_for_log}] Initialize TIMED OUT after {:?} \
                         — the agent never answered the handshake. Check the \
                         [stderr] lines above for agent-side errors. For a full \
                         JSON-RPC trace, re-launch with CODEG_ACP_DEBUG=1.",
                        init_started.elapsed()
                    );
                    return Err(sacp::util::internal_error(INIT_TIMEOUT_SENTINEL));
                }
            };
            emit_prompt_capabilities(
                &state,
                &emitter_clone,
                &init_resp.agent_capabilities.prompt_capabilities,
                agent_type,
            )
            .await;

            let supports_fork = init_resp
                .agent_capabilities
                .session_capabilities
                .fork
                .is_some();
            let supports_resume = init_resp
                .agent_capabilities
                .session_capabilities
                .resume
                .is_some();
            tracing::info!(
                "[ACP] Agent capabilities: load_session={}, fork={}, resume={}",
                init_resp.agent_capabilities.load_session, supports_fork, supports_resume
            );

            // Native live-feedback steering, synthesized ONCE from three gates
            // so every consumer (the submit split, the snapshot, the frontend)
            // reads a single authoritative bool: (1) the adapter advertises
            // the extension (top-level `_meta`), (2) the registry says this
            // agent type honors the `promptRequired` idle opt-in, and (3) the
            // RUNNING binary proves it via `agent_info.version` — launch
            // prefers a PATH-resolved install over the pinned package, so (2)
            // alone can't vouch for the process on the other end of the pipe.
            // The raw advertisement is deliberately NOT stored: exposing it
            // would tempt the frontend to re-derive eligibility and show the
            // instant channel for adapters (codex) that advertise steering but
            // would detach a turn on the idle race.
            let steering_advertised = init_advertises_steering(init_resp.meta.as_ref());
            let native_steering_available = synthesize_native_steering(
                agent_type,
                init_resp.meta.as_ref(),
                init_resp.agent_info.as_ref(),
            );
            tracing::info!(
                "[ACP][{}] steering: advertised={}, agent_version={:?}, native={}",
                agent_type,
                steering_advertised,
                init_resp.agent_info.as_ref().map(|i| i.version.as_str()),
                native_steering_available
            );

            // Goal channel selection, pinned for the whole connection: an
            // adapter advertising the provider-neutral goal extension
            // (`_meta.goal`, claude-agent-acp 0.66+/codex-acp 1.2+) publishes
            // goal snapshots ONLY there — and codex dropped the legacy
            // `_meta.codex.goal` key in the same release, so honoring the
            // advertisement is what keeps GoalCard alive across the bump (see
            // `session_info_goal_value`).
            let neutral_goal_channel = init_advertises_goal(init_resp.meta.as_ref());
            if neutral_goal_channel {
                tracing::info!("[ACP][{}] goal channel: provider-neutral (_meta.goal)", agent_type);
            }
            // Advertised goal-control surface (method + action vocabulary);
            // None keeps the legacy codex method and resolves the vocabulary to
            // the legacy pair — see the SessionState field docs.
            let goal_control = goal_advertised_control(init_resp.meta.as_ref());

            // Whether this agent accepts MCP server entries over the ACP wire
            // (`session/new`'s `mcpServers`). Almost all do; OpenClaw rejects
            // any server entry and fails session creation, so it must receive
            // NONE — neither user-configured servers nor the built-in codeg-mcp
            // companion. A custom agent carries the same flag as a stored
            // declaration the user flips in settings, for the same reason:
            // codeg cannot know whether an arbitrary ACP agent tolerates the
            // field, and one that doesn't fails to connect at all until it is
            // turned off. (The `mcpServers` key itself is always serialized as
            // `[]` by the ACP schema and OpenClaw tolerates the empty list; the
            // gate only guarantees the list stays empty for it.) This is the
            // single chokepoint feeding session/new, session/load, and the
            // load→new fallback, so gating here keeps server entries off the
            // wire on every path. See `AcpAgentMeta::supports_mcp`.
            let agent_supports_mcp = registry::get_agent_meta(agent_type).supports_mcp;

            // Load MCP servers configured for this agent and filter by the
            // capabilities the agent just declared. Stdio is mandatory per
            // ACP spec; HTTP/SSE are gated on `mcp_capabilities.{http,sse}`.
            let mut mcp_servers: Vec<McpServer> = if agent_supports_mcp {
                let mcp_caps = &init_resp.agent_capabilities.mcp_capabilities;
                load_mcp_servers_for_agent(agent_type)
                    .into_iter()
                    .filter(|s| match s {
                        McpServer::Stdio(_) => true,
                        McpServer::Http(server) => {
                            if mcp_caps.http {
                                true
                            } else {
                                tracing::warn!(
                                    "[ACP][{}] skip HTTP MCP server '{}': agent does not advertise mcpCapabilities.http",
                                    agent_type, server.name
                                );
                                false
                            }
                        }
                        McpServer::Sse(server) => {
                            if mcp_caps.sse {
                                true
                            } else {
                                tracing::warn!(
                                    "[ACP][{}] skip SSE MCP server '{}': agent does not advertise mcpCapabilities.sse",
                                    agent_type, server.name
                                );
                                false
                            }
                        }
                        _ => false,
                    })
                    .collect()
            } else {
                tracing::info!(
                    "[ACP][{}] supports_mcp=false: skipping all MCP wire forwarding (user servers + codeg-mcp companion)",
                    agent_type
                );
                Vec::new()
            };

            // Inject the built-in `codeg-mcp` MCP server. Stdio is
            // unconditionally supported by the ACP wire — no `mcp_caps`
            // filter needed. The returned token is stashed on the session
            // state so connection teardown can revoke it. Skipped entirely
            // for agents that don't accept MCP over the wire (above).
            let delegate_injection = if agent_supports_mcp && agent_delivers_wire_mcp(agent_type) {
                if let Some(inj) = delegation_injection.as_ref() {
                    // Task-engine launches (owner label "work_task") carry the
                    // task_progress / task_complete tool group.
                    let tasks_enabled =
                        { state.read().await.owner_window_label == "work_task" };
                    inject_codeg_mcp(
                        &mut mcp_servers,
                        inj,
                        &conn_id,
                        &cwd,
                        tasks_enabled,
                        host_tools,
                    )
                    .await
                } else {
                    None
                }
            } else {
                None
            };
            {
                let mut s = state.write().await;
                // Native steering is independent of the MCP companion — set it
                // even when no codeg-mcp is injected (it's exactly the channel
                // that needs no tool; OpenClaw-style `supports_mcp: false`
                // agents could ship it someday).
                s.native_steering_available = native_steering_available;
                s.neutral_goal_channel = neutral_goal_channel;
                // The vocabulary is decided HERE for every adapter, advertising
                // or not — this assignment is what flips it from "unknown" to
                // known, and a client reading the snapshot before it lands must
                // see `None` rather than a legacy pair it would latch (a claude
                // session offering a Pause the adapter rejects).
                let (goal_method, goal_actions) = resolve_goal_control(goal_control);
                if let Some(method) = goal_method {
                    s.goal_control_method = method;
                }
                s.goal_actions = Some(goal_actions);
                if let Some(ref injected) = delegate_injection {
                    s.delegation_token = Some(injected.token.clone());
                    // The agent's actual feedback capability for this session
                    // — the authoritative gate for submit + UI, fixed at
                    // launch.
                    s.feedback_tool_available = injected.feedback_available;
                }
            }

            // Emit fork support capability
            emit_with_state(
                &state,
                &emitter_clone,
                AcpEvent::ForkSupported {
                    supported: supports_fork,
                },
            )
            .await;

            // Emit connected status early so the frontend can show cached
            // selectors and enable sending while the session initialises.
            // Prompts sent before run_conversation_loop are buffered in
            // the cmd_rx channel and processed as soon as the loop starts.
            emit_with_state(
                &state,
                &emitter_clone,
                AcpEvent::StatusChanged {
                    status: ConnectionStatus::Connected,
                },
            )
            .await;

            if let Some(sid) = session_id {
                // Prefer session/resume when the agent advertises the
                // capability: it restores session context WITHOUT replaying
                // history (which session/load does only for us to drain and
                // discard — the transcript the user sees comes from the disk
                // parser, not the ACP wire). On any non-terminal resume failure
                // we fall through to the session/load block below, so the
                // effective chain is resume → load → new.
                if supports_resume {
                    let resume_req = build_resume_session_request(
                        agent_type,
                        SessionId::new(sid.clone()),
                        &cwd,
                        mcp_servers.clone(),
                    );
                    match send_resume_session(&cx, resume_req).await {
                        Ok((resume_resp, grok_models_raw)) => {
                            let initial_config_options = resume_resp.config_options.clone();
                            let new_resp = NewSessionResponse::new(SessionId::new(sid.clone()))
                                .modes(resume_resp.modes)
                                .config_options(resume_resp.config_options)
                                .meta(resume_resp.meta);
                            let grok_meta = if agent_type == AgentType::Grok {
                                new_resp.meta.clone()
                            } else {
                                None
                            };
                            // Opportunistic: grok may include per-model effort data
                            // on resume; absent ⇒ empty specs ⇒ flat fallback.
                            let grok_model_specs = (agent_type == AgentType::Grok)
                                .then(|| parse_grok_model_specs(grok_models_raw.as_ref()));
                            let mut session = cx.attach_session(new_resp, Default::default())?;

                            // No drain: session/resume does not replay history,
                            // so there is nothing to discard. Any buffered
                            // notification (e.g. an early AvailableCommandsUpdate)
                            // is consumed and forwarded by run_conversation_loop.

                            record_transcript_header(agent_type, &sid, &cwd.to_string_lossy());
                            emit_with_state(
                                &state,
                                &emitter_clone,
                                AcpEvent::SessionStarted {
                                    session_id: sid.clone(),
                                },
                            )
                            .await;
                            emit_session_modes(&state, &emitter_clone, session.modes()).await;
                            apply_and_emit_session_config_options(
                                &cx,
                                &mut session,
                                &state,
                                &emitter_clone,
                                agent_type,
                                grok_meta.as_ref(),
                                grok_model_specs.as_ref(),
                                preferred_mode_id.as_deref(),
                                &preferred_config_values,
                                initial_config_options.unwrap_or_default(),
                            )
                            .await;
                            emit_selectors_ready(&state, &emitter_clone).await;

                            let loop_result = run_conversation_loop(
                                &mut session,
                                &conn_id,
                                &emitter_clone,
                                &state,
                                agent_type,
                                &perms,
                                &mut cmd_rx,
                                terminal_runtime.clone(),
                                &cwd_string,
                                supports_fork,
                                &prompt_ledger,
                                delegation_injection.as_ref(),
                                &stderr_tail,
                            )
                            .await;
                            terminal_runtime.release_all_for_session(&sid).await;
                            drop(session);
                            // Explicit return: this arm is NOT in tail position
                            // (the session/load block follows it), so without
                            // `return` a successful resume would fall into
                            // session/load.
                            return handle_fork_or_exit(
                                loop_result,
                                &conn_id,
                                &emitter_clone,
                                &state,
                                agent_type,
                                &perms,
                                &mut cmd_rx,
                                terminal_runtime.clone(),
                                &cwd,
                                &cwd_string,
                                &prompt_ledger,
                                delegation_injection.as_ref(),
                                &stderr_tail,
                            )
                            .await;
                        }
                        Err(e) => {
                            // resume is unstable and NOT guaranteed equivalent to
                            // session/load, so a resume-specific failure must
                            // never deny a load that might still succeed. EVERY
                            // resume error — ResourceNotFound, "Authentication
                            // required", "Method not found", or anything else —
                            // falls through to the session/load block below,
                            // which already owns all terminal decisions
                            // (SessionLoadFailed for not-found, silent stop for
                            // auth, fallback to session/new otherwise). No
                            // user-facing event is emitted here: load re-derives
                            // the same outcome a moment later, so emitting now
                            // would double up (not-found) or flash a transient
                            // error that self-heals when load succeeds.
                            tracing::warn!(
                                "[ACP] session/resume failed ({e}); falling back to session/load"
                            );
                            // fall through to the session/load block below
                        }
                    }
                }

                // Load existing session via session/load.
                //
                // ACP is explicit that a client MUST NOT send `session/load` to
                // an agent that has not advertised `loadSession` (Zed enforces
                // the same gate). Skipping the RPC lands on exactly the
                // recovery its wire error would have taken — `session/new` plus
                // a `continues_from` link, so a custom agent's conversation
                // still reads as one history — without putting an unsupported
                // method on the wire.
                //
                // Only a declared **false** is trusted. A declared true is not:
                // agents that advertise `loadSession: true` and then answer
                // "Method not found" are real, so the whole error ladder below
                // stays exactly as it was.
                let attempted_load = init_resp.agent_capabilities.load_session;
                let load_result = if attempted_load {
                    let load_req = build_load_session_request(
                        agent_type,
                        SessionId::new(sid.clone()),
                        &cwd,
                        mcp_servers.clone(),
                    );
                    send_load_session(&cx, load_req).await
                } else {
                    Err(sacp::Error::method_not_found()
                        .data("agent does not advertise the loadSession capability"))
                };

                match load_result {
                    Ok(load_resp) => {
                        let initial_config_options = load_resp.config_options.clone();
                        let new_resp = NewSessionResponse::new(SessionId::new(sid.clone()))
                            .modes(load_resp.modes)
                            .config_options(load_resp.config_options)
                            .meta(load_resp.meta);
                        let grok_meta = if agent_type == AgentType::Grok {
                            new_resp.meta.clone()
                        } else {
                            None
                        };
                        let mut session = cx.attach_session(new_resp, Default::default())?;

                        // Drain historical replay notifications from session/load,
                        // but forward AvailableCommandsUpdate to the frontend.
                        //
                        // For a custom agent with no transcript yet — a session
                        // created outside codeg, or one whose recording was
                        // lost — this replay is the ONLY source of its history,
                        // so capture it instead of discarding it. When codeg
                        // already recorded the session live, the replay is a
                        // duplicate and stays drained.
                        let hydrate_from_replay = transcript_dir_for(agent_type).is_some_and(|dir| {
                            !crate::acp_transcript::has_recorded_history(dir, &sid)
                        });
                        if hydrate_from_replay {
                            tracing::info!(
                                "[ACP] hydrating custom agent transcript for {sid} from session/load replay"
                            );
                        }
                        // The header must land BEFORE any replayed entry:
                        // `record_header` is a no-op once the file is non-empty,
                        // so writing it after the drain would leave a hydrated
                        // transcript permanently headerless (no cwd, no start
                        // time, hence no folder in the conversation list).
                        record_transcript_header(agent_type, &sid, &cwd.to_string_lossy());
                        let mut drained = 0u32;
                        // Cleared if the writer ever stalls: from then on the
                        // drain still runs to completion (the session is not
                        // usable until the replay is consumed) but records
                        // nothing more, so the transcript ends at a line
                        // boundary instead of growing holes.
                        let mut recording = hydrate_from_replay;
                        while let Ok(Ok(msg)) = tokio::time::timeout(
                            std::time::Duration::from_millis(100),
                            session.read_update(),
                        )
                        .await
                        {
                            drained += 1;
                            if let SessionMessage::SessionMessage(dispatch) = msg {
                                let h = emitter_clone.clone();
                                let st = Arc::clone(&state);
                                let dispatch = fix_usage_update_nulls(dispatch);
                                let _ = MatchDispatch::new(dispatch)
                                    .if_notification(async |notif: SessionNotification| {
                                        if recording {
                                            recording = record_hydrated_update(
                                                agent_type,
                                                &sid,
                                                &notif.update,
                                            )
                                            .await;
                                        }
                                        if matches!(
                                            notif.update,
                                            SessionUpdate::AvailableCommandsUpdate(_)
                                        ) {
                                            // Historical-replay path only
                                            // forwards AvailableCommandsUpdate,
                                            // which never carries tool output or
                                            // tool-call titles — throwaway state
                                            // is fine.
                                            let mut replay_cache =
                                                ToolCallOutputCache::default();
                                            let mut replay_cb_state =
                                                CodeBuddyLiveState::default();
                                            emit_conversation_update(
                                                &st,
                                                &h,
                                                agent_type,
                                                notif.update,
                                                None,
                                                &mut replay_cache,
                                                &mut replay_cb_state,
                                            )
                                            .await;
                                        }
                                        Ok(())
                                    })
                                    .await
                                    .otherwise(async |dispatch| {
                                        // Historical replay: throwaway state,
                                        // mirroring the sibling closure above.
                                        // An ext notification that raises an
                                        // ALERT is skipped, though — a
                                        // compaction failure or a dropped image
                                        // recorded in a past session is not
                                        // happening now, and that path also
                                        // fires an OS notification. The typed
                                        // closure above draws the same line by
                                        // forwarding only AvailableCommands.
                                        let mut replay_cb_state =
                                            CodeBuddyLiveState::default();
                                        if !grok_ext_notification_is_alert(&dispatch, agent_type) {
                                            maybe_emit_ext_notification(&st, &h, agent_type, dispatch, &mut replay_cb_state).await;
                                        }
                                        Ok(())
                                    })
                                    .await;
                            }
                        }
                        if drained > 0 {
                            tracing::info!("[ACP] Drained {drained} historical replay notifications");
                        }

                        emit_with_state(
                            &state,
                            &emitter_clone,
                            AcpEvent::SessionStarted {
                                session_id: sid.clone(),
                            },
                        )
                        .await;
                        emit_session_modes(&state, &emitter_clone, session.modes()).await;
                        apply_and_emit_session_config_options(
                            &cx,
                            &mut session,
                            &state,
                            &emitter_clone,
                            agent_type,
                            grok_meta.as_ref(),
                            // `session/load` is a typed send with no raw `models`
                            // capture, so effort stays on the flat fallback.
                            None,
                            preferred_mode_id.as_deref(),
                            &preferred_config_values,
                            initial_config_options.unwrap_or_default(),
                        )
                        .await;
                        emit_selectors_ready(&state, &emitter_clone).await;

                        let loop_result = run_conversation_loop(
                            &mut session,
                            &conn_id,
                            &emitter_clone,
                            &state,
                            agent_type,
                            &perms,
                            &mut cmd_rx,
                            terminal_runtime.clone(),
                            &cwd_string,
                            supports_fork,
                            &prompt_ledger,
                            delegation_injection.as_ref(),
                            &stderr_tail,
                        )
                        .await;
                        terminal_runtime.release_all_for_session(&sid).await;
                        drop(session);
                        handle_fork_or_exit(
                            loop_result,
                            &conn_id,
                            &emitter_clone,
                            &state,
                            agent_type,
                            &perms,
                            &mut cmd_rx,
                            terminal_runtime.clone(),
                            &cwd,
                            &cwd_string,
                            &prompt_ledger,
                            delegation_injection.as_ref(),
                            &stderr_tail,
                        )
                        .await
                    }
                    Err(e) => {
                        // session/load failed. Classify it: an unrecoverable
                        // historical session — the agent has no record of it
                        // (ResourceNotFound, -32002) or the agent process/session
                        // died mid-load (Claude 0.58.1 reports this as a -32603
                        // Internal error, not -32002) — is surfaced to the
                        // frontend as SessionLoadFailed so the user can choose
                        // Reload vs New conversation. It is NOT auto-fallen-back
                        // to session/new, which would silently orphan the
                        // historical context (and, on a dead process, fail anyway
                        // and leak a raw protocol error). Every other failure
                        // keeps the session/new fallback below.
                        //
                        // Custom agents are the exception: their history is
                        // codeg's own transcript, not the agent's store, so
                        // "the agent forgot this session" costs nothing the
                        // user can see. Many custom agents keep sessions in
                        // memory only, which would make the banner appear on
                        // every restart of every conversation. They fall
                        // through to session/new instead, and the new
                        // transcript links back to the old one so the history
                        // reads as one conversation.
                        let err_str = e.to_string();
                        let forgotten_session = classify_session_load_failure(e.code, &err_str);
                        let recovers_locally =
                            recovers_load_failure_locally(agent_type, forgotten_session);
                        if let Some(code) = forgotten_session.filter(|_| !recovers_locally) {
                            tracing::warn!(
                                "[ACP] session/load failed ({err_str}); surfacing as session_load_failed={code}"
                            );
                            emit_with_state(
                                &state,
                                &emitter_clone,
                                AcpEvent::SessionLoadFailed {
                                    session_id: sid.clone(),
                                    message: err_str,
                                    code: code.to_string(),
                                },
                            )
                            .await;
                            emit_with_state(
                                &state,
                                &emitter_clone,
                                AcpEvent::StatusChanged {
                                    status: ConnectionStatus::Error,
                                },
                            )
                            .await;
                            return Ok(());
                        }
                        if attempted_load {
                            tracing::warn!(
                                "[ACP] session/load failed ({err_str}), falling back to session/new"
                            );
                        } else {
                            tracing::info!(
                                "[ACP] agent declares no loadSession support; opening a new session \
                                 for {sid} and linking its history instead of calling session/load"
                            );
                        }
                        // Only emit a visible error for unexpected failures;
                        // "Method not found" is expected for agents that don't
                        // support session resume (e.g. Cline).
                        // "Authentication required" is expected for agents whose
                        // credentials have expired (e.g. Gemini CLI) — skip
                        // session/new too since it will also fail.
                        if err_str.contains("Authentication required") {
                            return Ok(());
                        }
                        // An agent that simply forgot a session codeg recorded
                        // itself is the expected steady state after a restart,
                        // not an incident — an error toast on every reopen
                        // would be pure noise.
                        // A load codeg deliberately never sent is not a failure
                        // to report — the capability gate above is the expected
                        // path for agents that don't implement it.
                        if attempted_load && !err_str.contains("Method not found") && !recovers_locally
                        {
                            emit_with_state(
                                &state,
                                &emitter_clone,
                                AcpEvent::Error {
                                    message: format!("Failed to load session, starting new: {e}"),
                                    agent_type: agent_type.to_string(),
                                    code: None,
                                    details: None,
                                    // Recoverable: we fall through to `session/new`
                                    // below. Connection stays alive.
                                    terminal: false,
                                },
                            )
                            .await;
                        }
                        let (new_resp, grok_models_raw) = send_new_session_capturing_models(
                            &cx,
                            agent_type,
                            build_new_session_request(agent_type, &cwd, mcp_servers.clone()),
                        )
                        .await
                        .map_err(|e| tag_mcp_suspect(e, agent_type, &mcp_servers))?;
                        let fallback_sid = new_resp.session_id.0.to_string();
                        let initial_config_options = new_resp.config_options.clone();
                        let grok_meta = if agent_type == AgentType::Grok {
                            new_resp.meta.clone()
                        } else {
                            None
                        };
                        let grok_model_specs = (agent_type == AgentType::Grok)
                            .then(|| parse_grok_model_specs(grok_models_raw.as_ref()));
                        let mut session = cx.attach_session(new_resp, Default::default())?;
                        // Same conversation, new agent session: link the fresh
                        // transcript to the one the failed load was for, so the
                        // turns codeg already recorded keep rendering.
                        // Awaited: the link must be on disk before
                        // `SessionStarted` goes out, or the session-binding
                        // guard can read an empty chain and split this
                        // conversation into a permanent duplicate.
                        record_transcript_header_continuing(
                            agent_type,
                            &fallback_sid,
                            &cwd.to_string_lossy(),
                            Some(sid.as_str()),
                        )
                        .await;
                        emit_with_state(
                            &state,
                            &emitter_clone,
                            AcpEvent::SessionStarted {
                                session_id: fallback_sid.clone(),
                            },
                        )
                        .await;
                        emit_session_modes(&state, &emitter_clone, session.modes()).await;
                        apply_and_emit_session_config_options(
                            &cx,
                            &mut session,
                            &state,
                            &emitter_clone,
                            agent_type,
                            grok_meta.as_ref(),
                            grok_model_specs.as_ref(),
                            preferred_mode_id.as_deref(),
                            &preferred_config_values,
                            initial_config_options.unwrap_or_default(),
                        )
                        .await;
                        emit_selectors_ready(&state, &emitter_clone).await;

                        let loop_result = run_conversation_loop(
                            &mut session,
                            &conn_id,
                            &emitter_clone,
                            &state,
                            agent_type,
                            &perms,
                            &mut cmd_rx,
                            terminal_runtime.clone(),
                            &cwd_string,
                            supports_fork,
                            &prompt_ledger,
                            delegation_injection.as_ref(),
                            &stderr_tail,
                        )
                        .await;
                        terminal_runtime
                            .release_all_for_session(&fallback_sid)
                            .await;
                        drop(session);
                        handle_fork_or_exit(
                            loop_result,
                            &conn_id,
                            &emitter_clone,
                            &state,
                            agent_type,
                            &perms,
                            &mut cmd_rx,
                            terminal_runtime.clone(),
                            &cwd,
                            &cwd_string,
                            &prompt_ledger,
                            delegation_injection.as_ref(),
                            &stderr_tail,
                        )
                        .await
                    }
                }
            } else {
                // Create new session
                let (new_resp, grok_models_raw) = send_new_session_capturing_models(
                    &cx,
                    agent_type,
                    build_new_session_request(agent_type, &cwd, mcp_servers.clone()),
                )
                .await
                .map_err(|e| tag_mcp_suspect(e, agent_type, &mcp_servers))?;
                let sid = new_resp.session_id.0.to_string();
                let initial_config_options = new_resp.config_options.clone();
                let grok_meta = if agent_type == AgentType::Grok {
                    new_resp.meta.clone()
                } else {
                    None
                };
                let grok_model_specs = (agent_type == AgentType::Grok)
                    .then(|| parse_grok_model_specs(grok_models_raw.as_ref()));
                let mut session = cx.attach_session(new_resp, Default::default())?;
                record_transcript_header(agent_type, &sid, &cwd.to_string_lossy());
                emit_with_state(
                    &state,
                    &emitter_clone,
                    AcpEvent::SessionStarted {
                        session_id: sid.clone(),
                    },
                )
                .await;
                emit_session_modes(&state, &emitter_clone, session.modes()).await;
                apply_and_emit_session_config_options(
                    &cx,
                    &mut session,
                    &state,
                    &emitter_clone,
                    agent_type,
                    grok_meta.as_ref(),
                    grok_model_specs.as_ref(),
                    preferred_mode_id.as_deref(),
                    &preferred_config_values,
                    initial_config_options.unwrap_or_default(),
                )
                .await;
                emit_selectors_ready(&state, &emitter_clone).await;

                let loop_result = run_conversation_loop(
                    &mut session,
                    &conn_id,
                    &emitter_clone,
                    &state,
                    agent_type,
                    &perms,
                    &mut cmd_rx,
                    terminal_runtime.clone(),
                    &cwd_string,
                    supports_fork,
                    &prompt_ledger,
                    delegation_injection.as_ref(),
                    &stderr_tail,
                )
                .await;
                terminal_runtime.release_all_for_session(&sid).await;
                drop(session);
                handle_fork_or_exit(
                    loop_result,
                    &conn_id,
                    &emitter_clone,
                    &state,
                    agent_type,
                    &perms,
                    &mut cmd_rx,
                    terminal_runtime.clone(),
                    &cwd,
                    &cwd_string,
                    &prompt_ledger,
                    delegation_injection.as_ref(),
                    &stderr_tail,
                )
                .await
            }
        })
        .await
        .map_err(|e| {
            let raw = e.to_string();
            if raw.contains(INIT_TIMEOUT_SENTINEL) {
                AcpError::InitializeTimeout
            } else if raw.contains(MCP_SUSPECT_SENTINEL) {
                // Strip the marker so the user sees the agent's own words, then
                // let the frontend append the `supports_mcp` suggestion.
                AcpError::mcp_rejected(raw.replace(MCP_SUSPECT_SENTINEL, ""))
            } else {
                AcpError::protocol(raw)
            }
        })
}

/// Store the permission responder and emit event to frontend.
/// Grok's native `ask_user_question` tool issues this ACP ext request
/// (`_x.ai/ask_user_question`) and BLOCKS on the reply — it does NOT go through
/// the codeg-mcp ask tool. Transparent over the raw params object
/// (`{sessionId, toolCallId, questions, mode}`); the fields codeg needs are read
/// by [`crate::acp::question::parse_grok_ext_questions`]. sacp routes typed
/// handlers on the RAW wire method, so the derive keeps the leading `_`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonRpcRequest)]
#[request(method = "_x.ai/ask_user_question", response = serde_json::Value)]
#[serde(transparent)]
struct GrokAskUserQuestionRequest(serde_json::Value);

/// Store the plan-approval responder and render the approval card. Grok's native
/// `exit_plan_mode` tool issues this ACP ext request (`_x.ai/exit_plan_mode`) and
/// BLOCKS on the reply — the agent won't leave plan mode until the user acts.
/// Transparent over the raw params object (`{sessionId, toolCallId, planContent}`);
/// the fields codeg needs are read by
/// [`crate::acp::plan_approval::parse_grok_exit_plan_request`]. sacp routes typed
/// handlers on the RAW wire method, so the derive keeps the leading `_`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonRpcRequest)]
#[request(method = "_x.ai/exit_plan_mode", response = serde_json::Value)]
#[serde(transparent)]
struct GrokExitPlanModeRequest(serde_json::Value);

/// Every codex `elicitation/create` request — `request_user_input` (Plan
/// mode), generic MCP-server forms, MCP tool-call approvals, message-only
/// confirms — arrives here once codeg advertises `elicitation.form`. sacp
/// 11.0.0 ships no `JsonRpcRequest`/`JsonRpcResponse` impl for the schema's
/// elicitation types (and no feature to enable them), so — like the grok bridge
/// — take the raw params object and reply with a raw JSON value (the serialized
/// `CreateElicitationResponse`). sacp has no built-in elicitation handling, so
/// this custom method handler fills the gap with no dispatch conflict.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonRpcRequest)]
#[request(method = "elicitation/create", response = serde_json::Value)]
#[serde(transparent)]
struct CodexElicitationRequest(serde_json::Value);

/// Bridge grok's native `_x.ai/ask_user_question` ext request into codeg's
/// interactive question card. Grok blocks on the reply, so codeg registers the
/// questions through the shared [`crate::acp::question::SessionQuestionAccess`] —
/// the SAME path the codeg-mcp ask tool uses (it sets `pending_question`,
/// broadcasts `QuestionRequest`, and the `AskQuestionCard` renders) — then answers
/// the ext request with the user's choice, serialized to grok's own format, once
/// they submit. Every early return responds with an error, which makes grok fall
/// back to its inert fire-and-forget rendering — exactly the pre-bridge behavior,
/// so no path here can regress it.
async fn handle_grok_ask_user_question(
    access: &Option<(
        Arc<dyn crate::acp::question::SessionQuestionAccess>,
        crate::acp::question::QuestionRuntimeConfig,
    )>,
    connection_id: &str,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    req: GrokAskUserQuestionRequest,
    responder: Responder<serde_json::Value>,
) {
    let Some((questions, ask_cfg)) = access else {
        let _ = responder.respond_with_internal_error("ask_user_question bridge unavailable");
        return;
    };
    // Same kill switch as the codeg-mcp ask tool: when off, let grok fall back.
    if !ask_cfg.is_enabled().await {
        let _ = responder.respond_with_internal_error("ask_user_question is disabled");
        return;
    }
    let specs = match crate::acp::question::parse_grok_ext_questions(&req.0) {
        Ok(specs) => specs,
        Err(e) => {
            tracing::warn!("[grok ask] rejecting malformed ext request: {e}");
            let _ =
                responder.respond_with_internal_error(format!("invalid ask_user_question: {e}"));
            return;
        }
    };
    // Grok's tool_call_id correlates this ext ask with the (suppressed) native
    // tool_call in the live stream; reuse it so the synthesized result card is the
    // single card for that id. Absent → still answer grok, just skip the card.
    let tool_call_id = req
        .0
        .get("toolCallId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    // register_question consumes the specs; keep a copy to render the answered
    // in-stream card once the user submits.
    let card_specs = specs.clone();
    let Some(registered) = questions.register_question(connection_id, specs).await else {
        // Connection gone, or an ask is already pending on this connection.
        let _ = responder.respond_with_internal_error("could not register ask_user_question");
        return;
    };
    // The user answers out-of-band (the HTTP `answer_question` endpoint resolves
    // the one-shot below), so await it on a task — keeping the ACP dispatch loop
    // free — then reply to grok's blocked ext request.
    let state = Arc::clone(state);
    let emitter = emitter.clone();
    tokio::spawn(async move {
        match registered.answer_rx.await {
            Ok(outcome) => {
                // Surface the answered "提问回答" capsule in-stream — the codeg-mcp
                // ask parity grok's native tool never emits into the ACP stream (it
                // resolves the answer over THIS ext round-trip). Emit BEFORE
                // unblocking grok so the card lands ahead of grok's follow-up text;
                // grok is blocked on this reply, so nothing races the emit. The
                // matching raw ask tool_call/updates are suppressed in the live loop
                // (see `grok_ask_tool_ids`), so this synthesized event — keyed by the
                // same id — is the only card for the ask.
                if let Some(tool_call_id) = tool_call_id {
                    emit_with_state(
                        &state,
                        &emitter,
                        AcpEvent::ToolCall {
                            tool_call_id,
                            title: "ask_user_question".to_string(),
                            kind: "other".to_string(),
                            status: "completed".to_string(),
                            content: None,
                            raw_input: Some(
                                crate::acp::question::grok_result_card_input(&card_specs)
                                    .to_string(),
                            ),
                            raw_output: Some(
                                crate::acp::question::grok_result_card_output(&outcome).to_string(),
                            ),
                            locations: None,
                            meta: None,
                            images: None,
                        },
                    )
                    .await;
                }
                let _ = responder.respond(crate::acp::question::build_grok_ext_response(&outcome));
            }
            // Sender dropped: the ask was canceled or the connection tore down —
            // nothing to render; let grok fall back via skip_interview.
            Err(_) => {
                let _ = responder.respond(crate::acp::question::grok_ext_skip_response());
            }
        }
    });
}

/// Bridge grok's native `_x.ai/exit_plan_mode` ext request into codeg's
/// interactive plan-approval card. Grok BLOCKS on the reply — it won't leave plan
/// mode until the user acts — so codeg registers the approval through the shared
/// [`crate::acp::plan_approval::SessionPlanApprovalAccess`] (which sets
/// `pending_plan_approval`, broadcasts `PlanApprovalRequest`, and renders the card
/// above the composer), then answers the ext request with the user's decision once
/// they submit. Unlike the ask bridge there is no synthesized in-stream card:
/// grok's own `exit_plan_mode` tool_call renders the plan in the transcript (via
/// `PlanModeCard`), mirroring how the permission dialog coexists with the tool
/// call. Every early return replies with the disconnect-shaped response so grok
/// keeps plan mode active — it can never be read as a silent approval.
async fn handle_grok_exit_plan_mode(
    access: &Option<Arc<dyn crate::acp::plan_approval::SessionPlanApprovalAccess>>,
    connection_id: &str,
    req: GrokExitPlanModeRequest,
    responder: Responder<serde_json::Value>,
) {
    // Log the wire SHAPE (top-level field names), not the raw request — the plan
    // body can be large and carry file paths / source. The keys are what
    // wire-format verification needs (confirm `sessionId`/`toolCallId`/`planContent`
    // on the first real run); the malformed path below logs more if parsing fails.
    tracing::info!(
        "[grok exit_plan] received _x.ai/exit_plan_mode ext request: keys={:?}",
        req.0
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect::<Vec<_>>())
    );
    let Some(access) = access else {
        let _ =
            responder.respond(crate::acp::plan_approval::grok_exit_plan_disconnect_response());
        return;
    };
    let (plan_markdown, tool_call_id) =
        match crate::acp::plan_approval::parse_grok_exit_plan_request(&req.0) {
            Ok(parsed) => parsed,
            Err(e) => {
                tracing::warn!("[grok exit_plan] rejecting malformed ext request: {e}");
                let _ = responder
                    .respond(crate::acp::plan_approval::grok_exit_plan_disconnect_response());
                return;
            }
        };
    tracing::info!(
        "[grok exit_plan] toolCallId={tool_call_id:?} plan_chars={}",
        plan_markdown.chars().count()
    );
    let Some(registered) = access
        .register_plan_approval(connection_id, tool_call_id, plan_markdown)
        .await
    else {
        // Connection gone, or an approval is already pending on this connection.
        let _ =
            responder.respond(crate::acp::plan_approval::grok_exit_plan_disconnect_response());
        return;
    };
    // The user answers out-of-band (the HTTP `answer_plan_approval` endpoint
    // resolves the one-shot below), so await it on a task — keeping the ACP
    // dispatch loop free — then reply to grok's blocked ext request. The manager's
    // `answer_plan_approval` / teardown emit `PlanApprovalResolved` to clear the
    // card; this task only unblocks grok.
    tokio::spawn(async move {
        match registered.answer_rx.await {
            Ok(answer) => {
                let _ = responder.respond(
                    crate::acp::plan_approval::build_grok_exit_plan_response(&answer),
                );
            }
            // Sender dropped: the approval was canceled or the connection tore
            // down — reply disconnect so grok keeps plan mode active.
            Err(_) => {
                let _ = responder
                    .respond(crate::acp::plan_approval::grok_exit_plan_disconnect_response());
            }
        }
    });
}

/// Bridge codex's `elicitation/create` requests into codeg's interactive
/// surfaces. Codex only sends these when codeg declares `elicitation.form`
/// (see `connect_with`), then BLOCKS on the reply, so every shape must resolve
/// to something the user can act on (see
/// [`crate::acp::question::classify_elicitation`] for the full taxonomy):
///
///   * Question-style (Plan-mode `request_user_input`, generic MCP forms) →
///     the shared [`crate::acp::question::SessionQuestionAccess`] path — the
///     SAME one the codeg-mcp ask tool and the grok bridge use (it sets
///     `pending_question`, broadcasts `QuestionRequest`, and `AskQuestionCard`
///     renders) — answered once the user submits.
///   * Approval-style (MCP tool-call approvals, message-only confirms) → the
///     permission card via `pending_perms`, exactly like the
///     `session/request_permission` fallback codex-acp used before the
///     capability was advertised. Auto-declining these would reject the tool
///     call (including codeg-mcp's own tools in consent-requiring modes).
///
/// Question-path early returns DECLINE, which makes codex proceed with its own
/// judgment — no worse than the pre-bridge `{answers:{}}`, so nothing here can
/// regress it. Codex delivers `request_user_input` ONLY as this elicitation and
/// never puts a completed tool_call on the stream, so — like the grok bridge —
/// the question path synthesizes the answered result card itself once the user
/// submits (keyed by the elicitation's tool_call_id).
#[allow(clippy::too_many_arguments)]
async fn handle_elicitation_request(
    access: &Option<(
        Arc<dyn crate::acp::question::SessionQuestionAccess>,
        crate::acp::question::QuestionRuntimeConfig,
    )>,
    perms: &PendingPermissions,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    connection_id: &str,
    req: CodexElicitationRequest,
    responder: Responder<serde_json::Value>,
) {
    // The wire reply is the serialized `CreateElicitationResponse` (see the
    // newtype above). `Decline` makes codex proceed with its own judgment.
    fn decline() -> serde_json::Value {
        serde_json::to_value(crate::acp::question::elicitation_decline_response())
            .unwrap_or_default()
    }
    let raw = req.0;
    // Everything codex-acp can send once `elicitation.form` is advertised
    // resolves to a plan here — an unhandled shape would silently reject the
    // agent's blocked request (an MCP tool-call approval, most damagingly).
    let plan = match crate::acp::question::classify_elicitation(&raw) {
        Ok(plan) => plan,
        Err(e) => {
            tracing::warn!("[codex elicitation] declining unrenderable request: {e}");
            let _ = responder.respond(decline());
            return;
        }
    };
    match plan {
        // Approval-style (MCP tool-call approval / message-only confirm):
        // render through the permission card — the exact surface these used
        // before codeg advertised `elicitation.form` (codex-acp then sent
        // `session/request_permission`). Deliberately NOT gated by the
        // ask_user_question toggle: this is consent, not an agent question,
        // and auto-declining would reject the tool call outright.
        crate::acp::question::ElicitationPlan::Approval(approval) => {
            let request_id = uuid::Uuid::new_v4().to_string();
            // Mirror codex-acp's own `request_permission` fallback tool_call
            // shape (`buildPermissionRequest`) so the frontend permission card
            // renders it identically. When codex correlated the approval to an
            // already-rendered mcpToolCall item, reuse that id so the card
            // attaches to it.
            let tool_call = serde_json::json!({
                "toolCallId": approval
                    .tool_call_id
                    .clone()
                    .unwrap_or_else(|| format!("elicitation-{request_id}")),
                "title": approval.message,
                "kind": "execute",
                "status": "pending",
                "content": [{
                    "type": "content",
                    "content": {"type": "text", "text": approval.message},
                }],
            });
            let options: Vec<PermissionOptionInfo> = approval
                .options
                .iter()
                .map(|o| PermissionOptionInfo {
                    option_id: o.option_id.clone(),
                    name: o.label.clone(),
                    kind: o.kind.to_string(),
                    // Synthesized from an elicitation form, not from ACP
                    // `PermissionOption`s — there is no wire `_meta` to forward.
                    meta: None,
                })
                .collect();
            admit_permission(
                perms,
                state,
                emitter,
                PendingPermission::CodexElicitation {
                    responder,
                    approval,
                },
                QueuedPermission {
                    request_id,
                    tool_call,
                    options,
                },
            )
            .await;
        }
        // Question-style (codex `request_user_input`, generic MCP forms):
        // bridge into the same ask card as the codeg-mcp ask tool.
        crate::acp::question::ElicitationPlan::Questions(questions) => {
            let Some((question_access, ask_cfg)) = access else {
                let _ = responder.respond(decline());
                return;
            };
            // Same kill switch as the codeg-mcp ask tool and the grok bridge:
            // when the user has turned ask_user_question off, decline so codex
            // proceeds.
            if !ask_cfg.is_enabled().await {
                let _ = responder.respond(decline());
                return;
            }
            // register_question consumes the specs; `questions` keeps its copy
            // to correlate the answer back to each field when building the
            // response.
            let Some(registered) = question_access
                .register_question(connection_id, questions.specs.clone())
                .await
            else {
                // Connection gone, or an ask is already pending on this connection.
                let _ = responder.respond(decline());
                return;
            };
            // Codex advertises an auto-resolution timeout on some
            // `request_user_input` asks (`_meta.codex.autoResolutionMs`):
            // codex-acp races the elicitation against it and answers
            // `{answers: {}}` itself on expiry, ABANDONING this request. Reap
            // the by-then-pointless card shortly after so it can't linger as a
            // zombie; `cancel_question` is a no-op if the user already
            // answered.
            if let Some(ms) = crate::acp::question::elicitation_auto_resolution_ms(&raw) {
                let reaper_access = Arc::clone(question_access);
                let reaper_conn = connection_id.to_string();
                let reaper_qid = registered.question_id.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        ms.saturating_add(2_000),
                    ))
                    .await;
                    reaper_access.cancel_question(&reaper_conn, &reaper_qid).await;
                });
            }
            // The user answers out-of-band (the `answer_question` endpoint
            // resolves the one-shot below), so await it on a task — keeping
            // the ACP dispatch loop free — then reply to codex's blocked
            // elicitation request. Cloned here so the synthesized card below can
            // write through the session state from the spawned task.
            let card_state = Arc::clone(state);
            let card_emitter = emitter.clone();
            tokio::spawn(async move {
                let response = match registered.answer_rx.await {
                    Ok(outcome) => {
                        // Surface the answered "提问回答" capsule in-stream — the
                        // parity codex's `request_user_input` never emits itself: it
                        // resolves the answer over THIS elicitation round-trip and
                        // puts no completed tool_call on the ACP stream (so the live
                        // message has nothing to render otherwise). Emit BEFORE
                        // unblocking codex so the card lands ahead of its follow-up
                        // text; codex is blocked on this reply, so nothing races the
                        // emit. Keyed by the elicitation's tool_call_id so this is
                        // the single card for the ask and the reloaded history card
                        // (`codex.rs`, same id) replaces rather than duplicates it.
                        if let Some(tool_call_id) = questions.tool_call_id.clone() {
                            emit_with_state(
                                &card_state,
                                &card_emitter,
                                AcpEvent::ToolCall {
                                    tool_call_id,
                                    title: "request_user_input".to_string(),
                                    kind: "other".to_string(),
                                    status: "completed".to_string(),
                                    content: None,
                                    raw_input: Some(
                                        crate::acp::question::grok_result_card_input(
                                            &questions.specs,
                                        )
                                        .to_string(),
                                    ),
                                    raw_output: Some(
                                        crate::acp::question::grok_result_card_output(&outcome)
                                            .to_string(),
                                    ),
                                    locations: None,
                                    meta: None,
                                    images: None,
                                },
                            )
                            .await;
                        }
                        crate::acp::question::build_elicitation_response(&questions, &outcome)
                    }
                    // Sender dropped: canceled or the connection tore down.
                    // Decline so codex proceeds with its own judgment.
                    Err(_) => crate::acp::question::elicitation_decline_response(),
                };
                let _ = responder.respond(serde_json::to_value(response).unwrap_or_default());
            });
        }
    }
}

async fn handle_permission_request(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    perms: &PendingPermissions,
    cwd: &str,
    agent_type: AgentType,
    req: RequestPermissionRequest,
    responder: Responder<RequestPermissionResponse>,
) {
    let request_id = uuid::Uuid::new_v4().to_string();

    // Codex Plan-mode review gate: seed the tool call codex never announced, so
    // its follow-up `tool_call_update` (status + rawOutput only) merges into a
    // card that has an identity instead of creating an untitled orphan. See
    // `is_codex_plan_review`. `raw_input` is deliberately omitted: the plan text
    // is already in the transcript (codex emits the plan as an
    // `agent_message_chunk` before this) and the permission card renders it from
    // the request's own `rawInput.plan` — a third copy would be noise.
    if is_codex_plan_review(agent_type, req.meta.as_ref()) {
        emit_with_state(
            state,
            emitter,
            AcpEvent::ToolCall {
                tool_call_id: req.tool_call.tool_call_id.to_string(),
                title: req.tool_call.fields.title.clone().unwrap_or_default(),
                kind: "switch_mode".to_string(),
                status: "pending".to_string(),
                content: None,
                raw_input: None,
                raw_output: None,
                locations: None,
                meta: req
                    .meta
                    .as_ref()
                    .map(|m| serde_json::Value::Object(m.clone())),
                images: None,
            },
        )
        .await;
    }

    let options: Vec<PermissionOptionInfo> = req
        .options
        .iter()
        .map(|opt| PermissionOptionInfo {
            option_id: opt.option_id.to_string(),
            name: opt.name.clone(),
            kind: match opt.kind {
                PermissionOptionKind::AllowOnce => "allow_once".into(),
                PermissionOptionKind::AllowAlways => "allow_always".into(),
                PermissionOptionKind::RejectOnce => "reject_once".into(),
                PermissionOptionKind::RejectAlways => "reject_always".into(),
                _ => "unknown".into(),
            },
            // Opaque passthrough — the frontend reads codex-acp ≥1.1.8's
            // `_meta.permission.changes[].description` off this.
            meta: opt
                .meta
                .as_ref()
                .map(|m| serde_json::Value::Object(m.clone())),
        })
        .collect();

    let mut tool_call_value = serde_json::to_value(&req.tool_call).unwrap_or_default();

    // Resolve line numbers in rawInput for edit tool permission requests
    if let Some(obj) = tool_call_value.as_object_mut() {
        let key = ["rawInput", "raw_input"]
            .into_iter()
            .find(|k| obj.contains_key(*k));
        if let Some(key) = key {
            match obj.get_mut(key) {
                // rawInput is a JSON object: inject _start_line in place
                Some(v) if v.is_object() => {
                    inject_start_line(v, Some(cwd));
                }
                // rawInput is a JSON string: parse, inject, write back as object
                Some(serde_json::Value::String(text)) => {
                    let text = text.clone();
                    if let Ok(mut parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                        if inject_start_line(&mut parsed, Some(cwd)) {
                            obj.insert(key.to_string(), parsed);
                        }
                    } else if text.contains("@@\n") || text.contains("@@\r\n") {
                        if let Some(resolved) = crate::parsers::resolve_patch_text(&text, Some(cwd))
                        {
                            obj.insert(key.to_string(), serde_json::Value::String(resolved));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    admit_permission(
        perms,
        state,
        emitter,
        PendingPermission::Acp(responder),
        QueuedPermission {
            request_id,
            tool_call: tool_call_value,
            options,
        },
    )
    .await;
}

fn respond_terminal_request<T: sacp::JsonRpcResponse>(
    responder: Responder<T>,
    result: Result<T, TerminalRuntimeError>,
) -> Result<(), sacp::Error> {
    match result {
        Ok(response) => responder.respond(response),
        Err(error) => responder.respond_with_error(error.into_rpc_error()),
    }
}

/// Refuse a channel this launch never advertised
/// ([`HostToolsPolicy::Agent`], #436). Withholding the capability is a
/// DECLARATION; a non-conforming agent can still call the method, and if codeg
/// then served it the whole switch would be a suggestion — the operation would
/// land back in codeg's process, outside the agent's sandbox, which is exactly
/// the bug. `method_not_found` is the honest wire answer: as far as this
/// connection is concerned the method does not exist, which is what the agent
/// was told on Initialize.
fn refuse_unadvertised_channel<T: sacp::JsonRpcResponse>(
    responder: Responder<T>,
    method: &str,
) -> Result<(), sacp::Error> {
    tracing::warn!(
        "[ACP] refusing {method}: {HOST_TOOLS_ENV}=agent, so this channel was never \
         advertised — the agent must use its own (sandboxable) tools"
    );
    responder.respond_with_error(unadvertised_channel_error(method))
}

/// The error [`refuse_unadvertised_channel`] answers with. Split out because a
/// `Responder` cannot be built outside a live connection, so this is the part
/// of the refusal a unit test can pin; the wiring itself is covered end-to-end.
fn unadvertised_channel_error(method: &str) -> sacp::Error {
    sacp::Error::method_not_found().data(format!(
        "codeg does not host {method} for this agent ({HOST_TOOLS_ENV}=agent)"
    ))
}

fn respond_file_system_request<T: sacp::JsonRpcResponse>(
    responder: Responder<T>,
    result: Result<T, FileSystemRuntimeError>,
) -> Result<(), sacp::Error> {
    match result {
        Ok(response) => responder.respond(response),
        Err(error) => responder.respond_with_error(error.into_rpc_error()),
    }
}

async fn set_session_mode(
    session: &mut sacp::ActiveSession<'_, Agent>,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    mode_id: String,
) -> Result<(), sacp::Error> {
    let req = SetSessionModeRequest::new(session.session_id().clone(), mode_id.clone());
    session
        .connection()
        .send_request_to(Agent, req)
        .block_task()
        .await?;

    emit_with_state(state, emitter, AcpEvent::ModeChanged { mode_id }).await;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn set_session_config_option(
    cx: &ConnectionTo<Agent>,
    session_id: &SessionId,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    agent_type: AgentType,
    config_id: String,
    value_id: String,
) -> Result<(), sacp::Error> {
    // The whole selector transport carries values as opaque strings; only here,
    // at the wire, does the option's advertised kind decide how to encode it.
    let is_boolean = state
        .read()
        .await
        .config_options
        .as_ref()
        .and_then(|opts| opts.iter().find(|o| o.id == config_id))
        .is_some_and(|o| matches!(o.kind, SessionConfigKindInfo::Boolean(_)));
    let value = encode_config_option_value(is_boolean, &value_id);
    let updated =
        set_session_config_option_inner(cx, session_id, config_id.clone(), value).await?;
    // Compare BEFORE emitting: the agent's answer is the only place a request and
    // its outcome are correlated. Once the option list is broadcast it is
    // indistinguishable from an unsolicited update.
    if let Some(rejection) =
        config_option_rejection(&map_session_config_options(&updated), &config_id, &value_id)
    {
        emit_with_state(state, emitter, rejection).await;
    }
    emit_session_config_options_values(state, emitter, agent_type, updated).await;
    Ok(())
}

/// Build a [`AcpEvent::ConfigOptionRejected`] when the agent's answer settled the
/// option somewhere other than where the request asked.
///
/// Takes the mapped form so grouped and ungrouped selects are already flattened
/// into one value list. Returns `None` when the pick was honoured, when the agent
/// didn't advertise the option at all (nothing to compare against), or for a
/// non-select kind — silence is the right default, since a spurious "your pick was
/// changed" notice is worse than none.
fn config_option_rejection(
    updated: &[SessionConfigOptionInfo],
    config_id: &str,
    requested: &str,
) -> Option<AcpEvent> {
    let option = updated.iter().find(|o| o.id == config_id)?;
    let SessionConfigKindInfo::Select(select) = &option.kind else {
        return None;
    };
    if select.current_value == requested {
        return None;
    }
    // Labels, not ids: the composer's dropdown shows names, so the notice has to
    // name the same things the user was looking at.
    let label = |value: &str| {
        select
            .options
            .iter()
            .find(|item| item.value == value)
            .map(|item| item.name.clone())
            .unwrap_or_else(|| value.to_string())
    };
    Some(AcpEvent::ConfigOptionRejected {
        config_id: config_id.to_string(),
        option_name: option.name.clone(),
        requested: label(requested),
        actual: label(&select.current_value),
    })
}

/// Encode a selector value for `session/set_config_option`.
///
/// codeg keeps config values as opaque `String`s end to end (Tauri command, web
/// handler, and the per-agent preference store all use `Record<string, string>`
/// semantics), so the boolean round-trip is `"true"` ⇄ `true` and happens only
/// here. A `select` value stays a bare `{"value": "…"}`, byte-for-byte what
/// codeg sent before boolean options existed — every non-cline agent's wire
/// traffic is unchanged.
fn encode_config_option_value(is_boolean: bool, value: &str) -> SessionConfigOptionValue {
    if is_boolean {
        SessionConfigOptionValue::boolean(value == "true")
    } else {
        SessionConfigOptionValue::value_id(value.to_string())
    }
}

/// Whether an advertised option already holds `value`, so applying a saved
/// preference at connect can skip the round-trip. Reads the same `"true"` ⇄
/// `true` encoding [`encode_config_option_value`] writes; an option kind this
/// build cannot interpret is treated as "does not match" so the agent, not
/// codeg, decides.
fn config_option_already_holds(option: &SessionConfigOption, value: &str) -> bool {
    match &option.kind {
        SessionConfigKind::Select(s) => s.current_value.to_string() == value,
        SessionConfigKind::Boolean(b) => b.current_value == (value == "true"),
        _ => false,
    }
}

/// Wire-level half of `set_session_config_option`: send the JSON-RPC request and
/// return the agent's new config-options list, without touching SessionState or
/// emitting events. Used at session-init to apply saved preferences before the
/// single emit_session_config_options call so the frontend never sees an
/// "agent default → user preference" flicker.
async fn set_session_config_option_inner(
    cx: &ConnectionTo<Agent>,
    session_id: &SessionId,
    config_id: String,
    value: SessionConfigOptionValue,
) -> Result<Vec<SessionConfigOption>, sacp::Error> {
    let req = SetSessionConfigOptionRequest::new(session_id.clone(), config_id, value);
    let untyped_req = UntypedMessage::new("session/set_config_option", req).map_err(|e| {
        sacp::util::internal_error(format!("Failed to build config option request: {e}"))
    })?;

    let mut raw_response = cx.send_request_to(Agent, untyped_req).block_task().await?;
    strip_unknown_config_options(&mut raw_response, "session/set_config_option");
    let response: SetSessionConfigOptionResponse =
        serde_json::from_value(raw_response).map_err(|e| {
            sacp::util::internal_error(format!("Failed to parse config option response: {e}"))
        })?;

    Ok(response.config_options)
}

/// Send the connection's goal-control extension request to pause or clear the
/// session's active goal — the advertised `_session/goal` (claude 0.66+ / codex
/// 1.2+) or codex's bespoke `_codex/session/goal_control` (#293, v1.1.4) it
/// still accepts as an alias. Start / resume / re-objective are NOT this method
/// — they go through the `/goal` prompt.
///
/// The agent replies with an empty object and then pushes the resulting goal
/// snapshot as a normal `session_info_update` (`_meta.goal`, the legacy
/// `_meta.codex.goal`, or `null` for a clear), which the existing goal-card
/// path renders — so the response value carries nothing to parse and is
/// intentionally discarded.
///
/// This request does NOT stop a running turn on either adapter; that is the
/// manager's call (see `ConnectionManager::goal_control`), because whether an
/// interrupt is safe depends on how the adapter delivers the control.
///
/// Sent via `UntypedMessage` because `_codex/…` is a codex-private extension
/// method with no sacp typed variant — the same escape hatch used for
/// `session/set_config_option` and `session/fork`.
async fn send_goal_control(
    cx: &ConnectionTo<Agent>,
    session_id: &SessionId,
    action: GoalControlAction,
    method: &str,
) -> Result<(), sacp::Error> {
    // `method` is the connection's stored `goal_control_method`: the
    // advertised provider-neutral `_session/goal` (claude 0.66+/codex 1.2+)
    // or the legacy `_codex/session/goal_control` default. Both take the
    // same `{sessionId, action}` request shape.
    let params = serde_json::json!({
        "sessionId": session_id,
        "action": action,
    });
    let untyped_req = UntypedMessage::new(method, params).map_err(|e| {
        sacp::util::internal_error(format!("Failed to build goal_control request: {e}"))
    })?;
    cx.send_request_to(Agent, untyped_req).block_task().await?;
    Ok(())
}

/// Apply user-saved mode and config-option preferences to a freshly-attached
/// session BEFORE the initial `session_modes` / `session_config_options`
/// events are emitted to the frontend.
///
/// This is the single ownership point for "preference → agent state" — the
/// frontend stores the user's last selections per agent_type and ships them
/// to the backend on connect; we then call `session/set_mode` and
/// `session/set_config_option` to align the agent process so the snapshot
/// the frontend will see (whether via WS `snapshot` frame or fetched HTTP
/// snapshot) already reflects the user's choices. No client-side
/// "intercept event and rewrite then sync back" hack — single source of truth.
///
/// Returns the (possibly updated) list of config options that the caller
/// should emit. Mode preferences trigger a `ModeChanged` event from
/// `set_session_mode`, which the caller's `emit_session_modes` immediately
/// precedes — so the frontend sees `SessionModes{default}` then
/// `ModeChanged{preferred}` and converges to the preferred value before
/// `SelectorsReady` fires. Failures on individual preferences are logged
/// and skipped so a stale/invalid preference can't block session startup.
#[allow(clippy::too_many_arguments)]
async fn apply_preferred_session_options(
    cx: &ConnectionTo<Agent>,
    session: &mut sacp::ActiveSession<'_, Agent>,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    preferred_mode_id: Option<&str>,
    preferred_config_values: &BTreeMap<String, String>,
    initial_config_options: Vec<SessionConfigOption>,
) -> Vec<SessionConfigOption> {
    if let Some(pref_mode) = preferred_mode_id {
        let needs_apply = session
            .modes()
            .as_ref()
            .map(|m| m.current_mode_id.to_string() != pref_mode)
            .unwrap_or(false);
        if needs_apply {
            if let Err(e) = set_session_mode(session, state, emitter, pref_mode.to_string()).await {
                tracing::error!("[ACP] failed to apply preferred mode '{pref_mode}' on connect: {e}");
            }
        }
    }

    if preferred_config_values.is_empty() {
        return initial_config_options;
    }

    let session_id = session.session_id().clone();
    let mut options = initial_config_options;
    for (config_id, value_id) in preferred_config_values {
        // Skip the round-trip when the agent's current value already matches.
        // Note: codex-acp 1.0.0 advertises "mode" as a config option (so the
        // match check below normally fires), but we still do NOT skip when a
        // requested config_id is absent from the advertised options — older or
        // edge-case builds accept `set_config_option` for an unadvertised "mode"
        // (see `ensure_codex_mode_option`), so let the agent decide.
        let advertised = options.iter().find(|o| o.id.to_string() == *config_id);
        let already_matches =
            advertised.is_some_and(|o| config_option_already_holds(o, value_id.as_str()));
        if already_matches {
            continue;
        }
        // Encode against what the agent advertised for this id. An id the agent
        // never advertised falls back to the select form — the same value shape
        // codeg has always sent (see the note above on unadvertised "mode").
        let is_boolean =
            advertised.is_some_and(|o| matches!(o.kind, SessionConfigKind::Boolean(_)));
        let value = encode_config_option_value(is_boolean, value_id);
        match set_session_config_option_inner(cx, &session_id, config_id.clone(), value).await {
            Ok(updated) => options = updated,
            Err(e) => tracing::error!(
                "[ACP] failed to apply preferred config '{config_id}'='{value_id}' \
                 on connect: {e}"
            ),
        }
    }

    options
}

const TERMINAL_POLL_INTERVAL_MS: u64 = 200;
const TERMINAL_POLL_MISSING_LIMIT: u8 = 10;

/// Hard cap on the size of a single ACP event's `raw_output` payload.
///
/// Agents (e.g. Claude Code, Codex) frequently send `tool_call_update`
/// notifications where `raw_output` is the **full accumulated** tool output
/// rather than an incremental delta. For long-running terminal tools this
/// leads to O(N²) bytes flowing through the event pipeline and multi-GB
/// transient allocations (serde_json Value trees, IPC buffers, broadcast
/// channel backlog). This constant caps any single emitted chunk so the
/// pipeline never sees a multi-MB event.
const MAX_SINGLE_EMIT_BYTES: usize = 64 * 1024;

/// Byte length of the tail we retain per tool-call to verify that the next
/// incoming snapshot is a cumulative extension of the previous one. Small
/// enough to keep the cache bounded even in pathological sessions, large
/// enough that a matching tail is an extremely unlikely coincidence.
const MAX_CACHED_TAIL_BYTES: usize = 8 * 1024;

/// Hard cap on the number of tool-call entries the cache retains. Prevents
/// unbounded growth in long sessions where agents forget to mark tool calls
/// as completed. Entries are evicted FIFO by generation counter.
const MAX_CACHE_ENTRIES: usize = 256;

/// Prefix used when an emitted chunk had to be truncated.
const TRUNCATION_MARKER: &str = "[...truncated...]\n";

#[derive(Debug)]
struct CachedOutput {
    /// Total byte length of the last observed `raw_output`.
    total_len: usize,
    /// Tail of the last observed `raw_output`, up to `MAX_CACHED_TAIL_BYTES`
    /// bytes. Always aligned to a UTF-8 character boundary at the start.
    tail: String,
    /// Monotonic insertion/update tick used for FIFO eviction.
    generation: u64,
}

/// Per-session cache of the last `raw_output` fingerprint emitted for each
/// tool call. Enables delta detection: when an agent sends cumulative
/// snapshots, we forward only the suffix (with `raw_output_append=true`)
/// and keep the fingerprint bounded so it works even when the full output
/// grows into the multi-MB range.
#[derive(Debug, Default)]
struct ToolCallOutputCache {
    entries: HashMap<String, CachedOutput>,
    next_generation: u64,
}

impl ToolCallOutputCache {
    /// Diff an incoming full `raw_output` snapshot for `tool_call_id` against
    /// the cache and return what should be emitted downstream.
    ///
    /// Returns `None` when the incoming snapshot is identical to the
    /// previously emitted one (nothing to send). Otherwise returns
    /// `(payload, append)` where:
    /// - `append=true` — `payload` is a (possibly truncated) suffix delta;
    ///   the frontend should append it to the existing chunks.
    /// - `append=false` — `payload` is a (possibly truncated) replacement
    ///   for the full tool output; the frontend should reset chunks.
    fn consume(&mut self, tool_call_id: &str, curr: &str) -> Option<(String, bool)> {
        let curr_len = curr.len();

        let decision: Option<(String, bool)> = match self.entries.get(tool_call_id) {
            Some(prev) if curr_len >= prev.total_len && self.is_extension_of(prev, curr) => {
                if curr_len == prev.total_len {
                    // Identical output — nothing to emit. Cache stays fresh.
                    return None;
                }
                let suffix = &curr[prev.total_len..];
                Some(build_emit_payload(suffix, true))
            }
            _ => Some(build_emit_payload(curr, false)),
        };

        // Update cache snapshot to current state so the next update can
        // still detect a prefix extension.
        let tail =
            trim_partial_ansi_tail(truncate_tail_at_char_boundary(curr, MAX_CACHED_TAIL_BYTES))
                .to_string();
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        self.entries.insert(
            tool_call_id.to_string(),
            CachedOutput {
                total_len: curr_len,
                tail,
                generation,
            },
        );
        self.enforce_entry_cap();
        decision
    }

    /// Seed the cache with an initial snapshot for `tool_call_id`, WITHOUT
    /// attempting to diff against any prior state. Used for the initial
    /// `SessionUpdate::ToolCall` notification, whose frontend reducer
    /// treats `raw_output` as a full replacement.
    fn seed(&mut self, tool_call_id: &str, curr: &str) -> Option<String> {
        let (payload, _append) = build_emit_payload(curr, false);
        let tail =
            trim_partial_ansi_tail(truncate_tail_at_char_boundary(curr, MAX_CACHED_TAIL_BYTES))
                .to_string();
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        self.entries.insert(
            tool_call_id.to_string(),
            CachedOutput {
                total_len: curr.len(),
                tail,
                generation,
            },
        );
        self.enforce_entry_cap();
        if payload.is_empty() {
            None
        } else {
            Some(payload)
        }
    }

    /// Drop cached state for a tool call that has finished. Keeps the
    /// session-scoped cache bounded in long-running sessions.
    fn remove_if_final(&mut self, tool_call_id: &str, status: Option<&str>) {
        if matches!(status, Some("completed" | "failed" | "cancelled" | "error")) {
            self.entries.remove(tool_call_id);
        }
    }

    /// Returns true when the cached fingerprint matches `curr` at the
    /// expected offset — i.e. `curr` is a prefix extension (or identity)
    /// of the previously observed snapshot.
    fn is_extension_of(&self, prev: &CachedOutput, curr: &str) -> bool {
        let tail_start = prev.total_len.saturating_sub(prev.tail.len());
        curr.get(tail_start..prev.total_len)
            .is_some_and(|slice| slice == prev.tail.as_str())
    }

    /// Evict oldest entries (by `generation`) once the cache exceeds the
    /// entry cap. Linear scan over a bounded map, so O(MAX_CACHE_ENTRIES)
    /// per eviction — acceptable at this size.
    fn enforce_entry_cap(&mut self) {
        while self.entries.len() > MAX_CACHE_ENTRIES {
            let Some(oldest_id) = self
                .entries
                .iter()
                .min_by_key(|(_, v)| v.generation)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.entries.remove(&oldest_id);
        }
    }
}

/// Apply the per-event size cap + truncation marker. Returns `(payload,
/// append)`. An empty `text` yields an empty `payload`; callers should
/// decide whether to suppress the emission in that case.
fn build_emit_payload(text: &str, append: bool) -> (String, bool) {
    let truncated =
        trim_partial_ansi_tail(truncate_tail_at_char_boundary(text, MAX_SINGLE_EMIT_BYTES));
    let out = if truncated.len() < text.len() {
        format!("{TRUNCATION_MARKER}{truncated}")
    } else {
        truncated.to_string()
    };
    (out, append)
}

/// Return a substring of `s` whose byte length is `<= max_bytes`, aligned to
/// a UTF-8 character boundary and taken from the TAIL of `s` (so the most
/// recent output is preserved when truncation is required).
fn truncate_tail_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut start = s.len() - max_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// If the very end of `s` contains a partial ANSI escape sequence, trim it
/// so downstream ANSI parsers (e.g. the frontend `ansi-to-react` renderer)
/// don't see a half-emitted escape.
///
/// Handles the three common ACP-stream cases:
/// - CSI (`ESC [ ... final`): terminator is a byte in 0x40..=0x7E after
///   the `[` introducer.
/// - OSC (`ESC ] ... ST|BEL`): terminator is BEL (0x07) or `ESC \`.
/// - Simple two-byte escape (`ESC <byte>`): complete as soon as the byte
///   following ESC is present.
///
/// ESC is ASCII (1 byte), always a valid UTF-8 char boundary, so slicing
/// at `esc_pos` cannot produce an invalid UTF-8 string.
fn trim_partial_ansi_tail(s: &str) -> &str {
    let bytes = s.as_bytes();
    let Some(esc_pos) = bytes.iter().rposition(|&b| b == 0x1B) else {
        return s;
    };
    let after = &bytes[esc_pos + 1..];
    if after.is_empty() {
        return &s[..esc_pos];
    }
    let terminated = match after[0] {
        b'[' => after[1..].iter().any(|&b| (0x40..=0x7E).contains(&b)),
        b']' => {
            after[1..].contains(&0x07)
                || after[1..].windows(2).any(|w| w[0] == 0x1B && w[1] == b'\\')
        }
        // Two-byte escape sequences (ESC M, ESC D, …) are complete as
        // soon as the second byte is present.
        _ => true,
    };
    if terminated {
        s
    } else {
        &s[..esc_pos]
    }
}

#[derive(Debug, Default)]
struct TrackedTerminalToolCall {
    terminal_ids: Vec<String>,
    status: Option<String>,
    terminal_offsets: HashMap<String, u64>,
    terminal_exit_reported: HashSet<String>,
    has_emitted_output: bool,
    missing_polls: u8,
}

#[derive(Debug, Default)]
struct TerminalPollResult {
    output: Option<String>,
    append: bool,
    any_found: bool,
    all_exited: bool,
}

fn is_final_tool_call_status(status: Option<&str>) -> bool {
    matches!(status, Some("completed" | "failed"))
}

fn merge_terminal_ids(existing: &mut Vec<String>, incoming: Vec<String>) -> bool {
    let mut changed = false;
    for terminal_id in incoming {
        if !existing.iter().any(|id| id == &terminal_id) {
            existing.push(terminal_id);
            changed = true;
        }
    }
    changed
}

fn extract_terminal_ids(content: &[ToolCallContent]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut terminal_ids = Vec::new();
    for item in content {
        if let ToolCallContent::Terminal(terminal) = item {
            let terminal_id = terminal.terminal_id.to_string();
            if seen.insert(terminal_id.clone()) {
                terminal_ids.push(terminal_id);
            }
        }
    }
    terminal_ids
}

fn track_terminal_tool_calls(
    update: &SessionUpdate,
    tracked: &mut HashMap<String, TrackedTerminalToolCall>,
) -> bool {
    match update {
        SessionUpdate::ToolCall(tc) => {
            let terminal_ids = extract_terminal_ids(&tc.content);
            if terminal_ids.is_empty() {
                return false;
            }

            let status = format!("{:?}", tc.status).to_lowercase();
            let entry = tracked.entry(tc.tool_call_id.to_string()).or_default();
            let changed = merge_terminal_ids(&mut entry.terminal_ids, terminal_ids);
            entry.status = Some(status);
            changed
        }
        SessionUpdate::ToolCallUpdate(tcu) => {
            let mut changed = false;
            let mut should_track = false;

            let terminal_ids = tcu
                .fields
                .content
                .as_ref()
                .map(|content| extract_terminal_ids(content))
                .unwrap_or_default();
            if !terminal_ids.is_empty() {
                should_track = true;
            }

            if tracked.contains_key(&tcu.tool_call_id.to_string()) {
                should_track = true;
            }

            if !should_track {
                return false;
            }

            let entry = tracked.entry(tcu.tool_call_id.to_string()).or_default();
            if !terminal_ids.is_empty() {
                changed = merge_terminal_ids(&mut entry.terminal_ids, terminal_ids);
            }

            if let Some(status) = tcu.fields.status {
                let status_str = format!("{:?}", status).to_lowercase();
                if entry.status.as_deref() != Some(status_str.as_str()) {
                    changed = true;
                }
                entry.status = Some(status_str);
            }

            changed
        }
        _ => false,
    }
}

fn format_terminal_exit_status(exit_status: &TerminalExitStatus) -> String {
    let mut parts = Vec::new();
    if let Some(code) = exit_status.exit_code {
        parts.push(format!("exit code: {code}"));
    }
    if let Some(signal) = &exit_status.signal {
        parts.push(format!("signal: {signal}"));
    }
    if parts.is_empty() {
        "finished".to_string()
    } else {
        parts.join(", ")
    }
}

async fn poll_terminal_tool_call_output(
    terminal_runtime: &TerminalRuntime,
    session_id: &SessionId,
    tracked: &mut TrackedTerminalToolCall,
) -> Result<TerminalPollResult, TerminalRuntimeError> {
    let mut chunks: Vec<String> = Vec::new();
    let mut any_found = false;
    let mut all_exited = true;
    let include_headers = tracked.terminal_ids.len() > 1;

    for terminal_id in &tracked.terminal_ids {
        let from_offset = tracked.terminal_offsets.get(terminal_id).copied();
        let response = match terminal_runtime
            .terminal_output_delta(session_id.0.as_ref(), terminal_id, from_offset)
            .await
        {
            Ok(response) => response,
            Err(TerminalRuntimeError::InvalidParams(_)) => continue,
            Err(err) => return Err(err),
        };

        any_found = true;
        tracked
            .terminal_offsets
            .insert(terminal_id.clone(), response.next_offset);

        if response.exit_status.is_none() {
            all_exited = false;
        }

        let mut chunk = String::new();
        if include_headers {
            chunk.push_str(&format!("[Terminal: {terminal_id}]\n"));
        }

        if response.had_gap {
            chunk.push_str("[output truncated]\n");
        }

        if !response.output.is_empty() {
            chunk.push_str(&response.output);
            if !chunk.ends_with('\n') {
                chunk.push('\n');
            }
        }

        if response.truncated && from_offset.is_none() {
            chunk.push_str("[output truncated]\n");
        }

        if let Some(exit_status) = response.exit_status {
            if tracked.terminal_exit_reported.insert(terminal_id.clone()) {
                chunk.push_str(&format!(
                    "[terminal exited: {}]\n",
                    format_terminal_exit_status(&exit_status)
                ));
            }
        }

        if chunk.ends_with('\n') {
            chunk.pop();
        }
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
    }

    if !any_found {
        all_exited = false;
    }

    let append = tracked.has_emitted_output;
    if !chunks.is_empty() {
        tracked.has_emitted_output = true;
    }

    Ok(TerminalPollResult {
        output: if chunks.is_empty() {
            None
        } else {
            Some(chunks.join("\n\n"))
        },
        append,
        any_found,
        all_exited,
    })
}

async fn emit_terminal_output_update(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    tool_call_id: &str,
    output: String,
    append: bool,
) {
    // Safety cap: when a subprocess writes very fast between poll ticks,
    // the delta produced by `poll_terminal_tool_call_output` can still be
    // up to ~1 MB (the terminal buffer limit). Enforce the pipeline-wide
    // single-event cap (with ANSI-safe truncation) before emission so the
    // WS/IPC fanout never carries a multi-MB payload.
    let (payload, _append) = build_emit_payload(&output, append);
    emit_with_state(
        state,
        emitter,
        AcpEvent::ToolCallUpdate {
            tool_call_id: tool_call_id.to_string(),
            title: None,
            status: None,
            content: None,
            raw_input: None,
            raw_output: Some(payload),
            raw_output_append: Some(append),
            locations: None,
            meta: None,
            images: None,
        },
    )
    .await;
}

async fn poll_tracked_terminal_tool_calls(
    terminal_runtime: &TerminalRuntime,
    session_id: &SessionId,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    tracked: &mut HashMap<String, TrackedTerminalToolCall>,
) {
    if tracked.is_empty() {
        return;
    }

    let tool_call_ids: Vec<String> = tracked.keys().cloned().collect();
    let mut remove_ids: Vec<String> = Vec::new();

    for tool_call_id in tool_call_ids {
        let Some(entry) = tracked.get_mut(&tool_call_id) else {
            continue;
        };
        if entry.terminal_ids.is_empty() {
            remove_ids.push(tool_call_id.clone());
            continue;
        }

        let poll_result =
            match poll_terminal_tool_call_output(terminal_runtime, session_id, entry).await {
                Ok(result) => result,
                Err(err) => {
                    tracing::error!(
                        "[ACP] Failed to poll terminal output for tool call {}: {:?}",
                        tool_call_id, err
                    );
                    continue;
                }
            };

        if poll_result.any_found {
            entry.missing_polls = 0;
        } else {
            entry.missing_polls = entry.missing_polls.saturating_add(1);
        }

        if let Some(output) = poll_result.output {
            emit_terminal_output_update(state, emitter, &tool_call_id, output, poll_result.append)
                .await;
        }

        if (is_final_tool_call_status(entry.status.as_deref())
            && (!poll_result.any_found || poll_result.all_exited))
            || entry.missing_polls >= TERMINAL_POLL_MISSING_LIMIT
        {
            remove_ids.push(tool_call_id.clone());
        }
    }

    for tool_call_id in remove_ids {
        tracked.remove(&tool_call_id);
    }
}

/// Append the just-ended turn's observed span to the timing journal (see
/// `crate::turn_timings`). `probe` is `Some((send_stamp, prompt_hash))` only
/// on agents codeg journals for (Cursor) and is consumed on the first
/// journaling terminal path, so a turn appends at most one line.
///
/// ONLY cleanly completed turns are journaled — callers gate on the
/// NORMALIZED stop reason (`reason_str == "end_turn"`, which a raw
/// `end_turn` with no agent output does NOT satisfy: it reclassifies to
/// `"empty"` and is excluded). A canceled or empty turn may never have been
/// persisted by Cursor at all, and journaling such a phantom re-opens the
/// misassignment the parser's guards exist to prevent: a later same-hash
/// store turn could pair with the phantom's line even across non-contiguous
/// positions (Codex review R4-2). An unjournaled-but-persisted turn
/// mid-session merely stops the reverse walk (older turns lose their
/// clocks); when such turns make up the session's TAIL, the second accepted
/// residual in `turn_timings`' module docs applies (a stale journal tail can
/// hash-collide with the store's newest turn).
///
/// The append is queued to the journal's single-writer thread and awaited
/// with a short timeout: the normal case lands in microseconds BEFORE the
/// TurnComplete emit (so the post-turn reparse deterministically sees it),
/// while a hung filesystem blocks neither the turn loop nor any Tokio pool —
/// the queued job just lands late (still in order; the FIFO queue is what
/// makes overtaking structurally impossible) or is dropped at the queue cap.
async fn journal_turn_span(
    probe: &mut Option<(u64, String, u64)>,
    connection_id: &str,
    session_id: &str,
) {
    let Some((started_at_ms, prompt_sha, ord)) = probe.take() else {
        return;
    };
    let ack = crate::turn_timings::enqueue_turn_timing(
        crate::paths::codeg_turn_timings_root(),
        crate::turn_timings::CURSOR_JOURNAL_AGENT.to_string(),
        session_id.to_string(),
        crate::turn_timings::TurnTiming {
            v: crate::turn_timings::TURN_TIMING_SCHEMA_VERSION,
            ord,
            conn: connection_id.to_string(),
            prompt_sha,
            started_at_ms,
            ended_at_ms: crate::turn_timings::now_epoch_ms(),
        },
    );
    // Determinism window only — a timeout (or a dropped job's closed channel)
    // means the entry lands late or not at all, degrading that turn to a
    // missing footer clock. (See `turn_timings`' module docs for the two
    // narrow accepted residuals where missing lines can shift alignment.)
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), ack).await;
}

/// The image mime types grok's normalizer actually decodes — verbatim the set
/// its own clipboard reader accepts (`xai-grok-shared/src/clipboard.rs`), and
/// the boundary between the two carriages in [`normalize_grok_image_blocks`].
///
/// Measured against grok 1.0.0: png / webp / bmp / tiff round-trip through the
/// describe sidecar (jpeg is one of the two formats grok itself re-encodes to,
/// gif rides the same table). `image/svg+xml` does NOT — its validator answers
/// `unsupported or unrecognised image format` and the image never reaches the
/// model. Deliberately an allow-list, not a `image/*` prefix test: a mime we
/// have no evidence for keeps the resource carriage, which is where it already
/// was, so a wrong guess here can never be a regression.
fn grok_decodes_image_mime(mime: &str) -> bool {
    matches!(
        mime.trim().to_ascii_lowercase().as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "image/tiff"
    )
}

/// Whether a mime names an image at all — the outer guard on the demotion, so a
/// malformed `Image` block carrying something else entirely is left untouched.
/// Normalized exactly like [`grok_decodes_image_mime`]: if the two guards
/// disagreed about the same string (`IMAGE/SVG+XML` reads as "not an image" to a
/// case-sensitive test), an undecodable format would slip through as native.
fn is_image_mime(mime: &str) -> bool {
    mime.trim().to_ascii_lowercase().starts_with("image/")
}

/// Put every attached image on the carriage grok can actually read.
///
/// Two encodings carry the same bytes and grok treats them very differently:
///
/// * A native `Image` block runs its describe sidecar — the ONLY path where the
///   model sees pixels (see [`effective_prompt_capabilities`]). But grok
///   validates the format first and DROPS anything it cannot decode.
/// * A `Resource` blob is saved into the session's `assets/` and announced to
///   the model as a file path. No pixels, but for a text-shaped image (svg) the
///   model can just read the source — measurably better than a drop.
///
/// So decodable images are promoted (queued drafts and work-task prompts
/// composed before codeg advertised `image:true` still carry the old shape),
/// and undecodable ones are demoted back — the composer only sees the single
/// `image` capability bit and cannot make this call per mime.
fn normalize_grok_image_blocks(blocks: Vec<PromptInputBlock>) -> Vec<PromptInputBlock> {
    blocks
        .into_iter()
        .enumerate()
        .map(|(index, block)| match block {
            PromptInputBlock::Resource {
                uri,
                mime_type: Some(mime),
                text: None,
                blob: Some(blob),
            } if grok_decodes_image_mime(&mime) && !blob.is_empty() => PromptInputBlock::Image {
                data: blob,
                mime_type: mime,
                uri: Some(uri),
            },
            PromptInputBlock::Image {
                data,
                mime_type,
                uri,
            } if is_image_mime(&mime_type)
                && !grok_decodes_image_mime(&mime_type)
                && !data.is_empty() =>
            {
                PromptInputBlock::Resource {
                    // A pasted image has no path; its position in this prompt is
                    // the stable identifier, same as the work-task engine's.
                    uri: uri.unwrap_or_else(|| format!("clipboard://grok-image-{index}")),
                    mime_type: Some(mime_type),
                    text: None,
                    blob: Some(data),
                }
            }
            other => other,
        })
        .collect()
}

fn map_prompt_blocks(blocks: Vec<PromptInputBlock>) -> Vec<ContentBlock> {
    blocks
        .into_iter()
        .map(|block| match block {
            PromptInputBlock::Text { text } => ContentBlock::Text(TextContent::new(text)),
            PromptInputBlock::Image {
                data,
                mime_type,
                uri,
            } => ContentBlock::Image(ImageContent::new(data, mime_type).uri(uri)),
            PromptInputBlock::Resource {
                uri,
                mime_type,
                text,
                blob,
            } => {
                let resource = match (text, blob) {
                    (Some(text_value), _) => {
                        let content =
                            TextResourceContents::new(text_value, uri.clone()).mime_type(mime_type);
                        EmbeddedResourceResource::TextResourceContents(content)
                    }
                    (None, Some(blob_value)) => {
                        let content =
                            BlobResourceContents::new(blob_value, uri.clone()).mime_type(mime_type);
                        EmbeddedResourceResource::BlobResourceContents(content)
                    }
                    (None, None) => {
                        let content =
                            TextResourceContents::new("", uri.clone()).mime_type(mime_type);
                        EmbeddedResourceResource::TextResourceContents(content)
                    }
                };
                ContentBlock::Resource(EmbeddedResource::new(resource))
            }
            PromptInputBlock::ResourceLink {
                uri,
                name,
                mime_type,
                description,
            } => {
                let mut link = ResourceLink::new(name, uri);
                link.mime_type = mime_type;
                link.description = description;
                ContentBlock::ResourceLink(link)
            }
        })
        .collect()
}

/// Result when the conversation loop exits due to a fork request.
struct ForkExitInfo {
    fork_response: sacp::schema::ForkSessionResponse,
    /// Raw top-level `models` from the fork response (Grok per-model effort data),
    /// captured before the typed deserialize drops it. `None` when absent.
    fork_models_raw: Option<serde_json::Value>,
    original_session_id: String,
    reply: tokio::sync::oneshot::Sender<Result<crate::acp::types::ForkProtocolResult, AcpError>>,
    connection: ConnectionTo<Agent>,
}

/// After `run_conversation_loop` returns, handle normal exit or fork transition.
///
/// When fork is requested, the original session has already been dropped by the
/// caller.  We attach to the forked session (S2) directly using the
/// `ForkSessionResponse` — no separate `session/load` is needed because S2 was
/// just created in-memory by the agent on this connection.
#[allow(clippy::too_many_arguments)]
async fn handle_fork_or_exit(
    loop_result: Result<Option<ForkExitInfo>, sacp::Error>,
    conn_id: &str,
    emitter: &EventEmitter,
    state: &Arc<RwLock<SessionState>>,
    agent_type: AgentType,
    perms: &PendingPermissions,
    cmd_rx: &mut mpsc::Receiver<ConnectionCommand>,
    terminal_runtime: Arc<TerminalRuntime>,
    _cwd: &std::path::Path,
    cwd_string: &str,
    // Threaded through from run_connection: the connection-scoped prompt
    // ledger (the forked session's loop keeps fingerprinting into the SAME
    // ledger the still-running watcher consumes from).
    prompt_ledger: &background_watch::PromptLedger,
    // Threaded through from run_connection so the forked session's
    // run_conversation_loop call has the same delegation cascade
    // capability as the original.
    delegation_injection: Option<&DelegationInjection>,
    // Same rationale: the forked session keeps writing into (and reading from)
    // the SAME connection-scoped stderr buffer — the agent process is unchanged
    // across a fork, so its stderr history stays relevant.
    stderr_tail: &Arc<StderrTail>,
) -> Result<(), sacp::Error> {
    let fork_info = match loop_result {
        Ok(Some(info)) => info,
        Ok(None) => return Ok(()),
        Err(e) => return Err(e),
    };

    let cx = fork_info.connection;
    let fork_resp = fork_info.fork_response;
    let fork_models_raw = fork_info.fork_models_raw;
    let new_sid = fork_resp.session_id.0.to_string();

    tracing::info!(
        "[ACP] Fork transition: attaching to forked session {} (original: {})",
        new_sid, fork_info.original_session_id
    );

    // Reply protocol-level result to manager.fork_session, which will combine
    // it with the freshly-created sibling row id to produce the wire ForkResultInfo.
    let _ = fork_info
        .reply
        .send(Ok(crate::acp::types::ForkProtocolResult {
            forked_session_id: new_sid.clone(),
            original_session_id: fork_info.original_session_id,
        }));

    // Build a NewSessionResponse from the ForkSessionResponse so we can
    // attach directly — the forked session is already live on this process.
    let initial_config_options = fork_resp.config_options.clone();
    let new_resp = NewSessionResponse::new(fork_resp.session_id)
        .modes(fork_resp.modes)
        .config_options(fork_resp.config_options)
        .meta(fork_resp.meta);
    let grok_meta = if agent_type == AgentType::Grok {
        new_resp.meta.clone()
    } else {
        None
    };
    // Opportunistic: grok may carry per-model effort data on a fork response.
    let grok_model_specs =
        (agent_type == AgentType::Grok).then(|| parse_grok_model_specs(fork_models_raw.as_ref()));
    let mut session = cx.attach_session(new_resp, Default::default())?;

    // A fork is a new session id, hence a new transcript file. Its history
    // starts empty and accumulates from the fork point — the pre-fork turns
    // stay in the parent's transcript, which is what forking means.
    record_transcript_header(agent_type, &new_sid, cwd_string);
    emit_with_state(
        state,
        emitter,
        AcpEvent::SessionStarted {
            session_id: new_sid.clone(),
        },
    )
    .await;
    emit_session_modes(state, emitter, session.modes()).await;
    apply_and_emit_session_config_options(
        &cx,
        &mut session,
        state,
        emitter,
        agent_type,
        grok_meta.as_ref(),
        grok_model_specs.as_ref(),
        None,
        &BTreeMap::new(),
        initial_config_options.unwrap_or_default(),
    )
    .await;
    emit_selectors_ready(state, emitter).await;

    let loop_result = run_conversation_loop(
        &mut session,
        conn_id,
        emitter,
        state,
        agent_type,
        perms,
        cmd_rx,
        terminal_runtime.clone(),
        cwd_string,
        true, // fork already succeeded on this process
        prompt_ledger,
        delegation_injection,
        stderr_tail,
    )
    .await;
    terminal_runtime.release_all_for_session(&new_sid).await;
    drop(session);

    // Recursively handle nested forks
    Box::pin(handle_fork_or_exit(
        loop_result,
        conn_id,
        emitter,
        state,
        agent_type,
        perms,
        cmd_rx,
        terminal_runtime,
        _cwd,
        cwd_string,
        prompt_ledger,
        delegation_injection,
        stderr_tail,
    ))
    .await
}

/// Main conversation command loop: wait for frontend commands and process them.
///
/// Map ACP `StopReason` to a stable lowercase string carried in the
/// `TurnComplete` event. Covers all 5 spec variants so non-success reasons
/// (`Refusal`/`MaxTokens`/`MaxTurnRequests`) keep their semantics instead of
/// collapsing to `"unknown"` — the lifecycle subscriber and frontend rely on
/// this distinction. The wildcard arm exists because the upstream enum is
/// `#[non_exhaustive]`.
fn stop_reason_to_str(reason: StopReason) -> &'static str {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::Cancelled => "cancelled",
        StopReason::Refusal => "refusal",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        _ => "unknown",
    }
}

/// Classify a `session/load` failure into a stable frontend `code` when the
/// historical session cannot be restored — either the agent has no record of
/// it (`ResourceNotFound`, -32002) or the agent process/session died mid-load.
/// Claude 0.58.1 surfaces the latter as a -32603 Internal error whose message
/// contains "process exited with code N" (its `getOrCreateSession` only maps
/// "Query closed…"/"No conversation found…" to `ResourceNotFound`), so the
/// crash/ended family is matched on the wire message. Both codes route to the
/// same `SessionLoadFailed` banner (Reload / New conversation) instead of a raw
/// protocol error.
///
/// Returns `None` for failures that must keep the existing behavior:
/// "Method not found" (agent lacks resume → silent `session/new` fallback),
/// "Authentication required" (silent stop), and any other error (emit
/// "starting new" then fall through to `session/new`).
fn classify_session_load_failure(
    code: sacp::schema::ErrorCode,
    message: &str,
) -> Option<&'static str> {
    if matches!(code, sacp::schema::ErrorCode::ResourceNotFound) {
        return Some("resource_not_found");
    }
    // Upstream signals for an unrecoverable session (claude-agent-acp 0.58.1):
    //  - "process exited"    → "Claude Code process exited with code 1",
    //                          "The Claude Agent process exited unexpectedly…"
    //  - "session has ended" → SESSION_ENDED_MESSAGE
    //  - "Session not found" → a plain Error rethrown as an Internal error
    const UNRECOVERABLE: &[&str] =
        &["process exited", "session has ended", "Session not found"];
    if UNRECOVERABLE.iter().any(|s| message.contains(s)) {
        return Some("session_unavailable");
    }
    None
}

/// Whether codeg can absorb a "the agent forgot this session" load failure by
/// itself, rather than stopping and asking the user to Reload or start over.
///
/// It can exactly when codeg — not the agent — owns the conversation's history:
/// custom ACP agents, whose turns are recorded to
/// [`crate::acp_transcript`]. There the failure costs nothing visible — the
/// history still renders, and a fresh agent session (linked by
/// `continues_from`) continues the same conversation. Agents whose history is
/// read back out of their own store keep the banner: for them the session
/// really is gone, and silently starting a new one would orphan it.
///
/// `classified` is [`classify_session_load_failure`]'s verdict; `None` (an
/// unexpected failure) is never recovered here — it keeps the existing
/// emit-then-fall-back-to-`session/new` behaviour.
fn recovers_load_failure_locally(agent_type: AgentType, classified: Option<&'static str>) -> bool {
    classified.is_some() && transcript_dir_for(agent_type).is_some()
}

/// True when a `SessionUpdate` represents actual agent-produced output for
/// the current turn. Used to detect "silent EndTurn" cases where an agent
/// (notably OpenCode) reports the turn ended successfully but never emitted
/// any reply or tool call — in practice this means the model-side request
/// was swallowed and the user would otherwise see a blank conversation
/// transition silently to `PendingReview`. Metadata-only updates
/// (`UserMessageChunk`, `Plan`, `*ModeUpdate`, `ConfigOptionUpdate`,
/// `SessionInfoUpdate`, `AvailableCommandsUpdate`, `UsageUpdate`) do not
/// count.
fn is_agent_output_update(update: &SessionUpdate) -> bool {
    matches!(
        update,
        SessionUpdate::AgentMessageChunk(_)
            | SessionUpdate::AgentThoughtChunk(_)
            | SessionUpdate::ToolCall(_)
            | SessionUpdate::ToolCallUpdate(_)
    )
}

/// Handle one typed `SessionNotification` inside an active turn.
///
/// Extracted from the `if_notification` closure it is called from, and
/// **returns `()` rather than `Result` on purpose**: the closure's only
/// remaining statement is `Ok(())`, which makes it structurally impossible for
/// downstream handling to contribute an `Err` to `MatchDispatch`. That is what
/// lets the caller attribute a `MatchDispatch` failure to
/// [`DropSite::Dispatch`] (a params/schema mismatch) — without it, a future
/// `?` added here would silently be counted as a protocol mismatch, sending
/// triage in the wrong direction with no compile-time signal. A genuine
/// "downstream handling failed" channel must be added as an explicit new
/// `DropSite`, surfaced from this function's return type.
#[allow(clippy::too_many_arguments)]
async fn handle_turn_notification(
    notif: SessionNotification,
    agent_type: AgentType,
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    terminal_runtime: &TerminalRuntime,
    session_id: &SessionId,
    cwd: Option<&str>,
    tracked_terminal_tool_calls: &mut HashMap<String, TrackedTerminalToolCall>,
    raw_output_cache: &mut ToolCallOutputCache,
    cb_state: &mut CodeBuddyLiveState,
    probe: &mut TurnOutputProbe,
) {
    let should_poll_now = track_terminal_tool_calls(&notif.update, tracked_terminal_tool_calls);
    probe.note_update(&notif.update);
    // Custom agents have no store of their own to parse later.
    record_transcript_update(agent_type, &session_id.0, &notif.update);
    emit_conversation_update(
        state,
        emitter,
        agent_type,
        notif.update,
        cwd,
        raw_output_cache,
        cb_state,
    )
    .await;
    if should_poll_now {
        poll_tracked_terminal_tool_calls(
            terminal_runtime,
            session_id,
            state,
            emitter,
            tracked_terminal_tool_calls,
        )
        .await;
    }
}

/// Which of the two in-turn silent-drop sites swallowed an update.
///
/// They fail at different layers and point at different root causes, so they
/// are counted separately rather than lumped into one "dropped" bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropSite {
    /// `session.read_update()` could not decode the message at all — the agent
    /// most likely emitted malformed JSON-RPC.
    Decode,
    /// `MatchDispatch` matched the method but could not deserialize its params
    /// into the typed `SessionNotification` — an ACP schema version drift.
    Dispatch,
}

impl DropSite {
    fn label(self) -> &'static str {
        match self {
            DropSite::Decode => "decode",
            DropSite::Dispatch => "dispatch",
        }
    }
}

/// Minimum spacing between "dropped an unreadable update" WARN lines. Chosen to
/// match [`crate::logging::throttle::LAG_LOG_WINDOW`]: the first drop still
/// surfaces instantly, and a sustained mismatch keeps reporting itself roughly
/// every 10s instead of once per streaming chunk.
const DROPPED_UPDATE_LOG_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);

/// The WARN text for a dropped update. `where_` names the layer that dropped it
/// (the two [`DropSite`] labels, plus `"idle"` — the idle loop has no turn in
/// flight and therefore no probe).
///
/// `coalesced` is the throttle's occurrence count: suppressed hits are never
/// lost, the tally rides on the next emitted line. Pure so the "and the
/// suppressed count is reported" contract has a test that doesn't need a
/// subscriber.
fn dropped_update_log_line(where_: &str, error: &impl std::fmt::Display, coalesced: u64) -> String {
    let head = format!("[ACP] Ignoring unreadable session update ({where_}): {error}");
    if coalesced <= 1 {
        head
    } else {
        format!(
            "{head} (+{} more in the last {}s)",
            coalesced - 1,
            DROPPED_UPDATE_LOG_WINDOW.as_secs()
        )
    }
}

/// Emit the throttled "codeg dropped an update it could not read" WARN.
fn log_dropped_update(
    throttle: &mut LeadingEdgeThrottle,
    where_: &str,
    error: &impl std::fmt::Display,
) {
    if let Some(summary) = throttle.record(1) {
        tracing::warn!(
            "{}",
            dropped_update_log_line(where_, error, summary.occurrences)
        );
    }
}

/// What a single turn was observed to produce. Scoped to one turn and reset at
/// each turn start.
///
/// This exists because `EndTurn` with no output is ambiguous: it can be a real
/// agent-side failure, a turn whose output codeg failed to parse, or a
/// legitimately output-free command turn. Distinguishing them needs more than
/// the single "did we see output" bit this replaces.
#[derive(Debug, Default)]
struct TurnOutputProbe {
    /// Real agent output: reply text, thinking, or a tool call.
    saw_agent_output: bool,
    /// A `SessionUpdate` arrived, but a metadata-only one (plan, mode, usage,
    /// user echo, …).
    saw_metadata_update: bool,
    dropped_decode: u32,
    dropped_dispatch: u32,
    /// First drop's site and *already-redacted* summary. Redaction happens
    /// here, at capture time, so nothing downstream can hold plaintext —
    /// parser errors inline the offending value, and that value comes off the
    /// `session/update` channel (prompt text, file contents, tool args).
    first_drop: Option<(DropSite, String)>,
    /// `StderrTail` write position at turn start, so the diagnosis can scope
    /// stderr to this turn.
    stderr_mark: u64,
}

impl TurnOutputProbe {
    fn new(stderr_mark: u64) -> Self {
        Self {
            stderr_mark,
            ..Default::default()
        }
    }

    fn note_update(&mut self, update: &SessionUpdate) {
        if is_agent_output_update(update) {
            self.saw_agent_output = true;
        } else {
            self.saw_metadata_update = true;
        }
    }

    fn note_dropped(&mut self, site: DropSite, error: &impl std::fmt::Display) {
        match site {
            DropSite::Decode => self.dropped_decode += 1,
            DropSite::Dispatch => self.dropped_dispatch += 1,
        }
        if self.first_drop.is_none() {
            self.first_drop = Some((site, summarize_parser_error(&error.to_string())));
        }
    }

    fn dropped_total(&self) -> u32 {
        self.dropped_decode + self.dropped_dispatch
    }
}

/// Why a turn ended with `EndTurn` but no agent output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmptyTurnCause {
    /// codeg dropped updates it could not parse — the agent may well have
    /// replied; we just couldn't read it.
    ProtocolMismatch,
    /// Only metadata arrived. Observational only: this is NOT proof the turn
    /// was harmless — a real failure can follow a plan or usage update.
    MetadataOnly,
    /// Nothing arrived at all.
    NoOutput,
}

impl EmptyTurnCause {
    fn code(self) -> &'static str {
        match self {
            // Unchanged from before this split, so old clients and replayed
            // snapshots still localize the common case.
            EmptyTurnCause::NoOutput => "turn_failed_empty",
            EmptyTurnCause::ProtocolMismatch => "turn_failed_empty_protocol",
            EmptyTurnCause::MetadataOnly => "turn_failed_empty_metadata",
        }
    }

    fn message(self, agent_type: AgentType) -> String {
        match self {
            EmptyTurnCause::NoOutput => {
                format!("{agent_type} ended the turn without producing any response.")
            }
            EmptyTurnCause::ProtocolMismatch => format!(
                "{agent_type} produced output that codeg could not parse — \
                 the agent version may not match the protocol."
            ),
            EmptyTurnCause::MetadataOnly => format!(
                "{agent_type} sent only status updates this turn and no reply."
            ),
        }
    }
}

/// A diagnosed empty turn: its cause plus the redacted evidence behind it.
#[derive(Debug, Clone)]
struct EmptyTurnReport {
    cause: EmptyTurnCause,
    details: Option<String>,
}

/// Classify an empty turn. `ProtocolMismatch` wins over `MetadataOnly`:
/// "we couldn't read the output" is a stronger signal than "we only saw
/// metadata", and it changes where the user should look.
fn diagnose_empty_turn(probe: &TurnOutputProbe) -> EmptyTurnCause {
    if probe.dropped_total() > 0 {
        EmptyTurnCause::ProtocolMismatch
    } else if probe.saw_metadata_update {
        EmptyTurnCause::MetadataOnly
    } else {
        EmptyTurnCause::NoOutput
    }
}

/// Max stderr lines quoted in an error's `details`.
const EMPTY_TURN_STDERR_LINES: usize = 12;
/// Max bytes of stderr quoted in an error's `details`.
const EMPTY_TURN_STDERR_BYTES: usize = 900;
/// Overall cap on `details`.
const MAX_DETAILS_BYTES: usize = 1200;

/// Assemble the user-facing evidence for an empty turn: what we failed to
/// parse, and what the agent printed to stderr.
///
/// Every fragment is already redacted at its source (`TurnOutputProbe` for
/// parser errors, `StderrTail` for stderr), so this only formats.
fn build_empty_turn_details(probe: &TurnOutputProbe, stderr_tail: &StderrTail) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();

    if probe.dropped_total() > 0 {
        let mut line = format!(
            "dropped {} update(s) ({} decode, {} dispatch)",
            probe.dropped_total(),
            probe.dropped_decode,
            probe.dropped_dispatch
        );
        if let Some((site, summary)) = &probe.first_drop {
            line.push_str(&format!("; first ({}): {summary}", site.label()));
        }
        sections.push(line);
    }

    let tail = stderr_tail.tail_since(
        probe.stderr_mark,
        EMPTY_TURN_STDERR_LINES,
        EMPTY_TURN_STDERR_BYTES,
    );
    if !tail.is_empty() {
        let scope = match tail.scope {
            TailScope::ThisTurn => "this turn",
            TailScope::Recent => "recent",
        };
        let mut block = format!("stderr ({scope}, last {} lines):", tail.lines.len());
        for line in &tail.lines {
            block.push_str("\n  ");
            block.push_str(line);
        }
        sections.push(block);
    }

    if sections.is_empty() {
        return None;
    }
    let joined = sections.join("\n");
    if joined.len() <= MAX_DETAILS_BYTES {
        return Some(joined);
    }
    let end = joined
        .char_indices()
        .take_while(|(i, c)| i + c.len_utf8() <= MAX_DETAILS_BYTES)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    Some(format!("{}…", &joined[..end]))
}

/// Resolve a turn's final stop reason and, when it is `"empty"`, the diagnosis
/// behind it.
///
/// **Pure.** It emits nothing, records nothing, and cancels nothing — the two
/// turn exits keep their own (deliberately asymmetric) side effects in place.
/// In particular the `StopReason`-message exit does NOT call `record_turn_end`
/// while the prompt-response exit does; sharing this helper must not quietly
/// "align" them.
fn finish_turn_reason<'a>(
    probe: &TurnOutputProbe,
    raw_reason_str: &'a str,
    stderr_tail: &StderrTail,
) -> (&'a str, Option<EmptyTurnReport>) {
    if raw_reason_str != "end_turn" || probe.saw_agent_output {
        return (raw_reason_str, None);
    }
    let cause = diagnose_empty_turn(probe);
    let details = build_empty_turn_details(probe, stderr_tail);
    ("empty", Some(EmptyTurnReport { cause, details }))
}

/// Build an `AcpEvent::Error` for a non-success stop reason so the user gets a
/// toast instead of a silent transition to `PendingReview`. Returns `None` for
/// `end_turn` (success) and `cancelled` (already user-driven).
///
/// `Refusal` is included because OpenCode (and similar agents) map backend /
/// gateway errors to `Refusal` per the ACP spec gap — see
/// <https://shashikantjagtap.net/openclaw-acp-what-coding-agent-users-need-to-know-about-protocol-gaps/>.
/// `empty` is a synthesized reason emitted by `run_conversation_loop` when the
/// agent reports `EndTurn` without producing any agent output; `empty` carries
/// an `EmptyTurnReport` that refines the code and attaches redacted evidence.
fn turn_failure_error_event(
    reason_str: &str,
    agent_type: AgentType,
    empty: Option<&EmptyTurnReport>,
) -> Option<AcpEvent> {
    let (code, message, details) = match reason_str {
        "refusal" => (
            "turn_failed_refusal",
            format!("{agent_type} refused to continue this turn."),
            None,
        ),
        "max_tokens" => (
            "turn_failed_max_tokens",
            format!("{agent_type} reached the maximum token limit for this turn."),
            None,
        ),
        "max_turn_requests" => (
            "turn_failed_max_turn_requests",
            format!("{agent_type} reached the maximum number of allowed requests for this turn."),
            None,
        ),
        "unknown" => (
            "turn_failed_unknown",
            format!("{agent_type} ended the turn with an unrecognized stop reason."),
            None,
        ),
        "empty" => {
            // A missing report can only happen on a path that didn't run the
            // diagnosis; fall back to the pre-split behavior rather than
            // dropping the error entirely.
            let cause = empty.map(|r| r.cause).unwrap_or(EmptyTurnCause::NoOutput);
            (
                cause.code(),
                cause.message(agent_type),
                empty.and_then(|r| r.details.clone()),
            )
        }
        _ => return None,
    };
    Some(AcpEvent::Error {
        message,
        agent_type: agent_type.to_string(),
        code: Some(code.to_string()),
        details,
        // Non-terminal: this Error is paired with a `TurnComplete`
        // carrying the same stop reason. The connection stays alive and
        // the broker's pending entry is drained by `complete_call` with
        // the correct child-side mapping (`ChildRefusal` /
        // `ChildMaxTokens` / …). See F1 in the v0.14.3 sub-agent
        // delegation post-mortem.
        terminal: false,
    })
}

/// Returns `Ok(None)` on normal exit (disconnect / channel closed) or
/// `Ok(Some(ForkExitInfo))` when the loop should be restarted on a forked session.
#[allow(clippy::too_many_arguments)]
async fn run_conversation_loop<'a>(
    session: &mut sacp::ActiveSession<'a, Agent>,
    conn_id: &str,
    emitter: &EventEmitter,
    state: &Arc<RwLock<SessionState>>,
    agent_type: AgentType,
    perms: &PendingPermissions,
    cmd_rx: &mut mpsc::Receiver<ConnectionCommand>,
    terminal_runtime: Arc<TerminalRuntime>,
    cwd: &str,
    supports_fork: bool,
    // Connection-scoped (created once in `run_connection`, shared across fork
    // restarts of this loop): outgoing prompts are fingerprinted here so the
    // transcript watcher can classify their turns as wire-rendered foreground.
    prompt_ledger: &background_watch::PromptLedger,
    // Source of the broker reference used to cascade-cancel pending
    // delegations on parent prompt cancel / non-success TurnComplete.
    // `None` for test paths that don't wire delegation.
    delegation_injection: Option<&DelegationInjection>,
    // Connection-scoped (like `prompt_ledger`): the agent's stderr ring buffer,
    // read at turn end to explain a silent `EndTurn`.
    stderr_tail: &Arc<StderrTail>,
) -> Result<Option<ForkExitInfo>, sacp::Error> {
    // Session-scoped cache for diffing cumulative `raw_output` snapshots
    // into incremental deltas. Shared across the idle loop and the active
    // turn loop so tool calls that span turns stay consistent.
    let mut raw_output_cache = ToolCallOutputCache::default();
    // Session-scoped CodeBuddy live state: authoritative title rewrites
    // (tool_call_id → "agent" / inner `mcp__…` name) so a later status-only
    // update can't downgrade an Agent / delegation card mid-stream, plus the
    // open-sub-agent window used to suppress a sub-agent's interleaved
    // thought/message chunks. See `emit_conversation_update`. Shared across the
    // idle and turn loops.
    let mut cb_state = CodeBuddyLiveState::default();
    // 1-based per-connection turn counter for the timing journal's ordinal
    // (see `turn_timings::TurnTiming::ord`) — incremented for EVERY Cursor
    // prompt turn, journaled or not, so consecutive ordinals prove adjacent
    // turns to the reader.
    let mut cursor_turn_ord: u64 = 0;
    // Session-scoped throttle for the three "we dropped an update we couldn't
    // read" lines below (idle-loop decode, turn-loop decode, turn-loop dispatch).
    //
    // Each fires ONCE PER NOTIFICATION, i.e. per streaming chunk, at WARN — so
    // they are live under the DEFAULT level, and one schema-drifted agent turns
    // them into a firehose. That is exactly the shape that wrote 34GB in 8.8h in
    // the 0.23.3 field report (issue #427). The information they carry is
    // "codeg is dropping this agent's output", which is worth one line per
    // window, not one per token: `TurnOutputProbe` keeps the exact counts and the
    // first redacted error, and surfaces them in the empty-turn diagnosis.
    //
    // One throttle across all three sites on purpose — they are three layers of
    // the same failure (the agent is speaking a protocol we can't read), so the
    // operator wants one signal, not three interleaved ones.
    let mut drop_log_throttle = LeadingEdgeThrottle::new(DROPPED_UPDATE_LOG_WINDOW);
    loop {
        // Wait for either a user command or a session update (e.g. available_commands_update)
        let cmd = loop {
            tokio::select! {
                biased;
                cmd = cmd_rx.recv() => break cmd,
                update = session.read_update() => {
                    match update {
                        Ok(SessionMessage::SessionMessage(dispatch)) => {
                            let h = emitter.clone();
                            let st = Arc::clone(state);
                            let cwd_opt = Some(cwd);
                            let dispatch = fix_usage_update_nulls(dispatch);
                            let _ = MatchDispatch::new(dispatch)
                                .if_notification(
                                    async |notif: SessionNotification| {
                                        emit_conversation_update(&st, &h, agent_type, notif.update, cwd_opt, &mut raw_output_cache, &mut cb_state).await;
                                        Ok(())
                                    },
                                )
                                .await
                                .otherwise(async |dispatch| {
                                    maybe_emit_ext_notification(&st, &h, agent_type, dispatch, &mut cb_state).await;
                                    Ok(())
                                })
                                .await;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            log_dropped_update(&mut drop_log_throttle, "idle", &e);
                        }
                    }
                }
            }
        };
        match cmd {
            Some(ConnectionCommand::Prompt {
                blocks,
                user_message,
            }) => {
                // Fingerprint the outgoing prompt for the background watcher's
                // foreground/out-of-turn classifier BEFORE the blocks are
                // consumed: the transcript record this prompt becomes must
                // classify as wire-rendered foreground, not overlay.
                prompt_ledger.record_prompt_blocks(&blocks);
                // Cursor's ACP store carries no per-turn timestamps at all
                // (see `crate::turn_timings`), so codeg journals its own
                // observation of the turn span: hash + ordinal here (before
                // the blocks are consumed), the send stamp after the
                // `UserMessage` broadcast below, the append at TurnComplete.
                // The hash of the outgoing text blocks is what the history
                // parser correlates its user turns against; the ordinal is
                // its contiguity anchor (every turn consumes one, journaled
                // or not).
                let turn_timing_prep = matches!(agent_type, AgentType::Cursor).then(|| {
                    cursor_turn_ord += 1;
                    let text: String = blocks
                        .iter()
                        .filter_map(|b| match b {
                            PromptInputBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    (crate::turn_timings::prompt_hash(&text), cursor_turn_ord)
                });
                // Grok: settle each image onto the carriage grok can read —
                // decodable ones as native Image blocks (so its describe
                // sidecar runs), the rest back as resource blobs. The last
                // point that sees the blocks, so every producer (composer,
                // queued draft, work task, delegation) is covered at once.
                let blocks = if agent_type == AgentType::Grok {
                    normalize_grok_image_blocks(blocks)
                } else {
                    blocks
                };
                let prompt_blocks = map_prompt_blocks(blocks);
                if prompt_blocks.is_empty() {
                    // Defensive: the manager rejects empty prompts before the
                    // concurrency gate is set / the command is enqueued (see
                    // `send_prompt_inner`), and `map_prompt_blocks` is 1:1, so an
                    // empty prompt should never reach here. If one ever did, it
                    // would carry no turn-in-flight gate, so just surface the
                    // error and keep the idle loop alive.
                    emit_with_state(
                        state,
                        emitter,
                        AcpEvent::Error {
                            message: "Prompt must contain at least one content block".into(),
                            agent_type: agent_type.to_string(),
                            code: None,
                            details: None,
                            // Recoverable: idle loop continues, awaiting the
                            // next user command. Connection stays alive.
                            terminal: false,
                        },
                    )
                    .await;
                    continue;
                }

                emit_with_state(
                    state,
                    emitter,
                    AcpEvent::StatusChanged {
                        status: ConnectionStatus::Prompting,
                    },
                )
                .await;

                // Broadcast the user's prompt to cross-client viewers BEFORE
                // issuing the agent request. Emitting here (rather than at the
                // manager enqueue site) guarantees its seq strictly precedes the
                // turn's assistant/status events — viewers apply events in seq
                // order, so otherwise the reply could render above the message.
                // It also means a prompt that is never processed (rejected /
                // dropped) broadcasts nothing. `apply_event` records it as
                // `pending_user_message` so a client attaching mid-turn still
                // renders the user turn from the snapshot.
                if let Some((message_id, blocks)) = user_message {
                    emit_with_state(state, emitter, AcpEvent::UserMessage { message_id, blocks })
                        .await;
                }

                // Stamp the journal's turn start AFTER the `UserMessage`
                // broadcast: `apply_in_flight_message_id`'s recency gate
                // compares parsed user-turn timestamps — which the journal
                // upgrade rewrites to this stamp — against the broadcast's
                // application instant (`pending_user_message_started_at`,
                // stored at millisecond precision for exactly this
                // comparison). `emit_with_state` applies the event before
                // returning, so this stamp is never earlier than the gate's
                // threshold and the in-flight user turn stays stampable in
                // the journal-written-but-turn-still-pending window.
                let mut turn_timing_probe = turn_timing_prep.map(|(prompt_sha, ord)| {
                    (crate::turn_timings::now_epoch_ms(), prompt_sha, ord)
                });

                // Clone connection and session ID before entering the
                // select loop so we can send CancelNotification without
                // conflicting with session.read_update()'s mutable borrow.
                let cx = session.connection();
                let sid = session.session_id().clone();
                // Record the prompt BEFORE sending, so the transcript's line
                // order matches the wire order even if the agent replies
                // instantly — and awaited, so the replay gate can never see
                // this conversation as transcript-less (see `record_prompt`).
                record_prompt(agent_type, &sid.0, &prompt_blocks).await;
                let turn_started_at_ms = crate::acp_transcript::now_epoch_ms();
                let prompt_request = PromptRequest::new(sid.clone(), prompt_blocks);
                // Snapshot the stderr write position BEFORE the request is
                // dispatched. An agent that fails the moment the prompt lands
                // (bad credentials, unknown model) prints its error almost
                // immediately; marking after dispatch would race those lines
                // out of "this turn" and demote the most relevant evidence to a
                // stale-looking `recent` fallback.
                let stderr_mark = stderr_tail.mark();
                // Use Box::pin (heap) instead of tokio::pin! (stack) so the
                // future can be moved into a background task on cancel.
                let mut prompt_response = Box::pin(
                    cx.clone()
                        .send_request_to(Agent, prompt_request)
                        .block_task(),
                );
                let mut tracked_terminal_tool_calls: HashMap<String, TrackedTerminalToolCall> =
                    HashMap::new();
                let mut terminal_poll_interval = tokio::time::interval(
                    std::time::Duration::from_millis(TERMINAL_POLL_INTERVAL_MS),
                );
                terminal_poll_interval
                    .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut disconnect_requested = false;
                // What this turn was observed to produce. When an agent reports
                // `EndTurn` having produced no real output, we synthesize an
                // `"empty"` stop reason so the user gets an error toast instead
                // of a confusing `PendingReview` on a blank conversation — and
                // the probe's other fields say *why* it looked empty.
                let mut probe = TurnOutputProbe::new(stderr_mark);
                // A CodeBuddy native sub-agent's full lifecycle (Agent tool call
                // open → completed) happens within one turn, so reset the
                // suppression window at each turn start. This bounds the tracking
                // sets and guarantees a sub-agent that ended without a terminal
                // frame (cancel/abort) can never suppress the NEXT turn's
                // main-agent thinking. `title_overrides` intentionally persists
                // (a card's identity is session-stable).
                cb_state.open_subagents.clear();
                cb_state.closed_subagents.clear();
                // Grok spawn bookkeeping scoped to one turn: progress
                // eligibility (a prior turn's background child must never tick
                // into THIS turn's live message) and the un-paired pending
                // queue (a spawn whose `subagent_spawned` never arrived —
                // aborted turn — must not mispair with the next turn's first
                // spawn). The subagent→call map, seen and settled sets persist:
                // background children legitimately span turns.
                cb_state.grok_progress_eligible.clear();
                cb_state.grok_pending_spawn_ids.clear();
                // Grok's context ring needs the active model's window paired
                // with the cumulative token count riding each update. Resolve it
                // once here (the model can't change mid-turn) so the per-update
                // peek below never touches the state lock. A switch since the
                // last turn changes the denominator, so re-key the live pair to
                // the new window right away rather than leaving the previous
                // model's ring on screen until this turn's first token count.
                if agent_type == AgentType::Grok {
                    let window = grok_current_model_context_window(state).await;
                    cb_state.grok_turn_context_window = window;
                    if let Some((used, size)) =
                        grok_window_change_usage(window, cb_state.grok_last_usage)
                    {
                        cb_state.grok_last_usage = Some((used, size));
                        emit_with_state(state, emitter, AcpEvent::UsageUpdate { used, size }).await;
                    }
                }

                // Read updates until turn completes.
                // We must also listen for commands (e.g. RespondPermission)
                // to avoid deadlocking when the agent awaits a permission response.
                loop {
                    tokio::select! {
                        update = session.read_update() => {
                            let update = match update {
                                Ok(u) => u,
                                Err(e) => {
                                    // Silent-drop site #1 (transport/decode).
                                    // Record it: an agent whose output we
                                    // couldn't decode looks identical to one
                                    // that said nothing, and the two need
                                    // completely different fixes.
                                    probe.note_dropped(DropSite::Decode, &e);
                                    log_dropped_update(
                                        &mut drop_log_throttle,
                                        DropSite::Decode.label(),
                                        &e,
                                    );
                                    continue;
                                }
                            };
                            match update {
                                SessionMessage::SessionMessage(dispatch) => {
                                    let h = emitter.clone();
                                    let st = Arc::clone(state);
                                    let runtime = terminal_runtime.clone();
                                    let session_id = sid.clone();
                                    let cwd_opt = Some(cwd);
                                    let dispatch = fix_usage_update_nulls(dispatch);
                                    // grok reports `/compact` results on ext methods
                                    // that bypass the typed pipeline below and emit a
                                    // compaction card/error from `.otherwise`. Count
                                    // that as turn output up front (the dispatch is
                                    // about to be consumed) so a compaction-only turn
                                    // isn't misclassified as `"empty"` at turn end.
                                    if grok_ext_notification_is_turn_output(&dispatch, agent_type) {
                                        probe.saw_agent_output = true;
                                    }
                                    // Grok has no `usage_update` channel; its
                                    // cumulative token count rides the outer
                                    // `_meta` of ordinary updates. Peek it before
                                    // the typed pipeline consumes the dispatch
                                    // (which drops that `_meta`) so the composer
                                    // ring tracks the turn as it streams.
                                    if let Some((used, size)) = grok_live_usage_step(
                                        &dispatch,
                                        agent_type,
                                        cb_state.grok_turn_context_window,
                                        cb_state.grok_last_usage,
                                    ) {
                                        cb_state.grok_last_usage = Some((used, size));
                                        emit_with_state(
                                            state,
                                            emitter,
                                            AcpEvent::UsageUpdate { used, size },
                                        )
                                        .await;
                                    }
                                    if let Err(e) = MatchDispatch::new(dispatch)
                                        .if_notification(
                                            async |notif: SessionNotification| {
                                                // Body lives in a named `-> ()`
                                                // function so this closure is
                                                // infallible by construction —
                                                // see `handle_turn_notification`.
                                                handle_turn_notification(
                                                    notif,
                                                    agent_type,
                                                    &st,
                                                    &h,
                                                    runtime.as_ref(),
                                                    &session_id,
                                                    cwd_opt,
                                                    &mut tracked_terminal_tool_calls,
                                                    &mut raw_output_cache,
                                                    &mut cb_state,
                                                    &mut probe,
                                                )
                                                .await;
                                                Ok(())
                                            },
                                        )
                                        .await
                                        .otherwise(async |dispatch| {
                                            maybe_emit_ext_notification(&st, &h, agent_type, dispatch, &mut cb_state).await;
                                            Ok(())
                                        })
                                        .await
                                    {
                                        // Silent-drop site #2 (dispatch/params).
                                        // Both handler closures are infallible
                                        // by construction, so this `Err` can
                                        // only be a typed-deserialization
                                        // failure — i.e. ACP schema drift.
                                        probe.note_dropped(DropSite::Dispatch, &e);
                                        log_dropped_update(
                                            &mut drop_log_throttle,
                                            DropSite::Dispatch.label(),
                                            &e,
                                        );
                                    }
                                }
                                SessionMessage::StopReason(reason) => {
                                    if !tracked_terminal_tool_calls.is_empty() {
                                        poll_tracked_terminal_tool_calls(
                                            terminal_runtime.as_ref(),
                                            &sid,
                                            state,
                                            emitter,
                                            &mut tracked_terminal_tool_calls,
                                        )
                                        .await;
                                    }
                                    let raw_reason_str = stop_reason_to_str(reason);
                                    // Pure: resolves the reason and (for an
                                    // empty turn) its diagnosis. Side effects
                                    // below stay exactly where they were — note
                                    // this exit deliberately does NOT call
                                    // `record_turn_end`, unlike the
                                    // prompt-response exit.
                                    let (reason_str, empty_report) =
                                        finish_turn_reason(&probe, raw_reason_str, stderr_tail);
                                    if let Some(err_event) = turn_failure_error_event(
                                        reason_str,
                                        agent_type,
                                        empty_report.as_ref(),
                                    ) {
                                        emit_with_state(state, emitter, err_event).await;
                                    }
                                    // Clean completions only — a canceled/empty
                                    // turn may be unpersisted (see journal_turn_span).
                                    if reason_str == "end_turn" {
                                        journal_turn_span(&mut turn_timing_probe, conn_id, &sid.0).await;
                                    }
                                    // The turn is over, so any card still parked
                                    // here is moot — `TurnComplete` clears
                                    // `pending_permission` from the snapshot
                                    // unconditionally. Drain and emit as ONE
                                    // critical section so the queue can't keep a
                                    // `showing` id that no `RespondPermission`
                                    // will ever match, which would wedge the
                                    // queue and stop every LATER permission on
                                    // this connection from displaying. A no-op on
                                    // the normal path (an agent blocked on
                                    // approval does not end its turn).
                                    drain_permissions_then_emit(
                                        perms,
                                        state,
                                        emitter,
                                        AcpEvent::TurnComplete {
                                            session_id: sid.0.to_string(),
                                            stop_reason: reason_str.into(),
                                            agent_type: agent_type.to_string(),
                                        },
                                    )
                                    .await;
                                    // Cascade-cancel any pending delegations
                                    // whenever the parent's turn ended for a
                                    // reason other than clean `end_turn`. The
                                    // `end_turn` path lets the legitimate
                                    // delegation completion drain naturally;
                                    // every other reason (cancelled / refusal /
                                    // max_tokens / max_turn_requests / empty /
                                    // unknown) means the parent will never
                                    // consume the in-flight result, so the
                                    // child must be torn down. The connection
                                    // stays alive (only the turn ended), so use
                                    // the turn-scoped cancel that keeps the
                                    // parent's `consumed` tool_call memory — a
                                    // late re-emit must not re-register and
                                    // mis-bind the next same-key delegation.
                                    //
                                    // Await inline: the fast tracker +
                                    // parked-call drain MUST finish before the
                                    // loop accepts the next prompt so it stays
                                    // scoped to the just-ended turn. The broker
                                    // backgrounds the slow child teardown
                                    // (spawner.cancel/disconnect) internally, so
                                    // this won't block on slow agents; its
                                    // idempotent drain also lets the cleanup-
                                    // guard cascade at run_connection exit run
                                    // without race-double-drain.
                                    if reason_str != "end_turn" {
                                        if let Some(inj) = delegation_injection {
                                            inj.broker.cancel_by_parent_turn(conn_id).await;
                                        }
                                    }
                                    break;
                                }
                                _ => {}
                            }
                        }
                        prompt_result = &mut prompt_response => {
                            let response = prompt_result?;
                            // A turn's terminal AIR failure rides on the
                            // response `_meta` (see `response_session_failure`
                            // — the update channel only carries the retry
                            // warnings). Emit it BEFORE `TurnComplete`: the
                            // same-id higher-revision severity-"error" upsert
                            // must land before `apply_event`'s turn-boundary
                            // settle, or the retry warnings it escalates would
                            // be marked recovered while the failure is live.
                            let terminal_failure =
                                response_session_failure(response.meta.as_ref());
                            if let Some(record) = &terminal_failure {
                                emit_with_state(
                                    state,
                                    emitter,
                                    AcpEvent::SessionFailure { record: record.clone() },
                                )
                                .await;
                            }
                            let reason = response.stop_reason;
                            if !tracked_terminal_tool_calls.is_empty() {
                                poll_tracked_terminal_tool_calls(
                                    terminal_runtime.as_ref(),
                                    &sid,
                                    state,
                                    emitter,
                                    &mut tracked_terminal_tool_calls,
                                )
                                .await;
                            }
                            let raw_reason_str = stop_reason_to_str(reason);
                            // Same pure helper as the StopReason-message exit,
                            // so the two can't drift. This exit keeps its own
                            // extra side effect (`record_turn_end` below).
                            //
                            // Exception: a response carrying a typed terminal
                            // ERROR is a failed turn wearing the adapters'
                            // disguised `end_turn` — its blank output is
                            // explained by the AIR banner, so synthesizing an
                            // "empty" toast on top would misdiagnose a dead
                            // connection as "the agent produced nothing".
                            let (reason_str, empty_report) = if terminal_failure
                                .as_ref()
                                .is_some_and(|record| record.severity == "error")
                            {
                                (raw_reason_str, None)
                            } else {
                                finish_turn_reason(&probe, raw_reason_str, stderr_tail)
                            };
                            if let Some(err_event) =
                                turn_failure_error_event(reason_str, agent_type, empty_report.as_ref())
                            {
                                emit_with_state(state, emitter, err_event).await;
                            }
                            // Clean completions only — a canceled/empty turn
                            // may be unpersisted (see journal_turn_span).
                            if reason_str == "end_turn" {
                                journal_turn_span(&mut turn_timing_probe, conn_id, &sid.0).await;
                            }
                            // ACP has no turn-end notification — the stop
                            // reason arrives here, in the prompt RESPONSE — so
                            // codeg records it for the history parser. Unlike
                            // streamed chunks this one is bound-awaited: a
                            // conversation reopened right after a turn must not
                            // miss its tail.
                            record_turn_end(
                                agent_type,
                                &sid.0,
                                reason_str,
                                turn_started_at_ms,
                                current_session_model_id(state).await,
                            )
                            .await;
                            // Same wedge guard as the StopReason-message exit
                            // above — see that comment for why the drain and the
                            // event must share one critical section.
                            drain_permissions_then_emit(
                                perms,
                                state,
                                emitter,
                                AcpEvent::TurnComplete {
                                    session_id: sid.0.to_string(),
                                    stop_reason: reason_str.into(),
                                    agent_type: agent_type.to_string(),
                                },
                            )
                            .await;
                            // Mirror the StopReason-message branch above:
                            // cascade-cancel on any non-`end_turn` reason
                            // so in-flight delegations don't dangle when
                            // the parent's turn ended without consuming
                            // their result. Turn-scoped (connection stays
                            // alive → keep `consumed`) and awaited inline
                            // (fast drain before the next prompt; broker
                            // backgrounds the slow child teardown) for the
                            // same reasons as that branch — see above.
                            if reason_str != "end_turn" {
                                if let Some(inj) = delegation_injection {
                                    inj.broker.cancel_by_parent_turn(conn_id).await;
                                }
                            }
                            break;
                        }
                        _ = terminal_poll_interval.tick(), if !tracked_terminal_tool_calls.is_empty() => {
                            poll_tracked_terminal_tool_calls(
                                terminal_runtime.as_ref(),
                                &sid,
                                state,
                                emitter,
                                &mut tracked_terminal_tool_calls,
                            )
                            .await;
                        }
                        cmd = cmd_rx.recv() => {
                            match cmd {
                                Some(ConnectionCommand::RespondPermission {
                                    request_id,
                                    option_id,
                                }) => {
                                    resolve_permission(
                                        perms, state, emitter, request_id, option_id,
                                    )
                                    .await;
                                }
                                Some(ConnectionCommand::SetMode { mode_id }) => {
                                    let req = SetSessionModeRequest::new(sid.clone(), mode_id.clone());
                                    match cx.send_request_to(Agent, req).block_task().await {
                                        Ok(_) => {
                                            emit_with_state(
                                                state,
                                                emitter,
                                                AcpEvent::ModeChanged { mode_id },
                                            )
                                            .await;
                                        }
                                        Err(e) => {
                                            emit_with_state(
                                                state,
                                                emitter,
                                                AcpEvent::Error {
                                                    message: format!("Failed to set mode: {e}"),
                                                    agent_type: agent_type.to_string(),
                                                    code: None,
                                                    details: None,
                                                    // Recoverable: just a failed mode toggle.
                                                    terminal: false,
                                                },
                                            )
                                            .await;
                                        }
                                    }
                                }
                                Some(ConnectionCommand::SetConfigOption {
                                    config_id,
                                    value_id,
                                }) => {
                                    let set_result = if agent_type == AgentType::Grok {
                                        set_grok_config_option(
                                            &cx, &sid, state, emitter, config_id, value_id,
                                        )
                                        .await
                                    } else {
                                        set_session_config_option(
                                            &cx, &sid, state, emitter, agent_type, config_id,
                                            value_id,
                                        )
                                        .await
                                    };
                                    if let Err(e) = set_result {
                                        emit_with_state(
                                            state,
                                            emitter,
                                            AcpEvent::Error {
                                                message: format!("Failed to set config option: {e}"),
                                                agent_type: agent_type.to_string(),
                                                code: None,
                                                details: None,
                                                // Recoverable: just a failed config-option toggle.
                                                terminal: false,
                                            },
                                        )
                                        .await;
                                    }
                                }
                                Some(ConnectionCommand::GoalControl { action, reply }) => {
                                    let method = state.read().await.goal_control_method.clone();
                                    let landed =
                                        match send_goal_control(&cx, &sid, action, &method).await {
                                            Ok(()) => true,
                                            Err(e) => {
                                                emit_with_state(
                                                    state,
                                                    emitter,
                                                    AcpEvent::Error {
                                                        message: format!(
                                                            "Failed to control goal: {e}"
                                                        ),
                                                        agent_type: agent_type.to_string(),
                                                        code: None,
                                                        details: None,
                                                        // Recoverable: the goal
                                                        // is unchanged and the
                                                        // session is untouched.
                                                        terminal: false,
                                                    },
                                                )
                                                .await;
                                                false
                                            }
                                        };
                                    if let Some(reply) = reply {
                                        // A dead receiver is fine — the caller
                                        // that wanted to follow up with an
                                        // interrupt went away, and the control
                                        // itself already happened.
                                        let _ = reply.send(landed);
                                    }
                                }
                                Some(ConnectionCommand::Steer { text, reply }) => {
                                    // Protocol round-trip only — the manager's
                                    // cancellation-shielded task records the
                                    // note + broadcasts `FeedbackSubmitted`
                                    // once this outcome arrives (Fork's
                                    // protocol/persistence split). Awaiting
                                    // inline matches SetMode/SetConfigOption:
                                    // sacp pumps I/O on its own task, so the
                                    // round-trip only defers other queued
                                    // commands, not session updates. A dead
                                    // receiver is fine — the reply is then
                                    // moot (teardown), nothing to unwind.
                                    let outcome = send_steer_request(&cx, &sid, &text).await;
                                    // A steered message still lands in the
                                    // agent's OWN transcript as a user record,
                                    // which `group_into_turns` reads as the
                                    // start of a turn. Fingerprint it so the
                                    // background watcher classifies that turn
                                    // as wire-rendered foreground: the owning
                                    // prompt stays in flight across the steered
                                    // work (claude-agent-acp #958), so all of
                                    // it already streams into the live turn —
                                    // surfacing it as overlay activity too
                                    // renders it twice and reorders the
                                    // transcript as the upserts land.
                                    //
                                    // `Injected` ONLY: `PromptRequired` leaves
                                    // the content unconsumed (the caller
                                    // resends it as a real prompt, which
                                    // fingerprints itself, and a stale entry
                                    // would swallow a same-text out-of-turn
                                    // refire for the whole ledger TTL), and a
                                    // `StartedNewTurn` genuinely runs detached
                                    // — the overlay is the only place its work
                                    // can surface at all.
                                    if matches!(outcome, Ok(SteerOutcome::Injected)) {
                                        prompt_ledger.record_text(&text);
                                    }
                                    let _ = reply.send(outcome);
                                }
                                Some(ConnectionCommand::Cancel) => {
                                    // Send CancelNotification to agent to stop the current turn
                                    let _ = cx.send_notification_to(
                                        Agent,
                                        CancelNotification::new(sid.clone()),
                                    );
                                    // Also terminate any command runtimes created for this
                                    // session so cancellation does not hang on long-running
                                    // terminal tools.
                                    terminal_runtime
                                        .release_all_for_session(sid.0.as_ref())
                                        .await;
                                    tracked_terminal_tool_calls.clear();
                                    // Also cancel any pending permission requests
                                    // (queued ones included), clearing the card
                                    // that was on screen, and immediately emit
                                    // TurnComplete so the frontend transitions out
                                    // of "prompting" and the user can send new
                                    // messages. Don't wait for the agent -- it may
                                    // be slow to respond or not respond at all.
                                    // One critical section for the same reason as
                                    // the turn-end exits: a request admitted
                                    // between the two would be published and then
                                    // silently un-displayed by TurnComplete,
                                    // wedging the queue.
                                    drain_permissions_then_emit(
                                        perms,
                                        state,
                                        emitter,
                                        AcpEvent::TurnComplete {
                                            session_id: sid.0.to_string(),
                                            stop_reason: "cancelled".into(),
                                            agent_type: agent_type.to_string(),
                                        },
                                    )
                                    .await;
                                    // Cascade-cancel any in-flight delegations owned by
                                    // this parent connection. Idempotent with the
                                    // cleanup-guard cancel_by_parent at the end of
                                    // run_connection (#1: empty pending → no-op).
                                    // Without this, a user-initiated cancel of a parent
                                    // prompt mid-delegation would leave the child agent
                                    // running indefinitely (broker no longer applies a
                                    // timeout; only an MCP `notifications/cancelled` or
                                    // a parent/child disconnect would otherwise tear
                                    // the delegation down). Turn-scoped: the
                                    // connection stays alive after a prompt cancel,
                                    // so keep the parent's `consumed` tool_call
                                    // memory (a re-emit must not mis-bind the next
                                    // same-key delegation); the cleanup-guard
                                    // teardown still clears everything when the
                                    // connection finally goes away.
                                    //
                                    // Await inline so the fast tracker +
                                    // parked-call drain is ordered before the
                                    // next prompt (keeping it scoped to the
                                    // just-ended turn); the broker backgrounds
                                    // the slow child teardown internally, so the
                                    // user-visible Cancel path doesn't wait on
                                    // (potentially slow) child agent teardown.
                                    // The user already saw the parent's
                                    // TurnComplete above, and the broker's
                                    // drain-first lock guarantees no double
                                    // DelegationCompleted emit.
                                    if let Some(inj) = delegation_injection {
                                        inj.broker.cancel_by_parent_turn(conn_id).await;
                                        // Reclaim any parked `ask_user_question` /
                                        // Grok `exit_plan_mode` approval owned by this
                                        // connection. Unlike `perms` (drained inline
                                        // above), these registries live on the manager
                                        // and are otherwise only drained on full
                                        // connection teardown -- but a turn-scoped
                                        // Cancel keeps the connection alive. Without
                                        // this the entry lingers, and the
                                        // one-pending-per-connection guard in
                                        // `register_{question,plan_approval}` then
                                        // silently rejects the NEXT one on this
                                        // connection (its card never shows). Idempotent
                                        // with the cleanup-guard drain (empty map ->
                                        // no-op); the dropped sender declines the tool /
                                        // replies disconnect, exactly like the
                                        // permission drain above.
                                        inj.questions.cancel_questions_by_parent(conn_id).await;
                                        inj.plan_approvals
                                            .cancel_plan_approvals_by_parent(conn_id)
                                            .await;
                                    }
                                    // Drain the prompt response in the background so
                                    // the SACP library doesn't log "receiver dropped"
                                    // errors when the agent eventually responds.
                                    tokio::spawn(async move {
                                        let _ = prompt_response.await;
                                    });
                                    break;
                                }
                                Some(ConnectionCommand::Disconnect) | None => {
                                    tracing::info!(
                                        "[ACP] disconnect requested during prompting; connection_id={conn_id}"
                                    );
                                    let _ = cx.send_notification_to(
                                        Agent,
                                        CancelNotification::new(sid.clone()),
                                    );
                                    terminal_runtime
                                        .release_all_for_session(sid.0.as_ref())
                                        .await;
                                    tracked_terminal_tool_calls.clear();
                                    drain_permissions(perms, state, emitter).await;
                                    disconnect_requested = true;
                                    break;
                                }
                                Some(ConnectionCommand::Prompt { .. }) => {
                                    // Tripwire, not a user-facing case. The
                                    // `turn_in_flight` gate in `send_prompt_inner`
                                    // rejects a second prompt BEFORE it is ever
                                    // enqueued, so a `Prompt` reaching this mid-turn
                                    // command handler means an ungated sender slipped
                                    // past the gate (a broken invariant) — surface it
                                    // at `warn` instead of letting the `_ => {}` below
                                    // swallow it silently.
                                    tracing::warn!(
                                        connection_id = %conn_id,
                                        "[ACP] in-turn Prompt DROPPED — the turn_in_flight gate should have rejected this"
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                }

                if disconnect_requested {
                    tracing::info!(
                        "[ACP] closing connection loop after disconnect; connection_id={conn_id}"
                    );
                    break;
                }

                emit_with_state(
                    state,
                    emitter,
                    AcpEvent::StatusChanged {
                        status: ConnectionStatus::Connected,
                    },
                )
                .await;
            }
            Some(ConnectionCommand::RespondPermission {
                request_id,
                option_id,
            }) => {
                resolve_permission(perms, state, emitter, request_id, option_id).await;
            }
            Some(ConnectionCommand::SetMode { mode_id }) => {
                if let Err(e) = set_session_mode(session, state, emitter, mode_id).await {
                    emit_with_state(
                        state,
                        emitter,
                        AcpEvent::Error {
                            message: format!("Failed to set mode: {e}"),
                            agent_type: agent_type.to_string(),
                            code: None,
                            details: None,
                            // Recoverable: idle SetMode failure leaves the
                            // connection alive — same rationale as the
                            // mid-prompt SetMode site above.
                            terminal: false,
                        },
                    )
                    .await;
                }
            }
            Some(ConnectionCommand::SetConfigOption {
                config_id,
                value_id,
            }) => {
                let cx = session.connection();
                let sid = session.session_id().clone();
                let set_result = if agent_type == AgentType::Grok {
                    set_grok_config_option(&cx, &sid, state, emitter, config_id, value_id).await
                } else {
                    set_session_config_option(
                        &cx, &sid, state, emitter, agent_type, config_id, value_id,
                    )
                    .await
                };
                if let Err(e) = set_result {
                    emit_with_state(
                        state,
                        emitter,
                        AcpEvent::Error {
                            message: format!("Failed to set config option: {e}"),
                            agent_type: agent_type.to_string(),
                            code: None,
                            details: None,
                            // Recoverable: idle SetConfigOption failure leaves
                            // the connection alive.
                            terminal: false,
                        },
                    )
                    .await;
                }
            }
            Some(ConnectionCommand::GoalControl { action, reply }) => {
                let cx = session.connection();
                let sid = session.session_id().clone();
                let method = state.read().await.goal_control_method.clone();
                let landed = match send_goal_control(&cx, &sid, action, &method).await {
                    Ok(()) => true,
                    Err(e) => {
                        emit_with_state(
                            state,
                            emitter,
                            AcpEvent::Error {
                                message: format!("Failed to control goal: {e}"),
                                agent_type: agent_type.to_string(),
                                code: None,
                                details: None,
                                // Recoverable: an idle pause/clear failure leaves the
                                // connection alive.
                                terminal: false,
                            },
                        )
                        .await;
                        false
                    }
                };
                if let Some(reply) = reply {
                    // Reply — never drop — for the same reason the Steer arm
                    // below does: the manager awaits this oneshot to decide
                    // whether to follow up, and a dropped sender would hang it.
                    // (Its follow-up is an interrupt, which this idle arm's
                    // caller won't get anyway: there is no turn to stop.)
                    let _ = reply.send(landed);
                }
            }
            Some(ConnectionCommand::Steer { text: _, reply }) => {
                // Steering only means something for a RUNNING turn. Reply —
                // never drop — so the manager's shielded task can't hang on
                // the oneshot; the caller falls back to a normal prompt (the
                // same reroute the frontend already has for a turn-end race).
                let _ = reply.send(Err(AcpError::NoActiveTurn));
            }
            Some(ConnectionCommand::Cancel) => {
                let cx = session.connection();
                let sid = session.session_id().clone();
                let _ = cx.send_notification_to(Agent, CancelNotification::new(sid.clone()));
                terminal_runtime
                    .release_all_for_session(sid.0.as_ref())
                    .await;
                // Unlike the mid-turn Cancel branch, this one does NOT emit
                // `TurnComplete` (there is no turn), so nothing else would ever
                // clear the on-screen permission card — before the compensating
                // `PermissionResolved` inside `drain_permissions`, an idle Cancel
                // left a card up on every client with its responder already
                // cancelled (clicking it did nothing) and pinned a work task at
                // `awaiting_input` forever, because `PermissionResolved` was only
                // emitted from the `RespondPermission` path.
                drain_permissions(perms, state, emitter).await;
                // Cascade-cancel any pending delegations owned by this parent.
                // Reached when Cancel arrives between prompts (idle path); the
                // inner Cancel handler covers mid-prompt. Both must trigger
                // because the per-prompt cancel path doesn't tear down the
                // parent connection, so the cleanup-guard cancel_by_parent
                // at run_connection's exit wouldn't fire. Turn-scoped for that
                // same reason: the connection stays alive, so keep the parent's
                // `consumed` tool_call memory (a re-emit must not mis-bind the
                // next same-key delegation).
                //
                // Awaited inline (fast drain before the next prompt; broker
                // backgrounds the slow child teardown): see inner Cancel
                // handler above for rationale.
                if let Some(inj) = delegation_injection {
                    inj.broker.cancel_by_parent_turn(conn_id).await;
                }
            }
            Some(ConnectionCommand::Fork { reply }) => {
                if !supports_fork {
                    let _ = reply.send(Err(AcpError::protocol(
                        "This agent does not support session/fork".to_string(),
                    )));
                    continue;
                }
                let cx = session.connection();
                let sid = session.session_id().clone();
                tracing::info!(
                    "[ACP] Sending session/fork for session_id={} cwd={}",
                    sid.0, cwd
                );
                let result = crate::acp::fork::fork_session(&cx, &sid, cwd).await;
                match result {
                    Ok((fork_response, fork_models_raw)) => {
                        tracing::info!(
                            "[ACP] Fork succeeded: new_session_id={}",
                            fork_response.session_id.0
                        );
                        return Ok(Some(ForkExitInfo {
                            fork_response,
                            fork_models_raw,
                            original_session_id: sid.0.to_string(),
                            reply,
                            connection: cx,
                        }));
                    }
                    Err(e) => {
                        tracing::error!("[ACP] Fork failed: {e}");
                        let _ = reply.send(Err(e));
                    }
                }
            }
            Some(ConnectionCommand::Disconnect) | None => {
                break;
            }
        }
    }
    Ok(None)
}

/// Serialize tool-call `content` blocks into a single human-readable string.
///
/// `include_diffs = false` skips `Diff` blocks. Used when the edit has been
/// hoisted into a synthesized canonical `raw_input` (see
/// `synthesize_edit_input_from_diffs`): without this the same edit ships twice
/// (doubling the event) and the hunkless full-file `--- /+++` blob stays in the
/// tool `output`, where `extractEditLineChangeStats` mis-counts it as full-file
/// +/- totals in the card header even though the body shows the compact diff.
pub(crate) fn serialize_tool_call_content(
    content: &[ToolCallContent],
    include_diffs: bool,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for item in content {
        match item {
            ToolCallContent::Content(c) => {
                if let ContentBlock::Text(text) = &c.content {
                    parts.push(text.text.clone());
                }
            }
            ToolCallContent::Diff(diff) if include_diffs => {
                let path = diff.path.display();
                let mut diff_text = format!("--- {path}\n+++ {path}\n");
                if let Some(old) = &diff.old_text {
                    for line in old.lines() {
                        diff_text.push_str(&format!("-{line}\n"));
                    }
                }
                for line in diff.new_text.lines() {
                    diff_text.push_str(&format!("+{line}\n"));
                }
                parts.push(diff_text);
            }
            ToolCallContent::Terminal(t) => {
                parts.push(format!("[Terminal: {}]", t.terminal_id));
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Synthesize a canonical edit `raw_input` from `ToolCallContent::Diff` block(s).
///
/// codex-acp reports file edits as ACP `Diff` content blocks and leaves
/// `raw_input` empty — the edit lives only in `content`, and the ACP `title` is
/// the diff header `--- <path>`. With no `raw_input` the frontend classifier
/// (`inferLiveToolName`) falls back to `normalizeToolName(title)`, which returns
/// unrecognized strings verbatim, so the tool call renders as a generic tool
/// literally *named* `--- <path>` (wrench icon, raw header as the title) instead
/// of an edit card. The historical path is unaffected because the JSONL parser
/// stores codex's native `*** Begin Patch` text.
///
/// Reconstructing from the already-serialized `--- /+++` string would be lossy
/// (content lines beginning with `-`/`+`/`---`/`+++`, the old/new boundary,
/// CRLF). Here the structured `Diff` is still intact, so map it losslessly:
/// - exactly one Diff  -> `{"file_path","old_string","new_string"}`
/// - multiple Diffs    -> `{"changes":{"<path>":{"old_text","new_text"},…}}`
///
/// Both shapes classify as `"edit"` (`inferFromInput`) and render through the
/// existing `EditToolInput` / `EditChangesToolInput` → `generateUnifiedDiff`
/// pipeline (a real hunk diff, minimal even for full-file old/new). Returns
/// `None` when `content` carries no `Diff`, so callers only fall back to it when
/// the agent supplied no `raw_input` of its own.
pub(crate) fn synthesize_edit_input_from_diffs(content: &[ToolCallContent]) -> Option<String> {
    // Keep `old_text` as `Option`: ACP reports `None` for a newly created file
    // (`Diff.old_text` semantics). That distinction is the whole point of this
    // function's fix — collapsing `None` to `""` and emitting an edit shape
    // makes the frontend build a `--- a/<path>` diff, which `isAddedFileDiff`
    // does NOT match, so a freshly created file mis-renders as a modification
    // (the historical apply_patch `*** Add File:` path classifies it correctly).
    let diffs: Vec<(String, Option<String>, String)> = content
        .iter()
        .filter_map(|item| match item {
            ToolCallContent::Diff(diff) => Some((
                diff.path.display().to_string(),
                diff.old_text.clone(),
                diff.new_text.clone(),
            )),
            _ => None,
        })
        .collect();

    match diffs.as_slice() {
        [] => None,
        // New file (old_text absent) → write shape. `inferFromInput` classifies
        // `{file_path, content}` as `write`, whose diff builder emits the
        // `--- /dev/null` header `isAddedFileDiff` keys on → renders as a new
        // file, matching the reloaded-from-DB path.
        [(path, None, new)] => Some(
            serde_json::json!({
                "file_path": path,
                "content": new,
            })
            .to_string(),
        ),
        // Edit → canonical `{old_string,new_string}` for the frontend's
        // `generateUnifiedDiff` (a real hunk diff, minimal even for full-file
        // old/new).
        [(path, Some(old), new)] => Some(
            serde_json::json!({
                "file_path": path,
                "old_string": old,
                "new_string": new,
            })
            .to_string(),
        ),
        many => {
            let mut changes = serde_json::Map::new();
            for (path, old, new) in many {
                // Per-entry, mirror the single-diff split: a new file gets a
                // ready-made creation diff (`buildChunkFromEditChange` returns
                // it verbatim → `--- /dev/null` → new file); an edit hands
                // old/new text to the frontend to diff.
                let entry = match old {
                    None => serde_json::json!({ "diff": build_new_file_diff(path, new) }),
                    Some(old) => serde_json::json!({ "old_text": old, "new_text": new }),
                };
                changes.insert(path.clone(), entry);
            }
            Some(serde_json::json!({ "changes": changes }).to_string())
        }
    }
}

/// Build a minimal unified diff for a newly created file: the `--- /dev/null`
/// header the frontend's `isAddedFileDiff` keys on, then every line of
/// `new_text` as an addition. Byte-for-byte identical to the frontend `write`
/// op's diff builder (`session-files.ts`), so a multi-file batch's new-file
/// entries render exactly like a single-file creation.
fn build_new_file_diff(path: &str, new_text: &str) -> String {
    // `split('\n')` (not `lines()`) mirrors the frontend `content.split("\n")`:
    // it keeps the trailing empty segment from a final newline, so the `+N`
    // count and the trailing `+` addition line match exactly.
    let lines: Vec<&str> = new_text.split('\n').collect();
    let mut out = format!(
        "--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@",
        lines.len()
    );
    for line in lines {
        out.push('\n');
        out.push('+');
        out.push_str(line);
    }
    out
}

/// Extract `ContentBlock::Image` payloads from a `ToolCallContent` slice.
/// Returns `None` when no images are present so the upstream `images` field
/// on `AcpEvent::ToolCall(Update)` stays absent for non-image tool calls
/// (preserves replace-on-update semantics: an absent field means "keep
/// prior", a `Some(vec)` replaces).
pub(crate) fn extract_tool_call_images(content: &[ToolCallContent]) -> Option<Vec<ToolCallImageInfo>> {
    let mut imgs: Vec<ToolCallImageInfo> = Vec::new();
    for item in content {
        if let ToolCallContent::Content(c) = item {
            if let ContentBlock::Image(img) = &c.content {
                imgs.push(ToolCallImageInfo {
                    data: img.data.clone(),
                    mime_type: img.mime_type.clone(),
                    uri: img.uri.clone(),
                });
            }
        }
    }
    if imgs.is_empty() {
        None
    } else {
        Some(imgs)
    }
}

/// If the output looks like numbered lines (`   115→content`), strip them
/// and return `{"start_line":N,"content":"..."}` — same as the historical path.
fn structurize_live_output(text: &str) -> String {
    if let Some(json) = crate::parsers::strip_numbered_lines(text) {
        return json;
    }
    text.to_string()
}

/// Resolve line numbers for live tool call input.
///
/// Resolve line numbers for live tool call input (string form).
///
/// - For apply_patch with bare `@@`: resolve line numbers in place.
/// - For canonical edit JSON: inject `_start_line`.
fn resolve_live_tool_input(text: &str, cwd: Option<&str>) -> String {
    if text.contains("@@\n") || text.contains("@@\r\n") {
        if let Some(resolved) = crate::parsers::resolve_patch_text(text, cwd) {
            return resolved;
        }
    }
    if let Ok(mut parsed) = serde_json::from_str::<serde_json::Value>(text) {
        if inject_start_line(&mut parsed, cwd) {
            return parsed.to_string();
        }
    }
    text.to_string()
}

/// Try to inject `_start_line` into a JSON object with `file_path` + `old_string`.
/// Returns true if injected.
fn inject_start_line(value: &mut serde_json::Value, cwd: Option<&str>) -> bool {
    let obj = match value.as_object_mut() {
        Some(o) => o,
        None => return false,
    };
    let fp = obj
        .get("file_path")
        .or_else(|| obj.get("path"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let old_str = obj
        .get("old_string")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let (Some(fp), Some(old_str)) = (fp, old_str) {
        if let Some(sl) = find_string_start_line(&fp, &old_str, cwd) {
            obj.insert("_start_line".to_string(), serde_json::json!(sl));
            return true;
        }
    }
    false
}

/// Find the 1-based start line of `needle` in the file at `path`.
fn find_string_start_line(path: &str, needle: &str, cwd: Option<&str>) -> Option<u64> {
    if needle.is_empty() {
        return None;
    }
    let file_lines = crate::parsers::load_file_lines(path, cwd)?;
    let file_content = file_lines.join("\n");
    let byte_offset = file_content.find(needle)?;
    Some(file_content[..byte_offset].matches('\n').count() as u64 + 1)
}

pub(crate) fn json_value_to_text(val: &Option<serde_json::Value>) -> Option<String> {
    match val {
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(v) if !v.is_null() => Some(v.to_string()),
        _ => None,
    }
}

/// Resolve the live `raw_output` string for a Grok tool call.
///
/// Grok reports terminal output in the standard `content[]` channel (clean,
/// human-readable text) AND in a structured `rawOutput` object whose readable
/// text lives only in the string `output_for_prompt` (its `output` field is a
/// raw byte array, and the remaining keys — `command`, `exit_code`, … — are
/// metadata). Feeding that object through `json_value_to_text` stringifies the
/// whole thing into a JSON blob that (a) shadows the clean `content` — the live
/// renderer's `raw_output_chunks` win over `content`
/// (`conversation-runtime-store.ts`) — and (b) is then dropped by the terminal
/// renderer as a metadata-only "command envelope"
/// (`commandOutputFromJsonString` returns `""`), so a finished command shows no
/// result during live streaming even though the history parser renders it fine.
///
/// Mirror the history parser (`parsers/grok.rs::update_tool_output`): prefer the
/// already-serialized `content`, and only fall back — when `content` is empty —
/// to the object's string `output_for_prompt` (Bash/terminal), a background-task
/// `TaskOutput` envelope (see `parsers::grok::grok_task_output_envelope`, the
/// one exception to "never emit the object blob": the frontend parses it into a
/// background-task card), or, for an MCP `rawOutput`, the text under `output`
/// (see grok_mcp_output_text). Returning `None` lets the frontend render
/// `content`. Non-object / absent / unrecognized `rawOutput` → `None`.
///
/// Note: `content` here is `serialize_tool_call_content`, which for a Grok
/// terminal call is the plain text block (verified against real `~/.grok`
/// data). It could in principle also serialize `Diff`/`Terminal` blocks, in
/// which case a Grok tool carrying ONLY such a block plus `output_for_prompt`
/// would render the serialized block instead of the prompt text — but Grok's
/// `run_terminal_command` emits `content:text`, so this stays parity with
/// history for the shapes Grok actually produces.
fn grok_live_tool_output(
    content: &Option<String>,
    raw_output: &Option<serde_json::Value>,
) -> Option<String> {
    if content.as_deref().is_some_and(|c| !c.trim().is_empty()) {
        return None;
    }
    let raw = raw_output.as_ref()?;
    // Bash / terminal calls: the readable text lives only in `output_for_prompt`.
    if let Some(text) = raw
        .get("output_for_prompt")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Some(text.to_string());
    }
    // Background-task polls (`get_command_or_subagent_output`): the command,
    // exit code and shell text all live under the `TaskOutput` envelope, which
    // matches none of the paths around it — without this the card streams empty.
    // Shared with the history parser so both hand the frontend the same string.
    if let Some(envelope) = crate::parsers::grok::grok_task_output_envelope(raw) {
        return Some(envelope);
    }
    // MCP calls (Grok's `use_tool` envelope): the result text lives under
    // `output.<*Output>` instead (see grok_mcp_output_text). Without this a
    // finished MCP call — e.g. the `delegate_to_agent` ack carrying
    // `task_id=…` — would surface no output at all.
    grok_mcp_output_text(raw)
}

/// Grok wraps every MCP tool invocation in a generic `use_tool` envelope whose
/// `raw_input` is `{"tool_name": "<server>__<tool>", "tool_input": {..real args..}}`.
/// Peel it so the call is correlated (delegation `lifecycle.rs`), classified, and
/// parsed as a direct MCP call — identical to how hosts like Claude Code surface
/// MCP tools. Without this, Grok's `delegate_to_agent` (and the other codeg-mcp
/// companion tools) never resolve to their dedicated cards, and the delegation
/// broker can't correlate the parent tool call to bind the sub-session.
///
/// Returns `(inner_tool_name, inner_tool_input)` only for the envelope shape —
/// a non-empty string `tool_name` plus a `tool_input` value — so Grok's native
/// tools (`run_terminal_command`, `search_tool`, `spawn_subagent`, …), which
/// carry their args directly, pass through untouched.
fn unwrap_grok_use_tool(
    raw_input: Option<&serde_json::Value>,
) -> Option<(String, serde_json::Value)> {
    let obj = raw_input?.as_object()?;
    let tool_name = obj
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())?;
    let tool_input = obj.get("tool_input")?;
    Some((tool_name.to_string(), tool_input.clone()))
}

/// Extract the human-readable text from a Grok MCP `rawOutput`
/// (`{"type":"MCP","output":{"OkayOutput":"…"}}`, or an `*Output` error variant).
/// The MCP result is the first string value under `output` (`output` may itself
/// be a bare string on some tools). Returns `None` for a non-MCP `rawOutput` so
/// the caller can fall through to the Bash/`output_for_prompt` path.
fn grok_mcp_output_text(raw_output: &serde_json::Value) -> Option<String> {
    if raw_output.get("type").and_then(serde_json::Value::as_str) != Some("MCP") {
        return None;
    }
    let output = raw_output.get("output")?;
    if let Some(text) = output.as_str() {
        return (!text.is_empty()).then(|| text.to_string());
    }
    // First NON-EMPTY string value (the singleton `*Output` variant). Filtering
    // inside `find_map` — not after — so an earlier empty-string sibling can't
    // shadow a later populated one.
    output
        .as_object()?
        .values()
        .find_map(|v| v.as_str().filter(|s| !s.is_empty()))
        .map(str::to_string)
}

/// Recover a codeg-mcp companion tool's identity from its RESULT text, for
/// Cursor sessions only.
///
/// Cursor's ACP layer announces every MCP call from the first streaming
/// partial — before `McpArgs` exists — so the announcement is the literal
/// title "MCP: tool" with an empty `raw_input`, and `sendToolCallUpdate`
/// (bundle-verified) never forwards `title`/`raw_input` again. The ONLY
/// wire signal that ever identifies the call is the MCP result text arriving
/// on the completion update, and for the codeg-mcp companion tools that text
/// is a codeg-owned contract:
///
/// * a `delegate_to_agent` ack opens with
///   `"Delegation successful. task_id="` (`broker.rs::running_ack`);
/// * `get_delegation_status` renders the compact `{"tasks":[..]}` JSON
///   (`companion.rs::render_batch_report`), whose items carry `task_id` +
///   a `status` from the fixed report vocabulary.
///
/// (`cancel_delegation` results are free-form report messages with no stable
/// prefix, so a canceled call keeps the generic title — a rare op, accepted.)
///
/// Matching those shapes lets the completion update rewrite the title to the
/// canonical `<server>__<tool>` form (the exact name the history parser
/// derives from `McpArgs`), so the frontend resolves the dedicated delegation
/// cards instead of a generic "MCP: tool" call. Returns `None` for everything
/// else — an unrecognized result keeps the wire title untouched. Callers gate
/// the sniff to calls ANNOUNCED with the identity-less "MCP: tool" title
/// (`CodeBuddyLiveState::cursor_generic_mcp_ids`), so a native tool whose
/// output merely echoes these shapes is never re-titled.
fn cursor_companion_title_from_content(content: Option<&str>) -> Option<&'static str> {
    let text = content?.trim_start();
    if text.starts_with("Delegation successful. task_id=") {
        return Some(crate::acp::delegation::DELEGATE_TOOL_REWRITE_TITLE);
    }
    // Cheap guards before the full JSON parse: the status report is a JSON
    // object whose first key is `tasks`.
    if !text.starts_with('{') || !text.contains("\"tasks\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let tasks = v.get("tasks")?.as_array()?;
    let is_report_item = |t: &serde_json::Value| {
        t.get("task_id").and_then(|x| x.as_str()).is_some()
            && t.get("status").and_then(|x| x.as_str()).is_some_and(|s| {
                matches!(s, "running" | "completed" | "failed" | "canceled" | "unknown")
            })
    };
    if !tasks.is_empty() && tasks.iter().all(is_report_item) {
        return Some(crate::acp::delegation::STATUS_TOOL_REWRITE_TITLE);
    }
    None
}

/// Mirrors `parsers/opencode.rs:425-429` (and `parsers/codebuddy.rs`'s
/// `subagent_type → "Agent"` rewrite) so streaming and reload-from-DB render the
/// same Agent card. The SQLite-side condition is
/// `tool == "task" && state.input.subagent_type IS NOT NULL`, where `tool` is the
/// agent's **internal** tool name. ACP only exposes a user-facing `title` (e.g.
/// "Explore project structure") rather than the internal tool name, so we cannot
/// replicate the `tool == "task"` half of the AND here. We instead anchor on a
/// known sub-agent-capable `agent_type` (OpenCode and CodeBuddy — both surface a
/// description-style title and the standard `{…, subagent_type}` input, and never
/// emit a bare top-level `subagent_type` for anything but a sub-agent) plus the
/// non-empty `subagent_type` string in `raw_input` — together these uniquely
/// identify a sub-agent invocation in practice. Other agents stay excluded to
/// avoid any cross-agent collision a generic `subagent_type` field could cause.
fn is_subagent_invocation(agent_type: AgentType, raw_input: &Option<String>) -> bool {
    if !matches!(agent_type, AgentType::OpenCode | AgentType::CodeBuddy) {
        return false;
    }
    let Some(text) = raw_input.as_deref() else {
        return false;
    };
    // Cheap substring guard avoids parsing large `raw_input` payloads
    // (e.g. prompts with many KB of context) when the field is absent.
    if !text.contains("subagent_type") {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    value
        .get("subagent_type")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

/// CodeBuddy routes MCP tools through its `DeferExecuteTool` virtualization
/// layer, which surfaces over ACP as a tool call whose `raw_input` wraps the real
/// call as `{ "toolName": "mcp__…", "params": { … } }`. Return that inner
/// `toolName` so the caller can rewrite the live `title` to it — making the
/// frontend resolve the dedicated card (delegation / question / …), mirroring the
/// historical unwrap in `parsers/codebuddy.rs`. `raw_input` is left untouched
/// (the cards peel `params` themselves, and that keeps `inferFromInput` from
/// misclassifying `cancel_delegation`'s `{task_id}` as a generic task).
fn codebuddy_deferred_tool_name(agent_type: AgentType, raw_input: &Option<String>) -> Option<String> {
    if agent_type != AgentType::CodeBuddy {
        return None;
    }
    let text = raw_input.as_deref()?;
    // Cheap substring guard before parsing a potentially large payload.
    if !text.contains("toolName") {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    crate::parsers::codebuddy::deferred_tool_name(&value).map(|s| s.to_string())
}

/// CodeBuddy ships a deferred MCP tool's RESULT as a single re-serialized
/// `{ "type": "text", "text": <inner> }` content part (the OpenAI-Agents content
/// shape), where `<inner>` is the MCP `CallToolResult` content text — for the
/// delegation companion, the compact report / `{ "tasks": [...] }` JSON. The
/// dedicated cards (`parseStatusReport` / `parseToolOutput`) expect that bare
/// inner payload (the content-only host shape they already handle for Claude
/// Code), NOT this wrapper, so a live `get_delegation_status` / `cancel_delegation`
/// poll otherwise renders as raw JSON text. Peel the wrapper to its inner `text`,
/// mirroring the historical `deferred_result_envelope` normalization in
/// `parsers/codebuddy.rs`.
///
/// Gated on CodeBuddy + the exact wrapper shape (`type == "text"` with a string
/// `text`): a non-deferred result (Bash/Read/ToolSearch/…) is never a lone
/// `{type,text}` object, and no delegation report carries a top-level `type`, so
/// those pass through untouched. Unlike the title rewrite, this needs no
/// `raw_input`, so it also normalizes a result-only `ToolCallUpdate` that omits it.
fn unwrap_codebuddy_deferred_output(agent_type: AgentType, text: &str) -> Option<String> {
    if agent_type != AgentType::CodeBuddy {
        return None;
    }
    // Cheap substring guard before parsing a potentially large payload.
    if !text.contains("\"type\"") {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let obj = value.as_object()?;
    if obj.get("type").and_then(|t| t.as_str()) != Some("text") {
        return None;
    }
    obj.get("text").and_then(|t| t.as_str()).map(str::to_string)
}

/// True when a CodeBuddy tool call's ACP `_meta` identifies it as a native
/// sub-agent (`Agent`) invocation. CodeBuddy tags this in `_meta` from the FIRST
/// frame (`codebuddy.ai/toolName == "Agent"`) and later adds
/// `codebuddy.ai/isSubagent` / `subagentType` — whereas the `subagent_type`
/// field in `raw_input` (see `is_subagent_invocation`) only streams in dozens of
/// frames later. Reading the meta lets the title rewrite fire on frame 1, so the
/// Agent pill never spends an opening window classified as a generic tool (and
/// its child tool calls, which carry `codebuddy.ai/parentToolCallId` every frame,
/// nest from the start). Gated on CodeBuddy so the generic `codebuddy.ai/*` keys
/// can never affect another agent.
fn codebuddy_meta_marks_subagent(
    agent_type: AgentType,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    if agent_type != AgentType::CodeBuddy {
        return false;
    }
    let Some(meta) = meta else {
        return false;
    };
    if meta.get("codebuddy.ai/toolName").and_then(|v| v.as_str()) == Some("Agent") {
        return true;
    }
    if meta.get("codebuddy.ai/isSubagent").and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    meta.get("codebuddy.ai/subagentType")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
}

/// Name used when a codex sub-agent's `path` carries no usable segment. Matches
/// the fallback codex-acp itself uses when building the activity title.
const CODEX_SUBAGENT_FALLBACK_NAME: &str = "subagent";

/// How a Codex live `subAgentActivity` (codex-acp #304) should be handled.
///
/// codex 0.147's native team-of-agents runs sub-agents entirely inside the codex
/// process. Its orchestration calls (`spawn_agent` / `wait_agent`, the
/// `collaboration` namespace) never reach the ACP wire, and the inter-agent
/// message is an opaque encrypted envelope even in the on-disk rollout. The one
/// thing codex-acp forwards is `subAgentActivity`, as a `tool_call(kind:other)`
/// carrying `_meta.codex.subagent = {threadId, path, activity}`.
///
/// codeg used to DROP every one of these, on the premise that the
/// `collabAgentToolCall` capsule already showed the same thing. That premise
/// died with the team-of-agents rewrite: codex raises no `collabAgentToolCall`
/// for a spawn any more, so dropping this left a codex sub-agent completely
/// invisible while it ran — nothing appeared in the timeline until the session
/// was reopened and the rollout re-parsed.
enum CodexSubagentActivity {
    /// Not a codex sub-agent activity — handle the call normally.
    None,
    /// A sub-agent was launched. Carries the Agent-card input to render it with.
    Started(String),
    /// A later lifecycle marker (`interacted` / `interrupted`). Still dropped:
    /// they carry no content of their own and would each open a SECOND capsule
    /// with the same name and no way to tell it apart from the launch.
    Other,
}

/// Classify a live tool call's `_meta`, building the Agent-card input for a
/// launch. The three fields are the ones `parsers/codex.rs` writes on reload, so
/// live and history render the same capsule: the sub-agent's name is the last
/// segment of its `path` (`/root/pnpm_build` → `pnpm_build`) and `agent_id` is
/// its codex thread id, which the card renders as a short badge. No `prompt` —
/// the task text is encrypted on this wire.
///
/// The capsule settles as soon as codex acknowledges the launch, NOT when the
/// child finishes: the activity item's own lifecycle is the spawn's, and codex
/// forwards nothing else about the child over ACP. A child's eventual result
/// reaches the timeline as the parent's next message.
fn classify_codex_subagent_activity(
    agent_type: AgentType,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> CodexSubagentActivity {
    if agent_type != AgentType::Codex {
        return CodexSubagentActivity::None;
    }
    let Some(subagent) = meta
        .and_then(|m| m.get("codex"))
        .and_then(|codex| codex.get("subagent"))
    else {
        return CodexSubagentActivity::None;
    };
    // A status-only follow-up carries the same meta with the same `activity`,
    // so this classification is stable across the call's whole lifetime.
    if subagent.get("activity").and_then(|v| v.as_str()) != Some("started") {
        return CodexSubagentActivity::Other;
    }
    let name = subagent
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(CODEX_SUBAGENT_FALLBACK_NAME);
    let mut input = serde_json::Map::new();
    input.insert(
        "subagent_type".to_string(),
        serde_json::Value::String(name.to_string()),
    );
    if let Some(thread_id) = subagent
        .get("threadId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        input.insert(
            "agent_id".to_string(),
            serde_json::Value::String(thread_id.to_string()),
        );
    }
    // Say that this card stands for the LAUNCH, so its "completed" is not read
    // as "the sub-agent finished" (see the constant's doc).
    input.insert(
        crate::parsers::codex::CODEX_SUBAGENT_LAUNCH_KEY.to_string(),
        serde_json::Value::Bool(true),
    );
    CodexSubagentActivity::Started(serde_json::Value::Object(input).to_string())
}

/// True when a `session/request_permission` is codex's Plan-mode review gate
/// (codex-acp #351, v1.1.8+): `_meta.codex = {kind: "plan_review", planItemId}`
/// on the REQUEST (sibling of `options`), not on the tool call.
///
/// codex-acp raises this once a plan item settles while `collaboration_mode` is
/// `plan`, asking whether to implement the plan (`implement_plan`) or stay in
/// plan mode (`revise_plan`). Its `toolCall` (`plan-review:<itemId>`) is NEVER
/// announced as a `tool_call` — the only follow-up on the wire is a
/// `tool_call_update` carrying just a status and `rawOutput`. Without seeding a
/// tool call from this request that update lands on an unknown id and renders as
/// an untitled generic tool card, so `handle_permission_request` emits one.
/// Gated on Codex, mirroring [`classify_codex_subagent_activity`].
fn is_codex_plan_review(
    agent_type: AgentType,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    if agent_type != AgentType::Codex {
        return false;
    }
    meta.and_then(|m| m.get("codex"))
        .and_then(|codex| codex.get("kind"))
        .and_then(serde_json::Value::as_str)
        == Some("plan_review")
}

/// True when an `initialize` response advertises the ACP steering extension —
/// the TOP-LEVEL `_meta.steering.supported` flag, a sibling of
/// `agentCapabilities` (NOT `agentCapabilities._meta`, which belongs to other
/// conventions such as sacp's symposium capability ext). Both claude-agent-acp
/// (0.61+) and codex-acp (1.1.6+) advertise here; whether codeg actually
/// steers natively additionally requires the
/// `registry::steering_prompt_required_min_version` policy plus the runtime
/// version proof (see the synthesis in `run_connection`).
fn init_advertises_steering(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> bool {
    meta.and_then(|m| m.get("steering"))
        .and_then(|s| s.get("supported"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Whether the `initialize` response advertises the provider-neutral goal
/// extension: top-level `_meta.goal = {version: <integer >= 1>, controlMethod,
/// actions}`. Advertised ⇒ goal state arrives as
/// `session_info_update._meta.goal` snapshots and the legacy
/// `_meta.codex.goal` key is ignored for the WHOLE connection — codex-acp
/// switched to the neutral key silently in 1.2.0 (legacy key gone), and
/// claude-agent-acp speaks only the neutral form (0.66.0+). Pinning the
/// channel here, at initialize, makes the selection independent of update
/// arrival order, so a transitional adapter double-publishing one goal
/// transition through both namespaces can never produce two goal cards.
/// Non-integer or sub-1 versions fail closed onto the legacy channel.
fn init_advertises_goal(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> bool {
    meta.and_then(|m| m.get("goal"))
        .and_then(|g| g.get("version"))
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|version| version >= 1)
}

/// The goal-control surface an `initialize` response advertises:
/// `(_meta.goal.controlMethod, _meta.goal.actions)` — claude 0.66+ offers
/// `("_session/goal", ["set","clear"])`, codex 1.2+ the same method with all
/// four actions. `None` when the neutral goal extension isn't advertised
/// (see [`init_advertises_goal`]) or carries no usable method string — the
/// session then keeps the legacy codex method + actions
/// (`codex_goal::LEGACY_GOAL_CONTROL_METHOD` / `LEGACY_GOAL_ACTIONS`). An
/// advertised-but-empty actions array is honored as "no controls": the goal
/// card gates its buttons on this list, so only affordances the adapter
/// actually implements are offered.
fn goal_advertised_control(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<(String, Vec<String>)> {
    if !init_advertises_goal(meta) {
        return None;
    }
    let goal = meta?.get("goal")?;
    let method = goal
        .get("controlMethod")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())?
        .to_string();
    let actions = goal
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some((method, actions))
}

/// Resolve [`goal_advertised_control`]'s answer into what the session state
/// stores: an OPTIONAL method override (absent ⇒ keep the legacy codex method
/// the state was built with) and the CONCRETE action vocabulary.
///
/// The legacy pair belongs here, at initialize, and not in `SessionState::new`.
/// A client can read the snapshot during the handshake — `spawn_agent` returns
/// with `initialize` still in flight — and it latches whatever it finds, so a
/// construction-time legacy default hands a claude session a Pause its adapter
/// answers with `Invalid params: goal action must be "set" or "clear"`. Keeping
/// the field `None` until this runs is what makes "unknown" tellable from
/// "legacy" on the wire.
fn resolve_goal_control(
    advertised: Option<(String, Vec<String>)>,
) -> (Option<String>, Vec<String>) {
    match advertised {
        Some((method, actions)) => (Some(method), actions),
        None => (
            None,
            crate::acp::codex_goal::LEGACY_GOAL_ACTIONS
                .iter()
                .map(|a| (*a).to_string())
                .collect(),
        ),
    }
}

/// Pick the goal payload out of a `session_info_update`'s `_meta` according to
/// the channel pinned at initialize (see [`init_advertises_goal`]): the
/// neutral `_meta.goal` for advertising connections, the legacy
/// `_meta.codex.goal` otherwise — never both. Pure so the either/or contract
/// is unit-tested without the connection machinery.
fn session_info_goal_value(
    neutral_goal_channel: bool,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<&serde_json::Value> {
    let meta = meta?;
    if neutral_goal_channel {
        meta.get("goal")
    } else {
        meta.get("codex").and_then(|codex| codex.get("goal"))
    }
}

/// The raw `sessionFailure` value out of a `_meta.jetbrains.air` envelope —
/// present only when the envelope itself is well-formed (integer
/// `version >= 1`, mirroring the advertisement check the adapters run on
/// codeg's `clientCapabilities._meta.jetbrains.air`). A malformed or
/// future-incompatible envelope yields `None` and the carrier is treated as
/// holding no failure. Records ride TWO carriers with this same envelope: the
/// per-attempt upserts on `session_info_update._meta`, and a turn's terminal
/// failure on the prompt RESPONSE `_meta` (see [`response_session_failure`]).
fn air_session_failure(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<&serde_json::Value> {
    let air = meta?.get("jetbrains")?.get("air")?;
    let version = air.get("version").and_then(serde_json::Value::as_i64)?;
    if version < 1 {
        return None;
    }
    air.get("sessionFailure")
}

/// Validate one AIR failure upsert into a [`SessionFailureRecord`].
///
/// `id` (non-blank string) and `revision` (integer >= 1) are HARD
/// requirements — without identity there is nothing to merge
/// deterministically, so a record missing either is dropped (the caller logs
/// it at debug). Everything else is lenient: `category`/`severity` default to
/// `"unknown"`/`"error"` and pass through unrecognized values as plain
/// strings (the frontend falls back per field), `title` may be blank,
/// non-string `actions` entries are skipped. `resolved` starts `false`; the
/// stores flip it (see the type docs).
fn parse_session_failure_record(value: &serde_json::Value) -> Option<SessionFailureRecord> {
    let id = value.get("id")?.as_str()?.trim();
    if id.is_empty() {
        return None;
    }
    let revision = value.get("revision")?.as_u64()?;
    if revision < 1 {
        return None;
    }
    let text = |key: &str, default: &str| -> String {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(default)
            .to_string()
    };
    Some(SessionFailureRecord {
        id: id.to_string(),
        revision,
        category: text("category", "unknown"),
        severity: text("severity", "error"),
        title: text("title", ""),
        details: value
            .get("details")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        actions: value
            .get("actions")
            .and_then(serde_json::Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        resolved: false,
    })
}

/// The terminal AIR failure riding on a prompt RESPONSE's `_meta`, if any.
///
/// With `jetbrains.air` negotiated, BOTH adapters deliver a turn's terminal
/// failure ON the prompt response rather than as another
/// `session_info_update`: claude-agent-acp's `failActiveWithSessionFailure`
/// settles the turn with a disguised `end_turn` stop reason and attaches the
/// record here (its own comment calls the response "the canonical AIR
/// carrier" — the update channel only ever carries the per-attempt retry
/// warnings), and codex-acp's `terminalFailurePromptResponse` mirrors the
/// same shape as the catch-up for a record whose update was missed (the
/// strict revision merge de-duplicates when both arrive). Field report
/// 2026-08-15: a mid-turn network drop on claude 0.68.0 published
/// "Reconnecting to Claude, attempt N of 5" warnings via updates, then the
/// `transport_lost` error escalation ONLY here — ignoring this carrier lost
/// the terminal record entirely, so the turn-boundary settle painted the
/// still-dead connection as a recovered warning.
fn response_session_failure(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<SessionFailureRecord> {
    let raw = air_session_failure(meta)?;
    let record = parse_session_failure_record(raw);
    if record.is_none() {
        tracing::debug!(
            "[ACP] dropped prompt-response AIR sessionFailure without usable id/revision: {raw:?}"
        );
    }
    record
}

/// Strict SemVer floor check: true when `version >= min` by SemVer
/// PRECEDENCE. Prerelease ordering matters here — `0.64.0-rc1` precedes
/// `0.64.0` and may predate the very commit that shipped the
/// `promptRequired` guarantee, so it must NOT satisfy a `0.64.0` floor
/// (`0.64.1-beta.2` still does: its numeric core is above the floor). Build
/// metadata (`+sha`) is precedence-ignored per spec. Fail closed: anything
/// `semver` can't parse (missing segments, `v` prefixes, garbage suffixes)
/// routes live feedback to the MCP pull path — today's behavior.
fn version_at_least(version: &str, min: &str) -> bool {
    let (Ok(actual), Ok(floor)) = (
        semver::Version::parse(version.trim()),
        semver::Version::parse(min),
    ) else {
        return false;
    };
    actual.cmp_precedence(&floor) != std::cmp::Ordering::Less
}

/// Runtime half of the native-steering gate: does the adapter binary that is
/// ACTUALLY running — which launch may have resolved from PATH rather than
/// the pinned npx package (see `commands::acp::acp_get_agent_status_core`) —
/// report an `agent_info.version` at or above the registry minimum? Fail
/// closed on a missing `agent_info` or an unparseable version.
fn steering_version_ok(agent_info: Option<&sacp::schema::Implementation>, min: &str) -> bool {
    agent_info.is_some_and(|info| version_at_least(&info.version, min))
}

/// Synthesize `SessionState.native_steering_available` from an `initialize`
/// response: extension advertised (top-level `_meta`) AND registry policy says
/// this agent type honors `promptRequired` AND the running binary's
/// `agent_info.version` proves it. Pure so the full gate matrix is unit-tested;
/// `run_connection` calls it once and everything downstream reads the stored
/// bool.
fn synthesize_native_steering(
    agent_type: AgentType,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
    agent_info: Option<&sacp::schema::Implementation>,
) -> bool {
    init_advertises_steering(meta)
        && registry::steering_prompt_required_min_version(agent_type)
            .is_some_and(|min| steering_version_ok(agent_info, min))
}

/// Extract a retryable-turn-error indicator from a Codex `session_info_update`'s
/// `_meta` (codex-acp #289, v1.1.3+). codex ships a transient, auto-retried
/// error as `_meta.codex.error = {message, codexErrorInfo, additionalDetails,
/// turnId, willRetry}` and keeps the prompt alive; it emits this only when
/// `willRetry == true`. Returns `(message, http_status)` when a non-empty
/// message is present. `codexErrorInfo` may be a bare string enum, an object
/// variant carrying an inner `httpStatusCode`, or absent — only the object form
/// yields a status. Defensively refuses a `willRetry == false` payload so a
/// terminal error can never render as "retrying".
fn codex_retry_indicator(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<(String, Option<i64>)> {
    let err = meta?.get("codex")?.get("error")?;
    if err.get("willRetry").and_then(|v| v.as_bool()) == Some(false) {
        return None;
    }
    let message = err.get("message").and_then(|v| v.as_str())?.trim();
    if message.is_empty() {
        return None;
    }
    let http_status = err
        .get("codexErrorInfo")
        .and_then(|info| info.as_object())
        .and_then(|obj| obj.values().next())
        .and_then(|inner| inner.get("httpStatusCode"))
        .and_then(|v| v.as_i64());
    Some((message.to_string(), http_status))
}

/// True when an available command is really a config-option state toggle rather
/// than an invokable slash command (codex-acp #293, v1.1.4). codex advertises
/// e.g. `/plan` as an `AvailableCommand` tagged
/// `_meta.commandAction = {kind:"setConfigOption", configId:"collaboration_mode",
/// value:"plan", resetValue:"default", presentation:"state"}` — codex's signal
/// that the client should represent it as STATE. codeg already surfaces that
/// state as the `collaboration_mode` config-option selector (the generic
/// `SessionConfigOption` path), so also listing `/plan` as a slash command is
/// redundant and its static "Turn plan mode on" description is wrong once plan
/// mode is already on. Suppress these from the command list. Commands with any
/// other action kind (e.g. `/goal`'s `prefixPrompt`, which takes an objective
/// argument) are real commands and kept. Gated on Codex — `commandAction` is a
/// codex-private `_meta` extension (the ACP schema has no such type).
fn is_config_option_state_command(
    agent_type: AgentType,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    if agent_type != AgentType::Codex {
        return false;
    }
    meta.and_then(|m| m.get("commandAction"))
        .and_then(|action| action.get("kind"))
        .and_then(|kind| kind.as_str())
        == Some("setConfigOption")
}

/// True when a CodeBuddy sub-agent tool call's `_meta` marks it as a BACKGROUND
/// sub-agent (`codebuddy.ai/isBackground == true`). A background sub-agent runs
/// concurrently with the main agent, so the suppression-window invariant (parent
/// blocked → only sub-agent chunks in the window) does NOT hold for it — see
/// `track_subagent_window`, which excludes it from the window. Gated on CodeBuddy.
fn codebuddy_meta_marks_background(
    agent_type: AgentType,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    if agent_type != AgentType::CodeBuddy {
        return false;
    }
    meta.and_then(|m| m.get("codebuddy.ai/isBackground"))
        .and_then(|v| v.as_bool())
        == Some(true)
}

/// True when a CodeBuddy thought/message `ContentChunk`'s own `_meta` marks the
/// chunk as sub-agent output (`codebuddy.ai/isSubagent`, or a
/// `codebuddy.ai/parentToolCallId` link to the Agent call). This is a precision
/// supplement to the open-sub-agent window — CodeBuddy is not confirmed to
/// populate chunk `_meta`, so suppression never relies on it alone. Gated on
/// CodeBuddy.
fn codebuddy_chunk_marks_subagent(
    agent_type: AgentType,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    if agent_type != AgentType::CodeBuddy {
        return false;
    }
    let Some(meta) = meta else {
        return false;
    };
    if meta.get("codebuddy.ai/isSubagent").and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    meta.get("codebuddy.ai/parentToolCallId")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
}

/// Whether a live thought/message chunk should be dropped from the top-level
/// stream because it belongs to a CodeBuddy sub-agent (whose work is already
/// represented by the Agent pill + its nested tool calls).
///
/// Claude Code is NOT handled here: its sub-agent chunks (claude-agent-acp
/// ≥0.63 with the `subagent-transcript` capability) arrive with a precise
/// per-chunk `_meta.claudeCode.parentToolUseId` and are forwarded WITH that
/// attribution instead of suppressed — see `claude_chunk_parent_tool_use_id`.
///
/// Suppress while we're inside an open sub-agent window OR when the chunk's own
/// meta marks it. The window safety rests on a structural invariant: the window
/// only ever holds FOREGROUND (blocking) sub-agents — a synchronous `Agent` tool
/// call suspends the parent model until the tool returns its result, so between
/// that call's open frame and its terminal frame the main session carries ONLY
/// the sub-agent's chunks, never main-agent output. BACKGROUND sub-agents (which
/// run concurrently and could interleave main-agent output) are deliberately
/// excluded from the window by `track_subagent_window`, so `window_open` can
/// never cause a main-agent chunk to be dropped. Gated on CodeBuddy; every other
/// agent always emits.
fn should_suppress_subagent_chunk(
    agent_type: AgentType,
    window_open: bool,
    chunk_meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    if agent_type != AgentType::CodeBuddy {
        return false;
    }
    window_open || codebuddy_chunk_marks_subagent(agent_type, chunk_meta)
}

/// Extract the update-level `_meta.claudeCode.parentToolUseId` of a live
/// text/thought chunk — set by claude-agent-acp ≥0.63 on a subagent's
/// forwarded chunks once the client advertises the `subagent-transcript`
/// capability (see `build_client_capabilities`). The chunk is emitted WITH
/// this attribution (never suppressed): the frontend routes parented chunks
/// into the live Agent capsule instead of the main thread. Gated on
/// ClaudeCode so no other agent's namespaced meta can alias into parented
/// routing; empty strings are treated as absent.
fn claude_chunk_parent_tool_use_id(
    agent_type: AgentType,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<String> {
    if agent_type != AgentType::ClaudeCode {
        return None;
    }
    meta?
        .get("claudeCode")?
        .get("parentToolUseId")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Maintain the set of OPEN CodeBuddy sub-agent tool calls (`open`). `is_agent`
/// is true once `resolve_rewritten_title` classified this `tool_call_id` as a
/// native sub-agent (`"agent"`). A non-final status opens the window; a final
/// status (`completed` / `failed`) closes it and records the id in `closed`, so a
/// stray late non-final frame can't re-open an already-finished sub-agent.
///
/// `is_background` (from `codebuddy_meta_marks_background`) EXCLUDES a sub-agent
/// from the window: a background sub-agent runs concurrently with the main agent,
/// so the "window holds only sub-agent chunks" invariant that makes
/// `should_suppress_subagent_chunk` safe would not hold. We treat a background
/// marker exactly like a terminal frame (remove + record closed) so it can never
/// suppress interleaved main-agent output. (`isBackground` can stream in a frame
/// or two after the call opens, so a background sub-agent's earliest chunks may be
/// briefly suppressed before the marker arrives — an accepted, rare imperfection;
/// the user-reported case is foreground, where the marker is `false`.)
///
/// Gated on CodeBuddy so a single-agent-type connection of any other agent stays
/// inert.
fn track_subagent_window(
    agent_type: AgentType,
    is_agent: bool,
    is_background: bool,
    status: Option<&str>,
    tool_call_id: &str,
    open: &mut HashSet<String>,
    closed: &mut HashSet<String>,
) {
    if agent_type != AgentType::CodeBuddy || !is_agent {
        return;
    }
    let is_final = matches!(status, Some("completed") | Some("failed"));
    if is_final || is_background {
        open.remove(tool_call_id);
        closed.insert(tool_call_id.to_string());
    } else if !closed.contains(tool_call_id) {
        open.insert(tool_call_id.to_string());
    }
}

/// Per-session CodeBuddy live-stream state threaded through
/// `emit_conversation_update`. Consolidates the authoritative title rewrites and
/// the open-sub-agent suppression window so CodeBuddy's sparse, multi-frame
/// sub-agent stream resolves to a stable Agent pill (whose children nest) with
/// its interleaved thought/message chunks suppressed. Created per connection,
/// shared across the idle and active-turn loops; the historical-replay path uses
/// a throwaway instance. Mirrors `ToolCallOutputCache`'s lifetime.
#[derive(Default)]
struct CodeBuddyLiveState {
    /// tool_call_id → authoritative title: `"agent"` for a native sub-agent, or
    /// the inner `mcp__…` name for a `DeferExecuteTool` MCP call. Re-asserted on
    /// every later frame so a status-only update can't downgrade the card.
    title_overrides: HashMap<String, String>,
    /// Sub-agent tool calls currently OPEN (classified, not yet completed/failed).
    /// While non-empty, interleaved thought/message chunks belong to a sub-agent
    /// and are suppressed from the top-level stream (matching Claude Code).
    open_subagents: HashSet<String>,
    /// Sub-agent tool calls that already reached a final status — guards against a
    /// stray late non-final frame re-opening a finished sub-agent.
    closed_subagents: HashSet<String>,
    /// Objective of the Codex `/goal` run currently open on this connection (set
    /// by the latest `active` `session_info_update` goal, cleared on any terminal
    /// status). Lets a later `goal:null` close the run by objective — and be a
    /// no-op when no run is open. See `crate::acp::codex_goal::next_goal_marker`.
    ///
    /// This lives here (not in `SessionState`) because `CodeBuddyLiveState` and
    /// `SessionState` share one lifetime: a browser refresh / reconnect re-attaches
    /// to the *running* connection (`find_connection_for_reuse`), keeping both; a
    /// brand-new connection resets both together (empty live blocks + fresh state).
    /// So this state never resets while goal blocks it would close still exist.
    codex_open_goal: Option<String>,
    /// Monotonic per-connection counter for synthetic goal tool-call ids. Occurrence
    /// (not content) addressing keeps two runs that share an objective from
    /// colliding in the reducer's id-keyed live block list.
    codex_goal_seq: u64,
    /// Cursor tool calls announced with the identity-less "MCP: tool" title.
    /// Only these are eligible for the completion-time result sniff
    /// (`cursor_companion_title_from_content`) — a `shell`/`read` call whose
    /// OUTPUT merely echoes a delegation ack must never be re-titled. Entries
    /// are dropped once the call reaches a terminal status (the sniff, if any,
    /// has recorded its override by then), so the set tracks only in-flight
    /// calls.
    cursor_generic_mcp_ids: HashSet<String>,
    /// Grok tool_call ids whose interactive question already renders via the
    /// `_x.ai/ask_user_question` ext bridge (`handle_grok_ask_user_question`). The
    /// redundant native `tool_call` / `tool_call_update` stream for these is
    /// dropped so the card doesn't double-render; tracked by id because a later
    /// status-only update may drop the `x.ai/tool` meta that first identified it.
    grok_ask_tool_ids: HashSet<String>,
    /// Grok `spawn_subagent` tool_call ids ever announced on this connection
    /// (dedupe for the pending queue + status tracking on meta-less updates).
    grok_spawn_seen: HashSet<String>,
    /// Announced spawn calls not yet paired with a `subagent_spawned`
    /// notification. Grok's ext notifications carry a `subagent_id` but no
    /// `tool_call_id`, so pairing is by matching `(description, subagent_type)`
    /// captured from the launch `rawInput` against the notification's own
    /// fields, first match in stream order — FIFO in the common in-order case
    /// (mirroring the history parser's pairing), while a DELAYED prior-turn
    /// `subagent_spawned` fails the match against a new turn's differently-
    /// described entry instead of stealing its slot. A spawn that FAILS before
    /// pairing is removed so later pairs can't shift; the queue is cleared at
    /// every turn start (staleness bound).
    grok_pending_spawn_ids: VecDeque<GrokPendingSpawn>,
    /// subagent_id → its launching spawn tool_call id, from `subagent_spawned`.
    /// Routes `subagent_progress` ticks (live meta on the Agent card) and the
    /// `subagent_finished` settle; the entry is dropped at finish.
    grok_subagent_to_call: HashMap<String, String>,
    /// spawn tool_call id → the child's OWN session id (`child_session_id`, or
    /// the subagent id it always equals in practice). Grok runs each sub-agent
    /// as a standalone session that writes its transcript to disk while it
    /// works, so this is what lets the live Agent card offer "open the child's
    /// session" — the only way to watch a blocking child, whose launch call
    /// carries no output until it finishes. Kept in state (not just emitted
    /// once) because `upsert_tool_call` REPLACES a block's meta: every later
    /// progress tick must re-send it or the id would be dropped. Entry dropped
    /// at finish, alongside `grok_subagent_to_call`.
    grok_call_child_session: HashMap<String, String>,
    /// Spawn calls that reached a terminal wire status. A `subagent_finished`
    /// for one of these is a BACKGROUND child settling after its launch ack —
    /// the case whose result would otherwise never reach the card live; a
    /// blocking spawn's own completion frame carries the output instead.
    grok_settled_spawn_ids: HashSet<String>,
    /// Spawn calls announced in the CURRENT turn — the only ones whose
    /// `subagent_progress` ticks may emit a live `ToolCallUpdate`. Cleared at
    /// every turn start (with the pending queue): a tick for a PRIOR turn's
    /// background child would otherwise pass the Prompting gate during a later
    /// turn and get appended into that turn's live message as a ghost card
    /// (its real card lives in the promoted history). The settle path
    /// (`subagent_finished` → BackgroundActivity) is unaffected — it targets
    /// promoted turns by design.
    grok_progress_eligible: HashSet<String>,
    /// Context window of the model this Grok turn runs on, resolved ONCE at turn
    /// start (`grok_current_model_context_window`) so the per-update usage peek
    /// never takes the state lock on the streaming hot path. `None` for non-Grok
    /// agents, and for a Grok model whose spec carried no `totalContextTokens`
    /// — the ring then keeps falling back to the parsed per-turn stats.
    grok_turn_context_window: Option<u64>,
    /// Last `(used, size)` emitted as a live `UsageUpdate` on this connection.
    /// Grok repeats its cumulative `totalTokens` on nearly every update (169 of
    /// 189 in a real capture), so re-emitting each one would put a broadcast on
    /// a per-chunk hot path for a number that changes a handful of times a turn.
    /// The WINDOW is part of the key so a between-turn model switch re-emits
    /// even when the token count hasn't moved yet — otherwise the ring would
    /// keep dividing by the previous model's window.
    grok_last_usage: Option<(u64, u64)>,
}

/// One announced-but-unpaired Grok `spawn_subagent` call. `description` /
/// `subagent_type` come from the launch `rawInput` and validate the pairing
/// against the `subagent_spawned` notification's own fields (`None` = wildcard,
/// tolerant of older wire shapes).
#[derive(Debug)]
struct GrokPendingSpawn {
    call_id: String,
    description: Option<String>,
    subagent_type: Option<String>,
}

/// True when a Grok tool call's ACP `_meta` identifies it as the native
/// `spawn_subagent` launcher (`_meta["x.ai/tool"].name`). Present on the very
/// first frame (verified against real captures), unlike `subagent_type` which
/// rides `rawInput`. Gated on Grok so the namespaced key can't affect others.
fn grok_meta_marks_spawn_subagent(
    agent_type: AgentType,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    matches!(agent_type, AgentType::Grok)
        && meta
            .and_then(|m| m.get("x.ai/tool"))
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            == Some("spawn_subagent")
}

/// Track a Grok `spawn_subagent` call's lifecycle for the subagent-notification
/// pairing (see the `grok_*` fields on [`CodeBuddyLiveState`]). `is_spawn` is
/// the precomputed meta marker; a status-only update that lost the meta still
/// tracks via the seen-set. `raw_input` (the launch input, when this frame
/// carries it) supplies the `(description, subagent_type)` the pairing
/// validates against. No-op for other agents/tools by construction
/// (`is_spawn` false + id never seen).
fn track_grok_spawn_call(
    cb_state: &mut CodeBuddyLiveState,
    is_spawn: bool,
    status: Option<&str>,
    tool_call_id: &str,
    raw_input: &Option<String>,
) {
    if is_spawn && cb_state.grok_spawn_seen.insert(tool_call_id.to_string()) {
        let (description, subagent_type) = raw_input
            .as_deref()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
            .map(|input| {
                let field = |key: &str| {
                    input
                        .get(key)
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                };
                (field("description"), field("subagent_type"))
            })
            .unwrap_or((None, None));
        cb_state.grok_pending_spawn_ids.push_back(GrokPendingSpawn {
            call_id: tool_call_id.to_string(),
            description,
            subagent_type,
        });
        // Announced in the current turn → its progress ticks may render live.
        cb_state
            .grok_progress_eligible
            .insert(tool_call_id.to_string());
    }
    if !cb_state.grok_spawn_seen.contains(tool_call_id) {
        return;
    }
    match status {
        Some("completed") => {
            cb_state
                .grok_settled_spawn_ids
                .insert(tool_call_id.to_string());
        }
        Some("failed") => {
            cb_state
                .grok_settled_spawn_ids
                .insert(tool_call_id.to_string());
            // Never spawned a child (depth-limit / config error): drop it from
            // the pairing queue so the next spawn doesn't inherit its slot.
            cb_state
                .grok_pending_spawn_ids
                .retain(|pending| pending.call_id != tool_call_id);
        }
        _ => {}
    }
}

/// True when a tool call's ACP `_meta` marks it as grok's native
/// `ask_user_question` (`x.ai/tool.kind == "ask_user"`). Codeg answers grok's
/// blocking `_x.ai/ask_user_question` ext request by rendering the interactive
/// `AskQuestionCard` (see `handle_grok_ask_user_question`), so the parallel
/// `tool_call` stream grok emits for the same call is redundant — it is dropped
/// live so the question doesn't render twice (once answerable, once inert).
fn grok_meta_marks_ask_user(
    agent_type: AgentType,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    matches!(agent_type, AgentType::Grok)
        && meta
            .and_then(|m| m.get("x.ai/tool"))
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_str())
            == Some("ask_user")
}

/// Resolve a tool call's title, honoring an authoritative rewrite recorded for
/// the session in `overrides` (tool_call_id → resolved title).
///
/// Returns `Some(name)` when this event identifies a CodeBuddy `DeferExecuteTool`
/// (the inner `mcp__…` name, from `raw_input`) or a sub-agent invocation
/// (`"agent"`) — recording it — OR when a PRIOR event already classified this
/// `tool_call_id` and this event lost the marker (the override is re-asserted).
/// Returns `None` only when the call was never classified, so the caller falls
/// back to the event's own title.
///
/// Sub-agent detection fires on EITHER `raw_input.subagent_type`
/// (`is_subagent_invocation`) OR `meta_marks_subagent` — the precomputed
/// `codebuddy_meta_marks_subagent` result. The meta signal is what makes the pill
/// stable: CodeBuddy carries `codebuddy.ai/toolName == "Agent"` from the very
/// first frame, whereas `subagent_type` only reaches `raw_input` dozens of frames
/// later, so meta-first detection records the override immediately and every
/// later (sparse) frame re-asserts it.
///
/// The re-assertion is the fix for CodeBuddy's status-only `ToolCallUpdate`s:
/// they arrive without the original `subagent_type`/`toolName` payload but WITH
/// the agent's raw (non-agent) title. Without it the frontend
/// (`inferLiveToolName` → `getToolName`) downgrades the Agent / delegation card
/// back to a generic tool call mid-stream — which also un-nests its children.
/// `on_update` only tunes the (PII-safe, id-only) trace wording.
fn resolve_rewritten_title(
    agent_type: AgentType,
    raw_input: &Option<String>,
    tool_call_id: &str,
    on_update: bool,
    meta_marks_subagent: bool,
    overrides: &mut HashMap<String, String>,
) -> Option<String> {
    if let Some(inner) = codebuddy_deferred_tool_name(agent_type, raw_input) {
        tracing::info!(
            "[ACP][{agent_type}] unwrapped DeferExecuteTool to its real MCP tool (tool_call_id={tool_call_id}, on_update={on_update})"
        );
        overrides.insert(tool_call_id.to_string(), inner.clone());
        return Some(inner);
    }
    if is_subagent_invocation(agent_type, raw_input) || meta_marks_subagent {
        tracing::info!(
            "[ACP][{agent_type}] subagent detected, rewrote tool title to 'agent' (tool_call_id={tool_call_id}, on_update={on_update})"
        );
        overrides.insert(tool_call_id.to_string(), "agent".to_string());
        return Some("agent".to_string());
    }
    overrides.get(tool_call_id).cloned()
}

fn map_plan_priority(priority: &PlanEntryPriority) -> String {
    match priority {
        PlanEntryPriority::High => "high",
        PlanEntryPriority::Medium => "medium",
        PlanEntryPriority::Low => "low",
        _ => "unknown",
    }
    .to_string()
}

fn map_plan_status(status: &PlanEntryStatus) -> String {
    match status {
        PlanEntryStatus::Pending => "pending",
        PlanEntryStatus::InProgress => "in_progress",
        PlanEntryStatus::Completed => "completed",
        _ => "unknown",
    }
    .to_string()
}

fn map_plan_entries(plan: &Plan) -> Vec<PlanEntryInfo> {
    plan.entries
        .iter()
        .map(|entry| PlanEntryInfo {
            content: entry.content.clone(),
            priority: map_plan_priority(&entry.priority),
            status: map_plan_status(&entry.status),
        })
        .collect()
}

fn parse_claude_sdk_message_params(
    params: &serde_json::Value,
) -> Option<(String, serde_json::Value)> {
    let obj = params.as_object()?;
    let session_id = obj.get("sessionId")?.as_str()?.to_string();
    let message = obj.get("message")?.clone();
    Some((session_id, message))
}

fn is_claude_api_retry_message(message: &serde_json::Value) -> bool {
    let obj = match message.as_object() {
        Some(obj) => obj,
        None => return false,
    };
    let message_type = obj.get("type").and_then(|v| v.as_str());
    let message_subtype = obj.get("subtype").and_then(|v| v.as_str());
    matches!(message_type, Some("system")) && matches!(message_subtype, Some("api_retry"))
}

/// The JSON-RPC method claude-agent-acp uses to mirror raw SDK messages. Named
/// so `is_known_ext_method` and the mapper can't drift apart.
const CLAUDE_SDK_EXT_METHOD: &str = "_claude/sdkMessage";

fn map_claude_sdk_ext_notification(notification: &UntypedMessage) -> Option<AcpEvent> {
    if notification.method() != CLAUDE_SDK_EXT_METHOD {
        return None;
    }

    let (session_id, message) = parse_claude_sdk_message_params(notification.params())?;
    if !is_claude_api_retry_message(&message) {
        return None;
    }
    Some(AcpEvent::ClaudeSdkMessage {
        session_id,
        message,
    })
}

/// The JSON-RPC methods grok uses for its private, namespaced session updates.
/// Both share the standard `session/update` envelope (`params.update.
/// sessionUpdate` + fields, verified live against grok 0.2.111) but carry
/// variants the typed ACP pipeline can't deserialize, so codeg drops them.
const GROK_EXT_UPDATE_METHODS: [&str; 2] =
    ["_x.ai/session_notification", "_x.ai/session/update"];

/// A stable id for a synthetic event derived from a grok ext notification —
/// grok stamps `params._meta.eventId`; fall back to a fresh uuid.
fn grok_ext_event_id(params: &serde_json::Value) -> String {
    params
        .get("_meta")
        .and_then(|m| m.get("eventId"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("grok-ext-{}", uuid::Uuid::new_v4().simple()))
}

/// Map grok's private ext notifications — context compaction and dropped
/// prompt images — into `AcpEvent`s.
///
/// grok reports `/compact` (and auto-compaction) results, and the fate of an
/// image it refused to send, on
/// `_x.ai/session_notification` / `_x.ai/session/update` rather than as normal
/// `agent_message_chunk`s. Those methods never match the typed `session/update`
/// pipeline, so without this the whole turn is blank and `/compact` looks like
/// it failed. Only grok emits these, so gate on the agent. Turn-level failures
/// are intentionally NOT handled here — the `session/prompt` response path
/// (`turn_failure_error_event`) already surfaces those, and duplicating them
/// would double-report.
fn map_grok_ext_notification(
    notification: &UntypedMessage,
    agent_type: AgentType,
) -> Option<AcpEvent> {
    if !matches!(agent_type, AgentType::Grok) {
        return None;
    }
    if !GROK_EXT_UPDATE_METHODS.contains(&notification.method()) {
        return None;
    }
    let params = notification.params();
    let update = params.get("update")?;
    match update.get("sessionUpdate").and_then(|v| v.as_str())? {
        // grok always emits `completed` (even a no-op where before == after).
        // Render the shared context-compaction card (recognized frontend-side by
        // `_meta.contextCompaction`, same as codex) carrying the token delta.
        "auto_compact_completed" => {
            let mut meta = serde_json::Map::new();
            meta.insert(
                "contextCompaction".to_string(),
                serde_json::Value::Bool(true),
            );
            if let Some(before) = update.get("tokens_before").and_then(|v| v.as_u64()) {
                meta.insert("tokensBefore".to_string(), before.into());
            }
            if let Some(after) = update.get("tokens_after").and_then(|v| v.as_u64()) {
                meta.insert("tokensAfter".to_string(), after.into());
            }
            Some(AcpEvent::ToolCall {
                tool_call_id: grok_ext_event_id(params),
                title: "Context compaction".to_string(),
                kind: "other".to_string(),
                status: "completed".to_string(),
                content: None,
                raw_input: None,
                raw_output: None,
                locations: None,
                meta: Some(serde_json::Value::Object(meta)),
                images: None,
            })
        }
        // A prompt image was accepted on the wire but dropped before the
        // describe sidecar (too small, oversize, decode failure). Surface it
        // so the user isn't left wondering why Grok "can't see" the shot.
        "image_dropped" => {
            // `notes` is grok's own user-facing sentence, one per dropped image
            // ("Image 1 was dropped before send: too small (1×1); images must be
            // at least 8×8 pixels."). It already names the subject, so it is
            // shown verbatim — prefixing it would read "Image dropped: Image 1
            // was dropped before send: …". Only the shapeless fallbacks get a
            // prefix, because on their own they say nothing about images.
            let message = update
                .get("notes")
                .and_then(|v| v.as_array())
                .map(|notes| {
                    notes
                        .iter()
                        .filter_map(|n| n.as_str())
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    update
                        .get("reason")
                        .or_else(|| update.get("message"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        // A blank reason would render as a bare "Image dropped: "
                        // — worse than the generic sentence below.
                        .filter(|d| !d.is_empty())
                        .map(|d| format!("Image dropped: {d}"))
                })
                .unwrap_or_else(|| "An image was dropped before send.".to_string());
            Some(AcpEvent::Error {
                message,
                agent_type: agent_type.to_string(),
                code: None,
                details: None,
                terminal: false,
            })
        }
        // Compaction itself blew up (e.g. the summarizer model call failed) while
        // the turn still ended cleanly — surface a non-terminal error so the
        // result isn't a silent blank.
        "auto_compact_failed" => Some(AcpEvent::Error {
            message: format!(
                "Context compaction failed{}",
                update
                    .get("reason")
                    .or_else(|| update.get("message"))
                    .and_then(|v| v.as_str())
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default()
            ),
            agent_type: agent_type.to_string(),
            code: None,
            details: None,
            terminal: false,
        }),
        _ => None,
    }
}

/// Map grok's sub-agent lifecycle notifications (`_x.ai/session/update` with
/// `sessionUpdate: subagent_spawned | subagent_progress | subagent_finished`)
/// onto live events for the launching `spawn_subagent` Agent card. STATEFUL —
/// kept out of the pure [`map_grok_ext_notification`] (which doubles as the
/// turn-output predicate and must stay re-invocable).
///
/// Grok never forwards a child's chunks or tool calls over ACP (unlike
/// claude-agent-acp ≥0.63) — these three notifications are the ONLY live
/// signal a subagent emits, so:
///
/// * `subagent_spawned` pairs `subagent_id` → the oldest unpaired spawn call
///   (FIFO; the notifications carry no `tool_call_id` — same pairing as the
///   history parser).
/// * `subagent_progress` becomes a meta-only `ToolCallUpdate`
///   (`meta.grokSubagentProgress`) so the running Agent card can show a live
///   "N tools · M turns · ctx%" line. Replacing the block's meta is safe for
///   this call: its classification rides `rawInput.subagent_type`, not meta.
///   GATED on `turn_active`: after `TurnComplete` the frontend diverts wire
///   tool updates out of the transcript anyway, while the backend
///   `SessionState` apply arm would lazily RECREATE `live_message` for the
///   update — resurrecting a ghost pending card into every later
///   snapshot/attach. An out-of-turn tick is therefore dropped whole (the
///   `subagent_finished` settle below is the out-of-turn-safe channel).
/// * `subagent_finished` for a spawn whose CALL already settled (= a
///   BACKGROUND child; the launch ack was its final wire output) settles via
///   the same `BackgroundActivity` channel Claude's async sub-agents use: the
///   frontend flips the launch card's `[[codeg-background-task]]` marker
///   in-memory (works out-of-turn too), raises the OS notification, and
///   mirrors `outstanding` for the idle-sweep exemption. A BLOCKING spawn is
///   skipped — its own completion frame delivers the output.
fn map_grok_subagent_notification(
    notification: &UntypedMessage,
    agent_type: AgentType,
    turn_active: bool,
    cb_state: &mut CodeBuddyLiveState,
) -> Vec<AcpEvent> {
    map_grok_subagent_notification_inner(notification, agent_type, turn_active, cb_state)
        .unwrap_or_default()
}

/// The body of [`map_grok_subagent_notification`], written with `?` on the many
/// "this notification isn't one of ours / is malformed" checks. A spawn can
/// produce TWO events (the card's session stamp plus the background-activity
/// report), hence the list.
fn map_grok_subagent_notification_inner(
    notification: &UntypedMessage,
    agent_type: AgentType,
    turn_active: bool,
    cb_state: &mut CodeBuddyLiveState,
) -> Option<Vec<AcpEvent>> {
    if !matches!(agent_type, AgentType::Grok) {
        return None;
    }
    if !GROK_EXT_UPDATE_METHODS.contains(&notification.method()) {
        return None;
    }
    let params = notification.params();
    let update = params.get("update")?;
    let subagent_id = update.get("subagent_id").and_then(|v| v.as_str())?;
    // `outstanding` = paired subagents still running whose launch call already
    // settled — i.e. background children codeg would otherwise sweep as idle.
    let outstanding = |cb_state: &CodeBuddyLiveState| {
        cb_state
            .grok_subagent_to_call
            .values()
            .filter(|call_id| cb_state.grok_settled_spawn_ids.contains(*call_id))
            .count() as u32
    };
    match update.get("sessionUpdate").and_then(|v| v.as_str())? {
        "subagent_spawned" => {
            // First pending entry whose captured launch `(description,
            // subagent_type)` is consistent with the notification's own
            // fields. ASYMMETRIC rule: a value captured from the launch input
            // must be matched by an EQUAL value on the notification — a
            // notification that omits the field does NOT wildcard past it
            // (that shape would let a delayed, description-less prior-turn
            // notification steal a described new-turn entry). Only a pending
            // side that captured nothing (unparseable/absent input field —
            // then there is nothing to validate against) is a wildcard. Real
            // captures always carry both fields on both sides, so in-order
            // streams match at the head (plain FIFO); the fail direction for
            // an unmatched notification is pairing nothing (live progress/
            // settle degrade gracefully; history re-parse is unaffected).
            let event_field = |key: &str| {
                update
                    .get(key)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            };
            let matches = |pending: &Option<String>, event: Option<&str>| match (pending, event) {
                (Some(p), Some(e)) => p == e,
                (Some(_), None) => false,
                (None, _) => true,
            };
            let idx = cb_state.grok_pending_spawn_ids.iter().position(|p| {
                matches(&p.description, event_field("description"))
                    && matches(&p.subagent_type, event_field("subagent_type"))
            })?;
            let call_id = cb_state.grok_pending_spawn_ids.remove(idx)?.call_id;
            cb_state
                .grok_subagent_to_call
                .insert(subagent_id.to_string(), call_id.clone());
            // The child's own session id — `child_session_id` when the wire
            // carries it, else the subagent id (they are the same value in
            // every capture). Remembered so later progress ticks can re-send it
            // (`upsert_tool_call` replaces meta wholesale, so every tick must
            // carry it again).
            //
            // Gated by the same `is_safe_subagent_id` the history parser applies
            // to this field: the value is handed to the frontend, which asks
            // `get_conversation` to resolve a session directory by it, so both
            // paths have to reject a traversal-shaped id — not just the one that
            // reads from disk itself. Failing the gate simply leaves the card
            // without a session to open.
            let child_session_id = update
                .get("child_session_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(subagent_id);
            let child_session_id = crate::parsers::is_safe_subagent_id(child_session_id)
                .then(|| child_session_id.to_string());
            if let Some(child) = &child_session_id {
                cb_state
                    .grok_call_child_session
                    .insert(call_id.clone(), child.clone());
            }

            let mut events = Vec::new();
            // Stamp the child's session onto the launching Agent card so it can
            // offer to open the child's transcript WHILE it runs — for a
            // blocking spawn that is the only live window into the child, since
            // its call produces no output until the very end. Gated exactly like
            // a progress tick: out of turn, the `SessionState` apply arm would
            // lazily recreate `live_message` and strand a ghost card in every
            // later snapshot.
            if turn_active && cb_state.grok_progress_eligible.contains(&call_id) {
                events.push(AcpEvent::ToolCallUpdate {
                    tool_call_id: call_id.clone(),
                    title: None,
                    status: None,
                    content: None,
                    raw_input: None,
                    raw_output: None,
                    raw_output_append: None,
                    locations: None,
                    meta: Some(grok_subagent_meta(
                        subagent_id,
                        child_session_id.as_deref(),
                        None,
                    )),
                    images: None,
                });
            }
            // A background launch's call settles before/around the pairing;
            // surface the outstanding count so the connection is exempt from
            // idle sweeps while the child works. Nothing to add for a
            // blocking spawn (its turn is open — nothing can sweep it).
            if cb_state.grok_settled_spawn_ids.contains(&call_id) {
                let session_id = params.get("sessionId").and_then(|v| v.as_str())?;
                events.push(AcpEvent::BackgroundActivity {
                    session_id: session_id.to_string(),
                    turns: Vec::new(),
                    outstanding: outstanding(cb_state),
                    settled: Vec::new(),
                    watermark: 0,
                });
            }
            Some(events)
        }
        "subagent_progress" => {
            if !turn_active {
                return None;
            }
            let call_id = cb_state.grok_subagent_to_call.get(subagent_id)?.clone();
            // Launch-turn-specific gate on top of Prompting: a tick for a
            // PRIOR turn's background child must not be appended into the
            // CURRENT turn's live message (see `grok_progress_eligible`).
            if !cb_state.grok_progress_eligible.contains(&call_id) {
                return None;
            }
            let mut progress = serde_json::Map::new();
            for (wire, out) in [
                ("duration_ms", "durationMs"),
                ("turn_count", "turnCount"),
                ("tool_call_count", "toolCallCount"),
                ("context_usage_pct", "contextUsagePct"),
                ("error_count", "errorCount"),
            ] {
                if let Some(v) = update.get(wire).filter(|v| v.is_number()) {
                    progress.insert(out.to_string(), v.clone());
                }
            }
            if progress.is_empty() {
                return None;
            }
            // Carry the session stamp along: `upsert_tool_call` replaces the
            // block's meta wholesale, so a progress-only payload would drop the
            // "open the child's session" affordance mid-run.
            let child_session_id = cb_state.grok_call_child_session.get(&call_id).cloned();
            Some(vec![AcpEvent::ToolCallUpdate {
                tool_call_id: call_id,
                title: None,
                status: None,
                content: None,
                raw_input: None,
                raw_output: None,
                raw_output_append: None,
                locations: None,
                meta: Some(grok_subagent_meta(
                    subagent_id,
                    child_session_id.as_deref(),
                    Some(serde_json::Value::Object(progress)),
                )),
                images: None,
            }])
        }
        "subagent_finished" => {
            let call_id = cb_state.grok_subagent_to_call.remove(subagent_id)?;
            cb_state.grok_call_child_session.remove(&call_id);
            if !cb_state.grok_settled_spawn_ids.contains(&call_id) {
                // Blocking spawn: the child's output arrives on the call's own
                // completion frame; the settle channel would double-render it.
                return None;
            }
            let session_id = params.get("sessionId").and_then(|v| v.as_str())?;
            let status = update
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("completed");
            let result = update
                .get("output")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| crate::parsers::truncate_str(s, crate::parsers::claude::BACKGROUND_RESULT_MAX_CHARS));
            Some(vec![AcpEvent::BackgroundActivity {
                session_id: session_id.to_string(),
                turns: Vec::new(),
                outstanding: outstanding(cb_state),
                settled: vec![crate::acp::types::BackgroundSettledInfo {
                    task_id: subagent_id.to_string(),
                    status: status.to_string(),
                    summary: None,
                    tool_use_id: Some(call_id),
                    result,
                    // The settle itself flips the card in-memory; no later
                    // overlay content follows, so the syncing hint would dangle.
                    wire_visible: true,
                }],
                watermark: 0,
            }])
        }
        _ => None,
    }
}

/// The `meta` payload a Grok sub-agent's launching Agent card carries:
/// `grokSubagentSession` (ids — lets the card open the child's transcript) and,
/// while it runs, `grokSubagentProgress` (the live "N tools · M turns · ctx%"
/// line). Built in one place because `upsert_tool_call` REPLACES a block's
/// meta: every emission has to carry the whole picture, not just its own half.
fn grok_subagent_meta(
    subagent_id: &str,
    child_session_id: Option<&str>,
    progress: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut session = serde_json::Map::new();
    session.insert("subagentId".to_string(), subagent_id.into());
    if let Some(child) = child_session_id {
        session.insert("childSessionId".to_string(), child.into());
    }
    let mut meta = serde_json::Map::new();
    meta.insert(
        "grokSubagentSession".to_string(),
        serde_json::Value::Object(session),
    );
    if let Some(progress) = progress {
        meta.insert("grokSubagentProgress".to_string(), progress);
    }
    serde_json::Value::Object(meta)
}

/// Whether a dispatch is a grok ext notification that
/// `map_grok_ext_notification` renders as visible turn output (a compaction
/// card, a compaction error, or a dropped-image error). The active-turn loop
/// consults this BEFORE the typed
/// pipeline to mark the turn as non-empty: a `/compact` turn emits only these
/// ext notifications and no standard `agent_message_chunk`, so without this its
/// `end_turn` is reclassified as `"empty"` and re-surfaced as a spurious error —
/// the exact symptom this change removes. Reuses `map_grok_ext_notification` so
/// the handled-variant set can never drift from what actually emits. A turn
/// whose only output was a dropped image therefore reports THAT, rather than
/// the generic empty-turn failure it used to.
fn grok_ext_notification_is_turn_output(dispatch: &Dispatch, agent_type: AgentType) -> bool {
    match dispatch {
        Dispatch::Notification(notification) => {
            map_grok_ext_notification(notification, agent_type).is_some()
        }
        _ => false,
    }
}

/// Whether a grok ext notification would raise a user-facing ALERT (status-bar
/// entry + OS notification), as opposed to rendering a card in the turn.
///
/// Only the historical `session/load` replay asks: those notifications describe
/// a PAST session, so re-raising their alerts would report a compaction failure
/// or a dropped image as if it were happening now, for a session the user is
/// merely opening. Reuses the mapper for the same reason
/// [`grok_ext_notification_is_turn_output`] does — the alerting set cannot drift
/// away from what actually emits.
fn grok_ext_notification_is_alert(dispatch: &Dispatch, agent_type: AgentType) -> bool {
    match dispatch {
        Dispatch::Notification(notification) => matches!(
            map_grok_ext_notification(notification, agent_type),
            Some(AcpEvent::Error { .. })
        ),
        _ => false,
    }
}

/// Whether codeg has a mapper for this ext-notification method.
///
/// Used ONLY to keep the unrecognized-method log quiet about methods we do know
/// and merely declined to map this time. That distinction is the whole point:
/// `_claude/sdkMessage` arrives for every SDK message and only maps when the
/// payload is an API retry, so logging every unmapped one would put a line on a
/// per-message hot path — the shape that once grew a server's log file to 217GB.
///
/// Forgetting to list a newly-mapped method here fails in the SAFE direction —
/// its unmapped payloads just get logged (noise, still visible). The dangerous
/// direction is the reverse: listing a method no mapper claims would silence
/// exactly the gap this log exists to expose.
fn is_known_ext_method(method: &str) -> bool {
    method == CLAUDE_SDK_EXT_METHOD || GROK_EXT_UPDATE_METHODS.contains(&method)
}

/// Last stop for a dispatch the typed `session/update` pipeline didn't claim.
///
/// Every exit here DROPS the message, which is the pre-existing behavior and
/// stays that way — the change is that a drop is no longer invisible. All lines
/// are `debug!`: an agent is free to speak methods codeg doesn't implement, and
/// a per-message `warn!` on a chatty agent is how log storms start.
async fn maybe_emit_ext_notification(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    agent_type: AgentType,
    dispatch: Dispatch,
    cb_state: &mut CodeBuddyLiveState,
) {
    let notification = match dispatch {
        Dispatch::Notification(notification) => notification,
        // An agent calling a client method codeg doesn't implement. The
        // responder is dropped without a reply (as before), so the agent's
        // request goes unanswered — worth seeing when triaging a stalled turn.
        Dispatch::Request(request, _responder) => {
            tracing::debug!(
                method = %request.method(),
                "[ACP] dropping unhandled agent request (no reply will be sent)"
            );
            return;
        }
        // A response is normally consumed by the caller waiting on it, so one
        // surfacing here is unexpected rather than routine. (It still carries a
        // `ResponseRouter`, so "unroutable" would overstate it — the point is
        // only that nothing on this path wants it.)
        Dispatch::Response(..) => {
            tracing::debug!("[ACP] dropping unexpected response dispatch");
            return;
        }
    };

    // The CURRENT connection status decides whether a grok `subagent_progress`
    // tick may touch the live tool call — the same Prompting predicate the
    // out-of-turn chunk defenses use (#870). Read here (not at the call sites)
    // so a stray notification the active-turn loop drains AFTER the status
    // already flipped back is still classified as out-of-turn.
    let turn_active = state.read().await.status == ConnectionStatus::Prompting;
    // A grok `subagent_spawned` can yield TWO events (the card's session stamp
    // and the background-activity report), so this mapper hands back a list; an
    // empty one means "not mine", and the chain continues as before.
    let grok_subagent_events =
        map_grok_subagent_notification(&notification, agent_type, turn_active, cb_state);
    if !grok_subagent_events.is_empty() {
        for event in grok_subagent_events {
            emit_with_state(state, emitter, event).await;
        }
    } else if let Some(event) = map_claude_sdk_ext_notification(&notification)
        .or_else(|| map_grok_ext_notification(&notification, agent_type))
    {
        emit_with_state(state, emitter, event).await;
    } else if !is_known_ext_method(notification.method()) {
        // The gap #409's second point was reaching for: an agent emitting an ext
        // method codeg has never heard of was previously indistinguishable from
        // an agent saying nothing at all.
        tracing::debug!(
            method = %notification.method(),
            agent = %agent_type,
            "[ACP] ignoring unrecognized ext notification"
        );
    }
}

/// Fix null fields in `usage_update` notifications that would otherwise fail deserialization.
///
/// Some ACP agents send `"used": null` in usage_update notifications, but the
/// upstream schema expects `u64`. This function patches the raw JSON params
/// so that `null` numeric fields default to `0`.
fn fix_usage_update_nulls(mut dispatch: Dispatch) -> Dispatch {
    if let Dispatch::Notification(ref mut msg) = dispatch {
        if let Some(update) = msg.params.get_mut("update") {
            if update.get("sessionUpdate").and_then(|v| v.as_str()) == Some("usage_update") {
                if update.get("used").map(|v| v.is_null()).unwrap_or(false) {
                    update["used"] = serde_json::Value::from(0u64);
                }
                if update.get("size").map(|v| v.is_null()).unwrap_or(false) {
                    update["size"] = serde_json::Value::from(0u64);
                }
            }
        }
    }
    dispatch
}

/// Convert a SessionUpdate into AcpEvent(s) and emit to frontend.
///
/// `raw_output_cache` is a per-session cache used to detect cumulative
/// snapshots from agents and convert them into incremental deltas so the
/// event pipeline never carries a full N-MB tool output more than once.
///
/// `cb_state` is the per-session `CodeBuddyLiveState`: the authoritative
/// title-rewrite map (so a status-only update can't downgrade an Agent /
/// delegation card and un-nest its children) plus the open-sub-agent window used
/// to suppress a CodeBuddy sub-agent's interleaved thought/message chunks.
/// Mirrors `raw_output_cache`'s lifetime.
async fn emit_conversation_update(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    agent_type: AgentType,
    update: SessionUpdate,
    cwd: Option<&str>,
    raw_output_cache: &mut ToolCallOutputCache,
    cb_state: &mut CodeBuddyLiveState,
) {
    match update {
        SessionUpdate::UserMessageChunk(_) => {
            // User echo chunks are informational for transcript sync and
            // currently not rendered in live ACP UI.
        }
        SessionUpdate::AgentMessageChunk(ContentChunk {
            content: ContentBlock::Text(text),
            meta,
            ..
        }) => {
            // Drop a CodeBuddy sub-agent's interleaved message text — it belongs
            // to the Agent pill, not the main thread (see
            // `should_suppress_subagent_chunk`). No-op for every other agent.
            if !should_suppress_subagent_chunk(
                agent_type,
                !cb_state.open_subagents.is_empty(),
                meta.as_ref(),
            ) {
                // Claude subagent chunks (claude-agent-acp ≥0.63 with the
                // `subagent-transcript` capability) are NOT suppressed: they
                // emit with their parent id so the frontend can route them
                // into the live Agent capsule.
                let parent_tool_use_id = claude_chunk_parent_tool_use_id(agent_type, meta.as_ref());
                emit_with_state(
                    state,
                    emitter,
                    AcpEvent::ContentDelta {
                        text: text.text,
                        parent_tool_use_id,
                    },
                )
                .await;
            }
        }
        SessionUpdate::AgentMessageChunk(_) => {
            // Non-text chunks are currently not surfaced in live streaming UI.
        }
        SessionUpdate::AgentThoughtChunk(ContentChunk {
            content: ContentBlock::Text(text),
            meta,
            ..
        }) => {
            // Same suppression for a sub-agent's interleaved reasoning.
            if !should_suppress_subagent_chunk(
                agent_type,
                !cb_state.open_subagents.is_empty(),
                meta.as_ref(),
            ) {
                let parent_tool_use_id = claude_chunk_parent_tool_use_id(agent_type, meta.as_ref());
                emit_with_state(
                    state,
                    emitter,
                    AcpEvent::Thinking {
                        text: text.text,
                        parent_tool_use_id,
                    },
                )
                .await;
            }
        }
        SessionUpdate::AgentThoughtChunk(_) => {
            // Non-text thought chunks are currently ignored.
        }
        SessionUpdate::ToolCall(tc) => {
            // codex-acp #304 surfaces codex `subAgentActivity` as a live
            // `tool_call`. A launch becomes an Agent capsule (its own rawInput
            // is orchestration bookkeeping, so it is replaced wholesale); the
            // other lifecycle markers stay dropped. See
            // `classify_codex_subagent_activity`.
            let codex_subagent = match classify_codex_subagent_activity(agent_type, tc.meta.as_ref())
            {
                CodexSubagentActivity::None => None,
                CodexSubagentActivity::Started(input) => Some(input),
                CodexSubagentActivity::Other => return,
            };
            let tool_call_id = tc.tool_call_id.to_string();
            // Grok emits a redundant `tool_call` for its native ask_user_question
            // alongside the blocking `_x.ai/ask_user_question` ext request codeg
            // answers with the interactive card; drop it here (remembering the id so
            // later status-only updates that lost the meta are dropped too).
            if grok_meta_marks_ask_user(agent_type, tc.meta.as_ref()) {
                cb_state.grok_ask_tool_ids.insert(tool_call_id);
                return;
            }
            // CodeBuddy double-wraps a deferred MCP result as a `{type,text}`
            // content part; peel it (in both the content and raw_output channels)
            // so the dedicated delegation cards parse it instead of showing raw JSON.
            // codex-acp reports file edits as a `Diff` content block with no
            // `raw_input`; synthesize a canonical edit so the call classifies/
            // renders as an edit instead of a tool named after the raw diff
            // header (see synthesize_edit_input_from_diffs). When we do, drop the
            // `Diff` from `content` — it's the same edit re-serialized hunklessly,
            // which would otherwise double the event and skew the header +/- stats.
            // Blank raw_input is treated as absent (matches the frontend guard).
            // Grok wraps every MCP call in a `use_tool` envelope; peel it so the
            // call is correlated/classified/parsed as a direct MCP call — its
            // real `tool_input` becomes `raw_input`, its `tool_name` the title
            // below (see unwrap_grok_use_tool).
            let grok_use_tool = if matches!(agent_type, AgentType::Grok) {
                unwrap_grok_use_tool(tc.raw_input.as_ref())
            } else {
                None
            };
            let own_raw_input = match &grok_use_tool {
                Some((_, inner)) => {
                    json_value_to_text(&Some(inner.clone())).filter(|t| !t.trim().is_empty())
                }
                None => json_value_to_text(&tc.raw_input).filter(|t| !t.trim().is_empty()),
            };
            let synthesized_edit = if own_raw_input.is_none() {
                synthesize_edit_input_from_diffs(&tc.content)
            } else {
                None
            };
            let content =
                serialize_tool_call_content(&tc.content, synthesized_edit.is_none())
                    .map(|c| unwrap_codebuddy_deferred_output(agent_type, &c).unwrap_or(c));
            let images = extract_tool_call_images(&tc.content);
            let codex_subagent_launch = codex_subagent.is_some();
            let raw_input = codex_subagent
                .or(synthesized_edit)
                .or(own_raw_input)
                .map(|text| resolve_live_tool_input(&text, cwd));
            // Initial tool_call notification — the frontend reducer
            // treats `raw_output` as a full replacement, so we bypass
            // the diff path and seed the cache with the current snapshot.
            let raw_output_text = if matches!(agent_type, AgentType::Grok) {
                // Grok's structured rawOutput would shadow `content` and render
                // empty; take the parity path (see grok_live_tool_output).
                grok_live_tool_output(&content, &tc.raw_output)
            } else {
                json_value_to_text(&tc.raw_output)
                    .map(|text| unwrap_codebuddy_deferred_output(agent_type, &text).unwrap_or(text))
                    .map(|text| structurize_live_output(&text))
            };
            let raw_output =
                raw_output_text.and_then(|text| raw_output_cache.seed(&tool_call_id, &text));
            let locations = if tc.locations.is_empty() {
                None
            } else {
                serde_json::to_value(&tc.locations).ok()
            };
            // Read the CodeBuddy sub-agent markers from `_meta` BEFORE it's moved
            // into the emitted `Value` below — `meta_marks_subagent` is the early,
            // reliable signal (frame 1) that keeps the Agent pill from flickering;
            // `meta_marks_background` keeps a concurrent sub-agent out of the
            // suppression window (see fn docs).
            // `codex_subagent_launch` joins the CodeBuddy meta signal here for
            // the same reason it exists: it is what records the authoritative
            // "agent" title, so the status-only follow-up (which carries no
            // rawInput at all) re-asserts it instead of downgrading the capsule
            // to a generic tool card.
            let meta_marks_subagent = codebuddy_meta_marks_subagent(agent_type, tc.meta.as_ref())
                || codex_subagent_launch;
            let meta_marks_background = codebuddy_meta_marks_background(agent_type, tc.meta.as_ref());
            let grok_spawn = grok_meta_marks_spawn_subagent(agent_type, tc.meta.as_ref());
            let meta = tc.meta.map(serde_json::Value::Object);
            let status = format!("{:?}", tc.status).to_lowercase();
            raw_output_cache.remove_if_final(&tool_call_id, Some(status.as_str()));
            // Track Grok's spawn_subagent lifecycle for the subagent-notification
            // pairing (progress meta + finished settle). No-op for other agents.
            track_grok_spawn_call(
                cb_state,
                grok_spawn,
                Some(status.as_str()),
                &tool_call_id,
                &raw_input,
            );
            // Avoid logging titles/payloads below — they can be model-generated
            // user task descriptions (PII-adjacent) and would create noise in
            // server-mode log sinks. The opaque tool_call_id is enough to
            // correlate these events with downstream traces.
            // Record the peeled Grok MCP name as an authoritative title override
            // so later sparse `use_tool` updates (which carry the generic wrapper
            // title and no raw_input) re-assert it via resolve_rewritten_title
            // instead of reverting the delegation card to a generic tool. Mirrors
            // the CodeBuddy DeferExecuteTool / sub-agent title persistence.
            if let Some((name, _)) = &grok_use_tool {
                cb_state
                    .title_overrides
                    .insert(tool_call_id.clone(), name.clone());
            }
            // Resolve (and record) any authoritative title rewrite so a later
            // status-only update can't downgrade this card (see fn doc).
            let title = resolve_rewritten_title(
                agent_type,
                &raw_input,
                &tool_call_id,
                false,
                meta_marks_subagent,
                &mut cb_state.title_overrides,
            )
            .unwrap_or(tc.title);
            // Mark Cursor's identity-less MCP announcements as eligible for the
            // completion-time result sniff. Scoping the sniff to ids announced
            // with this exact title keeps a `shell`/`read` call whose OUTPUT
            // echoes a delegation ack from being re-titled.
            if matches!(agent_type, AgentType::Cursor)
                && title == crate::acp::lifecycle::CURSOR_IDENTITYLESS_MCP_TITLE
            {
                cb_state.cursor_generic_mcp_ids.insert(tool_call_id.clone());
            }
            // Open/close the sub-agent suppression window for this call. `title ==
            // "agent"` iff this is a classified native sub-agent (DeferExecuteTool
            // rewrites to an `mcp__…` name, never "agent").
            track_subagent_window(
                agent_type,
                title == "agent",
                meta_marks_background,
                Some(status.as_str()),
                &tool_call_id,
                &mut cb_state.open_subagents,
                &mut cb_state.closed_subagents,
            );
            emit_with_state(
                state,
                emitter,
                AcpEvent::ToolCall {
                    tool_call_id,
                    title,
                    kind: format!("{:?}", tc.kind).to_lowercase(),
                    status,
                    content,
                    raw_input,
                    raw_output,
                    locations,
                    meta,
                    images,
                },
            )
            .await;
        }
        SessionUpdate::ToolCallUpdate(tcu) => {
            // Symmetric with the `ToolCall` arm: the follow-up carries the same
            // `_meta.codex.subagent`, so it classifies identically — a launch's
            // completion is forwarded (settling its capsule), any other
            // lifecycle marker's is dropped like its opening frame was.
            let codex_subagent =
                match classify_codex_subagent_activity(agent_type, tcu.meta.as_ref()) {
                    CodexSubagentActivity::None => None,
                    CodexSubagentActivity::Started(input) => Some(input),
                    CodexSubagentActivity::Other => return,
                };
            let tool_call_id = tcu.tool_call_id.to_string();
            // Suppress the redundant update stream for grok's ask_user_question
            // (see the ToolCall arm): match the tracked id, or the meta on a late
            // update that still carries it.
            if cb_state.grok_ask_tool_ids.contains(&tool_call_id)
                || grok_meta_marks_ask_user(agent_type, tcu.meta.as_ref())
            {
                return;
            }
            // Peel CodeBuddy's `{type,text}` deferred-MCP wrapper here too — the
            // result often arrives on an update (see raw_output below).
            // Same Diff→canonical-edit hoist as the initial ToolCall path: the
            // edit may first arrive on an update. Drop the redundant Diff from
            // `content` when hoisted. The reducer preserves a prior raw_input on
            // status-only updates (`action.raw_input ?? block.info.raw_input`).
            // Grok `use_tool` unwrap, symmetric with the ToolCall arm — a rare
            // update that re-sends the envelope is peeled the same way (most
            // updates carry no raw_input, so this resolves to None and the
            // reducer keeps the prior unwrapped input).
            let grok_use_tool = if matches!(agent_type, AgentType::Grok) {
                unwrap_grok_use_tool(tcu.fields.raw_input.as_ref())
            } else {
                None
            };
            let own_raw_input = match &grok_use_tool {
                Some((_, inner)) => {
                    json_value_to_text(&Some(inner.clone())).filter(|t| !t.trim().is_empty())
                }
                None => {
                    json_value_to_text(&tcu.fields.raw_input).filter(|t| !t.trim().is_empty())
                }
            };
            let synthesized_edit = if own_raw_input.is_none() {
                tcu.fields
                    .content
                    .as_deref()
                    .and_then(synthesize_edit_input_from_diffs)
            } else {
                None
            };
            let content = tcu
                .fields
                .content
                .as_deref()
                .and_then(|c| serialize_tool_call_content(c, synthesized_edit.is_none()))
                .map(|c| unwrap_codebuddy_deferred_output(agent_type, &c).unwrap_or(c));
            let images = tcu
                .fields
                .content
                .as_deref()
                .and_then(extract_tool_call_images);
            let codex_subagent_launch = codex_subagent.is_some();
            let raw_input = codex_subagent
                .or(synthesized_edit)
                .or(own_raw_input)
                .map(|text| resolve_live_tool_input(&text, cwd));
            // Diff the incoming raw_output against the last snapshot we
            // emitted for this tool call. This turns cumulative snapshots
            // from agents (Claude Code, Codex, …) into incremental deltas
            // with `raw_output_append=true`, collapsing the O(N²) transfer
            // problem to O(N) while capping any single emitted chunk to
            // MAX_SINGLE_EMIT_BYTES.
            let raw_output_text = if matches!(agent_type, AgentType::Grok) {
                // Grok's structured rawOutput would shadow `content` and render
                // empty; take the parity path (see grok_live_tool_output).
                grok_live_tool_output(&content, &tcu.fields.raw_output)
            } else {
                json_value_to_text(&tcu.fields.raw_output)
                    .map(|text| unwrap_codebuddy_deferred_output(agent_type, &text).unwrap_or(text))
                    .map(|text| structurize_live_output(&text))
            };
            let (raw_output, raw_output_append) = match raw_output_text {
                Some(text) => match raw_output_cache.consume(&tool_call_id, &text) {
                    Some((payload, append)) => (Some(payload), Some(append)),
                    None => (None, None),
                },
                None => (None, None),
            };
            let locations = tcu
                .fields
                .locations
                .as_ref()
                .filter(|l| !l.is_empty())
                .and_then(|l| serde_json::to_value(l).ok());
            let meta_marks_subagent = codebuddy_meta_marks_subagent(agent_type, tcu.meta.as_ref())
                || codex_subagent_launch;
            let meta_marks_background = codebuddy_meta_marks_background(agent_type, tcu.meta.as_ref());
            let grok_spawn = grok_meta_marks_spawn_subagent(agent_type, tcu.meta.as_ref());
            let meta = tcu.meta.clone().map(serde_json::Value::Object);
            let status = tcu.fields.status.map(|s| format!("{:?}", s).to_lowercase());
            raw_output_cache.remove_if_final(&tool_call_id, status.as_deref());
            // Symmetric with the ToolCall arm: an update may carry the terminal
            // status (and, on grok, usually re-carries the `x.ai/tool` meta).
            track_grok_spawn_call(cb_state, grok_spawn, status.as_deref(), &tool_call_id, &raw_input);
            // Ordering variant: `subagent_spawned` can pair BEFORE the launch
            // call's terminal frame arrives. The pairing site skipped its
            // outstanding emission then (call not yet settled), so surface the
            // count here — a completed launch with a paired, still-running
            // child is a background subagent codeg must not idle-sweep. The
            // common ordering (completed first) emits from the pairing site,
            // and `subagent_finished` always re-emits the corrected count.
            if status.as_deref() == Some("completed")
                && cb_state
                    .grok_subagent_to_call
                    .values()
                    .any(|call| call == &tool_call_id)
            {
                let session_id = state.read().await.external_id.clone();
                if let Some(session_id) = session_id {
                    let outstanding = cb_state
                        .grok_subagent_to_call
                        .values()
                        .filter(|call| cb_state.grok_settled_spawn_ids.contains(*call))
                        .count() as u32;
                    emit_with_state(
                        state,
                        emitter,
                        AcpEvent::BackgroundActivity {
                            session_id,
                            turns: Vec::new(),
                            outstanding,
                            settled: Vec::new(),
                            watermark: 0,
                        },
                    )
                    .await;
                }
            }
            // Re-assert any authoritative title rewrite (see fn doc): an update
            // that carries the subagent/deferred marker classifies (and records)
            // the card, and — the key fix — a later status-only update that LOST
            // the marker but carries the agent's raw (non-agent) title still
            // resolves to the recorded override, so the Agent/delegation card and
            // its child nesting (`getToolName === "agent"`) don't revert to a
            // generic tool call mid-stream. Falls back to the event's own title
            // for never-classified tool calls.
            // Symmetric with the ToolCall arm: a (rare) update that re-sends the
            // envelope records the peeled name so it survives later sparse updates.
            if let Some((name, _)) = &grok_use_tool {
                cb_state
                    .title_overrides
                    .insert(tool_call_id.clone(), name.clone());
            }
            // Cursor loses MCP tool identity on the wire entirely (announced as
            // "MCP: tool" before McpArgs exists; updates never resend title or
            // raw_input). The completion update's result text is the one signal
            // left — recover the codeg-mcp companion identity from it and record
            // it as an authoritative override so the delegation / status cards
            // resolve instead of a generic tool. Gated to ids this connection
            // announced with the identity-less title (see the
            // `cursor_generic_mcp_ids` field doc); the entry is dropped once
            // the call goes terminal.
            if matches!(agent_type, AgentType::Cursor)
                && cb_state.cursor_generic_mcp_ids.contains(&tool_call_id)
            {
                if let Some(name) = cursor_companion_title_from_content(content.as_deref()) {
                    cb_state
                        .title_overrides
                        .insert(tool_call_id.clone(), name.to_string());
                }
                if matches!(status.as_deref(), Some("completed") | Some("failed")) {
                    cb_state.cursor_generic_mcp_ids.remove(&tool_call_id);
                }
            }
            let title = resolve_rewritten_title(
                agent_type,
                &raw_input,
                &tool_call_id,
                true,
                meta_marks_subagent,
                &mut cb_state.title_overrides,
            )
            .or(tcu.fields.title);
            // Keep/close the sub-agent suppression window by status (an update
            // resolving to "agent" is a classified native sub-agent).
            track_subagent_window(
                agent_type,
                title.as_deref() == Some("agent"),
                meta_marks_background,
                status.as_deref(),
                &tool_call_id,
                &mut cb_state.open_subagents,
                &mut cb_state.closed_subagents,
            );
            emit_with_state(
                state,
                emitter,
                AcpEvent::ToolCallUpdate {
                    tool_call_id,
                    title,
                    status,
                    content,
                    raw_input,
                    raw_output,
                    raw_output_append,
                    locations,
                    meta,
                    images,
                },
            )
            .await;
        }
        SessionUpdate::CurrentModeUpdate(update) => {
            emit_with_state(
                state,
                emitter,
                AcpEvent::ModeChanged {
                    mode_id: update.current_mode_id.to_string(),
                },
            )
            .await;
        }
        SessionUpdate::Plan(plan) => {
            emit_with_state(
                state,
                emitter,
                AcpEvent::PlanUpdate {
                    entries: map_plan_entries(&plan),
                },
            )
            .await;
        }
        SessionUpdate::ConfigOptionUpdate(update) => {
            emit_session_config_options_values(state, emitter, agent_type, update.config_options)
                .await;
        }
        SessionUpdate::AvailableCommandsUpdate(update) => {
            // Drop config-option state toggles (codex `/plan` — see
            // `is_config_option_state_command`): they're already the
            // `collaboration_mode` selector, not invokable commands. Then dedup:
            // some agents (e.g. Claude Code with overlapping user/project slash
            // commands) emit duplicate entries sharing the same name. Keep the
            // first occurrence so downstream consumers don't render duplicates;
            // the frontend reducer also dedupes as a defensive measure.
            let mut seen = HashSet::new();
            let commands: Vec<AvailableCommandInfo> = update
                .available_commands
                .iter()
                .filter(|cmd| !is_config_option_state_command(agent_type, cmd.meta.as_ref()))
                .filter(|cmd| seen.insert(cmd.name.clone()))
                .map(|cmd| {
                    let input_hint = cmd.input.as_ref().map(|input| match input {
                        sacp::schema::AvailableCommandInput::Unstructured(u) => u.hint.clone(),
                        _ => String::new(),
                    });
                    AvailableCommandInfo {
                        name: cmd.name.clone(),
                        description: cmd.description.clone(),
                        input_hint,
                    }
                })
                .collect();
            emit_with_state(state, emitter, AcpEvent::AvailableCommands { commands }).await;
        }
        SessionUpdate::UsageUpdate(update) => {
            emit_with_state(
                state,
                emitter,
                AcpEvent::UsageUpdate {
                    used: update.used,
                    size: update.size,
                },
            )
            .await;
        }
        SessionUpdate::SessionInfoUpdate(info) => {
            // codex-acp v1.1.0 (#263) reports `/goal` transitions as structured
            // session metadata instead of live "Goal updated (…)" agent text.
            // The goal object rides under ONE of two meta keys, selected per
            // connection at initialize (`SessionState.neutral_goal_channel`,
            // see `session_info_goal_value`): the provider-neutral
            // `_meta.goal` for adapters advertising the goal extension
            // (claude-agent-acp 0.66+, codex-acp 1.2+ — which dropped the
            // legacy key), else the legacy `_meta.codex.goal`. Either way it
            // maps onto codeg's canonical create_goal/update_goal synthetic
            // tool call so the existing goal-card pipeline
            // (groupGoalRuns/GoalCard) renders it — byte-identical to the
            // history path (parsers/codex.rs). Agents publishing no goal meta
            // no-op here. The neutral snapshot's status vocabulary
            // (active|paused|blocked|limited|complete) passes through
            // `normalize_goal_status` unchanged; its extra fields
            // (createdAt/updatedAt/iterations/lastReason/controlMethod)
            // survive inside the marker's raw goal object for the card.
            // (`info.title` is Codex's native thread name; it is adopted via the
            // parser auto-title path on the next conversation fetch, not here, to
            // keep this DB-agnostic emit path unchanged — see parsers/codex.rs.)
            let neutral_goal_channel = state.read().await.neutral_goal_channel;
            if let Some(goal) =
                session_info_goal_value(neutral_goal_channel, info.meta.as_ref())
            {
                if let Some(marker) =
                    crate::acp::codex_goal::next_goal_marker(&mut cb_state.codex_open_goal, goal)
                {
                    cb_state.codex_goal_seq += 1;
                    let tool_call_id =
                        crate::acp::codex_goal::goal_tool_call_id(cb_state.codex_goal_seq);
                    emit_with_state(
                        state,
                        emitter,
                        AcpEvent::ToolCall {
                            tool_call_id,
                            title: marker.title,
                            kind: "other".to_string(),
                            status: "completed".to_string(),
                            content: None,
                            raw_input: Some(marker.input_json),
                            raw_output: Some(marker.output_json),
                            locations: None,
                            meta: None,
                            images: None,
                        },
                    )
                    .await;
                }
                // Mirror "a goal run is open" (⟺ the last snapshot was active,
                // see `next_goal_marker`) onto the session state, where
                // `ConnectionManager::goal_control` can read it: only an ACTIVE
                // goal justifies following a pause/clear with an interrupt. A
                // paused goal is not driving anything, so clearing it must
                // leave whatever the user started themselves alone.
                state.write().await.goal_active = cb_state.codex_open_goal.is_some();
            }
            // JetBrains AIR typed session failure (claude-agent-acp 0.67+/
            // codex-acp 1.2+): published only because codeg advertises
            // `clientCapabilities._meta.jetbrains.air` (see
            // `build_client_capabilities`). Valid upserts are forwarded
            // verbatim — the monotonic id+revision merge runs identically in
            // `SessionState::apply_event` and the frontend reducer, so a
            // stale or replayed record is rejected the same way everywhere.
            // A record without usable identity cannot merge and is dropped.
            if let Some(raw) = air_session_failure(info.meta.as_ref()) {
                match parse_session_failure_record(raw) {
                    Some(record) => {
                        emit_with_state(state, emitter, AcpEvent::SessionFailure { record })
                            .await;
                    }
                    None => tracing::debug!(
                        "[ACP] dropped AIR sessionFailure without usable id/revision: {raw:?}"
                    ),
                }
            }
            // codex-acp #289 (v1.1.3+): a retryable turn error rides under
            // `_meta.codex.error` (only when `willRetry == true`) and the turn
            // stays alive. Surface a transient retry indicator (the frontend
            // reuses the Claude API-retry banner); it is NOT a turn failure.
            // With AIR advertised (above), codex 1.2+ REPLACES this channel
            // with severity-"warning" failure records, so this indicator now
            // serves only non-advertised/legacy paths.
            if let Some((message, error_status)) = codex_retry_indicator(info.meta.as_ref()) {
                emit_with_state(
                    state,
                    emitter,
                    AcpEvent::TurnRetrying {
                        message,
                        error_status,
                    },
                )
                .await;
            }
        }
        other => {
            // Unhandled update types, for debugging. DEBUG, not INFO: this arm
            // runs once per `session/update` notification, and `{other:?}` is the
            // whole payload — for a chunk-shaped variant that is the agent's full
            // text. At INFO a single agent emitting an update kind codeg doesn't
            // map would write the entire conversation to disk several times over
            // under the default level (issue #427). Nothing acts on this line;
            // it exists to be read while adding support for a new variant, which
            // is exactly when `debug` is on.
            //
            // The default-level signal for "a variant we don't map swallowed the
            // reply" is not this line but the empty-turn diagnosis: such an
            // update decodes fine, so `TurnOutputProbe::note_update` files it as
            // metadata and the turn reports `MetadataOnly`. Turn `debug` on from
            // there to find out *which* variant.
            tracing::debug!("[ACP] Unhandled SessionUpdate: {:?}", other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sacp::schema::{Diff, SessionConfigId};

    /// Unwrap a select selector. The Grok synthesizers below only ever build
    /// selects, so any other kind is a test failure rather than a branch to
    /// handle — this keeps the assertions as terse as the irrefutable `let`
    /// they replaced (which stopped compiling once `SessionConfigKindInfo`
    /// gained its `Boolean` variant).
    fn expect_select(kind: &SessionConfigKindInfo) -> &SessionConfigSelectInfo {
        match kind {
            SessionConfigKindInfo::Select(sel) => sel,
            other => panic!("expected a select config option, got {other:?}"),
        }
    }

    // ── PermissionQueue (#442) ──────────────────────────────────────────────
    //
    // The queue is what stops N concurrent `session/request_permission`s from
    // collapsing into the single card slot and stranding the losers' responders
    // forever. Driven here through a stub responder because sacp's `Responder`
    // has private fields and no public constructor.

    /// How a stubbed responder was settled. `Ord` so the drain assertions can
    /// sort — `HashMap::drain` yields an arbitrary order.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
    enum StubSettled {
        Selected,
        Cancelled,
    }

    /// Records `(request_id, how)` into a shared log when settled, so a test can
    /// assert that EVERY parked responder was answered exactly once — the whole
    /// point of the queue is that none is silently dropped.
    struct StubResponder {
        request_id: String,
        log: Arc<std::sync::Mutex<Vec<(String, StubSettled)>>>,
    }

    impl PermissionResponder for StubResponder {
        fn respond_selected(self, _option_id: String) {
            self.log
                .lock()
                .unwrap()
                .push((self.request_id, StubSettled::Selected));
        }

        fn respond_cancelled(self) {
            self.log
                .lock()
                .unwrap()
                .push((self.request_id, StubSettled::Cancelled));
        }
    }

    type StubLog = Arc<std::sync::Mutex<Vec<(String, StubSettled)>>>;

    /// Admit `id` into `queue`; returns the card the queue says to publish now.
    fn admit_stub(
        queue: &mut PermissionQueue<StubResponder>,
        log: &StubLog,
        id: &str,
    ) -> Option<QueuedPermission> {
        queue.admit(
            StubResponder {
                request_id: id.to_string(),
                log: Arc::clone(log),
            },
            QueuedPermission {
                request_id: id.to_string(),
                tool_call: serde_json::json!({ "toolCallId": id }),
                options: vec![],
            },
        )
    }

    fn stub_queue() -> (PermissionQueue<StubResponder>, StubLog) {
        (
            PermissionQueue::default(),
            Arc::new(std::sync::Mutex::new(Vec::new())),
        )
    }

    #[test]
    fn permission_queue_shows_only_the_first_of_concurrent_requests() {
        let (mut q, log) = stub_queue();
        // The regression itself: three approvals arrive back-to-back (codex
        // running parallel shell commands). Before the queue, each emit
        // overwrote the last and the first two became unanswerable.
        let first = admit_stub(&mut q, &log, "a").expect("first request must be shown");
        assert_eq!(first.request_id, "a");
        assert!(
            admit_stub(&mut q, &log, "b").is_none(),
            "a second concurrent request must queue, not replace the visible card"
        );
        assert!(admit_stub(&mut q, &log, "c").is_none());
        assert_eq!(q.waiting_len(), 2);
        assert_eq!(q.showing.as_deref(), Some("a"));
        assert!(log.lock().unwrap().is_empty(), "nothing settled yet");
    }

    #[test]
    fn permission_queue_promotes_in_fifo_order_and_settles_every_responder() {
        let (mut q, log) = stub_queue();
        admit_stub(&mut q, &log, "a");
        admit_stub(&mut q, &log, "b");
        admit_stub(&mut q, &log, "c");

        let after_a = q.resolve("a", "allow".into());
        assert!(after_a.answered);
        assert_eq!(
            after_a.next.map(|c| c.request_id).as_deref(),
            Some("b"),
            "answering the visible card must promote the FIFO head"
        );
        let after_b = q.resolve("b", "allow".into());
        assert_eq!(after_b.next.map(|c| c.request_id).as_deref(), Some("c"));
        let after_c = q.resolve("c", "allow".into());
        assert!(after_c.answered);
        assert!(after_c.next.is_none(), "queue drained, nothing left to show");
        assert_eq!(q.showing, None);
        assert_eq!(q.waiting_len(), 0);

        // Every one of the three agent tool calls got an answer — the bug was
        // that two of them never did.
        let settled = log.lock().unwrap().clone();
        assert_eq!(
            settled,
            vec![
                ("a".to_string(), StubSettled::Selected),
                ("b".to_string(), StubSettled::Selected),
                ("c".to_string(), StubSettled::Selected),
            ]
        );
    }

    #[test]
    fn permission_queue_ignores_unknown_and_duplicate_answers() {
        let (mut q, log) = stub_queue();
        admit_stub(&mut q, &log, "a");
        admit_stub(&mut q, &log, "b");

        let unknown = q.resolve("nope", "allow".into());
        assert!(
            !unknown.answered,
            "an unknown id must not advance the queue (the caller emits nothing)"
        );
        assert_eq!(q.showing.as_deref(), Some("a"));

        assert!(q.resolve("a", "allow".into()).answered);
        // Two clients racing the same card: the second answer must not consume
        // an already-settled responder or promote a second time.
        let again = q.resolve("a", "reject".into());
        assert!(!again.answered);
        assert_eq!(q.showing.as_deref(), Some("b"));
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn permission_queue_answering_a_queued_card_removes_it_from_the_queue() {
        // Defensive path: a stale client answers a card that never reached the
        // screen. Its queue entry must go too, or promoting it later would show
        // a card whose responder is already consumed.
        let (mut q, log) = stub_queue();
        admit_stub(&mut q, &log, "a");
        admit_stub(&mut q, &log, "b");
        admit_stub(&mut q, &log, "c");

        let out = q.resolve("b", "allow".into());
        assert!(out.answered);
        assert!(
            out.next.is_none(),
            "answering a non-visible card must not change what is on screen"
        );
        assert_eq!(q.showing.as_deref(), Some("a"));
        assert_eq!(q.waiting_len(), 1);

        assert_eq!(
            q.resolve("a", "allow".into()).next.map(|c| c.request_id),
            Some("c".to_string()),
            "the dead entry must be skipped by removal, not by a promote-time check"
        );
    }

    #[test]
    fn permission_queue_drain_cancels_all_and_reports_the_visible_card() {
        let (mut q, log) = stub_queue();
        admit_stub(&mut q, &log, "a");
        admit_stub(&mut q, &log, "b");
        admit_stub(&mut q, &log, "c");

        assert_eq!(
            q.drain().as_deref(),
            Some("a"),
            "the visible card must be reported so the caller can emit a \
             compensating PermissionResolved — without it the card lingers on \
             every client with no live responder (the idle-Cancel ghost)"
        );
        assert_eq!(q.showing, None);
        assert_eq!(q.waiting_len(), 0);

        let mut settled = log.lock().unwrap().clone();
        settled.sort();
        assert_eq!(
            settled,
            vec![
                ("a".to_string(), StubSettled::Cancelled),
                ("b".to_string(), StubSettled::Cancelled),
                ("c".to_string(), StubSettled::Cancelled),
            ],
            "queued responders must be cancelled too, not leaked"
        );
    }

    #[test]
    fn permission_queue_drain_with_nothing_shown_needs_no_compensation() {
        let (mut q, _log) = stub_queue();
        assert!(
            q.drain().is_none(),
            "an empty queue must not emit a spurious PermissionResolved"
        );
    }

    #[test]
    fn permission_queue_admits_again_after_drain() {
        // The TurnComplete wedge guard: if a drain left `showing` set, every
        // later permission on this connection would queue behind a card that can
        // never be answered — reproducing the very symptom being fixed.
        let (mut q, log) = stub_queue();
        admit_stub(&mut q, &log, "a");
        admit_stub(&mut q, &log, "b");
        q.drain();

        let fresh = admit_stub(&mut q, &log, "c");
        assert_eq!(
            fresh.map(|c| c.request_id).as_deref(),
            Some("c"),
            "a post-drain request must be shown immediately"
        );
        assert_eq!(q.showing.as_deref(), Some("c"));
    }

    #[test]
    fn permission_queue_admit_then_drain_never_leaves_an_inert_card() {
        // The interleaving that motivated putting the responder map and the
        // queue under ONE lock: admit publishes a card, a Cancel drains, and the
        // card must be reported for compensation rather than left up.
        let (mut q, log) = stub_queue();
        let shown = admit_stub(&mut q, &log, "a").expect("shown");
        assert_eq!(q.drain().as_deref(), Some(shown.request_id.as_str()));
        assert_eq!(
            log.lock().unwrap().clone(),
            vec![("a".to_string(), StubSettled::Cancelled)]
        );
    }

    #[test]
    fn grok_ask_ext_request_routes_and_parses_captured_wire_shape() {
        use sacp::JsonRpcMessage;
        // Routing: the derive matches ONLY the underscore-prefixed ext method
        // (sacp routes typed handlers on the raw wire method — verified against
        // grok 0.2.101, where the missing underscore made codeg answer "unhandled"
        // and grok fall back to inert rendering).
        assert!(GrokAskUserQuestionRequest::matches_method(
            "_x.ai/ask_user_question"
        ));
        assert!(!GrokAskUserQuestionRequest::matches_method(
            "x.ai/ask_user_question"
        ));
        assert!(!GrokAskUserQuestionRequest::matches_method("session/prompt"));

        // The exact params grok sends (captured from a real 0.2.101 run): the
        // transparent newtype must deserialize them and the raw object must parse
        // into register-valid specs.
        let params = serde_json::json!({
            "sessionId": "019f70eb-32e5-7692-ae92-86fb6cb916a5",
            "toolCallId": "call-1af86ae7-ed54-440e-a983-2c5d22aa6682-0",
            "questions": [{
                "question": "What is your favorite color?",
                "options": [
                    { "label": "Red", "description": "Red" },
                    { "label": "Green", "description": "Green" },
                    { "label": "Blue", "description": "Blue" }
                ],
                "multiSelect": false
            }],
            "mode": "default"
        });
        let req: GrokAskUserQuestionRequest = serde_json::from_value(params).unwrap();
        let specs = crate::acp::question::parse_grok_ext_questions(&req.0).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].question, "What is your favorite color?");
        assert_eq!(specs[0].options.len(), 3);
        assert!(!specs[0].multi_select);
        crate::acp::question::validate_specs(&specs).unwrap();
    }

    fn diff_content(path: &str, old: Option<&str>, new: &str) -> ToolCallContent {
        let mut d = Diff::new(path, new);
        if let Some(o) = old {
            d = d.old_text(o.to_string());
        }
        ToolCallContent::Diff(d)
    }

    /// Clone a `_meta` map out of a JSON object literal, mirroring how codex-acp
    /// ships tool-call / session-info `_meta`.
    fn meta_map(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().expect("object").clone()
    }

    fn subagent_launch_input(
        agent_type: AgentType,
        meta: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Option<serde_json::Value> {
        match classify_codex_subagent_activity(agent_type, meta) {
            CodexSubagentActivity::Started(input) => {
                Some(serde_json::from_str(&input).expect("valid JSON"))
            }
            _ => None,
        }
    }

    #[test]
    fn codex_subagent_launch_becomes_an_agent_capsule_input() {
        // codex 0.147 forwards nothing but this for a native sub-agent, so it is
        // the whole live signal: name from the path's last segment, thread id as
        // the card's badge, and no prompt (the task text is encrypted).
        let sub = meta_map(serde_json::json!({
            "codex": {
                "subagent": {
                    "threadId": "01a0098a-7e8a",
                    "path": "/root/pnpm_build",
                    "activity": "started",
                }
            }
        }));
        assert_eq!(
            subagent_launch_input(AgentType::Codex, Some(&sub)),
            Some(serde_json::json!({
                "subagent_type": "pnpm_build",
                "agent_id": "01a0098a-7e8a",
                // Says the card stands for the LAUNCH, so its "completed" is
                // not read as "the sub-agent finished" — the parser writes the
                // same key on reload.
                crate::parsers::codex::CODEX_SUBAGENT_LAUNCH_KEY: true,
            }))
        );
        // A trailing slash / empty path must not produce a nameless capsule, and
        // a missing thread id just drops the badge rather than the whole card.
        let odd = meta_map(serde_json::json!({
            "codex": { "subagent": { "path": "/", "activity": "started" } }
        }));
        assert_eq!(
            subagent_launch_input(AgentType::Codex, Some(&odd)),
            Some(serde_json::json!({
                "subagent_type": "subagent",
                crate::parsers::codex::CODEX_SUBAGENT_LAUNCH_KEY: true,
            }))
        );
    }

    #[test]
    fn codex_subagent_activity_classified_only_for_codex_subagent_meta() {
        let started = meta_map(serde_json::json!({
            "codex": { "subagent": { "threadId": "t1", "path": "/root/x", "activity": "started" } }
        }));
        // Only Codex is gated — the same meta never reshapes another agent's call.
        assert!(matches!(
            classify_codex_subagent_activity(AgentType::ClaudeCode, Some(&started)),
            CodexSubagentActivity::None
        ));
        // Later lifecycle markers stay dropped: they carry no content and would
        // open a second, indistinguishable capsule for the same sub-agent.
        for kind in ["interacted", "interrupted"] {
            let other = meta_map(serde_json::json!({
                "codex": { "subagent": { "threadId": "t1", "path": "/root/x", "activity": kind } }
            }));
            assert!(matches!(
                classify_codex_subagent_activity(AgentType::Codex, Some(&other)),
                CodexSubagentActivity::Other
            ));
        }
        // Absent meta and sibling codex meta keys (goal / collaboration) are not
        // subagent activity and must render normally.
        assert!(matches!(
            classify_codex_subagent_activity(AgentType::Codex, None),
            CodexSubagentActivity::None
        ));
        let goal = meta_map(serde_json::json!({ "codex": { "goal": { "objective": "x" } } }));
        assert!(matches!(
            classify_codex_subagent_activity(AgentType::Codex, Some(&goal)),
            CodexSubagentActivity::None
        ));
        let collab = meta_map(serde_json::json!({
            "codex": { "collaboration": { "tool": "spawnAgent" } }
        }));
        assert!(matches!(
            classify_codex_subagent_activity(AgentType::Codex, Some(&collab)),
            CodexSubagentActivity::None
        ));
    }

    #[test]
    fn codex_plan_review_detected_only_for_codex_plan_review_meta() {
        // codex-acp #351: `_meta.codex.kind = "plan_review"` on the permission
        // REQUEST marks the Plan-mode review gate whose tool call was never
        // announced.
        let review = meta_map(serde_json::json!({
            "codex": { "kind": "plan_review", "planItemId": "item-7" }
        }));
        assert!(is_codex_plan_review(AgentType::Codex, Some(&review)));
        // Gated on Codex: an identical meta from another agent seeds nothing.
        assert!(!is_codex_plan_review(AgentType::ClaudeCode, Some(&review)));
        // Ordinary permission requests carry no meta at all.
        assert!(!is_codex_plan_review(AgentType::Codex, None));
        // Sibling `codex` keys and other `kind` values must not seed a card.
        let other_kind = meta_map(serde_json::json!({ "codex": { "kind": "mcp_tool_call" } }));
        assert!(!is_codex_plan_review(AgentType::Codex, Some(&other_kind)));
        let sub = meta_map(serde_json::json!({ "codex": { "subagent": { "threadId": "t1" } } }));
        assert!(!is_codex_plan_review(AgentType::Codex, Some(&sub)));
        // A non-string `kind` must not be coerced into a match.
        let numeric = meta_map(serde_json::json!({ "codex": { "kind": 1 } }));
        assert!(!is_codex_plan_review(AgentType::Codex, Some(&numeric)));
    }

    #[test]
    fn config_option_state_command_suppressed_only_for_codex_set_config_action() {
        // codex-acp #293: `/plan` is a config-option state toggle (rendered as the
        // `collaboration_mode` selector), not an invokable slash command.
        let plan = meta_map(serde_json::json!({
            "commandAction": {
                "kind": "setConfigOption",
                "configId": "collaboration_mode",
                "value": "plan",
                "resetValue": "default",
                "presentation": "state"
            }
        }));
        assert!(is_config_option_state_command(AgentType::Codex, Some(&plan)));
        // Gated on Codex — the same meta never suppresses another agent's command.
        assert!(!is_config_option_state_command(
            AgentType::ClaudeCode,
            Some(&plan)
        ));
        // `/goal` uses a `prefixPrompt` action (takes an objective argument) → a
        // real command, kept.
        let goal = meta_map(serde_json::json!({
            "commandAction": { "kind": "prefixPrompt", "presentation": "state" }
        }));
        assert!(!is_config_option_state_command(AgentType::Codex, Some(&goal)));
        // Ordinary commands (no `commandAction`) and absent meta are kept.
        assert!(!is_config_option_state_command(AgentType::Codex, None));
        let plain = meta_map(serde_json::json!({ "somethingElse": true }));
        assert!(!is_config_option_state_command(
            AgentType::Codex,
            Some(&plain)
        ));
    }

    #[test]
    fn goal_control_action_roundtrips_codex_wire_values() {
        // codex-acp #293: `_codex/session/goal_control` expects lowercase
        // "pause" / "clear" on the wire, and the same strings arrive from the
        // tauri command / HTTP endpoint — both directions must match exactly.
        assert_eq!(
            serde_json::to_value(GoalControlAction::Pause).unwrap(),
            serde_json::json!("pause")
        );
        assert_eq!(
            serde_json::to_value(GoalControlAction::Clear).unwrap(),
            serde_json::json!("clear")
        );
        assert_eq!(
            serde_json::from_value::<GoalControlAction>(serde_json::json!("pause")).unwrap(),
            GoalControlAction::Pause
        );
        assert_eq!(
            serde_json::from_value::<GoalControlAction>(serde_json::json!("clear")).unwrap(),
            GoalControlAction::Clear
        );
    }

    // --- native steering: capability synthesis + wire helpers -------------
    // (reuses the shared `meta_map` test helper defined above)

    #[test]
    fn init_advertises_steering_reads_the_top_level_meta_flag() {
        let on = meta_map(serde_json::json!({"steering": {"supported": true}}));
        assert!(init_advertises_steering(Some(&on)));

        let off = meta_map(serde_json::json!({"steering": {"supported": false}}));
        assert!(!init_advertises_steering(Some(&off)));

        // Wrong nesting (e.g. another convention's namespace) must not count.
        let nested = meta_map(
            serde_json::json!({"symposium": {"steering": {"supported": true}}}),
        );
        assert!(!init_advertises_steering(Some(&nested)));

        // Non-bool / absent → false.
        let stringly = meta_map(serde_json::json!({"steering": {"supported": "true"}}));
        assert!(!init_advertises_steering(Some(&stringly)));
        assert!(!init_advertises_steering(None));
    }

    #[test]
    fn init_advertises_goal_requires_integer_version_at_least_1() {
        // The real advertisements: codex-acp 1.2.0+ / claude-agent-acp 0.66.0+.
        let codex = meta_map(serde_json::json!({"goal": {
            "version": 1,
            "controlMethod": "_session/goal",
            "actions": ["set", "pause", "resume", "clear"],
        }}));
        assert!(init_advertises_goal(Some(&codex)));
        // Future versions must keep selecting the neutral channel.
        let v2 = meta_map(serde_json::json!({"goal": {"version": 2}}));
        assert!(init_advertises_goal(Some(&v2)));

        // Fail closed onto the legacy channel: sub-1, non-integer, stringly,
        // absent, or wrongly-shaped advertisements.
        for bad in [
            serde_json::json!({"goal": {"version": 0}}),
            serde_json::json!({"goal": {"version": 1.5}}),
            serde_json::json!({"goal": {"version": "1"}}),
            serde_json::json!({"goal": {}}),
            serde_json::json!({"goal": true}),
            serde_json::json!({"steering": {"supported": true}}),
        ] {
            let meta = meta_map(bad);
            assert!(!init_advertises_goal(Some(&meta)));
        }
        assert!(!init_advertises_goal(None));
    }

    #[test]
    fn session_info_goal_value_reads_exactly_one_channel() {
        // A transitional adapter double-publishing the same transition through
        // both namespaces (even across separate updates) must yield the goal
        // from exactly ONE channel — the one pinned at initialize.
        let both = meta_map(serde_json::json!({
            "goal": {"objective": "neutral", "status": "active"},
            "codex": {"goal": {"objective": "legacy", "status": "active"}},
        }));
        let neutral = session_info_goal_value(true, Some(&both)).expect("neutral value");
        assert_eq!(neutral.get("objective").and_then(|v| v.as_str()), Some("neutral"));
        let legacy = session_info_goal_value(false, Some(&both)).expect("legacy value");
        assert_eq!(legacy.get("objective").and_then(|v| v.as_str()), Some("legacy"));

        // Neutral-pinned connections ignore a legacy-only update (and vice
        // versa) — the two updates of a double-publish collapse to one marker.
        let legacy_only = meta_map(
            serde_json::json!({"codex": {"goal": {"objective": "legacy", "status": "active"}}}),
        );
        assert!(session_info_goal_value(true, Some(&legacy_only)).is_none());
        let neutral_only = meta_map(
            serde_json::json!({"goal": {"objective": "neutral", "status": "active"}}),
        );
        assert!(session_info_goal_value(false, Some(&neutral_only)).is_none());

        // `goal: null` IS a value (the clear signal), not an absent key.
        let cleared = meta_map(serde_json::json!({"goal": null}));
        assert!(matches!(
            session_info_goal_value(true, Some(&cleared)),
            Some(v) if v.is_null()
        ));
        assert!(session_info_goal_value(false, Some(&cleared)).is_none());
        assert!(session_info_goal_value(true, None).is_none());
    }

    #[test]
    fn goal_advertised_control_reads_method_and_actions_or_falls_back() {
        // claude 0.66+ / codex 1.2+ advertisements.
        let claude = meta_map(serde_json::json!({"goal": {
            "version": 1,
            "controlMethod": "_session/goal",
            "actions": ["set", "clear"],
        }}));
        assert_eq!(
            goal_advertised_control(Some(&claude)),
            Some(("_session/goal".to_string(), vec!["set".to_string(), "clear".to_string()]))
        );
        // Advertised-but-empty actions are honored as "no controls" — the
        // card must not offer affordances the adapter never implemented.
        let none_offered = meta_map(serde_json::json!({"goal": {
            "version": 1,
            "controlMethod": "_session/goal",
            "actions": [],
        }}));
        assert_eq!(
            goal_advertised_control(Some(&none_offered)),
            Some(("_session/goal".to_string(), Vec::new()))
        );
        // No neutral advertisement / no usable method ⇒ None (legacy
        // defaults stay in force).
        let no_goal = meta_map(serde_json::json!({"steering": {"supported": true}}));
        assert_eq!(goal_advertised_control(Some(&no_goal)), None);
        let blank_method = meta_map(serde_json::json!({"goal": {
            "version": 1,
            "controlMethod": "   ",
            "actions": ["clear"],
        }}));
        assert_eq!(goal_advertised_control(Some(&blank_method)), None);
        assert_eq!(goal_advertised_control(None), None);
    }

    #[test]
    fn the_legacy_vocabulary_is_resolved_at_initialize_not_at_construction() {
        // A fresh session knows NOTHING: a snapshot read during the handshake
        // (which happens — `spawn_agent` returns before `initialize` lands)
        // must not hand the card a legacy pair it latches forever.
        let fresh = SessionState::new(
            "c-goal".to_string(),
            AgentType::ClaudeCode,
            None,
            "w".to_string(),
            None,
        );
        assert_eq!(fresh.goal_actions, None);
        assert_eq!(fresh.to_snapshot().goal_actions, None);
        // ... and `None` reaches the client as an explicit null, not as an
        // absent field — absent is reserved for a server too old to have it,
        // which the client still maps to the legacy pair.
        let wire = serde_json::to_value(fresh.to_snapshot()).unwrap();
        assert_eq!(wire.get("goal_actions"), Some(&serde_json::Value::Null));

        // Initialize is where both cases are decided. Advertised wins…
        assert_eq!(
            resolve_goal_control(Some((
                "_session/goal".to_string(),
                vec!["set".to_string(), "clear".to_string()],
            ))),
            (
                Some("_session/goal".to_string()),
                vec!["set".to_string(), "clear".to_string()]
            )
        );
        // …and a non-advertising adapter resolves to the legacy pair, keeping
        // the method the state was built with.
        assert_eq!(
            resolve_goal_control(None),
            (None, vec!["pause".to_string(), "clear".to_string()])
        );
    }

    #[test]
    fn air_session_failure_requires_wellformed_versioned_envelope() {
        let ok = meta_map(serde_json::json!({"jetbrains": {"air": {
            "version": 1,
            "sessionFailure": {"id": "t1:error", "revision": 1},
        }}}));
        assert!(air_session_failure(Some(&ok)).is_some());

        // Missing/zero/stringly version, or a failure outside the air
        // envelope, must yield nothing.
        for bad in [
            serde_json::json!({"jetbrains": {"air": {"sessionFailure": {"id": "x", "revision": 1}}}}),
            serde_json::json!({"jetbrains": {"air": {"version": 0, "sessionFailure": {}}}}),
            serde_json::json!({"jetbrains": {"air": {"version": "1", "sessionFailure": {}}}}),
            serde_json::json!({"jetbrains": {"sessionFailure": {"id": "x", "revision": 1}}}),
            serde_json::json!({"air": {"version": 1, "sessionFailure": {}}}),
        ] {
            let meta = meta_map(bad);
            assert!(air_session_failure(Some(&meta)).is_none());
        }
        assert!(air_session_failure(None).is_none());
    }

    #[test]
    fn response_session_failure_reads_the_prompt_response_carrier() {
        // claude `failActiveWithSessionFailure` / codex
        // `terminalFailurePromptResponse` both attach a turn's terminal record
        // to the prompt response `_meta` under the SAME jetbrains.air envelope
        // the update channel uses, with a disguised `end_turn` stop reason —
        // this carrier is the only wire delivery of claude terminal failures.
        let meta = meta_map(serde_json::json!({"jetbrains": {"air": {
            "version": 1,
            "sessionFailure": {
                "id": "prompt-1:error",
                "revision": 6,
                "category": "connection",
                "severity": "error",
                "title": "The connection to Claude was lost.",
                "actions": ["new_session"],
            },
        }}}));
        let record = response_session_failure(Some(&meta)).expect("record");
        assert_eq!(record.id, "prompt-1:error");
        assert_eq!(record.revision, 6);
        assert_eq!(record.severity, "error");
        assert_eq!(record.actions, vec!["new_session".to_string()]);

        // Same gates as the update channel: malformed envelope or missing
        // identity ⇒ no record (and no synthetic-empty suppression).
        let unversioned = meta_map(serde_json::json!({"jetbrains": {"air": {
            "sessionFailure": {"id": "x", "revision": 1},
        }}}));
        assert!(response_session_failure(Some(&unversioned)).is_none());
        let no_identity = meta_map(serde_json::json!({"jetbrains": {"air": {
            "version": 1,
            "sessionFailure": {"title": "no id"},
        }}}));
        assert!(response_session_failure(Some(&no_identity)).is_none());
        assert!(response_session_failure(None).is_none());
    }

    #[test]
    fn parse_session_failure_record_requires_identity_and_stays_lenient() {
        // The real codex shape (SESSION_FAILURE_POLICY: auth_required →
        // access/[login]).
        let full = serde_json::json!({
            "id": "turn-9:error",
            "revision": 2,
            "category": "access",
            "severity": "error",
            "title": "Authentication required.",
            "details": "Token expired",
            "actions": ["login"],
        });
        let record = parse_session_failure_record(&full).expect("record");
        assert_eq!(record.id, "turn-9:error");
        assert_eq!(record.revision, 2);
        assert_eq!(record.category, "access");
        assert_eq!(record.severity, "error");
        assert_eq!(record.title, "Authentication required.");
        assert_eq!(record.details.as_deref(), Some("Token expired"));
        assert_eq!(record.actions, vec!["login".to_string()]);
        assert!(!record.resolved);

        // id + revision are HARD requirements — no identity, no merge.
        for bad in [
            serde_json::json!({"revision": 1, "title": "x"}),
            serde_json::json!({"id": "", "revision": 1}),
            serde_json::json!({"id": "   ", "revision": 1}),
            serde_json::json!({"id": "x", "title": "no revision"}),
            serde_json::json!({"id": "x", "revision": 0}),
            serde_json::json!({"id": "x", "revision": -1}),
            serde_json::json!({"id": "x", "revision": "1"}),
        ] {
            assert!(parse_session_failure_record(&bad).is_none(), "{bad:?}");
        }

        // Everything else is lenient: unknown vocabulary passes through as
        // strings, blanks default, non-string action entries are skipped.
        let sparse = serde_json::json!({
            "id": "notice-1",
            "revision": 1,
            "category": "quantum",
            "actions": ["retry", 42, "sing"],
        });
        let record = parse_session_failure_record(&sparse).expect("record");
        assert_eq!(record.category, "quantum");
        assert_eq!(record.severity, "error");
        assert_eq!(record.title, "");
        assert_eq!(record.details, None);
        assert_eq!(record.actions, vec!["retry".to_string(), "sing".to_string()]);
    }

    #[test]
    fn client_capabilities_advertise_air_for_claude_and_codex_only() {
        // Both AIR speakers must send EXACTLY the shape the adapters gate on:
        // integer version >= 1 plus "sessionFailure" in the capabilities
        // array (`clientSupportsTypedSessionFailures` in codex,
        // `supportsAirSessionFailures` in claude).
        for agent in [AgentType::ClaudeCode, AgentType::Codex] {
            let caps =
                serde_json::to_value(build_client_capabilities(agent, HostToolsPolicy::Default))
                    .unwrap();
            let air = caps
                .get("_meta")
                .and_then(|m| m.get("jetbrains"))
                .and_then(|j| j.get("air"))
                .unwrap_or_else(|| panic!("{agent:?} must advertise jetbrains.air"));
            assert_eq!(air.get("version").and_then(|v| v.as_i64()), Some(1));
            let capabilities = air
                .get("capabilities")
                .and_then(|c| c.as_array())
                .unwrap_or_else(|| panic!("{agent:?} must advertise an AIR capabilities array"));
            assert!(capabilities
                .iter()
                .any(|v| v.as_str() == Some("sessionFailure")));
            // And nothing else. Adding a capability here is not free: it is
            // what turns a per-prompt request on, and "agentFileChangeReport"
            // (claude-agent-acp 0.69.0 / codex-acp 1.4.0) buys an extra model
            // round-trip per turn for a clamped, self-reported subset of what
            // the `workspace_state` watcher already sees. See the reasoning at
            // the advertisement site before relaxing this.
            assert_eq!(
                capabilities,
                &vec![serde_json::Value::String("sessionFailure".to_string())],
                "{agent:?} must advertise ONLY sessionFailure"
            );
        }
        // Claude keeps its subagent-transcript flag alongside.
        let claude = serde_json::to_value(build_client_capabilities(
            AgentType::ClaudeCode,
            HostToolsPolicy::Default,
        ))
        .unwrap();
        assert_eq!(
            claude
                .get("_meta")
                .and_then(|m| m.get("subagent-transcript"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        // Non-AIR agents advertise nothing under jetbrains.
        for agent in [AgentType::Gemini, AgentType::Grok, AgentType::OpenCode] {
            let caps =
                serde_json::to_value(build_client_capabilities(agent, HostToolsPolicy::Default))
                    .unwrap();
            assert!(caps
                .get("_meta")
                .and_then(|m| m.get("jetbrains"))
                .is_none());
        }
    }

    #[test]
    fn version_at_least_is_strict_semver_and_fails_closed() {
        assert!(version_at_least("0.64.0", "0.64.0"));
        assert!(version_at_least("0.64.1", "0.64.0"));
        assert!(version_at_least("0.65.0", "0.64.0"));
        assert!(version_at_least("1.0.0", "0.64.0"));
        // SemVer precedence: a prerelease of the FLOOR release precedes it —
        // it may predate the commit that shipped the promptRequired
        // guarantee, so it must not open the native channel…
        assert!(!version_at_least("0.64.0-rc1", "0.64.0"));
        // …while a prerelease whose numeric core is above the floor is fine.
        assert!(version_at_least("0.64.1-beta.2", "0.64.0"));
        // Build metadata is precedence-ignored per spec.
        assert!(version_at_least("0.64.0+sha.deadbeef", "0.64.0"));
        // Below the floor.
        assert!(!version_at_least("0.63.9", "0.64.0"));
        assert!(!version_at_least("0.9.9", "0.64.0"));
        // Fail closed on anything semver can't parse.
        assert!(!version_at_least("", "0.64.0"));
        assert!(!version_at_least("beta", "0.64.0"));
        assert!(!version_at_least("v0.64.0", "0.64.0"));
        assert!(!version_at_least("0.64", "0.64.0"));
        assert!(!version_at_least("0..1", "0.64.0"));
        assert!(!version_at_least("0.64.1abc", "0.64.0"));
    }

    #[test]
    fn synthesize_native_steering_requires_all_three_gates() {
        use sacp::schema::Implementation;
        let advertised = meta_map(serde_json::json!({"steering": {"supported": true}}));
        let proven = Implementation::new("claude-agent-acp", "0.65.0");
        let stale = Implementation::new("claude-agent-acp", "0.64.1");

        // All three gates → native.
        assert!(synthesize_native_steering(
            AgentType::ClaudeCode,
            Some(&advertised),
            Some(&proven)
        ));
        // Registry policy gate: codex advertises steering but has no
        // promptRequired minimum — never native, whatever it reports.
        assert!(!synthesize_native_steering(
            AgentType::Codex,
            Some(&advertised),
            Some(&proven)
        ));
        // Runtime proof gate: 0.64.1 advertises steering identically but still
        // settles the owning prompt on a mid-generation `injected` (#934, fixed
        // in 0.65.0 by #958). Launch prefers a PATH-resolved install over the
        // pinned package, so this arm is what keeps such a user on the pull
        // channel — as does a missing `agent_info`.
        assert!(!synthesize_native_steering(
            AgentType::ClaudeCode,
            Some(&advertised),
            Some(&stale)
        ));
        assert!(!synthesize_native_steering(
            AgentType::ClaudeCode,
            Some(&advertised),
            None
        ));
        // Advertisement gate.
        assert!(!synthesize_native_steering(
            AgentType::ClaudeCode,
            None,
            Some(&proven)
        ));
    }

    #[test]
    fn build_steer_params_shape_carries_the_prompt_required_opt_in() {
        let params = build_steer_params("sess-1", "use the staging db");
        assert_eq!(params["sessionId"], "sess-1");
        assert_eq!(params["prompt"][0]["type"], "text");
        assert_eq!(params["prompt"][0]["text"], "use the staging db");
        // The opt-in is what keeps the idle race host-owned — its absence
        // would regress to detached `startedNewTurn` turns.
        assert_eq!(params["_meta"]["steering"]["idleBehavior"], "promptRequired");
    }

    #[test]
    fn parse_steer_outcome_maps_the_wire_strings_and_rejects_unknowns() {
        assert_eq!(
            parse_steer_outcome(&serde_json::json!({"outcome": "injected"})).unwrap(),
            SteerOutcome::Injected
        );
        assert_eq!(
            parse_steer_outcome(
                &serde_json::json!({"outcome": "promptRequired", "reason": "noRunningTurn"})
            )
            .unwrap(),
            SteerOutcome::PromptRequired
        );
        assert_eq!(
            parse_steer_outcome(&serde_json::json!({"outcome": "startedNewTurn"})).unwrap(),
            SteerOutcome::StartedNewTurn
        );
        // Unknown or missing outcome is a protocol error, not a silent
        // success — the caller must know whether the content was consumed.
        assert!(parse_steer_outcome(&serde_json::json!({"outcome": "queued"})).is_err());
        assert!(parse_steer_outcome(&serde_json::json!({})).is_err());
        assert!(parse_steer_outcome(&serde_json::json!({"outcome": 1})).is_err());
    }

    #[test]
    fn codex_retry_indicator_extracts_message_and_object_http_status() {
        // codex-acp #289: object-variant `codexErrorInfo` carries an inner
        // `httpStatusCode`; the message + status are surfaced.
        let m = meta_map(serde_json::json!({
            "codex": { "error": {
                "message": "Reconnecting after provider returned 401",
                "codexErrorInfo": { "responseStreamDisconnected": { "httpStatusCode": 401 } },
                "additionalDetails": "HTTP status 401",
                "turnId": "turn-id",
                "willRetry": true
            } }
        }));
        assert_eq!(
            codex_retry_indicator(Some(&m)),
            Some((
                "Reconnecting after provider returned 401".to_string(),
                Some(401)
            ))
        );
    }

    #[test]
    fn codex_retry_indicator_string_enum_yields_no_status() {
        // A bare string `codexErrorInfo` yields the message but no http status.
        let m = meta_map(serde_json::json!({
            "codex": { "error": {
                "message": "Server overloaded",
                "codexErrorInfo": "serverOverloaded",
                "willRetry": true
            } }
        }));
        assert_eq!(
            codex_retry_indicator(Some(&m)),
            Some(("Server overloaded".to_string(), None))
        );
    }

    #[test]
    fn codex_retry_indicator_refuses_terminal_empty_and_absent() {
        // `willRetry: false` (e.g. 401 auth) must never render a retry banner.
        let terminal = meta_map(serde_json::json!({
            "codex": { "error": { "message": "unauthorized", "willRetry": false } }
        }));
        assert_eq!(codex_retry_indicator(Some(&terminal)), None);
        // Blank/whitespace message → nothing to show.
        let blank = meta_map(serde_json::json!({
            "codex": { "error": { "message": "   ", "willRetry": true } }
        }));
        assert_eq!(codex_retry_indicator(Some(&blank)), None);
        // No `codex.error` at all (a goal-only or empty session_info_update).
        let goal_only = meta_map(serde_json::json!({ "codex": { "goal": null } }));
        assert_eq!(codex_retry_indicator(Some(&goal_only)), None);
        assert_eq!(codex_retry_indicator(None), None);
    }

    #[test]
    fn classify_load_failure_resource_not_found_maps_to_code() {
        assert_eq!(
            classify_session_load_failure(
                sacp::schema::ErrorCode::ResourceNotFound,
                "session abc not found",
            ),
            Some("resource_not_found"),
        );
        // The structured -32002 code takes precedence even when the message
        // would otherwise match the crash/ended family.
        assert_eq!(
            classify_session_load_failure(
                sacp::schema::ErrorCode::ResourceNotFound,
                "process exited with code 1",
            ),
            Some("resource_not_found"),
        );
    }

    #[test]
    fn classify_load_failure_crash_and_ended_map_to_unavailable() {
        // The reported Claude 0.58.1 case: native CLI exits 1, wrapped as -32603.
        assert_eq!(
            classify_session_load_failure(
                sacp::schema::ErrorCode::InternalError,
                "Internal error: { \"details\": \"Claude Code process exited with code 1\" }",
            ),
            Some("session_unavailable"),
        );
        assert_eq!(
            classify_session_load_failure(
                sacp::schema::ErrorCode::InternalError,
                "The Claude Agent session has ended. Please start a new session.",
            ),
            Some("session_unavailable"),
        );
        assert_eq!(
            classify_session_load_failure(
                sacp::schema::ErrorCode::InternalError,
                "Session not found",
            ),
            Some("session_unavailable"),
        );
    }

    #[test]
    fn classify_load_failure_keeps_existing_behavior_for_recoverable_errors() {
        // "Method not found" (agent lacks resume) and "Authentication required"
        // must fall through to the existing session/new + silent-stop paths.
        assert_eq!(
            classify_session_load_failure(
                sacp::schema::ErrorCode::MethodNotFound,
                "Method not found",
            ),
            None,
        );
        assert_eq!(
            classify_session_load_failure(
                sacp::schema::ErrorCode::AuthRequired,
                "Authentication required",
            ),
            None,
        );
        // Any other internal error without a crash/ended signature stays a
        // session/new fallback.
        assert_eq!(
            classify_session_load_failure(
                sacp::schema::ErrorCode::InternalError,
                "some unrelated transient failure",
            ),
            None,
        );
    }

    #[test]
    fn agents_codeg_records_itself_absorb_a_forgotten_session() {
        // The reported case: a custom agent keeps sessions in memory, so every
        // restart makes session/load fail with "Session not found". codeg has
        // the turns in its own transcript, so it must recover silently instead
        // of blanking the conversation behind a load-failed banner.
        let custom = AgentType::custom("glm-acp-agent").expect("valid id");
        assert!(recovers_load_failure_locally(
            custom,
            Some("session_unavailable")
        ));
        assert!(recovers_load_failure_locally(
            custom,
            Some("resource_not_found")
        ));
        // An unexpected failure is not a "forgotten session" — keep the
        // existing emit-then-fall-back behaviour even for custom agents.
        assert!(!recovers_load_failure_locally(custom, None));

        // Built-ins read history back out of the agent's own store, so a
        // forgotten session really is gone and must still stop with the banner.
        for builtin in [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::Gemini,
            AgentType::Cursor,
        ] {
            assert!(
                !recovers_load_failure_locally(builtin, Some("session_unavailable")),
                "{builtin:?} has no codeg-side transcript to fall back on"
            );
        }
    }

    #[test]
    fn cursor_env_policy_clears_inherited_creds_only_in_subscription() {
        let sub: BTreeMap<String, String> =
            [("CURSOR_AUTH_MODE".to_string(), "subscription".to_string())].into();

        // No configured creds → both injected empty (⇒ spawn strips inherited).
        let mut merged = vec![("PATH".to_string(), "/usr/bin".to_string())];
        apply_cursor_env_policy(&mut merged, &sub);
        assert!(merged.iter().any(|(k, v)| k == "CURSOR_API_KEY" && v.is_empty()));
        assert!(merged
            .iter()
            .any(|(k, v)| k == "CURSOR_API_BASE_URL" && v.is_empty()));

        // A configured key is preserved; only the absent base URL is cleared.
        let mut with_key = vec![("CURSOR_API_KEY".to_string(), "sk-x".to_string())];
        apply_cursor_env_policy(&mut with_key, &sub);
        assert!(with_key.iter().any(|(k, v)| k == "CURSOR_API_KEY" && v == "sk-x"));
        assert!(with_key
            .iter()
            .any(|(k, v)| k == "CURSOR_API_BASE_URL" && v.is_empty()));

        // Custom mode and legacy/no-mode rows are left untouched.
        for mode in [Some("custom"), None] {
            let rt: BTreeMap<String, String> = mode
                .map(|m| [("CURSOR_AUTH_MODE".to_string(), m.to_string())].into())
                .unwrap_or_default();
            let mut env = vec![("PATH".to_string(), "/usr/bin".to_string())];
            apply_cursor_env_policy(&mut env, &rt);
            assert!(!env.iter().any(|(k, _)| k == "CURSOR_API_KEY"));
            assert!(!env.iter().any(|(k, _)| k == "CURSOR_API_BASE_URL"));
        }
    }

    #[test]
    fn grok_env_policy_clears_inherited_key_only_in_subscription() {
        let sub: BTreeMap<String, String> =
            [("GROK_AUTH_MODE".to_string(), "subscription".to_string())].into();

        // Subscription with no configured key → inject empty (⇒ spawn strips the
        // inherited XAI_API_KEY so `grok login` is used).
        let mut merged = vec![("PATH".to_string(), "/usr/bin".to_string())];
        apply_grok_env_policy(&mut merged, &sub);
        assert!(merged.iter().any(|(k, v)| k == "XAI_API_KEY" && v.is_empty()));

        // A configured key is preserved even in subscription mode (explicit wins).
        let mut with_key = vec![("XAI_API_KEY".to_string(), "xai-abc".to_string())];
        apply_grok_env_policy(&mut with_key, &sub);
        assert!(with_key
            .iter()
            .any(|(k, v)| k == "XAI_API_KEY" && v == "xai-abc"));

        // api_key mode and legacy/no-mode rows are left untouched.
        for mode in [Some("api_key"), None] {
            let rt: BTreeMap<String, String> = mode
                .map(|m| [("GROK_AUTH_MODE".to_string(), m.to_string())].into())
                .unwrap_or_default();
            let mut env = vec![("PATH".to_string(), "/usr/bin".to_string())];
            apply_grok_env_policy(&mut env, &rt);
            assert!(!env.iter().any(|(k, _)| k == "XAI_API_KEY"));
        }
    }

    #[test]
    fn codex_env_policy_forces_mcp_filter_off_and_overrides_user_twin() {
        // Codex gets the flag injected so codex-acp never drops the injected
        // `codeg-mcp` server on a config.toml name collision.
        let mut env = vec![("PATH".to_string(), "/usr/bin".to_string())];
        apply_codex_env_policy(AgentType::Codex, &mut env, None);
        assert!(env
            .iter()
            .any(|(k, v)| k == "DISABLE_MCP_CONFIG_FILTERING" && v == "true"));

        // A user-supplied twin is replaced (not duplicated) so the override wins.
        let mut with_twin = vec![(
            "DISABLE_MCP_CONFIG_FILTERING".to_string(),
            "false".to_string(),
        )];
        apply_codex_env_policy(AgentType::Codex, &mut with_twin, None);
        let hits: Vec<_> = with_twin
            .iter()
            .filter(|(k, _)| k == "DISABLE_MCP_CONFIG_FILTERING")
            .collect();
        assert_eq!(hits.len(), 1, "no duplicate key");
        assert_eq!(hits[0].1, "true", "codeg override wins over user twin");
    }

    #[test]
    fn codex_env_policy_is_noop_for_other_agents() {
        for agent in [AgentType::Grok, AgentType::ClaudeCode, AgentType::Gemini] {
            let mut env = vec![("PATH".to_string(), "/usr/bin".to_string())];
            apply_codex_env_policy(agent, &mut env, Some("read-only"));
            assert!(
                !env.iter().any(|(k, _)| k == "DISABLE_MCP_CONFIG_FILTERING"),
                "{agent:?} must not receive the codex-only flag"
            );
            assert!(
                !env.iter().any(|(k, _)| k == "INITIAL_AGENT_MODE"),
                "{agent:?} must not receive codex's approval preset"
            );
        }
    }

    #[test]
    fn codex_env_policy_injects_the_config_derived_approval_preset() {
        // Without this, codex-acp seeds its default `agent` preset and re-sends
        // that approvalPolicy every turn, so the user's ~/.codex/config.toml
        // sandbox/approval choice is dead (#442).
        let mut env = vec![("PATH".to_string(), "/usr/bin".to_string())];
        apply_codex_env_policy(AgentType::Codex, &mut env, Some("read-only"));
        assert!(env
            .iter()
            .any(|(k, v)| k == "INITIAL_AGENT_MODE" && v == "read-only"));

        // Nothing mappable → stay out of the way entirely, leaving codex-acp's
        // own default rather than pinning a preset the user never chose.
        let mut none = vec![("PATH".to_string(), "/usr/bin".to_string())];
        apply_codex_env_policy(AgentType::Codex, &mut none, None);
        assert!(!none.iter().any(|(k, _)| k == "INITIAL_AGENT_MODE"));
    }

    #[test]
    fn codex_env_policy_lets_an_explicit_env_preset_win_over_the_config() {
        // An explicit runtime-env key is a stronger signal than a config-file
        // inference, so it must NOT be clobbered (and must not be duplicated).
        let mut env = vec![(
            "INITIAL_AGENT_MODE".to_string(),
            "agent-full-access".to_string(),
        )];
        apply_codex_env_policy(AgentType::Codex, &mut env, Some("read-only"));
        let hits: Vec<_> = env
            .iter()
            .filter(|(k, _)| k == "INITIAL_AGENT_MODE")
            .collect();
        assert_eq!(hits.len(), 1, "no duplicate key");
        assert_eq!(hits[0].1, "agent-full-access");

        // A blank twin carries no intent, so the config-derived value fills it.
        let mut blank = vec![("INITIAL_AGENT_MODE".to_string(), "  ".to_string())];
        apply_codex_env_policy(AgentType::Codex, &mut blank, Some("read-only"));
        let hits: Vec<_> = blank
            .iter()
            .filter(|(k, _)| k == "INITIAL_AGENT_MODE")
            .collect();
        assert_eq!(hits.len(), 1, "blank twin replaced, not duplicated");
        assert_eq!(hits[0].1, "read-only");
    }

    #[test]
    fn synthesize_edit_single_diff_makes_canonical_edit() {
        let content = vec![diff_content("/a.rs", Some("old line\n"), "new line\n")];
        let json = synthesize_edit_input_from_diffs(&content).expect("one diff -> canonical edit");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["file_path"], "/a.rs");
        assert_eq!(v["old_string"], "old line\n");
        assert_eq!(v["new_string"], "new line\n");
        // Classifies as "edit" on the frontend via old_string/new_string.
        assert!(v.get("changes").is_none());
    }

    #[test]
    fn synthesize_edit_new_file_uses_write_shape() {
        // codex-acp sends old_text=None for new files. Encode that as a write-
        // shaped input (`{file_path, content}`) so the frontend classifies it as
        // a creation (`inferFromInput` → "write" → `--- /dev/null` diff), not a
        // modification. Edit-shaped keys must be absent, or `inferFromInput`
        // would route it back to "edit".
        let content = vec![diff_content("/new.rs", None, "fn main() {}\n")];
        let json = synthesize_edit_input_from_diffs(&content).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["file_path"], "/new.rs");
        assert_eq!(v["content"], "fn main() {}\n");
        assert!(v.get("old_string").is_none());
        assert!(v.get("new_string").is_none());
    }

    #[test]
    fn build_new_file_diff_matches_frontend_write_builder() {
        // Format parity with session-files.ts's `write` diff builder: a
        // `--- /dev/null` header (so `isAddedFileDiff` fires) then every
        // `split("\n")` segment — including the trailing empty one — as a `+`
        // line, with `+1,N` counting those segments.
        assert_eq!(
            build_new_file_diff("src/x.rs", "a\nb\n"),
            "--- /dev/null\n+++ b/src/x.rs\n@@ -0,0 +1,3 @@\n+a\n+b\n+"
        );
    }

    #[test]
    fn synthesize_edit_multi_diff_makes_changes_map() {
        let content = vec![
            diff_content("/a.rs", Some("a-old"), "a-new"),
            diff_content("/b.rs", None, "b-new"),
        ];
        let json = synthesize_edit_input_from_diffs(&content).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // Object map keyed by path — the shape extractEditChangesPayload reads.
        // /a.rs is an edit → old/new text for the frontend's generateUnifiedDiff.
        assert_eq!(v["changes"]["/a.rs"]["old_text"], "a-old");
        assert_eq!(v["changes"]["/a.rs"]["new_text"], "a-new");
        // /b.rs is a new file (old_text=None) → a ready-made creation diff whose
        // `--- /dev/null` header makes `isAddedFileDiff` classify it as new;
        // it must NOT carry old_text/new_text (that path builds a `--- a/…`
        // modification diff instead).
        let b_diff = v["changes"]["/b.rs"]["diff"]
            .as_str()
            .expect("new-file entry carries a prebuilt diff");
        assert!(b_diff.contains("--- /dev/null"));
        assert!(b_diff.contains("+b-new"));
        assert!(v["changes"]["/b.rs"].get("old_text").is_none());
        assert!(v["changes"]["/b.rs"].get("new_text").is_none());
    }

    #[test]
    fn synthesize_edit_returns_none_without_diff() {
        // No Diff block -> None, so callers keep the agent's own raw_input.
        assert!(synthesize_edit_input_from_diffs(&[]).is_none());
    }

    #[test]
    fn serialize_excludes_diffs_when_hoisted_to_raw_input() {
        let content = vec![diff_content("/a.rs", Some("old"), "new")];
        // Default keeps the diff (unchanged behavior for non-hoisted content).
        assert!(serialize_tool_call_content(&content, true)
            .unwrap()
            .contains("--- /a.rs"));
        // When the edit is hoisted into raw_input, the diff is dropped so it
        // isn't shipped twice and the header stats don't read the full-file blob.
        assert!(serialize_tool_call_content(&content, false).is_none());
    }

    #[test]
    fn pi_preflight_flags_missing_custom_command() {
        let mut env = BTreeMap::new();
        env.insert(
            "PI_ACP_PI_COMMAND".to_string(),
            "/nonexistent/definitely-not-pi-xyz".to_string(),
        );
        let msg =
            pi_launch_preflight(&env).expect("an unresolvable custom pi command must be flagged");
        // Frontend invariant: routes to the localized SDK-missing install prompt.
        assert!(msg.contains("is not installed"), "got: {msg}");
        assert!(msg.contains("definitely-not-pi-xyz"), "got: {msg}");
    }

    #[test]
    fn pi_preflight_accepts_resolvable_custom_command() {
        // A binary we know exists and is executable on this platform — proves the
        // preflight clears (returns None) for a resolvable PI_ACP_PI_COMMAND.
        let existing = if cfg!(windows) {
            "C:\\Windows\\System32\\cmd.exe"
        } else {
            "/bin/sh"
        };
        let mut env = BTreeMap::new();
        env.insert("PI_ACP_PI_COMMAND".to_string(), existing.to_string());
        assert!(pi_launch_preflight(&env).is_none());
    }

    #[test]
    fn prepend_path_unix_prepends_and_keeps_single_key() {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        prepend_dir_to_path_env(&mut env, "/home/u/.local/bin", "/fallback", false);
        assert_eq!(env.get("PATH").unwrap(), "/home/u/.local/bin:/usr/bin:/bin");
        assert_eq!(env.keys().filter(|k| k.as_str() == "PATH").count(), 1);
    }

    #[test]
    fn prepend_path_unix_seeds_from_fallback_when_absent() {
        let mut env = BTreeMap::new();
        prepend_dir_to_path_env(&mut env, "/x/bin", "/usr/bin:/bin", false);
        assert_eq!(env.get("PATH").unwrap(), "/x/bin:/usr/bin:/bin");
    }

    #[test]
    fn prepend_path_windows_is_case_insensitive_and_no_clobber() {
        // Regression for the `Path` vs `PATH` clobber: a pre-existing `Path`
        // must be reused (not joined by a second `PATH` key that a later
        // case-insensitive `Command::env` could overwrite).
        let mut env = BTreeMap::new();
        env.insert("Path".to_string(), r"C:\Windows".to_string());
        prepend_dir_to_path_env(
            &mut env,
            r"C:\Users\u\AppData\Local\OfficeCLI",
            "ignored-fallback",
            true,
        );
        // Exactly one PATH-ish key, the original casing preserved, value prepended.
        let path_keys: Vec<&String> =
            env.keys().filter(|k| k.eq_ignore_ascii_case("PATH")).collect();
        assert_eq!(path_keys.len(), 1, "{env:?}");
        assert_eq!(
            env.get("Path").unwrap(),
            r"C:\Users\u\AppData\Local\OfficeCLI;C:\Windows"
        );
    }

    #[test]
    fn prepend_path_windows_seeds_from_fallback_with_semicolon() {
        let mut env = BTreeMap::new();
        prepend_dir_to_path_env(&mut env, r"C:\OfficeCLI", r"C:\Windows;C:\Windows\System32", true);
        // No prior key → default `Path` casing on Windows.
        assert_eq!(env.get("Path").unwrap(), r"C:\OfficeCLI;C:\Windows;C:\Windows\System32");
    }

    #[test]
    fn prepend_path_windows_collapses_duplicate_casings() {
        // Pathological but possible: both `PATH` and `Path` present. All
        // PATH-ish keys must collapse to exactly one, prepended onto the
        // effective (last-applied → `Path`) value, so no stale duplicate can
        // overwrite the injected dir when the child Command applies env.
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), r"C:\a".to_string());
        env.insert("Path".to_string(), r"C:\b".to_string());
        prepend_dir_to_path_env(&mut env, r"C:\OfficeCLI", "ignored-fallback", true);
        let path_keys: Vec<&String> =
            env.keys().filter(|k| k.eq_ignore_ascii_case("PATH")).collect();
        assert_eq!(path_keys.len(), 1, "exactly one PATH-ish key must remain: {env:?}");
        assert_eq!(env.get("Path").unwrap(), r"C:\OfficeCLI;C:\b");
    }

    #[test]
    fn client_capabilities_gate_per_agent() {
        // Serialize to inspect the wire shape — `_meta` is the serde rename
        // and the exact key path the adapters read.
        let caps_of = |agent: AgentType| {
            serde_json::to_value(build_client_capabilities(agent, HostToolsPolicy::Default))
                .expect("caps serialize")
        };

        // Claude Code: subagent-transcript opt-in (strict boolean true), and
        // no elicitation (which would un-gate AskUserQuestion duplication).
        let claude = caps_of(AgentType::ClaudeCode);
        assert_eq!(
            claude["_meta"]["subagent-transcript"],
            serde_json::Value::Bool(true)
        );
        assert!(claude.get("elicitation").is_none());

        // Codex: form elicitation; its `_meta` carries ONLY the AIR
        // advertisement (no subagent-transcript, which is claude's opt-in).
        let codex = caps_of(AgentType::Codex);
        assert!(codex.get("elicitation").is_some());
        assert!(codex["_meta"].get("subagent-transcript").is_none());
        assert!(codex["_meta"].get("jetbrains").is_some());

        // DeepSeek: form elicitation too — deepseek-acp routes its
        // ask_user_question + plan review through `elicitation/create` forms
        // when the bit is advertised (button fallback otherwise).
        let deepseek = caps_of(AgentType::DeepSeek);
        assert!(deepseek.get("elicitation").is_some());
        assert!(deepseek.get("_meta").is_none());

        // Everyone else: neither gate; fs + terminal always advertised.
        let other = caps_of(AgentType::Gemini);
        assert!(other.get("_meta").is_none());
        assert!(other.get("elicitation").is_none());
        assert_eq!(other["terminal"], serde_json::Value::Bool(true));
        assert_eq!(other["fs"]["readTextFile"], serde_json::Value::Bool(true));
    }

    #[test]
    fn host_tools_agent_withholds_both_execution_channels() {
        let caps_of = |agent: AgentType, host_tools: HostToolsPolicy| {
            serde_json::to_value(build_client_capabilities(agent, host_tools))
                .expect("caps serialize")
        };

        // #436: the whole point. An agent told codeg hosts neither channel
        // does its own reads and runs its own shell, so its OS sandbox — the
        // only control still working under `grok agent stdio` — covers them.
        // BOTH must go: leaving either advertised hands the agent a way back
        // into codeg's unsandboxed process for the same file.
        //
        // `ClientCapabilities` serializes its unset fields as explicit `false`
        // rather than omitting them, so assert THAT shape — not absence. The
        // two are equivalent on the wire, verified against grok 1.0.0 under a
        // kernel `deny`: sending `{fs:{readTextFile:false,…},terminal:false}`
        // produced the same outcome as omitting the keys entirely (local read →
        // `EPERM`, every shell fallback blocked, `FsViolation` audited).
        let withheld = caps_of(AgentType::Grok, HostToolsPolicy::Agent);
        assert_eq!(withheld["terminal"], serde_json::Value::Bool(false));
        assert_eq!(withheld["fs"]["readTextFile"], serde_json::Value::Bool(false));
        assert_eq!(
            withheld["fs"]["writeTextFile"],
            serde_json::Value::Bool(false)
        );

        // Default is untouched — this is opt-in, and a regression here would
        // silently break every agent's terminal.
        let hosted = caps_of(AgentType::Grok, HostToolsPolicy::Default);
        assert_eq!(hosted["terminal"], serde_json::Value::Bool(true));
        assert_eq!(hosted["fs"]["readTextFile"], serde_json::Value::Bool(true));
        assert_eq!(hosted["fs"]["writeTextFile"], serde_json::Value::Bool(true));

        // The per-agent gates are about a DIFFERENT axis (which optional
        // protocol surfaces each adapter understands) and must survive the
        // withholding — dropping codex's elicitation would strand its Plan-mode
        // `request_user_input`, and dropping claude's `_meta` would silently
        // turn subagent transcripts back off.
        let codex = caps_of(AgentType::Codex, HostToolsPolicy::Agent);
        assert!(codex.get("elicitation").is_some());
        assert_eq!(codex["terminal"], serde_json::Value::Bool(false));
        let claude = caps_of(AgentType::ClaudeCode, HostToolsPolicy::Agent);
        assert_eq!(
            claude["_meta"]["subagent-transcript"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(claude["fs"]["readTextFile"], serde_json::Value::Bool(false));
    }

    #[test]
    fn a_withheld_channel_is_refused_as_method_not_found() {
        // Every channel codeg stops advertising must also stop being SERVED.
        // Advertisement is a declaration; an agent that calls the method anyway
        // (or a future adapter that ignores client capabilities) would otherwise
        // land the operation back in codeg's unsandboxed process — the bug.
        for method in [
            "fs/read_text_file",
            "fs/write_text_file",
            "terminal/create",
            "terminal/output",
            "terminal/wait_for_exit",
            "terminal/kill",
            "terminal/release",
        ] {
            let error = unadvertised_channel_error(method);
            assert_eq!(error.code, sacp::Error::method_not_found().code);
            let text = error.to_string();
            // The knob has to be named: a bare "Method not found" on a channel
            // that worked yesterday reads as a codeg bug, not as a setting.
            assert!(text.contains(method), "{text}");
            assert!(text.contains(HOST_TOOLS_ENV), "{text}");
        }
    }

    #[test]
    fn strict_fs_policy_is_only_a_boundary_once_the_terminal_is_withheld() {
        // Pins the condition behind the connect-time warning: `strict` gates
        // reads, but that gate is walkable through a shell for as long as codeg
        // serves one. The two knobs are orthogonal — this asserts the predicate
        // pair the warning keys off, so the warning can't silently stop firing.
        let strict = FsAccessPolicy::strict(Path::new("/workspace"));
        assert!(strict.confines_reads());
        assert!(HostToolsPolicy::Default.hosts_channels());
        assert!(!HostToolsPolicy::Agent.hosts_channels());

        // The default policy leaves reads open, so there is no false promise to
        // warn about — only writes are rooted.
        let permissive =
            FsAccessPolicy::permissive(Path::new("/workspace"), AgentType::Grok, &BTreeMap::new());
        assert!(!permissive.confines_reads());
    }

    #[test]
    fn claude_raw_sdk_meta_enabled_only_for_claude() {
        let claude_meta = claude_raw_sdk_session_meta(AgentType::ClaudeCode)
            .expect("Claude must have raw SDK meta");
        assert_eq!(
            claude_meta
                .get("claudeCode")
                .and_then(|v| v.get("emitRawSDKMessages"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );

        assert!(claude_raw_sdk_session_meta(AgentType::Codex).is_none());
    }

    #[test]
    fn map_claude_sdk_ext_notification_maps_valid_payload() {
        let raw = UntypedMessage::new(
            "_claude/sdkMessage",
            serde_json::json!({
                "sessionId": "session-123",
                "message": {
                    "type": "system",
                    "subtype": "api_retry",
                    "attempt": 3,
                    "max_retries": 10
                }
            }),
        )
        .unwrap();

        let event = map_claude_sdk_ext_notification(&raw).expect("valid sdk payload should map");

        match event {
            AcpEvent::ClaudeSdkMessage {
                session_id,
                message,
            } => {
                // connection_id 不再属于 AcpEvent，envelope 上提到顶层
                assert_eq!(session_id, "session-123");
                assert_eq!(message.get("type").and_then(|v| v.as_str()), Some("system"));
            }
            _ => panic!("expected ClaudeSdkMessage"),
        }
    }

    #[test]
    fn is_known_ext_method_covers_every_mapped_method() {
        // The anti-log-storm invariant: `_claude/sdkMessage` arrives for EVERY
        // SDK message but only maps when it is an API retry, so it must be
        // recognized here — otherwise each unmapped one logs a line on a
        // per-message hot path.
        assert!(is_known_ext_method(CLAUDE_SDK_EXT_METHOD));
        for method in GROK_EXT_UPDATE_METHODS {
            assert!(is_known_ext_method(method), "{method} must be known");
        }
        // A method no mapper claims is exactly what the log is for.
        assert!(!is_known_ext_method("_vendor/somethingNew"));
        assert!(!is_known_ext_method("session/update"));
    }

    #[test]
    fn map_claude_sdk_ext_notification_rejects_non_api_retry() {
        let non_retry = UntypedMessage::new(
            "_claude/sdkMessage",
            serde_json::json!({
                "sessionId": "session-123",
                "message": {"type": "system", "subtype": "status"}
            }),
        )
        .unwrap();
        assert!(map_claude_sdk_ext_notification(&non_retry).is_none());
    }

    #[test]
    fn map_claude_sdk_ext_notification_rejects_invalid_payload() {
        let wrong_method = UntypedMessage::new(
            "_other/method",
            serde_json::json!({"sessionId": "s", "message": {}}),
        )
        .unwrap();
        assert!(map_claude_sdk_ext_notification(&wrong_method).is_none());

        let missing_fields =
            UntypedMessage::new("_claude/sdkMessage", serde_json::json!({"sessionId": 1})).unwrap();
        assert!(map_claude_sdk_ext_notification(&missing_fields).is_none());
    }

    /// The exact `_x.ai/session_notification` envelope captured from grok 0.2.111
    /// running `/compact` — `auto_compact_completed` under `params.update`, with
    /// the token delta and an `_meta.eventId`.
    #[test]
    fn map_grok_ext_notification_maps_auto_compact_completed() {
        let raw = UntypedMessage::new(
            "_x.ai/session_notification",
            serde_json::json!({
                "sessionId": "019f9475-c67f-7390-9ee5-a09d29986a6c",
                "update": {
                    "sessionUpdate": "auto_compact_completed",
                    "tokens_before": 45389,
                    "tokens_after": 16486,
                    "summary_preview": null
                },
                "_meta": {
                    "eventId": "019f9475-c67f-7390-9ee5-a09d29986a6c-4",
                    "agentTimestampMs": 1784902203750u64
                }
            }),
        )
        .unwrap();

        let event = map_grok_ext_notification(&raw, AgentType::Grok)
            .expect("auto_compact_completed should map to a compaction card");
        match event {
            AcpEvent::ToolCall {
                tool_call_id,
                status,
                meta,
                ..
            } => {
                assert_eq!(tool_call_id, "019f9475-c67f-7390-9ee5-a09d29986a6c-4");
                assert_eq!(status, "completed");
                let meta = meta.expect("compaction card needs meta");
                assert_eq!(meta.get("contextCompaction").and_then(|v| v.as_bool()), Some(true));
                assert_eq!(meta.get("tokensBefore").and_then(|v| v.as_u64()), Some(45389));
                assert_eq!(meta.get("tokensAfter").and_then(|v| v.as_u64()), Some(16486));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    /// The same variant may arrive on the sibling `_x.ai/session/update` method.
    #[test]
    fn map_grok_ext_notification_handles_session_update_method() {
        let raw = UntypedMessage::new(
            "_x.ai/session/update",
            serde_json::json!({
                "sessionId": "s",
                "update": { "sessionUpdate": "auto_compact_completed", "tokens_before": 100, "tokens_after": 100 }
            }),
        )
        .unwrap();
        // No `_meta.eventId` → a generated id, but it must still map.
        assert!(matches!(
            map_grok_ext_notification(&raw, AgentType::Grok),
            Some(AcpEvent::ToolCall { .. })
        ));
    }

    #[test]
    fn map_grok_ext_notification_image_dropped_surfaces_error() {
        let raw = UntypedMessage::new(
            "_x.ai/session_notification",
            serde_json::json!({
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "image_dropped",
                    "notes": [
                        "Image 1 was dropped before send: too small (1×1); images must be at least 8×8 pixels."
                    ]
                }
            }),
        )
        .unwrap();
        match map_grok_ext_notification(&raw, AgentType::Grok) {
            Some(AcpEvent::Error {
                message, terminal, ..
            }) => {
                assert!(
                    message.contains("too small"),
                    "error should carry grok's drop reason; got: {message}"
                );
                // grok's note already opens with "Image 1 was dropped before
                // send"; a prefix here would stutter it back at the user.
                assert!(
                    message.starts_with("Image 1 was dropped"),
                    "grok's own sentence must be shown verbatim; got: {message}"
                );
                assert!(!terminal, "a dropped image must not kill the connection");
            }
            other => panic!("expected non-terminal Error, got {other:?}"),
        }
    }

    /// Without `notes` there is no sentence to show, so the fallback has to
    /// supply the subject itself.
    #[test]
    fn map_grok_ext_notification_image_dropped_without_notes_still_names_the_subject() {
        let raw = UntypedMessage::new(
            "_x.ai/session_notification",
            serde_json::json!({
                "sessionId": "s",
                "update": { "sessionUpdate": "image_dropped", "reason": "decode failed" }
            }),
        )
        .unwrap();
        match map_grok_ext_notification(&raw, AgentType::Grok) {
            Some(AcpEvent::Error { message, .. }) => {
                assert_eq!(message, "Image dropped: decode failed");
            }
            other => panic!("expected Error, got {other:?}"),
        }

        let bare = UntypedMessage::new(
            "_x.ai/session_notification",
            serde_json::json!({
                "sessionId": "s",
                "update": { "sessionUpdate": "image_dropped", "notes": [] }
            }),
        )
        .unwrap();
        match map_grok_ext_notification(&bare, AgentType::Grok) {
            Some(AcpEvent::Error { message, .. }) => {
                assert!(
                    message.to_lowercase().contains("image"),
                    "a note-less drop must still say what happened; got: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn normalize_grok_image_blocks_lifts_decodable_image_blobs_only() {
        let blocks = vec![
            PromptInputBlock::Text {
                text: "see this".into(),
            },
            PromptInputBlock::Resource {
                uri: "clipboard://shot.png-abc".into(),
                mime_type: Some("image/png".into()),
                text: None,
                blob: Some("aGk=".into()),
            },
            PromptInputBlock::Resource {
                uri: "clipboard://notes.md".into(),
                mime_type: Some("text/markdown".into()),
                text: Some("hi".into()),
                blob: None,
            },
            PromptInputBlock::Image {
                data: "already".into(),
                mime_type: "image/jpeg".into(),
                uri: None,
            },
        ];
        let out = normalize_grok_image_blocks(blocks);
        assert!(matches!(&out[0], PromptInputBlock::Text { text } if text == "see this"));
        assert!(
            matches!(
                &out[1],
                PromptInputBlock::Image { data, mime_type, uri: Some(u) }
                    if data == "aGk="
                        && mime_type == "image/png"
                        && u == "clipboard://shot.png-abc"
            ),
            "{:?}",
            out[1]
        );
        assert!(matches!(
            &out[2],
            PromptInputBlock::Resource {
                mime_type: Some(m),
                ..
            } if m == "text/markdown"
        ));
        assert!(matches!(
            &out[3],
            PromptInputBlock::Image { data, .. } if data == "already"
        ));
    }

    /// grok's validator rejects `image/svg+xml` outright ("unsupported or
    /// unrecognised image format") and the model then sees nothing at all. As a
    /// resource blob the same file lands in the session's assets, where the
    /// model can read the source — so an undecodable mime must NOT ride the
    /// native carriage, whichever producer built the block.
    #[test]
    fn normalize_grok_image_blocks_demotes_mimes_grok_cannot_decode() {
        let blocks = vec![
            PromptInputBlock::Image {
                data: "PHN2Zy8+".into(),
                mime_type: "image/svg+xml".into(),
                uri: Some("file:///tmp/diagram.svg".into()),
            },
            // Path-less (pasted): the demotion has to invent a stable uri.
            // Upper-cased on purpose — the "is it an image at all" guard and the
            // allow-list must read the same string the same way, or an
            // undecodable format sails through as native.
            PromptInputBlock::Image {
                data: "PHN2Zy8+".into(),
                mime_type: "IMAGE/SVG+XML".into(),
                uri: None,
            },
            // Already on the resource carriage and undecodable — left alone,
            // never promoted.
            PromptInputBlock::Resource {
                uri: "clipboard://icon.svg".into(),
                mime_type: Some("image/svg+xml".into()),
                text: None,
                blob: Some("PHN2Zy8+".into()),
            },
        ];
        let out = normalize_grok_image_blocks(blocks);
        assert!(
            matches!(
                &out[0],
                PromptInputBlock::Resource { uri, mime_type: Some(m), text: None, blob: Some(b) }
                    if uri == "file:///tmp/diagram.svg"
                        && m == "image/svg+xml"
                        && b == "PHN2Zy8+"
            ),
            "{:?}",
            out[0]
        );
        assert!(
            matches!(
                &out[1],
                PromptInputBlock::Resource { uri, blob: Some(b), .. }
                    if uri == "clipboard://grok-image-1" && b == "PHN2Zy8+"
            ),
            "{:?}",
            out[1]
        );
        assert!(
            matches!(
                &out[2],
                PromptInputBlock::Resource { mime_type: Some(m), .. } if m == "image/svg+xml"
            ),
            "{:?}",
            out[2]
        );
    }

    /// The allow-list is the boundary between the two carriages, so pin it:
    /// grok's own raster set in, everything else out.
    #[test]
    fn grok_decodes_image_mime_covers_groks_raster_set_only() {
        for mime in [
            "image/png",
            "image/jpeg",
            "image/gif",
            "image/webp",
            "image/bmp",
            "image/tiff",
            "IMAGE/PNG",
        ] {
            assert!(grok_decodes_image_mime(mime), "{mime} should be decodable");
        }
        for mime in [
            "image/svg+xml",
            "image/avif",
            "image/heic",
            "image/x-icon",
            "text/markdown",
            "",
        ] {
            assert!(
                !grok_decodes_image_mime(mime),
                "{mime} must keep the resource carriage"
            );
        }
    }

    #[test]
    fn map_grok_ext_notification_auto_compact_failed_surfaces_error() {
        let raw = UntypedMessage::new(
            "_x.ai/session_notification",
            serde_json::json!({
                "sessionId": "s",
                "update": { "sessionUpdate": "auto_compact_failed", "reason": "API error (status 503)" }
            }),
        )
        .unwrap();
        match map_grok_ext_notification(&raw, AgentType::Grok) {
            Some(AcpEvent::Error { message, terminal, .. }) => {
                assert!(message.contains("503"), "error should carry the reason; got: {message}");
                assert!(!terminal, "compaction failure must not kill the connection");
            }
            other => panic!("expected non-terminal Error, got {other:?}"),
        }
    }

    #[test]
    fn map_grok_ext_notification_is_grok_gated_and_scoped() {
        let compact = serde_json::json!({
            "sessionId": "s",
            "update": { "sessionUpdate": "auto_compact_completed", "tokens_before": 1, "tokens_after": 1 }
        });
        // Non-grok agent: ignored even for the same payload.
        let raw = UntypedMessage::new("_x.ai/session_notification", compact.clone()).unwrap();
        assert!(map_grok_ext_notification(&raw, AgentType::Codex).is_none());

        // Turn-level state is intentionally left to the prompt-response path.
        let turn = UntypedMessage::new(
            "_x.ai/session_notification",
            serde_json::json!({
                "sessionId": "s",
                "update": { "sessionUpdate": "turn_completed", "stop_reason": "error", "agent_result": "boom" }
            }),
        )
        .unwrap();
        assert!(map_grok_ext_notification(&turn, AgentType::Grok).is_none());

        // Unrelated method: ignored.
        let other = UntypedMessage::new("session/update", compact).unwrap();
        assert!(map_grok_ext_notification(&other, AgentType::Grok).is_none());
    }

    #[test]
    fn track_grok_spawn_call_pairs_dedupes_and_drops_failed() {
        let mut cb = CodeBuddyLiveState::default();
        // Announce (pending) — re-announce on a later frame must not re-queue.
        track_grok_spawn_call(&mut cb, true, Some("pending"), "call-1", &None);
        track_grok_spawn_call(&mut cb, true, None, "call-1", &None);
        assert_eq!(cb.grok_pending_spawn_ids.len(), 1);
        // A status-only update WITHOUT the meta marker still tracks a seen id.
        track_grok_spawn_call(&mut cb, false, Some("completed"), "call-1", &None);
        assert!(cb.grok_settled_spawn_ids.contains("call-1"));
        // A failed spawn leaves the pairing queue so later pairs can't shift.
        track_grok_spawn_call(&mut cb, true, Some("pending"), "call-2", &None);
        track_grok_spawn_call(&mut cb, false, Some("failed"), "call-2", &None);
        assert!(!cb
            .grok_pending_spawn_ids
            .iter()
            .any(|p| p.call_id == "call-2"));
        // A never-seen id (another tool) is entirely ignored.
        track_grok_spawn_call(&mut cb, false, Some("completed"), "call-x", &None);
        assert!(!cb.grok_settled_spawn_ids.contains("call-x"));
    }

    /// A DELAYED prior-turn `subagent_spawned` (its launch turn ended, its
    /// pending entry was cleared) must NOT steal the pairing slot a NEW turn's
    /// spawn queued: the `(description, subagent_type)` captured from the
    /// launch input has to match the notification's own fields.
    #[test]
    fn delayed_subagent_spawned_does_not_steal_a_new_turns_slot() {
        let mut cb = CodeBuddyLiveState::default();
        let input_b = Some(
            serde_json::json!({
                "description": "B task", "prompt": "PB", "subagent_type": "plan"
            })
            .to_string(),
        );
        track_grok_spawn_call(&mut cb, true, Some("pending"), "call-b", &input_b);

        // The prior turn's child announces late, with ITS OWN description.
        let stale = grok_subagent_notif(serde_json::json!({
            "sessionUpdate": "subagent_spawned",
            "subagent_id": "sub-a",
            "description": "A task",
            "subagent_type": "explore"
        }));
        assert!(
            map_grok_subagent_notification(&stale, AgentType::Grok, true, &mut cb).is_empty(),
            "a mismatched spawned must pair nothing"
        );
        assert_eq!(cb.grok_pending_spawn_ids.len(), 1, "B keeps its slot");

        // A delayed notification that OMITS the description (same type or
        // none at all) must not wildcard past B's captured description either.
        let stale_bare = grok_subagent_notif(serde_json::json!({
            "sessionUpdate": "subagent_spawned",
            "subagent_id": "sub-a2",
            "subagent_type": "plan"
        }));
        assert!(
            map_grok_subagent_notification(&stale_bare, AgentType::Grok, true, &mut cb)
                .is_empty(),
            "an event missing a captured field must not match"
        );
        assert_eq!(cb.grok_pending_spawn_ids.len(), 1, "B still keeps its slot");

        // B's own notification pairs it.
        let real = grok_subagent_notif(serde_json::json!({
            "sessionUpdate": "subagent_spawned",
            "subagent_id": "sub-b",
            "description": "B task",
            "subagent_type": "plan"
        }));
        map_grok_subagent_notification(&real, AgentType::Grok, true, &mut cb);
        assert_eq!(
            cb.grok_subagent_to_call.get("sub-b").map(String::as_str),
            Some("call-b")
        );
        assert!(cb.grok_pending_spawn_ids.is_empty());
    }

    fn grok_subagent_notif(update: serde_json::Value) -> UntypedMessage {
        UntypedMessage::new(
            "_x.ai/session/update",
            serde_json::json!({ "sessionId": "sess-1", "update": update }),
        )
        .unwrap()
    }

    /// Full background-subagent lifecycle over the stateful mapper: spawned
    /// pairs FIFO, progress lands as a meta-only ToolCallUpdate on the paired
    /// call, and finished settles over the BackgroundActivity channel with the
    /// launch call's id (so the frontend flips its marker card in-memory).
    #[test]
    fn map_grok_subagent_notification_background_lifecycle() {
        let mut cb = CodeBuddyLiveState::default();
        track_grok_spawn_call(&mut cb, true, Some("pending"), "call-1", &None);
        // Background: the launch call settles before the pairing arrives.
        track_grok_spawn_call(&mut cb, true, Some("completed"), "call-1", &None);

        let spawned = grok_subagent_notif(serde_json::json!({
            "sessionUpdate": "subagent_spawned",
            "subagent_id": "sub-1",
            "child_session_id": "sub-1",
            "subagent_type": "explore"
        }));
        // Pairing consumed the pending slot; the already-settled call means a
        // background child is now outstanding (idle-sweep exemption). Passed
        // with turn_active=false: the BackgroundActivity channel is
        // out-of-turn-safe by design and must not be gated.
        match map_grok_subagent_notification(&spawned, AgentType::Grok, false, &mut cb).as_slice() {
            // Out of turn there is no session stamp (it would resurrect a ghost
            // live_message) — only the background report.
            [AcpEvent::BackgroundActivity {
                outstanding,
                settled,
                ..
            }] => {
                assert_eq!(*outstanding, 1);
                assert!(settled.is_empty());
            }
            other => panic!("expected one BackgroundActivity, got {other:?}"),
        }
        assert!(cb.grok_pending_spawn_ids.is_empty());
        // The child's session id is remembered for the card, whether or not it
        // was emitted this time.
        assert_eq!(
            cb.grok_call_child_session.get("call-1").map(String::as_str),
            Some("sub-1")
        );

        let progress = grok_subagent_notif(serde_json::json!({
            "sessionUpdate": "subagent_progress",
            "subagent_id": "sub-1",
            "duration_ms": 4200,
            "turn_count": 1,
            "tool_call_count": 7,
            "context_usage_pct": 12.5,
            "tools_used": ["read_file"]
        }));
        // Out-of-turn tick: dropped whole. A post-TurnComplete ToolCallUpdate
        // would resurrect a ghost `live_message` in the SessionState snapshot
        // (the apply arm lazily creates it), while the frontend diverts it out
        // of the transcript anyway.
        assert!(
            map_grok_subagent_notification(&progress, AgentType::Grok, false, &mut cb).is_empty(),
            "progress must not emit outside an active turn"
        );
        // Cross-turn tick: the launch turn ended and a LATER turn is Prompting
        // (turn_active=true) — the per-turn eligibility clear must still drop
        // it, or the old tool id would be appended into the NEW turn's live
        // message as a ghost card.
        let eligibility_backup = cb.grok_progress_eligible.clone();
        cb.grok_progress_eligible.clear();
        assert!(
            map_grok_subagent_notification(&progress, AgentType::Grok, true, &mut cb).is_empty(),
            "a prior turn's background child must not tick into a later turn"
        );
        cb.grok_progress_eligible = eligibility_backup;
        match map_grok_subagent_notification(&progress, AgentType::Grok, true, &mut cb).as_slice() {
            [AcpEvent::ToolCallUpdate {
                tool_call_id,
                status,
                meta,
                ..
            }] => {
                assert_eq!(tool_call_id, "call-1");
                assert_eq!(*status, None, "progress must not touch the status");
                let progress = meta
                    .as_ref()
                    .and_then(|m| m.get("grokSubagentProgress"))
                    .expect("progress meta");
                assert_eq!(progress.get("toolCallCount").and_then(|v| v.as_u64()), Some(7));
                assert_eq!(progress.get("durationMs").and_then(|v| v.as_u64()), Some(4200));
                assert_eq!(
                    progress.get("contextUsagePct").and_then(|v| v.as_f64()),
                    Some(12.5)
                );
                // Meta is REPLACED on apply, so every tick must re-send the
                // session ids or the "open child session" affordance vanishes
                // the moment the first progress tick lands.
                assert_eq!(
                    meta.as_ref()
                        .and_then(|m| m.get("grokSubagentSession"))
                        .and_then(|s| s.get("childSessionId"))
                        .and_then(|v| v.as_str()),
                    Some("sub-1")
                );
            }
            other => panic!("expected one ToolCallUpdate, got {other:?}"),
        }

        let finished = grok_subagent_notif(serde_json::json!({
            "sessionUpdate": "subagent_finished",
            "subagent_id": "sub-1",
            "child_session_id": "sub-1",
            "status": "completed",
            "tool_calls": 7,
            "duration_ms": 63775,
            "output": "## Findings"
        }));
        // The settle is likewise out-of-turn-safe (a background child usually
        // finishes after its launch turn ended).
        match map_grok_subagent_notification(&finished, AgentType::Grok, false, &mut cb).as_slice() {
            [AcpEvent::BackgroundActivity {
                session_id,
                outstanding,
                settled,
                ..
            }] => {
                assert_eq!(session_id, "sess-1");
                assert_eq!(*outstanding, 0, "the settled child leaves the count");
                assert_eq!(settled.len(), 1);
                let s = &settled[0];
                assert_eq!(s.task_id, "sub-1");
                assert_eq!(s.status, "completed");
                assert_eq!(s.tool_use_id.as_deref(), Some("call-1"));
                assert_eq!(s.result.as_deref(), Some("## Findings"));
                assert!(s.wire_visible, "settle flips the card in-memory — no syncing hint");
            }
            other => panic!("expected BackgroundActivity, got {other:?}"),
        }
        // Lifecycle over: a duplicate finished no longer routes anywhere.
        assert!(map_grok_subagent_notification(&finished, AgentType::Grok, false, &mut cb)
            .is_empty());
    }

    /// A BLOCKING spawn (call not yet settled when the child finishes) must NOT
    /// emit a settle — its own completion frame carries the output; a duplicate
    /// marker would double-render it. Non-grok agents never route at all.
    #[test]
    fn map_grok_subagent_notification_skips_blocking_and_other_agents() {
        let mut cb = CodeBuddyLiveState::default();
        track_grok_spawn_call(&mut cb, true, Some("pending"), "call-1", &None);
        let spawned = grok_subagent_notif(serde_json::json!({
            "sessionUpdate": "subagent_spawned",
            "subagent_id": "sub-1"
        }));
        // Blocking: the call is still in_progress at pairing time, so there is
        // no background report — only the session stamp, which is exactly what
        // lets the user open a blocking child's transcript while it runs (its
        // launch call stays output-less until the very end).
        match map_grok_subagent_notification(&spawned, AgentType::Grok, true, &mut cb).as_slice() {
            [AcpEvent::ToolCallUpdate {
                tool_call_id, meta, ..
            }] => {
                assert_eq!(tool_call_id, "call-1");
                // No `child_session_id` on this wire shape → fall back to the
                // subagent id, which is the same value in every capture.
                assert_eq!(
                    meta.as_ref()
                        .and_then(|m| m.get("grokSubagentSession"))
                        .and_then(|s| s.get("childSessionId"))
                        .and_then(|v| v.as_str()),
                    Some("sub-1")
                );
            }
            other => panic!("expected one ToolCallUpdate, got {other:?}"),
        }
        let finished = grok_subagent_notif(serde_json::json!({
            "sessionUpdate": "subagent_finished",
            "subagent_id": "sub-1",
            "status": "completed",
            "output": "done"
        }));
        assert!(
            map_grok_subagent_notification(&finished, AgentType::Grok, true, &mut cb).is_empty(),
            "blocking spawn settles via its own completion frame"
        );

        // Gating: same payload from a non-grok agent is untouched (and state
        // untouched — the pending queue keeps its entry).
        let mut cb2 = CodeBuddyLiveState::default();
        track_grok_spawn_call(&mut cb2, true, Some("pending"), "call-9", &None);
        let spawned2 = grok_subagent_notif(serde_json::json!({
            "sessionUpdate": "subagent_spawned",
            "subagent_id": "sub-9"
        }));
        assert!(
            map_grok_subagent_notification(&spawned2, AgentType::Codex, true, &mut cb2).is_empty()
        );
        assert_eq!(cb2.grok_pending_spawn_ids.len(), 1);
    }

    /// A traversal-shaped `child_session_id` must not reach the card: the
    /// frontend hands that value to `get_conversation`, which resolves a session
    /// directory by it, so the live path applies the same `is_safe_subagent_id`
    /// gate the history parser (`grok::subagent_stats`) does. The stamp still
    /// carries the subagent id — the run is identified, there is just no session
    /// offered to open.
    #[test]
    fn map_grok_subagent_notification_rejects_unsafe_child_session_id() {
        let mut cb = CodeBuddyLiveState::default();
        track_grok_spawn_call(&mut cb, true, Some("pending"), "call-1", &None);
        let spawned = grok_subagent_notif(serde_json::json!({
            "sessionUpdate": "subagent_spawned",
            "subagent_id": "sub-1",
            "child_session_id": "../../../etc/passwd"
        }));
        match map_grok_subagent_notification(&spawned, AgentType::Grok, true, &mut cb).as_slice() {
            [AcpEvent::ToolCallUpdate { meta, .. }] => {
                let session = meta
                    .as_ref()
                    .and_then(|m| m.get("grokSubagentSession"))
                    .expect("session meta");
                assert_eq!(
                    session.get("subagentId").and_then(|v| v.as_str()),
                    Some("sub-1")
                );
                assert!(
                    session.get("childSessionId").is_none(),
                    "a traversal-shaped child id must not reach the viewer"
                );
            }
            other => panic!("expected one ToolCallUpdate, got {other:?}"),
        }
        // Nothing memorized either — a later progress tick has nothing to
        // re-send, so the rejection holds for the whole run.
        assert!(cb.grok_call_child_session.is_empty());
    }

    /// The turn-loop consults this to keep a compaction-only `/compact` turn
    /// from being reclassified as `"empty"` (which re-surfaces a spurious error).
    /// It must count exactly the compaction outcomes that emit a card/error.
    #[test]
    fn grok_ext_notification_is_turn_output_marks_compaction_outcomes() {
        let notif = |variant: &str| {
            Dispatch::Notification(
                UntypedMessage::new(
                    "_x.ai/session_notification",
                    serde_json::json!({
                        "sessionId": "s",
                        "update": {
                            "sessionUpdate": variant,
                            "tokens_before": 9, "tokens_after": 8, "reason": "x"
                        }
                    }),
                )
                .unwrap(),
            )
        };
        // Both compaction outcomes are visible turn output.
        assert!(grok_ext_notification_is_turn_output(&notif("auto_compact_completed"), AgentType::Grok));
        assert!(grok_ext_notification_is_turn_output(&notif("auto_compact_failed"), AgentType::Grok));
        // turn_completed is deliberately left to the prompt-response path — it is
        // NOT counted here (otherwise a genuinely empty turn would be masked).
        assert!(!grok_ext_notification_is_turn_output(&notif("turn_completed"), AgentType::Grok));
        // Never fires for a non-grok agent.
        assert!(!grok_ext_notification_is_turn_output(
            &notif("auto_compact_completed"),
            AgentType::Codex
        ));
    }

    /// The `session/load` replay drains a PAST session, so anything that would
    /// raise an alert (status-bar entry + OS notification) has to be recognised
    /// and skipped there — otherwise opening an old conversation reports its
    /// historical failures as if they were happening now.
    #[test]
    fn grok_ext_notification_is_alert_matches_only_the_error_outcomes() {
        let notif = |variant: &str| {
            Dispatch::Notification(
                UntypedMessage::new(
                    "_x.ai/session_notification",
                    serde_json::json!({
                        "sessionId": "s",
                        "update": {
                            "sessionUpdate": variant,
                            "tokens_before": 9, "tokens_after": 8, "reason": "x",
                            "notes": ["Image 1 was dropped before send: too small."]
                        }
                    }),
                )
                .unwrap(),
            )
        };
        // Both map to a non-terminal Error, so both alert.
        assert!(grok_ext_notification_is_alert(
            &notif("image_dropped"),
            AgentType::Grok
        ));
        assert!(grok_ext_notification_is_alert(
            &notif("auto_compact_failed"),
            AgentType::Grok
        ));
        // A successful compaction renders a CARD, not an alert — it stays
        // replayable, so the loaded transcript still shows what happened.
        assert!(!grok_ext_notification_is_alert(
            &notif("auto_compact_completed"),
            AgentType::Grok
        ));
        // Unmapped variants and non-grok agents never alert.
        assert!(!grok_ext_notification_is_alert(
            &notif("turn_completed"),
            AgentType::Grok
        ));
        assert!(!grok_ext_notification_is_alert(
            &notif("image_dropped"),
            AgentType::Codex
        ));
    }

    /// Grok's cumulative token count rides the OUTER `params._meta` of ordinary
    /// updates — the only live context signal it offers, and one the typed
    /// pipeline drops. The peek must read it there, hold its fire while the
    /// number repeats, and stay silent without a window to divide by.
    #[test]
    fn grok_live_usage_step_reads_the_outer_meta_and_dedupes() {
        let update = |meta: serde_json::Value| {
            Dispatch::Notification(
                UntypedMessage::new(
                    "session/update",
                    serde_json::json!({
                        "sessionId": "s",
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": {"type": "text", "text": "hi"}
                        },
                        "_meta": meta,
                    }),
                )
                .unwrap(),
            )
        };
        let streaming = update(serde_json::json!({"totalTokens": 4200, "agentTimestampMs": 3}));

        assert_eq!(
            grok_live_usage_step(&streaming, AgentType::Grok, Some(500_000), None),
            Some((4200, 500_000))
        );
        // Same count again → nothing to say (the field rides nearly every chunk).
        assert_eq!(
            grok_live_usage_step(&streaming, AgentType::Grok, Some(500_000), Some((4200, 500_000))),
            None
        );
        // A different prior value is a real step → emit.
        assert_eq!(
            grok_live_usage_step(&streaming, AgentType::Grok, Some(500_000), Some((3000, 500_000))),
            Some((4200, 500_000))
        );
        // Same count but a NEW window (the user switched model between turns) →
        // re-emit, or the ring would keep dividing by the old model's window.
        assert_eq!(
            grok_live_usage_step(&streaming, AgentType::Grok, Some(256_000), Some((4200, 500_000))),
            Some((4200, 256_000))
        );
        // No resolvable window → still report the count, with the frontend's
        // "unknown window" sentinel as the size, so the ring falls back to the
        // parsed session stats instead of freezing on a previous model's window.
        assert_eq!(
            grok_live_usage_step(&streaming, AgentType::Grok, None, None),
            Some((4200, 0))
        );
        assert_eq!(
            grok_live_usage_step(&streaming, AgentType::Grok, None, Some((4200, 0))),
            None
        );
        // `_meta` without the count, and a zero count, are both "no data".
        assert_eq!(
            grok_live_usage_step(
                &update(serde_json::json!({"agentTimestampMs": 3})),
                AgentType::Grok,
                Some(500_000),
                None
            ),
            None
        );
        assert_eq!(
            grok_live_usage_step(
                &update(serde_json::json!({"totalTokens": 0})),
                AgentType::Grok,
                Some(500_000),
                None
            ),
            None
        );
        // Never fires for another agent — they have a real `usage_update`.
        assert_eq!(
            grok_live_usage_step(&streaming, AgentType::ClaudeCode, Some(500_000), None),
            None
        );
    }

    /// A model switch between turns moves the denominator, and the frontend's
    /// reducer drops a `used == 0` update while it holds a positive one — so the
    /// previous model's window has to be re-keyed at turn start, not cleared.
    #[test]
    fn grok_window_change_usage_rekeys_a_live_pair_to_the_new_window() {
        // Same window (no switch) → nothing to do.
        assert_eq!(
            grok_window_change_usage(Some(500_000), Some((4200, 500_000))),
            None
        );
        // Switched to a smaller-window model → carry the count, swap the window.
        assert_eq!(
            grok_window_change_usage(Some(256_000), Some((4200, 500_000))),
            Some((4200, 256_000))
        );
        // Nothing emitted yet → nothing to re-key (the first real count will).
        assert_eq!(grok_window_change_usage(Some(256_000), None), None);
        // Switched to a BYO model nothing can size → relinquish the denominator
        // with the "unknown window" sentinel. This is the case that would
        // otherwise strand the previous model's window forever: no later update
        // can correct it, and a zero-`used` clear would be dropped by the
        // frontend reducer.
        assert_eq!(
            grok_window_change_usage(None, Some((4200, 500_000))),
            Some((4200, 0))
        );
        // …and once relinquished, it stays relinquished (no event storm).
        assert_eq!(grok_window_change_usage(None, Some((4200, 0))), None);
        // Switching BACK to a sized model re-keys off the sentinel.
        assert_eq!(
            grok_window_change_usage(Some(500_000), Some((4200, 0))),
            Some((4200, 500_000))
        );
    }

    /// The offline half of the live resolver: Grok's own on-disk catalog, then
    /// the id heuristic. A BYO endpoint is keyed by whatever id the user typed,
    /// so this genuinely can come back empty — `grok_window_change_usage` is
    /// what keeps that from stranding a stale window.
    #[test]
    fn grok_offline_context_window_falls_through_catalog_then_heuristic() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("models_cache.json"),
            r#"{"models":{"grok-4.5":{"info":{"context_window":500000}},
                          "deepseek-chat":{"info":{"context_window":131072}}}}"#,
        )
        .expect("write catalog");
        std::fs::write(
            home.path().join("config.toml"),
            "[model.\"my-byo\"]\nmodel = \"my-byo\"\ncontext_window = 64000\n",
        )
        .expect("write config");

        // Catalog wins, including for a non-`grok` id.
        assert_eq!(
            grok_offline_context_window("deepseek-chat", home.path()),
            Some(131_072)
        );
        // BYO block in config.toml is the second source.
        assert_eq!(
            grok_offline_context_window("my-byo", home.path()),
            Some(64_000)
        );
        // Neither file names it, but the id is Grok-shaped → heuristic answers.
        assert_eq!(
            grok_offline_context_window("grok-9-unreleased", home.path()),
            Some(256_000)
        );
        // Neither file names it AND the id belongs to no known family — the BYO
        // case Codex flagged. `None` here is load-bearing, not a gap.
        assert_eq!(
            grok_offline_context_window("some-local-llama", home.path()),
            None
        );
    }

    // ---- empty-turn diagnosis ----

    fn drop_err(msg: &str) -> String {
        msg.to_string()
    }

    /// The read loop drops at most one update per notification, so an agent
    /// speaking a protocol codeg can't decode used to put a WARN on the
    /// per-streaming-chunk path — under the DEFAULT level, which is how a field
    /// report ended up writing 34GB in 8.8h (issue #427). The throttle collapses
    /// the burst; this pins the part that must survive it, the tally.
    #[test]
    fn dropped_update_log_line_reports_what_the_throttle_swallowed() {
        // Leading edge: the plain line, no tally to report yet.
        let first = dropped_update_log_line("decode", &drop_err("bad json"), 1);
        assert_eq!(
            first,
            "[ACP] Ignoring unreadable session update (decode): bad json"
        );
        // A coalesced line names how many were suppressed and over what window,
        // so the reader can tell "happened once" from "happening constantly".
        let coalesced = dropped_update_log_line("dispatch", &drop_err("missing field"), 4213);
        assert!(
            coalesced.starts_with(
                "[ACP] Ignoring unreadable session update (dispatch): missing field"
            ),
            "{coalesced}"
        );
        assert!(coalesced.contains("+4212 more"), "{coalesced}");
        assert!(
            coalesced.contains(&format!("{}s", DROPPED_UPDATE_LOG_WINDOW.as_secs())),
            "{coalesced}"
        );
    }

    #[test]
    fn dropped_update_logging_emits_once_per_window() {
        let mut throttle = LeadingEdgeThrottle::new(DROPPED_UPDATE_LOG_WINDOW);
        let now = std::time::Instant::now();
        // First hit surfaces instantly...
        assert!(throttle.record_at(now, 1).is_some());
        // ...and a chunk-rate burst inside the window emits nothing at all.
        for i in 1..10_000u64 {
            assert!(
                throttle
                    .record_at(now + std::time::Duration::from_micros(i * 100), 1)
                    .is_none(),
                "burst hit {i} must be suppressed"
            );
        }
        // Past the window, one line carries the whole suppressed tally.
        let summary = throttle
            .record_at(now + DROPPED_UPDATE_LOG_WINDOW, 1)
            .expect("post-window hit emits");
        assert_eq!(summary.occurrences, 10_000);
    }

    #[test]
    fn note_update_splits_agent_output_from_metadata() {
        use sacp::schema::{ContentChunk, Plan};

        let mut probe = TurnOutputProbe::new(0);
        probe.note_update(&SessionUpdate::Plan(Plan::new(Vec::new())));
        assert!(!probe.saw_agent_output, "Plan is not agent output");
        assert!(probe.saw_metadata_update);

        probe.note_update(&SessionUpdate::AgentMessageChunk(ContentChunk::new(
            "hi".into(),
        )));
        assert!(probe.saw_agent_output);
    }

    /// The classifier's whole purpose is to separate "we went blind" from
    /// "nothing came" — a `Plan`-only turn must not read as agent output.
    #[test]
    fn diagnose_empty_turn_covers_three_causes_and_priority() {
        let mut none = TurnOutputProbe::new(0);
        assert_eq!(diagnose_empty_turn(&none), EmptyTurnCause::NoOutput);

        none.saw_metadata_update = true;
        assert_eq!(diagnose_empty_turn(&none), EmptyTurnCause::MetadataOnly);

        // Dropped updates outrank metadata: not being able to read the output
        // is the stronger signal and points somewhere else entirely.
        none.note_dropped(DropSite::Decode, &drop_err("boom"));
        assert_eq!(diagnose_empty_turn(&none), EmptyTurnCause::ProtocolMismatch);

        let mut dispatch_only = TurnOutputProbe::new(0);
        dispatch_only.note_dropped(DropSite::Dispatch, &drop_err("boom"));
        assert_eq!(
            diagnose_empty_turn(&dispatch_only),
            EmptyTurnCause::ProtocolMismatch
        );
    }

    #[test]
    fn note_dropped_counts_each_site_separately_and_keeps_the_first() {
        let mut probe = TurnOutputProbe::new(0);
        probe.note_dropped(DropSite::Dispatch, &drop_err("missing field `sessionUpdate`"));
        probe.note_dropped(DropSite::Decode, &drop_err("missing field `update`"));
        probe.note_dropped(DropSite::Decode, &drop_err("missing field `content`"));

        assert_eq!(probe.dropped_decode, 2);
        assert_eq!(probe.dropped_dispatch, 1);
        assert_eq!(probe.dropped_total(), 3);
        let (site, summary) = probe.first_drop.as_ref().expect("first drop recorded");
        assert_eq!(*site, DropSite::Dispatch, "first wins, not last");
        assert_eq!(summary, "missing field `sessionUpdate`");
    }

    /// Drop reasons reach the UI, so they must be redacted at capture time —
    /// a parser error inlines the value it choked on, and that value came off
    /// the `session/update` channel.
    #[test]
    fn note_dropped_redacts_the_captured_error() {
        const SECRET: &str = "sk-live-abcdefghijklmnop";
        let mut probe = TurnOutputProbe::new(0);
        probe.note_dropped(
            DropSite::Dispatch,
            &drop_err(&format!(
                r#"invalid type: string "{SECRET}", expected u64 at line 1 column 40"#
            )),
        );
        let (_, summary) = probe.first_drop.as_ref().unwrap();
        assert!(!summary.contains(SECRET), "leaked: {summary}");
        assert_eq!(summary, "invalid type, expected u64 at line 1 column 40");
    }

    #[test]
    fn finish_turn_reason_passes_through_non_empty_reasons() {
        let tail = StderrTail::new();
        let mut probe = TurnOutputProbe::new(0);
        probe.saw_agent_output = true;

        for reason in ["end_turn", "cancelled", "refusal", "max_tokens", "unknown"] {
            let (out, report) = finish_turn_reason(&probe, reason, &tail);
            assert_eq!(out, reason);
            assert!(report.is_none());
        }

        // Without agent output, only `end_turn` is rewritten.
        let silent = TurnOutputProbe::new(0);
        assert_eq!(finish_turn_reason(&silent, "cancelled", &tail).0, "cancelled");
        assert_eq!(finish_turn_reason(&silent, "end_turn", &tail).0, "empty");
    }

    /// Guards the two-exit refactor: the helper only computes, so calling it
    /// twice (as the two exits each do) is identical and side-effect free.
    #[test]
    fn finish_turn_reason_is_pure() {
        let tail = StderrTail::new();
        tail.push("boom");
        let probe = TurnOutputProbe::new(0);

        let (first_reason, first) = finish_turn_reason(&probe, "end_turn", &tail);
        let (second_reason, second) = finish_turn_reason(&probe, "end_turn", &tail);
        assert_eq!(first_reason, second_reason);
        assert_eq!(
            first.as_ref().map(|r| r.cause),
            second.as_ref().map(|r| r.cause)
        );
        assert_eq!(
            first.as_ref().and_then(|r| r.details.clone()),
            second.as_ref().and_then(|r| r.details.clone())
        );
    }

    #[test]
    fn empty_turn_details_quote_stderr_scoped_to_the_turn() {
        let tail = StderrTail::new();
        tail.push("older line from a previous turn");
        let probe = TurnOutputProbe::new(tail.mark());
        tail.push("Error: 401 Unauthorized");

        let details = build_empty_turn_details(&probe, &tail).expect("details");
        assert!(details.contains("stderr (this turn"), "{details}");
        assert!(details.contains("Error: 401 Unauthorized"));
        assert!(!details.contains("older line"));
    }

    #[test]
    fn empty_turn_details_fall_back_to_recent_stderr() {
        let tail = StderrTail::new();
        tail.push("connect-time failure");
        let probe = TurnOutputProbe::new(tail.mark());

        let details = build_empty_turn_details(&probe, &tail).expect("details");
        assert!(details.contains("stderr (recent"), "{details}");
        assert!(details.contains("connect-time failure"));
    }

    #[test]
    fn empty_turn_details_report_drop_counts() {
        let tail = StderrTail::new();
        let mut probe = TurnOutputProbe::new(0);
        probe.note_dropped(DropSite::Decode, &drop_err("trailing characters"));
        probe.note_dropped(DropSite::Dispatch, &drop_err("EOF while parsing a value"));

        let details = build_empty_turn_details(&probe, &tail).expect("details");
        assert!(details.contains("dropped 2 update(s) (1 decode, 1 dispatch)"), "{details}");
        assert!(details.contains("first (decode): trailing characters"), "{details}");
    }

    #[test]
    fn empty_turn_details_are_none_without_evidence() {
        let tail = StderrTail::new();
        let probe = TurnOutputProbe::new(0);
        assert!(build_empty_turn_details(&probe, &tail).is_none());
    }

    #[test]
    fn empty_turn_details_are_bounded() {
        let tail = StderrTail::new();
        for i in 0..40 {
            tail.push(&format!("{i:03} {}", "x".repeat(200)));
        }
        let probe = TurnOutputProbe::new(0);
        let details = build_empty_turn_details(&probe, &tail).expect("details");
        assert!(details.len() <= MAX_DETAILS_BYTES + '…'.len_utf8());
    }

    /// The existing non-empty reasons must keep their exact codes and stay
    /// `details`-free; only the empty family gained anything.
    #[test]
    fn turn_failure_error_event_preserves_existing_reasons() {
        assert!(turn_failure_error_event("end_turn", AgentType::ClaudeCode, None).is_none());
        assert!(turn_failure_error_event("cancelled", AgentType::ClaudeCode, None).is_none());

        for (reason, expected) in [
            ("refusal", "turn_failed_refusal"),
            ("max_tokens", "turn_failed_max_tokens"),
            ("max_turn_requests", "turn_failed_max_turn_requests"),
            ("unknown", "turn_failed_unknown"),
        ] {
            let Some(AcpEvent::Error { code, details, .. }) =
                turn_failure_error_event(reason, AgentType::ClaudeCode, None)
            else {
                panic!("{reason} should produce an error event");
            };
            assert_eq!(code.as_deref(), Some(expected));
            assert!(details.is_none(), "{reason} must not carry details");
        }
    }

    #[test]
    fn turn_failure_error_event_maps_each_empty_cause() {
        for (cause, expected) in [
            (EmptyTurnCause::NoOutput, "turn_failed_empty"),
            (
                EmptyTurnCause::ProtocolMismatch,
                "turn_failed_empty_protocol",
            ),
            (EmptyTurnCause::MetadataOnly, "turn_failed_empty_metadata"),
        ] {
            let report = EmptyTurnReport {
                cause,
                details: Some("evidence".into()),
            };
            let Some(AcpEvent::Error {
                code,
                details,
                terminal,
                ..
            }) = turn_failure_error_event("empty", AgentType::ClaudeCode, Some(&report))
            else {
                panic!("empty should produce an error event");
            };
            assert_eq!(code.as_deref(), Some(expected));
            assert_eq!(details.as_deref(), Some("evidence"));
            assert!(!terminal, "an empty turn never kills the connection");
        }

        // No report (a path that skipped diagnosis) keeps the original code.
        let Some(AcpEvent::Error { code, details, .. }) =
            turn_failure_error_event("empty", AgentType::ClaudeCode, None)
        else {
            panic!("empty should produce an error event");
        };
        assert_eq!(code.as_deref(), Some("turn_failed_empty"));
        assert!(details.is_none());
    }

    #[test]
    fn build_new_session_request_sets_claude_raw_meta() {
        let cwd = std::path::PathBuf::from("/tmp/codeg");
        let req = build_new_session_request(AgentType::ClaudeCode, &cwd, Vec::new());

        assert_eq!(
            req.meta
                .as_ref()
                .and_then(|m| m.get("claudeCode"))
                .and_then(|v| v.get("emitRawSDKMessages"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    /// The `loadSession` capability gate hands the failure ladder a synthetic
    /// error instead of sending an unsupported RPC. That error must classify as
    /// "just open a new session": anything else would put a "session could not
    /// be loaded" banner in front of every user whose agent simply does not
    /// implement `session/load`.
    #[test]
    fn a_session_load_never_sent_falls_back_without_alarming_the_user() {
        let e = sacp::Error::method_not_found()
            .data("agent does not advertise the loadSession capability");
        let text = e.to_string();
        assert_eq!(classify_session_load_failure(e.code, &text), None);
        assert!(text.contains("Method not found"), "{text}");
        assert!(!text.contains("Authentication required"), "{text}");
    }

    #[test]
    fn the_model_selector_is_the_model_recorded_on_a_turn() {
        let select = |id: &str, category: &str, current: &str| SessionConfigOptionInfo {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            category: Some(category.to_string()),
            kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                current_value: current.to_string(),
                options: Vec::new(),
                groups: Vec::new(),
            }),
        };

        // The model comes from the `model` selector, not from whichever
        // selector happens to be first — agents publish several.
        assert_eq!(
            current_model_id_from_opts(&[
                select("effort", "mode", "high"),
                select("model", "model", "grok-4"),
            ]),
            Some("grok-4".to_string())
        );
        // No model selector (the common case for custom agents) and an empty
        // current value both mean "unknown", never a placeholder.
        assert_eq!(
            current_model_id_from_opts(&[select("effort", "mode", "high")]),
            None
        );
        assert_eq!(current_model_id_from_opts(&[select("m", "model", "")]), None);
        assert_eq!(current_model_id_from_opts(&[]), None);
    }

    #[test]
    fn build_load_session_request_skips_meta_for_non_claude() {
        let cwd = std::path::PathBuf::from("/tmp/codeg");
        let req = build_load_session_request(
            AgentType::Codex,
            SessionId::new("abc".to_string()),
            &cwd,
            Vec::new(),
        );

        assert!(req.meta.is_none());
    }

    // OpenClaw rejects MCP server *entries* over the ACP wire, not the
    // `mcpServers` field itself. The ACP schema does not `skip_serializing_if`
    // that field on NewSessionRequest/LoadSessionRequest, so it always
    // serializes as `[]`; every agent (OpenClaw included) already receives
    // `mcpServers: []` on a fresh install with no servers configured and
    // codeg-mcp off — the known-good payload. The connection-layer gate
    // (`supports_mcp == false`) forces OpenClaw onto that empty payload
    // unconditionally. This pins the wire contract: both builders emit an
    // empty list, so no server entry can ever reach OpenClaw.
    #[test]
    fn openclaw_session_requests_carry_no_mcp_servers() {
        let cwd = std::path::PathBuf::from("/tmp/codeg");

        let new_req = build_new_session_request(AgentType::OpenClaw, &cwd, Vec::new());
        assert!(
            new_req.mcp_servers.is_empty(),
            "OpenClaw session/new must carry no MCP servers"
        );
        let new_json = serde_json::to_value(&new_req).unwrap();
        assert_eq!(
            new_json.get("mcpServers"),
            Some(&serde_json::json!([])),
            "OpenClaw session/new mcpServers must serialize as an empty list"
        );

        let load_req = build_load_session_request(
            AgentType::OpenClaw,
            SessionId::new("openclaw-session".to_string()),
            &cwd,
            Vec::new(),
        );
        assert!(
            load_req.mcp_servers.is_empty(),
            "OpenClaw session/load must carry no MCP servers"
        );
        let load_json = serde_json::to_value(&load_req).unwrap();
        assert_eq!(
            load_json.get("mcpServers"),
            Some(&serde_json::json!([])),
            "OpenClaw session/load mcpServers must serialize as an empty list"
        );
    }

    fn stdio_server(name: &str) -> McpServer {
        McpServer::Stdio(McpServerStdio::new(
            name,
            std::path::PathBuf::from("/usr/local/bin/node"),
        ))
    }

    // The `supports_mcp` hint fires only where the declaration could actually
    // be wrong: a custom agent that was handed server entries.
    #[test]
    fn mcp_suspect_tags_only_custom_agents_with_servers() {
        let servers = vec![stdio_server("codeg")];

        let tagged = tag_mcp_suspect(
            sacp::util::internal_error("session/new failed: unknown field `mcpServers`"),
            AgentType::Custom("my-agent"),
            &servers,
        );
        assert!(
            tagged.to_string().contains(MCP_SUSPECT_SENTINEL),
            "a custom agent that received MCP servers must be tagged"
        );

        // Nothing was forwarded, so MCP cannot be the cause.
        let empty = tag_mcp_suspect(
            sacp::util::internal_error("session/new failed: boom"),
            AgentType::Custom("my-agent"),
            &[],
        );
        assert!(
            !empty.to_string().contains(MCP_SUSPECT_SENTINEL),
            "an empty server list rules MCP out"
        );

        // A built-in's flag is a verified repository constant, and the user has
        // no switch to flip — blaming MCP would misdirect them.
        let builtin = tag_mcp_suspect(
            sacp::util::internal_error("session/new failed: boom"),
            AgentType::ClaudeCode,
            &servers,
        );
        assert!(
            !builtin.to_string().contains(MCP_SUSPECT_SENTINEL),
            "built-in agents must never be tagged"
        );
    }

    // The sentinel is a transport detail: it must be consumed by the
    // translation, never shown. This mirrors the `.map_err` in
    // `run_connection`, which cannot be called directly from a unit test.
    #[test]
    fn mcp_suspect_sentinel_translates_to_code_and_is_stripped() {
        let raw = tag_mcp_suspect(
            sacp::util::internal_error("Unsupported parameter: mcpServers"),
            AgentType::Custom("my-agent"),
            &[stdio_server("codeg")],
        )
        .to_string();

        let err = AcpError::mcp_rejected(raw.replace(MCP_SUSPECT_SENTINEL, ""));

        assert_eq!(err.code(), Some("mcp_rejected_by_agent"));
        let shown = err.to_string();
        assert!(
            shown.contains("Unsupported parameter: mcpServers"),
            "the agent's own message must survive: {shown}"
        );
        assert!(
            !shown.contains(MCP_SUSPECT_SENTINEL),
            "the sentinel must never reach the user: {shown}"
        );
    }

    #[test]
    fn build_resume_session_request_sets_claude_raw_meta() {
        let cwd = std::path::PathBuf::from("/tmp/codeg");
        let req = build_resume_session_request(
            AgentType::ClaudeCode,
            SessionId::new("abc".to_string()),
            &cwd,
            Vec::new(),
        );

        assert_eq!(
            req.meta
                .as_ref()
                .and_then(|m| m.get("claudeCode"))
                .and_then(|v| v.get("emitRawSDKMessages"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn build_resume_session_request_skips_meta_for_non_claude() {
        let cwd = std::path::PathBuf::from("/tmp/codeg");
        let req = build_resume_session_request(
            AgentType::Codex,
            SessionId::new("abc".to_string()),
            &cwd,
            Vec::new(),
        );

        assert!(req.meta.is_none());
    }

    // Unlike NewSessionRequest/LoadSessionRequest (whose `mcp_servers` has no
    // `skip_serializing_if`, so it always serializes as `[]`),
    // ResumeSessionRequest marks `mcp_servers` `skip_serializing_if =
    // Vec::is_empty` — an empty list is OMITTED from the wire entirely. OpenClaw
    // (which supports session/resume) tolerates both an absent key and `[]`, and
    // the connection-layer gate keeps the list empty regardless, so no server
    // entry can ever reach it. Pin both the empty-list invariant and the
    // documented wire-shape divergence here.
    #[test]
    fn openclaw_resume_request_carries_no_mcp_servers() {
        let cwd = std::path::PathBuf::from("/tmp/codeg");
        let req = build_resume_session_request(
            AgentType::OpenClaw,
            SessionId::new("openclaw-session".to_string()),
            &cwd,
            Vec::new(),
        );
        assert!(
            req.mcp_servers.is_empty(),
            "OpenClaw session/resume must carry no MCP servers"
        );

        let json = serde_json::to_value(&req).unwrap();
        assert!(
            json.get("mcpServers").is_none(),
            "empty mcp_servers must be omitted from the resume wire payload"
        );
        // camelCase round-trip: sanity that the UntypedMessage send produces the
        // ACP-correct shape.
        assert!(
            json.get("sessionId").is_some(),
            "sessionId must serialize in camelCase"
        );
        assert!(json.get("cwd").is_some());
    }

    #[test]
    fn canonical_spec_to_mcp_server_stdio() {
        // Use an absolute path so the test is portable across machines that
        // may or may not have `npx` on PATH.
        let spec = serde_json::json!({
            "type": "stdio",
            "command": "/usr/local/bin/npx",
            "args": ["-y", "@mcp_hub_org/cli@latest", "run", "figma-developer-mcp"],
            "env": {"FIGMA_API_KEY": "secret"},
        });
        let server = canonical_spec_to_mcp_server("figma", &spec).expect("stdio spec should map");
        match server {
            McpServer::Stdio(s) => {
                assert_eq!(s.name, "figma");
                assert_eq!(s.command, std::path::PathBuf::from("/usr/local/bin/npx"));
                assert_eq!(s.args.len(), 4);
                assert_eq!(s.env.len(), 1);
                assert_eq!(s.env[0].name, "FIGMA_API_KEY");
            }
            other => panic!("expected Stdio variant, got {other:?}"),
        }
    }

    #[test]
    fn canonical_spec_resolves_bare_command_to_absolute() {
        // Bare command names get resolved via PATH so the resulting payload
        // satisfies the ACP "command MUST be absolute" requirement. We use
        // `cargo` because the test process must have it on PATH.
        let spec = serde_json::json!({
            "type": "stdio",
            "command": "cargo",
        });
        let server = canonical_spec_to_mcp_server("x", &spec).expect("bare command should resolve");
        match server {
            McpServer::Stdio(s) => assert!(
                s.command.is_absolute(),
                "expected absolute path, got {}",
                s.command.display()
            ),
            other => panic!("expected Stdio variant, got {other:?}"),
        }
    }

    #[test]
    fn grok_incompatible_agent_switch_detects_stable_code() {
        // Exact shape Grok returns when switching to a model whose agentType
        // differs from the established conversation's (captured from a live
        // `session/set_model` probe against grok 0.2.94).
        let err = sacp::Error::new(-32600, "Cannot switch to model ...").data(serde_json::json!({
            "code": "MODEL_SWITCH_INCOMPATIBLE_AGENT",
            "activeAgentType": "grok-build-plan",
            "requiredAgentType": "cursor",
            "modelId": "grok-composer-2.5-fast",
            "suggestion": "start_new_session"
        }));
        assert!(is_grok_incompatible_agent_switch(&err));

        // A different data.code, or no data at all, must NOT be swallowed —
        // those fall through to the generic error path.
        let other = sacp::Error::new(-32603, "boom")
            .data(serde_json::json!({ "code": "SOMETHING_ELSE" }));
        assert!(!is_grok_incompatible_agent_switch(&other));
        assert!(!is_grok_incompatible_agent_switch(&sacp::Error::internal_error()));
    }

    #[test]
    fn synthesize_grok_config_options_yields_model_and_effort_selectors() {
        // `_meta["x.ai/sessionConfig"].options` as delivered by `session/new`
        // (captured live): both model choices and the "mode" effort choices.
        let meta: serde_json::Map<String, serde_json::Value> = serde_json::from_value(
            serde_json::json!({
                "x.ai/sessionConfig": {
                    "options": [
                        {"id": "grok-4.5", "category": "model", "label": "Grok 4.5", "selected": true},
                        {"id": "grok-composer-2.5-fast", "category": "model", "label": "Composer 2.5", "selected": false},
                        {"id": "high", "category": "mode", "label": "High Effort", "selected": true},
                        {"id": "low", "category": "mode", "label": "Low Effort", "selected": false}
                    ]
                }
            }),
        )
        .unwrap();

        // Empty specs → the effort selector comes from the flat `x.ai/sessionConfig`
        // "mode" list (the no-`models` fallback path).
        let opts =
            synthesize_grok_config_options(Some(&meta), &HashMap::new()).expect("should synthesize");
        assert_eq!(opts.len(), 2, "model + effort selectors");

        let model = &opts[0];
        assert_eq!(model.id, GROK_MODEL_OPTION_ID);
        assert_eq!(model.category.as_deref(), Some("model"));
        let model_sel = expect_select(&model.kind);
        // Both models appear (agent-type filtering is deliberately NOT applied —
        // cross-type switches are handled gracefully at set time instead).
        assert_eq!(model_sel.options.len(), 2);
        assert_eq!(model_sel.current_value, "grok-4.5", "the `selected` model is current");
        assert!(model_sel.options.iter().any(|o| o.value == "grok-composer-2.5-fast"));

        let effort = &opts[1];
        assert_eq!(effort.id, GROK_EFFORT_OPTION_ID);
        assert_eq!(effort.category.as_deref(), Some("mode"));
        let effort_sel = expect_select(&effort.kind);
        assert_eq!(effort_sel.options.len(), 2);
        assert_eq!(effort_sel.current_value, "high", "the `selected` effort is current");
        assert!(effort_sel.options.iter().any(|o| o.value == "low"));
    }

    #[test]
    fn synthesize_grok_config_options_model_only_when_no_effort_offered() {
        // A model that doesn't advertise `supportsReasoningEffort` yields no
        // `category:"mode"` entries → only the model selector is surfaced.
        let meta: serde_json::Map<String, serde_json::Value> = serde_json::from_value(
            serde_json::json!({
                "x.ai/sessionConfig": {
                    "options": [
                        {"id": "grok-composer-2.5-fast", "category": "model", "label": "Composer 2.5", "selected": true}
                    ]
                }
            }),
        )
        .unwrap();
        // Empty specs → the effort selector comes from the flat `x.ai/sessionConfig`
        // "mode" list (the no-`models` fallback path).
        let opts =
            synthesize_grok_config_options(Some(&meta), &HashMap::new()).expect("should synthesize");
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].id, GROK_MODEL_OPTION_ID);
    }

    #[test]
    fn grok_set_model_params_carry_effort_override() {
        // Pure model switch → no `_meta`, so grok keeps the current effort.
        let p = build_grok_set_model_params("s1", "grok-4.5", None);
        assert_eq!(p["sessionId"], "s1");
        assert_eq!(p["modelId"], "grok-4.5");
        assert!(p.get("_meta").is_none());
        // Effort override rides in `_meta.reasoningEffort` (the key grok parses).
        let p = build_grok_set_model_params("s1", "grok-4.5", Some("high"));
        assert_eq!(p["modelId"], "grok-4.5");
        assert_eq!(p["_meta"]["reasoningEffort"], "high");
    }

    // ── config-option verdicts ──────────────────────────────────────────────
    //
    // `session/set_config_option` is advisory: the agent answers with the option
    // list it adopted, and codeg renders that verbatim — so a refused pick reads
    // in the composer as the selector springing back for no reason. Only this
    // side can tell a request's answer from an unsolicited update, so the
    // comparison has to be exactly right here.

    fn rejection_fixture(current: &str) -> Vec<SessionConfigOptionInfo> {
        vec![SessionConfigOptionInfo {
            id: "thought_level".to_string(),
            name: "Thinking".to_string(),
            description: None,
            category: Some("thought_level".to_string()),
            kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                current_value: current.to_string(),
                options: vec![
                    SessionConfigSelectOptionInfo {
                        value: "off".to_string(),
                        name: "Thinking: off".to_string(),
                        description: None,
                    },
                    SessionConfigSelectOptionInfo {
                        value: "high".to_string(),
                        name: "Thinking: high".to_string(),
                        description: None,
                    },
                ],
                groups: vec![],
            }),
        }]
    }

    #[test]
    fn config_option_rejection_reports_a_clamped_pick_with_labels() {
        // pi clamps every level to `off` for a model that never declared
        // `reasoning` — the bug that made the picker look broken.
        let event = config_option_rejection(&rejection_fixture("off"), "thought_level", "high")
            .expect("a clamped pick is a rejection");
        match event {
            AcpEvent::ConfigOptionRejected {
                config_id,
                option_name,
                requested,
                actual,
            } => {
                assert_eq!(config_id, "thought_level");
                assert_eq!(option_name, "Thinking");
                // Labels, not ids: the dropdown showed these strings.
                assert_eq!(requested, "Thinking: high");
                assert_eq!(actual, "Thinking: off");
            }
            other => panic!("expected ConfigOptionRejected, got {other:?}"),
        }
    }

    #[test]
    fn config_option_rejection_is_silent_when_the_pick_landed() {
        assert!(config_option_rejection(&rejection_fixture("high"), "thought_level", "high").is_none());
    }

    #[test]
    fn config_option_rejection_falls_back_to_the_raw_id_without_a_label() {
        // An agent may settle on a value it never advertised; naming the raw id
        // beats naming nothing.
        let event = config_option_rejection(&rejection_fixture("medium"), "thought_level", "high")
            .expect("still a rejection");
        match event {
            AcpEvent::ConfigOptionRejected { actual, .. } => assert_eq!(actual, "medium"),
            other => panic!("expected ConfigOptionRejected, got {other:?}"),
        }
    }

    #[test]
    fn config_option_rejection_stays_silent_when_there_is_nothing_to_compare() {
        let options = rejection_fixture("off");
        // Option absent from the answer → nothing to compare against.
        assert!(config_option_rejection(&options, "model", "grok-4.5").is_none());
        // A kind with no comparable id → leave it to the agent. A false "your
        // pick was changed" notice is worse than none.
        let toggle = vec![SessionConfigOptionInfo {
            id: "auto_approve".to_string(),
            name: "Auto-approve tools".to_string(),
            description: None,
            category: None,
            kind: SessionConfigKindInfo::Boolean(SessionConfigBooleanInfo {
                current_value: false,
            }),
        }];
        assert!(config_option_rejection(&toggle, "auto_approve", "true").is_none());
    }

    #[test]
    fn config_option_rejection_reads_a_grouped_select() {
        // Grouped options are flattened by `map_session_config_options` before the
        // comparison, so a grouped model picker resolves its labels too.
        let raw = serde_json::json!([{
            "id": "model",
            "name": "Model",
            "type": "select",
            "currentValue": "openai/gpt-5",
            "options": [{
                "group": "openai",
                "name": "OpenAI",
                "options": [
                    {"value": "openai/gpt-5", "name": "GPT-5"},
                    {"value": "openai/gpt-5-mini", "name": "GPT-5 Mini"}
                ]
            }]
        }]);
        let parsed: Vec<SessionConfigOption> = serde_json::from_value(raw).expect("parses");
        let mapped = map_session_config_options(&parsed);

        let event = config_option_rejection(&mapped, "model", "openai/gpt-5-mini")
            .expect("the agent kept a different model");
        match event {
            AcpEvent::ConfigOptionRejected {
                requested, actual, ..
            } => {
                assert_eq!(requested, "GPT-5 Mini");
                assert_eq!(actual, "GPT-5");
            }
            other => panic!("expected ConfigOptionRejected, got {other:?}"),
        }
    }

    #[test]
    fn synthesize_grok_config_options_none_without_sessionconfig() {
        let empty: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        assert!(synthesize_grok_config_options(Some(&empty), &HashMap::new()).is_none());
        assert!(synthesize_grok_config_options(None, &HashMap::new()).is_none());
    }

    /// Raw top-level `models` mirroring grok 0.2.99's `session/new`: grok-4.5
    /// supports effort (default `xhigh`, switchable high/medium/low),
    /// grok-composer-2.5-fast supports none.
    fn grok_models_fixture() -> serde_json::Value {
        serde_json::json!({
            "currentModelId": "grok-4.5",
            "availableModels": [
                {
                    "modelId": "grok-4.5",
                    "name": "Grok 4.5",
                    "_meta": {
                        "totalContextTokens": 500000,
                        "supportsReasoningEffort": true,
                        "reasoningEffort": "xhigh",
                        "reasoningEfforts": [
                            {"id": "high", "label": "High Effort", "description": "Highest quality", "default": true},
                            {"id": "medium", "label": "Medium Effort", "description": "Balanced"},
                            {"id": "low", "label": "Low Effort", "description": "Fast"}
                        ]
                    }
                },
                {
                    "modelId": "grok-composer-2.5-fast",
                    "name": "Composer 2.5",
                    "_meta": {"supportsReasoningEffort": false}
                }
            ]
        })
    }

    #[test]
    fn parse_grok_model_specs_reads_per_model_meta() {
        let specs = parse_grok_model_specs(Some(&grok_models_fixture()));
        let g45 = specs.get("grok-4.5").expect("grok-4.5 present");
        assert!(g45.supports);
        assert_eq!(g45.default.as_deref(), Some("xhigh"));
        assert_eq!(g45.options.len(), 3);
        assert_eq!(g45.options[0].0, "high");
        // Grok's own window rides the same per-model `_meta` — the number the
        // context ring uses instead of inferring one from the model id.
        assert_eq!(g45.context_window, Some(500_000));
        let fast = specs
            .get("grok-composer-2.5-fast")
            .expect("composer present");
        assert!(!fast.supports);
        assert!(fast.default.is_none());
        assert!(fast.options.is_empty());
        // No `totalContextTokens` → no opinion (the id heuristic decides).
        assert_eq!(fast.context_window, None);
        // …and a zero window is "no opinion" too, never a ring divided by zero.
        let zeroed = parse_grok_model_specs(Some(&serde_json::json!({
            "availableModels": [{"modelId": "z", "_meta": {"totalContextTokens": 0}}]
        })));
        assert_eq!(zeroed["z"].context_window, None);
    }

    #[test]
    fn parse_grok_model_specs_absent_models_is_empty() {
        assert!(parse_grok_model_specs(None).is_empty());
        assert!(parse_grok_model_specs(Some(&serde_json::json!({}))).is_empty());
        // Missing `_meta` degrades to supports=false / default=None / options=[].
        let bare = serde_json::json!({ "availableModels": [{"modelId": "m1", "name": "M1"}] });
        let specs = parse_grok_model_specs(Some(&bare));
        let m1 = specs.get("m1").expect("m1 present");
        assert!(!m1.supports);
        assert!(m1.default.is_none());
        assert!(m1.options.is_empty());
    }

    #[test]
    fn build_grok_effort_option_injects_default_and_gates_supports() {
        let specs = parse_grok_model_specs(Some(&grok_models_fixture()));
        // grok-4.5: `xhigh` default is injected at the FRONT (not in the
        // switchable list), current = xhigh, with canonical labels.
        let effort = build_grok_effort_option("grok-4.5", &specs).expect("has effort");
        assert_eq!(effort.id, GROK_EFFORT_OPTION_ID);
        let sel = expect_select(&effort.kind);
        assert_eq!(sel.current_value, "xhigh");
        assert_eq!(sel.options.len(), 4, "high/medium/low + injected xhigh");
        assert_eq!(sel.options[0].value, "xhigh");
        assert_eq!(sel.options[0].name, "Max");
        // The injected default has no grok description, so it gets our canonical
        // one — every tier must have sub-text, not just high/medium/low.
        assert_eq!(
            sel.options[0].description.as_deref(),
            Some("Maximum reasoning for the most complex tasks")
        );
        assert!(sel.options.iter().all(|o| o.description.is_some()));
        // Grok's own per-tier text is preserved for the switchable tiers.
        assert!(sel
            .options
            .iter()
            .any(|o| o.value == "high" && o.name == "High" && o.description.as_deref() == Some("Highest quality")));
        // Unsupported model → no selector; unknown model → None.
        assert!(build_grok_effort_option("grok-composer-2.5-fast", &specs).is_none());
        assert!(build_grok_effort_option("nope", &specs).is_none());
    }

    #[test]
    fn synthesize_grok_config_options_model_reactive_effort_for_4_5() {
        // Flat sessionConfig marks grok-4.5 current; per-model specs drive effort.
        let meta: serde_json::Map<String, serde_json::Value> = serde_json::from_value(
            serde_json::json!({
                "x.ai/sessionConfig": {
                    "options": [
                        {"id": "grok-4.5", "category": "model", "label": "Grok 4.5", "selected": true},
                        {"id": "grok-composer-2.5-fast", "category": "model", "label": "Composer 2.5", "selected": false}
                    ]
                }
            }),
        )
        .unwrap();
        let specs = parse_grok_model_specs(Some(&grok_models_fixture()));
        let opts = synthesize_grok_config_options(Some(&meta), &specs).expect("synthesize");
        assert_eq!(opts.len(), 2, "model + effort");
        let effort = opts
            .iter()
            .find(|o| o.id == GROK_EFFORT_OPTION_ID)
            .expect("effort selector");
        let sel = expect_select(&effort.kind);
        assert_eq!(sel.current_value, "xhigh", "grok-4.5's real default");
        assert!(sel.options.iter().any(|o| o.value == "xhigh" && o.name == "Max"));
    }

    #[test]
    fn synthesize_grok_config_options_no_effort_for_composer_fast() {
        // Current model is the no-effort composer model → only the model selector.
        let meta: serde_json::Map<String, serde_json::Value> = serde_json::from_value(
            serde_json::json!({
                "x.ai/sessionConfig": {
                    "options": [
                        {"id": "grok-4.5", "category": "model", "label": "Grok 4.5", "selected": false},
                        {"id": "grok-composer-2.5-fast", "category": "model", "label": "Composer 2.5", "selected": true}
                    ]
                }
            }),
        )
        .unwrap();
        let specs = parse_grok_model_specs(Some(&grok_models_fixture()));
        let opts = synthesize_grok_config_options(Some(&meta), &specs).expect("synthesize");
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].id, GROK_MODEL_OPTION_ID);
    }

    #[test]
    fn set_grok_effort_selector_for_model_drops_and_adds() {
        let specs = parse_grok_model_specs(Some(&grok_models_fixture()));
        // Model + grok-4.5 effort → switching to the no-effort model DROPS effort.
        let mut opts = grok_model_options("grok-4.5");
        opts.push(build_grok_effort_option("grok-4.5", &specs).unwrap());
        assert_eq!(opts.len(), 2);
        set_grok_effort_selector_for_model(&mut opts, "grok-composer-2.5-fast", &specs);
        assert_eq!(opts.len(), 1);
        assert!(opts.iter().all(|o| o.id != GROK_EFFORT_OPTION_ID));
        // Switching back to grok-4.5 RE-ADDS it, current = xhigh.
        set_grok_effort_selector_for_model(&mut opts, "grok-4.5", &specs);
        let effort = opts
            .iter()
            .find(|o| o.id == GROK_EFFORT_OPTION_ID)
            .expect("re-added");
        let sel = expect_select(&effort.kind);
        assert_eq!(sel.current_value, "xhigh");
    }

    fn grok_model_options(current: &str) -> Vec<SessionConfigOptionInfo> {
        vec![SessionConfigOptionInfo {
            id: GROK_MODEL_OPTION_ID.to_string(),
            name: "Model".to_string(),
            description: None,
            category: Some("model".to_string()),
            kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                current_value: current.to_string(),
                options: vec![
                    SessionConfigSelectOptionInfo {
                        value: "grok-4.5".to_string(),
                        name: "Grok 4.5".to_string(),
                        description: None,
                    },
                    SessionConfigSelectOptionInfo {
                        value: "grok-composer-2.5-fast".to_string(),
                        name: "Composer 2.5".to_string(),
                        description: None,
                    },
                ],
                groups: Vec::new(),
            }),
        }]
    }

    #[tokio::test]
    async fn grok_incompatible_agent_switch_reverts_and_reports_without_deadlock() {
        use std::time::Duration;

        let mut st = SessionState::new(
            "conn-test".to_string(),
            AgentType::Grok,
            None,
            "win".to_string(),
            None,
        );
        // The conversation is on grok-4.5; the user optimistically picked the
        // cross-agent-type Composer model, which Grok rejected mid-conversation.
        st.config_options = Some(grok_model_options("grok-4.5"));
        let state = Arc::new(RwLock::new(st));
        let emitter = EventEmitter::Noop;

        // Regression guard: the recovery previously read `config_options` inline
        // in an `if let`, holding the read guard across `emit_*` (which take the
        // write lock) → deadlock. A timeout turns that hang into a failure.
        tokio::time::timeout(
            Duration::from_secs(5),
            emit_grok_incompatible_agent_switch(&state, &emitter),
        )
        .await
        .expect("recovery must complete, not deadlock on the state lock");

        let guard = state.read().await;

        // The optimistic pick is reverted: the authoritative model is unchanged.
        let opts = guard.config_options.as_ref().expect("options preserved");
        let sel = expect_select(&opts[0].kind);
        assert_eq!(sel.current_value, "grok-4.5");

        // Event ordering: the authoritative options (revert) precede the coded
        // error so the composer snaps back before the toast appears.
        let events = guard.recent_events_after(0).expect("events recorded");
        let cfg_idx = events
            .iter()
            .position(|e| matches!(&e.payload, AcpEvent::SessionConfigOptions { .. }))
            .expect("a session_config_options revert is emitted");
        let err_idx = events
            .iter()
            .position(|e| matches!(&e.payload, AcpEvent::Error { .. }))
            .expect("a coded error is emitted");
        assert!(cfg_idx < err_idx, "revert must precede the error");

        // The reverted options carry the original model.
        if let AcpEvent::SessionConfigOptions { config_options } = &events[cfg_idx].payload {
            let sel = expect_select(&config_options[0].kind);
            assert_eq!(sel.current_value, "grok-4.5");
        }

        // Exactly one error, carrying the localizable code (not a raw message)
        // and recoverable — no generic double-emit.
        let errors: Vec<(Option<String>, bool)> = events
            .iter()
            .filter_map(|e| match &e.payload {
                AcpEvent::Error {
                    code, terminal, ..
                } => Some((code.clone(), *terminal)),
                _ => None,
            })
            .collect();
        assert_eq!(errors.len(), 1, "no double error emit");
        assert_eq!(
            errors[0].0.as_deref(),
            Some(GROK_INCOMPATIBLE_AGENT_ERROR_CODE)
        );
        assert!(!errors[0].1, "recoverable, not terminal");
    }

    #[test]
    fn grok_live_tool_output_prefers_content() {
        // The clean content channel carries the output → don't ship raw_output
        // at all (frontend renders `content`, matching the parser's precedence).
        let content = Some("build ok\n".to_string());
        let raw = Some(serde_json::json!({
            "output_for_prompt": "exit: 0\n\nbuild ok",
            "exit_code": 0,
            "command": "pnpm build",
        }));
        assert_eq!(grok_live_tool_output(&content, &raw), None);
    }

    #[test]
    fn grok_live_tool_output_falls_back_to_output_for_prompt_when_content_empty() {
        // With no content, recover the readable text from the string
        // `output_for_prompt` (NOT the byte-array `output`, NOT the whole blob).
        let raw = Some(serde_json::json!({
            "output": [10, 62, 32],
            "output_for_prompt": "exit: 0\n\nok",
            "exit_code": 0,
            "command": "pnpm build",
        }));
        assert_eq!(
            grok_live_tool_output(&None, &raw).as_deref(),
            Some("exit: 0\n\nok")
        );
        // Whitespace-only content is treated as empty.
        let ws = Some("  \n".to_string());
        assert_eq!(
            grok_live_tool_output(&ws, &raw).as_deref(),
            Some("exit: 0\n\nok")
        );
    }

    /// A `get_command_or_subagent_output` poll has no `content[]` and no
    /// `output_for_prompt` — its whole result sits under the `TaskOutput`
    /// envelope, which used to be dropped, streaming an empty card. Live must
    /// emit the SAME string the history parser stores so the background-task
    /// card renders identically before and after a reload.
    #[test]
    fn grok_live_tool_output_emits_task_output_envelope() {
        let raw = serde_json::json!({
            "type": "TaskOutput",
            "Result": {
                "task_id": "term_b0d",
                "command": "/bin/bash -lc 'pnpm dev'",
                "status": "failed",
                "exit_code": 1,
                "output": "boom",
            },
        });
        let live = grok_live_tool_output(&None, &Some(raw.clone())).expect("envelope emitted");
        assert_eq!(
            live,
            crate::parsers::grok::grok_task_output_envelope(&raw).unwrap(),
            "live and history must hand the frontend the same string"
        );
        let parsed: serde_json::Value = serde_json::from_str(&live).unwrap();
        assert_eq!(parsed["Result"]["exit_code"], 1);
        // A poll that DOES carry clean content keeps content's precedence.
        assert_eq!(
            grok_live_tool_output(&Some("已完成".to_string()), &Some(raw)),
            None
        );
    }

    #[test]
    fn grok_live_tool_output_none_without_usable_string() {
        // Object without `output_for_prompt` (only the byte-array `output`).
        let no_prompt = Some(serde_json::json!({
            "output": [10, 62],
            "exit_code": 0,
            "command": "x",
        }));
        assert_eq!(grok_live_tool_output(&None, &no_prompt), None);
        // Non-object rawOutput.
        assert_eq!(
            grok_live_tool_output(&None, &Some(serde_json::json!("a string"))),
            None
        );
        // Absent rawOutput.
        assert_eq!(grok_live_tool_output(&None, &None), None);
    }

    /// A finished Grok terminal `tool_call_update` carries the readable output in
    /// BOTH the `content[]` channel and a structured `rawOutput` object (its
    /// `output` field a byte array, text only under `output_for_prompt`).
    /// Regression: the live path must NOT ship the stringified object as
    /// `raw_output` (which shadows `content` and renders empty) — it emits `None`
    /// so the frontend renders the clean `content`.
    #[tokio::test]
    async fn grok_terminal_update_emits_content_not_raw_output_blob() {
        let st = SessionState::new(
            "conn-grok".to_string(),
            AgentType::Grok,
            None,
            "win".to_string(),
            None,
        );
        let state = Arc::new(RwLock::new(st));
        let emitter = EventEmitter::Noop;
        let mut cache = ToolCallOutputCache::default();
        let mut cb = CodeBuddyLiveState::default();

        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-1",
            "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "\n> build\nbuild ok\n"}}],
            "rawOutput": {
                "output": [10, 62, 32],
                "output_for_prompt": "exit: 0\n\nbuild ok",
                "exit_code": 0,
                "command": "pnpm build",
            },
        }))
        .expect("valid tool_call_update wire shape");

        emit_conversation_update(
            &state,
            &emitter,
            AgentType::Grok,
            update,
            None,
            &mut cache,
            &mut cb,
        )
        .await;

        let guard = state.read().await;
        let events = guard.recent_events_after(0).expect("events recorded");
        let (raw_output, content) = events
            .iter()
            .find_map(|e| match &e.payload {
                AcpEvent::ToolCallUpdate {
                    raw_output,
                    content,
                    ..
                } => Some((raw_output.clone(), content.clone())),
                _ => None,
            })
            .expect("a tool_call_update event is emitted");

        assert!(
            raw_output.is_none(),
            "Grok must not ship the rawOutput object blob (it shadows content \
             and the terminal renderer drops it): {raw_output:?}"
        );
        assert!(
            content.as_deref().is_some_and(|c| c.contains("build ok")),
            "the clean content channel carries the executed command's output: {content:?}"
        );
    }

    /// Contrast guard: the Grok-only extraction must not change other agents.
    /// A non-Grok agent that sends the same object-shaped `rawOutput` still gets
    /// it stringified into `raw_output` (existing `json_value_to_text` behavior).
    #[tokio::test]
    async fn non_grok_object_raw_output_is_stringified_unchanged() {
        let st = SessionState::new(
            "conn-claude".to_string(),
            AgentType::ClaudeCode,
            None,
            "win".to_string(),
            None,
        );
        let state = Arc::new(RwLock::new(st));
        let emitter = EventEmitter::Noop;
        let mut cache = ToolCallOutputCache::default();
        let mut cb = CodeBuddyLiveState::default();

        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-1",
            "status": "completed",
            "rawOutput": {"output_for_prompt": "exit: 0\n\nok", "command": "x"},
        }))
        .expect("valid tool_call_update wire shape");

        emit_conversation_update(
            &state,
            &emitter,
            AgentType::ClaudeCode,
            update,
            None,
            &mut cache,
            &mut cb,
        )
        .await;

        let guard = state.read().await;
        let events = guard.recent_events_after(0).expect("events recorded");
        let raw_output = events
            .iter()
            .find_map(|e| match &e.payload {
                AcpEvent::ToolCallUpdate { raw_output, .. } => Some(raw_output.clone()),
                _ => None,
            })
            .expect("a tool_call_update event is emitted");
        assert!(
            raw_output.is_some(),
            "non-Grok agents keep the existing json_value_to_text behavior"
        );
    }

    #[test]
    fn unwrap_grok_use_tool_peels_mcp_envelope() {
        // Grok's `use_tool` envelope nests the real MCP tool name + args.
        let raw = serde_json::json!({
            "tool_name": "codeg-mcp__delegate_to_agent",
            "tool_input": {"agent_type": "codex", "task": "build", "working_dir": "/w"},
        });
        let (name, input) = unwrap_grok_use_tool(Some(&raw)).expect("envelope peels");
        assert_eq!(name, "codeg-mcp__delegate_to_agent");
        assert_eq!(input.get("task").and_then(|v| v.as_str()), Some("build"));
        assert_eq!(
            input.get("agent_type").and_then(|v| v.as_str()),
            Some("codex")
        );
    }

    #[test]
    fn unwrap_grok_use_tool_ignores_native_tools() {
        // Native Grok tools carry args directly (no tool_name/tool_input shape) —
        // they must pass through untouched.
        let terminal = serde_json::json!({"command": "pnpm build"});
        assert!(unwrap_grok_use_tool(Some(&terminal)).is_none());
        // Missing tool_input.
        assert!(unwrap_grok_use_tool(Some(&serde_json::json!({"tool_name": "x"}))).is_none());
        // Empty tool_name.
        assert!(
            unwrap_grok_use_tool(Some(&serde_json::json!({"tool_name": "", "tool_input": {}})))
                .is_none()
        );
        // Absent / non-object.
        assert!(unwrap_grok_use_tool(None).is_none());
        assert!(unwrap_grok_use_tool(Some(&serde_json::json!("s"))).is_none());
    }

    #[test]
    fn grok_mcp_output_text_extracts_result() {
        // `{type:MCP, output:{OkayOutput:"…"}}` — text is the first string value.
        let ok = serde_json::json!({
            "type": "MCP",
            "tool_name": "delegate_to_agent",
            "output": {"OkayOutput": "Delegation successful. task_id=abc-123."},
        });
        assert_eq!(
            grok_mcp_output_text(&ok).as_deref(),
            Some("Delegation successful. task_id=abc-123.")
        );
        // `output` may be a bare string.
        let bare = serde_json::json!({"type": "MCP", "output": "done"});
        assert_eq!(grok_mcp_output_text(&bare).as_deref(), Some("done"));
        // An empty-string sibling (sorted before the real key) must not shadow
        // the populated result.
        let empty_first = serde_json::json!({
            "type": "MCP",
            "output": {"AErr": "", "OkayOutput": "real result"},
        });
        assert_eq!(
            grok_mcp_output_text(&empty_first).as_deref(),
            Some("real result")
        );
        // A pure error variant (any `*Output` key) is surfaced too.
        let err = serde_json::json!({"type": "MCP", "output": {"ErrOutput": "boom"}});
        assert_eq!(grok_mcp_output_text(&err).as_deref(), Some("boom"));
        // Non-MCP rawOutput → None (caller falls through to output_for_prompt).
        let bash = serde_json::json!({"type": "Bash", "output_for_prompt": "ok"});
        assert_eq!(grok_mcp_output_text(&bash), None);
    }

    #[test]
    fn cursor_companion_title_resolves_delegate_ack() {
        // The broker's running ack (broker.rs::running_ack) — leading
        // whitespace tolerated, the prefix is the contract.
        let ack = "Delegation successful. task_id=799467c7-0188-4e7a-b5ef-241d4b141a83. \
                   Call get_delegation_status with this id in the task_ids array.";
        assert_eq!(
            cursor_companion_title_from_content(Some(ack)),
            Some("codeg-mcp__delegate_to_agent")
        );
        assert_eq!(
            cursor_companion_title_from_content(Some(&format!("  {ack}"))),
            Some("codeg-mcp__delegate_to_agent")
        );
    }

    #[test]
    fn cursor_companion_title_resolves_status_report() {
        // Real-device shape: companion.rs::render_batch_report's compact JSON.
        let report = r#"{"tasks":[{"agent_type":"claude_code","child_conversation_id":1576,"duration_ms":27288,"status":"completed","task_id":"799467c7-0188-4e7a-b5ef-241d4b141a83","text":"done"}]}"#;
        assert_eq!(
            cursor_companion_title_from_content(Some(report)),
            Some("codeg-mcp__get_delegation_status")
        );
        // Mixed batch with a running item still resolves.
        let mixed = r#"{"tasks":[{"task_id":"a","status":"running"},{"task_id":"b","status":"unknown"}]}"#;
        assert_eq!(
            cursor_companion_title_from_content(Some(mixed)),
            Some("codeg-mcp__get_delegation_status")
        );
    }

    #[test]
    fn cursor_companion_title_rejects_lookalikes() {
        // Foreign task-manager output: status outside the report vocabulary.
        let foreign =
            r#"{"tasks":[{"task_id":"T-1","status":"todo"},{"task_id":"T-2","status":"done"}]}"#;
        assert_eq!(cursor_companion_title_from_content(Some(foreign)), None);
        // Item missing task_id.
        let missing = r#"{"tasks":[{"status":"completed"}]}"#;
        assert_eq!(cursor_companion_title_from_content(Some(missing)), None);
        // Empty batch carries nothing to verify — leave the title alone.
        assert_eq!(
            cursor_companion_title_from_content(Some(r#"{"tasks":[]}"#)),
            None
        );
        // Plain text / absent / non-JSON.
        assert_eq!(cursor_companion_title_from_content(Some("ls -la ok")), None);
        assert_eq!(cursor_companion_title_from_content(None), None);
        // Ack prefix must match from the start, not mid-string.
        assert_eq!(
            cursor_companion_title_from_content(Some(
                "Note: Delegation successful. task_id=x."
            )),
            None
        );
    }

    #[test]
    fn grok_live_tool_output_recovers_mcp_result() {
        // An MCP call (delegate ack) has empty content and no output_for_prompt;
        // the readable text lives under `output.OkayOutput`.
        let raw = Some(serde_json::json!({
            "type": "MCP",
            "tool_name": "delegate_to_agent",
            "server_name": "codeg-mcp",
            "output": {"OkayOutput": "Delegation successful. task_id=2dc85849-5426."},
        }));
        assert_eq!(
            grok_live_tool_output(&None, &raw).as_deref(),
            Some("Delegation successful. task_id=2dc85849-5426.")
        );
    }

    /// Grok wraps `delegate_to_agent` in a `use_tool` envelope. The live path must
    /// peel it so the emitted event carries the MCP tool name as its title and the
    /// real `{agent_type, task}` as raw_input — the exact shape the delegation
    /// broker (`lifecycle.rs`) correlates on and the frontend classifies into the
    /// delegation card — and must surface the MCP ack (carrying `task_id`) as
    /// output instead of dropping it.
    #[tokio::test]
    async fn grok_use_tool_delegate_unwraps_to_direct_mcp_call() {
        let st = SessionState::new(
            "conn-grok".to_string(),
            AgentType::Grok,
            None,
            "win".to_string(),
            None,
        );
        let state = Arc::new(RwLock::new(st));
        let emitter = EventEmitter::Noop;
        let mut cache = ToolCallOutputCache::default();
        let mut cb = CodeBuddyLiveState::default();

        // Initial tool_call carries the use_tool envelope (real Grok wire shape —
        // no kind/status on the update object; they default).
        let call: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-d",
            "title": "use_tool",
            "rawInput": {
                "tool_name": "codeg-mcp__delegate_to_agent",
                "tool_input": {"agent_type": "codex", "working_dir": "/w", "task": "run build"},
            },
        }))
        .expect("valid tool_call wire shape");
        emit_conversation_update(
            &state,
            &emitter,
            AgentType::Grok,
            call,
            None,
            &mut cache,
            &mut cb,
        )
        .await;

        // The ack arrives on the completed update as an MCP rawOutput. Real Grok
        // updates re-send the generic `use_tool` wrapper title and carry NO
        // raw_input — the recorded override must re-assert the peeled name so the
        // frontend reducer doesn't revert the delegation card to a generic tool.
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-d",
            "title": "use_tool",
            "status": "completed",
            "rawOutput": {
                "type": "MCP",
                "tool_name": "delegate_to_agent",
                "server_name": "codeg-mcp",
                "output": {"OkayOutput": "Delegation successful. task_id=2dc85849-5426-44f7."},
            },
        }))
        .expect("valid tool_call_update wire shape");
        emit_conversation_update(
            &state,
            &emitter,
            AgentType::Grok,
            update,
            None,
            &mut cache,
            &mut cb,
        )
        .await;

        let guard = state.read().await;
        let events = guard.recent_events_after(0).expect("events recorded");

        // Initial ToolCall: title unwrapped to the MCP tool name; raw_input the
        // real delegation args (the `use_tool` wrapper gone).
        let (title, raw_input) = events
            .iter()
            .find_map(|e| match &e.payload {
                AcpEvent::ToolCall {
                    title, raw_input, ..
                } => Some((title.clone(), raw_input.clone())),
                _ => None,
            })
            .expect("a tool_call event is emitted");
        assert_eq!(title, "codeg-mcp__delegate_to_agent");
        let raw_input = raw_input.expect("raw_input present after unwrap");
        assert!(
            raw_input.contains("\"agent_type\":\"codex\""),
            "raw_input carries agent_type: {raw_input}"
        );
        assert!(
            raw_input.contains("\"task\":\"run build\""),
            "raw_input carries task: {raw_input}"
        );
        assert!(
            !raw_input.contains("tool_input"),
            "the use_tool wrapper is peeled: {raw_input}"
        );

        // Update: the MCP ack (with task_id) surfaces as output, AND the emitted
        // title re-asserts the peeled name — the sparse `use_tool` wrapper title
        // must not win.
        let (upd_title, raw_output) = events
            .iter()
            .find_map(|e| match &e.payload {
                AcpEvent::ToolCallUpdate {
                    title, raw_output, ..
                } => raw_output.clone().map(|o| (title.clone(), o)),
                _ => None,
            })
            .expect("a tool_call_update with output is emitted");
        assert!(
            raw_output.contains("task_id=2dc85849"),
            "the delegate ack (with task_id) surfaces as output: {raw_output}"
        );
        assert_eq!(
            upd_title.as_deref(),
            Some("codeg-mcp__delegate_to_agent"),
            "the sparse-update wrapper title is overridden by the recorded name"
        );
        // No emitted event ever ships the generic `use_tool` wrapper title.
        assert!(
            events.iter().all(|e| !matches!(
                &e.payload,
                AcpEvent::ToolCall { title, .. } if title == "use_tool"
            ) && !matches!(
                &e.payload,
                AcpEvent::ToolCallUpdate { title: Some(t), .. } if t == "use_tool"
            )),
            "no event ships the generic use_tool wrapper title"
        );
    }

    /// The unwrap is symmetric on the ToolCallUpdate arm: an update that itself
    /// carries the `use_tool` envelope (rawInput) is peeled the same way — title →
    /// MCP name, raw_input → the inner args.
    #[tokio::test]
    async fn grok_use_tool_envelope_on_update_is_unwrapped() {
        let st = SessionState::new(
            "conn-grok".to_string(),
            AgentType::Grok,
            None,
            "win".to_string(),
            None,
        );
        let state = Arc::new(RwLock::new(st));
        let emitter = EventEmitter::Noop;
        let mut cache = ToolCallOutputCache::default();
        let mut cb = CodeBuddyLiveState::default();

        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-u",
            "title": "use_tool",
            "status": "in_progress",
            "rawInput": {
                "tool_name": "codeg-mcp__cancel_delegation",
                "tool_input": {"task_id": "abc-123"},
            },
        }))
        .expect("valid tool_call_update wire shape");
        emit_conversation_update(
            &state,
            &emitter,
            AgentType::Grok,
            update,
            None,
            &mut cache,
            &mut cb,
        )
        .await;

        let guard = state.read().await;
        let events = guard.recent_events_after(0).expect("events recorded");
        let (title, raw_input) = events
            .iter()
            .find_map(|e| match &e.payload {
                AcpEvent::ToolCallUpdate {
                    title, raw_input, ..
                } => Some((title.clone(), raw_input.clone())),
                _ => None,
            })
            .expect("a tool_call_update event is emitted");
        assert_eq!(title.as_deref(), Some("codeg-mcp__cancel_delegation"));
        let raw_input = raw_input.expect("raw_input present after unwrap");
        assert!(
            raw_input.contains("\"task_id\":\"abc-123\""),
            "inner args surface as raw_input: {raw_input}"
        );
        assert!(
            !raw_input.contains("tool_input"),
            "the use_tool wrapper is peeled: {raw_input}"
        );
    }

    #[test]
    fn canonical_spec_to_mcp_server_http_with_headers() {
        let spec = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "headers": {"Authorization": "Bearer token"},
        });
        let server = canonical_spec_to_mcp_server("remote", &spec).expect("http spec should map");
        match server {
            McpServer::Http(s) => {
                assert_eq!(s.url, "https://example.com/mcp");
                assert_eq!(s.headers.len(), 1);
                assert_eq!(s.headers[0].name, "Authorization");
            }
            other => panic!("expected Http variant, got {other:?}"),
        }
    }

    #[test]
    fn canonical_spec_to_mcp_server_rejects_unknown_type() {
        let spec = serde_json::json!({"type": "websocket", "url": "wss://x"});
        assert!(canonical_spec_to_mcp_server("x", &spec).is_err());
    }

    #[test]
    fn stdio_server_serializes_to_acp_wire_format() {
        // Replicates the Figma MCP entry shipped to the agent and asserts the
        // exact JSON shape claude-agent-acp expects (no `type` tag for stdio,
        // env as [{name, value}] array, command as a string path).
        let spec = serde_json::json!({
            "type": "stdio",
            "command": "/usr/local/bin/npx",
            "args": ["-y", "@mcp_hub_org/cli@latest", "run", "figma-developer-mcp"],
        });
        let server = canonical_spec_to_mcp_server("figma", &spec).expect("stdio spec should map");
        let json = serde_json::to_value(&server).expect("server should serialize");
        assert_eq!(json["name"], "figma");
        assert_eq!(json["command"], "/usr/local/bin/npx");
        assert_eq!(json["args"][0], "-y");
        assert_eq!(json["args"][1], "@mcp_hub_org/cli@latest");
        assert!(
            json.get("type").is_none(),
            "stdio variant must serialize without a `type` tag (claude-agent-acp \
             treats absence-of-type as stdio); got {json:#?}"
        );
    }

    // ─── ToolCallOutputCache ────────────────────────────────────────────

    #[test]
    fn cache_first_update_emits_full_replace() {
        let mut cache = ToolCallOutputCache::default();
        let (payload, append) = cache.consume("t1", "hello world").expect("should emit");
        assert_eq!(payload, "hello world");
        assert!(!append, "first emit must be replacement");
    }

    #[test]
    fn cache_repeated_identical_snapshot_is_noop() {
        let mut cache = ToolCallOutputCache::default();
        cache.consume("t1", "same").unwrap();
        assert!(
            cache.consume("t1", "same").is_none(),
            "identical snapshot must not emit"
        );
    }

    #[test]
    fn cache_prefix_extension_emits_suffix_with_append() {
        let mut cache = ToolCallOutputCache::default();
        cache.consume("t1", "line-1\n").unwrap();
        let (payload, append) = cache
            .consume("t1", "line-1\nline-2\n")
            .expect("should emit");
        assert_eq!(payload, "line-2\n");
        assert!(append, "prefix extension must emit with append=true");
    }

    #[test]
    fn cache_divergent_snapshot_falls_back_to_replace() {
        let mut cache = ToolCallOutputCache::default();
        cache.consume("t1", "hello world").unwrap();
        let (payload, append) = cache.consume("t1", "foo bar baz").expect("should emit");
        assert_eq!(payload, "foo bar baz");
        assert!(!append, "non-extension snapshot must replace");
    }

    #[test]
    fn cache_tracks_extensions_past_cached_tail_boundary() {
        // Regression test for the original bug: when cumulative raw_output
        // exceeds MAX_CACHED_TAIL_BYTES, subsequent extensions must still be
        // detectable by comparing the cached tail against the expected
        // offset in the incoming snapshot.
        let mut cache = ToolCallOutputCache::default();
        // First snapshot: 10 KB of 'a' + unique 4 KB marker at the end.
        let prefix = "a".repeat(10 * 1024);
        let marker = "M".repeat(4 * 1024);
        let first = format!("{prefix}{marker}");
        cache.consume("t1", &first).unwrap();

        // Second snapshot extends first by 16 KB of 'Z'.
        let delta = "Z".repeat(16 * 1024);
        let second = format!("{first}{delta}");
        let (payload, append) = cache.consume("t1", &second).expect("should emit");
        assert!(
            append,
            "extension beyond cached tail must still be detected"
        );
        // The emitted payload should carry the delta (or its tail when
        // truncated at MAX_SINGLE_EMIT_BYTES). For a 16 KB delta that's
        // well below the 64 KB cap, we expect it verbatim.
        assert_eq!(payload, delta);
    }

    #[test]
    fn cache_extension_larger_than_emit_cap_gets_truncated() {
        let mut cache = ToolCallOutputCache::default();
        cache.consume("t1", "seed").unwrap();
        // Build a delta much larger than MAX_SINGLE_EMIT_BYTES.
        let big_delta = "X".repeat(MAX_SINGLE_EMIT_BYTES * 2);
        let second = format!("seed{big_delta}");
        let (payload, append) = cache.consume("t1", &second).expect("should emit");
        assert!(append);
        assert!(
            payload.starts_with(TRUNCATION_MARKER),
            "oversized delta must be prefixed with truncation marker"
        );
        // Payload length: marker + at most MAX_SINGLE_EMIT_BYTES of tail.
        assert!(payload.len() <= TRUNCATION_MARKER.len() + MAX_SINGLE_EMIT_BYTES);
    }

    #[test]
    fn cache_respects_utf8_char_boundary_on_truncation() {
        let mut cache = ToolCallOutputCache::default();
        // Single first-update whose byte length forces truncation at a
        // position that would otherwise fall mid-codepoint. 中 is 3 bytes
        // (E4 B8 AD) and MAX_SINGLE_EMIT_BYTES (65536) is not a multiple
        // of 3, so naïve byte slicing would land mid-char.
        let chinese_block = "中".repeat((MAX_SINGLE_EMIT_BYTES / 3) + 100);
        let (payload, _append) = cache.consume("t1", &chinese_block).expect("should emit");
        // Payload must start with the truncation marker (since size > cap).
        assert!(
            payload.starts_with(TRUNCATION_MARKER),
            "oversized snapshot must be truncated"
        );
        // Body after the marker must be valid UTF-8 consisting only of 中.
        let body = &payload[TRUNCATION_MARKER.len()..];
        assert!(!body.is_empty());
        assert!(
            body.chars().all(|c| c == '中'),
            "truncation boundary must land on a UTF-8 codepoint edge"
        );
    }

    #[test]
    fn cache_final_status_clears_entry() {
        let mut cache = ToolCallOutputCache::default();
        cache.consume("t1", "hello").unwrap();
        assert!(cache.entries.contains_key("t1"));
        cache.remove_if_final("t1", Some("completed"));
        assert!(!cache.entries.contains_key("t1"));

        cache.consume("t2", "x").unwrap();
        cache.remove_if_final("t2", Some("cancelled"));
        assert!(!cache.entries.contains_key("t2"));

        cache.consume("t3", "x").unwrap();
        cache.remove_if_final("t3", Some("in_progress"));
        assert!(
            cache.entries.contains_key("t3"),
            "in-progress status must not clear cache"
        );
    }

    #[test]
    fn cache_enforces_entry_cap_via_fifo_eviction() {
        let mut cache = ToolCallOutputCache::default();
        for i in 0..(MAX_CACHE_ENTRIES + 50) {
            cache.consume(&format!("tool-{i}"), "body").unwrap();
        }
        assert_eq!(cache.entries.len(), MAX_CACHE_ENTRIES);
        // Oldest entries should have been evicted; newest must still exist.
        assert!(!cache.entries.contains_key("tool-0"));
        assert!(cache
            .entries
            .contains_key(&format!("tool-{}", MAX_CACHE_ENTRIES + 49)));
    }

    #[test]
    fn cache_seed_always_replaces_and_caches() {
        let mut cache = ToolCallOutputCache::default();
        cache.consume("t1", "stale").unwrap();
        // A hypothetical replay would send another ToolCall for the same
        // id — seed() must install the new snapshot without trying to
        // diff against the stale prior entry.
        let payload = cache.seed("t1", "fresh").expect("seed emits");
        assert_eq!(payload, "fresh");
        // Next consume should diff against "fresh", not "stale".
        let (p2, append) = cache.consume("t1", "fresh+more").expect("emit");
        assert!(append, "should detect extension of freshly seeded entry");
        assert_eq!(p2, "+more");
    }

    // ─── trim_partial_ansi_tail ─────────────────────────────────────────

    #[test]
    fn ansi_trim_leaves_pure_text_unchanged() {
        assert_eq!(trim_partial_ansi_tail("plain text"), "plain text");
    }

    #[test]
    fn ansi_trim_keeps_completed_sequences() {
        let s = "\x1b[31mRED\x1b[0m done";
        assert_eq!(trim_partial_ansi_tail(s), s);
    }

    #[test]
    fn ansi_trim_cuts_unterminated_trailing_sequence() {
        let s = "hello \x1b[31";
        assert_eq!(trim_partial_ansi_tail(s), "hello ");
    }

    #[test]
    fn ansi_trim_handles_bare_escape_at_end() {
        let s = "hello\x1b";
        assert_eq!(trim_partial_ansi_tail(s), "hello");
    }

    // ─── truncate_tail_at_char_boundary ─────────────────────────────────

    #[test]
    fn truncate_under_cap_returns_as_is() {
        assert_eq!(truncate_tail_at_char_boundary("abc", 10), "abc");
    }

    #[test]
    fn truncate_returns_tail_on_overflow() {
        assert_eq!(truncate_tail_at_char_boundary("abcdef", 3), "def");
    }

    #[test]
    fn truncate_respects_multibyte_utf8_boundary() {
        // "中中中" is 9 bytes; asking for 4 bytes would land mid-char.
        let s = "中中中";
        let out = truncate_tail_at_char_boundary(s, 4);
        // Must be valid UTF-8 (indexing an invalid boundary would have
        // panicked at slicing time).
        assert!(out.chars().all(|c| c == '中'));
        assert!(out.len() <= 6); // at most 2 chars (6 bytes)
    }

    // ─── is_subagent_invocation ─────────────────────────────────

    #[test]
    fn subagent_detects_opencode_with_subagent_type_regardless_of_title() {
        // OpenCode's ACP title is the user-facing description (e.g. the
        // task's `description` field), NOT the internal tool name. The
        // historical-parser equivalent at parsers/opencode.rs:425-429
        // anchors on `tool == "task"`, which we can't replicate here
        // because ACP doesn't expose the internal tool name — so we rely
        // solely on agent_type + subagent_type. Verify the detection
        // triggers regardless of the title shape.
        let input = Some(r#"{"subagent_type":"researcher","prompt":"x"}"#.to_string());
        assert!(is_subagent_invocation(AgentType::OpenCode, &input));
    }

    #[test]
    fn subagent_gates_on_supported_agent_types() {
        // OpenCode and CodeBuddy both rewrite a `subagent_type`-bearing call to
        // the Agent card; other agents stay excluded so a generic `subagent_type`
        // field never triggers a cross-agent collision.
        let input = Some(r#"{"subagent_type":"x"}"#.to_string());
        assert!(is_subagent_invocation(AgentType::OpenCode, &input));
        assert!(is_subagent_invocation(AgentType::CodeBuddy, &input));
        assert!(!is_subagent_invocation(AgentType::ClaudeCode, &input));
        assert!(!is_subagent_invocation(AgentType::Codex, &input));
    }

    #[test]
    fn subagent_rejects_empty_or_non_string_subagent_type() {
        for raw in [
            r#"{"subagent_type":""}"#,
            r#"{"subagent_type":null}"#,
            r#"{"subagent_type":42}"#,
            r#"{"subagent_type":["a"]}"#,
        ] {
            assert!(
                !is_subagent_invocation(AgentType::OpenCode, &Some(raw.to_string())),
                "expected false for raw_input={raw}"
            );
        }
    }

    #[test]
    fn subagent_rejects_none_malformed_or_non_object_root() {
        assert!(!is_subagent_invocation(AgentType::OpenCode, &None));
        for raw in [
            "not json",
            "{}",
            r#""string""#,
            "[1,2,3]",
            // Substring guard short-circuits this before JSON parsing;
            // verify both code paths agree on the result.
            "12345",
            // Field name present as substring but not as object key — the
            // substring guard lets this through but JSON parsing rejects
            // it (the value is a number, not an object with that key).
            r#"{"note":"contains the word subagent_type as text"}"#,
        ] {
            assert!(
                !is_subagent_invocation(AgentType::OpenCode, &Some(raw.to_string())),
                "expected false for raw_input={raw}"
            );
        }
    }

    #[test]
    fn subagent_rejects_when_subagent_type_appears_only_as_value() {
        // The cheap substring guard lets this through (the bytes
        // "subagent_type" appear in the JSON text), but JSON parsing
        // correctly finds no top-level `subagent_type` key, so the helper
        // returns false. Regression guard against any future "optimisation"
        // that conflates the substring check with the field check.
        let input = Some(r#"{"description":"use subagent_type=foo"}"#.to_string());
        assert!(!is_subagent_invocation(
            AgentType::OpenCode,
            &input
        ));
    }

    #[test]
    fn subagent_detects_when_raw_input_has_other_fields_ahead_of_subagent_type() {
        // Mirrors the OpenCode wire shape `{description, prompt, subagent_type}`
        // — the field order in JSON doesn't matter, but exercise a realistic
        // payload (with non-trivial sizes) end-to-end.
        let input = Some(
            r#"{"description":"Explore project structure","prompt":"Look at the repo layout and summarise the stack.","subagent_type":"general-purpose"}"#
                .to_string(),
        );
        assert!(is_subagent_invocation(AgentType::OpenCode, &input));
    }

    // ─── codebuddy_deferred_tool_name ────────────────────────────────────

    #[test]
    fn deferred_unwraps_codebuddy_mcp_tool_name() {
        // CodeBuddy wraps MCP calls as `{toolName, params}` via DeferExecuteTool.
        let input = Some(
            r#"{"params":{"agent_type":"codex","task":"build"},"toolName":"mcp__codeg-mcp__delegate_to_agent"}"#
                .to_string(),
        );
        assert_eq!(
            codebuddy_deferred_tool_name(AgentType::CodeBuddy, &input).as_deref(),
            Some("mcp__codeg-mcp__delegate_to_agent")
        );
    }

    #[test]
    fn deferred_gates_on_codebuddy_and_shape() {
        let wrapped = Some(
            r#"{"params":{"task_id":"a"},"toolName":"mcp__codeg-mcp__cancel_delegation"}"#
                .to_string(),
        );
        // Only CodeBuddy is unwrapped.
        assert!(codebuddy_deferred_tool_name(AgentType::OpenCode, &wrapped).is_none());
        // Missing `params`, missing/blank `toolName`, or non-wrapper shapes → None.
        for raw in [
            r#"{"toolName":"mcp__codeg-mcp__delegate_to_agent"}"#, // no params
            r#"{"params":{"x":1},"toolName":""}"#,                 // blank toolName
            r#"{"params":{"x":1}}"#,                               // no toolName
            r#"{"command":"ls"}"#,                                 // plain tool
            "not json",
        ] {
            assert!(
                codebuddy_deferred_tool_name(AgentType::CodeBuddy, &Some(raw.to_string())).is_none(),
                "expected None for raw_input={raw}"
            );
        }
        assert!(codebuddy_deferred_tool_name(AgentType::CodeBuddy, &None).is_none());
    }

    // ─── unwrap_codebuddy_deferred_output ────────────────────────────────

    #[test]
    fn deferred_output_peels_codebuddy_content_wrapper() {
        // The exact live shape from the bug report: a `get_delegation_status`
        // batch result double-wrapped as a `{text,type}` content part, whose
        // inner `text` is the compact `{tasks:[...]}` JSON. Peeling it yields the
        // bare report JSON the frontend `parseStatusReports` already understands.
        let inner = r#"{"tasks":[{"status":"completed","task_id":"666da381","child_conversation_id":18,"text":"ok"}]}"#;
        let wrapped = serde_json::json!({ "text": inner, "type": "text" }).to_string();
        assert_eq!(
            unwrap_codebuddy_deferred_output(AgentType::CodeBuddy, &wrapped).as_deref(),
            Some(inner)
        );
    }

    #[test]
    fn deferred_output_gates_on_codebuddy_and_wrapper_shape() {
        let wrapped =
            serde_json::json!({ "text": "{\"status\":\"running\"}", "type": "text" }).to_string();
        // Only CodeBuddy is unwrapped — the wrapper is a CodeBuddy quirk.
        assert!(unwrap_codebuddy_deferred_output(AgentType::OpenCode, &wrapped).is_none());
        assert!(unwrap_codebuddy_deferred_output(AgentType::ClaudeCode, &wrapped).is_none());
        for raw in [
            // Plain (non-deferred) tool output passes through untouched.
            "build succeeded",
            // A delegation report has no top-level `type` discriminator.
            r#"{"status":"completed","task_id":"x","text":"done"}"#,
            // A batch envelope is already in the bare shape — no `type` either.
            r#"{"tasks":[{"status":"completed","task_id":"x"}]}"#,
            // Wrong discriminator value.
            r#"{"type":"image","text":"x"}"#,
            // Missing inner `text`.
            r#"{"type":"text"}"#,
            "not json",
        ] {
            assert!(
                unwrap_codebuddy_deferred_output(AgentType::CodeBuddy, raw).is_none(),
                "expected pass-through (None) for output={raw}"
            );
        }
    }

    // ─── resolve_rewritten_title (title state across updates) ────────────

    #[test]
    fn rewritten_title_persists_across_status_only_updates() {
        let mut overrides: HashMap<String, String> = HashMap::new();
        let subagent = Some(
            r#"{"description":"Run pnpm build","subagent_type":"general-purpose"}"#.to_string(),
        );
        // Initial event carrying the subagent marker → "agent", recorded.
        assert_eq!(
            resolve_rewritten_title(AgentType::CodeBuddy, &subagent, "tc1", false, false, &mut overrides)
                .as_deref(),
            Some("agent")
        );
        // The bug: a later status-only update lost the marker (raw_input None).
        // The override must be RE-ASSERTED, not downgraded to the event's title.
        assert_eq!(
            resolve_rewritten_title(AgentType::CodeBuddy, &None, "tc1", true, false, &mut overrides)
                .as_deref(),
            Some("agent"),
            "a status-only update must not downgrade the Agent card mid-stream"
        );
        // Even an update whose raw_input looks like a different tool keeps it.
        let bash = Some(r#"{"command":"ls"}"#.to_string());
        assert_eq!(
            resolve_rewritten_title(AgentType::CodeBuddy, &bash, "tc1", true, false, &mut overrides)
                .as_deref(),
            Some("agent")
        );
        // A never-classified tool call returns None → caller uses its own title.
        assert_eq!(
            resolve_rewritten_title(AgentType::CodeBuddy, &None, "tc2", true, false, &mut overrides),
            None
        );
        // Deferred MCP tool: inner name recorded, then re-asserted on a bare update.
        let deferred = Some(
            r#"{"params":{"agent_type":"codex","task":"x"},"toolName":"mcp__codeg-mcp__delegate_to_agent"}"#
                .to_string(),
        );
        assert_eq!(
            resolve_rewritten_title(AgentType::CodeBuddy, &deferred, "tc3", false, false, &mut overrides)
                .as_deref(),
            Some("mcp__codeg-mcp__delegate_to_agent")
        );
        assert_eq!(
            resolve_rewritten_title(AgentType::CodeBuddy, &None, "tc3", true, false, &mut overrides)
                .as_deref(),
            Some("mcp__codeg-mcp__delegate_to_agent")
        );
        // Non-CodeBuddy agent with no prior classification: never rewritten.
        assert_eq!(
            resolve_rewritten_title(AgentType::OpenCode, &None, "tc9", true, false, &mut overrides),
            None
        );
    }

    // ─── codebuddy_meta_marks_subagent ───────────────────────────────────

    #[test]
    fn meta_marks_subagent_reads_codebuddy_keys() {
        // Any one of the three CodeBuddy sub-agent markers is sufficient.
        let tool_name = serde_json::json!({ "codebuddy.ai/toolName": "Agent" });
        let is_sub = serde_json::json!({ "codebuddy.ai/isSubagent": true });
        let sub_type = serde_json::json!({ "codebuddy.ai/subagentType": "general-purpose" });
        for meta in [&tool_name, &is_sub, &sub_type] {
            assert!(codebuddy_meta_marks_subagent(
                AgentType::CodeBuddy,
                meta.as_object()
            ));
        }
        // Gated on CodeBuddy: the generic `codebuddy.ai/*` keys can't classify
        // another agent.
        assert!(!codebuddy_meta_marks_subagent(
            AgentType::OpenCode,
            tool_name.as_object()
        ));
        // Negative shapes: non-Agent toolName, empty subagentType, absent meta.
        let other = serde_json::json!({
            "codebuddy.ai/toolName": "Bash",
            "codebuddy.ai/subagentType": "",
            "codebuddy.ai/isSubagent": false,
        });
        assert!(!codebuddy_meta_marks_subagent(
            AgentType::CodeBuddy,
            other.as_object()
        ));
        assert!(!codebuddy_meta_marks_subagent(AgentType::CodeBuddy, None));
    }

    #[test]
    fn rewritten_title_fires_on_meta_before_raw_input() {
        let mut overrides: HashMap<String, String> = HashMap::new();
        // Frame 1: `raw_input` has NO `subagent_type` yet, but `_meta` already
        // marks it (the early, reliable signal). Title must already be "agent".
        assert_eq!(
            resolve_rewritten_title(AgentType::CodeBuddy, &None, "tc1", false, true, &mut overrides)
                .as_deref(),
            Some("agent")
        );
        // Later sparse frames carry NEITHER signal — the override is re-asserted,
        // so the pill never flickers back to a generic tool mid-stream.
        assert_eq!(
            resolve_rewritten_title(AgentType::CodeBuddy, &None, "tc1", true, false, &mut overrides)
                .as_deref(),
            Some("agent"),
            "meta-classified Agent pill must stay 'agent' across signal-less frames"
        );
        // DeferExecuteTool still wins over the meta path (distinct mechanism).
        let deferred = Some(
            r#"{"params":{"agent_type":"codex","task":"x"},"toolName":"mcp__codeg-mcp__delegate_to_agent"}"#
                .to_string(),
        );
        assert_eq!(
            resolve_rewritten_title(
                AgentType::CodeBuddy,
                &deferred,
                "tc2",
                false,
                false,
                &mut overrides
            )
            .as_deref(),
            Some("mcp__codeg-mcp__delegate_to_agent")
        );
    }

    // ─── track_subagent_window / should_suppress_subagent_chunk ──────────

    #[test]
    fn subagent_window_opens_and_closes_by_status() {
        let mut open: HashSet<String> = HashSet::new();
        let mut closed: HashSet<String> = HashSet::new();
        let fg = false; // foreground (not background)
        // A non-final foreground agent frame opens the window.
        track_subagent_window(
            AgentType::CodeBuddy,
            true,
            fg,
            Some("in_progress"),
            "a",
            &mut open,
            &mut closed,
        );
        assert!(open.contains("a"));
        // A final frame closes it.
        track_subagent_window(
            AgentType::CodeBuddy,
            true,
            fg,
            Some("completed"),
            "a",
            &mut open,
            &mut closed,
        );
        assert!(!open.contains("a"));
        // A stray late non-final frame must NOT re-open a finished sub-agent.
        track_subagent_window(
            AgentType::CodeBuddy,
            true,
            fg,
            Some("in_progress"),
            "a",
            &mut open,
            &mut closed,
        );
        assert!(!open.contains("a"), "completed sub-agent must not re-open");
        // Non-agent tool calls never enter the window.
        track_subagent_window(
            AgentType::CodeBuddy,
            false,
            fg,
            Some("in_progress"),
            "b",
            &mut open,
            &mut closed,
        );
        assert!(!open.contains("b"));
        // Other agents are inert.
        track_subagent_window(
            AgentType::OpenCode,
            true,
            fg,
            Some("in_progress"),
            "c",
            &mut open,
            &mut closed,
        );
        assert!(!open.contains("c"));
    }

    #[test]
    fn subagent_window_excludes_background_subagents() {
        // A BACKGROUND sub-agent runs concurrently with the main agent, so it must
        // never open the suppression window — otherwise interleaved MAIN-agent
        // chunks would be wrongly dropped (the case the reviewer flagged).
        let mut open: HashSet<String> = HashSet::new();
        let mut closed: HashSet<String> = HashSet::new();
        track_subagent_window(
            AgentType::CodeBuddy,
            true,
            true, // is_background
            Some("in_progress"),
            "bg",
            &mut open,
            &mut closed,
        );
        assert!(
            !open.contains("bg"),
            "a background sub-agent must not open the window"
        );
        // And once known-background, a later (still non-final, no-longer-marked)
        // frame must not re-open it either.
        track_subagent_window(
            AgentType::CodeBuddy,
            true,
            false,
            Some("in_progress"),
            "bg",
            &mut open,
            &mut closed,
        );
        assert!(
            !open.contains("bg"),
            "a sub-agent seen as background must stay excluded"
        );
    }

    #[test]
    fn suppress_subagent_chunk_by_window_or_chunk_meta() {
        // Inside an open FOREGROUND window → suppress. This is safe because the
        // window only ever holds foreground (blocking) sub-agents, during which
        // the parent model is suspended — so every chunk in the window is the
        // sub-agent's, never main-agent output (background sub-agents, which could
        // interleave main output, are excluded from the window upstream).
        assert!(should_suppress_subagent_chunk(AgentType::CodeBuddy, true, None));
        // Window closed and no chunk meta → emit (e.g. main-agent text before the
        // sub-agent opens or after it closes).
        assert!(!should_suppress_subagent_chunk(
            AgentType::CodeBuddy,
            false,
            None
        ));
        // Window closed but the chunk's own meta marks it → suppress (precision
        // supplement; never relied on alone).
        let sub = serde_json::json!({ "codebuddy.ai/isSubagent": true });
        let parented = serde_json::json!({ "codebuddy.ai/parentToolCallId": "call_x" });
        for meta in [&sub, &parented] {
            assert!(should_suppress_subagent_chunk(
                AgentType::CodeBuddy,
                false,
                meta.as_object()
            ));
        }
        // Other agents never suppress, even inside a (spurious) open window.
        assert!(!should_suppress_subagent_chunk(AgentType::OpenCode, true, None));
    }

    #[test]
    fn claude_chunk_parent_reads_only_wellformed_claude_meta() {
        let valid = serde_json::json!({ "claudeCode": { "parentToolUseId": "toolu_01A" } });
        assert_eq!(
            claude_chunk_parent_tool_use_id(AgentType::ClaudeCode, valid.as_object()),
            Some("toolu_01A".to_string())
        );
        // Gated on ClaudeCode — the same meta on another agent must not alias
        // into parented routing.
        assert_eq!(
            claude_chunk_parent_tool_use_id(AgentType::CodeBuddy, valid.as_object()),
            None
        );
        // Absent meta / absent key / wrong type / empty string → None.
        assert_eq!(
            claude_chunk_parent_tool_use_id(AgentType::ClaudeCode, None),
            None
        );
        let no_key = serde_json::json!({ "claudeCode": { "toolName": "Agent" } });
        assert_eq!(
            claude_chunk_parent_tool_use_id(AgentType::ClaudeCode, no_key.as_object()),
            None
        );
        let wrong_type = serde_json::json!({ "claudeCode": { "parentToolUseId": 42 } });
        assert_eq!(
            claude_chunk_parent_tool_use_id(AgentType::ClaudeCode, wrong_type.as_object()),
            None
        );
        let empty = serde_json::json!({ "claudeCode": { "parentToolUseId": "" } });
        assert_eq!(
            claude_chunk_parent_tool_use_id(AgentType::ClaudeCode, empty.as_object()),
            None
        );
    }

    #[test]
    fn meta_marks_background_reads_codebuddy_flag() {
        let bg = serde_json::json!({ "codebuddy.ai/isBackground": true });
        let fg = serde_json::json!({ "codebuddy.ai/isBackground": false });
        assert!(codebuddy_meta_marks_background(
            AgentType::CodeBuddy,
            bg.as_object()
        ));
        // Foreground (the user-reported case), absent flag, and other agents → false.
        assert!(!codebuddy_meta_marks_background(
            AgentType::CodeBuddy,
            fg.as_object()
        ));
        assert!(!codebuddy_meta_marks_background(AgentType::CodeBuddy, None));
        assert!(!codebuddy_meta_marks_background(
            AgentType::OpenCode,
            bg.as_object()
        ));
    }

    // ─── inject_codeg_mcp: enabled=false short-circuit ──────────
    //
    // Guards the "default off" product contract: when the broker config has
    // `enabled: false` (the new production default for fresh installs), the
    // delegate-MCP injection must not push a server entry and must not
    // register a per-launch token. The early return at the top of
    // `inject_codeg_mcp` is the single chokepoint that keeps a
    // codeg-mcp stdio MCP out of every ACP session until the user
    // opts in via the settings panel.
    #[tokio::test]
    async fn inject_codeg_delegate_skipped_when_broker_disabled() {
        use crate::acp::delegation::broker::{ConversationDepthLookup, DelegationBroker};
        use crate::acp::delegation::listener::TokenRegistry;
        use crate::acp::delegation::spawner::{mock::MockSpawner, ConnectionSpawner};
        use crate::acp::delegation::types::DelegationError;

        struct EmptyLookup;
        #[async_trait::async_trait]
        impl ConversationDepthLookup for EmptyLookup {
            async fn parent_of(&self, _id: i32) -> Result<Option<i32>, DelegationError> {
                Ok(None)
            }
        }

        let broker = Arc::new(DelegationBroker::new(
            Arc::new(MockSpawner::default()) as Arc<dyn ConnectionSpawner>,
            Arc::new(EmptyLookup) as Arc<dyn ConversationDepthLookup>,
        ));
        // No set_config call: broker carries its default config, which is
        // `enabled: false` after the product-default flip. This is the
        // exact state a fresh install reaches before the user touches the
        // settings panel. Feedback is likewise disabled by default, so with
        // BOTH features off the companion isn't injected at all.
        struct NoQuestions;
        #[async_trait::async_trait]
        impl crate::acp::question::SessionQuestionAccess for NoQuestions {
            async fn register_question(
                &self,
                _parent_connection_id: &str,
                _questions: Vec<crate::acp::question::QuestionSpec>,
            ) -> Option<crate::acp::question::RegisteredQuestion> {
                None
            }
            async fn cancel_question(&self, _parent_connection_id: &str, _question_id: &str) {}
            async fn cancel_questions_by_parent(&self, _parent_connection_id: &str) {}
        }
        struct NoPlanApprovals;
        #[async_trait::async_trait]
        impl crate::acp::plan_approval::SessionPlanApprovalAccess for NoPlanApprovals {
            async fn register_plan_approval(
                &self,
                _parent_connection_id: &str,
                _tool_call_id: String,
                _plan_markdown: String,
            ) -> Option<crate::acp::plan_approval::RegisteredPlanApproval> {
                None
            }
            async fn cancel_plan_approvals_by_parent(&self, _parent_connection_id: &str) {}
        }
        struct AllEnabled;
        #[async_trait::async_trait]
        impl AgentAvailabilityLookup for AllEnabled {
            async fn disabled_agent_wire_slugs(&self) -> Vec<String> {
                Vec::new()
            }
        }
        let injection = DelegationInjection {
            broker,
            tokens: Arc::new(TokenRegistry::default()),
            socket_path: std::path::PathBuf::from("/tmp/codeg-mcp.sock"),
            agent_availability: Arc::new(AllEnabled) as Arc<dyn AgentAvailabilityLookup>,
            feedback: crate::acp::feedback::FeedbackRuntimeConfig::new(),
            ask: crate::acp::question::QuestionRuntimeConfig::new(),
            sessions: crate::acp::session_info::SessionInfoRuntimeConfig::new(),
            authoring: crate::acp::chat_authoring::ChatAuthoringRuntimeConfig::new(),
            questions: Arc::new(NoQuestions)
                as Arc<dyn crate::acp::question::SessionQuestionAccess>,
            plan_approvals: Arc::new(NoPlanApprovals)
                as Arc<dyn crate::acp::plan_approval::SessionPlanApprovalAccess>,
        };

        let mut servers: Vec<McpServer> = Vec::new();
        let result = inject_codeg_mcp(
            &mut servers,
            &injection,
            "parent-conn",
            std::path::Path::new("/tmp"),
            false,
            HostToolsPolicy::Default,
        )
        .await;

        assert!(result.is_none(), "disabled broker must return None");
        assert!(
            servers.is_empty(),
            "disabled broker must not push any MCP server entry; got {servers:?}"
        );
        // Token registry stays untouched — no lookup should resolve to a
        // valid entry because nothing was registered.
        assert!(
            injection.tokens.lookup("any-token").await.is_none(),
            "disabled broker must not register a delegate token"
        );
    }

    // ─── delegate_target_args: enable-toggle filtering ──────────
    //
    // The companion's delegate enum must only advertise launchable targets:
    // enabled customs are appended, disabled customs are never appended (no
    // subtraction entry either), and disabled builtins ride the sorted
    // `--disabled-agents` list for companion-side subtraction.
    #[test]
    fn delegate_target_args_filter_disabled_agents() {
        use crate::acp::custom_registry::{
            hydrate, hydrate_test_guard, CustomAgentDef, CustomAgentSpec, CustomDistributionKind,
            NpxSpec,
        };
        let _guard = hydrate_test_guard();
        let def = |id: &str| CustomAgentDef {
            registry_id: id.into(),
            name: id.into(),
            description: String::new(),
            version: "1.0.0".into(),
            distribution_kind: CustomDistributionKind::Npx,
            spec: CustomAgentSpec {
                npx: Some(NpxSpec {
                    package: format!("{id}@1.0.0"),
                    ..Default::default()
                }),
                ..Default::default()
            },
            icon_url: None,
            skills_shared_store: false,
            skills_dir: None,
            source: Default::default(),
            version_probe: None,
            supports_mcp: true,
        };
        assert!(hydrate(&[def("delegate-on"), def("delegate-off")]).is_empty());

        let disabled = vec![
            "grok".to_string(),
            "codex".to_string(),
            "custom:delegate-off".to_string(),
        ];
        let (custom_slugs, disabled_builtins) = delegate_target_args(&disabled);
        assert_eq!(custom_slugs, vec!["custom:delegate-on".to_string()]);
        assert_eq!(
            disabled_builtins,
            vec!["codex".to_string(), "grok".to_string()],
            "builtins only, sorted for a deterministic arg string"
        );

        // Nothing disabled → both flags stay omitted (customs all advertised).
        let (custom_slugs, disabled_builtins) = delegate_target_args(&[]);
        assert_eq!(
            custom_slugs,
            vec![
                "custom:delegate-off".to_string(),
                "custom:delegate-on".to_string()
            ]
        );
        assert!(disabled_builtins.is_empty());

        hydrate(&[]);
    }

    #[test]
    fn host_tools_agent_also_withholds_the_delegation_group() {
        // `delegate_to_agent` is the third door into the room `fs/*` and
        // `terminal/*` open: it has codeg spawn a SECOND agent, in codeg's
        // process tree under that agent's own policy, and relays its output
        // back. Without this gate a sandboxed agent that cannot read `.env`
        // itself just asks a sibling to read it, and the switch's promise is
        // false. This pins the exact boolean `inject_codeg_mcp` computes.
        let delegation_for = |broker_enabled: bool, host_tools: HostToolsPolicy| {
            broker_enabled && host_tools.hosts_channels()
        };
        assert!(delegation_for(true, HostToolsPolicy::Default));
        assert!(!delegation_for(true, HostToolsPolicy::Agent));
        // The settings toggle still wins when it is the one saying no.
        assert!(!delegation_for(false, HostToolsPolicy::Default));

        // With delegation the only enabled group, withholding it must skip the
        // companion entirely rather than launch it with an empty `--features`
        // (which `CompanionFeatures::parse` would see as absent and default
        // back to delegation-only — re-opening the hole).
        let mut flags = CompanionFeatureFlags {
            delegation: delegation_for(true, HostToolsPolicy::Agent),
            ..CompanionFeatureFlags::default()
        };
        assert_eq!(companion_features_arg(flags), None);

        // But the groups that only surface codeg's OWN state keep working —
        // they execute nothing on the user's machine, so withholding them
        // would cost function for no boundary.
        flags.ask = true;
        flags.feedback = true;
        assert_eq!(
            companion_features_arg(flags),
            Some("feedback,ask".to_string())
        );
    }

    // ─── companion_features_arg: inject/skip decision + --features value ──
    //
    // The companion now carries two independently-toggled tool groups. It is
    // injected when EITHER is on, and the `--features` arg names exactly the
    // enabled groups so the companion hides the rest. Crucially, feedback alone
    // must still inject the companion (the historical delegation-only gate would
    // have skipped it).
    #[test]
    fn companion_features_arg_inject_skip_decision() {
        let only = |f: fn(&mut CompanionFeatureFlags)| {
            let mut flags = CompanionFeatureFlags::default();
            f(&mut flags);
            companion_features_arg(flags)
        };
        // All off → no companion at all.
        assert_eq!(
            companion_features_arg(CompanionFeatureFlags::default()),
            None
        );
        // Delegation only.
        assert_eq!(
            only(|f| f.delegation = true),
            Some("delegation".to_string())
        );
        // Feedback only — the decoupling: companion injected for feedback even
        // when delegation is off.
        assert_eq!(only(|f| f.feedback = true), Some("feedback".to_string()));
        // Ask only — likewise injects the companion on its own.
        assert_eq!(only(|f| f.ask = true), Some("ask".to_string()));
        // Sessions only — likewise injects the companion on its own.
        assert_eq!(only(|f| f.sessions = true), Some("sessions".to_string()));
        // Per-spawn tasks group: injects alone.
        assert_eq!(only(|f| f.tasks = true), Some("tasks".to_string()));
        // Each chat-authoring group injects the companion on its own too, so a
        // user who only wants "create a task from chat" still gets the tool.
        assert_eq!(
            only(|f| f.automations = true),
            Some("automations".to_string())
        );
        assert_eq!(only(|f| f.taskboard = true), Some("taskboard".to_string()));
        // All on → comma-joined, in the order the companion parses.
        assert_eq!(
            companion_features_arg(CompanionFeatureFlags {
                delegation: true,
                feedback: true,
                ask: true,
                sessions: true,
                tasks: true,
                automations: true,
                taskboard: true,
            }),
            Some("delegation,feedback,ask,sessions,tasks,automations,taskboard".to_string())
        );
    }

    // ── Boolean config options (cline 3.0.50 `auto_approve`) ──

    /// The exact `configOptions` entry cline 3.0.50 ships. Before
    /// `unstable_boolean_config` was enabled this failed to deserialize with
    /// `unknown variant 'boolean', expected 'select'` — and because
    /// `SessionConfigOption::kind` is a required flattened field, that one entry
    /// failed the WHOLE `session/new` response and left cline unusable.
    fn cline_auto_approve_json() -> serde_json::Value {
        serde_json::json!({
            "type": "boolean",
            "id": "auto_approve",
            "name": "Auto-approve tools",
            "description": "Automatically approve all tool calls without asking for permission",
            "currentValue": false
        })
    }

    #[test]
    fn cline_boolean_config_option_maps_to_a_toggle() {
        let option: SessionConfigOption =
            serde_json::from_value(cline_auto_approve_json()).expect("boolean kind must parse");

        let mapped = map_session_config_option(&option).expect("boolean options are surfaced");
        assert_eq!(mapped.id, "auto_approve");
        assert_eq!(mapped.name, "Auto-approve tools");
        match mapped.kind {
            SessionConfigKindInfo::Boolean(toggle) => assert!(!toggle.current_value),
            other => panic!("expected a boolean kind, got {other:?}"),
        }
    }

    /// The whole point of the fix: one boolean option must not take the session
    /// down with it.
    #[test]
    fn new_session_response_with_a_boolean_option_parses() {
        let raw = serde_json::json!({
            "sessionId": "sess-1",
            "configOptions": [
                {
                    "type": "select",
                    "id": "model",
                    "name": "Model",
                    "currentValue": "anthropic/claude-sonnet-5",
                    "options": [{"value": "anthropic/claude-sonnet-5", "name": "Claude Sonnet 5"}]
                },
                cline_auto_approve_json(),
            ]
        });
        let resp: NewSessionResponse = serde_json::from_value(raw).expect("must parse");
        assert_eq!(resp.config_options.map(|o| o.len()), Some(2));
    }

    #[test]
    fn select_values_keep_their_pre_boolean_wire_shape() {
        // Regression guard for every agent that is NOT cline: enabling
        // `unstable_boolean_config` changed `SetSessionConfigOptionRequest.value`
        // from a plain `SessionConfigValueId` to a flattened enum. The untagged
        // `ValueId` variant must still serialize to a bare `"value"` string, or
        // codex / claude / opencode model switching silently breaks.
        let req = SetSessionConfigOptionRequest::new(
            SessionId::new("sess-1"),
            SessionConfigId::new("model"),
            encode_config_option_value(false, "anthropic/claude-sonnet-5"),
        );
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            serde_json::json!({
                "sessionId": "sess-1",
                "configId": "model",
                "value": "anthropic/claude-sonnet-5"
            })
        );
    }

    #[test]
    fn boolean_values_carry_a_type_discriminator() {
        let on = SetSessionConfigOptionRequest::new(
            SessionId::new("sess-1"),
            SessionConfigId::new("auto_approve"),
            encode_config_option_value(true, "true"),
        );
        assert_eq!(
            serde_json::to_value(&on).unwrap(),
            serde_json::json!({
                "sessionId": "sess-1",
                "configId": "auto_approve",
                "type": "boolean",
                "value": true
            })
        );
        // Anything that is not the literal "true" is off — the selector only
        // ever emits "true"/"false".
        assert_eq!(
            encode_config_option_value(true, "false").as_bool(),
            Some(false)
        );
    }

    #[test]
    fn unknown_config_option_kinds_are_stripped_not_fatal() {
        let mut raw = serde_json::json!({
            "sessionId": "sess-1",
            "modes": null,
            "configOptions": [
                {
                    "type": "select",
                    "id": "model",
                    "name": "Model",
                    "currentValue": "m1",
                    "options": [{"value": "m1", "name": "M1"}]
                },
                cline_auto_approve_json(),
                // A kind newer than codeg's schema pin — today this is what
                // `boolean` was yesterday.
                {"type": "radio", "id": "future", "name": "Future", "currentValue": "a"},
            ]
        });
        strip_unknown_config_options(&mut raw, "session/new");

        let ids: Vec<&str> = raw["configOptions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["model", "auto_approve"], "only `radio` is dropped");
        // Untouched siblings survive, and the response still parses.
        assert_eq!(raw["sessionId"], "sess-1");
        serde_json::from_value::<NewSessionResponse>(raw).expect("parses after stripping");
    }

    /// `session/new` and `session/load` are now sent as `UntypedMessage`s for
    /// EVERY agent (so the raw response can be sanitized first), which means the
    /// literal method strings are load-bearing for all of them rather than just
    /// Grok. The schema's own constants are `pub(crate)`, but it re-exports them
    /// through `AGENT_METHOD_NAMES` — pin against that so a rename upstream is a
    /// failing test, not every agent silently getting "method not found".
    #[test]
    fn untyped_session_method_names_match_the_schema() {
        use sacp::schema::AGENT_METHOD_NAMES;
        assert_eq!(AGENT_METHOD_NAMES.session_new, "session/new");
        assert_eq!(AGENT_METHOD_NAMES.session_load, "session/load");
        assert_eq!(
            AGENT_METHOD_NAMES.session_set_config_option,
            "session/set_config_option"
        );
    }

    /// The untyped send must put the same params on the wire the typed send did
    /// — `UntypedMessage::new` runs the very same `serde_json::to_value` on the
    /// request, so this pins the payload rather than the mechanism.
    #[test]
    fn untyped_new_session_carries_the_typed_request_payload() {
        let cwd = std::path::PathBuf::from("/tmp/codeg");
        let req = build_new_session_request(AgentType::Cline, &cwd, Vec::new());
        let expected = serde_json::to_value(&req).unwrap();

        let untyped = UntypedMessage::new("session/new", req).expect("builds");
        assert_eq!(untyped.method(), "session/new");
        assert_eq!(untyped.params(), &expected);
    }

    #[test]
    fn saved_boolean_preference_skips_the_redundant_round_trip() {
        let off: SessionConfigOption =
            serde_json::from_value(cline_auto_approve_json()).expect("parses");
        assert!(config_option_already_holds(&off, "false"));
        assert!(!config_option_already_holds(&off, "true"));

        let mut on_json = cline_auto_approve_json();
        on_json["currentValue"] = serde_json::json!(true);
        let on: SessionConfigOption = serde_json::from_value(on_json).expect("parses");
        assert!(config_option_already_holds(&on, "true"));
        assert!(!config_option_already_holds(&on, "false"));
    }

    #[test]
    fn strip_leaves_responses_without_config_options_alone() {
        // `session/new` responses from agents that publish no selectors at all,
        // and entries with no `type`, must pass through untouched — serde gives
        // a better error for a malformed entry than a silent drop would.
        let mut none = serde_json::json!({"sessionId": "sess-1"});
        let before = none.clone();
        strip_unknown_config_options(&mut none, "session/new");
        assert_eq!(none, before);

        let mut untyped = serde_json::json!({"configOptions": [{"id": "weird"}]});
        strip_unknown_config_options(&mut untyped, "session/new");
        assert_eq!(untyped["configOptions"].as_array().unwrap().len(), 1);
    }
}
