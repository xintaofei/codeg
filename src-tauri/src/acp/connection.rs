use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sacp::schema::McpServerStdio;
use sacp::schema::{
    BlobResourceContents, CancelNotification, ClientCapabilities, ContentBlock, ContentChunk,
    CreateTerminalRequest, CreateTerminalResponse, EmbeddedResource, EmbeddedResourceResource,
    FileSystemCapabilities, ImageContent, InitializeRequest, KillTerminalRequest,
    KillTerminalResponse, LoadSessionRequest, NewSessionRequest, NewSessionResponse,
    PermissionOptionKind, Plan, PlanEntryPriority, PlanEntryStatus, PromptRequest, ProtocolVersion,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, ResourceLink,
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectGroup, SessionConfigSelectOption, SessionConfigSelectOptions, SessionId,
    SessionModeState, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, StopReason, TerminalExitStatus,
    TerminalOutputRequest, TerminalOutputResponse, TextContent, TextResourceContents,
    ToolCallContent, WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use sacp::util::MatchDispatch;
use sacp::{
    on_receive_request, Agent, Client, ConnectionTo, Dispatch, Responder, SessionMessage,
    UntypedMessage,
};
use sacp_tokio::AcpAgent;
use tokio::sync::mpsc;

use crate::acp::error::AcpError;
use crate::acp::file_system_runtime::{FileSystemRuntime, FileSystemRuntimeError};
use crate::acp::manager::ConnectionManager;
use crate::acp::registry::{self, AgentDistribution};
use crate::acp::terminal_runtime::{TerminalRuntime, TerminalRuntimeError};
use crate::acp::types::{
    AcpEvent, AvailableCommandInfo, ConnectionInfo, ConnectionStatus, PermissionOptionInfo,
    PlanEntryInfo, PromptCapabilitiesInfo, PromptInputBlock, SessionConfigKindInfo,
    SessionConfigOptionInfo, SessionConfigSelectGroupInfo, SessionConfigSelectInfo,
    SessionConfigSelectOptionInfo, SessionModeInfo, SessionModeStateInfo,
};
use crate::models::agent::AgentType;
use crate::network::proxy;
use crate::web::event_bridge::EventEmitter;

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

    merged.into_iter().collect()
}

