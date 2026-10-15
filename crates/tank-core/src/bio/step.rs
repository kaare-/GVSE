//! B0/B1 biology tick: move, feed, grow, divide, die, film.
#![allow(clippy::too_many_arguments)]

use wk_material::MaterialId;

use crate::bio::genome::{GenomeStore, GenomeV0};
use crate::bio::program::{glue_bond, Detach};
use crate::bio::profile::Block;
use crate::bio::store::{CellStore, OWNER_BIAS};
use crate::cell::Cell;
use crate::grid::PhysicsGrid;
use crate::hash::hash_u64;
use crate::pixels::PixelLayer;
use crate::tank::SimEvent;

/// Fixed-point energy: 256 = 1 g.
const FP: i32 = 256;
/// PHOTO income per unit at full light, g/s. Tuned so a tank greens in ~2 min.
const PHOTO_YIELD: i32 = (0.045 * FP as f32) as i32;
const DIVIDE_COST: i32 = 6 * FP;
const DIVIDE_ENERGY: i32 = 10 * FP;
const MIN_AGE_DIVIDE: u16 = 30; // 1 s at 30 Hz
const MAX_AGE: u16 = 60 * 30; // 60 s
const FILM_STUCK_AGE: u16 = 45;
/// Soft live-cell lever from the handoff (30k hard stop for B0).
const LIVE_CAP: usize = 30_000;
/// Growth ticks between size steps (handoff `t_grow = 8`).
const T_GROW: u16 = 8;
/// No mitosis below this tank temperature (°C).
const MITOSIS_FLOOR_C: f32 = 4.0;

pub struct BioStepResult {
    pub events: Vec<SimEvent>,
    pub divisions: u32,
    pub deaths: u32,
}

pub fn step(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    grid: &mut PhysicsGrid,
    pixels: &mut PixelLayer,
    world_seed: u64,
    tick: u64,
    t_base: f32,
) -> BioStepResult {
    let mut events = Vec::new();
    let mut divisions = 0u32;
    let mut deaths = 0u32;
    let ids: Vec<u32> = cells.iter_alive().map(|(id, _)| id).collect();
    for id in ids {
        move_cell(cells, genomes, grid, pixels, world_seed, tick, id);
        metabolise(
            cells,
            genomes,
            grid,
            pixels,
            world_seed,
            tick,
            id,
            t_base,
            &mut events,
            &mut divisions,
            &mut deaths,
        );
    }
    form_glue_bonds(cells, genomes, pixels);
    BioStepResult {
        events,
        divisions,
        deaths,
    }
}

fn move_cell(
    cells: &mut CellStore,
    genomes: &GenomeStore,
    grid: &PhysicsGrid,
    pixels: &mut PixelLayer,
    world_seed: u64,
    tick: u64,
    id: u32,
) {
    let Some(cell) = cells.get(id).copied() else {
        return;
    };
    if !cell.alive || cell.film {
        return;
    }
    // Body parts stay on the realised map; only free cells drift.
    if cell.parent_cell != u32::MAX {
        return;
    }
    let g = genomes.genome(cell.genotype);
    let (vx, vy) = pixel_to_phys(cell.px, cell.py, pixels.w, grid.visible_h() as u32);
    let here = grid.get_visible(vx, vy);
    let below = grid.get_visible(vx, vy - 1);
    let solid_below = !below.is_air();
    let in_water = here.is_air() && here.sat > 40;

    // Brownian + buoyancy.
    let h = hash_u64(world_seed, tick, id as u64, 0xB0_01);
    let mut nx = cell.px as i32;
    let mut ny = cell.py as i32;
    let jitter = (h % 5) as i32 - 2;
    nx += jitter;
    if g.buoy > 0 && in_water {
        // Toward the light (up = decreasing py in pixel space).
        if h % 8 < (g.buoy as u64).clamp(0, 7) {
            ny -= 1;
        }
    } else if g.buoy < 0 || !in_water {
        ny += 1;
    }
    // Settle on sand / stick to rock.
    if solid_below && in_water && (below.is_rock() || below.id() == MaterialId::Sand) && h % 32 == 0
    {
        if let Some(c) = cells.get_mut(id) {
            c.film = false;
            c.heading = cell.heading;
        }
    }
    let pw = pixels.w as i32;
    let ph = pixels.h as i32;
    nx = nx.clamp(0, pw - cell.w as i32);
    ny = ny.clamp(0, ph - cell.h as i32);
    if nx != cell.px as i32 || ny != cell.py as i32 {
        let _ = cells.move_cell(pixels, id, nx as u16, ny as u16);
    }
}

