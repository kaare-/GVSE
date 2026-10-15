//! Biology tick: sense → feed → maintain → act → develop (§b.2), then
//! body-level passes: bond energy sharing, shedding, free-body drift,
//! GLUE bonding, fields.
#![allow(clippy::too_many_arguments)]

use wk_material::MaterialId;

use crate::bio::fields::{deposit, step_fields, KeyedField};
use crate::bio::genome::GenomeStore;
use crate::bio::profile::{ModeCoeffs, DIVIDE_AGE_UNIT_TICKS};
use crate::bio::program::{glue_bond, heading_delta, ChildKind, Detach, Scalars};
use crate::bio::store::{CellStore, NONE, OWNER_BIAS};
use crate::cell::Cell;
use crate::consts::{
    ACTIVITY_FLOOR_Q8, ALGAE_COST_FP, BIRTH_ENERGY_FP, BOND_SHARE_DIV, FP, HEAT_DAMAGE_C, LIVE_CAP,
    MITOSIS_FLOOR_C, PHOTO_YIELD_MFP, TICK_HZ, T_GROW,
};
use crate::grid::PhysicsGrid;
use crate::hash::hash_u64;
use crate::pixels::PixelLayer;
use crate::tank::SimEvent;

const FILM_STUCK_AGE: u16 = 45;
/// Baseline glucose uptake per tick per unit of field (milli-fp per 255).
const BASE_UPTAKE_MFP: i64 = 600;
/// Lysis of a soft-litter seat yields this much dissolved glucose.
const LITTER_GLUCOSE: u8 = 40;

pub struct BioStepResult {
    pub events: Vec<SimEvent>,
    pub divisions: u32,
    pub deaths: u32,
    /// Deaths by cause this tick: [starved, old age, damage, shed].
    pub death_causes: [u32; 4],
}

/// Event kinds pushed by the biology (see `tank.rs` for the full table).
pub mod ev {
    pub const SEED: u32 = 1;
    pub const DIVISION: u32 = 2;
    pub const DEATH: u32 = 3;
    pub const MUTANT: u32 = 4;
    pub const MULTICELLULAR: u32 = 5;
    pub const LIFELESS: u32 = 10;
    pub const GLASS: u32 = 11;
    pub const BROOD_RELEASED: u32 = 12;
    pub const SHED: u32 = 13;
    pub const FRAGMENT: u32 = 14;
    pub const REGROWN: u32 = 15;
}

pub fn step(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    grid: &mut PhysicsGrid,
    pixels: &mut PixelLayer,
    world_seed: u64,
    tick: u64,
    t_tank: f32,
) -> BioStepResult {
    let mut out = BioStepResult {
        events: Vec::new(),
        divisions: 0,
        deaths: 0,
        death_causes: [0; 4],
    };
    let ids = cells.live_ids();
    for id in ids {
        if cells.get(id).is_none() {
            continue;
        }
        move_free_cell(cells, genomes, grid, pixels, world_seed, tick, id);
        metabolise(
            cells, genomes, grid, pixels, world_seed, tick, id, t_tank, &mut out,
        );
    }
    share_bond_energy(cells, genomes);
    if tick % 16 == 5 {
        shed(cells, genomes, pixels, tick, &mut out);
    }
    if tick % 4 == 1 {
        drift_bodies(cells, genomes, grid, pixels, world_seed, tick);
    }
    form_glue_bonds(cells, genomes, pixels);
    step_fields(pixels, tick);
    out
}

// ---------------------------------------------------------------- helpers

pub fn pixel_to_phys(px: u16, py: u16, vis_h: u32) -> (i32, i32) {
    let vx = (px as u32 / 3) as i32;
    let vy_top = py as u32 / 3;
    let vy = vis_h.saturating_sub(1).saturating_sub(vy_top) as i32;
    (vx, vy)
}

fn near_solid(grid: &PhysicsGrid, px: u16, py: u16) -> bool {
    let (vx, vy) = pixel_to_phys(px, py, grid.visible_h() as u32);
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let n = grid.get_visible(vx + dx, vy + dy);
        if !n.is_air() {
            return true;
        }
    }
    let here = grid.get_visible(vx, vy);
    let above = grid.get_visible(vx, vy + 1);
    here.is_air() && here.sat > 100 && above.is_air() && above.sat < 40
}

/// Integer `log2(x / 256)` in Q8 for x ≥ 256 (0 below).
pub fn log2_q8(x_q8: u32) -> u32 {
    if x_q8 <= 256 {
        return 0;
    }
    let ilog = 31 - x_q8.leading_zeros(); // floor(log2 x_q8)
    let int_part = ilog - 8;
    // Fractional part: linear interpolation inside the octave.
    let base = 1u32 << ilog;
    let frac = ((x_q8 - base) as u64 * 256 / base as u64) as u32;
    int_part * 256 + frac
}

/// Pace law (§b.6): `max_age = base_age · (1 + log2(body_cost / algae_cost))`.
/// Q10 = 2 metabolic rate multiplier relative to 20 °C, Q8 (256 = ×1),
/// clamped to ×0.25..×4.
pub fn q10_q8(t_c: f32) -> i64 {
    (2f32.powf((t_c - 20.0) / 10.0).clamp(0.25, 4.0) * 256.0) as i64
}

