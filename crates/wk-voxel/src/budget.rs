//! Interval mass budget for the diagnostic overlay.
//!
//! Not a heatmap. Snapshot water / mineral stores at a mark and show
//! deltas so a pretty cliff cannot impersonate physics. Closed-loop
//! rain/evap/freeze should keep `tracked` (cells + humidity + ice/snow
//! + in-flight landscape sat) near-flat; leftover after store-to-store
//! moves is the unexplained term.

use std::cell::Cell as StdCell;

use wk_material::MaterialId;

use crate::audit::{mineral_total, sat_totals};
use crate::cell::Cell;
use crate::grid::World;
use crate::humidity::Humidity;
use crate::landscape_body::LandscapeBodyStore;
use crate::mineral::cell_mineral;
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
    /// Ice thaw yield ([`crate::cell::frozen_thaw_sat`]). Not in
    /// [`crate::sat_totals`] (Ice/Snow sat is skipped there).
    pub ice: i64,
    /// Snow thaw / melt yield (same banking as ice).
    pub snow: i64,
    pub mineral_solid: i64,
    pub mineral_load: i64,
    /// Sat in flying landscape entities (off-grid slabs).
    pub body_water: i64,
    /// Carbonate in those entities.
    pub body_mineral: i64,
}

impl BudgetSnap {
    pub fn capture(world: &World, humidity: &Humidity) -> Self {
        Self::capture_with(world, humidity, None)
    }

