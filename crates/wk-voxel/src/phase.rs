//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Temperature-driven freeze / thaw / snow precip / slush.
//!
//! Hard per-column budgets mirror the column-stack lessons
//! (`MAX_FROZEN_SURFACE_MASS_KG`, flash-freeze caps, ice-pump) so cold
//! snaps cannot mint ice towers or flood the world.
//!
//! Rain stays **on top of** ice as a water film (it does not density-swap
//! under the sheet — that lofted ice into the rain column). Water on ice
//! melts the sheet when warm or when a full cell of rain has ponded.
//! Ice/Snow with dry air below **fall** as solids ([`crate::rules::apply_grain_fall`]);
//! the unsupported break pass no longer turns empty-air gaps into water.
//!
//! Cold ice lids **thicken downward** one cell per tick (wet Air under
//! Ice/Snow) so lakes do not stay liquid under a 1-px skin, and peak
//! "ice castles" of trapped water freeze through instead of sitting at
//! −20 °C forever. The lagged thermal field (`Temperature::step` with
//! material heat capacity) softens climate snaps; organics will read
//! the same field.
//!
//! Snow on cold ground is a **solid pack** on top of the material — it
//! does not soak pores. Cold wet-sand / snow avalanches live in
//! [`crate::rules::apply_cold_avalanche`]; thin lake ice that cannot
//! carry the debris breaks here ([`PhaseConfig::enable_ice_load_break`]).

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{is_grain, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::rules::{deposit_water_on_surface, is_standing_water};
use crate::temperature::Temperature;
use crate::worldgen::continental_surface_y;

/// Freeze / thaw knobs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PhaseConfig {
    /// Free water freezes at or below this skin temperature (°C).
    /// Ice/Snow thaw when warmer than this.
    pub freeze_point_c: f32,
    /// Minimum Air sat before a free-surface cell may become Ice.
    /// Must be a (near-)full cell: thaw always yields `Air+FULL`, so
    /// freezing partial sat would mint water on the next thaw.
    /// Default `255` — only standing water / full under-lid cells freeze.
    pub min_sat_to_freeze: u8,
    /// Max free-surface cells converted to Ice **per column per tick**.
    pub max_freeze_cells_per_column_per_tick: u8,
    /// Max Ice/Snow cells converted back to water **per column per tick**.
    pub max_thaw_cells_per_column_per_tick: u8,
    /// Max snow↔water / water-on-ice melt interactions **per column per tick**.
    pub max_slush_cells_per_column_per_tick: u8,
    /// Max unsupported Ice/Snow cells that may break **per column per tick**.
    pub max_break_cells_per_column_per_tick: u8,
    /// Minimum precip budget (sat units) to place one Snow / frost Ice
    /// cell. Must be a **full cell** (`255`): thaw / slush / break always
    /// yield `Air+FULL`, so seating a solid from a 40–64 droplet minted
    /// ~200 sat into the basin on melt. Shortfall → hold (`0`).
    pub min_budget_to_snow: f32,
    /// Hard cap on Ice+Snow cells stacked in one column. Excess at the
    /// top is culled to empty Air (removed, not melted — melting would
    /// replace an ice tower with a water tower). Beyond the cap, cold
    /// precip is held (not dumped as pore-soaking rain).
    pub max_ice_cells_per_column: u8,
    /// Lateral search radius (columns) when seating new snow. Prefers
    /// thinner packs so peaks don't monopolize every flake.
    pub snow_spread_radius: i32,
    /// Soft blanket depth: new snow prefers columns with Ice+Snow at or
    /// below this before stacking taller spikes.
    pub snow_blanket_depth: u8,
    /// Max Ice+Snow from **condensation frost** (rime / glaze) on a
    /// column. Real frost is a thin coat — not snow towers. Default `1`.
    pub frost_coat_depth: u8,
    /// Lateral search radius (columns) when seating condensation frost.
    /// Independent of [`Self::snow_spread_radius`] so rime can stay local
    /// while cloud snow blankets widely. Default `3`.
    pub frost_spread_radius: i32,
    /// Master switch for the whole phase pass (`I` in the demo).
    pub enabled: bool,
    /// Convert standing water → Ice when cold.
    pub enable_freeze: bool,
    /// Melt exposed Ice/Snow when warm.
    pub enable_thaw: bool,
    /// Water-on-ice melt and snow-on-water slush.
    pub enable_slush: bool,
    /// Legacy: break Ice/Snow over non-empty but non-supporting Air
    /// (haze). Empty-air gaps are handled by frozen fall in `tick`.
    pub enable_break_unsupported: bool,
    /// Break thin lake ice that is carrying grain / snow / ice debris.
    /// Thick lids ([`Self::ice_carry_thickness`]) hold the load.
    pub enable_ice_load_break: bool,
    /// Contiguous Ice cells needed to carry overburden. Default 2 —
    /// a one-cell skin fails under sand/snow; a thickened lid holds.
    pub ice_carry_thickness: u8,
    /// Max ice cells broken under debris load **per column per tick**.
    pub max_load_break_cells_per_column_per_tick: u8,
    /// Cold wet-sand / hillside-ice / snow spill onto ice (app wires
    /// [`crate::rules::apply_cold_avalanche`] when this is on).
    pub enable_cold_avalanche: bool,
    /// Cull Ice+Snow stacks taller than [`Self::max_ice_cells_per_column`].
    pub enable_cull: bool,
    /// Cold precip settles as Snow (when off, cold columns get liquid rain).
    pub enable_snow_precip: bool,
    /// Only run when `world.tick % period_ticks == 0`.
    /// Default `4` — ice still tracks weather, without a full-world
    /// column walk every physics tick.
    pub period_ticks: u64,
}

impl Default for PhaseConfig {
    fn default() -> Self {
        Self {
            freeze_point_c: 0.0,
            min_sat_to_freeze: 255,
            max_freeze_cells_per_column_per_tick: 1,
            max_thaw_cells_per_column_per_tick: 1,
            max_slush_cells_per_column_per_tick: 1,
            max_break_cells_per_column_per_tick: 2,
            min_budget_to_snow: 255.0,
            max_ice_cells_per_column: 12,
            snow_spread_radius: 6,
            snow_blanket_depth: 2,
            frost_coat_depth: 1,
            frost_spread_radius: 3,
            enabled: true,
            enable_freeze: true,
            enable_thaw: true,
            enable_slush: true,
            enable_break_unsupported: true,
            enable_ice_load_break: true,
            ice_carry_thickness: 2,
            max_load_break_cells_per_column_per_tick: 2,
            enable_cold_avalanche: true,
            enable_cull: true,
            enable_snow_precip: true,
            period_ticks: 4,
        }
    }
}