pub fn max_age_ticks(base_age_s: u8, body_cost_fp: i32) -> u32 {
    let base = base_age_s as u32 * TICK_HZ as u32;
    let ratio_q8 = (body_cost_fp.max(1) as u64 * 256 / ALGAE_COST_FP as u64) as u32;
    let mult_q8 = (256 + log2_q8(ratio_q8)).clamp(256, 8 * 256);
    (base as u64 * mult_q8 as u64 / 256) as u32
}

/// Sum of light over a footprint and count of touched neighbours.
fn sense(pixels: &PixelLayer, grid: &PhysicsGrid, px: u16, py: u16, w: u8, h: u8) -> (u32, u8, u32) {
    let pw = pixels.w as usize;
    let mut light = 0u32;
    let mut glucose = 0u32;
    for dy in 0..h as usize {
        let row = (py as usize + dy) * pw;
        for dx in 0..w as usize {
            let i = row + px as usize + dx;
            light += pixels.light[i] as u32;
            glucose += pixels.glucose[i] as u32;
        }
    }
    // Touch: occupied edge-adjacent pixels (max 4), plus solid substrate.
    let mut touch = 0u8;
    let (cx, cy) = (px as i32, py as i32);
    for (dx, dy) in [(-1i32, 0i32), (w as i32, 0), (0, -1), (0, h as i32)] {
        let x = cx + dx;
        let y = cy + dy;
        if x < 0 || y < 0 || x >= pixels.w as i32 || y >= pixels.h as i32 {
            touch += 1;
            continue;
        }
        if pixels.owner[(y as usize) * pw + x as usize] != 0 {
            touch += 1;
        }
    }
    if touch < 4 && near_solid(grid, px, py) {
        touch += 1;
    }
    (light, touch.min(4), glucose)
}

// ---------------------------------------------------------------- movement

fn move_free_cell(
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
    if cell.film || cell.parent_cell != NONE {
        return;
    }
    // Multi-cell bodies drift as a unit in `drift_bodies`.
    if cells.org_slot_count(cell.org) > 1 {
        return;
    }
    let g = genomes.genome(cell.genotype);
    let buoy = g.buoy;
    let mc = *genomes.coeffs(cell.genotype, cell.mode);
    let (vx, vy) = pixel_to_phys(cell.px, cell.py, grid.visible_h() as u32);
    let here = grid.get_visible(vx, vy);
    let below = grid.get_visible(vx, vy - 1);
    let in_water = here.is_air() && here.sat > 40;

    let h = hash_u64(world_seed, tick, id as u64, 0xB0_01);
    let mut nx = cell.px as i32;
    let mut ny = cell.py as i32;
    if in_water {
        nx += (h % 5) as i32 - 2;
    }
    if mc.cilia > 0 && in_water {
        // Run-and-tumble (§a.6): drive from the gate; tumble 90° when it drops.
        let drive = cell.running;
        if drive {
            let (dx, dy) = heading_delta(cell.heading);
            if h % (TICK_HZ / mc.cilia.max(1) as u64).max(1) == 0 {
                nx += dx;
                ny += dy;
            }
        }
        if h % 17 == 0 {
            // Light-seeking tumble: pick the brighter of the two 90° turns.
            let idx = (cell.py as usize) * (pixels.w as usize) + cell.px as usize;
            let here_l = pixels.light.get(idx).copied().unwrap_or(0);
            let mut best_h = cell.heading;
            let mut best_l = here_l;
            for cand in [cell.heading, cell.heading.wrapping_add(2) % 8, cell.heading.wrapping_add(6) % 8] {
                let (cx, cy) = heading_delta(cand);
                let px = cell.px as i32 + cx * 2;
                let py = cell.py as i32 + cy * 2;
                if px < 0 || py < 0 || px >= pixels.w as i32 || py >= pixels.h as i32 {
                    continue;
                }
                let li = (py as usize) * (pixels.w as usize) + px as usize;
                let l = pixels.light.get(li).copied().unwrap_or(0);
                if l > best_l {
                    best_l = l;
                    best_h = cand;
                }
            }
            if let Some(c) = cells.get_mut(id) {
                c.heading = best_h;
                c.running = true;
            }
        }
    } else {
        let buoy_units = mc.buoy as i32 * 10;
        let b = buoy as i32 + buoy_units;
        if in_water && b > 0 {
            if h % 8 < (b as u64).clamp(0, 7) {
                ny -= 1;
            }
        } else if !in_water || b < 0 || h % 16 == 0 {
            ny += 1;
        }
    }
    let solid_below = !below.is_air();
    if solid_below && ny > cell.py as i32 {
        ny = cell.py as i32;
    }
    let pw = pixels.w as i32;
    let ph = pixels.h as i32;
    nx = nx.clamp(0, pw - cell.w as i32);
    ny = ny.clamp(0, ph - cell.h as i32);
    if nx != cell.px as i32 || ny != cell.py as i32 {
        let moved = cells.move_cell(pixels, id, nx as u16, ny as u16);
        // A sinking cell blocked from below slides off diagonally, so free
        // cells never pile up out of the water.
        if !moved && ny > cell.py as i32 {
            let side = if h & 0x100 == 0 { 1 } else { -1 };
            let sx = (cell.px as i32 + side).clamp(0, pw - cell.w as i32);
            let _ = cells.move_cell(pixels, id, sx as u16, ny as u16);
        }
    }
}

