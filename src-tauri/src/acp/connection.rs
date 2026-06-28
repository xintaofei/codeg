use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sacp::schema::{
    BlobResourceContents, CancelNotification, ClientCapabilities, ContentBlock, ContentChunk,
    CreateTerminalRequest, CreateTerminalResponse, EmbeddedResource, EmbeddedResourceResource,
    FileSystemCapabilities, ImageContent, InitializeRequest, KillTerminalRequest,
    KillTerminalResponse, LoadSessionRequest, NewSessionRequest, NewSessionResponse,
    PermissionOptionKind, Plan, PlanEntryPriority, PlanEntryStatus, PromptRequest, ProtocolVersion,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, ResourceLink,
    ResumeSessionRequest, ResumeSessionResponse, SelectedPermissionOutcome, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectGroup, SessionConfigSelectOption, SessionConfigSelectOptions, SessionId,
    SessionModeState, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, StopReason, TerminalExitStatus,
    TerminalOutputRequest, TerminalOutputResponse, TextContent, TextResourceContents,
    ToolCallContent, WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use sacp::schema::{HttpHeader, McpServer, McpServerHttp, McpServerSse, McpServerStdio};
use sacp::util::MatchDispatch;
use sacp::{
    on_receive_request, Agent, Client, ConnectionTo, Dispatch, Responder, SessionMessage,
    UntypedMessage,
};
use sacp_tokio::AcpAgent;
use tokio::sync::{mpsc, RwLock};

use crate::acp::error::AcpError;
use crate::acp::file_system_runtime::{FileSystemRuntime, FileSystemRuntimeError};
use crate::acp::registry::{self, AgentDistribution};
use crate::acp::session_state::SessionState;
use crate::acp::terminal_runtime::{TerminalRuntime, TerminalRuntimeError};
use crate::acp::types::{
    AcpEvent, AvailableCommandInfo, ConnectionInfo, ConnectionStatus, PermissionOptionInfo,
    PlanEntryInfo, PromptCapabilitiesInfo, PromptInputBlock, SessionConfigKindInfo,
    SessionConfigOptionInfo, SessionConfigSelectGroupInfo, SessionConfigSelectInfo,
    SessionConfigSelectOptionInfo, SessionModeInfo, SessionModeStateInfo, ToolCallImageInfo,
    UserMessageBlock,
};
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
    Cancel,
    RespondPermission {
        request_id: String,
        option_id: String,
    },
    Fork {
        reply:
            tokio::sync::oneshot::Sender<Result<crate::acp::types::ForkProtocolResult, AcpError>>,
    },
    Disconnect,
}

/// Sentinel string embedded in a `sacp::Error` when the Initialize
/// handshake times out. Converted back to `AcpError::InitializeTimeout`
/// by the outer `.map_err(...)` in `run_connection`.
const INIT_TIMEOUT_SENTINEL: &str = "__codeg_init_timeout__";

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

