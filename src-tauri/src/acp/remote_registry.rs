use std::collections::BTreeMap;

use serde::Deserialize;

use crate::acp::registry;
use crate::app_error::AppCommandError;
use crate::models::agent::AgentType;

pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

#[derive(Debug, Clone)]
pub struct RegistryAgent {
    pub agent_type: AgentType,
    pub registry_id: String,
    pub name: String,
    pub description: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RegistryBinaryRelease {
    pub version: String,
    pub archive_url: String,
}

#[derive(Debug, Deserialize)]
struct RegistryPayload {
    agents: Vec<RegistryAgentItem>,
}

#[derive(Debug, Deserialize)]
struct RegistryAgentItem {
    id: String,
    name: String,
    description: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    distribution: RegistryDistribution,
}

#[derive(Debug, Default, Deserialize)]
struct RegistryDistribution {
    #[serde(default)]
    binary: BTreeMap<String, RegistryBinaryPlatformItem>,
}

#[derive(Debug, Default, Deserialize)]
struct RegistryBinaryPlatformItem {
    #[serde(default, alias = "url")]
    archive: String,
}

async fn fetch_registry_payload() -> Result<RegistryPayload, AppCommandError> {
    let response = reqwest::Client::new()
        .get(REGISTRY_URL)
        .send()
        .await
        .map_err(|e| AppCommandError::from(format!("failed to fetch ACP registry: {e}")))?;
    if !response.status().is_success() {
        return Err(format!(
            "failed to fetch ACP registry: HTTP {}",
            response.status()
        )
        .into());
    }

    let text = response
        .text()
        .await
        .map_err(|e| AppCommandError::from(format!("failed to read ACP registry response: {e}")))?;
    serde_json::from_str::<RegistryPayload>(&text)
        .map_err(|e| AppCommandError::from(format!("failed to parse ACP registry JSON: {e}")))
}

pub async fn fetch_supported_agents() -> Result<Vec<RegistryAgent>, AppCommandError> {
    let payload = fetch_registry_payload().await?;

    let mut supported = Vec::new();
    for item in payload.agents {
        if let Some(agent_type) = registry::from_registry_id(&item.id) {
            supported.push(RegistryAgent {
                agent_type,
                registry_id: item.id,
                name: item.name,
                description: item.description,
                version: item.version,
            });
        }
    }

    Ok(supported)
}

pub async fn fetch_binary_release(
    agent_type: AgentType,
    platform: &str,
) -> Result<Option<RegistryBinaryRelease>, AppCommandError> {
    let payload = fetch_registry_payload().await?;
    let item = payload.agents.into_iter().find(|item| {
        registry::from_registry_id(&item.id)
            .map(|candidate| candidate == agent_type)
            .unwrap_or(false)
    });

    let Some(item) = item else {
        return Ok(None);
    };
    if item.version.as_deref().unwrap_or_default().is_empty() {
        return Ok(None);
    }
    let platform_item = item.distribution.binary.get(platform);
    let Some(platform_item) = platform_item else {
        return Ok(None);
    };
    if platform_item.archive.is_empty() {
        return Ok(None);
    }

    Ok(Some(RegistryBinaryRelease {
        version: item.version.unwrap_or_default(),
        archive_url: platform_item.archive.clone(),
    }))
}