/// Commands sent from Tauri command handlers to the ACP connection loop.
pub enum ConnectionCommand {
    Prompt {
        blocks: Vec<PromptInputBlock>,
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
        reply: tokio::sync::oneshot::Sender<Result<crate::acp::types::ForkResultInfo, AcpError>>,
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
#[derive(Debug, Clone)]
pub struct AgentConnectionMetadata {
    pub status: ConnectionStatus,
    pub working_dir: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub session_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct AgentConnection {
    pub id: String,
    pub agent_type: AgentType,
    pub owner_window_label: String,
    pub cmd_tx: mpsc::Sender<ConnectionCommand>,
    pub metadata: Arc<tokio::sync::RwLock<AgentConnectionMetadata>>,
}

impl AgentConnection {
    pub async fn info(&self) -> ConnectionInfo {
        let metadata = self.metadata.read().await;
        ConnectionInfo {
            id: self.id.clone(),
            agent_type: self.agent_type,
            status: metadata.status.clone(),
            owner_window_label: self.owner_window_label.clone(),
            working_dir: metadata.working_dir.clone(),
            created_at: metadata.created_at,
            updated_at: metadata.updated_at,
            session_id: metadata.session_id.clone(),
            last_error: metadata.last_error.clone(),
        }
    }
}

async fn with_connection_metadata<F>(
    connections: &Arc<tokio::sync::Mutex<HashMap<String, AgentConnection>>>,
    connection_id: &str,
    update: F,
) where
    F: FnOnce(&mut AgentConnectionMetadata),
{
    let metadata = {
        let connections = connections.lock().await;
        connections
            .get(connection_id)
            .map(|connection| connection.metadata.clone())
    };

    if let Some(metadata) = metadata {
        let mut metadata = metadata.write().await;
        update(&mut metadata);
        metadata.updated_at = Utc::now();
    }
}

async fn emit_status_changed(
    connections: &Arc<tokio::sync::Mutex<HashMap<String, AgentConnection>>>,
    emitter: &EventEmitter,
    connection_id: &str,
    status: ConnectionStatus,
) {
    let metadata_status = status.clone();
    with_connection_metadata(connections, connection_id, move |metadata| {
        metadata.status = metadata_status.clone();
        if metadata_status != ConnectionStatus::Error {
            metadata.last_error = None;
        }
    })
    .await;

    crate::web::event_bridge::emit_event(
        emitter,
        "acp://event",
        AcpEvent::StatusChanged {
            connection_id: connection_id.to_string(),
            status,
        },
    );
}

async fn emit_connection_error(
    connections: &Arc<tokio::sync::Mutex<HashMap<String, AgentConnection>>>,
    emitter: &EventEmitter,
    connection_id: &str,
    agent_type: AgentType,
    message: String,
    code: Option<String>,
) {
    let metadata_message = message.clone();
    with_connection_metadata(connections, connection_id, move |metadata| {
        metadata.status = ConnectionStatus::Error;
        metadata.last_error = Some(metadata_message.clone());
    })
    .await;

    crate::web::event_bridge::emit_event(
        emitter,
        "acp://event",
        AcpEvent::Error {
            connection_id: connection_id.to_string(),
            message,
            agent_type: agent_type.to_string(),
            code,
        },
    );
}

async fn emit_session_started(
    connections: &Arc<tokio::sync::Mutex<HashMap<String, AgentConnection>>>,
    emitter: &EventEmitter,
    connection_id: &str,
    session_id: &str,
) {
    let session_id_string = session_id.to_string();
    with_connection_metadata(connections, connection_id, move |metadata| {
        metadata.session_id = Some(session_id_string.clone());
    })
    .await;

    crate::web::event_bridge::emit_event(
        emitter,
        "acp://event",
        AcpEvent::SessionStarted {
            connection_id: connection_id.to_string(),
            session_id: session_id.to_string(),
        },
    );
}

/// Build an AcpAgent from registry metadata.
async fn build_agent(
    agent_type: AgentType,
    runtime_env: &BTreeMap<String, String>,
) -> Result<AcpAgent, AcpError> {
    let meta = registry::get_agent_meta(agent_type);
    debug_assert_eq!(meta.agent_type, agent_type);

    match meta.distribution {
        AgentDistribution::Npx { cmd, args, env, .. } => {
            let merged_env = merge_agent_env(env, runtime_env);
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in &merged_env {
                parts.push(format!("{k}={v}"));
            }
            parts.push(
                which::which(cmd)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| {
                        crate::process::normalized_program(cmd)
                            .to_string_lossy()
                            .to_string()
                    }),
            );
            for a in args {
                parts.push((*a).into());
            }
            // Translate Generic-specific env vars to CLI flags
            if agent_type == AgentType::Generic {
                if let Some(url) = runtime_env
                    .get("GENERIC_GATEWAY_URL")
                    .filter(|v| !v.is_empty())
                {
                    parts.push("--url".into());
                    parts.push(url.clone());
                }
                if let Some(key) = runtime_env
                    .get("GENERIC_SESSION_KEY")
                    .filter(|v| !v.is_empty())
                {
                    parts.push("--session".into());
                    parts.push(key.clone());
                }
                if runtime_env
                    .get("GENERIC_RESET_SESSION")
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
                            eprintln!("[ACP][{agent_name}][stderr] {line}");
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
                eprintln!("[ACP][{}] Using cached binary {cached_version}", meta.name);
            } else {
                eprintln!(
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
            eprintln!(
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
                            eprintln!(
                                "[ACP][{agent_name}][{tag}] {}... <truncated {} bytes>",
                                &line[..head],
                                line.len() - head
                            );
                        } else {
                            eprintln!("[ACP][{agent_name}][{tag}] {line}");
                        }
                    },
                ),
            )
        }
    }
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
    manager: ConnectionManager,
    connection_id: String,
    agent_type: AgentType,
    working_dir: Option<String>,
    session_id: Option<String>,
    runtime_env: BTreeMap<String, String>,
    owner_window_label: String,
    emitter: EventEmitter,
) -> Result<(), AcpError> {
    let connections = manager.connections_arc();

    crate::web::event_bridge::emit_event(
        &emitter,
        "acp://event",
        AcpEvent::StatusChanged {
            connection_id: connection_id.clone(),
            status: ConnectionStatus::Connecting,
        },
    );

    let agent = build_agent(agent_type, &runtime_env).await?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<ConnectionCommand>(32);
    let conn_id = connection_id.clone();
    let emitter_clone = emitter.clone();
    let cleanup_connections = connections.clone();
    let event_connections = connections.clone();
    let run_connections = connections.clone();
    let cleanup_connection_id = connection_id.clone();
    let manager_clone = manager.clone_ref();
    let working_dir_for_metadata = working_dir.clone();
    let owner_window_label_for_log = owner_window_label.clone();

    // Insert the entry BEFORE spawning the background task so that a
    // fast-failing `run_connection` can never remove it before it was
    // inserted (would otherwise leak the entry).
    connections.lock().await.insert(
        connection_id.clone(),
        AgentConnection {
            id: connection_id,
            agent_type,
            owner_window_label,
            cmd_tx,
            metadata: Arc::new(tokio::sync::RwLock::new(AgentConnectionMetadata {
                status: ConnectionStatus::Connecting,
                working_dir: working_dir_for_metadata,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                session_id: None,
                last_error: None,
            })),
        },
    );

    tokio::spawn(async move {
        // RAII guard: runs on normal exit AND on panic unwinding, so a
        // panic inside `run_connection` can't leak a stale map entry.
        let _cleanup = ConnectionCleanupGuard {
            connections: cleanup_connections,
            connection_id: cleanup_connection_id,
        };

        let result = run_connection(
            agent,
            conn_id.clone(),
            agent_type,
            working_dir,
            session_id,
            cmd_rx,
            run_connections,
            emitter_clone.clone(),
        )
        .await;

        if let Err(e) = result {
            let code = e.code().map(String::from);
            manager_clone
                .record_failure(agent_type, &e.to_string(), Some(&conn_id))
                .await;
            emit_connection_error(
                &event_connections,
                &emitter_clone,
                &conn_id,
                agent_type,
                e.to_string(),
                code,
            )
            .await;
        }

        emit_status_changed(
            &event_connections,
            &emitter_clone,
            &conn_id,
            ConnectionStatus::Disconnected,
        )
        .await;
        manager_clone.log_runtime(
            "info",
            "acp",
            format!(
                "connection {} closed for {} ({})",
                conn_id, agent_type, owner_window_label_for_log
            ),
            None,
        );
        // `_cleanup` is dropped here — removes the connection entry from
        // the manager map. Same drop semantics apply on panic unwinding.
    });

    Ok(())
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

fn emit_session_modes(
    connection_id: &str,
    emitter: &EventEmitter,
    modes: &Option<SessionModeState>,
) {
    if let Some(mode_state) = modes {
        crate::web::event_bridge::emit_event(
            emitter,
            "acp://event",
            AcpEvent::SessionModes {
                connection_id: connection_id.into(),
                modes: map_session_modes(mode_state),
            },
        );
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

/// Codex-acp sometimes omits the "mode" (approval preset) config option when
/// the loaded sandbox policy does not exactly match one of the three built-in
/// presets (commonly because `writable_roots` was injected during config
/// loading).  When that happens, synthesize the option so the user can still
/// pick a preset.  codex-acp's `set_config_option` handler always accepts
/// `config_id = "mode"` regardless of whether it was advertised.
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
                current_value: "auto".to_string(),
                options: vec![
                    SessionConfigSelectOptionInfo {
                        value: "read-only".to_string(),
                        name: "Read Only".to_string(),
                        description: Some("Codex can only read files".to_string()),
                    },
                    SessionConfigSelectOptionInfo {
                        value: "auto".to_string(),
                        name: "Default".to_string(),
                        description: Some(
                            "Codex can edit files, but asks before running commands".to_string(),
                        ),
                    },
                    SessionConfigSelectOptionInfo {
                        value: "full-access".to_string(),
                        name: "Full Access".to_string(),
                        description: Some("Codex runs without asking for approval".to_string()),
                    },
                ],
                groups: vec![],
            }),
        },
    );
}

