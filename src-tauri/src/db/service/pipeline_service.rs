use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, PaginatorTrait, QueryFilter, QueryOrder, Set,
};

use crate::db::entities::{pipeline, pipeline_attempt, pipeline_run};
use crate::db::error::DbError;
use crate::models::{
    AttemptStatus, PipelineAttemptInfo, PipelineDraft, PipelineGraph, PipelineInfo,
    PipelineIsolation, PipelineRunInfo, PipelineRunStatus, PipelineVerdict,
};
use crate::pipeline::{presets, validate};

fn isolation_to_string(value: PipelineIsolation) -> &'static str {
    match value {
        PipelineIsolation::WorktreePerRun => "worktree_per_run",
        PipelineIsolation::SharedInRoot => "shared_in_root",
    }
}

fn isolation_from_string(value: &str) -> PipelineIsolation {
    match value {
        "shared_in_root" => PipelineIsolation::SharedInRoot,
        _ => PipelineIsolation::WorktreePerRun,
    }
}

fn run_status_to_string(value: PipelineRunStatus) -> &'static str {
    match value {
        PipelineRunStatus::Running => "running",
        PipelineRunStatus::Succeeded => "succeeded",
        PipelineRunStatus::Failed => "failed",
        PipelineRunStatus::Cancelled => "cancelled",
        PipelineRunStatus::Interrupted => "interrupted",
        PipelineRunStatus::StoppedMaxIterations => "stopped_max_iterations",
        PipelineRunStatus::Inconclusive => "inconclusive",
    }
}

fn run_status_from_string(value: &str) -> PipelineRunStatus {
    match value {
        "succeeded" => PipelineRunStatus::Succeeded,
        "failed" => PipelineRunStatus::Failed,
        "cancelled" => PipelineRunStatus::Cancelled,
        "interrupted" => PipelineRunStatus::Interrupted,
        "stopped_max_iterations" => PipelineRunStatus::StoppedMaxIterations,
        "inconclusive" => PipelineRunStatus::Inconclusive,
        _ => PipelineRunStatus::Running,
    }
}

fn attempt_status_to_string(value: AttemptStatus) -> &'static str {
    match value {
        AttemptStatus::Running => "running",
        AttemptStatus::Done => "done",
        AttemptStatus::Cancelled => "cancelled",
        AttemptStatus::TimedOut => "timed_out",
        AttemptStatus::Failed => "failed",
    }
}

fn attempt_status_from_string(value: &str) -> AttemptStatus {
    match value {
        "done" => AttemptStatus::Done,
        "cancelled" => AttemptStatus::Cancelled,
        "timed_out" => AttemptStatus::TimedOut,
        "failed" => AttemptStatus::Failed,
        _ => AttemptStatus::Running,
    }
}

fn verdict_from_string(value: Option<&str>) -> Option<PipelineVerdict> {
    match value {
        Some("pass") => Some(PipelineVerdict::Pass),
        Some("changes_requested") => Some(PipelineVerdict::ChangesRequested),
        Some("inconclusive") => Some(PipelineVerdict::Inconclusive),
        _ => None,
    }
}

/// Encodes a graph validation failure as JSON (`{"code": "...", ...}`, per
/// `PipelineValidationError`'s `#[serde(tag = "code")]`) rather than its
/// `Display` string, so `commands::pipeline::pipeline_save_core` can forward
/// it verbatim via `AppCommandError::configuration_invalid` and the frontend
/// can branch on `code` instead of matching English prose.
fn graph_validation_error(error: validate::PipelineValidationError) -> DbError {
    DbError::Validation(serde_json::to_string(&error).unwrap_or_else(|_| error.to_string()))
}

fn graph_from_json(json: &str) -> Result<PipelineGraph, DbError> {
    serde_json::from_str(json)
        .map_err(|e| DbError::Validation(format!("invalid pipeline graph: {e}")))
}

fn graph_to_json(graph: &PipelineGraph) -> Result<String, DbError> {
    serde_json::to_string(graph)
        .map_err(|e| DbError::Validation(format!("invalid pipeline graph: {e}")))
}

