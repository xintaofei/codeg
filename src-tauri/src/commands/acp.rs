use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

use crate::acp::binary_cache;
use crate::acp::error::AcpError;
use crate::acp::manager::ConnectionManager;
use crate::acp::preflight::{self, PreflightResult};
use crate::acp::registry;
use crate::acp::types::{
    AcpAgentInfo, AgentSkillContent, AgentSkillItem, AgentSkillLayout, AgentSkillLocation,
    AgentSkillScope, AgentSkillsListResult, ConnectionInfo, ForkResultInfo, PromptInputBlock,
};
use crate::db::service::agent_setting_service;
use crate::db::AppDatabase;
use crate::models::agent::AgentType;

const ACP_AGENTS_UPDATED_EVENT: &str = "app://acp-agents-updated";

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
struct AcpAgentsUpdatedEventPayload {
    reason: &'static str,
    agent_type: Option<AgentType>,
}

fn emit_acp_agents_updated(
    app: &tauri::AppHandle,
    reason: &'static str,
    agent_type: Option<AgentType>,
) {
    let _ = app.emit(
        ACP_AGENTS_UPDATED_EVENT,
        AcpAgentsUpdatedEventPayload { reason, agent_type },
    );
}

fn is_version_like(value: &str) -> bool {
    value.chars().any(|c| c.is_ascii_digit()) && value.contains('.')
}

fn normalize_version_candidate(value: &str) -> Option<String> {
    let normalized = value.trim().trim_start_matches('v');
    if is_version_like(normalized) {
        Some(normalized.to_string())
    } else {
        None
    }
}

fn version_from_package_spec(package: &str) -> Option<String> {
    let (_, maybe_version) = package.rsplit_once('@')?;
    let version = maybe_version.trim();
    if version.is_empty() || version.eq_ignore_ascii_case("latest") {
        return None;
    }
    normalize_version_candidate(version)
}

fn package_name_from_spec(package: &str) -> String {
    let normalized = package.trim();
    if normalized.is_empty() {
        return String::new();
    }

    if let Some(index) = normalized.rfind('@') {
        if index > 0 {
            let version_part = normalized[index + 1..].trim();
            if !version_part.is_empty() {
                return normalized[..index].to_string();
            }
        }
    }

    normalized.to_string()
}

async fn detect_global_cmd_version(cmd: &str) -> Option<String> {
    let output = crate::process::tokio_command(cmd)
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    normalize_version_candidate(&raw)
}

async fn detect_local_version(agent_type: AgentType) -> Option<String> {
    let meta = registry::get_agent_meta(agent_type);
    match meta.distribution {
        registry::AgentDistribution::Npx { cmd, .. } => {
            detect_global_cmd_version(cmd).await
        }
        registry::AgentDistribution::Binary { cmd, .. } => {
            binary_cache::detect_installed_version(agent_type, cmd)
                .ok()
                .flatten()
        }
    }
}

async fn install_npm_global_package(package: &str) -> Result<(), AcpError> {
    let output = crate::process::tokio_command("npm")
        .arg("install")
        .arg("-g")
        .arg(package)
        .output()
        .await
        .map_err(|e| AcpError::protocol(format!("failed to run npm install -g: {e}")))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if err.is_empty() {
            "failed to install npm package globally".to_string()
        } else {
            format!("failed to install npm package globally: {err}")
        };
        return Err(AcpError::protocol(msg));
    }

    Ok(())
}

