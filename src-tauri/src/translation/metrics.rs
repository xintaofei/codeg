//! Process-wide translation counters: dispatch volume, cache effectiveness,
//! gate rejections, and per-provider transport outcomes.
//!
//! Plain `AtomicU64` like [`crate::acp::internal_bus::EventBusMetrics`] — no
//! metrics framework. Two consumers read this: the settings page's status
//! strip (via the `translation_metrics` command) and the provider health
//! score, which reads each provider's rolling event window.
//!
//! Division of recording labor (so no outcome is counted twice):
//! - `client.rs` records **per outbound attempt**: the dispatch itself plus
//!   its transport-level verdict (ok / rate-limited / HTTP error / network
//!   error / parse failure). A retried chunk therefore shows every attempt.
//! - `mod.rs` records **per served slot**: cache hits, served totals, and the
//!   quality-gate rejections attributed to the provider that produced the
//!   reply.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Events kept per provider for the health score's rolling window. The
/// window itself is time-bounded (10 minutes); the cap bounds memory when a
/// fast endpoint produces far more events than the window needs.
const PROVIDER_EVENT_WINDOW_CAP: usize = 64;

/// How far back the health score's window reaches.
pub const HEALTH_WINDOW: Duration = Duration::from_secs(600);

/// One per-provider outcome in the rolling window. `latency_ms` is the full
/// round trip for the attempt that produced the event (0 where no request
/// was made — currently never; gate rejections reuse the attempt's latency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderEvent {
    /// Millis since the Unix epoch, so events survive across the midnight
    /// log rotation and read naturally in snapshots.
    pub at_ms: u64,
    pub kind: ProviderEventKind,
    pub latency_ms: u64,
}

/// The transport- and quality-level verdicts the two recorders emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEventKind {
    /// The endpoint returned a parseable, complete translation.
    Ok,
    /// The reply was parseable but the quality gate refused it
    /// (invented content, echo/refusal, dropped numbers).
    GateRejected,
    /// The reply was parseable but the quality gate refused it on the
    /// least-trustworthy signal (dropped numbers — often a formatting
    /// difference, not an invention). Tracked apart so the health score
    /// can weight it at half a hard rejection.
    GateRejectedSoft,
    /// HTTP 429.
    RateLimited,
    /// Any other non-success HTTP status.
    HttpError,
    /// Transport failure: timeout, connection reset, DNS.
    NetworkError,
    /// The reply body could not be parsed into a translation.
    ParseError,
}

impl ProviderEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderEventKind::Ok => "ok",
            ProviderEventKind::GateRejected => "gate_rejected",
            ProviderEventKind::GateRejectedSoft => "gate_rejected_soft",
            ProviderEventKind::RateLimited => "rate_limited",
            ProviderEventKind::HttpError => "http_error",
            ProviderEventKind::NetworkError => "network_error",
            ProviderEventKind::ParseError => "parse_error",
        }
    }
}

/// Per-provider counters plus the rolling event window.
#[derive(Debug, Default)]
pub struct ProviderCounters {
    pub sent: AtomicU64,
    pub ok: AtomicU64,
    pub gate_rejected: AtomicU64,
    pub rate_limited: AtomicU64,
    pub http_error: AtomicU64,
    pub network_error: AtomicU64,
    pub parse_error: AtomicU64,
    pub latency_ms_sum: AtomicU64,
    pub latency_count: AtomicU64,
    /// Minute index (Unix minutes) the `dispatch_minute_count` bucket covers.
    dispatch_minute: AtomicU64,
    dispatch_minute_count: AtomicU64,
    events: Mutex<VecDeque<ProviderEvent>>,
}

