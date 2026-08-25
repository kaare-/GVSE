//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Grain fall, repose, cold avalanche, and flow erosion.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use wk_material::{HydroOverrides, MaterialId};

use crate::active::{partition_checkerboard, plan_active, ActiveChunk};
use crate::cell::{
    falls_through_empty_air, is_flow_erodible, is_grain, is_repose_grain,
    water_capacity_cell, Cell, CellFlags, Sat,
};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::fungi::{move_mycelium_meta, swap_cells_preserving_mycelium, swap_mycelium_meta};
use crate::grid::World;
use crate::parallel::{
    self, for_each_region_parallel, for_each_region_serial_moore, map_chunk_coords_parallel,
};
use crate::temperature::Temperature;

use super::gravity::apply_gravity_fall;
use super::plan::{regions_for_standalone, regions_loose_moore};
use super::util::hash_prob;

/// Extra `max_step` cells when a living Root occupies the grain cell.
///
/// Column ecology used `repose_rise *= (1 + 3ρ)`; for sand
/// (`repose_rise_m=0.15`, `SAMPLE_WIDTH_M=0.25`) that is ≈ +2 steps at
/// full root density. Binary per-cell roots use the same bonus.
pub const ROOT_REPOSE_STEP_BONUS: i32 = 2;

/// Flow-erosion susceptibility divisor when a living Root is present
/// (`≈ 1 + 2.5ρ` at ρ=1 from column `run_sediment`).
pub const ROOT_EROSION_BIND: f32 = 3.5;

/// Full mycelium intensity (255) matches living-root repose bonus.
///
/// Stickiness scales with Organic [`Cell::mycelium`] 0..=255 — the same
/// cream field fungi already paint. No separate sticky material.
pub const MYCELIUM_REPOSE_STEP_BONUS: i32 = ROOT_REPOSE_STEP_BONUS;

/// Full mycelium intensity matches living-root erosion bind.
pub const MYCELIUM_EROSION_BIND: f32 = ROOT_EROSION_BIND;

/// Floating Organic columns with at least this mycelium sail as one raft
/// (cream mats stick together instead of tearing column-by-column).
pub const MYCELIUM_RAFT_BIND_MIN: u8 = 40;

/// At full mycelium, floating Organic waterlogs at this fraction of the
/// base rate — colonized mats float longer, bare litter sinks sooner.
const MYCELIUM_WATERLOG_MIN_SCALE: f32 = 0.12;

/// Max vertical fall cells when settling unsupported grains in one
/// call. Default sky is ~5 chunks tall (`64×5`); cover that so F3
/// litter does not take hundreds of ticks to land.
pub const GRAIN_SETTLE_PASSES: u32 = 1024;

/// Repose / polish settle when nothing is freefalling. Busy shores used
/// to run up toward [`GRAIN_SETTLE_PASSES`] on micro-moves (organic /
/// bank fidget) and dominate the physics tick on Super-Server.
pub const GRAIN_SETTLE_PASSES_SHALLOW: u32 = 8;

/// FPS-path deep settle cap. Organic+water shores used to trip the full
/// ×1024 freefall budget every tick (Super-Server → ~1 FPS) via buoyancy
/// fidget and grounded-column walks. Full-feel / unit tests still use
/// [`GRAIN_SETTLE_PASSES`].
pub const GRAIN_SETTLE_PASSES_FPS_DEEP: u32 = 64;

/// True when any cell in `active` is mid-air over **empty/haze** Air
/// (F3 sky paint / real freefall).
///
/// Must **not** treat sand over wet/lake Air as unsupported — dense grains
/// sink through any sat, and shores under closed-loop rain would otherwise
/// force deep ×1024 settle every tick (Super-Server: physics tick ~25 ms
/// while a no-rain mirror showed ~12 ms).
///
/// Buoyant litter on **full** standing water is treated as seated here
/// without a 512-deep grounded walk — this gate only chooses settle depth.
/// Mid-air full-sat blobs are rare after condensation fixes; rise+fall
/// still use the grounded check.
pub fn active_has_unsupported_grain(world: &World, active: &[ActiveChunk]) -> bool {
    for ac in active {
        let Some(chunk) = world.chunks.get(&ac.coord) else {
            continue;
        };
        let base_gx = ac.coord.cx * CHUNK_CELLS_W as i32;
        let base_gy = ac.coord.cy * CHUNK_CELLS_H as i32;
        for y in ac.rect.y0..=ac.rect.y1 {
            for x in ac.rect.x0..=ac.rect.x1 {
                let cell = chunk.get(x as usize, y as usize);
                let loose = is_grain(cell.material) || falls_through_empty_air(cell.material);
                if !loose {
                    continue;
                }
                let gx = world.wrap_x(base_gx + x as i32);
                let gy = base_gy + y as i32;
                let Some(below) = world.get_cell(gx, gy - 1) else {
                    return true;
                };
                if below.material != MaterialId::Air {
                    continue;
                }
                // Wet film / lake under a grain is a shore seat, not sky.
                if below.sat.0 > GRAIN_REPOSE_HAZE_MAX {
                    continue;
                }
                // Buoyant litter on full standing water — seated for depth gate.
                if falls_through_empty_air(cell.material)
                    && !cell.is_waterlogged_organic()
                    && below.sat.is_full()
                {
                    continue;
                }
                return true;
            }
        }
    }
    false
}

/// Repose `max_step` bonus from mycelium intensity on Organic (0..=2).
#[inline]
pub fn mycelium_repose_bonus(cell: Cell) -> i32 {
    if cell.material != MaterialId::Organic || cell.mycelium() == 0 {
        return 0;
    }
    let m = cell.mycelium() as i32;
    // Round so light colonization already helps a little.
    (m * MYCELIUM_REPOSE_STEP_BONUS + 254) / 255
}

/// Flow-erosion susceptibility divisor from mycelium (1.0..=bind).
#[inline]
pub fn mycelium_erosion_bind(cell: Cell) -> f32 {
    if cell.material != MaterialId::Organic || cell.mycelium() == 0 {
        return 1.0;
    }
    let t = cell.mycelium() as f32 / 255.0;
    1.0 + (MYCELIUM_EROSION_BIND - 1.0) * t
}

/// Scale waterlog probability by mycelium (1.0 bare → ~0.12 at full cream).
#[inline]
pub fn mycelium_waterlog_scale(myc: u8) -> f32 {
    if myc == 0 {
        return 1.0;
    }
    let t = myc as f32 / 255.0;
    (1.0 - (1.0 - MYCELIUM_WATERLOG_MIN_SCALE) * t).clamp(MYCELIUM_WATERLOG_MIN_SCALE, 1.0)
}

/// Strongest mycelium in a floating Organic column stack.
fn column_max_mycelium(world: &World, gx: i32, bottom: i32, height: i32) -> u8 {
    let mut m = 0u8;
    for y in bottom..bottom.saturating_add(height) {
        if let Some(c) = world.get_cell(gx, y) {
            if c.material == MaterialId::Organic {
                m = m.max(c.mycelium());
            }
        }
    }
    m
}

/// One-cell-per-pass grain fall.
///
/// Each `Air` cell **pulls** a granular neighbour from directly above
/// (swap). Whatever water saturation the Air cell had rises into the
/// vacated upper cell, so a grain sinking through water displaces
/// exactly the water it walks through — mass is conserved.
///
/// Pull + bottom-up + checkerboard matches [`apply_gravity_fall`]:
/// one cell per invocation, including across chunk seams.
///
/// V1 kept simple: dense grains fall through Air *any* saturation;
/// Snow/Ice/Organic fall through *empty* Air only (float on water).
/// Density-ordered stacking between grain species is a follow-up.
pub fn apply_grain_fall(world: &mut World) {
    let regions = regions_for_standalone(world);
    for pass in partition_checkerboard(&regions) {
        apply_grain_fall_regions(world, &pass);
    }
}

/// Drop unsupported grains / litter through Air until seated or the
/// pass budget is spent. Starts from the world's current dirty wake
/// (F3 paint, editor writes, prior CA).
///
/// The terrain editor pauses the full tick while open — that is fine;
/// on unpause, [`tick_with_life`] calls this so painted Organic/Sand
/// seats instead of hanging until roof-collapse / erosion re-wakes
/// the column.
pub fn settle_loose_grains(
    world: &mut World,
    rooted: Option<&HashSet<(i32, i32)>>,
    max_passes: u32,
) {
    let active = plan_active(world);
    settle_loose_grains_regions(world, &active, rooted, max_passes);
}

/// True when a full-sat Air cell is part of a water column that rests on
/// solid (a real lake / puddle). Mid-air full-sat blobs (condensation /
/// invisible suspended water) return false so Organic/Snow sink through
/// instead of hanging forever on an invisible seat.
///
/// Partial-sat Air (haze / soak drawdown) still counts as column water —
/// only a true empty gap (`sat == 0`) breaks grounding. Otherwise floating
/// litter soak punching a deep donor cell would make the whole lake look
/// "ungrounded" and Organic would freefall through the ocean every tick.
fn water_column_grounded_world(world: &World, gx: i32, gy: i32) -> bool {
    let mut y = gy;
    for _ in 0..512 {
        let Some(c) = world.get_cell(gx, y) else {
            // Ran off the loaded map through water — treat as bedded.
            return true;
        };
        if c.material == MaterialId::Air {
            if c.sat.is_empty() {
                return false;
            }
            y -= 1;
            continue;
        }
        return true;
    }
    false
}

fn water_column_grounded_ptrs(
    ptrs: &parallel::ChunkPtrMap,
    wrap_width: Option<i32>,
    gx: i32,
    gy: i32,
) -> bool {
    let mut y = gy;
    for _ in 0..512 {
        let Some(c) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx, y) }) else {
            // Lower chunk not in this pass's ptr map (checkerboard / halo)
            // while we are still in water — do not treat as a suspended
            // gap or Organic freefalls through the ocean every settle pass.
            return true;
        };
        if c.material == MaterialId::Air {
            if c.sat.is_empty() {
                return false;
            }
            y -= 1;
            continue;
        }
        return true;
    }
    false
}

/// Snow / Ice / Organic may float on this Air seat only when it is full
/// standing water that reaches solid ground (lake surface).
fn floats_on_air_seat_world(world: &World, seat: Cell, gx: i32, gy: i32) -> bool {
    seat.material == MaterialId::Air
        && seat.sat.is_full()
        && water_column_grounded_world(world, gx, gy)
}

fn floats_on_air_seat_ptrs(
    ptrs: &parallel::ChunkPtrMap,
    wrap_width: Option<i32>,
    seat: Cell,
    gx: i32,
    gy: i32,
) -> bool {
    seat.material == MaterialId::Air
        && seat.sat.is_full()
        && water_column_grounded_ptrs(ptrs, wrap_width, gx, gy)
}

/// True when `litter_y` is buoyant litter whose column reaches a grounded
/// lake seat.
///
/// Walks down through more litter **and** dense grains. After the first
/// punch of a tall pile the stack is often `Rock|Organic|Rock|Water` —
/// stopping at the submerged grain left the upper cargo stranded on the
/// raft forever (the in-game "still holding" bug).
fn raft_rests_on_float_water_world(world: &World, gx: i32, litter_y: i32) -> bool {
    let Some(start) = world.get_cell(gx, litter_y) else {
        return false;
    };
    if !falls_through_empty_air(start.material) {
        return false;
    }
    if start.is_waterlogged_organic() {
        return false;
    }
    let mut y = litter_y - 1;
    for _ in 0..128 {
        let Some(c) = world.get_cell(gx, y) else {
            return false;
        };
        if floats_on_air_seat_world(world, c, gx, y) {
            return true;
        }
        // Partial / haze water still counts as a lake column under a raft
        // (surface may be momentarily not-full after flow/soak).
        if c.material == MaterialId::Air && !c.sat.is_empty() && water_column_grounded_world(world, gx, y)
        {
            return true;
        }
        if falls_through_empty_air(c.material) || is_grain(c.material) {
            y -= 1;
            continue;
        }
        return false;
    }
    false
}

fn raft_rests_on_float_water_ptrs(
    ptrs: &parallel::ChunkPtrMap,
    wrap_width: Option<i32>,
    gx: i32,
    litter_y: i32,
) -> bool {
    let Some(start) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx, litter_y) }) else {
        return false;
    };
    if !falls_through_empty_air(start.material) {
        return false;
    }
    if start.is_waterlogged_organic() {
        return false;
    }
    let mut y = litter_y - 1;
    for _ in 0..128 {
        let Some(c) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx, y) }) else {
            return false;
        };
        if floats_on_air_seat_ptrs(ptrs, wrap_width, c, gx, y) {
            return true;
        }
        if c.material == MaterialId::Air
            && !c.sat.is_empty()
            && water_column_grounded_ptrs(ptrs, wrap_width, gx, y)
        {
            return true;
        }
        if falls_through_empty_air(c.material) || is_grain(c.material) {
            y -= 1;
            continue;
        }
        return false;
    }
    false
}

/// Max punch swaps per call — tall piles on thick rafts need several
/// grain↔litter steps before cargo reaches the water seat. Kept modest
/// so Organic floods do not rescan every loose chunk ×64 times.
const FLOAT_PUNCH_MAX: u32 = 16;

