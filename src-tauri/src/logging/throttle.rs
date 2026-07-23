//! Time-windowed throttle for high-frequency, near-duplicate WARN logs.
//!
//! Several broadcast subscribers (`pet_state_mapper`, `ws`, the chat-channel
//! subscribers, `automation::engine`, `acp::lifecycle`) log a `warn!` whenever
//! their `tokio::sync::broadcast` receiver returns `Lagged(n)`. `Lagged`
//! already coalesces `n` dropped events into ONE `recv()` error, so it can't
//! runaway on its own — but under sustained backpressure (a chronically slow
//! subscriber while a steady event stream flows) these branches still emit a
//! continuous trickle of near-identical lines. The `lagged_count` /
//! `forwarder_lagged_count` metrics remain the authoritative record of loss;
//! the log line is only an operator convenience, so collapsing bursts loses
//! nothing.
//!
//! [`LagLogThrottle`] is a pure state machine (no clock capture in the core,
//! no lock, no alloc). Each subscriber holds one instance as a task-local and
//! feeds every lag occurrence through [`LagLogThrottle::record`]; it returns
//! `Some(LagSummary)` only when a line should actually be written.

use std::time::{Duration, Instant};

/// Shared default window for lag WARN throttling. Leading-edge emit (see
/// [`LagLogThrottle`]) keeps the first lag instantly visible; a 10s window
/// then collapses a burst to at most one line while still surfacing ongoing
/// pressure roughly every 10 seconds.
pub const LAG_LOG_WINDOW: Duration = Duration::from_secs(10);

/// What a throttled lag WARN line should report: how many lag occurrences were
/// coalesced, and the summed count of dropped events across them, since the
/// previously emitted line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LagSummary {
    /// Lag occurrences coalesced into this line (always `>= 1`).
    pub occurrences: u64,
    /// Sum of dropped-event counts since the last emitted line.
    pub dropped: u64,
}

/// Leading-edge, time-windowed throttle.
///
/// The first hit emits immediately; further hits within `window` are counted
/// and suppressed; the first hit after the window elapses emits a
/// [`LagSummary`] carrying everything coalesced since the last emitted line.
/// Nothing is dropped silently — the suppressed tally always rides the next
/// emitted line.
#[derive(Debug)]
pub struct LagLogThrottle {
    window: Duration,
    /// `None` until the first line is emitted, which forces the leading-edge
    /// emit regardless of clock state.
    last_emit: Option<Instant>,
    pending_occ: u64,
    pending_dropped: u64,
}

impl LagLogThrottle {
    /// Build a throttle with the given minimum spacing between emitted lines.
    pub const fn new(window: Duration) -> Self {
        Self {
            window,
            last_emit: None,
            pending_occ: 0,
            pending_dropped: 0,
        }
    }

    /// Record one lag occurrence dropping `dropped` events, timestamped now.
    /// Returns `Some` when the caller should emit a WARN line this instant.
    pub fn record(&mut self, dropped: u64) -> Option<LagSummary> {
        self.record_at(Instant::now(), dropped)
    }

