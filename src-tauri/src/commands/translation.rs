//! Tauri command layer for the content-translation middleware.
//!
//! Every entry point is a thin wrapper over a `_core` function so the desktop
//! commands here and the web handlers in `web::handlers::translation` share
//! one implementation — including the rule that the stored API key never
//! leaves the backend unmasked.

use sea_orm::DatabaseConnection;
#[cfg(feature = "tauri-runtime")]
use tauri::State;

use crate::app_error::AppCommandError;
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;
use crate::translation::settings::TranslationSettings;
use crate::translation::{self, TranslationCacheStats, TranslationResult};

/// Read the saved settings, masked for display.
pub async fn translation_get_settings_core(
    conn: &DatabaseConnection,
) -> Result<TranslationSettings, AppCommandError> {
    Ok(translation::settings::load(conn).await.masked())
}

/// Validate and persist. Returns the saved settings, masked.
pub async fn translation_update_settings_core(
    conn: &DatabaseConnection,
    settings: TranslationSettings,
) -> Result<TranslationSettings, AppCommandError> {
    translation::settings::save(conn, settings).await
}

/// Prove the endpoint, key, and model resolve, using the settings the user is
/// currently looking at rather than what is stored — the point is to test an
/// unsaved form.
///
/// The masked key is the one case where the *stored* value is needed: the page
/// never holds the real key, so an untouched field arrives as the mask.
///
/// Bounded end to end: the settings page parks a spinner on this call, so a
/// slow or black-holing endpoint must surface as an error within a couple of
/// the endpoint's own request deadlines, not spin forever.
pub async fn translation_test_core(
    conn: &DatabaseConnection,
    settings: TranslationSettings,
    ui_locale: &str,
    provider_id: Option<String>,
) -> Result<String, AppCommandError> {
    let stored = translation::settings::load(conn).await;
    let candidate = resolve_candidate_settings(stored, settings);

    // Test what the user typed, not what `enabled` currently says — the whole
    // point is to check the configuration *before* switching it on.
    let candidate = translation::settings::validate(TranslationSettings {
        enabled: true,
        ..candidate
    })?;

    let target = translation::resolve_target_lang(&candidate, ui_locale);
    let test = translation::client::test_connection(
        &candidate,
        translation::display_language(&target),
        provider_id.as_deref(),
    );
    tokio::time::timeout(TEST_CONNECTION_TIMEOUT, test)
        .await
        .map_err(|_| {
            AppCommandError::network(
                "The translation endpoint did not respond within 150 seconds",
            )
        })?
}

/// Wall-clock ceiling for the settings page's connection test: one full
/// [`client::READ_TIMEOUT`] attempt plus the pacing slack around it. A
/// reasoning endpoint may genuinely need the whole window.
const TEST_CONNECTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(150);

/// Rebuild the settings the user is looking at: a form field left as the mask
/// means "unchanged", and the stored value is the only place the real key
/// lives. Per provider, the mask merges by id; the flat mirror merges by
/// itself (it is rebuilt from the pool head on save anyway). Shared by every
/// action that runs against the unsaved form.
fn resolve_candidate_settings(
    stored: TranslationSettings,
    mut incoming: TranslationSettings,
) -> TranslationSettings {
    for provider in &mut incoming.providers {
        if provider.api_key == translation::settings::API_KEY_MASK {
            provider.api_key = stored
                .providers
                .iter()
                .find(|stored| stored.id == provider.id && !stored.id.is_empty())
                .map(|stored| stored.api_key.clone())
                .unwrap_or_default();
        }
    }
    if incoming.providers.is_empty() {
        incoming.api_key = if incoming.api_key == translation::settings::API_KEY_MASK {
            stored.api_key
        } else {
            incoming.api_key
        };
    }
    incoming
}

