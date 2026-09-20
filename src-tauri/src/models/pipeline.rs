use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use crate::db::entities::automation::IsolationMode as PipelineIsolation;

fn default_step_timeout() -> u64 {
    1800
}

fn default_max_iterations() -> u32 {
    3
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineRole {
    Planner,
    Coder,
    Reviewer,
    Tests,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStep {
    pub id: String,
    pub role: PipelineRole,
    pub label: String,
    pub agent_type: String,
    #[serde(default)]
    pub mode_id: Option<String>,
    #[serde(default)]
    pub config_values: BTreeMap<String, String>,
    pub prompt_template: String,
    #[serde(default = "default_step_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub read_memory: bool,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopBack {
    pub from_step: String,
    pub to_step: String,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PipelineGraph {
    pub steps: Vec<PipelineStep>,
    #[serde(default)]
    pub loops: Vec<LoopBack>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineInfo {
    pub id: i32,
    pub name: String,
    pub preset_key: Option<String>,
    pub folder_id: Option<i32>,
    pub graph: PipelineGraph,
    pub isolation: PipelineIsolation,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PipelineDraft {
    pub name: String,
    pub folder_id: Option<i32>,
    pub graph: PipelineGraph,
    pub isolation: PipelineIsolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineRunStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
    StoppedMaxIterations,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineVerdict {
    Pass,
    ChangesRequested,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Running,
    Done,
    Cancelled,
    TimedOut,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineAttemptInfo {
    pub id: i32,
    pub run_id: i32,
    pub step_id: String,
    /// Monotonically increasing across the whole run (not reset per step):
    /// a loop-back to an earlier step continues the same counter rather
    /// than restarting it.
    pub iteration: u32,
    pub status: AttemptStatus,
    pub conversation_id: Option<i32>,
    pub model_requested: Option<String>,
    pub model_actual: Option<String>,
    pub verdict: Option<PipelineVerdict>,
    pub verdict_source: Option<String>,
    pub notes: Option<String>,
    pub summary: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineRunInfo {
    pub id: i32,
    pub pipeline_id: Option<i32>,
    pub folder_id: i32,
    pub worktree_folder_id: Option<i32>,
    pub parent_conversation_id: Option<i32>,
    pub graph: PipelineGraph,
    pub status: PipelineRunStatus,
    pub current_step_id: Option<String>,
    pub current_iteration: u32,
    pub error: Option<String>,
    pub attempts: Vec<PipelineAttemptInfo>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRunRequest {
    pub folder_id: i32,
    pub pipeline_id: Option<i32>,
    pub graph: Option<PipelineGraph>,
    pub isolation: Option<PipelineIsolation>,
    pub prompt_blocks: Vec<serde_json::Value>,
    pub display_text: String,
    pub parent_conversation_id: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_graph_roundtrips_through_serde() {
        let graph = PipelineGraph {
            steps: vec![PipelineStep {
                id: "coder".into(),
                role: PipelineRole::Coder,
                label: "Coder".into(),
                agent_type: "claude_code".into(),
                mode_id: Some("plan".into()),
                config_values: BTreeMap::from([("model".into(), "sonnet".into())]),
                prompt_template: "$task".into(),
                timeout_secs: 900,
                read_memory: true,
                read_only: false,
            }],
            loops: vec![LoopBack {
                from_step: "coder".into(),
                to_step: "coder".into(),
                max_iterations: 5,
            }],
        };
        let json = serde_json::to_string(&graph).expect("serialize");
        let back: PipelineGraph = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.steps.len(), 1);
        assert_eq!(back.steps[0].id, "coder");
        assert_eq!(back.steps[0].config_values.get("model").unwrap(), "sonnet");
        assert_eq!(back.loops[0].max_iterations, 5);
    }

    #[test]
    fn pipeline_run_request_accepts_camel_case_wire_format() {
        let json = r#"{
            "folderId": 1,
            "pipelineId": null,
            "graph": null,
            "isolation": null,
            "promptBlocks": [],
            "displayText": "run it",
            "parentConversationId": 7
        }"#;
        let req: PipelineRunRequest = serde_json::from_str(json).expect("deserialize");
        assert_eq!(req.folder_id, 1);
        assert_eq!(req.parent_conversation_id, Some(7));
    }
}