/// Multi-cell bodies: anchored to substrate ⇒ static; else drift as one
/// (bonds transmit force, §b.3). Sand landing on a body pushes it down.
fn drift_bodies(
    cells: &mut CellStore,
    genomes: &GenomeStore,
    grid: &PhysicsGrid,
    pixels: &mut PixelLayer,
    world_seed: u64,
    tick: u64,
) {
    let orgs: Vec<(u32, u32, i8)> = cells
        .iter_orgs()
        .filter(|(_, o)| o.n_alive >= 2)
        .map(|(id, o)| {
            let g = cells
                .get(o.seed)
                .map(|c| genomes.genome(c.genotype).buoy)
                .unwrap_or(0);
            (id, o.seed, g)
        })
        .collect();
    for (org_id, _seed, buoy) in orgs {
        let Some(o) = cells.org(org_id) else {
            continue;
        };
        let mut anchored = false;
        let mut all_water = true;
        let mut sand_above = false;
        for s in o.slots.iter().filter(|s| s.cell != NONE) {
            let Some(c) = cells.get(s.cell) else {
                continue;
            };
            if c.film || near_solid(grid, c.px, c.py) {
                anchored = true;
                break;
            }
            let (vx, vy) = pixel_to_phys(c.px, c.py, grid.visible_h() as u32);
            let here = grid.get_visible(vx, vy);
            if !(here.is_air() && here.sat > 40) {
                all_water = false;
            }
            let above = grid.get_visible(vx, vy + 1);
            if above.id() == MaterialId::Sand {
                sand_above = true;
            }
        }
        if anchored {
            continue;
        }
        let h = hash_u64(world_seed, tick, org_id as u64, 0xB2_D1);
        let mut dx = 0;
        let mut dy = 0;
        if sand_above || !all_water {
            dy = 1;
        } else if buoy > 0 {
            if h % 8 < (buoy as u64).clamp(0, 7) {
                dy = -1;
            }
        } else if buoy < 0 || h % 4 == 0 {
            dy = 1;
        }
        if all_water && h % 6 == 0 {
            dx = if (h >> 8) % 2 == 0 { 1 } else { -1 };
        }
        if dx != 0 || dy != 0 {
            let _ = cells.move_org(pixels, org_id, dx, dy);
        }
    }
}

// ---------------------------------------------------------------- per cell

