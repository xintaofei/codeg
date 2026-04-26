use std::collections::BTreeMap;

use crate::acp::error::AcpError;
use crate::acp::manager::ConnectionManager;
use crate::commands::acp as acp_commands;
use crate::db::service::{agent_setting_service, squad_service};
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::squad::{
    SquadArtifactInfo, SquadArtifactType, SquadRoleKind, SquadRoleProfileInfo, SquadRoleRunInfo,
    SquadRoleRunStatus, SquadRunMode, SquadRunStatus, SquadTaskInfo, SquadTaskStatus,
};
use crate::squad::conductor_parser::{parse_conductor_output, ParseReport};
use crate::squad::events::{emit_payload, emit_squad_event, RoleConnectionAttachedPayload};
use crate::squad::prompt_builder;
use crate::squad::worktree_manager;
use crate::web::event_bridge::EventEmitter;

fn parse_profile(role_run: &SquadRoleRunInfo) -> Result<SquadRoleProfileInfo, AcpError> {
    squad_service::role_profile_from_snapshot(&role_run.role_profile_snapshot_json)
        .map_err(|e| AcpError::protocol(e.to_string()))
}

fn parse_profile_env(raw: Option<&str>) -> Result<BTreeMap<String, String>, AcpError> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    serde_json::from_str::<BTreeMap<String, String>>(raw)
        .map_err(|e| AcpError::protocol(format!("invalid squad role env_json: {e}")))
}

pub async fn connect_role(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    run_id: i32,
    role_kind: SquadRoleKind,
    working_dir: Option<String>,
) -> Result<SquadRoleRunInfo, AcpError> {
    let snapshot = squad_service::get_run(&db.conn, run_id)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    let role_run = snapshot
        .roles
        .into_iter()
        .find(|role| role.role_kind == role_kind)
        .ok_or_else(|| AcpError::protocol("squad role run not found"))?;
    let profile = parse_profile(&role_run)?;

    let role_workspace = worktree_manager::ensure_role_workspace(
        working_dir.as_deref(),
        run_id,
        role_kind,
        profile.workspace_policy,
    )
    .await
    .map_err(|e| AcpError::protocol(format!("failed to materialize role workspace: {e}")))?;
    let role_run =
        squad_service::update_role_workspace(&db.conn, role_run.id, role_workspace.clone(), None)
            .await
            .map_err(|e| AcpError::protocol(e.to_string()))?;

    let connecting = squad_service::update_role_connection(
        &db.conn,
        role_run.id,
        None,
        None,
        SquadRoleRunStatus::Connecting,
        None,
    )
    .await
    .map_err(|e| AcpError::protocol(e.to_string()))?;
    emit_payload(
        emitter,
        "squad_role_status_changed",
        run_id,
        Some(role_kind),
        &connecting,
    );

    let setting = agent_setting_service::get_by_agent_type(&db.conn, profile.agent_type)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    if setting.as_ref().map(|row| !row.enabled).unwrap_or(false) {
        let err = AcpError::protocol(format!("{} is disabled in settings", profile.agent_type));
        let failed = squad_service::update_role_connection(
            &db.conn,
            role_run.id,
            None,
            None,
            SquadRoleRunStatus::Failed,
            Some(err.to_string()),
        )
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
        emit_payload(
            emitter,
            "squad_role_status_changed",
            run_id,
            Some(role_kind),
            &failed,
        );
        return Err(err);
    }

    if let Err(err) = acp_commands::verify_agent_installed(profile.agent_type) {
        let failed = squad_service::update_role_connection(
            &db.conn,
            role_run.id,
            None,
            None,
            SquadRoleRunStatus::Failed,
            Some(err.to_string()),
        )
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
        emit_payload(
            emitter,
            "squad_role_status_changed",
            run_id,
            Some(role_kind),
            &failed,
        );
        return Err(err);
    }

    let local_config_json = acp_commands::load_agent_local_config_json(profile.agent_type);
    let mut runtime_env = acp_commands::build_runtime_env_from_setting(
        profile.agent_type,
        setting.as_ref(),
        local_config_json.as_deref(),
    );
    acp_commands::apply_model_provider_env(
        profile.agent_type,
        setting.as_ref(),
        profile.model_provider_id,
        &mut runtime_env,
        &db.conn,
    )
    .await;
    let profile_env = match parse_profile_env(profile.env_json.as_deref()) {
        Ok(env) => env,
        Err(err) => {
            let failed = squad_service::update_role_connection(
                &db.conn,
                role_run.id,
                None,
                None,
                SquadRoleRunStatus::Failed,
                Some(err.to_string()),
            )
            .await
            .map_err(|e| AcpError::protocol(e.to_string()))?;
            emit_payload(
                emitter,
                "squad_role_status_changed",
                run_id,
                Some(role_kind),
                &failed,
            );
            return Err(err);
        }
    };
    for (key, value) in profile_env {
        runtime_env.insert(key, value);
    }
    runtime_env.insert("CODEG_SQUAD_RUN_ID".into(), run_id.to_string());
    runtime_env.insert(
        "CODEG_SQUAD_ROLE_KIND".into(),
        serde_json::to_string(&role_kind)
            .unwrap_or_default()
            .trim_matches('"')
            .to_string(),
    );
    if let Some(path) = &role_workspace {
        runtime_env.insert("CODEG_SQUAD_WORKSPACE_PATH".into(), path.clone());
    }

    let connection_id = match manager
        .spawn_agent(
            profile.agent_type,
            role_workspace.clone(),
            None,
            runtime_env,
            format!("squad:{run_id}"),
            emitter.clone(),
        )
        .await
    {
        Ok(connection_id) => connection_id,
        Err(err) => {
            let failed = squad_service::update_role_connection(
                &db.conn,
                role_run.id,
                None,
                None,
                SquadRoleRunStatus::Failed,
                Some(err.to_string()),
            )
            .await
            .map_err(|e| AcpError::protocol(e.to_string()))?;
            emit_payload(
                emitter,
                "squad_role_status_changed",
                run_id,
                Some(role_kind),
                &failed,
            );
            return Err(err);
        }
    };

    let connected = squad_service::update_role_connection(
        &db.conn,
        role_run.id,
        Some(connection_id.clone()),
        None,
        SquadRoleRunStatus::Connected,
        None,
    )
    .await
    .map_err(|e| AcpError::protocol(e.to_string()))?;
    emit_payload(
        emitter,
        "squad_role_status_changed",
        run_id,
        Some(role_kind),
        &connected,
    );
    emit_payload(
        emitter,
        "squad_role_connection_attached",
        run_id,
        Some(role_kind),
        &RoleConnectionAttachedPayload {
            connection_id,
            agent_type: profile.agent_type,
            working_dir: role_workspace,
            session_id: None,
        },
    );
    Ok(connected)
}

