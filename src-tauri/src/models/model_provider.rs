use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProviderInfo {
    pub id: i32,
    pub name: String,
    pub api_url: String,
    pub api_key_masked: String,
    pub agent_types: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn mask_api_key(key: &str) -> String {
    let len = key.len();
    if len <= 8 {
        "\u{2022}".repeat(len)
    } else {
        format!(
            "{}{}{}",
            &key[..4],
            "\u{2022}".repeat(len.min(20) - 8),
            &key[len - 4..]
        )
    }
}

impl From<crate::db::entities::model_provider::Model> for ModelProviderInfo {
    fn from(m: crate::db::entities::model_provider::Model) -> Self {
        let agent_types: Vec<String> =
            serde_json::from_str(&m.agent_types_json).unwrap_or_default();
        Self {
            id: m.id,
            name: m.name,
            api_url: m.api_url,
            api_key_masked: mask_api_key(&m.api_key),
            agent_types,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
        }
    }
}
