//! Mid-conversation agent handoff commands (`acp_handoff_plan`,
//! `acp_handoff`). The pure halves (path decision, transcript copy, briefing,
//! divider splice) live in `acp::handoff`; this module is the choreography:
//! stop the source, move or brief, spawn the target, verify it took the
//! session, and only then move the conversation row.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;
#[cfg(feature = "tauri-runtime")]
use tauri::{Manager, State};

use crate::acp::handoff::{
    self, build_briefing, BriefingInput, HandoffPath, DEFAULT_BRIEFING_BUDGET,
};
use crate::acp::manager::ConnectionManager;
use crate::acp::types::{ConnectionStatus, PromptInputBlock};
use crate::app_error::{AppCommandError, AppErrorCode};
use crate::commands::acp::{build_session_runtime_env, verify_agent_installed};
use crate::commands::conversations::{emit_conversation_upsert, get_folder_conversation_core};
use crate::db::service::{conversation_service, folder_service, handoff_service};
use crate::db::AppDatabase;
use crate::models::{AgentType, DbConversationSummary, MessageTurn};
use crate::web::event_bridge::EventEmitter;

/// How long the target gets to report its session (a `session/load` replay of
/// a long transcript, or a cold `session/new`). Generous on purpose: a slow
/// load is not a failure, an unrelated session id is.
const TARGET_SESSION_TIMEOUT: Duration = Duration::from_secs(120);
const TARGET_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// What the dialog shows before the user confirms.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffPlan {
    pub source_agent_type: AgentType,
    pub target_agent_type: AgentType,
    pub path: HandoffPath,
    /// Set when the same-family path was possible in principle but had to be
    /// demoted to a summary (`"transcript_missing"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_reason: Option<&'static str>,
    /// Stable code the frontend localizes when the handoff cannot run:
    /// `same_agent`, `no_session`, `not_installed`, `disabled`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_message: Option<String>,
    pub turn_count: usize,
    /// Summary path only: what the briefing would carry.
    pub briefing_chars: usize,
    pub briefing_truncated: bool,
    pub verbatim_turns: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffResult {
    pub conversation_id: i32,
    pub folder_id: i32,
    pub from_agent_type: AgentType,
    pub to_agent_type: AgentType,
    /// The session the conversation is bound to now.
    pub external_id: String,
    pub path: HandoffPath,
    pub connection_id: String,
    pub briefing_truncated: bool,
}

#[derive(Debug, Clone)]
pub struct HandoffRequest {
    pub conversation_id: i32,
    pub target_agent_type: AgentType,
    pub note: Option<String>,
    /// The target agent's own saved selector preferences (mode / model), read
    /// by the frontend from the same store a normal connect uses, so the
    /// handoff never carries the source agent's model over.
    pub preferred_mode_id: Option<String>,
    pub preferred_config_values: BTreeMap<String, String>,
}

struct Prepared {
    summary: DbConversationSummary,
    working_dir: String,
    plan: HandoffPlan,
    target_env: BTreeMap<String, String>,
    turns: Vec<MessageTurn>,
}