fn metabolise(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    grid: &mut PhysicsGrid,
    pixels: &mut PixelLayer,
    world_seed: u64,
    tick: u64,
    id: u32,
    t_base: f32,
    events: &mut Vec<SimEvent>,
    divisions: &mut u32,
    deaths: &mut u32,
) {
    let Some(cell) = cells.get(id).copied() else {
        return;
    };
    if !cell.alive {
        return;
    }
    let g = genomes.genome(cell.genotype);
    let idx = (cell.py as usize) * (pixels.w as usize) + cell.px as usize;
    let light = if idx < pixels.light.len() {
        pixels.light[idx]
    } else {
        0
    };
    let glucose = if idx < pixels.glucose.len() {
        pixels.glucose[idx]
    } else {
        0
    };
    let area = (cell.w as i32).max(1) * (cell.h as i32).max(1);
    let temp_q = g.temp_eff_q8(t_base) as i32;
    let eff = g.metab_eff as i32 * temp_q / 255;
    let photo = g.photo() as i32;
    // Perimeter-bound income: larger cells don't get free area scaling.
    let peri = 2 * (cell.w as i32 + cell.h as i32).max(1);
    let mut income = PHOTO_YIELD * photo * light as i32 * eff / (255 * 128);
    income = income * peri / (4 * area).max(1);
    // Baseline + UPTAKE(glucose) from the dissolved field.
    let uptake = g.mode.units_of(crate::bio::profile::Block::UptakeGlucose) as i32;
    income += (2 + uptake * 4) * glucose as i32 * FP / 255;
    if glucose > 0 && idx < pixels.glucose.len() {
        pixels.glucose[idx] = glucose.saturating_sub(1 + uptake as u8);
    }
    let upkeep = g.mode.upkeep_fp() * area;
    // Structure upkeep 0.01 g/s/px (handoff).
    let upkeep = upkeep + (area * FP) / 100;
    let energy = cell.energy as i32 + income - upkeep;
    let age = cell.age.saturating_add(1);

    // Stress: weighted hunger / dark / heat / cold.
    let hunger = ((DIVIDE_ENERGY - energy.max(0)).max(0) * 255 / DIVIDE_ENERGY) as u8;
    let dark = 255u8.saturating_sub(light);
    let heat = if t_base > 30.0 {
        ((t_base - 30.0) * 25.0).min(255.0) as u8
    } else {
        0
    };
    let cold = if t_base < 10.0 {
        ((10.0 - t_base) * 25.0).min(255.0) as u8
    } else {
        0
    };
    let stress_sum = (hunger as u32 * g.stress_hunger as u32
        + dark as u32 * g.stress_dark as u32
        + heat as u32 * g.stress_heat as u32
        + cold as u32 * g.stress_cold as u32)
        / (255 * 4);
    let stress = stress_sum.min(255) as u8;

    // EXCRETE dumps a little waste into the field.
    let excrete = g.mode.units_of(crate::bio::profile::Block::Excrete);
    if excrete > 0 && idx < pixels.waste.len() {
        pixels.waste[idx] = pixels.waste[idx].saturating_add(excrete.min(4));
    }

    if energy <= 0 || age > MAX_AGE || (t_base > 38.0 && heat > 200) {
        let (vx, vy) = pixel_to_phys(cell.px, cell.py, pixels.w, grid.visible_h() as u32);
        // Soft litter: only replace nearly-dry air so free water is conserved.
        let seat = grid.get_visible(vx, vy);
        if seat.is_air() && seat.sat < 40 {
            grid.set_visible(vx, vy, Cell::solid(MaterialId::Organic));
        }
        let to_film =
            cell.film || age > FILM_STUCK_AGE && near_solid(grid, pixels, cell.px, cell.py);
        cells.kill(genomes, pixels, id, to_film);
        *deaths += 1;
        events.push(SimEvent {
            kind: 3, // death
            arg0: id,
        });
        return;
    }

    if let Some(c) = cells.get_mut(id) {
        c.energy = energy.min(u16::MAX as i32) as u16;
        c.age = age;
        c.stress = stress;
    }

    // Grow toward the mode's target shape, one axis step every t_grow.
    if age % T_GROW == 0 {
        let tw = g.mode.shape_w.max(1);
        let th = g.mode.shape_h.max(1);
        let _ = cells.try_grow(pixels, id, tw, th);
    }

    // Stick to a wall / waterline → contribute film.
    if age > FILM_STUCK_AGE
        && near_solid(grid, pixels, cell.px, cell.py)
        && idx < pixels.film.len()
        && pixels.film[idx] < 40
    {
        pixels.film[idx] = 40;
    }

    // Division only at target shape (B1); no mitosis in the cold.
    let at_shape = cells
        .get(id)
        .map(|c| c.w >= g.mode.shape_w && c.h >= g.mode.shape_h)
        .unwrap_or(false);
    if energy >= DIVIDE_ENERGY
        && age >= MIN_AGE_DIVIDE
        && at_shape
        && t_base >= MITOSIS_FLOOR_C
        && cells.len() < LIVE_CAP
        && try_divide(
            cells, genomes, pixels, world_seed, tick, id, &g, energy, stress, t_base, events,
        )
    {
        *divisions += 1;
    }
}