/// Dense grains cannot ride a floating Organic / Snow / Ice raft.
///
/// Collects punch candidates once, then applies up to [`FLOAT_PUNCH_MAX`]
/// swaps (re-validating each). Settle/tick then sinks the grain through water.
pub fn punch_through_floating_rafts(world: &mut World) -> u32 {
    let mut swaps: Vec<(i32, i32)> = Vec::new();
    let coords = loose_chunk_coords(world);
    for coord in coords {
        let x0 = coord.cx * CHUNK_CELLS_W as i32;
        let y0 = coord.cy * CHUNK_CELLS_H as i32;
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let cell = chunk.get(lx, ly);
                if !is_grain(cell.material) {
                    continue;
                }
                let gx = x0 + lx as i32;
                let gy = y0 + ly as i32;
                let Some(below) = world.get_cell(gx, gy - 1) else {
                    continue;
                };
                if !falls_through_empty_air(below.material) {
                    continue;
                }
                if !raft_rests_on_float_water_world(world, gx, gy - 1) {
                    continue;
                }
                swaps.push((gx, gy));
            }
        }
    }
    if swaps.is_empty() {
        return 0;
    }
    // Bottom grains first so Soil under LooseRock punches before rock.
    swaps.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    let mut total = 0u32;
    for (gx, gy) in swaps {
        if total >= FLOAT_PUNCH_MAX {
            break;
        }
        let Some(grain) = world.get_cell(gx, gy) else {
            continue;
        };
        if !is_grain(grain.material) {
            continue;
        }
        let Some(litter) = world.get_cell(gx, gy - 1) else {
            continue;
        };
        if !falls_through_empty_air(litter.material) {
            continue;
        }
        if !raft_rests_on_float_water_world(world, gx, gy - 1) {
            continue;
        }
        // Grain ↔ litter — cream + strain shares ride with each host.
        swap_cells_preserving_mycelium(world, gx, gy - 1, gx, gy);
        // Keep the water seat awake so the next fall pass sinks cargo.
        if let Some(seat) = world.get_cell(gx, gy - 2) {
            if seat.material == MaterialId::Air {
                world.touch_dirty(gx, gy - 2);
            }
        }
        total = total.saturating_add(1);
    }
    total
}

/// Chunks that may hold grain / litter (sticky [`Chunk::has_loose`]).
///
/// Falls back to every loaded chunk when no flag is set yet (old saves).
fn loose_chunk_coords(world: &World) -> Vec<ChunkCoord> {
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_loose)
        .map(|(&coord, _)| coord)
        .collect();
    if coords.is_empty() && !world.chunks.is_empty() {
        coords = world.chunks.keys().copied().collect();
    }
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
}

/// Freefall / raft-cargo counts from a grain wake scan.
#[derive(Debug, Clone, Copy, Default)]
pub struct GrainWake {
    /// Unsupported grain / litter over empty/haze Air.
    pub freefall: u32,
    /// Dense grain sitting on a floating Organic/Snow/Ice raft.
    pub raft_cargo: u32,
}

/// Re-dirty freefall seats **and** over-steep repose faces.
///
/// Returns [`GrainWake`] — freefall counts drive deep vs shallow settle;
/// `raft_cargo` lets callers skip empty [`punch_through_floating_rafts`].
///
/// `only_coords = None` scans every sticky-loose chunk (periodic full
/// insurance). `Some(…)` restricts to those coords (dirty-halo wake).
pub fn wake_grains_for_settle(world: &mut World) -> GrainWake {
    let coords = loose_chunk_coords(world);
    wake_grains_for_settle_coords(world, &coords)
}

/// [`wake_grains_for_settle`] restricted to an explicit chunk list.
pub fn wake_grains_for_settle_coords(world: &mut World, coords: &[ChunkCoord]) -> GrainWake {
    let mut dirty: Vec<(i32, i32)> = Vec::new();
    let mut clear_loose: Vec<ChunkCoord> = Vec::new();
    let mut freefall = 0u32;
    let mut raft_cargo = 0u32;
    for &coord in coords {
        let x0 = coord.cx * CHUNK_CELLS_W as i32;
        let y0 = coord.cy * CHUNK_CELLS_H as i32;
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        let mut saw_loose = false;
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let cell = chunk.get(lx, ly);
                let gx = x0 + lx as i32;
                let gy = y0 + ly as i32;
                let loose = is_grain(cell.material)
                    || falls_through_empty_air(cell.material)
                    || is_repose_grain(cell.material);
                if !loose {
                    continue;
                }
                saw_loose = true;
                // --- unsupported / freefall ---
                let Some(below) = world.get_cell(gx, gy - 1) else {
                    dirty.push((gx, gy));
                    freefall += 1;
                    continue;
                };
                if is_grain(cell.material) && falls_through_empty_air(below.material) {
                    if raft_rests_on_float_water_world(world, gx, gy - 1) {
                        // Cargo on a float raft — punch handles this; not a
                        // sky freefall (must not trip deep ×1024 settle).
                        raft_cargo = raft_cargo.saturating_add(1);
                        dirty.push((gx, gy));
                        dirty.push((gx, gy - 1));
                        let mut y = gy - 2;
                        while let Some(c) = world.get_cell(gx, y) {
                            if falls_through_empty_air(c.material) {
                                dirty.push((gx, y));
                                y -= 1;
                                continue;
                            }
                            if c.material == MaterialId::Air {
                                dirty.push((gx, y));
                            }
                            break;
                        }
                        continue;
                    }
                }
                if below.material == MaterialId::Air {
                    if falls_through_empty_air(cell.material)
                        && !cell.is_waterlogged_organic()
                        && floats_on_air_seat_world(world, below, gx, gy - 1)
                    {
                        if let Some(above) = world.get_cell(gx, gy + 1) {
                            if floats_on_air_seat_world(world, above, gx, gy + 1) {
                                dirty.push((gx, gy));
                                dirty.push((gx, gy + 1));
                                // Floating raft stack — not freefall into void.
                            }
                        }
                    } else {
                        dirty.push((gx, gy));
                        dirty.push((gx, gy - 1));
                        // Only empty/haze Air is sky freefall. Wet/lake Air
                        // under sand is a shore seat — counting it forced
                        // deep settle every wake on rainy demos.
                        if below.sat.0 <= GRAIN_REPOSE_HAZE_MAX {
                            freefall += 1;
                        }
                    }
                    continue;
                }
                // --- unstable slopes (supported repose grains only) ---
                if !is_repose_grain(cell.material) {
                    continue;
                }
                let max_step = {
                    let wet = crate::failure::pore_wetness_with(cell, &world.hydro);
                    crate::failure::grain_repose_max_step(cell.material, wet)
                };
                let surface_organic =
                    organic_is_surface_litter_world(world, gx, gy, cell);
                let gap = repose_gap_mode(cell, surface_organic);
                let settled_ooze = cell.material == MaterialId::Soil
                    || (cell.material == MaterialId::Organic && !surface_organic);
                let mut woke = false;
                for dx in [-1, 1] {
                    let sx = gx + dx;
                    let sy = gy - 1;
                    let Some(seat) = world.get_cell(sx, sy) else {
                        continue;
                    };
                    if seat.material != MaterialId::Air {
                        continue;
                    }
                    // Surface Organic / Snow — do not wake into full lake seats.
                    // Settled Soil/Organic: lake seats must wake (UW bank repose).
                    if seat.sat.is_full()
                        && (cell.material == MaterialId::Snow
                            || (cell.material == MaterialId::Organic && surface_organic))
                    {
                        continue;
                    }
                    // Dense grains: haze + lake seats wake; mid film only for
                    // Soil / settled Organic. Sand mid-film stays quiet.
                    if is_grain(cell.material) && !settled_ooze && !grain_repose_air_seat(seat)
                    {
                        continue;
                    }
                    if settled_ooze
                        && !grain_repose_air_seat(seat)
                        && !soil_land_film_seat(seat, world.get_cell(sx, sy - 1))
                    {
                        continue;
                    }
                    if diag_drop_exceeds_world(world, sx, gy, max_step, gap) {
                        dirty.push((gx, gy));
                        dirty.push((sx, sy));
                        woke = true;
                        break;
                    }
                }
                if woke || max_step > 0 {
                    continue;
                }
                for dx in [-1, 1] {
                    let sx = gx + dx;
                    let Some(seat) = world.get_cell(sx, gy) else {
                        continue;
                    };
                    if seat.material != MaterialId::Air {
                        continue;
                    }
                    if is_grain(cell.material)
                        && !settled_ooze
                        && !grain_repose_air_seat(seat)
                    {
                        continue;
                    }
                    if cell.material == MaterialId::Organic
                        && surface_organic
                        && seat.sat.is_full()
                    {
                        continue;
                    }
                    let Some(below_seat) = world.get_cell(sx, gy - 1) else {
                        continue;
                    };
                    // Walk-off wake: dry/haze drop only (not film / lake).
                    if below_seat.material != MaterialId::Air
                        || below_seat.sat.0 > GRAIN_REPOSE_HAZE_MAX
                    {
                        continue;
                    }
                    dirty.push((gx, gy));
                    dirty.push((sx, gy));
                    break;
                }
            }
        }
        if !saw_loose {
            clear_loose.push(coord);
        }
    }
    for (gx, gy) in dirty {
        world.touch_dirty(gx, gy);
    }
    for coord in clear_loose {
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            chunk.has_loose = false;
        }
    }
    GrainWake {
        freefall,
        raft_cargo,
    }
}

/// Re-dirty every grain / litter cell that has empty (or non-supporting)
/// Air directly below — and the Air seat itself.
///
/// Prefer [`wake_grains_for_settle`] in the hot tick (one scan).
pub fn wake_unsupported_grains(world: &mut World) {
    let _ = wake_grains_for_settle(world);
}

/// Re-dirty supported grains whose diagonal-down seat is steeper than
/// repose — vertical Organic/sand cliff faces that already have solid
/// under them never trip freefall wake, so without this they freeze as
/// walls after the first settle pass.
///
/// Prefer [`wake_grains_for_settle`] in the hot tick (one scan).
pub fn wake_unstable_slopes(world: &mut World) {
    let _ = wake_grains_for_settle(world);
}

fn diag_drop_exceeds_world(
    world: &World,
    dest_gx: i32,
    from_y: i32,
    max_step: i32,
    gap: ReposeGapMode,
) -> bool {
    let mut drop = 0i32;
    for dy in 1..=(max_step + 2) {
        let y = from_y - dy;
        let Some(c) = world.get_cell(dest_gx, y) else {
            break;
        };
        if c.material != MaterialId::Air {
            break;
        }
        if !repose_gap_air(c, gap) {
            break;
        }
        drop += 1;
        if drop > max_step {
            return true;
        }
    }
    drop > max_step
}

/// [`settle_loose_grains`] from a pre-planned active set (e.g. the
/// post-flow halo when water wrote nothing).
///
/// Interleaves fall and repose: a fall-then-repose split left Organic
/// cliff faces / overhangs because repose can undercut a stack and
/// those cells never freefall again in the same settle.
pub fn settle_loose_grains_regions(
    world: &mut World,
    initial: &[ActiveChunk],
    rooted: Option<&HashSet<(i32, i32)>>,
    max_passes: u32,
) {
    settle_loose_grains_regions_ex(world, initial, rooted, max_passes, true);
}

/// Like [`settle_loose_grains_regions`] with optional in-fall buoyancy.
///
/// After [`rise_and_soak_buoyant_litter`], tick settles with
/// `allow_buoyancy = false` so Organic does not one-cell bob through
/// wet Air for dozens of passes (FPS spike).
/// Keep only regions whose chunk may hold loose material (sticky
/// [`Chunk::has_loose`]). Bootstrap (no flags set yet) keeps everything.
fn keep_loose_regions(world: &World, active: &[ActiveChunk]) -> Vec<ActiveChunk> {
    if active.is_empty() {
        return Vec::new();
    }
    if !world.chunks.values().any(|c| c.has_loose) {
        return active.to_vec();
    }
    active
        .iter()
        .copied()
        .filter(|ac| {
            world
                .chunks
                .get(&ac.coord)
                .map(|c| c.has_loose)
                .unwrap_or(false)
        })
        .collect()
}

pub fn settle_loose_grains_regions_ex(
    world: &mut World,
    initial: &[ActiveChunk],
    rooted: Option<&HashSet<(i32, i32)>>,
    max_passes: u32,
    allow_buoyancy: bool,
) {
    let mut cur: Vec<ActiveChunk> = initial.to_vec();
    for _ in 0..max_passes {
        if cur.is_empty() {
            break;
        }
        let mut moved = 0u32;
        // Checkerboard so pull write-sets stay disjoint under rayon.
        // `water_column_grounded_ptrs` treats a missing lower chunk as
        // bedded, so float seats no longer freefall when the ocean floor
        // is outside this colour's ptr map.
        for pass in &partition_checkerboard(&cur) {
            if !pass.is_empty() {
                moved += apply_grain_fall_regions_ex(world, pass, allow_buoyancy);
            }
        }
        // Re-plan is global dirty, which on a wet world is dominated by pore
        // seepage in limestone / stone chunks. Repose can only move loose
        // grains, so filter to sticky-loose chunks — otherwise every
        // groundwater tick dragged the whole halo through the repose scan.
        let after_fall = keep_loose_regions(world, &plan_active(world));
        let repose_src = if after_fall.is_empty() {
            cur.clone()
        } else {
            after_fall
        };
        let repose_passes = partition_checkerboard(&repose_src);
        for pass in &repose_passes {
            moved += apply_grain_repose_regions(world, pass, rooted);
        }
        if moved == 0 {
            break;
        }
        let next = plan_active(world);
        if next.is_empty() {
            break;
        }
        cur = next;
    }
}

