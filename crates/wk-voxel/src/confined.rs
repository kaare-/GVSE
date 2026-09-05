//! Persistent confined-head bodies (time-coarsen item 3, Δt = 1).
//!
//! Isolation: no column-stack imports. No world types — [`World`] holds
//! this store; the BFS lives in [`crate::rules::water_flow`].
//!
//! A communicating vessel is labeled once. Later period-16 wakes look
//! up the stored donor and recompute head from live sat so ocean evap
//! still drives a far well. A higher standing row clears the store
//! (new tarn). Do **not** shrink the BFS limit or starve the wake —
//! persist, then skip only the walk.
//!
//! This is not a Δt>1 integrator and not a seam ledger.

use crate::fasthash::FxHashMap;

/// Seed (full pressure cell) → free-surface donor.
///
/// Derived. Never saved. Losing it on load costs one BFS.
#[derive(Debug, Clone, Default)]
pub struct ConfinedStore {
    by_seed: FxHashMap<(i32, i32), (i32, i32)>,
    /// Highest standing-air row when `by_seed` was last filled.
    /// A *higher* row means new water that the stored donor may miss.
    max_stand_gy: Option<i32>,
    /// BFS walks this session (tests / leftover probe).
    pub bfs_runs: u64,
}

impl ConfinedStore {
    pub fn is_empty(&self) -> bool {
        self.by_seed.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_seed.len()
    }

    /// Drop remembered vessels. Next confined apply BFS-es again.
    pub fn clear(&mut self) {
        self.by_seed.clear();
        self.max_stand_gy = None;
    }

    /// Prepare for a wake. `None` is bootstrap (unset standing bands) —
    /// do not trust a stored donor. A higher standing row is a new
    /// reservoir; clear so the far well can find it.
    pub fn begin_wake(&mut self, max_stand_gy: Option<i32>) -> bool {
        let persist = max_stand_gy.is_some();
        if !persist {
            self.clear();
            return false;
        }
        if self
            .max_stand_gy
            .is_some_and(|old| max_stand_gy.unwrap() > old)
        {
            self.by_seed.clear();
        }
        self.max_stand_gy = max_stand_gy;
        true
    }

    pub fn donor_of(&self, seed: (i32, i32)) -> Option<(i32, i32)> {
        self.by_seed.get(&seed).copied()
    }

    pub fn remember(&mut self, seed: (i32, i32), donor: (i32, i32)) {
        self.by_seed.insert(seed, donor);
    }

    pub fn forget(&mut self, seed: (i32, i32)) {
        self.by_seed.remove(&seed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_standing_row_clears_store() {
        let mut s = ConfinedStore::default();
        assert!(s.begin_wake(Some(8)));
        s.remember((1, 2), (3, 8));
        assert_eq!(s.len(), 1);
        assert!(s.begin_wake(Some(10)));
        assert!(s.is_empty());
        assert_eq!(s.max_stand_gy, Some(10));
    }

    #[test]
    fn lower_or_same_standing_row_keeps_store() {
        let mut s = ConfinedStore::default();
        assert!(s.begin_wake(Some(8)));
        s.remember((1, 2), (3, 8));
        assert!(s.begin_wake(Some(8)));
        assert_eq!(s.donor_of((1, 2)), Some((3, 8)));
        assert!(s.begin_wake(Some(6)));
        assert_eq!(s.donor_of((1, 2)), Some((3, 8)));
    }

    #[test]
    fn unset_band_does_not_persist() {
        let mut s = ConfinedStore::default();
        s.remember((0, 0), (1, 1));
        assert!(!s.begin_wake(None));
        assert!(s.is_empty());
    }
}