fn metabolise(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    grid: &mut PhysicsGrid,
    pixels: &mut PixelLayer,
    world_seed: u64,
    tick: u64,
    id: u32,
    t_tank: f32,
    out: &mut BioStepResult,
) {
    let Some(cell) = cells.get(id).copied() else {
        return;
    };
    let mc = *genomes.coeffs(cell.genotype, cell.mode);
    let (metab_eff, temp_q, base_age, stress_def, wall_key, glue_key, strategy_q8, lambda_m) = {
        let g = genomes.genome(cell.genotype);
        (
            g.metab_eff as i64,
            g.temp_eff_q8(t_tank) as i64,
            g.base_age,
            g.stress,
            g.wall_key,
            g.glue_key,
            g.division_cost_q8(),
            g.lambda_milli(cell.stress, t_tank),
        )
    };
    let area = cell.area();
    let idx = (cell.py as usize) * (pixels.w as usize) + cell.px as usize;
    let (light_sum, touch, glucose_sum) = sense(pixels, grid, cell.px, cell.py, cell.w, cell.h);
    let light_mean = (light_sum / area as u32) as u8;

    // ---- feed
    // Yield × eff(T) × metab_eff; upkeep and ageing × Q10 rate ÷ metab_eff
    // (§4: all rates scale with temperature, lifespan ∝ 1/rate).
    let eff_q = metab_eff * temp_q / 255; // ≈128 = ×1
    let q10_q8 = q10_q8(t_tank);
    let mut income: i64 = 0;
    if mc.photo > 0 {
        income += PHOTO_YIELD_MFP * mc.photo as i64 * light_sum as i64 / (area as i64 * 255 * 1000);
    }
    // Baseline + UPTAKE(glucose) from the dissolved field under the cell.
    let uptake_mult = 1 + 4 * mc.uptake_glucose as i64;
    if glucose_sum > 0 {
        income += BASE_UPTAKE_MFP * uptake_mult * glucose_sum as i64 / (255 * 1000);
        let take = 1 + mc.uptake_glucose;
        let pw = pixels.w as usize;
        for dy in 0..cell.h as usize {
            for dx in 0..cell.w as usize {
                let i = (cell.py as usize + dy) * pw + cell.px as usize + dx;
                pixels.glucose[i] = pixels.glucose[i].saturating_sub(take);
            }
        }
    }
    income = income * eff_q / 128;

    // ---- maintain
    // Activity (Q8): a photosynthesiser's metabolic rate tracks its light,
    // idling at ¼ in the dark; a cell with no income at all idles too.
    // Upkeep and ageing both scale with it (lifespan ∝ 1/rate, §4), which
    // is what lets a lineage last the 20-minute night.
    let act_q8: i64 = if mc.photo > 0 {
        if light_mean == 255 {
            256
        } else {
            (light_mean as i64).max(ACTIVITY_FLOOR_Q8)
        }
    } else if income > 0 {
        256
    } else {
        ACTIVITY_FLOOR_Q8
    };
    let act_q8 = (act_q8 * q10_q8 / 256).max(16);
    let upkeep =
        (mc.upkeep_fp_tick(area) as i64 * 128 / metab_eff.max(32) * act_q8 / 256) as i32;
    let mut energy = cell.energy + income as i32 - upkeep;
    let store_cap = mc.store_cap_fp(area);
    if energy > store_cap {
        energy = store_cap;
    }
    let acc = cell.age_acc as u16 + act_q8 as u16;
    let age_acc = (acc & 0xFF) as u8;
    let age = cell.age.saturating_add(acc >> 8);
    let body_cost = cells.org_body_cost_fp(cell.org).max(mc.build_fp);
    let max_age = max_age_ticks(base_age, body_cost);

    let mut damage = cell.damage;
    let mut waste = cell.waste;
    if acc >> 8 == 1 && age % 16 == 0 {
        if mc.excrete == 0 {
            waste = waste.saturating_add(1);
        }
        if waste > 200 {
            damage = damage.saturating_add(1);
        }
        if mc.repair > 0 && damage > 0 && energy > FP {
            damage = damage.saturating_sub(mc.repair);
            energy -= FP / 16 * mc.repair as i32;
        }
    }
    if t_tank > HEAT_DAMAGE_C && tick % 4 == 0 {
        damage = damage.saturating_add(2);
    }

    // ---- scalars and stress
    let energy_frac = (energy.max(0) as i64 * 255 / store_cap.max(1) as i64).min(255) as u8;
    let age_frac = ((age as u64 * 255) / max_age.max(1) as u64).min(255) as u8;
    let hunger = 255 - energy_frac;
    let dark = 255u8.saturating_sub(light_mean);
    let heat = if t_tank > 30.0 {
        ((t_tank - 30.0) * 25.0).min(255.0) as u8
    } else {
        0
    };
    let cold = if t_tank < 10.0 {
        ((10.0 - t_tank) * 25.0).min(255.0) as u8
    } else {
        0
    };
    let crowding = touch.saturating_mul(64);
    let stress_sum = (hunger as u32 * stress_def.hunger as u32
        + dark as u32 * stress_def.dark as u32
        + heat as u32 * stress_def.heat as u32
        + cold as u32 * stress_def.cold as u32
        + damage as u32 * stress_def.damage as u32
        + crowding as u32 * stress_def.crowding as u32
        + waste as u32 * stress_def.waste as u32)
        / (255 * 3);
    let stress = stress_sum.min(255) as u8;
    let scalars = Scalars {
        energy_frac,
        age_frac,
        stress,
        damage,
        light: light_mean,
        touch,
        depth: cell.depth,
        sig: cell.sig,
        kin: 0,
        waste,
        temp_dev: ((t_tank - 22.0).abs() * 8.0).min(255.0) as u8,
        sense: light_mean,
    };

    // ---- act
    if mc.excrete > 0 && waste > 0 {
        let (dx, dy) = heading_delta(cell.heading.wrapping_add(4) % 8);
        let x = cell.px as i32 + dx;
        let y = cell.py as i32 + dy;
        if x >= 0 && y >= 0 && x < pixels.w as i32 && y < pixels.h as i32 {
            let j = (y as usize) * (pixels.w as usize) + x as usize;
            let push = mc.excrete.min(waste);
            pixels.waste[j] = pixels.waste[j].saturating_add(push);
            waste -= push;
        }
    }
    if mc.enzyme > 0 && mc.enzyme_gate.passes(&scalars) {
        if mc.enzyme_secrete {
            deposit(pixels, KeyedField::Enzyme, idx, mc.enzyme.saturating_mul(8), mc.enzyme_key);
            energy -= FP / 500 * mc.enzyme as i32;
        }
        // Contact lysis of a soft-litter seat (litter band 0–63).
        if mc.enzyme_key < 64 {
            let (vx, vy) = pixel_to_phys(cell.px, cell.py, grid.visible_h() as u32);
            let seat = grid.get_visible(vx, vy);
            if seat.id() == MaterialId::Organic {
                grid.set_visible(vx, vy, Cell::air());
                pixels.glucose[idx] = pixels.glucose[idx].saturating_add(LITTER_GLUCOSE);
            }
        }
    }
    let _ = wall_key;
    if mc.cilia > 0 {
        let drive = mc.cilia_gate.passes(&scalars);
        if drive {
            energy -= FP / 50 / TICK_HZ as i32 * mc.cilia as i32;
        }
        if let Some(c) = cells.get_mut(id) {
            if !drive && c.running {
                // Tumble: rotate 90° to a hash-chosen side.
                let side = if hash_u64(world_seed, tick, id as u64, 0xB3_01) % 2 == 0 { 2 } else { 6 };
                c.heading = c.heading.wrapping_add(side) % 8;
            }
            c.running = drive;
        }
    }

    // ---- death
    if energy <= 0 || age as u32 > max_age || damage == 255 {
        let cause = if energy <= 0 {
            0
        } else if age as u32 > max_age {
            1
        } else {
            2
        };
        out.death_causes[cause] += 1;
        die(cells, genomes, grid, pixels, id, age, tick, out);
        return;
    }

    if let Some(c) = cells.get_mut(id) {
        c.energy = energy;
        c.age = age;
        c.age_acc = age_acc;
        c.stress = stress;
        c.damage = damage;
        c.waste = waste;
        c.flash = c.flash.saturating_sub(1);
        c.sig = 0;
    }

    // ---- develop: grow, transition, brood release, divide
    if age % T_GROW == 0 && (cell.w < mc.shape_w || cell.h < mc.shape_h) {
        let cost = mc.birth_fp * strategy_q8 as i32 / 256;
        if energy > cost + FP && cells.try_grow(pixels, id, mc.shape_w, mc.shape_h) {
            if let Some(c) = cells.get_mut(id) {
                c.energy -= cost;
            }
        }
    }

    // Stick to a wall / waterline → contribute film.
    if age > FILM_STUCK_AGE
        && cell.parent_cell == NONE
        && near_solid(grid, cell.px, cell.py)
        && pixels.film[idx] < 40
    {
        pixels.film[idx] = 40;
    }

    if tick % 4 == (id as u64 % 4) && try_transition(cells, genomes, pixels, id, &scalars) {
        return;
    }

    if cell.brood {
        if cell.detach_gate.passes(&scalars) {
            if let Some(new_org) = cells.detach(genomes, id, tick) {
                out.events.push(SimEvent {
                    kind: ev::BROOD_RELEASED,
                    arg0: new_org,
                });
            }
        }
        return;
    }

    let Some(cur) = cells.get(id).copied() else {
        return;
    };
    let at_shape = cur.w >= mc.shape_w && cur.h >= mc.shape_h;
    let dw = mc.divide_when;
    if at_shape
        && energy_frac >= dw.energy_frac
        && age >= dw.age as u16 * DIVIDE_AGE_UNIT_TICKS
        && stress <= dw.stress
        && t_tank >= MITOSIS_FLOOR_C
        && cells.len() < LIVE_CAP
        && try_divide(
            cells, genomes, grid, pixels, world_seed, tick, id, &mc, strategy_q8, lambda_m,
            glue_key, out,
        )
    {
        out.divisions += 1;
    }
}

