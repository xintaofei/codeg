use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::app_error::AppCommandError;

const MARKETPLACE_OFFICIAL: &str = "official_registry";
const MARKETPLACE_SMITHERY: &str = "smithery";
static MARKETPLACE_HTTP_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .user_agent("codeg-mcp-market/1.0")
        .build()
        .map_err(|e| format!("failed to initialize marketplace HTTP client: {e}"))
});

fn mcp_invalid_input(message: impl Into<String>) -> AppCommandError {
    AppCommandError::invalid_input(message)
}

fn mcp_not_found(message: impl Into<String>) -> AppCommandError {
    AppCommandError::not_found(message)
}

fn mcp_configuration_invalid(message: impl Into<String>) -> AppCommandError {
    AppCommandError::configuration_invalid(message)
}

fn mcp_network(message: impl Into<String>) -> AppCommandError {
    AppCommandError::network(message)
}

/// Build the parameter map for an i18n-tagged MCP error.
fn mcp_i18n_params<const N: usize>(pairs: [(&str, &str); N]) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAppType {
    ClaudeCode,
    Codex,
    Gemini,
    OpenClaw,
    OpenCode,
    Cline,
    Hermes,
    CodeBuddy,
    KimiCode,
    Grok,
    Cursor,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalMcpServer {
    pub id: String,
    pub spec: Value,
    pub apps: Vec<McpAppType>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpMarketplaceProvider {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpMarketplaceItem {
    pub provider_id: String,
    pub server_id: String,
    pub name: String,
    pub description: String,
    pub homepage: Option<String>,
    pub remote: bool,
    pub verified: bool,
    pub icon_url: Option<String>,
    pub latest_version: Option<String>,
    pub protocols: Vec<String>,
    pub owner: Option<String>,
    pub namespace: Option<String>,
    pub downloads: Option<u64>,
    pub score: Option<f64>,
    pub is_deployed: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpMarketplaceInstallParameter {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub required: bool,
    pub secret: bool,
    pub kind: String,
    pub default_value: Option<Value>,
    pub placeholder: Option<String>,
    pub enum_values: Vec<String>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpMarketplaceInstallOption {
    pub id: String,
    pub protocol: String,
    pub label: String,
    pub description: Option<String>,
    pub spec: Value,
    pub parameters: Vec<McpMarketplaceInstallParameter>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpMarketplaceServerDetail {
    pub provider_id: String,
    pub server_id: String,
    pub name: String,
    pub description: String,
    pub homepage: Option<String>,
    pub remote: bool,
    pub verified: bool,
    pub icon_url: Option<String>,
    pub latest_version: Option<String>,
    pub protocols: Vec<String>,
    pub owner: Option<String>,
    pub namespace: Option<String>,
    pub downloads: Option<u64>,
    pub score: Option<f64>,
    pub is_deployed: Option<bool>,
    pub default_option_id: Option<String>,
    pub install_options: Vec<McpMarketplaceInstallOption>,
    pub spec: Value,
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn mcp_scan_local() -> Result<Vec<LocalMcpServer>, AppCommandError> {
    scan_local_servers()
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn mcp_list_marketplaces() -> Result<Vec<McpMarketplaceProvider>, AppCommandError> {
    Ok(vec![
        McpMarketplaceProvider {
            id: MARKETPLACE_OFFICIAL.to_string(),
            name: "Official MCP Registry".to_string(),
            description: "registry.modelcontextprotocol.io official MCP server registry"
                .to_string(),
        },
        McpMarketplaceProvider {
            id: MARKETPLACE_SMITHERY.to_string(),
            name: "Smithery".to_string(),
            description: "smithery.ai MCP server marketplace".to_string(),
        },
    ])
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn mcp_search_marketplace(
    provider_id: String,
    query: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<McpMarketplaceItem>, AppCommandError> {
    let q = query.unwrap_or_default();
    let max = limit.unwrap_or(30).clamp(1, 100);

    match provider_id.as_str() {
        MARKETPLACE_OFFICIAL => search_official_registry(&q, max).await,
        MARKETPLACE_SMITHERY => search_smithery(&q, max).await,
        _ => Err(mcp_invalid_input(format!(
            "unsupported marketplace provider: {provider_id}"
        ))),
    }
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn mcp_get_marketplace_server_detail(
    provider_id: String,
    server_id: String,
) -> Result<McpMarketplaceServerDetail, AppCommandError> {
    match provider_id.as_str() {
        MARKETPLACE_OFFICIAL => {
            let detail = fetch_official_server_detail(&server_id).await?;
            let item = official_entry_to_item(&detail);
            let install_options = build_official_install_options(&detail.server)?;
            let default_option = select_default_install_option(&install_options);
            let spec = default_option
                .map(|item| item.spec.clone())
                .ok_or_else(|| {
                    mcp_not_found(format!(
                        "official MCP server '{}' does not expose an installable transport",
                        item.server_id
                    ))
                })?;
            Ok(McpMarketplaceServerDetail {
                provider_id: MARKETPLACE_OFFICIAL.to_string(),
                server_id: item.server_id,
                name: item.name,
                description: item.description,
                homepage: item.homepage,
                remote: item.remote,
                verified: item.verified,
                icon_url: item.icon_url,
                latest_version: item.latest_version,
                protocols: item.protocols,
                owner: item.owner,
                namespace: item.namespace,
                downloads: item.downloads,
                score: item.score,
                is_deployed: item.is_deployed,
                default_option_id: default_option.map(|item| item.id.clone()),
                install_options,
                spec,
            })
        }
        MARKETPLACE_SMITHERY => {
            let detail = fetch_smithery_server_detail(&server_id).await?;
            let summary = fetch_smithery_server_summary(&server_id).await.ok();
            let install_options = build_smithery_install_options(&detail)?;
            let default_option = select_default_install_option(&install_options);
            let spec = default_option
                .map(|item| item.spec.clone())
                .ok_or_else(|| {
                    mcp_not_found(format!(
                        "smithery server '{}' does not provide installable connection info",
                        detail.qualified_name
                    ))
                })?;
            Ok(McpMarketplaceServerDetail {
                provider_id: MARKETPLACE_SMITHERY.to_string(),
                server_id: detail.qualified_name.clone(),
                name: detail.display_name.clone(),
                description: detail
                    .description
                    .as_deref()
                    .or_else(|| {
                        summary
                            .as_ref()
                            .and_then(|item| item.description.as_deref())
                    })
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| "No description".to_string()),
                homepage: detail
                    .homepage
                    .as_deref()
                    .or_else(|| summary.as_ref().and_then(|item| item.homepage.as_deref()))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                remote: detail.remote,
                verified: detail.verified
                    || summary.as_ref().map(|item| item.verified).unwrap_or(false),
                icon_url: detail
                    .icon_url
                    .as_deref()
                    .or_else(|| summary.as_ref().and_then(|item| item.icon_url.as_deref()))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                latest_version: None,
                protocols: collect_protocols_from_options(&install_options),
                owner: detail
                    .owner
                    .as_deref()
                    .or_else(|| summary.as_ref().and_then(|item| item.owner.as_deref()))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                namespace: detail
                    .namespace
                    .as_deref()
                    .or_else(|| summary.as_ref().and_then(|item| item.namespace.as_deref()))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                downloads: detail
                    .use_count
                    .or_else(|| summary.as_ref().and_then(|item| item.use_count)),
                score: detail
                    .score
                    .or_else(|| summary.as_ref().and_then(|item| item.score)),
                is_deployed: detail
                    .is_deployed
                    .or_else(|| summary.as_ref().and_then(|item| item.is_deployed)),
                default_option_id: default_option.map(|item| item.id.clone()),
                install_options,
                spec,
            })
        }
        _ => Err(mcp_invalid_input(format!(
            "unsupported marketplace provider: {provider_id}"
        ))),
    }
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn mcp_install_from_marketplace(
    provider_id: String,
    server_id: String,
    apps: Vec<McpAppType>,
    spec_override: Option<Value>,
    option_id: Option<String>,
    protocol: Option<String>,
    parameter_values: Option<Value>,
) -> Result<LocalMcpServer, AppCommandError> {
    let normalized_apps = normalize_apps(apps);
    if normalized_apps.is_empty() {
        return Err(mcp_invalid_input("at least one target app is required")
            .with_i18n("errors.appsRequired", BTreeMap::new()));
    }

    let selection = InstallSelection::new(option_id, protocol, parameter_values)?;

    let canonical_spec = if let Some(raw_spec) = spec_override.as_ref() {
        canonicalize_spec(raw_spec, "marketplace install override")?
    } else {
        match provider_id.as_str() {
            MARKETPLACE_OFFICIAL => {
                let detail = fetch_official_server_detail(&server_id).await?;
                resolve_official_install_spec_with_selection(&detail.server, &selection)?
            }
            MARKETPLACE_SMITHERY => {
                let detail = fetch_smithery_server_detail(&server_id).await?;
                resolve_smithery_install_spec_with_selection(&detail, &selection)?
            }
            _ => {
                return Err(mcp_invalid_input(format!(
                    "unsupported marketplace provider: {provider_id}"
                )));
            }
        }
    };

    let (hostable, excluded): (Vec<McpAppType>, Vec<McpAppType>) = normalized_apps
        .iter()
        .copied()
        .partition(|app| app_can_host_spec(*app, &canonical_spec));
    if hostable.is_empty() {
        // Every selected agent was excluded (e.g. only Codex for an SSE server);
        // fail instead of reporting success while writing nothing (and possibly
        // returning a pre-existing server with the same id). See issue #325.
        return Err(mcp_invalid_input(
            "none of the selected agents can host this MCP server's transport (e.g. Codex does not support SSE)",
        ));
    }
    // A selected-but-excluded app can't host this transport; remove any stale entry
    // for this id there so it can't win scan precedence and reclassify the spec.
    for app in excluded {
        tracing::warn!(
            "[MCP] {app:?} cannot host server '{server_id}' (transport unsupported); removing any stale entry"
        );
        let _ = remove_server_for_app(app, &server_id)?;
    }
    for app in hostable {
        upsert_server_for_app(app, &server_id, &canonical_spec)?;
    }

    find_local_server(&server_id)?.ok_or_else(|| {
        mcp_configuration_invalid(format!(
            "installed server '{server_id}', but failed to load it from local configuration"
        ))
    })
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn mcp_upsert_local_server(
    server_id: String,
    spec: Value,
    apps: Vec<McpAppType>,
) -> Result<LocalMcpServer, AppCommandError> {
    let canonical_spec = canonicalize_spec(&spec, "local MCP save")?;
    let target_apps = normalize_apps(apps);
    if target_apps.is_empty() {
        return Err(mcp_invalid_input("at least one target app is required")
            .with_i18n("errors.appsRequired", BTreeMap::new()));
    }

    // Preflight-exclude apps whose config can't host this transport (e.g. Codex +
    // SSE) so a multi-agent save neither writes a misrepresented entry nor aborts
    // the whole operation on the fail-fast `?` below. See issue #325.
    let target_set = target_apps
        .iter()
        .copied()
        .filter(|app| app_can_host_spec(*app, &canonical_spec))
        .collect::<BTreeSet<_>>();
    if target_set.is_empty() {
        // Every selected agent was excluded (e.g. only Codex chosen for an SSE
        // server). Surface a clear error rather than silently write nothing and then
        // fail the reload below.
        return Err(mcp_invalid_input(
            "none of the selected agents can host this MCP server's transport (e.g. Codex does not support SSE)",
        ));
    }
    let all_apps = [
        McpAppType::ClaudeCode,
        McpAppType::Codex,
        McpAppType::Gemini,
        McpAppType::OpenClaw,
        McpAppType::OpenCode,
        McpAppType::Cline,
        McpAppType::Hermes,
        McpAppType::CodeBuddy,
        McpAppType::KimiCode,
        McpAppType::Grok,
        McpAppType::Cursor,
    ];

    for app in all_apps {
        if target_set.contains(&app) {
            upsert_server_for_app(app, &server_id, &canonical_spec)?;
        } else {
            let _ = remove_server_for_app(app, &server_id)?;
        }
    }

    find_local_server(&server_id)?.ok_or_else(|| {
        mcp_configuration_invalid(format!(
            "saved local MCP server '{server_id}', but failed to reload it"
        ))
    })
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn mcp_set_server_apps(
    server_id: String,
    apps: Vec<McpAppType>,
) -> Result<Option<LocalMcpServer>, AppCommandError> {
    let target_apps = normalize_apps(apps);
    let current = find_local_server(&server_id)?
        .ok_or_else(|| mcp_not_found(format!("local MCP server not found: {server_id}")))?;

    // Preflight-exclude apps whose config can't host this transport (e.g. Codex +
    // SSE); such an app is treated as "not targeted" so any stale entry is removed
    // rather than rewritten as a misrepresented one. See issue #325.
    let target_set = target_apps
        .iter()
        .copied()
        .filter(|app| app_can_host_spec(*app, &current.spec))
        .collect::<BTreeSet<_>>();
    if !target_apps.is_empty() && target_set.is_empty() {
        // Every explicitly selected agent was excluded (e.g. only Codex chosen for
        // an SSE server). Fail before mutating rather than silently delete the
        // server; an explicit empty `apps` still means "remove from all" and is
        // allowed to fall through.
        return Err(mcp_invalid_input(
            "none of the selected agents can host this MCP server's transport (e.g. Codex does not support SSE)",
        ));
    }
    let current_set = current.apps.iter().copied().collect::<BTreeSet<_>>();

    for app in current_set.difference(&target_set) {
        remove_server_for_app(*app, &server_id)?;
    }

    for app in target_set.difference(&current_set) {
        upsert_server_for_app(*app, &server_id, &current.spec)?;
    }

    find_local_server(&server_id)
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn mcp_remove_server(
    server_id: String,
    apps: Option<Vec<McpAppType>>,
) -> Result<bool, AppCommandError> {
    let target_apps = match apps {
        Some(selected) => normalize_apps(selected),
        None => vec![
            McpAppType::ClaudeCode,
            McpAppType::Codex,
            McpAppType::Gemini,
            McpAppType::OpenClaw,
            McpAppType::OpenCode,
            McpAppType::Cline,
            McpAppType::Hermes,
            McpAppType::CodeBuddy,
            McpAppType::KimiCode,
            McpAppType::Grok,
            McpAppType::Cursor,
        ],
    };

    if target_apps.is_empty() {
        return Ok(false);
    }

    let mut removed = false;
    for app in target_apps {
        removed |= remove_server_for_app(app, &server_id)?;
    }
    Ok(removed)
}

fn normalize_apps(apps: Vec<McpAppType>) -> Vec<McpAppType> {
    let mut seen = BTreeSet::new();
    for app in apps {
        seen.insert(app);
    }
    seen.into_iter().collect()
}

/// Whether `app`'s on-disk config can faithfully host `canonical_spec`. Codex's
/// config.toml has only stdio and streamable-HTTP transports, so it cannot host an
/// SSE server — writing one would persist a url-only entry that Codex loads as HTTP
/// and codeg then reads back as `http`, silently reclassifying the shared canonical
/// spec. Write paths preflight-exclude such (app, spec) pairs instead of writing a
/// misrepresented entry or aborting the whole multi-agent operation. See issue #325.
fn app_can_host_spec(app: McpAppType, canonical_spec: &Value) -> bool {
    let is_sse = canonical_spec.get("type").and_then(Value::as_str) == Some("sse");
    !(app == McpAppType::Codex && is_sse)
}

#[derive(Debug, Clone)]
struct InstallSelection {
    option_id: Option<String>,
    protocol: Option<String>,
    parameter_values: Map<String, Value>,
}

impl InstallSelection {
    fn new(
        option_id: Option<String>,
        protocol: Option<String>,
        parameter_values: Option<Value>,
    ) -> Result<Self, AppCommandError> {
        let parsed = if let Some(raw) = parameter_values {
            let obj = raw
                .as_object()
                .ok_or_else(|| mcp_invalid_input("parameter_values must be a JSON object"))?;
            obj.clone()
        } else {
            Map::new()
        };

        Ok(Self {
            option_id: option_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            protocol: protocol
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(normalize_protocol_value),
            parameter_values: parsed,
        })
    }
}

/// Normalize a user-supplied MCP transport type string into one of the
/// canonical values understood by `canonicalize_spec`.
///
/// Stage 1 (precise): trimmed lowercase exact match against the ACP/MCP-spec
/// canonical names (`stdio` / `http` / `sse`) plus the OpenCode-native markers
/// (`local` / `remote`). The latter two are NOT ACP types — they appear only
/// as a redirect signal so `canonicalize_spec` can hand off to
/// `canonicalize_opencode_spec` when a user pastes OpenCode-format JSON
/// (`type: "local" | "remote"`, command-as-array, `environment` instead of
/// `env`). After translation, the canonical output's type is always one of
/// `stdio` / `http` / `sse`.
///
/// Stage 2 (alias collapse, http only): strip non-ASCII-alphanumeric characters
/// and lowercase, then match `streamablehttp` -> `http`. Catches
/// `streamable-http`, `streamableHttp`, `streamable_http`, `Streamable HTTP`,
/// etc. Inputs containing non-ASCII separators (e.g. U+2010 hyphen, full-width
/// letters from CJK IME) are intentionally rejected and fall through to the
/// caller's unsupported-type error — that path echoes the raw value, so users
/// can spot the encoding issue.
///
/// Returns `None` for unknown values so callers can decide between strict
/// rejection and permissive fallback.
fn normalize_mcp_type(raw: &str) -> Option<&'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "stdio" => return Some("stdio"),
        "http" => return Some("http"),
        "sse" => return Some("sse"),
        "local" => return Some("local"),
        "remote" => return Some("remote"),
        _ => {}
    }

    let collapsed: String = lower
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if collapsed == "streamablehttp" {
        return Some("http");
    }

    None
}

fn normalize_protocol_value(raw: &str) -> String {
    normalize_mcp_type(raw)
        .map(str::to_string)
        .unwrap_or_else(|| raw.trim().to_string())
}

fn protocol_priority(protocol: &str) -> i32 {
    match normalize_protocol_value(protocol).as_str() {
        "stdio" => 0,
        "http" => 1,
        "sse" => 2,
        _ => 10,
    }
}

fn select_default_install_option(
    options: &[McpMarketplaceInstallOption],
) -> Option<&McpMarketplaceInstallOption> {
    options
        .iter()
        .min_by_key(|item| protocol_priority(&item.protocol))
}

fn collect_protocols_from_options(options: &[McpMarketplaceInstallOption]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    for option in options {
        seen.insert(normalize_protocol_value(&option.protocol));
    }
    seen.into_iter().collect()
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

fn claude_config_path() -> PathBuf {
    home_dir_or_default().join(".claude.json")
}

fn claude_settings_path() -> PathBuf {
    home_dir_or_default().join(".claude").join("settings.json")
}

/// The marketplace suffix codeg uses when toggling user-scope Claude Code
/// MCP servers via `enabledPlugins`. Empirically validated: `figma@local`
/// activates a user-scope MCP, `figma@user` does not. The suffix is treated
/// by Claude Code CLI as a free-form tag identifying the source — `local`
/// is the conventional value for user-managed entries.
const CLAUDE_LOCAL_PLUGIN_MARKETPLACE: &str = "local";

fn claude_local_plugin_key(id: &str) -> String {
    format!("{id}@{CLAUDE_LOCAL_PLUGIN_MARKETPLACE}")
}

fn codex_config_toml_path() -> PathBuf {
    codex_home_dir().join("config.toml")
}

fn opencode_config_path() -> PathBuf {
    home_dir_or_default()
        .join(".config")
        .join("opencode")
        .join("opencode.json")
}

fn gemini_config_path() -> PathBuf {
    home_dir_or_default().join(".gemini").join("settings.json")
}

fn openclaw_config_path() -> PathBuf {
    home_dir_or_default()
        .join(".openclaw")
        .join("openclaw.json")
}

fn cline_config_path() -> PathBuf {
    home_dir_or_default()
        .join(".cline")
        .join("data")
        .join("settings")
        .join("cline_mcp_settings.json")
}

fn read_json_file(path: &Path) -> Result<Value, AppCommandError> {
    if !path.exists() {
        return Ok(json!({}));
    }

    let raw = fs::read_to_string(path).map_err(AppCommandError::io)?;
    serde_json::from_str::<Value>(&raw)
        .map_err(|e| mcp_configuration_invalid(format!("invalid JSON at {}: {e}", path.display())))
}

fn write_json_file(path: &Path, value: &Value) -> Result<(), AppCommandError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(AppCommandError::io)?;
    }
    let serialized = serde_json::to_string_pretty(value).map_err(|e| {
        mcp_configuration_invalid(format!(
            "failed to serialize JSON for {}: {e}",
            path.display()
        ))
    })?;
    fs::write(path, format!("{serialized}\n")).map_err(AppCommandError::io)
}

fn read_codex_root_toml() -> Result<toml::Value, AppCommandError> {
    let path = codex_config_toml_path();
    if !path.exists() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }

    let raw = fs::read_to_string(&path).map_err(AppCommandError::io)?;
    let parsed = raw.parse::<toml::Value>().map_err(|e| {
        mcp_configuration_invalid(format!("invalid TOML at {}: {e}", path.display()))
    })?;

    if !parsed.is_table() {
        return Err(mcp_configuration_invalid(format!(
            "invalid TOML root at {}: expected table",
            path.display()
        )));
    }

    Ok(parsed)
}

fn write_codex_root_toml(root: &toml::Value) -> Result<(), AppCommandError> {
    let path = codex_config_toml_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(AppCommandError::io)?;
    }

    let serialized = toml::to_string_pretty(root).map_err(|e| {
        mcp_configuration_invalid(format!(
            "failed to serialize TOML for {}: {e}",
            path.display()
        ))
    })?;
    fs::write(&path, format!("{serialized}\n")).map_err(AppCommandError::io)
}

fn obj_as_string_map(value: Option<&Value>) -> Option<Map<String, Value>> {
    let obj = value.and_then(Value::as_object)?;

    let mut output = Map::with_capacity(obj.len());
    for (key, item) in obj {
        let Some(s) = item.as_str() else {
            continue;
        };
        let trimmed = s.trim();
        if trimmed.is_empty() {
            continue;
        }
        output.insert(key.to_string(), Value::String(trimmed.to_string()));
    }

    if output.is_empty() {
        None
    } else {
        Some(output)
    }
}

fn contains_unresolved_placeholder(value: &str) -> bool {
    value.contains('{') && value.contains('}')
}

fn marketplace_http_client() -> Result<reqwest::Client, AppCommandError> {
    match &*MARKETPLACE_HTTP_CLIENT {
        Ok(client) => Ok(client.clone()),
        Err(err) => Err(mcp_network(err.clone())),
    }
}

fn should_retry_http_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn format_market_network_error(context: &str, err: &reqwest::Error) -> String {
    if err.is_timeout() {
        return format!(
            "{context}: request timed out. Please check network/proxy settings and retry: {err}"
        );
    }
    if err.is_connect() {
        return format!(
            "{context}: network connection failed. Please check network/proxy settings and retry: {err}"
        );
    }
    format!("{context}: {err}")
}

async fn send_request_with_retry<F>(
    context: &str,
    mut build: F,
) -> Result<reqwest::Response, AppCommandError>
where
    F: FnMut() -> reqwest::RequestBuilder,
{
    const MAX_ATTEMPTS: usize = 3;
    let mut last_error: Option<String> = None;

    for attempt in 1..=MAX_ATTEMPTS {
        match build().send().await {
            Ok(response) => {
                if should_retry_http_status(response.status()) && attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(Duration::from_millis((attempt as u64) * 350)).await;
                    continue;
                }
                return Ok(response);
            }
            Err(err) => {
                last_error = Some(format_market_network_error(context, &err));
                if attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(Duration::from_millis((attempt as u64) * 350)).await;
                }
            }
        }
    }

    Err(mcp_network(
        last_error.unwrap_or_else(|| format!("{context}: request failed")),
    ))
}

async fn parse_json_response<T: DeserializeOwned>(
    response: reqwest::Response,
    context: &str,
) -> Result<T, AppCommandError> {
    let raw = response
        .text()
        .await
        .map_err(|e| mcp_network(format!("{context}: failed to read response body: {e}")))?;
    serde_json::from_str::<T>(&raw)
        .map_err(|e| mcp_network(format!("{context}: invalid JSON response: {e}")))
}

async fn parse_json_value_response(
    response: reqwest::Response,
    context: &str,
) -> Result<Value, AppCommandError> {
    let raw = response
        .text()
        .await
        .map_err(|e| mcp_network(format!("{context}: failed to read response body: {e}")))?;
    serde_json::from_str::<Value>(&raw)
        .map_err(|e| mcp_network(format!("{context}: invalid JSON response: {e}")))
}

fn canonicalize_spec(spec: &Value, source: &str) -> Result<Value, AppCommandError> {
    let obj = spec.as_object().ok_or_else(|| {
        mcp_invalid_input(format!("{source}: MCP spec must be a JSON object"))
            .with_i18n("errors.specMustBeObject", BTreeMap::new())
    })?;

    let raw_type = obj
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();

    let resolved_type: &'static str = if raw_type.is_empty() {
        if obj.get("command").is_some() {
            "stdio"
        } else if obj.get("url").is_some() {
            "http"
        } else {
            return Err(mcp_invalid_input(format!(
                "{source}: MCP spec missing 'type'; provide one of stdio, http (aliases: streamable-http, streamableHttp), sse"
            ))
            .with_i18n("errors.missingType", BTreeMap::new()));
        }
    } else {
        match normalize_mcp_type(&raw_type) {
            Some(value) => value,
            None => {
                return Err(mcp_invalid_input(format!(
                    "{source}: unsupported MCP server type '{raw_type}'; supported: stdio, http (aliases: streamable-http, streamableHttp), sse"
                ))
                .with_i18n(
                    "errors.unsupportedType",
                    mcp_i18n_params([("type", raw_type.as_str())]),
                ));
            }
        }
    };

    let mut normalized = Map::new();

    match resolved_type {
        "stdio" => {
            let command = obj
                .get("command")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    mcp_invalid_input(format!(
                        "{source}: stdio MCP spec requires a non-empty command"
                    ))
                    .with_i18n("errors.stdioCommandRequired", BTreeMap::new())
                })?;

            normalized.insert("type".to_string(), Value::String("stdio".to_string()));
            normalized.insert("command".to_string(), Value::String(command.to_string()));

            if let Some(args) = obj.get("args").and_then(Value::as_array) {
                let values = args
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| Value::String(value.to_string()))
                    .collect::<Vec<_>>();
                if !values.is_empty() {
                    normalized.insert("args".to_string(), Value::Array(values));
                }
            }

            if let Some(env) = obj_as_string_map(obj.get("env")) {
                normalized.insert("env".to_string(), Value::Object(env));
            }

            if let Some(cwd) = obj
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                normalized.insert("cwd".to_string(), Value::String(cwd.to_string()));
            }
        }
        "http" | "sse" => {
            let url = obj
                .get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    mcp_invalid_input(format!(
                        "{source}: remote MCP spec requires a non-empty url"
                    ))
                    .with_i18n("errors.remoteUrlRequired", BTreeMap::new())
                })?;

            normalized.insert("type".to_string(), Value::String(resolved_type.to_string()));
            normalized.insert("url".to_string(), Value::String(url.to_string()));

            if let Some(headers) = obj_as_string_map(obj.get("headers")) {
                normalized.insert("headers".to_string(), Value::Object(headers));
            }
        }
        "local" | "remote" => {
            return canonicalize_opencode_spec(spec, source);
        }
        _ => unreachable!("normalize_mcp_type returns one of stdio/http/sse/local/remote"),
    }

    for (key, value) in obj {
        if normalized.contains_key(key) {
            continue;
        }
        if key == "type"
            || key == "command"
            || key == "args"
            || key == "env"
            || key == "cwd"
            || key == "url"
            || key == "headers"
        {
            continue;
        }
        if !value.is_null() {
            normalized.insert(key.clone(), value.clone());
        }
    }

    Ok(Value::Object(normalized))
}

