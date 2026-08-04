//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Grain fall, repose, cold avalanche, and flow erosion.

use std::collections::HashSet;

use wk_material::{HydroOverrides, MaterialId};

use crate::active::{partition_checkerboard, plan_active, ActiveChunk};
use crate::cell::{
    falls_through_empty_air, is_flow_erodible, is_grain, is_repose_grain,
    water_capacity_with, Cell, CellFlags, Sat,
};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::{self, for_each_region_parallel, for_each_region_serial_moore};
use crate::temperature::Temperature;

use super::gravity::apply_gravity_fall;
use super::plan::regions_for_standalone;
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

/// Max vertical fall cells when settling unsupported grains in one
/// call. Default sky is ~5 chunks tall (`64×5`); cover that so F3
/// litter does not take hundreds of ticks to land.
pub const GRAIN_SETTLE_PASSES: u32 = 1024;

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

/// Max punch swaps per call — tall piles on thick rafts need several
/// grain↔litter steps before cargo reaches the water seat.
const FLOAT_PUNCH_MAX: u32 = 64;

/// Dense grains cannot ride a floating Organic / Snow / Ice raft.
///
/// Scans the whole grid (like [`rise_buoyant_litter`]) so punch still
/// runs when only the pile was dirtied and the water seat is quiet.
/// Each pass swaps a grain with the litter cell immediately below it
/// when that litter column rests on a grounded lake; settle/tick then
/// sinks the grain through water.
pub fn punch_through_floating_rafts(world: &mut World) -> u32 {
    let mut total = 0u32;
    for _ in 0..FLOAT_PUNCH_MAX {
        let mut swaps: Vec<(i32, i32)> = Vec::new();
        for &coord in world.chunks.keys() {
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
            break;
        }
        // Bottom grains first so Soil under LooseRock punches before rock.
        swaps.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        let mut moved = 0u32;
        for (gx, gy) in swaps {
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
            world.set_cell(gx, gy - 1, grain);
            world.set_cell(gx, gy, litter);
            // Keep the water seat awake so the next fall pass sinks cargo.
            if let Some(seat) = world.get_cell(gx, gy - 2) {
                if seat.material == MaterialId::Air {
                    world.touch_dirty(gx, gy - 2);
                }
            }
            moved += 1;
        }
        total += moved;
        if moved == 0 {
            break;
        }
    }
    total
}

/// Re-dirty every grain / litter cell that has empty (or non-supporting)
/// Air directly below — and the Air seat itself.
///
/// Needed when mid-air F3 paint lost its dirty wake (quiet water ticks
/// cleared it before grain fall ran, or the world was saved/loaded).
/// Without this, floating sand only moves when roof-collapse / erosion
/// happens to re-wake the column.
pub fn wake_unsupported_grains(world: &mut World) {
    let coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
    for coord in coords {
        let x0 = coord.cx * CHUNK_CELLS_W as i32;
        let y0 = coord.cy * CHUNK_CELLS_H as i32;
        for ly in 0..CHUNK_CELLS_H as i32 {
            for lx in 0..CHUNK_CELLS_W as i32 {
                let gx = x0 + lx;
                let gy = y0 + ly;
                let Some(cell) = world.get_cell(gx, gy) else {
                    continue;
                };
                let loose = is_grain(cell.material)
                    || falls_through_empty_air(cell.material)
                    || is_repose_grain(cell.material);
                if !loose {
                    continue;
                }
                let Some(below) = world.get_cell(gx, gy - 1) else {
                    world.touch_dirty(gx, gy);
                    continue;
                };
                // Dense cargo on a floating litter raft — punch-through wake.
                if is_grain(cell.material) && falls_through_empty_air(below.material) {
                    if raft_rests_on_float_water_world(world, gx, gy - 1) {
                        world.touch_dirty(gx, gy);
                        world.touch_dirty(gx, gy - 1);
                        // Water seat under the raft must be active so fall
                        // can sink cargo after the punch swap.
                        let mut y = gy - 2;
                        while let Some(c) = world.get_cell(gx, y) {
                            if falls_through_empty_air(c.material) {
                                world.touch_dirty(gx, y);
                                y -= 1;
                                continue;
                            }
                            if c.material == MaterialId::Air {
                                world.touch_dirty(gx, y);
                            }
                            break;
                        }
                        continue;
                    }
                }
                if below.material != MaterialId::Air {
                    continue;
                }
                // Snow / Ice / Organic float on grounded lakes only.
                if falls_through_empty_air(cell.material)
                    && floats_on_air_seat_world(world, below, gx, gy - 1)
                {
                    // Submerged under a refilled surface: full water above
                    // means this cell must buoyancy-rise, not sit forever.
                    if let Some(above) = world.get_cell(gx, gy + 1) {
                        if floats_on_air_seat_world(world, above, gx, gy + 1) {
                            world.touch_dirty(gx, gy);
                            world.touch_dirty(gx, gy + 1);
                        }
                    }
                    continue;
                }
                world.touch_dirty(gx, gy);
                world.touch_dirty(gx, gy - 1);
            }
        }
    }
}