/// Fetch the endpoint's model list for the settings page's picker. Runs
/// against the unsaved form: the masked key is refilled from storage, and the
/// model field is deliberately not required — it is what this call fills in.
/// `provider_id` aims the probe at one pool row; absent, the first active
/// member (or the legacy flat fields) serves.
pub async fn translation_list_models_core(
    conn: &DatabaseConnection,
    settings: TranslationSettings,
    provider_id: Option<String>,
) -> Result<Vec<String>, AppCommandError> {
    let stored = translation::settings::load(conn).await;
    let candidate = resolve_candidate_settings(stored, settings);

    translation::client::list_models(&candidate, provider_id.as_deref()).await
}

/// The rotation pool's live state for the settings page's status badges.
/// The pool's shape comes from the stored settings; the rates, cooldowns, and
/// session disables are process-wide runtime memory.
pub async fn translation_pool_status_core(
    conn: &DatabaseConnection,
) -> Vec<crate::translation::pool::ProviderStatus> {
    let settings = translation::settings::load(conn).await;
    crate::translation::pool::pool_status(&settings)
}

/// The process-wide translation counters: dispatch volume, cache
/// effectiveness, gate rejections, per-provider transport outcomes. No
/// settings needed — everything here is runtime memory.
pub fn translation_metrics_core() -> crate::translation::metrics::TranslationMetricsSnapshot {
    translation::metrics::translation_metrics().snapshot()
}

/// Translate a batch of already-masked texts, serving cache hits first.
/// `priority` queues reader-facing requests on their own concurrency lane so
/// background thinking-block work can never delay them. `override_target_lang`
/// carries the selection card's own language choice when the user picked one
/// different from the configured target.
pub async fn translation_translate_core(
    conn: &DatabaseConnection,
    texts: Vec<String>,
    ui_locale: &str,
    priority: translation::client::Priority,
    override_target_lang: Option<String>,
    trace: Option<String>,
) -> Result<Vec<TranslationResult>, AppCommandError> {
    let settings = translation::settings::load(conn).await;
    translation::translate_with_cache(
        &texts,
        ui_locale,
        &settings,
        priority,
        override_target_lang.as_deref(),
        trace.as_deref(),
    )
    .await
}

pub fn translation_cache_stats_core() -> TranslationCacheStats {
    translation::translation_cache().stats()
}