fn canonicalize_opencode_spec(spec: &Value, source: &str) -> Result<Value, AppCommandError> {
    let obj = spec.as_object().ok_or_else(|| {
        mcp_invalid_input(format!("{source}: OpenCode MCP spec must be a JSON object"))
    })?;

    let typ = obj
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("local");

    match typ {
        "local" => {
            let mut converted = Map::new();
            converted.insert("type".to_string(), Value::String("stdio".to_string()));

            if let Some(command) = obj.get("command") {
                if let Some(arr) = command.as_array() {
                    let first = arr
                        .first()
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .ok_or_else(|| {
                            mcp_invalid_input(format!(
                                "{source}: local MCP command array must include executable"
                            ))
                        })?;
                    converted.insert("command".to_string(), Value::String(first.to_string()));

                    if arr.len() > 1 {
                        let args = arr[1..]
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::trim)
                            .filter(|item| !item.is_empty())
                            .map(|item| Value::String(item.to_string()))
                            .collect::<Vec<_>>();
                        if !args.is_empty() {
                            converted.insert("args".to_string(), Value::Array(args));
                        }
                    }
                } else if let Some(raw) = command.as_str() {
                    let trimmed = raw.trim();
                    if trimmed.is_empty() {
                        return Err(mcp_invalid_input(format!(
                            "{source}: local MCP command must be non-empty"
                        )));
                    }
                    converted.insert("command".to_string(), Value::String(trimmed.to_string()));
                }
            }

            if let Some(env) = obj_as_string_map(obj.get("environment")) {
                converted.insert("env".to_string(), Value::Object(env));
            }

            if let Some(cwd) = obj
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                converted.insert("cwd".to_string(), Value::String(cwd.to_string()));
            }

            canonicalize_spec(&Value::Object(converted), source)
        }
        "remote" => {
            let mut converted = Map::new();
            let remote_type = obj
                .get("transport")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| *value == "sse")
                .map(|_| "sse")
                .unwrap_or("http");
            converted.insert("type".to_string(), Value::String(remote_type.to_string()));

            if let Some(url) = obj
                .get("url")
                .or_else(|| obj.get("deploymentUrl"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                converted.insert("url".to_string(), Value::String(url.to_string()));
            }

            if let Some(headers) = obj_as_string_map(obj.get("headers")) {
                converted.insert("headers".to_string(), Value::Object(headers));
            }

            canonicalize_spec(&Value::Object(converted), source)
        }
        _ => canonicalize_spec(spec, source),
    }
}

fn canonical_to_opencode_spec(spec: &Value) -> Result<Value, AppCommandError> {
    let canonical = canonicalize_spec(spec, "OpenCode conversion")?;
    let obj = canonical.as_object().ok_or_else(|| {
        mcp_invalid_input("OpenCode conversion: canonical spec must be an object")
    })?;

    let typ = obj.get("type").and_then(Value::as_str).unwrap_or("stdio");

    let mut out = Map::new();

    match typ {
        "stdio" => {
            let cmd = obj.get("command").and_then(Value::as_str).ok_or_else(|| {
                mcp_invalid_input("OpenCode conversion: stdio MCP spec missing command")
            })?;
            out.insert("type".to_string(), Value::String("local".to_string()));

            let mut command = vec![Value::String(cmd.to_string())];
            if let Some(args) = obj.get("args").and_then(Value::as_array) {
                for arg in args {
                    if let Some(raw) = arg.as_str() {
                        let trimmed = raw.trim();
                        if !trimmed.is_empty() {
                            command.push(Value::String(trimmed.to_string()));
                        }
                    }
                }
            }
            out.insert("command".to_string(), Value::Array(command));

            if let Some(env) = obj_as_string_map(obj.get("env")) {
                out.insert("environment".to_string(), Value::Object(env));
            }

            if let Some(cwd) = obj
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                out.insert("cwd".to_string(), Value::String(cwd.to_string()));
            }
        }
        "http" | "sse" => {
            let url = obj.get("url").and_then(Value::as_str).ok_or_else(|| {
                mcp_invalid_input("OpenCode conversion: remote MCP spec missing url")
            })?;
            out.insert("type".to_string(), Value::String("remote".to_string()));
            out.insert("url".to_string(), Value::String(url.to_string()));
            if typ == "sse" {
                out.insert("transport".to_string(), Value::String("sse".to_string()));
            }
            if let Some(headers) = obj_as_string_map(obj.get("headers")) {
                out.insert("headers".to_string(), Value::Object(headers));
            }
        }
        _ => {
            return Err(mcp_invalid_input(format!(
                "OpenCode conversion: unsupported MCP type '{typ}'"
            )));
        }
    }

    out.insert("enabled".to_string(), Value::Bool(true));

    Ok(Value::Object(out))
}

fn json_to_toml_value(value: &Value) -> Option<toml::Value> {
    match value {
        Value::Null => None,
        Value::Bool(v) => Some(toml::Value::Boolean(*v)),
        Value::Number(v) => {
            if let Some(i) = v.as_i64() {
                Some(toml::Value::Integer(i))
            } else {
                v.as_f64().map(toml::Value::Float)
            }
        }
        Value::String(v) => Some(toml::Value::String(v.clone())),
        Value::Array(values) => {
            let mut converted = Vec::with_capacity(values.len());
            for item in values {
                let next = json_to_toml_value(item)?;
                converted.push(next);
            }
            Some(toml::Value::Array(converted))
        }
        Value::Object(map) => {
            let mut table = toml::map::Map::new();
            for (key, val) in map {
                let Some(next) = json_to_toml_value(val) else {
                    continue;
                };
                table.insert(key.clone(), next);
            }
            Some(toml::Value::Table(table))
        }
    }
}

fn toml_to_json_value(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(v) => Value::String(v.clone()),
        toml::Value::Integer(v) => Value::Number((*v).into()),
        toml::Value::Float(v) => serde_json::Number::from_f64(*v)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(v) => Value::Bool(*v),
        toml::Value::Datetime(v) => Value::String(v.to_string()),
        toml::Value::Array(values) => Value::Array(values.iter().map(toml_to_json_value).collect()),
        toml::Value::Table(table) => {
            let mut out = Map::new();
            for (key, item) in table {
                out.insert(key.to_string(), toml_to_json_value(item));
            }
            Value::Object(out)
        }
    }
}

fn codex_entry_to_canonical(id: &str, value: &toml::Value) -> Result<Value, AppCommandError> {
    let table = value
        .as_table()
        .ok_or_else(|| mcp_invalid_input(format!("Codex MCP entry '{id}' must be a table")))?;

    // Codex's native `[mcp_servers.*]` tables carry no `type` key — the transport
    // is implied by the keys present (`command` = stdio, `url` = streamable HTTP).
    // Honor an explicit `type` when present (older codeg output or hand-written
    // configs), but when it is absent infer the transport from the keys rather
    // than blindly assuming stdio, which would drop every url-only HTTP server
    // (including the ones codeg now writes). See issue #325.
    let raw_type = table
        .get("type")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let has_key = |key: &str| {
        table
            .get(key)
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    };
    // Codex hard-errors on an entry that carries BOTH `command` and `url` (mixed
    // transports). Reject it here rather than silently classifying it as stdio and
    // dropping `url` — which would both misrepresent the entry and let a later save
    // erase the conflicting field. Presence (not just non-empty) mirrors Codex's
    // own `throw_if_set` check. See issue #325.
    if table.contains_key("command") && table.contains_key("url") {
        return Err(mcp_invalid_input(format!(
            "Codex MCP entry '{id}' sets both 'command' and 'url'; Codex accepts exactly one transport"
        )));
    }
    let canonical_type = match raw_type.as_deref() {
        Some(raw) => normalize_mcp_type(raw).ok_or_else(|| {
            mcp_invalid_input(format!(
                "Codex MCP entry '{id}' has unsupported type '{raw}'"
            ))
            .with_i18n(
                "errors.codexEntryUnsupportedType",
                mcp_i18n_params([("id", id), ("type", raw)]),
            )
        })?,
        // No `command` and no `url` falls back to stdio so the downstream
        // canonicalize surfaces a clear "missing command" error.
        None if has_key("url") && !has_key("command") => "http",
        None => "stdio",
    };

    let mut spec = Map::new();
    spec.insert(
        "type".to_string(),
        Value::String(canonical_type.to_string()),
    );

    match canonical_type {
        "stdio" => {
            if let Some(command) = table
                .get("command")
                .and_then(toml::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                spec.insert("command".to_string(), Value::String(command.to_string()));
            }

            if let Some(args) = table.get("args").and_then(toml::Value::as_array) {
                let values = args
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| Value::String(value.to_string()))
                    .collect::<Vec<_>>();
                if !values.is_empty() {
                    spec.insert("args".to_string(), Value::Array(values));
                }
            }

            if let Some(env) = table.get("env").and_then(toml::Value::as_table) {
                let mut env_map = Map::new();
                for (key, value) in env {
                    let Some(text) = value.as_str() else {
                        continue;
                    };
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    env_map.insert(key.to_string(), Value::String(trimmed.to_string()));
                }
                if !env_map.is_empty() {
                    spec.insert("env".to_string(), Value::Object(env_map));
                }
            }

            if let Some(cwd) = table
                .get("cwd")
                .and_then(toml::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                spec.insert("cwd".to_string(), Value::String(cwd.to_string()));
            }
        }
        "http" | "sse" => {
            if let Some(url) = table
                .get("url")
                .and_then(toml::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                spec.insert("url".to_string(), Value::String(url.to_string()));
            }

            let headers_table = table
                .get("http_headers")
                .and_then(toml::Value::as_table)
                .or_else(|| table.get("headers").and_then(toml::Value::as_table));

            if let Some(headers) = headers_table {
                let mut mapped = Map::new();
                for (key, value) in headers {
                    let Some(text) = value.as_str() else {
                        continue;
                    };
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    mapped.insert(key.to_string(), Value::String(trimmed.to_string()));
                }
                if !mapped.is_empty() {
                    spec.insert("headers".to_string(), Value::Object(mapped));
                }
            }
        }
        _ => {
            // Reachable only when an explicit `type` normalized to an OpenCode-only
            // alias (`local`/`remote`), which Codex TOML does not accept.
            let raw = raw_type.as_deref().unwrap_or(canonical_type);
            return Err(mcp_invalid_input(format!(
                "Codex MCP entry '{id}' has unsupported type '{raw}'"
            ))
            .with_i18n(
                "errors.codexEntryUnsupportedType",
                mcp_i18n_params([("id", id), ("type", raw)]),
            ));
        }
    }

    for (key, value) in table {
        if key == "type"
            || key == "command"
            || key == "args"
            || key == "env"
            || key == "cwd"
            || key == "url"
            || key == "headers"
            || key == "http_headers"
        {
            continue;
        }
        spec.insert(key.to_string(), toml_to_json_value(value));
    }

    canonicalize_spec(&Value::Object(spec), "Codex config")
}

fn canonical_to_codex_entry(spec: &Value) -> Result<toml::Value, AppCommandError> {
    let canonical = canonicalize_spec(spec, "Codex conversion")?;
    let obj = canonical
        .as_object()
        .ok_or_else(|| mcp_invalid_input("Codex conversion: canonical spec must be an object"))?;

    let typ = obj.get("type").and_then(Value::as_str).unwrap_or("stdio");

    // Codex's config.toml has NO `type` field under `[mcp_servers.*]`: it infers
    // the transport from the keys present — `command` = stdio, `url` = streamable
    // HTTP. An emitted `type` is silently ignored on Codex's default read path but
    // is schema-invalid (Codex's generated JSON-Schema rejects it) and FATAL under
    // `codex --strict-config`, so the `type` discriminator is used only to branch
    // here and is never written out. Same hazard for any other foreign key (see the
    // allowlist below). See issue #325.
    let mut table = toml::map::Map::new();

    match typ {
        "stdio" => {
            let command = obj.get("command").and_then(Value::as_str).ok_or_else(|| {
                mcp_invalid_input("Codex conversion: stdio MCP spec missing command")
            })?;
            table.insert(
                "command".to_string(),
                toml::Value::String(command.to_string()),
            );

            if let Some(args) = obj.get("args").and_then(Value::as_array) {
                let values = args
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| toml::Value::String(value.to_string()))
                    .collect::<Vec<_>>();
                if !values.is_empty() {
                    table.insert("args".to_string(), toml::Value::Array(values));
                }
            }

            if let Some(cwd) = obj
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                table.insert("cwd".to_string(), toml::Value::String(cwd.to_string()));
            }

            if let Some(env) = obj.get("env").and_then(Value::as_object) {
                let mut env_table = toml::map::Map::new();
                for (key, value) in env {
                    let Some(text) = value.as_str() else {
                        continue;
                    };
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    env_table.insert(key.to_string(), toml::Value::String(trimmed.to_string()));
                }
                if !env_table.is_empty() {
                    table.insert("env".to_string(), toml::Value::Table(env_table));
                }
            }
        }
        "http" => {
            // env intentionally not written for http: per ACP/MCP spec, env is
            // stdio-only; remote transports use headers. canonicalize_spec strips
            // env upstream too.
            let url = obj.get("url").and_then(Value::as_str).ok_or_else(|| {
                mcp_invalid_input("Codex conversion: remote MCP spec missing url")
            })?;
            table.insert("url".to_string(), toml::Value::String(url.to_string()));

            if let Some(headers) = obj.get("headers").and_then(Value::as_object) {
                let mut headers_table = toml::map::Map::new();
                for (key, value) in headers {
                    let Some(text) = value.as_str() else {
                        continue;
                    };
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    headers_table.insert(key.to_string(), toml::Value::String(trimmed.to_string()));
                }
                if !headers_table.is_empty() {
                    table.insert(
                        "http_headers".to_string(),
                        toml::Value::Table(headers_table),
                    );
                }
            }
        }
        "sse" => {
            // Codex's config.toml has only stdio and streamable-HTTP transports — it
            // cannot represent SSE. Reject rather than degrade to a bare `url`, which
            // Codex would load as HTTP and codeg would then read back as `http`,
            // silently reclassifying the shared canonical spec (and defeating the ACP
            // wire-path SSE capability gate). Batch callers preflight-exclude Codex
            // from an SSE server's targets (see `app_can_host_spec`); this is the
            // backstop for any direct caller. See issue #325.
            return Err(mcp_invalid_input(
                "Codex conversion: SSE MCP servers are not supported by Codex; use streamable HTTP",
            ));
        }
        _ => {
            return Err(mcp_invalid_input(format!(
                "Codex conversion: unsupported MCP type '{typ}'"
            )));
        }
    }

    // Pass through only Codex `RawMcpServerConfig` fields that are transport-agnostic
    // AND validated to have Codex's exact value type here. A field-name allowlist
    // alone is not enough: canonicalization preserves arbitrary values, so a
    // same-named foreign field of the wrong shape (e.g. `"enabled": "false"`, or a
    // number where Codex wants a bool) would be written to Codex TOML and fail strict
    // deserialization — the same class of bug as the `type` field. Transport-specific
    // or complex/uncertain fields (env_vars, auth, oauth, tools, bearer_token_env_var,
    // startup_timeout_*, name, …) are emitted by the transport arms where they belong
    // or intentionally NOT round-tripped — a rare, non-fatal loss versus a
    // `--strict-config` failure. See issue #325.
    for (key, value) in obj {
        let allowed = match key.as_str() {
            "enabled" | "required" => value.is_boolean(),
            _ => false,
        };
        if !allowed {
            continue;
        }
        if let Some(converted) = json_to_toml_value(value) {
            table.insert(key.to_string(), converted);
        }
    }

    Ok(toml::Value::Table(table))
}

