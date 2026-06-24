use std::path::{Path, PathBuf};
use std::time::Duration;

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, QueryResult,
    Statement,
};
use serde_json::{Map, Value};

use crate::app_error::AppCommandError;
use crate::db::entities::model_provider;
use crate::models::model_provider::{
    CcSwitchModelProviderPreviewItem, CcSwitchModelProviderSkipReason,
};

#[derive(Debug, Clone)]
struct CcSwitchProviderRow {
    id: String,
    app_type: String,
    name: String,
    settings_config: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCcSwitchModelProvider {
    pub source_id: String,
    pub source_app_type: String,
    pub target_agent_type: String,
    pub name: String,
    pub api_url: String,
    pub api_key: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
struct LoadedCcSwitchProvider {
    preview: CcSwitchModelProviderPreviewItem,
    resolved: Option<ResolvedCcSwitchModelProvider>,
}

pub(crate) fn resolve_cc_switch_db_path() -> PathBuf {
    dirs::home_dir()
        .map(|home| home.join(".cc-switch").join("cc-switch.db"))
        .unwrap_or_else(|| PathBuf::from(".cc-switch/cc-switch.db"))
}

pub(crate) async fn load_cc_switch_preview_items(
    db_path: &Path,
    existing: &[model_provider::Model],
) -> Result<Vec<CcSwitchModelProviderPreviewItem>, AppCommandError> {
    Ok(load_cc_switch_sources(db_path, existing)
        .await?
        .into_iter()
        .map(|item| item.preview)
        .collect())
}

pub(crate) async fn load_cc_switch_import_candidates(
    db_path: &Path,
    existing: &[model_provider::Model],
) -> Result<Vec<ResolvedCcSwitchModelProvider>, AppCommandError> {
    Ok(load_cc_switch_sources(db_path, existing)
        .await?
        .into_iter()
        .filter_map(|item| item.resolved)
        .collect())
}

async fn load_cc_switch_sources(
    db_path: &Path,
    existing: &[model_provider::Model],
) -> Result<Vec<LoadedCcSwitchProvider>, AppCommandError> {
    let conn = open_cc_switch_sqlite_connection(db_path).await?;
    let rows = conn
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT id, app_type, name, settings_config FROM providers ORDER BY app_type ASC, name ASC, id ASC"
                .to_owned(),
        ))
        .await
        .map_err(|err| {
            AppCommandError::database_error("Failed to query cc-switch providers")
                .with_detail(err.to_string())
        })?;

    Ok(rows
        .iter()
        .map(parse_cc_switch_provider_row)
        .map(|row| normalize_cc_switch_provider_row(&row, existing))
        .collect())
}

async fn open_cc_switch_sqlite_connection(
    db_path: &Path,
) -> Result<DatabaseConnection, AppCommandError> {
    let db_url = format!(
        "sqlite:{}?mode=ro",
        urlencoding::encode(&db_path.to_string_lossy())
    );
    let mut opts = ConnectOptions::new(db_url);
    opts.max_connections(1)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(30))
        .sqlx_logging(false);

    let conn = Database::connect(opts).await.map_err(|err| {
        AppCommandError::database_error("Failed to open cc-switch database")
            .with_detail(err.to_string())
    })?;
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "PRAGMA busy_timeout=3000;".to_owned(),
    ))
    .await
    .map_err(|err| {
        AppCommandError::database_error("Failed to configure cc-switch database")
            .with_detail(err.to_string())
    })?;
    Ok(conn)
}

fn parse_cc_switch_provider_row(row: &QueryResult) -> CcSwitchProviderRow {
    let id: String = row.try_get("", "id").unwrap_or_default();
    let app_type: String = row.try_get("", "app_type").unwrap_or_default();
    let name: String = row.try_get("", "name").unwrap_or_default();
    let raw_settings: Option<String> = row.try_get("", "settings_config").ok();
    let settings_config = raw_settings
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());

    CcSwitchProviderRow {
        id,
        app_type,
        name,
        settings_config,
    }
}

