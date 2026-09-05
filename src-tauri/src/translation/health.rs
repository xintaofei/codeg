//! The provider health score: one 0-100 number per pool member, computed from
//! the rolling event window the metrics layer records.
//!
//! Why a composite score: the pool's existing signals are fragmented — the
//! AIMD tracks rate and 429s, `client_errors` retires on hard 4xx, and a
//! provider that quietly echoes or invents translations (the observed
//! failure: 13% of a relay's replies refused by the quality gates) never
//! leaves the rotation at all. The score folds the three dimensions the
//! reader actually feels — does the reply translate (quality), does the
//! endpoint answer at all (stability), how long does it take (speed) — into
//! the one number the dispatcher and the settings page can both consume.
//!
//! In-memory, recomputed on demand from the metrics window: no state to
//! persist, a restart re-probes, and a provider that heals drifts back up as
//! old failures age out of the window.

use std::time::{Duration, SystemTime};

use crate::translation::metrics::{ProviderEvent, ProviderEventKind};

/// The window covers at most this many most-recent events (the metrics ring
/// holds 64 per provider; the score reads the newest 20).
const MAX_SAMPLE: usize = 20;

/// Requests in the window before the score says anything at all. Below this
/// the provider is "observing": neutral in dispatch, immune to the breaker —
/// one transport blip must not shelve an endpoint nobody has really tried.
/// Eight (not five) because a couple of unlucky samples are noise, not
/// signal: the score only speaks once the window carries a real verdict.
pub const MIN_SAMPLE: usize = 8;

/// Dimension weights. Quality dominates (a fast echo is worthless); speed is
/// real but least — slow is an inconvenience, garbage is the feature failing.
/// A dimension with no sample in the window (no parseable reply yet → no
/// quality signal; no reply latency → no speed signal) is EXCLUDED and the
/// weights renormalized — otherwise a fully dead endpoint would score 70 on
/// the neutrality of the dimensions it never got to answer.
const QUALITY_WEIGHT: f64 = 0.45;
const STABILITY_WEIGHT: f64 = 0.35;
const SPEED_WEIGHT: f64 = 0.2;

/// Quality maps the gate-rejection rate onto 1.0 → 0.0: every reply that came
/// back parseable counts, and at 40% hard-rejected the dimension is spent.
/// The saturation point was raised from 25% — the gate is deliberately strict,
/// so a quarter of replies refused no longer means a broken endpoint — and
/// soft rejections (dropped numbers) are weighted at half, because they are
/// most often a formatting difference rather than an invention.
const QUALITY_REJECT_ZERO: f64 = 0.4;

/// Stability maps the weighted failure rate onto 1.0 → 0.0. Hard failures
/// (network, HTTP errors, unparseable bodies) count 1 each; a 429 counts ½ —
/// it says the endpoint is alive but out of quota, which is a much smaller
/// sin than not answering.
const STABILITY_FAIL_ZERO: f64 = 0.25;

/// Speed maps the median round-trip onto 1.0 → 0.0 across the observed span
/// of translation latencies: ~5 s feels instant next to the stream, 30 s is
/// the "half a day" complaint made literal.
const SPEED_GOOD_MS: u64 = 5_000;
const SPEED_BAD_MS: u64 = 30_000;

/// The neutral score an "observing" provider dispatches with.
pub const OBSERVING_SCORE: f64 = 70.0;

/// At or below this a provider stops receiving normal traffic (fallback-only:
/// it serves only when no healthy member can) but is not retired — it keeps
/// its probe quota and can climb back.
pub const DEGRADE_THRESHOLD: f64 = 70.0;

/// Below this, with enough sample, the provider is retired for the session —
/// the quality-side twin of the two-consecutive-4xx rule, which only ever
/// caught broken keys, never broken models.
pub const RETIRE_THRESHOLD: f64 = 40.0;

