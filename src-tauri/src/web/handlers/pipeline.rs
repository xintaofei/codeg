use std::sync::Arc;

use axum::{extract::Extension, Json};

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::pipeline as core;
use crate::models::{PipelineInfo, PipelineRunInfo};

pub async fn pipeline_list(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::PipelineListParams>,
) -> Result<Json<Vec<PipelineInfo>>, AppCommandError> {
    Ok(Json(
        core::pipeline_list_core(&state.db, params.folder_id).await?,
    ))
}

pub async fn pipeline_get(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::PipelineIdParams>,
) -> Result<Json<PipelineInfo>, AppCommandError> {
    Ok(Json(core::pipeline_get_core(&state.db, params.id).await?))
}

pub async fn pipeline_save(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::PipelineSaveParams>,
) -> Result<Json<PipelineInfo>, AppCommandError> {
    Ok(Json(
        core::pipeline_save_core(&state.emitter, &state.db, params.id, params.draft).await?,
    ))
}

pub async fn pipeline_delete(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::PipelineIdParams>,
) -> Result<Json<()>, AppCommandError> {
    core::pipeline_delete_core(&state.emitter, &state.db, params.id).await?;
    Ok(Json(()))
}

pub async fn pipeline_presets() -> Result<Json<Vec<PipelineInfo>>, AppCommandError> {
    Ok(Json(core::pipeline_presets_core().await))
}

pub async fn pipeline_run(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::PipelineRunParams>,
) -> Result<Json<PipelineRunInfo>, AppCommandError> {
    Ok(Json(
        core::pipeline_run_core(&state.emitter, &state.db, params.request).await?,
    ))
}

pub async fn pipeline_cancel(
    Json(params): Json<core::PipelineRunIdParams>,
) -> Result<Json<()>, AppCommandError> {
    core::pipeline_cancel_core(params.run_id).await?;
    Ok(Json(()))
}

pub async fn pipeline_run_status(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::PipelineRunIdParams>,
) -> Result<Json<PipelineRunInfo>, AppCommandError> {
    Ok(Json(
        core::pipeline_run_status_core(&state.db, params.run_id).await?,
    ))
}

pub async fn pipeline_runs(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::PipelineRunsParams>,
) -> Result<Json<Vec<PipelineRunInfo>>, AppCommandError> {
    Ok(Json(
        core::pipeline_runs_core(&state.db, params.folder_id, params.limit).await?,
    ))
}

pub async fn pipeline_request_changes(
    Json(params): Json<core::PipelineRequestChangesParams>,
) -> Result<Json<()>, AppCommandError> {
    core::pipeline_request_changes_core(params.run_id, params.notes).await?;
    Ok(Json(()))
}

pub async fn pipeline_stop_manual(
    Json(params): Json<core::PipelineRunIdParams>,
) -> Result<Json<()>, AppCommandError> {
    core::pipeline_stop_manual_core(params.run_id).await?;
    Ok(Json(()))
}

pub async fn pipeline_run_diff(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::PipelineRunIdParams>,
) -> Result<Json<core::PipelineDiff>, AppCommandError> {
    Ok(Json(
        core::pipeline_run_diff_core(&state.db, params.run_id).await?,
    ))
}

pub async fn pipeline_run_apply(
    Json(params): Json<core::PipelineApplyParams>,
) -> Result<Json<()>, AppCommandError> {
    core::pipeline_run_apply_core(params.run_id, params.strategy).await?;
    Ok(Json(()))
}
