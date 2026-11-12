//! Thread-safe sliding-window rate limiter with exponential backoff and jitter.
//!
//! The photo API allows roughly 100 requests per IP per 5 minutes, shared between
//! `/v1/photos/random` and `/v1/images/...` downloads. The limiter stops at a safe ceiling of
//! 90. Every time-dependent method has an `_at(now)` form so tests never sleep.

use crate::{RATE_LIMIT_MAX_REQUESTS, RATE_LIMIT_WINDOW};
use parking_lot::Mutex;
use rand::Rng;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// First backoff step.
pub const BACKOFF_BASE_MS: u64 = 2_000;
/// Backoff ceiling before jitter.
pub const BACKOFF_MAX_MS: u64 = 120_000;
/// Doubling stops after this many doublings (2s * 2^6 = 128s, capped at 120s).
pub const BACKOFF_MAX_EXPONENT: u32 = 6;
/// A server-provided `Retry-After` is honoured up to this long.
pub const RETRY_AFTER_MAX: Duration = Duration::from_secs(3_600);

/// Sliding-window limiter shared by JSON calls and image downloads.
pub struct SlidingRateLimiter {
    max_requests: usize,
    window: Duration,
    timestamps: Mutex<VecDeque<Instant>>,
    cooldown_until: Mutex<Option<Instant>>,
    consecutive_failures: AtomicU32,
    /// Last API attempt failed with a network error and nothing has succeeded since.
    offline: AtomicBool,
}

impl Default for SlidingRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl SlidingRateLimiter {
    /// 90 requests per 5 minutes.
    pub fn new() -> Self {
        Self::with_limits(RATE_LIMIT_MAX_REQUESTS, RATE_LIMIT_WINDOW)
    }