fn try_divide(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    pixels: &mut PixelLayer,
    world_seed: u64,
    tick: u64,
    parent: u32,
    parent_g: &GenomeV0,
    energy: i32,
    stress: u8,
    t_base: f32,
    events: &mut Vec<SimEvent>,
) -> bool {
    let Some(cell) = cells.get(parent).copied() else {
        return false;
    };
    if parent_g.mode.n_children == 0 {
        return false;
    }
    // Advance past exhausted child entries.
    let mut child_idx = cell.child_idx;
    let mut child_reps = cell.child_reps;
    while child_idx < parent_g.mode.n_children && child_reps == 0 {
        child_idx += 1;
        child_reps = parent_g
            .mode
            .children
            .get(child_idx as usize)
            .map(|c| c.repeat)
            .unwrap_or(0);
    }
    if child_idx >= parent_g.mode.n_children || child_reps == 0 {
        if let Some(c) = cells.get_mut(parent) {
            c.child_idx = child_idx;
            c.child_reps = 0;
        }
        return false;
    }
    let spec = parent_g.mode.children[child_idx as usize];
    let (dx, dy) = spec.dir.delta(cell.heading);
    let nx = cell.px as i32 + dx * cell.w as i32;
    let ny = cell.py as i32 + dy * cell.h as i32;
    if nx < 0 || ny < 0 || nx >= pixels.w as i32 || ny >= pixels.h as i32 {
        return false;
    }
    let lambda_m = parent_g.lambda_milli(stress, t_base);
    let mutate = hash_u64(world_seed, tick, parent as u64, 0xD1_01) % 1000 < lambda_m as u64;
    let child_g = if mutate {
        parent_g.mutate(world_seed, tick ^ (parent as u64) << 8)
    } else {
        *parent_g
    };
    let geno = if mutate {
        genomes.intern_child(cell.genotype, child_g)
    } else {
        cell.genotype
    };
    let heading = parent_g.mode.heading_rule.apply(cell.heading);
    let cost = DIVIDE_COST * (200 + parent_g.accurate as i32) / (200 + parent_g.cheap as i32);
    let daughter = match spec.detach {
        Detach::Immediate => {
            let gen = cells.org_generation(cell.org).saturating_add(1);
            cells.spawn(
                genomes,
                pixels,
                nx as u16,
                ny as u16,
                geno,
                gen,
                (cost / 2) as u16,
                heading,
                world_seed,
            )
        }
        Detach::Never => cells.spawn_bonded(
            genomes,
            pixels,
            nx as u16,
            ny as u16,
            geno,
            cell.org,
            (cost / 2) as u16,
            heading,
            parent,
            cell.depth.saturating_add(1),
            world_seed,
        ),
    };
    let Some(did) = daughter else {
        return false;
    };
    if let Some(c) = cells.get_mut(parent) {
        c.energy = (energy - cost).max(0) as u16;
        c.age = 0;
        let left = child_reps.saturating_sub(1);
        if left == 0 {
            c.child_idx = child_idx.saturating_add(1);
            c.child_reps = parent_g
                .mode
                .children
                .get(c.child_idx as usize)
                .map(|ch| ch.repeat)
                .unwrap_or(0);
        } else {
            c.child_idx = child_idx;
            c.child_reps = left;
        }
    }
    events.push(SimEvent {
        kind: 2, // division
        arg0: did,
    });
    if matches!(spec.detach, Detach::Never) && cells.org_slot_count(cell.org) == 2 {
        events.push(SimEvent {
            kind: 5, // multicellular
            arg0: cell.org,
        });
    }
    if mutate {
        events.push(SimEvent {
            kind: 4, // new colour / mutant
            arg0: geno,
        });
    }
    true
}

