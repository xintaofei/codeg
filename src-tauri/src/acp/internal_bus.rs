//! In-process ACP event bus.
//!
//! Carries `Arc<EventEnvelope>` directly to back-end consumers (lifecycle,
//! pet state mapper, chat-channel subscribers). Distinct from
//! `WebEventBroadcaster`, which carries `Arc<serde_json::Value>` for
//! transport-bound JSON delivery to WS clients.
//!
//! Two reasons to split the buses:
//!
//! 1. **No JSON parse on the consumer side.** Every back-end subscriber used
//!    to call `serde_json::from_value(payload.clone())` on the broadcaster's
//!    `WebEvent.payload`, paying the parse cost per event per subscriber.
//!    With a typed bus they receive the envelope directly.
//!
//! 2. **No frontend dedup needed.** Before the split, web/remote-desktop WS
//!    clients received `acp://event` from BOTH the per-connection attach
//!    stream AND the global broadcaster firehose, forcing a receiver-side
//!    dedup `Set<connectionId>` on the client. With ACP events removed from
//!    the global broadcaster, the per-connection stream is the sole path
//!    and the dedup goes away.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::acp::types::EventEnvelope;

/// Capacity of the broadcast channel. Sized to the same headroom as
/// `WebEventBroadcaster` (4096) — they observe the same emit rate so the
/// burst tolerance is identical.
const BUS_CAPACITY: usize = 4096;

/// Process-wide bus delivering ACP envelopes to in-process consumers.
///
/// Subscribers (lifecycle / pet / chat-channel) call `subscribe()` once at
/// startup and hold the receiver for the lifetime of the process.
/// `emit_with_state` calls `send()` after the per-connection stream so the
/// envelope arrives in lockstep with the WS attach delivery.
#[derive(Debug)]
pub struct InternalEventBus {
    sender: broadcast::Sender<Arc<EventEnvelope>>,
    metrics: Arc<EventBusMetrics>,
}

impl InternalEventBus {
    pub fn new(metrics: Arc<EventBusMetrics>) -> Self {
        let (sender, _) = broadcast::channel(BUS_CAPACITY);
        Self { sender, metrics }
    }

    /// Broadcast `envelope` to every subscriber. No-op if there are none —
    /// avoids `SendError` allocation on the hot emit path.
    pub fn send(&self, envelope: Arc<EventEnvelope>) {
        if self.sender.receiver_count() == 0 {
            return;
        }
        // SendError can only fire when receiver_count() == 0, which we just
        // checked under the same lock-free atomic. The race window is narrow
        // (a subscriber dropping between the check and the send) and a
        // dropped envelope in that exact window is benign — there's no one
        // to deliver to anyway.
        let _ = self.sender.send(envelope);
        self.metrics.emitted_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Subscribe to the bus. The returned receiver buffers up to
    /// `BUS_CAPACITY` events behind the slowest subscriber; if it falls
    /// further behind, the next `recv()` returns `RecvError::Lagged(n)`
    /// and bumps `lagged_count`.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<EventEnvelope>> {
        self.sender.subscribe()
    }

    pub fn metrics(&self) -> &Arc<EventBusMetrics> {
        &self.metrics
    }
}

/// Counters surfaced on the `/debug/event_metrics` HTTP endpoint and via
/// shutdown logs. Kept as plain `AtomicU64` to avoid pulling in a metrics
/// framework — load is low, the only consumers are operators tailing logs
/// or fetching the debug endpoint.
#[derive(Debug, Default)]
pub struct EventBusMetrics {
    /// Envelopes pushed onto `InternalEventBus`. Tracks emit volume.
    pub emitted_count: AtomicU64,
    /// `RecvError::Lagged(n)` occurrences across all subscribers — sum of
    /// dropped-events `n`. Spike means a subscriber is behind on DB writes
    /// or otherwise too slow.
    pub lagged_count: AtomicU64,
    /// Envelopes evicted from a per-connection `RecentEventsBuffer`
    /// (FIFO trim by either count cap or byte cap). Drives the snapshot-vs-
    /// replay decision when an attach-with-cursor lands too late.
    pub ring_buffer_evict_count: AtomicU64,
    /// Attach decisions: client supplied a cursor that fell within the ring
    /// buffer and was small enough to batch. Tracks happy-path resync.
    pub replay_count: AtomicU64,
    /// Sum of envelope counts across all replay batches. Average batch size
    /// = `replay_event_total / replay_count`, useful for sizing
    /// `REPLAY_BATCH_THRESHOLD`.
    pub replay_event_total: AtomicU64,
    /// Attach decisions: client supplied a cursor that fell outside the
    /// ring buffer (or buffer was too large to batch), so the server fell
    /// back to a full snapshot. High rate suggests buffer caps need lifting.
    pub snapshot_fallback_count: AtomicU64,
    /// Attach decisions: client requested a snapshot explicitly (no cursor).
    /// Cold-start frontends + post-disconnect re-attaches with no preserved
    /// state.
    pub snapshot_cold_count: AtomicU64,
    /// Per-attach forwarder tasks that exited with `Lagged`. Each one
    /// triggers a client re-attach (and therefore a snapshot or replay).
    pub forwarder_lagged_count: AtomicU64,
    /// Lifecycle dispatcher observed a per-connection worker mailbox at
    /// capacity and had to block on `send().await` to enqueue. **No event
    /// is dropped** — the dispatcher waits for the worker to drain. Counts
    /// each occurrence so operators can spot connections that chronically
    /// stall their worker (typically a SQLite contention pattern). Distinct
    /// from `lagged_count` (bus-level event loss).
    pub worker_queue_full_count: AtomicU64,
}