fn read_claude_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    let path = claude_config_path();
    let root = read_json_file(&path)?;
    let mut out = BTreeMap::new();

    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
        return Ok(out);
    };

    for (id, spec) in servers {
        match canonicalize_spec(spec, "Claude config") {
            Ok(normalized) => {
                out.insert(id.to_string(), normalized);
            }
            Err(err) => {
                tracing::warn!("[MCP] skip invalid Claude MCP entry id={id}: {err}");
            }
        }
    }

    Ok(out)
}

fn upsert_claude_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    let path = claude_config_path();
    let mut root = read_json_file(&path)?;
    if !root.is_object() {
        root = json!({});
    }

    let canonical = canonicalize_spec(spec, "Claude write")?;

    let obj = root.as_object_mut().ok_or_else(|| {
        mcp_configuration_invalid(format!("invalid JSON root in {}", path.display()))
    })?;
    if !obj.get("mcpServers").map(Value::is_object).unwrap_or(false) {
        obj.insert("mcpServers".to_string(), Value::Object(Map::new()));
    }

    let map = obj
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            mcp_configuration_invalid(format!("invalid mcpServers in {}", path.display()))
        })?;
    map.insert(id.to_string(), canonical);

    write_json_file(&path, &root)?;
    enable_claude_local_plugin(id)
}

fn remove_claude_server(id: &str) -> Result<bool, AppCommandError> {
    let path = claude_config_path();
    if !path.exists() {
        // Even if `~/.claude.json` is missing, `enabledPlugins` could still
        // have a stale entry from a prior session — clean it up regardless
        // so the user doesn't end up with dangling activation markers.
        disable_claude_local_plugin(id)?;
        return Ok(false);
    }

    let mut root = read_json_file(&path)?;
    let Some(obj) = root.as_object_mut() else {
        disable_claude_local_plugin(id)?;
        return Ok(false);
    };
    let Some(servers) = obj.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        disable_claude_local_plugin(id)?;
        return Ok(false);
    };

    let removed = servers.remove(id).is_some();
    if removed {
        write_json_file(&path, &root)?;
    }
    disable_claude_local_plugin(id)?;
    Ok(removed)
}

/// Add `<id>@local: true` to `~/.claude/settings.json.enabledPlugins`. The
/// Claude Code CLI uses this map as a gate for activating user-scope MCP
/// servers from `~/.claude.json.mcpServers` (a server can be defined but
/// will not load until it appears in this list). Existing fields in the
/// settings file (env, model, other plugin entries) are preserved.
fn enable_claude_local_plugin(id: &str) -> Result<(), AppCommandError> {
    let path = claude_settings_path();
    let mut root = read_json_file(&path)?;
    if !root.is_object() {
        root = json!({});
    }
    let obj = root.as_object_mut().ok_or_else(|| {
        mcp_configuration_invalid(format!("invalid JSON root in {}", path.display()))
    })?;
    if !obj
        .get("enabledPlugins")
        .map(Value::is_object)
        .unwrap_or(false)
    {
        obj.insert("enabledPlugins".to_string(), Value::Object(Map::new()));
    }
    let plugins = obj
        .get_mut("enabledPlugins")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            mcp_configuration_invalid(format!("invalid enabledPlugins in {}", path.display()))
        })?;
    let key = claude_local_plugin_key(id);
    let already_true = matches!(plugins.get(&key), Some(Value::Bool(true)));
    if already_true {
        // Avoid an unnecessary disk write that would needlessly trip the
        // settings-file watcher in claude-agent-acp's SettingsManager.
        return Ok(());
    }
    plugins.insert(key, Value::Bool(true));
    write_json_file(&path, &root)
}

/// Remove `<id>@local` from `~/.claude/settings.json.enabledPlugins` if
/// present. Other entries (including any `<id>@<other-marketplace>` that
/// the user manages manually) are intentionally left untouched.
fn disable_claude_local_plugin(id: &str) -> Result<(), AppCommandError> {
    let path = claude_settings_path();
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_json_file(&path)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(());
    };
    let Some(plugins) = obj.get_mut("enabledPlugins").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let key = claude_local_plugin_key(id);
    if plugins.remove(&key).is_some() {
        write_json_file(&path, &root)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CodeBuddy  (~/.codebuddy.json  →  mcpServers)
//
// CodeBuddy is a Claude Code derivative and shares its on-disk MCP layout:
// user-scope servers live in `~/.codebuddy.json.mcpServers`, gated for
// activation by `<id>@local: true` in
// `~/.codebuddy/settings.json.enabledPlugins`. These mirror the Claude helpers,
// only pointed at CodeBuddy's files.
// ---------------------------------------------------------------------------

fn codebuddy_config_path() -> PathBuf {
    home_dir_or_default().join(".codebuddy.json")
}

fn codebuddy_settings_path() -> PathBuf {
    home_dir_or_default().join(".codebuddy").join("settings.json")
}

fn read_codebuddy_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    let path = codebuddy_config_path();
    let root = read_json_file(&path)?;
    let mut out = BTreeMap::new();

    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
        return Ok(out);
    };

    for (id, spec) in servers {
        match canonicalize_spec(spec, "CodeBuddy config") {
            Ok(normalized) => {
                out.insert(id.to_string(), normalized);
            }
            Err(err) => {
                eprintln!("[MCP] skip invalid CodeBuddy MCP entry id={id}: {err}");
            }
        }
    }

    Ok(out)
}

fn upsert_codebuddy_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    let path = codebuddy_config_path();
    let mut root = read_json_file(&path)?;
    if !root.is_object() {
        root = json!({});
    }

    let canonical = canonicalize_spec(spec, "CodeBuddy write")?;

    let obj = root.as_object_mut().ok_or_else(|| {
        mcp_configuration_invalid(format!("invalid JSON root in {}", path.display()))
    })?;
    if !obj.get("mcpServers").map(Value::is_object).unwrap_or(false) {
        obj.insert("mcpServers".to_string(), Value::Object(Map::new()));
    }

    let map = obj
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            mcp_configuration_invalid(format!("invalid mcpServers in {}", path.display()))
        })?;
    map.insert(id.to_string(), canonical);

    write_json_file(&path, &root)?;
    enable_codebuddy_local_plugin(id)
}

fn remove_codebuddy_server(id: &str) -> Result<bool, AppCommandError> {
    let path = codebuddy_config_path();
    if !path.exists() {
        disable_codebuddy_local_plugin(id)?;
        return Ok(false);
    }

    let mut root = read_json_file(&path)?;
    let Some(obj) = root.as_object_mut() else {
        disable_codebuddy_local_plugin(id)?;
        return Ok(false);
    };
    let Some(servers) = obj.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        disable_codebuddy_local_plugin(id)?;
        return Ok(false);
    };

    let removed = servers.remove(id).is_some();
    if removed {
        write_json_file(&path, &root)?;
    }
    disable_codebuddy_local_plugin(id)?;
    Ok(removed)
}

/// Add `<id>@local: true` to `~/.codebuddy/settings.json.enabledPlugins`,
/// mirroring the Claude Code plugin-activation gate that CodeBuddy inherits.
fn enable_codebuddy_local_plugin(id: &str) -> Result<(), AppCommandError> {
    let path = codebuddy_settings_path();
    let mut root = read_json_file(&path)?;
    if !root.is_object() {
        root = json!({});
    }
    let obj = root.as_object_mut().ok_or_else(|| {
        mcp_configuration_invalid(format!("invalid JSON root in {}", path.display()))
    })?;
    if !obj
        .get("enabledPlugins")
        .map(Value::is_object)
        .unwrap_or(false)
    {
        obj.insert("enabledPlugins".to_string(), Value::Object(Map::new()));
    }
    let plugins = obj
        .get_mut("enabledPlugins")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            mcp_configuration_invalid(format!("invalid enabledPlugins in {}", path.display()))
        })?;
    let key = claude_local_plugin_key(id);
    if matches!(plugins.get(&key), Some(Value::Bool(true))) {
        return Ok(());
    }
    plugins.insert(key, Value::Bool(true));
    write_json_file(&path, &root)
}

/// Remove `<id>@local` from `~/.codebuddy/settings.json.enabledPlugins` if
/// present. Other entries are intentionally left untouched.
fn disable_codebuddy_local_plugin(id: &str) -> Result<(), AppCommandError> {
    let path = codebuddy_settings_path();
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_json_file(&path)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(());
    };
    let Some(plugins) = obj.get_mut("enabledPlugins").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let key = claude_local_plugin_key(id);
    if plugins.remove(&key).is_some() {
        write_json_file(&path, &root)?;
    }
    Ok(())
}

fn read_codex_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    let root = read_codex_root_toml()?;
    let Some(table) = root.as_table() else {
        return Ok(BTreeMap::new());
    };

    let mut out = BTreeMap::new();

    if let Some(current) = table.get("mcp_servers").and_then(toml::Value::as_table) {
        for (id, spec) in current {
            match codex_entry_to_canonical(id, spec) {
                Ok(normalized) => {
                    out.insert(id.to_string(), normalized);
                }
                Err(err) => {
                    tracing::warn!("[MCP] skip invalid Codex mcp_servers entry id={id}: {err}");
                }
            }
        }
    }

    if let Some(legacy_mcp) = table.get("mcp").and_then(toml::Value::as_table) {
        if let Some(legacy_servers) = legacy_mcp.get("servers").and_then(toml::Value::as_table) {
            for (id, spec) in legacy_servers {
                if out.contains_key(id) {
                    continue;
                }
                match codex_entry_to_canonical(id, spec) {
                    Ok(normalized) => {
                        out.insert(id.to_string(), normalized);
                    }
                    Err(err) => {
                        tracing::warn!("[MCP] skip invalid Codex mcp.servers entry id={id}: {err}");
                    }
                }
            }
        }
    }

    Ok(out)
}

fn upsert_codex_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    let mut root = read_codex_root_toml()?;
    let table = root
        .as_table_mut()
        .ok_or_else(|| mcp_configuration_invalid("Codex root TOML must be a table"))?;

    let codex_entry = canonical_to_codex_entry(spec)?;

    if !table
        .get("mcp_servers")
        .map(toml::Value::is_table)
        .unwrap_or(false)
    {
        table.insert(
            "mcp_servers".to_string(),
            toml::Value::Table(toml::map::Map::new()),
        );
    }

    let mcp_servers = table
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| mcp_configuration_invalid("Codex mcp_servers must be a TOML table"))?;
    mcp_servers.insert(id.to_string(), codex_entry);

    if let Some(legacy_mcp) = table.get_mut("mcp").and_then(toml::Value::as_table_mut) {
        if let Some(legacy_servers) = legacy_mcp
            .get_mut("servers")
            .and_then(toml::Value::as_table_mut)
        {
            legacy_servers.remove(id);
            if legacy_servers.is_empty() {
                legacy_mcp.remove("servers");
            }
        }
        if legacy_mcp.is_empty() {
            table.remove("mcp");
        }
    }

    write_codex_root_toml(&root)
}

fn remove_codex_server(id: &str) -> Result<bool, AppCommandError> {
    let path = codex_config_toml_path();
    if !path.exists() {
        return Ok(false);
    }

    let mut root = read_codex_root_toml()?;
    let Some(table) = root.as_table_mut() else {
        return Ok(false);
    };

    let mut removed = false;

    if let Some(mcp_servers) = table
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
    {
        removed |= mcp_servers.remove(id).is_some();
        if mcp_servers.is_empty() {
            table.remove("mcp_servers");
        }
    }

    if let Some(legacy_mcp) = table.get_mut("mcp").and_then(toml::Value::as_table_mut) {
        if let Some(legacy_servers) = legacy_mcp
            .get_mut("servers")
            .and_then(toml::Value::as_table_mut)
        {
            removed |= legacy_servers.remove(id).is_some();
            if legacy_servers.is_empty() {
                legacy_mcp.remove("servers");
            }
        }
        if legacy_mcp.is_empty() {
            table.remove("mcp");
        }
    }

    if removed {
        write_codex_root_toml(&root)?;
    }

    Ok(removed)
}

fn read_opencode_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    let path = opencode_config_path();
    let root = read_json_file(&path)?;

    let mut out = BTreeMap::new();

    if let Some(servers) = root.get("mcpServers").and_then(Value::as_object) {
        for (id, spec) in servers {
            match canonicalize_spec(spec, "OpenCode mcpServers") {
                Ok(normalized) => {
                    out.insert(id.to_string(), normalized);
                }
                Err(err) => {
                    tracing::warn!("[MCP] skip invalid OpenCode mcpServers entry id={id}: {err}");
                }
            }
        }
    }

    if let Some(servers) = root.get("mcp").and_then(Value::as_object) {
        for (id, spec) in servers {
            if out.contains_key(id) {
                continue;
            }
            match canonicalize_opencode_spec(spec, "OpenCode mcp") {
                Ok(normalized) => {
                    out.insert(id.to_string(), normalized);
                }
                Err(err) => {
                    tracing::warn!("[MCP] skip invalid OpenCode mcp entry id={id}: {err}");
                }
            }
        }
    }

    Ok(out)
}

fn upsert_opencode_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    let path = opencode_config_path();
    let mut root = read_json_file(&path)?;
    if !root.is_object() {
        root = json!({});
    }

    let obj = root.as_object_mut().ok_or_else(|| {
        mcp_configuration_invalid(format!("invalid JSON root in {}", path.display()))
    })?;

    if obj.get("mcpServers").map(Value::is_object).unwrap_or(false) {
        let canonical = canonicalize_spec(spec, "OpenCode write mcpServers")?;
        let map = obj
            .get_mut("mcpServers")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                mcp_configuration_invalid(format!("invalid mcpServers in {}", path.display()))
            })?;
        map.insert(id.to_string(), canonical);
    } else {
        if !obj.get("mcp").map(Value::is_object).unwrap_or(false) {
            obj.insert("mcp".to_string(), Value::Object(Map::new()));
        }
        let converted = canonical_to_opencode_spec(spec)?;
        let map = obj
            .get_mut("mcp")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                mcp_configuration_invalid(format!("invalid mcp in {}", path.display()))
            })?;
        map.insert(id.to_string(), converted);
    }

    write_json_file(&path, &root)
}

fn remove_opencode_server(id: &str) -> Result<bool, AppCommandError> {
    let path = opencode_config_path();
    if !path.exists() {
        return Ok(false);
    }

    let mut root = read_json_file(&path)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(false);
    };

    let mut removed = false;

    if let Some(servers) = obj.get_mut("mcpServers").and_then(Value::as_object_mut) {
        removed |= servers.remove(id).is_some();
    }

    if let Some(servers) = obj.get_mut("mcp").and_then(Value::as_object_mut) {
        removed |= servers.remove(id).is_some();
    }

    if removed {
        write_json_file(&path, &root)?;
    }

    Ok(removed)
}

// ---------------------------------------------------------------------------
// Gemini CLI  (~/.gemini/settings.json  →  mcpServers)
// ---------------------------------------------------------------------------

fn read_gemini_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    let path = gemini_config_path();
    let root = read_json_file(&path)?;
    let mut out = BTreeMap::new();

    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
        return Ok(out);
    };

    for (id, spec) in servers {
        match canonicalize_spec(spec, "Gemini config") {
            Ok(normalized) => {
                out.insert(id.to_string(), normalized);
            }
            Err(err) => {
                tracing::warn!("[MCP] skip invalid Gemini MCP entry id={id}: {err}");
            }
        }
    }

    Ok(out)
}

fn upsert_gemini_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    let path = gemini_config_path();
    let mut root = read_json_file(&path)?;
    if !root.is_object() {
        root = json!({});
    }

    let canonical = canonicalize_spec(spec, "Gemini write")?;

    let obj = root.as_object_mut().ok_or_else(|| {
        mcp_configuration_invalid(format!("invalid JSON root in {}", path.display()))
    })?;
    if !obj.get("mcpServers").map(Value::is_object).unwrap_or(false) {
        obj.insert("mcpServers".to_string(), Value::Object(Map::new()));
    }

    let map = obj
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            mcp_configuration_invalid(format!("invalid mcpServers in {}", path.display()))
        })?;
    map.insert(id.to_string(), canonical);

    write_json_file(&path, &root)
}

fn remove_gemini_server(id: &str) -> Result<bool, AppCommandError> {
    let path = gemini_config_path();
    if !path.exists() {
        return Ok(false);
    }

    let mut root = read_json_file(&path)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(false);
    };
    let Some(servers) = obj.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(false);
    };

    let removed = servers.remove(id).is_some();
    if removed {
        write_json_file(&path, &root)?;
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// OpenClaw  (~/.openclaw/openclaw.json  →  mcp.servers)
// ---------------------------------------------------------------------------

fn read_openclaw_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    let path = openclaw_config_path();
    let root = read_json_file(&path)?;
    let mut out = BTreeMap::new();

    let Some(mcp) = root.get("mcp").and_then(Value::as_object) else {
        return Ok(out);
    };
    let Some(servers) = mcp.get("servers").and_then(Value::as_object) else {
        return Ok(out);
    };

    for (id, spec) in servers {
        match canonicalize_spec(spec, "OpenClaw config") {
            Ok(normalized) => {
                out.insert(id.to_string(), normalized);
            }
            Err(err) => {
                tracing::warn!("[MCP] skip invalid OpenClaw MCP entry id={id}: {err}");
            }
        }
    }

    Ok(out)
}

fn upsert_openclaw_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    let path = openclaw_config_path();
    let mut root = read_json_file(&path)?;
    if !root.is_object() {
        root = json!({});
    }

    let canonical = canonicalize_spec(spec, "OpenClaw write")?;

    let obj = root.as_object_mut().ok_or_else(|| {
        mcp_configuration_invalid(format!("invalid JSON root in {}", path.display()))
    })?;

    if !obj.get("mcp").map(Value::is_object).unwrap_or(false) {
        obj.insert("mcp".to_string(), json!({}));
    }
    let mcp = obj
        .get_mut("mcp")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| mcp_configuration_invalid(format!("invalid mcp in {}", path.display())))?;

    if !mcp.get("servers").map(Value::is_object).unwrap_or(false) {
        mcp.insert("servers".to_string(), Value::Object(Map::new()));
    }
    let servers = mcp
        .get_mut("servers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            mcp_configuration_invalid(format!("invalid mcp.servers in {}", path.display()))
        })?;
    servers.insert(id.to_string(), canonical);

    write_json_file(&path, &root)
}

fn remove_openclaw_server(id: &str) -> Result<bool, AppCommandError> {
    let path = openclaw_config_path();
    if !path.exists() {
        return Ok(false);
    }

    let mut root = read_json_file(&path)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(false);
    };
    let Some(mcp) = obj.get_mut("mcp").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    let Some(servers) = mcp.get_mut("servers").and_then(Value::as_object_mut) else {
        return Ok(false);
    };

    let removed = servers.remove(id).is_some();
    if removed {
        if servers.is_empty() {
            mcp.remove("servers");
        }
        if mcp.is_empty() {
            obj.remove("mcp");
        }
        write_json_file(&path, &root)?;
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Cline  (~/.cline/data/settings/cline_mcp_settings.json  →  mcpServers)
// ---------------------------------------------------------------------------

fn read_cline_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    let path = cline_config_path();
    let root = read_json_file(&path)?;
    let mut out = BTreeMap::new();

    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
        return Ok(out);
    };

    for (id, spec) in servers {
        match canonicalize_spec(spec, "Cline config") {
            Ok(normalized) => {
                out.insert(id.to_string(), normalized);
            }
            Err(err) => {
                tracing::warn!("[MCP] skip invalid Cline MCP entry id={id}: {err}");
            }
        }
    }

    Ok(out)
}

/// Convert codeg's canonical spec into a Cline `mcpServers` entry.
///
/// Cline validates each entry with a zod union whose `type` is a literal enum of
/// exactly `stdio | sse | streamableHttp` — it does NOT accept the canonical
/// `http`. Worse, `mcpServers` is validated as one `z.record`, so a single
/// rejected entry makes Cline load *zero* servers. Remap `http` → `streamableHttp`
/// (which codeg's reader collapses straight back to canonical `http` via
/// `normalize_mcp_type`); stdio/sse already match Cline's literals and pass
/// through untouched. See issue #325.
fn canonical_to_cline_entry(spec: &Value) -> Result<Value, AppCommandError> {
    let mut canonical = canonicalize_spec(spec, "Cline write")?;
    if let Some(obj) = canonical.as_object_mut() {
        if obj.get("type").and_then(Value::as_str) == Some("http") {
            obj.insert(
                "type".to_string(),
                Value::String("streamableHttp".to_string()),
            );
        }
    }
    Ok(canonical)
}

fn upsert_cline_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    let path = cline_config_path();
    let mut root = read_json_file(&path)?;
    if !root.is_object() {
        root = json!({});
    }

    let canonical = canonical_to_cline_entry(spec)?;

    let obj = root.as_object_mut().ok_or_else(|| {
        mcp_configuration_invalid(format!("invalid JSON root in {}", path.display()))
    })?;
    if !obj.get("mcpServers").map(Value::is_object).unwrap_or(false) {
        obj.insert("mcpServers".to_string(), Value::Object(Map::new()));
    }

    let map = obj
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            mcp_configuration_invalid(format!("invalid mcpServers in {}", path.display()))
        })?;
    map.insert(id.to_string(), canonical);

    write_json_file(&path, &root)
}

fn remove_cline_server(id: &str) -> Result<bool, AppCommandError> {
    let path = cline_config_path();
    if !path.exists() {
        return Ok(false);
    }

    let mut root = read_json_file(&path)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(false);
    };
    let Some(servers) = obj.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(false);
    };

    let removed = servers.remove(id).is_some();
    if removed {
        write_json_file(&path, &root)?;
    }
    Ok(removed)
}