async fn prepare(
    db: &AppDatabase,
    data_dir: &Path,
    conversation_id: i32,
    target: AgentType,
) -> Result<Prepared, AppCommandError> {
    let summary = conversation_service::get_by_id(&db.conn, conversation_id)
        .await
        .map_err(AppCommandError::from)?;
    let source = summary.agent_type;
    let folder = folder_service::get_folder_by_id(&db.conn, summary.folder_id)
        .await
        .map_err(AppCommandError::from)?;
    let working_dir = summary
        .origin_cwd
        .clone()
        .or_else(|| folder.map(|f| f.path))
        .ok_or_else(|| AppCommandError::not_found("conversation folder not found"))?;

    let mut plan = HandoffPlan {
        source_agent_type: source,
        target_agent_type: target,
        path: HandoffPath::Summary,
        native_reason: None,
        blocked: None,
        blocked_message: None,
        turn_count: 0,
        briefing_chars: 0,
        briefing_truncated: false,
        verbatim_turns: 0,
    };

    let mut target_env = BTreeMap::new();
    if target == source {
        plan.blocked = Some("same_agent");
    } else if summary.external_id.as_deref().unwrap_or("").is_empty() {
        plan.blocked = Some("no_session");
    } else {
        match build_session_runtime_env(db, target, None, data_dir).await {
            Ok(env) => target_env = env,
            Err(e) => {
                plan.blocked = Some("disabled");
                plan.blocked_message = Some(e.to_string());
            }
        }
        if plan.blocked.is_none() {
            if let Err(e) = verify_agent_installed(target).await {
                plan.blocked = Some("not_installed");
                plan.blocked_message = Some(e.to_string());
            }
        }
    }

    let (detail, _) = get_folder_conversation_core(&db.conn, conversation_id).await?;
    // Earlier handoffs already sit in this timeline as dividers; they are
    // bookkeeping, not something the next agent needs to read.
    let turns: Vec<MessageTurn> = detail
        .turns
        .into_iter()
        .filter(|t| !handoff::is_divider_turn(t))
        .collect();
    plan.turn_count = turns.len();

    if plan.blocked.is_none() {
        plan.path = handoff::plan_path(source, target);
        if plan.path == HandoffPath::Native {
            // The move is only lossless when the source's own store still has
            // the transcript. A row whose session file is gone (pruned, or an
            // external id that never matched a file) gets the summary instead
            // of a native transfer that would come up empty.
            let source_env = build_session_runtime_env(db, source, None, data_dir)
                .await
                .unwrap_or_default();
            let source_home = handoff::claude_config_dir_for(source, &source_env);
            let session_id = summary.external_id.as_deref().unwrap_or("");
            let present = crate::parsers::claude::find_session_file_in(
                &source_home.join("projects"),
                session_id,
            )
            .is_some();
            if !present {
                plan.path = HandoffPath::Summary;
                plan.native_reason = Some("transcript_missing");
            }
        }
        if plan.path == HandoffPath::Summary {
            let briefing = build_briefing(&BriefingInput {
                source_label: &source.to_string(),
                target_label: &target.to_string(),
                working_dir: Some(&working_dir),
                title: summary.title.as_deref(),
                turns: &turns,
                note: None,
                budget: DEFAULT_BRIEFING_BUDGET,
            });
            plan.briefing_chars = briefing.text.chars().count();
            plan.briefing_truncated = briefing.truncated;
            plan.verbatim_turns = briefing.verbatim_turns;
        }
    }

    Ok(Prepared {
        summary,
        working_dir,
        plan,
        target_env,
        turns,
    })
}

pub async fn handoff_plan_core(
    db: &AppDatabase,
    data_dir: &Path,
    conversation_id: i32,
    target: AgentType,
) -> Result<HandoffPlan, AppCommandError> {
    Ok(prepare(db, data_dir, conversation_id, target).await?.plan)
}

/// The live connection currently serving this conversation, if any.
async fn source_connection(
    manager: &ConnectionManager,
    summary: &DbConversationSummary,
) -> Option<String> {
    if let Some(id) = manager
        .find_connection_by_conversation_id(summary.id)
        .await
    {
        return Some(id);
    }
    let sid = summary.external_id.as_deref()?;
    manager
        .find_connection_by_external_id(sid, summary.agent_type)
        .await
}

/// Wait for the freshly spawned target to name its session. Returns the id
/// it reported and the status it settled on; `None` means it never did.
async fn wait_for_target_session(
    manager: &ConnectionManager,
    connection_id: &str,
) -> (Option<String>, ConnectionStatus) {
    let deadline = tokio::time::Instant::now() + TARGET_SESSION_TIMEOUT;
    loop {
        let Some(state) = manager.get_state(connection_id).await else {
            return (None, ConnectionStatus::Disconnected);
        };
        let (external_id, status) = {
            let s = state.read().await;
            (s.external_id.clone(), s.status.clone())
        };
        if external_id.is_some()
            || matches!(status, ConnectionStatus::Error | ConnectionStatus::Disconnected)
        {
            return (external_id, status);
        }
        if tokio::time::Instant::now() >= deadline {
            return (external_id, status);
        }
        tokio::time::sleep(TARGET_POLL_INTERVAL).await;
    }
}

