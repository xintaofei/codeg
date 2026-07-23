use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::{
    extract::{Extension, WebSocketUpgrade},
    response::IntoResponse,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::shutdown::ShutdownSignal;
use super::ws_attach::{self, ClientMsg, DetachReason, ServerMsg, OUTBOUND_CAPACITY};
use crate::app_state::AppState;
use crate::logging::throttle::{LagLogThrottle, LAG_LOG_WINDOW};

/// One entry per live attach subscription. The `epoch` is the per-WS-session
/// monotonic counter assigned at spawn time; it threads through the cleanup
/// channel so a stale exit signal from a now-replaced handle cannot
/// accidentally remove the fresh handle. See the `cleanup_tx` comment in
/// `handle_ws_connection` for full rationale.
struct ActiveSubscription {
    handle: JoinHandle<()>,
    epoch: u64,
}

/// Apply a forwarder self-cleanup signal: remove the handle for `sub_id`
/// only if the stored epoch matches `signal_epoch`. Extracted from the
/// select! branch so the epoch-matching invariant has a unit test.
fn apply_cleanup_signal(
    subscriptions: &mut HashMap<String, ActiveSubscription>,
    sub_id: &str,
    signal_epoch: u64,
) {
    if let Some(sub) = subscriptions.get(sub_id) {
        if sub.epoch == signal_epoch {
            subscriptions.remove(sub_id);
        }
    }
}

// MUST match `WS_READY_CHANNEL` in `src/lib/transport/constants.ts`.
// Drift between the two values silently breaks the handshake (the client
// keeps waiting and falls back to the timeout warning path after 5 s).
//
// Phase 1: kept active in parallel with the new attach protocol so existing
// clients (web / remote desktop) continue to work while transports migrate.
// Phase 4 will retire this channel.
const WS_READY_CHANNEL: &str = "__ready__";

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<AppState>>,
    Extension(shutdown_signal): Extension<Arc<ShutdownSignal>>,
) -> impl IntoResponse {
    ws.protocols([super::auth::WS_EVENT_PROTOCOL])
        .on_upgrade(|socket| handle_ws_connection(socket, state, shutdown_signal))
}

