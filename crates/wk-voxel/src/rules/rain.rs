//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Climatic rain injection and surface deposit helpers.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{Cell, Sat};
use crate::grid::World;

use super::util::hash_prob;

/// Rain source parameters for [`apply_rain`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct RainConfig {
    /// World-y row where droplets appear.
    pub top_y: i32,
    /// Inclusive `(x0, x1)` world-x range over which rain can fall.
    pub x_range: (i32, i32),
    /// Chance per column per tick of receiving a droplet.
    pub prob_per_col_per_tick: f32,
    /// Sat delta added per droplet (clamped so a cell can't exceed
    /// `u8::MAX`).
    pub droplet_sat: u8,
    /// Salt mixed into the per-column tick hash so callers can run
    /// multiple independent rain streams (mist vs storm) without
    /// them colliding.
    pub seed_salt: u64,
    /// When true (default), climatic rain drains humidity and cannot
    /// mint water. Pass a humidity map into [`apply_rain_with_temp`].
    pub closed_loop: bool,
    /// Sea level used with [`Self::max_flood_above_sea`] to refuse
    /// deepening an already-flooded column.
    pub sea_level_y: i32,
    /// Refuse climatic deposit when the free-water surface is more
    /// than this many cells above sea level (0 = no flood guard).
    pub max_flood_above_sea: i32,
}

impl Default for RainConfig {
    fn default() -> Self {
        Self {
            top_y: 0,
            x_range: (0, 0),
            prob_per_col_per_tick: 0.02,
            droplet_sat: 64,
            seed_salt: 0xC10D,
            closed_loop: true,
            sea_level_y: 0,
            max_flood_above_sea: 12,
        }
    }
}


/// Scenario / test injector: drop water on the ground / ocean surface
/// under each column. The play app does **not** call this — weather
/// there is condensation (`C`) plus evaporation (`E`).
///
/// Closed-loop by default ([`RainConfig::closed_loop`]): deposits only
/// what humidity can pay. Pass `humidity = None` with
/// `closed_loop = false` for the legacy open faucet (unit tests).
///
/// Determinism: same world.seed + same tick + same config = same
/// droplet placements.
pub fn apply_rain(world: &mut World, cfg: &RainConfig) {
    apply_rain_with_temp(world, cfg, None, None, None);
}

/// Climatic precip: snow when **air** at [`RainConfig::top_y`] is cold
/// (melts on warm ground — see [`crate::phase::deposit_precip_on_surface`]).
///
/// When `cfg.closed_loop` and `humidity` is provided, each droplet
/// drains atmospheric mass equal to what actually landed.
pub fn apply_rain_with_temp(
    world: &mut World,
    cfg: &RainConfig,
    temp: Option<&crate::temperature::Temperature>,
    phase: Option<&crate::phase::PhaseConfig>,
    mut humidity: Option<&mut crate::humidity::Humidity>,
) {
    let (x0, x1) = cfg.x_range;
    if x0 > x1 {
        return;
    }
    if cfg.closed_loop && humidity.is_none() {
        // Closed loop with no atmosphere handle — refuse to mint.
        return;
    }
    let seed = world.seed.0;
    let tick_no = world.tick;
    for gx in x0..=x1 {
        let roll = hash_prob(seed, gx, tick_no, cfg.seed_salt);
        if roll >= cfg.prob_per_col_per_tick {
            continue;
        }
        if cfg.max_flood_above_sea > 0
            && column_flooded_above_sea(world, gx, cfg.top_y, cfg.sea_level_y, cfg.max_flood_above_sea)
        {
            continue;
        }
        let want = cfg.droplet_sat as f32;
        let budget = if cfg.closed_loop {
            let Some(h) = humidity.as_deref() else {
                continue;
            };
            let avail = h.peek_near(gx, cfg.top_y);
            if avail < 1.0 {
                continue;
            }
            want.min(avail)
        } else {
            want
        };
        let landed = crate::phase::deposit_precip_on_surface(
            world,
            gx,
            cfg.top_y,
            budget,
            temp,
            phase,
        );
        if landed > 0.0 {
            if let Some(h) = humidity.as_deref_mut() {
                let _ = h.take_near(gx, cfg.top_y, landed);
            }
        }
    }
}

