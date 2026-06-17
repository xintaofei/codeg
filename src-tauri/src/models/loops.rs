//! DTOs for the loop engineering subsystem. Field names are snake_case so the
//! serialized JSON matches the TypeScript mirrors in `src/lib/types.ts`. Entity
//! enums are reused directly (single source of truth for the wire vocabulary).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::agent::AgentType;
use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus, ReviewVerdict};
use crate::db::entities::loop_artifact_revision::ActorKind;
use crate::db::entities::loop_criterion::CriterionKind;
use crate::db::entities::loop_criterion_check::CheckVerdict;
use crate::db::entities::loop_gate_decision::GateOutcome;
use crate::db::entities::loop_inbox_item::{InboxKind, InboxStatus};
use crate::db::entities::loop_issue::{IssuePriority, IssueRoute, IssueStatus, PauseReason};
use crate::db::entities::loop_iteration::{IterationStatus, LaunchedBy, Stage};
use crate::db::entities::loop_link::LinkKind;
use crate::db::entities::loop_memory::{MemoryKind, MemoryStatus, TrustTier};

/// An agent plus the same startup mode/config knobs the regular sub-agent
/// settings expose. Used both for each per-stage agent override (a field of
/// [`StageAgents`]) and for each reviewer in a task's review round. Always
/// serialized and parsed as an object (`{"agent": "...", ...}`); empty
/// mode/config are skipped on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSpec {
    pub agent: AgentType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config_values: BTreeMap<String, String>,
}

/// Historical name retained as an alias to avoid churn at reviewer call sites.
pub type ReviewerSpec = AgentSpec;

/// The `{"inherit": true}` reviewer form. The bool is always `true`; its
/// presence is what distinguishes inherit from a concrete [`AgentSpec`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewerInherit {
    pub inherit: bool,
}

/// One reviewer in [`IssueConfig::reviewers`]: a concrete [`AgentSpec`] object,
/// or the `{"inherit": true}` marker that defers to the issue's default agent at
/// dispatch. Untagged — the `Inherit` arm requires an `inherit` key (which an
/// `AgentSpec` object never carries), so an agent object always parses as `Spec`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ReviewerEntry {
    /// `{"inherit": true}` — use the issue's default agent.
    Inherit(ReviewerInherit),
    /// A concrete agent + its startup mode/config.
    Spec(AgentSpec),
}

/// How a task's review round aggregates its reviewer verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPassRule {
    /// Any fail → rework.
    Unanimous,
    /// Pass if more than half pass.
    Majority,
}

/// Per-stage agent overrides. `default` is required and is used for any stage
/// without an explicit override. There is intentionally no `review` field —
/// reviewers are configured via [`IssueConfig::reviewers`], which resolve their
/// inherit markers against `default`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StageAgents {
    pub default: AgentSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<AgentSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refine: Option<AgentSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design: Option<AgentSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<AgentSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implement: Option<AgentSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalize: Option<AgentSpec>,
}

impl StageAgents {
    /// The agent spec for a stage: its override if set, else `default`. `Review`
    /// resolves to `default` (review dispatch uses the reviewers list, not this).
    pub fn for_stage(&self, stage: Stage) -> &AgentSpec {
        let o: Option<&AgentSpec> = match stage {
            Stage::Triage => self.triage.as_ref(),
            Stage::Refine => self.refine.as_ref(),
            Stage::Design => self.design.as_ref(),
            Stage::Plan => self.plan.as_ref(),
            Stage::Implement => self.implement.as_ref(),
            Stage::Finalize => self.finalize.as_ref(),
            Stage::Review => None,
        };
        o.unwrap_or(&self.default)
    }
}

/// Per-issue Loop Contract knobs (stored JSON-encoded in `loop_issue.config`,
/// or in `loop_space.default_config` for the space default).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueConfig {
    /// Per-stage agents (`default` + optional single-stage overrides).
    pub agents: StageAgents,
    /// Deterministic verification commands, run in the worktree after implement.
    pub validation_commands: Vec<String>,
    /// Reviewers to run per task (one review iteration each); the count of
    /// concurrent reviews = `reviewers.len()`. Each entry is a concrete
    /// [`AgentSpec`] or an inherit marker (defers to `agents.default`). Required
    /// non-empty — see [`IssueConfig::validate`].
    pub reviewers: Vec<ReviewerEntry>,
    /// How reviewer verdicts aggregate.
    pub review_pass_rule: ReviewPassRule,
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
    /// Optional watchdog: file a `stalled` inbox card when an iteration has been
    /// in flight (turn running, not yet settled) for at least this many seconds.
    /// `tokens_used` only lands at settle, so there is no mid-turn progress
    /// counter to diff — elapsed-since-start is the honest in-flight signal. None
    /// = off = no alert (honors "no artificial limits"). Never auto-cancels —
    /// only surfaces to the human, who decides whether to step in.
    pub stall_alert_secs: Option<u64>,
}

