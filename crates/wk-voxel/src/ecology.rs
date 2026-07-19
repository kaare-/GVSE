//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse ecology — biomass overlay driven by cell material + sat.
//!
//! This is the first ecology slice for `wk-voxel` (migration §3.4):
//! no ECS agents, no gene modules, no column `Ecology` port. A sparse
//! per-cell biomass map grows on wet plantable surfaces (Sand / Clay /
//! Organic) and decays when dry. Callers run [`apply_ecology`] after
//! the fluid tick (same pattern as rain / evap in `wk-voxel-app`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::Cell;
use crate::grid::World;

/// Sparse alive biomass keyed by world cell `(gx, gy)`.
///
/// Keys are the **surface solid** cell where the stand is rooted.
/// Missing → 0. Units are arbitrary mass (same spirit as column-GVSE
/// `alive_biomass`, but `f32` so growth can be fractional).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Biomass {
    pub cells: HashMap<(i32, i32), f32>,
}

impl Biomass {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn at(&self, gx: i32, gy: i32) -> f32 {
        *self.cells.get(&(gx, gy)).unwrap_or(&0.0)
    }

    pub fn total_mass(&self) -> f32 {
        self.cells.values().copied().sum()
    }

    fn set(&mut self, gx: i32, gy: i32, mass: f32) {
        if mass <= 1e-4 {
            self.cells.remove(&(gx, gy));
        } else {
            self.cells.insert((gx, gy), mass);
        }
    }
}

/// Tunables for [`apply_ecology`].
#[derive(Debug, Clone, Copy)]
pub struct EcologyConfig {
    /// Pore / free-water sat that counts as "moist enough to grow".
    pub min_sat_to_grow: u8,
    /// Biomass added per tick on a fully moist, lit plantable cell.
    pub grow_per_tick: f32,
    /// Biomass removed per tick when the stand is dry or buried.
    pub decay_per_tick: f32,
    /// Soft cap per surface cell.
    pub max_biomass: f32,
}

impl Default for EcologyConfig {
    fn default() -> Self {
        Self {
            min_sat_to_grow: 40,
            grow_per_tick: 0.35,
            decay_per_tick: 0.08,
            max_biomass: 255.0,
        }
    }
}

fn is_plantable(material: MaterialId) -> bool {
    matches!(
        material,
        MaterialId::Sand | MaterialId::Clay | MaterialId::Organic
    )
}

fn is_solid(cell: Cell) -> bool {
    cell.material != MaterialId::Air
}

/// Moisture available to a surface cell: max of its own pore sat and
/// free water in the air cell immediately above (puddle / rain film).
fn surface_moisture(world: &World, gx: i32, gy: i32, surface: Cell) -> u8 {
    let mut m = surface.sat.0;
    if let Some(above) = world.get_cell(gx, gy + 1) {
        if above.material == MaterialId::Air {
            m = m.max(above.sat.0);
        }
    }
    m
}

/// Crude sky openness: true when a few air cells sit above the
/// surface before the next solid (or open sky / missing chunk).
fn has_sky_access(world: &World, gx: i32, surface_gy: i32, y_max: i32) -> bool {
    let mut air = 0u32;
    for gy in (surface_gy + 1)..y_max {
        match world.get_cell(gx, gy) {
            None => return true,
            Some(c) if c.material == MaterialId::Air => {
                air += 1;
                if air >= 2 {
                    return true;
                }
            }
            Some(_) => return air >= 2,
        }
    }
    air >= 2
}

/// Find the topmost solid cell in column `gx` within `[y_min, y_max)`.
fn find_surface_y(world: &World, gx: i32, y_min: i32, y_max: i32) -> Option<i32> {
    let mut y = y_max - 1;
    while y >= y_min {
        if let Some(c) = world.get_cell(gx, y) {
            if is_solid(c) {
                return Some(y);
            }
        }
        y -= 1;
    }
    None
}

