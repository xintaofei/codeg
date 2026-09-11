//! WS attach protocol — Phase 1 of the Subscribe-with-Snapshot redesign.
//!
//! Replaces the legacy "subscribe to a global firehose + fetch HTTP snapshot
//! separately" flow. A client expressing interest in a specific connection
//! sends an `attach` message; the server atomically (under the SessionState
//! read lock) decides between a `snapshot` or `replay` response and
//! registers a per-connection broadcast receiver. After the response, live
//! events from that connection are delivered as `event` frames over the
//! same WebSocket.
//!
//! The legacy global `acp://event` channel remains active during Phase 1-3
//! for backward compatibility; Phase 4 retires it.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::acp::internal_bus::EventBusMetrics;
use crate::acp::manager::ConnectionManager;
use crate::acp::session_state::LiveSessionSnapshot;
use crate::acp::types::EventEnvelope;

/// Maximum number of events delivered in a single `replay` response. Larger
/// gaps fall through to a `snapshot` even when the ring buffer can satisfy
/// them — past this many events, snapshot serialization is comparable in
/// size and avoids forcing the client to apply the events one-by-one.
pub const REPLAY_BATCH_THRESHOLD: usize = 32;

/// Capacity of the per-WS-connection outbound mpsc channel. Backpressure
/// from a slow WS write naturally throttles per-subscription forwarders;
/// 64 in-flight messages is enough for short bursts without making
/// memory blow up if the client stops reading.
pub const OUTBOUND_CAPACITY: usize = 64;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Subscribe this WebSocket to a specific connection's event stream.
    /// `since_seq` allows incremental catch-up after a brief disconnect;
    /// `None` requests a full snapshot.
    Attach {
        subscription_id: String,
        connection_id: String,
        #[serde(default)]
        since_seq: Option<u64>,
    },
    /// Cancel a prior `attach` by `subscription_id`.
    Detach { subscription_id: String },
    /// Liveness check. Server replies with `pong`.
    Ping,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Initial state for `attach` (cold start, large gap, or cursor invalid).
    /// `event_seq` is the high-water mark; subsequent `event` frames carry
    /// `seq > event_seq`. The snapshot is `Box`'d so the enum variant size
    /// doesn't dominate every other (small) variant.
    Snapshot {
        subscription_id: String,
        connection_id: String,
        snapshot: Box<LiveSessionSnapshot>,
        event_seq: u64,
    },
    /// Batched catch-up for a small gap. `high_water_seq` is the largest
    /// seq in `events`; subsequent `event` frames carry `seq > high_water_seq`.
    Replay {
        subscription_id: String,
        connection_id: String,
        events: Vec<Arc<EventEnvelope>>,
        high_water_seq: u64,
    },
    /// Live event delivered after the initial Snapshot/Replay frame.
    Event {
        subscription_id: String,
        envelope: Arc<EventEnvelope>,
    },
    /// Subscription was terminated by the server. `reason` is a stable code
    /// the client maps to UX (re-attach vs. drop the conversation).
    Detached {
        subscription_id: String,
        reason: DetachReason,
    },
    /// Liveness response.
    Pong,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetachReason {
    /// `connection_id` is unknown to the manager (possibly GC'd, possibly
    /// never existed). Client should treat this as a terminal state for
    /// the conversation.
    ConnectionGone,
    /// The per-connection broadcast channel dropped events because this
    /// subscriber couldn't keep up. Client must re-attach with its
    /// `lastAppliedSeq` to resync.
    Lagged,
    /// Server is shutting down. Reconnect after the next handshake.
    ServerShutdown,
}

/// Decision returned by the attach handler describing what frame to send
/// back to the client and which `broadcast::Receiver` to forward live
/// events from.
pub struct AttachOutcome {
    pub initial_msg: ServerMsg,
    pub receiver: tokio::sync::broadcast::Receiver<Arc<EventEnvelope>>,
}

