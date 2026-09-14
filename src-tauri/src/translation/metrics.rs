//! Process-wide translation counters: dispatch volume, cache effectiveness,
//! gate rejections, and transport outcomes for the one endpoint.
//!
//! Plain `AtomicU64` like [`crate::acp::internal_bus::EventBusMetrics`] — no
//! metrics framework. The settings page's status strip reads the snapshot
//! (via the `translation_metrics` command). PR1 keeps counters only — the
//! per-provider tables and minute series return with the rotation pool in a
//! later PR.
//!
//! Division of recording labor (so no outcome is counted twice):
//! - `client.rs` records **per outbound attempt**: the dispatch itself plus
//!   its transport-level verdict (rate-limited / HTTP error / network error /
//!   parse failure). A retried chunk therefore shows every attempt.
//! - `client.rs` also records the **acceptance verdict** for a parseable
//!   reply: `record_attempt(Ok)` only after the quality gate accepted it, or
//!   `record_gate_rejection` when the gate refused it. A parseable reply can
//!   therefore never count as both an ok and a rejection.
//! - `mod.rs` records **per served slot**: cache hits and served totals.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use serde::Serialize;

/// The transport-level verdicts `client.rs` emits per attempt. The
/// acceptance verdict is NOT a transport verdict: a parseable reply rides
/// into `Ok` only after the quality gate accepted it, and a gate refusal is
/// reported through [`TranslationMetrics::record_gate_rejection`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptKind {
    /// The endpoint returned a parseable, gate-accepted translation.
    Ok,
    /// HTTP 429.
    RateLimited,
    /// Any other non-success HTTP status.
    HttpError,
    /// Transport failure: timeout, connection reset, DNS.
    NetworkError,
    /// The reply body could not be parsed into a translation.
    ParseError,
    /// The request was still in flight when the soft in-flight deadline
    /// passed — a reader-visible slowness signal that feeds no counter yet,
    /// because the reply may still land fine.
    SlowInflight,
}

/// Global counters. Process-wide, in-memory only — a restart re-probes.
#[derive(Debug, Default)]
pub struct TranslationMetrics {
    /// Outbound POSTs, all attempts included.
    dispatched_total: AtomicU64,
    /// Attempts whose parseable reply the quality gate ACCEPTED.
    ok_total: AtomicU64,
    /// Attempts that failed: rate limited, HTTP error, network error, parse
    /// failure, or a quality-gate rejection. Slow-inflight signals count
    /// neither way — the reply may still land fine.
    failed_total: AtomicU64,
    /// Replies the quality gate refused (all buckets below sum to this).
    gate_rejected_total: AtomicU64,
    gate_rejected_invented: AtomicU64,
    gate_rejected_echo: AtomicU64,
    gate_rejected_dropped_numbers: AtomicU64,
    /// Replies the endpoint cut off mid-translation (max_tokens / `length`).
    truncated_total: AtomicU64,
    /// Slots served from the content-addressed cache (no network).
    cache_hits: AtomicU64,
    /// Slots whose text rendered (cache, network success, or native skip
    /// minus the identity short-circuit — see `mod.rs`).
    served_total: AtomicU64,
    latency_ms_sum: AtomicU64,
    latency_count: AtomicU64,
}

static METRICS: OnceLock<TranslationMetrics> = OnceLock::new();

/// The process-wide metrics instance.
pub fn translation_metrics() -> &'static TranslationMetrics {
    METRICS.get_or_init(TranslationMetrics::default)
}

/// Which quality gate refused a reply — drives both the user-visible message
/// and the rejection bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRejection {
    Invented,
    EchoOrRefusal,
    DroppedNumbers,
}

impl TranslationMetrics {
    /// One outbound POST (per attempt, not per chunk — a retried chunk shows
    /// every attempt, which is what pacing analysis needs).
    pub fn record_dispatch(&self) {
        self.dispatched_total.fetch_add(1, Ordering::Relaxed);
    }

    /// A per-attempt verdict from `client.rs`. `AttemptKind::Ok` is recorded
    /// ONLY after the quality gate accepted the parseable reply — see the
    /// module docs for why the acceptance decision owns this counter.
    pub fn record_attempt(&self, kind: AttemptKind, latency_ms: u64) {
        match kind {
            AttemptKind::Ok => {
                self.ok_total.fetch_add(1, Ordering::Relaxed);
                self.latency_ms_sum.fetch_add(latency_ms, Ordering::Relaxed);
                self.latency_count.fetch_add(1, Ordering::Relaxed);
            }
            AttemptKind::RateLimited
            | AttemptKind::HttpError
            | AttemptKind::NetworkError
            | AttemptKind::ParseError => {
                self.failed_total.fetch_add(1, Ordering::Relaxed);
            }
            // A soft, in-flight signal: the reply may still turn out fine, so
            // it feeds no success/failure counter.
            AttemptKind::SlowInflight => {}
        }
    }