    /// [`Self::capture`] plus in-flight landscape slabs.
    pub fn capture_with(
        world: &World,
        humidity: &Humidity,
        landscape: Option<&LandscapeBodyStore>,
    ) -> Self {
        let sat = sat_totals(world);
        let steam: i64 = world.steam.values().map(|&v| v as i64).sum();
        let cave_h: i64 = world.cave_humidity.values().map(|&v| v as i64).sum();
        let pipe = pipe_mass_sat(world);
        let load: i64 = world.dissolved.values().map(|&v| v as i64).sum();
        let mineral = mineral_total(world);
        let (body_water, body_mineral) = landscape
            .map(LandscapeBodyStore::overlay_inventory)
            .unwrap_or((0, 0));
        let mut ice = 0i64;
        let mut snow = 0i64;
        for chunk in world.chunks.values() {
            for cell in &chunk.cells {
                match cell.material {
                    MaterialId::Ice => ice += crate::cell::frozen_thaw_sat(*cell) as i64,
                    MaterialId::Snow => snow += crate::cell::frozen_thaw_sat(*cell) as i64,
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
            body_water,
            body_mineral,
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

    /// Cell water + sky humidity + ice/snow + in-flight landscape sat.
    /// Closed-loop rain/evap/freeze should keep this near-flat; leftover is unexplained.
    pub fn tracked(self) -> f64 {
        self.cell_total() as f64
            + self.body_water as f64
            + f64::from(self.humidity)
            + self.phase_water() as f64
    }

    pub fn mineral_total(self) -> i64 {
        self.mineral_solid + self.mineral_load + self.body_mineral
    }

    pub fn delta(self, mark: Self) -> BudgetDelta {
        BudgetDelta {
            ticks: self.tick.saturating_sub(mark.tick),
            d_free: self.free_air - mark.free_air,
            d_pore: self.pore - mark.pore,
            d_steam: self.steam - mark.steam,
            d_cave: self.cave_h - mark.cave_h,
            d_pipe: self.pipe - mark.pipe,
            d_body: self.body_water - mark.body_water,
            d_humidity: f64::from(self.humidity) - f64::from(mark.humidity),
            d_ice: self.ice - mark.ice,
            d_snow: self.snow - mark.snow,
            d_tracked: self.tracked() - mark.tracked(),
            d_min_solid: self.mineral_solid - mark.mineral_solid,
            d_min_load: self.mineral_load - mark.mineral_load,
            d_min_body: self.body_mineral - mark.body_mineral,
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
    pub d_body: i64,
    pub d_humidity: f64,
    pub d_ice: i64,
    pub d_snow: i64,
    pub d_tracked: f64,
    pub d_min_solid: i64,
    pub d_min_load: i64,
    pub d_min_body: i64,
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
        self.enable_with(world, humidity, None);
    }

    pub fn enable_with(
        &mut self,
        world: &World,
        humidity: &Humidity,
        landscape: Option<&LandscapeBodyStore>,
    ) {
        let s = BudgetSnap::capture_with(world, humidity, landscape);
        self.mark = Some(s);
        self.now = Some(s);
        probe_reset();
        probe_set_on(true);
    }

    pub fn disable(&mut self) {
        self.mark = None;
        self.now = None;
        probe_set_on(false);
        probe_reset();
    }

    pub fn remake_mark(&mut self) {
        if let Some(n) = self.now {
            self.mark = Some(n);
        }
    }

    /// Force a rescan while the overlay is on (F3 close, N remake).
    pub fn refresh(&mut self, world: &World, humidity: &Humidity) {
        self.refresh_with(world, humidity, None);
    }

    pub fn refresh_with(
        &mut self,
        world: &World,
        humidity: &Humidity,
        landscape: Option<&LandscapeBodyStore>,
    ) {
        if self.mark.is_some() {
            self.now = Some(BudgetSnap::capture_with(world, humidity, landscape));
        }
    }

    /// Rescan when enough ticks have passed (overlay on only).
    pub fn sample_if_due(&mut self, world: &World, humidity: &Humidity) {
        self.sample_if_due_with(world, humidity, None);
    }

    pub fn sample_if_due_with(
        &mut self,
        world: &World,
        humidity: &Humidity,
        landscape: Option<&LandscapeBodyStore>,
    ) {
        if self.mark.is_none() {
            return;
        }
        let due = match self.now {
            None => true,
            Some(n) => world.tick.saturating_sub(n.tick) >= self.period,
        };
        if due {
            self.now = Some(BudgetSnap::capture_with(world, humidity, landscape));
        }
    }
}

/// Named drop / write probes while the B overlay is on.
///
/// Store deltas (`TRACKED`, `min.tot`) stay the leftover. These counters
/// say *which class of write* produced it so the next hunt is not a guess.
/// Gated on the overlay — play FPS is unchanged with `B` off.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BudgetProbe {
    /// `set_cell` sat/ice change when **material** changed (moves that
    /// keep the material are ignored — gravity / evap / seepage).
    pub water_swap: i64,
    /// `park_orphan_*` leftover the caller discarded.
    pub water_park: i64,
    /// Humidity `try_add` refused because the tile is outside bounds.
    pub water_hum_rej: i64,
    /// `Humidity::clamp_to_bounds` dropped vapour.
    pub water_clamp: i64,
    /// Cumulative `Humidity::total_mass` Δ across `advect_with_surface`
    /// (should be ~0; snow-onset mint hunt).
    pub water_hum_advect: f64,
    /// Cumulative `Humidity::total_mass` Δ across `diffuse` (should be ~0).
    pub water_hum_diffuse: f64,
    /// Evap path: cumulative humidity mass `try_add*` accepted (sat units).
    pub water_evap_add: i64,
    /// Evap path: cumulative free-air sat actually removed after try_add.
    pub water_evap_debit: i64,
    /// Evap path: sat removed from orphan-boosted surface films.
    pub water_orphan_rm: i64,
    /// Condensation liquid: free sat written by `deposit_water_in_air` / surface.
    pub water_dep_add: i64,
    /// Condensation liquid: humidity actually drained for that deposit.
    pub water_dep_debit: i64,
    /// `set_cell` carbonate delta **outside** widen / scour / precip / emit.
    pub mineral_bare: i64,
    /// Same delta **inside** those ledger APIs (should be paired with load).
    pub mineral_credit: i64,
    /// `add_dissolved` remainder after neighbour spill (clipped load).
    pub mineral_clip: i64,
}

impl BudgetProbe {
    pub fn snapshot() -> Self {
        PROBE.with(|p| p.get())
    }

    pub fn is_recording() -> bool {
        probe_on()
    }

    pub fn is_quiet(self) -> bool {
        self.water_swap == 0
            && self.water_park == 0
            && self.water_hum_rej == 0
            && self.water_clamp == 0
            && self.water_hum_advect.abs() < 1e-3
            && self.water_hum_diffuse.abs() < 1e-3
            && self.mineral_bare == 0
            && self.mineral_credit == 0
            && self.mineral_clip == 0
    }
}

thread_local! {
    static PROBE_ON: StdCell<bool> = const { StdCell::new(false) };
    static PROBE: StdCell<BudgetProbe> = const { StdCell::new(BudgetProbe {
        water_swap: 0,
        water_park: 0,
        water_hum_rej: 0,
        water_clamp: 0,
        water_hum_advect: 0.0,
        water_hum_diffuse: 0.0,
        water_evap_add: 0,
        water_evap_debit: 0,
        water_orphan_rm: 0,
        water_dep_add: 0,
        water_dep_debit: 0,
        mineral_bare: 0,
        mineral_credit: 0,
        mineral_clip: 0,
    }) };
    static MINERAL_SCOPE: StdCell<u32> = const { StdCell::new(0) };
}

/// Cumulative humidity mass change across one `advect_with_surface` call.
#[inline]
pub fn note_hum_advect_delta(delta: f32) {
    if !probe_on() || delta.abs() < 1e-9 {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.water_hum_advect += f64::from(delta);
        p.set(v);
    });
}

/// Cumulative humidity mass change across one `diffuse` call.
#[inline]
pub fn note_hum_diffuse_delta(delta: f32) {
    if !probe_on() || delta.abs() < 1e-9 {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.water_hum_diffuse += f64::from(delta);
        p.set(v);
    });
}

/// Evap → humidity `try_add*` accepted mass (integer sat units).
#[inline]
pub fn note_evap_hum_add(units: i32) {
    if units == 0 || !probe_on() {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.water_evap_add += i64::from(units);
        p.set(v);
    });
}

/// Free-air sat removed in the same evap apply step as [`note_evap_hum_add`].
#[inline]
pub fn note_evap_sat_debit(units: i32) {
    if units == 0 || !probe_on() {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.water_evap_debit += i64::from(units);
        p.set(v);
    });
}