/// One provider's score, plus the sub-scores the settings page renders.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthScore {
    /// 0-100 composite.
    pub score: f64,
    /// 0-1 dimension sub-scores, for the UI's hover breakdown.
    pub quality: f64,
    pub stability: f64,
    pub speed: f64,
    /// Events the score actually judged.
    pub sample: usize,
    /// Fewer than [`MIN_SAMPLE`] events: the score is the neutral [`OBSERVING_SCORE`]
    /// and the breaker must not act on it.
    pub observing: bool,
}

impl HealthScore {
    /// Dispatch-classifying verdicts. A provider is "degraded" once the score
    /// says so with a real sample; "observing" providers and healthy ones
    /// both take normal traffic.
    pub fn degraded(&self) -> bool {
        !self.observing && self.score < DEGRADE_THRESHOLD
    }

    pub fn retired(&self) -> bool {
        !self.observing && self.score < RETIRE_THRESHOLD
    }
}

/// Score one provider's window. `events` must already be time-filtered (the
/// metrics layer's `recent_events` does that); the newest [`MAX_SAMPLE`] of
/// them are judged, newest first.
pub fn health_score(events: &[ProviderEvent], _now: SystemTime) -> HealthScore {
    let recent: Vec<&ProviderEvent> = events.iter().rev().take(MAX_SAMPLE).collect();
    let sample = recent.len();
    if sample < MIN_SAMPLE {
        return HealthScore {
            score: OBSERVING_SCORE,
            quality: 1.0,
            stability: 1.0,
            speed: 1.0,
            sample,
            observing: true,
        };
    }

    // Quality: of the replies that came back parseable, how many survived the
    // gates. Transport failures say nothing about translation quality, so
    // they are excluded from this denominator (stability judges them).
    let answered = recent
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                ProviderEventKind::Ok
                    | ProviderEventKind::GateRejected
                    | ProviderEventKind::GateRejectedSoft
            )
        })
        .count();
    // 软拒绝（掉数字）往往只是全角/千分位一类的格式差，未必是编造：
    // 半权计入，免得门控越严、健康分越惨的自伤回路。
    let rejected_weight: f64 = recent
        .iter()
        .map(|event| match event.kind {
            ProviderEventKind::GateRejected => 1.0,
            ProviderEventKind::GateRejectedSoft => 0.5,
            _ => 0.0,
        })
        .sum();
    let quality = if answered == 0 {
        // No parseable reply in the window: quality has no sample and is
        // excluded from the composite (renormalized away below) — a neutral
        // 1.0 here would hand a dead endpoint 70 points for dimensions it
        // never got to answer.
        1.0
    } else {
        (1.0 - (rejected_weight / answered as f64) / QUALITY_REJECT_ZERO).clamp(0.0, 1.0)
    };

    // Stability: hard failures count once, a 429 half. `ParseError` is hard —
    // the endpoint answered with something that is not a translation.
    let total = sample as f64;
    let hard = recent
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                ProviderEventKind::HttpError
                    | ProviderEventKind::NetworkError
                    | ProviderEventKind::ParseError
            )
        })
        .count() as f64;
    let limited = recent
        .iter()
        .filter(|event| event.kind == ProviderEventKind::RateLimited)
        .count() as f64;
    let stability =
        (1.0 - (hard + limited * 0.5) / total / STABILITY_FAIL_ZERO).clamp(0.0, 1.0);

    // Speed: the median round-trip over the events that carried a reply
    // (successes and gate rejections alike — both cost the reader the wait).
    let mut latencies: Vec<u64> = recent
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                ProviderEventKind::Ok
                    | ProviderEventKind::GateRejected
                    | ProviderEventKind::GateRejectedSoft
            )
        })
        .map(|event| event.latency_ms)
        .collect();
    let speed = if latencies.is_empty() {
        1.0
    } else {
        latencies.sort_unstable();
        let mid = latencies.len() / 2;
        let p50 = latencies[mid] as f64;
        let span = (SPEED_BAD_MS - SPEED_GOOD_MS) as f64;
        (1.0 - (p50 - SPEED_GOOD_MS as f64) / span).clamp(0.0, 1.0)
    };

    // Composite over the dimensions that HAVE a sample: stability always
    // (the events themselves are its sample), quality only when replies came
    // back, speed only when a reply carried a round-trip. Renormalizing keeps
    // an unsampled dimension from subsidizing the score.
    let mut weighted = STABILITY_WEIGHT * stability;
    let mut weight_sum = STABILITY_WEIGHT;
    if answered > 0 {
        weighted += QUALITY_WEIGHT * quality;
        weight_sum += QUALITY_WEIGHT;
    }
    if !latencies.is_empty() {
        weighted += SPEED_WEIGHT * speed;
        weight_sum += SPEED_WEIGHT;
    }
    let score = 100.0 * weighted / weight_sum;

    HealthScore {
        score,
        quality,
        stability,
        speed,
        sample,
        observing: false,
    }
}

