//! B0 biology tick: move, feed, divide, die, film.
#![allow(clippy::too_many_arguments)]

use wk_material::MaterialId;

use crate::bio::genome::{GenomeStore, GenomeV0};
use crate::bio::store::CellStore;
use crate::cell::Cell;
use crate::grid::PhysicsGrid;
use crate::hash::hash_u64;
use crate::pixels::PixelLayer;
use crate::tank::SimEvent;

/// Fixed-point energy: 256 = 1 g.
const FP: i32 = 256;
/// PHOTO income per unit at full light, g/s. Tuned so a tank greens in ~2 min.
const PHOTO_YIELD: i32 = (0.045 * FP as f32) as i32;
const PHOTO_UPKEEP: i32 = (0.02 * FP as f32) as i32;
const STRUCT_UPKEEP: i32 = (0.01 * FP as f32) as i32;
const DIVIDE_COST: i32 = 6 * FP;
const DIVIDE_ENERGY: i32 = 10 * FP;
const MIN_AGE_DIVIDE: u16 = 30; // 1 s at 30 Hz
const MAX_AGE: u16 = 60 * 30; // 60 s
const FILM_STUCK_AGE: u16 = 45;
/// Soft live-cell lever from the handoff (30k hard stop for B0).
const LIVE_CAP: usize = 30_000;

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
            &mut events,
            &mut divisions,
            &mut deaths,
        );
    }
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
    nx = nx.clamp(0, pw - 1);
    ny = ny.clamp(0, ph - 1);
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
    let eff = g.metab_eff as i32;
    let income = PHOTO_YIELD * g.photo as i32 * light as i32 * eff / (255 * 128);
    let upkeep = PHOTO_UPKEEP * g.photo as i32 + STRUCT_UPKEEP;
    let energy = cell.energy as i32 + income - upkeep;
    let age = cell.age.saturating_add(1);

    if energy <= 0 || age > MAX_AGE {
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
    }

    // Stick to a wall / waterline → contribute film.
    if age > FILM_STUCK_AGE
        && near_solid(grid, pixels, cell.px, cell.py)
        && idx < pixels.film.len()
        && pixels.film[idx] < 40
    {
        pixels.film[idx] = 40;
    }

    if energy >= DIVIDE_ENERGY
        && age >= MIN_AGE_DIVIDE
        && cells.len() < LIVE_CAP
        && try_divide(
            cells,
            genomes,
            pixels,
            world_seed,
            tick,
            id,
            &g,
            energy,
            events,
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
    events: &mut Vec<SimEvent>,
) -> bool {
    let Some(cell) = cells.get(parent).copied() else {
        return false;
    };
    let (dx, dy) = heading_delta(cell.heading);
    let nx = cell.px as i32 + dx;
    let ny = cell.py as i32 + dy;
    if nx < 0 || ny < 0 || nx >= pixels.w as i32 || ny >= pixels.h as i32 {
        return false;
    }
    // λ ≈ 0.1 per division (handoff seed is 0.3; B0 starts gentler).
    let mutate = hash_u64(world_seed, tick, parent as u64, 0xD1_01) % 1000 < 100;
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
    let gen = cells
        .get(parent)
        .map(|c| cells.org_generation(c.org))
        .unwrap_or(0)
        .saturating_add(1);
    let daughter = cells.spawn(
        genomes,
        pixels,
        nx as u16,
        ny as u16,
        geno,
        gen,
        (DIVIDE_COST / 2) as u16,
        cell.heading,
        world_seed,
    );
    if daughter.is_none() {
        return false;
    }
    if let Some(c) = cells.get_mut(parent) {
        c.energy = (energy - DIVIDE_COST).max(0) as u16;
        c.age = 0;
    }
    events.push(SimEvent {
        kind: 2, // division
        arg0: daughter.unwrap(),
    });
    if mutate {
        events.push(SimEvent {
            kind: 4, // new colour / mutant
            arg0: geno,
        });
    }
    true
}

fn heading_delta(h: u8) -> (i32, i32) {
    match h % 8 {
        0 => (1, 0),
        1 => (1, -1),
        2 => (0, -1),
        3 => (-1, -1),
        4 => (-1, 0),
        5 => (-1, 1),
        6 => (0, 1),
        _ => (1, 1),
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