fn to_info(model: pipeline::Model) -> Result<PipelineInfo, DbError> {
    Ok(PipelineInfo {
        id: model.id,
        name: model.name,
        preset_key: model.preset_key,
        folder_id: model.folder_id,
        graph: graph_from_json(&model.graph)?,
        isolation: isolation_from_string(&model.isolation),
        created_at: model.created_at,
        updated_at: model.updated_at,
    })
}

fn attempt_to_info(model: pipeline_attempt::Model) -> PipelineAttemptInfo {
    PipelineAttemptInfo {
        id: model.id,
        run_id: model.run_id,
        step_id: model.step_id,
        iteration: model.iteration as u32,
        status: attempt_status_from_string(&model.status),
        conversation_id: model.conversation_id,
        model_requested: model.model_requested,
        model_actual: model.model_actual,
        verdict: verdict_from_string(model.verdict.as_deref()),
        verdict_source: model.verdict_source,
        notes: model.notes,
        summary: model.summary,
        started_at: model.started_at,
        ended_at: model.ended_at,
    }
}

async fn run_to_info(
    conn: &DatabaseConnection,
    model: pipeline_run::Model,
) -> Result<PipelineRunInfo, DbError> {
    let attempts = pipeline_attempt::Entity::find()
        .filter(pipeline_attempt::Column::RunId.eq(model.id))
        .order_by_asc(pipeline_attempt::Column::Id)
        .all(conn)
        .await?
        .into_iter()
        .map(attempt_to_info)
        .collect();
    Ok(PipelineRunInfo {
        id: model.id,
        pipeline_id: model.pipeline_id,
        folder_id: model.folder_id,
        worktree_folder_id: model.worktree_folder_id,
        parent_conversation_id: model.parent_conversation_id,
        graph: graph_from_json(&model.graph)?,
        status: run_status_from_string(&model.status),
        current_step_id: model.current_step_id,
        current_iteration: model.current_iteration as u32,
        error: model.error,
        attempts,
        started_at: model.started_at,
        ended_at: model.ended_at,
    })
}

pub async fn list(
    conn: &DatabaseConnection,
    folder_id: Option<i32>,
) -> Result<Vec<PipelineInfo>, DbError> {
    let mut query = pipeline::Entity::find().filter(pipeline::Column::DeletedAt.is_null());
    if let Some(folder_id) = folder_id {
        query = query.filter(
            pipeline::Column::FolderId
                .eq(folder_id)
                .or(pipeline::Column::FolderId.is_null()),
        );
    }
    query
        .order_by_asc(pipeline::Column::Name)
        .all(conn)
        .await?
        .into_iter()
        .map(to_info)
        .collect::<Result<Vec<_>, _>>()
}

pub async fn get(conn: &DatabaseConnection, id: i32) -> Result<PipelineInfo, DbError> {
    let model = pipeline::Entity::find_by_id(id)
        .filter(pipeline::Column::DeletedAt.is_null())
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("pipeline {id} not found")))?;
    to_info(model)
}

pub async fn save(
    conn: &DatabaseConnection,
    id: Option<i32>,
    draft: PipelineDraft,
) -> Result<PipelineInfo, DbError> {
    if draft.name.trim().is_empty() {
        return Err(DbError::Validation("pipeline name is required".into()));
    }
    validate::validate_graph(&draft.graph).map_err(graph_validation_error)?;
    let graph = graph_to_json(&draft.graph)?;
    let now = Utc::now();
    let model = match id {
        Some(id) => {
            let existing = pipeline::Entity::find_by_id(id)
                .filter(pipeline::Column::DeletedAt.is_null())
                .one(conn)
                .await?
                .ok_or_else(|| DbError::NotFound(format!("pipeline {id} not found")))?;
            let mut active = existing.into_active_model();
            active.name = Set(draft.name.trim().to_string());
            active.folder_id = Set(draft.folder_id);
            active.graph = Set(graph);
            active.isolation = Set(isolation_to_string(draft.isolation).to_string());
            active.updated_at = Set(now);
            active.update(conn).await?
        }
        None => {
            pipeline::ActiveModel {
                id: NotSet,
                name: Set(draft.name.trim().to_string()),
                preset_key: Set(None),
                folder_id: Set(draft.folder_id),
                graph: Set(graph),
                isolation: Set(isolation_to_string(draft.isolation).to_string()),
                created_at: Set(now),
                updated_at: Set(now),
                deleted_at: Set(None),
            }
            .insert(conn)
            .await?
        }
    };
    to_info(model)
}