/// Full phase pass: cull → break unsupported → break overloaded thin ice →
/// water-on-ice / slush → thaw → freeze.
pub fn apply_phase(world: &mut World, temp: &Temperature, cfg: &PhaseConfig) {
    if !cfg.enabled {
        return;
    }
    let period = cfg.period_ticks.max(1);
    if world.tick % period != 0 {
        return;
    }
    let columns = column_xs(world);
    for gx in columns {
        if !column_may_phase(world, gx, temp, cfg) {
            continue;
        }
        if cfg.enable_cull {
            cull_frozen_column(world, gx, cfg.max_ice_cells_per_column);
        }
        if cfg.enable_break_unsupported {
            break_unsupported_frozen(world, gx, cfg);
        }
        if cfg.enable_ice_load_break {
            break_overloaded_ice(world, gx, cfg);
        }
        if cfg.enable_slush {
            water_on_ice_and_slush(world, gx, temp, cfg);
        }
        if cfg.enable_thaw {
            thaw_column(world, gx, temp, cfg);
        }
        if cfg.enable_freeze {
            freeze_column_surface(world, gx, temp, cfg);
        }
    }
}

/// Cheap column gate: skip warm dry columns with no ice/snow near the
/// free surface. Cold wet columns and any frozen band still run.
fn column_may_phase(world: &World, gx: i32, temp: &Temperature, cfg: &PhaseConfig) -> bool {
    let Some((y0, y1)) = y_bounds(world) else {
        return false;
    };
    // Drop through empty sky to the first non-empty cell, then peek a
    // short band — avoids full-height walks on tropical daytime land.
    // Start near rock∪sea (+margin), not world y_hi — Super-Server stress
    // paid ~4 ms walking empty sky from the ceiling.
    const BAND: i32 = 12;
    const SKY_SLACK: i32 = 32;
    let rock = continental_surface_y(temp.seed, gx, temp.sea_level_y, temp.width_cols);
    let start = rock
        .max(temp.sea_level_y)
        .saturating_add(SKY_SLACK)
        .min(y1)
        .max(y0);
    for y in (y0..=start).rev() {
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        if cell.material == MaterialId::Air && cell.sat.is_empty() {
            continue;
        }
        let mut has_frozen = is_frozen_solid(cell.material);
        let mut has_freezable = cell.material == MaterialId::Air
            && cell.sat.0 >= cfg.min_sat_to_freeze;
        let band_lo = (y - BAND + 1).max(y0);
        for yy in band_lo..y {
            let Some(c) = world.get_cell(gx, yy) else {
                continue;
            };
            if is_frozen_solid(c.material) {
                has_frozen = true;
            }
            if c.material == MaterialId::Air && c.sat.0 >= cfg.min_sat_to_freeze {
                has_freezable = true;
            }
            if has_frozen && has_freezable {
                break;
            }
        }
        if has_frozen {
            return true;
        }
        if !has_freezable {
            return false;
        }
        let t_c = temp.at_cell(gx, y);
        return t_c <= cfg.freeze_point_c + 1.5;
    }
    false
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
        pore: 128,
    }
}

fn snow_cell() -> Cell {
    Cell {
        material: MaterialId::Snow,
        sat: Sat::EMPTY,
        flags: Default::default(),
        _pad: 0,
        pore: 128,
    }
}

fn frozen_count_in_column(world: &World, gx: i32) -> usize {
    let Some((y0, y1)) = y_bounds(world) else {
        return 0;
    };
    let mut n = 0usize;
    for y in y0..=y1 {
        if let Some(cell) = world.get_cell(gx, y) {
            if is_frozen_solid(cell.material) {
                n += 1;
            }
        }
    }
    n
}

/// Y used to sample temperature for precip phase — skips Ice/Snow so a
/// growing pack cannot make the column read colder (column elev feedback).
fn ground_sample_y(world: &World, gx: i32) -> i32 {
    let Some((y0, y1)) = y_bounds(world) else {
        return 0;
    };
    for y in (y0..=y1).rev() {
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        if is_frozen_solid(cell.material) {
            continue;
        }
        if cell.material != MaterialId::Air {
            return y;
        }
        if is_standing_water(world, gx, y) {
            return y;
        }
    }
    y0
}

/// Deposit precip using **air temperature at `start_y`** (cloud / sky
/// origin) to choose flake vs drop, then **ground contact** to melt:
///
/// | air ≤ freeze | ground ≤ freeze | result |
/// |-------------:|----------------:|--------|
/// | yes | yes | snow pack |
/// | yes | no | melts → liquid |
/// | no | either | liquid (phase may freeze ponds later) |
///
/// **Cold-air snow that survives never soaks** pores: short budget, full
/// frozen cap, or no seat → `0` (caller keeps the mass).
///
/// When `temp` / `phase` are `None`, always rains (test / warm paths).
pub fn deposit_precip_on_surface(
    world: &mut World,
    gx: i32,
    start_y: i32,
    budget: f32,
    temp: Option<&Temperature>,
    phase: Option<&PhaseConfig>,
) -> f32 {
    let (Some(temp), Some(phase)) = (temp, phase) else {
        return deposit_water_on_surface(world, gx, start_y, budget);
    };
    if !phase.enable_snow_precip {
        return deposit_water_on_surface(world, gx, start_y, budget);
    }
    // Form phase from air at the precip origin (cloud / sky), not ground.
    let air_t = temp.at_cell(gx, start_y);
    if air_t > phase.freeze_point_c {
        return deposit_water_on_surface(world, gx, start_y, budget);
    }
    // Snow aloft — melt on warm ground contact.
    let ground_y = ground_sample_y(world, gx);
    let ground_t = temp.at_cell(gx, ground_y);
    if ground_t > phase.freeze_point_c {
        return deposit_water_on_surface(world, gx, start_y, budget);
    }
    // Cold air + cold ground: solid snow pack only — full cell or hold.
    let need = phase.min_budget_to_snow.max(u8::MAX as f32);
    if budget < need {
        return 0.0;
    }
    deposit_snow_spread(world, gx, start_y, temp, phase).unwrap_or(0.0)
}

/// Condensation drizzle deposit: liquid rain when warm; a **thin Ice
/// glaze** (frost / rime) when cold air hits cold ground.
///
/// Unlike cloud snow, this never places `Snow` and never stacks past
/// [`PhaseConfig::frost_coat_depth`] (default one cell). Once the coat
/// is on, further vapor is held in the humidity tile.
pub fn deposit_condensate_on_surface(
    world: &mut World,
    gx: i32,
    start_y: i32,
    budget: f32,
    temp: Option<&Temperature>,
    phase: Option<&PhaseConfig>,
) -> f32 {
    let (Some(temp), Some(phase)) = (temp, phase) else {
        return deposit_water_on_surface(world, gx, start_y, budget);
    };
    if budget <= 0.0 {
        return 0.0;
    }
    let air_t = temp.at_cell(gx, start_y);
    let ground_y = ground_sample_y(world, gx);
    let ground_t = temp.at_cell(gx, ground_y);
    // Warm air or warm ground → liquid (phase may freeze ponds later).
    if air_t > phase.freeze_point_c || ground_t > phase.freeze_point_c {
        return deposit_water_on_surface(world, gx, start_y, budget);
    }
    // Cold frost glaze — one full Ice cell, paid in full (thaw → FULL).
    let need = phase.min_budget_to_snow.max(u8::MAX as f32);
    if budget < need {
        return 0.0;
    }
    match deposit_frost_coat(world, gx, start_y, temp, phase) {
        Some(consumed) => consumed,
        None => 0.0,
    }
}

