//! Interval mass budget for the diagnostic overlay.
//!
//! Not a heatmap. Snapshot water / mineral stores at a mark and show
//! deltas so a pretty cliff cannot impersonate physics. Closed-loop
//! rain/evap/freeze should keep `tracked` (cells + humidity + ice/snow)
//! near-flat; leftover after store-to-store moves is the unexplained term.

use wk_material::MaterialId;

use crate::audit::{mineral_total, sat_totals};
use crate::grid::World;
use crate::humidity::Humidity;
use crate::pipe::pipe_mass_sat;

/// Default sample cadence while the overlay is on (ticks).
pub const BUDGET_SAMPLE_PERIOD: u64 = 60;

/// `|Δ tracked|` above this is the unexplained flag (sat units).
pub const BUDGET_UNEXPLAINED_EPS: f64 = 16.0;

/// One inventory snapshot (water stores + mineral).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetSnap {
    pub tick: u64,
    pub free_air: i64,
    pub pore: i64,
    pub steam: i64,
    pub cave_h: i64,
    pub pipe: i64,
    pub humidity: f32,
    /// Ice cells × 255 (thaw yield). Not in [`crate::sat_totals`].
    pub ice: i64,
    /// Snow cells × 255 (thaw / melt yield).
    pub snow: i64,
    pub mineral_solid: i64,
    pub mineral_load: i64,
}

impl BudgetSnap {
    pub fn capture(world: &World, humidity: &Humidity) -> Self {
        let sat = sat_totals(world);
        let steam: i64 = world.steam.values().map(|&v| v as i64).sum();
        let cave_h: i64 = world.cave_humidity.values().map(|&v| v as i64).sum();
        let pipe = pipe_mass_sat(world);
        let load: i64 = world.dissolved.values().map(|&v| v as i64).sum();
        let mineral = mineral_total(world);
        let mut ice = 0i64;
        let mut snow = 0i64;
        let full = u8::MAX as i64;
        for chunk in world.chunks.values() {
            for cell in &chunk.cells {
                match cell.material {
                    MaterialId::Ice => ice += full,
                    MaterialId::Snow => snow += full,
                    _ => {}
                }
            }
        }
        Self {
            tick: world.tick,
            free_air: sat.free_air,
            pore: sat.pore,
            steam,
            cave_h,
            pipe,
            humidity: humidity.total_mass(),
            ice,
            snow,
            mineral_solid: mineral - load,
            mineral_load: load,
        }
    }

    /// Grid + books (same as [`crate::sat_totals`] `cell_total`).
    pub fn cell_total(self) -> i64 {
        self.free_air + self.pore + self.steam + self.cave_h + self.pipe
    }

    /// Frozen free-surface water (Ice + Snow thaw yield). Pore ice stays
    /// on `sat` and is already in [`Self::pore`].
    pub fn phase_water(self) -> i64 {
        self.ice + self.snow
    }

    /// Cell water + sky humidity + ice/snow. Closed-loop rain/evap/freeze
    /// should keep this near-flat; leftover is unexplained.
    pub fn tracked(self) -> f64 {
        self.cell_total() as f64 + f64::from(self.humidity) + self.phase_water() as f64
    }

    pub fn mineral_total(self) -> i64 {
        self.mineral_solid + self.mineral_load
    }

    pub fn delta(self, mark: Self) -> BudgetDelta {
        BudgetDelta {
            ticks: self.tick.saturating_sub(mark.tick),
            d_free: self.free_air - mark.free_air,
            d_pore: self.pore - mark.pore,
            d_steam: self.steam - mark.steam,
            d_cave: self.cave_h - mark.cave_h,
            d_pipe: self.pipe - mark.pipe,
            d_humidity: f64::from(self.humidity) - f64::from(mark.humidity),
            d_ice: self.ice - mark.ice,
            d_snow: self.snow - mark.snow,
            d_tracked: self.tracked() - mark.tracked(),
            d_min_solid: self.mineral_solid - mark.mineral_solid,
            d_min_load: self.mineral_load - mark.mineral_load,
            d_min_total: self.mineral_total() - mark.mineral_total(),
        }
    }
}

/// Store-to-store change since the mark. `d_tracked` is unexplained
/// (mint, cull, OOB drop, or a named sink we have not credited).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BudgetDelta {
    pub ticks: u64,
    pub d_free: i64,
    pub d_pore: i64,
    pub d_steam: i64,
    pub d_cave: i64,
    pub d_pipe: i64,
    pub d_humidity: f64,
    pub d_ice: i64,
    pub d_snow: i64,
    pub d_tracked: f64,
    pub d_min_solid: i64,
    pub d_min_load: i64,
    pub d_min_total: i64,
}