pub async fn prompt_role(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    run_id: i32,
    role_kind: SquadRoleKind,
    task: Option<SquadTaskInfo>,
) -> Result<SquadRoleRunInfo, AcpError> {
    let snapshot = squad_service::get_run(&db.conn, run_id)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    let role_run = snapshot
        .roles
        .into_iter()
        .find(|role| role.role_kind == role_kind)
        .ok_or_else(|| AcpError::protocol("squad role run not found"))?;
    let connection_id = role_run
        .connection_id
        .clone()
        .ok_or_else(|| AcpError::protocol("squad role is not connected"))?;
    let profile = parse_profile(&role_run)?;
    let blocks = prompt_builder::build_role_prompt(
        &profile,
        &snapshot.run.goal_summary,
        task.as_ref(),
        role_run.workspace_path.as_deref(),
    );
    manager.send_prompt(&connection_id, blocks).await?;
    let prompted = squad_service::update_role_connection(
        &db.conn,
        role_run.id,
        Some(connection_id),
        role_run.session_id,
        SquadRoleRunStatus::Prompting,
        None,
    )
    .await
    .map_err(|e| AcpError::protocol(e.to_string()))?;
    emit_payload(
        emitter,
        "squad_role_status_changed",
        run_id,
        Some(role_kind),
        &prompted,
    );
    Ok(prompted)
}

