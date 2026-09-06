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
use crate::translation::settings::ProviderConfig;

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
    /// Consecutive 4xx (non-429) responses. At the disable threshold the
    /// provider leaves the rotation for the session.
    client_errors: u32,
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

    /// A clean response earns climb credit. Notifies only when the reward
    /// actually moved the observable rate (every [`AimdController`]'s
    /// [`crate::translation::aimd`] step), not on the quiet successes
    /// between steps — a per-request event would be pure noise.
    pub fn report_success(&self) {
        let changed = {
            let mut runtime = self
                .runtime
                .lock()
                .expect("pool runtime lock is never poisoned across a panic-free run");
            let entry = runtime.entry(self.provider_id.clone()).or_default();
            entry.client_errors = 0;
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

    /// A rate-limit verdict: halve and cool down per the endpoint's header.
    /// Takes the lock briefly; the change is visible to every queued request
    /// immediately — one 429 slows the whole provider, not just the request
    /// that drew it. Always observable, so always notifies.
    pub fn report_rate_limited(&self, retry_after: Option<Duration>) {
        {
            let mut runtime = self
                .runtime
                .lock()
                .expect("pool runtime lock is never poisoned across a panic-free run");
            let entry = runtime.entry(self.provider_id.clone()).or_default();
            entry.client_errors = 0;
            if let Some(aimd) = entry.aimd.as_mut() {
                aimd.penalize(retry_after, Instant::now());
            }
        }
        notify_change();
    }

    /// A client error (bad key, wrong URL). Two in a row retire the provider
    /// for the session — the rotation stops spending real quota on a request
    /// that cannot succeed. Notifies when the disable lands, which is the
    /// only observable step here.
    pub fn report_client_error(&self, detail: &str) {
        let newly_disabled = {
            let mut runtime = self
                .runtime
                .lock()
                .expect("pool runtime lock is never poisoned across a panic-free run");
            let entry = runtime.entry(self.provider_id.clone()).or_default();
            entry.client_errors += 1;
            let newly = AimdController::should_disable(entry.client_errors)
                && entry.disabled_reason.is_none();
            if newly {
                entry.disabled_reason = Some(format!(
                    "the endpoint rejected the request twice ({detail}) — disabled for this session"
                ));
                tracing::warn!(
                    "[translation] provider {} disabled for the session: {detail}",
                    self.provider_id,
                );
            }
            newly
        };
        if newly_disabled {
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
    /// `Retry-After` parking remaining, milliseconds; 0 when dispatchable.
    pub cooldown_remaining_ms: u64,
    /// Set when the provider was retired for the session, with the reason.
    pub disabled_reason: Option<String>,
    /// POSTs actually dispatched in the current wall-clock minute. The
    /// adaptive rate is what the limiter ALLOWS; this is what the endpoint is
    /// really being asked to serve, which is what "rate is high but nothing
    /// translates" reports turn on.
    pub dispatched_last_minute: u64,
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
    ///
    /// Three gates run before the choice, in escalation order:
    ///
    /// 1. **Health retirement** — a member whose health score has sunk below
    ///    [`health::RETIRE_THRESHOLD`] with a real sample leaves the rotation
    ///    for the session, the quality-side twin of the two-consecutive-4xx
    ///    rule. Never applied when it would empty the pool: one endpoint,
    ///    however bad, beats none.
    /// 2. **Fallback partition** — members below [`health::DEGRADE_THRESHOLD`]
    ///    stop receiving normal traffic; the batch goes to whoever is still
    ///    healthy. If nobody is healthy the degraded members serve anyway
    ///    (a weak endpoint beats no endpoint).
    /// 3. **Probe** — a degraded member otherwise starves (no traffic, no
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
    /// Every member cooling at once: wait for the earliest cooldown to lapse
    /// (capped), then pick again — failing immediately would surface a 429
    /// the rotation could have absorbed by breathing for a few seconds.
    pub async fn pick(&self) -> Result<PickedProvider, AppCommandError> {
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
                survivors.push((id, health));
            }
            if survivors.is_empty() {
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
                let (rpm, cooldown, reason) = match entry {
                    Some(entry) => {
                        let cooldown = entry
                            .aimd
                            .as_mut()
                            .and_then(|aimd| aimd.cooldown_remaining(now))
                            .map(|remaining| remaining.as_millis() as u64)
                            .unwrap_or(0);
                        (
                            entry.aimd.as_ref().map(AimdController::allowed_rpm),
                            cooldown,
                            entry.disabled_reason.clone(),
                        )
                    }
                    None => (None, 0, None),
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
                    disabled_reason: reason,
                    dispatched_last_minute: crate::translation::metrics::translation_metrics()
                        .dispatched_last_minute(&provider.id),
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

/// Pick a provider for one outbound request from the given settings' pool.
pub async fn pick_provider(
    settings: &crate::translation::settings::TranslationSettings,
) -> Result<PickedProvider, AppCommandError> {
    let providers = settings.active_providers();
    if providers.is_empty() {
        return Err(AppCommandError::configuration_missing(
            "No enabled translation provider is fully configured",
        ));
    }
    pool_for(providers).pick().await
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
            providers: vec![provider("a", "a.example.com"), provider("b", "b.example.com")],
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

    /// A provider retired for the session leaves the rotation; disabling the
    /// last one surfaces a classified error instead of a doomed request.
    #[tokio::test]
    async fn a_disabled_provider_is_skipped_and_the_last_one_errors() {
        let pool = PoolState {
            runtime: Arc::new(Mutex::new(HashMap::new())),
            providers: vec![provider("a", "a.example.com"), provider("b", "b.example.com")],
            cursor: AtomicUsize::new(0),
            origin: Instant::now(),
            _last_dispatch: AtomicU64::new(0),
        };
        let first = pool.pick().await.expect("pick");
        first.report_client_error("HTTP 401");
        first.report_client_error("HTTP 401");
        let second = pool.pick().await.expect("pick");
        assert_ne!(second.id(), first.id(), "the retired member is skipped");
        let third = pool.pick().await.expect("pick");
        assert_ne!(third.id(), first.id(), "still skipped");
        third.report_client_error("HTTP 401");
        third.report_client_error("HTTP 401");
        let err = pool.pick().await.expect_err("no member left");
        assert!(
            err.message.contains("disabled for this session"),
            "the error must say the pool is exhausted, got: {}",
            err.message
        );
    }

    /// A `Retry-After` park defers a member's next pick: with one of two
    /// cooling, the other serves; with both cooling, `pick` waits past the
    /// window instead of erroring.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_cooling_provider_defers_to_the_others() {
        let pool = PoolState {
            runtime: Arc::new(Mutex::new(
                vec![provider("a", "a.example.com"), provider("b", "b.example.com")]
                    .into_iter()
                    .map(|p| (p.id.clone(), ProviderRuntime::seeded(&p)))
                    .collect(),
            )),
            providers: vec![provider("a", "a.example.com"), provider("b", "b.example.com")],
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
        assert!(pool.status()[0].allowed_rpm > 0.0, "a member shows its rate");
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
    /// penalty always, a disable when it lands, and a success only on the
    /// every-tenth one that moves the rate. The registry is process-global,
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
        let picked = pool.pick().await.expect("pick");

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

        // Two client errors retire the provider — the landing notifies.
        picked.report_client_error("HTTP 401");
        let after_first = fired.load(Ordering::SeqCst);
        picked.report_client_error("HTTP 401");
        assert!(
            fired.load(Ordering::SeqCst) > after_first,
            "the disable must notify"
        );

        reset_notifiers_for_test();
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
        assert_eq!(first.id(), "deg2", "the probe ride goes to the degraded member");
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
            let reason = runtime["dead3"].disabled_reason.as_deref().expect("retired");
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