/// Where codeg keeps its own transcript for a custom agent's session. A stale
/// copy from an earlier visit of the same session would stop the load replay
/// from re-hydrating it (`has_recorded_history` gate), so it is set aside
/// before the target loads and put back if the load fails.
fn custom_transcript_path(agent_type: AgentType, session_id: &str) -> Option<std::path::PathBuf> {
    agent_type.custom_id()?;
    let dir = crate::acp::registry::registry_id_for(agent_type);
    crate::acp_transcript::transcript_path_in(
        &crate::paths::codeg_acp_transcripts_root(),
        dir,
        session_id,
    )
    .filter(|p| p.exists())
}

fn set_aside(path: &Path) -> Option<std::path::PathBuf> {
    let aside = path.with_extension(format!(
        "jsonl.superseded-{}",
        crate::acp_transcript::now_epoch_ms()
    ));
    std::fs::rename(path, &aside).ok().map(|_| aside)
}

#[allow(clippy::too_many_arguments)]
pub async fn handoff_core(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    data_dir: &Path,
    owner_window_label: String,
    request: HandoffRequest,
) -> Result<HandoffResult, AppCommandError> {
    let target = request.target_agent_type;
    let prepared = prepare(db, data_dir, request.conversation_id, target).await?;
    if let Some(code) = prepared.plan.blocked {
        let message = prepared
            .plan
            .blocked_message
            .clone()
            .unwrap_or_else(|| format!("handoff blocked: {code}"));
        return Err(AppCommandError::invalid_input(message).with_detail(code));
    }
    let summary = prepared.summary;
    let source = summary.agent_type;
    let conversation_id = summary.id;
    let session_id = summary
        .external_id
        .clone()
        .ok_or_else(|| AppCommandError::invalid_input("conversation has no session yet"))?;

    // A turn in flight would keep writing into the source session after the
    // row moved. Refuse rather than race it; the frontend re-queues on this
    // code the way it does for a fork.
    let source_conn = source_connection(manager, &summary).await;
    if let Some(conn_id) = &source_conn {
        if let Some(state) = manager.get_state(conn_id).await {
            if state.read().await.turn_in_flight {
                // Wording carries the `turn already in progress` marker the
                // frontend's `isTurnInProgressRejection` matches on the Tauri
                // transport, where only the Display string reaches it.
                return Err(AppCommandError::new(
                    AppErrorCode::TurnInProgress,
                    "turn already in progress: stop it or wait for it to finish before handing off",
                ));
            }
        }
    }

    let user_turns_before = handoff::count_user_turns(&prepared.turns);
    let note = request
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);

    match prepared.plan.path {
        HandoffPath::Native => {
            let source_env = build_session_runtime_env(db, source, None, data_dir)
                .await
                .unwrap_or_default();
            let source_home = handoff::claude_config_dir_for(source, &source_env);
            let target_home = handoff::claude_config_dir_for(target, &prepared.target_env);
            if source_home == target_home {
                return Err(AppCommandError::invalid_input(
                    "both agents use the same Claude config directory; there is nothing to move",
                ));
            }

            // The source connection is done with this session; disconnect it
            // so nothing appends to the file while it is copied.
            if let Some(conn_id) = &source_conn {
                let _ = manager.disconnect(conn_id).await;
            }

            let copied = handoff::copy_claude_session(&source_home, &target_home, &session_id)
                .map_err(|e| AppCommandError::io_error(e.to_string()))?;
            let set_aside_transcript = custom_transcript_path(target, &session_id)
                .and_then(|p| set_aside(&p).map(|aside| (p, aside)));

            let rollback = |copied: &handoff::CopiedSession| {
                handoff::remove_copied_session(copied);
                if let Some((original, aside)) = &set_aside_transcript {
                    let _ = std::fs::rename(aside, original);
                }
            };

            let connection_id = match manager
                .spawn_agent(
                    target,
                    Some(prepared.working_dir.clone()),
                    Some(session_id.clone()),
                    prepared.target_env.clone(),
                    owner_window_label,
                    emitter.clone(),
                    request.preferred_mode_id.clone(),
                    request.preferred_config_values.clone(),
                )
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    rollback(&copied);
                    return Err(AppCommandError::task_execution_failed(e.to_string()));
                }
            };

            // The target must have loaded THIS session. A custom Claude slot
            // that cannot load a session quietly opens a fresh one instead
            // (`recovers_load_failure_locally`), and a built-in that cannot
            // ends in `Error`; both leave the copy where it is and the row
            // untouched.
            let (reported, status) = wait_for_target_session(manager, &connection_id).await;
            let loaded = reported.as_deref() == Some(session_id.as_str())
                && !matches!(status, ConnectionStatus::Error | ConnectionStatus::Disconnected);
            if !loaded {
                let _ = manager.disconnect(&connection_id).await;
                if let Some(stray) = reported.filter(|r| r != &session_id) {
                    // The fallback session's header-only transcript would list
                    // as an empty conversation of the target; drop it.
                    if let Some(path) = custom_transcript_path(target, &stray) {
                        let dir = crate::acp::registry::registry_id_for(target);
                        if !crate::acp_transcript::has_entries_in(
                            &crate::paths::codeg_acp_transcripts_root(),
                            dir,
                            &stray,
                        ) {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                }
                rollback(&copied);
                return Err(AppCommandError::task_execution_failed(format!(
                    "{target} could not load the copied transcript (session {session_id}); \
                     the conversation was left on {source}"
                )));
            }

            if let Err(e) =
                conversation_service::rebind_for_handoff(&db.conn, conversation_id, target, &session_id)
                    .await
            {
                let _ = manager.disconnect(&connection_id).await;
                rollback(&copied);
                return Err(AppCommandError::from(e));
            }
            handoff_service::record(
                &db.conn,
                handoff_service::NewHandoff {
                    conversation_id,
                    from_agent_type: source,
                    from_external_id: Some(session_id.clone()),
                    to_agent_type: target,
                    to_external_id: session_id.clone(),
                    path: HandoffPath::Native.as_str(),
                    carried: true,
                    user_turns_before: u32::try_from(user_turns_before).unwrap_or(u32::MAX),
                    note,
                    briefing: None,
                    truncated: false,
                },
            )
            .await
            .map_err(AppCommandError::from)?;
            emit_conversation_upsert(emitter, &db.conn, conversation_id).await;
            tracing::info!(
                conversation_id,
                from = %source,
                to = %target,
                session_id = %session_id,
                "[handoff] native transfer complete"
            );
            Ok(HandoffResult {
                conversation_id,
                folder_id: summary.folder_id,
                from_agent_type: source,
                to_agent_type: target,
                external_id: session_id,
                path: HandoffPath::Native,
                connection_id,
                briefing_truncated: false,
            })
        }
        HandoffPath::Summary => {
            let briefing = build_briefing(&BriefingInput {
                source_label: &source.to_string(),
                target_label: &target.to_string(),
                working_dir: Some(&prepared.working_dir),
                title: summary.title.as_deref(),
                turns: &prepared.turns,
                note: note.as_deref(),
                budget: DEFAULT_BRIEFING_BUDGET,
            });

            if let Some(conn_id) = &source_conn {
                let _ = manager.disconnect(conn_id).await;
            }

            let connection_id = manager
                .spawn_agent(
                    target,
                    Some(prepared.working_dir.clone()),
                    None,
                    prepared.target_env.clone(),
                    owner_window_label,
                    emitter.clone(),
                    request.preferred_mode_id.clone(),
                    request.preferred_config_values.clone(),
                )
                .await
                .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;

            let (reported, status) = wait_for_target_session(manager, &connection_id).await;
            let new_session_id = match reported {
                Some(id)
                    if !matches!(status, ConnectionStatus::Error | ConnectionStatus::Disconnected) =>
                {
                    id
                }
                _ => {
                    let _ = manager.disconnect(&connection_id).await;
                    return Err(AppCommandError::task_execution_failed(format!(
                        "{target} did not open a session; the conversation was left on {source}"
                    )));
                }
            };

            if let Err(e) = conversation_service::rebind_for_handoff(
                &db.conn,
                conversation_id,
                target,
                &new_session_id,
            )
            .await
            {
                let _ = manager.disconnect(&connection_id).await;
                return Err(AppCommandError::from(e));
            }
            handoff_service::record(
                &db.conn,
                handoff_service::NewHandoff {
                    conversation_id,
                    from_agent_type: source,
                    from_external_id: Some(session_id.clone()),
                    to_agent_type: target,
                    to_external_id: new_session_id.clone(),
                    path: HandoffPath::Summary.as_str(),
                    carried: false,
                    user_turns_before: u32::try_from(user_turns_before).unwrap_or(u32::MAX),
                    note,
                    briefing: Some(briefing.text.clone()),
                    truncated: briefing.truncated,
                },
            )
            .await
            .map_err(AppCommandError::from)?;
            emit_conversation_upsert(emitter, &db.conn, conversation_id).await;

            // The row already holds the new session, so this bind is the
            // ordinary "row already holds the id" case in `bind_external_id`:
            // nothing splits. The briefing is the target's first prompt.
            manager
                .send_prompt_linked(
                    db,
                    &connection_id,
                    vec![PromptInputBlock::Text {
                        text: briefing.text,
                    }],
                    Some(summary.folder_id),
                    Some(conversation_id),
                    None,
                )
                .await
                .map_err(|e| {
                    AppCommandError::task_execution_failed(format!(
                        "the conversation now belongs to {target}, but the briefing could not be \
                         sent: {e}. Send a message to continue."
                    ))
                })?;
            tracing::info!(
                conversation_id,
                from = %source,
                to = %target,
                session_id = %new_session_id,
                truncated = briefing.truncated,
                "[handoff] summary handoff complete"
            );
            Ok(HandoffResult {
                conversation_id,
                folder_id: summary.folder_id,
                from_agent_type: source,
                to_agent_type: target,
                external_id: new_session_id,
                path: HandoffPath::Summary,
                connection_id,
                briefing_truncated: briefing.truncated,
            })
        }
    }
}