impl ProviderCounters {
    fn record(&self, kind: ProviderEventKind, latency_ms: u64) {
        match kind {
            ProviderEventKind::Ok => {
                self.ok.fetch_add(1, Ordering::Relaxed);
                self.latency_ms_sum.fetch_add(latency_ms, Ordering::Relaxed);
                self.latency_count.fetch_add(1, Ordering::Relaxed);
            }
            ProviderEventKind::GateRejected | ProviderEventKind::GateRejectedSoft => {
                self.gate_rejected.fetch_add(1, Ordering::Relaxed);
            }
            ProviderEventKind::RateLimited => {
                self.rate_limited.fetch_add(1, Ordering::Relaxed);
            }
            ProviderEventKind::HttpError => {
                self.http_error.fetch_add(1, Ordering::Relaxed);
            }
            ProviderEventKind::NetworkError => {
                self.network_error.fetch_add(1, Ordering::Relaxed);
            }
            ProviderEventKind::ParseError => {
                self.parse_error.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.push_event(kind, latency_ms);
    }

    fn push_event(&self, kind: ProviderEventKind, latency_ms: u64) {
        let at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or_default();
        let mut events = self.events.lock().expect("provider event window lock");
        events.push_back(ProviderEvent {
            at_ms,
            kind,
            latency_ms,
        });
        while events.len() > PROVIDER_EVENT_WINDOW_CAP {
            events.pop_front();
        }
    }

    /// The window events the health score consumes: everything within
    /// [`HEALTH_WINDOW`], oldest first.
    pub fn recent_events(&self, now: SystemTime) -> Vec<ProviderEvent> {
        let cutoff = now
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or_default()
            .saturating_sub(HEALTH_WINDOW.as_millis() as u64);
        self.events
            .lock()
            .expect("provider event window lock")
            .iter()
            .filter(|event| event.at_ms >= cutoff)
            .copied()
            .collect()
    }

    /// POSTs this provider made in the current wall-clock minute. The bucket
    /// resets lazily on the first dispatch of a new minute; a read from an
    /// older minute reports 0.
    fn dispatched_last_minute(&self) -> u64 {
        let now_minute = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs() / 60)
            .unwrap_or_default();
        if self.dispatch_minute.load(Ordering::Relaxed) == now_minute {
            self.dispatch_minute_count.load(Ordering::Relaxed)
        } else {
            0
        }
    }

    fn note_dispatch(&self) {
        let now_minute = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs() / 60)
            .unwrap_or_default();
        if self.dispatch_minute.swap(now_minute, Ordering::Relaxed) != now_minute {
            self.dispatch_minute_count.store(0, Ordering::Relaxed);
        }
        self.dispatch_minute_count.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> ProviderMetricsSnapshot {
        let latency_count = self.latency_count.load(Ordering::Relaxed);
        ProviderMetricsSnapshot {
            sent: self.sent.load(Ordering::Relaxed),
            ok: self.ok.load(Ordering::Relaxed),
            gate_rejected: self.gate_rejected.load(Ordering::Relaxed),
            rate_limited: self.rate_limited.load(Ordering::Relaxed),
            http_error: self.http_error.load(Ordering::Relaxed),
            network_error: self.network_error.load(Ordering::Relaxed),
            parse_error: self.parse_error.load(Ordering::Relaxed),
            avg_latency_ms: if latency_count > 0 {
                self.latency_ms_sum.load(Ordering::Relaxed) / latency_count
            } else {
                0
            },
            dispatched_last_minute: self.dispatched_last_minute(),
        }
    }
}

/// Global counters plus per-provider entries. Process-wide, in-memory only —
/// a restart re-probes, matching the pool's own philosophy.
#[derive(Debug, Default)]
pub struct TranslationMetrics {
    /// Outbound POSTs, all attempts included.
    pub dispatched_total: AtomicU64,
    /// Slots served from the content-addressed cache (no network).
    pub cache_hits: AtomicU64,
    /// Slots whose text rendered (cache, native-skip, or network success).
    pub served_total: AtomicU64,
    /// Slots the quality gate refused (all buckets below sum to this).
    pub gate_rejected_total: AtomicU64,
    pub gate_rejected_invented: AtomicU64,
    pub gate_rejected_echo: AtomicU64,
    pub gate_rejected_dropped_numbers: AtomicU64,
    /// Slots the endpoint truncated (max_tokens / `finish_reason: length`).
    pub truncated_total: AtomicU64,
    providers: Mutex<HashMap<String, std::sync::Arc<ProviderCounters>>>,
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
    fn entry(&self, provider_id: &str) -> std::sync::Arc<ProviderCounters> {
        self.providers
            .lock()
            .expect("translation metrics lock")
            .entry(provider_id.to_string())
            .or_default()
            .clone()
    }

