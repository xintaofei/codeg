use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProviderInfo {
    pub id: i32,
    pub name: String,
    pub api_url: String,
    pub api_key: String,
    pub api_key_masked: String,
    pub agent_type: String,
    pub model: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CcSwitchModelProviderSkipReason {
    UnsupportedAppType,
    MissingName,
    MissingApiUrl,
    MissingApiKey,
    InvalidModel,
    DuplicateName,
    DuplicateConfig,
    MalformedSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchModelProviderPreviewItem {
    pub source_id: String,
    pub source_app_type: String,
    pub target_agent_type: String,
    pub name: String,
    pub api_url: Option<String>,
    pub model: Option<String>,
    pub importable: bool,
    pub skip_reason: Option<CcSwitchModelProviderSkipReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListImportableCcSwitchModelProvidersResult {
    pub available: bool,
    pub source_path: String,
    pub items: Vec<CcSwitchModelProviderPreviewItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportCcSwitchModelProvidersRequest {
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub overwrite_same_name: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportCcSwitchModelProvidersResult {
    pub imported_ids: Vec<i32>,
    pub skipped: Vec<CcSwitchModelProviderPreviewItem>,
}

fn mask_api_key(key: &str) -> String {
    // Operate on Unicode scalar values, not bytes: an API key may contain a
    // multibyte character (e.g. a full-width char typed with a CJK IME), and
    // byte-slicing `&key[..4]` would panic on a non-char-boundary. Such a panic
    // in `From` propagates out of every `list_model_providers` call once the
    // row is persisted, permanently breaking the provider list.
    let chars: Vec<char> = key.chars().collect();
    let len = chars.len();
    if len <= 8 {
        "\u{2022}".repeat(len)
    } else {
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[len - 4..].iter().collect();
        format!("{}{}{}", prefix, "\u{2022}".repeat(len.min(20) - 8), suffix)
    }
}

impl From<crate::db::entities::model_provider::Model> for ModelProviderInfo {
    fn from(m: crate::db::entities::model_provider::Model) -> Self {
        let agent_type = if m.agent_type.is_empty() {
            // Fallback for rows backfilled with empty agent_type.
            serde_json::from_str::<Vec<String>>(&m.agent_types_json)
                .ok()
                .and_then(|list| list.into_iter().next())
                .unwrap_or_default()
        } else {
            m.agent_type
        };
        Self {
            id: m.id,
            name: m.name,
            api_url: m.api_url,
            api_key: m.api_key.clone(),
            api_key_masked: mask_api_key(&m.api_key),
            agent_type,
            model: m.model,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{mask_api_key, CcSwitchModelProviderPreviewItem, CcSwitchModelProviderSkipReason};

    #[test]
    fn masks_short_ascii_key() {
        assert_eq!(mask_api_key("abc123"), "\u{2022}".repeat(6));
    }

    #[test]
    fn masks_long_ascii_key_keeping_edges() {
        assert_eq!(mask_api_key("sk-test-1234567890"), "sk-t\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}7890");
    }

    #[test]
    fn does_not_panic_on_multibyte_key() {
        // Byte index 4 falls inside '密' (bytes 3..6); a byte slice would panic.
        let masked = mask_api_key("sk-密钥abcd1234");
        assert!(masked.starts_with("sk-密"));
        assert!(masked.ends_with("1234"));
    }

    #[test]
    fn masks_short_multibyte_key_without_panic() {
        assert_eq!(mask_api_key("密钥abc"), "\u{2022}".repeat(5));
    }

    #[test]
    fn cc_switch_preview_item_serializes_camel_case() {
        let item = CcSwitchModelProviderPreviewItem {
            source_id: "codex:demo".to_string(),
            source_app_type: "codex".to_string(),
            target_agent_type: "codex".to_string(),
            name: "Demo".to_string(),
            api_url: Some("https://api.example.com/v1".to_string()),
            model: Some("gpt-5".to_string()),
            importable: false,
            skip_reason: Some(CcSwitchModelProviderSkipReason::DuplicateName),
        };

        let value = serde_json::to_value(item).expect("serialize preview item");
        assert_eq!(value["sourceId"], "codex:demo");
        assert_eq!(value["sourceAppType"], "codex");
        assert_eq!(value["targetAgentType"], "codex");
        assert_eq!(value["importable"], false);
        assert_eq!(value["skipReason"], "duplicate_name");
    }
}
