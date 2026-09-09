//! The provider rotation pool: who serves the next translation request, and
//! what each endpoint has taught the limiter about itself.
//!
//! One [`PoolState`] per settings shape lives for the process's duration,
//! keyed by the enabled providers' identities: editing the provider list (or
//! reordering it) builds a fresh pool, while untouched settings keep their
//! adaptive history across requests. AIMD state and session-level disables
//! are runtime facts, never persisted — a restart re-probes, which costs a
//! few 429s at worst and keeps nothing stale on disk.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

use crate::app_error::AppCommandError;
use crate::translation::aimd::AimdController;
use crate::translation::health::{self, HealthScore};
use crate::translation::settings::{
    ProviderConfig, COOLDOWN_SECONDS_DEFAULT, FAILURE_THRESHOLD_DEFAULT,
};

/// Park the whole dispatch no longer than this when every provider is in a
/// `Retry-After` cooldown. A reading flow waiting behind a longer window is
/// better served by failing the batch and letting the frontend's bounded
/// retry converge than by holding request futures for minutes.
const MAX_WAIT_ALL_COOLING: Duration = Duration::from_secs(30);

type RuntimeMap = Arc<Mutex<HashMap<String, ProviderRuntime>>>;

/// Callbacks fired when a pool member's OBSERVABLE state changes — its
/// adaptive rate moved, a `Retry-After` parked or released it, or it was
/// retired for the session. The app wires one callback at startup that emits
/// `translation-pool-changed` to the frontends, which re-fetch the status
/// strip immediately instead of polling.
type ChangeCallback = Arc<dyn Fn() + Send + Sync>;

static CHANGE_NOTIFIERS: OnceLock<RwLock<Vec<ChangeCallback>>> = OnceLock::new();

fn notifiers() -> &'static RwLock<Vec<ChangeCallback>> {
    CHANGE_NOTIFIERS.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register a change listener. Wired once per process at startup (desktop and
/// server mode each route the callback into their own event channel); the
/// callback runs synchronously on the reporting request's task, so keep it
/// cheap — emit-and-return, no status computation.
pub fn on_change(callback: ChangeCallback) {
    notifiers()
        .write()
        .expect("pool notifier lock is never poisoned across a panic-free run")
        .push(callback);
}

fn notify_change() {
    let callbacks = notifiers()
        .read()
        .expect("pool notifier lock is never poisoned across a panic-free run");
    for callback in callbacks.iter() {
        callback();
    }
}

/// The notifier registry is process-global and tests drive real
/// notify_change() calls (retirement), so every test that touches it holds
/// this lock — the serialization the older notification test relied on by
/// running alone.
#[cfg(test)]
static NOTIFIER_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
pub(crate) fn reset_notifiers_for_test() {
    notifiers()
        .write()
        .expect("pool notifier lock is never poisoned across a panic-free run")
        .clear();
}

/// Runtime per-provider facts the adaptive layer learns and the settings page
/// displays. Keyed by the provider's stable id.
#[derive(Debug, Default)]
struct ProviderRuntime {
    aimd: Option<AimdController>,
    /// Consecutive failed requests (429s and hard errors alike). When the
    /// streak reaches the settings' failure threshold the member sits out a
    /// cooldown and the streak resets.
    consecutive_failures: u32,
    /// Until this instant the member sits out a failure cooldown; `None` when
    /// dispatchable. The threshold and the length live in the settings and
    /// are read per request, not here — the pool registry keys runtime state
    /// by membership only, so settings-shaped values cannot live in entries.
    cooldown_until: Option<Instant>,
    /// Set once retired: the reason, shown verbatim in the settings page.
    disabled_reason: Option<String>,
    /// The provider's last claimed pacing slot, in micros since the pool's
    /// origin — one timeline per provider, so a re-entering member does not
    /// inherit the pool's shared cadence.
    last_dispatch_us: u64,
    /// The last health-probe dispatch this member received. A degraded
    /// member gets one request per [`health::PROBE_INTERVAL`] so a healed
    /// endpoint can prove itself and climb back into rotation — without it,
    /// a member with no traffic has no fresh events and no way to recover.
    last_probe_at: Option<Instant>,
}

impl ProviderRuntime {
    /// The entry for a provider that has never been hit: an adaptive limiter
    /// seeded from its configured ceiling.
    fn seeded(provider: &ProviderConfig) -> Self {
        Self {
            aimd: Some(AimdController::new(provider.rpm_cap)),
            ..Default::default()
        }
    }
}

/// The rotation pool for one settings shape.
pub struct PoolState {
    providers: Vec<ProviderConfig>,
    runtime: RuntimeMap,
    /// Monotonic round-robin cursor across the whole pool.
    cursor: AtomicUsize,
    /// Microsecond timestamp origin shared by the per-provider pacing slots.
    origin: Instant,
    /// Reserved for a future pool-wide pace; per-provider slots superseded it.
    _last_dispatch: AtomicU64,
}

/// One pick from the pool: which endpoint to hit, plus the runtime handle the
/// client reports the outcome to.
pub struct PickedProvider {
    pub provider: ProviderConfig,
    runtime: RuntimeMap,
    origin: Instant,
    provider_id: String,
    /// The failure streak that lands a cooldown, and that cooldown's length —
    /// snapshotted from the settings accessors at pick time, so a mid-flight
    /// settings edit cannot rewrite the thresholds under a request already in
    /// the air.
    failure_threshold: u32,
    cooldown_secs: u64,
}

impl std::fmt::Debug for PickedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PickedProvider")
            .field("provider_id", &self.provider_id)
            .field("base_url", &self.provider.base_url)
            .finish_non_exhaustive()
    }
}

impl PickedProvider {
    /// The endpoint's identity for logging and status.
    pub fn id(&self) -> &str {
        &self.provider_id
    }

    /// A clean response earns climb credit and clears the failure streak.
    /// Notifies only when the reward actually moved the observable rate
    /// (every [`AimdController`]'s [`crate::translation::aimd`] step), not on
    /// the quiet successes between steps — a per-request event would be pure
    /// noise.
    pub fn report_success(&self) {
        let changed = {
            let mut runtime = self
                .runtime
                .lock()
                .expect("pool runtime lock is never poisoned across a panic-free run");
            let entry = runtime.entry(self.provider_id.clone()).or_default();
            entry.consecutive_failures = 0;
            match entry.aimd.as_mut() {
                Some(aimd) => {
                    let before = aimd.allowed_rpm();
                    aimd.reward();
                    aimd.allowed_rpm() != before
                }
                None => false,
            }
        };
        if changed {
            notify_change();
        }
    }

    /// A rate-limit verdict: halve and cool down per the endpoint's header,
    /// and count the 429 toward the failure threshold — a rate-limited
    /// endpoint is still a failed request, so a run of them parks the member
    /// just as hard errors do. The cooldown itself still lands only through
    /// [`Self::report_failure`]: a 429 already carries its own `Retry-After`
    /// park. Takes the lock briefly; the change is visible to every queued
    /// request immediately — one 429 slows the whole provider, not just the
    /// request that drew it. Always observable, so always notifies.
    pub fn report_rate_limited(&self, retry_after: Option<Duration>) {
        {
            let mut runtime = self
                .runtime
                .lock()
                .expect("pool runtime lock is never poisoned across a panic-free run");
            let entry = runtime.entry(self.provider_id.clone()).or_default();
            entry.consecutive_failures += 1;
            if let Some(aimd) = entry.aimd.as_mut() {
                aimd.penalize(retry_after, Instant::now());
            }
        }
        notify_change();
    }