pub fn translation_clear_cache_core() -> TranslationCacheStats {
    let cache = translation::translation_cache();
    cache.clear();
    cache.stats()
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn translation_get_settings(
    db: State<'_, AppDatabase>,
) -> Result<TranslationSettings, AppCommandError> {
    translation_get_settings_core(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn translation_update_settings(
    settings: TranslationSettings,
    db: State<'_, AppDatabase>,
) -> Result<TranslationSettings, AppCommandError> {
    translation_update_settings_core(&db.conn, settings).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn translation_test(
    settings: TranslationSettings,
    ui_locale: String,
    provider_id: Option<String>,
    db: State<'_, AppDatabase>,
) -> Result<String, AppCommandError> {
    translation_test_core(&db.conn, settings, &ui_locale, provider_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn translation_list_models(
    settings: TranslationSettings,
    provider_id: Option<String>,
    db: State<'_, AppDatabase>,
) -> Result<Vec<String>, AppCommandError> {
    translation_list_models_core(&db.conn, settings, provider_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn translation_pool_status(
    db: State<'_, AppDatabase>,
) -> Result<Vec<crate::translation::pool::ProviderStatus>, AppCommandError> {
    Ok(translation_pool_status_core(&db.conn).await)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub fn translation_metrics(
) -> Result<crate::translation::metrics::TranslationMetricsSnapshot, AppCommandError> {
    Ok(translation_metrics_core())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn translation_translate(
    texts: Vec<String>,
    ui_locale: String,
    priority: Option<bool>,
    target_lang: Option<String>,
    trace: Option<String>,
    db: State<'_, AppDatabase>,
) -> Result<Vec<TranslationResult>, AppCommandError> {
    let priority = if priority.unwrap_or(false) {
        translation::client::Priority::Priority
    } else {
        translation::client::Priority::Background
    };
    translation_translate_core(&db.conn, texts, &ui_locale, priority, target_lang, trace).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn translation_cache_stats() -> Result<TranslationCacheStats, AppCommandError> {
    Ok(translation_cache_stats_core())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn translation_clear_cache() -> Result<TranslationCacheStats, AppCommandError> {
    Ok(translation_clear_cache_core())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    fn complete() -> TranslationSettings {
        TranslationSettings {
            enabled: true,
            base_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-secret".to_string(),
            model: "gpt-4o-mini".to_string(),
            target_lang: None,
            translate_thinking: false,
            selection_translate: true,
            selection_target_lang: None,
            toggle_always_visible: false,
            api_format: String::new(),
            batch_max_chars: None,
            carry_context: true,
            providers: Vec::new(),
        }
    }

    #[tokio::test]
    async fn an_unconfigured_install_reads_as_disabled() {
        let db = fresh_in_memory_db().await;
        let settings = translation_get_settings_core(&db.conn)
            .await
            .expect("read settings");

        assert!(!settings.enabled);
        assert!(settings.base_url.is_empty());
    }

    /// The settings page never holds real keys; a legacy row's provider
    /// (synthesized by the load-time migration, id "legacy") must still match
    /// by id when the form echoes the mask back — an unmatched mask merges to
    /// empty, and the test would then fail validation with "needs at least
    /// one enabled provider".
    #[tokio::test]
    async fn a_masked_legacy_provider_key_refills_by_id() {
        let db = fresh_in_memory_db().await;
        translation_update_settings_core(&db.conn, complete())
            .await
            .expect("save legacy shape");

        let stored_masked = translation_get_settings_core(&db.conn)
            .await
            .expect("read settings");
        assert_eq!(stored_masked.providers[0].id, "legacy");

        // The real test path refills from the UNMASKED stored row (see
        // `translation_test_core`); the masked read is only what the page sees.
        let stored = translation::settings::load(&db.conn).await;
        let mut form = stored_masked;
        form.providers[0].api_key = translation::settings::API_KEY_MASK.to_string();
        let resolved = resolve_candidate_settings(stored, form);
        assert_eq!(
            resolved.providers[0].api_key, "sk-secret",
            "the mask must refill from the stored key, not merge to empty"
        );
    }

    /// The renderer must never receive the real key.
    #[tokio::test]
    async fn the_read_path_masks_the_key() {
        let db = fresh_in_memory_db().await;
        translation_update_settings_core(&db.conn, complete())
            .await
            .expect("save");

        let settings = translation_get_settings_core(&db.conn)
            .await
            .expect("read settings");
        assert_eq!(settings.api_key, translation::settings::API_KEY_MASK);
        assert_ne!(settings.api_key, "sk-secret");
    }

    #[tokio::test]
    async fn saving_an_unusable_url_is_rejected() {
        let db = fresh_in_memory_db().await;
        // P4 made bare hosts legal (`not-a-url` now defaults to https), so the
        // rejection here must come from something structurally impossible:
        // a scheme-less paste that names no host at all.
        let result = translation_update_settings_core(
            &db.conn,
            TranslationSettings {
                base_url: "http://".to_string(),
                ..complete()
            },
        )
        .await;

        assert!(result.is_err());
    }

    /// Translation requests must not reach the network while the feature is
    /// off, whatever the frontend does.
    #[tokio::test]
    async fn translating_while_disabled_is_refused() {
        let db = fresh_in_memory_db().await;
        let result = translation_translate_core(
            &db.conn,
            vec!["hello".to_string()],
            "zh-CN",
            translation::client::Priority::Background,
            None,
            None,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn an_empty_batch_is_accepted_while_enabled() {
        let db = fresh_in_memory_db().await;
        translation_update_settings_core(&db.conn, complete())
            .await
            .expect("save");

        let results = translation_translate_core(
            &db.conn,
            Vec::new(),
            "zh-CN",
            translation::client::Priority::Priority,
            None,
            None,
        )
        .await
        .expect("an empty batch needs no endpoint");
        assert!(results.is_empty());
    }
}