/// Every preset the user has overridden. Ordered by key so the overlay is
/// deterministic.
pub async fn list_presets(conn: &DatabaseConnection) -> Result<Vec<PipelineInfo>, DbError> {
    pipeline::Entity::find()
        .filter(pipeline::Column::DeletedAt.is_null())
        .filter(pipeline::Column::PresetKey.is_not_null())
        .order_by_asc(pipeline::Column::PresetKey)
        .all(conn)
        .await?
        .into_iter()
        .map(to_info)
        .collect()
}

/// Drop the user's override so the compiled-in chain takes over again.
/// Silently succeeds when there was nothing stored.
pub async fn reset_preset(conn: &DatabaseConnection, key: &str) -> Result<(), DbError> {
    let Some(existing) = pipeline::Entity::find()
        .filter(pipeline::Column::PresetKey.eq(key))
        .filter(pipeline::Column::DeletedAt.is_null())
        .one(conn)
        .await?
    else {
        return Ok(());
    };
    // Hard delete, unlike `delete` above which soft-deletes and refuses preset
    // rows outright. Either would satisfy the partial unique index (it is
    // scoped to `deleted_at IS NULL`), but a reset means "forget my override"
    // — keeping a tombstone of a chain the user threw away buys nothing and
    // would surface in any future query that forgets the deleted_at filter.
    pipeline::Entity::delete_by_id(existing.id).exec(conn).await?;
    Ok(())
}

pub async fn save_preset(
    conn: &DatabaseConnection,
    key: &str,
    name: &str,
    graph: PipelineGraph,
) -> Result<PipelineInfo, DbError> {
    validate::validate_graph(&graph).map_err(graph_validation_error)?;
    let now = Utc::now();
    if let Some(existing) = pipeline::Entity::find()
        .filter(pipeline::Column::PresetKey.eq(key))
        .filter(pipeline::Column::DeletedAt.is_null())
        .one(conn)
        .await?
    {
        let mut active = existing.into_active_model();
        active.name = Set(name.to_string());
        active.graph = Set(graph_to_json(&graph)?);
        active.updated_at = Set(now);
        return to_info(active.update(conn).await?);
    }
    to_info(
        pipeline::ActiveModel {
            id: NotSet,
            name: Set(name.to_string()),
            preset_key: Set(Some(key.to_string())),
            folder_id: Set(None),
            graph: Set(graph_to_json(&graph)?),
            isolation: Set(isolation_to_string(PipelineIsolation::WorktreePerRun).to_string()),
            created_at: Set(now),
            updated_at: Set(now),
            deleted_at: Set(None),
        }
        .insert(conn)
        .await?,
    )
}

pub async fn delete(conn: &DatabaseConnection, id: i32) -> Result<(), DbError> {
    let existing = pipeline::Entity::find_by_id(id)
        .filter(pipeline::Column::DeletedAt.is_null())
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("pipeline {id} not found")))?;
    if existing.preset_key.is_some() {
        return Err(DbError::Conflict(
            "builtin pipelines cannot be deleted".into(),
        ));
    }
    let mut active = existing.into_active_model();
    active.deleted_at = Set(Some(Utc::now()));
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

pub async fn list_runs(
    conn: &DatabaseConnection,
    folder_id: i32,
    limit: u64,
) -> Result<Vec<PipelineRunInfo>, DbError> {
    let models = pipeline_run::Entity::find()
        .filter(pipeline_run::Column::FolderId.eq(folder_id))
        .order_by_desc(pipeline_run::Column::StartedAt)
        .paginate(conn, limit.clamp(1, 100))
        .fetch_page(0)
        .await?;
    let mut result = Vec::with_capacity(models.len());
    for model in models {
        result.push(run_to_info(conn, model).await?);
    }
    Ok(result)
}

pub async fn get_run(conn: &DatabaseConnection, id: i32) -> Result<PipelineRunInfo, DbError> {
    let model = pipeline_run::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("pipeline run {id} not found")))?;
    run_to_info(conn, model).await
}

