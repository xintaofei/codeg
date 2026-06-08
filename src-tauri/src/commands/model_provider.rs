use crate::app_error::AppCommandError;
use crate::commands::acp;
use crate::db::service::{agent_setting_service, model_provider_service};
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::model_provider::ModelProviderInfo;
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

fn validate_fields(
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

fn validate_model(agent_type: &str, model: Option<&str>) -> Result<(), AppCommandError> {
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
#[allow(clippy::too_many_arguments)]
pub async fn update_model_provider(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    name: Option<String>,
    api_url: Option<String>,
    api_key: Option<String>,
    agent_type: Option<String>,
    model: Option<String>,
    app: tauri::AppHandle,
) -> Result<ModelProviderInfo, AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    update_model_provider_core(&db, id, name, api_url, api_key, agent_type, model, &emitter).await
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
}