impl Default for IssueConfig {
    fn default() -> Self {
        Self {
            agents: StageAgents {
                default: AgentSpec {
                    agent: AgentType::ClaudeCode,
                    mode_id: None,
                    config_values: BTreeMap::new(),
                },
                triage: None,
                refine: None,
                design: None,
                plan: None,
                implement: None,
                finalize: None,
            },
            validation_commands: Vec::new(),
            // One reviewer that inherits the default agent.
            reviewers: vec![ReviewerEntry::Inherit(ReviewerInherit { inherit: true })],
            review_pass_rule: ReviewPassRule::Unanimous,
            max_attempts: 6,
            auto_merge: false,
            force_route: None,
            iteration_timeout_secs: None,
            token_budget_per_turn: None,
            stall_alert_secs: None,
        }
    }
}

impl IssueConfig {
    /// Resolve each reviewer slot to a concrete agent: an inherit marker becomes
    /// `agents.default` (carrying its mode/config); a concrete entry passes
    /// through unchanged.
    pub fn effective_reviewers(&self) -> Vec<ReviewerSpec> {
        self.reviewers
            .iter()
            .map(|e| match e {
                ReviewerEntry::Spec(s) => s.clone(),
                ReviewerEntry::Inherit(_) => self.agents.default.clone(),
            })
            .collect()
    }

    /// Validate a config before it is stored (D7): reject shapes the engine could
    /// never dispatch on. An empty reviewer list would leave every task with no
    /// review round. `max_attempts == 0` is intentionally allowed — the engine
    /// reads it as "unlimited / no no-progress breaker" (honoring "no artificial
    /// limits"), so it is a valid setting, not an error.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.reviewers.is_empty() {
            return Err("reviewers must not be empty");
        }
        Ok(())
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
    /// Space default issue config (parsed). Always present — every space stores a
    /// concrete config that inheriting issues resolve against.
    pub default_config: IssueConfig,
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
    /// The issue's own config, or `None` to inherit the space default. The
    /// resolved effective config is computed at read time, not stored here.
    pub config: Option<IssueConfig>,
    pub worktree_folder_id: Option<i32>,
    pub base_branch: Option<String>,
    pub base_commit: Option<String>,
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
    pub kind: CriterionKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopLinkRow {
    pub id: i32,
    pub from_artifact_id: i32,
    pub to_artifact_id: i32,
    pub kind: LinkKind,
    /// For design→requirement `derives_from` edges: the requirement revision this
    /// design derived from (lineage content snapshot). `None` for other edges.
    pub source_revision_id: Option<i32>,
}

/// One criterion-level coverage edge: `task_artifact_id` claims it satisfies
/// `criterion_id` (an acceptance criterion on some requirement).
#[derive(Debug, Clone, Serialize)]
pub struct LoopCoverageRow {
    pub id: i32,
    pub task_artifact_id: i32,
    pub criterion_id: i32,
}

/// One reviewer's structured pass/fail of one criterion (§3.4). `iteration_id`
/// is the stable reviewer identity for per-criterion quorum aggregation;
/// `scope_artifact_id` is the artifact judged (a task, or the result for the
/// integration gate).
#[derive(Debug, Clone, Serialize)]
pub struct LoopCriterionCheckRow {
    pub id: i32,
    pub criterion_id: i32,
    pub iteration_id: i32,
    pub scope_artifact_id: i32,
    pub verdict: CheckVerdict,
    pub evidence: String,
}

/// Immutable gate-decision audit row: the aggregated `outcome` of a gate over
/// `target_artifact_id` at `(stage, attempt)`, plus the check ids it aggregated.
#[derive(Debug, Clone, Serialize)]
pub struct LoopGateDecisionRow {
    pub id: i32,
    pub target_artifact_id: i32,
    pub stage: String,
    pub attempt: i32,
    pub outcome: GateOutcome,
    pub input_check_ids: Vec<i32>,
    pub created_at: String,
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
    /// Criterion-level coverage edges across this issue (task → criterion).
    pub coverage: Vec<LoopCoverageRow>,
    /// Per-criterion review checks across this issue (the trace matrix).
    pub criterion_checks: Vec<LoopCriterionCheckRow>,
    /// Immutable gate decisions across this issue (task review + integration).
    pub gate_decisions: Vec<LoopGateDecisionRow>,
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
    pub summary: Option<String>,
    pub content: String,
    pub trust_tier: TrustTier,
    pub status: MemoryStatus,
    pub superseded_by: Option<i32>,
    pub source_issue_id: Option<i32>,
    pub source_artifact_id: Option<i32>,
    pub produced_by_iteration_id: Option<i32>,
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

    /// A per-stage agent spec with no mode/config override.
    fn bare(agent: AgentType) -> AgentSpec {
        AgentSpec {
            agent,
            mode_id: None,
            config_values: BTreeMap::new(),
        }
    }