pub async fn create_run(
    conn: &DatabaseConnection,
    pipeline_id: Option<i32>,
    folder_id: i32,
    graph: &PipelineGraph,
    isolation: PipelineIsolation,
    parent_conversation_id: Option<i32>,
    display_text: Option<String>,
) -> Result<PipelineRunInfo, DbError> {
    validate::validate_graph(graph).map_err(graph_validation_error)?;
    let now = Utc::now();
    let model = pipeline_run::ActiveModel {
        id: NotSet,
        pipeline_id: Set(pipeline_id),
        folder_id: Set(folder_id),
        worktree_folder_id: Set(None),
        parent_conversation_id: Set(parent_conversation_id),
        graph: Set(graph_to_json(graph)?),
        display_text: Set(display_text),
        status: Set(run_status_to_string(PipelineRunStatus::Running).to_string()),
        isolation: Set(isolation_to_string(isolation).to_string()),
        current_step_id: Set(None),
        current_iteration: Set(0),
        error: Set(None),
        started_at: Set(now),
        ended_at: Set(None),
    }
    .insert(conn)
    .await?;
    run_to_info(conn, model).await
}

pub async fn create_attempt(
    conn: &DatabaseConnection,
    run_id: i32,
    step_id: String,
    iteration: u32,
    connection_id: Option<String>,
    model_requested: Option<String>,
) -> Result<PipelineAttemptInfo, DbError> {
    let model = pipeline_attempt::ActiveModel {
        id: NotSet,
        run_id: Set(run_id),
        step_id: Set(step_id),
        iteration: Set(iteration as i32),
        status: Set(attempt_status_to_string(AttemptStatus::Running).to_string()),
        connection_id: Set(connection_id),
        conversation_id: Set(None),
        model_requested: Set(model_requested),
        model_actual: Set(None),
        verdict: Set(None),
        verdict_source: Set(None),
        notes: Set(None),
        summary: Set(None),
        started_at: Set(Utc::now()),
        ended_at: Set(None),
    }
    .insert(conn)
    .await?;
    Ok(attempt_to_info(model))
}

pub async fn cas_attempt_status(
    conn: &DatabaseConnection,
    attempt_id: i32,
    from: AttemptStatus,
    to: AttemptStatus,
) -> Result<bool, DbError> {
    let ended_at = (to != AttemptStatus::Running).then(Utc::now);
    let result = pipeline_attempt::Entity::update_many()
        .col_expr(
            pipeline_attempt::Column::Status,
            sea_orm::sea_query::Expr::value(attempt_status_to_string(to)),
        )
        .col_expr(
            pipeline_attempt::Column::EndedAt,
            sea_orm::sea_query::Expr::value(ended_at),
        )
        .filter(pipeline_attempt::Column::Id.eq(attempt_id))
        .filter(pipeline_attempt::Column::Status.eq(attempt_status_to_string(from)))
        .exec(conn)
        .await?;
    Ok(result.rows_affected == 1)
}

/// CAS transition for a run's status: succeeds only while the run is still
/// `Running` (`rows_affected == 1`), so a late caller racing a cancel/timeout
/// cannot resurrect or overwrite a run that already settled.
pub async fn update_run_status(
    conn: &DatabaseConnection,
    run_id: i32,
    status: PipelineRunStatus,
    error: Option<String>,
) -> Result<bool, DbError> {
    let ended_at = (status != PipelineRunStatus::Running).then(Utc::now);
    let result = pipeline_run::Entity::update_many()
        .col_expr(
            pipeline_run::Column::Status,
            sea_orm::sea_query::Expr::value(run_status_to_string(status)),
        )
        .col_expr(
            pipeline_run::Column::Error,
            sea_orm::sea_query::Expr::value(error),
        )
        .col_expr(
            pipeline_run::Column::EndedAt,
            sea_orm::sea_query::Expr::value(ended_at),
        )
        .filter(pipeline_run::Column::Id.eq(run_id))
        .filter(pipeline_run::Column::Status.eq(run_status_to_string(PipelineRunStatus::Running)))
        .exec(conn)
        .await?;
    Ok(result.rows_affected == 1)
}