    /// Pure core of [`record`](Self::record) with an injected clock, so the
    /// windowing logic is unit-testable without sleeping.
    pub fn record_at(&mut self, now: Instant, dropped: u64) -> Option<LagSummary> {
        // Both accumulators saturate: reaching u64::MAX within a window is
        // infeasible, but this keeps the two counters consistent and avoids a
        // debug-build overflow panic on the occurrence count.
        self.pending_occ = self.pending_occ.saturating_add(1);
        self.pending_dropped = self.pending_dropped.saturating_add(dropped);

        // `saturating_duration_since` yields zero if the clock appears to move
        // backwards, so a backwards jump simply suppresses (never spuriously
        // emits and never panics).
        let due = match self.last_emit {
            None => true,
            Some(t) => now.saturating_duration_since(t) >= self.window,
        };
        if !due {
            return None;
        }

        let summary = LagSummary {
            occurrences: self.pending_occ,
            dropped: self.pending_dropped,
        };
        self.pending_occ = 0;
        self.pending_dropped = 0;
        self.last_emit = Some(now);
        Some(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Duration = Duration::from_secs(10);

    #[test]
    fn first_hit_emits_immediately() {
        let mut t = LagLogThrottle::new(W);
        let now = Instant::now();
        assert_eq!(
            t.record_at(now, 7),
            Some(LagSummary {
                occurrences: 1,
                dropped: 7,
            }),
            "the leading edge must surface instantly"
        );
    }

    #[test]
    fn burst_within_window_is_suppressed_and_accumulated() {
        let mut t = LagLogThrottle::new(W);
        let t0 = Instant::now();
        // Leading-edge line.
        assert!(t.record_at(t0, 3).is_some());
        // Everything else inside the window is swallowed...
        assert_eq!(t.record_at(t0 + Duration::from_secs(1), 5), None);
        assert_eq!(t.record_at(t0 + Duration::from_secs(2), 2), None);
        assert_eq!(t.record_at(t0 + Duration::from_secs(9), 1), None);
        // ...and re-surfaces coalesced on the first hit past the window,
        // carrying every suppressed occurrence + the one that crossed it.
        assert_eq!(
            t.record_at(t0 + Duration::from_secs(10), 4),
            Some(LagSummary {
                occurrences: 4,        // the 3 suppressed + this one
                dropped: 5 + 2 + 1 + 4,
            })
        );
    }

    #[test]
    fn window_boundary_is_inclusive() {
        let mut t = LagLogThrottle::new(W);
        let t0 = Instant::now();
        assert!(t.record_at(t0, 1).is_some());
        // Exactly `window` later counts as due (>=, not >).
        assert!(t.record_at(t0 + W, 1).is_some());
    }

    #[test]
    fn counters_reset_after_each_emit() {
        let mut t = LagLogThrottle::new(W);
        let t0 = Instant::now();
        assert_eq!(
            t.record_at(t0, 100),
            Some(LagSummary {
                occurrences: 1,
                dropped: 100,
            })
        );
        // Second window opens fresh — not cumulative with the first.
        assert_eq!(
            t.record_at(t0 + Duration::from_secs(11), 2),
            Some(LagSummary {
                occurrences: 1,
                dropped: 2,
            })
        );
    }

    #[test]
    fn dropped_sum_saturates_without_panicking() {
        let mut t = LagLogThrottle::new(W);
        let t0 = Instant::now();
        // First hit emits (occ=1) and resets; feed u64::MAX then one more
        // within the same window so the accumulator must saturate.
        assert!(t.record_at(t0, u64::MAX).is_some());
        assert_eq!(t.record_at(t0 + Duration::from_secs(1), u64::MAX), None);
        let summary = t
            .record_at(t0 + Duration::from_secs(11), 5)
            .expect("post-window hit emits");
        assert_eq!(summary.occurrences, 2);
        assert_eq!(summary.dropped, u64::MAX, "sum saturates rather than wrapping");
    }

    #[test]
    fn backwards_clock_does_not_emit_or_panic() {
        let mut t = LagLogThrottle::new(W);
        let t0 = Instant::now() + Duration::from_secs(60);
        assert!(t.record_at(t0, 1).is_some());
        // A clock that appears to move backwards saturates to zero elapsed,
        // which is < window, so it suppresses (and never panics).
        assert_eq!(t.record_at(t0 - Duration::from_secs(30), 1), None);
    }

    #[test]
    fn instances_are_independent() {
        let mut a = LagLogThrottle::new(W);
        let mut b = LagLogThrottle::new(W);
        let now = Instant::now();
        assert!(a.record_at(now, 1).is_some());
        // b has its own state; its first hit still emits.
        assert!(b.record_at(now, 1).is_some());
    }
}