fn scan_local_servers() -> Result<Vec<LocalMcpServer>, AppCommandError> {
    let mut merged: BTreeMap<String, (Value, BTreeSet<McpAppType>)> = BTreeMap::new();

    for (id, spec) in read_claude_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::ClaudeCode);
    }

    for (id, spec) in read_codex_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::Codex);
    }

    for (id, spec) in read_opencode_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::OpenCode);
    }

    for (id, spec) in read_gemini_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::Gemini);
    }

    for (id, spec) in read_openclaw_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::OpenClaw);
    }

    for (id, spec) in read_cline_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::Cline);
    }

    for (id, spec) in read_hermes_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::Hermes);
    }

    for (id, spec) in read_codebuddy_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::CodeBuddy);
    }

    for (id, spec) in read_kimi_code_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::KimiCode);
    }

    for (id, spec) in read_grok_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::Grok);
    }

    for (id, spec) in read_cursor_servers()? {
        let entry = merged
            .entry(id)
            .or_insert_with(|| (spec.clone(), BTreeSet::new()));
        entry.1.insert(McpAppType::Cursor);
    }

    Ok(merged
        .into_iter()
        .map(|(id, (spec, apps))| LocalMcpServer {
            id,
            spec,
            apps: apps.into_iter().collect(),
        })
        .collect())
}

fn find_local_server(server_id: &str) -> Result<Option<LocalMcpServer>, AppCommandError> {
    let servers = scan_local_servers()?;
    Ok(servers.into_iter().find(|item| item.id == server_id))
}

fn upsert_server_for_app(app: McpAppType, id: &str, spec: &Value) -> Result<(), AppCommandError> {
    match app {
        McpAppType::ClaudeCode => upsert_claude_server(id, spec),
        McpAppType::Codex => upsert_codex_server(id, spec),
        McpAppType::OpenCode => upsert_opencode_server(id, spec),
        McpAppType::Gemini => upsert_gemini_server(id, spec),
        McpAppType::OpenClaw => upsert_openclaw_server(id, spec),
        McpAppType::Cline => upsert_cline_server(id, spec),
        McpAppType::Hermes => upsert_hermes_server(id, spec),
        McpAppType::CodeBuddy => upsert_codebuddy_server(id, spec),
        McpAppType::KimiCode => upsert_kimi_code_server(id, spec),
        McpAppType::Grok => upsert_grok_server(id, spec),
        McpAppType::Cursor => upsert_cursor_server(id, spec),
    }
}

pub fn read_servers_for_agent_type(
    agent_type: crate::models::agent::AgentType,
) -> Result<BTreeMap<String, Value>, AppCommandError> {
    use crate::models::agent::AgentType;
    match agent_type {
        AgentType::ClaudeCode => read_claude_servers(),
        AgentType::Codex => read_codex_servers(),
        AgentType::OpenCode => read_opencode_servers(),
        AgentType::Gemini => read_gemini_servers(),
        AgentType::OpenClaw => read_openclaw_servers(),
        AgentType::Cline => read_cline_servers(),
        AgentType::Hermes => read_hermes_servers(),
        AgentType::CodeBuddy => read_codebuddy_servers(),
        AgentType::KimiCode => read_kimi_code_servers(),
        AgentType::Grok => read_grok_servers(),
        AgentType::Cursor => read_cursor_servers(),
        // pi-acp drops ACP-wire MCP and pi has no native MCP (it needs a
        // third-party extension), so codeg manages no MCP servers for pi (v1).
        AgentType::Pi => Ok(BTreeMap::new()),
    }
}

// ---------------------------------------------------------------------------
// Kimi Code  (~/.kimi-code/mcp.json  →  top-level `mcpServers`)
//
// Kimi reads its user-global MCP config from `<KIMI_CODE_HOME>/mcp.json`
// (default `~/.kimi-code/mcp.json`) — a JSON file with a top-level `mcpServers`
// object of Claude-shaped entries (`command`/`args`/`env`/`cwd`, or `url` for
// http/sse). This mirrors CodeBuddy/Cline's JSON layout (NOT Codex's TOML).
//
// Because Kimi loads this file natively at session start, `KimiCode` is on the
// ACP forward skip list in `connection.rs` (like Hermes) so the same user
// servers aren't double-registered over `session/new`. The built-in `codeg-mcp`
// companion is injected separately by `inject_codeg_mcp`, so it still reaches
// Kimi regardless.
// ---------------------------------------------------------------------------

fn kimi_code_mcp_json_path() -> PathBuf {
    crate::parsers::kimi_code::resolve_kimi_code_home_dir().join("mcp.json")
}

fn read_kimi_code_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    read_kimi_code_servers_at(&kimi_code_mcp_json_path())
}

/// Convert one Kimi `mcpServers` entry into codeg's canonical spec.
///
/// Kimi Code 0.23.3 validates `mcp.json` with a Zod discriminated union keyed on
/// `transport` (`stdio`/`http`/`sse`): `command` ⇒ stdio, and a url-only remote
/// entry DEFAULTS to streamable HTTP — it never infers SSE from the URL path, and
/// `type` is not a recognized field (silently stripped). Mirror that so codeg
/// classifies an entry the way Kimi actually will: stdio from `command`; otherwise
/// a `url` is remote with transport taken from an explicit `transport` key (only
/// `sse` yields SSE), else HTTP. `type` is intentionally NOT consulted for remote
/// (Kimi ignores it). `transport` is then dropped from the canonical spec so it
/// can't leak into another agent's config on a cross-agent sync. See issue #325.
fn kimi_code_entry_to_canonical(spec: &Value, id: &str) -> Result<Value, AppCommandError> {
    let Some(obj) = spec.as_object() else {
        return canonicalize_spec(spec, "Kimi Code config");
    };
    let mut obj = obj.clone();
    // Kimi 0.23.3 keys the transport off the `transport` DISCRIMINANT whenever it is
    // present (exact literals `stdio`/`http`/`sse`, and it overrides `command`/`url`
    // shape); only when `transport` is ABSENT does it infer (`command` ⇒ stdio,
    // `url` ⇒ http). Crucially, Kimi never consults `type` — it strips it — so drop
    // any on-disk `type` up front: an explicit `transport` sets the canonical type
    // below, and an absent one leaves classification to canonicalize's own
    // command⇒stdio / url⇒http inference (matching Kimi) rather than a stale `type`.
    // `transport` is likewise dropped after mapping so it can't leak into another
    // agent's config on a cross-agent sync. See issue #325.
    obj.remove("type");
    // Read the discriminant into an owned value first so the map isn't borrowed when
    // we mutate it below. `transport` absent ⇒ infer; present-but-non-string or an
    // unknown literal ⇒ reject (as Kimi would).
    let explicit_transport = obj.get("transport").and_then(Value::as_str).map(str::to_string);
    if obj.contains_key("transport") {
        let canonical_type = match explicit_transport.as_deref() {
            Some("stdio") => "stdio",
            Some("http") => "http",
            Some("sse") => "sse",
            other => {
                let shown = other.unwrap_or("<non-string>");
                return Err(mcp_invalid_input(format!(
                    "Kimi Code config '{id}': unsupported transport '{shown}' (Kimi accepts only \"stdio\", \"http\", or \"sse\")"
                )));
            }
        };
        obj.insert("type".to_string(), Value::String(canonical_type.to_string()));
    }
    obj.remove("transport");
    canonicalize_spec(&Value::Object(obj), &format!("Kimi Code config '{id}'"))
}

fn read_kimi_code_servers_at(path: &Path) -> Result<BTreeMap<String, Value>, AppCommandError> {
    let root = read_json_file(path)?;
    let mut out = BTreeMap::new();

    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
        return Ok(out);
    };

    for (id, spec) in servers {
        match kimi_code_entry_to_canonical(spec, id) {
            Ok(normalized) => {
                out.insert(id.to_string(), normalized);
            }
            Err(err) => {
                eprintln!("[MCP] skip invalid Kimi Code MCP entry id={id}: {err}");
            }
        }
    }

    Ok(out)
}

/// Convert codeg's canonical spec into a Kimi `mcpServers` entry.
///
/// Kimi Code 0.23.3 keys the transport off a `transport` field (Zod
/// discriminated union), defaulting a url-only remote entry to streamable HTTP — an
/// SSE server MUST carry an explicit `transport: "sse"` or it silently downgrades to
/// HTTP. So emit `transport` for remote entries. The streamable-HTTP literal is
/// `"http"` (NOT `"streamable-http"`, which Kimi rejects — and one bad entry fails
/// the whole `mcpServers` record). stdio needs no `transport` (Kimi injects it from
/// `command`). The canonical `type` is left in place but Kimi ignores/strips it.
/// See issue #325.
fn canonical_to_kimi_code_entry(spec: &Value) -> Result<Value, AppCommandError> {
    let canonical = canonicalize_spec(spec, "Kimi Code write")?;
    let Some(obj) = canonical.as_object() else {
        return Ok(canonical);
    };
    let transport = match obj.get("type").and_then(Value::as_str) {
        Some("http") => Some("http"),
        Some("sse") => Some("sse"),
        _ => None, // stdio: Kimi infers the transport from `command`
    };
    // Emit only the fields Kimi models, each validated to its expected type — the
    // same guard the Codex writer uses. Kimi validates its known fields and rejects
    // the ENTIRE `mcpServers` record on a wrong-typed one (e.g. `"enabled": "false"`),
    // so a stray same-named foreign value must not ride canonicalize's passthrough
    // onto disk. The canonical `command`/`args`/`env`/`cwd`/`url`/`headers` already
    // carry Kimi-compatible types; `type` is kept but Kimi ignores/strips it.
    // See issue #325.
    let mut out = Map::new();
    for (key, value) in obj {
        let keep = match key.as_str() {
            "type" | "command" | "args" | "env" | "cwd" | "url" | "headers" => true,
            "enabled" => value.is_boolean(),
            _ => false,
        };
        if keep {
            out.insert(key.clone(), value.clone());
        }
    }
    if let Some(transport) = transport {
        out.insert("transport".to_string(), Value::String(transport.to_string()));
    }
    Ok(Value::Object(out))
}

fn upsert_kimi_code_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    upsert_kimi_code_server_at(&kimi_code_mcp_json_path(), id, spec)
}

fn upsert_kimi_code_server_at(
    path: &Path,
    id: &str,
    spec: &Value,
) -> Result<(), AppCommandError> {
    let mut root = read_json_file(path)?;
    if !root.is_object() {
        root = json!({});
    }

    let canonical = canonical_to_kimi_code_entry(spec)?;

    let obj = root.as_object_mut().ok_or_else(|| {
        mcp_configuration_invalid(format!("invalid JSON root in {}", path.display()))
    })?;
    if !obj.get("mcpServers").map(Value::is_object).unwrap_or(false) {
        obj.insert("mcpServers".to_string(), Value::Object(Map::new()));
    }

    let map = obj
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            mcp_configuration_invalid(format!("invalid mcpServers in {}", path.display()))
        })?;
    map.insert(id.to_string(), canonical);

    write_json_file(path, &root)
}

fn remove_kimi_code_server(id: &str) -> Result<bool, AppCommandError> {
    remove_kimi_code_server_at(&kimi_code_mcp_json_path(), id)
}

fn remove_kimi_code_server_at(path: &Path, id: &str) -> Result<bool, AppCommandError> {
    if !path.exists() {
        return Ok(false);
    }

    let mut root = read_json_file(path)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(false);
    };
    let Some(servers) = obj.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(false);
    };

    let removed = servers.remove(id).is_some();
    if removed {
        write_json_file(path, &root)?;
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Grok  (~/.grok/config.toml  →  [mcp_servers.<name>])
//
// Grok reads its user-global MCP config from `<GROK_HOME>/config.toml` (default
// `~/.grok/config.toml`) under `[mcp_servers.<name>]` sections — the same TOML
// table Codex uses, but WITHOUT a `type` discriminator: Grok infers the
// transport from the presence of `command` (stdio) vs `url` (http/sse). The
// file also holds unrelated sections (`[cli]`, `[ui]`, `[model.*]`), so we
// read/modify/write the whole document and only touch `[mcp_servers]`.
//
// Because Grok loads this file natively at session start, `Grok` is on the ACP
// forward skip list in `connection.rs` (like Hermes/Kimi) so the same user
// servers aren't double-registered over `session/new`. The built-in `codeg-mcp`
// companion is injected separately by `inject_codeg_mcp`, so it still reaches
// Grok over the wire regardless.
// ---------------------------------------------------------------------------

fn grok_config_toml_path() -> PathBuf {
    crate::parsers::grok::resolve_grok_home_dir().join("config.toml")
}

fn read_grok_root_toml_at(path: &Path) -> Result<toml::Value, AppCommandError> {
    if !path.exists() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    let raw = fs::read_to_string(path).map_err(AppCommandError::io)?;
    let parsed = raw.parse::<toml::Value>().map_err(|e| {
        mcp_configuration_invalid(format!("invalid TOML at {}: {e}", path.display()))
    })?;
    if !parsed.is_table() {
        return Err(mcp_configuration_invalid(format!(
            "invalid TOML root at {}: expected table",
            path.display()
        )));
    }
    Ok(parsed)
}

fn write_grok_root_toml_at(path: &Path, root: &toml::Value) -> Result<(), AppCommandError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(AppCommandError::io)?;
    }
    let serialized = toml::to_string_pretty(root).map_err(|e| {
        mcp_configuration_invalid(format!("failed to serialize TOML for {}: {e}", path.display()))
    })?;
    fs::write(path, format!("{serialized}\n")).map_err(AppCommandError::io)
}

/// Canonical spec → a Grok `[mcp_servers.<name>]` TOML entry. Grok has no
/// `type` key (it infers transport from `command`/`url`), so we never write one;
/// unknown canonical keys (e.g. `enabled`, `startup_timeout_sec`) pass through.
fn canonical_to_grok_entry(spec: &Value) -> Result<toml::Value, AppCommandError> {
    let canonical = canonicalize_spec(spec, "Grok conversion")?;
    let obj = canonical
        .as_object()
        .ok_or_else(|| mcp_invalid_input("Grok conversion: canonical spec must be an object"))?;
    let typ = obj.get("type").and_then(Value::as_str).unwrap_or("stdio");

    let mut table = toml::map::Map::new();
    match typ {
        "stdio" => {
            let command = obj.get("command").and_then(Value::as_str).ok_or_else(|| {
                mcp_invalid_input("Grok conversion: stdio MCP spec missing command")
            })?;
            table.insert("command".to_string(), toml::Value::String(command.to_string()));
            if let Some(args) = obj.get("args").and_then(Value::as_array) {
                let values = args
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| toml::Value::String(value.to_string()))
                    .collect::<Vec<_>>();
                if !values.is_empty() {
                    table.insert("args".to_string(), toml::Value::Array(values));
                }
            }
            if let Some(env) = obj.get("env").and_then(Value::as_object) {
                let mut env_table = toml::map::Map::new();
                for (key, value) in env {
                    if let Some(text) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        env_table.insert(key.to_string(), toml::Value::String(text.to_string()));
                    }
                }
                if !env_table.is_empty() {
                    table.insert("env".to_string(), toml::Value::Table(env_table));
                }
            }
            if let Some(cwd) = obj
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                table.insert("cwd".to_string(), toml::Value::String(cwd.to_string()));
            }
        }
        "http" | "sse" => {
            // Grok infers `http` from a bare `url` and omits `type` for it, but
            // SSE must carry an explicit `type = "sse"` (verified against Grok's
            // CLI) — otherwise it round-trips back to `http` and loses the SSE
            // transport.
            if typ == "sse" {
                table.insert("type".to_string(), toml::Value::String("sse".to_string()));
            }
            let url = obj
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| mcp_invalid_input("Grok conversion: remote MCP spec missing url"))?;
            table.insert("url".to_string(), toml::Value::String(url.to_string()));
            if let Some(headers) = obj.get("headers").and_then(Value::as_object) {
                let mut headers_table = toml::map::Map::new();
                for (key, value) in headers {
                    if let Some(text) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        headers_table
                            .insert(key.to_string(), toml::Value::String(text.to_string()));
                    }
                }
                if !headers_table.is_empty() {
                    table.insert("headers".to_string(), toml::Value::Table(headers_table));
                }
            }
        }
        other => {
            return Err(mcp_invalid_input(format!(
                "Grok conversion: unsupported MCP type '{other}'"
            )));
        }
    }

    // Preserve any extra canonical keys (e.g. `enabled`, timeouts) except the
    // transport fields we already emitted and `type` (Grok has none).
    for (key, value) in obj {
        if matches!(
            key.as_str(),
            "type" | "command" | "args" | "env" | "cwd" | "url" | "headers"
        ) {
            continue;
        }
        if let Some(converted) = json_to_toml_value(value) {
            table.insert(key.to_string(), converted);
        }
    }

    Ok(toml::Value::Table(table))
}

/// A Grok `[mcp_servers.<name>]` TOML entry → canonical spec. Transport is
/// inferred: a `url` is http (unless SSE is explicit elsewhere), else stdio.
fn grok_entry_to_canonical(id: &str, value: &toml::Value) -> Result<Value, AppCommandError> {
    let table = value
        .as_table()
        .ok_or_else(|| mcp_invalid_input(format!("Grok MCP entry '{id}' must be a table")))?;

    let mut spec = Map::new();
    // Grok omits `type` for stdio and http (a bare `url` implies http), but
    // writes `type = "sse"` explicitly for SSE (verified against Grok's CLI).
    // Honor an explicit type; otherwise infer the transport from `url` presence.
    let explicit_type = table
        .get("type")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let has_url = table
        .get("url")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some();
    let is_remote = matches!(explicit_type, Some("http") | Some("sse"))
        || (has_url && explicit_type != Some("stdio"));

    if is_remote {
        let canonical_type = if explicit_type == Some("sse") { "sse" } else { "http" };
        spec.insert("type".to_string(), Value::String(canonical_type.to_string()));
        if let Some(url) = table.get("url").and_then(toml::Value::as_str) {
            spec.insert("url".to_string(), Value::String(url.trim().to_string()));
        }
        if let Some(headers) = table.get("headers").and_then(toml::Value::as_table) {
            let mut mapped = Map::new();
            for (key, value) in headers {
                if let Some(text) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    mapped.insert(key.to_string(), Value::String(text.to_string()));
                }
            }
            if !mapped.is_empty() {
                spec.insert("headers".to_string(), Value::Object(mapped));
            }
        }
    } else {
        spec.insert("type".to_string(), Value::String("stdio".to_string()));
        if let Some(command) = table
            .get("command")
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            spec.insert("command".to_string(), Value::String(command.to_string()));
        }
        if let Some(args) = table.get("args").and_then(toml::Value::as_array) {
            let values = args
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_string()))
                .collect::<Vec<_>>();
            if !values.is_empty() {
                spec.insert("args".to_string(), Value::Array(values));
            }
        }
        if let Some(env) = table.get("env").and_then(toml::Value::as_table) {
            let mut env_map = Map::new();
            for (key, value) in env {
                if let Some(text) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    env_map.insert(key.to_string(), Value::String(text.to_string()));
                }
            }
            if !env_map.is_empty() {
                spec.insert("env".to_string(), Value::Object(env_map));
            }
        }
        if let Some(cwd) = table
            .get("cwd")
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            spec.insert("cwd".to_string(), Value::String(cwd.to_string()));
        }
    }

    // Passthrough for any Grok-specific keys we don't model (enabled, timeouts).
    // `type` is handled explicitly above (transport inference), so skip it here.
    for (key, value) in table {
        if matches!(
            key.as_str(),
            "type" | "command" | "args" | "env" | "cwd" | "url" | "headers"
        ) {
            continue;
        }
        spec.insert(key.to_string(), toml_to_json_value(value));
    }

    canonicalize_spec(&Value::Object(spec), "Grok config")
}

fn read_grok_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    read_grok_servers_at(&grok_config_toml_path())
}

fn read_grok_servers_at(path: &Path) -> Result<BTreeMap<String, Value>, AppCommandError> {
    let root = read_grok_root_toml_at(path)?;
    let mut out = BTreeMap::new();
    let Some(table) = root.as_table() else {
        return Ok(out);
    };
    if let Some(servers) = table.get("mcp_servers").and_then(toml::Value::as_table) {
        for (id, spec) in servers {
            match grok_entry_to_canonical(id, spec) {
                Ok(normalized) => {
                    out.insert(id.to_string(), normalized);
                }
                Err(err) => {
                    tracing::warn!("[MCP] skip invalid Grok mcp_servers entry id={id}: {err}");
                }
            }
        }
    }
    Ok(out)
}

fn upsert_grok_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    upsert_grok_server_at(&grok_config_toml_path(), id, spec)
}

fn upsert_grok_server_at(path: &Path, id: &str, spec: &Value) -> Result<(), AppCommandError> {
    let mut root = read_grok_root_toml_at(path)?;
    let table = root
        .as_table_mut()
        .ok_or_else(|| mcp_configuration_invalid("Grok root TOML must be a table"))?;

    let entry = canonical_to_grok_entry(spec)?;
    if !table
        .get("mcp_servers")
        .map(toml::Value::is_table)
        .unwrap_or(false)
    {
        table.insert(
            "mcp_servers".to_string(),
            toml::Value::Table(toml::map::Map::new()),
        );
    }
    let mcp_servers = table
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| mcp_configuration_invalid("Grok mcp_servers must be a TOML table"))?;
    mcp_servers.insert(id.to_string(), entry);

    write_grok_root_toml_at(path, &root)
}

fn remove_grok_server(id: &str) -> Result<bool, AppCommandError> {
    remove_grok_server_at(&grok_config_toml_path(), id)
}