async fn uninstall_npm_global_package(package: &str) -> Result<(), AcpError> {
    let package_name = package_name_from_spec(package);

    if !package_name.is_empty() {
        let output = crate::process::tokio_command("npm")
            .arg("uninstall")
            .arg("-g")
            .arg(&package_name)
            .output()
            .await
            .map_err(|e| AcpError::protocol(format!("failed to run npm uninstall -g: {e}")))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if err.is_empty() {
                "failed to uninstall npm package globally".to_string()
            } else {
                format!("failed to uninstall npm package globally: {err}")
            };
            return Err(AcpError::protocol(msg));
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkillStorageKind {
    SkillDirectoryOnly,
    SkillDirectoryOrMarkdownFile,
}

#[derive(Debug, Clone)]
struct SkillStorageSpec {
    kind: SkillStorageKind,
    global_dirs: Vec<PathBuf>,
    project_rel_dirs: Vec<&'static str>,
}

fn home_dir_or_default() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn codex_home_dir() -> PathBuf {
    let configured = std::env::var("CODEX_HOME").ok().and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    match configured {
        Some(value) => {
            if value == "~" {
                home_dir_or_default()
            } else if let Some(remain) = value.strip_prefix("~/") {
                home_dir_or_default().join(remain)
            } else {
                PathBuf::from(value)
            }
        }
        None => home_dir_or_default().join(".codex"),
    }
}

fn codex_config_toml_path() -> PathBuf {
    codex_home_dir().join("config.toml")
}

fn codex_auth_json_path() -> PathBuf {
    codex_home_dir().join("auth.json")
}

fn opencode_primary_config_path() -> PathBuf {
    home_dir_or_default()
        .join(".config")
        .join("opencode")
        .join("opencode.json")
}

fn opencode_legacy_config_path() -> PathBuf {
    home_dir_or_default()
        .join(".config")
        .join("opencode")
        .join("config.json")
}

fn resolve_opencode_config_path() -> PathBuf {
    let primary = opencode_primary_config_path();
    if primary.exists() {
        return primary;
    }

    let legacy = opencode_legacy_config_path();
    if legacy.exists() {
        return legacy;
    }

    primary
}

fn opencode_auth_json_path() -> PathBuf {
    home_dir_or_default()
        .join(".local")
        .join("share")
        .join("opencode")
        .join("auth.json")
}

fn load_opencode_auth_json_raw() -> Option<String> {
    fs::read_to_string(opencode_auth_json_path()).ok()
}

fn load_codex_auth_json_raw() -> Option<String> {
    fs::read_to_string(codex_auth_json_path()).ok()
}

fn load_codex_config_toml_raw() -> Option<String> {
    fs::read_to_string(codex_config_toml_path()).ok()
}

fn load_codex_local_config_json() -> Option<String> {
    let mut merged = serde_json::Map::new();

    if let Ok(raw_toml) = fs::read_to_string(codex_config_toml_path()) {
        if let Ok(value) = raw_toml.parse::<toml::Value>() {
            if let Some(model) = value
                .get("model")
                .and_then(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                merged.insert(
                    "model".to_string(),
                    serde_json::Value::String(model.to_string()),
                );
            }

            let model_provider = value
                .get("model_provider")
                .and_then(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string);

            let mut api_base_url: Option<String> = None;
            if let Some(provider) = model_provider {
                api_base_url = value
                    .get("model_providers")
                    .and_then(|table| table.get(provider.as_str()))
                    .and_then(|table| table.get("base_url"))
                    .and_then(|item| item.as_str())
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string);
            }
            if api_base_url.is_none() {
                api_base_url = value
                    .get("model_providers")
                    .and_then(|table| table.as_table())
                    .and_then(|providers| {
                        providers.values().find_map(|item| {
                            item.get("base_url")
                                .and_then(|base| base.as_str())
                                .map(str::trim)
                                .filter(|base| !base.is_empty())
                                .map(str::to_string)
                        })
                    });
            }
            if let Some(base_url) = api_base_url {
                merged.insert(
                    "apiBaseUrl".to_string(),
                    serde_json::Value::String(base_url),
                );
            }

            if let Some(env) = value.get("env").and_then(|item| item.as_table()) {
                let mut env_map = serde_json::Map::new();
                for (key, item) in env {
                    let Some(raw) = item.as_str() else {
                        continue;
                    };
                    let trimmed = raw.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    env_map.insert(
                        key.to_string(),
                        serde_json::Value::String(trimmed.to_string()),
                    );
                }
                if !env_map.is_empty() {
                    merged.insert("env".to_string(), serde_json::Value::Object(env_map));
                }
            }
        }
    }

    if let Ok(raw_auth) = fs::read_to_string(codex_auth_json_path()) {
        if let Ok(auth) = serde_json::from_str::<serde_json::Value>(&raw_auth) {
            if let Some(api_key) = auth
                .get("OPENAI_API_KEY")
                .and_then(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                merged.insert(
                    "apiKey".to_string(),
                    serde_json::Value::String(api_key.to_string()),
                );
            }
        }
    }

    if merged.is_empty() {
        return None;
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(merged)).ok()
}

fn persist_codex_local_config(config_patch_json: Option<&str>) -> Result<(), AcpError> {
    let Some(raw_patch) = config_patch_json else {
        return Ok(());
    };
    let runtime = serde_json::from_str::<AgentRuntimeConfig>(raw_patch)
        .map_err(|e| AcpError::protocol(format!("invalid config_json: {e}")))?;
    let AgentRuntimeConfig {
        api_base_url,
        api_key,
        model,
        env,
    } = runtime;

    let config_path = codex_config_toml_path();
    let mut toml_value = if config_path.exists() {
        match fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| raw.parse::<toml::Value>().ok())
        {
            Some(existing) if existing.is_table() => existing,
            _ => toml::Value::Table(toml::map::Map::new()),
        }
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    let table = toml_value
        .as_table_mut()
        .ok_or_else(|| AcpError::protocol("codex config root must be a TOML table"))?;

    match trim_non_empty(model) {
        Some(model) => {
            table.insert("model".to_string(), toml::Value::String(model));
        }
        None => {
            table.remove("model");
        }
    }

    let provider_name = table
        .get("model_provider")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "codeg".to_string());
    table.insert(
        "model_provider".to_string(),
        toml::Value::String(provider_name.clone()),
    );

    let providers_item = table
        .entry("model_providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !providers_item.is_table() {
        *providers_item = toml::Value::Table(toml::map::Map::new());
    }
    let providers = providers_item
        .as_table_mut()
        .ok_or_else(|| AcpError::protocol("invalid model_providers table"))?;
    let provider_item = providers
        .entry(provider_name.clone())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !provider_item.is_table() {
        *provider_item = toml::Value::Table(toml::map::Map::new());
    }
    let provider_table = provider_item
        .as_table_mut()
        .ok_or_else(|| AcpError::protocol("invalid model provider table"))?;
    match trim_non_empty(api_base_url) {
        Some(base_url) => {
            provider_table.insert("base_url".to_string(), toml::Value::String(base_url));
        }
        None => {
            provider_table.remove("base_url");
        }
    }
    if provider_name == "codeg" {
        provider_table.insert("name".to_string(), toml::Value::String("codeg".to_string()));
        provider_table.insert(
            "wire_api".to_string(),
            toml::Value::String("responses".to_string()),
        );
        provider_table.insert(
            "requires_openai_auth".to_string(),
            toml::Value::Boolean(true),
        );
    }

    if env.is_empty() {
        table.remove("env");
    } else {
        let mut env_table = toml::map::Map::new();
        for (key, value) in env {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                continue;
            }
            env_table.insert(key, toml::Value::String(trimmed.to_string()));
        }
        if env_table.is_empty() {
            table.remove("env");
        } else {
            table.insert("env".to_string(), toml::Value::Table(env_table));
        }
    }

    let serialized_toml = toml::to_string_pretty(&toml_value)
        .map_err(|e| AcpError::protocol(format!("serialize codex toml failed: {e}")))?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            AcpError::protocol(format!("create codex config directory failed: {e}"))
        })?;
    }
    fs::write(&config_path, format!("{serialized_toml}\n"))
        .map_err(|e| AcpError::protocol(format!("write codex config failed: {e}")))?;

    let auth_path = codex_auth_json_path();
    let mut auth_value = if auth_path.exists() {
        match fs::read_to_string(&auth_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(existing) if existing.is_object() => existing,
            _ => serde_json::json!({}),
        }
    } else {
        serde_json::json!({})
    };
    let auth_obj = auth_value
        .as_object_mut()
        .ok_or_else(|| AcpError::protocol("codex auth root must be object"))?;
    match trim_non_empty(api_key) {
        Some(api_key) => {
            auth_obj.insert(
                "OPENAI_API_KEY".to_string(),
                serde_json::Value::String(api_key),
            );
        }
        None => {
            auth_obj.remove("OPENAI_API_KEY");
        }
    }
    let serialized_auth = serde_json::to_string_pretty(&auth_value)
        .map_err(|e| AcpError::protocol(format!("serialize codex auth failed: {e}")))?;
    if let Some(parent) = auth_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AcpError::protocol(format!("create codex auth directory failed: {e}")))?;
    }
    fs::write(&auth_path, format!("{serialized_auth}\n"))
        .map_err(|e| AcpError::protocol(format!("write codex auth failed: {e}")))?;

    Ok(())
}