/// Grain fall restricted to a pre-planned active set.
///
/// Returns how many Air↔grain swaps this pass performed.
pub fn apply_grain_fall_regions(world: &mut World, active: &[ActiveChunk]) -> u32 {
    apply_grain_fall_regions_ex(world, active, true)
}

/// [`apply_grain_fall_regions`] with optional buoyancy pull.
pub fn apply_grain_fall_regions_ex(
    world: &mut World,
    active: &[ActiveChunk],
    allow_buoyancy: bool,
) -> u32 {
    let moves = std::sync::atomic::AtomicU32::new(0);
    // Parallel cell writes can't touch `World::mycelium_strains`; replay
    // share/lineage swaps after the pass so cream stays color-keyed.
    let share_swaps: Mutex<Vec<(i32, i32, i32, i32)>> = Mutex::new(Vec::new());
    for_each_region_parallel(world, active, |ptrs, wrap_width, ac| {
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                // SAFETY: see [`crate::parallel`].
                let Some(cur) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy) }) else {
                    continue;
                };
                if cur.material != MaterialId::Air {
                    continue;
                }
                let Some(above) =
                    (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy + 1) })
                else {
                    continue;
                };
                if is_grain(above.material) {
                    // Dense grains sink through any Air sat.
                } else if falls_through_empty_air(above.material) {
                    // Snow / Ice / Organic: drop through empty Air, haze,
                    // and *suspended* full-sat blobs. Float only on
                    // grounded lake / puddle surfaces (unless waterlogged).
                    if floats_on_air_seat_ptrs(ptrs, wrap_width, cur, gx, gy)
                        && !above.is_waterlogged_organic()
                    {                        // Floating raft cannot carry dense cargo. Walk up
                        // contiguous litter to the lowest grain and swap
                        // that grain with the water-contact litter cell.
                        let mut cargo_y = gy + 2;
                        for _ in 0..32 {
                            let Some(cargo) = (unsafe {
                                parallel::get_cell(ptrs, wrap_width, gx, cargo_y)
                            }) else {
                                break;
                            };
                            if falls_through_empty_air(cargo.material) {
                                cargo_y += 1;
                                continue;
                            }
                            if is_grain(cargo.material) {
                                unsafe {
                                    parallel::set_cell(
                                        ptrs, wrap_width, gx, gy + 1, cargo,
                                    );
                                    parallel::set_cell(
                                        ptrs, wrap_width, gx, cargo_y, above,
                                    );
                                }
                                share_swaps.lock().unwrap().push((gx, gy + 1, gx, cargo_y));
                                moves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            break;
                        }
                        continue;
                    }
                } else if allow_buoyancy {
                    // Buoyancy: only pull litter up through grounded full
                    // water. (Freeboard pop into empty Air was removed —
                    // on an uneven ocean surface it swapped Organic up
                    // then immediately fell back, burning 1024 settle
                    // passes per tick.) Skipped when rise already ran.
                    let Some(below) =
                        (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy - 1) })
                    else {
                        continue;
                    };
                    if !falls_through_empty_air(below.material) {
                        continue;
                    }
                    if below.is_waterlogged_organic() {
                        continue;
                    }
                    if !floats_on_air_seat_ptrs(ptrs, wrap_width, cur, gx, gy) {
                        continue;
                    }
                    unsafe {
                        parallel::set_cell(ptrs, wrap_width, gx, gy, below);
                        parallel::set_cell(ptrs, wrap_width, gx, gy - 1, cur);
                    }
                    share_swaps.lock().unwrap().push((gx, gy, gx, gy - 1));
                    moves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    continue;
                } else {
                    continue;
                }
                unsafe {
                    parallel::set_cell(ptrs, wrap_width, gx, gy, above);
                    parallel::set_cell(ptrs, wrap_width, gx, gy + 1, cur);
                }
                share_swaps.lock().unwrap().push((gx, gy, gx, gy + 1));
                moves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });
    for (ax, ay, bx, by) in share_swaps.into_inner().unwrap_or_default() {
        swap_mycelium_meta(world, ax, ay, bx, by);
    }
    moves.into_inner()
}

/// Max pore sat absorbed per tick from the water column under a floating
/// Organic / Soil raft. Keeps the surface cell full so the raft still floats.
const FLOAT_SOAK_RATE: u8 = 16;
/// Max cells a submerged litter grain may rise in one tick (column teleport).
const BUOYANT_RISE_MAX: i32 = 48;

/// Chunks that may hold Organic / Snow / Ice (sticky [`Chunk::has_buoyant`]).
fn buoyant_chunk_coords(world: &World) -> Vec<ChunkCoord> {
    world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_buoyant)
        .map(|(&coord, _)| coord)
        .collect()
}

fn collect_buoyant_litter(world: &World) -> Vec<(i32, i32)> {
    let mut litter = Vec::new();
    // Prefer buoyant sticky flag — sand-only shores used to scan every
    // `has_loose` chunk for litter that was never there.
    let coords = {
        let buoyant = buoyant_chunk_coords(world);
        if buoyant.is_empty() {
            // Legacy worlds / snow before has_buoyant was sticky.
            loose_chunk_coords(world)
        } else {
            buoyant
        }
    };
    for coord in coords {
        let x0 = coord.cx * CHUNK_CELLS_W as i32;
        let y0 = coord.cy * CHUNK_CELLS_H as i32;
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let cell = chunk.get(lx, ly);
                if falls_through_empty_air(cell.material) {
                    litter.push((x0 + lx as i32, y0 + ly as i32));
                }
            }
        }
    }
    litter
}

/// [`rise_buoyant_litter`] then [`soak_floating_litter`] with one litter scan.
///
/// Uses [`GrainConfig::default`] waterlog rate. Prefer
/// [`rise_and_soak_buoyant_litter_cfg`] when live Tab knobs are available.
pub fn rise_and_soak_buoyant_litter(world: &mut World) {
    rise_and_soak_buoyant_litter_cfg(world, &GrainConfig::default());
}

/// [`rise_and_soak_buoyant_litter`] with live [`GrainConfig`] waterlog rate.
pub fn rise_and_soak_buoyant_litter_cfg(world: &mut World, grain: &GrainConfig) {
    let mut litter = collect_buoyant_litter(world);
    if litter.is_empty() {
        return;
    }
    rise_buoyant_litter_list(world, &mut litter);
    soak_floating_litter_list(world, &litter, grain.organic_waterlog_rate);
}

/// Organic / Soil sitting on a grounded lake surface soaks pore water from
/// deeper cells in the water column (never drains the surface seat below
/// full — that would sink the raft and recreate submerged litter lines).
///
/// Once Organic pores are full, a slow probabilistic counter waterlogs the
/// cell ([`CellFlags::WATERLOGGED`]) so it eventually sinks.
///
/// Scans only buoyant litter cells (not the whole grid).
pub fn soak_floating_litter(world: &mut World) {
    soak_floating_litter_cfg(world, &GrainConfig::default());
}

/// [`soak_floating_litter`] with live [`GrainConfig`] waterlog rate.
pub fn soak_floating_litter_cfg(world: &mut World, grain: &GrainConfig) {
    let litter = collect_buoyant_litter(world);
    soak_floating_litter_list(world, &litter, grain.organic_waterlog_rate);
}

fn soak_floating_litter_list(world: &mut World, litter: &[(i32, i32)], waterlog_rate: f32) {
    let waterlog_rate = waterlog_rate.clamp(0.0, 1.0);
    for &(gx, gy) in litter {
        let Some(raft) = world.get_cell(gx, gy) else {
            continue;
        };
        if !matches!(raft.material, MaterialId::Organic | MaterialId::Soil) {
            continue;
        }
        let Some(surface) = world.get_cell(gx, gy - 1) else {
            continue;
        };
        // Cheap float seat: full standing water. Skip 512-deep grounded
        // walks — rise already seats litter, and mid-air full-sat is rare.
        if surface.material != MaterialId::Air || !surface.sat.is_full() {
            continue;
        }
        let cap = water_capacity_cell(raft, &world.hydro);
        if cap == 0 {
            continue;
        }
        // Fully soaked Organic: age toward waterlogging / sink.
        // Colonized mats (high mycelium) soak slower — raft toughness.
        if raft.sat.0 >= cap {
            if raft.material == MaterialId::Organic
                && !raft.flags.contains(CellFlags::WATERLOGGED)
            {
                let rate = waterlog_rate * mycelium_waterlog_scale(raft.mycelium());
                if rate > 0.0
                    && hash_prob(
                        world.seed.0,
                        gx,
                        world.tick.wrapping_add(gy as u64),
                        0x50A7_51A7u64,
                    ) < rate
                {
                    let mut wet = raft;
                    wet.flags.set(CellFlags::WATERLOGGED);
                    world.set_cell(gx, gy, wet);
                }
            }
            continue;
        }
        let room = cap - raft.sat.0;
        let want = room.min(FLOAT_SOAK_RATE);
        if want == 0 {
            continue;
        }
        let mut taken = 0u8;
        // Drain deepest water first so mid-column stays full for longer
        // (float seats remain grounded full-sat at the surface).
        let mut donors: Vec<i32> = Vec::new();
        for dy in 2..=24 {
            let y = gy - dy;
            let Some(donor) = world.get_cell(gx, y) else {
                break;
            };
            if donor.material != MaterialId::Air {
                break;
            }
            if donor.sat.0 > 0 {
                donors.push(y);
            }
        }
        for y in donors.into_iter().rev() {
            if taken >= want {
                break;
            }
            let Some(donor) = world.get_cell(gx, y) else {
                break;
            };
            if donor.material != MaterialId::Air || donor.sat.0 == 0 {
                continue;
            }
            let give = donor.sat.0.min(want - taken);
            if give == 0 {
                continue;
            }
            world.set_cell(
                gx,
                y,
                Cell {
                    sat: Sat(donor.sat.0 - give),
                    ..donor
                },
            );
            taken = taken.saturating_add(give);
        }
        if taken == 0 {
            continue;
        }
        let Some(raft_now) = world.get_cell(gx, gy) else {
            continue;
        };
        world.set_cell(
            gx,
            gy,
            Cell {
                sat: Sat(raft_now.sat.0.saturating_add(taken)),
                ..raft_now
            },
        );
    }
}

/// Base chance for a 1-cell Organic film to drift one column when
/// `|wind|·tile_cols ≈ 0.2` (typical mean climate). Tall sails multiply this.
const RAFT_DRIFT_BASE: f32 = 0.08;
/// Stream/cascade contribution blended into raft push (with wind).
/// Organic must visibly ride current instead of crusting the free surface.
const RAFT_DRIFT_FLOW_BASE: f32 = 0.32;
/// Extra sail from each Organic cell stacked above the waterline.
const RAFT_DRIFT_ORGANIC_SAIL: f32 = 0.35;
/// Extra sail per cell of living plant height above the raft top.
const RAFT_DRIFT_PLANT_SAIL: f32 = 0.55;

#[inline]
fn raft_air_sat(world: &World, gx: i32, gy: i32) -> i16 {
    match world.get_cell(gx, gy) {
        Some(c) if c.material == MaterialId::Air => c.sat.0 as i16,
        _ => 0,
    }
}

/// Local stream push under a floating raft: `(dir ±1, strength)`.
///
/// Combines sat-gradient heuristics with real [`flow_bias`] under the
/// mat so Organic rides the same current the water uses (shore crusts
/// used to ignore cascade lips and look glued).
fn raft_stream_push(world: &World, gx: i32, waterline_y: i32) -> (i32, f32) {
    let mut score_pos = 0.0f32;
    let mut score_neg = 0.0f32;

    // Primary: CA flow bias on the water seat(s) under / at the free surface.
    for y in [waterline_y, waterline_y - 1] {
        let Some(seat) = world.get_cell(gx, y) else {
            continue;
        };
        if seat.material != MaterialId::Air || seat.sat.0 < 180 {
            continue;
        }
        if let Some(dx) = flow_bias(world, gx, y, seat.sat) {
            let w = if y == waterline_y { 1.15 } else { 0.85 };
            if dx > 0 {
                score_pos += w;
            } else {
                score_neg += w;
            }
        }
    }

    let here = raft_air_sat(world, gx, waterline_y)
        .max(raft_air_sat(world, gx, waterline_y - 1));
    for dx in [-2_i32, -1, 1, 2] {
        let nx = world.wrap_x(gx + dx);
        let falloff = 1.0 / (dx.abs() as f32);
        let n = raft_air_sat(world, nx, waterline_y)
            .max(raft_air_sat(world, nx, waterline_y - 1));
        let mut score = 0.0f32;
        if here > n.saturating_add(24) {
            score += 0.45 * falloff;
        } else if n > here.saturating_add(24) {
            score -= 0.35 * falloff;
        }
        match world.get_cell(nx, waterline_y) {
            Some(c) if c.material == MaterialId::Air && !c.sat.is_full() => {
                if here > 64 {
                    score += 0.55 * falloff;
                }
            }
            None => {
                score += 0.25 * falloff;
            }
            _ => {}
        }
        let mut top_n = None;
        for dy in 0..8 {
            let y = waterline_y + dy;
            match world.get_cell(nx, y) {
                Some(c) if c.material == MaterialId::Air && c.sat.is_full() => {
                    top_n = Some(y);
                }
                Some(c) if c.material == MaterialId::Air => break,
                _ => break,
            }
        }
        if let Some(tn) = top_n {
            if waterline_y > tn {
                score += ((waterline_y - tn) as f32 / 5.0).min(0.5) * falloff;
            }
        } else if here > 64 {
            score += 0.40 * falloff;
        }
        if dx > 0 {
            score_pos += score.max(0.0);
            score_neg += (-score).max(0.0);
        } else {
            score_neg += score.max(0.0);
            score_pos += (-score).max(0.0);
        }
    }
    let dir = if score_pos >= score_neg { 1 } else { -1 };
    let strength = score_pos.max(score_neg).clamp(0.0, 1.65);
    (dir, strength)
}

