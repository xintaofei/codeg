//! DTOs for the loop engineering subsystem. Field names are snake_case so the
//! serialized JSON matches the TypeScript mirrors in `src/lib/types.ts`. Entity
//! enums are reused directly (single source of truth for the wire vocabulary).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::agent::AgentType;
use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus, ReviewVerdict};
use crate::db::entities::loop_artifact_revision::ActorKind;
use crate::db::entities::loop_inbox_item::{InboxKind, InboxStatus};
use crate::db::entities::loop_issue::{IssuePriority, IssueRoute, IssueStatus, PauseReason};
use crate::db::entities::loop_iteration::{IterationStatus, LaunchedBy, Stage};
use crate::db::entities::loop_link::LinkKind;
use crate::db::entities::loop_memory::{MemoryKind, MemoryStatus};

fn config_version() -> u32 {
    1
}

/// One reviewer in a task's review round: which agent runs it, plus the same
/// startup mode/config knobs the regular sub-agent settings expose. The number
/// of configured reviewers is the number of concurrent reviews run per task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewerSpec {
    pub agent: AgentType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config_values: BTreeMap<String, String>,
}

/// Per-issue Loop Contract knobs (stored JSON-encoded in `loop_issue.config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueConfig {
    #[serde(default = "config_version")]
    pub v: u32,
    /// Agent per stage; the `"default"` key is the fallback. Stage-specific keys
    /// (e.g. `"review"`) override it.
    pub agents: BTreeMap<String, AgentType>,
    /// Deterministic verification commands, run in the worktree after implement.
    pub validation_commands: Vec<String>,
    /// Concurrent reviewer agents per task.
    pub reviewer_count: u32,
    /// `"unanimous"` (any fail → rework) or `"majority"`.
    pub review_pass_rule: String,
    /// Node rework cap before the no-progress breaker trips.
    pub max_attempts: u32,
    /// When false (default), result merge requires human approval.
    pub auto_merge: bool,
    /// Human override of the triage-decided route, if any.
    pub force_route: Option<IssueRoute>,
    /// Optional per-iteration wall-clock cap (none = unlimited).
    pub iteration_timeout_secs: Option<u64>,
    /// Optional per-turn token soft cap (none = unlimited).
    pub token_budget_per_turn: Option<i64>,
    /// Reviewers to run per task (one review iteration each). When empty, falls
    /// back to `reviewer_count` copies of the resolved review agent (see
    /// [`IssueConfig::effective_reviewers`]) so pre-existing issues are unchanged.
    #[serde(default)]
    pub reviewers: Vec<ReviewerSpec>,
    /// Optional watchdog: file a `stalled` inbox card when an iteration has been
    /// in flight (turn running, not yet settled) for at least this many seconds.
    /// `tokens_used` only lands at settle, so there is no mid-turn progress
    /// counter to diff — elapsed-since-start is the honest in-flight signal. None
    /// (default) = off = no alert (honors "no artificial limits"). Never
    /// auto-cancels — only surfaces to the human, who decides whether to step in.
    #[serde(default)]
    pub stall_alert_secs: Option<u64>,
}

impl Default for IssueConfig {
    fn default() -> Self {
        let mut agents = BTreeMap::new();
        agents.insert("default".to_string(), AgentType::ClaudeCode);
        Self {
            v: 1,
            agents,
            validation_commands: Vec::new(),
            reviewer_count: 1,
            review_pass_rule: "unanimous".to_string(),
            max_attempts: 6,
            auto_merge: false,
            force_route: None,
            iteration_timeout_secs: None,
            token_budget_per_turn: None,
            reviewers: Vec::new(),
            stall_alert_secs: None,
        }
    }
}