    /// A failed request. Counts toward the configured failure threshold; when
    /// the streak reaches it, the member sits out a cooldown of the
    /// configured length and the streak resets — a bounded timeout the
    /// endpoint can recover from on its own, not a session exile. Notifies
    /// when the cooldown lands, which is the only observable step here.
    pub fn report_failure(&self, detail: &str) {
        let cooled = {
            let mut runtime = self
                .runtime
                .lock()
                .expect("pool runtime lock is never poisoned across a panic-free run");
            let entry = runtime.entry(self.provider_id.clone()).or_default();
            entry.consecutive_failures += 1;
            let cooled = entry.consecutive_failures >= self.failure_threshold;
            if cooled {
                entry.consecutive_failures = 0;
                entry.cooldown_until =
                    Some(Instant::now() + Duration::from_secs(self.cooldown_secs));
            }
            cooled
        };
        if cooled {
            tracing::warn!(
                "[translation] provider {} failed repeatedly: {detail} — cooling for {}s",
                self.provider_id,
                self.cooldown_secs,
            );
            notify_change();
        }
    }

    /// A slow-inflight signal: the request has been on the wire past the soft
    /// in-flight deadline while the reader stares at an untranslated block.
    /// Nothing failed — the reply may still land and count normally — but the
    /// lane's concurrency slots are being held hostage, so the rate halves
    /// right now instead of after the full round trip. Always observable (the
    /// rate moved), so always notifies.
    pub fn report_slow_inflight(&self) {
        {
            let mut runtime = self
                .runtime
                .lock()
                .expect("pool runtime lock is never poisoned across a panic-free run");
            let entry = runtime.entry(self.provider_id.clone()).or_default();
            if let Some(aimd) = entry.aimd.as_mut() {
                aimd.penalize(None, Instant::now());
            }
        }
        notify_change();
    }

    /// Space this provider's consecutive dispatches by its adaptive interval.
    /// Atomic slot claiming, one timeline per provider: two callers cannot
    /// pick the same slot and fire together. The interval is read under the
    /// lock so a penalty lands mid-wait for every later claimant.
    pub async fn wait_for_dispatch_slot(&self) {
        let slot_us = {
            let mut runtime = self
                .runtime
                .lock()
                .expect("pool runtime lock is never poisoned across a panic-free run");
            let entry = runtime.entry(self.provider_id.clone()).or_default();
            let interval = entry
                .aimd
                .as_ref()
                .map(AimdController::dispatch_interval)
                .unwrap_or_default();
            let now_us = self.origin.elapsed().as_micros() as u64;
            let slot = (entry.last_dispatch_us + interval.as_micros() as u64).max(now_us);
            entry.last_dispatch_us = slot;
            slot
        };
        let now_us = self.origin.elapsed().as_micros() as u64;
        if slot_us > now_us {
            tokio::time::sleep(Duration::from_micros(slot_us - now_us)).await;
        }
    }
}

/// The registry of live pools, keyed by the enabled providers' identity list.
/// A settings change that alters the pool builds a new entry; stale entries
/// cost a handful of bytes and never grow unbounded (a session edits settings
/// a bounded number of times).
static POOLS: OnceLock<Mutex<HashMap<String, Arc<PoolState>>>> = OnceLock::new();

fn pools() -> &'static Mutex<HashMap<String, Arc<PoolState>>> {
    POOLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The pool key: which providers are in, in what order. Membership and order
/// both reset the rotation (a reordered list should not inherit a cursor that
/// means something else now).
fn pool_key(providers: &[ProviderConfig]) -> String {
    providers
        .iter()
        .map(|provider| provider.provider_id())
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

/// Get (or build) the pool for this provider list.
fn pool_for(providers: Vec<ProviderConfig>) -> Arc<PoolState> {
    let key = pool_key(&providers);
    let mut pools = pools().lock().expect("pool registry lock");
    pools
        .entry(key)
        .or_insert_with(|| {
            Arc::new(PoolState {
                runtime: Arc::new(Mutex::new(
                    providers
                        .iter()
                        .map(|provider| (provider.id.clone(), ProviderRuntime::seeded(provider)))
                        .collect(),
                )),
                providers,
                cursor: AtomicUsize::new(0),
                origin: Instant::now(),
                _last_dispatch: AtomicU64::new(0),
            })
        })
        .clone()
}

/// The visible state of one pool member, for the settings page's badges.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub id: String,
    pub name: Option<String>,
    pub base_url: String,
    pub model: String,
    /// The adaptive limiter's current allowance. 0 before the first request.
    pub allowed_rpm: f64,
    /// Milliseconds until the member becomes dispatchable again: the longer
    /// of a `Retry-After` park and a failure-threshold cooldown; 0 when
    /// dispatchable.
    pub cooldown_remaining_ms: u64,
    /// Consecutive failed requests (429s and hard errors alike) since the
    /// last success or reset. At the settings' failure threshold the member
    /// enters a cooldown and the streak resets.
    pub consecutive_failures: u32,
    /// Set when the provider was retired for the session, with the reason.
    pub disabled_reason: Option<String>,
    /// POSTs actually dispatched in the current wall-clock minute. The
    /// adaptive rate is what the limiter ALLOWS; this is what the endpoint is
    /// really being asked to serve, which is what "rate is high but nothing
    /// translates" reports turn on.
    pub dispatched_last_minute: u64,
    /// Milliseconds since this member last claimed a dispatch slot; `None`
    /// when it has never dispatched.
    pub last_dispatch_ago_ms: Option<u64>,
    /// The member's current health, when its window has anything in it.
    pub health: Option<ProviderHealthStatus>,
}

/// The health-score view one pool member exposes to the settings page: the
/// composite and the three sub-scores behind it, plus the dispatch verdict.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealthStatus {
    /// 0-100 composite.
    pub score: f64,
    /// 0-1 dimension sub-scores (quality / stability / speed).
    pub quality: f64,
    pub stability: f64,
    pub speed: f64,
    /// Events the score judged; below the minimum sample the score is the
    /// neutral observing value.
    pub sample: usize,
    pub observing: bool,
    /// True when the member is degraded below [`health::DEGRADE_THRESHOLD`]
    /// and only serves fallback (or probe) traffic.
    pub degraded: bool,
}

/// A member's current health, read from the metrics event window. Unknown or
/// unsampled members come back "observing" — neutral in dispatch, immune to
/// the breaker.
fn health_of(provider_id: &str) -> HealthScore {
    let metrics = crate::translation::metrics::translation_metrics();
    let now = SystemTime::now();
    health::health_score(&metrics.recent_events(provider_id, now), now)
}

/// Two scores this close name the same tier — the rotation, not the
/// sub-point differences, decides between such members.
const HEALTH_TIE_EPSILON: f64 = 0.5;

