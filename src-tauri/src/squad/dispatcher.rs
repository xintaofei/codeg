use std::collections::BTreeMap;

use crate::acp::error::AcpError;
use crate::acp::manager::ConnectionManager;
use crate::commands::acp as acp_commands;
use crate::db::service::{agent_setting_service, squad_service};
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::squad::{
    SquadRoleKind, SquadRoleProfileInfo, SquadRoleRunInfo, SquadRoleRunStatus, SquadRunMode,
    SquadRunStatus, SquadTaskInfo,
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