fn persist_codex_native_config_files(
    codex_auth_json: Option<&str>,
    codex_config_toml: Option<&str>,
) -> Result<(), AcpError> {
    if let Some(raw_toml) = codex_config_toml {
        toml::from_str::<toml::Table>(raw_toml)
            .map_err(|e| AcpError::protocol(format!("invalid codex config.toml: {e}")))?;
        let path = codex_config_toml_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AcpError::protocol(format!("create codex directory failed: {e}")))?;
        }
        fs::write(&path, raw_toml)
            .map_err(|e| AcpError::protocol(format!("write codex config.toml failed: {e}")))?;
    }

    if let Some(raw_auth) = codex_auth_json {
        let parsed = serde_json::from_str::<serde_json::Value>(raw_auth)
            .map_err(|e| AcpError::protocol(format!("invalid codex auth.json: {e}")))?;
        if !parsed.is_object() {
            return Err(AcpError::protocol(
                "invalid codex auth.json: root must be a JSON object",
            ));
        }
        let path = codex_auth_json_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AcpError::protocol(format!("create codex directory failed: {e}")))?;
        }
        fs::write(&path, raw_auth)
            .map_err(|e| AcpError::protocol(format!("write codex auth.json failed: {e}")))?;
    }

    Ok(())
}

fn persist_opencode_auth_json(raw_auth: &str) -> Result<(), AcpError> {
    let parsed = serde_json::from_str::<serde_json::Value>(raw_auth)
        .map_err(|e| AcpError::protocol(format!("invalid opencode auth.json: {e}")))?;
    if !parsed.is_object() {
        return Err(AcpError::protocol(
            "invalid opencode auth.json: root must be a JSON object",
        ));
    }
    let path = opencode_auth_json_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AcpError::protocol(format!("create opencode directory failed: {e}")))?;
    }
    fs::write(&path, format!("{raw_auth}\n"))
        .map_err(|e| AcpError::protocol(format!("write opencode auth.json failed: {e}")))?;
    Ok(())
}

fn agent_local_config_path(agent_type: AgentType) -> Option<PathBuf> {
    match agent_type {
        AgentType::ClaudeCode => Some(home_dir_or_default().join(".claude").join("settings.json")),
        AgentType::Gemini => Some(home_dir_or_default().join(".gemini").join("settings.json")),
        AgentType::OpenCode => Some(resolve_opencode_config_path()),
        _ => None,
    }
}

fn load_agent_local_config_json(agent_type: AgentType) -> Option<String> {
    if agent_type == AgentType::Codex {
        return load_codex_local_config_json();
    }

    let path = agent_local_config_path(agent_type)?;
    if !path.exists() {
        return None;
    }

    let raw = fs::read_to_string(path).ok()?;
    let parsed = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    if !parsed.is_object() {
        return None;
    }
    serde_json::to_string_pretty(&parsed).ok()
}

fn merge_json_values(base: &mut serde_json::Value, patch: &serde_json::Value) {
    if let (Some(base_obj), Some(patch_obj)) = (base.as_object_mut(), patch.as_object()) {
        for (key, patch_value) in patch_obj {
            match base_obj.get_mut(key) {
                Some(base_value) => merge_json_values(base_value, patch_value),
                None => {
                    base_obj.insert(key.clone(), patch_value.clone());
                }
            }
        }
        return;
    }

    *base = patch.clone();
}

fn persist_agent_local_config_json(
    agent_type: AgentType,
    config_patch_json: Option<&str>,
) -> Result<(), AcpError> {
    if agent_type == AgentType::Codex {
        return persist_codex_local_config(config_patch_json);
    }

    let Some(path) = agent_local_config_path(agent_type) else {
        return Ok(());
    };
    let Some(raw_patch) = config_patch_json else {
        return Ok(());
    };

    let patch = serde_json::from_str::<serde_json::Value>(raw_patch)
        .map_err(|e| AcpError::protocol(format!("invalid config_json: {e}")))?;
    if !patch.is_object() {
        return Err(AcpError::protocol(
            "invalid config_json: root must be a JSON object",
        ));
    }

    if agent_type == AgentType::OpenCode {
        let serialized = serde_json::to_string_pretty(&patch)
            .map_err(|e| AcpError::protocol(format!("serialize config_json failed: {e}")))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AcpError::protocol(format!("create config directory failed: {e}")))?;
        }
        fs::write(&path, format!("{serialized}\n"))
            .map_err(|e| AcpError::protocol(format!("write local config failed: {e}")))?;
        return Ok(());
    }

    let mut base = if path.exists() {
        match fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(existing) if existing.is_object() => existing,
            _ => serde_json::json!({}),
        }
    } else {
        serde_json::json!({})
    };

    merge_json_values(&mut base, &patch);
    let serialized = serde_json::to_string_pretty(&base)
        .map_err(|e| AcpError::protocol(format!("serialize config_json failed: {e}")))?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AcpError::protocol(format!("create config directory failed: {e}")))?;
    }
    fs::write(&path, format!("{serialized}\n"))
        .map_err(|e| AcpError::protocol(format!("write local config failed: {e}")))?;

    Ok(())
}

