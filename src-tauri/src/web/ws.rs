use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::{
    extract::{Extension, Query, WebSocketUpgrade},
    response::IntoResponse,
};
use serde::Deserialize;
use tokio::time::sleep;

use crate::app_state::AppState;
use crate::web::client_owner::{
    normalize_web_client_id, owner_label_for_client, CleanupLease, WEB_CLIENT_CLEANUP_DELAY,
};

#[derive(Deserialize)]
pub(crate) struct WsQueryParams {
    #[serde(alias = "client_id")]
    client_id: Option<String>,
}

pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsQueryParams>,
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    let client_id = params.client_id.and_then(|value| normalize_web_client_id(&value));
    ws.on_upgrade(|socket| handle_ws_connection(socket, state, client_id))
}

async fn handle_ws_connection(
    mut socket: WebSocket,
    state: Arc<AppState>,
    client_id: Option<String>,
) {
    if let Some(client_id) = client_id.as_deref() {
        state.web_client_registry.register_socket(client_id).await;
    }

    let mut rx = state.event_broadcaster.subscribe();

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        if let Ok(msg) = serde_json::to_string(&event) {
                            if socket.send(Message::Text(msg.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("[WS] receiver lagged, skipped {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(_)) => {
                        // Client messages currently unused; reserved for future use
                    }
                    _ => break,
                }
            }
        }
    }

    if let Some(client_id) = client_id {
        if let Some(cleanup_lease) = state.web_client_registry.unregister_socket(&client_id).await {
            schedule_web_client_cleanup(state, cleanup_lease);
        }
    }
}

fn schedule_web_client_cleanup(state: Arc<AppState>, cleanup_lease: CleanupLease) {
    // Track the cleanup task so server shutdown can await pending cleanups
    // instead of dropping them mid-sleep.
    let tracker = state.task_tracker.clone();
    let state_for_task = state.clone();
    tracker.spawn(async move {
        let state = state_for_task;
        sleep(WEB_CLIENT_CLEANUP_DELAY).await;
        if !state
            .web_client_registry
            .should_cleanup(&cleanup_lease)
            .await
        {
            return;
        }

        let owner_window_label = owner_label_for_client(&cleanup_lease.client_id);
        let disconnected = state
            .connection_manager
            .disconnect_by_owner_window(&owner_window_label)
            .await;
        let killed = state
            .terminal_manager
            .kill_by_owner_window(&owner_window_label);

        state
            .web_client_registry
            .finish_cleanup(&cleanup_lease)
            .await;

        if disconnected > 0 || killed > 0 {
            eprintln!(
                "[WS] cleaned up owner_window={} disconnected_connections={} killed_terminals={}",
                owner_window_label, disconnected, killed
            );
        }
    });
}
