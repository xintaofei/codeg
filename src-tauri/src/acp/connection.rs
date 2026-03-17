use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use sacp::schema::McpServerStdio;
use sacp::schema::{
    BlobResourceContents, CancelNotification, ClientCapabilities, ContentBlock, ContentChunk,
    CreateTerminalRequest, CreateTerminalResponse, EmbeddedResource, EmbeddedResourceResource,
    FileSystemCapability, ImageContent, InitializeRequest, KillTerminalCommandRequest,
    KillTerminalCommandResponse, LoadSessionRequest, NewSessionRequest, NewSessionResponse,
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
    on_receive_request, Agent, Client, ConnectionTo, Responder, SessionMessage, UntypedMessage,
};
use sacp_tokio::AcpAgent;
use tauri::Emitter;
use tokio::sync::mpsc;

use crate::acp::error::AcpError;
use crate::acp::file_system_runtime::{FileSystemRuntime, FileSystemRuntimeError};
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

/// Represents a single active ACP agent connection.
pub struct AgentConnection {
    pub id: String,
    pub agent_type: AgentType,
    pub status: ConnectionStatus,
    pub owner_window_label: String,
    pub cmd_tx: mpsc::Sender<ConnectionCommand>,
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
async fn build_agent(
    agent_type: AgentType,
    runtime_env: &BTreeMap<String, String>,
    connection_id: &str,
    app_handle: &tauri::AppHandle,
) -> Result<AcpAgent, AcpError> {
    let meta = registry::get_agent_meta(agent_type);
    debug_assert_eq!(meta.agent_type, agent_type);

    match meta.distribution {
        AgentDistribution::Npx {
            cmd, args, env, ..
        } => {
            let merged_env = merge_agent_env(env, runtime_env);
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in &merged_env {
                parts.push(format!("{k}={v}"));
            }
            parts.push(
                crate::process::normalized_program(cmd)
                    .to_string_lossy()
                    .to_string(),
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
                    .map_or(false, |v| v == "1")
                {
                    parts.push("--reset-session".into());
                }
            }
            let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
            AcpAgent::from_args(&refs).map_err(|e| AcpError::SpawnFailed(e.to_string()))
        }
        AgentDistribution::Binary {
            version,
            cmd,
            args,
            env,
            platforms,
        } => {
            let platform = registry::current_platform();
            let info = platforms
                .iter()
                .find(|p| p.platform == platform)
                .ok_or_else(|| {
                    AcpError::PlatformNotSupported(format!(
                        "{} is not available on {platform}",
                        meta.name
                    ))
                })?;

            let has_cached_binary =
                crate::acp::binary_cache::find_cached_binary_for_agent(agent_type, version, cmd)
                    .ok()
                    .flatten()
                    .is_some();
            if !has_cached_binary {
                let _ = app_handle.emit(
                    "acp://event",
                    AcpEvent::StatusChanged {
                        connection_id: connection_id.into(),
                        status: ConnectionStatus::Downloading,
                    },
                );
            }
            let binary_path = crate::acp::binary_cache::ensure_binary_for_agent(
                agent_type, version, info.url, cmd,
            )
            .await?;

            let binary_str = binary_path.to_string_lossy().to_string();
            let mut server = McpServerStdio::new(meta.name, &binary_str);
            let cmd_args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
            if !cmd_args.is_empty() {
                server = server.args(cmd_args);
            }
            let merged_env = merge_agent_env(env, runtime_env);
            if !merged_env.is_empty() {
                let env_vars: Vec<sacp::schema::EnvVariable> = merged_env
                    .iter()
                    .map(|(k, v)| sacp::schema::EnvVariable::new(k, v))
                    .collect();
                server = server.env(env_vars);
            }
            Ok(AcpAgent::new(sacp::schema::McpServer::Stdio(server)))
        }
    }
}

/// Spawn an ACP agent process and run the connection loop in a background task.
pub async fn spawn_agent_connection(
    connection_id: String,
    agent_type: AgentType,
    working_dir: Option<String>,
    session_id: Option<String>,
    runtime_env: BTreeMap<String, String>,
    owner_window_label: String,
    app_handle: tauri::AppHandle,
) -> Result<AgentConnection, AcpError> {
    let _ = app_handle.emit(
        "acp://event",
        AcpEvent::StatusChanged {
            connection_id: connection_id.clone(),
            status: ConnectionStatus::Connecting,
        },
    );

    let agent = build_agent(agent_type, &runtime_env, &connection_id, &app_handle).await?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<ConnectionCommand>(32);
    let conn_id = connection_id.clone();
    let handle = app_handle.clone();

    tokio::spawn(async move {
        let result = run_connection(
            agent,
            conn_id.clone(),
            working_dir,
            session_id,
            cmd_rx,
            handle.clone(),
        )
        .await;

        if let Err(e) = result {
            let _ = handle.emit(
                "acp://event",
                AcpEvent::Error {
                    connection_id: conn_id.clone(),
                    message: e.to_string(),
                },
            );
        }

        let _ = handle.emit(
            "acp://event",
            AcpEvent::StatusChanged {
                connection_id: conn_id,
                status: ConnectionStatus::Disconnected,
            },
        );
    });

    Ok(AgentConnection {
        id: connection_id,
        agent_type,
        status: ConnectionStatus::Connecting,
        owner_window_label,
        cmd_tx,
    })
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
    app_handle: &tauri::AppHandle,
    modes: &Option<SessionModeState>,
) {
    if let Some(mode_state) = modes {
        let _ = app_handle.emit(
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

fn emit_session_config_options_values(
    connection_id: &str,
    app_handle: &tauri::AppHandle,
    config_options: Vec<SessionConfigOption>,
) {
    let mapped = map_session_config_options(&config_options);
    let _ = app_handle.emit(
        "acp://event",
        AcpEvent::SessionConfigOptions {
            connection_id: connection_id.into(),
            config_options: mapped,
        },
    );
}

fn emit_session_config_options(
    connection_id: &str,
    app_handle: &tauri::AppHandle,
    config_options: &Option<Vec<SessionConfigOption>>,
) {
    // Always emit one config-options snapshot after session attach.
    // Some agents (e.g. Gemini CLI) may not expose session config options
    // and return `None`; emitting an empty list lets the frontend settle
    // loading state instead of waiting forever.
    let options = config_options.clone().unwrap_or_default();
    emit_session_config_options_values(connection_id, app_handle, options);
}

fn emit_selectors_ready(connection_id: &str, app_handle: &tauri::AppHandle) {
    let _ = app_handle.emit(
        "acp://event",
        AcpEvent::SelectorsReady {
            connection_id: connection_id.into(),
        },
    );
}

fn emit_prompt_capabilities(
    connection_id: &str,
    app_handle: &tauri::AppHandle,
    capabilities: &sacp::schema::PromptCapabilities,
) {
    let _ = app_handle.emit(
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

/// The main ACP connection loop.
async fn run_connection(
    agent: AcpAgent,
    connection_id: String,
    working_dir: Option<String>,
    session_id: Option<String>,
    mut cmd_rx: mpsc::Receiver<ConnectionCommand>,
    app_handle: tauri::AppHandle,
) -> Result<(), AcpError> {
    let pending_perms: PendingPermissions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let terminal_runtime = Arc::new(TerminalRuntime::new());
    let cwd = resolve_working_dir(working_dir.as_deref());
    let cwd_string = cwd.to_string_lossy().to_string();
    let file_system_runtime = Arc::new(FileSystemRuntime::new(cwd.clone()));

    let conn_id = connection_id.clone();
    let handle = app_handle.clone();
    let perms = pending_perms.clone();

    Client
        .builder()
        .name("codeg")
        .on_receive_request(
            {
                let conn_id = conn_id.clone();
                let handle = handle.clone();
                let perms = perms.clone();
                async move |req: RequestPermissionRequest,
                            responder: Responder<RequestPermissionResponse>,
                            _cx: ConnectionTo<Agent>| {
                    handle_permission_request(&conn_id, &handle, &perms, req, responder).await;
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
                async move |req: KillTerminalCommandRequest,
                            responder: Responder<KillTerminalCommandResponse>,
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
        .connect_with(agent, async move |cx| {
            // Advertise filesystem + terminal capabilities for ACP tool execution.
            let init_request = InitializeRequest::new(ProtocolVersion::LATEST).client_capabilities(
                ClientCapabilities::new()
                    .terminal(true)
                    .fs(FileSystemCapability::new()
                        .read_text_file(true)
                        .write_text_file(true)),
            );
            let init_resp = cx.send_request_to(Agent, init_request).block_task().await?;
            emit_prompt_capabilities(
                &conn_id,
                &handle,
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
            let _ = handle.emit(
                "acp://event",
                AcpEvent::ForkSupported {
                    connection_id: conn_id.clone(),
                    supported: supports_fork,
                },
            );

            // Emit connected status
            let _ = handle.emit(
                "acp://event",
                AcpEvent::StatusChanged {
                    connection_id: conn_id.clone(),
                    status: ConnectionStatus::Connected,
                },
            );

            if let Some(sid) = session_id {
                // Load existing session via session/load
                let load_req = LoadSessionRequest::new(SessionId::new(sid.clone()), &cwd);
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
                                let h = handle.clone();
                                let _ = MatchDispatch::new(dispatch)
                                    .if_notification(async |notif: SessionNotification| {
                                        if matches!(
                                            notif.update,
                                            SessionUpdate::AvailableCommandsUpdate(_)
                                        ) {
                                            emit_conversation_update(&cid, &h, notif.update);
                                        }
                                        Ok(())
                                    })
                                    .await
                                    .otherwise_ignore();
                            }
                        }
                        if drained > 0 {
                            eprintln!("[ACP] Drained {drained} historical replay notifications");
                        }

                        let _ = handle.emit(
                            "acp://event",
                            AcpEvent::SessionStarted {
                                connection_id: conn_id.clone(),
                                session_id: sid.clone(),
                            },
                        );
                        emit_session_modes(&conn_id, &handle, session.modes());
                        emit_session_config_options(&conn_id, &handle, &initial_config_options);
                        emit_selectors_ready(&conn_id, &handle);

                        let loop_result = run_conversation_loop(
                            &mut session,
                            &conn_id,
                            &handle,
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
                            &handle,
                            &perms,
                            &mut cmd_rx,
                            terminal_runtime.clone(),
                            &cwd,
                            &cwd_string,
                        )
                        .await
                    }
                    Err(e) => {
                        // session/load failed (e.g. ephemeral forked session).
                        // Fall back to session/new so the tab still works.
                        eprintln!(
                            "[ACP] session/load failed ({}), falling back to session/new",
                            e
                        );
                        let _ = handle.emit(
                            "acp://event",
                            AcpEvent::Error {
                                connection_id: conn_id.clone(),
                                message: format!("Failed to load session, starting new: {e}"),
                            },
                        );
                        let new_resp = cx
                            .send_request_to(Agent, NewSessionRequest::new(cwd.clone()))
                            .block_task()
                            .await?;
                        let fallback_sid = new_resp.session_id.0.to_string();
                        let initial_config_options = new_resp.config_options.clone();
                        let mut session =
                            cx.attach_session(new_resp, Default::default())?;
                        let _ = handle.emit(
                            "acp://event",
                            AcpEvent::SessionStarted {
                                connection_id: conn_id.clone(),
                                session_id: fallback_sid.clone(),
                            },
                        );
                        emit_session_modes(&conn_id, &handle, session.modes());
                        emit_session_config_options(
                            &conn_id,
                            &handle,
                            &initial_config_options,
                        );
                        emit_selectors_ready(&conn_id, &handle);

                        let loop_result = run_conversation_loop(
                            &mut session,
                            &conn_id,
                            &handle,
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
                            &handle,
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
                    .send_request_to(Agent, NewSessionRequest::new(cwd.clone()))
                    .block_task()
                    .await?;
                let sid = new_resp.session_id.0.to_string();
                let initial_config_options = new_resp.config_options.clone();
                let mut session = cx.attach_session(new_resp, Default::default())?;
                let _ = handle.emit(
                    "acp://event",
                    AcpEvent::SessionStarted {
                        connection_id: conn_id.clone(),
                        session_id: sid.clone(),
                    },
                );
                emit_session_modes(&conn_id, &handle, session.modes());
                emit_session_config_options(&conn_id, &handle, &initial_config_options);
                emit_selectors_ready(&conn_id, &handle);

                let loop_result = run_conversation_loop(
                    &mut session,
                    &conn_id,
                    &handle,
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
                    &handle,
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
        .map_err(|e| AcpError::protocol(e.to_string()))
}

/// Store the permission responder and emit event to frontend.
async fn handle_permission_request(
    conn_id: &str,
    handle: &tauri::AppHandle,
    perms: &PendingPermissions,
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

    let tool_call_value = serde_json::to_value(&req.tool_call).unwrap_or_default();

    perms.lock().await.insert(request_id.clone(), responder);

    let _ = handle.emit(
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
    handle: &tauri::AppHandle,
    mode_id: String,
) -> Result<(), sacp::Error> {
    let req = SetSessionModeRequest::new(session.session_id().clone(), mode_id.clone());
    session
        .connection()
        .send_request_to(Agent, req)
        .block_task()
        .await?;

    let _ = handle.emit(
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
    handle: &tauri::AppHandle,
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

    emit_session_config_options_values(conn_id, handle, response.config_options);
    Ok(())
}

const TERMINAL_POLL_INTERVAL_MS: u64 = 200;
const TERMINAL_POLL_MISSING_LIMIT: u8 = 10;

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
    app_handle: &tauri::AppHandle,
    tool_call_id: &str,
    output: String,
    append: bool,
) {
    let _ = app_handle.emit(
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
        },
    );
}

async fn poll_tracked_terminal_tool_calls(
    terminal_runtime: &TerminalRuntime,
    session_id: &SessionId,
    connection_id: &str,
    app_handle: &tauri::AppHandle,
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
                app_handle,
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
async fn handle_fork_or_exit(
    loop_result: Result<Option<ForkExitInfo>, sacp::Error>,
    conn_id: &str,
    handle: &tauri::AppHandle,
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

    let _ = handle.emit(
        "acp://event",
        AcpEvent::SessionStarted {
            connection_id: conn_id.to_string(),
            session_id: new_sid.clone(),
        },
    );
    emit_session_modes(conn_id, handle, session.modes());
    emit_session_config_options(conn_id, handle, &initial_config_options);
    emit_selectors_ready(conn_id, handle);

    let loop_result = run_conversation_loop(
        &mut session,
        conn_id,
        handle,
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
        loop_result, conn_id, handle, perms, cmd_rx, terminal_runtime, _cwd, cwd_string,
    ))
    .await
}

/// Main conversation command loop: wait for frontend commands and process them.
///
/// Returns `Ok(None)` on normal exit (disconnect / channel closed) or
/// `Ok(Some(ForkExitInfo))` when the loop should be restarted on a forked session.
async fn run_conversation_loop<'a>(
    session: &mut sacp::ActiveSession<'a, Agent>,
    conn_id: &str,
    handle: &tauri::AppHandle,
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
                            let h = handle.clone();
                            let _ = MatchDispatch::new(dispatch)
                                .if_notification(
                                    async |notif: SessionNotification| {
                                        emit_conversation_update(&cid, &h, notif.update);
                                        Ok(())
                                    },
                                )
                                .await
                                .otherwise_ignore();
                        }
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("[ACP] Ignoring unrecognized session update in idle loop: {e}");
                        }
                    }
                }
            }
        };
        match cmd {
            Some(ConnectionCommand::Prompt { blocks }) => {
                let prompt_blocks = map_prompt_blocks(blocks);
                if prompt_blocks.is_empty() {
                    let _ = handle.emit(
                        "acp://event",
                        AcpEvent::Error {
                            connection_id: conn_id.into(),
                            message: "Prompt must contain at least one content block".into(),
                        },
                    );
                    continue;
                }

                let _ = handle.emit(
                    "acp://event",
                    AcpEvent::StatusChanged {
                        connection_id: conn_id.into(),
                        status: ConnectionStatus::Prompting,
                    },
                );

                // Clone connection handle and session ID before entering the
                // select loop so we can send CancelNotification without
                // conflicting with session.read_update()'s mutable borrow.
                let cx = session.connection();
                let sid = session.session_id().clone();
                let prompt_request = PromptRequest::new(sid.clone(), prompt_blocks);
                let prompt_response = cx
                    .clone()
                    .send_request_to(Agent, prompt_request)
                    .block_task();
                tokio::pin!(prompt_response);
                let mut tracked_terminal_tool_calls: HashMap<String, TrackedTerminalToolCall> =
                    HashMap::new();
                let mut terminal_poll_interval = tokio::time::interval(
                    std::time::Duration::from_millis(TERMINAL_POLL_INTERVAL_MS),
                );
                terminal_poll_interval
                    .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut disconnect_requested = false;

                // Read updates until turn completes.
                // We must also listen for commands (e.g. RespondPermission)
                // to avoid deadlocking when the agent awaits a permission response.
                loop {
                    tokio::select! {
                        update = session.read_update() => {
                            let update = match update {
                                Ok(u) => u,
                                Err(e) => {
                                    eprintln!("[ACP] Ignoring unrecognized session update: {e}");
                                    continue;
                                }
                            };
                            match update {
                                SessionMessage::SessionMessage(dispatch) => {
                                    let cid = conn_id.to_string();
                                    let h = handle.clone();
                                    let runtime = terminal_runtime.clone();
                                    let session_id = sid.clone();
                                    if let Err(e) = MatchDispatch::new(dispatch)
                                        .if_notification(
                                            async |notif: SessionNotification| {
                                                let should_poll_now = track_terminal_tool_calls(
                                                    &notif.update,
                                                    &mut tracked_terminal_tool_calls,
                                                );
                                                emit_conversation_update(&cid, &h, notif.update);
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
                                        .otherwise_ignore()
                                    {
                                        eprintln!("[ACP] Ignoring dispatch parse error: {e}");
                                    }
                                }
                                SessionMessage::StopReason(reason) => {
                                    if !tracked_terminal_tool_calls.is_empty() {
                                        poll_tracked_terminal_tool_calls(
                                            terminal_runtime.as_ref(),
                                            &sid,
                                            conn_id,
                                            handle,
                                            &mut tracked_terminal_tool_calls,
                                        )
                                        .await;
                                    }
                                    let reason_str = match reason {
                                        StopReason::EndTurn => "end_turn",
                                        StopReason::Cancelled => "cancelled",
                                        _ => "unknown",
                                    };
                                    let _ = handle.emit(
                                        "acp://event",
                                        AcpEvent::TurnComplete {
                                            connection_id: conn_id.into(),
                                            session_id: sid.0.to_string(),
                                            stop_reason: reason_str.into(),
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
                                    handle,
                                    &mut tracked_terminal_tool_calls,
                                )
                                .await;
                            }
                            let reason_str = match reason {
                                StopReason::EndTurn => "end_turn",
                                StopReason::Cancelled => "cancelled",
                                _ => "unknown",
                            };
                            let _ = handle.emit(
                                "acp://event",
                                AcpEvent::TurnComplete {
                                    connection_id: conn_id.into(),
                                    session_id: sid.0.to_string(),
                                    stop_reason: reason_str.into(),
                                },
                            );
                            break;
                        }
                        _ = terminal_poll_interval.tick(), if !tracked_terminal_tool_calls.is_empty() => {
                            poll_tracked_terminal_tool_calls(
                                terminal_runtime.as_ref(),
                                &sid,
                                conn_id,
                                handle,
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
                                    }
                                }
                                Some(ConnectionCommand::SetMode { mode_id }) => {
                                    let req = SetSessionModeRequest::new(sid.clone(), mode_id.clone());
                                    match cx.send_request_to(Agent, req).block_task().await {
                                        Ok(_) => {
                                            let _ = handle.emit(
                                                "acp://event",
                                                AcpEvent::ModeChanged {
                                                    connection_id: conn_id.into(),
                                                    mode_id,
                                                },
                                            );
                                        }
                                        Err(e) => {
                                            let _ = handle.emit(
                                                "acp://event",
                                                AcpEvent::Error {
                                                    connection_id: conn_id.into(),
                                                    message: format!("Failed to set mode: {e}"),
                                                },
                                            );
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
                                        handle,
                                        config_id,
                                        value_id,
                                    )
                                    .await
                                    {
                                        let _ = handle.emit(
                                            "acp://event",
                                            AcpEvent::Error {
                                                connection_id: conn_id.into(),
                                                message: format!("Failed to set config option: {e}"),
                                            },
                                        );
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

                let _ = handle.emit(
                    "acp://event",
                    AcpEvent::StatusChanged {
                        connection_id: conn_id.into(),
                        status: ConnectionStatus::Connected,
                    },
                );
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
                if let Err(e) = set_session_mode(session, conn_id, handle, mode_id).await {
                    let _ = handle.emit(
                        "acp://event",
                        AcpEvent::Error {
                            connection_id: conn_id.into(),
                            message: format!("Failed to set mode: {e}"),
                        },
                    );
                }
            }
            Some(ConnectionCommand::SetConfigOption {
                config_id,
                value_id,
            }) => {
                let cx = session.connection();
                let sid = session.session_id().clone();
                if let Err(e) =
                    set_session_config_option(&cx, &sid, conn_id, handle, config_id, value_id).await
                {
                    let _ = handle.emit(
                        "acp://event",
                        AcpEvent::Error {
                            connection_id: conn_id.into(),
                            message: format!("Failed to set config option: {e}"),
                        },
                    );
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

/// Convert a SessionUpdate into AcpEvent(s) and emit to frontend.
fn emit_conversation_update(
    connection_id: &str,
    app_handle: &tauri::AppHandle,
    update: SessionUpdate,
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
            let _ = app_handle.emit(
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
            let _ = app_handle.emit(
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
            let raw_input = json_value_to_text(&tc.raw_input);
            let raw_output = json_value_to_text(&tc.raw_output);
            let _ = app_handle.emit(
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
                },
            );
        }
        SessionUpdate::ToolCallUpdate(tcu) => {
            let content = tcu
                .fields
                .content
                .as_deref()
                .and_then(serialize_tool_call_content);
            let raw_input = json_value_to_text(&tcu.fields.raw_input);
            let raw_output = json_value_to_text(&tcu.fields.raw_output);
            let _ = app_handle.emit(
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
                },
            );
        }
        SessionUpdate::CurrentModeUpdate(update) => {
            let _ = app_handle.emit(
                "acp://event",
                AcpEvent::ModeChanged {
                    connection_id: connection_id.into(),
                    mode_id: update.current_mode_id.to_string(),
                },
            );
        }
        SessionUpdate::Plan(plan) => {
            let _ = app_handle.emit(
                "acp://event",
                AcpEvent::PlanUpdate {
                    connection_id: connection_id.into(),
                    entries: map_plan_entries(&plan),
                },
            );
        }
        SessionUpdate::ConfigOptionUpdate(update) => {
            emit_session_config_options_values(connection_id, app_handle, update.config_options);
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
            let _ = app_handle.emit(
                "acp://event",
                AcpEvent::AvailableCommands {
                    connection_id: connection_id.into(),
                    commands,
                },
            );
        }
        SessionUpdate::UsageUpdate(update) => {
            let _ = app_handle.emit(
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