fn normalize_cc_switch_provider_row(
    row: &CcSwitchProviderRow,
    existing: &[model_provider::Model],
) -> LoadedCcSwitchProvider {
    let source_id = format!("{}:{}", row.app_type, row.id);
    let name = row.name.trim().to_string();
    let skip = |reason: CcSwitchModelProviderSkipReason,
                api_url: Option<String>,
                model: Option<String>,
                target_agent_type: &str| LoadedCcSwitchProvider {
        preview: CcSwitchModelProviderPreviewItem {
            source_id: source_id.clone(),
            source_app_type: row.app_type.clone(),
            target_agent_type: target_agent_type.to_string(),
            name: name.clone(),
            api_url,
            model,
            importable: false,
            skip_reason: Some(reason),
        },
        resolved: None,
    };

    if name.is_empty() {
        return skip(CcSwitchModelProviderSkipReason::MissingName, None, None, "");
    }

    let Some(settings) = row.settings_config.as_ref() else {
        return skip(
            CcSwitchModelProviderSkipReason::MalformedSource,
            None,
            None,
            target_agent_type_for(row.app_type.as_str()),
        );
    };

    let target_agent_type = target_agent_type_for(row.app_type.as_str());
    let Some((api_url, api_key, model)) = extract_fields(row.app_type.as_str(), settings) else {
        let reason = if !matches!(
            row.app_type.as_str(),
            "claude" | "codex" | "gemini" | "opencode" | "openclaw" | "hermes"
        ) {
            CcSwitchModelProviderSkipReason::UnsupportedAppType
        } else {
            CcSwitchModelProviderSkipReason::MalformedSource
        };
        return skip(reason, None, None, target_agent_type);
    };

    let preview_api_url = (!api_url.trim().is_empty()).then_some(api_url.clone());
    let preview_model = model.clone();

    if api_url.trim().is_empty() {
        return skip(
            CcSwitchModelProviderSkipReason::MissingApiUrl,
            preview_api_url,
            preview_model,
            target_agent_type,
        );
    }
    if api_key.trim().is_empty() {
        return skip(
            CcSwitchModelProviderSkipReason::MissingApiKey,
            preview_api_url,
            preview_model,
            target_agent_type,
        );
    }
    if crate::commands::model_provider::validate_fields(Some(&name), Some(&api_url), Some(&api_key))
        .is_err()
    {
        return skip(
            CcSwitchModelProviderSkipReason::MalformedSource,
            preview_api_url,
            preview_model,
            target_agent_type,
        );
    }
    if crate::commands::model_provider::validate_model(target_agent_type, model.as_deref()).is_err()
    {
        return skip(
            CcSwitchModelProviderSkipReason::InvalidModel,
            preview_api_url,
            preview_model,
            target_agent_type,
        );
    }

    let resolved = ResolvedCcSwitchModelProvider {
        source_id: source_id.clone(),
        source_app_type: row.app_type.clone(),
        target_agent_type: target_agent_type.to_string(),
        name: name.clone(),
        api_url,
        api_key,
        model,
    };

    if let Some(reason) = duplicate_reason_for_candidate(&resolved, existing) {
        let keep_resolved = matches!(reason, CcSwitchModelProviderSkipReason::DuplicateName);
        return LoadedCcSwitchProvider {
            preview: CcSwitchModelProviderPreviewItem {
                source_id,
                source_app_type: row.app_type.clone(),
                target_agent_type: resolved.target_agent_type.clone(),
                name,
                api_url: Some(resolved.api_url.clone()),
                model: resolved.model.clone(),
                importable: false,
                skip_reason: Some(reason),
            },
            resolved: keep_resolved.then_some(resolved),
        };
    }

    LoadedCcSwitchProvider {
        preview: CcSwitchModelProviderPreviewItem {
            source_id,
            source_app_type: row.app_type.clone(),
            target_agent_type: resolved.target_agent_type.clone(),
            name,
            api_url: Some(resolved.api_url.clone()),
            model: resolved.model.clone(),
            importable: true,
            skip_reason: None,
        },
        resolved: Some(resolved),
    }
}

