//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Temperature-driven freeze / thaw of free water ↔ Ice (Snow later).
//!
//! Hard per-column budgets mirror the column-stack lessons
//! (`MAX_FROZEN_SURFACE_MASS_KG`, flash-freeze caps, ice-pump) so cold
//! snaps cannot mint ice towers or flood the world. Density settle
//! keeps liquid **under** ice so water briefly on ice cannot freeze
//! upward into a tower.

use wk_material::MaterialId;

use crate::cell::{Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::rules::is_standing_water;
use crate::temperature::Temperature;

/// Freeze / thaw knobs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseConfig {
    /// Free water freezes at or below this skin temperature (°C).
    /// Ice/Snow thaw when warmer than this.
    pub freeze_point_c: f32,
    /// Minimum Air sat before a free-surface cell may become Ice.
    /// Ignores mist / 1-sat films so we don't flash-freeze haze.
    pub min_sat_to_freeze: u8,
    /// Max free-surface cells converted to Ice **per column per tick**.
    pub max_freeze_cells_per_column_per_tick: u8,
    /// Max Ice/Snow cells converted back to water **per column per tick**.
    pub max_thaw_cells_per_column_per_tick: u8,
    /// Hard cap on Ice+Snow cells stacked in one column. Excess at the
    /// top is culled to empty Air (removed, not melted — melting would
    /// replace an ice tower with a water tower).
    pub max_ice_cells_per_column: u8,
    /// Only run when `world.tick % period_ticks == 0`.
    pub period_ticks: u64,
}

impl Default for PhaseConfig {
    fn default() -> Self {
        Self {
            freeze_point_c: 0.0,
            min_sat_to_freeze: 64,
            max_freeze_cells_per_column_per_tick: 1,
            max_thaw_cells_per_column_per_tick: 1,
            max_ice_cells_per_column: 12,
            period_ticks: 1,
        }
    }
}

/// Full phase pass: cull → settle water under ice → thaw → freeze.
pub fn apply_phase(world: &mut World, temp: &Temperature, cfg: &PhaseConfig) {
    let period = cfg.period_ticks.max(1);
    if world.tick % period != 0 {
        return;
    }
    let columns = column_xs(world);
    for gx in columns {
        cull_frozen_column(world, gx, cfg.max_ice_cells_per_column);
        settle_water_under_ice(world, gx);
        thaw_column(world, gx, temp, cfg);
        freeze_column_surface(world, gx, temp, cfg);
    }
}

/// Alias for [`apply_phase`] (milestone-1 name kept for call sites).
pub fn apply_freeze(world: &mut World, temp: &Temperature, cfg: &PhaseConfig) {
    apply_phase(world, temp, cfg);
}

fn column_xs(world: &World) -> Vec<i32> {
    let mut coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    let mut xs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for coord in coords {
        let x0 = coord.cx * CHUNK_CELLS_W as i32;
        for lx in 0..CHUNK_CELLS_W as i32 {
            let gx = world.wrap_x(x0 + lx);
            if seen.insert(gx) {
                xs.push(gx);
            }
        }
    }
    xs.sort_unstable();
    xs
}

fn y_bounds(world: &World) -> Option<(i32, i32)> {
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    for coord in world.chunks.keys() {
        let y0 = coord.cy * CHUNK_CELLS_H as i32;
        min_y = min_y.min(y0);
        max_y = max_y.max(y0 + CHUNK_CELLS_H as i32 - 1);
    }
    if min_y > max_y {
        None
    } else {
        Some((min_y, max_y))
    }
}

fn is_frozen_solid(mat: MaterialId) -> bool {
    matches!(mat, MaterialId::Ice | MaterialId::Snow)
}

fn is_wet_air(cell: Cell) -> bool {
    cell.material == MaterialId::Air && !cell.sat.is_empty()
}

fn ice_cell() -> Cell {
    Cell {
        material: MaterialId::Ice,
        sat: Sat::EMPTY,
        flags: Default::default(),
        _pad: 0,
    }
}

/// Count Ice+Snow in the column and remove excess from the top.
fn cull_frozen_column(world: &mut World, gx: i32, max_cells: u8) {
    let Some((y0, y1)) = y_bounds(world) else {
        return;
    };
    let max_cells = max_cells as usize;
    let mut frozen_ys: Vec<i32> = Vec::new();
    for y in y0..=y1 {
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        if is_frozen_solid(cell.material) {
            frozen_ys.push(y);
        }
    }
    if frozen_ys.len() <= max_cells {
        return;
    }
    // Highest Y first — peel the top of the tower.
    frozen_ys.sort_unstable_by(|a, b| b.cmp(a));
    let excess = frozen_ys.len() - max_cells;
    for &y in frozen_ys.iter().take(excess) {
        world.set_cell(gx, y, Cell::air());
    }
}