fn die(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    grid: &mut PhysicsGrid,
    pixels: &mut PixelLayer,
    id: u32,
    age: u16,
    tick: u64,
    out: &mut BioStepResult,
) {
    let Some(cell) = cells.get(id).copied() else {
        return;
    };
    let (vx, vy) = pixel_to_phys(cell.px, cell.py, grid.visible_h() as u32);
    // Soft litter: only replace nearly-dry air so free water is conserved.
    let seat = grid.get_visible(vx, vy);
    if seat.is_air() && seat.sat < 40 {
        grid.set_visible(vx, vy, Cell::solid(MaterialId::Organic));
    }
    let to_film = cell.film || age > FILM_STUCK_AGE && near_solid(grid, cell.px, cell.py);
    let fragments = cells.kill(genomes, pixels, id, to_film, tick);
    out.deaths += 1;
    out.events.push(SimEvent {
        kind: ev::DEATH,
        arg0: id,
    });
    for f in fragments {
        out.events.push(SimEvent {
            kind: ev::FRAGMENT,
            arg0: f,
        });
    }
}

/// First passing transition changes the mode in place (§b.4 item 5).
fn try_transition(
    cells: &mut CellStore,
    genomes: &GenomeStore,
    pixels: &mut PixelLayer,
    id: u32,
    scalars: &Scalars,
) -> bool {
    let Some(cell) = cells.get(id).copied() else {
        return false;
    };
    let g = genomes.genome(cell.genotype);
    let mode = g.mode(cell.mode);
    let mut target: Option<u8> = None;
    for i in 0..mode.n_transitions as usize {
        let t = mode.transitions[i];
        if t.mode != cell.mode && t.gate.passes(scalars) {
            target = Some(t.mode);
            break;
        }
    }
    let Some(target) = target else {
        return false;
    };
    let old = genomes.coeffs(cell.genotype, cell.mode);
    let new = genomes.coeffs(cell.genotype, target);
    // Pay build for new units, reclaim 50 % of removed.
    let delta = new.birth_fp - old.birth_fp;
    let cost = if delta > 0 { delta } else { delta / 2 };
    if cell.energy - cost <= FP / 2 {
        return false;
    }
    let new_mode = g.mode(target);
    let reps = if new_mode.n_children > 0 {
        new_mode.children[0].repeat
    } else {
        0
    };
    // Shrink the footprint if the new shape is smaller.
    let (nw, nh) = (cell.w.min(new.shape_w), cell.h.min(new.shape_h));
    if nw != cell.w || nh != cell.h {
        crate::bio::store::release_rect(pixels, cell.px, cell.py, cell.w, cell.h, id + OWNER_BIAS);
        crate::bio::store::claim_rect(pixels, cell.px, cell.py, nw, nh, id + OWNER_BIAS);
    }
    let build_delta = new.build_fp - old.build_fp;
    if let Some(c) = cells.get_mut(id) {
        c.mode = target;
        c.energy -= cost;
        c.child_idx = 0;
        c.child_reps = reps;
        c.w = nw;
        c.h = nh;
    }
    if let Some(o) = cells.org_mut(cell.org) {
        o.body_cost_fp = (o.body_cost_fp + build_delta).max(0);
        if let Some(s) = o.slots.get_mut(cell.slot as usize) {
            s.mode = target;
        }
    }
    true
}