pub(crate) fn duplicate_reason_for_candidate(
    candidate: &ResolvedCcSwitchModelProvider,
    existing: &[model_provider::Model],
) -> Option<CcSwitchModelProviderSkipReason> {
    if existing
        .iter()
        .any(|provider| provider.name == candidate.name)
    {
        return Some(CcSwitchModelProviderSkipReason::DuplicateName);
    }

    existing
        .iter()
        .any(|provider| {
            provider.agent_type == candidate.target_agent_type
                && provider.api_url == candidate.api_url
                && provider.api_key == candidate.api_key
                && provider.model.as_deref() == candidate.model.as_deref()
        })
        .then_some(CcSwitchModelProviderSkipReason::DuplicateConfig)
}

fn target_agent_type_for(app_type: &str) -> &str {
    match app_type {
        "claude" => "claude_code",
        "codex" => "codex",
        "gemini" => "gemini",
        "opencode" => "open_code",
        "openclaw" => "open_claw",
        "hermes" => "hermes",
        _ => "",
    }
}

fn extract_fields(app_type: &str, settings: &Value) -> Option<(String, String, Option<String>)> {
    match app_type {
        "claude" => extract_claude(settings),
        "codex" => extract_codex(settings),
        "gemini" => extract_gemini(settings),
        "opencode" | "openclaw" => extract_openai_compatible(settings),
        "hermes" => extract_hermes(settings),
        _ => None,
    }
}