/// Ice/Snow float on water: if wet Air sits directly above a frozen
/// cell, swap so liquid sinks and ice rises. One bottom-up pass lets
/// water fall through a short ice stack in a single tick.
///
/// This is the voxel analogue of column `settle_by_density` — without
/// it, rain on ice freezes into towers (the classic ice pump).
fn settle_water_under_ice(world: &mut World, gx: i32) {
    let Some((y0, y1)) = y_bounds(world) else {
        return;
    };
    for y in y0..y1 {
        let Some(below) = world.get_cell(gx, y) else {
            continue;
        };
        let Some(above) = world.get_cell(gx, y + 1) else {
            continue;
        };
        if !is_frozen_solid(below.material) || !is_wet_air(above) {
            continue;
        }
        // Swap: water moves down, ice/snow moves up. Preserve both cells.
        world.set_cell(gx, y, above);
        world.set_cell(gx, y + 1, below);
    }
}

fn open_sky_above(world: &World, gx: i32, gy: i32) -> bool {
    match world.get_cell(gx, gy + 1) {
        None => true,
        Some(above) if above.material == MaterialId::Air && !above.sat.is_full() => true,
        _ => false,
    }
}

fn below_is_frozen(world: &World, gx: i32, gy: i32) -> bool {
    matches!(
        world.get_cell(gx, gy - 1),
        Some(b) if is_frozen_solid(b.material)
    )
}

fn freeze_column_surface(world: &mut World, gx: i32, temp: &Temperature, cfg: &PhaseConfig) {
    let Some((y0, y1)) = y_bounds(world) else {
        return;
    };
    let budget = cfg.max_ice_cells_per_column as usize;
    let mut frozen_count = 0usize;
    for y in y0..=y1 {
        if let Some(cell) = world.get_cell(gx, y) {
            if is_frozen_solid(cell.material) {
                frozen_count += 1;
            }
        }
    }
    if frozen_count >= budget {
        return;
    }

    let mut freezes_left = cfg.max_freeze_cells_per_column_per_tick.max(1) as i32;
    // Top-down: freeze the free surface first (classic lake skin).
    for y in (y0..=y1).rev() {
        if freezes_left <= 0 || frozen_count >= budget {
            break;
        }
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        if cell.material != MaterialId::Air || cell.sat.0 < cfg.min_sat_to_freeze {
            continue;
        }
        if !is_standing_water(world, gx, y) || !open_sky_above(world, gx, y) {
            continue;
        }
        // Refuse water-on-ice even after settle missed a cell this tick.
        if below_is_frozen(world, gx, y) {
            continue;
        }
        let t_c = temp.at_cell(gx, y);
        if t_c > cfg.freeze_point_c {
            continue;
        }
        world.set_cell(gx, y, ice_cell());
        freezes_left -= 1;
        frozen_count += 1;
    }
}