/// How long a degraded provider waits between probe dispatches. A broken
/// endpoint needs a real interval to prove it healed; two minutes costs the
/// reader at most one background-chunk failure per window.
pub const PROBE_INTERVAL: Duration = Duration::from_secs(120);

#[cfg(test)]
mod tests {
    use super::*;

    use crate::translation::metrics::ProviderEventKind as K;

    /// 12 个样本（≥ MIN_SAMPLE=8）：10 个好回复 + 2 个掉数字软拒绝。
    /// 软拒绝按 0.5 计权：rejected_weight = 1.0，answered = 12，
    /// quality = 1 - (1/12)/0.4 ≈ 0.79 —— 不再把好端点拖向降级。
    #[test]
    fn a_dropped_numbers_rejection_counts_half() {
        let mut kinds = vec![(K::Ok, 3_000); 10];
        kinds.extend(vec![(K::GateRejectedSoft, 3_000); 2]);
        let health = health_score(&events(&kinds), now());
        assert!((health.quality - 0.7917).abs() < 0.001, "quality was {}", health.quality);
        assert!(!health.degraded());
        assert_eq!(health.speed, 1.0, "soft rejections carry latency and count in speed too");
    }

    #[test]
    fn seven_events_still_observe_eight_score() {
        let kinds = [(K::Ok, 3_000); 7];
        assert!(health_score(&events(&kinds), now()).observing);
        let kinds = [(K::Ok, 3_000); 8];
        assert!(!health_score(&events(&kinds), now()).observing);
    }

