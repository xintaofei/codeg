#[cfg(feature = "tauri-runtime")]
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::acp::delegation::continuation::ContinuationCoordinator;
use crate::app_error::AppCommandError;
use crate::db::service::app_metadata_service;

/// Storage key for the continuable-delegation experiment. DEFAULT OFF
/// (v2 design D6): the key's absence reads as disabled, so an old database
/// upgrades to "off" without a migration.
pub const KEY_CONTINUATION_ENABLED: &str = "continuable_delegation_enabled";

/// Read the continuable-delegation experiment flag. Missing/corrupt → false.
pub async fn load_continuation_enabled(conn: &DatabaseConnection) -> bool {
    match app_metadata_service::get_value(conn, KEY_CONTINUATION_ENABLED).await {
        Ok(Some(raw)) => raw.trim().eq_ignore_ascii_case("true"),
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(
                "[continuation] reading the experiment flag failed ({e}); defaulting OFF"
            );
            false
        }
    }
}

/// Restore the persisted experiment flag into the live coordinator at startup.
/// The coordinator deliberately starts disabled, so both runtime entry points
/// must call this before accepting agent work.
pub async fn apply_persisted_continuation_config(
    conn: &DatabaseConnection,
    coordinator: &ContinuationCoordinator,
) {
    coordinator
        .set_enabled(load_continuation_enabled(conn).await)
        .await;
}

/// Persist the continuable-delegation experiment flag AND apply it to the
/// live coordinator. Shared by the desktop command and the web handler.
pub async fn set_continuation_enabled_core(
    conn: &DatabaseConnection,
    coordinator: &ContinuationCoordinator,
    enabled: bool,
) -> Result<bool, AppCommandError> {
    app_metadata_service::upsert_value(conn, KEY_CONTINUATION_ENABLED, &enabled.to_string())
        .await
        .map_err(|e| AppCommandError::configuration_invalid(format!("persist failed: {e}")))?;
    coordinator.set_enabled(enabled).await;
    Ok(enabled)
}

/// Wire payload for the continuation experiment toggle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuationSettings {
    pub continuable_delegation_enabled: bool,
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_continuation_settings(
    #[cfg(feature = "tauri-runtime")] db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<ContinuationSettings, AppCommandError> {
    #[cfg(feature = "tauri-runtime")]
    {
        Ok(ContinuationSettings {
            continuable_delegation_enabled: load_continuation_enabled(&db.conn).await,
        })
    }
    #[cfg(not(feature = "tauri-runtime"))]
    {
        Err(AppCommandError::configuration_invalid("tauri-only command"))
    }
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn set_continuation_settings(
    #[cfg(feature = "tauri-runtime")] db: tauri::State<'_, crate::db::AppDatabase>,
    #[cfg(feature = "tauri-runtime")] coordinator: tauri::State<'_, Arc<ContinuationCoordinator>>,
    settings: ContinuationSettings,
) -> Result<ContinuationSettings, AppCommandError> {
    #[cfg(feature = "tauri-runtime")]
    {
        set_continuation_enabled_core(
            &db.conn,
            coordinator.inner(),
            settings.continuable_delegation_enabled,
        )
        .await?;
        Ok(settings)
    }
    #[cfg(not(feature = "tauri-runtime"))]
    {
        let _ = settings;
        Err(AppCommandError::configuration_invalid("tauri-only command"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::acp::delegation::broker::NoopOutcomeStore;
    use crate::acp::delegation::continuation::NoopRuntime;
    use crate::db::{test_helpers::fresh_in_memory_db, AppDatabase};

    fn coordinator(db: &AppDatabase) -> ContinuationCoordinator {
        ContinuationCoordinator::new(
            Arc::new(AppDatabase {
                conn: db.conn.clone(),
            }),
            Arc::new(NoopRuntime),
            Arc::new(NoopOutcomeStore),
        )
    }

    #[tokio::test]
    async fn persisted_true_enables_continuation_at_startup() {
        let db = fresh_in_memory_db().await;
        app_metadata_service::upsert_value(&db.conn, KEY_CONTINUATION_ENABLED, "true")
            .await
            .unwrap();
        let coordinator = coordinator(&db);

        apply_persisted_continuation_config(&db.conn, &coordinator).await;

        assert!(coordinator.is_enabled().await);
    }

    #[tokio::test]
    async fn false_missing_and_corrupt_values_disable_continuation_at_startup() {
        for stored in [Some("false"), None, Some("not-a-boolean")] {
            let db = fresh_in_memory_db().await;
            if let Some(value) = stored {
                app_metadata_service::upsert_value(&db.conn, KEY_CONTINUATION_ENABLED, value)
                    .await
                    .unwrap();
            }
            let coordinator = coordinator(&db);
            coordinator.set_enabled(true).await;

            apply_persisted_continuation_config(&db.conn, &coordinator).await;

            assert!(
                !coordinator.is_enabled().await,
                "stored value {stored:?} must restore as disabled"
            );
        }
    }
}