async fn handle_ws_connection(
    mut socket: WebSocket,
    state: Arc<AppState>,
    shutdown_signal: Arc<ShutdownSignal>,
) {
    // Late handshake guard: if shutdown already fired before this task
    // even started, exit before subscribing to anything else.
    if shutdown_signal.is_triggered() {
        let _ = socket.send(Message::Close(None)).await;
        return;
    }

    // Legacy global firehose subscriber. Removed in Phase 4 once all
    // transports use the attach protocol.
    let mut global_rx = state.event_broadcaster.subscribe();

    // Outbound channel funnels every server-→-client frame through one
    // sender so the WS write side has a single owner. Per-attach forwarder
    // tasks push `Event`/`Detached` frames here; the main loop pushes
    // `Snapshot`/`Replay`/`Pong` directly.
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<ServerMsg>(OUTBOUND_CAPACITY);

    // Cleanup channel: forwarders signal `(subscription_id, epoch)` here
    // when they self-exit (lagged / channel closed) so the main loop drops
    // the now-completed `JoinHandle` from `subscriptions`. Without this,
    // dead handles would sit in the map until socket close. Sized at
    // OUTBOUND_CAPACITY which comfortably covers a burst of simultaneous
    // forwarder exits; on overflow the forwarder's `try_send` no-ops and
    // the socket-close drain still reaps everything.
    //
    // The epoch is what makes the cleanup race-free. `JoinHandle::is_finished()`
    // looked tempting but is racy on multi-threaded Tokio — the cleanup
    // signal can land before the runtime has marked the JoinHandle's slot
    // finished. Instead, each Attach allocates a fresh u64 epoch (monotonic
    // per WS session) and stores it alongside the handle. The cleanup
    // branch only removes when the stored epoch matches the signal's
    // epoch — re-attaching with the same subscription_id stamps a new
    // epoch so any stale signal from the previous handle becomes a no-op.
    let (cleanup_tx, mut cleanup_rx) = mpsc::channel::<(String, u64)>(OUTBOUND_CAPACITY);

    // Track active attach subscriptions on this socket so a `detach`
    // message can abort the matching forwarder task and so we can clean
    // them all up on socket close. Each entry stores the forwarder's
    // epoch (see cleanup channel above) alongside the JoinHandle.
    let mut subscriptions: HashMap<String, ActiveSubscription> = HashMap::new();
    let mut next_epoch: u64 = 0;

    // Server→client ready handshake (legacy `__ready__` frame). Phase 1
    // keeps this so unmigrated transports still gate `acp_connect` on the
    // server-side receiver being subscribed. New attach-protocol clients
    // ignore this frame (channel name doesn't match any attach payload).
    let ready_payload = serde_json::json!({
        "channel": WS_READY_CHANNEL,
        "payload": null,
    });
    match serde_json::to_string(&ready_payload) {
        Ok(text) => {
            if let Err(e) = socket.send(Message::Text(text.into())).await {
                tracing::warn!("[WS][WARN] failed to send __ready__ frame: {e}");
                return;
            }
        }
        Err(e) => {
            tracing::warn!("[WS][WARN] failed to serialize __ready__ frame: {e}");
            return;
        }
    }

    // Throttles the per-connection global-firehose lag WARN so a slow client
    // reading behind a steady side-channel stream can't trickle near-duplicate
    // lines. Task-local: one instance per WS connection.
    let mut lag_throttle = LagLogThrottle::new(LAG_LOG_WINDOW);

    loop {
        tokio::select! {
            // Server-initiated shutdown: notify any active attach
            // subscriptions before closing so the client can decide
            // whether to retry on the next reconnect.
            _ = shutdown_signal.wait() => {
                for sub_id in subscriptions.keys() {
                    let frame = ServerMsg::Detached {
                        subscription_id: sub_id.clone(),
                        reason: DetachReason::ServerShutdown,
                    };
                    if let Ok(text) = serde_json::to_string(&frame) {
                        let _ = socket.send(Message::Text(text.into())).await;
                    }
                }
                let _ = socket.send(Message::Close(None)).await;
                break;
            }

            // Forwarder self-cleanup: a per-attach task that exited (Lagged
            // or broadcast Closed) reports `(sub_id, epoch)` so we can
            // drop the dead JoinHandle. The epoch match guarantees we
            // only remove the handle the signal was actually for —
            // a re-attach between the dead forwarder's exit and this
            // recv would have stamped a new epoch, making the stale
            // signal a no-op.
            cleanup = cleanup_rx.recv() => {
                if let Some((sub_id, signal_epoch)) = cleanup {
                    apply_cleanup_signal(&mut subscriptions, &sub_id, signal_epoch);
                }
            }

            // Outbound queue (per-attach forwarders + main-loop direct sends).
            outgoing = outbound_rx.recv() => {
                match outgoing {
                    Some(msg) => {
                        match serde_json::to_string(&msg) {
                            Ok(text) => {
                                if socket.send(Message::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("[WS][WARN] failed to serialize ServerMsg: {e}");
                            }
                        }
                    }
                    // Channel closed only when all senders dropped — i.e. this
                    // task itself dropped `outbound_tx` AND every spawned
                    // forwarder exited. Won't happen while the loop runs.
                    None => break,
                }
            }

            // Legacy global firehose. Forwarded as-is (uses the old
            // `WebEvent { channel, payload }` shape, not the attach
            // protocol's `ServerMsg`).
            result = global_rx.recv() => {
                match result {
                    Ok(event) => {
                        if let Ok(msg) = serde_json::to_string(&event) {
                            if socket.send(Message::Text(msg.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        if let Some(s) = lag_throttle.record(n) {
                            tracing::warn!(
                                "[WS][WARN] global receiver lagged: skipped {} events across \
                                 {} occurrence(s) in the last {}s",
                                s.dropped,
                                s.occurrences,
                                LAG_LOG_WINDOW.as_secs()
                            );
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }

            // Client-→-server messages. Text frames are parsed as
            // `ClientMsg`; everything else is ignored (binary, ping/pong
            // are handled by axum, close is handled by the None match arm).
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(cmsg) => {
                                handle_client_msg(
                                    cmsg,
                                    &state,
                                    &outbound_tx,
                                    &cleanup_tx,
                                    &mut subscriptions,
                                    &mut next_epoch,
                                ).await;
                            }
                            Err(e) => {
                                tracing::warn!("[WS][WARN] malformed client message: {e}");
                            }
                        }
                    }
                    Some(Ok(_)) => {
                        // Binary / ping / pong: ignore.
                    }
                    _ => break,
                }
            }
        }
    }

    // Cleanup: abort all active forwarder tasks. Their broadcast receivers
    // will be dropped, freeing the per-connection broadcaster slot.
    for (_, sub) in subscriptions.drain() {
        sub.handle.abort();
    }
}

async fn handle_client_msg(
    msg: ClientMsg,
    state: &Arc<AppState>,
    outbound_tx: &mpsc::Sender<ServerMsg>,
    cleanup_tx: &mpsc::Sender<(String, u64)>,
    subscriptions: &mut HashMap<String, ActiveSubscription>,
    next_epoch: &mut u64,
) {
    match msg {
        ClientMsg::Attach {
            subscription_id,
            connection_id,
            since_seq,
        } => {
            // Re-attach with the same subscription_id replaces the prior
            // forwarder. Abort the old one first so its receiver drops
            // and we don't leak a broadcaster slot.
            if let Some(old) = subscriptions.remove(&subscription_id) {
                old.handle.abort();
            }

            match ws_attach::handle_attach(
                &state.connection_manager,
                state.acp_event_bus.metrics(),
                subscription_id.clone(),
                connection_id,
                since_seq,
            )
            .await
            {
                Ok(outcome) => {
                    // Send the initial frame (snapshot or replay) BEFORE
                    // spawning the forwarder so the client sees state
                    // before the first live event.
                    if outbound_tx.send(outcome.initial_msg).await.is_err() {
                        return;
                    }
                    // Allocate a fresh epoch for this spawn. wrapping_add is
                    // defensive — u64 overflow per WS session is impossible
                    // in practice (would require ~10^19 attaches on one
                    // socket) but the wrap behavior is well-defined and
                    // matches our epoch-equality semantics.
                    *next_epoch = next_epoch.wrapping_add(1);
                    let epoch = *next_epoch;
                    let handle = ws_attach::spawn_forwarder(
                        subscription_id.clone(),
                        epoch,
                        state.acp_event_bus.metrics().clone(),
                        outcome.receiver,
                        outbound_tx.clone(),
                        cleanup_tx.clone(),
                    );
                    subscriptions.insert(subscription_id, ActiveSubscription { handle, epoch });
                }
                Err(reason) => {
                    let _ = outbound_tx
                        .send(ServerMsg::Detached {
                            subscription_id,
                            reason,
                        })
                        .await;
                }
            }
        }
        ClientMsg::Detach { subscription_id } => {
            if let Some(sub) = subscriptions.remove(&subscription_id) {
                sub.handle.abort();
            }
        }
        ClientMsg::Ping => {
            let _ = outbound_tx.send(ServerMsg::Pong).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an ActiveSubscription wrapping a no-op spawned task. The
    /// JoinHandle is real so abort() in cleanup paths is realistic.
    async fn dummy_sub(epoch: u64) -> ActiveSubscription {
        let handle = tokio::spawn(async {
            // Park forever; aborted by cleanup paths or dropped at test end.
            std::future::pending::<()>().await;
        });
        ActiveSubscription { handle, epoch }
    }

    #[tokio::test]
    async fn cleanup_signal_with_matching_epoch_removes_entry() {
        let mut subs = HashMap::new();
        subs.insert("sub-A".to_string(), dummy_sub(1).await);

        apply_cleanup_signal(&mut subs, "sub-A", 1);

        assert!(!subs.contains_key("sub-A"));
    }

    #[tokio::test]
    async fn cleanup_signal_with_stale_epoch_keeps_fresh_handle() {
        // Simulate the race: old forwarder exited with epoch=1; client
        // re-attached and a new forwarder was spawned with epoch=2 in the
        // same map slot. The old cleanup signal must NOT remove the fresh
        // handle, otherwise the new forwarder is orphaned.
        let mut subs = HashMap::new();
        let fresh = dummy_sub(2).await;
        subs.insert("sub-A".to_string(), fresh);

        apply_cleanup_signal(&mut subs, "sub-A", 1);

        assert!(
            subs.contains_key("sub-A"),
            "stale cleanup must not evict the freshly re-attached handle"
        );
        assert_eq!(subs.get("sub-A").unwrap().epoch, 2);
    }

    #[tokio::test]
    async fn cleanup_signal_for_unknown_sub_id_is_noop() {
        let mut subs = HashMap::new();
        subs.insert("other".to_string(), dummy_sub(1).await);

        apply_cleanup_signal(&mut subs, "missing", 1);

        assert!(subs.contains_key("other"));
    }
}