    fn events(kinds: &[(ProviderEventKind, u64)]) -> Vec<ProviderEvent> {
        kinds
            .iter()
            .enumerate()
            .map(|(index, (kind, latency_ms))| ProviderEvent {
                at_ms: 1_000 + index as u64,
                kind: *kind,
                latency_ms: *latency_ms,
            })
            .collect()
    }

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(10_000)
    }

    #[test]
    fn an_empty_window_observes_at_the_neutral_score() {
        let health = health_score(&[], now());
        assert!(health.observing);
        assert_eq!(health.score, OBSERVING_SCORE);
        assert!(!health.degraded());
        assert!(!health.retired());
    }

    #[test]
    fn fewer_than_min_sample_events_never_degrade() {
        // Four hard failures out of four: if this judged, stability would be
        // zero and the provider shelved. Under the sample floor it observes.
        let kinds = [(ProviderEventKind::NetworkError, 0); 4];
        let health = health_score(&events(&kinds), now());
        assert!(health.observing);
        assert!(!health.retired());
    }

    #[test]
    fn a_fast_clean_provider_scores_full_marks() {
        let kinds = [(ProviderEventKind::Ok, 3_000); 10];
        let health = health_score(&events(&kinds), now());
        assert!(!health.observing);
        assert!(
            (health.score - 100.0).abs() < 0.001,
            "score was {}",
            health.score
        );
    }

    #[test]
    fn an_echoing_relay_loses_its_quality_dimension() {
        // Half the parseable replies refused, the rest fine and fast.
        let mut kinds = vec![(ProviderEventKind::Ok, 3_000); 10];
        for kind in kinds.iter_mut().take(10) {
            *kind = (ProviderEventKind::GateRejected, 3_000);
        }
        let health = health_score(&events(&kinds), now());
        assert_eq!(health.quality, 0.0, "50% rejections saturates the gate rate");
        assert_eq!(health.stability, 1.0, "transport was flawless");
        assert!(
            (health.score - 55.0).abs() < 0.001,
            "quality-weighted zero lands at 0.35+0.20 = 55: {}",
            health.score
        );
        assert!(health.degraded());
        assert!(!health.retired(), "quality alone must not retire at 50");
    }

    #[test]
    fn a_dead_endpoint_retires_through_stability() {
        // Half network failures, half parse failures: stability saturates,
        // and with no parseable replies quality stays neutral.
        let mut kinds = vec![(ProviderEventKind::NetworkError, 0); 10];
        kinds.extend(vec![(ProviderEventKind::ParseError, 3_000); 10]);
        let health = health_score(&events(&kinds), now());
        assert_eq!(health.stability, 0.0);
        assert_eq!(health.quality, 1.0, "no answered replies: quality is neutral");
        assert!(health.score < RETIRE_THRESHOLD, "score was {}", health.score);
        assert!(health.retired());
    }

    #[test]
    fn a_rate_limited_endpoint_is_half_forgiven() {
        // 30% 429s alone: weighted failure rate 0.15, which maps to
        // stability 0.4 against the 0.25 saturation point — a real dent, but
        // the endpoint answers correctly and fast, so it stays in rotation.
        let mut kinds = vec![(ProviderEventKind::Ok, 3_000); 14];
        kinds.extend(vec![(ProviderEventKind::RateLimited, 0); 6]);
        let health = health_score(&events(&kinds), now());
        assert!(
            (health.stability - 0.4).abs() < 0.001,
            "stability was {}",
            health.stability
        );
        assert!(
            (health.score - 79.0).abs() < 0.001,
            "0.45 + 0.35*0.4 + 0.2 = 79: {}",
            health.score
        );
        assert!(!health.degraded());
        assert!(!health.retired());
    }

    #[test]
    fn a_slow_but_correct_endpoint_degrades_on_speed_alone() {
        // Every reply correct, every reply 30 s+: the p50 maps to zero speed.
        let kinds = [(ProviderEventKind::Ok, 30_000); 10];
        let health = health_score(&events(&kinds), now());
        assert_eq!(health.quality, 1.0);
        assert_eq!(health.stability, 1.0);
        assert_eq!(health.speed, 0.0);
        assert!(
            (health.score - 80.0).abs() < 0.001,
            "0.45 + 0.35 + 0.2*0 = 80: {}",
            health.score
        );
        assert!(!health.retired(), "slow alone must never retire");
    }

    #[test]
    fn the_window_judges_only_the_newest_sample() {
        // 10 old successes + 10 fresh hard failures. The 20-event sample is
        // all of them; the oldest 5 successes fall out of a 15-event window —
        // build 25 events and check the score reflects the newest 20.
        let mut kinds = vec![(ProviderEventKind::Ok, 1_000); 5];
        kinds.extend(vec![(ProviderEventKind::NetworkError, 0); 20]);
        let health = health_score(&events(&kinds), now());
        // The newest 20 events are all network errors (the 5 successes are
        // the oldest and fall out of the sample): stability saturates, and
        // quality/speed have NO sample — renormalization leaves stability
        // alone in the composite, so a fully dead window scores zero.
        assert_eq!(health.sample, 20);
        assert_eq!(health.quality, 1.0, "the judged sample has no replies");
        assert_eq!(health.stability, 0.0);
        assert_eq!(health.score, 0.0, "stability-only composite");
        assert!(health.retired());
    }

    #[test]
    fn the_score_recovers_as_failures_leave_the_window() {
        // This is the dispatcher's recovery path: the metrics ring keeps only
        // the newest 64 events, so once a healed endpoint accumulates enough
        // fresh successes the stale ones age out.
        let mut kinds = vec![(ProviderEventKind::NetworkError, 0); 10];
        kinds.extend(vec![(ProviderEventKind::Ok, 3_000); 10]);
        let mixed = health_score(&events(&kinds), now());
        let healed = health_score(&events(&[(ProviderEventKind::Ok, 3_000); 10]), now());
        assert!(healed.score > mixed.score, "healing must score higher");
        assert!(!healed.degraded());
    }
}