    /// One outbound POST (per attempt, not per chunk — a retried chunk shows
    /// every attempt, which is what pacing and health analysis need).
    pub fn record_dispatch(&self, provider_id: &str) {
        self.dispatched_total.fetch_add(1, Ordering::Relaxed);
        let entry = self.entry(provider_id);
        entry.sent.fetch_add(1, Ordering::Relaxed);
        entry.note_dispatch();
    }

    /// A per-attempt transport/parse verdict from `client.rs`.
    pub fn record_attempt(&self, provider_id: &str, kind: ProviderEventKind, latency_ms: u64) {
        self.entry(provider_id).record(kind, latency_ms);
    }

    /// A quality-gate rejection from `mod.rs`, attributed to the provider
    /// that produced the refused reply.
    pub fn record_gate_rejection(
        &self,
        provider_id: &str,
        rejection: GateRejection,
        latency_ms: u64,
    ) {
        self.gate_rejected_total.fetch_add(1, Ordering::Relaxed);
        match rejection {
            GateRejection::Invented => {
                self.gate_rejected_invented.fetch_add(1, Ordering::Relaxed)
            }
            GateRejection::EchoOrRefusal => self.gate_rejected_echo.fetch_add(1, Ordering::Relaxed),
            GateRejection::DroppedNumbers => {
                self.gate_rejected_dropped_numbers
                    .fetch_add(1, Ordering::Relaxed)
            }
        };
        let event_kind = match rejection {
            GateRejection::Invented | GateRejection::EchoOrRefusal => ProviderEventKind::GateRejected,
            GateRejection::DroppedNumbers => ProviderEventKind::GateRejectedSoft,
        };
        self.entry(provider_id).record(event_kind, latency_ms);
    }