fn remove_grok_server_at(path: &Path, id: &str) -> Result<bool, AppCommandError> {
    if !path.exists() {
        return Ok(false);
    }
    let mut root = read_grok_root_toml_at(path)?;
    let Some(table) = root.as_table_mut() else {
        return Ok(false);
    };
    let mut removed = false;
    if let Some(mcp_servers) = table.get_mut("mcp_servers").and_then(toml::Value::as_table_mut) {
        removed |= mcp_servers.remove(id).is_some();
        if mcp_servers.is_empty() {
            table.remove("mcp_servers");
        }
    }
    if removed {
        write_grok_root_toml_at(path, &root)?;
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Cursor  (~/.cursor/mcp.json  →  top-level `mcpServers`)
//
// Cursor's CLI (and IDE — the file is shared) reads its user-global MCP config
// from `<CURSOR_CONFIG_DIR>/mcp.json` (default `~/.cursor/mcp.json`) — a JSON
// file with a top-level `mcpServers` object. The 2026.07.16 CLI validates it
// with a Zod union discriminated purely on shape: `command` present ⇒ stdio,
// `url` present ⇒ remote (transport auto-negotiated http→sse); there is no
// `type`/`transport` key, and unknown keys are stripped on parse (not
// rejected). The writer below therefore emits only the fields Cursor models —
// `command`/`args`/`env`/`cwd` for stdio, `url`/`headers` for remote — so a
// foreign key can't ride canonicalize's passthrough onto disk.
//
// Because Cursor loads this file natively at session start, `Cursor` is on the
// ACP forward skip list in `connection.rs` (like Hermes/Kimi/Grok) so the same
// user servers aren't double-registered over `session/new`. The built-in
// `codeg-mcp` companion is injected separately by `inject_codeg_mcp`, so it
// still reaches Cursor over the wire regardless.
// ---------------------------------------------------------------------------

fn cursor_mcp_json_path() -> PathBuf {
    // Deliberately NOT `resolve_cursor_config_dir()`: the CLI reads its
    // user-level MCP config from a hardcoded `~/.cursor/mcp.json` (every
    // loader in the 2026.07.16 bundle joins `homedir()`), even when
    // `CURSOR_CONFIG_DIR`/`XDG_CONFIG_HOME` relocate chats + cli-config.json.
    dirs::home_dir().unwrap_or_default().join(".cursor").join("mcp.json")
}

fn read_cursor_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    read_cursor_servers_at(&cursor_mcp_json_path())
}

fn read_cursor_servers_at(path: &Path) -> Result<BTreeMap<String, Value>, AppCommandError> {
    let root = read_json_file(path)?;
    let mut out = BTreeMap::new();

    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
        return Ok(out);
    };

    for (id, spec) in servers {
        // Cursor discriminates on shape alone; strip any foreign `type` key so
        // canonicalize re-infers it the way Cursor actually will (`command` ⇒
        // stdio, `url` ⇒ http).
        let mut spec = spec.clone();
        if let Some(obj) = spec.as_object_mut() {
            obj.remove("type");
        }
        match canonicalize_spec(&spec, &format!("Cursor config '{id}'")) {
            Ok(normalized) => {
                out.insert(id.to_string(), normalized);
            }
            Err(err) => {
                eprintln!("[MCP] skip invalid Cursor MCP entry id={id}: {err}");
            }
        }
    }

    Ok(out)
}

/// Convert codeg's canonical spec into a Cursor `mcpServers` entry: only the
/// fields Cursor models, shape-discriminated (no `type`/`transport` key).
fn canonical_to_cursor_entry(spec: &Value) -> Result<Value, AppCommandError> {
    let canonical = canonicalize_spec(spec, "Cursor write")?;
    let Some(obj) = canonical.as_object() else {
        return Ok(canonical);
    };
    let mut out = Map::new();
    for (key, value) in obj {
        let keep = matches!(
            key.as_str(),
            "command" | "args" | "env" | "cwd" | "url" | "headers"
        );
        if keep {
            out.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(out))
}

fn upsert_cursor_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    upsert_cursor_server_at(&cursor_mcp_json_path(), id, spec)
}

fn upsert_cursor_server_at(path: &Path, id: &str, spec: &Value) -> Result<(), AppCommandError> {
    let mut root = read_json_file(path)?;
    if !root.is_object() {
        root = json!({});
    }

    let canonical = canonical_to_cursor_entry(spec)?;

    let obj = root.as_object_mut().ok_or_else(|| {
        mcp_configuration_invalid(format!("invalid JSON root in {}", path.display()))
    })?;
    if !obj.get("mcpServers").map(Value::is_object).unwrap_or(false) {
        obj.insert("mcpServers".to_string(), Value::Object(Map::new()));
    }

    let map = obj
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            mcp_configuration_invalid(format!("invalid mcpServers in {}", path.display()))
        })?;
    map.insert(id.to_string(), canonical);

    write_json_file(path, &root)
}

fn remove_cursor_server(id: &str) -> Result<bool, AppCommandError> {
    remove_cursor_server_at(&cursor_mcp_json_path(), id)
}

fn remove_cursor_server_at(path: &Path, id: &str) -> Result<bool, AppCommandError> {
    if !path.exists() {
        return Ok(false);
    }

    let mut root = read_json_file(path)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(false);
    };
    let Some(servers) = obj.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(false);
    };

    let removed = servers.remove(id).is_some();
    if removed {
        write_json_file(path, &root)?;
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Hermes Agent  (~/.hermes/config.yaml  →  mcp_servers)
//
// Hermes reads the `mcp_servers` section of its own config.yaml natively at
// launch (registering each as an `mcp-<name>` toolset), so codeg manages that
// section directly — the same "write the agent's own config file" model used
// for Codex/OpenCode — rather than forwarding servers over the ACP wire. The
// ACP forward path (`load_mcp_servers_for_agent`) deliberately skips Hermes to
// avoid double-registering what Hermes already reads from config.yaml.
//
// Hermes' entry shape: stdio = `{command, args, env}`; remote = `{url}` (+
// `transport: sse` for SSE, optional `headers` / `client_cert` / `client_key`).
// Translate to/from codeg's canonical spec, whose discriminator is `type`.
// ---------------------------------------------------------------------------

/// Convert one Hermes `mcp_servers` YAML entry into codeg's canonical spec.
fn hermes_entry_to_canonical(
    entry: &serde_yaml::Value,
    id: &str,
) -> Result<Value, AppCommandError> {
    let source = format!("Hermes mcp_servers '{id}'");
    let mut json = serde_json::to_value(entry)
        .map_err(|e| mcp_configuration_invalid(format!("{source}: cannot read entry: {e}")))?;
    let obj = json
        .as_object_mut()
        .ok_or_else(|| mcp_configuration_invalid(format!("{source}: entry must be a mapping")))?;
    // Hermes encodes SSE via `transport: sse` (not a `type` field); a bare `url`
    // is StreamableHTTP. Map that onto the canonical `type` so `canonicalize_spec`
    // classifies it (stdio is inferred from `command`). `transport` stays as a
    // passthrough key.
    if obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .is_empty()
        && obj.get("url").is_some()
    {
        let is_sse = obj
            .get("transport")
            .and_then(Value::as_str)
            .map(|t| t.eq_ignore_ascii_case("sse"))
            .unwrap_or(false);
        obj.insert(
            "type".to_string(),
            Value::String(if is_sse { "sse" } else { "http" }.to_string()),
        );
    }
    // `transport` is Hermes' encoding of the remote kind; the canonical `type`
    // now carries it, so drop the redundant key (keeps round-trips stable and
    // doesn't leak a Hermes-ism into specs shared with other agents).
    obj.remove("transport");
    canonicalize_spec(&json, &source)
}

/// Convert codeg's canonical spec into a Hermes `mcp_servers` YAML entry.
fn canonical_to_hermes_entry(spec: &Value) -> Result<serde_yaml::Value, AppCommandError> {
    let canonical = canonicalize_spec(spec, "Hermes conversion")?;
    let obj = canonical
        .as_object()
        .ok_or_else(|| mcp_invalid_input("Hermes conversion: canonical spec must be an object"))?;
    let typ = obj.get("type").and_then(Value::as_str).unwrap_or("stdio");

    let mut out = Map::new();
    match typ {
        "stdio" => {
            // Hermes 0.16.0 reads only `command`/`args`/`env` for stdio MCP
            // (tools/mcp_tool.py → StdioServerParameters); it ignores `cwd`, so
            // don't write it — a silently-ignored key would misrepresent what
            // Hermes actually honors.
            for key in ["command", "args", "env"] {
                if let Some(value) = obj.get(key) {
                    out.insert(key.to_string(), value.clone());
                }
            }
        }
        "http" | "sse" => {
            if let Some(url) = obj.get("url") {
                out.insert("url".to_string(), url.clone());
            }
            if typ == "sse" {
                out.insert("transport".to_string(), Value::String("sse".to_string()));
            }
            if let Some(headers) = obj.get("headers") {
                out.insert("headers".to_string(), headers.clone());
            }
        }
        other => {
            return Err(mcp_invalid_input(format!(
                "Hermes conversion: unsupported MCP type '{other}'"
            )));
        }
    }
    // Preserve passthrough keys Hermes understands (mTLS `client_cert`/
    // `client_key`, an explicit `enabled` flag, etc.) — anything beyond the
    // transport fields and the `type` discriminator translated above.
    for (key, value) in obj {
        if matches!(
            key.as_str(),
            "type" | "command" | "args" | "env" | "cwd" | "url" | "headers" | "transport"
        ) {
            continue;
        }
        if !value.is_null() {
            out.insert(key.clone(), value.clone());
        }
    }

    serde_yaml::to_value(Value::Object(out)).map_err(|e| {
        mcp_configuration_invalid(format!("Hermes conversion: serialize entry failed: {e}"))
    })
}

/// Read Hermes' MCP servers from `~/.hermes/config.yaml` (`mcp_servers`). A
/// missing or unparseable config.yaml surfaces no servers rather than failing
/// the whole MCP scan — the file is large and user-owned.
fn read_hermes_servers() -> Result<BTreeMap<String, Value>, AppCommandError> {
    let path = crate::commands::acp::hermes_config_yaml_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return Ok(BTreeMap::new());
    };
    let root: serde_yaml::Value = match serde_yaml::from_str(&raw) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!("[MCP] skip Hermes mcp_servers: invalid config.yaml: {err}");
            return Ok(BTreeMap::new());
        }
    };

    let mut out = BTreeMap::new();
    let Some(servers) = root
        .get("mcp_servers")
        .and_then(serde_yaml::Value::as_mapping)
    else {
        return Ok(out);
    };
    for (key, entry) in servers {
        let Some(id) = key.as_str() else { continue };
        match hermes_entry_to_canonical(entry, id) {
            Ok(spec) => {
                out.insert(id.to_string(), spec);
            }
            Err(err) => {
                tracing::warn!("[MCP] skip invalid Hermes mcp_servers entry id={id}: {err}");
            }
        }
    }
    Ok(out)
}

/// Insert/update a Hermes MCP server in `~/.hermes/config.yaml` (`mcp_servers`),
/// preserving every other key. Written through the Hermes secret writer
/// (owner-only perms, symlink-preserving) since the file can carry env secrets.
/// Note: like the structured model save, this round-trips config.yaml through
/// serde_yaml and so drops comments — consistent with codeg's existing Hermes
/// config edits.
fn upsert_hermes_server(id: &str, spec: &Value) -> Result<(), AppCommandError> {
    use serde_yaml::{Mapping, Value as Yaml};
    let entry = canonical_to_hermes_entry(spec)?;
    let path = crate::commands::acp::hermes_config_yaml_path();

    // Only a genuinely absent (or empty) config starts from a fresh mapping.
    // A permission / invalid-UTF-8 read error must NOT silently discard the
    // user's real config.yaml by overwriting it with a near-empty document.
    let mut root: Yaml = match fs::read_to_string(&path) {
        Ok(raw) if !raw.trim().is_empty() => serde_yaml::from_str(&raw)
            .map_err(|e| mcp_configuration_invalid(format!("invalid hermes config.yaml: {e}")))?,
        Ok(_) => Yaml::Mapping(Mapping::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Yaml::Mapping(Mapping::new()),
        Err(e) => {
            return Err(mcp_configuration_invalid(format!(
                "read hermes config.yaml failed: {e}"
            )));
        }
    };
    if !root.is_mapping() {
        root = Yaml::Mapping(Mapping::new());
    }
    let root_map = root.as_mapping_mut().expect("root is a mapping");
    let servers_key = Yaml::String("mcp_servers".to_string());
    if !root_map
        .get(&servers_key)
        .map(Yaml::is_mapping)
        .unwrap_or(false)
    {
        root_map.insert(servers_key.clone(), Yaml::Mapping(Mapping::new()));
    }
    let servers = root_map
        .get_mut(&servers_key)
        .and_then(Yaml::as_mapping_mut)
        .ok_or_else(|| mcp_configuration_invalid("hermes mcp_servers must be a mapping"))?;
    servers.insert(Yaml::String(id.to_string()), entry);

    let yaml = serde_yaml::to_string(&root).map_err(|e| {
        mcp_configuration_invalid(format!("serialize hermes config.yaml failed: {e}"))
    })?;
    crate::commands::acp::ensure_hermes_home_secure(&crate::commands::acp::hermes_home_dir())
        .map_err(|e| mcp_configuration_invalid(format!("prepare hermes home failed: {e}")))?;
    crate::commands::acp::write_hermes_secret_file(&path, &yaml, "config.yaml")
        .map_err(|e| mcp_configuration_invalid(format!("write hermes config.yaml failed: {e}")))?;
    Ok(())
}

/// Remove a Hermes MCP server from `~/.hermes/config.yaml` (`mcp_servers`).
fn remove_hermes_server(id: &str) -> Result<bool, AppCommandError> {
    use serde_yaml::Value as Yaml;
    let path = crate::commands::acp::hermes_config_yaml_path();
    let raw = match fs::read_to_string(&path) {
        Ok(raw) if !raw.trim().is_empty() => raw,
        _ => return Ok(false),
    };
    let mut root: Yaml = match serde_yaml::from_str(&raw) {
        Ok(value) => value,
        Err(err) => {
            tracing::info!("[MCP] Hermes remove '{id}': invalid config.yaml: {err}");
            return Ok(false);
        }
    };
    let Some(root_map) = root.as_mapping_mut() else {
        return Ok(false);
    };
    let servers_key = Yaml::String("mcp_servers".to_string());
    let Some(servers) = root_map
        .get_mut(&servers_key)
        .and_then(Yaml::as_mapping_mut)
    else {
        return Ok(false);
    };
    let removed = servers.remove(Yaml::String(id.to_string())).is_some();
    if servers.is_empty() {
        root_map.remove(servers_key);
    }
    if removed {
        let yaml = serde_yaml::to_string(&root).map_err(|e| {
            mcp_configuration_invalid(format!("serialize hermes config.yaml failed: {e}"))
        })?;
        crate::commands::acp::write_hermes_secret_file(&path, &yaml, "config.yaml").map_err(
            |e| mcp_configuration_invalid(format!("write hermes config.yaml failed: {e}")),
        )?;
    }
    Ok(removed)
}

fn remove_server_for_app(app: McpAppType, id: &str) -> Result<bool, AppCommandError> {
    match app {
        McpAppType::ClaudeCode => remove_claude_server(id),
        McpAppType::Codex => remove_codex_server(id),
        McpAppType::OpenCode => remove_opencode_server(id),
        McpAppType::Gemini => remove_gemini_server(id),
        McpAppType::OpenClaw => remove_openclaw_server(id),
        McpAppType::Cline => remove_cline_server(id),
        McpAppType::Hermes => remove_hermes_server(id),
        McpAppType::CodeBuddy => remove_codebuddy_server(id),
        McpAppType::KimiCode => remove_kimi_code_server(id),
        McpAppType::Grok => remove_grok_server(id),
        McpAppType::Cursor => remove_cursor_server(id),
    }
}

#[derive(Debug, Deserialize)]
struct OfficialServerResponse {
    server: OfficialServer,
    #[serde(default)]
    _meta: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct OfficialServer {
    name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "websiteUrl")]
    website_url: Option<String>,
    #[serde(default)]
    repository: Option<OfficialRepository>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    icons: Option<Vec<OfficialIcon>>,
    #[serde(default)]
    remotes: Option<Vec<OfficialTransport>>,
    #[serde(default)]
    packages: Option<Vec<OfficialPackage>>,
}

#[derive(Debug, Deserialize)]
struct OfficialRepository {
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OfficialTransport {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_official_key_value_inputs")]
    headers: Option<Vec<OfficialKeyValueInput>>,
    #[serde(default, deserialize_with = "deserialize_official_key_value_inputs")]
    variables: Option<Vec<OfficialKeyValueInput>>,
}

#[derive(Debug, Deserialize)]
struct OfficialIcon {
    #[serde(default)]
    src: Option<String>,
    #[serde(default, rename = "mimeType")]
    _mime_type: Option<String>,
    #[serde(default)]
    _sizes: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct OfficialPackage {
    #[serde(default, rename = "registryType")]
    registry_type: String,
    identifier: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default, rename = "runtimeHint")]
    runtime_hint: Option<String>,
    #[serde(default, rename = "runtimeArguments")]
    runtime_arguments: Vec<OfficialArgument>,
    #[serde(default, rename = "packageArguments")]
    package_arguments: Vec<OfficialArgument>,
    #[serde(default, rename = "environmentVariables")]
    environment_variables: Vec<OfficialKeyValueInput>,
    transport: OfficialTransport,
}

#[derive(Debug, Deserialize)]
struct OfficialArgument {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default, rename = "isRequired")]
    is_required: Option<bool>,
    #[serde(default, rename = "isRepeated")]
    _is_repeated: Option<bool>,
    #[serde(default, rename = "valueHint")]
    value_hint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OfficialKeyValueInput {
    name: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default, rename = "isRequired")]
    is_required: Option<bool>,
    #[serde(default, rename = "isSecret")]
    is_secret: Option<bool>,
    #[serde(default, rename = "valueHint")]
    value_hint: Option<String>,
}

fn deserialize_official_key_value_inputs<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<OfficialKeyValueInput>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<Value>::deserialize(deserializer)?;
    let Some(value) = raw else {
        return Ok(None);
    };

    if value.is_null() {
        return Ok(None);
    }

    let mut out = Vec::new();

    if let Some(items) = value.as_array() {
        for item in items {
            let Ok(parsed) = serde_json::from_value::<OfficialKeyValueInput>(item.clone()) else {
                continue;
            };
            out.push(parsed);
        }
        if out.is_empty() {
            return Ok(None);
        }
        return Ok(Some(out));
    }

    if let Some(map) = value.as_object() {
        for (key, item) in map {
            let name = key.trim().to_string();
            if name.is_empty() {
                continue;
            }

            let mut parsed = OfficialKeyValueInput {
                name,
                value: None,
                default: None,
                description: None,
                format: None,
                is_required: None,
                is_secret: None,
                value_hint: None,
            };

            if let Some(text) = item.as_str() {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    parsed.value = Some(trimmed.to_string());
                }
                out.push(parsed);
                continue;
            }

            if let Some(obj) = item.as_object() {
                parsed.value = obj
                    .get("value")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                parsed.default = obj
                    .get("default")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                parsed.description = obj
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                parsed.format = obj
                    .get("format")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                parsed.is_required = obj.get("isRequired").and_then(Value::as_bool);
                parsed.is_secret = obj.get("isSecret").and_then(Value::as_bool);
                parsed.value_hint = obj
                    .get("valueHint")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
            }

            out.push(parsed);
        }
    }

    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

#[derive(Debug, Deserialize)]
struct SmitheryServerListResponse {
    #[serde(default)]
    servers: Vec<SmitheryServerSummary>,
}

#[derive(Debug, Deserialize)]
struct SmitheryServerSummary {
    #[serde(default)]
    _id: Option<String>,
    #[serde(rename = "qualifiedName")]
    qualified_name: String,
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default, rename = "iconUrl")]
    icon_url: Option<String>,
    #[serde(default)]
    namespace: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    remote: bool,
    #[serde(default)]
    verified: bool,
    #[serde(default, rename = "useCount")]
    use_count: Option<u64>,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default, rename = "isDeployed")]
    is_deployed: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct SmitheryServerDetail {
    #[serde(rename = "qualifiedName")]
    qualified_name: String,
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default, rename = "iconUrl")]
    icon_url: Option<String>,
    #[serde(default)]
    namespace: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default, rename = "deploymentUrl")]
    deployment_url: Option<String>,
    #[serde(default)]
    remote: bool,
    #[serde(default)]
    verified: bool,
    #[serde(default, rename = "useCount")]
    use_count: Option<u64>,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default, rename = "isDeployed")]
    is_deployed: Option<bool>,
    #[serde(default)]
    connections: Vec<SmitheryConnection>,
}

#[derive(Debug, Deserialize)]
struct SmitheryConnection {
    #[serde(default)]
    r#type: String,
    #[serde(default, rename = "deploymentUrl")]
    deployment_url: Option<String>,
    #[serde(default, rename = "configSchema")]
    config_schema: Option<Value>,
}

