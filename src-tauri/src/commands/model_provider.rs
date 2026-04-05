use crate::app_error::AppCommandError;
use crate::db::service::model_provider_service;
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::model_provider::ModelProviderInfo;

// ---------------------------------------------------------------------------
// Shared core functions (used by both Tauri commands and web handlers)
// ---------------------------------------------------------------------------

fn validate_agent_types(agent_types: &[String]) -> Result<(), AppCommandError> {
    if agent_types.is_empty() {
        return Err(AppCommandError::invalid_input(
            "At least one agent type is required",
        ));
    }
    for at in agent_types {
        let _: AgentType = serde_json::from_value(serde_json::Value::String(at.clone()))
            .map_err(|_| {
                AppCommandError::invalid_input(format!("Invalid agent type: {at}"))
            })?;
    }
    Ok(())
}

fn validate_fields(
    name: Option<&str>,
    api_url: Option<&str>,
    api_key: Option<&str>,
) -> Result<(), AppCommandError> {
    if let Some(n) = name {
        if n.len() > 256 {
            return Err(AppCommandError::invalid_input("Name must be 256 characters or less"));
        }
    }
    if let Some(u) = api_url {
        if u.len() > 2048 {
            return Err(AppCommandError::invalid_input("API URL must be 2048 characters or less"));
        }
        if !u.starts_with("http://") && !u.starts_with("https://") {
            return Err(AppCommandError::invalid_input("API URL must start with http:// or https://"));
        }
    }
    if let Some(k) = api_key {
        if k.len() > 4096 {
            return Err(AppCommandError::invalid_input("API Key must be 4096 characters or less"));
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
    agent_types: Vec<String>,
) -> Result<ModelProviderInfo, AppCommandError> {
    validate_fields(Some(&name), Some(&api_url), Some(&api_key))?;
    validate_agent_types(&agent_types)?;
    let agent_types_json = serde_json::to_string(&agent_types)
        .map_err(|e| AppCommandError::invalid_input(e.to_string()))?;

    let model = model_provider_service::create(
        &db.conn,
        name,
        api_url,
        api_key,
        agent_types_json,
    )
    .await
    .map_err(AppCommandError::from)?;
    Ok(ModelProviderInfo::from(model))
}

pub async fn update_model_provider_core(
    db: &AppDatabase,
    id: i32,
    name: Option<String>,
    api_url: Option<String>,
    api_key: Option<String>,
    agent_types: Option<Vec<String>>,
) -> Result<ModelProviderInfo, AppCommandError> {
    validate_fields(name.as_deref(), api_url.as_deref(), api_key.as_deref())?;
    let agent_types_json = if let Some(ref ats) = agent_types {
        validate_agent_types(ats)?;
        Some(
            serde_json::to_string(ats)
                .map_err(|e| AppCommandError::invalid_input(e.to_string()))?,
        )
    } else {
        None
    };

    let model = model_provider_service::update(
        &db.conn,
        id,
        name,
        api_url,
        api_key,
        agent_types_json,
    )
    .await
    .map_err(AppCommandError::from)?;
    Ok(ModelProviderInfo::from(model))
}

pub async fn delete_model_provider_core(
    db: &AppDatabase,
    id: i32,
) -> Result<(), AppCommandError> {
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
    agent_types: Vec<String>,
) -> Result<ModelProviderInfo, AppCommandError> {
    create_model_provider_core(&db, name, api_url, api_key, agent_types).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn update_model_provider(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    name: Option<String>,
    api_url: Option<String>,
    api_key: Option<String>,
    agent_types: Option<Vec<String>>,
) -> Result<ModelProviderInfo, AppCommandError> {
    update_model_provider_core(&db, id, name, api_url, api_key, agent_types).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn delete_model_provider(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<(), AppCommandError> {
    delete_model_provider_core(&db, id).await
}