fn emit_session_config_options_values(
    connection_id: &str,
    emitter: &EventEmitter,
    agent_type: AgentType,
    config_options: Vec<SessionConfigOption>,
) {
    let mut mapped = map_session_config_options(&config_options);
    if agent_type == AgentType::Codex {
        ensure_codex_mode_option(&mut mapped);
    }
    crate::web::event_bridge::emit_event(
        emitter,
        "acp://event",
        AcpEvent::SessionConfigOptions {
            connection_id: connection_id.into(),
            config_options: mapped,
        },
    );
}

fn emit_session_config_options(
    connection_id: &str,
    emitter: &EventEmitter,
    agent_type: AgentType,
    config_options: &Option<Vec<SessionConfigOption>>,
) {
    // Always emit one config-options snapshot after session attach.
    // Some agents (e.g. Gemini CLI) may not expose session config options
    // and return `None`; emitting an empty list lets the frontend settle
    // loading state instead of waiting forever.
    let options = config_options.clone().unwrap_or_default();
    emit_session_config_options_values(connection_id, emitter, agent_type, options);
}

fn emit_selectors_ready(connection_id: &str, emitter: &EventEmitter) {
    crate::web::event_bridge::emit_event(
        emitter,
        "acp://event",
        AcpEvent::SelectorsReady {
            connection_id: connection_id.into(),
        },
    );
}