/// Adjacent GLUE cells of different organisms form a kin bond (no merge).
fn form_glue_bonds(cells: &mut CellStore, genomes: &GenomeStore, pixels: &PixelLayer) {
    let ids: Vec<u32> = cells.iter_alive().map(|(id, _)| id).collect();
    for &id in &ids {
        let Some(cell) = cells.get(id).copied() else {
            continue;
        };
        let g = genomes.genome(cell.genotype);
        let glue_u = g.mode.units_of(Block::Glue);
        if glue_u == 0 {
            continue;
        }
        if cell.glue_to != u32::MAX {
            // Keep existing bond if neighbour still alive and adjacent.
            if let Some(other) = cells.get(cell.glue_to) {
                if other.alive
                    && other.org != cell.org
                    && (other.px as i32 - cell.px as i32).abs() <= 1
                    && (other.py as i32 - cell.py as i32).abs() <= 1
                {
                    continue;
                }
            }
            if let Some(c) = cells.get_mut(id) {
                c.glue_to = u32::MAX;
                c.glue_strength = 0;
            }
        }
        for (dx, dy) in [(-1i32, 0), (1, 0), (0, -1), (0, 1)] {
            let nx = cell.px as i32 + dx;
            let ny = cell.py as i32 + dy;
            if nx < 0 || ny < 0 || nx >= pixels.w as i32 || ny >= pixels.h as i32 {
                continue;
            }
            let idx = (ny as usize) * (pixels.w as usize) + nx as usize;
            let owner = pixels.owner[idx];
            if owner < OWNER_BIAS {
                continue;
            }
            let oid = owner - OWNER_BIAS;
            if oid == id {
                continue;
            }
            let Some(other) = cells.get(oid).copied() else {
                continue;
            };
            if !other.alive || other.org == cell.org {
                continue;
            }
            let og = genomes.genome(other.genotype);
            if og.mode.units_of(Block::Glue) == 0 {
                continue;
            }
            let strength = glue_bond(g.glue_key, og.glue_key, g.glue_width, glue_u.saturating_mul(40));
            if strength == 0 {
                continue;
            }
            if let Some(c) = cells.get_mut(id) {
                c.glue_to = oid;
                c.glue_strength = strength;
            }
            break;
        }
    }
}

fn pixel_to_phys(px: u16, py: u16, pw: u32, vis_h: u32) -> (i32, i32) {
    let vx = (px as u32 / 3) as i32;
    let vy_top = py as u32 / 3;
    let vy = vis_h.saturating_sub(1).saturating_sub(vy_top) as i32;
    let _ = pw;
    (vx, vy)
}

fn near_solid(grid: &PhysicsGrid, pixels: &PixelLayer, px: u16, py: u16) -> bool {
    let (vx, vy) = pixel_to_phys(px, py, pixels.w, grid.visible_h() as u32);
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let n = grid.get_visible(vx + dx, vy + dy);
        if !n.is_air() {
            return true;
        }
    }
    // Waterline: air above, water here.
    let here = grid.get_visible(vx, vy);
    let above = grid.get_visible(vx, vy + 1);
    here.is_air() && here.sat > 100 && above.is_air() && above.sat < 40
}

/// Swipe-clean the front-pane film along a stroke in pixel space.
pub fn swipe_film(pixels: &mut PixelLayer, x0: i32, y0: i32, x1: i32, y1: i32, radius: i32) -> u32 {
    let mut cleared = 0u32;
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    for i in 0..=steps {
        let x = x0 + (x1 - x0) * i / steps;
        let y = y0 + (y1 - y0) * i / steps;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
                let px = x + dx;
                let py = y + dy;
                if px < 0 || py < 0 || px >= pixels.w as i32 || py >= pixels.h as i32 {
                    continue;
                }
                let idx = (py as usize) * (pixels.w as usize) + px as usize;
                if pixels.film[idx] > 0 {
                    pixels.film[idx] = 0;
                    cleared += 1;
                }
            }
        }
    }
    cleared
}