fn skill_storage_spec(agent_type: AgentType) -> Option<SkillStorageSpec> {
    match agent_type {
        AgentType::ClaudeCode => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOnly,
            global_dirs: vec![home_dir_or_default().join(".claude").join("skills")],
            project_rel_dirs: vec![".claude/skills"],
        }),
        AgentType::Codex => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOrMarkdownFile,
            global_dirs: vec![
                home_dir_or_default().join(".agents").join("skills"),
                codex_home_dir().join("skills"),
            ],
            project_rel_dirs: vec![".agents/skills", ".codex/skills"],
        }),
        AgentType::OpenCode => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOnly,
            global_dirs: vec![home_dir_or_default()
                .join(".config")
                .join("opencode")
                .join("skills")],
            project_rel_dirs: vec![".opencode/skills"],
        }),
        AgentType::Gemini => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOnly,
            global_dirs: vec![
                home_dir_or_default().join(".gemini").join("skills"),
                home_dir_or_default().join(".agents").join("skills"),
            ],
            project_rel_dirs: vec![".gemini/skills", ".agents/skills"],
        }),
        AgentType::OpenClaw => Some(SkillStorageSpec {
            kind: SkillStorageKind::SkillDirectoryOnly,
            global_dirs: vec![home_dir_or_default().join(".openclaw").join("skills")],
            project_rel_dirs: vec!["skills"],
        }),
    }
}

fn scope_rank(scope: AgentSkillScope) -> u8 {
    match scope {
        AgentSkillScope::Global => 0,
        AgentSkillScope::Project => 1,
    }
}

fn validate_skill_id(raw: &str) -> Result<String, AcpError> {
    let id = raw.trim();
    if id.is_empty() {
        return Err(AcpError::protocol("skill id cannot be empty"));
    }
    if id.starts_with('.') {
        return Err(AcpError::protocol("skill id cannot start with a dot (.)"));
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(AcpError::protocol(
            "skill id cannot contain path separators",
        ));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(AcpError::protocol(
            "skill id can only include letters, numbers, '-', '_' and '.'",
        ));
    }
    Ok(id.to_string())
}

fn scoped_skill_dirs(
    agent_type: AgentType,
    scope: AgentSkillScope,
    workspace_path: Option<&str>,
) -> Result<Vec<PathBuf>, AcpError> {
    let spec = skill_storage_spec(agent_type).ok_or_else(|| {
        AcpError::protocol(format!(
            "{agent_type} skills are not supported in Settings yet"
        ))
    })?;

    match scope {
        AgentSkillScope::Global => Ok(spec.global_dirs),
        AgentSkillScope::Project => {
            let workspace = workspace_path
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| {
                    AcpError::protocol("workspace_path is required for project scoped skills")
                })?;
            Ok(spec
                .project_rel_dirs
                .iter()
                .map(|relative| PathBuf::from(workspace).join(relative))
                .collect())
        }
    }
}

fn preferred_scope_skill_dir(
    agent_type: AgentType,
    scope: AgentSkillScope,
    workspace_path: Option<&str>,
) -> Result<PathBuf, AcpError> {
    let dirs = scoped_skill_dirs(agent_type, scope, workspace_path)?;
    dirs.into_iter()
        .next()
        .ok_or_else(|| AcpError::protocol("no skill directory resolved for this agent"))
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn skill_name_from_id(id: &str) -> String {
    id.to_string()
}

fn build_skill_item(
    id: String,
    scope: AgentSkillScope,
    layout: AgentSkillLayout,
    path: PathBuf,
) -> AgentSkillItem {
    AgentSkillItem {
        name: skill_name_from_id(&id),
        id,
        scope,
        layout,
        path: path.to_string_lossy().to_string(),
    }
}

fn skill_content_path(layout: AgentSkillLayout, skill_path: &Path) -> PathBuf {
    match layout {
        AgentSkillLayout::SkillDirectory => skill_path.join("SKILL.md"),
        AgentSkillLayout::MarkdownFile => skill_path.to_path_buf(),
    }
}

fn list_skills_from_dir(
    scope: AgentSkillScope,
    dir: &Path,
    kind: SkillStorageKind,
) -> Result<Vec<AgentSkillItem>, AcpError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(dir)
        .map_err(|e| AcpError::protocol(format!("failed to read skills directory: {e}")))?;

    let mut by_id: BTreeMap<String, AgentSkillItem> = BTreeMap::new();
    for entry in entries {
        let entry = match entry {
            Ok(value) => value,
            Err(_) => continue,
        };
        let path = entry.path();
        let file_name = entry.file_name();
        let id = file_name.to_string_lossy().to_string();

        if path.is_dir()
            && matches!(
                kind,
                SkillStorageKind::SkillDirectoryOnly
                    | SkillStorageKind::SkillDirectoryOrMarkdownFile
            )
        {
            let skill_doc = path.join("SKILL.md");
            if !skill_doc.is_file() {
                continue;
            }
            by_id.insert(
                id.clone(),
                build_skill_item(id, scope, AgentSkillLayout::SkillDirectory, path),
            );
            continue;
        }

        if path.is_file()
            && matches!(kind, SkillStorageKind::SkillDirectoryOrMarkdownFile)
            && is_markdown_file(&path)
        {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
                .unwrap_or_else(|| id.clone());
            if by_id.contains_key(&stem) {
                continue;
            }
            by_id.insert(
                stem.clone(),
                build_skill_item(stem, scope, AgentSkillLayout::MarkdownFile, path),
            );
        }
    }

    Ok(by_id.into_values().collect())
}

fn locate_existing_skill(
    dir: &Path,
    kind: SkillStorageKind,
    skill_id: &str,
    scope: AgentSkillScope,
) -> Option<AgentSkillItem> {
    if matches!(
        kind,
        SkillStorageKind::SkillDirectoryOnly | SkillStorageKind::SkillDirectoryOrMarkdownFile
    ) {
        let skill_dir = dir.join(skill_id);
        if skill_dir.is_dir() && skill_dir.join("SKILL.md").is_file() {
            return Some(build_skill_item(
                skill_id.to_string(),
                scope,
                AgentSkillLayout::SkillDirectory,
                skill_dir,
            ));
        }
    }

    if matches!(kind, SkillStorageKind::SkillDirectoryOrMarkdownFile) {
        let file_path = dir.join(format!("{skill_id}.md"));
        if file_path.is_file() {
            return Some(build_skill_item(
                skill_id.to_string(),
                scope,
                AgentSkillLayout::MarkdownFile,
                file_path,
            ));
        }
    }

    None
}