    /// Custom budget (used by tests).
    pub fn with_limits(max_requests: usize, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            timestamps: Mutex::new(VecDeque::with_capacity(max_requests + 4)),
            cooldown_until: Mutex::new(None),
            consecutive_failures: AtomicU32::new(0),
            offline: AtomicBool::new(false),
        }
    }

    /// Maximum requests allowed inside one window.
    pub fn max_requests(&self) -> usize {
        self.max_requests
    }

    fn prune(&self, q: &mut VecDeque<Instant>, now: Instant) {
        while let Some(&front) = q.front() {
            if now.saturating_duration_since(front) > self.window {
                q.pop_front();
            } else {
                break;
            }
        }
    }

    fn cooling_down(&self, now: Instant) -> bool {
        matches!(*self.cooldown_until.lock(), Some(until) if now < until)
    }

    /// True when no cooldown is active and the window has room.
    pub fn can_make_request_at(&self, now: Instant) -> bool {
        if self.cooling_down(now) {
            return false;
        }
        let mut q = self.timestamps.lock();
        self.prune(&mut q, now);
        q.len() < self.max_requests
    }

    /// [`Self::can_make_request_at`] with the current time.
    pub fn can_make_request(&self) -> bool {
        self.can_make_request_at(Instant::now())
    }

    /// Records one request at `now` without checking the budget.
    pub fn record_request_at(&self, now: Instant) {
        self.timestamps.lock().push_back(now);
    }

    /// [`Self::record_request_at`] with the current time.
    pub fn record_request(&self) {
        self.record_request_at(Instant::now());
    }

    /// Atomically checks the budget and, if allowed, records the request.
    pub fn try_acquire_at(&self, now: Instant) -> bool {
        if self.cooling_down(now) {
            return false;
        }
        let mut q = self.timestamps.lock();
        self.prune(&mut q, now);
        if q.len() < self.max_requests {
            q.push_back(now);
            true
        } else {
            false
        }
    }

    /// [`Self::try_acquire_at`] with the current time.
    pub fn try_acquire(&self) -> bool {
        self.try_acquire_at(Instant::now())
    }

    /// Requests still available inside the current window (0 while cooling down).
    pub fn remaining_at(&self, now: Instant) -> usize {
        if self.cooling_down(now) {
            return 0;
        }
        let mut q = self.timestamps.lock();
        self.prune(&mut q, now);
        self.max_requests.saturating_sub(q.len())
    }

    /// [`Self::remaining_at`] with the current time.
    pub fn remaining(&self) -> usize {
        self.remaining_at(Instant::now())
    }

    /// Time left on the active cooldown, if any.
    pub fn cooldown_remaining_at(&self, now: Instant) -> Option<Duration> {
        match *self.cooldown_until.lock() {
            Some(until) if now < until => Some(until - now),
            _ => None,
        }
    }

    /// [`Self::cooldown_remaining_at`] with the current time.
    pub fn cooldown_remaining(&self) -> Option<Duration> {
        self.cooldown_remaining_at(Instant::now())
    }

    /// Un-jittered exponential delay for the n-th consecutive failure: 2s, 4s, 8s, ... capped
    /// at 120s. The first failure (and a defensive 0) waits the base 2s.
    pub fn backoff_base(consecutive_failures: u32) -> Duration {
        let exp = consecutive_failures
            .saturating_sub(1)
            .min(BACKOFF_MAX_EXPONENT);
        let ms = BACKOFF_BASE_MS
            .saturating_mul(1_u64 << exp)
            .min(BACKOFF_MAX_MS);
        Duration::from_millis(ms)
    }

    /// Exponential delay plus up to 25% additive jitter. `jitter_unit` is clamped to `[0, 1]`.
    pub fn backoff_delay(consecutive_failures: u32, jitter_unit: f64) -> Duration {
        let base = Self::backoff_base(consecutive_failures);
        let jitter_ms = (base.as_millis() as f64 * 0.25 * jitter_unit.clamp(0.0, 1.0)) as u64;
        base + Duration::from_millis(jitter_ms)
    }

    /// Starts a cooldown of `backoff_delay(failures, jitter_unit)` measured from `now`. A new
    /// cooldown never shortens one that is already running.
    pub fn trigger_backoff_at(
        &self,
        now: Instant,
        consecutive_failures: u32,
        jitter_unit: f64,
    ) -> Duration {
        let dur = Self::backoff_delay(consecutive_failures, jitter_unit);
        let mut guard = self.cooldown_until.lock();
        let new_until = now + dur;
        *guard = Some(match *guard {
            Some(existing) if existing > new_until => existing,
            _ => new_until,
        });
        dur
    }

    /// Starts a randomly jittered cooldown from now.
    pub fn trigger_backoff(&self, consecutive_failures: u32) -> Duration {
        let jitter: f64 = rand::thread_rng().gen();
        self.trigger_backoff_at(Instant::now(), consecutive_failures, jitter)
    }

    /// Starts a cooldown of exactly `dur` from `now` (never shortens a running one).
    pub fn trigger_cooldown_at(&self, now: Instant, dur: Duration) {
        let mut guard = self.cooldown_until.lock();
        let new_until = now + dur;
        *guard = Some(match *guard {
            Some(existing) if existing > new_until => existing,
            _ => new_until,
        });
    }

    /// Cooldown for one failure: the exponential backoff, or the server's `Retry-After` when
    /// that is longer (capped at [`RETRY_AFTER_MAX`]).
    fn fail_at(&self, now: Instant, retry_after: Option<Duration>, jitter_unit: f64) -> Duration {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        let mut dur = Self::backoff_delay(failures, jitter_unit);
        if let Some(ra) = retry_after {
            dur = dur.max(ra.min(RETRY_AFTER_MAX));
        }
        self.trigger_cooldown_at(now, dur);
        dur
    }

    /// Registers a 403 / 429 / 503 and starts the next exponential cooldown. The failure
    /// counter lives in the limiter so it survives across refill calls.
    pub fn note_throttled(&self) -> Duration {
        self.note_throttled_with(None)
    }

    /// [`Self::note_throttled`] honouring a parsed `Retry-After`.
    pub fn note_throttled_with(&self, retry_after: Option<Duration>) -> Duration {
        let jitter: f64 = rand::thread_rng().gen();
        self.fail_at(Instant::now(), retry_after, jitter)
    }

    /// Registers a network failure (DNS, TLS, timeout, connection refused): marks the limiter
    /// offline and starts the same escalating cooldown, so callers never hot-loop.
    pub fn note_network_error(&self) -> Duration {
        self.offline.store(true, Ordering::SeqCst);
        self.note_throttled_with(None)
    }

    /// True when the last API attempt failed on the network and nothing succeeded since.
    pub fn is_offline(&self) -> bool {
        self.offline.load(Ordering::SeqCst)
    }

    /// Registers a successful response: resets the failure counter and clears offline.
    pub fn note_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.offline.store(false, Ordering::SeqCst);
    }

    /// Forgets the failure counter and any running cooldown so the next request may go out now
    /// (manual "Retry now"). The `offline` flag stays until a request succeeds. The sliding
    /// request window is untouched, so the rate budget is still respected.
    pub fn clear_backoff(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        *self.cooldown_until.lock() = None;
    }

    /// Consecutive throttled responses since the last success.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_backoff_ends_the_cooldown_but_keeps_offline() {
        let limiter = SlidingRateLimiter::new();
        limiter.note_network_error();
        assert!(limiter.cooldown_remaining().is_some());
        assert_eq!(limiter.consecutive_failures(), 1);
        limiter.clear_backoff();
        assert!(limiter.cooldown_remaining().is_none());
        assert!(limiter.can_make_request());
        assert_eq!(limiter.consecutive_failures(), 0);
        assert!(limiter.is_offline());
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn default_ceiling_is_90_of_100() {
        assert_eq!(RATE_LIMIT_MAX_REQUESTS, 90);
        assert_eq!(SlidingRateLimiter::new().max_requests(), 90);
        assert_eq!(RATE_LIMIT_WINDOW, secs(300));
    }

    #[test]
    fn blocks_at_ceiling_and_reopens_after_window() {
        let l = SlidingRateLimiter::new();
        let t0 = Instant::now();
        for i in 0..90 {
            assert!(
                l.try_acquire_at(t0 + Duration::from_millis(i)),
                "request {i}"
            );
        }
        assert!(!l.can_make_request_at(t0 + secs(1)));
        assert!(!l.try_acquire_at(t0 + secs(299)));
        assert_eq!(l.remaining_at(t0 + secs(299)), 0);
        // Entries exactly one window old are still counted (strictly-greater expiry).
        assert!(!l.can_make_request_at(t0 + secs(300)));
        assert!(l.can_make_request_at(t0 + secs(301)));
        assert_eq!(l.remaining_at(t0 + secs(301)), 90);
    }

    #[test]
    fn window_slides_instead_of_resetting() {
        let l = SlidingRateLimiter::with_limits(3, secs(10));
        let t0 = Instant::now();
        assert!(l.try_acquire_at(t0));
        assert!(l.try_acquire_at(t0 + secs(4)));
        assert!(l.try_acquire_at(t0 + secs(8)));
        assert!(!l.try_acquire_at(t0 + secs(9)));
        // t0 expired, the other two still count.
        assert_eq!(l.remaining_at(t0 + secs(11)), 1);
        assert!(l.try_acquire_at(t0 + secs(11)));
        assert!(!l.try_acquire_at(t0 + secs(12)));
        assert!(l.try_acquire_at(t0 + secs(15)));
    }

    #[test]
    fn record_request_counts_like_acquire() {
        let l = SlidingRateLimiter::with_limits(2, secs(10));
        let t0 = Instant::now();
        l.record_request_at(t0);
        l.record_request_at(t0);
        assert!(!l.can_make_request_at(t0));
        let l = SlidingRateLimiter::with_limits(2, secs(10));
        l.record_request();
        assert_eq!(l.remaining(), 1);
        assert!(l.can_make_request());
    }

    #[test]
    fn backoff_base_doubles_and_caps_at_120s() {
        let ms = |n| SlidingRateLimiter::backoff_base(n).as_millis();
        assert_eq!(ms(0), 2_000);
        assert_eq!(ms(1), 2_000, "first failure waits 2s");
        assert_eq!(ms(2), 4_000);
        assert_eq!(ms(3), 8_000);
        assert_eq!(ms(6), 64_000);
        assert_eq!(ms(7), 120_000);
        assert_eq!(ms(40), 120_000);
    }

    #[test]
    fn jitter_adds_zero_to_25_percent() {
        let d = |n, j| SlidingRateLimiter::backoff_delay(n, j).as_millis();
        assert_eq!(d(2, 0.0), 4_000);
        assert_eq!(d(2, 1.0), 5_000);
        assert_eq!(d(2, 0.5), 4_500);
        assert_eq!(d(2, 9.0), 5_000, "jitter is clamped");
        assert_eq!(d(8, 1.0), 150_000);
    }

    #[test]
    fn backoff_blocks_until_cooldown_expires_and_never_shrinks() {
        let l = SlidingRateLimiter::new();
        let t0 = Instant::now();
        let dur = l.trigger_backoff_at(t0, 3, 0.0);
        assert_eq!(dur, secs(8));
        assert!(!l.can_make_request_at(t0 + secs(7)));
        assert!(!l.try_acquire_at(t0 + secs(7)));
        assert_eq!(l.remaining_at(t0 + secs(7)), 0);
        assert_eq!(l.cooldown_remaining_at(t0 + secs(5)), Some(secs(3)));
        // A shorter backoff does not cut the running one short.
        l.trigger_backoff_at(t0 + secs(1), 0, 0.0);
        assert!(!l.can_make_request_at(t0 + secs(7)));
        assert!(l.can_make_request_at(t0 + secs(8)));
        assert_eq!(l.cooldown_remaining_at(t0 + secs(8)), None);
    }

    #[test]
    fn failure_counter_escalates_until_success() {
        let l = SlidingRateLimiter::new();
        let d1 = l.note_throttled();
        let d2 = l.note_throttled();
        assert!((2_000..=2_500).contains(&(d1.as_millis() as u64)));
        assert!((4_000..=5_000).contains(&(d2.as_millis() as u64)));
        assert_eq!(l.consecutive_failures(), 2);
        assert!(!l.can_make_request());
        l.note_success();
        assert_eq!(l.consecutive_failures(), 0);
    }

    #[test]
    fn retry_after_extends_cooldown_and_is_capped() {
        let l = SlidingRateLimiter::new();
        let d = l.note_throttled_with(Some(secs(30)));
        assert_eq!(d, secs(30));
        assert!(l.cooldown_remaining().unwrap() > secs(29));
        let l = SlidingRateLimiter::new();
        let d = l.note_throttled_with(Some(secs(1_000_000)));
        assert_eq!(d, RETRY_AFTER_MAX);
        // A short Retry-After never undercuts the exponential backoff.
        let l = SlidingRateLimiter::new();
        assert!(l.note_throttled_with(Some(secs(0))) >= secs(2));
    }

    #[test]
    fn network_errors_cool_down_and_flag_offline_until_success() {
        let l = SlidingRateLimiter::new();
        assert!(!l.is_offline());
        let d = l.note_network_error();
        assert!(d >= secs(2));
        assert!(l.is_offline());
        assert!(!l.can_make_request(), "no hot loop while offline");
        l.note_success();
        assert!(!l.is_offline());
    }
}