    /// A `StageAgents` with the given default and no single-stage overrides.
    fn stage_agents(default: AgentSpec) -> StageAgents {
        StageAgents {
            default,
            triage: None,
            refine: None,
            design: None,
            plan: None,
            implement: None,
            finalize: None,
        }
    }

    #[test]
    fn effective_reviewers_prefers_explicit_list() {
        let cfg = IssueConfig {
            reviewers: vec![ReviewerEntry::Spec(ReviewerSpec {
                agent: AgentType::Gemini,
                mode_id: Some("auto".to_string()),
                config_values: BTreeMap::new(),
            })],
            ..IssueConfig::default()
        };
        let r = cfg.effective_reviewers();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].agent, AgentType::Gemini);
        assert_eq!(r[0].mode_id.as_deref(), Some("auto"));
    }

    #[test]
    fn effective_reviewers_resolves_inherit_to_default() {
        // An inherit entry resolves to `agents.default` (carrying its
        // mode/config); concrete entries pass through unchanged.
        let cfg = IssueConfig {
            agents: stage_agents(AgentSpec {
                agent: AgentType::Codex,
                mode_id: Some("auto".to_string()),
                config_values: BTreeMap::new(),
            }),
            reviewers: vec![
                ReviewerEntry::Inherit(ReviewerInherit { inherit: true }),
                ReviewerEntry::Spec(bare(AgentType::Gemini)),
            ],
            ..IssueConfig::default()
        };
        let r = cfg.effective_reviewers();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].agent, AgentType::Codex); // inherit → default
        assert_eq!(r[0].mode_id.as_deref(), Some("auto"));
        assert_eq!(r[1].agent, AgentType::Gemini); // concrete passthrough
    }

    #[test]
    fn reviewer_entry_parses_object_and_inherit_forms() {
        // A full object and the inherit marker parse; the inherit marker
        // round-trips as `{"inherit":true}`. Bare strings no longer parse.
        let json = r#"[{"agent":"gemini","mode_id":"auto"},{"inherit":true}]"#;
        let entries: Vec<ReviewerEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            &entries[0],
            ReviewerEntry::Spec(s)
                if s.agent == AgentType::Gemini && s.mode_id.as_deref() == Some("auto")
        ));
        assert!(matches!(&entries[1], ReviewerEntry::Inherit(_)));
        assert_eq!(
            serde_json::to_string(&entries[1]).unwrap(),
            r#"{"inherit":true}"#
        );
        // A bare agent string is rejected.
        assert!(serde_json::from_str::<ReviewerEntry>(r#""codex""#).is_err());
    }

    #[test]
    fn issue_config_round_trips_clean_no_v_no_count() {
        let cfg = IssueConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("\"v\""), "no version tag: {json}");
        assert!(!json.contains("reviewer_count"), "no reviewer_count: {json}");
        let back: IssueConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.reviewers.len(), 1);
        assert_eq!(back.agents.default.agent, AgentType::ClaudeCode);
        assert_eq!(back.review_pass_rule, ReviewPassRule::Unanimous);
    }

    #[test]
    fn agents_object_form_only() {
        // Bare strings are no longer accepted for stage agents.
        let bad = r#"{"default":"codex"}"#;
        assert!(serde_json::from_str::<StageAgents>(bad).is_err());
        let ok = r#"{"default":{"agent":"codex"},"implement":{"agent":"gemini","mode_id":"auto"}}"#;
        let a: StageAgents = serde_json::from_str(ok).unwrap();
        assert_eq!(a.for_stage(Stage::Implement).agent, AgentType::Gemini);
        assert_eq!(a.for_stage(Stage::Implement).mode_id.as_deref(), Some("auto"));
        assert_eq!(a.for_stage(Stage::Plan).agent, AgentType::Codex); // falls back to default
        assert_eq!(a.for_stage(Stage::Review).agent, AgentType::Codex); // review → default
    }

    #[test]
    fn validate_rejects_empty_reviewers() {
        let cfg = IssueConfig {
            reviewers: vec![],
            ..IssueConfig::default()
        };
        assert!(cfg.validate().is_err());
        assert!(IssueConfig::default().validate().is_ok());
    }

    #[test]
    fn agent_spec_parses_full_object_and_round_trips() {
        let mut cv = BTreeMap::new();
        cv.insert("reasoning".to_string(), "high".to_string());
        let spec = AgentSpec {
            agent: AgentType::Gemini,
            mode_id: Some("plan".into()),
            config_values: cv,
        };
        let json = serde_json::to_string(&spec).unwrap();
        let back: AgentSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, spec);
        // Empty extras serialize to the minimal object form.
        let bare = AgentSpec {
            agent: AgentType::Codex,
            mode_id: None,
            config_values: BTreeMap::new(),
        };
        assert_eq!(serde_json::to_string(&bare).unwrap(), r#"{"agent":"codex"}"#);
    }
}
