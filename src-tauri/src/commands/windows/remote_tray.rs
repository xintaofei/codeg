//! Saved remote workspaces in the native tray; no main webview is required.

use std::collections::HashSet;
use std::sync::Mutex;

use sea_orm::DatabaseConnection;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::app_error::AppCommandError;
use crate::db::service::remote_workspace_connection_service;
use crate::db::AppDatabase;
use crate::models::system::AppLocale;

const REMOTE_MENU_PREFIX: &str = "tray:remote:";

pub(super) struct RemoteTrayState {
    // Serialize the DB read and menu replacement, so an older snapshot cannot
    // overwrite a newer save/reorder or language change.
    locale: tokio::sync::Mutex<AppLocale>,
    opening: Mutex<HashSet<i32>>,
}

impl RemoteTrayState {
    pub(super) fn new(locale: AppLocale) -> Self {
        Self {
            locale: tokio::sync::Mutex::new(locale),
            opening: Mutex::new(HashSet::new()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RemoteTrayEntry {
    id: i32,
    pub(super) name: String,
}

impl RemoteTrayEntry {
    pub(super) fn menu_id(&self) -> String {
        format!("{REMOTE_MENU_PREFIX}{}", self.id)
    }

    pub(super) fn menu_label(&self) -> String {
        // Native menus treat a single ampersand as a mnemonic marker.
        self.name.replace('&', "&&")
    }
}

fn connection_id(menu_id: &str) -> Option<i32> {
    let raw = menu_id.strip_prefix(REMOTE_MENU_PREFIX)?;
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse::<i32>().ok().filter(|id| *id > 0)
}

async fn load_entries(conn: &DatabaseConnection) -> Result<Vec<RemoteTrayEntry>, AppCommandError> {
    Ok(remote_workspace_connection_service::list(conn)
        .await
        .map_err(AppCommandError::db)?
        .into_iter()
        .map(|connection| RemoteTrayEntry {
            id: connection.id,
            name: connection.name,
        })
        .collect())
}

pub(crate) async fn refresh(
    app: &AppHandle,
    locale: Option<AppLocale>,
) -> Result<(), AppCommandError> {
    let Some(tray) = app.tray_by_id(super::TRAY_ICON_ID) else {
        return Ok(());
    };
    let state = app.state::<RemoteTrayState>();
    let mut current_locale = state.locale.lock().await;
    if let Some(locale) = locale {
        *current_locale = locale;
    }
    let db = app.state::<AppDatabase>();
    let entries = load_entries(&db.conn).await?;
    let menu = super::build_tray_menu(app, *current_locale, &entries)
        .map_err(|e| AppCommandError::window("Failed to refresh tray menu", e.to_string()))?;
    tray.set_menu(Some(menu))
        .map_err(|e| AppCommandError::window("Failed to refresh tray menu", e.to_string()))
}

/// A committed save must still report success if a native menu update fails.
pub(crate) async fn refresh_saved_connections(app: &AppHandle) {
    if let Err(error) = refresh(app, None).await {
        tracing::warn!(
            "[Tray] failed to refresh remote workspaces: {}",
            error.message
        );
    }
}

pub(crate) fn handle_menu_event(app: &AppHandle, menu_id: &str) {
    let Some(id) = connection_id(menu_id) else {
        return;
    };
    let state = app.state::<RemoteTrayState>();
    // Health checks may take several seconds. Ignore another click for this
    // connection until its first open finishes, but allow other connections.
    if !state.opening.lock().unwrap().insert(id) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = crate::commands::remote_workspace::open_remote_workspace(
            app.clone(),
            app.state::<AppDatabase>(),
            id,
        )
        .await;
        app.state::<RemoteTrayState>()
            .opening
            .lock()
            .unwrap()
            .remove(&id);
        if let Err(error) = result {
            // The main window can be hidden, so a webview toast is insufficient.
            app.dialog()
                .message(error.message)
                .title("Codeg")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_menu_ids_round_trip_independently_of_names() {
        for id in [1, 42, i32::MAX] {
            let entry = RemoteTrayEntry {
                id,
                name: "开发机: staging & production".into(),
            };
            assert_eq!(connection_id(&entry.menu_id()), Some(id));
        }
    }

    #[test]
    fn saved_names_escape_native_menu_mnemonics() {
        let entry = RemoteTrayEntry {
            id: 1,
            name: "R&D && 开发".into(),
        };
        assert_eq!(entry.menu_label(), "R&&D &&&& 开发");
    }

    #[test]
    fn unrelated_and_malformed_menu_ids_do_not_open_connections() {
        for id in [
            "tray:show",
            "tray:quit",
            "pet:remote:1",
            "tray:remote:",
            "tray:remote:0",
            "tray:remote:-1",
            "tray:remote:+1",
            "tray:remote:1:2",
            "tray:remote:2147483648",
            "tray:remote: 1",
        ] {
            assert_eq!(connection_id(id), None, "{id}");
        }
    }

    #[tokio::test]
    async fn saved_entries_follow_database_names_order_and_deletion() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        assert!(load_entries(&db.conn).await.unwrap().is_empty());
        let first = remote_workspace_connection_service::create(
            &db.conn,
            "开发机",
            "http://localhost:3080",
            "first-token",
            &[],
        )
        .await
        .unwrap();
        // Identical display names must still route to distinct connections.
        let second = remote_workspace_connection_service::create(
            &db.conn,
            "开发机",
            "http://localhost:3081",
            "second-token",
            &[],
        )
        .await
        .unwrap();
        let entries = load_entries(&db.conn).await.unwrap();
        assert_eq!(
            entries.iter().map(|e| e.id).collect::<Vec<_>>(),
            [first.id, second.id]
        );
        assert_ne!(entries[0].menu_id(), entries[1].menu_id());

        remote_workspace_connection_service::update(
            &db.conn,
            first.id,
            "Production",
            "http://localhost:3080",
            "new-token",
            &[],
        )
        .await
        .unwrap();
        remote_workspace_connection_service::reorder(&db.conn, vec![second.id, first.id])
            .await
            .unwrap();
        assert_eq!(
            load_entries(&db.conn).await.unwrap(),
            vec![
                RemoteTrayEntry {
                    id: second.id,
                    name: "开发机".into()
                },
                RemoteTrayEntry {
                    id: first.id,
                    name: "Production".into()
                },
            ]
        );
        remote_workspace_connection_service::delete(&db.conn, second.id)
            .await
            .unwrap();
        assert_eq!(
            load_entries(&db.conn).await.unwrap(),
            vec![RemoteTrayEntry {
                id: first.id,
                name: "Production".into()
            },]
        );
        remote_workspace_connection_service::delete(&db.conn, first.id)
            .await
            .unwrap();
        assert!(load_entries(&db.conn).await.unwrap().is_empty());
    }
}