fn extract_claude(settings: &Value) -> Option<(String, String, Option<String>)> {
    let env = settings.get("env")?.as_object()?;
    let api_url = get_object_string(env, "ANTHROPIC_BASE_URL")?;
    let api_key = get_object_string(env, "ANTHROPIC_AUTH_TOKEN")
        .or_else(|| get_object_string(env, "ANTHROPIC_API_KEY"))?;

    let mut model = Map::new();
    for (field, env_key) in [
        ("main", "ANTHROPIC_MODEL"),
        ("reasoning", "ANTHROPIC_REASONING_MODEL"),
        ("haiku", "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        ("sonnet", "ANTHROPIC_DEFAULT_SONNET_MODEL"),
        ("opus", "ANTHROPIC_DEFAULT_OPUS_MODEL"),
        ("customOption", "ANTHROPIC_CUSTOM_MODEL_OPTION"),
        ("customOptionName", "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"),
        (
            "customOptionDescription",
            "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
        ),
    ] {
        if let Some(value) = get_object_string(env, env_key) {
            model.insert(field.to_string(), Value::String(value));
        }
    }
    let serialized_model = if model.is_empty() {
        None
    } else {
        serde_json::to_string(&Value::Object(model)).ok()
    };

    Some((api_url, api_key, serialized_model))
}

fn extract_codex(settings: &Value) -> Option<(String, String, Option<String>)> {
    let config_toml = settings.get("config")?.as_str()?;
    let config: toml::Value = toml::from_str(config_toml).ok()?;
    let api_url = config.get("base_url")?.as_str()?.trim().to_string();
    let model = config
        .get("model")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let api_key = settings
        .get("auth")
        .and_then(|value| value.get("OPENAI_API_KEY"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            settings
                .get("env")
                .and_then(|value| value.as_object())
                .and_then(|env| get_object_string(env, "CODEX_API_KEY"))
        })?;

    Some((api_url, api_key, model))
}

fn extract_gemini(settings: &Value) -> Option<(String, String, Option<String>)> {
    let env = settings.get("env")?.as_object()?;
    Some((
        get_object_string(env, "GOOGLE_GEMINI_BASE_URL")?,
        get_object_string(env, "GEMINI_API_KEY")?,
        get_object_string(env, "GEMINI_MODEL"),
    ))
}

fn extract_openai_compatible(settings: &Value) -> Option<(String, String, Option<String>)> {
    Some((
        get_value_string(settings, &["baseUrl", "base_url"])?,
        get_value_string(settings, &["apiKey", "api_key"])?,
        Some(get_value_string(settings, &["model"])?),
    ))
}

fn extract_hermes(settings: &Value) -> Option<(String, String, Option<String>)> {
    let api_url = get_value_string(settings, &["base_url"])?;
    let api_key = get_value_string(settings, &["api_key"])?;
    let model = settings
        .get("models")
        .and_then(|value| value.as_array())
        .and_then(|models| models.first())
        .and_then(|model| model.get("id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)?;
    Some((api_url, api_key, Some(model)))
}

fn get_object_string(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn get_value_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_gemini_row_into_importable_preview() {
        let row = CcSwitchProviderRow {
            id: "gem-1".to_string(),
            app_type: "gemini".to_string(),
            name: "Gemini Demo".to_string(),
            settings_config: Some(serde_json::json!({
                "env": {
                    "GOOGLE_GEMINI_BASE_URL": "https://api.example.com",
                    "GEMINI_API_KEY": "gm-key",
                    "GEMINI_MODEL": "gemini-3.5-flash"
                }
            })),
        };

        let item = normalize_cc_switch_provider_row(&row, &[]);
        assert!(item.preview.importable);
        assert_eq!(item.preview.target_agent_type, "gemini");
        assert_eq!(
            item.preview.api_url.as_deref(),
            Some("https://api.example.com")
        );
        assert_eq!(item.preview.model.as_deref(), Some("gemini-3.5-flash"));
        assert!(item.resolved.is_some());
    }

    #[test]
    fn skips_duplicate_name_even_when_config_differs() {
        let row = CcSwitchProviderRow {
            id: "codex-1".to_string(),
            app_type: "codex".to_string(),
            name: "Shared Name".to_string(),
            settings_config: Some(serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-demo" },
                "config": "model = \"gpt-5\"\nbase_url = \"https://api.example.com/v1\"\n"
            })),
        };
        let existing = vec![model_provider::Model {
            id: 1,
            name: "Shared Name".to_string(),
            api_url: "https://other.example.com/v1".to_string(),
            api_key: "sk-other".to_string(),
            agent_types_json: "[\"codex\"]".to_string(),
            agent_type: "codex".to_string(),
            model: Some("gpt-5".to_string()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }];

        let item = normalize_cc_switch_provider_row(&row, &existing);
        assert!(!item.preview.importable);
        assert_eq!(
            item.preview.skip_reason,
            Some(CcSwitchModelProviderSkipReason::DuplicateName)
        );
    }

    #[test]
    fn skips_openai_compatible_row_without_model() {
        let row = CcSwitchProviderRow {
            id: "opencode-1".to_string(),
            app_type: "opencode".to_string(),
            name: "OpenCode Missing Model".to_string(),
            settings_config: Some(serde_json::json!({
                "baseUrl": "https://api.example.com/v1",
                "apiKey": "sk-demo"
            })),
        };

        let item = normalize_cc_switch_provider_row(&row, &[]);
        assert!(!item.preview.importable);
        assert_eq!(
            item.preview.skip_reason,
            Some(CcSwitchModelProviderSkipReason::MalformedSource)
        );
    }

    #[test]
    fn skips_hermes_row_without_model() {
        let row = CcSwitchProviderRow {
            id: "hermes-1".to_string(),
            app_type: "hermes".to_string(),
            name: "Hermes Missing Model".to_string(),
            settings_config: Some(serde_json::json!({
                "base_url": "https://api.example.com/v1",
                "api_key": "sk-demo",
                "models": []
            })),
        };

        let item = normalize_cc_switch_provider_row(&row, &[]);
        assert!(!item.preview.importable);
        assert_eq!(
            item.preview.skip_reason,
            Some(CcSwitchModelProviderSkipReason::MalformedSource)
        );
    }
}