impl PoolState {
    /// Pick the next dispatchable provider, skipping cooldowns, session
    /// disables, and incomplete entries.
    pub async fn pick(&self) -> Result<PickedProvider, AppCommandError> {
        // No settings shape in hand here: the built-in defaults keep direct
        // `pick` callers (and tests) on the same thresholds the settings
        // accessors would produce for an untouched configuration.
        self.pick_excluding(
            &[],
            FAILURE_THRESHOLD_DEFAULT,
            u64::from(COOLDOWN_SECONDS_DEFAULT),
        )
        .await
    }

    /// [`Self::pick`] with per-request exclusions: `exclude` names provider
    /// ids that must not serve this one dispatch. Exclusion is a property of
    /// the request, not the pool — the registry keys runtime state by
    /// membership only, so an excluded member keeps its adaptive history
    /// untouched. The failure-threshold cooldown's length comes from the
    /// settings accessors, read per request for the same reason.
    ///
    /// Gates run before the choice, in escalation order:
    ///
    /// 1. **Health retirement** — a member whose health score has sunk below
    ///    [`health::RETIRE_THRESHOLD`] with a real sample leaves the rotation
    ///    for the session, the quality-side twin of the failure-threshold
    ///    cooldown. Never applied when it would empty the pool: one endpoint,
    ///    however bad, beats none.
    /// 2. **Failure cooldown** — a member whose consecutive-failure streak
    ///    reached the settings' threshold sits out a cooldown of the
    ///    configured length; while it lasts it is not a candidate at all.
    /// 3. **Fallback partition** — members below [`health::DEGRADE_THRESHOLD`]
    ///    stop receiving normal traffic; the batch goes to whoever is still
    ///    healthy. If nobody is healthy the degraded members serve anyway
    ///    (a weak endpoint beats no endpoint).
    /// 4. **Probe** — a degraded member otherwise starves (no traffic, no
    ///    fresh events, no way to recover), so once per
    ///    [`health::PROBE_INTERVAL`] it receives one dispatch to prove it
    ///    healed.
    ///
    /// Among the survivors the highest health wins, and exact ties rotate
    /// strictly — the old idle-time ranking burst A,A,B,B under a two-member
    /// pool, hammering one endpoint twice before the other saw work. (Speed
    /// needs no separate tiebreak: it already weighs inside the composite,
    /// so a slow member's composite sinks below its faster rival on its
    /// own — what was missing was the slow signal arriving in time, which
    /// the SlowInflight event fixes upstream.)
    ///
    /// Every member cooling at once (either kind): wait for the earliest
    /// cooldown to lapse (capped), then pick again — failing immediately
    /// would surface a 429 the rotation could have absorbed by breathing for
    /// a few seconds.
    async fn pick_excluding(
        &self,
        exclude: &[String],
        failure_threshold: u32,
        cooldown_secs: u64,
    ) -> Result<PickedProvider, AppCommandError> {
        loop {
            let now = Instant::now();
            let candidates: Vec<String> = {
                let runtime = self.runtime.lock().expect("pool runtime lock");
                self.providers
                    .iter()
                    .filter(|provider| provider.is_complete())
                    .filter(|provider| {
                        runtime
                            .get(&provider.id)
                            .and_then(|entry| entry.disabled_reason.as_deref())
                            .is_none()
                    })
                    // Per-request exclusions come last: a fallback caller's
                    // already-failed members leave the candidate set without
                    // touching the pool's own state.
                    .filter(|provider| !exclude.iter().any(|excluded| excluded == &provider.id))
                    .map(|provider| provider.id.clone())
                    .collect()
            };
            if candidates.is_empty() {
                return Err(if self.providers.iter().any(|p| p.is_complete()) {
                    AppCommandError::network(
                        "All translation endpoints are disabled for this session — check the provider settings",
                    )
                } else {
                    AppCommandError::configuration_missing(
                        "No enabled translation provider is fully configured",
                    )
                });
            }

            // Health retirement, with the single-point guard: a pool of one
            // member is never thinned further.
            let complete_count = self
                .providers
                .iter()
                .filter(|provider| provider.is_complete())
                .count();
            let mut survivors: Vec<(String, HealthScore)> = Vec::with_capacity(candidates.len());
            let mut failure_cooling: Vec<Duration> = Vec::new();
            for id in candidates {
                let health = health_of(&id);
                if health.retired() && complete_count > 1 {
                    let newly_disabled = {
                        let mut runtime = self.runtime.lock().expect("pool runtime lock");
                        let entry = runtime.entry(id.clone()).or_default();
                        let newly = entry.disabled_reason.is_none();
                        if newly {
                            entry.disabled_reason = Some(format!(
                                "health score {:.0} — failing (quality or stability) repeatedly; disabled for this session",
                                health.score
                            ));
                        }
                        newly
                    };
                    if newly_disabled {
                        tracing::warn!(
                            "[translation] provider {id} retired by health score {:.0}",
                            health.score
                        );
                        notify_change();
                    }
                    continue;
                }
                // Failure-threshold cooldown: the member sits this one out,
                // and its remaining window feeds the all-cooling wait below.
                // A lapsed window clears on read, like AIMD's.
                let remaining = {
                    let mut runtime = self.runtime.lock().expect("pool runtime lock");
                    match runtime.get_mut(&id) {
                        Some(entry) => match entry.cooldown_until {
                            Some(until) => match until.checked_duration_since(now) {
                                Some(remaining) => Some(remaining),
                                None => {
                                    entry.cooldown_until = None;
                                    None
                                }
                            },
                            None => None,
                        },
                        None => None,
                    }
                };
                if let Some(remaining) = remaining {
                    failure_cooling.push(remaining);
                    continue;
                }
                survivors.push((id, health));
            }
            if survivors.is_empty() {
                // Every candidate is in a failure cooldown: wait out the
                // earliest window (capped) and re-pick, mirroring the
                // Retry-After path — failing immediately would surface an
                // error the rotation could have absorbed by breathing for a
                // few seconds.
                if let Some(earliest) = failure_cooling
                    .iter()
                    .min()
                    .copied()
                    .map(|remaining| remaining.min(MAX_WAIT_ALL_COOLING))
                {
                    tracing::debug!(
                        "[translation] every provider is in a failure cooldown; waiting {earliest:?}"
                    );
                    tokio::time::sleep(earliest).await;
                }
                continue;
            }

            // Every surviving member cooling at once: wait for the earliest
            // cooldown to lapse (capped), then pick again — failing
            // immediately would surface a 429 the rotation could have
            // absorbed by breathing for a few seconds.
            let cooling: Vec<Duration> = {
                let mut runtime = self.runtime.lock().expect("pool runtime lock");
                survivors
                    .iter()
                    .filter_map(|(id, _)| {
                        let aimd = runtime.get_mut(id)?.aimd.as_mut()?;
                        aimd.cooldown_remaining(now)
                    })
                    .collect()
            };
            if cooling.len() == survivors.len() {
                let earliest = cooling
                    .iter()
                    .min()
                    .copied()
                    .unwrap_or(MAX_WAIT_ALL_COOLING)
                    .min(MAX_WAIT_ALL_COOLING);
                tracing::debug!(
                    "[translation] every provider is cooling down; waiting {earliest:?}"
                );
                tokio::time::sleep(earliest).await;
                continue;
            }

            // Fallback partition, then the probe so a degraded member keeps
            // one path back into rotation.
            let mut probe_target: Option<String> = None;
            let degraded: Vec<(String, HealthScore)> = survivors
                .iter()
                .filter(|(_, health)| health.degraded())
                .cloned()
                .collect();
            let primary: Vec<(String, HealthScore)> = survivors
                .iter()
                .filter(|(_, health)| !health.degraded())
                .cloned()
                .collect();
            let pool_to_serve: Vec<(String, HealthScore)> = if primary.is_empty() {
                // Nobody healthy: the degraded members serve anyway.
                degraded
            } else {
                // Someone healthy: a degraded member's only traffic is its
                // scheduled probe.
                let now_instant = Instant::now();
                for (id, _) in &degraded {
                    let due = {
                        let mut runtime = self.runtime.lock().expect("pool runtime lock");
                        let entry = runtime.entry(id.clone()).or_default();
                        let due = entry
                            .last_probe_at
                            .map(|last| now_instant.duration_since(last) >= health::PROBE_INTERVAL)
                            .unwrap_or(true);
                        if due {
                            entry.last_probe_at = Some(now_instant);
                        }
                        due
                    };
                    if due {
                        probe_target = Some(id.clone());
                        break;
                    }
                }
                if let Some(id) = &probe_target {
                    vec![(id.clone(), health_of(id))]
                } else {
                    primary
                }
            };

            // Health-first, strict rotation among ties.
            let best = pool_to_serve
                .iter()
                .map(|(_, health)| health.score)
                .fold(f64::NEG_INFINITY, f64::max);
            let tied: Vec<&str> = pool_to_serve
                .iter()
                .filter(|(_, health)| (health.score - best).abs() <= HEALTH_TIE_EPSILON)
                .map(|(id, _)| id.as_str())
                .collect();
            let cursor = self.cursor.fetch_add(1, Ordering::SeqCst);
            let picked_id = tied[cursor % tied.len()].to_string();

            let provider = self
                .providers
                .iter()
                .find(|provider| provider.id == picked_id)
                .cloned()
                .expect("the candidate ids come from the same list");
            return Ok(PickedProvider {
                provider,
                runtime: Arc::clone(&self.runtime),
                origin: self.origin,
                provider_id: picked_id,
                failure_threshold,
                cooldown_secs,
            });
        }
    }