fn emit_prompt_capabilities(
    connection_id: &str,
    emitter: &EventEmitter,
    capabilities: &sacp::schema::PromptCapabilities,
) {
    crate::web::event_bridge::emit_event(
        emitter,
        "acp://event",
        AcpEvent::PromptCapabilities {
            connection_id: connection_id.into(),
            prompt_capabilities: PromptCapabilitiesInfo {
                image: capabilities.image,
                audio: capabilities.audio,
                embedded_context: capabilities.embedded_context,
            },
        },
    );
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

fn build_new_session_request(agent_type: AgentType, cwd: &Path) -> NewSessionRequest {
    let mut req = NewSessionRequest::new(cwd.to_path_buf());
    if let Some(meta) = claude_raw_sdk_session_meta(agent_type) {
        req = req.meta(meta);
    }
    req
}

fn build_load_session_request(
    agent_type: AgentType,
    session_id: SessionId,
    cwd: &Path,
) -> LoadSessionRequest {
    let mut req = LoadSessionRequest::new(session_id, cwd.to_path_buf());
    if let Some(meta) = claude_raw_sdk_session_meta(agent_type) {
        req = req.meta(meta);
    }
    req
}

/// The main ACP connection loop.
#[allow(clippy::too_many_arguments)]
async fn run_connection(
    agent: AcpAgent,
    connection_id: String,
    agent_type: AgentType,
    working_dir: Option<String>,
    session_id: Option<String>,
    mut cmd_rx: mpsc::Receiver<ConnectionCommand>,
    connections: Arc<tokio::sync::Mutex<HashMap<String, AgentConnection>>>,
    emitter: EventEmitter,
) -> Result<(), AcpError> {
    let pending_perms: PendingPermissions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let terminal_runtime = Arc::new(TerminalRuntime::new());
    let cwd = resolve_working_dir(working_dir.as_deref());
    let cwd_string = cwd.to_string_lossy().to_string();
    let file_system_runtime = Arc::new(FileSystemRuntime::new(cwd.clone()));

    let conn_id = connection_id.clone();
    let emitter_clone = emitter.clone();
    let perms = pending_perms.clone();

    Client
        .builder()
        .name("codeg")
        .on_receive_request(
            {
                let conn_id = conn_id.clone();
                let emitter_inner = emitter_clone.clone();
                let perms = perms.clone();
                let perm_cwd = cwd_string.clone();
                async move |req: RequestPermissionRequest,
                            responder: Responder<RequestPermissionResponse>,
                            _cx: ConnectionTo<Agent>| {
                    handle_permission_request(
                        &conn_id,
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
            eprintln!(
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
                    eprintln!(
                        "[ACP][{agent_name_for_log}] Initialize responded in {:?}",
                        init_started.elapsed()
                    );
                    resp
                }
                Ok(Err(e)) => {
                    eprintln!(
                        "[ACP][{agent_name_for_log}] Initialize failed in {:?}: {e}",
                        init_started.elapsed()
                    );
                    return Err(e);
                }
                Err(_) => {
                    eprintln!(
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
                &conn_id,
                &emitter_clone,
                &init_resp.agent_capabilities.prompt_capabilities,
            );

            let supports_fork = init_resp
                .agent_capabilities
                .session_capabilities
                .fork
                .is_some();
            eprintln!(
                "[ACP] Agent capabilities: load_session={}, fork={}",
                init_resp.agent_capabilities.load_session, supports_fork
            );

            // Emit fork support capability
            crate::web::event_bridge::emit_event(
                &emitter_clone,
                "acp://event",
                AcpEvent::ForkSupported {
                    connection_id: conn_id.clone(),
                    supported: supports_fork,
                },
            );

            // Emit connected status early so the frontend can show cached
            // selectors and enable sending while the session initialises.
            // Prompts sent before run_conversation_loop are buffered in
            // the cmd_rx channel and processed as soon as the loop starts.
            emit_status_changed(
                &connections,
                &emitter_clone,
                &conn_id,
                ConnectionStatus::Connected,
            )
            .await;

            if let Some(sid) = session_id {
                // Load existing session via session/load
                let load_req =
                    build_load_session_request(agent_type, SessionId::new(sid.clone()), &cwd);
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
                                let cid = conn_id.clone();
                                let h = emitter_clone.clone();
                                let dispatch = fix_usage_update_nulls(dispatch);
                                let _ = MatchDispatch::new(dispatch)
                                    .if_notification(async |notif: SessionNotification| {
                                        if matches!(
                                            notif.update,
                                            SessionUpdate::AvailableCommandsUpdate(_)
                                        ) {
                                            emit_conversation_update(
                                                &cid,
                                                &h,
                                                agent_type,
                                                notif.update,
                                                None,
                                            );
                                        }
                                        Ok(())
                                    })
                                    .await
                                    .otherwise(async |dispatch| {
                                        maybe_emit_claude_sdk_ext_notification(&cid, &h, dispatch);
                                        Ok(())
                                    })
                                    .await;
                            }
                        }
                        if drained > 0 {
                            eprintln!("[ACP] Drained {drained} historical replay notifications");
                        }

                        emit_session_started(&connections, &emitter_clone, &conn_id, &sid).await;
                        emit_session_modes(&conn_id, &emitter_clone, session.modes());
                        emit_session_config_options(
                            &conn_id,
                            &emitter_clone,
                            agent_type,
                            &initial_config_options,
                        );
                        emit_selectors_ready(&conn_id, &emitter_clone);

                        let loop_result = run_conversation_loop(
                            &mut session,
                            &conn_id,
                            &connections,
                            &emitter_clone,
                            agent_type,
                            &perms,
                            &mut cmd_rx,
                            terminal_runtime.clone(),
                            &cwd_string,
                            supports_fork,
                        )
                        .await;
                        terminal_runtime.release_all_for_session(&sid).await;
                        drop(session);
                        handle_fork_or_exit(
                            loop_result,
                            &conn_id,
                            &connections,
                            &emitter_clone,
                            agent_type,
                            &perms,
                            &mut cmd_rx,
                            terminal_runtime.clone(),
                            &cwd,
                            &cwd_string,
                        )
                        .await
                    }
                    Err(e) => {
                        // session/load failed (e.g. agent doesn't support resume,
                        // or ephemeral forked session).
                        // Fall back to session/new so the tab still works.
                        let err_str = e.to_string();
                        eprintln!(
                            "[ACP] session/load failed ({}), falling back to session/new",
                            err_str
                        );
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
                            emit_connection_error(
                                &connections,
                                &emitter_clone,
                                &conn_id,
                                agent_type,
                                format!("Failed to load session, starting new: {e}"),
                                None,
                            )
                            .await;
                        }
                        let new_resp = cx
                            .send_request_to(Agent, build_new_session_request(agent_type, &cwd))
                            .block_task()
                            .await?;
                        let fallback_sid = new_resp.session_id.0.to_string();
                        let initial_config_options = new_resp.config_options.clone();
                        let mut session = cx.attach_session(new_resp, Default::default())?;
                        emit_session_started(&connections, &emitter_clone, &conn_id, &fallback_sid)
                            .await;
                        emit_session_modes(&conn_id, &emitter_clone, session.modes());
                        emit_session_config_options(
                            &conn_id,
                            &emitter_clone,
                            agent_type,
                            &initial_config_options,
                        );
                        emit_selectors_ready(&conn_id, &emitter_clone);

                        let loop_result = run_conversation_loop(
                            &mut session,
                            &conn_id,
                            &connections,
                            &emitter_clone,
                            agent_type,
                            &perms,
                            &mut cmd_rx,
                            terminal_runtime.clone(),
                            &cwd_string,
                            supports_fork,
                        )
                        .await;
                        terminal_runtime
                            .release_all_for_session(&fallback_sid)
                            .await;
                        drop(session);
                        handle_fork_or_exit(
                            loop_result,
                            &conn_id,
                            &connections,
                            &emitter_clone,
                            agent_type,
                            &perms,
                            &mut cmd_rx,
                            terminal_runtime.clone(),
                            &cwd,
                            &cwd_string,
                        )
                        .await
                    }
                }
            } else {
                // Create new session
                let new_resp = cx
                    .send_request_to(Agent, build_new_session_request(agent_type, &cwd))
                    .block_task()
                    .await?;
                let sid = new_resp.session_id.0.to_string();
                let initial_config_options = new_resp.config_options.clone();
                let mut session = cx.attach_session(new_resp, Default::default())?;
                emit_session_started(&connections, &emitter_clone, &conn_id, &sid).await;
                emit_session_modes(&conn_id, &emitter_clone, session.modes());
                emit_session_config_options(
                    &conn_id,
                    &emitter_clone,
                    agent_type,
                    &initial_config_options,
                );
                emit_selectors_ready(&conn_id, &emitter_clone);

                let loop_result = run_conversation_loop(
                    &mut session,
                    &conn_id,
                    &connections,
                    &emitter_clone,
                    agent_type,
                    &perms,
                    &mut cmd_rx,
                    terminal_runtime.clone(),
                    &cwd_string,
                    supports_fork,
                )
                .await;
                terminal_runtime.release_all_for_session(&sid).await;
                drop(session);
                handle_fork_or_exit(
                    loop_result,
                    &conn_id,
                    &connections,
                    &emitter_clone,
                    agent_type,
                    &perms,
                    &mut cmd_rx,
                    terminal_runtime.clone(),
                    &cwd,
                    &cwd_string,
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
    conn_id: &str,
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

    crate::web::event_bridge::emit_event(
        emitter,
        "acp://event",
        AcpEvent::PermissionRequest {
            connection_id: conn_id.into(),
            request_id,
            tool_call: tool_call_value,
            options,
        },
    );
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
    conn_id: &str,
    emitter: &EventEmitter,
    mode_id: String,
) -> Result<(), sacp::Error> {
    let req = SetSessionModeRequest::new(session.session_id().clone(), mode_id.clone());
    session
        .connection()
        .send_request_to(Agent, req)
        .block_task()
        .await?;

    crate::web::event_bridge::emit_event(
        emitter,
        "acp://event",
        AcpEvent::ModeChanged {
            connection_id: conn_id.into(),
            mode_id,
        },
    );

    Ok(())
}

async fn set_session_config_option(
    cx: &ConnectionTo<Agent>,
    session_id: &SessionId,
    conn_id: &str,
    emitter: &EventEmitter,
    agent_type: AgentType,
    config_id: String,
    value_id: String,
) -> Result<(), sacp::Error> {
    let req = SetSessionConfigOptionRequest::new(session_id.clone(), config_id, value_id);
    let untyped_req = UntypedMessage::new("session/set_config_option", req).map_err(|e| {
        sacp::util::internal_error(format!("Failed to build config option request: {e}"))
    })?;

    let raw_response = cx.send_request_to(Agent, untyped_req).block_task().await?;
    let response: SetSessionConfigOptionResponse =
        serde_json::from_value(raw_response).map_err(|e| {
            sacp::util::internal_error(format!("Failed to parse config option response: {e}"))
        })?;

    emit_session_config_options_values(conn_id, emitter, agent_type, response.config_options);
    Ok(())
}

const TERMINAL_POLL_INTERVAL_MS: u64 = 200;
const TERMINAL_POLL_MISSING_LIMIT: u8 = 10;

/// Maximum time a turn may go without ANY observable signal from the agent
/// (notification, tool poll tick, prompt response, parse warning, etc.)
/// before we consider it stuck. Auto-compaction can take a while, so this
/// is intentionally generous; the watchdog is reset by every event,
/// including compaction markers.
const TURN_IDLE_TIMEOUT_MS: u64 = 180_000;

/// Emit a non-fatal stream warning to the UI. Used when we have to
/// `continue` past a parse error inside the streaming loop — the UI must
/// have *some* signal that something went sideways instead of a silent
/// hang.
fn emit_stream_warning(connection_id: &str, emitter: &EventEmitter, message: impl Into<String>) {
    let message = message.into();
    eprintln!("[ACP] stream warning ({connection_id}): {message}");
    crate::web::event_bridge::emit_event(
        emitter,
        "acp://event",
        AcpEvent::StreamWarning {
            connection_id: connection_id.to_string(),
            message,
        },
    );
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

fn emit_terminal_output_update(
    connection_id: &str,
    emitter: &EventEmitter,
    tool_call_id: &str,
    output: String,
    append: bool,
) {
    crate::web::event_bridge::emit_event(
        emitter,
        "acp://event",
        AcpEvent::ToolCallUpdate {
            connection_id: connection_id.into(),
            tool_call_id: tool_call_id.to_string(),
            title: None,
            status: None,
            content: None,
            raw_input: None,
            raw_output: Some(output),
            raw_output_append: Some(append),
            locations: None,
            meta: None,
        },
    );
}

async fn poll_tracked_terminal_tool_calls(
    terminal_runtime: &TerminalRuntime,
    session_id: &SessionId,
    connection_id: &str,
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
                    eprintln!(
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
            emit_terminal_output_update(
                connection_id,
                emitter,
                &tool_call_id,
                output,
                poll_result.append,
            );
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
    reply: tokio::sync::oneshot::Sender<Result<crate::acp::types::ForkResultInfo, AcpError>>,
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
    connections: &Arc<tokio::sync::Mutex<HashMap<String, AgentConnection>>>,
    emitter: &EventEmitter,
    agent_type: AgentType,
    perms: &PendingPermissions,
    cmd_rx: &mut mpsc::Receiver<ConnectionCommand>,
    terminal_runtime: Arc<TerminalRuntime>,
    _cwd: &std::path::Path,
    cwd_string: &str,
) -> Result<(), sacp::Error> {
    let fork_info = match loop_result {
        Ok(Some(info)) => info,
        Ok(None) => return Ok(()),
        Err(e) => return Err(e),
    };

    let cx = fork_info.connection;
    let fork_resp = fork_info.fork_response;
    let new_sid = fork_resp.session_id.0.to_string();

    eprintln!(
        "[ACP] Fork transition: attaching to forked session {} (original: {})",
        new_sid, fork_info.original_session_id
    );

    // Reply success to the frontend
    let _ = fork_info.reply.send(Ok(crate::acp::types::ForkResultInfo {
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

    emit_session_started(connections, emitter, conn_id, &new_sid).await;
    emit_session_modes(conn_id, emitter, session.modes());
    emit_session_config_options(conn_id, emitter, agent_type, &initial_config_options);
    emit_selectors_ready(conn_id, emitter);

    let loop_result = run_conversation_loop(
        &mut session,
        conn_id,
        connections,
        emitter,
        agent_type,
        perms,
        cmd_rx,
        terminal_runtime.clone(),
        cwd_string,
        true, // fork already succeeded on this process
    )
    .await;
    terminal_runtime.release_all_for_session(&new_sid).await;
    drop(session);

    // Recursively handle nested forks
    Box::pin(handle_fork_or_exit(
        loop_result,
        conn_id,
        connections,
        emitter,
        agent_type,
        perms,
        cmd_rx,
        terminal_runtime,
        _cwd,
        cwd_string,
    ))
    .await
}

/// Main conversation command loop: wait for frontend commands and process them.
///
/// Returns `Ok(None)` on normal exit (disconnect / channel closed) or
/// `Ok(Some(ForkExitInfo))` when the loop should be restarted on a forked session.
#[allow(clippy::too_many_arguments)]
async fn run_conversation_loop<'a>(
    session: &mut sacp::ActiveSession<'a, Agent>,
    conn_id: &str,
    connections: &Arc<tokio::sync::Mutex<HashMap<String, AgentConnection>>>,
    emitter: &EventEmitter,
    agent_type: AgentType,
    perms: &PendingPermissions,
    cmd_rx: &mut mpsc::Receiver<ConnectionCommand>,
    terminal_runtime: Arc<TerminalRuntime>,
    cwd: &str,
    supports_fork: bool,
) -> Result<Option<ForkExitInfo>, sacp::Error> {
    loop {
        // Wait for either a user command or a session update (e.g. available_commands_update)
        let cmd = loop {
            tokio::select! {
                biased;
                cmd = cmd_rx.recv() => break cmd,
                update = session.read_update() => {
                    match update {
                        Ok(SessionMessage::SessionMessage(dispatch)) => {
                            let cid = conn_id.to_string();
                            let h = emitter.clone();
                            let cwd_opt = Some(cwd);
                            let dispatch = fix_usage_update_nulls(dispatch);
                            let _ = MatchDispatch::new(dispatch)
                                .if_notification(
                                    async |notif: SessionNotification| {
                                        emit_conversation_update(&cid, &h, agent_type, notif.update, cwd_opt);
                                        Ok(())
                                    },
                                )
                                .await
                                .otherwise(async |dispatch| {
                                    maybe_emit_claude_sdk_ext_notification(&cid, &h, dispatch);
                                    Ok(())
                                })
                                .await;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            emit_stream_warning(
                                conn_id,
                                emitter,
                                format!("idle stream parse error: {e}"),
                            );
                        }
                    }
                }
            }
        };
        match cmd {
            Some(ConnectionCommand::Prompt { blocks }) => {
                let prompt_blocks = map_prompt_blocks(blocks);
                if prompt_blocks.is_empty() {
                    emit_connection_error(
                        connections,
                        emitter,
                        conn_id,
                        agent_type,
                        "Prompt must contain at least one content block".into(),
                        None,
                    )
                    .await;
                    continue;
                }

                emit_status_changed(connections, emitter, conn_id, ConnectionStatus::Prompting)
                    .await;

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

                // Idle watchdog: if nothing is heard from the agent for
                // TURN_IDLE_TIMEOUT_MS, abort the turn so the UI is not
                // stuck in "prompting" forever. Reset by every observable
                // signal (stream notification, parse warning, terminal
                // poll tick, command, prompt response).
                let idle_deadline =
                    tokio::time::sleep(std::time::Duration::from_millis(TURN_IDLE_TIMEOUT_MS));
                tokio::pin!(idle_deadline);
                // Helper macro: every active branch resets the deadline.
                // Using a macro instead of a closure avoids re-borrowing
                // the pinned future across an await point.
                macro_rules! reset_idle {
                    () => {
                        idle_deadline.as_mut().reset(
                            tokio::time::Instant::now()
                                + std::time::Duration::from_millis(TURN_IDLE_TIMEOUT_MS),
                        );
                    };
                }

                // Read updates until turn completes.
                // We must also listen for commands (e.g. RespondPermission)
                // to avoid deadlocking when the agent awaits a permission response.
                loop {
                    tokio::select! {
                        update = session.read_update() => {
                            // Any signal — even a parse error — proves the
                            // agent process is still alive and producing
                            // output, so reset the idle watchdog.
                            reset_idle!();
                            let update = match update {
                                Ok(u) => u,
                                Err(e) => {
                                    emit_stream_warning(
                                        conn_id,
                                        emitter,
                                        format!("unrecognized session update: {e}"),
                                    );
                                    continue;
                                }
                            };
                            match update {
                                SessionMessage::SessionMessage(dispatch) => {
                                    let cid = conn_id.to_string();
                                    let h = emitter.clone();
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
                                                emit_conversation_update(&cid, &h, agent_type, notif.update, cwd_opt);
                                                if should_poll_now {
                                                    poll_tracked_terminal_tool_calls(
                                                        runtime.as_ref(),
                                                        &session_id,
                                                        &cid,
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
                                            maybe_emit_claude_sdk_ext_notification(&cid, &h, dispatch);
                                            Ok(())
                                        })
                                        .await
                                    {
                                        emit_stream_warning(
                                            conn_id,
                                            emitter,
                                            format!("dispatch parse error: {e}"),
                                        );
                                    }
                                }
                                SessionMessage::StopReason(reason) => {
                                    if !tracked_terminal_tool_calls.is_empty() {
                                        poll_tracked_terminal_tool_calls(
                                            terminal_runtime.as_ref(),
                                            &sid,
                                            conn_id,
                                            emitter,
                                            &mut tracked_terminal_tool_calls,
                                        )
                                        .await;
                                    }
                                    let reason_str = match reason {
                                        StopReason::EndTurn => "end_turn",
                                        StopReason::Cancelled => "cancelled",
                                        _ => "unknown",
                                    };
                                    crate::web::event_bridge::emit_event(
                                        emitter,
                                        "acp://event",
                                        AcpEvent::TurnComplete {
                                            connection_id: conn_id.into(),
                                            session_id: sid.0.to_string(),
                                            stop_reason: reason_str.into(),
                                            agent_type: agent_type.to_string(),
                                        },
                                    );
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
                                    conn_id,
                                    emitter,
                                    &mut tracked_terminal_tool_calls,
                                )
                                .await;
                            }
                            let reason_str = match reason {
                                StopReason::EndTurn => "end_turn",
                                StopReason::Cancelled => "cancelled",
                                _ => "unknown",
                            };
                            crate::web::event_bridge::emit_event(
                                emitter,
                                "acp://event",
                                AcpEvent::TurnComplete {
                                    connection_id: conn_id.into(),
                                    session_id: sid.0.to_string(),
                                    stop_reason: reason_str.into(),
                                    agent_type: agent_type.to_string(),
                                },
                            );
                            break;
                        }
                        _ = terminal_poll_interval.tick(), if !tracked_terminal_tool_calls.is_empty() => {
                            // Active tool polling counts as "we're still
                            // doing work" — keep the watchdog at bay.
                            reset_idle!();
                            poll_tracked_terminal_tool_calls(
                                terminal_runtime.as_ref(),
                                &sid,
                                conn_id,
                                emitter,
                                &mut tracked_terminal_tool_calls,
                            )
                            .await;
                        }
                        _ = &mut idle_deadline => {
                            // No event from the agent for the whole
                            // TURN_IDLE_TIMEOUT_MS window. The turn is
                            // wedged (often because an SDK message we
                            // don't understand killed the stream silently
                            // or because the child process hung after
                            // auto-compaction). Cancel and surface a
                            // synthetic TurnIdleTimeout + TurnComplete so
                            // the UI exits "prompting" cleanly.
                            eprintln!(
                                "[ACP] turn idle timeout after {}ms; cancelling. connection_id={conn_id}",
                                TURN_IDLE_TIMEOUT_MS
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
                            drop(locked);
                            crate::web::event_bridge::emit_event(
                                emitter,
                                "acp://event",
                                AcpEvent::TurnIdleTimeout {
                                    connection_id: conn_id.into(),
                                    session_id: sid.0.to_string(),
                                    agent_type: agent_type.to_string(),
                                    idle_ms: TURN_IDLE_TIMEOUT_MS,
                                },
                            );
                            crate::web::event_bridge::emit_event(
                                emitter,
                                "acp://event",
                                AcpEvent::TurnComplete {
                                    connection_id: conn_id.into(),
                                    session_id: sid.0.to_string(),
                                    stop_reason: "idle_timeout".into(),
                                    agent_type: agent_type.to_string(),
                                },
                            );
                            // Drain prompt_response in the background;
                            // the agent may still respond eventually.
                            tokio::spawn(async move {
                                let _ = prompt_response.await;
                            });
                            break;
                        }
                        cmd = cmd_rx.recv() => {
                            // User-initiated commands (cancel, permission
                            // response, etc.) are also activity and reset
                            // the watchdog.
                            reset_idle!();
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
                                    }
                                }
                                Some(ConnectionCommand::SetMode { mode_id }) => {
                                    let req = SetSessionModeRequest::new(sid.clone(), mode_id.clone());
                                    match cx.send_request_to(Agent, req).block_task().await {
                                        Ok(_) => {
                                            crate::web::event_bridge::emit_event(
                                                emitter,
                                                "acp://event",
                                                AcpEvent::ModeChanged {
                                                    connection_id: conn_id.into(),
                                                    mode_id,
                                                },
                                            );
                                        }
                                        Err(e) => {
                                            emit_connection_error(
                                                connections,
                                                emitter,
                                                conn_id,
                                                agent_type,
                                                format!("Failed to set mode: {e}"),
                                                None,
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
                                        conn_id,
                                        emitter,
                                        agent_type,
                                        config_id,
                                        value_id,
                                    )
                                    .await
                                    {
                                        emit_connection_error(
                                            connections,
                                            emitter,
                                            conn_id,
                                            agent_type,
                                            format!("Failed to set config option: {e}"),
                                            None,
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
                                    // Immediately emit TurnComplete so the frontend
                                    // transitions out of "prompting" and the user can
                                    // send new messages.  Don't wait for the agent --
                                    // it may be slow to respond or not respond at all.
                                    crate::web::event_bridge::emit_event(
                                        emitter,
                                        "acp://event",
                                        AcpEvent::TurnComplete {
                                            connection_id: conn_id.into(),
                                            session_id: sid.0.to_string(),
                                            stop_reason: "cancelled".into(),
                                            agent_type: agent_type.to_string(),
                                        },
                                    );
                                    // Drain the prompt response in the background so
                                    // the SACP library doesn't log "receiver dropped"
                                    // errors when the agent eventually responds.
                                    tokio::spawn(async move {
                                        let _ = prompt_response.await;
                                    });
                                    break;
                                }
                                Some(ConnectionCommand::Disconnect) | None => {
                                    eprintln!(
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
                    eprintln!(
                        "[ACP] closing connection loop after disconnect; connection_id={conn_id}"
                    );
                    break;
                }

                emit_status_changed(connections, emitter, conn_id, ConnectionStatus::Connected)
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
                }
            }
            Some(ConnectionCommand::SetMode { mode_id }) => {
                if let Err(e) = set_session_mode(session, conn_id, emitter, mode_id).await {
                    emit_connection_error(
                        connections,
                        emitter,
                        conn_id,
                        agent_type,
                        format!("Failed to set mode: {e}"),
                        None,
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
                    &cx, &sid, conn_id, emitter, agent_type, config_id, value_id,
                )
                .await
                {
                    emit_connection_error(
                        connections,
                        emitter,
                        conn_id,
                        agent_type,
                        format!("Failed to set config option: {e}"),
                        None,
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
                eprintln!(
                    "[ACP] Sending session/fork for session_id={} cwd={}",
                    sid.0, cwd
                );
                let result = crate::acp::fork::fork_session(&cx, &sid, cwd).await;
                match result {
                    Ok(fork_response) => {
                        eprintln!(
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
                        eprintln!("[ACP] Fork failed: {e}");
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

/// Classify a `_claude/sdkMessage` payload's compaction state.
///
/// The Claude Agent SDK announces auto-compaction with one of several
/// markers depending on version: a `system/compact_boundary` notification,
/// a `system/compact` system message, or an inline `subtype: "compact"` /
/// `prompt_too_long` recovery message. We treat any `compact_boundary`
/// / `compact_started` / `prompt_too_long` as the start of compaction
/// and `compact_finished` / `compacted` as the end. When the SDK only
/// emits a single boundary marker we still surface it as
/// `CompactionStarted` so the UI can display "compacting…" and reset
/// its watchdog; the next non-system event will naturally end the hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionSignal {
    Started,
    Finished,
}

fn classify_compaction_signal(message: &serde_json::Value) -> Option<CompactionSignal> {
    let obj = message.as_object()?;
    let message_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let subtype = obj.get("subtype").and_then(|v| v.as_str()).unwrap_or("");

    // Common SDK markers, normalized.
    match (message_type, subtype) {
        ("system", "compact_boundary")
        | ("system", "compact")
        | ("system", "compact_started")
        | ("system", "compacting")
        | ("system", "prompt_too_long") => Some(CompactionSignal::Started),
        ("system", "compact_finished")
        | ("system", "compacted")
        | ("system", "compact_complete") => Some(CompactionSignal::Finished),
        _ => None,
    }
}

fn map_claude_sdk_ext_notification(
    connection_id: &str,
    notification: &UntypedMessage,
) -> Option<AcpEvent> {
    if notification.method() != "_claude/sdkMessage" {
        return None;
    }

    let (session_id, message) = parse_claude_sdk_message_params(notification.params())?;

    // Compaction takes priority: the UI needs to know the agent is
    // alive but busy summarizing.
    if let Some(signal) = classify_compaction_signal(&message) {
        return Some(match signal {
            CompactionSignal::Started => AcpEvent::CompactionStarted {
                connection_id: connection_id.to_string(),
                session_id,
            },
            CompactionSignal::Finished => AcpEvent::CompactionFinished {
                connection_id: connection_id.to_string(),
                session_id,
            },
        });
    }

    if !is_claude_api_retry_message(&message) {
        return None;
    }
    Some(AcpEvent::ClaudeSdkMessage {
        connection_id: connection_id.to_string(),
        session_id,
        message,
    })
}

fn maybe_emit_claude_sdk_ext_notification(
    connection_id: &str,
    emitter: &EventEmitter,
    dispatch: Dispatch,
) {
    let Dispatch::Notification(notification) = dispatch else {
        return;
    };

    if let Some(event) = map_claude_sdk_ext_notification(connection_id, &notification) {
        crate::web::event_bridge::emit_event(emitter, "acp://event", event);
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
fn emit_conversation_update(
    connection_id: &str,
    emitter: &EventEmitter,
    agent_type: AgentType,
    update: SessionUpdate,
    cwd: Option<&str>,
) {
    match update {
        SessionUpdate::UserMessageChunk(_) => {
            // User echo chunks are informational for transcript sync and
            // currently not rendered in live ACP UI.
        }
        SessionUpdate::AgentMessageChunk(ContentChunk {
            content: ContentBlock::Text(text),
            ..
        }) => {
            crate::web::event_bridge::emit_event(
                emitter,
                "acp://event",
                AcpEvent::ContentDelta {
                    connection_id: connection_id.into(),
                    text: text.text,
                },
            );
        }
        SessionUpdate::AgentMessageChunk(_) => {
            // Non-text chunks are currently not surfaced in live streaming UI.
        }
        SessionUpdate::AgentThoughtChunk(ContentChunk {
            content: ContentBlock::Text(text),
            ..
        }) => {
            crate::web::event_bridge::emit_event(
                emitter,
                "acp://event",
                AcpEvent::Thinking {
                    connection_id: connection_id.into(),
                    text: text.text,
                },
            );
        }
        SessionUpdate::AgentThoughtChunk(_) => {
            // Non-text thought chunks are currently ignored.
        }
        SessionUpdate::ToolCall(tc) => {
            let content = serialize_tool_call_content(&tc.content);
            let raw_input =
                json_value_to_text(&tc.raw_input).map(|text| resolve_live_tool_input(&text, cwd));
            let raw_output =
                json_value_to_text(&tc.raw_output).map(|text| structurize_live_output(&text));
            let locations = if tc.locations.is_empty() {
                None
            } else {
                serde_json::to_value(&tc.locations).ok()
            };
            let meta = tc.meta.map(serde_json::Value::Object);
            crate::web::event_bridge::emit_event(
                emitter,
                "acp://event",
                AcpEvent::ToolCall {
                    connection_id: connection_id.into(),
                    tool_call_id: tc.tool_call_id.to_string(),
                    title: tc.title,
                    kind: format!("{:?}", tc.kind).to_lowercase(),
                    status: format!("{:?}", tc.status).to_lowercase(),
                    content,
                    raw_input,
                    raw_output,
                    locations,
                    meta,
                },
            );
        }
        SessionUpdate::ToolCallUpdate(tcu) => {
            let content = tcu
                .fields
                .content
                .as_deref()
                .and_then(serialize_tool_call_content);
            let raw_input = json_value_to_text(&tcu.fields.raw_input)
                .map(|text| resolve_live_tool_input(&text, cwd));
            let raw_output = json_value_to_text(&tcu.fields.raw_output)
                .map(|text| structurize_live_output(&text));
            let locations = tcu
                .fields
                .locations
                .as_ref()
                .filter(|l| !l.is_empty())
                .and_then(|l| serde_json::to_value(l).ok());
            let meta = tcu.meta.clone().map(serde_json::Value::Object);
            crate::web::event_bridge::emit_event(
                emitter,
                "acp://event",
                AcpEvent::ToolCallUpdate {
                    connection_id: connection_id.into(),
                    tool_call_id: tcu.tool_call_id.to_string(),
                    title: tcu.fields.title,
                    status: tcu.fields.status.map(|s| format!("{:?}", s).to_lowercase()),
                    content,
                    raw_input,
                    raw_output,
                    raw_output_append: None,
                    locations,
                    meta,
                },
            );
        }
        SessionUpdate::CurrentModeUpdate(update) => {
            crate::web::event_bridge::emit_event(
                emitter,
                "acp://event",
                AcpEvent::ModeChanged {
                    connection_id: connection_id.into(),
                    mode_id: update.current_mode_id.to_string(),
                },
            );
        }
        SessionUpdate::Plan(plan) => {
            crate::web::event_bridge::emit_event(
                emitter,
                "acp://event",
                AcpEvent::PlanUpdate {
                    connection_id: connection_id.into(),
                    entries: map_plan_entries(&plan),
                },
            );
        }
        SessionUpdate::ConfigOptionUpdate(update) => {
            emit_session_config_options_values(
                connection_id,
                emitter,
                agent_type,
                update.config_options,
            );
        }
        SessionUpdate::AvailableCommandsUpdate(update) => {
            let commands: Vec<AvailableCommandInfo> = update
                .available_commands
                .iter()
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
            crate::web::event_bridge::emit_event(
                emitter,
                "acp://event",
                AcpEvent::AvailableCommands {
                    connection_id: connection_id.into(),
                    commands,
                },
            );
        }
        SessionUpdate::UsageUpdate(update) => {
            crate::web::event_bridge::emit_event(
                emitter,
                "acp://event",
                AcpEvent::UsageUpdate {
                    connection_id: connection_id.into(),
                    used: update.used,
                    size: update.size,
                },
            );
        }
        other => {
            // Log unhandled update types for debugging
            eprintln!("[ACP] Unhandled SessionUpdate: {:?}", other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let event =
            map_claude_sdk_ext_notification("conn-1", &raw).expect("valid sdk payload should map");

        match event {
            AcpEvent::ClaudeSdkMessage {
                connection_id,
                session_id,
                message,
            } => {
                assert_eq!(connection_id, "conn-1");
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
        assert!(map_claude_sdk_ext_notification("conn-1", &non_retry).is_none());
    }

    #[test]
    fn map_claude_sdk_ext_notification_rejects_invalid_payload() {
        let wrong_method = UntypedMessage::new(
            "_other/method",
            serde_json::json!({"sessionId": "s", "message": {}}),
        )
        .unwrap();
        assert!(map_claude_sdk_ext_notification("conn-1", &wrong_method).is_none());

        let missing_fields =
            UntypedMessage::new("_claude/sdkMessage", serde_json::json!({"sessionId": 1})).unwrap();
        assert!(map_claude_sdk_ext_notification("conn-1", &missing_fields).is_none());
    }

    #[test]
    fn build_new_session_request_sets_claude_raw_meta() {
        let cwd = std::path::PathBuf::from("/tmp/codeg");
        let req = build_new_session_request(AgentType::ClaudeCode, &cwd);

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
        let req =
            build_load_session_request(AgentType::Codex, SessionId::new("abc".to_string()), &cwd);

        assert!(req.meta.is_none());
    }
}