fn first_non_empty_icon_src(icons: Option<&[OfficialIcon]>) -> Option<String> {
    icons.and_then(|items| {
        items
            .iter()
            .filter_map(|icon| icon.src.as_deref())
            .map(str::trim)
            .find(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn transport_protocol(kind: &str) -> Option<String> {
    match normalize_mcp_type(kind)? {
        canonical @ ("stdio" | "http" | "sse") => Some(canonical.to_string()),
        _ => None,
    }
}

fn official_server_protocols(server: &OfficialServer) -> Vec<String> {
    let mut seen = BTreeSet::new();
    if let Some(remotes) = server.remotes.as_ref() {
        for remote in remotes {
            if let Some(protocol) = transport_protocol(&remote.r#type) {
                seen.insert(protocol);
            }
        }
    }
    if let Some(packages) = server.packages.as_ref() {
        for package in packages {
            if let Some(protocol) = transport_protocol(&package.transport.r#type) {
                seen.insert(protocol);
            }
        }
    }
    seen.into_iter().collect()
}

fn official_entry_to_item(entry: &OfficialServerResponse) -> McpMarketplaceItem {
    let server = &entry.server;
    let name = server
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| server.name.clone());

    let description = server
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "No description".to_string());

    let homepage = server
        .website_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            server
                .repository
                .as_ref()
                .and_then(|repo| repo.url.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        });

    let remote = server
        .remotes
        .as_ref()
        .map(|items| !items.is_empty())
        .unwrap_or(false);

    let verified = entry
        ._meta
        .as_ref()
        .and_then(|meta| {
            meta.get("io.modelcontextprotocol.registry/official")
                .and_then(Value::as_object)
                .and_then(|official| official.get("status"))
                .and_then(Value::as_str)
        })
        .map(|status| status == "active")
        .unwrap_or(false);

    McpMarketplaceItem {
        provider_id: MARKETPLACE_OFFICIAL.to_string(),
        server_id: server.name.clone(),
        name,
        description,
        homepage,
        remote,
        verified,
        icon_url: first_non_empty_icon_src(server.icons.as_deref()),
        latest_version: server
            .version
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        protocols: official_server_protocols(server),
        owner: None,
        namespace: None,
        downloads: None,
        score: None,
        is_deployed: None,
    }
}

async fn search_official_registry(
    query: &str,
    limit: u32,
) -> Result<Vec<McpMarketplaceItem>, AppCommandError> {
    let client = marketplace_http_client()?;
    let trimmed = query.trim();

    let response = send_request_with_retry("failed to query official MCP registry", || {
        client
            .get("https://registry.modelcontextprotocol.io/v0.1/servers")
            .query(&[
                ("limit", limit.to_string()),
                ("version", "latest".to_string()),
            ])
            .query(&[("search", trimmed.to_string())])
    })
    .await?;

    if !response.status().is_success() {
        return Err(mcp_network(format!(
            "official MCP registry request failed: HTTP {}",
            response.status()
        )));
    }

    let payload =
        parse_json_value_response(response, "failed to parse official MCP registry response")
            .await?;

    let entries = payload
        .get("servers")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            mcp_configuration_invalid(
                "failed to parse official MCP registry response: missing servers array",
            )
        })?;

    let mut out = Vec::new();
    for (index, raw_entry) in entries.iter().enumerate() {
        match serde_json::from_value::<OfficialServerResponse>(raw_entry.clone()) {
            Ok(item) => out.push(official_entry_to_item(&item)),
            Err(err) => {
                tracing::warn!(
                    "[MCP] skip invalid official registry server list entry at index={index}: {err}"
                );
            }
        }
    }

    Ok(out)
}

async fn fetch_official_server_detail(
    server_name: &str,
) -> Result<OfficialServerResponse, AppCommandError> {
    let encoded_name = urlencoding::encode(server_name);
    let url = format!(
        "https://registry.modelcontextprotocol.io/v0.1/servers/{encoded_name}/versions/latest"
    );

    let client = marketplace_http_client()?;
    let response = send_request_with_retry("failed to fetch official MCP server detail", || {
        client.get(url.clone())
    })
    .await?;

    if !response.status().is_success() {
        return Err(mcp_network(format!(
            "official MCP server detail request failed: HTTP {}",
            response.status()
        )));
    }

    parse_json_response::<OfficialServerResponse>(
        response,
        "failed to parse official MCP server detail",
    )
    .await
}

fn official_remote_option_id(index: usize, protocol: &str) -> String {
    format!("official:remote:{index}:{protocol}")
}

fn official_package_option_id(index: usize, protocol: &str) -> String {
    format!("official:package:{index}:{protocol}")
}

fn parse_official_option_id(option_id: &str) -> Option<(&str, usize)> {
    let mut parts = option_id.split(':');
    let provider = parts.next()?;
    let source = parts.next()?;
    let idx = parts.next()?.parse::<usize>().ok()?;
    if provider != "official" {
        return None;
    }
    Some((source, idx))
}

fn select_option_from_list<'a>(
    options: &'a [McpMarketplaceInstallOption],
    selection: &InstallSelection,
) -> Result<&'a McpMarketplaceInstallOption, AppCommandError> {
    if let Some(option_id) = selection.option_id.as_deref() {
        return options
            .iter()
            .find(|item| item.id == option_id)
            .ok_or_else(|| {
                mcp_not_found(format!("selected install option not found: {option_id}"))
            });
    }

    if let Some(protocol) = selection.protocol.as_deref() {
        let mut by_protocol = options
            .iter()
            .filter(|item| normalize_protocol_value(&item.protocol) == protocol);
        if let Some(first) = by_protocol.next() {
            let mut best = first;
            for next in by_protocol {
                if protocol_priority(&next.protocol) < protocol_priority(&best.protocol) {
                    best = next;
                }
            }
            return Ok(best);
        }
        return Err(mcp_not_found(format!(
            "no install option found for protocol '{protocol}'"
        )));
    }

    select_default_install_option(options)
        .ok_or_else(|| mcp_not_found("server does not provide installable options"))
}

fn key_looks_secret(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.contains("token")
        || lowered.contains("secret")
        || lowered.contains("password")
        || lowered.contains("api_key")
        || lowered.ends_with("key")
}

fn official_text_to_value(kind: &str, value: &str) -> Value {
    let trimmed = value.trim();
    match kind {
        "boolean" => Value::Bool(trimmed.eq_ignore_ascii_case("true")),
        "number" => trimmed
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(trimmed.to_string())),
        "integer" => trimmed
            .parse::<i64>()
            .ok()
            .map(|item| Value::Number(item.into()))
            .unwrap_or_else(|| Value::String(trimmed.to_string())),
        _ => Value::String(trimmed.to_string()),
    }
}

fn infer_parameter_kind(format: Option<&str>) -> String {
    match format.map(str::trim).unwrap_or("string") {
        "boolean" => "boolean".to_string(),
        "number" => "number".to_string(),
        "integer" => "integer".to_string(),
        "object" | "array" => "json".to_string(),
        _ => "string".to_string(),
    }
}

fn value_as_text(value: &Value) -> Option<String> {
    match value {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Number(raw) => Some(raw.to_string()),
        Value::Bool(raw) => Some(raw.to_string()),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).ok(),
        Value::Null => None,
    }
}

fn read_parameter_value_as_text(values: &Map<String, Value>, key: &str) -> Option<String> {
    values.get(key).and_then(value_as_text)
}

fn official_kv_default(item: &OfficialKeyValueInput) -> Option<String> {
    item.value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            item.default
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .filter(|value| !contains_unresolved_placeholder(value))
        .map(str::to_string)
}

fn official_kv_is_required(item: &OfficialKeyValueInput) -> bool {
    if item.is_required.unwrap_or(false) {
        return true;
    }
    let has_placeholder = item
        .value
        .as_deref()
        .map(contains_unresolved_placeholder)
        .unwrap_or(false)
        || item
            .default
            .as_deref()
            .map(contains_unresolved_placeholder)
            .unwrap_or(false);
    has_placeholder || official_kv_default(item).is_none()
}

fn append_query_param(url: &str, key: &str, value: &str) -> String {
    let encoded_key = urlencoding::encode(key);
    let encoded_value = urlencoding::encode(value);
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}{encoded_key}={encoded_value}")
}

fn apply_transport_variables(
    base_url: &str,
    variables: Option<&[OfficialKeyValueInput]>,
    values: &Map<String, Value>,
    enforce_required: bool,
) -> Result<String, AppCommandError> {
    let Some(items) = variables else {
        return Ok(base_url.to_string());
    };

    let mut url = base_url.to_string();
    for item in items {
        let key_name = item.name.trim();
        if key_name.is_empty() {
            continue;
        }
        let field_key = format!("variables.{key_name}");
        let value =
            read_parameter_value_as_text(values, &field_key).or_else(|| official_kv_default(item));
        if let Some(text) = value {
            let encoded = urlencoding::encode(&text);
            let brace = format!("{{{key_name}}}");
            let moustache = format!("{{{{{key_name}}}}}");
            if url.contains(&brace) {
                url = url.replace(&brace, &encoded);
            } else if url.contains(&moustache) {
                url = url.replace(&moustache, &encoded);
            } else {
                url = append_query_param(&url, key_name, &text);
            }
            continue;
        }
        if enforce_required && official_kv_is_required(item) {
            return Err(mcp_invalid_input(format!(
                "missing required variable '{key_name}'"
            )));
        }
    }
    Ok(url)
}

fn remote_spec_from_transport_with_values(
    transport: &OfficialTransport,
    values: &Map<String, Value>,
    enforce_required: bool,
) -> Result<Value, AppCommandError> {
    let kind = transport.r#type.trim();
    let canonical_type = match normalize_mcp_type(kind) {
        Some(value @ ("http" | "sse")) => value,
        _ => {
            return Err(
                mcp_invalid_input(format!("unsupported transport type '{kind}'")).with_i18n(
                    "errors.unsupportedTransportType",
                    mcp_i18n_params([("type", kind)]),
                ),
            )
        }
    };

    let base_url = transport
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| mcp_invalid_input("remote transport missing URL"))?;

    let url = apply_transport_variables(
        base_url,
        transport.variables.as_deref(),
        values,
        enforce_required,
    )?;

    let mut spec = Map::new();
    spec.insert(
        "type".to_string(),
        Value::String(canonical_type.to_string()),
    );
    spec.insert("url".to_string(), Value::String(url));

    let mut headers = Map::new();
    if let Some(items) = transport.headers.as_deref() {
        for item in items {
            let key_name = item.name.trim();
            if key_name.is_empty() {
                continue;
            }
            let field_key = format!("headers.{key_name}");
            let value = read_parameter_value_as_text(values, &field_key)
                .or_else(|| official_kv_default(item));
            if let Some(text) = value {
                headers.insert(key_name.to_string(), Value::String(text));
                continue;
            }
            if enforce_required && official_kv_is_required(item) {
                return Err(mcp_invalid_input(format!(
                    "missing required header '{key_name}'"
                )));
            }
        }
    }
    if !headers.is_empty() {
        spec.insert("headers".to_string(), Value::Object(headers));
    }

    canonicalize_spec(&Value::Object(spec), "official transport")
}

fn official_remote_parameter_fields(
    transport: &OfficialTransport,
) -> Vec<McpMarketplaceInstallParameter> {
    let mut fields = Vec::new();
    if let Some(headers) = transport.headers.as_deref() {
        for item in headers {
            let key = item.name.trim();
            if key.is_empty() {
                continue;
            }
            let kind = infer_parameter_kind(item.format.as_deref());
            fields.push(McpMarketplaceInstallParameter {
                key: format!("headers.{key}"),
                label: key.to_string(),
                description: item
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                required: official_kv_is_required(item),
                secret: item.is_secret.unwrap_or(false) || key_looks_secret(key),
                kind: kind.clone(),
                default_value: official_kv_default(item)
                    .as_deref()
                    .map(|value| official_text_to_value(&kind, value)),
                placeholder: item
                    .value_hint
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                enum_values: Vec::new(),
                location: Some("header".to_string()),
            });
        }
    }

    if let Some(variables) = transport.variables.as_deref() {
        for item in variables {
            let key = item.name.trim();
            if key.is_empty() {
                continue;
            }
            let kind = infer_parameter_kind(item.format.as_deref());
            fields.push(McpMarketplaceInstallParameter {
                key: format!("variables.{key}"),
                label: key.to_string(),
                description: item
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                required: official_kv_is_required(item),
                secret: item.is_secret.unwrap_or(false) || key_looks_secret(key),
                kind: kind.clone(),
                default_value: official_kv_default(item)
                    .as_deref()
                    .map(|value| official_text_to_value(&kind, value)),
                placeholder: item
                    .value_hint
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                enum_values: Vec::new(),
                location: Some("query".to_string()),
            });
        }
    }

    fields
}

fn build_official_install_options(
    server: &OfficialServer,
) -> Result<Vec<McpMarketplaceInstallOption>, AppCommandError> {
    let mut options = Vec::new();

    if let Some(packages) = server.packages.as_ref() {
        for (index, package) in packages.iter().enumerate() {
            let Some(protocol) = transport_protocol(&package.transport.r#type) else {
                continue;
            };

            if protocol == "stdio" {
                match resolve_official_stdio_package(package) {
                    Ok(spec) => {
                        let runtime = package
                            .runtime_hint
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .unwrap_or("runtime");
                        options.push(McpMarketplaceInstallOption {
                            id: official_package_option_id(index, &protocol),
                            protocol: protocol.clone(),
                            label: format!("stdio ({runtime})"),
                            description: Some(format!("Run package {}", package.identifier)),
                            spec,
                            parameters: official_stdio_parameter_fields(package),
                        });
                    }
                    Err(err) => {
                        tracing::warn!("[MCP] skip invalid official stdio package: {err}");
                    }
                }
            } else if let Ok(spec) =
                remote_spec_from_transport_with_values(&package.transport, &Map::new(), false)
            {
                options.push(McpMarketplaceInstallOption {
                    id: official_package_option_id(index, &protocol),
                    protocol: protocol.clone(),
                    label: format!("{protocol} (package)"),
                    description: Some(format!("Remote package {}", package.identifier)),
                    spec,
                    parameters: official_remote_parameter_fields(&package.transport),
                });
            }
        }
    }

    if let Some(remotes) = server.remotes.as_ref() {
        for (index, transport) in remotes.iter().enumerate() {
            let Some(protocol) = transport_protocol(&transport.r#type) else {
                continue;
            };
            if let Ok(spec) = remote_spec_from_transport_with_values(transport, &Map::new(), false)
            {
                options.push(McpMarketplaceInstallOption {
                    id: official_remote_option_id(index, &protocol),
                    protocol: protocol.clone(),
                    label: format!("{protocol} (remote)"),
                    description: transport
                        .url
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    spec,
                    parameters: official_remote_parameter_fields(transport),
                });
            }
        }
    }

    if options.is_empty() {
        return Err(mcp_not_found(format!(
            "official MCP server '{}' does not expose an installable transport",
            server.name
        )));
    }

    Ok(options)
}

fn resolve_official_install_spec_with_selection(
    server: &OfficialServer,
    selection: &InstallSelection,
) -> Result<Value, AppCommandError> {
    let options = build_official_install_options(server)?;
    let selected = select_option_from_list(&options, selection)?;
    let values = &selection.parameter_values;

    if let Some((source, index)) = parse_official_option_id(&selected.id) {
        if source == "package" {
            let package = server
                .packages
                .as_ref()
                .and_then(|items| items.get(index))
                .ok_or_else(|| {
                    mcp_not_found(format!(
                        "selected package option index is out of range: {index}"
                    ))
                })?;
            if normalize_protocol_value(&selected.protocol) == "stdio" {
                return resolve_official_stdio_package_with_values(package, values, true);
            }
            return remote_spec_from_transport_with_values(&package.transport, values, true);
        }
        if source == "remote" {
            let remote = server
                .remotes
                .as_ref()
                .and_then(|items| items.get(index))
                .ok_or_else(|| {
                    mcp_not_found(format!(
                        "selected remote option index is out of range: {index}"
                    ))
                })?;
            return remote_spec_from_transport_with_values(remote, values, true);
        }
    }

    Err(mcp_invalid_input(format!(
        "unsupported official install option '{}'",
        selected.id
    )))
}

fn package_identifier_with_version(package: &OfficialPackage, runtime: &str) -> String {
    let identifier = package.identifier.trim();
    if identifier.is_empty() {
        return String::new();
    }

    let version = package
        .version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "latest");

    let Some(version) = version else {
        return identifier.to_string();
    };

    if runtime == "uvx" {
        if package.registry_type.trim() == "pypi" {
            return format!("{identifier}=={version}");
        }
        return identifier.to_string();
    }

    if runtime == "npx" {
        if identifier.contains('@') || identifier.starts_with("http") {
            return identifier.to_string();
        }
        return format!("{identifier}@{version}");
    }

    identifier.to_string()
}

fn argument_value(arg: &OfficialArgument) -> Option<String> {
    arg.value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            arg.default
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .filter(|value| !contains_unresolved_placeholder(value))
        .map(str::to_string)
}

fn argument_is_required(arg: &OfficialArgument) -> bool {
    arg.is_required.unwrap_or(false)
}

fn argument_kind(arg: &OfficialArgument) -> String {
    infer_parameter_kind(arg.format.as_deref())
}

fn argument_parameter_key(scope: &str, index: usize) -> String {
    format!("{scope}.{index}")
}

fn resolve_argument_value(
    arg: &OfficialArgument,
    scope: &str,
    index: usize,
    values: &Map<String, Value>,
) -> Option<String> {
    let key = argument_parameter_key(scope, index);
    read_parameter_value_as_text(values, &key).or_else(|| argument_value(arg))
}

fn append_argument_value(
    target: &mut Vec<String>,
    arg: &OfficialArgument,
    scope: &str,
    index: usize,
    values: &Map<String, Value>,
    enforce_required: bool,
) -> Result<(), AppCommandError> {
    let kind = arg.r#type.as_deref().map(str::trim).unwrap_or("positional");
    let resolved = resolve_argument_value(arg, scope, index, values);

    if kind == "named" {
        let Some(name) = arg
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(());
        };
        if let Some(value) = resolved {
            target.push(name.to_string());
            target.push(value);
            return Ok(());
        }
        if enforce_required && argument_is_required(arg) {
            return Err(mcp_invalid_input(format!(
                "missing required argument '{name}'"
            )));
        }
        return Ok(());
    }

    if let Some(value) = resolved {
        target.push(value);
        return Ok(());
    }
    if enforce_required && argument_is_required(arg) {
        let name = arg
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("positional");
        return Err(mcp_invalid_input(format!(
            "missing required argument '{name}'"
        )));
    }
    Ok(())
}

fn official_stdio_parameter_fields(
    package: &OfficialPackage,
) -> Vec<McpMarketplaceInstallParameter> {
    let mut fields = Vec::new();

    for (index, arg) in package.runtime_arguments.iter().enumerate() {
        let kind = argument_kind(arg);
        let label = arg
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("runtime arg {}", index + 1));
        fields.push(McpMarketplaceInstallParameter {
            key: argument_parameter_key("runtime_arguments", index),
            label,
            description: arg
                .description
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            required: argument_is_required(arg),
            secret: false,
            kind: kind.clone(),
            default_value: argument_value(arg)
                .as_deref()
                .map(|value| official_text_to_value(&kind, value)),
            placeholder: arg
                .value_hint
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            enum_values: Vec::new(),
            location: Some("arg".to_string()),
        });
    }

    for (index, arg) in package.package_arguments.iter().enumerate() {
        let kind = argument_kind(arg);
        let label = arg
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("package arg {}", index + 1));
        fields.push(McpMarketplaceInstallParameter {
            key: argument_parameter_key("package_arguments", index),
            label,
            description: arg
                .description
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            required: argument_is_required(arg),
            secret: false,
            kind: kind.clone(),
            default_value: argument_value(arg)
                .as_deref()
                .map(|value| official_text_to_value(&kind, value)),
            placeholder: arg
                .value_hint
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            enum_values: Vec::new(),
            location: Some("arg".to_string()),
        });
    }

    for item in &package.environment_variables {
        let key = item.name.trim();
        if key.is_empty() {
            continue;
        }
        let kind = infer_parameter_kind(item.format.as_deref());
        fields.push(McpMarketplaceInstallParameter {
            key: format!("env.{key}"),
            label: key.to_string(),
            description: item
                .description
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            required: official_kv_is_required(item),
            secret: item.is_secret.unwrap_or(false) || key_looks_secret(key),
            kind: kind.clone(),
            default_value: official_kv_default(item)
                .as_deref()
                .map(|value| official_text_to_value(&kind, value)),
            placeholder: item
                .value_hint
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            enum_values: Vec::new(),
            location: Some("env".to_string()),
        });
    }

    fields
}

fn resolve_official_stdio_package(package: &OfficialPackage) -> Result<Value, AppCommandError> {
    resolve_official_stdio_package_with_values(package, &Map::new(), false)
}

fn resolve_official_stdio_package_with_values(
    package: &OfficialPackage,
    values: &Map<String, Value>,
    enforce_required: bool,
) -> Result<Value, AppCommandError> {
    let runtime = package
        .runtime_hint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| match package.registry_type.trim() {
            "npm" => Some("npx".to_string()),
            "pypi" => Some("uvx".to_string()),
            _ => None,
        })
        .ok_or_else(|| {
            mcp_configuration_invalid(format!(
                "official package '{}' missing runtime hint",
                package.identifier
            ))
        })?;

    let mut args = Vec::new();
    if runtime == "npx" {
        args.push("-y".to_string());
    }

    for (index, arg) in package.runtime_arguments.iter().enumerate() {
        append_argument_value(
            &mut args,
            arg,
            "runtime_arguments",
            index,
            values,
            enforce_required,
        )?;
    }

    let package_identifier = package_identifier_with_version(package, &runtime);
    if package_identifier.is_empty() {
        return Err(mcp_configuration_invalid(
            "official package identifier is empty",
        ));
    }
    args.push(package_identifier);

    for (index, arg) in package.package_arguments.iter().enumerate() {
        append_argument_value(
            &mut args,
            arg,
            "package_arguments",
            index,
            values,
            enforce_required,
        )?;
    }

    let mut env = Map::new();
    for item in &package.environment_variables {
        let key = item.name.trim();
        if key.is_empty() {
            continue;
        }
        let field_key = format!("env.{key}");
        let value =
            read_parameter_value_as_text(values, &field_key).or_else(|| official_kv_default(item));
        if let Some(value) = value {
            env.insert(key.to_string(), Value::String(value.to_string()));
            continue;
        }
        if enforce_required && official_kv_is_required(item) {
            return Err(mcp_invalid_input(format!(
                "missing required environment variable '{key}'"
            )));
        }
    }

    let mut spec = Map::new();
    spec.insert("type".to_string(), Value::String("stdio".to_string()));
    spec.insert("command".to_string(), Value::String(runtime));
    if !args.is_empty() {
        spec.insert(
            "args".to_string(),
            Value::Array(args.into_iter().map(Value::String).collect()),
        );
    }
    if !env.is_empty() {
        spec.insert("env".to_string(), Value::Object(env));
    }

    Ok(Value::Object(spec))
}