pub async fn start_run(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    run_id: i32,
    working_dir: Option<String>,
) -> Result<(), AcpError> {
    let snapshot = squad_service::get_run(&db.conn, run_id)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    let mode = snapshot.run.mode;
    let run = squad_service::set_run_status(&db.conn, run_id, SquadRunStatus::Running, None)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    emit_payload(emitter, "squad_run_status_changed", run_id, None, &run);

    let mut failures = Vec::new();
    let mut connected_enabled: Vec<SquadRoleKind> = Vec::new();
    for role in snapshot.roles {
        let profile = parse_profile(&role)?;
        if !profile.enabled {
            continue;
        }
        match connect_role(
            db,
            manager,
            emitter,
            run_id,
            role.role_kind,
            working_dir.clone(),
        )
        .await
        {
            Ok(_) => connected_enabled.push(role.role_kind),
            Err(err) => failures.push(format!(
                "{} connect failed: {err}",
                profile.role_kind.as_str()
            )),
        }
    }

    // Mode-specific dispatch policy. connect_role above is shared; only the
    // *who gets prompted automatically* part differs.
    match mode {
        SquadRunMode::Manual => {
            // No auto-prompt. The user drives prompts via squad_prompt_role.
        }
        SquadRunMode::ConductorDispatch => {
            // Conductor plans the task list; workers wait for dispatch_pending_tasks.
            if connected_enabled.contains(&SquadRoleKind::Conductor) {
                if let Err(err) =
                    prompt_role(db, manager, emitter, run_id, SquadRoleKind::Conductor, None).await
                {
                    failures.push(format!("conductor prompt failed: {err}"));
                }
            } else {
                failures.push(
                    "conductor_dispatch mode requires a connected Conductor role".to_string(),
                );
            }
        }
        SquadRunMode::AllHandsReview => {
            // Every connected role gets prompted once for an independent review pass.
            for role_kind in &connected_enabled {
                if let Err(err) = prompt_role(db, manager, emitter, run_id, *role_kind, None).await
                {
                    failures.push(format!("{} prompt failed: {err}", role_kind.as_str()));
                }
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        let run = squad_service::set_run_status(
            &db.conn,
            run_id,
            SquadRunStatus::Blocked,
            Some(failures.join("\n")),
        )
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
        emit_payload(emitter, "squad_run_status_changed", run_id, None, &run);
        Ok(())
    }
}

pub async fn stop_run(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    run_id: i32,
) -> Result<(), AcpError> {
    let roles = squad_service::list_role_runs(&db.conn, run_id)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    for role in roles {
        if let Some(connection_id) = role.connection_id.clone() {
            let _ = manager.disconnect(&connection_id).await;
        }
        let stopped = squad_service::update_role_connection(
            &db.conn,
            role.id,
            None,
            role.session_id,
            SquadRoleRunStatus::Stopped,
            None,
        )
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
        emit_payload(
            emitter,
            "squad_role_status_changed",
            run_id,
            Some(role.role_kind),
            &stopped,
        );
    }
    let run = squad_service::set_run_status(&db.conn, run_id, SquadRunStatus::Cancelled, None)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    emit_payload(emitter, "squad_run_status_changed", run_id, None, &run);
    Ok(())
}

pub fn _agent_type_for_profile(profile: &SquadRoleProfileInfo) -> AgentType {
    profile.agent_type
}

/// Persist an artifact for a squad role and emit `squad_artifact_created`
/// so any subscriber (UI, log tail, future analytics) reacts in one place.
/// Re-used by every artifact-writing path (frontend command, conductor
/// pipeline, future ACP listener) so events never get skipped.
#[allow(clippy::too_many_arguments)]
pub async fn record_role_artifact(
    db: &AppDatabase,
    emitter: &EventEmitter,
    run_id: i32,
    role_kind: Option<SquadRoleKind>,
    task_id: Option<i32>,
    artifact_type: SquadArtifactType,
    title: String,
    content_json: String,
) -> Result<SquadArtifactInfo, AcpError> {
    let artifact = squad_service::create_artifact(
        &db.conn,
        run_id,
        role_kind,
        task_id,
        artifact_type,
        title,
        content_json,
    )
    .await
    .map_err(|e| AcpError::protocol(e.to_string()))?;
    emit_payload(
        emitter,
        "squad_artifact_created",
        run_id,
        role_kind,
        &artifact,
    );
    Ok(artifact)
}

/// What a single ACP turn produced as squad artifacts.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnArtifactsResult {
    pub summary: Option<SquadArtifactInfo>,
    pub plan: Option<SquadArtifactInfo>,
}

/// Capture an ACP turn's output as squad artifacts. Frontends that already
/// receive `acp://event` chunks can call this on `TurnComplete` with the
/// accumulated agent text + the latest plan JSON. Both inputs are optional:
///
/// - `agent_text` — when non-empty (after trim), persisted as an artifact
///   of type `Summary`. Stored under a JSON envelope `{"text": "..."}` so
///   future fields (token usage, cwd, etc.) can be added without a schema
///   bump.
/// - `plan_json` — when present, persisted as-is as an artifact of type
///   `Plan`. The service layer validates JSON shape, so malformed input
///   surfaces a clear error.
pub async fn record_turn_artifacts(
    db: &AppDatabase,
    emitter: &EventEmitter,
    run_id: i32,
    role_kind: SquadRoleKind,
    task_id: Option<i32>,
    agent_text: String,
    plan_json: Option<String>,
) -> Result<TurnArtifactsResult, AcpError> {
    // Validate the run exists once up front.
    let _ = squad_service::get_run(&db.conn, run_id)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    let summary = if !agent_text.trim().is_empty() {
        let envelope = serde_json::json!({ "text": agent_text }).to_string();
        let title = synthesize_summary_title(&agent_text);
        Some(
            record_role_artifact(
                db,
                emitter,
                run_id,
                Some(role_kind),
                task_id,
                SquadArtifactType::Summary,
                title,
                envelope,
            )
            .await?,
        )
    } else {
        None
    };

    let plan = match plan_json
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => Some(
            record_role_artifact(
                db,
                emitter,
                run_id,
                Some(role_kind),
                task_id,
                SquadArtifactType::Plan,
                "Plan update".to_string(),
                raw.to_string(),
            )
            .await?,
        ),
        None => None,
    };

    Ok(TurnArtifactsResult { summary, plan })
}

