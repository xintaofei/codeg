//! Delegation settings persistence + Tauri/HTTP command surface.
//!
//! Two knobs survive across restarts:
//!   * `delegation.enabled` — feature kill switch (default false)
//!   * `delegation.depth_limit` — max chain depth a child is allowed to sit at
//!
//! On startup `apply_persisted_config` reads both keys from `app_metadata`
//! and pushes them into the live `DelegationBroker`. On UI save,
//! `set_delegation_settings_core` writes the two keys and immediately
//! re-applies — the broker has no concept of "pending config", it just
//! owns the current `DelegationConfig`. The previously-persisted
//! `delegation.default_timeout_seconds` key is ignored on read (the broker
//! no longer applies a timeout; cancellation flows through MCP
//! `notifications/cancelled` instead).

use std::collections::BTreeMap;
use std::path::PathBuf;
#[cfg(any(test, feature = "tauri-runtime"))]
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::acp::delegation::broker::{DelegationBroker, DelegationConfig};
use crate::acp::delegation::types::AgentDelegationDefaults;
use crate::app_error::AppCommandError;
use crate::db::service::app_metadata_service;
use crate::models::AgentType;

pub const KEY_DELEGATION_ENABLED: &str = "delegation.enabled";
pub const KEY_DELEGATION_DEPTH: &str = "delegation.depth_limit";
/// Single JSON-serialized key for the per-agent delegation overrides.
/// Stored as one blob (rather than one row per agent×option) because the
/// option set is dynamic and per-agent — flat keys can't enumerate it.
pub const KEY_DELEGATION_AGENT_DEFAULTS: &str = "delegation.agent_defaults";

pub const DEPTH_MIN: u32 = 1;
pub const DEPTH_MAX: u32 = 8;

/// Newtype so the Tauri managed-state lookup can distinguish the delegation
/// UDS path from other `PathBuf`s in the state graph.
#[derive(Clone)]
pub struct DelegationSocketPath(pub PathBuf);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationSettings {
    pub enabled: bool,
    pub depth_limit: u32,
    /// Per-agent default overrides applied by the delegation broker when
    /// codeg-mcp spawns a subagent. Empty map → no overrides anywhere,
    /// which is the pre-existing behavior.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_defaults: BTreeMap<AgentType, AgentDelegationDefaults>,
}

impl Default for DelegationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            depth_limit: 1,
            agent_defaults: BTreeMap::new(),
        }
    }
}

impl DelegationSettings {
    fn clamped(self) -> Self {
        Self {
            enabled: self.enabled,
            depth_limit: self.depth_limit.clamp(DEPTH_MIN, DEPTH_MAX),
            agent_defaults: self
                .agent_defaults
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .collect(),
        }
    }

    fn into_broker_config(self) -> DelegationConfig {
        DelegationConfig {
            enabled: self.enabled,
            depth_limit: self.depth_limit,
            agent_defaults: self.agent_defaults,
        }
    }
}

/// Read all persisted keys from `app_metadata`, falling back to defaults
/// for any missing or malformed value. Never errors hard — corrupt
/// persistence is treated as "no preference yet."
pub async fn load_delegation_settings(conn: &DatabaseConnection) -> DelegationSettings {
    let mut settings = DelegationSettings::default();
    if let Ok(Some(raw)) = app_metadata_service::get_value(conn, KEY_DELEGATION_ENABLED).await {
        if let Ok(v) = raw.parse::<bool>() {
            settings.enabled = v;
        }
    }
    if let Ok(Some(raw)) = app_metadata_service::get_value(conn, KEY_DELEGATION_DEPTH).await {
        if let Ok(v) = raw.parse::<u32>() {
            settings.depth_limit = v;
        }
    }
    if let Ok(Some(raw)) =
        app_metadata_service::get_value(conn, KEY_DELEGATION_AGENT_DEFAULTS).await
    {
        // Corrupt JSON → keep defaults (empty map). Matches the "never errors
        // hard" contract on the other two keys above.
        if let Ok(parsed) =
            serde_json::from_str::<BTreeMap<AgentType, AgentDelegationDefaults>>(&raw)
        {
            settings.agent_defaults = parsed;
        }
    }
    settings.clamped()
}