    /// A quality-gate rejection from the acceptance decision: the reply was
    /// parseable, so it is not a transport failure, but it was refused
    /// instead of accepted — the failed counter moves WITH the gate bucket
    /// so ok + failed always covers every completed attempt.
    pub fn record_gate_rejection(&self, rejection: GateRejection, latency_ms: u64) {
        self.gate_rejected_total.fetch_add(1, Ordering::Relaxed);
        match rejection {
            GateRejection::Invented => {
                self.gate_rejected_invented.fetch_add(1, Ordering::Relaxed);
            }
            GateRejection::EchoOrRefusal => {
                self.gate_rejected_echo.fetch_add(1, Ordering::Relaxed);
            }
            GateRejection::DroppedNumbers => {
                self.gate_rejected_dropped_numbers
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        self.failed_total.fetch_add(1, Ordering::Relaxed);
        self.latency_ms_sum.fetch_add(latency_ms, Ordering::Relaxed);
        self.latency_count.fetch_add(1, Ordering::Relaxed);
    }

    /// A reply the endpoint cut off mid-translation.
    pub fn record_truncated(&self) {
        self.truncated_total.fetch_add(1, Ordering::Relaxed);
    }

    /// A slot served from the cache.
    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// A slot whose text rendered for the reader.
    pub fn record_served(&self) {
        self.served_total.fetch_add(1, Ordering::Relaxed);
    }

    /// The JSON-serializable view for the settings page — one row, PR1 shape.
    pub fn snapshot(&self) -> TranslationMetricsSnapshot {
        let latency_count = self.latency_count.load(Ordering::Relaxed);
        TranslationMetricsSnapshot {
            dispatched_total: self.dispatched_total.load(Ordering::Relaxed),
            ok_total: self.ok_total.load(Ordering::Relaxed),
            failed_total: self.failed_total.load(Ordering::Relaxed),
            gate_rejected_total: self.gate_rejected_total.load(Ordering::Relaxed),
            gate_rejected_invented: self.gate_rejected_invented.load(Ordering::Relaxed),
            gate_rejected_echo: self.gate_rejected_echo.load(Ordering::Relaxed),
            gate_rejected_dropped_numbers: self
                .gate_rejected_dropped_numbers
                .load(Ordering::Relaxed),
            truncated_total: self.truncated_total.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            served_total: self.served_total.load(Ordering::Relaxed),
            avg_latency_ms: if latency_count > 0 {
                self.latency_ms_sum.load(Ordering::Relaxed) / latency_count
            } else {
                0
            },
        }
    }
}

/// JSON-serializable metrics view. Plain `u64`s — atomic types serialize
/// erratically across serde versions (see `EventBusMetricsSnapshot`).
/// Single row in PR1; per-provider rows come back with the rotation pool.
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranslationMetricsSnapshot {
    pub dispatched_total: u64,
    pub ok_total: u64,
    pub failed_total: u64,
    pub gate_rejected_total: u64,
    pub gate_rejected_invented: u64,
    pub gate_rejected_echo: u64,
    pub gate_rejected_dropped_numbers: u64,
    pub truncated_total: u64,
    pub cache_hits: u64,
    pub served_total: u64,
    /// Mean round-trip of the accepted attempts and gate rejections; 0 when
    /// none.
    pub avg_latency_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_kinds_land_in_their_own_counters() {
        let metrics = TranslationMetrics::default();
        metrics.record_dispatch();
        metrics.record_dispatch();
        metrics.record_attempt(AttemptKind::Ok, 1500);
        metrics.record_attempt(AttemptKind::NetworkError, 0);
        metrics.record_attempt(AttemptKind::RateLimited, 10);
        metrics.record_attempt(AttemptKind::SlowInflight, 0);

        let snap = metrics.snapshot();
        assert_eq!(snap.dispatched_total, 2, "dispatches count attempts");
        assert_eq!((snap.ok_total, snap.failed_total), (1, 2));
        assert_eq!(
            snap.avg_latency_ms, 1500,
            "only the accepted attempt carries latency into the average"
        );
    }

    #[test]
    fn gate_rejections_bucket_and_count_as_failures() {
        let metrics = TranslationMetrics::default();
        metrics.record_gate_rejection(GateRejection::EchoOrRefusal, 900);
        metrics.record_gate_rejection(GateRejection::DroppedNumbers, 900);
        metrics.record_gate_rejection(GateRejection::Invented, 900);

        let snap = metrics.snapshot();
        assert_eq!(snap.gate_rejected_total, 3);
        assert_eq!(
            (
                snap.gate_rejected_echo,
                snap.gate_rejected_dropped_numbers,
                snap.gate_rejected_invented
            ),
            (1, 1, 1)
        );
        // A parseable reply is never allowed to count as both ok and
        // rejected: the rejection lands in failed, never in ok.
        assert_eq!(snap.failed_total, 3);
        assert_eq!(snap.ok_total, 0);
    }

    #[test]
    fn ok_and_rejected_are_mutually_exclusive_for_one_reply() {
        let metrics = TranslationMetrics::default();
        // The acceptance decision records exactly one of the two per reply.
        metrics.record_attempt(AttemptKind::Ok, 100);
        let before = metrics.snapshot();
        assert_eq!((before.ok_total, before.failed_total), (1, 0));

        metrics.record_gate_rejection(GateRejection::Invented, 100);
        let after = metrics.snapshot();
        assert_eq!(
            (after.ok_total, after.failed_total),
            (1, 1),
            "the refused reply moved failed, never ok"
        );
    }

    #[test]
    fn cache_hits_and_served_slots_count_separately_from_attempts() {
        let metrics = TranslationMetrics::default();
        metrics.record_cache_hit();
        metrics.record_cache_hit();
        metrics.record_served();
        metrics.record_truncated();

        let snap = metrics.snapshot();
        assert_eq!(snap.cache_hits, 2);
        assert_eq!(snap.served_total, 1);
        assert_eq!(snap.truncated_total, 1);
        assert_eq!(
            snap.dispatched_total, 0,
            "slot counters must not move the attempt counter"
        );
    }

    #[test]
    fn an_idle_snapshot_is_all_zeros() {
        assert_eq!(
            TranslationMetrics::default().snapshot(),
            TranslationMetricsSnapshot::default()
        );
    }
}