pub async fn has_active_run(conn: &DatabaseConnection, folder_id: i32) -> Result<bool, DbError> {
    let count = pipeline_run::Entity::find()
        .filter(pipeline_run::Column::FolderId.eq(folder_id))
        .filter(pipeline_run::Column::Status.eq("running"))
        .count(conn)
        .await?;
    Ok(count > 0)
}

pub async fn get_pipeline(conn: &DatabaseConnection, id: i32) -> Result<PipelineInfo, DbError> {
    get(conn, id).await
}

pub async fn get_run_raw(
    conn: &DatabaseConnection,
    id: i32,
) -> Result<pipeline_run::Model, DbError> {
    pipeline_run::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("pipeline run {id} not found")))
}

pub async fn get_run_info(
    conn: &DatabaseConnection,
    id: i32,
) -> Result<crate::models::PipelineRunInfo, DbError> {
    let model = get_run_raw(conn, id).await?;
    run_to_info(conn, model).await
}

pub async fn interrupt_running_runs(conn: &DatabaseConnection) -> Result<(), DbError> {
    pipeline_run::Entity::update_many()
        .col_expr(
            pipeline_run::Column::Status,
            sea_orm::sea_query::Expr::value("interrupted"),
        )
        .col_expr(
            pipeline_run::Column::EndedAt,
            sea_orm::sea_query::Expr::value(Some(Utc::now())),
        )
        .filter(pipeline_run::Column::Status.eq("running"))
        .exec(conn)
        .await?;
    Ok(())
}

pub async fn set_attempt_verdict(
    conn: &DatabaseConnection,
    attempt_id: i32,
    verdict: crate::models::PipelineVerdict,
    source: Option<String>,
    notes: Option<String>,
) -> Result<(), DbError> {
    let verdict_str = match verdict {
        crate::models::PipelineVerdict::Pass => "pass",
        crate::models::PipelineVerdict::ChangesRequested => "changes_requested",
        crate::models::PipelineVerdict::Inconclusive => "inconclusive",
    };
    pipeline_attempt::Entity::update_many()
        .col_expr(
            pipeline_attempt::Column::Verdict,
            sea_orm::sea_query::Expr::value(Some(verdict_str)),
        )
        .col_expr(
            pipeline_attempt::Column::VerdictSource,
            sea_orm::sea_query::Expr::value(source),
        )
        .col_expr(
            pipeline_attempt::Column::Notes,
            sea_orm::sea_query::Expr::value(notes),
        )
        .filter(pipeline_attempt::Column::Id.eq(attempt_id))
        .exec(conn)
        .await?;
    Ok(())
}

pub async fn get_attempt(
    conn: &DatabaseConnection,
    attempt_id: i32,
) -> Result<Option<PipelineAttemptInfo>, DbError> {
    Ok(pipeline_attempt::Entity::find_by_id(attempt_id)
        .one(conn)
        .await?
        .map(attempt_to_info))
}

pub fn builtin_preset_graph(key: &str) -> Option<PipelineGraph> {
    presets::builtin_presets(None)
        .into_iter()
        .find(|(preset_key, _, _)| *preset_key == key)
        .map(|(_, _, graph)| graph)
}

pub async fn attach_attempt_runtime(
    conn: &DatabaseConnection,
    attempt_id: i32,
    conversation_id: Option<i32>,
    connection_id: Option<String>,
    model_actual: Option<String>,
) -> Result<(), DbError> {
    pipeline_attempt::Entity::update_many()
        .col_expr(
            pipeline_attempt::Column::ConversationId,
            sea_orm::sea_query::Expr::value(conversation_id),
        )
        .col_expr(
            pipeline_attempt::Column::ConnectionId,
            sea_orm::sea_query::Expr::value(connection_id),
        )
        .col_expr(
            pipeline_attempt::Column::ModelActual,
            sea_orm::sea_query::Expr::value(model_actual),
        )
        .filter(pipeline_attempt::Column::Id.eq(attempt_id))
        .exec(conn)
        .await?;
    Ok(())
}