impl BudgetDelta {
    /// Free surface fell into pores (lake drawdown that is not a destroy).
    pub fn soak_like(self) -> bool {
        self.d_free < 0
            && self.d_pore > 0
            && self.d_pore >= 16
            && (self.d_free + self.d_pore).unsigned_abs() * 4 < self.d_pore.unsigned_abs()
    }

    /// Sky humidity rose while cell water fell — evap / mouth→H, not soak.
    pub fn evap_like(self) -> bool {
        self.d_humidity > BUDGET_UNEXPLAINED_EPS
            && self.d_free + self.d_pore < 0
            && (self.d_humidity + (self.d_free + self.d_pore) as f64).abs() * 4.0
                < self.d_humidity.abs()
    }

    /// Humidity spent on rain / snow (free or ice/snow rose).
    pub fn rain_like(self) -> bool {
        self.d_humidity < -BUDGET_UNEXPLAINED_EPS
            && self.d_free + self.d_ice + self.d_snow > 0
            && (self.d_humidity + (self.d_free + self.d_ice + self.d_snow) as f64).abs() * 4.0
                < self.d_humidity.abs()
    }

    /// Free surface became Ice/Snow (phase, not a destroy).
    pub fn freeze_like(self) -> bool {
        let phase = self.d_ice + self.d_snow;
        self.d_free < 0
            && phase > 16
            && (self.d_free + phase).unsigned_abs() * 4 < phase.unsigned_abs()
    }

    pub fn unexplained_water(self) -> bool {
        self.d_tracked.abs() > BUDGET_UNEXPLAINED_EPS
    }

    pub fn unexplained_mineral(self) -> bool {
        (self.d_min_total as f64).abs() > BUDGET_UNEXPLAINED_EPS
    }
}

/// Running mark / current pair for the overlay.
#[derive(Debug, Clone)]
pub struct BudgetLedger {
    pub period: u64,
    mark: Option<BudgetSnap>,
    now: Option<BudgetSnap>,
}

impl Default for BudgetLedger {
    fn default() -> Self {
        Self {
            period: BUDGET_SAMPLE_PERIOD,
            mark: None,
            now: None,
        }
    }
}

impl BudgetLedger {
    pub fn is_on(&self) -> bool {
        self.mark.is_some()
    }

    pub fn mark(&self) -> Option<BudgetSnap> {
        self.mark
    }

    pub fn now(&self) -> Option<BudgetSnap> {
        self.now
    }

    pub fn delta(&self) -> Option<BudgetDelta> {
        Some(self.now?.delta(self.mark?))
    }

    /// Turn on and pin the mark to this inventory.
    pub fn enable(&mut self, world: &World, humidity: &Humidity) {
        let s = BudgetSnap::capture(world, humidity);
        self.mark = Some(s);
        self.now = Some(s);
    }

    pub fn disable(&mut self) {
        self.mark = None;
        self.now = None;
    }

    pub fn remake_mark(&mut self) {
        if let Some(n) = self.now {
            self.mark = Some(n);
        }
    }

    /// Force a rescan while the overlay is on (F3 close, N remake).
    pub fn refresh(&mut self, world: &World, humidity: &Humidity) {
        if self.mark.is_some() {
            self.now = Some(BudgetSnap::capture(world, humidity));
        }
    }