/// Decide-and-subscribe under a single `SessionState` read lock. The
/// returned receiver is registered before the lock releases, so any event
/// fired after this function returns is delivered (no race against
/// `emit_with_state`'s write lock).
///
/// `metrics` records which response shape was returned (cold snapshot vs.
/// resumed replay vs. snapshot fallback) so operators can spot when the
/// `REPLAY_BATCH_THRESHOLD` / ring-buffer caps need tuning.
pub async fn handle_attach(
    manager: &ConnectionManager,
    metrics: &EventBusMetrics,
    subscription_id: String,
    connection_id: String,
    since_seq: Option<u64>,
) -> Result<AttachOutcome, DetachReason> {
    let state_arc = manager
        .get_state(&connection_id)
        .await
        .ok_or(DetachReason::ConnectionGone)?;

    let s = state_arc.read().await;

    // Decide response shape. Order of checks matters:
    //   - explicit None → snapshot (fresh attach; client has no state yet)
    //   - cursor at or past head → snapshot anyway, defends against client
    //     bugs where lastAppliedSeq was advanced past an event we never
    //     actually broadcast (not currently possible, but cheap to guard)
    //   - cursor in ring buffer with small gap → replay
    //   - cursor in ring buffer with large gap → snapshot (cheaper)
    //   - cursor older than ring buffer → snapshot (only choice)
    let snapshot_msg = || ServerMsg::Snapshot {
        subscription_id: subscription_id.clone(),
        connection_id: connection_id.clone(),
        snapshot: Box::new(s.to_snapshot()),
        event_seq: s.event_seq,
    };

    let initial_msg = match since_seq {
        None => {
            metrics.snapshot_cold_count.fetch_add(1, Ordering::Relaxed);
            snapshot_msg()
        }
        Some(cursor) if cursor >= s.event_seq => {
            // Cursor at-or-past head — treat as fresh attach. Doesn't bump
            // `snapshot_fallback_count` because there's no gap-too-large
            // semantic; this is just a defensive equivalent of cold start.
            metrics.snapshot_cold_count.fetch_add(1, Ordering::Relaxed);
            snapshot_msg()
        }
        Some(cursor) => match s.recent_events_after(cursor) {
            Some(events) if events.len() <= REPLAY_BATCH_THRESHOLD && !events.is_empty() => {
                let event_count = events.len() as u64;
                let high_water_seq = events.last().expect("non-empty checked above").seq;
                metrics.replay_count.fetch_add(1, Ordering::Relaxed);
                metrics
                    .replay_event_total
                    .fetch_add(event_count, Ordering::Relaxed);
                ServerMsg::Replay {
                    subscription_id: subscription_id.clone(),
                    connection_id: connection_id.clone(),
                    events,
                    high_water_seq,
                }
            }
            // Either too many to batch, or buffer doesn't cover the cursor.
            _ => {
                metrics
                    .snapshot_fallback_count
                    .fetch_add(1, Ordering::Relaxed);
                snapshot_msg()
            }
        },
    };

    let receiver = s.event_stream().subscribe();
    drop(s);

    Ok(AttachOutcome {
        initial_msg,
        receiver,
    })
}

/// Spawn a forwarding task that drains the per-connection broadcast receiver
/// and sends `Event` frames to the shared outbound channel. Exits on
/// receiver close (connection went away) or `Lagged` (slow consumer); in
/// both cases sends a `Detached` frame so the client knows to re-attach.
///
/// `cleanup_tx` carries `(subscription_id, epoch)` back to the WS main loop
/// on every self-exit path so the loop can drop the now-completed
/// `JoinHandle` from its `subscriptions` map. The `epoch` is critical: a
/// stale signal arriving after the client has re-attached (which replaces
/// the handle) would otherwise orphan the fresh forwarder. The main loop
/// only removes when the stored epoch matches the signal's epoch — re-attach
/// stamps a new epoch so old signals become no-ops. Without epoch matching,
/// `JoinHandle::is_finished()` is racy on multi-threaded runtimes (the
/// runtime may not have updated the JoinHandle slot yet when the cleanup
/// signal is consumed).
///
/// Send is `try_send` so a saturated cleanup channel never blocks the
/// exiting task; the socket-close `subscriptions.drain()` is the safety net.
///
/// `metrics` records `Lagged` exits so operators can correlate attach
/// re-attachment storms with per-connection broadcast pressure.
pub fn spawn_forwarder(
    subscription_id: String,
    epoch: u64,
    metrics: Arc<EventBusMetrics>,
    mut receiver: tokio::sync::broadcast::Receiver<Arc<EventEnvelope>>,
    outbound: mpsc::Sender<ServerMsg>,
    cleanup_tx: mpsc::Sender<(String, u64)>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let signal_cleanup = || {
            let _ = cleanup_tx.try_send((subscription_id.clone(), epoch));
        };
        loop {
            match receiver.recv().await {
                Ok(envelope) => {
                    let msg = ServerMsg::Event {
                        subscription_id: subscription_id.clone(),
                        envelope,
                    };
                    if outbound.send(msg).await.is_err() {
                        // WS closed; nothing to forward to. Map cleanup
                        // is handled by the main loop's drain on exit,
                        // but the signal is harmless either way.
                        signal_cleanup();
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        "[WS attach] subscription {} lagged ({} events dropped); detaching",
                        subscription_id,
                        n
                    );
                    metrics
                        .forwarder_lagged_count
                        .fetch_add(1, Ordering::Relaxed);
                    let _ = outbound
                        .send(ServerMsg::Detached {
                            subscription_id: subscription_id.clone(),
                            reason: DetachReason::Lagged,
                        })
                        .await;
                    signal_cleanup();
                    return;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    let _ = outbound
                        .send(ServerMsg::Detached {
                            subscription_id: subscription_id.clone(),
                            reason: DetachReason::ConnectionGone,
                        })
                        .await;
                    signal_cleanup();
                    return;
                }
            }
        }
    })
}