/// Cheap float-seat check for raft drift (no 512-deep grounded walk).
///
/// Full [`floats_on_air_seat_world`] is correct for fall/buoyancy but too
/// expensive to re-run on every Organic cell every tick. Drift accepts
/// near-full standing water so mats can follow stream sat gradients (strict
/// `is_full()` left them glued on quiet rivers).
pub(crate) fn drift_float_seat(seat: Cell) -> bool {
    seat.material == MaterialId::Air && seat.sat.0 >= 200
}

fn drift_dest_clear(world: &World, nx: i32, bottom_y: i32, height: i32) -> bool {
    let Some(dest_seat) = world.get_cell(nx, bottom_y - 1) else {
        return false;
    };
    if !drift_float_seat(dest_seat) {
        return false;
    }
    for dy in 0..height {
        let Some(dest) = world.get_cell(nx, bottom_y + dy) else {
            return false;
        };
        if dest.material != MaterialId::Air || dest.sat.is_full() {
            return false;
        }
        if dy > 0 && !dest.sat.is_empty() && dest.sat.0 > GRAIN_REPOSE_HAZE_MAX {
            return false;
        }
    }
    true
}

/// Thin unbound film may wash onto a cascade lip / empty freeboard — then
/// fall or scour carries it downstream instead of sealing a shore ring.
fn drift_dest_wash_lip(world: &World, nx: i32, bottom_y: i32, height: i32) -> bool {
    if height > 1 {
        return false;
    }
    let Some(dest_seat) = world.get_cell(nx, bottom_y - 1) else {
        return false;
    };
    // Still need Air below (not solid bank). Float seats use the normal path.
    if dest_seat.material != MaterialId::Air || drift_float_seat(dest_seat) {
        return false;
    }
    let Some(dest) = world.get_cell(nx, bottom_y) else {
        return false;
    };
    dest.material == MaterialId::Air && !dest.sat.is_full()
}

fn drift_dest_ok(world: &World, nx: i32, bottom_y: i32, height: i32, allow_lip: bool) -> bool {
    if drift_dest_clear(world, nx, bottom_y, height) {
        return true;
    }
    allow_lip && drift_dest_wash_lip(world, nx, bottom_y, height)
}

/// Blend local stream push with climate wind for one raft column.
fn raft_column_push(world: &World, gx: i32, waterline_y: i32, wind_push: f32) -> f32 {
    let (dir, strength) = raft_stream_push(world, gx, waterline_y);
    let flow_push = dir as f32 * strength;
    if strength > 0.12 && flow_push.abs() > wind_push.abs() * 0.45 {
        flow_push + wind_push * 0.20
    } else {
        wind_push * 0.90 + flow_push * 0.95
    }
}

fn drift_move_column(world: &mut World, gx: i32, bottom_y: i32, height: i32, nx: i32) {
    for dy in (0..height).rev() {
        let y = bottom_y + dy;
        if world.get_cell(gx, y).is_none() || world.get_cell(nx, y).is_none() {
            continue;
        }
        swap_cells_preserving_mycelium(world, gx, y, nx, y);
    }
}

/// One floating Organic column at `gx`, if present: `(bottom_y, height)`.
pub fn floating_organic_column_at(world: &World, gx: i32) -> Option<(i32, i32)> {
    let gx = world.wrap_x(gx);
    let cx = gx.div_euclid(CHUNK_CELLS_W as i32);
    let lx = gx.rem_euclid(CHUNK_CELLS_W as i32) as usize;
    let mut best: Option<(i32, i32)> = None;
    for &coord in world.chunks.keys() {
        if coord.cx != cx {
            continue;
        }
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        if !chunk.has_organic {
            continue;
        }
        let y0 = coord.cy * CHUNK_CELLS_H as i32;
        for ly in 0..CHUNK_CELLS_H {
            let cell = chunk.get(lx, ly);
            if cell.material != MaterialId::Organic || cell.is_waterlogged_organic() {
                continue;
            }
            let gy = y0 + ly as i32;
            if let Some(below_org) = world.get_cell(gx, gy - 1) {
                if below_org.material == MaterialId::Organic {
                    continue;
                }
            }
            let Some(seat) = world.get_cell(gx, gy - 1) else {
                continue;
            };
            if !drift_float_seat(seat) {
                continue;
            }
            let mut height = 1i32;
            while let Some(above) = world.get_cell(gx, gy + height) {
                if above.material != MaterialId::Organic {
                    break;
                }
                height += 1;
                if height > 48 {
                    break;
                }
            }
            best = Some((gy, height));
        }
    }
    best
}

/// Floating Organic columns: `gx → (waterline_y, stack_height)`.
///
/// Skips chunks that never held Organic (`has_organic`).
pub fn collect_floating_organic_columns(
    world: &World,
) -> std::collections::HashMap<i32, (i32, i32)> {
    let mut columns: std::collections::HashMap<i32, (i32, i32)> =
        std::collections::HashMap::new();
    for &coord in world.chunks.keys() {
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        if !chunk.has_organic {
            continue;
        }
        let x0 = coord.cx * CHUNK_CELLS_W as i32;
        let y0 = coord.cy * CHUNK_CELLS_H as i32;
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let cell = chunk.get(lx, ly);
                if cell.material != MaterialId::Organic {
                    continue;
                }
                if cell.is_waterlogged_organic() {
                    continue;
                }
                let gx = x0 + lx as i32;
                let gy = y0 + ly as i32;
                if let Some(below_org) = world.get_cell(gx, gy - 1) {
                    if below_org.material == MaterialId::Organic {
                        continue;
                    }
                }
                let Some(seat) = world.get_cell(gx, gy - 1) else {
                    continue;
                };
                if !drift_float_seat(seat) {
                    continue;
                }
                let mut height = 1i32;
                while let Some(above) = world.get_cell(gx, gy + height) {
                    if above.material != MaterialId::Organic {
                        break;
                    }
                    height += 1;
                    if height > 48 {
                        break;
                    }
                }
                columns.insert(gx, (gy, height));
            }
        }
    }
    columns
}

/// Float columns near plant crowns (`xs` ± `pad`) — organism tick path.
pub fn collect_floating_organic_columns_near(
    world: &World,
    xs: &[i32],
    pad: i32,
) -> std::collections::HashMap<i32, (i32, i32)> {
    let mut columns: std::collections::HashMap<i32, (i32, i32)> =
        std::collections::HashMap::new();
    if xs.is_empty() {
        return columns;
    }
    // No Organic anywhere → free.
    if !world.chunks.values().any(|c| c.has_organic) {
        return columns;
    }
    let pad = pad.max(0);
    let mut seen = std::collections::HashSet::new();
    for &x in xs {
        for dx in -pad..=pad {
            let gx = world.wrap_x(x + dx);
            if !seen.insert(gx) {
                continue;
            }
            if let Some(col) = floating_organic_column_at(world, gx) {
                columns.insert(gx, col);
            }
        }
    }
    columns
}

/// Wind shove for Organic piles floating on grounded lakes.
///
/// Loose litter drifts per-column (wind can tear thin mats apart). Columns
/// in `root_bound_columns` move as one raft. Returns
/// `(columns_moved, wind_sign, source_columns_that_moved)`.
///
/// `plant_tops`: world-x → max plant cell y for sail area.
///
/// Uses [`GrainConfig::default`] raft bind radius. Prefer
/// [`drift_floating_organic_cfg`] when live Tab knobs are available.
pub fn drift_floating_organic(
    world: &mut World,
    wind_vx_tiles: f32,
    tile_cols: i32,
    plant_tops: Option<&std::collections::HashMap<i32, i32>>,
    root_bound_columns: Option<&HashSet<i32>>,
) -> (u32, i32, HashSet<i32>) {
    drift_floating_organic_cfg(
        world,
        wind_vx_tiles,
        tile_cols,
        plant_tops,
        root_bound_columns,
        &GrainConfig::default(),
    )
}

/// [`drift_floating_organic`] with live [`GrainConfig`] raft bind radius.
pub fn drift_floating_organic_cfg(
    world: &mut World,
    wind_vx_tiles: f32,
    tile_cols: i32,
    plant_tops: Option<&std::collections::HashMap<i32, i32>>,
    root_bound_columns: Option<&HashSet<i32>>,
    grain: &GrainConfig,
) -> (u32, i32, HashSet<i32>) {
    let columns = collect_floating_organic_columns(world);
    drift_floating_organic_columns_cfg(
        world,
        &columns,
        wind_vx_tiles,
        tile_cols,
        plant_tops,
        root_bound_columns,
        grain,
    )
}

/// Like [`drift_floating_organic`] with a precomputed floating-Organic map
/// (avoids a second full-world scan when the plant layer already collected).
pub fn drift_floating_organic_columns(
    world: &mut World,
    columns: &std::collections::HashMap<i32, (i32, i32)>,
    wind_vx_tiles: f32,
    tile_cols: i32,
    plant_tops: Option<&std::collections::HashMap<i32, i32>>,
    root_bound_columns: Option<&HashSet<i32>>,
) -> (u32, i32, HashSet<i32>) {
    drift_floating_organic_columns_cfg(
        world,
        columns,
        wind_vx_tiles,
        tile_cols,
        plant_tops,
        root_bound_columns,
        &GrainConfig::default(),
    )
}