/// Re-dirty supported grains whose diagonal-down seat is steeper than
/// repose — vertical Organic/sand cliff faces that already have solid
/// under them never trip [`wake_unsupported_grains`], so without this
/// they freeze as walls after the first settle pass.
pub fn wake_unstable_slopes(world: &mut World) {
    let coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
    for coord in coords {
        let x0 = coord.cx * CHUNK_CELLS_W as i32;
        let y0 = coord.cy * CHUNK_CELLS_H as i32;
        for ly in 0..CHUNK_CELLS_H as i32 {
            for lx in 0..CHUNK_CELLS_W as i32 {
                let gx = x0 + lx;
                let gy = y0 + ly;
                let Some(cell) = world.get_cell(gx, gy) else {
                    continue;
                };
                if !is_repose_grain(cell.material) {
                    continue;
                }
                let Some(below) = world.get_cell(gx, gy - 1) else {
                    continue;
                };
                // Freefall seats are handled by [`wake_unsupported_grains`].
                if below.material == MaterialId::Air {
                    continue;
                }
                let max_step = {
                    let wet = crate::failure::pore_wetness_with(cell, &world.hydro);
                    crate::failure::grain_repose_max_step(cell.material, wet)
                };
                let through_haze =
                    matches!(cell.material, MaterialId::Organic | MaterialId::Soil);
                for dx in [-1, 1] {
                    let sx = gx + dx;
                    let sy = gy - 1;
                    let Some(seat) = world.get_cell(sx, sy) else {
                        continue;
                    };
                    if seat.material != MaterialId::Air {
                        continue;
                    }
                    if seat.sat.is_full()
                        && matches!(
                            cell.material,
                            MaterialId::Organic | MaterialId::Soil | MaterialId::Snow
                        )
                    {
                        continue;
                    }
                    if is_grain(cell.material) && seat.sat.0 > GRAIN_REPOSE_HAZE_MAX {
                        continue;
                    }
                    if diag_drop_exceeds_world(world, sx, gy, max_step, through_haze) {
                        world.touch_dirty(gx, gy);
                        world.touch_dirty(sx, sy);
                        break;
                    }
                }
                // Same-Y walk-off seat (Air beside with Air below).
                if max_step > 0 {
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
                    if is_grain(cell.material) && seat.sat.0 > GRAIN_REPOSE_HAZE_MAX {
                        continue;
                    }
                    if matches!(cell.material, MaterialId::Organic | MaterialId::Soil)
                        && seat.sat.is_full()
                    {
                        continue;
                    }
                    let Some(below_seat) = world.get_cell(sx, gy - 1) else {
                        continue;
                    };
                    if below_seat.material != MaterialId::Air || below_seat.sat.is_full() {
                        continue;
                    }
                    world.touch_dirty(gx, gy);
                    world.touch_dirty(sx, gy);
                    break;
                }
            }
        }
    }
}