    /// The status snapshot the settings page renders.
    pub fn status(&self) -> Vec<ProviderStatus> {
        let now = Instant::now();
        let mut runtime = self.runtime.lock().expect("pool runtime lock");
        self.providers
            .iter()
            .map(|provider| {
                let entry = runtime.get_mut(&provider.id);
                let (rpm, cooldown, reason, failures, dispatch_ago) = match entry {
                    Some(entry) => {
                        let aimd_cooldown = entry
                            .aimd
                            .as_mut()
                            .and_then(|aimd| aimd.cooldown_remaining(now))
                            .map(|remaining| remaining.as_millis() as u64)
                            .unwrap_or(0);
                        // The failure cooldown counts too: the badge shows
                        // whichever window keeps the member parked longer.
                        // A lapsed window clears on read, like AIMD's.
                        let failure_cooldown = match entry.cooldown_until {
                            Some(until) => match until.checked_duration_since(now) {
                                Some(remaining) => remaining.as_millis() as u64,
                                None => {
                                    entry.cooldown_until = None;
                                    0
                                }
                            },
                            None => 0,
                        };
                        // Micros since the pool's origin minus the claimed
                        // slot; a slot still in the future (a pacing wait in
                        // flight) reads as "just dispatched".
                        let dispatch_ago = (entry.last_dispatch_us != 0).then(|| {
                            (self.origin.elapsed().as_micros() as u64)
                                .saturating_sub(entry.last_dispatch_us)
                                / 1000
                        });
                        (
                            entry.aimd.as_ref().map(AimdController::allowed_rpm),
                            aimd_cooldown.max(failure_cooldown),
                            entry.disabled_reason.clone(),
                            entry.consecutive_failures,
                            dispatch_ago,
                        )
                    }
                    None => (None, 0, None, 0, None),
                };
                let health = health_of(&provider.id);
                let health = (!health.observing).then(|| ProviderHealthStatus {
                    score: health.score,
                    quality: health.quality,
                    stability: health.stability,
                    speed: health.speed,
                    sample: health.sample,
                    observing: health.observing,
                    degraded: health.degraded(),
                });
                ProviderStatus {
                    id: provider.id.clone(),
                    name: provider.name.clone(),
                    base_url: provider.base_url.clone(),
                    model: provider.model.clone(),
                    allowed_rpm: rpm.unwrap_or(0.0),
                    cooldown_remaining_ms: cooldown,
                    consecutive_failures: failures,
                    disabled_reason: reason,
                    dispatched_last_minute: crate::translation::metrics::translation_metrics()
                        .dispatched_last_minute(&provider.id),
                    last_dispatch_ago_ms: dispatch_ago,
                    health,
                }
            })
            .collect()
    }
}

/// The settings page's view of the pool currently in force.
pub fn pool_status(
    settings: &crate::translation::settings::TranslationSettings,
) -> Vec<ProviderStatus> {
    let providers = settings.active_providers();
    if providers.is_empty() {
        return Vec::new();
    }
    pool_for(providers).status()
}

/// The manual "this endpoint is fixed" action behind the settings page's
/// reset: every live pool that contains `provider_id` replaces the member's
/// runtime entry with a fresh seed from its own configuration — the session
/// disable, the failure streak and its cooldown, and any AIMD penalty/
/// cooldown are dropped and the limiter re-seeds from the configured
/// ceiling. The metrics side (counters and minute series) is reset
/// separately by the command layer. Notifies the frontends only when some
/// pool actually changed.
pub fn reset_provider(provider_id: &str) {
    let mut changed = false;
    {
        let pools = pools().lock().expect("pool registry lock");
        for pool in pools.values() {
            let config = pool.providers.iter().find(|p| p.id == provider_id);
            let mut runtime = pool.runtime.lock().expect("pool runtime lock");
            if !runtime.contains_key(provider_id) {
                continue;
            }
            match config {
                Some(provider) => {
                    runtime.insert(provider_id.to_string(), ProviderRuntime::seeded(provider));
                }
                // The member's configuration is gone (settings changed since);
                // a runtime entry without a config row is dead weight.
                None => {
                    runtime.remove(provider_id);
                }
            }
            changed = true;
        }
    }
    if changed {
        notify_change();
    }
}

/// The manual "keep this provider out of rotation" action: session-disable
/// every live pool's entry under the given reason, shown verbatim in the
/// settings page. Runtime-only. Returns whether anything actually changed —
/// an unknown id, or an entry already carrying the same reason, changes
/// nothing and notifies no one.
pub fn disable_provider(provider_id: &str, reason: &str) -> bool {
    let mut changed = false;
    {
        let pools = pools().lock().expect("pool registry lock");
        for pool in pools.values() {
            let mut runtime = pool.runtime.lock().expect("pool runtime lock");
            let Some(entry) = runtime.get_mut(provider_id) else {
                continue;
            };
            if entry.disabled_reason.as_deref() != Some(reason) {
                entry.disabled_reason = Some(reason.to_string());
                changed = true;
            }
        }
    }
    if changed {
        notify_change();
    }
    changed
}