/// [`drift_floating_organic_columns`] with live [`GrainConfig`] raft bind radius.
pub fn drift_floating_organic_columns_cfg(
    world: &mut World,
    columns: &std::collections::HashMap<i32, (i32, i32)>,
    wind_vx_tiles: f32,
    tile_cols: i32,
    plant_tops: Option<&std::collections::HashMap<i32, i32>>,
    root_bound_columns: Option<&HashSet<i32>>,
    grain: &GrainConfig,
) -> (u32, i32, HashSet<i32>) {
    if columns.is_empty() {
        return (0, 0, HashSet::new());
    }

    let wind = wind_vx_tiles * tile_cols.max(1) as f32;
    let wind_push = wind.clamp(-1.5, 1.5);

    let bind_radius = grain.raft_root_bind_radius.max(0);

    // Precomputed by the plant layer (span of every root on the mat).
    let mut bound: HashSet<i32> = root_bound_columns
        .map(|s| s.iter().copied().filter(|x| columns.contains_key(x)).collect())
        .unwrap_or_default();
    // Cream mycelium also felts floating mats into cohesive rafts.
    let mut myc_bound: HashSet<i32> = HashSet::new();
    for (&gx, &(bottom, height)) in columns {
        if column_max_mycelium(world, gx, bottom, height) >= MYCELIUM_RAFT_BIND_MIN {
            myc_bound.insert(gx);
            bound.insert(gx);
        }
    }
    // Dilate so a holdfast still grips neighbouring litter (0 = body span only).
    if bind_radius > 0 {
        let claimed: Vec<i32> = bound.iter().copied().collect();
        for c in claimed {
            for dx in -bind_radius..=bind_radius {
                let nx = world.wrap_x(c + dx);
                if columns.contains_key(&nx) {
                    bound.insert(nx);
                }
            }
        }
    }
    // Mycelium mats stitch one neighbour so cream edges don't fray.
    if !myc_bound.is_empty() {
        let claimed: Vec<i32> = myc_bound.iter().copied().collect();
        for c in claimed {
            for dx in -1..=1 {
                let nx = world.wrap_x(c + dx);
                if columns.contains_key(&nx) {
                    bound.insert(nx);
                }
            }
        }
    }

    // Union-find over bound columns connected by adjacency.
    let mut parent: std::collections::HashMap<i32, i32> =
        bound.iter().map(|&x| (x, x)).collect();
    fn find(parent: &mut std::collections::HashMap<i32, i32>, x: i32) -> i32 {
        let mut v = x;
        while parent.get(&v).copied().unwrap_or(v) != v {
            let p = parent[&v];
            let gp = parent.get(&p).copied().unwrap_or(p);
            parent.insert(v, gp);
            v = p;
        }
        v
    }
    fn union(parent: &mut std::collections::HashMap<i32, i32>, a: i32, b: i32) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent.insert(ra, rb);
        }
    }
    let mut bound_list: Vec<i32> = bound.iter().copied().collect();
    bound_list.sort_unstable();
    for window in bound_list.windows(2) {
        let a = window[0];
        let b = window[1];
        // Adjacent in world-x (including wrap seam: treat wrap_x(a+1)==b).
        if b == a + 1 || world.wrap_x(a + 1) == b || world.wrap_x(b + 1) == a {
            union(&mut parent, a, b);
        }
    }

    let mut components: std::collections::HashMap<i32, Vec<i32>> =
        std::collections::HashMap::new();
    for &c in &bound_list {
        let r = find(&mut parent, c);
        components.entry(r).or_default().push(c);
    }

    let mut moved_cols: HashSet<i32> = HashSet::new();
    let mut moved = 0u32;
    // Report dominant sign for HUD / tests (mean of successful moves).
    let mut sign_acc = 0i32;

    // 1) Root-bound rafts — one roll, whole component moves or none.
    //    Push is local to the component (still-lake mats must not dilute
    //    river current into a global near-zero mean).
    let mut comp_list: Vec<Vec<i32>> = components.into_values().collect();
    for comp in &mut comp_list {
        comp.sort_unstable();
        let mut push_sum = 0.0f32;
        let mut flow_abs = 0.0f32;
        for &gx in comp.iter() {
            let Some(&(bottom, _)) = columns.get(&gx) else {
                continue;
            };
            let p = raft_column_push(world, gx, bottom - 1, wind_push);
            push_sum += p;
            flow_abs += p.abs();
        }
        let n = comp.len().max(1) as f32;
        let push = push_sum / n;
        if push.abs() < 1e-4 {
            continue;
        }
        let sign: i32 = if push >= 0.0 { 1 } else { -1 };
        let speed = push.abs();
        let flow = (flow_abs / n).min(1.45);
        if sign > 0 {
            comp.reverse(); // downwind first within the mat
        }
        let mut sail = 1.0f32;
        for &gx in comp.iter() {
            let Some(&(bottom, height)) = columns.get(&gx) else {
                continue;
            };
            let plant_h = plant_tops
                .and_then(|m| m.get(&gx).copied())
                .map(|top| (top - (bottom + height - 1)).max(0))
                .unwrap_or(0);
            sail = sail
                .max(
                    1.0
                        + RAFT_DRIFT_ORGANIC_SAIL * (height - 1) as f32
                        + RAFT_DRIFT_PLANT_SAIL * plant_h as f32,
                );
        }
        // Bound mats get a small cohesion bonus (harder to strand, easier sail).
        sail += 0.25 * (comp.len() as f32).sqrt();
        let p = (speed * (RAFT_DRIFT_BASE + RAFT_DRIFT_FLOW_BASE * flow.min(1.0)) * sail)
            .clamp(0.0, 0.9);
        let key = comp.first().copied().unwrap_or(0);
        if hash_prob(world.seed.0, key, world.tick, 0xD61F_4AF7) >= p {
            continue;
        }
        let mut all_clear = true;
        for &gx in comp.iter() {
            let Some(&(bottom, height)) = columns.get(&gx) else {
                all_clear = false;
                break;
            };
            let nx = world.wrap_x(gx + sign);
            // Destination may be another column of this same raft vacating —
            // allow if that dest column is also in the component.
            if comp.iter().any(|&c| c == nx) {
                continue;
            }
            // Bound mats stay on float seats (no lip wash — keep holdfasts).
            if !drift_dest_clear(world, nx, bottom, height) {
                all_clear = false;
                break;
            }
        }
        if !all_clear {
            continue;
        }
        for &gx in comp.iter() {
            let Some(&(bottom, height)) = columns.get(&gx) else {
                continue;
            };
            let nx = world.wrap_x(gx + sign);
            if comp.iter().any(|&c| c == nx) {
                // Shift within the raft footprint: defer to a two-phase move.
                continue;
            }
            drift_move_column(world, gx, bottom, height, nx);
            moved_cols.insert(gx);
            moved += 1;
            sign_acc += sign;
        }
        // Second phase: columns whose dest was in-component (convoy).
        // Process again downwind→upwind into seats freed above.
        for &gx in comp.iter() {
            if moved_cols.contains(&gx) {
                continue;
            }
            let Some(&(bottom, height)) = columns.get(&gx) else {
                continue;
            };
            let nx = world.wrap_x(gx + sign);
            if !drift_dest_clear(world, nx, bottom, height) {
                continue;
            }
            drift_move_column(world, gx, bottom, height, nx);
            moved_cols.insert(gx);
            moved += 1;
            sign_acc += sign;
        }
    }

    // 2) Loose (unbound) litter — may blow apart / wash over lips column by column.
    let mut loose: Vec<i32> = columns
        .keys()
        .copied()
        .filter(|x| !bound.contains(x))
        .collect();
    loose.sort_unstable();
    // Prefer processing +push first so convoy into freed seats works; when
    // signs differ we sort by |x| after computing push per column.
    loose.sort_by_key(|&gx| {
        let Some(&(bottom, _)) = columns.get(&gx) else {
            return 0i32;
        };
        let push = raft_column_push(world, gx, bottom - 1, wind_push);
        // Negative push → process low x first; positive → high x first.
        if push >= 0.0 {
            -gx
        } else {
            gx
        }
    });
    for gx in loose {
        let Some(&(bottom_y, height)) = columns.get(&gx) else {
            continue;
        };
        let push = raft_column_push(world, gx, bottom_y - 1, wind_push);
        if push.abs() < 1e-4 {
            continue;
        }
        let sign: i32 = if push >= 0.0 { 1 } else { -1 };
        let speed = push.abs();
        let (_, flow_strength) = raft_stream_push(world, gx, bottom_y - 1);
        let nx = world.wrap_x(gx + sign);
        // Thin unbound film may wash onto cascade lips when current is strong.
        let allow_lip = height <= 1 && flow_strength >= 0.15;
        if !drift_dest_ok(world, nx, bottom_y, height, allow_lip) {
            continue;
        }
        let plant_h = plant_tops
            .and_then(|m| m.get(&gx).copied())
            .map(|top| (top - (bottom_y + height - 1)).max(0))
            .unwrap_or(0);
        let sail = 1.0
            + RAFT_DRIFT_ORGANIC_SAIL * (height - 1) as f32
            + RAFT_DRIFT_PLANT_SAIL * plant_h as f32;
        // Unbound film in a real current always rides it — probabilistic
        // drift left mats damming lakes and combing the free surface.
        let unbound_current = height <= 2
            && flow_strength >= 0.20
            && column_max_mycelium(world, gx, bottom_y, height) < MYCELIUM_RAFT_BIND_MIN;
        let p = if unbound_current {
            1.0
        } else {
            (speed
                * (RAFT_DRIFT_BASE + RAFT_DRIFT_FLOW_BASE * flow_strength.min(1.0))
                * sail)
                .clamp(0.0, 0.90)
        };
        if !unbound_current && hash_prob(world.seed.0, gx, world.tick, 0xD61F_1005) >= p {
            continue;
        }
        drift_move_column(world, gx, bottom_y, height, nx);
        moved_cols.insert(gx);
        moved += 1;
        sign_acc += sign;
    }

    let report_sign = if sign_acc >= 0 { 1 } else { -1 };
    (moved, report_sign, moved_cols)
}

/// Current shoves thin unbound floating Organic blocking a cascade /
/// head-drop so mats do not dam lakes into comb teeth.
///
/// Call after wind/stream drift. Deterministic: if water beside the mat
/// has [`flow_bias`] into/along the film and the down-current seat is
/// clear (or a lip), the film moves one column.
pub fn shove_floating_organic_with_current(world: &mut World) -> u32 {
    let columns = collect_floating_organic_columns(world);
    if columns.is_empty() {
        return 0;
    }
    let mut keys: Vec<i32> = columns.keys().copied().collect();
    keys.sort_unstable();
    let mut moved = 0u32;
    // Snapshot dest occupancy so two mats don't swap into each other.
    let mut claimed: HashSet<i32> = HashSet::new();
    for gx in keys {
        let Some(&(bottom, height)) = columns.get(&gx) else {
            continue;
        };
        if height > 2 {
            continue;
        }
        if column_max_mycelium(world, gx, bottom, height) >= MYCELIUM_RAFT_BIND_MIN {
            continue;
        }
        let waterline = bottom - 1;
        let mut sign = 0i32;
        // Bias under the mat, then from wet neighbours pointing at us.
        for y in [waterline, waterline - 1] {
            let Some(seat) = world.get_cell(gx, y) else {
                continue;
            };
            if seat.material != MaterialId::Air || seat.sat.0 < 180 {
                continue;
            }
            if let Some(dx) = flow_bias(world, gx, y, seat.sat) {
                sign = dx;
                break;
            }
        }
        if sign == 0 {
            for dx in [-1_i32, 1] {
                let nx = world.wrap_x(gx + dx);
                let Some(n) = world.get_cell(nx, waterline) else {
                    continue;
                };
                if n.material != MaterialId::Air || n.sat.0 < 180 {
                    continue;
                }
                if let Some(bdx) = flow_bias(world, nx, waterline, n.sat) {
                    // Neighbour current toward the mat or continuing past it.
                    if bdx == -dx || bdx == dx {
                        sign = bdx;
                        break;
                    }
                }
            }
        }
        if sign == 0 {
            continue;
        }
        let nx = world.wrap_x(gx + sign);
        if claimed.contains(&nx) || columns.contains_key(&nx) {
            continue;
        }
        let allow_lip = height <= 1;
        if !drift_dest_ok(world, nx, bottom, height, allow_lip) {
            continue;
        }
        drift_move_column(world, gx, bottom, height, nx);
        claimed.insert(nx);
        moved = moved.saturating_add(1);
    }
    moved
}

/// Lift submerged Snow/Ice/Organic through grounded full water, and pop
/// litter that still has lake water beside it onto freeboard Air.
///
/// Litter-centric (not a full-grid × height scan): finds buoyant cells once,
/// then bubbles each up at most [`BUOYANT_RISE_MAX`] steps this tick.
pub fn rise_buoyant_litter(world: &mut World) {
    let mut litter = collect_buoyant_litter(world);
    rise_buoyant_litter_list(world, &mut litter);
}

fn rise_buoyant_litter_list(world: &mut World, litter: &mut [(i32, i32)]) {
    if litter.is_empty() {
        return;
    }
    // Memoize grounded lake seats for this rise pass (flooded Organic used
    // to re-walk ≤512 cells per step × rise max).
    let mut grounded: HashMap<(i32, i32), bool> = HashMap::new();
    let mut float_seat = |world: &World, seat: Cell, gx: i32, gy: i32| -> bool {
        if seat.material != MaterialId::Air || !seat.sat.is_full() {
            return false;
        }
        if let Some(&g) = grounded.get(&(gx, gy)) {
            return g;
        }
        let g = water_column_grounded_world(world, gx, gy);
        grounded.insert((gx, gy), g);
        g
    };
    // Bottom-up so lower cells rise before upper ones in the same column.
    litter.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    for entry in litter.iter_mut() {
        let gx = entry.0;
        let gy0 = entry.1;
        let Some(here) = world.get_cell(gx, gy0) else {
            continue;
        };
        if !falls_through_empty_air(here.material) || here.is_waterlogged_organic() {
            continue;
        }
        // Find freeboard in one pass, then teleport (endpoint swap) instead
        // of bubbling one cell at a time through settle.
        let mut top = gy0;
        for step in 1..=BUOYANT_RISE_MAX {
            let y = gy0 + step;
            let Some(above) = world.get_cell(gx, y) else {
                break;
            };
            if above.material != MaterialId::Air {
                break;
            }
            if !float_seat(world, above, gx, y) {
                break;
            }
            top = y;
        }
        if top > gy0 {
            swap_cells_preserving_mycelium(world, gx, gy0, gx, top);
            entry.1 = top;
        }
    }
}

/// Angle-of-repose slide: supported grains move diagonally down into Air
/// when the local step is steeper than [`grain_max_stable_step`].
///
/// Sand (`max_step = 0`) won't hold a 1-cell cliff — piles flatten.
/// LooseRock (`max_step ≥ 1`) can hold short stairs. Wet grains
/// (pore sat or standing water below) loosen by one step; Clay uses a
/// dry-powder / plastic / mud curve instead. Living Root cells raise
/// the local step via [`ROOT_REPOSE_STEP_BONUS`]. Organic mycelium
/// intensity (0..=255) raises it the same way — cream mats feel sticky.
/// One move per cell per pass; run after [`apply_grain_fall`].
pub fn apply_grain_repose(world: &mut World) {
    apply_grain_repose_bound(world, None);
}

/// [`apply_grain_repose`] with an optional set of living root cells.
pub fn apply_grain_repose_bound(
    world: &mut World,
    rooted: Option<&HashSet<(i32, i32)>>,
) {
    let regions = regions_for_standalone(world);
    for pass in partition_checkerboard(&regions) {
        apply_grain_repose_regions(world, &pass, rooted);
    }
}