    /// Rescan when enough ticks have passed (overlay on only).
    pub fn sample_if_due(&mut self, world: &World, humidity: &Humidity) {
        if self.mark.is_none() {
            return;
        }
        let due = match self.now {
            None => true,
            Some(n) => world.tick.saturating_sub(n.tick) >= self.period,
        };
        if due {
            self.now = Some(BudgetSnap::capture(world, humidity));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::grid::World;
    use crate::humidity::Humidity;
    use crate::rules::tick;
    use wk_material::MaterialId;

    fn empty_h() -> Humidity {
        Humidity::with_world_bounds(4, 0, 0, 32, 32)
    }

    #[test]
    fn snap_splits_free_and_pore() {
        let mut w = World::new(1);
        w.set_cell(0, 0, Cell::water());
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = 8;
        w.set_cell(1, 0, sand);
        w.set_cell(2, 0, Cell::solid(MaterialId::Ice));
        let h = empty_h();
        let s = BudgetSnap::capture(&w, &h);
        assert_eq!(s.free_air, u8::MAX as i64);
        assert_eq!(s.pore, 8);
        assert_eq!(s.ice, u8::MAX as i64);
        assert_eq!(s.cell_total(), s.free_air + s.pore);
        assert_eq!(s.tracked(), (s.cell_total() + s.ice) as f64);
        assert_eq!(s.mineral_load, 0);
    }

    #[test]
    fn gravity_soak_is_tracked_flat_and_soak_like() {
        let mut w = World::new(1);
        for x in 1..=3 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for y in 1..=4 {
            w.set_cell(1, y, Cell::solid(MaterialId::Bedrock));
            w.set_cell(3, y, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(2, 2, Cell::water());
        w.set_cell(2, 3, Cell::water());
        let h = empty_h();
        let mark = BudgetSnap::capture(&w, &h);
        for _ in 0..40 {
            tick(&mut w);
        }
        let now = BudgetSnap::capture(&w, &h);
        let d = now.delta(mark);
        assert!(
            d.d_pore > 0,
            "sand should take pore water (Δpore={})",
            d.d_pore
        );
        assert!(
            d.d_free < 0,
            "free surface should fall (Δfree={})",
            d.d_free
        );
        assert!(
            d.d_tracked.abs() < 1.0,
            "closed tick must not mint/destroy (tracked Δ={})",
            d.d_tracked
        );
        assert!(d.soak_like(), "free→pore should read as soak {d:?}");
        assert!(!d.evap_like());
        assert!(!d.unexplained_water());
    }

    #[test]
    fn heuristics_from_store_deltas() {
        let soak = BudgetDelta {
            d_free: -80,
            d_pore: 80,
            ..BudgetDelta::default()
        };
        assert!(soak.soak_like());
        assert!(!soak.evap_like());
        assert!(!soak.unexplained_water());

        let evap = BudgetDelta {
            d_free: -40,
            d_humidity: 40.0,
            ..BudgetDelta::default()
        };
        assert!(evap.evap_like());
        assert!(!evap.soak_like());

        let rain = BudgetDelta {
            d_free: 50,
            d_humidity: -50.0,
            ..BudgetDelta::default()
        };
        assert!(rain.rain_like());

        let freeze = BudgetDelta {
            d_free: -255,
            d_ice: 255,
            ..BudgetDelta::default()
        };
        assert!(freeze.freeze_like());
        assert!(!freeze.unexplained_water());
    }

    #[test]
    fn freeze_cell_stays_tracked_flat() {
        let mut w = World::new(1);
        w.set_cell(0, 0, Cell::water());
        let h = empty_h();
        let mark = BudgetSnap::capture(&w, &h);
        w.set_cell(0, 0, Cell::solid(MaterialId::Ice));
        let now = BudgetSnap::capture(&w, &h);
        let d = now.delta(mark);
        assert_eq!(d.d_free, -(u8::MAX as i64));
        assert_eq!(d.d_ice, u8::MAX as i64);
        assert!(d.d_tracked.abs() < 1.0);
        assert!(d.freeze_like());
        assert!(!d.unexplained_water());
    }

    #[test]
    fn unexplained_is_tracked_delta() {
        let mut w = World::new(1);
        w.set_cell(0, 0, Cell::water());
        let h = empty_h();
        let mark = BudgetSnap::capture(&w, &h);
        w.set_cell(0, 0, Cell::air());
        let now = BudgetSnap::capture(&w, &h);
        let d = now.delta(mark);
        assert_eq!(d.d_free, -(u8::MAX as i64));
        assert!(d.d_tracked < -200.0);
        assert!(d.unexplained_water());
        assert!(!d.soak_like());
    }

    #[test]
    fn ledger_enable_sample_remake() {
        let mut w = World::new(1);
        w.set_cell(0, 0, Cell::water());
        let h = empty_h();
        let mut led = BudgetLedger::default();
        assert!(!led.is_on());
        led.enable(&w, &h);
        assert!(led.is_on());
        w.set_cell(1, 0, Cell::water());
        w.tick = led.period;
        led.sample_if_due(&w, &h);
        let d = led.delta().unwrap();
        assert_eq!(d.d_free, u8::MAX as i64);
        led.remake_mark();
        assert_eq!(led.delta().unwrap().d_free, 0);
        w.set_cell(2, 0, Cell::water());
        led.refresh(&w, &h);
        assert_eq!(led.delta().unwrap().d_free, u8::MAX as i64);
        led.disable();
        assert!(!led.is_on());
    }
}