/// The manual "sit this one out" action: park every live pool's entry for
/// `seconds` without ending its session, clearing the failure streak so the
/// re-entry starts clean. Runtime-only. Returns whether anything actually
/// changed.
pub fn cooldown_provider(provider_id: &str, seconds: u64) -> bool {
    let mut changed = false;
    {
        let pools = pools().lock().expect("pool registry lock");
        for pool in pools.values() {
            let mut runtime = pool.runtime.lock().expect("pool runtime lock");
            let Some(entry) = runtime.get_mut(provider_id) else {
                continue;
            };
            let until = Instant::now() + Duration::from_secs(seconds);
            if entry.cooldown_until != Some(until) || entry.consecutive_failures != 0 {
                entry.cooldown_until = Some(until);
                entry.consecutive_failures = 0;
                changed = true;
            }
        }
    }
    if changed {
        notify_change();
    }
    changed
}

/// Pick a provider for one outbound request from the given settings' pool.
pub async fn pick_provider(
    settings: &crate::translation::settings::TranslationSettings,
) -> Result<PickedProvider, AppCommandError> {
    pick_provider_excluding(settings, &[]).await
}

/// [`pick_provider`] with per-request exclusions: `exclude` names provider
/// ids that must not serve this dispatch (a fallback retry's already-failed
/// members). Excluded ids are dropped after the completeness and disable
/// filters; if nothing remains, the existing pool-exhaustion error path
/// surfaces. Exclusion is a property of the request, not the pool — the
/// registry keys runtime state by membership only, so excluded members keep
/// their adaptive history untouched.
pub async fn pick_provider_excluding(
    settings: &crate::translation::settings::TranslationSettings,
    exclude: &[String],
) -> Result<PickedProvider, AppCommandError> {
    let providers = settings.active_providers();
    if providers.is_empty() {
        return Err(AppCommandError::configuration_missing(
            "No enabled translation provider is fully configured",
        ));
    }
    pool_for(providers)
        .pick_excluding(
            exclude,
            settings.failure_threshold(),
            settings.cooldown_seconds(),
        )
        .await
}