/// Pull settings from the DB and push the resulting `DelegationConfig` onto
/// the broker. Idempotent — safe to call on startup, after settings save, or
/// after any external write to `app_metadata`.
pub async fn apply_persisted_config(conn: &DatabaseConnection, broker: &DelegationBroker) {
    let settings = load_delegation_settings(conn).await;
    broker.set_config(settings.into_broker_config()).await;
}

/// Persist + apply. Used by both the Tauri command and the HTTP handler so
/// the clamp / re-apply chain is in exactly one place.
pub async fn set_delegation_settings_core(
    conn: &DatabaseConnection,
    broker: &DelegationBroker,
    desired: DelegationSettings,
) -> Result<DelegationSettings, AppCommandError> {
    let clamped = desired.clamped();
    app_metadata_service::upsert_value(conn, KEY_DELEGATION_ENABLED, &clamped.enabled.to_string())
        .await
        .map_err(AppCommandError::from)?;
    app_metadata_service::upsert_value(
        conn,
        KEY_DELEGATION_DEPTH,
        &clamped.depth_limit.to_string(),
    )
    .await
    .map_err(AppCommandError::from)?;
    // Whole-blob replace semantics: save mirrors what the UI sent. Empty map
    // serializes to "{}" — still write it so a user can clear all overrides
    // back to the agent defaults.
    let agent_defaults_json = serde_json::to_string(&clamped.agent_defaults).map_err(|e| {
        AppCommandError::configuration_invalid(format!("serialize agent_defaults: {e}"))
    })?;
    app_metadata_service::upsert_value(
        conn,
        KEY_DELEGATION_AGENT_DEFAULTS,
        &agent_defaults_json,
    )
    .await
    .map_err(AppCommandError::from)?;
    broker
        .set_config(clamped.clone().into_broker_config())
        .await;
    Ok(clamped)
}

