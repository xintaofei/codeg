use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::loops as core;
use crate::db::entities::loop_inbox_item::InboxStatus;
use crate::db::entities::loop_issue::{IssuePriority, IssueStatus};
use crate::db::entities::loop_memory::{MemoryKind, MemoryStatus};
use crate::models::loops::{
    IssueConfig, LoopArtifactDetail, LoopArtifactRow, LoopDagView, LoopInboxItemRow,
    LoopIssueDetail, LoopIssueRow, LoopIterationRow, LoopMemoryRow, LoopSpaceSummary,
    LoopValidationRunRow,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdParam {
    pub id: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceIdParam {
    pub space_id: i32,
}

// ─── Spaces ──────────────────────────────────────────────────────────────

pub async fn list_loop_spaces(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<LoopSpaceSummary>>, AppCommandError> {
    Ok(Json(core::list_loop_spaces_core(&state.db.conn).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSpaceParams {
    pub name: String,
    pub folder_id: i32,
}

pub async fn create_loop_space(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<CreateSpaceParams>,
) -> Result<Json<LoopSpaceSummary>, AppCommandError> {
    Ok(Json(
        core::create_loop_space_core(&state.db.conn, &state.emitter, p.name, p.folder_id).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSpaceParams {
    pub id: i32,
    pub name: String,
}

pub async fn update_loop_space(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<UpdateSpaceParams>,
) -> Result<Json<LoopSpaceSummary>, AppCommandError> {
    Ok(Json(
        core::update_loop_space_core(&state.db.conn, &state.emitter, p.id, p.name).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSpaceDefaultConfigParams {
    pub id: i32,
    pub config: Option<IssueConfig>,
}

pub async fn set_loop_space_default_config(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<SetSpaceDefaultConfigParams>,
) -> Result<Json<()>, AppCommandError> {
    core::set_loop_space_default_config_core(&state.db.conn, &state.emitter, p.id, p.config).await?;
    Ok(Json(()))
}

pub async fn delete_loop_space(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<()>, AppCommandError> {
    core::delete_loop_space_core(&state.db.conn, &state.emitter, p.id).await?;
    Ok(Json(()))
}

// ─── Issues ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListIssuesParams {
    pub space_id: i32,
    pub statuses: Option<Vec<IssueStatus>>,
}

pub async fn list_loop_issues(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<ListIssuesParams>,
) -> Result<Json<Vec<LoopIssueRow>>, AppCommandError> {
    Ok(Json(
        core::list_loop_issues_core(&state.db.conn, p.space_id, p.statuses).await?,
    ))
}

pub async fn get_loop_issue(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<Option<LoopIssueDetail>>, AppCommandError> {
    Ok(Json(core::get_loop_issue_core(&state.db.conn, p.id).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateIssueParams {
    pub space_id: i32,
    pub title: String,
    pub description: String,
    pub priority: IssuePriority,
    pub config: Option<IssueConfig>,
}

pub async fn create_loop_issue(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<CreateIssueParams>,
) -> Result<Json<LoopIssueDetail>, AppCommandError> {
    Ok(Json(
        core::create_loop_issue_core(
            &state.db.conn,
            &state.emitter,
            p.space_id,
            p.title,
            p.description,
            p.priority,
            p.config,
        )
        .await?,
    ))
}

pub async fn delete_loop_issue(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<()>, AppCommandError> {
    core::delete_loop_issue_core(&state.db.conn, &state.emitter, p.id).await?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateIssueConfigParams {
    pub id: i32,
    pub config: IssueConfig,
    pub token_budget: Option<i64>,
    pub config_inherits: bool,
}

pub async fn update_loop_issue_config(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<UpdateIssueConfigParams>,
) -> Result<Json<()>, AppCommandError> {
    core::update_loop_issue_config_core(
        &state.db.conn,
        &state.emitter,
        p.id,
        p.config,
        p.token_budget,
        p.config_inherits,
    )
    .await?;
    Ok(Json(()))
}

// ─── Engine actions (trigger / pause / resume / cancel) ─────────────────────

pub async fn trigger_loop_issue(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<()>, AppCommandError> {
    core::trigger_loop_issue_core(&state.db.conn, &state.emitter, &state.loop_engine, p.id).await?;
    Ok(Json(()))
}

pub async fn pause_loop_issue(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<()>, AppCommandError> {
    core::pause_loop_issue_core(&state.db.conn, &state.emitter, &state.loop_engine, p.id).await?;
    Ok(Json(()))
}

pub async fn resume_loop_issue(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<()>, AppCommandError> {
    core::resume_loop_issue_core(&state.db.conn, &state.emitter, &state.loop_engine, p.id).await?;
    Ok(Json(()))
}

pub async fn cancel_loop_issue(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<()>, AppCommandError> {
    core::cancel_loop_issue_core(&state.db.conn, &state.emitter, &state.loop_engine, p.id).await?;
    Ok(Json(()))
}

pub async fn retry_loop_issue(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<()>, AppCommandError> {
    core::retry_loop_issue_core(&state.db.conn, &state.emitter, &state.loop_engine, p.id).await?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddBudgetParams {
    pub id: i32,
    pub additional: i64,
}

pub async fn add_loop_issue_budget(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<AddBudgetParams>,
) -> Result<Json<()>, AppCommandError> {
    core::add_loop_issue_budget_core(
        &state.db.conn,
        &state.emitter,
        &state.loop_engine,
        p.id,
        p.additional,
    )
    .await?;
    Ok(Json(()))
}

pub async fn approve_loop_merge(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<()>, AppCommandError> {
    core::approve_loop_merge_core(&state.db.conn, &state.emitter, &state.loop_engine, p.id).await?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectMergeParams {
    pub id: i32,
    pub comment: Option<String>,
}

pub async fn reject_loop_merge(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<RejectMergeParams>,
) -> Result<Json<()>, AppCommandError> {
    core::reject_loop_merge_core(
        &state.db.conn,
        &state.emitter,
        &state.loop_engine,
        p.id,
        p.comment,
    )
    .await?;
    Ok(Json(()))
}

pub async fn approve_loop_design(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<()>, AppCommandError> {
    core::approve_loop_design_core(&state.db.conn, &state.emitter, &state.loop_engine, p.id).await?;
    Ok(Json(()))
}

pub async fn reject_loop_design(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<RejectMergeParams>,
) -> Result<Json<()>, AppCommandError> {
    core::reject_loop_design_core(
        &state.db.conn,
        &state.emitter,
        &state.loop_engine,
        p.id,
        p.comment,
    )
    .await?;
    Ok(Json(()))
}

// ─── Artifacts / DAG ───────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueIdParam {
    pub issue_id: i32,
}

pub async fn get_loop_dag(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IssueIdParam>,
) -> Result<Json<LoopDagView>, AppCommandError> {
    Ok(Json(core::get_loop_dag_core(&state.db.conn, p.issue_id).await?))
}

pub async fn list_loop_artifacts(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<SpaceIdParam>,
) -> Result<Json<Vec<LoopArtifactRow>>, AppCommandError> {
    Ok(Json(
        core::list_loop_artifacts_core(&state.db.conn, p.space_id).await?,
    ))
}

pub async fn get_loop_artifact(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<IdParam>,
) -> Result<Json<Option<LoopArtifactDetail>>, AppCommandError> {
    Ok(Json(core::get_loop_artifact_core(&state.db.conn, p.id).await?))
}

// ─── Iterations ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListIterationsParams {
    pub space_id: i32,
    pub issue_id: Option<i32>,
}

pub async fn list_loop_iterations(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<ListIterationsParams>,
) -> Result<Json<Vec<LoopIterationRow>>, AppCommandError> {
    Ok(Json(
        core::list_loop_iterations_core(&state.db.conn, p.space_id, p.issue_id).await?,
    ))
}

pub async fn list_loop_validations(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<SpaceIdParam>,
) -> Result<Json<Vec<LoopValidationRunRow>>, AppCommandError> {
    Ok(Json(
        core::list_loop_validations_core(&state.db.conn, p.space_id).await?,
    ))
}

// ─── Inbox ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListInboxParams {
    pub space_id: i32,
    pub status: Option<InboxStatus>,
}

pub async fn list_loop_inbox(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<ListInboxParams>,
) -> Result<Json<Vec<LoopInboxItemRow>>, AppCommandError> {
    Ok(Json(
        core::list_loop_inbox_core(&state.db.conn, p.space_id, p.status).await?,
    ))
}

// ─── Memory ────────────────────────────────────────────────────────────────

pub async fn list_loop_memory(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<SpaceIdParam>,
) -> Result<Json<Vec<LoopMemoryRow>>, AppCommandError> {
    Ok(Json(
        core::list_loop_memory_core(&state.db.conn, p.space_id).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMemoryParams {
    pub space_id: i32,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
}

pub async fn create_loop_memory(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<CreateMemoryParams>,
) -> Result<Json<LoopMemoryRow>, AppCommandError> {
    Ok(Json(
        core::create_loop_memory_core(
            &state.db.conn,
            &state.emitter,
            p.space_id,
            p.kind,
            p.title,
            p.content,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMemoryParams {
    pub space_id: i32,
    pub id: i32,
    pub title: String,
    pub content: String,
    pub status: MemoryStatus,
}

pub async fn update_loop_memory(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<UpdateMemoryParams>,
) -> Result<Json<()>, AppCommandError> {
    core::update_loop_memory_core(
        &state.db.conn,
        &state.emitter,
        p.space_id,
        p.id,
        p.title,
        p.content,
        p.status,
    )
    .await?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteMemoryParams {
    pub space_id: i32,
    pub id: i32,
}

pub async fn delete_loop_memory(
    Extension(state): Extension<Arc<AppState>>,
    Json(p): Json<DeleteMemoryParams>,
) -> Result<Json<()>, AppCommandError> {
    core::delete_loop_memory_core(&state.db.conn, &state.emitter, p.space_id, p.id).await?;
    Ok(Json(()))
}