fn locate_existing_skill_across_dirs(
    dirs: &[PathBuf],
    kind: SkillStorageKind,
    skill_id: &str,
    scope: AgentSkillScope,
) -> Option<AgentSkillItem> {
    for dir in dirs {
        if let Some(found) = locate_existing_skill(dir, kind, skill_id, scope) {
            return Some(found);
        }
    }
    None
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRuntimeConfig {
    #[serde(default, alias = "api_base_url")]
    api_base_url: Option<String>,
    #[serde(default, alias = "api_key")]
    api_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

fn trim_non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn important_env_targets(agent_type: AgentType) -> (&'static str, &'static str, &'static str) {
    match agent_type {
        AgentType::ClaudeCode => ("ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY", "ANTHROPIC_MODEL"),
        AgentType::Gemini => ("GOOGLE_GEMINI_BASE_URL", "GEMINI_API_KEY", "GEMINI_MODEL"),
        _ => ("OPENAI_BASE_URL", "OPENAI_API_KEY", "OPENAI_MODEL"),
    }
}

fn build_runtime_env_from_setting(
    agent_type: AgentType,
    setting: Option<&crate::db::entities::agent_setting::Model>,
    local_config_json: Option<&str>,
) -> BTreeMap<String, String> {
    let mut merged = setting
        .and_then(|model| model.env_json.as_deref())
        .and_then(|raw| serde_json::from_str::<BTreeMap<String, String>>(raw).ok())
        .unwrap_or_default();

    let Some(raw_config_json) = local_config_json else {
        return merged;
    };
    let Ok(config) = serde_json::from_str::<AgentRuntimeConfig>(raw_config_json) else {
        return merged;
    };

    for (key, value) in config.env {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        merged.insert(key, trimmed.to_string());
    }

    let (api_base_url_key, api_key_key, model_key) = important_env_targets(agent_type);
    if let Some(value) = trim_non_empty(config.api_base_url) {
        merged.insert(api_base_url_key.to_string(), value);
    }
    if let Some(value) = trim_non_empty(config.api_key) {
        merged.insert(api_key_key.to_string(), value);
    }
    if agent_type != AgentType::ClaudeCode {
        if let Some(value) = trim_non_empty(config.model) {
            merged.insert(model_key.to_string(), value);
        }
    }

    merged
}

#[tauri::command]
pub async fn acp_preflight(agent_type: AgentType) -> Result<PreflightResult, AcpError> {
    Ok(preflight::run_preflight(agent_type).await)
}

#[tauri::command]
pub async fn acp_connect(
    agent_type: AgentType,
    working_dir: Option<String>,
    session_id: Option<String>,
    manager: State<'_, ConnectionManager>,
    db: State<'_, AppDatabase>,
    app_handle: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<String, AcpError> {
    let meta = registry::get_agent_meta(agent_type);

    let setting = agent_setting_service::get_by_agent_type(&db.conn, agent_type)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    let disabled = setting
        .as_ref()
        .map(|model| !model.enabled)
        .unwrap_or(false);
    if disabled {
        return Err(AcpError::protocol(format!(
            "{agent_type} is disabled in settings"
        )));
    }
    let local_config_json = load_agent_local_config_json(agent_type);
    let mut runtime_env =
        build_runtime_env_from_setting(agent_type, setting.as_ref(), local_config_json.as_deref());

    // For OpenClaw: when creating a new conversation (no session_id to resume),
    // signal that we want a fresh transcript via --reset-session.
    if agent_type == AgentType::OpenClaw && session_id.is_none() {
        runtime_env.insert("OPENCLAW_RESET_SESSION".into(), "1".into());
    }

    if let registry::AgentDistribution::Npx { cmd, package, .. } = meta.distribution {
        if detect_global_cmd_version(cmd).await.is_none() {
            install_npm_global_package(package).await?;
        }
    }

    manager
        .spawn_agent(
            agent_type,
            working_dir,
            session_id,
            runtime_env,
            window.label().to_string(),
            app_handle,
        )
        .await
}

#[tauri::command]
pub async fn acp_prompt(
    connection_id: String,
    blocks: Vec<PromptInputBlock>,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager.send_prompt(&connection_id, blocks).await
}

#[tauri::command]
pub async fn acp_set_mode(
    connection_id: String,
    mode_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager.set_mode(&connection_id, mode_id).await
}

#[tauri::command]
pub async fn acp_set_config_option(
    connection_id: String,
    config_id: String,
    value_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager
        .set_config_option(&connection_id, config_id, value_id)
        .await
}

#[tauri::command]
pub async fn acp_cancel(
    connection_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager.cancel(&connection_id).await
}

#[tauri::command]
pub async fn acp_fork(
    connection_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<ForkResultInfo, AcpError> {
    manager.fork_session(&connection_id).await
}

#[tauri::command]
pub async fn acp_respond_permission(
    connection_id: String,
    request_id: String,
    option_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager
        .respond_permission(&connection_id, &request_id, &option_id)
        .await
}

#[tauri::command]
pub async fn acp_disconnect(
    connection_id: String,
    manager: State<'_, ConnectionManager>,
) -> Result<(), AcpError> {
    manager.disconnect(&connection_id).await
}

#[tauri::command]
pub async fn acp_list_connections(
    manager: State<'_, ConnectionManager>,
) -> Result<Vec<ConnectionInfo>, AcpError> {
    Ok(manager.list_connections().await)
}

#[tauri::command]
pub async fn acp_list_agents(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<AcpAgentInfo>, AcpError> {
    let platform = registry::current_platform();
    let agent_types = registry::all_acp_agents();

    let defaults = agent_types
        .iter()
        .enumerate()
        .map(
            |(idx, agent_type)| agent_setting_service::AgentDefaultInput {
                agent_type: *agent_type,
                registry_id: registry::registry_id_for(*agent_type).to_string(),
                default_sort_order: idx as i32,
            },
        )
        .collect::<Vec<_>>();

    agent_setting_service::ensure_defaults(&db.conn, &defaults)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    let settings_map = agent_setting_service::list_map_by_agent_type(&db.conn)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    let mut agents = Vec::new();
    for (idx, agent_type) in agent_types.into_iter().enumerate() {
        let setting = settings_map.get(&agent_type);
        let meta = registry::get_agent_meta(agent_type);
        let (available, dist_type, local_installed_version) = match &meta.distribution {
            registry::AgentDistribution::Npx { .. } => (
                true,
                "npx",
                setting.and_then(|m| m.installed_version.clone()),
            ),
            registry::AgentDistribution::Binary { platforms, cmd, .. } => {
                let detected = binary_cache::detect_installed_version(agent_type, cmd)
                    .ok()
                    .flatten();
                (
                    platforms.iter().any(|p| p.platform == platform),
                    "binary",
                    detected,
                )
            }
        };

        let mut env = setting
            .and_then(|m| m.env_json.as_deref())
            .and_then(|s| serde_json::from_str::<BTreeMap<String, String>>(s).ok())
            .unwrap_or_default();
        let local_config_json = load_agent_local_config_json(agent_type);
        if let Some(raw_local_config) = local_config_json.as_deref() {
            if let Ok(local_cfg) = serde_json::from_str::<AgentRuntimeConfig>(raw_local_config) {
                for (key, value) in local_cfg.env {
                    let trimmed = value.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    env.insert(key, trimmed.to_string());
                }
                let (api_base_url_key, api_key_key, model_key) = important_env_targets(agent_type);
                if let Some(value) = trim_non_empty(local_cfg.api_base_url) {
                    env.insert(api_base_url_key.to_string(), value);
                }
                if let Some(value) = trim_non_empty(local_cfg.api_key) {
                    env.insert(api_key_key.to_string(), value);
                }
                if agent_type != AgentType::ClaudeCode {
                    if let Some(value) = trim_non_empty(local_cfg.model) {
                        env.insert(model_key.to_string(), value);
                    }
                }
            }
        }
        let sort_order = setting.map(|m| m.sort_order).unwrap_or(idx as i32);
        if dist_type == "binary" {
            let _ = agent_setting_service::set_installed_version(
                &db.conn,
                agent_type,
                local_installed_version.clone(),
            )
            .await;
        }
        let codex_auth_json = if agent_type == AgentType::Codex {
            load_codex_auth_json_raw()
        } else {
            None
        };
        let opencode_auth_json = if agent_type == AgentType::OpenCode {
            load_opencode_auth_json_raw()
        } else {
            None
        };
        let codex_config_toml = if agent_type == AgentType::Codex {
            load_codex_config_toml_raw()
        } else {
            None
        };

        agents.push(AcpAgentInfo {
            agent_type,
            registry_id: registry::registry_id_for(agent_type).to_string(),
            registry_version: meta.registry_version().map(ToString::to_string),
            name: meta.name.to_string(),
            description: meta.description.to_string(),
            available,
            distribution_type: dist_type.to_string(),
            enabled: setting.map(|m| m.enabled).unwrap_or(true),
            sort_order,
            installed_version: local_installed_version,
            env,
            config_json: local_config_json,
            config_file_path: agent_local_config_path(agent_type)
                .map(|path| path.display().to_string()),
            opencode_auth_json,
            codex_auth_json,
            codex_config_toml,
        });
    }

    agents.sort_by(|a, b| {
        a.sort_order
            .cmp(&b.sort_order)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(agents)
}

#[tauri::command]
pub async fn acp_clear_binary_cache(agent_type: AgentType) -> Result<(), AcpError> {
    let meta = registry::get_agent_meta(agent_type);
    if matches!(
        meta.distribution,
        registry::AgentDistribution::Binary { .. }
    ) {
        binary_cache::clear_agent_cache(agent_type)?;
    }
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn acp_update_agent_preferences(
    agent_type: AgentType,
    enabled: bool,
    env: BTreeMap<String, String>,
    config_json: Option<String>,
    opencode_auth_json: Option<String>,
    codex_auth_json: Option<String>,
    codex_config_toml: Option<String>,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    let default = agent_setting_service::AgentDefaultInput {
        agent_type,
        registry_id: registry::registry_id_for(agent_type).to_string(),
        default_sort_order: i32::MAX / 2,
    };

    agent_setting_service::ensure_defaults(&db.conn, &[default])
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    let env_json = if env.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&env).map_err(|e| AcpError::protocol(e.to_string()))?)
    };
    let config_json = config_json.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let opencode_auth_json = opencode_auth_json.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    if let Some(raw) = config_json.as_deref() {
        let parsed = serde_json::from_str::<serde_json::Value>(raw)
            .map_err(|e| AcpError::protocol(format!("invalid config_json: {e}")))?;
        if !parsed.is_object() {
            return Err(AcpError::protocol(
                "invalid config_json: root must be a JSON object",
            ));
        }
    }

    let patch = agent_setting_service::AgentSettingsUpdate { enabled, env_json };
    agent_setting_service::update(&db.conn, agent_type, patch)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;

    if agent_type == AgentType::Codex {
        if codex_auth_json.is_some() || codex_config_toml.is_some() {
            persist_codex_native_config_files(
                codex_auth_json.as_deref(),
                codex_config_toml.as_deref(),
            )?;
        }
        emit_acp_agents_updated(&app, "preferences_updated", Some(agent_type));
        return Ok(());
    }

    if agent_type == AgentType::OpenCode {
        if let Some(raw_auth) = opencode_auth_json.as_deref() {
            persist_opencode_auth_json(raw_auth)?;
        }
        if let Some(raw) = config_json.as_deref() {
            persist_agent_local_config_json(agent_type, Some(raw))?;
        }
        emit_acp_agents_updated(&app, "preferences_updated", Some(agent_type));
        return Ok(());
    }

    let mut local_patch_value = config_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if !env.is_empty() {
        let env_json_value =
            serde_json::to_value(&env).map_err(|e| AcpError::protocol(e.to_string()))?;
        if let Some(obj) = local_patch_value.as_object_mut() {
            obj.insert("env".to_string(), env_json_value);
        }
    }
    let local_patch_json = serde_json::to_string(&local_patch_value)
        .map_err(|e| AcpError::protocol(format!("serialize local patch failed: {e}")))?;
    persist_agent_local_config_json(agent_type, Some(local_patch_json.as_str()))?;
    emit_acp_agents_updated(&app, "preferences_updated", Some(agent_type));
    Ok(())
}

#[tauri::command]
pub async fn acp_download_agent_binary(
    agent_type: AgentType,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    let meta = registry::get_agent_meta(agent_type);
    match meta.distribution {
        registry::AgentDistribution::Binary {
            version,
            cmd,
            platforms,
            ..
        } => {
            let platform = registry::current_platform();
            let fallback = platforms
                .iter()
                .find(|p| p.platform == platform)
                .ok_or_else(|| {
                    AcpError::PlatformNotSupported(format!(
                        "{} is not available on {platform}",
                        meta.name
                    ))
                })?;

            let _ = binary_cache::ensure_binary_for_agent(agent_type, version, fallback.url, cmd)
                .await?;
            emit_acp_agents_updated(&app, "binary_downloaded", Some(agent_type));
            Ok(())
        }
        registry::AgentDistribution::Npx { .. } => Err(
            AcpError::protocol("download is only supported for binary agents"),
        ),
    }
}

#[tauri::command]
pub async fn acp_detect_agent_local_version(
    agent_type: AgentType,
    db: State<'_, AppDatabase>,
) -> Result<Option<String>, AcpError> {
    let detected = detect_local_version(agent_type).await;
    if let Some(version) = detected.clone() {
        let _ = agent_setting_service::set_installed_version(
            &db.conn,
            agent_type,
            Some(version.clone()),
        )
        .await;
        return Ok(Some(version));
    }

    // For package-based agents, probing can miss cached availability.
    // Fall back to last known installed version persisted in DB.
    let fallback = agent_setting_service::get_by_agent_type(&db.conn, agent_type)
        .await
        .ok()
        .flatten()
        .and_then(|m| m.installed_version);
    Ok(fallback)
}

#[tauri::command]
pub async fn acp_prepare_npx_agent(
    agent_type: AgentType,
    registry_version: Option<String>,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<String, AcpError> {
    let meta = registry::get_agent_meta(agent_type);
    match meta.distribution {
        registry::AgentDistribution::Npx { package, .. } => {
            let default = agent_setting_service::AgentDefaultInput {
                agent_type,
                registry_id: registry::registry_id_for(agent_type).to_string(),
                default_sort_order: i32::MAX / 2,
            };
            agent_setting_service::ensure_defaults(&db.conn, &[default])
                .await
                .map_err(|e| AcpError::protocol(e.to_string()))?;

            let existing = agent_setting_service::get_by_agent_type(&db.conn, agent_type)
                .await
                .ok()
                .flatten()
                .and_then(|m| m.installed_version);

            install_npm_global_package(package).await?;
            let resolved = detect_local_version(agent_type)
                .await
                .or_else(|| version_from_package_spec(package))
                .or_else(|| {
                    registry_version
                        .as_deref()
                        .and_then(normalize_version_candidate)
                })
                .or(existing)
                .ok_or_else(|| {
                    AcpError::protocol(
                        "npm global install succeeded but failed to determine local version",
                    )
                })?;

            agent_setting_service::set_installed_version(
                &db.conn,
                agent_type,
                Some(resolved.clone()),
            )
            .await
            .map_err(|e| AcpError::protocol(e.to_string()))?;
            emit_acp_agents_updated(&app, "npx_prepared", Some(agent_type));
            Ok(resolved)
        }
        registry::AgentDistribution::Binary { .. } => Err(AcpError::protocol(
            "prepare is only supported for npx agents",
        )),
    }
}

#[tauri::command]
pub async fn acp_uninstall_agent(
    agent_type: AgentType,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    let meta = registry::get_agent_meta(agent_type);
    match meta.distribution {
        registry::AgentDistribution::Binary { .. } => {
            binary_cache::clear_agent_cache(agent_type)?;
        }
        registry::AgentDistribution::Npx { package, .. } => {
            uninstall_npm_global_package(package).await?;
        }
    }

    agent_setting_service::set_installed_version(&db.conn, agent_type, None)
        .await
        .map_err(|e| AcpError::protocol(e.to_string()))?;
    emit_acp_agents_updated(&app, "agent_uninstalled", Some(agent_type));
    Ok(())
}

#[tauri::command]
pub async fn acp_reorder_agents(
    agent_types: Vec<AgentType>,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<(), AcpError> {
    if agent_types.is_empty() {
        return Ok(());
    }
    agent_setting_service::reorder(&db.conn, &agent_types)
        .await
        .map_err(|e| {
            let message = e.to_string();
            if message.contains("database or disk is full") || message.contains("(code: 13)") {
                AcpError::protocol("无法保存排序：数据库可写空间不足。请释放磁盘空间后重试。")
            } else {
                AcpError::protocol(message)
            }
        })?;
    emit_acp_agents_updated(&app, "agent_reordered", None);
    Ok(())
}

#[tauri::command]
pub async fn acp_list_agent_skills(
    agent_type: AgentType,
    workspace_path: Option<String>,
) -> Result<AgentSkillsListResult, AcpError> {
    let Some(spec) = skill_storage_spec(agent_type) else {
        return Ok(AgentSkillsListResult {
            supported: false,
            message: Some(format!(
                "{agent_type} 暂不支持在设置页管理 Skills（当前仅支持 Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw）"
            )),
            locations: Vec::new(),
            skills: Vec::new(),
        });
    };

    let mut locations = Vec::new();
    let mut skills_by_key: BTreeMap<String, AgentSkillItem> = BTreeMap::new();

    for dir in &spec.global_dirs {
        locations.push(AgentSkillLocation {
            scope: AgentSkillScope::Global,
            path: dir.to_string_lossy().to_string(),
            exists: dir.exists(),
        });
        let listed = list_skills_from_dir(AgentSkillScope::Global, dir, spec.kind)?;
        for skill in listed {
            let key = format!("global:{}", skill.id);
            skills_by_key.entry(key).or_insert(skill);
        }
    }

    if let Some(workspace) = workspace_path.as_deref().map(str::trim) {
        if !workspace.is_empty() {
            for relative in &spec.project_rel_dirs {
                let project_dir = PathBuf::from(workspace).join(relative);
                locations.push(AgentSkillLocation {
                    scope: AgentSkillScope::Project,
                    path: project_dir.to_string_lossy().to_string(),
                    exists: project_dir.exists(),
                });
                let listed =
                    list_skills_from_dir(AgentSkillScope::Project, &project_dir, spec.kind)?;
                for skill in listed {
                    let key = format!("project:{}", skill.id);
                    skills_by_key.entry(key).or_insert(skill);
                }
            }
        }
    }

    let mut skills = skills_by_key.into_values().collect::<Vec<_>>();
    skills.sort_by(|a, b| {
        scope_rank(a.scope)
            .cmp(&scope_rank(b.scope))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(AgentSkillsListResult {
        supported: true,
        message: None,
        locations,
        skills,
    })
}

#[tauri::command]
pub async fn acp_read_agent_skill(
    agent_type: AgentType,
    scope: AgentSkillScope,
    skill_id: String,
    workspace_path: Option<String>,
) -> Result<AgentSkillContent, AcpError> {
    let Some(spec) = skill_storage_spec(agent_type) else {
        return Err(AcpError::protocol(format!(
            "{agent_type} skills are not supported in Settings yet"
        )));
    };
    let id = validate_skill_id(&skill_id)?;
    let dirs = scoped_skill_dirs(agent_type, scope, workspace_path.as_deref())?;

    let skill = locate_existing_skill_across_dirs(&dirs, spec.kind, &id, scope)
        .ok_or_else(|| AcpError::protocol(format!("skill not found: {id}")))?;
    let content_path = skill_content_path(skill.layout, Path::new(&skill.path));
    let content = fs::read_to_string(&content_path)
        .map_err(|e| AcpError::protocol(format!("failed to read skill content: {e}")))?;
    Ok(AgentSkillContent { skill, content })
}

#[tauri::command]
pub async fn acp_save_agent_skill(
    agent_type: AgentType,
    scope: AgentSkillScope,
    skill_id: String,
    content: String,
    workspace_path: Option<String>,
    layout: Option<AgentSkillLayout>,
) -> Result<AgentSkillItem, AcpError> {
    let Some(spec) = skill_storage_spec(agent_type) else {
        return Err(AcpError::protocol(format!(
            "{agent_type} skills are not supported in Settings yet"
        )));
    };
    let id = validate_skill_id(&skill_id)?;
    let dirs = scoped_skill_dirs(agent_type, scope, workspace_path.as_deref())?;
    let preferred_dir = preferred_scope_skill_dir(agent_type, scope, workspace_path.as_deref())?;

    fs::create_dir_all(&preferred_dir)
        .map_err(|e| AcpError::protocol(format!("failed to create skills directory: {e}")))?;

    let existing = locate_existing_skill_across_dirs(&dirs, spec.kind, &id, scope);
    let skill = if let Some(item) = existing {
        item
    } else {
        let new_layout = match spec.kind {
            SkillStorageKind::SkillDirectoryOnly => AgentSkillLayout::SkillDirectory,
            SkillStorageKind::SkillDirectoryOrMarkdownFile => {
                layout.unwrap_or(AgentSkillLayout::MarkdownFile)
            }
        };
        let skill_path = match new_layout {
            AgentSkillLayout::SkillDirectory => preferred_dir.join(&id),
            AgentSkillLayout::MarkdownFile => preferred_dir.join(format!("{id}.md")),
        };
        build_skill_item(id.clone(), scope, new_layout, skill_path)
    };

    let skill_path = PathBuf::from(&skill.path);
    let content_path = skill_content_path(skill.layout, &skill_path);

    if skill.layout == AgentSkillLayout::SkillDirectory {
        fs::create_dir_all(&skill_path).map_err(|e| {
            AcpError::protocol(format!(
                "failed to create skill directory '{}': {e}",
                skill.path
            ))
        })?;
    } else if let Some(parent) = content_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            AcpError::protocol(format!("failed to create skill parent directory: {e}"))
        })?;
    }

    fs::write(&content_path, content)
        .map_err(|e| AcpError::protocol(format!("failed to write skill content: {e}")))?;

    Ok(skill)
}

#[tauri::command]
pub async fn acp_delete_agent_skill(
    agent_type: AgentType,
    scope: AgentSkillScope,
    skill_id: String,
    workspace_path: Option<String>,
) -> Result<(), AcpError> {
    let Some(spec) = skill_storage_spec(agent_type) else {
        return Err(AcpError::protocol(format!(
            "{agent_type} skills are not supported in Settings yet"
        )));
    };
    let id = validate_skill_id(&skill_id)?;
    let dirs = scoped_skill_dirs(agent_type, scope, workspace_path.as_deref())?;

    let skill = locate_existing_skill_across_dirs(&dirs, spec.kind, &id, scope)
        .ok_or_else(|| AcpError::protocol(format!("skill not found: {id}")))?;
    let skill_path = PathBuf::from(&skill.path);
    if skill.layout == AgentSkillLayout::SkillDirectory {
        fs::remove_dir_all(&skill_path)
            .map_err(|e| AcpError::protocol(format!("failed to delete skill directory: {e}")))?;
    } else {
        fs::remove_file(&skill_path)
            .map_err(|e| AcpError::protocol(format!("failed to delete skill file: {e}")))?;
    }
    Ok(())
}