/// Sat removed from an orphan-boosted surface film.
#[inline]
pub fn note_orphan_film_rm(units: i32) {
    if units == 0 || !probe_on() {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.water_orphan_rm += i64::from(units);
        p.set(v);
    });
}

/// Condensation liquid free-sat written (paired with [`note_dep_hum_debit`]).
#[inline]
pub fn note_dep_sat_add(units: i32) {
    if units == 0 || !probe_on() {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.water_dep_add += i64::from(units);
        p.set(v);
    });
}

/// Humidity drained for a condensation liquid deposit.
#[inline]
pub fn note_dep_hum_debit(units: i32) {
    if units == 0 || !probe_on() {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.water_dep_debit += i64::from(units);
        p.set(v);
    });
}

fn probe_set_on(on: bool) {
    PROBE_ON.with(|c| c.set(on));
}

fn probe_reset() {
    PROBE.with(|p| p.set(BudgetProbe::default()));
    MINERAL_SCOPE.with(|s| s.set(0));
}

#[inline]
fn probe_on() -> bool {
    PROBE_ON.with(|c| c.get())
}

fn overlay_water_units(cell: Cell) -> i64 {
    match cell.material {
        MaterialId::Ice | MaterialId::Snow => crate::cell::frozen_thaw_sat(cell) as i64,
        _ => cell.sat.0 as i64,
    }
}

/// Called from [`World::set_cell`](crate::grid::World::set_cell) when `B` is on.
#[inline]
pub fn note_set_cell(prev: Cell, next: Cell) {
    if !probe_on() {
        return;
    }
    if prev.material != next.material {
        let d = overlay_water_units(next) - overlay_water_units(prev);
        if d != 0 {
            PROBE.with(|p| {
                let mut v = p.get();
                v.water_swap += d;
                p.set(v);
            });
        }
    }
    let dm = cell_mineral(next) as i64 - cell_mineral(prev) as i64;
    if dm == 0 {
        return;
    }
    let credited = MINERAL_SCOPE.with(|s| s.get() > 0);
    PROBE.with(|p| {
        let mut v = p.get();
        if credited {
            v.mineral_credit += dm;
        } else {
            v.mineral_bare += dm;
        }
        p.set(v);
    });
}

/// Discarded `park_orphan` remainder — a real water drop.
pub fn note_unplaced_water(units: u32) {
    if units == 0 || !probe_on() {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.water_park += units as i64;
        p.set(v);
    });
}

pub fn note_water_hum_rej(mass: f32) {
    if mass <= 0.0 || !probe_on() {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.water_hum_rej += mass.round() as i64;
        p.set(v);
    });
}

pub fn note_water_clamp(mass: f32) {
    if mass <= 0.0 || !probe_on() {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.water_clamp += mass.round() as i64;
        p.set(v);
    });
}

pub fn note_mineral_clip(units: u16) {
    if units == 0 || !probe_on() {
        return;
    }
    PROBE.with(|p| {
        let mut v = p.get();
        v.mineral_clip += units as i64;
        p.set(v);
    });
}