async fn search_smithery(
    query: &str,
    limit: u32,
) -> Result<Vec<McpMarketplaceItem>, AppCommandError> {
    let client = marketplace_http_client()?;
    let trimmed = query.trim();

    let response = send_request_with_retry("failed to query smithery marketplace", || {
        client
            .get("https://api.smithery.ai/servers")
            .query(&[("limit", limit.to_string()), ("q", trimmed.to_string())])
    })
    .await?;

    if !response.status().is_success() {
        return Err(mcp_network(format!(
            "smithery marketplace request failed: HTTP {}",
            response.status()
        )));
    }

    let payload = parse_json_response::<SmitheryServerListResponse>(
        response,
        "failed to parse smithery response",
    )
    .await?;

    Ok(payload
        .servers
        .into_iter()
        .map(|item| McpMarketplaceItem {
            provider_id: MARKETPLACE_SMITHERY.to_string(),
            server_id: item.qualified_name,
            name: item.display_name,
            description: item
                .description
                .unwrap_or_else(|| "No description".to_string()),
            homepage: item.homepage,
            remote: item.remote,
            verified: item.verified,
            icon_url: item
                .icon_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            latest_version: None,
            protocols: if item.remote {
                vec!["http".to_string()]
            } else {
                Vec::new()
            },
            owner: item
                .owner
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            namespace: item
                .namespace
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            downloads: item.use_count,
            score: item.score,
            is_deployed: item.is_deployed,
        })
        .collect())
}

async fn fetch_smithery_server_summary(
    server_id: &str,
) -> Result<SmitheryServerSummary, AppCommandError> {
    let client = marketplace_http_client()?;
    let response = send_request_with_retry("failed to fetch smithery server summary", || {
        client
            .get("https://api.smithery.ai/servers")
            .query(&[("limit", "30"), ("q", server_id)])
    })
    .await?;

    if !response.status().is_success() {
        return Err(mcp_network(format!(
            "smithery server summary request failed: HTTP {}",
            response.status()
        )));
    }

    let payload = parse_json_response::<SmitheryServerListResponse>(
        response,
        "failed to parse smithery server summary",
    )
    .await?;

    payload
        .servers
        .into_iter()
        .find(|item| item.qualified_name == server_id)
        .ok_or_else(|| mcp_not_found(format!("smithery server summary not found: {server_id}")))
}

async fn fetch_smithery_server_detail(
    server_id: &str,
) -> Result<SmitheryServerDetail, AppCommandError> {
    let url = format!("https://api.smithery.ai/servers/{server_id}");
    let client = marketplace_http_client()?;
    let response = send_request_with_retry("failed to fetch smithery server detail", || {
        client.get(url.clone())
    })
    .await?;

    if !response.status().is_success() {
        return Err(mcp_network(format!(
            "smithery server detail request failed: HTTP {}",
            response.status()
        )));
    }

    parse_json_response::<SmitheryServerDetail>(response, "failed to parse smithery server detail")
        .await
}

#[derive(Debug, Clone)]
struct SmitheryConfigField {
    key: String,
    description: Option<String>,
    required: bool,
    secret: bool,
    kind: String,
    default_value: Option<Value>,
    enum_values: Vec<String>,
    location: String,
}

fn smithery_option_id(index: usize, protocol: &str) -> String {
    format!("smithery:connection:{index}:{protocol}")
}

fn parse_smithery_option_id(option_id: &str) -> Option<usize> {
    let mut parts = option_id.split(':');
    let provider = parts.next()?;
    let source = parts.next()?;
    let idx = parts.next()?.parse::<usize>().ok()?;
    if provider != "smithery" || source != "connection" {
        return None;
    }
    Some(idx)
}

fn smithery_connection_protocol(connection: &SmitheryConnection) -> String {
    match normalize_mcp_type(&connection.r#type) {
        Some("sse") => "sse".to_string(),
        Some("http") => "http".to_string(),
        _ => "http".to_string(),
    }
}

fn smithery_connection_url(
    connection: &SmitheryConnection,
    fallback: Option<&str>,
) -> Option<String> {
    connection
        .deployment_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            fallback
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

fn smithery_property_kind(prop: &Map<String, Value>) -> String {
    if let Some(raw) = prop.get("type") {
        if let Some(typ) = raw.as_str() {
            return match typ.trim() {
                "boolean" => "boolean".to_string(),
                "number" => "number".to_string(),
                "integer" => "integer".to_string(),
                "object" | "array" => "json".to_string(),
                _ => "string".to_string(),
            };
        }
        if let Some(types) = raw.as_array() {
            for item in types {
                let Some(typ) = item.as_str() else {
                    continue;
                };
                if typ == "null" {
                    continue;
                }
                return match typ {
                    "boolean" => "boolean".to_string(),
                    "number" => "number".to_string(),
                    "integer" => "integer".to_string(),
                    "object" | "array" => "json".to_string(),
                    _ => "string".to_string(),
                };
            }
        }
    }
    "string".to_string()
}

fn smithery_field_location(key: &str, prop: &Map<String, Value>, secret: bool) -> String {
    let explicit = prop
        .get("x-from")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if explicit.eq_ignore_ascii_case("header") {
        return "header".to_string();
    }
    if explicit.eq_ignore_ascii_case("query") {
        return "query".to_string();
    }
    if secret || key_looks_secret(key) {
        return "header".to_string();
    }
    "query".to_string()
}

fn parse_smithery_config_fields(schema: Option<&Value>) -> Vec<SmitheryConfigField> {
    let Some(root) = schema.and_then(Value::as_object) else {
        return Vec::new();
    };
    let required = root
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let Some(properties) = root.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut fields = Vec::new();
    for (key, raw_prop) in properties {
        let Some(prop) = raw_prop.as_object() else {
            continue;
        };
        let kind = smithery_property_kind(prop);
        let secret = prop
            .get("writeOnly")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || key_looks_secret(key);
        let location = smithery_field_location(key, prop, secret);
        let enum_values = prop
            .get("enum")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        fields.push(SmitheryConfigField {
            key: key.to_string(),
            description: prop
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            required: required.contains(key),
            secret,
            kind,
            default_value: prop.get("default").cloned(),
            enum_values,
            location,
        });
    }

    fields
}

fn smithery_parameter_fields(
    connection: &SmitheryConnection,
) -> Vec<McpMarketplaceInstallParameter> {
    parse_smithery_config_fields(connection.config_schema.as_ref())
        .into_iter()
        .map(|field| McpMarketplaceInstallParameter {
            key: field.key.clone(),
            label: field.key,
            description: field.description,
            required: field.required,
            secret: field.secret,
            kind: field.kind,
            default_value: field.default_value,
            placeholder: None,
            enum_values: field.enum_values,
            location: Some(field.location),
        })
        .collect()
}

fn smithery_header_value_to_text(value: &Value) -> Option<String> {
    value_as_text(value)
}

fn smithery_query_value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).ok(),
        _ => value_as_text(value),
    }
}

fn resolve_smithery_connection_spec_with_values(
    connection: &SmitheryConnection,
    fallback_url: Option<&str>,
    values: &Map<String, Value>,
    enforce_required: bool,
) -> Result<Value, AppCommandError> {
    let protocol = smithery_connection_protocol(connection);
    let url = smithery_connection_url(connection, fallback_url)
        .ok_or_else(|| mcp_configuration_invalid("smithery connection missing deployment URL"))?;

    let config_fields = parse_smithery_config_fields(connection.config_schema.as_ref());
    let mut next_url = url;
    let mut headers = Map::new();

    for field in config_fields {
        let mut value = values.get(&field.key).cloned();
        if value.is_none() {
            value = field.default_value.clone();
        }

        let Some(value) = value else {
            if enforce_required && field.required {
                return Err(mcp_invalid_input(format!(
                    "missing required configuration '{}'",
                    field.key
                )));
            }
            continue;
        };

        if field.location == "header" {
            if let Some(text) = smithery_header_value_to_text(&value) {
                headers.insert(field.key, Value::String(text));
            } else if enforce_required && field.required {
                return Err(mcp_invalid_input(format!(
                    "invalid configuration value '{}'",
                    field.key
                )));
            }
            continue;
        }

        if let Some(text) = smithery_query_value_to_text(&value) {
            next_url = append_query_param(&next_url, &field.key, &text);
        } else if enforce_required && field.required {
            return Err(mcp_invalid_input(format!(
                "invalid configuration value '{}'",
                field.key
            )));
        }
    }

    let mut spec = Map::new();
    spec.insert("type".to_string(), Value::String(protocol));
    spec.insert("url".to_string(), Value::String(next_url));
    if !headers.is_empty() {
        spec.insert("headers".to_string(), Value::Object(headers));
    }

    canonicalize_spec(&Value::Object(spec), "smithery install")
}

fn build_smithery_install_options(
    server: &SmitheryServerDetail,
) -> Result<Vec<McpMarketplaceInstallOption>, AppCommandError> {
    let mut options = Vec::new();
    for (index, connection) in server.connections.iter().enumerate() {
        let protocol = smithery_connection_protocol(connection);
        if let Ok(spec) = resolve_smithery_connection_spec_with_values(
            connection,
            server.deployment_url.as_deref(),
            &Map::new(),
            false,
        ) {
            options.push(McpMarketplaceInstallOption {
                id: smithery_option_id(index, &protocol),
                protocol: protocol.clone(),
                label: format!("{protocol} (connection {})", index + 1),
                description: connection
                    .deployment_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                spec,
                parameters: smithery_parameter_fields(connection),
            });
        }
    }

    if options.is_empty() {
        if let Some(fallback) = server
            .deployment_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let spec = canonicalize_spec(
                &json!({
                    "type": "http",
                    "url": fallback,
                }),
                "smithery fallback",
            )?;
            options.push(McpMarketplaceInstallOption {
                id: "smithery:fallback:http".to_string(),
                protocol: "http".to_string(),
                label: "http".to_string(),
                description: Some(fallback.to_string()),
                spec,
                parameters: Vec::new(),
            });
        }
    }

    if options.is_empty() {
        return Err(mcp_not_found(format!(
            "smithery server '{}' does not provide installable connection info",
            server.qualified_name
        )));
    }

    Ok(options)
}