/// Build a short title from the first non-empty line of a turn's text,
/// trimmed to 80 chars so artifact lists stay readable.
fn synthesize_summary_title(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("(empty turn)");
    if first.chars().count() > 80 {
        let mut out: String = first.chars().take(77).collect();
        out.push('…');
        out
    } else {
        first.to_string()
    }
}

/// Outcome of feeding a Conductor reply through the parser + task-writer pipeline.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyConductorOutputResult {
    pub created_tasks: Vec<SquadTaskInfo>,
    pub skipped_reasons: Vec<String>,
}

/// Parse a Conductor's free-form reply into a task list and persist each
/// recovered item as a `squad_task` row. Emits `squad_task_created` per task
/// and a single trailing `squad_conductor_plan_applied` summary event so the
/// UI can refresh once instead of N times.
///
/// This is intentionally callable from any layer (Tauri command, Web handler,
/// or — eventually — an ACP TurnComplete listener) so the pipeline doesn't
/// have to be re-implemented when the live stream gets wired up.
pub async fn apply_conductor_output(
    db: &AppDatabase,
    emitter: &EventEmitter,
    run_id: i32,
    raw_text: &str,
) -> Result<ApplyConductorOutputResult, AcpError> {
    // Validate the run exists so we don't write orphan tasks.
    let _ = squad_service::get_run(&db.conn, run_id)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    let ParseReport { tasks, skipped } = parse_conductor_output(raw_text);

    let mut created = Vec::with_capacity(tasks.len());
    for parsed in tasks {
        let task = squad_service::create_task(
            &db.conn,
            run_id,
            parsed.role,
            parsed.title,
            parsed.description,
        )
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
        emit_payload(
            emitter,
            "squad_task_created",
            run_id,
            Some(parsed.role),
            &task,
        );
        created.push(task);
    }

    let summary = serde_json::json!({
        "createdCount": created.len(),
        "skippedCount": skipped.len(),
        "skippedReasons": &skipped,
    });
    emit_squad_event(
        emitter,
        "squad_conductor_plan_applied",
        run_id,
        None,
        Some(summary),
    );

    Ok(ApplyConductorOutputResult {
        created_tasks: created,
        skipped_reasons: skipped,
    })
}