/// True when standing water already reaches more than `margin` cells
/// above sea level in this column (flood guard for climatic rain).
fn column_flooded_above_sea(
    world: &World,
    gx: i32,
    top_y: i32,
    sea_level_y: i32,
    margin: i32,
) -> bool {
    let limit = sea_level_y + margin;
    let jx = world.wrap_x(gx);
    let mut y = top_y;
    for _ in 0..160 {
        let Some(cell) = world.get_cell(jx, y) else {
            y -= 1;
            continue;
        };
        if cell.material != MaterialId::Air {
            return false;
        }
        if cell.sat.0 >= 200 && is_standing_water(world, jx, y) {
            return y > limit;
        }
        y -= 1;
        if y < sea_level_y - 4 {
            return false;
        }
    }
    false
}

/// True when wet Air is a standing pool / ocean film / land puddle
/// (rests on solid or on near-full water below) — not a mid-air droplet.
pub fn is_standing_water(world: &World, gx: i32, gy: i32) -> bool {
    let Some(cell) = world.get_cell(gx, gy) else {
        return false;
    };
    if cell.material != MaterialId::Air || cell.sat.is_empty() {
        return false;
    }
    match world.get_cell(gx, gy - 1) {
        Some(below) if below.material != MaterialId::Air => true,
        Some(below) => below.sat.0 >= 200,
        None => false,
    }
}