/// Seat a thin Ice glaze on bare cold ground. Prefers neighbours that
/// still lack a coat so hills get a sheet, not a spike under the tile.
fn deposit_frost_coat(
    world: &mut World,
    gx: i32,
    start_y: i32,
    temp: &Temperature,
    phase: &PhaseConfig,
) -> Option<f32> {
    let max_coat = phase.frost_coat_depth.max(1) as usize;
    let radius = phase.frost_spread_radius.max(0);
    let mut candidates: Vec<(i32, usize, i32)> = Vec::with_capacity((radius * 2 + 1) as usize);
    for dx in -radius..=radius {
        let cx = world.wrap_x(gx + dx);
        let sample_y = ground_sample_y(world, cx);
        if temp.at_cell(cx, sample_y) > phase.freeze_point_c {
            continue;
        }
        let pack = frozen_count_in_column(world, cx);
        if pack >= max_coat {
            continue;
        }
        candidates.push((cx, pack, dx.abs()));
    }
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by_key(|&(_, pack, dist)| (pack, dist));
    for &(cx, _, _) in &candidates {
        if let Some(consumed) = deposit_ice_on_surface(world, cx, start_y) {
            return Some(consumed);
        }
    }
    None
}

/// True when precip from air height `air_y` should draw/fall as snow
/// (cold air and snow precip enabled). Contact melt is a deposit-time
/// concern — visuals use air only so streaks match the cloud.
pub fn precip_forms_snow_at_air(
    temp: &Temperature,
    gx: i32,
    air_y: i32,
    phase: &PhaseConfig,
) -> bool {
    phase.enable_snow_precip && temp.at_cell(gx, air_y) <= phase.freeze_point_c
}

/// Seat snow on the aim column or a colder neighbour with a thinner pack
/// so cover spreads across the landscape instead of peak spikes.
fn deposit_snow_spread(
    world: &mut World,
    gx: i32,
    start_y: i32,
    temp: &Temperature,
    phase: &PhaseConfig,
) -> Option<f32> {
    let radius = phase.snow_spread_radius.max(0);
    let blanket = phase.snow_blanket_depth as usize;
    let hard = phase.max_ice_cells_per_column as usize;
    let mut candidates: Vec<(i32, usize, i32)> = Vec::with_capacity((radius * 2 + 1) as usize);
    for dx in -radius..=radius {
        let cx = world.wrap_x(gx + dx);
        let sample_y = ground_sample_y(world, cx);
        if temp.at_cell(cx, sample_y) > phase.freeze_point_c {
            continue;
        }
        let pack = frozen_count_in_column(world, cx);
        if pack >= hard {
            continue;
        }
        candidates.push((cx, pack, dx.abs()));
    }
    if candidates.is_empty() {
        return None;
    }
    // Thinnest pack first, then closest to the aim column.
    candidates.sort_by_key(|&(_, pack, dist)| (pack, dist));
    for &(cx, pack, _) in &candidates {
        if pack > blanket {
            continue;
        }
        if let Some(consumed) = deposit_snow_on_surface(world, cx, start_y) {
            return Some(consumed);
        }
    }
    // Hold mass once the soft blanket is full — do not grow 1-wide
    // spikes up to `max_ice_cells_per_column` away from the storm.
    None
}

fn rests_on_solid_or_pack(world: &World, gx: i32, gy: i32) -> bool {
    match world.get_cell(gx, gy - 1) {
        Some(below) if below.material != MaterialId::Air => true,
        Some(below) if is_frozen_solid(below.material) => true,
        _ => false,
    }
}

/// Place one Snow cell on the free surface under `start_y`.
/// Returns sat-equivalent mass consumed (`255`) or `None` if no seat.
///
/// Snow is a solid lid on rock / sand / pack. A wet Air film on solid
/// ground becomes Snow (it is not pushed into pores). Deep water gets
/// snow seated in the empty Air above the free surface.
///
/// Plant shoot modules (Stem / Nucleus / leaf) are draw overlays — they do
/// not lift this seat, so snow piles on the ground, not on the canopy.
/// Soft blanket depth caps column spikes. Leaf frost tint is a future
/// overlay animation, not world Snow on Photosystem cells.
fn deposit_snow_on_surface(world: &mut World, gx: i32, start_y: i32) -> Option<f32> {
    deposit_frozen_lid_on_surface(world, gx, start_y, snow_cell())
}

/// Place one Ice cell as a surface glaze (condensation frost / rime).
fn deposit_ice_on_surface(world: &mut World, gx: i32, start_y: i32) -> Option<f32> {
    deposit_frozen_lid_on_surface(world, gx, start_y, ice_cell())
}

fn deposit_frozen_lid_on_surface(
    world: &mut World,
    gx: i32,
    start_y: i32,
    lid: Cell,
) -> Option<f32> {
    let jx = world.wrap_x(gx);
    let mut y = start_y;
    let mut last_empty_air_y: Option<i32> = None;
    for _ in 0..512 {
        let Some(cell) = world.get_cell(jx, y) else {
            last_empty_air_y = None;
            y -= 1;
            continue;
        };
        if cell.material != MaterialId::Air {
            // Solid / pack — seat in the Air cell directly above (film ok).
            if let Some(above) = world.get_cell(jx, y + 1) {
                if above.material == MaterialId::Air {
                    world.set_cell(jx, y + 1, lid);
                    return Some(u8::MAX as f32);
                }
            }
            return None;
        }
        if !cell.sat.is_empty() {
            // Puddle on solid / pack → become frozen lid (no soak).
            if rests_on_solid_or_pack(world, jx, y) {
                world.set_cell(jx, y, lid);
                return Some(u8::MAX as f32);
            }
            // Standing water body — seat lid in empty air above.
            if let Some(ay) = last_empty_air_y {
                if ay == y + 1 {
                    world.set_cell(jx, ay, lid);
                    return Some(u8::MAX as f32);
                }
            }
            return None;
        }
        last_empty_air_y = Some(y);
        y -= 1;
    }
    None
}