fn try_divide(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    grid: &PhysicsGrid,
    pixels: &mut PixelLayer,
    world_seed: u64,
    tick: u64,
    parent: u32,
    mc: &ModeCoeffs,
    strategy_q8: u32,
    lambda_m: u32,
    glue_key: u8,
    out: &mut BioStepResult,
) -> bool {
    let _ = glue_key;
    let Some(cell) = cells.get(parent).copied() else {
        return false;
    };
    let genotype = cell.genotype;
    // Advance past exhausted child entries.
    let (spec, child_idx, child_reps) = {
        let g = genomes.genome(genotype);
        let mode = g.mode(cell.mode);
        let mut child_idx = cell.child_idx;
        let mut child_reps = cell.child_reps;
        while child_idx < mode.n_children && child_reps == 0 {
            child_idx += 1;
            child_reps = mode
                .children
                .get(child_idx as usize)
                .map(|c| c.repeat)
                .unwrap_or(0);
        }
        if child_idx >= mode.n_children || child_reps == 0 {
            if let Some(c) = cells.get_mut(parent) {
                c.child_idx = child_idx.min(mode.n_children);
                c.child_reps = 0;
            }
            // Program exhausted: regrow a dead child slot instead (§b.5).
            return mc.regrow
                && try_regrow(cells, genomes, pixels, parent, mc, strategy_q8, out);
        }
        (mode.children[child_idx as usize], child_idx, child_reps)
    };
    if spec.kind != ChildKind::Grow {
        // Secretions land in B3 (2.3.4); skip the entry for now.
        if let Some(c) = cells.get_mut(parent) {
            c.child_idx = child_idx + 1;
            c.child_reps = genomes
                .genome(genotype)
                .mode(cell.mode)
                .children
                .get(c.child_idx as usize)
                .map(|ch| ch.repeat)
                .unwrap_or(0);
        }
        return false;
    }
    let immediate = matches!(spec.detach, Detach::Immediate);
    // Place the daughter adjacent to the footprint on the chosen side. Phase 2
    // life is aquatic: the seat must be free water. A propagule that finds
    // air or a neighbour there tries the other cardinal sides instead.
    let vis_h = grid.visible_h() as u32;
    let seat_ok = |nx: i32, ny: i32| -> bool {
        if nx < 0 || ny < 0 || nx >= pixels.w as i32 || ny >= pixels.h as i32 {
            return false;
        }
        if pixels.owner[(ny as usize) * (pixels.w as usize) + nx as usize] != 0 {
            return false;
        }
        let (vx, vy) = pixel_to_phys(nx as u16, ny as u16, vis_h);
        let seat = grid.get_visible(vx, vy);
        seat.is_air() && seat.sat > 40
    };
    let (dx, dy) = spec.dir.delta(cell.heading);
    let mut nx = cell.px as i32 + side_offset(dx, cell.w);
    let mut ny = cell.py as i32 + side_offset(dy, cell.h);
    if !seat_ok(nx, ny) {
        if !immediate {
            return false;
        }
        let start = hash_u64(world_seed, tick, parent as u64, 0xD1_05) % 4;
        let mut found = false;
        for k in 0..4u64 {
            let h = ((start + k) % 4) as u8 * 2;
            let (ddx, ddy) = heading_delta(h);
            let cx = cell.px as i32 + side_offset(ddx, cell.w);
            let cy = cell.py as i32 + side_offset(ddy, cell.h);
            if seat_ok(cx, cy) {
                nx = cx;
                ny = cy;
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
    }
    let child_cc = *genomes.coeffs(genotype, spec.mode);
    // Pace law: a propagule costs its full mature build; a part its birth.
    let cost = if immediate {
        child_cc.build_fp * strategy_q8 as i32 / 256 + BIRTH_ENERGY_FP
    } else {
        child_cc.birth_fp * strategy_q8 as i32 / 256 + BIRTH_ENERGY_FP / 2
    };
    if cell.energy - cost < FP {
        return false;
    }
    let daughter_energy = if immediate {
        child_cc.build_fp - child_cc.birth_fp + BIRTH_ENERGY_FP
    } else {
        BIRTH_ENERGY_FP / 2
    };
    // Mutation: Poisson-ish count from λ (cap 2).
    let roll = hash_u64(world_seed, tick, parent as u64, 0xD1_01) % 1000;
    let mutate = roll < lambda_m.min(999) as u64;
    let geno = if mutate {
        let g = genomes.genome(genotype);
        let mut child_g = g.mutate_once(world_seed, tick ^ ((parent as u64) << 8));
        if hash_u64(world_seed, tick, parent as u64, 0xD1_02) % 1000 < (lambda_m / 2) as u64 {
            child_g = child_g.mutate_once(world_seed, tick ^ ((parent as u64) << 8) ^ 0x5A);
        }
        genomes.intern_child(genotype, child_g)
    } else {
        genotype
    };
    // Daughter heading: she faces the way she was placed, turned by her
    // own mode's rule (Inherit ⇒ Ahead children keep the parent's heading,
    // side children face away from the parent).
    let heading = {
        let g = genomes.genome(genotype);
        let placed = spec.dir.heading(cell.heading);
        g.mode(spec.mode).heading_rule.apply(placed)
    };
    let daughter = match spec.detach {
        Detach::Immediate => {
            let gen = cells.org_generation(cell.org).saturating_add(1);
            // A mutant daughter runs her own genome's child mode index.
            let mode = if mutate {
                genomes.genome(geno).seed_mode
            } else {
                spec.mode
            };
            cells.spawn(
                genomes,
                pixels,
                nx as u16,
                ny as u16,
                geno,
                mode,
                gen,
                daughter_energy,
                heading,
                tick,
                world_seed,
            )
        }
        Detach::Never | Detach::When(_) => {
            let brood = match spec.detach {
                Detach::When(g) => Some(g),
                _ => None,
            };
            // Body parts share the parent's genotype; a mutation applies to
            // the propagules this body later releases, not to its own parts
            // (the realised body stays one genotype so inspect reads clean).
            cells.spawn_bonded(
                genomes,
                pixels,
                parent,
                nx as u16,
                ny as u16,
                spec.mode,
                spec.bond,
                heading,
                daughter_energy,
                brood,
            )
        }
    };
    let Some(did) = daughter else {
        return false;
    };
    if let Some(c) = cells.get_mut(parent) {
        c.energy = (c.energy - cost).max(0);
        c.flash = 2;
        let left = child_reps.saturating_sub(1);
        if left == 0 {
            c.child_idx = child_idx.saturating_add(1);
            c.child_reps = genomes
                .genome(genotype)
                .mode(cell.mode)
                .children
                .get(c.child_idx as usize)
                .map(|ch| ch.repeat)
                .unwrap_or(0);
        } else {
            c.child_idx = child_idx;
            c.child_reps = left;
        }
    }
    if let Some(d) = cells.get_mut(did) {
        d.flash = 2;
    }
    out.events.push(SimEvent {
        kind: ev::DIVISION,
        arg0: did,
    });
    if !immediate && cells.org_slot_count(cell.org) == 2 {
        out.events.push(SimEvent {
            kind: ev::MULTICELLULAR,
            arg0: cell.org,
        });
    }
    if mutate && immediate {
        out.events.push(SimEvent {
            kind: ev::MUTANT,
            arg0: geno,
        });
    }
    true
}

/// Regrow the first dead child slot at full build cost.
fn try_regrow(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    pixels: &mut PixelLayer,
    parent: u32,
    _mc: &ModeCoeffs,
    strategy_q8: u32,
    out: &mut BioStepResult,
) -> bool {
    let Some((org, slot)) = cells.dead_child_slot(parent) else {
        return false;
    };
    let Some(cell) = cells.get(parent).copied() else {
        return false;
    };
    let slot_mode = match cells.org(org).and_then(|o| o.slots.get(slot as usize)) {
        Some(s) => s.mode,
        None => return false,
    };
    let cc = *genomes.coeffs(cell.genotype, slot_mode);
    let cost = cc.birth_fp * strategy_q8 as i32 / 256 + BIRTH_ENERGY_FP / 2;
    if cell.energy - cost < FP {
        return false;
    }
    let Some(did) = cells.regrow_slot(genomes, pixels, org, slot, BIRTH_ENERGY_FP / 2) else {
        return false;
    };
    if let Some(c) = cells.get_mut(parent) {
        c.energy -= cost;
        c.flash = 2;
    }
    out.events.push(SimEvent {
        kind: ev::REGROWN,
        arg0: did,
    });
    true
}

// ---------------------------------------------------------------- body passes

/// Bonds carry glucose: each bonded pair moves 1/N of its difference in
/// *fill fraction* per tick, so a STORE cell can hold more than its neighbours.
fn share_bond_energy(cells: &mut CellStore, genomes: &GenomeStore) {
    let pairs: Vec<(u32, u32)> = cells
        .iter_alive()
        .filter(|(_, c)| c.parent_cell != NONE)
        .map(|(id, c)| (id, c.parent_cell))
        .collect();
    for (child, parent) in pairs {
        let (Some(c), Some(p)) = (cells.get(child).copied(), cells.get(parent).copied()) else {
            continue;
        };
        let c_cap = genomes.coeffs(c.genotype, c.mode).store_cap_fp(c.area()).max(1) as i64;
        let p_cap = genomes.coeffs(p.genotype, p.mode).store_cap_fp(p.area()).max(1) as i64;
        // Equalise fill fractions: target child energy at the shared fraction.
        let total = c.energy as i64 + p.energy as i64;
        let frac_q16 = (total << 16) / (c_cap + p_cap);
        let c_target = (frac_q16 * c_cap) >> 16;
        let diff = ((c_target - c.energy as i64) / BOND_SHARE_DIV as i64) as i32;
        if diff == 0 {
            continue;
        }
        if let Some(pc) = cells.get_mut(parent) {
            pc.energy -= diff;
        }
        if let Some(cc) = cells.get_mut(child) {
            cc.energy += diff;
        }
    }
}

/// Starving bodies shed the highest `sacrifice_order` cell first; 50 % of
/// its birth cost returns to its parent (§b.5).
fn shed(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    pixels: &mut PixelLayer,
    tick: u64,
    out: &mut BioStepResult,
) {
    let orgs: Vec<u32> = cells
        .iter_orgs()
        .filter(|(_, o)| o.n_alive >= 2)
        .map(|(id, _)| id)
        .collect();
    for org_id in orgs {
        let Some(o) = cells.org(org_id) else {
            continue;
        };
        let ids: Vec<u32> = o
            .slots
            .iter()
            .filter(|s| s.cell != NONE)
            .map(|s| s.cell)
            .collect();
        let Some(seed) = cells.get(o.seed) else {
            continue;
        };
        let thr_per_cell = genomes.genome(seed.genotype).sacrifice_threshold as i32 * FP / 64;
        let mut total = 0i32;
        let mut victim: Option<(u32, u8, u8, u32)> = None;
        for &id in &ids {
            let Some(c) = cells.get(id) else {
                continue;
            };
            total += c.energy;
            if c.parent_cell == NONE {
                continue;
            }
            let order = genomes.coeffs(c.genotype, c.mode).sacrifice_order;
            let better = match victim {
                None => true,
                Some((_, vo, vd, _)) => order > vo || (order == vo && c.depth > vd),
            };
            if better {
                victim = Some((id, order, c.depth, c.parent_cell));
            }
        }
        if total >= thr_per_cell * ids.len() as i32 {
            continue;
        }
        let Some((vid, _, _, parent)) = victim else {
            continue;
        };
        let refund = cells
            .get(vid)
            .map(|c| genomes.coeffs(c.genotype, c.mode).birth_fp / 2)
            .unwrap_or(0);
        let _ = cells.kill(genomes, pixels, vid, false, tick);
        out.deaths += 1;
        out.death_causes[3] += 1;
        if let Some(p) = cells.get_mut(parent) {
            p.energy += refund;
        }
        out.events.push(SimEvent {
            kind: ev::SHED,
            arg0: org_id,
        });
    }
}

/// Adjacent GLUE cells of different organisms form a kin bond (no merge).
fn form_glue_bonds(cells: &mut CellStore, genomes: &GenomeStore, pixels: &PixelLayer) {
    let ids: Vec<u32> = cells
        .iter_alive()
        .filter(|(_, c)| genomes.coeffs(c.genotype, c.mode).glue > 0)
        .map(|(id, _)| id)
        .collect();
    for &id in &ids {
        let Some(cell) = cells.get(id).copied() else {
            continue;
        };
        let g = genomes.genome(cell.genotype);
        let (my_key, my_width) = (g.glue_key, g.glue_width);
        let glue_u = genomes.coeffs(cell.genotype, cell.mode).glue;
        if cell.glue_to != NONE {
            if let Some(other) = cells.get(cell.glue_to) {
                if other.org != cell.org
                    && (other.px as i32 - cell.px as i32).abs() <= 1
                    && (other.py as i32 - cell.py as i32).abs() <= 1
                {
                    continue;
                }
            }
            if let Some(c) = cells.get_mut(id) {
                c.glue_to = NONE;
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
            if other.org == cell.org {
                continue;
            }
            if genomes.coeffs(other.genotype, other.mode).glue == 0 {
                continue;
            }
            let their_key = genomes.genome(other.genotype).glue_key;
            let strength = glue_bond(my_key, their_key, my_width, glue_u.saturating_mul(40));
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

/// Offset of the adjacent pixel on the `d` side of a footprint of extent `ext`.
pub fn side_offset(d: i32, ext: u8) -> i32 {
    match d.cmp(&0) {
        std::cmp::Ordering::Greater => ext as i32,
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
    }
}