/// RAII: `set_cell` carbonate deltas inside widen / scour / precip / emit
/// count as `mineral_credit`, not `mineral_bare`.
pub struct MineralLedgerScope;

impl MineralLedgerScope {
    pub fn enter() -> Self {
        MINERAL_SCOPE.with(|s| s.set(s.get().saturating_add(1)));
        Self
    }
}

impl Drop for MineralLedgerScope {
    fn drop(&mut self) {
        MINERAL_SCOPE.with(|s| s.set(s.get().saturating_sub(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::grid::World;
    use crate::humidity::Humidity;
    use crate::landscape_body::{LandscapeBody, LandscapeBodyStore};
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

    #[test]
    fn probe_ignores_writes_when_overlay_off() {
        let mut w = World::new(1);
        let mut lime = Cell::solid(MaterialId::Limestone);
        lime.pore = 40;
        w.set_cell(0, 0, lime);
        lime.pore = 128;
        w.set_cell(0, 0, lime);
        let p = BudgetProbe::snapshot();
        assert_eq!(p.mineral_bare, 0);
        assert!(p.is_quiet());
    }

    #[test]
    fn probe_tags_bare_limestone_pore_reset() {
        let mut w = World::new(1);
        let h = empty_h();
        let mut led = BudgetLedger::default();
        let mut lime = Cell::solid(MaterialId::Limestone);
        lime.pore = 40;
        w.set_cell(0, 0, lime);
        led.enable(&w, &h);
        lime.pore = 128;
        w.set_cell(0, 0, lime);
        let p = BudgetProbe::snapshot();
        assert_eq!(p.mineral_bare, (255 - 128) - (255 - 40));
        assert_eq!(p.mineral_credit, 0);
        led.disable();
        assert!(BudgetProbe::snapshot().is_quiet());
    }

    #[test]
    fn probe_credits_mineral_ledger_scope() {
        let mut w = World::new(1);
        let h = empty_h();
        let mut led = BudgetLedger::default();
        let mut lime = Cell::solid(MaterialId::Limestone);
        lime.pore = 40;
        w.set_cell(0, 0, lime);
        led.enable(&w, &h);
        {
            let _g = MineralLedgerScope::enter();
            lime.pore = 41;
            w.set_cell(0, 0, lime);
        }
        let p = BudgetProbe::snapshot();
        assert_eq!(p.mineral_bare, 0);
        assert_eq!(p.mineral_credit, -1);
    }

    #[test]
    fn probe_water_swap_on_material_change_not_same_mat_sat() {
        let mut w = World::new(1);
        let h = empty_h();
        let mut led = BudgetLedger::default();
        w.set_cell(0, 0, Cell::water());
        led.enable(&w, &h);
        let mut film = Cell::air();
        film.sat.0 = 200;
        w.set_cell(0, 0, film);
        assert_eq!(
            BudgetProbe::snapshot().water_swap,
            0,
            "Air sat drop is evap/flow, not a material swap"
        );
        w.set_cell(0, 0, Cell::solid(MaterialId::Stone));
        assert_eq!(
            BudgetProbe::snapshot().water_swap,
            -200,
            "Air→Stone must tag the sat that vanished with the material"
        );
    }

    #[test]
    fn snap_counts_in_flight_landscape_body() {
        let w = World::new(1);
        let h = empty_h();
        let mut lime = Cell::solid(MaterialId::Limestone);
        lime.pore = 40;
        lime.sat.0 = 12;
        let mut store = LandscapeBodyStore::new();
        store.bodies.push(LandscapeBody {
            id: 1,
            cells: vec![(0, 0, lime)],
            cargo: vec![],
            ox: 0,
            oy: 0,
            fall_streak: 0,
            stuck_ticks: 0,
        });
        let s = BudgetSnap::capture_with(&w, &h, Some(&store));
        assert_eq!(s.body_water, 12);
        assert_eq!(s.body_mineral, crate::mineral::cell_mineral(lime) as i64);
        assert_eq!(s.mineral_total(), s.body_mineral);
        assert_eq!(s.tracked(), 12.0);
        let grid_only = BudgetSnap::capture(&w, &h);
        assert_eq!(grid_only.body_mineral, 0);
        assert_eq!(grid_only.body_water, 0);
        assert_eq!(grid_only.mineral_total(), 0);
    }
}