impl IssueConfig {
    /// The effective reviewer list: the explicit `reviewers` when non-empty,
    /// else `reviewer_count` copies of the resolved review agent (a `"review"`
    /// stage override, then `"default"`, then Claude Code) with no extra
    /// mode/config. Keeps pre-`reviewers` issues working unchanged.
    pub fn effective_reviewers(&self) -> Vec<ReviewerSpec> {
        if !self.reviewers.is_empty() {
            return self.reviewers.clone();
        }
        let agent = self
            .agents
            .get("review")
            .or_else(|| self.agents.get("default"))
            .copied()
            .unwrap_or(AgentType::ClaudeCode);
        let n = self.reviewer_count.max(1) as usize;
        (0..n)
            .map(|_| ReviewerSpec {
                agent,
                mode_id: None,
                config_values: BTreeMap::new(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopSpaceSummary {
    pub id: i32,
    pub name: String,
    pub folder_id: i32,
    pub folder_path: Option<String>,
    /// True when the bound folder is soft-deleted or missing (read-only space).
    pub detached: bool,
    pub issue_count: i64,
    pub running_count: i64,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// Space default issue config (parsed). `None` = no default set (engine
    /// default applies to inheriting issues).
    pub default_config: Option<IssueConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopIssueRow {
    pub id: i32,
    pub space_id: i32,
    pub seq_no: i32,
    pub title: String,
    pub priority: IssuePriority,
    pub status: IssueStatus,
    pub pause_reason: Option<PauseReason>,
    pub route: IssueRoute,
    pub token_used: i64,
    pub token_budget: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopIssueDetail {
    #[serde(flatten)]
    pub row: LoopIssueRow,
    pub description: String,
    pub config: IssueConfig,
    pub worktree_folder_id: Option<i32>,
    pub base_branch: Option<String>,
    pub base_commit: Option<String>,
    pub active_task_artifact_id: Option<i32>,
    /// When true the issue uses the space default config; `config` above is its
    /// preserved last custom value.
    pub config_inherits: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopArtifactRow {
    pub id: i32,
    pub issue_id: i32,
    pub issue_seq: i32,
    pub kind: ArtifactKind,
    pub title: String,
    pub status: ArtifactStatus,
    pub origin: ActorKind,
    pub produced_by_iteration_id: Option<i32>,
    pub verdict: Option<ReviewVerdict>,
    pub attempt: i32,
    pub sort: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopRevision {
    pub id: i32,
    pub seq: i32,
    pub content: String,
    pub actor_kind: ActorKind,
    pub iteration_id: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopCriterionRow {
    pub id: i32,
    pub label: String,
    pub text: String,
    pub sort: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopLinkRow {
    pub id: i32,
    pub from_artifact_id: i32,
    pub to_artifact_id: i32,
    pub kind: LinkKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopArtifactDetail {
    #[serde(flatten)]
    pub row: LoopArtifactRow,
    pub revisions: Vec<LoopRevision>,
    pub criteria: Vec<LoopCriterionRow>,
    pub links: Vec<LoopLinkRow>,
}

/// Per-issue DAG payload (nodes + edges) for the graph/board views.
#[derive(Debug, Clone, Serialize)]
pub struct LoopDagView {
    pub artifacts: Vec<LoopArtifactRow>,
    pub links: Vec<LoopLinkRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopIterationRow {
    pub id: i32,
    pub issue_id: i32,
    pub issue_seq: i32,
    pub stage: Stage,
    pub target_artifact_id: Option<i32>,
    pub target_title: Option<String>,
    pub conversation_id: Option<i32>,
    pub status: IterationStatus,
    pub launched_by: LaunchedBy,
    pub attempt: i32,
    pub tokens_used: i64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopValidationRunRow {
    pub id: i32,
    pub task_artifact_id: i32,
    pub iteration_id: Option<i32>,
    pub commands: Vec<String>,
    pub exit_codes: Vec<i32>,
    pub passed: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopInboxItemRow {
    pub id: i32,
    pub issue_id: i32,
    pub issue_seq: i32,
    pub iteration_id: Option<i32>,
    pub kind: InboxKind,
    pub subject_key: String,
    pub payload: serde_json::Value,
    pub status: InboxStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopMemoryRow {
    pub id: i32,
    pub kind: MemoryKind,
    pub source: ActorKind,
    pub title: String,
    pub content: String,
    pub status: MemoryStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Coarse cache-invalidation event (`loop://changed`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopChanged {
    pub v: u32,
    pub space_id: i32,
    pub issue_id: Option<i32>,
    pub subject_kind: String,
    pub subject_id: i32,
    pub kind: String,
}

/// Event name for [`LoopChanged`]. Lives beside the payload so both the command
/// layer (CRUD writes) and the engine (autonomous dispatch/settle) emit the same
/// channel without one depending on the other.
pub const LOOP_CHANGED_EVENT: &str = "loop://changed";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_reviewers_falls_back_to_count() {
        let cfg = IssueConfig {
            reviewer_count: 2,
            ..IssueConfig::default()
        };
        let r = cfg.effective_reviewers();
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|s| s.agent == AgentType::ClaudeCode));
        assert!(r
            .iter()
            .all(|s| s.mode_id.is_none() && s.config_values.is_empty()));
    }

    #[test]
    fn effective_reviewers_uses_review_stage_agent_for_fallback() {
        let mut agents = BTreeMap::new();
        agents.insert("review".to_string(), AgentType::Codex);
        let cfg = IssueConfig {
            agents,
            reviewer_count: 1,
            ..IssueConfig::default()
        };
        let r = cfg.effective_reviewers();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].agent, AgentType::Codex);
    }

    #[test]
    fn effective_reviewers_prefers_explicit_list() {
        let cfg = IssueConfig {
            reviewer_count: 5, // ignored when `reviewers` is non-empty
            reviewers: vec![ReviewerSpec {
                agent: AgentType::Gemini,
                mode_id: Some("auto".to_string()),
                config_values: BTreeMap::new(),
            }],
            ..IssueConfig::default()
        };
        let r = cfg.effective_reviewers();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].agent, AgentType::Gemini);
        assert_eq!(r[0].mode_id.as_deref(), Some("auto"));
    }

    #[test]
    fn old_config_json_without_reviewers_deserializes() {
        // A config JSON predating `reviewers` still parses (serde default) and
        // falls back to `reviewer_count` reviewers.
        let json = r#"{"v":1,"agents":{"default":"claude_code"},"validation_commands":[],"reviewer_count":3,"review_pass_rule":"unanimous","max_attempts":6,"auto_merge":false,"force_route":null,"iteration_timeout_secs":null,"token_budget_per_turn":null}"#;
        let cfg: IssueConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.reviewers.is_empty());
        assert_eq!(cfg.effective_reviewers().len(), 3);
    }
}