/// Repose slide restricted to a pre-planned active set.
///
/// Returns how many diagonal slides this pass performed.
pub fn apply_grain_repose_regions(
    world: &mut World,
    active: &[ActiveChunk],
    rooted: Option<&HashSet<(i32, i32)>>,
) -> u32 {
    apply_repose_pass(world, active, None, f32::INFINITY, rooted)
}

/// Cold snap avalanche: wet sand loosens, snow/hillside ice spill onto
/// lake ice (including a wet film on the lid). Open water is still
/// refused for snow/ice. Call from the demo after `Temperature::step`
/// and before [`crate::phase::apply_phase`] so thin lids can then break
/// under the new load.
pub fn apply_cold_avalanche(world: &mut World, temp: &Temperature, freeze_point_c: f32) {
    apply_cold_avalanche_bound(world, temp, freeze_point_c, None);
}

/// [`apply_cold_avalanche`] with optional living-root binding.
pub fn apply_cold_avalanche_bound(
    world: &mut World,
    temp: &Temperature,
    freeze_point_c: f32,
    rooted: Option<&HashSet<(i32, i32)>>,
) {
    // Prefer the dirty halo when present, but never fall back to a full
    // world scan — Super-Server stress paid ~8 ms/tick when post-physics
    // dirty was empty and `regions_for_standalone` expanded to all chunks.
    // Loose + Moore covers snow/ice/sand sources and Air seats next door.
    //
    // Warm empty-dirty: skip the Moore insurance entirely. Ambient repose
    // already handles snow/sand; this pass only adds cold wet-sand, ice
    // glaze, and snow→ice-lid seats. Stress (mean ~25°C, snow=0) was
    // still paying ~4 ms walking ~100 full loose chunks.
    let planned = plan_active(world);
    let regions = if planned.is_empty() {
        if temp.mean() > freeze_point_c {
            return;
        }
        regions_loose_moore(world)
    } else {
        let loose = regions_loose_moore(world);
        if loose.is_empty() {
            planned
        } else {
            let loose_coords: HashSet<ChunkCoord> =
                loose.iter().map(|ac| ac.coord).collect();
            let filtered: Vec<_> = planned
                .into_iter()
                .filter(|ac| loose_coords.contains(&ac.coord))
                .collect();
            if filtered.is_empty() {
                if temp.mean() > freeze_point_c {
                    return;
                }
                loose
            } else {
                filtered
            }
        }
    };
    for pass in partition_checkerboard(&regions) {
        apply_repose_pass(world, &pass, Some(temp), freeze_point_c, rooted);
    }
}

fn apply_repose_pass(
    world: &mut World,
    active: &[ActiveChunk],
    temp: Option<&Temperature>,
    freeze_point_c: f32,
    rooted: Option<&HashSet<(i32, i32)>>,
) -> u32 {
    let seed = world.seed.0;
    let tick_no = world.tick;
    let cold_mode = temp.is_some();
    let hydro = world.hydro;
    let moves = std::sync::atomic::AtomicU32::new(0);
    let share_swaps: Mutex<Vec<(i32, i32, i32, i32)>> = Mutex::new(Vec::new());
    // Moore ptr map + serial: repose writes horizontally across seams.
    for_each_region_serial_moore(world, active, |ptrs, wrap_width, ac| {
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                // SAFETY: see [`crate::parallel`].
                let Some(dest) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy) }) else {
                    continue;
                };
                if dest.material != MaterialId::Air {
                    continue;
                }
                let below_dest =
                    unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy - 1) };
                let prefer_pos = hash_prob(seed, gx, tick_no, 0x5A17_D1EEu64) >= 0.5;
                let order: [i32; 2] = if prefer_pos { [1, -1] } else { [-1, 1] };

                // Diagonal-down pull (standard repose / cold avalanche).
                let mut moved = false;
                for &from_dx in &order {
                    let sx = gx + from_dx;
                    let sy = gy + 1;
                    let Some(src) =
                        (unsafe { parallel::get_cell(ptrs, wrap_width, sx, sy) })
                    else {
                        continue;
                    };
                    let below_src =
                        unsafe { parallel::get_cell(ptrs, wrap_width, sx, sy - 1) };
                    if !avalanche_source_ok(src.material, below_src, cold_mode) {
                        continue;
                    }
                    match below_src {
                        Some(b) if b.material == MaterialId::Air => continue,
                        None => continue,
                        _ => {}
                    }
                    let surface_organic =
                        organic_is_surface_litter_ptrs(ptrs, wrap_width, sx, sy, src);
                    if !avalanche_seat_ok(src, dest, below_dest, surface_organic) {
                        continue;
                    }
                    let cold = temp
                        .map(|t| t.at_cell(sx, sy) <= freeze_point_c)
                        .unwrap_or(false);
                    if cold_mode {
                        // Ambient [`apply_grain_repose`] already ran in tick.
                        // This pass only adds cold wet-sand, snow→ice, and
                        // hillside ice motion.
                        let allowed = match src.material {
                            MaterialId::Snow => true,
                            MaterialId::Ice => cold,
                            m if is_grain(m) => cold,
                            _ => false,
                        };
                        if !allowed {
                            continue;
                        }
                    }
                    let mut max_step = if src.material == MaterialId::Ice {
                        0 // hillside glaze — no 1-cell cliff
                    } else {
                        // Clay plasticity + F2a wet loosen for other grains.
                        crate::failure::grain_repose_max_step(
                            src.material,
                            wet_frac_for(src, below_src, &hydro),
                        )
                    };
                    // Live roots bind the grain (column Ecology.root_density).
                    if rooted.is_some_and(|r| r.contains(&(sx, sy))) {
                        max_step = max_step.saturating_add(ROOT_REPOSE_STEP_BONUS);
                    }
                    // Cream mycelium felts Organic — same 0..=255 intensity.
                    // Wash-wet Organic (standing water neighbour) sheds the
                    // felt so mats cannot hold vertical dams in a lake.
                    if !(src.material == MaterialId::Organic
                        && organic_wash_wet_ptrs(ptrs, wrap_width, sx, sy))
                    {
                        max_step = max_step.saturating_add(mycelium_repose_bonus(src));
                    }
                    let gap = repose_gap_mode(src, surface_organic);
                    if !diag_drop_exceeds(ptrs, wrap_width, gx, sy, max_step, gap) {
                        continue;
                    }
                    write_repose_swap(
                        ptrs,
                        wrap_width,
                        &hydro,
                        gx,
                        gy,
                        dest,
                        sx,
                        sy,
                        src,
                        &share_swaps,
                    );
                    moved = true;
                    break;
                }
                if moved {
                    moves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    continue;
                }
                // Same-Y walk-off: max_step==0 grains on a rock ledge where
                // diagonal-down is blocked can still step sideways into Air
                // that opens downward — clears 2–3 cell sand lips on slopes.
                // Below must be dry/haze only: a film toe under a lake seat
                // used to let sand walk into standing water and bypass the
                // shore-film refuse (fleck cycle). Diagonal-down into full
                // lake water still handles gentler submerged banks.
                if !cold_mode {
                    let open_drop = matches!(
                        below_dest,
                        Some(b)
                            if b.material == MaterialId::Air
                                && b.sat.0 <= GRAIN_REPOSE_HAZE_MAX
                    );
                    if !open_drop {
                        continue;
                    }
                    for &from_dx in &order {
                        let sx = gx + from_dx;
                        let sy = gy;
                        let Some(src) =
                            (unsafe { parallel::get_cell(ptrs, wrap_width, sx, sy) })
                        else {
                            continue;
                        };
                        let below_src =
                            unsafe { parallel::get_cell(ptrs, wrap_width, sx, sy - 1) };
                        let mut step = crate::failure::grain_repose_max_step(
                            src.material,
                            wet_frac_for(src, below_src, &hydro),
                        );
                        if rooted.is_some_and(|r| r.contains(&(sx, sy))) {
                            step = step.saturating_add(ROOT_REPOSE_STEP_BONUS);
                        }
                        if !(src.material == MaterialId::Organic
                            && organic_wash_wet_ptrs(ptrs, wrap_width, sx, sy))
                        {
                            step = step.saturating_add(mycelium_repose_bonus(src));
                        }
                        // Rooted sand / plastic clay can hold short stairs —
                        // don't walk off. Colonized Organic likewise.
                        if step > 0 {
                            continue;
                        }
                        if !is_grain(src.material)
                            && !matches!(src.material, MaterialId::Organic | MaterialId::Soil)
                        {
                            continue;
                        }
                        match below_src {
                            Some(b) if b.material == MaterialId::Air => continue,
                            None => continue,
                            _ => {}
                        }
                        let surface_organic =
                            organic_is_surface_litter_ptrs(ptrs, wrap_width, sx, sy, src);
                        if !avalanche_seat_ok(src, dest, below_dest, surface_organic) {
                            continue;
                        }
                        write_repose_swap(
                            ptrs,
                            wrap_width,
                            &hydro,
                            gx,
                            gy,
                            dest,
                            sx,
                            sy,
                            src,
                            &share_swaps,
                        );
                        moves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    continue;
                }
                // Same-Y smear: cold wet sand (or snow) onto an ice lid seat.
                if !seat_on_ice(below_dest) {
                    continue;
                }
                for &from_dx in &order {
                    let sx = gx + from_dx;
                    let sy = gy;
                    let Some(src) =
                        (unsafe { parallel::get_cell(ptrs, wrap_width, sx, sy) })
                    else {
                        continue;
                    };
                    let below_src =
                        unsafe { parallel::get_cell(ptrs, wrap_width, sx, sy - 1) };
                    let cold = temp
                        .map(|t| t.at_cell(sx, sy) <= freeze_point_c)
                        .unwrap_or(false);
                    if !cold {
                        continue;
                    }
                    let wet = grain_is_wet(src, below_src);
                    let can_smear = (is_grain(src.material) && wet)
                        || src.material == MaterialId::Snow
                        || (src.material == MaterialId::Ice
                            && hillside_ice_support(below_src));
                    if !can_smear {
                        continue;
                    }
                    let surface_organic =
                        organic_is_surface_litter_ptrs(ptrs, wrap_width, sx, sy, src);
                    if !avalanche_seat_ok(src, dest, below_dest, surface_organic) {
                        continue;
                    }
                    // Must be supported at source (not freefall).
                    match below_src {
                        Some(b) if b.material == MaterialId::Air => continue,
                        None => continue,
                        _ => {}
                    }
                    write_repose_swap(
                        ptrs,
                        wrap_width,
                        &hydro,
                        gx,
                        gy,
                        dest,
                        sx,
                        sy,
                        src,
                        &share_swaps,
                    );
                    moves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    break;
                }
            }
        }
    });
    for (ax, ay, bx, by) in share_swaps.into_inner().unwrap_or_default() {
        swap_mycelium_meta(world, ax, ay, bx, by);
    }
    moves.into_inner()
}

/// Swap grain into a diagonal/lateral Air seat.
///
/// Underwater seats:
/// - **Full lake water** — plain swap leaves standing water in the
///   vacated cell (mass conserved; gentler submerged banks).
/// - **Empty / haze bubble** beside the lake — soak seat sat into pores
///   and **steal** neighbour standing water into the vacated cell so the
///   slope face does not sky-flash. Never mint a fresh full water cell.
/// Mid shore film is refused upstream ([`avalanche_seat_ok`]).
fn write_repose_swap(
    ptrs: &parallel::ChunkPtrMap,
    wrap_width: Option<i32>,
    hydro: &HydroOverrides,
    dest_x: i32,
    dest_y: i32,
    dest: Cell,
    src_x: i32,
    src_y: i32,
    src: Cell,
    share_swaps: &Mutex<Vec<(i32, i32, i32, i32)>>,
) {
    let submerged = dest.sat.0 >= 200
        || air_has_standing_water_neighbor(ptrs, wrap_width, dest_x, dest_y)
        || air_has_standing_water_neighbor(ptrs, wrap_width, src_x, src_y);
    if is_grain(src.material) && submerged && dest.sat.0 < 200 {
        let cap = water_capacity_cell(src, hydro);
        let room = cap.saturating_sub(src.sat.0);
        let into_pore = dest.sat.0.min(room);
        let mut placed = src;
        placed.sat = Sat(src.sat.0.saturating_add(into_pore));
        if let Some(fill) =
            steal_standing_water_neighbor(ptrs, wrap_width, src_x, src_y, dest_x, dest_y)
        {
            unsafe {
                parallel::set_cell(ptrs, wrap_width, dest_x, dest_y, placed);
                parallel::set_cell(ptrs, wrap_width, src_x, src_y, fill);
            }
            // Grain moved src→dest; fill is Air (no cream). Meta swap is fine.
            share_swaps
                .lock()
                .unwrap()
                .push((src_x, src_y, dest_x, dest_y));
            return;
        }
        // No neighbour water to steal — keep the bubble (swap) rather than mint.
    }
    unsafe {
        parallel::set_cell(ptrs, wrap_width, dest_x, dest_y, src);
        parallel::set_cell(ptrs, wrap_width, src_x, src_y, dest);
    }
    share_swaps
        .lock()
        .unwrap()
        .push((src_x, src_y, dest_x, dest_y));
}