// -------- Tauri commands -----------------------------------------------------

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_delegation_settings(
    #[cfg(feature = "tauri-runtime")] db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<DelegationSettings, AppCommandError> {
    #[cfg(feature = "tauri-runtime")]
    {
        Ok(load_delegation_settings(&db.conn).await)
    }
    #[cfg(not(feature = "tauri-runtime"))]
    {
        // Server mode reaches this via the web handler, not this command.
        Err(AppCommandError::configuration_invalid("tauri-only command"))
    }
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn set_delegation_settings(
    #[cfg(feature = "tauri-runtime")] db: tauri::State<'_, crate::db::AppDatabase>,
    #[cfg(feature = "tauri-runtime")] broker: tauri::State<'_, Arc<DelegationBroker>>,
    settings: DelegationSettings,
) -> Result<DelegationSettings, AppCommandError> {
    #[cfg(feature = "tauri-runtime")]
    {
        set_delegation_settings_core(&db.conn, broker.inner(), settings).await
    }
    #[cfg(not(feature = "tauri-runtime"))]
    {
        let _ = settings;
        Err(AppCommandError::configuration_invalid("tauri-only command"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::delegation::broker::{ConversationDepthLookup, DelegationBroker};
    use crate::acp::delegation::spawner::{mock::MockSpawner, ConnectionSpawner};
    use crate::acp::delegation::types::DelegationError;
    use async_trait::async_trait;

    struct EmptyLookup;
    #[async_trait]
    impl ConversationDepthLookup for EmptyLookup {
        async fn parent_of(&self, _id: i32) -> Result<Option<i32>, DelegationError> {
            Ok(None)
        }
    }

    fn make_broker() -> DelegationBroker {
        DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            Arc::new(EmptyLookup) as Arc<dyn ConversationDepthLookup>,
        )
    }

    #[test]
    fn settings_clamp_to_safe_range() {
        let s = DelegationSettings {
            enabled: true,
            depth_limit: 99,
            ..DelegationSettings::default()
        }
        .clamped();
        assert_eq!(s.depth_limit, DEPTH_MAX);
    }

    #[tokio::test]
    async fn load_returns_defaults_when_unset() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let settings = load_delegation_settings(&db.conn).await;
        assert!(!settings.enabled);
        assert_eq!(settings.depth_limit, 1);
    }

    #[tokio::test]
    async fn set_then_load_round_trip_and_broker_applied() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let broker = make_broker();
        let desired = DelegationSettings {
            enabled: false,
            depth_limit: 3,
            ..DelegationSettings::default()
        };
        let saved = set_delegation_settings_core(&db.conn, &broker, desired)
            .await
            .unwrap();
        assert!(!saved.enabled);
        assert_eq!(saved.depth_limit, 3);

        let loaded = load_delegation_settings(&db.conn).await;
        assert_eq!(loaded.enabled, saved.enabled);
        assert_eq!(loaded.depth_limit, saved.depth_limit);

        let cfg = broker.config_snapshot().await;
        assert!(!cfg.enabled);
        assert_eq!(cfg.depth_limit, 3);
    }

    #[tokio::test]
    async fn agent_defaults_round_trip_through_db_and_broker() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let broker = make_broker();

        let mut claude_cfg = BTreeMap::new();
        claude_cfg.insert("model".into(), "claude-sonnet-4-5".into());
        let mut agent_defaults: BTreeMap<AgentType, AgentDelegationDefaults> = BTreeMap::new();
        agent_defaults.insert(
            AgentType::ClaudeCode,
            AgentDelegationDefaults {
                mode_id: Some("auto".into()),
                config_values: claude_cfg.clone(),
            },
        );

        let desired = DelegationSettings {
            enabled: true,
            depth_limit: 4,
            agent_defaults: agent_defaults.clone(),
        };
        let saved = set_delegation_settings_core(&db.conn, &broker, desired)
            .await
            .unwrap();
        assert_eq!(saved.agent_defaults, agent_defaults);

        // Re-read from DB — the JSON blob should round-trip identically.
        let loaded = load_delegation_settings(&db.conn).await;
        assert_eq!(loaded.agent_defaults, agent_defaults);

        // Broker should have the same map applied.
        let cfg = broker.config_snapshot().await;
        let entry = cfg.agent_defaults.get(&AgentType::ClaudeCode).unwrap();
        assert_eq!(entry.mode_id.as_deref(), Some("auto"));
        assert_eq!(entry.config_values, claude_cfg);
    }

    #[tokio::test]
    async fn clamped_drops_empty_agent_defaults_entries() {
        // Empty entries (no mode, no config_values) should be filtered out so
        // the persisted JSON stays compact.
        let mut agent_defaults: BTreeMap<AgentType, AgentDelegationDefaults> = BTreeMap::new();
        agent_defaults.insert(AgentType::ClaudeCode, AgentDelegationDefaults::default());
        agent_defaults.insert(
            AgentType::Codex,
            AgentDelegationDefaults {
                mode_id: Some("auto".into()),
                config_values: BTreeMap::new(),
            },
        );
        let s = DelegationSettings {
            enabled: true,
            depth_limit: 2,
            agent_defaults,
        }
        .clamped();
        assert!(!s.agent_defaults.contains_key(&AgentType::ClaudeCode));
        assert!(s.agent_defaults.contains_key(&AgentType::Codex));
    }

    #[tokio::test]
    async fn set_clamps_out_of_range_values() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let broker = make_broker();
        let saved = set_delegation_settings_core(
            &db.conn,
            &broker,
            DelegationSettings {
                enabled: true,
                depth_limit: 999,
                ..DelegationSettings::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(saved.depth_limit, DEPTH_MAX);
    }
}