#[cfg(feature = "tauri-runtime")]
fn effective_data_dir(app_handle: &tauri::AppHandle) -> std::path::PathBuf {
    app_handle
        .path()
        .app_data_dir()
        .map(|p| crate::paths::resolve_effective_data_dir(&p))
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn acp_handoff_plan(
    conversation_id: i32,
    target_agent_type: AgentType,
    db: State<'_, AppDatabase>,
    app_handle: tauri::AppHandle,
) -> Result<HandoffPlan, AppCommandError> {
    let data_dir = effective_data_dir(&app_handle);
    handoff_plan_core(&db, &data_dir, conversation_id, target_agent_type).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
#[allow(clippy::too_many_arguments)]
pub async fn acp_handoff(
    conversation_id: i32,
    target_agent_type: AgentType,
    note: Option<String>,
    preferred_mode_id: Option<String>,
    preferred_config_values: Option<BTreeMap<String, String>>,
    db: State<'_, AppDatabase>,
    manager: State<'_, ConnectionManager>,
    app_handle: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<HandoffResult, AppCommandError> {
    let data_dir = effective_data_dir(&app_handle);
    let emitter = EventEmitter::Tauri(app_handle.clone());
    handoff_core(
        &db,
        &manager,
        &emitter,
        &data_dir,
        window.label().to_string(),
        HandoffRequest {
            conversation_id,
            target_agent_type,
            note,
            preferred_mode_id,
            preferred_config_values: preferred_config_values.unwrap_or_default(),
        },
    )
    .await
}