/// Move standing water from an adjacent Air cell into a fill cell.
/// Prefer an upward donor first so dry bubbles rise into open water
/// instead of sliding sideways along a sand face (perpetual cycling).
/// Then prefer fuller cells. Leaves the donor empty. Skips `dest`.
fn steal_standing_water_neighbor(
    ptrs: &parallel::ChunkPtrMap,
    wrap_width: Option<i32>,
    src_x: i32,
    src_y: i32,
    dest_x: i32,
    dest_y: i32,
) -> Option<Cell> {
    // Rank: upward first, then fuller sat, then closer to vertical.
    let mut chosen: Option<(i32, i32, u8)> = None;
    let mut best_key: Option<(i32, u8, i32)> = None;
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0), (0, -1)] {
        let nx = src_x + dx;
        let ny = src_y + dy;
        if nx == dest_x && ny == dest_y {
            continue;
        }
        let Some(n) = (unsafe { parallel::get_cell(ptrs, wrap_width, nx, ny) }) else {
            continue;
        };
        if n.material != MaterialId::Air || n.sat.0 < 200 {
            continue;
        }
        let key = (if dy > 0 { 0 } else { 1 }, u8::MAX - n.sat.0, dy.abs());
        if best_key.map(|b| key < b).unwrap_or(true) {
            best_key = Some(key);
            chosen = Some((nx, ny, n.sat.0));
        }
    }
    let (nx, ny, sat) = chosen?;
    unsafe {
        parallel::set_cell(ptrs, wrap_width, nx, ny, Cell::air());
    }
    Some(Cell {
        material: MaterialId::Air,
        sat: Sat(sat),
        flags: CellFlags::empty(),
        _pad: 0,
        pore: 128,
    })
}

fn air_has_standing_water_neighbor(
    ptrs: &parallel::ChunkPtrMap,
    wrap_width: Option<i32>,
    gx: i32,
    gy: i32,
) -> bool {
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let Some(n) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx + dx, gy + dy) }) else {
            continue;
        };
        if n.material == MaterialId::Air && n.sat.0 >= 200 {
            return true;
        }
    }
    false
}

fn grain_is_wet(src: Cell, below_src: Option<Cell>) -> bool {
    if src.sat.0 >= 40 {
        return true;
    }
    matches!(
        below_src,
        Some(b) if b.material == MaterialId::Air && b.sat.0 >= 200
    )
}

/// Pore wetness for repose, treating standing water under the grain as
/// fully wet (shore / lake toe).
fn wet_frac_for(
    src: Cell,
    below_src: Option<Cell>,
    hydro: &wk_material::HydroOverrides,
) -> f32 {
    let pore = crate::failure::pore_wetness_with(src, hydro);
    let standing_wet = matches!(
        below_src,
        Some(b) if b.material == MaterialId::Air && b.sat.0 >= 200
    );
    if standing_wet {
        1.0
    } else {
        pore
    }
}

fn seat_on_ice(below_dest: Option<Cell>) -> bool {
    matches!(below_dest, Some(b) if b.material == MaterialId::Ice)
}

/// Atmospheric haze sand/gravel/clay may repose into. Wetter mid-film
/// seats are shore film (fleck cycle) and refused. Full standing water
/// is allowed so submerged banks can avalanche to a gentler repose.
/// Organic/Soil still use full through-haze on land, but refuse
/// underwater film seats.
pub const GRAIN_REPOSE_HAZE_MAX: u8 = 32;
/// Standing-water floor for dense-grain lake seats (matches steal /
/// submerged helpers). Mid film `HAZE_MAX+1 .. LAKE_MIN-1` stays refused.
pub const GRAIN_REPOSE_LAKE_MIN: u8 = 200;

/// Dense grains may slide into dry Air, thin haze, or standing water.
#[inline]
fn grain_repose_air_seat(dest: Cell) -> bool {
    dest.sat.0 <= GRAIN_REPOSE_HAZE_MAX || dest.sat.0 >= GRAIN_REPOSE_LAKE_MIN
}

/// How wet Air counts as vertical relief when measuring a repose drop.
#[derive(Clone, Copy)]
enum ReposeGapMode {
    /// Sand / Clay / … — haze + lake are gap; mid shore film is support.
    Dense,
    /// Soil — any wet Air is gap (land humid film + underwater lake).
    Soil,
    /// Organic litter — non-full film is gap; full lake is support (float).
    Litter,
}

fn repose_gap_mode(src: Cell, surface_organic: bool) -> ReposeGapMode {
    match src.material {
        // Surface / raft Organic: full lake is support (float).
        MaterialId::Organic if surface_organic => ReposeGapMode::Litter,
        // Soil + submerged / waterlogged Organic: lake is relief.
        MaterialId::Organic | MaterialId::Soil => ReposeGapMode::Soil,
        _ => ReposeGapMode::Dense,
    }
}

/// True when Organic sits next to standing water — mycelium felt must
/// not hold a vertical dam; water washes the mat loose.
fn organic_wash_wet_ptrs(
    ptrs: &parallel::ChunkPtrMap,
    wrap_width: Option<i32>,
    gx: i32,
    gy: i32,
) -> bool {
    for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1), (-1, -1), (1, -1)] {
        let Some(n) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx + dx, gy + dy) }) else {
            continue;
        };
        if n.material == MaterialId::Air && n.sat.0 >= GRAIN_REPOSE_LAKE_MIN {
            return true;
        }
    }
    false
}

/// True when Organic is still a surface mat / beach litter (not under a
/// water column). Waterlogged cells and piles with standing water above
/// count as settled ooze for underwater repose.
fn organic_is_surface_litter_world(world: &World, gx: i32, gy: i32, src: Cell) -> bool {
    if src.material != MaterialId::Organic {
        return false;
    }
    if src.is_waterlogged_organic() {
        return false;
    }
    let mut y = gy + 1;
    for _ in 0..48 {
        let Some(c) = world.get_cell(gx, y) else {
            return true;
        };
        if c.material == MaterialId::Organic {
            y += 1;
            continue;
        }
        if c.material == MaterialId::Air && c.sat.0 >= GRAIN_REPOSE_LAKE_MIN {
            return false; // submerged under the lake
        }
        return true; // open sky, film, or solid cover
    }
    true
}

fn organic_is_surface_litter_ptrs(
    ptrs: &parallel::ChunkPtrMap,
    wrap_width: Option<i32>,
    gx: i32,
    gy: i32,
    src: Cell,
) -> bool {
    if src.material != MaterialId::Organic {
        return false;
    }
    if src.is_waterlogged_organic() {
        return false;
    }
    let mut y = gy + 1;
    for _ in 0..48 {
        let Some(c) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx, y) }) else {
            return true;
        };
        if c.material == MaterialId::Organic {
            y += 1;
            continue;
        }
        if c.material == MaterialId::Air && c.sat.0 >= GRAIN_REPOSE_LAKE_MIN {
            return false;
        }
        return true;
    }
    true
}

/// Land mid-film seat for Soil humid sprawl (not an underwater water-column gap).
fn soil_land_film_seat(dest: Cell, below_dest: Option<Cell>) -> bool {
    if dest.sat.is_empty() || dest.sat.is_full() || grain_repose_air_seat(dest) {
        return false;
    }
    // Underwater gap: Air with standing water beneath — refuse (fleck / crawl).
    !matches!(
        below_dest,
        Some(b) if b.material == MaterialId::Air && b.sat.0 >= GRAIN_REPOSE_LAKE_MIN
    )
}

/// Snow/ice may sit on empty Air or on a wet film that rests on Ice.
/// Dense grains may repose into dry Air, thin atmospheric haze, **or**
/// standing lake water (gentler submerged banks). Mid shore film is
/// still refused for Sand — sliding into film + stealing lake water was
/// the fleck cycle. **Soil** and **submerged / waterlogged Organic** take
/// lake seats (UW banks) plus land mid-film sprawl. **Surface Organic**
/// (rafts / beach litter with open air above) still refuses lake seats.
fn avalanche_seat_ok(
    src: Cell,
    dest: Cell,
    below_dest: Option<Cell>,
    surface_organic: bool,
) -> bool {
    if dest.sat.is_empty() {
        return true;
    }
    if src.material == MaterialId::Organic && surface_organic {
        if dest.sat.is_full() {
            return false;
        }
        // Underwater gap: Air with standing water beneath — refuse so
        // litter does not crawl into the water column under a raft.
        if matches!(
            below_dest,
            Some(b) if b.material == MaterialId::Air && b.sat.0 >= GRAIN_REPOSE_LAKE_MIN
        ) {
            return false;
        }
        return true;
    }
    if src.material == MaterialId::Soil
        || (src.material == MaterialId::Organic && !surface_organic)
    {
        // Lake + haze like dense grains; land mid-film for humid sprawl.
        grain_repose_air_seat(dest) || soil_land_film_seat(dest, below_dest)
    } else if is_grain(src.material) {
        // Thin haze + full lake OK; mid shore film still blocked.
        grain_repose_air_seat(dest)
    } else {
        // Snow / hillside ice: spill onto lake ice, not into open water.
        seat_on_ice(below_dest)
    }
}

/// Whether this Air cell counts as empty gap for a repose drop measure.
fn repose_gap_air(c: Cell, gap: ReposeGapMode) -> bool {
    if c.material != MaterialId::Air {
        return false;
    }
    if c.sat.is_empty() {
        return true;
    }
    match gap {
        ReposeGapMode::Litter => {
            // Organic: any non-full film; full lake is support (floats).
            !c.sat.is_full()
        }
        ReposeGapMode::Soil => {
            // Soil sinks: lake, haze, and land mid-film all count as relief.
            true
        }
        ReposeGapMode::Dense => {
            // Dense grains: thin haze **or** standing water. Mid film = support.
            grain_repose_air_seat(c)
        }
    }
}

fn hillside_ice_support(below_src: Option<Cell>) -> bool {
    // Floating lake lids rest on wet Air or more Ice — not avalanche sources.
    // Glaze on rock / sand / snow can peel off in a cold snap.
    match below_src {
        Some(b) if b.material == MaterialId::Air => false,
        Some(b) if b.material == MaterialId::Ice => false,
        Some(_) => true,
        None => false,
    }
}

fn avalanche_source_ok(mat: MaterialId, below_src: Option<Cell>, cold_mode: bool) -> bool {
    if is_repose_grain(mat) {
        return true;
    }
    cold_mode && mat == MaterialId::Ice && hillside_ice_support(below_src)
}

/// Tunables for grain / sediment + floating Organic litter.
///
/// Angle-of-repose slides always run inside [`tick`]; flow erosion is
/// gated by [`Self::enabled`]. Floating-litter soak / raft bind also
/// live here so Tab can tune shore stickiness live.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GrainConfig {
    /// When false, [`apply_flow_erosion`] is a no-op.
    pub enabled: bool,
    /// Scales material susceptibility (`1 - resistance/180`). Default
    /// ~0.14 — sand banks under cascade move over tens of ticks; still
    /// lakes (no flow bias) do not erode.
    pub erosion_rate: f32,
    /// Standing water below this sat does not drive erosion.
    pub min_flow_sat: u8,
    /// Cap on erosion events applied per call (0 = unlimited).
    pub max_events_per_tick: u32,
    pub seed_salt: u64,
    /// After pores fill, chance per tick that floating Organic waterlogs
    /// and begins sinking. Expected wait ≈ `1 / rate` ticks (~500 at
    /// `0.002`). Higher = mats shed water blisters sooner.
    pub organic_waterlog_rate: f32,
    /// Extra neighbour columns a living-root holdfast stitches into its
    /// wind raft (`0` = only columns the plant body already claims).
    /// Higher values make Organic mats stick to plants and form perched
    /// water bladders on slopes.
    pub raft_root_bind_radius: i32,
}

impl Default for GrainConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            erosion_rate: 0.14,
            min_flow_sat: 180,
            max_events_per_tick: 96,
            seed_salt: 0xE70D_E5ED_u64,
            // Faster than the old 0.0004 (~2500 ticks) so shore mats don't
            // glue plants into perched water blisters for so long.
            organic_waterlog_rate: 0.002,
            // Body-span bind only — no neighbour dilation (was 1).
            raft_root_bind_radius: 0,
        }
    }
}

/// True when this cell can be picked up by flow bedload / bank undercut.
///
/// Dense grains use [`is_flow_erodible`]. Grounded or waterlogged Organic
/// also scours (beach litter / sunk mats) so current can drag compost
/// downhill like sand. Thick / mycelium-bound floating rafts stay
/// wind-owned (holdfasts). Thin unbound floating film may scour under
/// cascade so shore rings do not seal water into sticky bubbles.
fn cell_is_flow_erodible(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    if is_flow_erodible(cell.material) {
        return true;
    }
    if cell.material != MaterialId::Organic {
        return false;
    }
    use wk_material::MaterialRegistry;
    if MaterialRegistry::erosion_rank(MaterialId::Organic) >= 150 {
        return false;
    }
    if !raft_rests_on_float_water_world(world, gx, gy) {
        // Grounded beach / waterlogged bedload.
        return true;
    }
    // Floating: only thin unbound surface film is scourable.
    if cell.mycelium() >= MYCELIUM_RAFT_BIND_MIN {
        return false;
    }
    if matches!(
        world.get_cell(gx, gy + 1),
        Some(a) if a.material == MaterialId::Organic
    ) {
        return false;
    }
    if matches!(
        world.get_cell(gx, gy - 1),
        Some(b) if b.material == MaterialId::Organic
    ) {
        return false;
    }
    true
}

