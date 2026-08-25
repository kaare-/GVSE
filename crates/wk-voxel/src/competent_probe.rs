//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app.
//!
//! Counters for diagnosing competent-fall cost. Debug tooling only — all
//! increments are `Relaxed` atomics behind `#[inline]` helpers so release
//! builds pay a single add per event.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

macro_rules! counters {
  ($($name:ident),* $(,)?) => {
    $(
      #[allow(non_upper_case_globals)]
      pub static $name: AtomicU64 = AtomicU64::new(0);
    )*

    /// Snapshot of every counter.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct Probe {
      $(pub $name: u64,)*
    }

    pub fn snapshot() -> Probe {
      Probe { $($name: $name.load(Relaxed),)* }
    }

    pub fn reset() {
      $($name.store(0, Relaxed);)*
    }
  };
}

counters!(
  build_calls,
  seed_candidates,
  seeds_passed,
  floods,
  flood_cells,
  strata_bailouts,
  split_calls,
  split_cells,
  hang_calls,
  components,
  cargo_calls,
  cargo_cells,
  // Why the pass ran at all, and what each component decided. A settled
  // world should drive all of these toward zero; anything that stays high
  // is a body being re-evaluated forever instead of going to sleep.
  wake_cells,
  region_cells,
  // Which source refilled the wake list.
  wake_from_solidity,
  wake_from_moved,
  wake_from_cadence_float,
  wake_from_cadence_seed,
  comp_slept,
  comp_floating,
  comp_unsupported_stuck,
  comp_fell,
  comp_fall_refused,
  comp_rolled,
  comp_shattered,
);

#[inline]
pub fn bump(c: &AtomicU64) {
  c.fetch_add(1, Relaxed);
}

#[inline]
pub fn add(c: &AtomicU64, n: u64) {
  c.fetch_add(n, Relaxed);
}