/// Water film on Ice/Snow, and Snow sitting on water.
///
/// - **Water on ice/snow:** stays on top (no density swap under the sheet).
///   Melts the frozen cell when **warm** only — cold ponded rain must not
///   melt ice (that churned melt→refreeze towers).
/// - **Snow on water:** warm → melt snow; cold → freeze **full** water
///   under the snow into ice (snow-on-ice pack).
fn water_on_ice_and_slush(world: &mut World, gx: i32, temp: &Temperature, cfg: &PhaseConfig) {
    let Some((y0, y1)) = y_bounds(world) else {
        return;
    };
    let mut left = cfg.max_slush_cells_per_column_per_tick.max(1) as i32;
    let sample_y = ground_sample_y(world, gx);
    let t_c = temp.at_cell(gx, sample_y);
    let warm = t_c > cfg.freeze_point_c;

    for y in (y0..=y1).rev() {
        if left <= 0 {
            break;
        }
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };

        // Water film directly above ice → melt the sheet from above when
        // warm. Cold full rain must NOT melt ice (that churned melt→freeze
        // towers and looked like minted ice pillars).
        if cell.material == MaterialId::Ice {
            if let Some(above) = world.get_cell(gx, y + 1) {
                if is_wet_air(above) {
                    if warm {
                        world.set_cell(gx, y, Cell::water());
                        left -= 1;
                    }
                    continue;
                }
            }
        }

        // Snow sitting on water (snow above, wet below).
        if cell.material == MaterialId::Snow {
            let Some(below) = world.get_cell(gx, y - 1) else {
                continue;
            };
            if !is_wet_air(below) {
                // Water film on snow — melt snow from above when warm only.
                if let Some(above) = world.get_cell(gx, y + 1) {
                    if is_wet_air(above) && warm {
                        world.set_cell(gx, y, Cell::water());
                        left -= 1;
                    }
                }
                continue;
            }
            if warm {
                world.set_cell(gx, y, Cell::water());
                left -= 1;
            } else if below.sat.0 >= cfg.min_sat_to_freeze {
                // Full water under snow → ice (conversion, not mint).
                world.set_cell(gx, y - 1, ice_cell());
                left -= 1;
            }
        }
    }
}

/// Ice/Snow must rest on solid, standing water, or more frozen pack.
///
/// **Air below** (empty *or* haze short of [`PhaseConfig::min_sat_to_freeze`])
/// is owned by [`crate::rules::apply_grain_fall`] — the pack drops through
/// misty gaps instead of melting. Melting haze seats used to fight freeze
/// at the free surface and pump a flake ±1 cell every phase period.
fn break_unsupported_frozen(world: &mut World, gx: i32, cfg: &PhaseConfig) {
    let Some((y0, y1)) = y_bounds(world) else {
        return;
    };
    let mut left = cfg.max_break_cells_per_column_per_tick.max(1) as i32;
    // Bottom-up so a floating stack collapses from the underside first.
    for y in y0..=y1 {
        if left <= 0 {
            break;
        }
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        if !is_frozen_solid(cell.material) {
            continue;
        }
        if frozen_is_supported(world, gx, y, cfg) {
            continue;
        }
        // Empty / haze gap → fall in tick, do not melt into water mid-air.
        if matches!(
            world.get_cell(gx, y - 1),
            Some(b) if b.material == MaterialId::Air && b.sat.0 < cfg.min_sat_to_freeze
        ) {
            continue;
        }
        world.set_cell(gx, y, Cell::water());
        left -= 1;
    }
}

fn frozen_is_supported(world: &World, gx: i32, gy: i32, cfg: &PhaseConfig) -> bool {
    match world.get_cell(gx, gy - 1) {
        None => false,
        Some(below) if is_frozen_solid(below.material) => true,
        Some(below) if below.material != MaterialId::Air => true,
        Some(below) if below.sat.0 >= cfg.min_sat_to_freeze => true,
        _ => false,
    }
}

/// Contiguous `Ice` cells from `gy` downward. 0 if `gy` is not Ice.
pub fn ice_lid_thickness(world: &World, gx: i32, gy: i32) -> u8 {
    let mut n = 0u8;
    let mut y = gy;
    loop {
        match world.get_cell(gx, y) {
            Some(c) if c.material == MaterialId::Ice => {
                n = n.saturating_add(1);
                y -= 1;
            }
            _ => break,
        }
    }
    n
}

fn is_debris_load(material: MaterialId) -> bool {
    // Grain packs and snow blankets. Ice-on-ice is the lid itself
    // (or glaze that merged into it) — counting it as load would make
    // every multi-cell sheet break from the bottom up.
    is_grain(material) || material == MaterialId::Snow
}