/// Melt exposed Ice/Snow into `Air + FULL` water when warm.
///
/// Top-down, rate-limited — a sudden warm snap cannot dump a whole
/// ice cliff into the basin in one tick (mass stays one cell at a time).
fn thaw_column(world: &mut World, gx: i32, temp: &Temperature, cfg: &PhaseConfig) {
    let Some((y0, y1)) = y_bounds(world) else {
        return;
    };
    let mut thaws_left = cfg.max_thaw_cells_per_column_per_tick.max(1) as i32;
    for y in (y0..=y1).rev() {
        if thaws_left <= 0 {
            break;
        }
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        if !is_frozen_solid(cell.material) {
            continue;
        }
        // Only thaw the top of a frozen stack (sky or non-frozen above)
        // so buried ice under colder caps isn't melted out of order.
        let top_of_stack = match world.get_cell(gx, y + 1) {
            None => true,
            Some(above) if !is_frozen_solid(above.material) => true,
            _ => false,
        };
        if !top_of_stack {
            continue;
        }
        let t_c = temp.at_cell(gx, y);
        if t_c <= cfg.freeze_point_c {
            continue;
        }
        // Whole-cell thaw → one full water cell. No fractional sat minting.
        world.set_cell(gx, y, Cell::water());
        thaws_left -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;
    use crate::temperature::Temperature;

    fn cold_temp(world_w: i32, world_h: i32, temp_c: f32) -> Temperature {
        let mut t = Temperature::with_world_bounds(
            4, 0, 0, world_w, world_h, 1, world_w, world_h / 2, false,
        );
        t.config.base_temp_c = temp_c;
        for v in t.cells.values_mut() {
            *v = temp_c;
        }
        t
    }

    fn pond_world() -> World {
        let mut w = World::new(7);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Bedrock));
        }
        // Contained pool.
        w.set_cell(2, 2, Cell::solid(MaterialId::Bedrock));
        w.set_cell(5, 2, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 3, Cell::solid(MaterialId::Bedrock));
        w.set_cell(5, 3, Cell::solid(MaterialId::Bedrock));
        for x in 3..=4 {
            w.set_cell(x, 2, Cell::water());
            w.set_cell(x, 3, Cell::water());
        }
        w
    }

    #[test]
    fn standing_water_freezes_when_cold() {
        let mut w = pond_world();
        let temp = cold_temp(16, 16, -5.0);
        let cfg = PhaseConfig::default();
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(w.get_cell(3, 3).unwrap().material, MaterialId::Ice);
        assert_eq!(w.get_cell(4, 3).unwrap().material, MaterialId::Ice);
        assert_eq!(w.get_cell(3, 2).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(3, 2).unwrap().sat.is_full());
    }

    #[test]
    fn warm_water_does_not_freeze() {
        let mut w = pond_world();
        let temp = cold_temp(16, 16, 8.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(w.get_cell(3, 3).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(3, 3).unwrap().sat.is_full());
    }

    #[test]
    fn thin_film_below_min_sat_does_not_freeze() {
        let mut w = pond_world();
        w.set_cell(
            3,
            3,
            Cell {
                material: MaterialId::Air,
                sat: Sat(16),
                flags: Default::default(),
                _pad: 0,
            },
        );
        let temp = cold_temp(16, 16, -8.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(w.get_cell(3, 3).unwrap().material, MaterialId::Air);
        assert_eq!(w.get_cell(3, 3).unwrap().sat.0, 16);
    }

    #[test]
    fn ice_skin_does_not_keep_freezing_water_under_it() {
        let mut w = pond_world();
        let temp = cold_temp(16, 16, -10.0);
        let cfg = PhaseConfig::default();
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(w.get_cell(3, 3).unwrap().material, MaterialId::Ice);
        assert!(w.get_cell(3, 2).unwrap().sat.is_full());
        w.tick = 1;
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(
            w.get_cell(3, 2).unwrap().material,
            MaterialId::Air,
            "water under ice must not flash-freeze into a solid column"
        );
        assert!(w.get_cell(3, 2).unwrap().sat.is_full());
    }

    #[test]
    fn ice_column_budget_culls_runaway_tower() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for y in 0..20 {
            w.set_cell(1, y, Cell::solid(MaterialId::Ice));
        }
        let temp = cold_temp(16, 32, -5.0);
        let cfg = PhaseConfig {
            max_ice_cells_per_column: 4,
            ..PhaseConfig::default()
        };
        apply_phase(&mut w, &temp, &cfg);
        let mut ice = 0;
        for y in 0..20 {
            if w.get_cell(1, y).unwrap().material == MaterialId::Ice {
                ice += 1;
            }
        }
        assert_eq!(ice, 4, "excess ice must be culled, not melted");
        assert_eq!(w.get_cell(1, 19).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(1, 19).unwrap().sat.is_empty());
    }

    #[test]
    fn freeze_does_not_create_mass_from_empty_air() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::air());
        let temp = cold_temp(16, 16, -20.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(w.get_cell(1, 1).unwrap().material, MaterialId::Air);
    }

    #[test]
    fn ice_thaws_to_water_when_warm() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::solid(MaterialId::Ice));
        let temp = cold_temp(16, 16, 6.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        let cell = w.get_cell(1, 1).unwrap();
        assert_eq!(cell.material, MaterialId::Air);
        assert!(cell.sat.is_full(), "thaw must yield a full water cell");
    }

    #[test]
    fn cold_ice_does_not_thaw() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::solid(MaterialId::Ice));
        let temp = cold_temp(16, 16, -3.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(w.get_cell(1, 1).unwrap().material, MaterialId::Ice);
    }

    #[test]
    fn thaw_rate_limits_one_cell_per_column() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::solid(MaterialId::Ice));
        w.set_cell(1, 2, Cell::solid(MaterialId::Ice));
        let temp = cold_temp(16, 16, 8.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        // Top melts first; buried ice waits.
        assert_eq!(w.get_cell(1, 2).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(1, 2).unwrap().sat.is_full());
        assert_eq!(w.get_cell(1, 1).unwrap().material, MaterialId::Ice);
        w.tick = 1;
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(w.get_cell(1, 1).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(1, 1).unwrap().sat.is_full());
    }

    #[test]
    fn settle_moves_water_under_ice() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::solid(MaterialId::Ice));
        w.set_cell(1, 2, Cell::water()); // wrongly on top of ice
        let temp = cold_temp(16, 16, -5.0); // cold — must not freeze into a taller tower
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert!(
            w.get_cell(1, 1).unwrap().material == MaterialId::Air
                && w.get_cell(1, 1).unwrap().sat.is_full(),
            "water must sink under ice"
        );
        assert_eq!(
            w.get_cell(1, 2).unwrap().material,
            MaterialId::Ice,
            "ice must float above water"
        );
    }

    #[test]
    fn settle_then_cold_does_not_build_ice_tower_from_water_on_ice() {
        // Classic pump: water lands on ice → if we froze it in place we'd
        // grow a tower. Settle first, then freeze only open-sky water
        // that is *not* sitting on ice.
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::solid(MaterialId::Ice));
        w.set_cell(1, 2, Cell::water());
        let temp = cold_temp(16, 16, -8.0);
        for tick in 0..5 {
            w.tick = tick;
            apply_phase(&mut w, &temp, &PhaseConfig::default());
        }
        let mut ice = 0;
        for y in 0..8 {
            if w.get_cell(1, y).map(|c| c.material == MaterialId::Ice) == Some(true) {
                ice += 1;
            }
        }
        assert_eq!(ice, 1, "must stay one ice cell, not a growing tower (ice={ice})");
    }
}