/// A provider wrapped for a direct dispatch, bypassing rotation: the settings
/// page tests exactly the row it is editing, even while other members cool
/// down. It shares the live pool's runtime map (the registry keeps one for
/// this membership) so AIMD learning from a settings-page test carries into
/// real traffic.
pub fn standalone(provider: ProviderConfig) -> PickedProvider {
    let pool = pool_for(vec![provider.clone()]);
    PickedProvider {
        provider,
        runtime: Arc::clone(&pool.runtime),
        origin: pool.origin,
        provider_id: pool
            .providers
            .first()
            .map(|first| first.id.clone())
            .unwrap_or_default(),
        // The settings-page test path carries no settings shape; the built-in
        // defaults keep its reports behaving like rotation's.
        failure_threshold: FAILURE_THRESHOLD_DEFAULT,
        cooldown_secs: u64::from(COOLDOWN_SECONDS_DEFAULT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, base: &str) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            base_url: format!("https://{base}/v1"),
            api_key: "sk-test".to_string(),
            model: "m".to_string(),
            enabled: true,
            ..Default::default()
        }
    }

    /// Two clean rounds of a two-member pool visit both, alternating.
    #[tokio::test]
    async fn rotation_alternates_between_members() {
        let pool = PoolState {
            runtime: Arc::new(Mutex::new(HashMap::new())),
            providers: vec![
                provider("a", "a.example.com"),
                provider("b", "b.example.com"),
            ],
            cursor: AtomicUsize::new(0),
            origin: Instant::now(),
            _last_dispatch: AtomicU64::new(0),
        };
        let first = pool.pick().await.expect("pick");
        let second = pool.pick().await.expect("pick");
        assert_ne!(first.id(), second.id(), "two members must alternate");
        let third = pool.pick().await.expect("pick");
        assert_eq!(first.id(), third.id(), "the wheel comes around");
    }

    /// A provider whose failure streak reaches the threshold sits out a
    /// cooldown: the other member serves while it cools. Reports below the
    /// threshold only count — the member stays dispatchable, and no session
    /// disable is recorded anymore.
    #[tokio::test]
    async fn a_cooled_provider_is_skipped_while_the_other_serves() {
        let pool = PoolState {
            runtime: Arc::new(Mutex::new(HashMap::new())),
            providers: vec![
                provider("a", "a.example.com"),
                provider("b", "b.example.com"),
            ],
            cursor: AtomicUsize::new(0),
            origin: Instant::now(),
            _last_dispatch: AtomicU64::new(0),
        };
        let mut first = pool.pick().await.expect("pick");
        first.failure_threshold = 2;
        first.report_failure("HTTP 401");
        {
            let runtime = pool.runtime.lock().unwrap();
            let entry = runtime.get(first.id()).expect("entry exists");
            assert_eq!(
                entry.consecutive_failures, 1,
                "below threshold: counted, not parked"
            );
            assert!(entry.cooldown_until.is_none(), "no window yet");
        }
        first.report_failure("HTTP 401");
        {
            let runtime = pool.runtime.lock().unwrap();
            let entry = runtime.get(first.id()).expect("entry exists");
            assert_eq!(
                entry.consecutive_failures, 0,
                "the streak resets when the cooldown lands"
            );
            assert!(entry.cooldown_until.is_some(), "the cooldown landed");
            assert!(
                entry.disabled_reason.is_none(),
                "a failure cooldown is bounded, not a session exile"
            );
        }
        for _ in 0..2 {
            let picked = pool.pick().await.expect("the other member serves");
            assert_ne!(picked.id(), first.id(), "the cooling member is skipped");
        }
    }

    /// The manual reset returns a cooling member to service. A single-member
    /// pool makes the before/after unambiguous: `reset_provider` re-seeds the
    /// runtime entry, dropping the failure cooldown and streak together with
    /// the session disable and any AIMD penalties.
    #[tokio::test]
    async fn reset_provider_clears_the_failure_streak_and_cooldown() {
        let _guard = NOTIFIER_TEST_LOCK.lock().await;
        reset_notifiers_for_test();

        let providers = vec![provider("reset-a", "reset-a.example.com")];
        let pool = pool_for(providers);
        let id = "reset-a".to_string();

        let mut picked = pool.pick().await.expect("pick");
        picked.failure_threshold = 1;
        picked.report_failure("HTTP 500");
        {
            let runtime = pool.runtime.lock().unwrap();
            let entry = runtime.get(&id).expect("the entry exists");
            assert!(entry.cooldown_until.is_some(), "the cooldown landed");
            assert!(entry.disabled_reason.is_none());
        }

        reset_provider(&id);

        {
            let runtime = pool.runtime.lock().unwrap();
            let entry = runtime
                .get(&id)
                .expect("the entry is re-seeded, not dropped");
            assert!(entry.disabled_reason.is_none(), "the disable is cleared");
            assert!(entry.cooldown_until.is_none(), "the cooldown is cleared");
            assert_eq!(
                entry.consecutive_failures, 0,
                "the failure streak is cleared"
            );
            assert!(entry.aimd.is_some(), "the limiter is re-seeded");
        }
        let again = pool.pick().await.expect("the reset member serves again");
        assert_eq!(again.id(), id);

        reset_notifiers_for_test();
    }

    /// A `Retry-After` park defers a member's next pick: with one of two
    /// cooling, the other serves; with both cooling, `pick` waits past the
    /// window instead of erroring.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_cooling_provider_defers_to_the_others() {
        let pool = PoolState {
            runtime: Arc::new(Mutex::new(
                vec![
                    provider("a", "a.example.com"),
                    provider("b", "b.example.com"),
                ]
                .into_iter()
                .map(|p| (p.id.clone(), ProviderRuntime::seeded(&p)))
                .collect(),
            )),
            providers: vec![
                provider("a", "a.example.com"),
                provider("b", "b.example.com"),
            ],
            cursor: AtomicUsize::new(0),
            origin: Instant::now(),
            _last_dispatch: AtomicU64::new(0),
        };
        let first = pool.pick().await.expect("pick");
        first.report_rate_limited(Some(Duration::from_secs(60)));
        // Advance the paused clock past nothing — b is dispatchable, a is not.
        let second = pool.pick().await.expect("pick");
        let other = if first.id() == "a" { "b" } else { "a" };
        assert_eq!(second.id(), other);
        // Now park b too; the pick must WAIT (auto-advance the paused clock)
        // and eventually return a member rather than an error.
        second.report_rate_limited(Some(Duration::from_secs(2)));
        let third = pool.pick().await.expect("waits out the earliest window");
        assert!(third.id() == "a" || third.id() == "b");
    }

    /// The status snapshot reflects the limiter's learned rate and a live
    /// cooldown.
    #[tokio::test]
    async fn status_reports_the_learned_rate_and_cooldowns() {
        let providers = vec![provider("a", "a.example.com")];
        let pool = PoolState {
            runtime: Arc::new(Mutex::new(
                providers
                    .iter()
                    .map(|p| (p.id.clone(), ProviderRuntime::seeded(p)))
                    .collect(),
            )),
            providers,
            cursor: AtomicUsize::new(0),
            origin: Instant::now(),
            _last_dispatch: AtomicU64::new(0),
        };
        let picked = pool.pick().await.expect("pick");
        let baseline = pool.status()[0].allowed_rpm;
        picked.report_success();
        assert!(
            pool.status()[0].allowed_rpm > 0.0,
            "a member shows its rate"
        );
        assert_eq!(pool.status()[0].cooldown_remaining_ms, 0);

        picked.report_rate_limited(Some(Duration::from_secs(10)));
        let status = pool.status();
        assert!(
            status[0].cooldown_remaining_ms > 0,
            "a parked member shows its remaining window"
        );
        assert!(
            status[0].allowed_rpm < baseline,
            "a 429 halves the shown rate"
        );
    }

    /// A slow-inflight report throttles without parking: the rate halves so
    /// later chunks rotate elsewhere while the slow request is still on the
    /// wire, but the provider stays dispatchable — the reply may still be
    /// fine, and the pacing is the throttle.
    #[tokio::test]
    async fn a_slow_inflight_report_halves_the_rate_without_parking() {
        let providers = vec![provider("slow", "slow.example.com")];
        let pool = PoolState {
            runtime: Arc::new(Mutex::new(
                providers
                    .iter()
                    .map(|p| (p.id.clone(), ProviderRuntime::seeded(p)))
                    .collect(),
            )),
            providers,
            cursor: AtomicUsize::new(0),
            origin: Instant::now(),
            _last_dispatch: AtomicU64::new(0),
        };
        let picked = pool.pick().await.expect("pick");
        let baseline = pool.status()[0].allowed_rpm;
        picked.report_slow_inflight();
        let status = pool.status();
        assert!(
            (status[0].allowed_rpm - (baseline / 2.0)).abs() < f64::EPSILON,
            "the rate halved: {} -> {}",
            baseline,
            status[0].allowed_rpm
        );
        assert_eq!(
            status[0].cooldown_remaining_ms, 0,
            "no Retry-After was named; the provider stays dispatchable"
        );
    }

    /// Change notifications fire on observable mutations only: a `Retry-After`
    /// penalty always, a failure cooldown when it lands, and a success only on
    /// the every-tenth one that moves the rate. The registry is process-global,
    /// so the reset keeps other tests' reports from leaking in before ours.
    #[tokio::test]
    async fn change_notifications_fire_on_observable_mutations() {
        let _guard = NOTIFIER_TEST_LOCK.lock().await;
        reset_notifiers_for_test();
        let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&fired);
        on_change(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        let providers = vec![provider("a", "a.example.com")];
        let pool = PoolState {
            runtime: Arc::new(Mutex::new(
                providers
                    .iter()
                    .map(|p| (p.id.clone(), ProviderRuntime::seeded(p)))
                    .collect(),
            )),
            providers,
            cursor: AtomicUsize::new(0),
            origin: Instant::now(),
            _last_dispatch: AtomicU64::new(0),
        };
        let mut picked = pool.pick().await.expect("pick");

        // Four quiet successes change nothing observable.
        for _ in 0..4 {
            picked.report_success();
        }
        assert_eq!(
            fired.load(Ordering::SeqCst),
            0,
            "rate didn't move; no event"
        );
        // The fifth steps the rate up.
        picked.report_success();
        assert!(fired.load(Ordering::SeqCst) >= 1, "climb must notify");

        // A 429 verdict always notifies.
        picked.report_rate_limited(Some(Duration::from_secs(5)));
        assert!(fired.load(Ordering::SeqCst) >= 2);

        // A clean reply clears the failure streak the 429 opened.
        picked.report_success();

        // A failure below the threshold is quiet.
        picked.failure_threshold = 2;
        picked.report_failure("HTTP 401");
        let after_first = fired.load(Ordering::SeqCst);
        // Reaching the threshold lands the cooldown — that notifies.
        picked.report_failure("HTTP 401");
        assert!(
            fired.load(Ordering::SeqCst) > after_first,
            "the cooldown landing must notify"
        );

        reset_notifiers_for_test();
    }

    /// A seeded pool over the given member ids (distinct hosts, so direct
    /// `PoolState`s never share anything anyway).
    fn pool_with(ids: &[&str]) -> PoolState {
        let providers: Vec<ProviderConfig> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| provider(id, &format!("q{index}.example.com")))
            .collect();
        PoolState {
            runtime: Arc::new(Mutex::new(
                providers
                    .iter()
                    .map(|p| (p.id.clone(), ProviderRuntime::seeded(p)))
                    .collect(),
            )),
            providers,
            cursor: AtomicUsize::new(0),
            origin: Instant::now(),
            _last_dispatch: AtomicU64::new(0),
        }
    }

    /// The failure cooldown is bounded: `pick` waits it out and the member
    /// rejoins on its own — no manual reset required. (The paused clock makes
    /// each wait instantaneous; the window itself runs on the real clock,
    /// like the Retry-After test above.)
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_failure_cooldown_expires_and_the_member_rejoins() {
        let pool = pool_with(&["rejoin"]);
        let mut picked = pool.pick().await.expect("pick");
        picked.failure_threshold = 1;
        picked.cooldown_secs = 1;
        picked.report_failure("HTTP 500");

        // The only member is cooling, so pick waits out the window and
        // serves it again.
        let again = pool.pick().await.expect("rejoins after the cooldown");
        assert_eq!(again.id(), "rejoin");
        {
            let runtime = pool.runtime.lock().unwrap();
            let entry = &runtime["rejoin"];
            assert_eq!(entry.consecutive_failures, 0, "the streak stayed reset");
            assert!(
                entry.cooldown_until.is_none(),
                "a lapsed window clears on read"
            );
        }
    }

    /// A 429 counts toward the failure threshold like any hard error, but
    /// the failure cooldown itself still lands only through the failure
    /// report: a 429 already carries its own Retry-After park.
    #[tokio::test]
    async fn a_429_counts_toward_the_failure_threshold() {
        let pool = pool_with(&["rl"]);
        let mut picked = pool.pick().await.expect("pick");
        picked.failure_threshold = 2;

        picked.report_rate_limited(None);
        {
            let runtime = pool.runtime.lock().unwrap();
            assert_eq!(runtime["rl"].consecutive_failures, 1, "the 429 counts");
        }
        picked.report_rate_limited(None);
        {
            let runtime = pool.runtime.lock().unwrap();
            let entry = &runtime["rl"];
            assert_eq!(entry.consecutive_failures, 2, "past the threshold");
            assert!(
                entry.cooldown_until.is_none(),
                "a 429 parks via Retry-After only; it does not land the failure cooldown"
            );
        }

        // The next failure report of any kind lands it.
        picked.report_failure("HTTP 500");
        {
            let runtime = pool.runtime.lock().unwrap();
            let entry = &runtime["rl"];
            assert!(entry.cooldown_until.is_some(), "the cooldown landed");
            assert_eq!(entry.consecutive_failures, 0);
        }
    }

    /// A clean response clears the streak: alternating failure/success runs
    /// never accumulate into a cooldown.
    #[tokio::test]
    async fn a_success_resets_the_failure_streak() {
        let pool = pool_with(&["okr"]);
        let mut picked = pool.pick().await.expect("pick");
        picked.failure_threshold = 3;
        for round in 0..3 {
            picked.report_failure("HTTP 500");
            picked.report_failure("HTTP 500");
            picked.report_success();
            let runtime = pool.runtime.lock().unwrap();
            let entry = &runtime["okr"];
            assert_eq!(
                entry.consecutive_failures, 0,
                "round {round}: success wipes the streak"
            );
            assert!(entry.cooldown_until.is_none());
        }
    }

    /// The manual ops behave as named: disable parks the member for the
    /// session under the given reason, cooldown parks it for a bounded
    /// window and clears the streak, and both report whether they changed
    /// anything (an unknown id, or an already-identical disable, changes
    /// nothing). `reset_provider` is the undo for both.
    #[tokio::test]
    async fn manual_disable_and_cooldown_ops_report_and_revert() {
        let _guard = NOTIFIER_TEST_LOCK.lock().await;
        reset_notifiers_for_test();

        let providers = vec![
            provider("man-a", "man-a.example.com"),
            provider("man-b", "man-b.example.com"),
        ];
        let pool = pool_for(providers);

        // Unknown ids change nothing.
        assert!(!disable_provider("nope", "reason"));
        assert!(!cooldown_provider("nope", 10));

        // Disable: the member leaves the rotation, the other serves.
        assert!(disable_provider("man-a", "test disable"));
        {
            let runtime = pool.runtime.lock().unwrap();
            assert_eq!(
                runtime["man-a"].disabled_reason.as_deref(),
                Some("test disable")
            );
        }
        for _ in 0..2 {
            let picked = pool.pick().await.expect("pick");
            assert_eq!(picked.id(), "man-b", "the disabled member is skipped");
        }
        // Re-disabling with the same reason is a no-op.
        assert!(!disable_provider("man-a", "test disable"));

        // Cooldown: sets the window and clears the streak, without ending
        // the session.
        {
            let mut runtime = pool.runtime.lock().unwrap();
            runtime.get_mut("man-b").unwrap().consecutive_failures = 2;
        }
        assert!(cooldown_provider("man-b", 120));
        {
            let runtime = pool.runtime.lock().unwrap();
            let entry = &runtime["man-b"];
            assert!(entry.cooldown_until.is_some(), "the window is set");
            assert_eq!(entry.consecutive_failures, 0, "the streak clears");
            assert!(entry.disabled_reason.is_none(), "no session disable");
        }
        let status = pool.status();
        let b = status.iter().find(|s| s.id == "man-b").expect("status row");
        assert!(b.cooldown_remaining_ms > 0, "the badge shows the window");

        // reset_provider is the undo for both.
        reset_provider("man-a");
        reset_provider("man-b");
        {
            let runtime = pool.runtime.lock().unwrap();
            for id in ["man-a", "man-b"] {
                let entry = &runtime[id];
                assert!(entry.disabled_reason.is_none());
                assert!(entry.cooldown_until.is_none());
            }
        }
        let both = pool.pick().await.expect("both serve again");
        assert!(both.id() == "man-a" || both.id() == "man-b");

        reset_notifiers_for_test();
    }

    /// Per-request exclusion drops members from the candidate set after the
    /// completeness and disable filters: the survivors rotate among
    /// themselves, and excluding everyone surfaces the existing
    /// pool-exhaustion error.
    #[tokio::test]
    async fn pick_provider_excluding_skips_the_named_members() {
        let settings = crate::translation::settings::TranslationSettings {
            providers: vec![
                provider("ex-a", "ex-a.example.com"),
                provider("ex-b", "ex-b.example.com"),
            ],
            enabled: true,
            ..Default::default()
        };

        let one = pick_provider_excluding(&settings, &["ex-a".to_string()])
            .await
            .expect("pick");
        assert_eq!(one.id(), "ex-b", "the excluded member does not serve");
        let two = pick_provider_excluding(&settings, &["ex-a".to_string()])
            .await
            .expect("pick");
        assert_eq!(two.id(), "ex-b");

        // An empty exclusion list is plain pick_provider.
        let three = pick_provider_excluding(&settings, &[]).await.expect("pick");
        assert_eq!(three.id(), "ex-a", "a stays out only while excluded");

        // Everyone excluded: the existing exhaustion error path.
        let err = pick_provider_excluding(&settings, &["ex-a".to_string(), "ex-b".to_string()])
            .await
            .expect_err("no member left");
        assert!(
            err.message.contains("disabled for this session"),
            "the error must say the pool is exhausted, got: {}",
            err.message
        );
    }

    /// The status row carries the failure streak and the last-dispatch age:
    /// `None` before the member ever dispatched, a fresh age once a slot is
    /// claimed, and a landed failure cooldown shows in the badge.
    #[tokio::test]
    async fn status_reports_failure_streak_and_dispatch_age() {
        let pool = pool_with(&["st"]);
        let status = pool.status();
        assert_eq!(status[0].consecutive_failures, 0);
        assert_eq!(status[0].last_dispatch_ago_ms, None, "never dispatched");

        let mut picked = pool.pick().await.expect("pick");
        picked.failure_threshold = 2;
        picked.report_failure("HTTP 500");
        let status = pool.status();
        assert_eq!(status[0].consecutive_failures, 1, "the streak shows");
        assert_eq!(status[0].cooldown_remaining_ms, 0, "no window yet");

        picked.report_failure("HTTP 500");
        let status = pool.status();
        assert!(
            status[0].cooldown_remaining_ms > 0,
            "the landed window shows"
        );
        assert_eq!(status[0].consecutive_failures, 0, "reset on landing");

        // A claimed slot shows as a dispatch age; zero micros still means
        // "never".
        {
            let mut runtime = pool.runtime.lock().unwrap();
            runtime.get_mut("st").unwrap().last_dispatch_us = 0;
        }
        assert_eq!(pool.status()[0].last_dispatch_ago_ms, None);
        {
            let mut runtime = pool.runtime.lock().unwrap();
            runtime.get_mut("st").unwrap().last_dispatch_us = 1_500_000;
        }
        let ago = pool.status()[0]
            .last_dispatch_ago_ms
            .expect("dispatched once");
        assert!(ago < 5_000, "a fresh dispatch age, got {ago}ms");
    }
}