impl EventBusMetrics {
    pub fn snapshot(&self) -> EventBusMetricsSnapshot {
        EventBusMetricsSnapshot {
            emitted_count: self.emitted_count.load(Ordering::Relaxed),
            lagged_count: self.lagged_count.load(Ordering::Relaxed),
            ring_buffer_evict_count: self.ring_buffer_evict_count.load(Ordering::Relaxed),
            replay_count: self.replay_count.load(Ordering::Relaxed),
            replay_event_total: self.replay_event_total.load(Ordering::Relaxed),
            snapshot_fallback_count: self.snapshot_fallback_count.load(Ordering::Relaxed),
            snapshot_cold_count: self.snapshot_cold_count.load(Ordering::Relaxed),
            forwarder_lagged_count: self.forwarder_lagged_count.load(Ordering::Relaxed),
            worker_queue_full_count: self.worker_queue_full_count.load(Ordering::Relaxed),
        }
    }
}

/// JSON-serializable view of `EventBusMetrics` for the debug HTTP endpoint.
/// Plain `u64` so the response is stable JSON — atomic types serialize
/// erratically across serde-versions.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EventBusMetricsSnapshot {
    pub emitted_count: u64,
    pub lagged_count: u64,
    pub ring_buffer_evict_count: u64,
    pub replay_count: u64,
    pub replay_event_total: u64,
    pub snapshot_fallback_count: u64,
    pub snapshot_cold_count: u64,
    pub forwarder_lagged_count: u64,
    pub worker_queue_full_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::types::AcpEvent;

    fn fake_envelope(seq: u64) -> Arc<EventEnvelope> {
        Arc::new(EventEnvelope {
            seq,
            connection_id: "c1".into(),
            payload: AcpEvent::ContentDelta {
                text: "x".into(),
                parent_tool_use_id: None,
            },
        })
    }

    #[tokio::test]
    async fn send_with_no_subscribers_is_noop_and_does_not_count() {
        // No-receiver fast path must not bump emitted_count — the metric
        // tracks delivered emit attempts, not orphaned ones (otherwise a
        // process with no UI would still rack up emits during agent runs).
        let metrics = Arc::new(EventBusMetrics::default());
        let bus = InternalEventBus::new(metrics.clone());
        bus.send(fake_envelope(1));
        assert_eq!(metrics.emitted_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn send_delivers_to_all_subscribers_and_counts_once() {
        let metrics = Arc::new(EventBusMetrics::default());
        let bus = InternalEventBus::new(metrics.clone());
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.send(fake_envelope(7));
        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert_eq!(e1.seq, 7);
        assert_eq!(e2.seq, 7);
        // Same Arc — broadcast clones the handle, not the payload.
        assert!(Arc::ptr_eq(&e1, &e2));
        assert_eq!(metrics.emitted_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn metrics_snapshot_returns_loaded_values() {
        let metrics = Arc::new(EventBusMetrics::default());
        metrics.emitted_count.store(42, Ordering::Relaxed);
        metrics.lagged_count.store(3, Ordering::Relaxed);
        metrics.snapshot_fallback_count.store(1, Ordering::Relaxed);
        let snap = metrics.snapshot();
        assert_eq!(snap.emitted_count, 42);
        assert_eq!(snap.lagged_count, 3);
        assert_eq!(snap.snapshot_fallback_count, 1);
    }
}
