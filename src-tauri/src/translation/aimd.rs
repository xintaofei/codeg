//! The per-provider adaptive rate limiter (AIMD: additive increase, multiplicative
//! decrease).
//!
//! Translation endpoints differ wildly in what they tolerate — a shared relay
//! may cap the account at 10 requests a minute, a paid endpoint at 500 — and
//! the cap is rarely knowable in advance. Rather than asking the user to tune
//! pacing knobs by trial and error, every provider's dispatch rate adapts to
//! what the endpoint actually does: a 429 halves the allowed rate (and cools
//! down for the `Retry-After` it names, when it does); a run of successes
//! climbs back toward the configured ceiling, one step at a time.
//!
//! All state is in-memory and per-process: restarting re-probes, which costs
//! a few 429s at worst and keeps nothing stale on disk.

use std::time::Duration;

/// The rate an unexplored provider starts at when the user set no explicit
/// ceiling. High enough that a typical endpoint never sees a 429; low enough
/// that a burst against a strict one gives up the secret within seconds.
pub const AUTO_START_RPM: f64 = 15.0;

/// The climb ceiling in auto mode: beyond this, "the endpoint allows it"
/// stops mattering — codeg would be the reason a shared relay falls over.
pub const AUTO_MAX_RPM: f64 = 60.0;

/// The rate floor. Halving below this serves nobody: one request every 30
/// seconds is already the outer edge of a reading flow that still converges.
const MIN_RPM: f64 = 2.0;

/// Five consecutive successes earn one additive step (+2 RPM).
const REWARDS_PER_STEP: u32 = 5;

/// 一次爬升的步长。5 次连续成功 +2 RPM：一次 429 的惩罚在几十秒内
/// 可以消化，而不是像 +1 那样在低速率下滞留数分钟。
const REWARD_STEP_RPM: f64 = 2.0;

/// The longest a `Retry-After` may park a provider. An hour-long backoff is
/// the endpoint saying "come back tomorrow" — the rotation should find
/// another member meanwhile, and this one re-enters on its own when the
/// cooldown lapses.
pub const MAX_COOLDOWN: Duration = Duration::from_secs(120);

/// One provider's adaptive dispatch budget.
///
/// Not `Clone`: the state is owned by the pool and mutated through it, so a
/// copied controller would fork the very history the adaptation learns from.
#[derive(Debug)]
pub struct AimdController {
    /// Requests per minute the provider may dispatch right now. The dispatch
    /// interval is `60 / allowed_rpm` seconds.
    allowed_rpm: f64,
    /// The climb ceiling: the user's explicit cap, or [`AUTO_MAX_RPM`] in
    /// auto mode.
    ceiling: f64,
    /// Successes since the last penalty; [`REWARDS_PER_STEP`] buys +1 RPM.
    consecutive_successes: u32,
    /// Until this instant the provider accepts nothing (a `Retry-After`
    /// verdict). `None` when idle. Deadlines in the past are cleared lazily.
    cooldown_until: Option<std::time::Instant>,
    /// The most recent cooldown, for status reporting after it lapses.
    last_cooldown: Option<std::time::Instant>,
}

impl AimdController {
    /// Start at the user's ceiling when one is set (trust the configuration
    /// that names the endpoint's real quota), otherwise probe from
    /// [`AUTO_START_RPM`].
    pub fn new(rpm_cap: Option<u32>) -> Self {
        let ceiling = rpm_cap
            .map(|value| value as f64)
            .unwrap_or(AUTO_MAX_RPM);
        let start = rpm_cap.map(|value| value as f64).unwrap_or(AUTO_START_RPM);
        Self {
            allowed_rpm: start.min(ceiling),
            ceiling,
            consecutive_successes: 0,
            cooldown_until: None,
            last_cooldown: None,
        }
    }

    /// The rate in force right now, for status reporting.
    pub fn allowed_rpm(&self) -> f64 {
        self.allowed_rpm
    }

    /// The gap two consecutive dispatches must keep apart.
    pub fn dispatch_interval(&self) -> Duration {
        let per_second = self.allowed_rpm / 60.0;
        let interval = 1.0 / per_second.max(f64::MIN_POSITIVE);
        Duration::from_secs_f64(interval.min(30.0))
    }

    /// Whether the provider is parked by a `Retry-After`, and until when.
    /// Expired cooldowns are cleared lazily so status reads stay truthful
    /// without a background sweeper.
    pub fn cooldown_remaining(&mut self, now: std::time::Instant) -> Option<Duration> {
        let until = self.cooldown_until?;
        if now >= until {
            self.cooldown_until = None;
            return None;
        }
        Some(until - now)
    }

    /// A rate-limit verdict: halve the allowed rate and park the provider for
    /// the window the endpoint asked for. Takes `&mut self` under the pool's
    /// lock, so the change is visible to every queued request immediately —
    /// one 429 slows the whole provider, not just the request that drew it.
    pub fn penalize(&mut self, retry_after: Option<Duration>, now: std::time::Instant) {
        self.allowed_rpm = (self.allowed_rpm / 2.0).max(MIN_RPM).min(self.ceiling);
        self.consecutive_successes = 0;
        let window = retry_after
            .map(|window| window.min(MAX_COOLDOWN))
            .unwrap_or(Duration::ZERO);
        if window.is_zero() {
            // No window named: stay dispatchable at the halved rate — the
            // pacing below is the throttling.
            self.last_cooldown = self.cooldown_until.take();
            return;
        }
        let until = now + window;
        self.last_cooldown = Some(until);
        self.cooldown_until = Some(until);
    }

