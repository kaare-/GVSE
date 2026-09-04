//! Frame wrapper around [`crate::step_world`].
//!
//! Isolation: no world types. This only answers "how many product
//! steps may start this frame?" Catch-up (`N > 1`) is how the mouse
//! dies when water is interesting — the default max is **1**.
//! After a step that exceeds `budget`, the next decision is 0 so the
//! frame loop can present. The decision after that is 1 again (do
//! not lock into skip-forever).
//!
//! `budget == 0` means unlimited: always allow one step (old loop).
//! This is not time-coarsen and not a client protocol.

use std::time::Duration;

/// Default play-app budget. Quiet Super-Server ticks (~8 ms) stay
/// every frame; a soaked leftover step (~50 ms) yields one present
/// in between.
pub const DEFAULT_SIM_BUDGET: Duration = Duration::from_millis(12);

/// How many [`crate::step_world`] calls this frame may start.
#[derive(Debug, Clone)]
pub struct SimClock {
    /// Wall-clock cap for one step. Zero = never skip.
    pub budget: Duration,
    /// Hard cap per frame. Stay at 1 unless you have measured that
    /// two cheap steps beat one hitch. Catch-up is opt-in.
    pub max_steps: u32,
    last_step: Duration,
    skip_next: bool,
}

impl Default for SimClock {
    fn default() -> Self {
        Self::new(DEFAULT_SIM_BUDGET)
    }
}

impl SimClock {
    pub fn new(budget: Duration) -> Self {
        Self {
            budget,
            max_steps: 1,
            last_step: Duration::ZERO,
            skip_next: false,
        }
    }

    pub fn unlimited() -> Self {
        Self::new(Duration::ZERO)
    }

    /// Tab slider: milliseconds. `<= 0` turns the budget off.
    pub fn set_budget_ms(&mut self, ms: f32) {
        self.budget = if ms <= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64((ms as f64 / 1000.0).clamp(0.001, 1.0))
        };
    }

    pub fn budget_ms(&self) -> f32 {
        if self.budget.is_zero() {
            0.0
        } else {
            self.budget.as_secs_f32() * 1000.0
        }
    }

    pub fn last_step(&self) -> Duration {
        self.last_step
    }

    /// Steps this frame may start. At most [`Self::max_steps`] (clamped
    /// to 1 unless `max_steps` is raised on purpose).
    pub fn allow_steps(&mut self) -> u32 {
        if self.skip_next {
            self.skip_next = false;
            return 0;
        }
        self.max_steps.min(1).max(1)
    }

    /// Record the wall time of a step that actually ran.
    pub fn record(&mut self, step: Duration) {
        self.last_step = step;
        if !self.budget.is_zero() && step > self.budget {
            self.skip_next = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_frame_allows_one_step() {
        let mut c = SimClock::new(Duration::from_millis(12));
        assert_eq!(c.allow_steps(), 1);
    }

    #[test]
    fn cheap_step_does_not_skip() {
        let mut c = SimClock::new(Duration::from_millis(12));
        assert_eq!(c.allow_steps(), 1);
        c.record(Duration::from_millis(5));
        assert_eq!(c.allow_steps(), 1);
        assert_eq!(c.allow_steps(), 1);
    }

    #[test]
    fn over_budget_skips_exactly_one_frame() {
        let mut c = SimClock::new(Duration::from_millis(12));
        assert_eq!(c.allow_steps(), 1);
        c.record(Duration::from_millis(50));
        assert_eq!(c.allow_steps(), 0);
        assert_eq!(c.allow_steps(), 1);
        // last_step still 50 ms, but we already paid the skip
        c.record(Duration::from_millis(50));
        assert_eq!(c.allow_steps(), 0);
        assert_eq!(c.allow_steps(), 1);
    }

    #[test]
    fn zero_budget_never_skips() {
        let mut c = SimClock::unlimited();
        assert_eq!(c.allow_steps(), 1);
        c.record(Duration::from_millis(80));
        assert_eq!(c.allow_steps(), 1);
    }

    #[test]
    fn set_budget_ms_zero_disables() {
        let mut c = SimClock::default();
        c.set_budget_ms(0.0);
        assert!(c.budget.is_zero());
        c.record(Duration::from_millis(80));
        assert_eq!(c.allow_steps(), 1);
    }

    #[test]
    fn default_max_is_one() {
        let mut c = SimClock::default();
        c.max_steps = 8;
        // v1 clamp: catch-up stays off even if someone raises the field
        assert_eq!(c.allow_steps(), 1);
    }
}