pub async fn update_run_current_step(
    conn: &DatabaseConnection,
    run_id: i32,
    current_step_id: Option<String>,
    current_iteration: u32,
    worktree_folder_id: Option<i32>,
) -> Result<(), DbError> {
    pipeline_run::Entity::update_many()
        .col_expr(
            pipeline_run::Column::CurrentStepId,
            sea_orm::sea_query::Expr::value(current_step_id),
        )
        .col_expr(
            pipeline_run::Column::CurrentIteration,
            sea_orm::sea_query::Expr::value(current_iteration as i32),
        )
        .col_expr(
            pipeline_run::Column::WorktreeFolderId,
            sea_orm::sea_query::Expr::value(worktree_folder_id),
        )
        .filter(pipeline_run::Column::Id.eq(run_id))
        .exec(conn)
        .await?;
    Ok(())
}

pub async fn set_attempt_summary(
    conn: &DatabaseConnection,
    attempt_id: i32,
    summary: Option<String>,
) -> Result<(), DbError> {
    pipeline_attempt::Entity::update_many()
        .col_expr(
            pipeline_attempt::Column::Summary,
            sea_orm::sea_query::Expr::value(summary),
        )
        .filter(pipeline_attempt::Column::Id.eq(attempt_id))
        .exec(conn)
        .await?;
    Ok(())
}

pub async fn get_last_planner_summary(
    conn: &DatabaseConnection,
    run_id: i32,
    graph: &PipelineGraph,
) -> Result<Option<String>, DbError> {
    let planner_step_ids: Vec<String> = graph
        .steps
        .iter()
        .filter(|s| s.role == crate::models::PipelineRole::Planner)
        .map(|s| s.id.clone())
        .collect();
    if planner_step_ids.is_empty() {
        return Ok(None);
    }
    let attempt = pipeline_attempt::Entity::find()
        .filter(pipeline_attempt::Column::RunId.eq(run_id))
        .filter(pipeline_attempt::Column::StepId.is_in(planner_step_ids))
        .filter(pipeline_attempt::Column::Summary.is_not_null())
        .order_by_desc(pipeline_attempt::Column::Id)
        .one(conn)
        .await?;
    Ok(attempt.and_then(|a| a.summary))
}

pub async fn get_last_changes_requested_notes(
    conn: &DatabaseConnection,
    run_id: i32,
) -> Result<Option<String>, DbError> {
    let attempt = pipeline_attempt::Entity::find()
        .filter(pipeline_attempt::Column::RunId.eq(run_id))
        .filter(pipeline_attempt::Column::Verdict.eq("changes_requested"))
        .filter(pipeline_attempt::Column::Notes.is_not_null())
        .order_by_desc(pipeline_attempt::Column::Id)
        .one(conn)
        .await?;
    Ok(attempt.and_then(|a| a.notes))
}

pub async fn list_active_runs(conn: &DatabaseConnection) -> Result<Vec<PipelineRunInfo>, DbError> {
    let models = pipeline_run::Entity::find()
        .filter(pipeline_run::Column::Status.eq("running"))
        .all(conn)
        .await?;
    let mut result = Vec::with_capacity(models.len());
    for m in models {
        result.push(run_to_info(conn, m).await?);
    }
    Ok(result)
}

pub async fn interrupt_running_attempts(
    conn: &DatabaseConnection,
    run_id: i32,
) -> Result<(), DbError> {
    pipeline_attempt::Entity::update_many()
        .col_expr(
            pipeline_attempt::Column::Status,
            sea_orm::sea_query::Expr::value("failed"),
        )
        .col_expr(
            pipeline_attempt::Column::EndedAt,
            sea_orm::sea_query::Expr::value(Some(Utc::now())),
        )
        .filter(pipeline_attempt::Column::RunId.eq(run_id))
        .filter(pipeline_attempt::Column::Status.eq("running"))
        .exec(conn)
        .await?;
    Ok(())
}

pub async fn reset_run_to_running(
    conn: &DatabaseConnection,
    run_id: i32,
    current_iteration: u32,
) -> Result<(), DbError> {
    pipeline_run::Entity::update_many()
        .col_expr(
            pipeline_run::Column::Status,
            sea_orm::sea_query::Expr::value("running"),
        )
        .col_expr(
            pipeline_run::Column::CurrentIteration,
            sea_orm::sea_query::Expr::value(current_iteration as i32),
        )
        .col_expr(
            pipeline_run::Column::Error,
            sea_orm::sea_query::Expr::value(None::<String>),
        )
        .col_expr(
            pipeline_run::Column::EndedAt,
            sea_orm::sea_query::Expr::value(None::<chrono::DateTime<Utc>>),
        )
        .filter(pipeline_run::Column::Id.eq(run_id))
        .exec(conn)
        .await?;
    Ok(())
}