    /// A clean response: at [`REWARDS_PER_STEP`] consecutive successes, climb
    /// [`REWARD_STEP_RPM`] toward the ceiling. The counter resets on penalty,
    /// so a flapping endpoint oscillates around its real quota instead of
    /// ratcheting past it on stale credit.
    pub fn reward(&mut self) {
        self.consecutive_successes += 1;
        if self.consecutive_successes >= REWARDS_PER_STEP {
            self.consecutive_successes = 0;
            self.allowed_rpm = (self.allowed_rpm + REWARD_STEP_RPM).min(self.ceiling);
        }
    }

    /// Whether a client error (4xx other than 429) should retire the
    /// provider for the session. One 401 proves the key wrong — retrying it
    /// spends nothing and fixes nothing; a single transport blip must not.
    /// Two consecutive client errors on a *configured* endpoint is a
    /// configuration problem the pool should stop feeding.
    pub fn should_disable(client_errors: u32) -> bool {
        client_errors >= 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn controller(cap: Option<u32>) -> AimdController {
        AimdController::new(cap)
    }

    #[test]
    fn an_explicit_cap_starts_at_the_cap_and_never_climbs_past_it() {
        let mut c = controller(Some(30));
        assert_eq!(c.allowed_rpm(), 30.0);
        for _ in 0..200 {
            c.reward();
        }
        assert_eq!(c.allowed_rpm(), 30.0, "the ceiling holds");
    }

    #[test]
    fn auto_mode_starts_conservative_and_climbs_to_the_auto_ceiling() {
        let mut c = controller(None);
        assert_eq!(c.allowed_rpm(), AUTO_START_RPM);
        for _ in 0..2000 {
            c.reward();
        }
        assert_eq!(c.allowed_rpm(), AUTO_MAX_RPM);
    }

    #[test]
    fn the_dispatch_interval_is_sixty_seconds_over_the_rate() {
        assert_eq!(controller(Some(30)).dispatch_interval(), Duration::from_secs(2));
        assert_eq!(controller(Some(60)).dispatch_interval(), Duration::from_secs(1));
    }

    #[test]
    fn a_penalty_halves_the_rate_and_lands_on_the_floor() {
        let mut c = controller(Some(30));
        c.penalize(None, Instant::now());
        assert_eq!(c.allowed_rpm(), 15.0);
        for _ in 0..10 {
            c.penalize(None, Instant::now());
        }
        assert_eq!(c.allowed_rpm(), MIN_RPM, "repeated halvings stop at the floor");
    }

    #[test]
    fn a_penalty_parks_the_provider_for_the_retry_after_window() {
        let mut c = controller(None);
        let now = Instant::now();
        c.penalize(Some(Duration::from_secs(45)), now);
        let remaining = c.cooldown_remaining(now).expect("parked");
        assert!(remaining > Duration::from_secs(44) && remaining <= Duration::from_secs(45));
        // A penalty without a window throttles by rate alone.
        let mut bare = controller(None);
        bare.penalize(None, now);
        assert_eq!(bare.cooldown_remaining(now), None);
    }

    #[test]
    fn a_retry_after_beyond_the_cap_is_clamped() {
        let mut c = controller(None);
        let now = Instant::now();
        c.penalize(Some(Duration::from_secs(3600)), now);
        let remaining = c.cooldown_remaining(now).expect("parked");
        assert!(remaining <= MAX_COOLDOWN);
    }

    #[test]
    fn an_expired_cooldown_clears_on_read() {
        let mut c = controller(None);
        let now = Instant::now();
        c.penalize(Some(Duration::from_secs(1)), now);
        // Probing "later" (the same `now` base, but past the deadline) is
        // modeled by penalizing at an earlier instant — construct one by
        // penalizing with a short window and checking after the window.
        let mut early = controller(None);
        early.penalize(Some(Duration::from_secs(1)), now - Duration::from_secs(2));
        assert_eq!(early.cooldown_remaining(now), None, "a lapsed window clears");
    }

    #[test]
    fn a_penalty_resets_the_climb_credit() {
        let mut c = controller(None);
        for _ in 0..(REWARDS_PER_STEP - 1) {
            c.reward();
        }
        c.penalize(None, Instant::now());
        c.reward();
        assert_eq!(
            c.allowed_rpm(),
            (AUTO_START_RPM / 2.0).max(MIN_RPM),
            "one success after a penalty buys nothing"
        );
    }

    #[test]
    fn five_consecutive_successes_climb_two_rpm() {
        let mut c = controller(None);
        c.penalize(None, Instant::now());
        let halved = c.allowed_rpm();
        for _ in 0..(REWARDS_PER_STEP - 1) {
            c.reward();
        }
        assert_eq!(c.allowed_rpm(), halved, "four successes buy nothing");
        c.reward();
        assert_eq!(c.allowed_rpm(), halved + 2.0);
    }

    #[test]
    fn two_client_errors_in_a_row_retire_the_provider() {
        assert!(!AimdController::should_disable(0));
        assert!(!AimdController::should_disable(1));
        assert!(AimdController::should_disable(2));
    }
}
