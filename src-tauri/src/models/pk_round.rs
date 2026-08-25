use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One contestant entry in the round config. Supports both a plain string
/// (backward compat with old rounds: `"claude_code"`) and a labeled object
/// (new format: `{"agent":"claude_code","label":"Sonnet", ...}`).
/// `config_values` is applied to
/// this slot's ACP session before its first prompt; `label` is the captured
/// human-readable value used to disambiguate same-agent slots.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PkContestantEntry {
    Simple(String),
    Labeled {
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        config_values: BTreeMap<String, String>,
    },
}

impl PkContestantEntry {
    pub fn agent(&self) -> &str {
        match self {
            PkContestantEntry::Simple(a) => a,
            PkContestantEntry::Labeled { agent, .. } => agent,
        }
    }
    pub fn label(&self) -> Option<&str> {
        match self {
            PkContestantEntry::Simple(_) => None,
            PkContestantEntry::Labeled { label, .. } => label.as_deref(),
        }
    }
}

/// Config stored as JSON in `pk_round.config`. Mirrors the launcher's options
/// so a round is fully reproducible from the DB row alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkRoundConfig {
    /// The agent types selected as contestants, in pick order. Each entry
    /// is either a plain string (old format) or a labeled object (new format).
    pub agents: Vec<PkContestantEntry>,
    /// Round-level permission policy applied to every contestant.
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    /// Bare mode: contestants are instructed to use no skills at all.
    #[serde(default)]
    pub bare_mode: bool,
    /// Uniform reasoning-effort request applied to every contestant.
    #[serde(default = "default_effort")]
    pub effort: String,
    /// Optional judge agent — after all contestants finish, this agent reads
    /// every diff and produces a structured verdict. Stored in config (not a
    /// separate column) because it is round-level input, set at creation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_agent: Option<String>,
    /// Optional custom judge evaluation dimensions. Each entry is a free-form
    /// line that replaces the default 4 (Correctness / Code quality /
    /// Completeness / Efficiency). Empty or absent = use the defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub judge_dimensions: Vec<String>,
    /// The git ref each contestant worktree is branched from. Absent or null
    /// = current HEAD (the default, "from now"). When the launcher picks a
    /// commit X as the task source, this is set to `X^` so the worktree starts
    /// one commit BEFORE X — contestants never see X's changes, only its
    /// message as the task description. Physical isolation, not a prompt
    /// instruction the agent can ignore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
}

fn default_permission_mode() -> String {
    "default".into()
}

fn default_effort() -> String {
    "default".into()
}

/// A PK round summary as returned to the frontend. Carries the round's own
/// fields plus the live contestant status (computed from the linked
/// conversations, not stored on the round row itself).
#[derive(Debug, Clone, Serialize)]
pub struct PkRoundInfo {
    pub id: i32,
    pub folder_id: i32,
    pub task: String,
    pub config: PkRoundConfig,
    pub status: String,
    pub failure_reason: Option<String>,
    /// JSON-serialized judge verdict, or null if no judge / not yet run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_result: Option<serde_json::Value>,
    /// idle | running | done | error | skipped
    pub judge_status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[cfg(test)]
mod tests {
    use super::PkContestantEntry;

    #[test]
    fn contestant_entry_keeps_legacy_formats_compatible() {
        let simple: PkContestantEntry = serde_json::from_str(r#""codex""#).unwrap();
        assert_eq!(simple.agent(), "codex");
        assert_eq!(simple.label(), None);

        let labeled: PkContestantEntry =
            serde_json::from_str(r#"{"agent":"claude_code","label":"Sonnet"}"#).unwrap();
        assert_eq!(labeled.agent(), "claude_code");
        assert_eq!(labeled.label(), Some("Sonnet"));
    }

    #[test]
    fn contestant_entry_round_trips_pinned_config_values() {
        let entry: PkContestantEntry = serde_json::from_str(
            r#"{"agent":"claude_code","label":"Opus","config_values":{"model":"opus"}}"#,
        )
        .unwrap();
        let json = serde_json::to_value(entry).unwrap();
        assert_eq!(json["agent"], "claude_code");
        assert_eq!(json["label"], "Opus");
        assert_eq!(json["config_values"]["model"], "opus");
    }
}