/// Find the running attempt a live child connection belongs to.
///
/// The engine keeps an in-memory index of live attempts, but that index is lost
/// when the app restarts while a child agent is still connected. Looking the
/// attempt up by connection id lets a late `TurnComplete` still be recorded.
pub async fn find_running_attempt_by_connection(
    conn: &DatabaseConnection,
    connection_id: &str,
) -> Result<Option<pipeline_attempt::Model>, DbError> {
    Ok(pipeline_attempt::Entity::find()
        .filter(pipeline_attempt::Column::ConnectionId.eq(connection_id))
        .filter(pipeline_attempt::Column::Status.eq(attempt_status_to_string(AttemptStatus::Running)))
        .order_by_desc(pipeline_attempt::Column::Id)
        .one(conn)
        .await?)
}

/// Close every attempt still marked running, whatever run it belongs to.
///
/// Used at boot: attempts of a run interrupted by a restart have no live agent
/// behind them, and leaving them running would both mislead the UI and let a
/// stale connection id match a finished attempt.
pub async fn interrupt_all_running_attempts(conn: &DatabaseConnection) -> Result<u64, DbError> {
    let res = pipeline_attempt::Entity::update_many()
        .col_expr(
            pipeline_attempt::Column::Status,
            sea_orm::sea_query::Expr::value(attempt_status_to_string(AttemptStatus::Failed)),
        )
        .col_expr(
            pipeline_attempt::Column::EndedAt,
            sea_orm::sea_query::Expr::value(Some(Utc::now())),
        )
        .filter(
            pipeline_attempt::Column::Status
                .eq(attempt_status_to_string(AttemptStatus::Running)),
        )
        .exec(conn)
        .await?;
    Ok(res.rows_affected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;
    use crate::models::{PipelineRole, PipelineStep};

    fn draft(name: &str) -> PipelineDraft {
        PipelineDraft {
            name: name.into(),
            folder_id: Some(1),
            graph: PipelineGraph {
                steps: vec![PipelineStep {
                    id: "coder".into(),
                    role: PipelineRole::Coder,
                    label: "Coder".into(),
                    agent_type: "claude_code".into(),
                    mode_id: None,
                    config_values: Default::default(),
                    prompt_template: "$task".into(),
                    timeout_secs: 1800,
                    read_memory: false,
                    read_only: false,
                }],
                loops: vec![],
            },
            isolation: PipelineIsolation::WorktreePerRun,
        }
    }

    #[tokio::test]
    async fn crud_and_soft_delete_roundtrip() {
        let db = fresh_in_memory_db().await;
        let created = save(&db.conn, None, draft("nightly")).await.expect("save");
        assert_eq!(created.name, "nightly");

        let fetched = get(&db.conn, created.id).await.expect("get");
        assert_eq!(fetched.id, created.id);

        let listed = list(&db.conn, Some(1)).await.expect("list");
        assert!(listed.iter().any(|p| p.id == created.id));

        delete(&db.conn, created.id).await.expect("delete");
        assert!(matches!(
            get(&db.conn, created.id).await,
            Err(DbError::NotFound(_))
        ));
        let listed_after = list(&db.conn, Some(1)).await.expect("list after delete");
        assert!(!listed_after.iter().any(|p| p.id == created.id));
    }

    #[tokio::test]
    async fn save_rejects_invalid_graph_with_structured_error() {
        let db = fresh_in_memory_db().await;
        let mut bad = draft("bad");
        bad.graph.steps.clear();
        let err = save(&db.conn, None, bad).await.unwrap_err();
        match err {
            DbError::Validation(message) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(&message).expect("json-encoded validation error");
                assert_eq!(parsed["code"], "empty");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    /// The duet and team buttons run whatever `list_presets` overlays, so a
    /// reset has to actually remove the row — a soft delete would keep it
    /// under the partial unique index and block the next save.
    #[tokio::test]
    async fn preset_override_is_listed_then_reset_restores_the_builtin() {
        let db = fresh_in_memory_db().await;
        assert!(list_presets(&db.conn).await.expect("list").is_empty());

        let mut graph = draft("x").graph;
        graph.steps[0].agent_type = "antigravity".into();
        save_preset(&db.conn, "duet", "Duet", graph)
            .await
            .expect("save preset");

        let listed = list_presets(&db.conn).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].preset_key.as_deref(), Some("duet"));
        assert_eq!(listed[0].graph.steps[0].agent_type, "antigravity");

        reset_preset(&db.conn, "duet").await.expect("reset");
        assert!(list_presets(&db.conn).await.expect("list").is_empty());
        // Resetting twice is not an error, and the key is free again.
        reset_preset(&db.conn, "duet").await.expect("reset again");
        save_preset(&db.conn, "duet", "Duet", draft("y").graph)
            .await
            .expect("save after reset");
    }

    #[tokio::test]
    async fn save_preset_does_not_duplicate_rows() {
        let db = fresh_in_memory_db().await;
        let graph = draft("duet").graph;
        let first = save_preset(&db.conn, "duet", "Duet", graph.clone())
            .await
            .expect("first save");
        let second = save_preset(&db.conn, "duet", "Duet", graph)
            .await
            .expect("second save");
        assert_eq!(first.id, second.id);
        let all = list(&db.conn, None).await.expect("list");
        assert_eq!(
            all.iter()
                .filter(|p| p.preset_key.as_deref() == Some("duet"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn cas_attempt_status_only_succeeds_from_expected_state() {
        let db = fresh_in_memory_db().await;
        let run = create_run(
            &db.conn,
            None,
            1,
            &draft("duet").graph,
            PipelineIsolation::WorktreePerRun,
            None,
            None,
        )
        .await
        .expect("create run");
        let attempt = create_attempt(&db.conn, run.id, "coder".into(), 1, None, None)
            .await
            .expect("create attempt");

        // A stale CAS (wrong `from`) must not apply.
        let stale = cas_attempt_status(
            &db.conn,
            attempt.id,
            AttemptStatus::Done,
            AttemptStatus::Failed,
        )
        .await
        .expect("cas call");
        assert!(!stale);

        let ok = cas_attempt_status(
            &db.conn,
            attempt.id,
            AttemptStatus::Running,
            AttemptStatus::Done,
        )
        .await
        .expect("cas call");
        assert!(ok);

        let reloaded = pipeline_attempt::Entity::find_by_id(attempt.id)
            .one(&db.conn)
            .await
            .expect("query")
            .expect("row");
        assert_eq!(reloaded.status, "done");
        assert!(reloaded.ended_at.is_some());

        // Second transition from the now-stale `Running` state is rejected.
        let repeat = cas_attempt_status(
            &db.conn,
            attempt.id,
            AttemptStatus::Running,
            AttemptStatus::Failed,
        )
        .await
        .expect("cas call");
        assert!(!repeat);
    }

    #[tokio::test]
    async fn update_run_status_cas_only_transitions_running_runs() {
        let db = fresh_in_memory_db().await;
        let run = create_run(
            &db.conn,
            None,
            1,
            &draft("duet").graph,
            PipelineIsolation::WorktreePerRun,
            None,
            None,
        )
        .await
        .expect("create run");

        let ok = update_run_status(&db.conn, run.id, PipelineRunStatus::Succeeded, None)
            .await
            .expect("cas call");
        assert!(ok);

        let reloaded = pipeline_run::Entity::find_by_id(run.id)
            .one(&db.conn)
            .await
            .expect("query")
            .expect("row");
        assert_eq!(reloaded.status, "succeeded");
        assert!(reloaded.ended_at.is_some());

        // Already settled: a second CAS must not overwrite it.
        let second = update_run_status(&db.conn, run.id, PipelineRunStatus::Failed, None)
            .await
            .expect("cas call");
        assert!(!second);
        let unchanged = pipeline_run::Entity::find_by_id(run.id)
            .one(&db.conn)
            .await
            .expect("query")
            .expect("row");
        assert_eq!(unchanged.status, "succeeded");
    }
}