/// Per-task dispatch outcome — useful for tests, logs, and the UI summary.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchedTask {
    pub task_id: i32,
    pub role_kind: SquadRoleKind,
    pub outcome: DispatchOutcome,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchOutcome {
    /// Task was prompted to its role; status moved Pending → Assigned.
    Dispatched,
    /// Skipped this round because at least one dependency hasn't completed yet.
    BlockedOnDeps,
    /// The assigned role isn't connected (or not enabled) — left Pending.
    RoleNotConnected,
    /// Already in flight (Assigned/Running); not re-dispatched.
    AlreadyInFlight,
    /// Failed to prompt the role; task moved Pending → Failed with note.
    PromptFailed,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchPendingTasksResult {
    pub run_id: i32,
    pub considered: usize,
    pub dispatched: Vec<DispatchedTask>,
}

/// Walk every task on a run and prompt each Pending task whose dependencies
/// are satisfied. Idempotent: re-running won't re-dispatch tasks already in
/// flight, and tasks blocked on deps will simply be reported and tried again
/// next call. Intended to be invoked after `apply_conductor_output` and again
/// each time a task transitions to Completed.
pub async fn dispatch_pending_tasks(
    db: &AppDatabase,
    manager: &ConnectionManager,
    emitter: &EventEmitter,
    run_id: i32,
) -> Result<DispatchPendingTasksResult, AcpError> {
    let snapshot = squad_service::get_run(&db.conn, run_id)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    // Build a quick lookup of which roles are currently connected so we don't
    // try to prompt a role that isn't even up.
    let connected_roles: std::collections::HashSet<SquadRoleKind> = snapshot
        .roles
        .iter()
        .filter(|r| {
            matches!(
                r.status,
                SquadRoleRunStatus::Connected | SquadRoleRunStatus::Prompting
            )
        })
        .map(|r| r.role_kind)
        .collect();

    // Index task statuses for dependency checks.
    let task_status: std::collections::HashMap<i32, SquadTaskStatus> =
        snapshot.tasks.iter().map(|t| (t.id, t.status)).collect();

    let mut dispatched = Vec::new();
    let considered = snapshot.tasks.len();

    for task in &snapshot.tasks {
        match task.status {
            SquadTaskStatus::Pending => {}
            SquadTaskStatus::Assigned | SquadTaskStatus::Running => {
                dispatched.push(DispatchedTask {
                    task_id: task.id,
                    role_kind: task.assigned_role_kind,
                    outcome: DispatchOutcome::AlreadyInFlight,
                    note: None,
                });
                continue;
            }
            // Blocked / Completed / Failed / Cancelled: nothing to do this pass.
            _ => continue,
        }

        if !deps_satisfied(task, &task_status) {
            dispatched.push(DispatchedTask {
                task_id: task.id,
                role_kind: task.assigned_role_kind,
                outcome: DispatchOutcome::BlockedOnDeps,
                note: None,
            });
            continue;
        }

        if !connected_roles.contains(&task.assigned_role_kind) {
            dispatched.push(DispatchedTask {
                task_id: task.id,
                role_kind: task.assigned_role_kind,
                outcome: DispatchOutcome::RoleNotConnected,
                note: None,
            });
            continue;
        }

        // Move to Assigned *before* prompting so a parallel dispatch_pending_tasks
        // call can't double-fire.
        let assigned =
            squad_service::update_task_status(&db.conn, task.id, SquadTaskStatus::Assigned)
                .await
                .map_err(|e| AcpError::protocol(e.to_string()))?;
        emit_payload(
            emitter,
            "squad_task_status_changed",
            run_id,
            Some(task.assigned_role_kind),
            &assigned,
        );

        match prompt_role(
            db,
            manager,
            emitter,
            run_id,
            task.assigned_role_kind,
            Some(assigned.clone()),
        )
        .await
        {
            Ok(_) => {
                dispatched.push(DispatchedTask {
                    task_id: task.id,
                    role_kind: task.assigned_role_kind,
                    outcome: DispatchOutcome::Dispatched,
                    note: None,
                });
            }
            Err(err) => {
                let note = err.to_string();
                let failed =
                    squad_service::update_task_status(&db.conn, task.id, SquadTaskStatus::Failed)
                        .await
                        .map_err(|e| AcpError::protocol(e.to_string()))?;
                emit_payload(
                    emitter,
                    "squad_task_status_changed",
                    run_id,
                    Some(task.assigned_role_kind),
                    &failed,
                );
                dispatched.push(DispatchedTask {
                    task_id: task.id,
                    role_kind: task.assigned_role_kind,
                    outcome: DispatchOutcome::PromptFailed,
                    note: Some(note),
                });
            }
        }
    }

    let summary = serde_json::json!({
        "considered": considered,
        "dispatched": &dispatched,
    });
    emit_squad_event(
        emitter,
        "squad_dispatch_round_completed",
        run_id,
        None,
        Some(summary),
    );

    Ok(DispatchPendingTasksResult {
        run_id,
        considered,
        dispatched,
    })
}

/// Returns true when every task id in `task.depends_on_json` (if any) has
/// status Completed. Missing/unparsable dep lists are treated as "no deps"
/// rather than failing — the parser is best-effort.
fn deps_satisfied(
    task: &SquadTaskInfo,
    statuses: &std::collections::HashMap<i32, SquadTaskStatus>,
) -> bool {
    let Some(raw) = task.depends_on_json.as_deref() else {
        return true;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return true;
    }
    let deps: Vec<i32> = match serde_json::from_str(trimmed) {
        Ok(deps) => deps,
        Err(_) => return true,
    };
    deps.iter()
        .all(|dep_id| matches!(statuses.get(dep_id), Some(SquadTaskStatus::Completed)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_with_deps(id: i32, status: SquadTaskStatus, deps: Option<&str>) -> SquadTaskInfo {
        SquadTaskInfo {
            id,
            squad_run_id: 1,
            assigned_role_kind: SquadRoleKind::Worker,
            title: format!("t{id}"),
            description: String::new(),
            input_summary: None,
            status,
            depends_on_json: deps.map(str::to_string),
            priority: 0,
            created_at: String::new(),
            updated_at: String::new(),
            completed_at: None,
            error_message: None,
        }
    }

    #[test]
    fn deps_none_means_satisfied() {
        let t = task_with_deps(1, SquadTaskStatus::Pending, None);
        let map = std::collections::HashMap::new();
        assert!(deps_satisfied(&t, &map));
    }

    #[test]
    fn deps_empty_array_means_satisfied() {
        let t = task_with_deps(1, SquadTaskStatus::Pending, Some("[]"));
        let map = std::collections::HashMap::new();
        assert!(deps_satisfied(&t, &map));
    }

    #[test]
    fn deps_unparsable_treated_as_satisfied() {
        let t = task_with_deps(1, SquadTaskStatus::Pending, Some("not json"));
        let map = std::collections::HashMap::new();
        assert!(deps_satisfied(&t, &map));
    }

    #[test]
    fn deps_block_when_any_incomplete() {
        let t = task_with_deps(3, SquadTaskStatus::Pending, Some("[1, 2]"));
        let mut map = std::collections::HashMap::new();
        map.insert(1, SquadTaskStatus::Completed);
        map.insert(2, SquadTaskStatus::Running);
        assert!(!deps_satisfied(&t, &map));
    }

    #[test]
    fn deps_all_completed_unblocks() {
        let t = task_with_deps(3, SquadTaskStatus::Pending, Some("[1, 2]"));
        let mut map = std::collections::HashMap::new();
        map.insert(1, SquadTaskStatus::Completed);
        map.insert(2, SquadTaskStatus::Completed);
        assert!(deps_satisfied(&t, &map));
    }

    #[test]
    fn deps_missing_id_blocks() {
        // dep id we have no record of — be conservative and block.
        let t = task_with_deps(3, SquadTaskStatus::Pending, Some("[99]"));
        let map = std::collections::HashMap::new();
        assert!(!deps_satisfied(&t, &map));
    }

    #[test]
    fn summary_title_truncates_long_first_line() {
        let long = "a".repeat(200);
        let title = synthesize_summary_title(&long);
        // 77 chars + ellipsis
        assert_eq!(title.chars().count(), 78);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn summary_title_uses_first_non_empty_line() {
        let text = "\n\n  Hello, world!  \nsecond line";
        assert_eq!(synthesize_summary_title(text), "Hello, world!");
    }

    #[test]
    fn summary_title_handles_empty() {
        assert_eq!(synthesize_summary_title(""), "(empty turn)");
        assert_eq!(synthesize_summary_title("   \n  "), "(empty turn)");
    }
}