fn diag_drop_exceeds_world(
    world: &World,
    dest_gx: i32,
    from_y: i32,
    max_step: i32,
    through_haze: bool,
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
        if !repose_gap_air(c, through_haze) {
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
                moved += apply_grain_fall_regions(world, pass);
            }
        }
        let after_fall = plan_active(world);
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
    let moves = std::sync::atomic::AtomicU32::new(0);
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
                    // grounded lake / puddle surfaces.
                    if floats_on_air_seat_ptrs(ptrs, wrap_width, cur, gx, gy) {
                        // Floating raft cannot carry dense cargo. Walk up
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
                                moves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            break;
                        }
                        continue;
                    }
                } else {
                    // Buoyancy: only pull litter up through grounded full
                    // water. (Freeboard pop into empty Air was removed —
                    // on an uneven ocean surface it swapped Organic up
                    // then immediately fell back, burning 1024 settle
                    // passes per tick.)
                    let Some(below) =
                        (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy - 1) })
                    else {
                        continue;
                    };
                    if !falls_through_empty_air(below.material) {
                        continue;
                    }
                    if !floats_on_air_seat_ptrs(ptrs, wrap_width, cur, gx, gy) {
                        continue;
                    }
                    unsafe {
                        parallel::set_cell(ptrs, wrap_width, gx, gy, below);
                        parallel::set_cell(ptrs, wrap_width, gx, gy - 1, cur);
                    }
                    moves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    continue;
                }
                unsafe {
                    parallel::set_cell(ptrs, wrap_width, gx, gy, above);
                    parallel::set_cell(ptrs, wrap_width, gx, gy + 1, cur);
                }
                moves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });
    moves.into_inner()
}

/// Max pore sat absorbed per tick from the water column under a floating
/// Organic / Soil raft. Keeps the surface cell full so the raft still floats.
const FLOAT_SOAK_RATE: u8 = 16;
/// Max cells a submerged litter grain may rise in one tick.
const BUOYANT_RISE_MAX: i32 = 16;

