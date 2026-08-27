use axum::Json;
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::commands::agent_rules::{self, AgentRulesInspectResult, AgentRulesRenderResult};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectParams {
    root_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderParams {
    root_path: String,
    rule_ids: Vec<String>,
    expected_source_hash: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProfileParams {
    root_path: String,
    name: String,
    rule_ids: Vec<String>,
    expected_source_hash: String,
    set_default: bool,
    overwrite: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameProfileParams {
    root_path: String,
    old_name: String,
    new_name: String,
    overwrite: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProfileParams {
    root_path: String,
    name: String,
}

pub async fn inspect(
    Json(params): Json<InspectParams>,
) -> Result<Json<AgentRulesInspectResult>, AppCommandError> {
    agent_rules::agent_rules_inspect(params.root_path)
        .await
        .map(Json)
}

pub async fn render(
    Json(params): Json<RenderParams>,
) -> Result<Json<AgentRulesRenderResult>, AppCommandError> {
    agent_rules::agent_rules_render(
        params.root_path,
        params.rule_ids,
        params.expected_source_hash,
    )
    .await
    .map(Json)
}

pub async fn save_profile(
    Json(params): Json<SaveProfileParams>,
) -> Result<Json<AgentRulesInspectResult>, AppCommandError> {
    agent_rules::agent_rules_save_profile(
        params.root_path,
        params.name,
        params.rule_ids,
        params.expected_source_hash,
        params.set_default,
        params.overwrite,
    )
    .await
    .map(Json)
}

pub async fn rename_profile(
    Json(params): Json<RenameProfileParams>,
) -> Result<Json<AgentRulesInspectResult>, AppCommandError> {
    agent_rules::agent_rules_rename_profile(
        params.root_path,
        params.old_name,
        params.new_name,
        params.overwrite,
    )
    .await
    .map(Json)
}

pub async fn delete_profile(
    Json(params): Json<DeleteProfileParams>,
) -> Result<Json<AgentRulesInspectResult>, AppCommandError> {
    agent_rules::agent_rules_delete_profile(params.root_path, params.name)
        .await
        .map(Json)
}