async fn build_agent(
    agent_type: AgentType,
    runtime_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<AcpAgent, AcpError> {
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
                // Trust the workspace codeg is launching pi into (default on, via
                // the PI_ACP_TRUST_WORKSPACE env_json key) so pi loads the
                // project's local config/skills without a redundant prompt. Gates
                // config loading only, never execution; scoped, additive, and
                // best-effort (never blocks the connect).
                crate::commands::acp::seed_pi_workspace_trust(cwd, runtime_env);
            }
            let mut merged_env = merge_agent_env(env, runtime_env);
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
            AcpAgent::from_args(&refs)
                .map(|a| {
                    a.with_debug(move |line, dir| {
                        if dir == sacp_tokio::LineDirection::Stderr {
                            tracing::debug!("[ACP][{agent_name}][stderr] {line}");
                        }
                    })
                })
                .map_err(|e| AcpError::SpawnFailed(e.to_string()))
        }
        AgentDistribution::Binary {
            version: registry_version,
            cmd,
            args,
            env,
            platforms,
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
            // INVARIANT: the substring "is not installed" is matched
            // verbatim by the frontend catch block in
            // `src/contexts/acp-connections-context.tsx` to surface a
            // localized install prompt. Do not change the wording.
            let (binary_path, cached_version) =
                crate::acp::binary_cache::find_best_cached_binary_for_agent(agent_type, cmd)?
                    .ok_or_else(|| {
                        AcpError::SdkNotInstalled(format!(
                            "{} is not installed. Please install it in Agent Settings.",
                            meta.name
                        ))
                    })?;
            if cached_version == registry_version {
                tracing::info!("[ACP][{}] Using cached binary {cached_version}", meta.name);
            } else {
                tracing::info!(
                    "[ACP][{}] Using cached binary {cached_version} (registry recommends {registry_version})",
                    meta.name
                );
            }

            let binary_str = binary_path.to_string_lossy().to_string();
            let binary_size = std::fs::metadata(&binary_path)
                .map(|m| m.len())
                .unwrap_or(0);
            let mut server = McpServerStdio::new(meta.name, &binary_str);
            let cmd_args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
            let cmd_args_for_log = cmd_args.clone();
            if !cmd_args.is_empty() {
                server = server.args(cmd_args);
            }
            let merged_env = merge_agent_env(env, runtime_env);
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
            Ok(
                AcpAgent::new(sacp::schema::McpServer::Stdio(server)).with_debug(
                    move |line, dir| {
                        let (tag, enabled) = match dir {
                            sacp_tokio::LineDirection::Stderr => ("stderr", true),
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
                    },
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
                // Fallback: the agent's own CLI is already on PATH (e.g.
                // `hermes acp`), installed via its official installer rather
                // than provisioned through uvx.
                tracing::warn!(
                    "[ACP][{}] uvx unavailable; falling back to system command {:?}",
                    meta.name, sys_path
                );
                // `system_cmd` is a complete launch recipe for the PATH binary;
                // the uvx entry-script `args` don't necessarily apply to it
                // (for Hermes both are empty / `["acp"]`, so this is exact).
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
            AcpAgent::from_args(&refs)
                .map(|a| {
                    a.with_debug(move |line, dir| {
                        if dir == sacp_tokio::LineDirection::Stderr {
                            tracing::debug!("[ACP][{agent_name}][stderr] {line}");
                        }
                    })
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
    let agent = build_agent(agent_type, &runtime_env, &launch_cwd).await?;

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
        },
    );

    tokio::spawn(async move {
        // RAII guard: runs on normal exit AND on panic unwinding, so a
        // panic inside `run_connection` can't leak a stale map entry.
        let _cleanup = ConnectionCleanupGuard {
            connections: cleanup_connections,
            connection_id: cleanup_connection_id,
        };

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
            preferred_mode_id,
            preferred_config_values,
            delegation_injection,
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
        // `_cleanup` is dropped here — removes the connection entry from
        // the manager map. Same drop semantics apply on panic unwinding.
    });

    Ok(session_started_rx)
}

/// Shared state for pending permission responders.
type PendingPermissions =
    Arc<tokio::sync::Mutex<HashMap<String, Responder<RequestPermissionResponse>>>>;

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
        _ => None,
    }
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

async fn emit_prompt_capabilities(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    capabilities: &sacp::schema::PromptCapabilities,
) {
    emit_with_state(
        state,
        emitter,
        AcpEvent::PromptCapabilities {
            prompt_capabilities: PromptCapabilitiesInfo {
                image: capabilities.image,
                audio: capabilities.audio,
                embedded_context: capabilities.embedded_context,
            },
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
) -> Result<ResumeSessionResponse, sacp::Error> {
    let untyped_req = UntypedMessage::new("session/resume", req).map_err(|e| {
        sacp::util::internal_error(format!("Failed to build resume request: {e}"))
    })?;

    let raw_response = cx.send_request_to(Agent, untyped_req).block_task().await?;
    serde_json::from_value(raw_response).map_err(|e| {
        sacp::util::internal_error(format!("Failed to parse resume response: {e}"))
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
    // Hermes and Kimi Code each read their own native MCP config at launch —
    // Hermes from `~/.hermes/config.yaml` (`mcp_servers`, registered as
    // `mcp-<name>` toolsets), Kimi Code from `~/.kimi-code/mcp.json`
    // (`mcpServers`). codeg manages those files directly via the MCP settings
    // UI, so forwarding the same servers over the ACP wire here would
    // double-register them — skip it. (The built-in `codeg-mcp` companion is
    // injected separately by `inject_codeg_mcp`, so it still reaches them.)
    if matches!(agent_type, AgentType::Hermes | AgentType::KimiCode) {
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
#[derive(Clone)]
pub struct DelegationInjection {
    pub broker: Arc<crate::acp::delegation::broker::DelegationBroker>,
    pub tokens: Arc<crate::acp::delegation::listener::TokenRegistry>,
    pub socket_path: PathBuf,
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
    /// Question registry handle for the teardown cascade. The `run_connection`
    /// cleanup guard calls `cancel_questions_by_parent` through this so a pending
    /// `ask_user_question` is reclaimed synchronously on disconnect, mirroring
    /// the delegation `broker.cancel_by_parent` cleanup. Shares the same backing
    /// `ConnectionManager` as the listener's question lookup.
    pub questions: Arc<dyn crate::acp::question::SessionQuestionAccess>,
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
/// The `--features` value for a companion launch given the four feature flags,
/// or `None` when none is enabled (the companion isn't injected at all).
/// Pulled out as a pure function so the inject/skip decision is unit-testable
/// without a real binary on disk or a live broker.
fn companion_features_arg(
    delegation_enabled: bool,
    feedback_enabled: bool,
    ask_enabled: bool,
    sessions_enabled: bool,
) -> Option<String> {
    if !delegation_enabled && !feedback_enabled && !ask_enabled && !sessions_enabled {
        return None;
    }
    let mut features: Vec<&str> = Vec::new();
    if delegation_enabled {
        features.push("delegation");
    }
    if feedback_enabled {
        features.push("feedback");
    }
    if ask_enabled {
        features.push("ask");
    }
    if sessions_enabled {
        features.push("sessions");
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
) -> Option<CompanionInjection> {
    // codeg-mcp carries BOTH the delegation tools and the live-feedback tool.
    // Inject it when EITHER feature is enabled; the `--features` arg tells the
    // companion which tool groups to expose so a disabled feature's tools never
    // surface to the LLM. (Historically this was gated on delegation alone.)
    let delegation_enabled = injection.broker.config_snapshot().await.enabled;
    let feedback_enabled = injection.feedback.is_enabled().await;
    let ask_enabled = injection.ask.is_enabled().await;
    let sessions_enabled = injection.sessions.is_enabled().await;
    // `None` (no feature enabled) short-circuits the whole injection.
    let features_arg = companion_features_arg(
        delegation_enabled,
        feedback_enabled,
        ask_enabled,
        sessions_enabled,
    )?;
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
    server = server.args(vec![
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
        // Tool groups to expose this launch (delegation / feedback / ask / sessions).
        "--features".to_string(),
        features_arg,
    ]);
    servers.push(McpServer::Stdio(server));
    Some(CompanionInjection {
        token,
        feedback_available: feedback_enabled,
    })
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
    preferred_mode_id: Option<String>,
    preferred_config_values: BTreeMap<String, String>,
    delegation_injection: Option<DelegationInjection>,
) -> Result<(), AcpError> {
    let pending_perms: PendingPermissions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    // `terminal_base_env` already filtered to just the credential helper
    // keys upstream — see `spawn_agent_connection` for the rationale and
    // why we don't forward the full agent runtime_env here.
    let cwd = resolve_working_dir(working_dir.as_deref());
    // Default terminals to the session working directory so an agent that calls
    // `terminal/create` without a `cwd` (e.g. CodeBuddy) runs in the folder the
    // conversation runs in rather than codeg's own process cwd.
    let terminal_runtime = Arc::new(
        TerminalRuntime::with_base_env(terminal_base_env).with_default_cwd(Some(cwd.clone())),
    );
    let cwd_string = cwd.to_string_lossy().to_string();
    let file_system_runtime = Arc::new(FileSystemRuntime::new(cwd.clone()));

    let conn_id = connection_id.clone();
    let emitter_clone = emitter.clone();
    let perms = pending_perms.clone();
    let state_outer = Arc::clone(&state);

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
                            _cx: ConnectionTo<Agent>| {
                    respond_terminal_request(responder, runtime.wait_for_terminal_exit(req).await)?;
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
                    respond_terminal_request(responder, runtime.release_terminal(req).await)?;
                    Ok(())
                }
            },
            on_receive_request!(),
        )
        .connect_with(agent, async move |cx| -> Result<(), sacp::Error> {
            let state = state_outer;
            let agent_name_for_log = registry::get_agent_meta(agent_type).name;

            // Advertise filesystem + terminal capabilities for ACP tool execution.
            let init_request = InitializeRequest::new(ProtocolVersion::LATEST).client_capabilities(
                ClientCapabilities::new()
                    .terminal(true)
                    .fs(FileSystemCapabilities::new()
                        .read_text_file(true)
                        .write_text_file(true)),
            );
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

            // Whether this agent accepts MCP server entries over the ACP wire
            // (`session/new`'s `mcpServers`). Almost all do; OpenClaw rejects
            // any server entry and fails session creation, so it must receive
            // NONE — neither user-configured servers nor the built-in codeg-mcp
            // companion. (The `mcpServers` key itself is always serialized as
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
                    inject_codeg_mcp(&mut mcp_servers, inj, &conn_id, &cwd).await
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(ref injected) = delegate_injection {
                let mut s = state.write().await;
                s.delegation_token = Some(injected.token.clone());
                // The agent's actual feedback capability for this session — the
                // authoritative gate for submit + UI, fixed at launch.
                s.feedback_tool_available = injected.feedback_available;
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
                        Ok(resume_resp) => {
                            let initial_config_options = resume_resp.config_options.clone();
                            let new_resp = NewSessionResponse::new(SessionId::new(sid.clone()))
                                .modes(resume_resp.modes)
                                .config_options(resume_resp.config_options)
                                .meta(resume_resp.meta);
                            let mut session = cx.attach_session(new_resp, Default::default())?;

                            // No drain: session/resume does not replay history,
                            // so there is nothing to discard. Any buffered
                            // notification (e.g. an early AvailableCommandsUpdate)
                            // is consumed and forwarded by run_conversation_loop.

                            emit_with_state(
                                &state,
                                &emitter_clone,
                                AcpEvent::SessionStarted {
                                    session_id: sid.clone(),
                                },
                            )
                            .await;
                            emit_session_modes(&state, &emitter_clone, session.modes()).await;
                            let updated_config_options = apply_preferred_session_options(
                                &cx,
                                &mut session,
                                &state,
                                &emitter_clone,
                                preferred_mode_id.as_deref(),
                                &preferred_config_values,
                                initial_config_options.unwrap_or_default(),
                            )
                            .await;
                            emit_session_config_options_values(
                                &state,
                                &emitter_clone,
                                agent_type,
                                updated_config_options,
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
                                delegation_injection.as_ref(),
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
                                delegation_injection.as_ref(),
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

                // Load existing session via session/load
                let load_req = build_load_session_request(
                    agent_type,
                    SessionId::new(sid.clone()),
                    &cwd,
                    mcp_servers.clone(),
                );
                let load_result = cx.send_request_to(Agent, load_req).block_task().await;

                match load_result {
                    Ok(load_resp) => {
                        let initial_config_options = load_resp.config_options.clone();
                        let new_resp = NewSessionResponse::new(SessionId::new(sid.clone()))
                            .modes(load_resp.modes)
                            .config_options(load_resp.config_options)
                            .meta(load_resp.meta);
                        let mut session = cx.attach_session(new_resp, Default::default())?;

                        // Drain historical replay notifications from session/load,
                        // but forward AvailableCommandsUpdate to the frontend
                        let mut drained = 0u32;
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
                                        maybe_emit_claude_sdk_ext_notification(&st, &h, dispatch).await;
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
                        let updated_config_options = apply_preferred_session_options(
                            &cx,
                            &mut session,
                            &state,
                            &emitter_clone,
                            preferred_mode_id.as_deref(),
                            &preferred_config_values,
                            initial_config_options.unwrap_or_default(),
                        )
                        .await;
                        emit_session_config_options_values(
                            &state,
                            &emitter_clone,
                            agent_type,
                            updated_config_options,
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
                            delegation_injection.as_ref(),
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
                            delegation_injection.as_ref(),
                        )
                        .await
                    }
                    Err(e) => {
                        // session/load failed (e.g. agent doesn't support resume,
                        // or ephemeral forked session).
                        // Fall back to session/new so the tab still works.
                        let err_str = e.to_string();
                        let is_resource_not_found = matches!(
                            e.code,
                            sacp::schema::ErrorCode::ResourceNotFound
                        );
                        tracing::warn!(
                            "[ACP] session/load failed ({}){}",
                            err_str,
                            if is_resource_not_found {
                                ", surfacing as session_load_failed"
                            } else {
                                ", falling back to session/new"
                            }
                        );
                        // ResourceNotFound (-32002): the agent has no record of
                        // this session_id (deleted/expired/never existed).
                        // Don't auto-fallback to session/new — that would
                        // silently orphan the historical context. Surface to
                        // the frontend so the user can choose between Reload
                        // (transient agent restart) and New conversation.
                        if is_resource_not_found {
                            emit_with_state(
                                &state,
                                &emitter_clone,
                                AcpEvent::SessionLoadFailed {
                                    session_id: sid.clone(),
                                    message: err_str,
                                    code: "resource_not_found".to_string(),
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
                        // Only emit a visible error for unexpected failures;
                        // "Method not found" is expected for agents that don't
                        // support session resume (e.g. Cline).
                        // "Authentication required" is expected for agents whose
                        // credentials have expired (e.g. Gemini CLI) — skip
                        // session/new too since it will also fail.
                        if err_str.contains("Authentication required") {
                            return Ok(());
                        }
                        if !err_str.contains("Method not found") {
                            emit_with_state(
                                &state,
                                &emitter_clone,
                                AcpEvent::Error {
                                    message: format!("Failed to load session, starting new: {e}"),
                                    agent_type: agent_type.to_string(),
                                    code: None,
                                    // Recoverable: we fall through to `session/new`
                                    // below. Connection stays alive.
                                    terminal: false,
                                },
                            )
                            .await;
                        }
                        let new_resp = cx
                            .send_request_to(
                                Agent,
                                build_new_session_request(
                                    agent_type,
                                    &cwd,
                                    mcp_servers.clone(),
                                ),
                            )
                            .block_task()
                            .await?;
                        let fallback_sid = new_resp.session_id.0.to_string();
                        let initial_config_options = new_resp.config_options.clone();
                        let mut session = cx.attach_session(new_resp, Default::default())?;
                        emit_with_state(
                            &state,
                            &emitter_clone,
                            AcpEvent::SessionStarted {
                                session_id: fallback_sid.clone(),
                            },
                        )
                        .await;
                        emit_session_modes(&state, &emitter_clone, session.modes()).await;
                        let updated_config_options = apply_preferred_session_options(
                            &cx,
                            &mut session,
                            &state,
                            &emitter_clone,
                            preferred_mode_id.as_deref(),
                            &preferred_config_values,
                            initial_config_options.unwrap_or_default(),
                        )
                        .await;
                        emit_session_config_options_values(
                            &state,
                            &emitter_clone,
                            agent_type,
                            updated_config_options,
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
                            delegation_injection.as_ref(),
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
                            delegation_injection.as_ref(),
                        )
                        .await
                    }
                }
            } else {
                // Create new session
                let new_resp = cx
                    .send_request_to(
                        Agent,
                        build_new_session_request(agent_type, &cwd, mcp_servers.clone()),
                    )
                    .block_task()
                    .await?;
                let sid = new_resp.session_id.0.to_string();
                let initial_config_options = new_resp.config_options.clone();
                let mut session = cx.attach_session(new_resp, Default::default())?;
                emit_with_state(
                    &state,
                    &emitter_clone,
                    AcpEvent::SessionStarted {
                        session_id: sid.clone(),
                    },
                )
                .await;
                emit_session_modes(&state, &emitter_clone, session.modes()).await;
                let updated_config_options = apply_preferred_session_options(
                    &cx,
                    &mut session,
                    &state,
                    &emitter_clone,
                    preferred_mode_id.as_deref(),
                    &preferred_config_values,
                    initial_config_options.unwrap_or_default(),
                )
                .await;
                emit_session_config_options_values(
                    &state,
                    &emitter_clone,
                    agent_type,
                    updated_config_options,
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
                    delegation_injection.as_ref(),
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
                    delegation_injection.as_ref(),
                )
                .await
            }
        })
        .await
        .map_err(|e| {
            let raw = e.to_string();
            if raw.contains(INIT_TIMEOUT_SENTINEL) {
                AcpError::InitializeTimeout
            } else {
                AcpError::protocol(raw)
            }
        })
}

/// Store the permission responder and emit event to frontend.
async fn handle_permission_request(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    perms: &PendingPermissions,
    cwd: &str,
    req: RequestPermissionRequest,
    responder: Responder<RequestPermissionResponse>,
) {
    let request_id = uuid::Uuid::new_v4().to_string();

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

    perms.lock().await.insert(request_id.clone(), responder);

    emit_with_state(
        state,
        emitter,
        AcpEvent::PermissionRequest {
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
    let updated = set_session_config_option_inner(cx, session_id, config_id, value_id).await?;
    emit_session_config_options_values(state, emitter, agent_type, updated).await;
    Ok(())
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
    value_id: String,
) -> Result<Vec<SessionConfigOption>, sacp::Error> {
    let req = SetSessionConfigOptionRequest::new(session_id.clone(), config_id, value_id);
    let untyped_req = UntypedMessage::new("session/set_config_option", req).map_err(|e| {
        sacp::util::internal_error(format!("Failed to build config option request: {e}"))
    })?;

    let raw_response = cx.send_request_to(Agent, untyped_req).block_task().await?;
    let response: SetSessionConfigOptionResponse =
        serde_json::from_value(raw_response).map_err(|e| {
            sacp::util::internal_error(format!("Failed to parse config option response: {e}"))
        })?;

    Ok(response.config_options)
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
        let already_matches = options.iter().any(|o| {
            o.id.to_string() == *config_id
                && matches!(
                    &o.kind,
                    SessionConfigKind::Select(s) if s.current_value.to_string() == *value_id
                )
        });
        if already_matches {
            continue;
        }
        match set_session_config_option_inner(cx, &session_id, config_id.clone(), value_id.clone())
            .await
        {
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
    // Threaded through from run_connection so the forked session's
    // run_conversation_loop call has the same delegation cascade
    // capability as the original.
    delegation_injection: Option<&DelegationInjection>,
) -> Result<(), sacp::Error> {
    let fork_info = match loop_result {
        Ok(Some(info)) => info,
        Ok(None) => return Ok(()),
        Err(e) => return Err(e),
    };

    let cx = fork_info.connection;
    let fork_resp = fork_info.fork_response;
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
    let mut session = cx.attach_session(new_resp, Default::default())?;

    emit_with_state(
        state,
        emitter,
        AcpEvent::SessionStarted {
            session_id: new_sid.clone(),
        },
    )
    .await;
    emit_session_modes(state, emitter, session.modes()).await;
    emit_session_config_options_values(
        state,
        emitter,
        agent_type,
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
        delegation_injection,
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
        delegation_injection,
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

/// Build an `AcpEvent::Error` for a non-success stop reason so the user gets a
/// toast instead of a silent transition to `PendingReview`. Returns `None` for
/// `end_turn` (success) and `cancelled` (already user-driven).
///
/// `Refusal` is included because OpenCode (and similar agents) map backend /
/// gateway errors to `Refusal` per the ACP spec gap — see
/// <https://shashikantjagtap.net/openclaw-acp-what-coding-agent-users-need-to-know-about-protocol-gaps/>.
/// `empty` is a synthesized reason emitted by `run_conversation_loop` when
/// the agent reports `EndTurn` without producing any agent output.
fn turn_failure_error_event(reason_str: &str, agent_type: AgentType) -> Option<AcpEvent> {
    let (code, message) = match reason_str {
        "refusal" => (
            "turn_failed_refusal",
            format!("{agent_type} refused to continue this turn."),
        ),
        "max_tokens" => (
            "turn_failed_max_tokens",
            format!("{agent_type} reached the maximum token limit for this turn."),
        ),
        "max_turn_requests" => (
            "turn_failed_max_turn_requests",
            format!("{agent_type} reached the maximum number of allowed requests for this turn."),
        ),
        "unknown" => (
            "turn_failed_unknown",
            format!("{agent_type} ended the turn with an unrecognized stop reason."),
        ),
        "empty" => (
            "turn_failed_empty",
            format!(
                "{agent_type} ended the turn without producing any response. \
                 Please check the agent's configuration."
            ),
        ),
        _ => return None,
    };
    Some(AcpEvent::Error {
        message,
        agent_type: agent_type.to_string(),
        code: Some(code.to_string()),
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
    // Source of the broker reference used to cascade-cancel pending
    // delegations on parent prompt cancel / non-success TurnComplete.
    // `None` for test paths that don't wire delegation.
    delegation_injection: Option<&DelegationInjection>,
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
                                    maybe_emit_claude_sdk_ext_notification(&st, &h, dispatch).await;
                                    Ok(())
                                })
                                .await;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!("[ACP] Ignoring unrecognized session update in idle loop: {e}");
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

                // Clone connection and session ID before entering the
                // select loop so we can send CancelNotification without
                // conflicting with session.read_update()'s mutable borrow.
                let cx = session.connection();
                let sid = session.session_id().clone();
                let prompt_request = PromptRequest::new(sid.clone(), prompt_blocks);
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
                // Tracks whether the agent produced any real output during
                // this turn (text reply, thinking chunk, or tool call). When
                // an agent reports `EndTurn` with this still false, we treat
                // it as a silent failure and synthesize an `"empty"` stop
                // reason so the user gets an error toast instead of a
                // confusing `PendingReview` on a blank conversation.
                let mut turn_had_agent_output = false;
                // A CodeBuddy native sub-agent's full lifecycle (Agent tool call
                // open → completed) happens within one turn, so reset the
                // suppression window at each turn start. This bounds the tracking
                // sets and guarantees a sub-agent that ended without a terminal
                // frame (cancel/abort) can never suppress the NEXT turn's
                // main-agent thinking. `title_overrides` intentionally persists
                // (a card's identity is session-stable).
                cb_state.open_subagents.clear();
                cb_state.closed_subagents.clear();

                // Read updates until turn completes.
                // We must also listen for commands (e.g. RespondPermission)
                // to avoid deadlocking when the agent awaits a permission response.
                loop {
                    tokio::select! {
                        update = session.read_update() => {
                            let update = match update {
                                Ok(u) => u,
                                Err(e) => {
                                    tracing::warn!("[ACP] Ignoring unrecognized session update: {e}");
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
                                    if let Err(e) = MatchDispatch::new(dispatch)
                                        .if_notification(
                                            async |notif: SessionNotification| {
                                                let should_poll_now = track_terminal_tool_calls(
                                                    &notif.update,
                                                    &mut tracked_terminal_tool_calls,
                                                );
                                                if is_agent_output_update(&notif.update) {
                                                    turn_had_agent_output = true;
                                                }
                                                emit_conversation_update(&st, &h, agent_type, notif.update, cwd_opt, &mut raw_output_cache, &mut cb_state).await;
                                                if should_poll_now {
                                                    poll_tracked_terminal_tool_calls(
                                                        runtime.as_ref(),
                                                        &session_id,
                                                        &st,
                                                        &h,
                                                        &mut tracked_terminal_tool_calls,
                                                    )
                                                    .await;
                                                }
                                                Ok(())
                                            },
                                        )
                                        .await
                                        .otherwise(async |dispatch| {
                                            maybe_emit_claude_sdk_ext_notification(&st, &h, dispatch).await;
                                            Ok(())
                                        })
                                        .await
                                    {
                                        tracing::warn!("[ACP] Ignoring dispatch parse error: {e}");
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
                                    let reason_str = if raw_reason_str == "end_turn"
                                        && !turn_had_agent_output
                                    {
                                        "empty"
                                    } else {
                                        raw_reason_str
                                    };
                                    if let Some(err_event) =
                                        turn_failure_error_event(reason_str, agent_type)
                                    {
                                        emit_with_state(state, emitter, err_event).await;
                                    }
                                    emit_with_state(
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
                            let reason = prompt_result?.stop_reason;
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
                            let reason_str = if raw_reason_str == "end_turn"
                                && !turn_had_agent_output
                            {
                                "empty"
                            } else {
                                raw_reason_str
                            };
                            if let Some(err_event) =
                                turn_failure_error_event(reason_str, agent_type)
                            {
                                emit_with_state(state, emitter, err_event).await;
                            }
                            emit_with_state(
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
                                    if let Some(responder) = perms.lock().await.remove(&request_id) {
                                        let outcome = RequestPermissionOutcome::Selected(
                                            SelectedPermissionOutcome::new(option_id),
                                        );
                                        let _ = responder.respond(RequestPermissionResponse::new(outcome));
                                        emit_with_state(
                                            state,
                                            emitter,
                                            AcpEvent::PermissionResolved { request_id },
                                        )
                                        .await;
                                    }
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
                                    if let Err(e) = set_session_config_option(
                                        &cx,
                                        &sid,
                                        state,
                                        emitter,
                                        agent_type,
                                        config_id,
                                        value_id,
                                    )
                                    .await
                                    {
                                        emit_with_state(
                                            state,
                                            emitter,
                                            AcpEvent::Error {
                                                message: format!("Failed to set config option: {e}"),
                                                agent_type: agent_type.to_string(),
                                                code: None,
                                                // Recoverable: just a failed config-option toggle.
                                                terminal: false,
                                            },
                                        )
                                        .await;
                                    }
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
                                    let mut locked = perms.lock().await;
                                    for (_, responder) in locked.drain() {
                                        let _ = responder.respond(RequestPermissionResponse::new(
                                            RequestPermissionOutcome::Cancelled,
                                        ));
                                    }
                                    drop(locked);
                                    // Immediately emit TurnComplete so the frontend
                                    // transitions out of "prompting" and the user can
                                    // send new messages.  Don't wait for the agent --
                                    // it may be slow to respond or not respond at all.
                                    emit_with_state(
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
                                    let mut locked = perms.lock().await;
                                    for (_, responder) in locked.drain() {
                                        let _ = responder.respond(RequestPermissionResponse::new(
                                            RequestPermissionOutcome::Cancelled,
                                        ));
                                    }
                                    disconnect_requested = true;
                                    break;
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
                if let Some(responder) = perms.lock().await.remove(&request_id) {
                    let outcome = RequestPermissionOutcome::Selected(
                        SelectedPermissionOutcome::new(option_id),
                    );
                    let _ = responder.respond(RequestPermissionResponse::new(outcome));
                    emit_with_state(state, emitter, AcpEvent::PermissionResolved { request_id })
                        .await;
                }
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
                if let Err(e) = set_session_config_option(
                    &cx, &sid, state, emitter, agent_type, config_id, value_id,
                )
                .await
                {
                    emit_with_state(
                        state,
                        emitter,
                        AcpEvent::Error {
                            message: format!("Failed to set config option: {e}"),
                            agent_type: agent_type.to_string(),
                            code: None,
                            // Recoverable: idle SetConfigOption failure leaves
                            // the connection alive.
                            terminal: false,
                        },
                    )
                    .await;
                }
            }
            Some(ConnectionCommand::Cancel) => {
                let cx = session.connection();
                let sid = session.session_id().clone();
                let _ = cx.send_notification_to(Agent, CancelNotification::new(sid.clone()));
                terminal_runtime
                    .release_all_for_session(sid.0.as_ref())
                    .await;
                let mut locked = perms.lock().await;
                for (_, responder) in locked.drain() {
                    let _ = responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ));
                }
                drop(locked);
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
                    Ok(fork_response) => {
                        tracing::info!(
                            "[ACP] Fork succeeded: new_session_id={}",
                            fork_response.session_id.0
                        );
                        return Ok(Some(ForkExitInfo {
                            fork_response,
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

/// Serialize a Vec<ToolCallContent> into a human-readable text string.
fn serialize_tool_call_content(content: &[ToolCallContent]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for item in content {
        match item {
            ToolCallContent::Content(c) => {
                if let ContentBlock::Text(text) = &c.content {
                    parts.push(text.text.clone());
                }
            }
            ToolCallContent::Diff(diff) => {
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

/// Extract `ContentBlock::Image` payloads from a `ToolCallContent` slice.
/// Returns `None` when no images are present so the upstream `images` field
/// on `AcpEvent::ToolCall(Update)` stays absent for non-image tool calls
/// (preserves replace-on-update semantics: an absent field means "keep
/// prior", a `Some(vec)` replaces).
fn extract_tool_call_images(content: &[ToolCallContent]) -> Option<Vec<ToolCallImageInfo>> {
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

fn json_value_to_text(val: &Option<serde_json::Value>) -> Option<String> {
    match val {
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(v) if !v.is_null() => Some(v.to_string()),
        _ => None,
    }
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
/// represented by the Agent pill + its nested tool calls). Matches Claude Code,
/// which never streams a sub-agent's internal reasoning onto the main session.
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

fn map_claude_sdk_ext_notification(notification: &UntypedMessage) -> Option<AcpEvent> {
    if notification.method() != "_claude/sdkMessage" {
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

async fn maybe_emit_claude_sdk_ext_notification(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    dispatch: Dispatch,
) {
    let Dispatch::Notification(notification) = dispatch else {
        return;
    };

    if let Some(event) = map_claude_sdk_ext_notification(&notification) {
        emit_with_state(state, emitter, event).await;
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
                emit_with_state(state, emitter, AcpEvent::ContentDelta { text: text.text }).await;
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
                emit_with_state(state, emitter, AcpEvent::Thinking { text: text.text }).await;
            }
        }
        SessionUpdate::AgentThoughtChunk(_) => {
            // Non-text thought chunks are currently ignored.
        }
        SessionUpdate::ToolCall(tc) => {
            let tool_call_id = tc.tool_call_id.to_string();
            // CodeBuddy double-wraps a deferred MCP result as a `{type,text}`
            // content part; peel it (in both the content and raw_output channels)
            // so the dedicated delegation cards parse it instead of showing raw JSON.
            let content = serialize_tool_call_content(&tc.content)
                .map(|c| unwrap_codebuddy_deferred_output(agent_type, &c).unwrap_or(c));
            let images = extract_tool_call_images(&tc.content);
            let raw_input =
                json_value_to_text(&tc.raw_input).map(|text| resolve_live_tool_input(&text, cwd));
            // Initial tool_call notification — the frontend reducer
            // treats `raw_output` as a full replacement, so we bypass
            // the diff path and seed the cache with the current snapshot.
            let raw_output = json_value_to_text(&tc.raw_output)
                .map(|text| unwrap_codebuddy_deferred_output(agent_type, &text).unwrap_or(text))
                .map(|text| structurize_live_output(&text))
                .and_then(|text| raw_output_cache.seed(&tool_call_id, &text));
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
            let meta_marks_subagent = codebuddy_meta_marks_subagent(agent_type, tc.meta.as_ref());
            let meta_marks_background = codebuddy_meta_marks_background(agent_type, tc.meta.as_ref());
            let meta = tc.meta.map(serde_json::Value::Object);
            let status = format!("{:?}", tc.status).to_lowercase();
            raw_output_cache.remove_if_final(&tool_call_id, Some(status.as_str()));
            // Avoid logging titles/payloads below — they can be model-generated
            // user task descriptions (PII-adjacent) and would create noise in
            // server-mode log sinks. The opaque tool_call_id is enough to
            // correlate these events with downstream traces.
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
            let tool_call_id = tcu.tool_call_id.to_string();
            // Peel CodeBuddy's `{type,text}` deferred-MCP wrapper here too — the
            // result often arrives on an update (see raw_output below).
            let content = tcu
                .fields
                .content
                .as_deref()
                .and_then(serialize_tool_call_content)
                .map(|c| unwrap_codebuddy_deferred_output(agent_type, &c).unwrap_or(c));
            let images = tcu
                .fields
                .content
                .as_deref()
                .and_then(extract_tool_call_images);
            let raw_input = json_value_to_text(&tcu.fields.raw_input)
                .map(|text| resolve_live_tool_input(&text, cwd));
            // Diff the incoming raw_output against the last snapshot we
            // emitted for this tool call. This turns cumulative snapshots
            // from agents (Claude Code, Codex, …) into incremental deltas
            // with `raw_output_append=true`, collapsing the O(N²) transfer
            // problem to O(N) while capping any single emitted chunk to
            // MAX_SINGLE_EMIT_BYTES.
            let raw_output_text = json_value_to_text(&tcu.fields.raw_output)
                .map(|text| unwrap_codebuddy_deferred_output(agent_type, &text).unwrap_or(text))
                .map(|text| structurize_live_output(&text));
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
            let meta_marks_subagent = codebuddy_meta_marks_subagent(agent_type, tcu.meta.as_ref());
            let meta_marks_background = codebuddy_meta_marks_background(agent_type, tcu.meta.as_ref());
            let meta = tcu.meta.clone().map(serde_json::Value::Object);
            let status = tcu.fields.status.map(|s| format!("{:?}", s).to_lowercase());
            raw_output_cache.remove_if_final(&tool_call_id, status.as_deref());
            // Re-assert any authoritative title rewrite (see fn doc): an update
            // that carries the subagent/deferred marker classifies (and records)
            // the card, and — the key fix — a later status-only update that LOST
            // the marker but carries the agent's raw (non-agent) title still
            // resolves to the recorded override, so the Agent/delegation card and
            // its child nesting (`getToolName === "agent"`) don't revert to a
            // generic tool call mid-stream. Falls back to the event's own title
            // for never-classified tool calls.
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
            // Some agents (e.g. Claude Code with overlapping user/project slash
            // commands) emit duplicate entries sharing the same name. Keep the
            // first occurrence so downstream consumers don't render duplicates;
            // the frontend reducer also dedupes as a defensive measure.
            let mut seen = HashSet::new();
            let commands: Vec<AvailableCommandInfo> = update
                .available_commands
                .iter()
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
        other => {
            // Log unhandled update types for debugging
            tracing::info!("[ACP] Unhandled SessionUpdate: {:?}", other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let injection = DelegationInjection {
            broker,
            tokens: Arc::new(TokenRegistry::default()),
            socket_path: std::path::PathBuf::from("/tmp/codeg-mcp.sock"),
            feedback: crate::acp::feedback::FeedbackRuntimeConfig::new(),
            ask: crate::acp::question::QuestionRuntimeConfig::new(),
            sessions: crate::acp::session_info::SessionInfoRuntimeConfig::new(),
            questions: Arc::new(NoQuestions)
                as Arc<dyn crate::acp::question::SessionQuestionAccess>,
        };

        let mut servers: Vec<McpServer> = Vec::new();
        let result = inject_codeg_mcp(
            &mut servers,
            &injection,
            "parent-conn",
            std::path::Path::new("/tmp"),
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

    // ─── companion_features_arg: inject/skip decision + --features value ──
    //
    // The companion now carries two independently-toggled tool groups. It is
    // injected when EITHER is on, and the `--features` arg names exactly the
    // enabled groups so the companion hides the rest. Crucially, feedback alone
    // must still inject the companion (the historical delegation-only gate would
    // have skipped it).
    #[test]
    fn companion_features_arg_inject_skip_decision() {
        // All off → no companion at all.
        assert_eq!(companion_features_arg(false, false, false, false), None);
        // Delegation only.
        assert_eq!(
            companion_features_arg(true, false, false, false),
            Some("delegation".to_string())
        );
        // Feedback only — the decoupling: companion injected for feedback even
        // when delegation is off.
        assert_eq!(
            companion_features_arg(false, true, false, false),
            Some("feedback".to_string())
        );
        // Ask only — likewise injects the companion on its own.
        assert_eq!(
            companion_features_arg(false, false, true, false),
            Some("ask".to_string())
        );
        // Sessions only — likewise injects the companion on its own.
        assert_eq!(
            companion_features_arg(false, false, false, true),
            Some("sessions".to_string())
        );
        // All on → comma-joined, in declaration order.
        assert_eq!(
            companion_features_arg(true, true, true, true),
            Some("delegation,feedback,ask,sessions".to_string())
        );
    }
}