/// True when a full surface film at `(gx, gy)` can still spread or
/// cascade — rain must wait, or hills grow wedges. A dry lake bed of
/// full films has no outlet, so rain is allowed to stack and pond.
fn film_has_outlet(world: &World, gx: i32, gy: i32) -> bool {
    for dx in [-1_i32, 1] {
        let nx = world.wrap_x(gx + dx);
        match world.get_cell(nx, gy) {
            None => return true,
            Some(side) if side.material == MaterialId::Air && !side.sat.is_full() => {
                return true;
            }
            Some(side) if side.material == MaterialId::Air => {
                if matches!(
                    world.get_cell(nx, gy - 1),
                    Some(b) if b.material == MaterialId::Air && !b.sat.is_full()
                ) {
                    return true;
                }
            }
            _ => {}
        }
        match world.get_cell(nx, gy - 1) {
            None => return true,
            Some(c) if c.material == MaterialId::Air && !c.sat.is_full() => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Max cells to walk down from a precip origin. Demo sky is 5 chunks
/// tall (320); ceiling clouds must still reach sea-level lakes (~80).
/// A short 128-cell walk used to miss the ground entirely — rain kept
/// looking heavy while basins only evaporated.
const SURFACE_DEPOSIT_SCAN: i32 = 512;

/// Deposit atmospheric water onto the free-air surface under `start_y`.
///
/// Lands just above solid ground or standing water. Deepens existing
/// water columns. A full one-cell film on bare rock does **not** stack
/// into a hillside wedge while it can still spread or cascade — but a
/// closed basin of full films (dry lake) *does* pond, otherwise long
/// soaks rain forever and never refill.
/// Nucleate atmospheric water **where the vapour is**, as falling rain.
///
/// The counterpart to [`deposit_water_on_surface`], which scans up to 512 cells
/// down from the sky and lands water on the ground — rain that teleports rather
/// than falls. Here the droplet appears in the air cell that held the vapour and
/// gravity carries it down like any other water, so rain is a real thing in the
/// world with a position and a fall time instead of a deposit with an animation
/// drawn over it.
pub(crate) fn deposit_water_in_air(world: &mut World, gx: i32, y: i32, budget: f32) -> f32 {
    if budget <= 0.0 {
        return 0.0;
    }
    let jx = world.wrap_x(gx);
    let Some(air_y) = first_air_y_for_deposit(world, jx, y) else {
        return 0.0;
    };
    let Some(cell) = world.get_cell(jx, air_y) else {
        return 0.0;
    };
    fill_air_sat(world, jx, air_y, cell, budget)
}

/// Condensation samples the humidity tile centre. After slip, that centre is
/// often the last solid of a slope, with free air one cell up. Walk a short
/// column so rain still nucleates; stay buried and refuse.
fn first_air_y_for_deposit(world: &World, gx: i32, y: i32) -> Option<i32> {
    for dy in 0..=4 {
        let yy = y + dy;
        if world
            .get_cell(gx, yy)
            .is_some_and(|c| c.material == MaterialId::Air)
        {
            return Some(yy);
        }
    }
    None
}

pub(crate) fn deposit_water_on_surface(world: &mut World, gx: i32, start_y: i32, budget: f32) -> f32 {
    if budget <= 0.0 {
        return 0.0;
    }
    let jx = world.wrap_x(gx);
    let mut y = start_y;
    let mut last_free_air_y: Option<i32> = None;
    let floor_y = start_y - SURFACE_DEPOSIT_SCAN;
    while y >= floor_y {
        let Some(cell) = world.get_cell(jx, y) else {
            // Unloaded gap — forget the air seat so we never fill far
            // above the next solid/film we meet.
            last_free_air_y = None;
            y -= 1;
            continue;
        };
        if cell.material != MaterialId::Air {
            // Terrain — fill the open air we just left (directly above).
            // Do not spawn water above a solid pillar into empty sky.
            if let Some(ay) = last_free_air_y {
                if ay == y + 1 {
                    if let Some(ac) = world.get_cell(jx, ay) {
                        return fill_air_sat(world, jx, ay, ac, budget);
                    }
                }
            }
            return 0.0;
        }
        if cell.sat.is_full() {
            // Existing water column (wet over wet) may deepen upward.
            // A full film sitting on bare rock must not stack into a wedge.
            let below_is_water = matches!(
                world.get_cell(jx, y - 1),
                Some(b) if b.material == MaterialId::Air && b.sat.0 >= 200
            );
            if below_is_water {
                if let Some(ay) = last_free_air_y {
                    if ay == y + 1 {
                        if let Some(ac) = world.get_cell(jx, ay) {
                            return fill_air_sat(world, jx, ay, ac, budget);
                        }
                    }
                }
            }
            // Enclosed full films (dry lake / puddle with no outlet)
            // must be allowed to pond. Hillside films still wait.
            if !film_has_outlet(world, jx, y) {
                if let Some(ay) = last_free_air_y {
                    if ay == y + 1 {
                        if let Some(ac) = world.get_cell(jx, ay) {
                            return fill_air_sat(world, jx, ay, ac, budget);
                        }
                    }
                }
            }
            return 0.0;
        }
        last_free_air_y = Some(y);
        y -= 1;
    }
    0.0
}

fn fill_air_sat(world: &mut World, gx: i32, gy: i32, cell: Cell, budget: f32) -> f32 {
    let free = u8::MAX as f32 - cell.sat.0 as f32;
    let transfer = budget.min(free).max(0.0);
    let u = transfer.round() as i32;
    if u <= 0 {
        return 0.0;
    }
    let new_sat = (cell.sat.0 as i32 + u).clamp(0, u8::MAX as i32) as u8;
    world.set_cell(
        gx,
        gy,
        Cell {
            sat: Sat(new_sat),
            ..cell
        },
    );
    u as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::chunk::ChunkCoord;

    #[test]
    fn deposit_water_in_air_lands_on_first_air_above_solid() {
        let mut world = World::new(7);
        world.ensure_chunk(ChunkCoord::new(0, 0));
        world.set_cell(16, 16, Cell::solid(MaterialId::Stone));
        world.set_cell(16, 17, Cell::air());
        assert_eq!(deposit_water_in_air(&mut world, 16, 16, 40.0), 40.0);
        assert_eq!(
            world.get_cell(16, 16).unwrap().material,
            MaterialId::Stone
        );
        assert_eq!(world.get_cell(16, 17).unwrap().sat.0, 40);
    }

    #[test]
    fn deposit_water_in_air_refuses_a_fully_buried_column() {
        let mut world = World::new(8);
        world.ensure_chunk(ChunkCoord::new(0, 0));
        for y in 0..=20 {
            world.set_cell(16, y, Cell::solid(MaterialId::Stone));
        }
        assert_eq!(deposit_water_in_air(&mut world, 16, 16, 40.0), 0.0);
        assert_eq!(
            world.get_cell(16, 16).unwrap().material,
            MaterialId::Stone
        );
    }

    #[test]
    fn deposit_water_in_air_fills_empty_air() {
        let mut world = World::new(9);
        world.ensure_chunk(ChunkCoord::new(0, 0));
        world.set_cell(16, 20, Cell::air());
        assert_eq!(deposit_water_in_air(&mut world, 16, 20, 40.0), 40.0);
        assert_eq!(world.get_cell(16, 20).unwrap().sat.0, 40);
    }
}