/// Thin ice under grain / snow / ice debris fails; thick lids carry it.
///
/// Runs after unsupported-break so floating skins are already gone.
/// Top-down so the contact cell under the load opens first and the
/// debris can fall into the basin on the next gravity pass.
fn break_overloaded_ice(world: &mut World, gx: i32, cfg: &PhaseConfig) {
    let Some((y0, y1)) = y_bounds(world) else {
        return;
    };
    let carry = cfg.ice_carry_thickness.max(1);
    let mut left = cfg.max_load_break_cells_per_column_per_tick.max(1) as i32;
    for y in (y0..=y1).rev() {
        if left <= 0 {
            break;
        }
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        if cell.material != MaterialId::Ice {
            continue;
        }
        let Some(above) = world.get_cell(gx, y + 1) else {
            continue;
        };
        if !is_debris_load(above.material) {
            continue;
        }
        // Only the top contact of a lid: debris directly above, or ice
        // debris that is not itself part of a deeper continuous lid
        // stack we're measuring from this cell.
        let thick = ice_lid_thickness(world, gx, y);
        if thick >= carry {
            continue;
        }
        world.set_cell(gx, y, Cell::water());
        left -= 1;
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
    let mut frozen_count = frozen_count_in_column(world, gx);
    if frozen_count >= budget {
        return;
    }

    let mut freezes_left = cfg.max_freeze_cells_per_column_per_tick.max(1) as i32;
    // Top-down: (1) thicken under an existing lid, else (2) freeze the
    // open-sky free surface (classic lake skin). One cell / tick budget.
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
        let under_lid = above_is_frozen(world, gx, y);
        // Open-surface skin only when the column has no ice/snow below.
        // Otherwise a fallen / submerged flake leaves a water gap and a
        // second skin freezes above it — the flake looks like it "floated
        // up" after breaking/falling (shore pump).
        let open_surface = is_standing_water(world, gx, y)
            && open_sky_above(world, gx, y)
            && !below_is_frozen(world, gx, y)
            && !frozen_anywhere_below(world, gx, y, y0);
        if !under_lid && !open_surface {
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

/// True when any Ice/Snow sits strictly below `gy` in this column.
fn frozen_anywhere_below(world: &World, gx: i32, gy: i32, y0: i32) -> bool {
    for y in y0..gy {
        if matches!(
            world.get_cell(gx, y).map(|c| c.material),
            Some(MaterialId::Ice) | Some(MaterialId::Snow)
        ) {
            return true;
        }
    }
    false
}

fn above_is_frozen(world: &World, gx: i32, gy: i32) -> bool {
    matches!(
        world.get_cell(gx, gy + 1),
        Some(a) if is_frozen_solid(a.material)
    )
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
                pore: 128,
            },
        );
        let temp = cold_temp(16, 16, -8.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(w.get_cell(3, 3).unwrap().material, MaterialId::Air);
        assert_eq!(w.get_cell(3, 3).unwrap().sat.0, 16);
    }

    #[test]
    fn ice_thickens_one_cell_per_tick_under_lid() {
        let mut w = pond_world();
        let temp = cold_temp(16, 16, -10.0);
        // Force every-tick cadence so the thickening step is visible.
        let cfg = PhaseConfig {
            period_ticks: 1,
            ..PhaseConfig::default()
        };
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(w.get_cell(3, 3).unwrap().material, MaterialId::Ice);
        assert!(
            w.get_cell(3, 2).unwrap().sat.is_full(),
            "first tick is skin only — deep water stays liquid"
        );
        assert_eq!(w.get_cell(3, 2).unwrap().material, MaterialId::Air);
        w.tick = 1;
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(
            w.get_cell(3, 2).unwrap().material,
            MaterialId::Ice,
            "cold lid must thicken downward into the pond"
        );
    }

    #[test]
    fn ice_column_budget_culls_runaway_tower() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..20 {
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
        let cfg = PhaseConfig {
            period_ticks: 1,
            ..PhaseConfig::default()
        };
        apply_phase(&mut w, &temp, &cfg);
        // Top melts first; buried ice waits.
        assert_eq!(w.get_cell(1, 2).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(1, 2).unwrap().sat.is_full());
        assert_eq!(w.get_cell(1, 1).unwrap().material, MaterialId::Ice);
        w.tick = 1;
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(w.get_cell(1, 1).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(1, 1).unwrap().sat.is_full());
    }

    #[test]
    fn rain_stays_on_ice_and_does_not_loft_ice_upward() {
        // Former density-settle lofted ice into the rain column. Water
        // film must remain on top; ice stays put when supported.
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::water()); // support
        w.set_cell(1, 2, Cell::solid(MaterialId::Ice));
        w.set_cell(
            1,
            3,
            Cell {
                material: MaterialId::Air,
                sat: Sat(80),
                flags: Default::default(),
                _pad: 0,
                pore: 128,
            },
        );
        let temp = cold_temp(16, 16, -5.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(
            w.get_cell(1, 2).unwrap().material,
            MaterialId::Ice,
            "supported ice must not swap upward into rain"
        );
        assert_eq!(w.get_cell(1, 3).unwrap().material, MaterialId::Air);
        assert_eq!(w.get_cell(1, 3).unwrap().sat.0, 80);
    }

    #[test]
    fn full_water_on_ice_does_not_melt_when_cold() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::water());
        w.set_cell(1, 2, Cell::solid(MaterialId::Ice));
        w.set_cell(1, 3, Cell::water()); // full rain ponded on ice
        let temp = cold_temp(16, 16, -8.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(
            w.get_cell(1, 2).unwrap().material,
            MaterialId::Ice,
            "cold rain on ice must stay a film — not melt→refreeze churn"
        );
        assert_eq!(w.get_cell(1, 3).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(1, 3).unwrap().sat.is_full());
    }

    #[test]
    fn partial_sat_does_not_freeze_then_thaw_into_extra_water() {
        // Freeze of sat=100 then thaw to FULL would mint ~155 sat.
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(
            1,
            1,
            Cell {
                material: MaterialId::Air,
                sat: Sat(100),
                flags: Default::default(),
                _pad: 0,
                pore: 128,
            },
        );
        // Open-sky standing film on bedrock.
        let temp = cold_temp(16, 16, -10.0);
        let cfg = PhaseConfig::default();
        assert_eq!(cfg.min_sat_to_freeze, 255);
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(
            w.get_cell(1, 1).unwrap().material,
            MaterialId::Air,
            "partial sat must not become ice (thaw would mint a full cell)"
        );
        assert_eq!(w.get_cell(1, 1).unwrap().sat.0, 100);
    }

    #[test]
    fn warm_water_on_ice_melts_sheet() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::water());
        w.set_cell(1, 2, Cell::solid(MaterialId::Ice));
        w.set_cell(
            1,
            3,
            Cell {
                material: MaterialId::Air,
                sat: Sat(100),
                flags: Default::default(),
                _pad: 0,
                pore: 128,
            },
        );
        let temp = cold_temp(16, 16, 4.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(w.get_cell(1, 2).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(1, 2).unwrap().sat.is_full());
    }

    #[test]
    fn unsupported_ice_over_empty_air_is_not_melted_by_phase() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        // Gap: dry air under ice (basin dropped) — fall owns this, not break.
        w.set_cell(1, 1, Cell::air());
        w.set_cell(1, 2, Cell::solid(MaterialId::Ice));
        w.set_cell(1, 3, Cell::water()); // trapped above
        let temp = cold_temp(16, 16, -5.0);
        let cfg = PhaseConfig {
            enable_freeze: false,
            enable_slush: false,
            ..PhaseConfig::default()
        };
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(
            w.get_cell(1, 2).unwrap().material,
            MaterialId::Ice,
            "phase must not melt ice hanging over empty air (fall drops it)"
        );
    }

    #[test]
    fn thin_ice_breaks_under_sand_load() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::water());
        w.set_cell(2, 2, Cell::solid(MaterialId::Ice)); // 1-cell skin
        w.set_cell(2, 3, Cell::solid(MaterialId::Sand));
        assert_eq!(ice_lid_thickness(&w, 2, 2), 1);
        let temp = cold_temp(16, 16, -8.0);
        let cfg = PhaseConfig {
            enable_freeze: false, // don't re-freeze the break
            enable_thaw: false,
            enable_slush: false,
            ..PhaseConfig::default()
        };
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(
            w.get_cell(2, 2).unwrap().material,
            MaterialId::Air,
            "1-cell ice skin must fail under sand"
        );
        assert!(w.get_cell(2, 2).unwrap().sat.is_full());
    }

    #[test]
    fn thick_ice_carries_sand_load() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::water());
        w.set_cell(2, 2, Cell::solid(MaterialId::Ice));
        w.set_cell(2, 3, Cell::solid(MaterialId::Ice)); // 2-cell lid
        w.set_cell(2, 4, Cell::solid(MaterialId::Sand));
        assert_eq!(ice_lid_thickness(&w, 2, 3), 2);
        let temp = cold_temp(16, 16, -8.0);
        let cfg = PhaseConfig {
            enable_freeze: false,
            enable_thaw: false,
            enable_slush: false,
            ice_carry_thickness: 2,
            ..PhaseConfig::default()
        };
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(w.get_cell(2, 3).unwrap().material, MaterialId::Ice);
        assert_eq!(w.get_cell(2, 2).unwrap().material, MaterialId::Ice);
        assert_eq!(
            w.get_cell(2, 4).unwrap().material,
            MaterialId::Sand,
            "debris stays on a thick lid"
        );
    }

    #[test]
    fn water_on_ice_does_not_freeze_into_tower() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::water());
        w.set_cell(1, 2, Cell::solid(MaterialId::Ice));
        w.set_cell(
            1,
            3,
            Cell {
                material: MaterialId::Air,
                sat: Sat(80),
                flags: Default::default(),
                _pad: 0,
                pore: 128,
            },
        );
        let temp = cold_temp(16, 16, -8.0);
        for tick in 0..5 {
            w.tick = tick;
            apply_phase(&mut w, &temp, &PhaseConfig::default());
        }
        // Lid may thicken downward into the pond — that is intended.
        // Thin rain on top must stay a film (no upward ice tower).
        assert_eq!(
            w.get_cell(1, 3).unwrap().material,
            MaterialId::Air,
            "thin rain on ice must not freeze into an upward tower"
        );
        assert!(w.get_cell(1, 3).unwrap().sat.0 > 0);
        for y in 4..8 {
            assert_ne!(
                w.get_cell(1, y).map(|c| c.material),
                Some(MaterialId::Ice),
                "ice must not grow above the rain film at y={y}"
            );
        }
    }

    #[test]
    fn cold_precip_deposits_snow_on_ground() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        let temp = cold_temp(16, 16, -6.0);
        let cfg = PhaseConfig::default();
        let landed = deposit_precip_on_surface(&mut w, 2, 10, 255.0, Some(&temp), Some(&cfg));
        assert!(landed > 0.0);
        assert_eq!(w.get_cell(2, 2).unwrap().material, MaterialId::Snow);
    }

    #[test]
    fn underpaid_cold_precip_does_not_mint_snow_cell() {
        // Climatic droplet_sat=64 used to seat Snow then thaw to FULL (+191).
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        let temp = cold_temp(16, 16, -10.0);
        let cfg = PhaseConfig::default();
        let landed = deposit_precip_on_surface(&mut w, 2, 10, 64.0, Some(&temp), Some(&cfg));
        assert_eq!(landed, 0.0, "must hold — not seat underpaid Snow");
        assert_ne!(
            w.get_cell(2, 2).map(|c| c.material),
            Some(MaterialId::Snow)
        );
        assert_eq!(w.get_cell(2, 2).map(|c| c.sat.0), Some(0));
    }

    #[test]
    fn warm_precip_stays_rain() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        let temp = cold_temp(16, 16, 8.0);
        let cfg = PhaseConfig::default();
        let landed = deposit_precip_on_surface(&mut w, 2, 10, 64.0, Some(&temp), Some(&cfg));
        assert!(landed > 0.0);
        assert_eq!(w.get_cell(2, 2).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(2, 2).unwrap().sat.0 >= 64);
    }

    fn set_cell_temp(t: &mut Temperature, gx: i32, gy: i32, c: f32) {
        let (hx, hy) = t.tile_of(gx, gy);
        t.cells.insert((hx, hy), c);
    }

    #[test]
    fn cold_air_snow_melts_on_warm_ground() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        // Cold aloft (start_y=10), warm surface.
        let mut temp = cold_temp(16, 16, -8.0);
        set_cell_temp(&mut temp, 2, 1, 6.0);
        let cfg = PhaseConfig::default();
        assert!(precip_forms_snow_at_air(&temp, 2, 10, &cfg));
        let landed = deposit_precip_on_surface(&mut w, 2, 10, 64.0, Some(&temp), Some(&cfg));
        assert!(landed > 0.0);
        assert_eq!(
            w.get_cell(2, 2).unwrap().material,
            MaterialId::Air,
            "snow must melt to liquid on warm ground"
        );
        assert!(w.get_cell(2, 2).unwrap().sat.0 >= 64);
    }

    #[test]
    fn warm_air_rains_even_on_cold_ground() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        // Warm cloud air, cold surface — rain, not snowflakes from a warm sky.
        let mut temp = cold_temp(16, 16, 8.0);
        set_cell_temp(&mut temp, 2, 1, -6.0);
        let cfg = PhaseConfig::default();
        assert!(!precip_forms_snow_at_air(&temp, 2, 10, &cfg));
        let landed = deposit_precip_on_surface(&mut w, 2, 10, 64.0, Some(&temp), Some(&cfg));
        assert!(landed > 0.0);
        assert_eq!(w.get_cell(2, 2).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(2, 2).unwrap().sat.0 >= 64);
        assert_ne!(
            w.get_cell(2, 2).unwrap().material,
            MaterialId::Snow,
            "warm-air precip must not become snow just because ground is cold"
        );
    }

    #[test]
    fn frozen_budget_full_holds_precip_does_not_soak() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        for y in 2..=5 {
            w.set_cell(2, y, Cell::solid(MaterialId::Snow));
        }
        let temp = cold_temp(16, 16, -10.0);
        let cfg = PhaseConfig {
            max_ice_cells_per_column: 4,
            ..PhaseConfig::default()
        };
        assert_eq!(frozen_count_in_column(&w, 2), 4);
        let landed = deposit_precip_on_surface(&mut w, 2, 20, 80.0, Some(&temp), Some(&cfg));
        assert_eq!(landed, 0.0, "cold cap-full must hold precip, not dump rain");
        assert_eq!(
            frozen_count_in_column(&w, 2),
            4,
            "must not grow snow past the column budget"
        );
        assert_eq!(
            w.get_cell(2, 1).unwrap().sat.0,
            0,
            "peak sand must stay dry — no pore soak under a full snow pack"
        );
    }

    #[test]
    fn snow_prefers_bare_neighbour_over_tall_peak_pack() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Peak spike at x=3, bare sand at x=1 and x=5 within spread radius.
        for x in 1..=5 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
        }
        for y in 2..=6 {
            w.set_cell(3, y, Cell::solid(MaterialId::Snow));
        }
        let temp = cold_temp(16, 16, -10.0);
        let cfg = PhaseConfig {
            snow_spread_radius: 3,
            snow_blanket_depth: 2,
            ..PhaseConfig::default()
        };
        let landed = deposit_precip_on_surface(&mut w, 3, 20, 255.0, Some(&temp), Some(&cfg));
        assert!(landed > 0.0);
        assert_eq!(
            frozen_count_in_column(&w, 3),
            5,
            "peak pack must not grow while bare neighbours exist"
        );
        let left = w.get_cell(1, 2).map(|c| c.material) == Some(MaterialId::Snow);
        let right = w.get_cell(5, 2).map(|c| c.material) == Some(MaterialId::Snow);
        let mid_l = w.get_cell(2, 2).map(|c| c.material) == Some(MaterialId::Snow);
        let mid_r = w.get_cell(4, 2).map(|c| c.material) == Some(MaterialId::Snow);
        assert!(
            left || right || mid_l || mid_r,
            "snow must seat on a thinner neighbour column"
        );
    }

    #[test]
    fn condensate_frosts_one_ice_cell_then_holds() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        let temp = cold_temp(16, 16, -10.0);
        let cfg = PhaseConfig::default();
        let underpay =
            deposit_condensate_on_surface(&mut w, 2, 12, 96.0, Some(&temp), Some(&cfg));
        assert_eq!(underpay, 0.0, "underpaid frost must not mint Ice");
        let first =
            deposit_condensate_on_surface(&mut w, 2, 12, 255.0, Some(&temp), Some(&cfg));
        assert_eq!(first, 255.0);
        assert_eq!(w.get_cell(2, 2).unwrap().material, MaterialId::Ice);
        assert_ne!(w.get_cell(2, 2).unwrap().material, MaterialId::Snow);
        let second =
            deposit_condensate_on_surface(&mut w, 2, 12, 255.0, Some(&temp), Some(&cfg));
        assert_eq!(second, 0.0, "further condensate must not thicken frost");
        assert_eq!(frozen_count_in_column(&w, 2), 1);
        for y in 3..10 {
            assert_ne!(
                w.get_cell(2, y).map(|c| c.material),
                Some(MaterialId::Ice),
                "no frost tower at y={y}"
            );
        }
    }

    #[test]
    fn frost_thaw_roundtrip_does_not_mint_water() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        let cold = cold_temp(16, 16, -10.0);
        let warm = cold_temp(16, 16, 8.0);
        let cfg = PhaseConfig::default();
        let paid =
            deposit_condensate_on_surface(&mut w, 2, 12, 255.0, Some(&cold), Some(&cfg));
        assert_eq!(paid, 255.0);
        apply_phase(&mut w, &warm, &cfg);
        assert_eq!(
            w.get_cell(2, 2).unwrap().material,
            MaterialId::Air,
            "frost must thaw to water"
        );
        assert_eq!(
            w.get_cell(2, 2).unwrap().sat.0,
            u8::MAX,
            "thaw yields one full cell — equal to the frost payment"
        );
    }

    #[test]
    fn snow_holds_when_soft_blanket_is_full() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 1..=3 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
            w.set_cell(x, 2, Cell::solid(MaterialId::Snow));
            w.set_cell(x, 3, Cell::solid(MaterialId::Snow));
            w.set_cell(x, 4, Cell::solid(MaterialId::Snow)); // pack = 3 > blanket 2
        }
        let temp = cold_temp(16, 16, -10.0);
        let cfg = PhaseConfig {
            snow_spread_radius: 2,
            snow_blanket_depth: 2,
            ..PhaseConfig::default()
        };
        let before: usize = (1..=3).map(|x| frozen_count_in_column(&w, x)).sum();
        let landed = deposit_precip_on_surface(&mut w, 2, 20, 255.0, Some(&temp), Some(&cfg));
        assert_eq!(landed, 0.0, "must hold snow once the soft blanket is full");
        let after: usize = (1..=3).map(|x| frozen_count_in_column(&w, x)).sum();
        assert_eq!(after, before, "must not grow spikes past snow_blanket_depth");
    }

    #[test]
    fn cold_snow_settles_on_sand_without_wetting_pores() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        let temp = cold_temp(16, 16, -12.0);
        let cfg = PhaseConfig::default();
        let landed = deposit_precip_on_surface(&mut w, 2, 12, 255.0, Some(&temp), Some(&cfg));
        assert!(landed > 0.0);
        assert_eq!(
            w.get_cell(2, 2).unwrap().material,
            MaterialId::Snow,
            "cold precip must settle as Snow on sand"
        );
        assert_eq!(
            w.get_cell(2, 1).unwrap().sat.0,
            0,
            "snow must not permeate into sand pores"
        );
        assert_eq!(w.get_cell(2, 1).unwrap().material, MaterialId::Sand);
    }

    #[test]
    fn cold_snow_replaces_wet_film_on_sand() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(
            2,
            2,
            Cell {
                material: MaterialId::Air,
                sat: Sat(80),
                flags: Default::default(),
                _pad: 0,
                pore: 128,
            },
        );
        let temp = cold_temp(16, 16, -12.0);
        let landed = deposit_precip_on_surface(
            &mut w,
            2,
            12,
            255.0,
            Some(&temp),
            Some(&PhaseConfig::default()),
        );
        assert!(landed > 0.0);
        assert_eq!(w.get_cell(2, 2).unwrap().material, MaterialId::Snow);
        assert_eq!(w.get_cell(2, 1).unwrap().sat.0, 0);
    }

    #[test]
    fn slush_cold_freezes_water_under_snow() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::water());
        w.set_cell(1, 2, Cell::solid(MaterialId::Snow));
        let temp = cold_temp(16, 16, -5.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(w.get_cell(1, 2).unwrap().material, MaterialId::Snow);
        assert_eq!(
            w.get_cell(1, 1).unwrap().material,
            MaterialId::Ice,
            "cold snow must freeze the water film into ice (slush pack)"
        );
    }

    #[test]
    fn slush_warm_melts_snow_on_water() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::water());
        w.set_cell(1, 2, Cell::solid(MaterialId::Snow));
        let temp = cold_temp(16, 16, 5.0);
        apply_phase(&mut w, &temp, &PhaseConfig::default());
        assert_eq!(
            w.get_cell(1, 2).unwrap().material,
            MaterialId::Air,
            "warm water melts snow into water"
        );
        assert!(w.get_cell(1, 2).unwrap().sat.is_full());
    }

    #[test]
    fn unsupported_ice_over_haze_is_not_melted_by_phase() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(
            1,
            1,
            Cell {
                material: MaterialId::Air,
                sat: Sat(128),
                flags: Default::default(),
                _pad: 0,
                pore: 128,
            },
        );
        w.set_cell(1, 2, Cell::solid(MaterialId::Ice));
        let temp = cold_temp(16, 16, -5.0);
        let cfg = PhaseConfig {
            enable_freeze: false,
            enable_slush: false,
            ..PhaseConfig::default()
        };
        apply_phase(&mut w, &temp, &cfg);
        assert_eq!(
            w.get_cell(1, 2).unwrap().material,
            MaterialId::Ice,
            "phase must not melt ice over haze (grain fall drops it)"
        );
    }

    fn ice_footprint(w: &World, x0: i32, x1: i32, y0: i32, y1: i32) -> Vec<(i32, i32)> {
        let mut cells = Vec::new();
        for x in x0..=x1 {
            for y in y0..=y1 {
                if w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Ice) {
                    cells.push((x, y));
                }
            }
        }
        cells.sort_unstable();
        cells
    }

    #[test]
    fn shore_ice_with_water_on_top_does_not_pump() {
        // Shoreline like the demo GIF: sand slope into a pond, thin ice at
        // the waterline with ponded water on/around it. Full tick + cold
        // avalanche + phase must not bob the flake ±1 every few frames.
        use crate::rules::{apply_cold_avalanche, apply_grain_fall, tick};

        let mut w = World::new(42);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Bedrock floor; sand beach rising to the right; pond on the left.
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let sand_top = if x < 6 {
                1
            } else if x < 10 {
                2
            } else {
                3
            };
            for y in 1..=sand_top {
                w.set_cell(x, y, Cell::solid(MaterialId::Sand));
            }
            for y in (sand_top + 1)..=4 {
                if x < 10 {
                    w.set_cell(x, y, Cell::water());
                } else {
                    w.set_cell(x, y, Cell::air());
                }
            }
        }
        // Flake at the waterline on the sand step, with water on top.
        w.set_cell(8, 3, Cell::solid(MaterialId::Ice));
        w.set_cell(9, 3, Cell::solid(MaterialId::Ice));
        w.set_cell(8, 4, Cell::water());
        w.set_cell(9, 4, Cell::water());

        let temp = cold_temp(32, 16, -6.0);
        let cfg = PhaseConfig {
            period_ticks: 1, // stress the phase cadence
            ..PhaseConfig::default()
        };
        let mut top_ys = Vec::new();
        for _ in 0..40 {
            tick(&mut w);
            apply_grain_fall(&mut w);
            apply_cold_avalanche(&mut w, &temp, cfg.freeze_point_c);
            apply_phase(&mut w, &temp, &cfg);
            let cells = ice_footprint(&w, 0, 15, 0, 8);
            top_ys.push(cells.iter().map(|(_, y)| *y).max());
        }
        let present: Vec<i32> = top_ys.iter().filter_map(|y| *y).collect();
        assert!(
            !present.is_empty(),
            "ice should persist at the shoreline (top_ys={top_ys:?})"
        );
        let min_y = *present.iter().min().unwrap();
        let max_y = *present.iter().max().unwrap();
        assert!(
            max_y - min_y <= 1,
            "shore ice must not pump vertically (span={} top_ys={top_ys:?})",
            max_y - min_y
        );
        // No period-2 alternation in the second half.
        let tail = &present[present.len() / 2..];
        let flips = tail.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(
            flips <= 2,
            "settled shore ice should not keep flipping Y (flips={flips} tail={tail:?})"
        );
    }

    #[test]
    fn hillside_shore_ice_peel_does_not_refreeze_pump() {
        // Ice glaze on sand at the waterline can cold-avalanche peel into the
        // basin, fall, then freeze again at the free surface — a break/fall/
        // refreeze bob if peel keeps firing every phase tick.
        use crate::rules::{apply_cold_avalanche, apply_grain_fall, tick};

        let mut w = World::new(43);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            // Flat lake bed to x=7; sand step at x=8.. that holds glaze.
            if x <= 7 {
                w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
                for y in 2..=5 {
                    w.set_cell(x, y, Cell::water());
                }
            } else {
                w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
                w.set_cell(x, 2, Cell::solid(MaterialId::Sand));
                w.set_cell(x, 3, Cell::solid(MaterialId::Sand));
                for y in 4..=6 {
                    w.set_cell(x, y, Cell::air());
                }
            }
        }
        // Glaze on the sand lip (hillside support — avalanche-eligible).
        w.set_cell(8, 4, Cell::solid(MaterialId::Ice));
        w.set_cell(9, 4, Cell::solid(MaterialId::Ice));
        // Ponded water touching the lip so peel has a wet neighbor.
        w.set_cell(7, 4, Cell::water());
        w.set_cell(7, 5, Cell::water());

        let temp = cold_temp(32, 16, -8.0);
        let cfg = PhaseConfig {
            period_ticks: 1,
            ..PhaseConfig::default()
        };
        let mut top_ys = Vec::new();
        for _ in 0..48 {
            tick(&mut w);
            apply_grain_fall(&mut w);
            apply_cold_avalanche(&mut w, &temp, cfg.freeze_point_c);
            apply_phase(&mut w, &temp, &cfg);
            let cells = ice_footprint(&w, 0, 15, 0, 8);
            top_ys.push(cells.iter().map(|(_, y)| *y).max());
        }
        let present: Vec<i32> = top_ys.iter().filter_map(|y| *y).collect();
        if present.is_empty() {
            // Ice may fully melt into the basin — that is not a pump.
            return;
        }
        let min_y = *present.iter().min().unwrap();
        let max_y = *present.iter().max().unwrap();
        let flips = present.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(
            max_y - min_y <= 1 && flips <= 3,
            "hillside shore ice must not endlessly peel/refreeze pump (span={} flips={flips} ys={top_ys:?})",
            max_y - min_y
        );
    }

    #[test]
    fn no_second_ice_skin_above_submerged_flake() {
        // Fallen flake at y=2 with full water above must not grow a new
        // open-surface skin at y=4 (the visual "float up" after a fall).
        let mut w = World::new(44);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(3, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(3, 1, Cell::water());
        w.set_cell(3, 2, Cell::solid(MaterialId::Ice)); // submerged flake
        w.set_cell(3, 3, Cell::water());
        w.set_cell(3, 4, Cell::water());
        let temp = cold_temp(16, 16, -10.0);
        let cfg = PhaseConfig {
            period_ticks: 1,
            ..PhaseConfig::default()
        };
        for tick in 0..6 {
            w.tick = tick;
            apply_phase(&mut w, &temp, &cfg);
        }
        assert_eq!(
            w.get_cell(3, 2).unwrap().material,
            MaterialId::Ice,
            "submerged flake remains"
        );
        // Under-lid may thicken downward into y=1; never a new skin at/above y=3.
        for y in 3..8 {
            assert_ne!(
                w.get_cell(3, y).map(|c| c.material),
                Some(MaterialId::Ice),
                "must not skin a second lid above submerged ice at y={y}"
            );
        }
    }

    #[test]
    fn ice_on_haze_does_not_melt_refreeze_pump() {
        // Regression: flake over partial sat used to float (grain) but fail
        // phase support → melt → freeze at free surface → ±1 cell pump.
        let mut w = World::new(11);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::water());
            w.set_cell(x, 2, Cell::water());
        }
        // Misty column under a free-floating flake (neighbours keep full water).
        w.set_cell(
            3,
            2,
            Cell {
                material: MaterialId::Air,
                sat: Sat(64),
                flags: Default::default(),
                _pad: 0,
                pore: 128,
            },
        );
        w.set_cell(3, 3, Cell::solid(MaterialId::Ice));
        let temp = cold_temp(16, 16, -8.0);
        let cfg = PhaseConfig::default();
        let mut ys = Vec::new();
        for t in 0..24u64 {
            w.tick = t;
            crate::rules::apply_grain_fall(&mut w);
            apply_phase(&mut w, &temp, &cfg);
            let y_ice = (0..8)
                .rev()
                .find(|&y| {
                    w.get_cell(3, y).map(|c| c.material) == Some(MaterialId::Ice)
                })
                .expect("ice flake must persist");
            ys.push(y_ice);
        }
        let min_y = *ys.iter().min().unwrap();
        let max_y = *ys.iter().max().unwrap();
        assert!(
            max_y - min_y <= 1,
            "ice must not pump ±1 every phase period (ys={ys:?})"
        );
        // No alternating every period_ticks once settled.
        let period = cfg.period_ticks.max(1) as usize;
        if ys.len() > period * 3 {
            let tail = &ys[ys.len() - period * 2..];
            let unique: std::collections::HashSet<_> = tail.iter().copied().collect();
            assert!(
                unique.len() == 1,
                "settled flake Y must be steady across phase periods (tail={tail:?})"
            );
        }
    }
}
