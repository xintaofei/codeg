use std::path::Path;

use crate::acp::manager::ConnectionManager;
use crate::acp::types::ConfigStaleKind;
use crate::app_error::AppCommandError;
use crate::commands::{acp, cc_switch_model_provider_import};
use crate::db::service::{agent_setting_service, model_provider_service};
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::model_provider::{
    CcSwitchModelProviderPreviewItem, ImportCcSwitchModelProvidersRequest,
    ImportCcSwitchModelProvidersResult, ListImportableCcSwitchModelProvidersResult,
    ModelProviderInfo,
};
use crate::web::event_bridge::EventEmitter;

// ---------------------------------------------------------------------------
// Shared core functions (used by both Tauri commands and web handlers)
// ---------------------------------------------------------------------------

fn validate_agent_type(agent_type: &str) -> Result<(), AppCommandError> {
    if agent_type.trim().is_empty() {
        return Err(AppCommandError::invalid_input("Agent type is required"));
    }
    let _: AgentType = serde_json::from_value(serde_json::Value::String(agent_type.to_string()))
        .map_err(|_| AppCommandError::invalid_input(format!("Invalid agent type: {agent_type}")))?;
    Ok(())
}

pub(crate) fn validate_fields(
    name: Option<&str>,
    api_url: Option<&str>,
    api_key: Option<&str>,
) -> Result<(), AppCommandError> {
    if let Some(n) = name {
        if n.len() > 256 {
            return Err(AppCommandError::invalid_input(
                "Name must be 256 characters or less",
            ));
        }
    }
    if let Some(u) = api_url {
        if u.len() > 2048 {
            return Err(AppCommandError::invalid_input(
                "API URL must be 2048 characters or less",
            ));
        }
        if !u.starts_with("http://") && !u.starts_with("https://") {
            return Err(AppCommandError::invalid_input(
                "API URL must start with http:// or https://",
            ));
        }
    }
    if let Some(k) = api_key {
        if k.len() > 4096 {
            return Err(AppCommandError::invalid_input(
                "API Key must be 4096 characters or less",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_model(agent_type: &str, model: Option<&str>) -> Result<(), AppCommandError> {
    let Some(raw) = model.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    if raw.len() > 4096 {
        return Err(AppCommandError::invalid_input(
            "Model must be 4096 characters or less",
        ));
    }
    // ClaudeCode requires a JSON object; other agents accept a plain string.
    if agent_type == "claude_code" {
        let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
            AppCommandError::invalid_input(format!("Invalid Claude model JSON: {e}"))
        })?;
        if !value.is_object() {
            return Err(AppCommandError::invalid_input(
                "Claude model must be a JSON object",
            ));
        }
    }
    Ok(())
}

pub async fn list_model_providers_core(
    db: &AppDatabase,
) -> Result<Vec<ModelProviderInfo>, AppCommandError> {
    let rows = model_provider_service::list_all(&db.conn)
        .await
        .map_err(AppCommandError::from)?;
    Ok(rows.into_iter().map(ModelProviderInfo::from).collect())
}

pub async fn create_model_provider_core(
    db: &AppDatabase,
    name: String,
    api_url: String,
    api_key: String,
    agent_type: String,
    model: Option<String>,
) -> Result<ModelProviderInfo, AppCommandError> {
    validate_fields(Some(&name), Some(&api_url), Some(&api_key))?;
    validate_agent_type(&agent_type)?;
    validate_model(&agent_type, model.as_deref())?;

    let model_row =
        model_provider_service::create(&db.conn, name, api_url, api_key, agent_type, model)
            .await
            .map_err(AppCommandError::from)?;
    Ok(ModelProviderInfo::from(model_row))
}

async fn list_importable_cc_switch_model_providers_from_path_core(
    db: &AppDatabase,
    db_path: &Path,
) -> Result<ListImportableCcSwitchModelProvidersResult, AppCommandError> {
    if !db_path.is_file() {
        return Ok(ListImportableCcSwitchModelProvidersResult {
            available: false,
            source_path: db_path.display().to_string(),
            items: Vec::new(),
        });
    }

    let existing = model_provider_service::list_all(&db.conn)
        .await
        .map_err(AppCommandError::from)?;
    let items =
        cc_switch_model_provider_import::load_cc_switch_preview_items(db_path, &existing).await?;

    Ok(ListImportableCcSwitchModelProvidersResult {
        available: true,
        source_path: db_path.display().to_string(),
        items,
    })
}

pub async fn list_importable_cc_switch_model_providers_core(
    db: &AppDatabase,
) -> Result<ListImportableCcSwitchModelProvidersResult, AppCommandError> {
    let db_path = cc_switch_model_provider_import::resolve_cc_switch_db_path();
    list_importable_cc_switch_model_providers_from_path_core(db, &db_path).await
}

async fn import_cc_switch_model_providers_from_path_core(
    db: &AppDatabase,
    db_path: &Path,
    request: ImportCcSwitchModelProvidersRequest,
) -> Result<ImportCcSwitchModelProvidersResult, AppCommandError> {
    let preview = list_importable_cc_switch_model_providers_from_path_core(db, db_path).await?;
    if !preview.available {
        return Ok(ImportCcSwitchModelProvidersResult {
            imported_ids: Vec::new(),
            skipped: Vec::new(),
        });
    }
    let wanted: std::collections::HashSet<&str> =
        request.source_ids.iter().map(String::as_str).collect();
    let mut skipped: Vec<CcSwitchModelProviderPreviewItem> = preview
        .items
        .iter()
        .filter(|item| {
            wanted.contains(item.source_id.as_str())
                && !item.importable
                && !(request.overwrite_same_name
                    && item.skip_reason
                        == Some(crate::models::model_provider::CcSwitchModelProviderSkipReason::DuplicateName))
        })
        .cloned()
        .collect();

    let candidates = cc_switch_model_provider_import::load_cc_switch_import_candidates(
        db_path,
        &model_provider_service::list_all(&db.conn)
            .await
            .map_err(AppCommandError::from)?,
    )
    .await?;

    let mut imported_ids = Vec::new();
    for candidate in candidates
        .into_iter()
        .filter(|candidate| wanted.contains(candidate.source_id.as_str()))
    {
        let current_existing = model_provider_service::list_all(&db.conn)
            .await
            .map_err(AppCommandError::from)?;
        if let Some(reason) = cc_switch_model_provider_import::duplicate_reason_for_candidate(
            &candidate,
            &current_existing,
        ) {
            if request.overwrite_same_name
                && reason
                    == crate::models::model_provider::CcSwitchModelProviderSkipReason::DuplicateName
            {
                if let Some(existing) = current_existing
                    .iter()
                    .find(|provider| provider.name == candidate.name)
                {
                    if existing.agent_type == candidate.target_agent_type {
                        let updated = update_model_provider_core(
                            db,
                            existing.id,
                            None,
                            Some(candidate.api_url.clone()),
                            Some(candidate.api_key.clone()),
                            None,
                            Some(candidate.model.clone().unwrap_or_default()),
                            &EventEmitter::Noop,
                        )
                        .await?;
                        imported_ids.push(updated.id);
                        continue;
                    }
                }
            }

            skipped.push(CcSwitchModelProviderPreviewItem {
                source_id: candidate.source_id.clone(),
                source_app_type: candidate.source_app_type.clone(),
                target_agent_type: candidate.target_agent_type.clone(),
                name: candidate.name.clone(),
                api_url: Some(candidate.api_url.clone()),
                model: candidate.model.clone(),
                importable: false,
                skip_reason: Some(reason),
            });
            continue;
        }

        let created = create_model_provider_core(
            db,
            candidate.name,
            candidate.api_url,
            candidate.api_key,
            candidate.target_agent_type,
            candidate.model,
        )
        .await?;
        imported_ids.push(created.id);
    }

    Ok(ImportCcSwitchModelProvidersResult {
        imported_ids: imported_ids,
        skipped,
    })
}

pub async fn import_cc_switch_model_providers_core(
    db: &AppDatabase,
    request: ImportCcSwitchModelProvidersRequest,
) -> Result<ImportCcSwitchModelProvidersResult, AppCommandError> {
    let db_path = cc_switch_model_provider_import::resolve_cc_switch_db_path();
    import_cc_switch_model_providers_from_path_core(db, &db_path, request).await
}

/// Update a model provider. For the `model` parameter:
/// - `None` (omitted) means "don't change"
/// - `Some("")` means "clear"
/// - `Some(value)` means "set to value"
#[allow(clippy::too_many_arguments)]
pub async fn update_model_provider_core(
    db: &AppDatabase,
    id: i32,
    name: Option<String>,
    api_url: Option<String>,
    api_key: Option<String>,
    agent_type: Option<String>,
    model: Option<String>,
    emitter: &EventEmitter,
) -> Result<ModelProviderInfo, AppCommandError> {
    validate_fields(name.as_deref(), api_url.as_deref(), api_key.as_deref())?;
    if let Some(ref at) = agent_type {
        validate_agent_type(at)?;
    }

    // Fetch old provider to detect changes and to determine effective agent_type for model validation.
    let old_provider = model_provider_service::get_by_id(&db.conn, id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found(format!("model provider not found: {id}")))?;

    // agent_type is immutable after creation: dependent agents bind to a provider by id
    // and rely on provider.agent_type matching their own. Changing it would silently
    // mis-parse provider.model (e.g. Claude JSON written into Codex's config.toml).
    if let Some(ref new_at) = agent_type {
        if new_at != &old_provider.agent_type {
            return Err(AppCommandError::invalid_input(format!(
                "agent_type is immutable after creation (current: {}, requested: {new_at})",
                old_provider.agent_type
            )));
        }
    }

    let effective_agent_type = old_provider.agent_type.as_str();
    if let Some(ref raw) = model {
        validate_model(effective_agent_type, Some(raw))?;
    }

    // Translate Some("") to Some(None) (clear), Some(value) to Some(Some(value)), None to None.
    let model_patch: Option<Option<String>> = model.as_ref().map(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    let model_row = model_provider_service::update(
        &db.conn,
        id,
        name,
        api_url.clone(),
        api_key.clone(),
        None, // agent_type is immutable; rejected above if differs
        model_patch.clone(),
    )
    .await
    .map_err(AppCommandError::from)?;

    // Cascade credential/model changes to all dependent agent settings and config files.
    let url_changed = api_url
        .as_deref()
        .is_some_and(|u| u != old_provider.api_url);
    let key_changed = api_key
        .as_deref()
        .is_some_and(|k| k != old_provider.api_key);
    let model_changed = model_patch
        .as_ref()
        .is_some_and(|new_value| new_value.as_deref() != old_provider.model.as_deref());

    if url_changed || key_changed || model_changed {
        let final_url = api_url.as_deref().unwrap_or(&old_provider.api_url);
        let final_key = api_key.as_deref().unwrap_or(&old_provider.api_key);
        let final_model_owned: Option<String> = match &model_patch {
            Some(inner) => inner.clone(),
            None => old_provider.model.clone(),
        };
        acp::cascade_update_model_provider(
            db,
            id,
            final_url,
            final_key,
            final_model_owned.as_deref(),
            emitter,
        )
        .await
        .map_err(|e| AppCommandError::invalid_input(e.to_string()))?;
    }

    Ok(ModelProviderInfo::from(model_row))
}

/// Result of `update_model_provider`: the updated provider row plus how many
/// running sessions the cascade left on stale (launch-time) config — for the
/// settings-side "N sessions need restart" toast.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateModelProviderResult {
    pub provider: ModelProviderInfo,
    pub affected_running_sessions: usize,
}

/// `update_model_provider_core` followed by a staleness refresh for every agent
/// bound to this provider. Shared by the Tauri command and the web handler so
/// both surface how many running sessions need a restart to pick up the new
/// credentials/model. If the save didn't actually change url/key/model, the
/// cascade is skipped, fingerprints are unchanged, and the refresh is a silent
/// no-op returning 0.
#[allow(clippy::too_many_arguments)]
pub async fn update_model_provider_and_refresh(
    db: &AppDatabase,
    manager: &ConnectionManager,
    data_dir: &Path,
    id: i32,
    name: Option<String>,
    api_url: Option<String>,
    api_key: Option<String>,
    agent_type: Option<String>,
    model: Option<String>,
    emitter: &EventEmitter,
) -> Result<UpdateModelProviderResult, AppCommandError> {
    let provider =
        update_model_provider_core(db, id, name, api_url, api_key, agent_type, model, emitter)
            .await?;

    // Every agent bound to this provider may now be on stale config (the cascade
    // rewrote their env_json + native config files). Recompute and notify.
    let agent_types: Vec<AgentType> =
        agent_setting_service::find_by_model_provider_id(&db.conn, id)
            .await
            .unwrap_or_default()
            .iter()
            .filter_map(|setting| serde_json::from_str(&setting.agent_type).ok())
            .collect();
    let affected_running_sessions = acp::refresh_config_staleness(
        manager,
        db,
        data_dir,
        &agent_types,
        ConfigStaleKind::ModelProvider,
    )
    .await;

    Ok(UpdateModelProviderResult {
        provider,
        affected_running_sessions,
    })
}

pub async fn delete_model_provider_core(db: &AppDatabase, id: i32) -> Result<(), AppCommandError> {
    // Check if any agent settings reference this provider.
    let dependents = agent_setting_service::find_by_model_provider_id(&db.conn, id)
        .await
        .map_err(AppCommandError::from)?;

    if !dependents.is_empty() {
        let agent_names: Vec<String> = dependents
            .iter()
            .filter_map(|row| {
                serde_json::from_str::<AgentType>(&row.agent_type)
                    .ok()
                    .map(|at| at.to_string())
            })
            .collect();
        let names_joined = agent_names.join(", ");
        return Err(AppCommandError::invalid_input(format!(
            "PROVIDER_IN_USE:{names_joined}"
        )));
    }

    model_provider_service::delete(&db.conn, id)
        .await
        .map_err(AppCommandError::from)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn list_model_providers(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<ModelProviderInfo>, AppCommandError> {
    list_model_providers_core(&db).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn create_model_provider(
    db: tauri::State<'_, AppDatabase>,
    name: String,
    api_url: String,
    api_key: String,
    agent_type: String,
    model: Option<String>,
) -> Result<ModelProviderInfo, AppCommandError> {
    create_model_provider_core(&db, name, api_url, api_key, agent_type, model).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn list_importable_cc_switch_model_providers(
    db: tauri::State<'_, AppDatabase>,
) -> Result<ListImportableCcSwitchModelProvidersResult, AppCommandError> {
    list_importable_cc_switch_model_providers_core(&db).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn import_cc_switch_model_providers(
    db: tauri::State<'_, AppDatabase>,
    request: ImportCcSwitchModelProvidersRequest,
) -> Result<ImportCcSwitchModelProvidersResult, AppCommandError> {
    import_cc_switch_model_providers_core(&db, request).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_model_provider(
    db: tauri::State<'_, AppDatabase>,
    manager: tauri::State<'_, ConnectionManager>,
    id: i32,
    name: Option<String>,
    api_url: Option<String>,
    api_key: Option<String>,
    agent_type: Option<String>,
    model: Option<String>,
    app: tauri::AppHandle,
) -> Result<UpdateModelProviderResult, AppCommandError> {
    use tauri::Manager;
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map(|p| crate::paths::resolve_effective_data_dir(&p))
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let emitter = EventEmitter::Tauri(app);
    update_model_provider_and_refresh(
        &db,
        &manager,
        &app_data_dir,
        id,
        name,
        api_url,
        api_key,
        agent_type,
        model,
        &emitter,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn delete_model_provider(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<(), AppCommandError> {
    delete_model_provider_core(&db, id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    use tempfile::tempdir;

    async fn seed_cc_switch_db(
        providers: &[(&str, &str, &str, serde_json::Value)],
    ) -> tempfile::TempDir {
        let dir = tempdir().expect("temp dir");
        let db_path = dir.path().join("cc-switch.db");
        let conn =
            sea_orm::Database::connect(format!("sqlite:{}?mode=rwc", db_path.to_string_lossy()))
                .await
                .expect("open cc-switch db");
        conn.execute(Statement::from_string(
            DbBackend::Sqlite,
            "CREATE TABLE providers (id TEXT PRIMARY KEY, app_type TEXT NOT NULL, name TEXT NOT NULL, settings_config TEXT);"
                .to_owned(),
        ))
        .await
        .expect("create providers table");

        for (id, app_type, name, settings) in providers {
            let settings_json = serde_json::to_string(settings).expect("serialize settings config");
            let escaped_name = name.replace('\'', "''");
            let escaped_settings = settings_json.replace('\'', "''");
            conn.execute(Statement::from_string(
                DbBackend::Sqlite,
                format!(
                    "INSERT INTO providers (id, app_type, name, settings_config) VALUES ('{}', '{}', '{}', '{}');",
                    id.replace('\'', "''"),
                    app_type.replace('\'', "''"),
                    escaped_name,
                    escaped_settings
                ),
            ))
            .await
            .expect("insert cc-switch provider row");
        }

        dir
    }

    /// Regression: an `api_key` containing a multibyte character (e.g. a
    /// full-width char typed under a CJK IME) must not panic the masking in
    /// `ModelProviderInfo::from`. Before the fix, `create` persisted the row
    /// and then panicked, after which every `list_model_providers` call
    /// panicked on that row — breaking the settings list and the agent
    /// management provider dropdown until the row was removed.
    #[tokio::test]
    async fn create_and_list_tolerate_multibyte_api_key() {
        let db = fresh_in_memory_db().await;

        let created = create_model_provider_core(
            &db,
            "Provider".to_string(),
            "https://api.example.com".to_string(),
            "sk-密钥abcd1234".to_string(),
            "codex".to_string(),
            None,
        )
        .await;
        assert!(
            created.is_ok(),
            "create panicked/failed: {:?}",
            created.err()
        );

        let rows = list_model_providers_core(&db)
            .await
            .expect("list must not fail on a multibyte api_key");
        assert_eq!(rows.len(), 1);
        // The raw key round-trips; only the masked view is derived.
        assert_eq!(rows[0].api_key, "sk-密钥abcd1234");
        assert!(!rows[0].api_key_masked.is_empty());
    }

    /// Regression for the model-provider staleness path: editing a provider must
    /// flag the running sessions of agents bound to it. The mechanism is "the
    /// bound agent's config fingerprint shifts" — `refresh_connection_staleness`
    /// (tested in manager.rs) then flags any session whose spawn fingerprint no
    /// longer matches. This proves the shift actually happens for a credential
    /// change, and that a non-runtime edit (display name) does NOT shift it (so
    /// provider edits don't over-flag).
    ///
    /// DB-only: we mutate the provider row directly via the service rather than
    /// `update_model_provider_core`, so the on-disk config cascade never runs and
    /// the test can't touch a developer's real agent config files. The fingerprint
    /// also reads native config files, but only ever reads them and only between
    /// DB mutations, so that component stays constant across the comparisons.
    #[tokio::test]
    async fn provider_credential_change_shifts_bound_agent_fingerprint() {
        use crate::db::entities::agent_setting;
        use crate::models::agent::AgentType;
        use sea_orm::{ActiveModelTrait, NotSet, Set};

        let db = fresh_in_memory_db().await;
        let data_dir = std::env::temp_dir();

        let provider = create_model_provider_core(
            &db,
            "Prov".to_string(),
            "https://api.example.com".to_string(),
            "sk-old-key".to_string(),
            "codex".to_string(),
            None,
        )
        .await
        .expect("create provider");

        // A Codex agent setting bound to that provider.
        let now = chrono::Utc::now();
        agent_setting::ActiveModel {
            id: NotSet,
            agent_type: Set(serde_json::to_string(&AgentType::Codex).unwrap()),
            registry_id: Set("codex".to_string()),
            enabled: Set(true),
            sort_order: Set(0),
            installed_version: Set(None),
            env_json: Set(Some("{}".to_string())),
            model_provider_id: Set(Some(provider.id)),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&db.conn)
        .await
        .expect("insert codex agent setting");

        let fp_before = acp::compute_session_config_fingerprint(&db, AgentType::Codex, &data_dir)
            .await
            .expect("fingerprint before");

        // Changing the api_key (DB-only) must shift the bound agent's fingerprint:
        // `apply_model_provider_env` injects the provider's key into the env.
        model_provider_service::update(
            &db.conn,
            provider.id,
            None,
            None,
            Some("sk-new-key".to_string()),
            None,
            None,
        )
        .await
        .expect("update provider key");

        let fp_after_key =
            acp::compute_session_config_fingerprint(&db, AgentType::Codex, &data_dir)
                .await
                .expect("fingerprint after key change");
        assert_ne!(
            fp_before, fp_after_key,
            "changing the bound provider's api_key must shift the agent fingerprint"
        );

        // A non-runtime change (display name only) must NOT shift it.
        model_provider_service::update(
            &db.conn,
            provider.id,
            Some("Renamed".to_string()),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("rename provider");

        let fp_after_name =
            acp::compute_session_config_fingerprint(&db, AgentType::Codex, &data_dir)
                .await
                .expect("fingerprint after rename");
        assert_eq!(
            fp_after_key, fp_after_name,
            "renaming the provider must not shift the agent fingerprint"
        );
    }

    #[tokio::test]
    async fn list_importable_cc_switch_model_providers_reports_missing_db_as_unavailable() {
        let db = fresh_in_memory_db().await;
        let dir = tempdir().expect("temp dir");
        let db_path = dir.path().join("missing.db");

        let result = list_importable_cc_switch_model_providers_from_path_core(&db, &db_path)
            .await
            .expect("list preview");

        assert!(!result.available);
        assert!(result.items.is_empty());
        assert_eq!(result.source_path, db_path.display().to_string());
    }

    #[tokio::test]
    async fn import_cc_switch_model_providers_imports_only_selected_rows() {
        let db = fresh_in_memory_db().await;
        let cc_switch_dir = seed_cc_switch_db(&[
            (
                "codex-1",
                "codex",
                "Codex One",
                serde_json::json!({
                    "auth": { "OPENAI_API_KEY": "sk-codex-1" },
                    "config": "model = \"gpt-5\"\nbase_url = \"https://api.codex.one/v1\"\n"
                }),
            ),
            (
                "gemini-1",
                "gemini",
                "Gemini One",
                serde_json::json!({
                    "env": {
                        "GOOGLE_GEMINI_BASE_URL": "https://api.gemini.one",
                        "GEMINI_API_KEY": "gm-key-1",
                        "GEMINI_MODEL": "gemini-3-pro"
                    }
                }),
            ),
        ])
        .await;
        let db_path = cc_switch_dir.path().join("cc-switch.db");

        let result = import_cc_switch_model_providers_from_path_core(
            &db,
            &db_path,
            ImportCcSwitchModelProvidersRequest {
                source_ids: vec!["codex:codex-1".to_string()],
                overwrite_same_name: false,
            },
        )
        .await
        .expect("import selected rows");

        assert_eq!(result.imported_ids.len(), 1);
        assert!(result.skipped.is_empty());

        let providers = list_model_providers_core(&db)
            .await
            .expect("list imported providers");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "Codex One");
        assert_eq!(providers[0].agent_type, "codex");
        assert_eq!(providers[0].model.as_deref(), Some("gpt-5"));
    }

    #[tokio::test]
    async fn import_cc_switch_model_providers_rechecks_duplicates_between_selected_rows() {
        let db = fresh_in_memory_db().await;
        let cc_switch_dir = seed_cc_switch_db(&[
            (
                "codex-1",
                "codex",
                "Shared Name",
                serde_json::json!({
                    "auth": { "OPENAI_API_KEY": "sk-codex-1" },
                    "config": "model = \"gpt-5\"\nbase_url = \"https://api.first.example/v1\"\n"
                }),
            ),
            (
                "gemini-1",
                "gemini",
                "Shared Name",
                serde_json::json!({
                    "env": {
                        "GOOGLE_GEMINI_BASE_URL": "https://api.second.example",
                        "GEMINI_API_KEY": "gm-key-1",
                        "GEMINI_MODEL": "gemini-3-pro"
                    }
                }),
            ),
        ])
        .await;
        let db_path = cc_switch_dir.path().join("cc-switch.db");

        let result = import_cc_switch_model_providers_from_path_core(
            &db,
            &db_path,
            ImportCcSwitchModelProvidersRequest {
                source_ids: vec!["codex:codex-1".to_string(), "gemini:gemini-1".to_string()],
                overwrite_same_name: false,
            },
        )
        .await
        .expect("import rows with internal duplicate");

        assert_eq!(result.imported_ids.len(), 1);
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(
            result.skipped[0].skip_reason,
            Some(crate::models::model_provider::CcSwitchModelProviderSkipReason::DuplicateName)
        );

        let providers = list_model_providers_core(&db)
            .await
            .expect("list imported providers");
        assert_eq!(providers.len(), 1);
    }

    #[tokio::test]
    async fn import_cc_switch_model_providers_overwrites_same_name_same_agent_type() {
        let db = fresh_in_memory_db().await;
        let existing = create_model_provider_core(
            &db,
            "Shared Name".to_string(),
            "https://old.example.com/v1".to_string(),
            "sk-old".to_string(),
            "codex".to_string(),
            Some("gpt-4.1".to_string()),
        )
        .await
        .expect("create existing provider");

        let cc_switch_dir = seed_cc_switch_db(&[(
            "codex-1",
            "codex",
            "Shared Name",
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-new" },
                "config": "model = \"gpt-5\"\nbase_url = \"https://new.example.com/v1\"\n"
            }),
        )])
        .await;
        let db_path = cc_switch_dir.path().join("cc-switch.db");

        let result = import_cc_switch_model_providers_from_path_core(
            &db,
            &db_path,
            ImportCcSwitchModelProvidersRequest {
                source_ids: vec!["codex:codex-1".to_string()],
                overwrite_same_name: true,
            },
        )
        .await
        .expect("overwrite same-name provider");

        assert_eq!(result.imported_ids, vec![existing.id]);
        assert!(result.skipped.is_empty());

        let providers = list_model_providers_core(&db)
            .await
            .expect("list providers after overwrite");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, existing.id);
        assert_eq!(providers[0].api_url, "https://new.example.com/v1");
        assert_eq!(providers[0].api_key, "sk-new");
        assert_eq!(providers[0].model.as_deref(), Some("gpt-5"));
    }

    #[tokio::test]
    async fn import_cc_switch_model_providers_does_not_overwrite_same_name_different_agent_type() {
        let db = fresh_in_memory_db().await;
        create_model_provider_core(
            &db,
            "Shared Name".to_string(),
            "https://existing.example.com/v1".to_string(),
            "sk-existing".to_string(),
            "gemini".to_string(),
            Some("gemini-2.5-pro".to_string()),
        )
        .await
        .expect("create existing provider");

        let cc_switch_dir = seed_cc_switch_db(&[(
            "codex-1",
            "codex",
            "Shared Name",
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-new" },
                "config": "model = \"gpt-5\"\nbase_url = \"https://new.example.com/v1\"\n"
            }),
        )])
        .await;
        let db_path = cc_switch_dir.path().join("cc-switch.db");

        let result = import_cc_switch_model_providers_from_path_core(
            &db,
            &db_path,
            ImportCcSwitchModelProvidersRequest {
                source_ids: vec!["codex:codex-1".to_string()],
                overwrite_same_name: true,
            },
        )
        .await
        .expect("attempt overwrite with mismatched agent type");

        assert!(result.imported_ids.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(
            result.skipped[0].skip_reason,
            Some(crate::models::model_provider::CcSwitchModelProviderSkipReason::DuplicateName)
        );

        let providers = list_model_providers_core(&db)
            .await
            .expect("list providers after mismatch skip");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].agent_type, "gemini");
        assert_eq!(providers[0].api_url, "https://existing.example.com/v1");
    }
}