fn resolve_smithery_install_spec_with_selection(
    server: &SmitheryServerDetail,
    selection: &InstallSelection,
) -> Result<Value, AppCommandError> {
    let options = build_smithery_install_options(server)?;
    let selected = select_option_from_list(&options, selection)?;

    if let Some(index) = parse_smithery_option_id(&selected.id) {
        let connection = server.connections.get(index).ok_or_else(|| {
            mcp_not_found(format!(
                "selected smithery connection is out of range: {index}"
            ))
        })?;
        return resolve_smithery_connection_spec_with_values(
            connection,
            server.deployment_url.as_deref(),
            &selection.parameter_values,
            true,
        );
    }

    canonicalize_spec(&selected.spec, "smithery selected option")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_mcp_type_canonical_pass_through() {
        assert_eq!(normalize_mcp_type("stdio"), Some("stdio"));
        assert_eq!(normalize_mcp_type("http"), Some("http"));
        assert_eq!(normalize_mcp_type("sse"), Some("sse"));
        assert_eq!(normalize_mcp_type("local"), Some("local"));
        assert_eq!(normalize_mcp_type("remote"), Some("remote"));
    }

    #[test]
    fn normalize_mcp_type_streamable_http_aliases_collapse_to_http() {
        for raw in [
            "streamable-http",
            "streamableHttp",
            "streamable_http",
            "Streamable HTTP",
            "STREAMABLE-HTTP",
            "  streamable-http  ",
            "streamable.http",
        ] {
            assert_eq!(normalize_mcp_type(raw), Some("http"), "input {raw:?}");
        }
    }

    #[test]
    fn normalize_mcp_type_rejects_unknown() {
        assert!(normalize_mcp_type("").is_none());
        assert!(normalize_mcp_type("   ").is_none());
        assert!(normalize_mcp_type("Foo").is_none());
        assert!(normalize_mcp_type("ws").is_none());
    }

    #[test]
    fn kimi_code_mcp_json_round_trips() {
        // Kimi reads `<KIMI_CODE_HOME>/mcp.json` (`mcpServers`) natively; verify
        // the read/upsert/remove cycle against an isolated path.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mcp.json");

        // Missing file → no servers, and removing is a no-op.
        assert!(read_kimi_code_servers_at(&path)
            .expect("read missing")
            .is_empty());
        assert!(!remove_kimi_code_server_at(&path, "ctx7").expect("remove missing"));

        // Upsert a stdio server.
        let spec = json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "ctx7-mcp"],
        });
        upsert_kimi_code_server_at(&path, "ctx7", &spec).expect("upsert");

        // It round-trips, canonicalized, under `mcpServers`.
        let servers = read_kimi_code_servers_at(&path).expect("read back");
        assert_eq!(servers.len(), 1);
        let stored = servers.get("ctx7").expect("ctx7 present");
        assert_eq!(stored.get("type").and_then(Value::as_str), Some("stdio"));
        assert_eq!(stored.get("command").and_then(Value::as_str), Some("npx"));

        // On-disk shape is `{ "mcpServers": { "ctx7": { .. } } }`.
        let raw = std::fs::read_to_string(&path).expect("read file");
        let root: Value = serde_json::from_str(&raw).expect("parse json");
        assert!(root
            .get("mcpServers")
            .and_then(Value::as_object)
            .map(|m| m.contains_key("ctx7"))
            .unwrap_or(false));

        // Remove it; the file no longer lists it and a second remove is a no-op.
        assert!(remove_kimi_code_server_at(&path, "ctx7").expect("remove"));
        assert!(read_kimi_code_servers_at(&path)
            .expect("read after remove")
            .is_empty());
        assert!(!remove_kimi_code_server_at(&path, "ctx7").expect("remove again"));
    }

    #[test]
    fn cursor_mcp_json_round_trips_and_strips_type() {
        // Cursor reads `~/.cursor/mcp.json` (`mcpServers`) natively, shape-
        // discriminated (command ⇒ stdio, url ⇒ remote) with NO `type` key —
        // the writer must emit only the fields Cursor models, and the reader
        // must re-infer transport rather than trusting a foreign `type`.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mcp.json");

        assert!(read_cursor_servers_at(&path).expect("read missing").is_empty());
        assert!(!remove_cursor_server_at(&path, "ctx7").expect("remove missing"));

        // Upsert a stdio server; the canonical `type` must not reach disk.
        let spec = json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "ctx7-mcp"],
            "env": { "TOKEN": "t" },
        });
        upsert_cursor_server_at(&path, "ctx7", &spec).expect("upsert");
        let raw = std::fs::read_to_string(&path).expect("read file");
        let root: Value = serde_json::from_str(&raw).expect("parse json");
        let on_disk = root.pointer("/mcpServers/ctx7").expect("entry on disk");
        assert!(on_disk.get("type").is_none(), "no type key on disk");
        assert_eq!(on_disk.get("command").and_then(Value::as_str), Some("npx"));

        // Read-back canonicalizes (command ⇒ stdio).
        let servers = read_cursor_servers_at(&path).expect("read back");
        assert_eq!(
            servers.get("ctx7").and_then(|s| s.get("type")).and_then(Value::as_str),
            Some("stdio")
        );

        // A remote entry keeps url/headers only; a foreign on-disk `type` is
        // ignored on read (shape wins, like the CLI's Zod parse).
        upsert_cursor_server_at(
            &path,
            "remote",
            &json!({"type": "sse", "url": "https://mcp.example.com/sse"}),
        )
        .expect("upsert remote");
        let raw2 = std::fs::read_to_string(&path).expect("read file 2");
        let root2: Value = serde_json::from_str(&raw2).expect("parse json 2");
        assert!(root2.pointer("/mcpServers/remote/type").is_none());
        let servers2 = read_cursor_servers_at(&path).expect("read back 2");
        assert_eq!(
            servers2.get("remote").and_then(|s| s.get("type")).and_then(Value::as_str),
            Some("http"),
            "url-only entries classify as http (Cursor auto-negotiates)"
        );

        // Remove round-trips.
        assert!(remove_cursor_server_at(&path, "ctx7").expect("remove"));
        assert!(remove_cursor_server_at(&path, "remote").expect("remove remote"));
        assert!(read_cursor_servers_at(&path).expect("read after remove").is_empty());
    }

    #[test]
    fn grok_config_toml_round_trips_and_preserves_sections() {
        // Grok reads `<GROK_HOME>/config.toml` `[mcp_servers.<name>]` natively —
        // same table as Codex but with NO `type` key (transport inferred). The
        // file also holds unrelated `[cli]`/`[ui]` sections that must survive.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[cli]\nauto_update = true\n\n[ui]\nyolo = false\n",
        )
        .expect("seed config");

        // Missing entry → no servers; removing is a no-op.
        assert!(read_grok_servers_at(&path).expect("read seed").is_empty());
        assert!(!remove_grok_server_at(&path, "fs").expect("remove missing"));

        // Upsert a stdio server carrying command/args/env/cwd.
        let stdio = json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
            "env": { "TOKEN": "sk-abc" },
            "cwd": "/work/dir",
        });
        upsert_grok_server_at(&path, "fs", &stdio).expect("upsert stdio");

        // Upsert a remote server with headers (Grok uses `headers`, not
        // Codex's `http_headers`).
        let http = json!({
            "type": "http",
            "url": "https://mcp.example.com/mcp",
            "headers": { "Authorization": "Bearer xyz" },
        });
        upsert_grok_server_at(&path, "remote", &http).expect("upsert http");

        // Upsert an SSE server — Grok marks these with an explicit `type = "sse"`;
        // without it the entry would round-trip back to `http`.
        let sse = json!({ "type": "sse", "url": "https://mcp.linear.app/sse" });
        upsert_grok_server_at(&path, "linear", &sse).expect("upsert sse");

        // All round-trip, canonicalized, with cwd + headers + sse transport kept.
        let servers = read_grok_servers_at(&path).expect("read back");
        assert_eq!(servers.len(), 3);
        let fs = servers.get("fs").expect("fs present");
        assert_eq!(fs.get("type").and_then(Value::as_str), Some("stdio"));
        assert_eq!(fs.get("command").and_then(Value::as_str), Some("npx"));
        assert_eq!(fs.get("cwd").and_then(Value::as_str), Some("/work/dir"));
        assert_eq!(
            fs.pointer("/env/TOKEN").and_then(Value::as_str),
            Some("sk-abc")
        );
        let remote = servers.get("remote").expect("remote present");
        assert_eq!(remote.get("type").and_then(Value::as_str), Some("http"));
        assert_eq!(
            remote.pointer("/headers/Authorization").and_then(Value::as_str),
            Some("Bearer xyz")
        );
        let linear = servers.get("linear").expect("linear present");
        assert_eq!(linear.get("type").and_then(Value::as_str), Some("sse"));
        assert_eq!(
            linear.get("url").and_then(Value::as_str),
            Some("https://mcp.linear.app/sse")
        );

        // On-disk: `[mcp_servers.fs]` has NO `type` key, `[cli]`/`[ui]` survive.
        let raw = std::fs::read_to_string(&path).expect("read file");
        let root: toml::Value = raw.parse().expect("parse toml");
        let table = root.as_table().expect("root table");
        assert!(table.contains_key("cli"), "[cli] preserved");
        assert!(table.contains_key("ui"), "[ui] preserved");
        let fs_entry = table
            .get("mcp_servers")
            .and_then(toml::Value::as_table)
            .and_then(|m| m.get("fs"))
            .and_then(toml::Value::as_table)
            .expect("mcp_servers.fs");
        assert!(!fs_entry.contains_key("type"), "stdio entries omit `type`");
        assert_eq!(
            fs_entry.get("cwd").and_then(toml::Value::as_str),
            Some("/work/dir")
        );
        // SSE entries, by contrast, must keep the explicit `type = "sse"`.
        let linear_entry = table
            .get("mcp_servers")
            .and_then(toml::Value::as_table)
            .and_then(|m| m.get("linear"))
            .and_then(toml::Value::as_table)
            .expect("mcp_servers.linear");
        assert_eq!(
            linear_entry.get("type").and_then(toml::Value::as_str),
            Some("sse")
        );

        // Remove one; the others and the unrelated sections remain.
        assert!(remove_grok_server_at(&path, "fs").expect("remove fs"));
        let after = read_grok_servers_at(&path).expect("read after remove");
        assert_eq!(after.len(), 2);
        assert!(after.contains_key("remote"));
        assert!(after.contains_key("linear"));
        let raw2 = std::fs::read_to_string(&path).expect("read file 2");
        let root2: toml::Value = raw2.parse().expect("parse toml 2");
        assert!(root2.as_table().expect("t").contains_key("cli"));
    }

    fn codex_entry(toml_src: &str) -> toml::Value {
        toml::from_str::<toml::Value>(toml_src).expect("parse test toml")
    }

    #[test]
    fn codex_entry_canonicalizes_streamable_http_aliases() {
        for raw in ["streamableHttp", "streamable-http", "streamable_http"] {
            let value = codex_entry(&format!(
                "type = \"{raw}\"\nurl = \"https://mcp.example.com/mcp\"\n"
            ));
            let canonical = codex_entry_to_canonical("ex", &value)
                .unwrap_or_else(|err| panic!("input {raw:?} should normalize: {err}"));
            assert_eq!(
                canonical
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "http",
                "input {raw:?}"
            );
            assert_eq!(
                canonical
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "https://mcp.example.com/mcp"
            );
        }
    }

    #[test]
    fn codex_entry_keeps_canonical_types_intact() {
        let stdio = codex_entry("type = \"stdio\"\ncommand = \"npx\"\n");
        let canonical = codex_entry_to_canonical("ex", &stdio).expect("stdio entry");
        assert_eq!(canonical.get("type").and_then(Value::as_str), Some("stdio"));
        assert_eq!(
            canonical.get("command").and_then(Value::as_str),
            Some("npx")
        );

        let sse = codex_entry("type = \"sse\"\nurl = \"https://mcp.example.com/sse\"\n");
        let canonical = codex_entry_to_canonical("ex", &sse).expect("sse entry");
        assert_eq!(canonical.get("type").and_then(Value::as_str), Some("sse"));
    }

    #[test]
    fn codex_entry_rejects_unknown_type_with_raw_in_message() {
        let value = codex_entry("type = \"Foo\"\nurl = \"https://x\"\n");
        let err = codex_entry_to_canonical("ex", &value).expect_err("Foo should be rejected");
        let msg = err.to_string();
        assert!(msg.contains("'Foo'"), "error should echo raw type: {msg}");
        assert!(msg.contains("'ex'"), "error should mention id: {msg}");
        assert_eq!(
            err.i18n_key.as_deref(),
            Some("errors.codexEntryUnsupportedType")
        );
        let params = err.i18n_params.as_ref().expect("i18n params attached");
        assert_eq!(params.get("id").map(String::as_str), Some("ex"));
        assert_eq!(params.get("type").map(String::as_str), Some("Foo"));
    }

    #[test]
    fn codex_entry_rejects_opencode_only_aliases() {
        // OpenCode-native types are not valid in Codex TOML; catching them keeps
        // the Codex pipeline's accepted set tight.
        for raw in ["local", "remote"] {
            let value = codex_entry(&format!("type = \"{raw}\"\nurl = \"https://x\"\n"));
            assert!(
                codex_entry_to_canonical("ex", &value).is_err(),
                "raw {raw:?} should not be accepted by Codex pipeline",
            );
        }
    }

    #[test]
    fn canonical_to_codex_entry_never_emits_type_field() {
        // Codex infers the transport from the keys present; an emitted `type` is
        // schema-invalid and fatal under `codex --strict-config` (#325). No
        // transport may emit it.
        let stdio = canonical_to_codex_entry(&json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "tavily-mcp@0.2.15"],
        }))
        .expect("stdio entry")
        .as_table()
        .cloned()
        .expect("stdio table");
        assert!(!stdio.contains_key("type"), "stdio must not carry type");
        assert_eq!(
            stdio.get("command").and_then(toml::Value::as_str),
            Some("npx")
        );

        let http = canonical_to_codex_entry(&json!({
            "type": "http",
            "url": "https://mcp.exa.ai/mcp",
        }))
        .expect("http entry")
        .as_table()
        .cloned()
        .expect("http table");
        assert!(!http.contains_key("type"), "http must not carry type");
        assert_eq!(
            http.get("url").and_then(toml::Value::as_str),
            Some("https://mcp.exa.ai/mcp")
        );

        // Codex can't represent SSE (its config.toml has only stdio + streamable
        // HTTP); the writer rejects it rather than degrade to a bare `url` that
        // would read back as `http` and reclassify the shared spec.
        assert!(
            canonical_to_codex_entry(&json!({
                "type": "sse",
                "url": "https://mcp.example.com/sse",
            }))
            .is_err(),
            "sse must be rejected for Codex"
        );
    }

    #[test]
    fn app_can_host_spec_excludes_codex_from_sse_only() {
        let sse = json!({"type": "sse", "url": "https://x/sse"});
        let http = json!({"type": "http", "url": "https://x/mcp"});
        let stdio = json!({"type": "stdio", "command": "npx"});
        // Codex can host stdio/http but not sse.
        assert!(!app_can_host_spec(McpAppType::Codex, &sse));
        assert!(app_can_host_spec(McpAppType::Codex, &http));
        assert!(app_can_host_spec(McpAppType::Codex, &stdio));
        // Every other agent can host sse.
        assert!(app_can_host_spec(McpAppType::Gemini, &sse));
        assert!(app_can_host_spec(McpAppType::Cline, &sse));
        assert!(app_can_host_spec(McpAppType::KimiCode, &sse));
    }

    #[test]
    fn codex_entry_infers_transport_when_type_absent() {
        // Native Codex tables (and codeg's own post-#325 output) carry no `type`;
        // the reader must infer it from the transport keys, not assume stdio (which
        // silently dropped every url-only server). Mirrors the issue's config.
        let http = codex_entry("url = \"https://mcp.exa.ai/mcp\"\n");
        let canonical = codex_entry_to_canonical("exa", &http).expect("url-only entry");
        assert_eq!(canonical.get("type").and_then(Value::as_str), Some("http"));
        assert_eq!(
            canonical.get("url").and_then(Value::as_str),
            Some("https://mcp.exa.ai/mcp")
        );

        let stdio = codex_entry("command = \"npx\"\nargs = [\"-y\", \"tavily-mcp@0.2.15\"]\n");
        let canonical = codex_entry_to_canonical("tavily", &stdio).expect("command-only entry");
        assert_eq!(canonical.get("type").and_then(Value::as_str), Some("stdio"));
        assert_eq!(
            canonical.get("command").and_then(Value::as_str),
            Some("npx")
        );
    }

    #[test]
    fn codex_write_read_round_trips_without_type_key() {
        // The writer's output must read back to the same canonical spec with no
        // `type` ever hitting disk. (sse is excluded: Codex rejects it — covered by
        // `canonical_to_codex_entry_never_emits_type_field`.)
        for spec in [
            json!({"type": "stdio", "command": "npx", "args": ["-y", "srv"], "env": {"A": "b"}}),
            json!({"type": "http", "url": "https://mcp.exa.ai/mcp"}),
        ] {
            let entry = canonical_to_codex_entry(&spec).expect("to codex entry");
            assert!(
                !entry.as_table().expect("table").contains_key("type"),
                "no type on disk for {spec}"
            );
            let back = codex_entry_to_canonical("id", &entry).expect("read back");
            assert_eq!(
                back.get("type").and_then(Value::as_str),
                spec.get("type").and_then(Value::as_str),
                "round-trip type for {spec}"
            );
        }
    }

    #[test]
    fn canonical_to_cline_entry_remaps_http_to_streamable_http() {
        // Cline's zod `type` literal accepts only stdio|sse|streamableHttp; the
        // canonical `http` must become `streamableHttp` or Cline drops every
        // server (#325). stdio/sse pass through unchanged.
        let http = canonical_to_cline_entry(&json!({
            "type": "http",
            "url": "https://mcp.exa.ai/mcp",
        }))
        .expect("http entry");
        assert_eq!(
            http.get("type").and_then(Value::as_str),
            Some("streamableHttp"),
            "http must be remapped for Cline"
        );
        assert_eq!(
            http.get("url").and_then(Value::as_str),
            Some("https://mcp.exa.ai/mcp")
        );

        let stdio = canonical_to_cline_entry(&json!({"type": "stdio", "command": "npx"}))
            .expect("stdio entry");
        assert_eq!(stdio.get("type").and_then(Value::as_str), Some("stdio"));

        let sse = canonical_to_cline_entry(&json!({"type": "sse", "url": "https://x/sse"}))
            .expect("sse entry");
        assert_eq!(sse.get("type").and_then(Value::as_str), Some("sse"));

        // And codeg reads `streamableHttp` straight back to canonical `http`.
        let round_trip = canonicalize_spec(
            &json!({"type": "streamableHttp", "url": "https://mcp.exa.ai/mcp"}),
            "test",
        )
        .expect("canonicalize streamableHttp");
        assert_eq!(round_trip.get("type").and_then(Value::as_str), Some("http"));
    }

    #[test]
    fn canonical_to_kimi_code_entry_pins_remote_transport() {
        // Kimi 0.23.3 keys the transport off `transport` (defaulting url-only to
        // HTTP), so codeg must emit an explicit `transport` or an SSE server silently
        // downgrades to HTTP (#325). stdio is left as-is (Kimi infers it from
        // `command`).
        let sse = canonical_to_kimi_code_entry(&json!({"type": "sse", "url": "https://x/stream"}))
            .expect("sse entry");
        assert_eq!(sse.get("transport").and_then(Value::as_str), Some("sse"));
        assert_eq!(sse.get("type").and_then(Value::as_str), Some("sse"));

        let http = canonical_to_kimi_code_entry(&json!({"type": "http", "url": "https://x/mcp"}))
            .expect("http entry");
        assert_eq!(http.get("transport").and_then(Value::as_str), Some("http"));

        let stdio = canonical_to_kimi_code_entry(&json!({"type": "stdio", "command": "npx"}))
            .expect("stdio entry");
        assert!(
            stdio.get("transport").is_none(),
            "stdio must not carry a transport key"
        );
    }

    #[test]
    fn kimi_code_entry_reads_native_transport_and_never_leaks_it() {
        // A native Kimi SSE entry uses `transport: "sse"`, not `type`; the reader
        // must classify it as sse from that explicit `transport` and must NOT surface
        // `transport` in the canonical spec — otherwise it would leak into e.g. Codex
        // TOML when the same server is later synced to another agent (#325).
        let native_sse = json!({"url": "https://x/stream", "transport": "sse"});
        let canonical = kimi_code_entry_to_canonical(&native_sse, "srv").expect("native sse");
        assert_eq!(canonical.get("type").and_then(Value::as_str), Some("sse"));
        assert!(
            canonical.get("transport").is_none(),
            "transport must be consumed, never leaked into the canonical spec"
        );

        // Full writer→reader round-trip stays canonical and transport-free.
        let written = canonical_to_kimi_code_entry(&json!({"type": "sse", "url": "https://x/stream"}))
            .expect("write sse");
        let back = kimi_code_entry_to_canonical(&written, "srv").expect("read back");
        assert_eq!(back.get("type").and_then(Value::as_str), Some("sse"));
        assert!(back.get("transport").is_none());
    }

    #[test]
    fn kimi_code_entry_mirrors_kimi_0_23_transport_selection() {
        // Kimi Code 0.23.3 defaults a url-only remote entry to HTTP and does NOT
        // infer SSE from a `/sse` URL path — only an explicit `transport: "sse"`
        // yields SSE. (Corrects the earlier FastMCP-based reader; verified against
        // the published 0.23.3 Zod schema.)
        let sse_url = kimi_code_entry_to_canonical(&json!({"url": "https://host/sse"}), "s")
            .expect("url-only /sse");
        assert_eq!(
            sse_url.get("type").and_then(Value::as_str),
            Some("http"),
            "url-only must be http, not sse-from-url"
        );

        let http_url = kimi_code_entry_to_canonical(&json!({"url": "https://host/mcp"}), "s")
            .expect("url-only");
        assert_eq!(http_url.get("type").and_then(Value::as_str), Some("http"));

        // An on-disk `type` with NO `transport` does not classify: Kimi strips `type`
        // and infers HTTP from the url, so codeg must too (not report it as SSE).
        let stale_type = kimi_code_entry_to_canonical(
            &json!({"type": "sse", "url": "https://host/mcp"}),
            "s",
        )
        .expect("type-without-transport");
        assert_eq!(stale_type.get("type").and_then(Value::as_str), Some("http"));

        // Explicit `transport: "sse"` yields SSE (and `type` is ignored, matching
        // Kimi); `transport` is stripped from the canonical spec.
        let sse = kimi_code_entry_to_canonical(
            &json!({"type": "http", "url": "https://host/mcp", "transport": "sse"}),
            "s",
        )
        .expect("explicit sse");
        assert_eq!(sse.get("type").and_then(Value::as_str), Some("sse"));
        assert!(sse.get("transport").is_none());

        // An explicit unknown transport Kimi would hard-reject is surfaced as an
        // invalid entry, not reported as an active server. (`stdio` on a url-only
        // entry is likewise invalid — Kimi's stdio variant requires `command`.)
        for bad in ["streamable-http", "ws", "stdio"] {
            assert!(
                kimi_code_entry_to_canonical(
                    &json!({"url": "https://host/mcp", "transport": bad}),
                    "s"
                )
                .is_err(),
                "transport {bad:?} must be rejected"
            );
        }
        // A non-string transport is rejected too (Kimi's literals are exact).
        assert!(
            kimi_code_entry_to_canonical(&json!({"url": "https://host/mcp", "transport": 3}), "s")
                .is_err()
        );

        // The `transport` discriminant wins over the entry's key shape: an explicit
        // `sse` on an entry that ALSO carries `command` is SSE (Kimi ignores the
        // extra `command`), not stdio.
        let sse_over_cmd = kimi_code_entry_to_canonical(
            &json!({"transport": "sse", "command": "npx", "url": "https://host/mcp"}),
            "s",
        )
        .expect("transport wins over command");
        assert_eq!(sse_over_cmd.get("type").and_then(Value::as_str), Some("sse"));
    }

    #[test]
    fn canonical_to_kimi_code_entry_drops_wrong_typed_and_foreign_fields() {
        // Kimi validates its known fields and rejects the whole `mcpServers` record on
        // a wrong-typed one, so the writer must not let a stray same-named foreign
        // value ride canonicalize's passthrough onto disk. See #325.
        let entry = canonical_to_kimi_code_entry(&json!({
            "type": "http",
            "url": "https://host/mcp",
            "enabled": "false",   // wrong shape (string, not bool) → dropped
            "autoApprove": ["a"], // foreign key → dropped
        }))
        .expect("http entry");
        let obj = entry.as_object().expect("object");
        assert_eq!(obj.get("transport").and_then(Value::as_str), Some("http"));
        assert!(!obj.contains_key("enabled"), "wrong-typed enabled must be dropped");
        assert!(!obj.contains_key("autoApprove"), "foreign key must be dropped");

        // A correctly-typed `enabled` bool is preserved.
        let ok = canonical_to_kimi_code_entry(&json!({
            "type": "http", "url": "https://host/mcp", "enabled": true,
        }))
        .expect("http entry");
        assert_eq!(
            ok.as_object().and_then(|o| o.get("enabled")).and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn codex_entry_rejects_both_command_and_url() {
        // Codex hard-errors on a mixed-transport entry; codeg must reject it rather
        // than silently classify as stdio and drop the `url` (#325).
        let both = codex_entry("command = \"npx\"\nurl = \"https://x/mcp\"\n");
        assert!(codex_entry_to_canonical("mixed", &both).is_err());
        // Rejected even when an explicit (legacy) type is present.
        let both_typed =
            codex_entry("type = \"stdio\"\ncommand = \"npx\"\nurl = \"https://x/mcp\"\n");
        assert!(codex_entry_to_canonical("mixed", &both_typed).is_err());
    }

    #[test]
    fn canonical_to_codex_entry_passthrough_is_type_validated() {
        // Foreign keys, transport-specific fields, and — crucially — same-named
        // fields of the WRONG shape must NOT reach Codex TOML; each is fatal under
        // --strict-config. Only transport-agnostic fields validated to Codex's exact
        // type pass through. See #325.
        let entry = canonical_to_codex_entry(&json!({
            "type": "http",
            "url": "https://mcp.exa.ai/mcp",
            "enabled": true,             // valid Codex bool → kept
            "required": "yes",           // wrong shape (string, not bool) → dropped
            "autoApprove": ["a"],        // foreign key → dropped
            "transport": "sse",          // canonical-only discriminator → dropped
            "env_vars": [{"name": "X"}], // stdio-only, wrong arm here → dropped
            "startup_timeout_sec": 10.0, // not in the minimal allowlist → dropped
        }))
        .expect("http entry")
        .as_table()
        .cloned()
        .expect("table");
        assert_eq!(entry.get("enabled").and_then(toml::Value::as_bool), Some(true));
        for dropped in [
            "type",
            "required",
            "autoApprove",
            "transport",
            "env_vars",
            "startup_timeout_sec",
        ] {
            assert!(
                !entry.contains_key(dropped),
                "'{dropped}' must be dropped from Codex TOML"
            );
        }
    }

    #[test]
    fn transport_protocol_normalizes_aliases() {
        assert_eq!(transport_protocol("stdio"), Some("stdio".to_string()));
        assert_eq!(transport_protocol("http"), Some("http".to_string()));
        assert_eq!(transport_protocol("sse"), Some("sse".to_string()));
        assert_eq!(
            transport_protocol("streamable-http"),
            Some("http".to_string())
        );
        assert_eq!(
            transport_protocol("streamableHttp"),
            Some("http".to_string())
        );
        assert_eq!(transport_protocol("local"), None);
        assert_eq!(transport_protocol("foo"), None);
    }

    fn make_transport(kind: &str, url: &str) -> OfficialTransport {
        let payload = serde_json::json!({
            "type": kind,
            "url": url,
        });
        serde_json::from_value(payload).expect("OfficialTransport from json")
    }

    #[test]
    fn remote_spec_from_transport_normalizes_aliases() {
        for raw in ["streamable-http", "streamableHttp", "http"] {
            let transport = make_transport(raw, "https://mcp.example.com/mcp");
            let spec =
                remote_spec_from_transport_with_values(&transport, &Map::new(), false).unwrap();
            assert_eq!(
                spec.get("type").and_then(Value::as_str),
                Some("http"),
                "raw {raw:?}"
            );
        }

        let sse = make_transport("sse", "https://mcp.example.com/sse");
        let spec = remote_spec_from_transport_with_values(&sse, &Map::new(), false).unwrap();
        assert_eq!(spec.get("type").and_then(Value::as_str), Some("sse"));

        let unknown = make_transport("ws", "https://x");
        let err = remote_spec_from_transport_with_values(&unknown, &Map::new(), false)
            .expect_err("ws should be rejected");
        assert_eq!(
            err.i18n_key.as_deref(),
            Some("errors.unsupportedTransportType")
        );
        let params = err.i18n_params.as_ref().expect("i18n params attached");
        assert_eq!(params.get("type").map(String::as_str), Some("ws"));
    }

    fn make_smithery_connection(kind: &str) -> SmitheryConnection {
        let payload = serde_json::json!({ "type": kind });
        serde_json::from_value(payload).expect("SmitheryConnection from json")
    }

    #[test]
    fn smithery_connection_protocol_normalizes_aliases() {
        assert_eq!(
            smithery_connection_protocol(&make_smithery_connection("streamable-http")),
            "http"
        );
        assert_eq!(
            smithery_connection_protocol(&make_smithery_connection("streamableHttp")),
            "http"
        );
        assert_eq!(
            smithery_connection_protocol(&make_smithery_connection("sse")),
            "sse"
        );
        // Unknown falls back to http (preserves prior permissive behavior).
        assert_eq!(
            smithery_connection_protocol(&make_smithery_connection("ws")),
            "http"
        );
    }

    fn hermes_entry(yaml_src: &str) -> serde_yaml::Value {
        serde_yaml::from_str::<serde_yaml::Value>(yaml_src).expect("parse test yaml")
    }

    #[test]
    fn hermes_entry_to_canonical_stdio() {
        let entry = hermes_entry(
            "command: npx\nargs:\n  - -y\n  - \"@modelcontextprotocol/server-github\"\nenv:\n  GITHUB_TOKEN: ghp_x\n",
        );
        let spec = hermes_entry_to_canonical(&entry, "github").expect("canonical");
        assert_eq!(spec.get("type").and_then(Value::as_str), Some("stdio"));
        assert_eq!(spec.get("command").and_then(Value::as_str), Some("npx"));
        let args = spec.get("args").and_then(Value::as_array).expect("args");
        assert_eq!(args.len(), 2);
        assert_eq!(
            spec.get("env")
                .and_then(|e| e.get("GITHUB_TOKEN"))
                .and_then(Value::as_str),
            Some("ghp_x")
        );
    }

    #[test]
    fn hermes_entry_to_canonical_http_and_sse() {
        // A bare `url` is StreamableHTTP.
        let http = hermes_entry_to_canonical(
            &hermes_entry("url: https://mcp.example.com/mcp\n"),
            "remote-http",
        )
        .expect("http canonical");
        assert_eq!(http.get("type").and_then(Value::as_str), Some("http"));
        assert_eq!(
            http.get("url").and_then(Value::as_str),
            Some("https://mcp.example.com/mcp")
        );
        // `transport: sse` maps to the canonical `sse` type.
        let sse = hermes_entry_to_canonical(
            &hermes_entry("url: http://localhost:8000/sse\ntransport: sse\n"),
            "remote-sse",
        )
        .expect("sse canonical");
        assert_eq!(sse.get("type").and_then(Value::as_str), Some("sse"));
    }

    #[test]
    fn canonical_to_hermes_entry_drops_type_and_maps_transport() {
        // stdio → command/args/env, no `type`/`transport` keys.
        let stdio = canonical_to_hermes_entry(&json!({
            "type": "stdio",
            "command": "uvx",
            "args": ["some-server"],
            "env": {"KEY": "v"},
        }))
        .expect("stdio entry");
        let map = stdio.as_mapping().expect("mapping");
        assert!(map.contains_key(serde_yaml::Value::String("command".into())));
        assert!(!map.contains_key(serde_yaml::Value::String("type".into())));
        assert!(!map.contains_key(serde_yaml::Value::String("transport".into())));

        // sse → url + `transport: sse`, no `type`; mTLS keys pass through.
        let sse = canonical_to_hermes_entry(&json!({
            "type": "sse",
            "url": "https://x/sse",
            "headers": {"Authorization": "Bearer t"},
            "client_cert": "/tmp/cert.pem",
        }))
        .expect("sse entry");
        let map = sse.as_mapping().expect("mapping");
        assert_eq!(
            map.get(serde_yaml::Value::String("transport".into()))
                .and_then(serde_yaml::Value::as_str),
            Some("sse")
        );
        assert!(!map.contains_key(serde_yaml::Value::String("type".into())));
        assert_eq!(
            map.get(serde_yaml::Value::String("client_cert".into()))
                .and_then(serde_yaml::Value::as_str),
            Some("/tmp/cert.pem")
        );
    }

    #[test]
    fn hermes_mcp_canonical_round_trips() {
        // canonical → hermes entry → canonical is stable for both transports.
        for spec in [
            json!({"type": "stdio", "command": "npx", "args": ["-y", "srv"], "env": {"A": "b"}}),
            json!({"type": "sse", "url": "https://x/sse", "headers": {"H": "v"}}),
            json!({"type": "http", "url": "https://x/mcp"}),
        ] {
            let entry = canonical_to_hermes_entry(&spec).expect("to entry");
            let back = hermes_entry_to_canonical(&entry, "srv").expect("from entry");
            let canonical = canonicalize_spec(&spec, "expected").expect("canonical");
            assert_eq!(back, canonical, "round-trip mismatch for {spec}");
        }
    }
}