    pub fn record_truncated(&self) {
        self.truncated_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_served(&self) {
        self.served_total.fetch_add(1, Ordering::Relaxed);
    }

    /// The window events for one provider, oldest first. Unknown providers
    /// have no window.
    pub fn recent_events(&self, provider_id: &str, now: SystemTime) -> Vec<ProviderEvent> {
        self.providers
            .lock()
            .expect("translation metrics lock")
            .get(provider_id)
            .map(|entry| entry.recent_events(now))
            .unwrap_or_default()
    }

    /// POSTs `provider_id` made in the current wall-clock minute; 0 for an
    /// unknown provider. The pool status reads this for the "actual dispatch
    /// rate" badge.
    pub fn dispatched_last_minute(&self, provider_id: &str) -> u64 {
        self.providers
            .lock()
            .expect("translation metrics lock")
            .get(provider_id)
            .map(|entry| entry.dispatched_last_minute())
            .unwrap_or(0)
    }

    /// The JSON-serializable view for the settings page.
    pub fn snapshot(&self) -> TranslationMetricsSnapshot {
        let providers = self
            .providers
            .lock()
            .expect("translation metrics lock")
            .iter()
            .map(|(id, entry)| {
                (
                    id.clone(),
                    entry.snapshot(),
                )
            })
            .collect();
        TranslationMetricsSnapshot {
            dispatched_total: self.dispatched_total.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            served_total: self.served_total.load(Ordering::Relaxed),
            gate_rejected_total: self.gate_rejected_total.load(Ordering::Relaxed),
            gate_rejected_invented: self.gate_rejected_invented.load(Ordering::Relaxed),
            gate_rejected_echo: self.gate_rejected_echo.load(Ordering::Relaxed),
            gate_rejected_dropped_numbers: self.gate_rejected_dropped_numbers.load(Ordering::Relaxed),
            truncated_total: self.truncated_total.load(Ordering::Relaxed),
            providers,
        }
    }
}

/// JSON-serializable metrics view. Plain `u64`s — atomic types serialize
/// erratically across serde versions (see `EventBusMetricsSnapshot`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationMetricsSnapshot {
    pub dispatched_total: u64,
    pub cache_hits: u64,
    pub served_total: u64,
    pub gate_rejected_total: u64,
    pub gate_rejected_invented: u64,
    pub gate_rejected_echo: u64,
    pub gate_rejected_dropped_numbers: u64,
    pub truncated_total: u64,
    /// Keyed by the provider id (`ProviderConfig::id`, `"legacy"` for the
    /// migrated flat row) so the settings page can join it with the pool
    /// status rows.
    pub providers: HashMap<String, ProviderMetricsSnapshot>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMetricsSnapshot {
    pub sent: u64,
    pub ok: u64,
    pub gate_rejected: u64,
    pub rate_limited: u64,
    pub http_error: u64,
    pub network_error: u64,
    pub parse_error: u64,
    pub avg_latency_ms: u64,
    pub dispatched_last_minute: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_kinds_land_in_their_own_counters() {
        let metrics = TranslationMetrics::default();
        metrics.record_dispatch("a");
        metrics.record_dispatch("a");
        metrics.record_attempt("a", ProviderEventKind::Ok, 1500);
        metrics.record_attempt("a", ProviderEventKind::NetworkError, 0);
        metrics.record_attempt("a", ProviderEventKind::RateLimited, 10);
        metrics.record_attempt("b", ProviderEventKind::Ok, 3000);

        let snap = metrics.snapshot();
        assert_eq!(snap.dispatched_total, 2, "dispatches are global");
        let a = &snap.providers["a"];
        assert_eq!((a.sent, a.ok, a.network_error, a.rate_limited), (2, 1, 1, 1));
        assert_eq!(a.avg_latency_ms, 1500, "average covers the ok attempt only");
        assert_eq!(snap.providers["b"].ok, 1);
        assert!(a.dispatched_last_minute > 0, "same-minute dispatch is visible");
    }

    #[test]
    fn gate_rejections_bucket_and_attribute() {
        let metrics = TranslationMetrics::default();
        metrics.record_gate_rejection("a", GateRejection::EchoOrRefusal, 900);
        metrics.record_gate_rejection("a", GateRejection::DroppedNumbers, 900);
        metrics.record_gate_rejection("a", GateRejection::Invented, 900);

        let snap = metrics.snapshot();
        assert_eq!(snap.gate_rejected_total, 3);
        assert_eq!((snap.gate_rejected_echo, snap.gate_rejected_dropped_numbers, snap.gate_rejected_invented), (1, 1, 1));
        assert_eq!(snap.providers["a"].gate_rejected, 3);
    }

    #[test]
    fn the_event_window_is_time_bounded_and_ordered() {
        let metrics = TranslationMetrics::default();
        metrics.record_attempt("a", ProviderEventKind::Ok, 100);
        let now = SystemTime::now();
        // A fresh window contains the fresh event.
        assert_eq!(metrics.recent_events("a", now).len(), 1);
        // Unknown providers expose no window.
        assert!(metrics.recent_events("ghost", now).is_empty());

        // An event stamped outside the window does not come back.
        let entry = metrics.entry("a");
        {
            let mut events = entry.events.lock().unwrap();
            events.push_front(ProviderEvent {
                at_ms: now.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
                    - HEALTH_WINDOW.as_millis() as u64
                    - 1_000,
                kind: ProviderEventKind::Ok,
                latency_ms: 1,
            });
        }
        let window = metrics.recent_events("a", now);
        assert_eq!(window.len(), 1, "only the in-window event survives");
    }

    #[test]
    fn the_per_provider_event_cap_bounds_memory() {
        let metrics = TranslationMetrics::default();
        for _ in 0..(PROVIDER_EVENT_WINDOW_CAP + 20) {
            metrics.record_attempt("a", ProviderEventKind::Ok, 1);
        }
        let entry = metrics.entry("a");
        assert_eq!(entry.events.lock().unwrap().len(), PROVIDER_EVENT_WINDOW_CAP);
    }
}
