//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Mass inventory for the product question: no unexplained water drift.
//!
//! Cell sat is summed from the grid (free Air vs pore). Humidity is the
//! atmosphere store — include it via [`tracked_totals`] when auditing a
//! full atmosphere step. Cloud parcels are a visual echo and are not
//! counted. The default physics [`tick`](crate::tick) only moves cell sat,
//! so the in-tick debug check compares [`SatTotals::cell_total`].

use std::sync::atomic::{AtomicBool, Ordering};

use wk_material::MaterialId;

use crate::cell::water_capacity;
use crate::clouds::CloudStore;
use crate::grid::World;
use crate::humidity::Humidity;

/// Opt-in in-tick mass check (`debug_assertions` only).
///
/// Default **off** — full-world sums every tick are too expensive for
/// large benches. Enable with [`set_mass_audit_enabled`] or env
/// `GVSE_MASS_AUDIT=1`.
static MASS_AUDIT_ENABLED: AtomicBool = AtomicBool::new(false);
static ENV_LOADED: AtomicBool = AtomicBool::new(false);

/// Enable or disable the debug tick mass assert (process-wide).
pub fn set_mass_audit_enabled(on: bool) {
    ENV_LOADED.store(true, Ordering::Relaxed);
    MASS_AUDIT_ENABLED.store(on, Ordering::Relaxed);
}

/// Whether the debug tick mass assert will run.
///
/// On first call, reads `GVSE_MASS_AUDIT` (`1` / `true` / nonempty
/// enables; `0` / `false` leaves default off) unless already set via
/// [`set_mass_audit_enabled`].
pub fn mass_audit_enabled() -> bool {
    if !ENV_LOADED.swap(true, Ordering::Relaxed) {
        if let Some(v) = std::env::var_os("GVSE_MASS_AUDIT") {
            let on = v != "0" && v != "false" && v != "False" && !v.is_empty();
            MASS_AUDIT_ENABLED.store(on, Ordering::Relaxed);
        }
    }
    MASS_AUDIT_ENABLED.load(Ordering::Relaxed)
}

/// Allowed `|Δ cell_total|` across one physics [`tick`](crate::tick).
///
/// Gravity / flow / seepage / grain are designed exact-integer
/// conservative. Keep 0 until a documented sink appears inside tick.
pub const CELL_SAT_TICK_TOLERANCE: i64 = 0;

/// Water sat inventory from the cell grid (+ optional atmosphere).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SatTotals {
    /// `Air` cell `sat` sum (free standing / film water).
    pub free_air: i64,
    /// Porous solid `sat` sum (pore water).
    pub pore: i64,
    /// `free_air + pore`.
    pub cell_total: i64,
    /// [`Humidity::total_mass`] (same sat units as f32).
    pub humidity: f32,
    /// Always 0 in [`tracked_totals`] — parcels echo humidity.
    pub clouds: f32,
}

impl SatTotals {
    /// Cell sat + humidity + clouds (f64 for stable compare).
    pub fn tracked(self) -> f64 {
        self.cell_total as f64 + f64::from(self.humidity) + f64::from(self.clouds)
    }
}

/// Sum free-air and pore water over every loaded cell. Atmosphere fields
/// are left at 0 — use [`tracked_totals`] to include them.
///
/// # Example
///
/// Physics [`tick`](crate::tick) should conserve cell sat on a closed bed:
///
/// ```
/// use wk_material::MaterialId;
/// use wk_voxel::{sat_totals, tick, Cell, World};
///
/// let mut world = World::new(7);
/// for x in 0..16 {
///     world.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
/// }
/// world.set_cell(8, 4, Cell::water());
/// let before = sat_totals(&world).cell_total;
/// tick(&mut world);
/// assert_eq!(sat_totals(&world).cell_total, before);
/// ```
pub fn sat_totals(world: &World) -> SatTotals {
    let mut free_air = 0i64;
    let mut pore = 0i64;
    for chunk in world.chunks.values() {
        for cell in &chunk.cells {
            let s = cell.sat.0 as i64;
            if s == 0 {
                continue;
            }
            match cell.material {
                MaterialId::Air => free_air += s,
                m if water_capacity(m) > 0 => pore += s,
                _ => {
                    // Impermeable with nonzero sat should be rare; still
                    // count it as pore-like so drift is visible.
                    pore += s;
                }
            }
        }
    }
    SatTotals {
        free_air,
        pore,
        cell_total: free_air + pore,
        humidity: 0.0,
        clouds: 0.0,
    }
}

/// [`sat_totals`] plus humidity (parcels are not a second water store).
pub fn tracked_totals(world: &World, humidity: &Humidity, _clouds: &CloudStore) -> SatTotals {
    let mut t = sat_totals(world);
    t.humidity = humidity.total_mass();
    // Parcels are a visual echo of humidity — do not double-count.
    t.clouds = 0.0;
    t
}

/// Assert cell sat did not drift beyond [`CELL_SAT_TICK_TOLERANCE`].
#[inline]
pub fn assert_cell_sat_conserved(before: &SatTotals, after: &SatTotals, context: &str) {
    let delta = after.cell_total - before.cell_total;
    assert!(
        delta.abs() <= CELL_SAT_TICK_TOLERANCE,
        "mass audit {context}: cell_total {before} → {after} (Δ={delta}, tol={CELL_SAT_TICK_TOLERANCE})",
        before = before.cell_total,
        after = after.cell_total,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use wk_material::MaterialId;

    #[test]
    fn splits_free_air_and_pore() {
        let mut w = World::new(1);
        w.set_cell(0, 0, Cell::solid(MaterialId::Sand));
        // Pore: fill sand to capacity.
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = water_capacity(MaterialId::Sand);
        w.set_cell(1, 0, sand);
        w.set_cell(2, 0, Cell::water());
        let t = sat_totals(&w);
        assert_eq!(t.free_air, u8::MAX as i64);
        assert_eq!(t.pore, water_capacity(MaterialId::Sand) as i64);
        assert_eq!(t.cell_total, t.free_air + t.pore);
        assert_eq!(t.humidity, 0.0);
        assert_eq!(t.clouds, 0.0);
    }
}