/// One ecology pass over columns `[x_min, x_max)` and rows
/// `[y_min, y_max)`.
///
/// For each column: locate the top solid cell; if it is plantable and
/// moist with sky access, grow biomass there; otherwise decay any
/// existing stand on that column's previous surface key.
///
/// Biomass keys that are no longer the surface (erosion / burial) are
/// decayed when visited via the column's current surface lookup —
/// orphaned keys from vanished columns are pruned opportunistically
/// when their mass hits zero through decay on a re-visit. A light
/// sweep removes keys whose cell is no longer solid plantable.
pub fn apply_ecology(
    world: &World,
    biomass: &mut Biomass,
    cfg: &EcologyConfig,
    x_min: i32,
    x_max: i32,
    y_min: i32,
    y_max: i32,
) {
    if x_max <= x_min || y_max <= y_min {
        return;
    }

    // Track which surface cells we touched so orphans can decay.
    let mut live: HashMap<(i32, i32), ()> = HashMap::new();

    for gx in x_min..x_max {
        let gx = world.wrap_x(gx);
        let Some(gy) = find_surface_y(world, gx, y_min, y_max) else {
            continue;
        };
        let Some(surface) = world.get_cell(gx, gy) else {
            continue;
        };
        live.insert((gx, gy), ());

        let plantable = is_plantable(surface.material);
        let moist = surface_moisture(world, gx, gy, surface) >= cfg.min_sat_to_grow;
        let lit = has_sky_access(world, gx, gy, y_max);
        let mut mass = biomass.at(gx, gy);

        if plantable && moist && lit {
            let wetness = surface_moisture(world, gx, gy, surface) as f32 / 255.0;
            mass = (mass + cfg.grow_per_tick * wetness).min(cfg.max_biomass);
        } else if mass > 0.0 {
            mass -= cfg.decay_per_tick;
        }
        biomass.set(gx, gy, mass);
    }

    // Decay stands whose surface cell is gone / out of the scan
    // (landslide, dig, or column outside range this pass).
    let orphans: Vec<(i32, i32)> = biomass
        .cells
        .keys()
        .copied()
        .filter(|k| !live.contains_key(k))
        .collect();
    for (gx, gy) in orphans {
        let mass = biomass.at(gx, gy) - cfg.decay_per_tick;
        biomass.set(gx, gy, mass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Sat;
    use crate::chunk::ChunkCoord;

    fn strip_world() -> World {
        // Bedrock floor, sand surface at y=2, open air above.
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
            w.set_cell(x, 2, Cell::solid(MaterialId::Sand));
        }
        w
    }

    #[test]
    fn wet_sand_grows_biomass() {
        let mut w = strip_world();
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(200);
        w.set_cell(3, 2, sand);
        let mut bio = Biomass::new();
        let cfg = EcologyConfig::default();
        for _ in 0..40 {
            apply_ecology(&w, &mut bio, &cfg, 0, 8, 0, 16);
        }
        assert!(
            bio.at(3, 2) > 5.0,
            "expected growth on wet sand, got {}",
            bio.at(3, 2)
        );
        assert!(bio.total_mass() > 5.0);
    }

    #[test]
    fn dry_sand_does_not_grow() {
        let w = strip_world();
        let mut bio = Biomass::new();
        let cfg = EcologyConfig::default();
        for _ in 0..40 {
            apply_ecology(&w, &mut bio, &cfg, 0, 8, 0, 16);
        }
        assert_eq!(bio.at(3, 2), 0.0);
        assert_eq!(bio.total_mass(), 0.0);
    }

    #[test]
    fn stone_surface_never_grows() {
        let mut w = strip_world();
        w.set_cell(3, 2, Cell::solid(MaterialId::Stone));
        let mut wet = Cell::air();
        wet.sat = Sat(255);
        w.set_cell(3, 3, wet);
        let mut bio = Biomass::new();
        let cfg = EcologyConfig::default();
        for _ in 0..40 {
            apply_ecology(&w, &mut bio, &cfg, 0, 8, 0, 16);
        }
        assert_eq!(bio.at(3, 2), 0.0);
    }

    #[test]
    fn dry_stand_decays() {
        let w = strip_world();
        let mut bio = Biomass::new();
        bio.set(3, 2, 10.0);
        let cfg = EcologyConfig {
            decay_per_tick: 1.0,
            ..EcologyConfig::default()
        };
        for _ in 0..15 {
            apply_ecology(&w, &mut bio, &cfg, 0, 8, 0, 16);
        }
        assert_eq!(bio.at(3, 2), 0.0);
    }

    #[test]
    fn buried_stand_decays_as_orphan() {
        let mut w = strip_world();
        let mut bio = Biomass::new();
        bio.set(3, 2, 10.0);
        // Bury the old surface under stone — new surface is y=3.
        w.set_cell(3, 3, Cell::solid(MaterialId::Stone));
        let cfg = EcologyConfig {
            decay_per_tick: 2.0,
            ..EcologyConfig::default()
        };
        apply_ecology(&w, &mut bio, &cfg, 0, 8, 0, 16);
        assert!(
            bio.at(3, 2) < 10.0,
            "orphaned key must decay, got {}",
            bio.at(3, 2)
        );
    }
}