fn collect_buoyant_litter(world: &World) -> Vec<(i32, i32)> {
    let mut litter = Vec::new();
    for &coord in world.chunks.keys() {
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

/// Organic / Soil sitting on a grounded lake surface soaks pore water from
/// deeper cells in the water column (never drains the surface seat below
/// full — that would sink the raft and recreate submerged litter lines).
///
/// Scans only buoyant litter cells (not the whole grid).
pub fn soak_floating_litter(world: &mut World) {
    let litter = collect_buoyant_litter(world);
    for (gx, gy) in litter {
        let Some(raft) = world.get_cell(gx, gy) else {
            continue;
        };
        if !matches!(raft.material, MaterialId::Organic | MaterialId::Soil) {
            continue;
        }
        let cap = water_capacity_with(raft.material, &world.hydro);
        if cap == 0 || raft.sat.0 >= cap {
            continue;
        }
        let Some(surface) = world.get_cell(gx, gy - 1) else {
            continue;
        };
        if !floats_on_air_seat_world(world, surface, gx, gy - 1) {
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
        for dy in 2..=64 {
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

/// Lift submerged Snow/Ice/Organic through grounded full water, and pop
/// litter that still has lake water beside it onto freeboard Air.
///
/// Litter-centric (not a full-grid × height scan): finds buoyant cells once,
/// then bubbles each up at most [`BUOYANT_RISE_MAX`] steps this tick.
pub fn rise_buoyant_litter(world: &mut World) {
    let mut litter = collect_buoyant_litter(world);
    if litter.is_empty() {
        return;
    }
    // Bottom-up so lower cells rise before upper ones in the same column.
    litter.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    for (gx, gy0) in litter {
        let mut gy = gy0;
        for _ in 0..BUOYANT_RISE_MAX {
            let Some(here) = world.get_cell(gx, gy) else {
                break;
            };
            if !falls_through_empty_air(here.material) {
                break;
            }
            let Some(above) = world.get_cell(gx, gy + 1) else {
                break;
            };
            if above.material != MaterialId::Air {
                break;
            }
            let rise = if floats_on_air_seat_world(world, above, gx, gy + 1) {
                true
            } else {
                false
            };
            if !rise {
                break;
            }
            world.set_cell(gx, gy + 1, here);
            world.set_cell(gx, gy, above);
            gy += 1;
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
/// the local step via [`ROOT_REPOSE_STEP_BONUS`]. One move per cell per
/// pass; run after [`apply_grain_fall`].
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
    let regions = regions_for_standalone(world);
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
                    if !avalanche_seat_ok(src.material, dest, below_dest) {
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
                    let through_haze =
                        matches!(src.material, MaterialId::Organic | MaterialId::Soil);
                    if !diag_drop_exceeds(ptrs, wrap_width, gx, sy, max_step, through_haze) {
                        continue;
                    }
                    write_repose_swap(
                        ptrs, wrap_width, &hydro, gx, gy, dest, sx, sy, src,
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
                if !cold_mode {
                    let open_drop = matches!(
                        below_dest,
                        Some(b) if b.material == MaterialId::Air && !b.sat.is_full()
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
                        // Rooted sand / plastic clay can hold short stairs —
                        // don't walk off.
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
                        if !avalanche_seat_ok(src.material, dest, below_dest) {
                            continue;
                        }
                        write_repose_swap(
                            ptrs, wrap_width, &hydro, gx, gy, dest, sx, sy, src,
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
                    if !avalanche_seat_ok(src.material, dest, below_dest) {
                        continue;
                    }
                    // Must be supported at source (not freefall).
                    match below_src {
                        Some(b) if b.material == MaterialId::Air => continue,
                        None => continue,
                        _ => {}
                    }
                    write_repose_swap(
                        ptrs, wrap_width, &hydro, gx, gy, dest, sx, sy, src,
                    );
                    moves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    break;
                }
            }
        }
    });
    moves.into_inner()
}

/// Swap grain into a diagonal/lateral Air seat.
///
/// Underwater, sliding into empty / film Air used to leave that pale
/// low-sat cell on the slope face for a frame (sky / film flash) until
/// flow refilled it. Dense grains collapse those bubbles: soak seat sat
/// into pores and **steal** standing water from a neighbour into the
/// vacated cell — never mint a fresh full water cell.
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
) {
    let submerged = dest.sat.0 >= 200
        || air_has_standing_water_neighbor(ptrs, wrap_width, dest_x, dest_y)
        || air_has_standing_water_neighbor(ptrs, wrap_width, src_x, src_y);
    if is_grain(src.material) && submerged && dest.sat.0 < 200 {
        let cap = water_capacity_with(src.material, hydro);
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
            return;
        }
        // No neighbour water to steal — keep the bubble (swap) rather than mint.
    }
    unsafe {
        parallel::set_cell(ptrs, wrap_width, dest_x, dest_y, src);
        parallel::set_cell(ptrs, wrap_width, src_x, src_y, dest);
    }
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

/// Atmospheric haze sand/gravel/clay may repose into. Wetter seats are
/// treated as shore film (fleck cycle) and refused. Organic/Soil still
/// use full through-haze on land, but refuse underwater film seats.
pub const GRAIN_REPOSE_HAZE_MAX: u8 = 32;

/// Snow/ice may sit on empty Air or on a wet film that rests on Ice.
/// Dense grains may repose into **dry** Air or thin atmospheric haze;
/// sliding into shore film + stealing lake water was the fleck cycle.
/// Organic litter / composted Soil match grain-fall: sprawl through
/// land haze/film, float only on grounded full standing water (else humid
/// cliffs freeze). Suspended mid-air full-sat is not a seat. Organic must
/// not sprawl into underwater film (that left a submerged "glitch line").
fn avalanche_seat_ok(src: MaterialId, dest: Cell, below_dest: Option<Cell>) -> bool {
    if dest.sat.is_empty() {
        return true;
    }
    if matches!(src, MaterialId::Organic | MaterialId::Soil) {
        if dest.sat.is_full() {
            return false;
        }
        // Underwater gap: Air with standing water beneath — refuse so
        // litter does not crawl into the water column under a raft.
        if matches!(
            below_dest,
            Some(b) if b.material == MaterialId::Air && b.sat.0 >= 200
        ) {
            return false;
        }
        return true;
    }
    if is_grain(src) {
        // Thin humidity haze OK (inland cliffs); shore film / lake not.
        return dest.sat.0 <= GRAIN_REPOSE_HAZE_MAX;
    }
    // Snow / hillside ice: spill onto lake ice, not into open water.
    seat_on_ice(below_dest)
}

/// Whether this Air cell counts as empty gap for a repose drop measure.
fn repose_gap_air(c: Cell, through_haze: bool) -> bool {
    if c.material != MaterialId::Air {
        return false;
    }
    if c.sat.is_empty() {
        return true;
    }
    if c.sat.is_full() {
        return false;
    }
    if through_haze {
        return true; // Organic/Soil: any non-full film
    }
    // Dense grains: only thin atmospheric haze.
    c.sat.0 <= GRAIN_REPOSE_HAZE_MAX
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

/// Tunables for flow bedload / bank erosion + deposition.
///
/// Angle-of-repose slides always run inside [`tick`]; this config only
/// gates the water-driven transport pass ([`apply_flow_erosion`]).
#[derive(Debug, Clone)]
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
}

impl Default for GrainConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            erosion_rate: 0.14,
            min_flow_sat: 180,
            max_events_per_tick: 96,
            seed_salt: 0xE70D_E5ED_u64,
        }
    }
}

/// Flow erosion + immediate downhill deposition for erodible grains.
///
/// Standing water with a cascade / head-drop neighbor undercuts the bed
/// or bank and places the grain on a lower solid-supported Air seat.
/// Still pools (no flow bias) are skipped so lakes don't chew their
/// floors. Ice is never targeted ([`is_flow_erodible`]).
///
/// Compute-then-apply; deterministic given `(seed, tick, cfg.seed_salt)`.
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

    let seed = world.seed.0;
    let tick_no = world.tick;
    let mut events: Vec<ErosionEvent> = Vec::new();
    // Skip dry chunks — same sticky flag evaporation uses. Still pools
    // without flow bias remain no-ops inside the scan.
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_air)
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));

    for coord in coords {
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
                    if is_flow_erodible(bed.material) {
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
                            &mut events,
                        );
                    }
                }
                // Bank undercut at the water surface.
                for bank_dx in [-1_i32, 1] {
                    let bx = gx + bank_dx;
                    let Some(bank) = world.get_cell(bx, gy) else {
                        continue;
                    };
                    if !is_flow_erodible(bank.material) {
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
                        &mut events,
                    );
                }
            }
        }
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
        let (placed, mut leftover) =
            absorb_free_water_into_grain(ev.grain, dest.sat, &world.hydro);
        world.set_cell(ev.deposit_x, ev.deposit_y, placed);
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
    let cap = water_capacity_with(grain.material, hydro);
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
    if rooted.is_some_and(|r| r.contains(&(ex, ey))) {
        sus /= ROOT_EROSION_BIND;
    }
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
                    score += 0.35;
                }
            }
            _ => {}
        }
        if score > best_score {
            best_score = score;
            best_dx = dx;
        }
    }
    if best_score >= 0.5 {
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
/// Any wet Air (film or standing) is support — grains do not treat the
/// waterline as a dry cliff to avalanche into.
fn diag_drop_exceeds(
    ptrs: &parallel::ChunkPtrMap,
    wrap_width: Option<i32>,
    dest_gx: i32,
    from_y: i32,
    max_step: i32,
    through_haze: bool,
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
        if !repose_gap_air(c, through_haze) {
            break;
        }
        drop += 1;
        if drop > max_step {
            return true;
        }
    }
    drop > max_step
}
