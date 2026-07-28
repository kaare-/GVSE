//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Climatic rain injection and surface deposit helpers.

use wk_material::MaterialId;

use crate::cell::{Cell, Sat};
use crate::grid::World;

use super::util::hash_prob;

/// Rain source parameters for [`apply_rain`].
#[derive(Debug, Clone, Copy)]
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


/// Inject climatic rain that **lands on the ground / ocean surface**
/// under each column (cosmetic sky streaks are drawn separately).
///
/// Closed-loop by default ([`RainConfig::closed_loop`]): deposits only
/// what humidity can pay, so overnight `W` cannot mint a flood. Pass
/// `humidity = None` with `closed_loop = false` for the legacy open
/// faucet (unit tests).
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

/// Deposit atmospheric water onto the free-air surface under `start_y`.
///
/// Lands just above solid ground or standing water. Deepens existing
/// water columns, but will **not** grow a one-cell film on bare rock
/// into a tall slope wedge (returns 0 when that film is already full
/// so runoff can clear the hillside first).
pub(crate) fn deposit_water_on_surface(world: &mut World, gx: i32, start_y: i32, budget: f32) -> f32 {
    if budget <= 0.0 {
        return 0.0;
    }
    let jx = world.wrap_x(gx);
    let mut y = start_y;
    let mut last_free_air_y: Option<i32> = None;
    for _ in 0..128 {
        let Some(cell) = world.get_cell(jx, y) else {
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
                    if let Some(ac) = world.get_cell(jx, ay) {
                        return fill_air_sat(world, jx, ay, ac, budget);
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