#[cfg(test)]
mod health_rotation_tests {
    use super::*;
    use crate::translation::metrics::{translation_metrics, ProviderEventKind};

    fn provider(id: &str, base: &str) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            base_url: format!("https://{base}/v1"),
            api_key: "sk-test".to_string(),
            model: "m".to_string(),
            enabled: true,
            ..Default::default()
        }
    }

    fn pool_with(ids: &[&str]) -> PoolState {
        let providers: Vec<ProviderConfig> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| provider(id, &format!("p{index}.example.com")))
            .collect();
        PoolState {
            runtime: Arc::new(Mutex::new(
                providers
                    .iter()
                    .map(|p| (p.id.clone(), ProviderRuntime::seeded(p)))
                    .collect(),
            )),
            providers,
            cursor: AtomicUsize::new(0),
            origin: Instant::now(),
            _last_dispatch: AtomicU64::new(0),
        }
    }

    /// Seed one provider's metrics window: `ok` clean fast replies, `bad`
    /// gate rejections, `dead` network errors.
    fn seed(id: &str, ok: usize, bad: usize, dead: usize) {
        let metrics = translation_metrics();
        for _ in 0..ok {
            metrics.record_attempt(id, ProviderEventKind::Ok, 3_000);
        }
        for _ in 0..bad {
            metrics.record_attempt(id, ProviderEventKind::GateRejected, 3_000);
        }
        for _ in 0..dead {
            metrics.record_attempt(id, ProviderEventKind::NetworkError, 0);
        }
    }

    /// Equal health rotates strictly: a two-member pool alternates A,B,A,B —
    /// the old idle-time ranking produced the A,A,B,B burst.
    #[tokio::test]
    async fn tied_members_strictly_alternate() {
        let _guard = NOTIFIER_TEST_LOCK.lock().await;
        reset_notifiers_for_test();
        let pool = pool_with(&["t1", "t2"]);
        let first = pool.pick().await.expect("pick");
        let second = pool.pick().await.expect("pick");
        let third = pool.pick().await.expect("pick");
        let fourth = pool.pick().await.expect("pick");
        assert_eq!(first.id(), third.id());
        assert_eq!(second.id(), fourth.id());
        assert_ne!(first.id(), second.id());
        reset_notifiers_for_test();
    }

    /// A member with a poisoned window (half its replies refused by the
    /// quality gates) stops receiving normal traffic while the healthy one
    /// serves.
    #[tokio::test]
    async fn a_degraded_member_stops_getting_normal_traffic() {
        reset_notifiers_for_test();
        // score 55 → degraded; the healthy partner stays at 100.
        seed("deg1", 5, 5, 0);
        seed("ok1", 10, 0, 0);
        let pool = pool_with(&["deg1", "ok1"]);
        {
            let _guard = NOTIFIER_TEST_LOCK.lock().await;
            // The first pick is the degraded member's scheduled probe (it has
            // never been probed); afterwards normal traffic flows to the
            // healthy member only.
            let warmup = pool.pick().await.expect("pick");
            assert_eq!(warmup.id(), "deg1", "the first pick is the probe ride");
            for _ in 0..4 {
                let picked = pool.pick().await.expect("pick");
                assert_eq!(picked.id(), "ok1", "the healthy member serves");
            }
        }
        reset_notifiers_for_test();
    }

    /// A degraded member still receives one probe per interval, so a healed
    /// endpoint can re-enter; and when the healthy member disappears, the
    /// degraded one serves anyway.
    #[tokio::test]
    async fn a_degraded_member_probes_and_serves_as_fallback() {
        let _guard = NOTIFIER_TEST_LOCK.lock().await;
        reset_notifiers_for_test();
        seed("deg2", 5, 5, 0);
        let pool = pool_with(&["deg2", "ok2"]);
        // First pick: the probe is due (never probed), so deg2 is served.
        let first = pool.pick().await.expect("pick");
        assert_eq!(
            first.id(),
            "deg2",
            "the probe ride goes to the degraded member"
        );
        // Immediately after, normal traffic flows to the healthy member.
        let second = pool.pick().await.expect("pick");
        assert_eq!(second.id(), "ok2");
        // With the healthy member gone, the degraded member serves regardless.
        let mut pool = pool;
        pool.providers.retain(|p| p.id != "ok2");
        let third = pool.pick().await.expect("fallback pick");
        assert_eq!(third.id(), "deg2");
        reset_notifiers_for_test();
    }

    /// A member whose score sinks below the retire threshold leaves the
    /// rotation for the session — unless it is the pool's only member.
    #[tokio::test]
    async fn a_retired_member_leaves_but_the_last_member_never_retires() {
        let _guard = NOTIFIER_TEST_LOCK.lock().await;
        reset_notifiers_for_test();
        seed("dead3", 0, 0, 20);
        seed("ok3", 10, 0, 0);
        let pool = pool_with(&["dead3", "ok3"]);
        // dead3 is retired on the first pick; ok3 serves from then on.
        for _ in 0..3 {
            let picked = pool.pick().await.expect("pick");
            assert_eq!(picked.id(), "ok3");
        }
        {
            let runtime = pool.runtime.lock().unwrap();
            let reason = runtime["dead3"]
                .disabled_reason
                .as_deref()
                .expect("retired");
            assert!(reason.contains("health score"), "reason was: {reason}");
        }
        // Single-member pool: the same poisoned window must NOT retire the
        // last endpoint standing — a weak endpoint beats none.
        seed("solo", 0, 0, 20);
        let solo = pool_with(&["solo"]);
        let picked = solo.pick().await.expect("the last member always serves");
        assert_eq!(picked.id(), "solo");
        {
            let runtime = solo.runtime.lock().unwrap();
            assert!(runtime["solo"].disabled_reason.is_none());
        }
        reset_notifiers_for_test();
    }
}