/// Flow erosion + immediate downhill deposition for erodible grains.
///
/// Standing water with a cascade / head-drop neighbor undercuts the bed
/// or bank and places the grain on a lower solid-supported Air seat.
/// Still pools (no flow bias) are skipped so lakes don't chew their
/// floors. Ice is never targeted ([`is_flow_erodible`]). Grounded /
/// waterlogged Organic is included; floating rafts are not.
///
/// Compute-then-apply; deterministic given `(seed, tick, cfg.seed_salt)`.
/// Chunk scans use rayon when [`crate::parallel::parallel_enabled`]
/// (frame-shell Phase 1); apply stays serial.
pub fn apply_flow_erosion(world: &mut World, cfg: &GrainConfig) {
    apply_flow_erosion_bound(world, cfg, None);
}

/// [`apply_flow_erosion`] with optional living-root binding.
pub fn apply_flow_erosion_bound(
    world: &mut World,
    cfg: &GrainConfig,
    rooted: Option<&HashSet<(i32, i32)>>,
) {
    if !cfg.enabled || cfg.erosion_rate <= 0.0 {
        return;
    }
    // Every other tick — Super-Server demo ~1.25 ms/tick; half-rate keeps
    // bedload feel while freeing ~0.6 ms toward 60 FPS. Unit tests that
    // call this without advancing `world.tick` still run (tick 0).
    if world.tick % 2 != 0 {
        return;
    }

    let seed = world.seed.0;
    let tick_no = world.tick;
    // Skip dry chunks — same sticky flag evaporation uses. Still pools
    // without flow bias remain no-ops inside the scan.
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_air)
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));

    let per_chunk = map_chunk_coords_parallel(&coords, |coord| {
        let mut local: Vec<ErosionEvent> = Vec::new();
        for y in 0..CHUNK_CELLS_H {
            let gy = coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                let Some(water) = world.get_cell(gx, gy) else {
                    continue;
                };
                if water.material != MaterialId::Air || water.sat.0 < cfg.min_flow_sat {
                    continue;
                }
                // Must be standing (or a deep water column cell).
                let standing = match world.get_cell(gx, gy - 1) {
                    Some(b) if b.material != MaterialId::Air => true,
                    Some(b) => b.sat.0 >= 200,
                    None => false,
                };
                if !standing {
                    continue;
                }
                let Some(flow_dx) = flow_bias(world, gx, gy, water.sat) else {
                    continue;
                };

                // Bed scour under this water cell.
                if let Some(bed) = world.get_cell(gx, gy - 1) {
                    if cell_is_flow_erodible(world, gx, gy - 1, bed) {
                        maybe_queue_erosion(
                            world,
                            cfg,
                            seed,
                            tick_no,
                            gx,
                            gy - 1,
                            bed,
                            flow_dx,
                            true,
                            rooted,
                            &mut local,
                        );
                    }
                }
                // Bank undercut at the water surface.
                for bank_dx in [-1_i32, 1] {
                    let bx = gx + bank_dx;
                    let Some(bank) = world.get_cell(bx, gy) else {
                        continue;
                    };
                    if !cell_is_flow_erodible(world, bx, gy, bank) {
                        continue;
                    }
                    maybe_queue_erosion(
                        world,
                        cfg,
                        seed,
                        tick_no,
                        bx,
                        gy,
                        bank,
                        flow_dx,
                        false,
                        rooted,
                        &mut local,
                    );
                }
            }
        }
        local
    });

    let mut events: Vec<ErosionEvent> = Vec::new();
    for local in per_chunk {
        events.extend(local);
    }

    events.sort_by(|a, b| {
        a.erode_y
            .cmp(&b.erode_y)
            .then(a.erode_x.cmp(&b.erode_x))
            .then(a.deposit_x.cmp(&b.deposit_x))
    });
    let mut used_erode = std::collections::HashSet::new();
    let mut used_deposit = std::collections::HashSet::new();
    let mut applied = 0u32;
    for ev in events {
        if cfg.max_events_per_tick > 0 && applied >= cfg.max_events_per_tick {
            break;
        }
        let ek = (ev.erode_x, ev.erode_y);
        let dk = (ev.deposit_x, ev.deposit_y);
        if !used_erode.insert(ek) || !used_deposit.insert(dk) {
            continue;
        }
        let Some(cur) = world.get_cell(ev.erode_x, ev.erode_y) else {
            continue;
        };
        if cur.material != ev.grain.material {
            continue;
        }
        let Some(dest) = world.get_cell(ev.deposit_x, ev.deposit_y) else {
            continue;
        };
        if dest.material != MaterialId::Air {
            continue;
        }
        // Deposit must not destroy free water in the seat: soak what
        // fits into the grain's pore capacity, push the rest upward
        // through the Air column, and park any remainder in the vacated
        // cell. Bed scour leaves empty Air (gravity pulls the column
        // down) — never mint a fresh Cell::water().
        let (mut placed, mut leftover) =
            absorb_free_water_into_grain(ev.grain, dest.sat, &world.hydro);
        // Organic moved by current stays as bedload: waterlog wet deposits
        // so buoyancy does not yank them back onto the free surface.
        if placed.material == MaterialId::Organic
            && (dest.sat.0 > 0 || placed.sat.0 > 0 || ev.grain.is_waterlogged_organic())
        {
            placed.flags.set(CellFlags::WATERLOGGED);
        }
        world.set_cell(ev.deposit_x, ev.deposit_y, placed);
        // Cream + strain shares follow bedload (river piles stay colored).
        move_mycelium_meta(
            world,
            ev.erode_x,
            ev.erode_y,
            ev.deposit_x,
            ev.deposit_y,
        );
        leftover = push_sat_upward(world, ev.deposit_x, ev.deposit_y + 1, leftover);
        // Vacated hole is empty Air, plus any free water that could not
        // be displaced upward. Pore water rides with the grain (placed).
        world.set_cell(
            ev.erode_x,
            ev.erode_y,
            Cell {
                material: MaterialId::Air,
                sat: leftover,
                flags: CellFlags::empty(),
                _pad: 0,
                pore: cur.pore,
            },
        );
        applied = applied.wrapping_add(1);
    }
    // Erosion runs after `tick`'s gravity pass in the demo — pull water
    // into any empty scour holes so they don't flash dry Air for a frame.
    if applied > 0 {
        apply_gravity_fall(world);
    }
}

/// Soak free-water `sat` into a moving grain's pores; return `(placed, leftover)`.
fn absorb_free_water_into_grain(
    grain: Cell,
    free: Sat,
    hydro: &HydroOverrides,
) -> (Cell, Sat) {
    let cap = water_capacity_cell(grain, hydro);
    let room = cap.saturating_sub(grain.sat.0);
    let into_pore = free.0.min(room);
    let mut placed = grain;
    placed.sat = Sat(grain.sat.0.saturating_add(into_pore));
    (placed, Sat(free.0.saturating_sub(into_pore)))
}

/// Push free-water sat upward through a stack of Air cells. Returns any
/// remainder that could not fit (caller parks it elsewhere).
fn push_sat_upward(world: &mut World, gx: i32, start_y: i32, mut sat: Sat) -> Sat {
    if sat.is_empty() {
        return sat;
    }
    let mut y = start_y;
    for _ in 0..32 {
        if sat.is_empty() {
            break;
        }
        let Some(cell) = world.get_cell(gx, y) else {
            break;
        };
        if cell.material != MaterialId::Air {
            break;
        }
        let room = u8::MAX.saturating_sub(cell.sat.0);
        let add = sat.0.min(room);
        if add > 0 {
            let mut next = cell;
            next.sat = Sat(cell.sat.0.saturating_add(add));
            world.set_cell(gx, y, next);
            sat = Sat(sat.0.saturating_sub(add));
        }
        if sat.is_empty() {
            break;
        }
        y += 1;
    }
    sat
}

struct ErosionEvent {
    erode_x: i32,
    erode_y: i32,
    deposit_x: i32,
    deposit_y: i32,
    grain: Cell,
}

fn maybe_queue_erosion(
    world: &World,
    cfg: &GrainConfig,
    seed: u64,
    tick_no: u64,
    ex: i32,
    ey: i32,
    grain: Cell,
    flow_dx: i32,
    bed_scour: bool,
    rooted: Option<&HashSet<(i32, i32)>>,
    out: &mut Vec<ErosionEvent>,
) {
    use wk_material::MaterialRegistry;
    let resistance = MaterialRegistry::erosion_rank(grain.material) as f32;
    let mut sus = (1.0 - (resistance / 180.0).clamp(0.0, 0.95)).max(0.02);
    if grain.sat.0 >= 40 {
        sus *= 1.4; // wet grains loosen (column sim saturation collapse)
    }
    if grain.material == MaterialId::Organic {
        // Dead stems / litter should leave a current instead of damming it.
        sus *= 2.2;
    }
    if rooted.is_some_and(|r| r.contains(&(ex, ey))) {
        sus /= ROOT_EROSION_BIND;
    }
    // Colonized Organic resists cascade chew (scales with cream intensity).
    sus /= mycelium_erosion_bind(grain);
    let p = (sus * cfg.erosion_rate).clamp(0.0, 1.0);
    let roll = hash_prob(
        seed,
        ex.wrapping_mul(73_856_093).wrapping_add(ey),
        tick_no,
        cfg.seed_salt
            .wrapping_add(if bed_scour { 1 } else { 2 })
            .wrapping_add((flow_dx as u64) << 3),
    );
    if roll >= p {
        return;
    }
    let Some((dx, dy)) = find_deposit_seat(world, ex, ey, flow_dx) else {
        return;
    };
    let _ = bed_scour; // only affects the roll salt above
    out.push(ErosionEvent {
        erode_x: ex,
        erode_y: ey,
        deposit_x: dx,
        deposit_y: dy,
        grain,
    });
}

/// Direction water wants to leave this cell, if any. `None` = still pool.
fn flow_bias(world: &World, gx: i32, gy: i32, sat: Sat) -> Option<i32> {
    let mut best_dx = 0i32;
    let mut best_score = 0.0f32;
    for dx in [-1_i32, 1] {
        let Some(n) = world.get_cell(gx + dx, gy) else {
            continue;
        };
        if n.material != MaterialId::Air {
            continue;
        }
        let mut score = 0.0f32;
        if n.sat.0.saturating_add(32) < sat.0 {
            score += 0.5;
        }
        match world.get_cell(gx + dx, gy - 1) {
            // Real cascade lip: side column has room below (not a full
            // water column). Treating sat-full Air as a lip made every
            // deep lake cell scour its bed forever.
            Some(b) if b.material == MaterialId::Air && !b.sat.is_full() => {
                score += 1.0;
            }
            Some(b) if b.material != MaterialId::Air => {
                // Lower neighbor column surface (thin-sheet downhill).
                if n.sat.is_empty() {
                    score += 0.40;
                }
            }
            _ => {}
        }
        if score > best_score {
            best_score = score;
            best_dx = dx;
        }
    }
    if best_score >= 0.34 {
        Some(best_dx)
    } else {
        None
    }
}

/// Solid-supported Air seat for a picked-up grain.
///
/// Prefers downhill (`dy > 0`), but also accepts same-Y beach / cascade-lip
/// seats so bedload can leave a flat shelf onto the next column.
fn find_deposit_seat(world: &World, from_x: i32, from_y: i32, prefer_dx: i32) -> Option<(i32, i32)> {
    let dxs = [
        prefer_dx,
        prefer_dx.saturating_mul(2),
        -prefer_dx,
        0,
        prefer_dx.saturating_mul(3),
        -prefer_dx.saturating_mul(2),
    ];
    for dy in 0..=6 {
        for &dx in &dxs {
            if dy == 0 && dx == 0 {
                continue;
            }
            let tx = from_x + dx;
            let ty = from_y - dy;
            if tx == from_x && ty == from_y {
                continue;
            }
            let Some(c) = world.get_cell(tx, ty) else {
                continue;
            };
            if c.material != MaterialId::Air {
                continue;
            }
            let Some(below) = world.get_cell(tx, ty - 1) else {
                continue;
            };
            if below.material == MaterialId::Air {
                continue;
            }
            return Some((tx, ty));
        }
    }
    None
}

/// True when the destination column has more than `max_step` empty
/// Air cells stacked downward from `from_y - 1` (the diagonal seat).
/// Dense grains treat standing lake water as gap (gentler UW banks);
/// mid shore film remains support. Soil treats any wet Air as gap.
/// Organic still treats full water as support (floats).
fn diag_drop_exceeds(
    ptrs: &parallel::ChunkPtrMap,
    wrap_width: Option<i32>,
    dest_gx: i32,
    from_y: i32,
    max_step: i32,
    gap: ReposeGapMode,
) -> bool {
    let mut drop = 0i32;
    for dy in 1..=(max_step + 2) {
        let y = from_y - dy;
        let Some(c) = (unsafe { parallel::get_cell(ptrs, wrap_width, dest_gx, y) }) else {
            break;
        };
        if c.material != MaterialId::Air {
            break;
        }
        if !repose_gap_air(c, gap) {
            break;
        }
        drop += 1;
        if drop > max_step {
            return true;
        }
    }
    drop > max_step
}
