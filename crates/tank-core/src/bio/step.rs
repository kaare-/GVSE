//! Biology tick: sense → feed → maintain → act → develop (§b.2), then
//! body-level passes: bond energy sharing, shedding, free-body drift,
//! GLUE bonding, fields.
#![allow(clippy::too_many_arguments)]

use wk_material::MaterialId;

use crate::bio::chem::{self, HIT_ENZYME, HIT_TOXIN};
use crate::bio::fields::{add_plain, step_fields, Field};
use crate::bio::genome::GenomeStore;
use crate::bio::hydrate;
use crate::bio::minerals;
use crate::bio::profile::{ModeCoeffs, DIVIDE_AGE_UNIT_TICKS};
use crate::bio::program::{glue_bond, heading_delta, ChildKind, Detach, Scalar, Scalars};
use crate::bio::store::{secretion_kind, CellStore, EMPTY_OWNER, NONE, OWNER_BIAS};
use crate::cell::Cell;
use crate::consts::{
    ACTIVITY_FLOOR_Q8, ALGAE_COST_FP, BIRTH_ENERGY_FP, BOND_SHARE_DIV, DESICCATE_BASE,
    DESICCATE_EVERY, FP, GLUCOSE_UNIT_FP, HEAT_DAMAGE_C, HYDRATION_EVERY, LIVE_CAP,
    MITOSIS_FLOOR_C, PHOTO_YIELD_MFP, THERMO_COST_FP_TICK_PER_UNIT, THERMO_INJECT_EVERY, TICK_HZ,
    T_GROW, WARM_BLOODED_Q2, WARM_BLOODED_TICKS,
};
use crate::grid::PhysicsGrid;
use crate::hash::hash_u64;
use crate::pixels::{CoarseField, PixelLayer};
use crate::tank::SimEvent;

const FILM_STUCK_AGE: u16 = 45;
/// Lysis of a soft-litter *seat* (dry-air Organic cell) yields this much
/// dissolved glucose; in water the dead leave pixel litter instead.
const LITTER_GLUCOSE: u8 = 40;
/// Minimum received signal that counts as "heard" for the signalling event.
const SIGNAL_HEARD: u8 = 16;

pub struct BioStepResult {
    pub events: Vec<SimEvent>,
    pub divisions: u32,
    pub deaths: u32,
    /// Deaths by cause this tick: [starved, old age, damage, shed].
    pub death_causes: [u32; 4],
    /// Suspended fines released by crumbling corpses this tick:
    /// `(pixel column, glucose units)` (§a.4 S3; consumed by Phase 4).
    pub fines: Vec<(u16, u32)>,
}

/// Share of a corpse's soft mass that drifts off as fines (Q8).
pub const FINES_SHARE_Q8: u32 = 77; // 30 %
/// Corpses settle one pixel per this many ticks.
const CORPSE_SINK_EVERY: u64 = 2;
/// Fungus-made soil is checked for this often (`fields::make_soil`).
const SOIL_EVERY: u64 = 16;

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
    /// A living cell was killed by another organism's enzyme (arg0 = victim).
    pub const PREDATION: u32 = 16;
    /// A living cell was killed by toxin (arg0 = victim).
    pub const POISON: u32 = 17;
    /// A RECEPTOR cell heard a signal of its own kind (arg0 = listener).
    pub const SIGNALLING: u32 = 18;
    /// A cell lives mainly on dissolved glucose, not light (arg0 = cell).
    pub const HETEROTROPH: u32 = 19;
    /// Digestion turned sand into soil (arg0 = soil cells made this pass).
    pub const FUNGUS: u32 = 20;
    /// Something moved: a cell or body swam ≥ 3 px on its cilia within
    /// 60 ticks, not counting buoyancy, drift or jitter (arg0 = cell).
    pub const MOVED: u32 = 21;

    // Milestones (2.3.8, `milestones.rs`): the player-facing "firsts",
    // each once per tank unless noted. Kinds 1–21 above are the raw
    // stream they are derived from; `LIFELESS` and `GLASS` are the
    // repeatable milestones and keep their kinds.
    /// First living cell in the tank (arg0 = cell).
    pub const MS_FIRST_LIFE: u32 = 32;
    pub const MS_FIRST_DIVISION: u32 = 33;
    pub const MS_FIRST_DEATH: u32 = 34;
    /// A lineage's colour drifted from its founder's (arg0 = genotype);
    /// repeats per lineage, stepped and rate-limited.
    pub const MS_NEW_COLOUR: u32 = 35;
    /// A multicellular body that breeds true (arg0 = organism).
    pub const MS_MULTICELLULAR: u32 = 36;
    /// A cell of area ≥ 16 px (arg0 = cell).
    pub const MS_GIANT: u32 = 37;
    pub const MS_FUNGUS: u32 = 38;
    /// A glucose-fed lineage three generations deep (arg0 = cell).
    pub const MS_HETEROTROPH: u32 = 39;
    pub const MS_PREDATION: u32 = 40;
    pub const MS_POISON: u32 = 41;
    pub const MS_SIGNALLING: u32 = 42;
    pub const MS_MOVED: u32 = 43;
    /// A new species was founded (arg0 = species id); raw stream, every
    /// founding.
    pub const NEW_SPECIES: u32 = 22;
    /// Milestone: a new species, rate-limited (arg0 = species id).
    pub const MS_NEW_SPECIES: u32 = 44;
    /// Raw (Phase 4): a THERMO cell has held ≥ +3 °C over ambient for
    /// `WARM_BLOODED_TICKS` (arg0 = cell).
    pub const WARM_BLOODED: u32 = 23;
    /// Milestone: first warm-blooded body.
    pub const MS_WARM_BLOODED: u32 = 45;
    /// Repeatable (Phase 4): the water is cloudy — mean cloud factor under
    /// `CLOUDY_MEAN` (arg0 = haze 0..255).
    pub const CLOUDY: u32 = 24;
    /// Raw (Phase 4): a living cell sits on the air side (arg0 = cell).
    pub const LIFE_ON_LAND: u32 = 25;
    /// Milestone: first life on land.
    pub const MS_LIFE_ON_LAND: u32 = 46;
    /// Raw (Phase 4): a fragment at `depth == 0` grew a root into moist
    /// substrate (arg0 = cell).
    pub const CUTTING_TOOK_ROOT: u32 = 26;
    /// Milestone: first cutting that took root.
    pub const MS_CUTTING_TOOK_ROOT: u32 = 47;
    /// Raw (Phase 4): a body shows a colour crown — ≥ 3 pigment-dominant
    /// tip cells over PHOTO-dominant tissue (arg0 = organism).
    pub const COLOUR_CROWN: u32 = 27;
    /// Milestone: first colour crown.
    pub const MS_COLOUR_CROWN: u32 = 48;
    /// Raw (Phase 5): a geometric eye — ≥2 light SENSOR cells around a
    /// TRANSPARENT window with NERVE and a motor (arg0 = organism).
    pub const EYE: u32 = 28;
    /// Milestone: first eye.
    pub const MS_EYE: u32 = 49;
    /// Raw (Phase 5): a muscle-driven body swam ≥ 3 px (arg0 = organism).
    pub const LOCOMOTION: u32 = 29;
    /// Milestone: first locomotion.
    pub const MS_LOCOMOTION: u32 = 50;
    /// Raw (Phase 5): a swallowed organism left an eater alive (arg0 = prey org).
    pub const ESCAPE: u32 = 30;
    /// Milestone: first escape.
    pub const MS_ESCAPE: u32 = 51;
    /// Raw (Phase 5): a geometric mouth — hard walls + enzyme lining +
    /// muscle opening enclosing water (arg0 = organism).
    pub const CHAMBER: u32 = 31;
    /// Milestone: first chamber / mouth.
    pub const MS_CHAMBER: u32 = 52;
}

/// "Something moved" (§a.6): net cilia displacement over this window.
const MOVED_WINDOW: u64 = 60;
const MOVED_PX: i32 = 3;
/// A running swimmer tumbles on its own about this often, so a blind cell
/// still explores.
const SPONTANEOUS_TUMBLE: u64 = 90;
/// Thrust units → speed: `px/s = 4·units / perimeter_px` (a 1×1 cell with
/// one beating unit makes ≈ 1 px/s; a 6×6 needs 6× the units, §a.6).
const THRUST_PX_S_NUM: u64 = 4;

pub fn step(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    grid: &mut PhysicsGrid,
    pixels: &mut PixelLayer,
    coarse: &mut CoarseField,
    world_seed: u64,
    tick: u64,
    t_tank: f32,
    lambda_scale_q8: u32,
    grav_x: f32,
) -> BioStepResult {
    genomes.set_tick(tick);
    let mut out = BioStepResult {
        events: Vec::new(),
        divisions: 0,
        deaths: 0,
        death_causes: [0; 4],
        fines: Vec::new(),
    };
    if tick % HYDRATION_EVERY == 0 {
        hydrate::refresh(cells, genomes, grid, coarse);
    }
    let (_, anchored) = hydrate::bound_cells(cells, genomes, grid, pixels);
    let ids = cells.live_ids();
    for id in ids {
        if cells.get(id).is_none() {
            continue;
        }
        move_free_cell(cells, genomes, grid, pixels, world_seed, tick, id);
        metabolise(
            cells, genomes, grid, pixels, coarse, world_seed, tick, id, t_tank, lambda_scale_q8,
            grav_x, &mut out,
        );
    }
    share_bond_energy(cells, genomes);
    if tick % 16 == 5 {
        shed(cells, genomes, pixels, tick, &mut out);
    }
    if tick % 4 == 1 {
        drift_bodies(cells, genomes, grid, pixels, world_seed, tick, &anchored);
    }
    swim_bodies(cells, genomes, grid, pixels, world_seed, tick, &anchored);
    form_glue_bonds(cells, genomes, pixels);
    step_corpses(cells, grid, pixels, tick, &mut out);
    rebuild_block(cells, genomes, pixels);
    step_fields(pixels, grid, coarse, tick);
    minerals::diffuse(coarse, grid, tick);
    if tick % SOIL_EVERY == 7 {
        let made = crate::bio::fields::make_soil(grid, coarse);
        if made > 0 {
            out.events.push(SimEvent {
                kind: ev::FUNGUS,
                arg0: made,
            });
        }
    }
    out
}

/// Corpses (§a.4 detritus): sink through water a pixel at a time (film
/// corpses stay stuck), and crumble into litter when their time is up or
/// when the saprotrophs have eaten all of their soft mass.
fn step_corpses(
    cells: &mut CellStore,
    grid: &PhysicsGrid,
    pixels: &mut PixelLayer,
    tick: u64,
    out: &mut BioStepResult,
) {
    let vis_h = grid.visible_h() as u32;
    for id in cells.corpse_ids() {
        let Some(c) = cells.corpse(id).copied() else {
            continue;
        };
        if c.energy < GLUCOSE_UNIT_FP || cells.corpse_decay(id, 1) {
            crumble_corpse(cells, pixels, id, out);
            continue;
        }
        if c.film || tick % CORPSE_SINK_EVERY != 0 {
            continue;
        }
        let below_py = c.py + c.h as u16;
        if below_py as u32 >= pixels.h {
            continue;
        }
        let (bx, by) = pixel_to_phys(c.px, below_py, vis_h);
        if grid.get_visible(bx, by).is_open() {
            let _ = cells.move_corpse(pixels, id, c.px, c.py + 1);
        }
    }
}

/// A corpse becomes litter where it lies: 70 % of its soft mass as soft
/// litter, 30 % as suspended fines (2.3.7), and its wall minerals as bone.
fn crumble_corpse(cells: &mut CellStore, pixels: &mut PixelLayer, id: u32, out: &mut BioStepResult) {
    let Some(c) = cells.expire_corpse(pixels, id) else {
        return;
    };
    let soft_units = (c.energy.max(0) / GLUCOSE_UNIT_FP) as u32;
    let fines_units = soft_units * FINES_SHARE_Q8 / 256;
    chem::deposit_corpse(pixels, &c, soft_units - fines_units, c.corpse_bone as u32);
    if fines_units > 0 {
        out.fines.push((c.px + (c.w / 2) as u16, fines_units));
    }
}

/// Diffusion barrier mask (§b.3): hard living tissue (`h > 0.5`) and dead
/// bone / shell pixels. Rebuilt every tick so it never goes stale.
fn rebuild_block(cells: &CellStore, genomes: &GenomeStore, pixels: &mut PixelLayer) {
    pixels.block.fill(0);
    let pw = pixels.w as usize;
    for (_, c) in cells.iter_alive() {
        if !genomes.coeffs(c.genotype, c.mode).is_hard {
            continue;
        }
        for dy in 0..c.h as usize {
            for dx in 0..c.w as usize {
                pixels.block[(c.py as usize + dy) * pw + c.px as usize + dx] = 1;
            }
        }
    }
    for (_, o) in cells.iter_orgs() {
        for sec in &o.secretions {
            pixels.block[(sec.py as usize) * pw + sec.px as usize] = 1;
        }
    }
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
        if !n.is_open() {
            return true;
        }
    }
    let here = grid.get_visible(vx, vy);
    let above = grid.get_visible(vx, vy + 1);
    here.is_open() && here.sat > 100 && above.is_open() && above.sat < 40
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
    // A GLUE attachment holds the cell in its colony (bonds carry force).
    if cell.glue_to != NONE {
        return;
    }
    // A root co-occupying moist sand/soil is static (§h.1).
    {
        let (vx, vy) = pixel_to_phys(cell.px, cell.py, grid.visible_h() as u32);
        let seat = grid.get_visible(vx, vy);
        if genomes.coeffs(cell.genotype, cell.mode).uptake_water >= 2 && !seat.is_open() {
            return;
        }
    }
    // Multi-cell bodies drift as a unit in `drift_bodies`.
    if cells.org_slot_count(cell.org) > 1 {
        return;
    }
    let g = genomes.genome(cell.genotype);
    let buoy = g.buoy;
    let mc = *genomes.coeffs(cell.genotype, cell.mode);
    let vis_h = grid.visible_h() as u32;
    let (vx, vy) = pixel_to_phys(cell.px, cell.py, vis_h);
    let here = grid.get_visible(vx, vy);
    // The physics cell under the footprint's bottom pixel row, so a cell
    // settles onto the floor pixel rather than the top of the floor cell.
    let below = if (cell.py + cell.h as u16) as u32 >= pixels.h {
        Cell::bedrock()
    } else {
        let (bx, by) = pixel_to_phys(cell.px, cell.py + cell.h as u16, vis_h);
        grid.get_visible(bx, by)
    };
    let in_water = here.is_open() && here.sat > 40;

    let h = hash_u64(world_seed, tick, id as u64, 0xB0_01);
    let mut nx = cell.px as i32;
    let mut ny = cell.py as i32;
    // Brownian jitter of one pixel: a free cell cannot jump a 1-px wall.
    // A swimmer's motion is its motor's; it jitters a quarter as much.
    if in_water && (mc.cilia == 0 || tick % 4 == 0) {
        nx += (h % 3) as i32 - 1;
    }
    let mut thrust: Option<(i32, i32)> = None;
    if mc.cilia > 0 && in_water {
        // Run-and-tumble (§a.6): the drive was set in `metabolise`; a run
        // moves one pixel along the heading at the thrust speed.
        if cell.running {
            let (dx, dy) = heading_delta(cell.heading);
            if h % thrust_period(mc.cilia as u32, 2 * (cell.w as u32 + cell.h as u32)) == 0 {
                nx += dx;
                ny += dy;
                thrust = Some((dx, dy));
            }
        }
        if (h >> 20) % SPONTANEOUS_TUMBLE == 0 {
            if let Some(c) = cells.get_mut(id) {
                let side = if (h >> 30) % 2 == 0 { 2 } else { 6 };
                c.heading = c.heading.wrapping_add(side) % 8;
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
    let solid_below = !below.is_open();
    if solid_below && ny > cell.py as i32 {
        ny = cell.py as i32;
    }
    let pw = pixels.w as i32;
    let ph = pixels.h as i32;
    nx = nx.clamp(0, pw - cell.w as i32);
    ny = ny.clamp(0, ph - cell.h as i32);
    if nx != cell.px as i32 || ny != cell.py as i32 {
        let moved = cells.move_cell(pixels, id, nx as u16, ny as u16);
        if moved {
            if let Some((dx, dy)) = thrust {
                if let Some(c) = cells.get_mut(id) {
                    c.moved_dx = c.moved_dx.saturating_add(dx as i8);
                    c.moved_dy = c.moved_dy.saturating_add(dy as i8);
                }
            }
        }
        // A sinking cell blocked from below slides off diagonally, so free
        // cells never pile up out of the water.
        if !moved && ny > cell.py as i32 && thrust.is_none() {
            let side = if h & 0x100 == 0 { 1 } else { -1 };
            let sx = (cell.px as i32 + side).clamp(0, pw - cell.w as i32);
            let _ = cells.move_cell(pixels, id, sx as u16, ny as u16);
        }
    }
}

/// Ticks between 1-px thrust steps for `units` beating cilia units on a
/// body of `perimeter_px` pixels (§a.6 speed law).
fn thrust_period(units: u32, perimeter_px: u32) -> u64 {
    if units == 0 {
        return u64::MAX;
    }
    (TICK_HZ * perimeter_px as u64 / (THRUST_PX_S_NUM * units as u64)).max(1)
}

/// SENSOR (§a.4 #14 / §h.5). Light / chem: contrast along a heading ray.
/// Touch: `touch × 64`. Colour: first owned cell's hue vs `sensor_hue`
/// ring-match. Temp / humidity: local scalars. Gravity: |tilt| × 255.
fn sensor_read(
    cells: &CellStore,
    genomes: &GenomeStore,
    pixels: &PixelLayer,
    grid: &PhysicsGrid,
    c: &crate::bio::CellRec,
    mc: &ModeCoeffs,
    here_light: u8,
    here_glucose: u8,
    touch: u8,
    humidity: u8,
    t_tank: f32,
    grav_x: f32,
) -> u8 {
    use crate::bio::profile::{
        SENSOR_CHEM, SENSOR_COLOUR, SENSOR_GRAVITY, SENSOR_HUMIDITY, SENSOR_TEMP, SENSOR_TOUCH,
    };
    use crate::bio::program::ring_match_q8;
    match mc.sensor_kind {
        SENSOR_TOUCH => return touch.saturating_mul(64),
        SENSOR_TEMP => {
            // 22 °C → 128; ±16 °C → edges.
            return ((t_tank - 6.0) * (255.0 / 32.0)).clamp(0.0, 255.0) as u8;
        }
        SENSOR_HUMIDITY => return humidity,
        SENSOR_GRAVITY => {
            return (grav_x.abs().min(1.0) * 255.0) as u8;
        }
        _ => {}
    }
    let (dx, dy) = heading_delta(c.heading);
    // First pixel outside the footprint on the heading side, centred.
    let (mut x, mut y) = if dx != 0 {
        (c.px as i32 + side_offset(dx, c.w), c.py as i32 + c.h as i32 / 2)
    } else {
        (c.px as i32 + c.w as i32 / 2, c.py as i32 + side_offset(dy, c.h))
    };
    let pw = pixels.w as i32;
    let ph = pixels.h as i32;
    let vis_h = grid.visible_h() as u32;
    let len = 8 * mc.sensor as i32;
    if mc.sensor_kind == SENSOR_COLOUR {
        for _ in 0..len {
            if x < 0 || y < 0 || x >= pw || y >= ph {
                break;
            }
            let i = (y * pw + x) as usize;
            let owner = pixels.owner[i];
            if owner != EMPTY_OWNER {
                if owner < OWNER_BIAS || secretion_kind(owner).is_some() {
                    break;
                }
                let Some(seen) = cells.get(owner - OWNER_BIAS) else {
                    break;
                };
                let rgb = genomes.coeffs(seen.genotype, seen.mode).colour;
                let hue = rgb_to_hue(rgb[0], rgb[1], rgb[2]);
                // Width 48 ≈ ±34° of a 256-hue circle — selective but not brittle.
                return ring_match_q8(hue, mc.sensor_hue, 48);
            }
            let (vx, vy) = pixel_to_phys(x as u16, y as u16, vis_h);
            if !grid.get_visible(vx, vy).is_open() {
                break;
            }
            x += dx;
            y += dy;
        }
        return 0;
    }
    let field = if mc.sensor_kind == SENSOR_CHEM {
        &pixels.glucose
    } else {
        &pixels.light
    };
    let here = if mc.sensor_kind == SENSOR_CHEM {
        here_glucose
    } else {
        here_light
    } as i32;
    let mut far: Option<i32> = None;
    for _ in 0..len {
        if x < 0 || y < 0 || x >= pw || y >= ph {
            break;
        }
        let i = (y * pw + x) as usize;
        if pixels.owner[i] != EMPTY_OWNER {
            break;
        }
        let (vx, vy) = pixel_to_phys(x as u16, y as u16, vis_h);
        if !grid.get_visible(vx, vy).is_open() {
            break;
        }
        far = Some(field[i] as i32);
        x += dx;
        y += dy;
    }
    match far {
        None => 0,
        Some(f) => (128 + (f - here).clamp(-127, 127)) as u8,
    }
}

/// Coarse RGB → hue byte on a 256-circle (HSV-style).
fn rgb_to_hue(r: u8, g: u8, b: u8) -> u8 {
    let r = r as i32;
    let g = g as i32;
    let b = b as i32;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d == 0 {
        return 0;
    }
    let h = if max == r {
        ((g - b) * 43) / d
    } else if max == g {
        85 + ((b - r) * 43) / d
    } else {
        171 + ((r - g) * 43) / d
    };
    h.rem_euclid(256) as u8
}

/// Motor drive (§a.6, Q8): the CILIA gate contributes +1 when it passes;
/// each intra-cell link adds `weight · input · 4` where input is `scalar −
/// threshold` (`Level`) or `scalar − running average` (`Rise`). Positive
/// drive = run.
fn motor_drive(mc: &ModeCoeffs, s: &Scalars, avg: &mut [u16; crate::consts::MAX_LINKS]) -> i32 {
    let mut d: i32 = if mc.cilia_gate.passes(s) { 256 } else { 0 };
    for (l, a) in mc.links.iter().zip(avg.iter_mut()).take(mc.n_links as usize) {
        let v = s.read(l.scalar) as i32;
        let v = if l.scalar == Scalar::Touch { (v * 64).min(255) } else { v };
        let input = if l.rise {
            let mean = ((*a as i32 + 128) >> 8).min(255);
            *a = (*a as i32 + ((v << 8) - *a as i32) / 16).clamp(0, 255 << 8) as u16;
            v - mean
        } else {
            v - l.threshold as i32
        };
        d += l.weight as i32 * input * 4;
    }
    d
}

/// Where a free body sits.
struct BodyEnv {
    /// Some cell is surface film.
    film: bool,
    /// Some cell touches solid substrate (holds a drifting body still;
    /// cilia may still push off it).
    touching: bool,
    all_water: bool,
    sand_above: bool,
    /// Some cell bears cilia: the body swims and, like a free cilia cell,
    /// holds station instead of sinking or floating passively.
    cilia: bool,
}

fn body_env(cells: &CellStore, genomes: &GenomeStore, grid: &PhysicsGrid, org_id: u32) -> Option<BodyEnv> {
    let o = cells.org(org_id)?;
    let mut env = BodyEnv { film: false, touching: false, all_water: true, sand_above: false, cilia: false };
    for s in o.slots.iter().filter(|s| s.cell != NONE) {
        let Some(c) = cells.get(s.cell) else {
            continue;
        };
        if c.film {
            env.film = true;
            return Some(env);
        }
        if near_solid(grid, c.px, c.py) {
            env.touching = true;
        }
        if genomes.coeffs(c.genotype, c.mode).cilia > 0 {
            env.cilia = true;
        }
        let (vx, vy) = pixel_to_phys(c.px, c.py, grid.visible_h() as u32);
        let here = grid.get_visible(vx, vy);
        if !(here.is_open() && here.sat > 40) {
            env.all_water = false;
        }
        let above = grid.get_visible(vx, vy + 1);
        if above.id() == MaterialId::Sand {
            env.sand_above = true;
        }
    }
    Some(env)
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
    anchored: &[u32],
) {
    let orgs: Vec<(u32, i8)> = cells
        .iter_orgs()
        .filter(|(_, o)| o.n_alive >= 2)
        .map(|(id, o)| {
            let g = cells
                .get(o.seed)
                .map(|c| genomes.genome(c.genotype).buoy)
                .unwrap_or(0);
            (id, g)
        })
        .collect();
    for (org_id, buoy) in orgs {
        let Some(env) = body_env(cells, genomes, grid, org_id) else {
            continue;
        };
        // Bound roots (Phase 4) and substrate contact hold a body still.
        if env.film || env.touching || hydrate::is_anchored(anchored, org_id) {
            continue;
        }
        let (all_water, sand_above) = (env.all_water, env.sand_above);
        let h = hash_u64(world_seed, tick, org_id as u64, 0xB2_D1);
        let mut dx = 0;
        let mut dy = 0;
        if sand_above || !all_water {
            dy = 1;
        } else if env.cilia {
            // Swimmers hold station; their cilia move them (`swim_bodies`).
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

/// Every tick: free bodies in water swim on their cilia (§a.6).
fn swim_bodies(
    cells: &mut CellStore,
    genomes: &GenomeStore,
    grid: &PhysicsGrid,
    pixels: &mut PixelLayer,
    world_seed: u64,
    tick: u64,
    anchored: &[u32],
) {
    let orgs: Vec<u32> = cells
        .iter_orgs()
        .filter(|(_, o)| o.n_alive >= 2)
        .map(|(id, _)| id)
        .collect();
    for org_id in orgs {
        let Some(env) = body_env(cells, genomes, grid, org_id) else {
            continue;
        };
        if env.film || !env.all_water || hydrate::is_anchored(anchored, org_id) {
            continue;
        }
        let h = hash_u64(world_seed, tick, org_id as u64, 0xB2_D2);
        body_thrust(cells, genomes, pixels, org_id, h);
    }
}

/// Cilia on a free body (§a.6): every running cilia cell pushes along its
/// own heading; adjacent running cilia cells beat in phase, thrust ×
/// `(1 + 0.25·n_adjacent)` capped at ×2. The net vector moves the body
/// one pixel along its dominant axis at the speed law's rate.
fn body_thrust(cells: &mut CellStore, genomes: &GenomeStore, pixels: &mut PixelLayer, org_id: u32, h: u64) {
    let Some(o) = cells.org(org_id) else {
        return;
    };
    let mut beating: Vec<(i32, i32, i32, i32, u8, u32)> = Vec::new();
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for s in o.slots.iter().filter(|s| s.cell != NONE) {
        let Some(c) = cells.get(s.cell) else {
            continue;
        };
        x0 = x0.min(c.px as i32);
        y0 = y0.min(c.py as i32);
        x1 = x1.max(c.px as i32 + c.w as i32);
        y1 = y1.max(c.py as i32 + c.h as i32);
        let mc = genomes.coeffs(c.genotype, c.mode);
        if mc.cilia > 0 && c.running {
            beating.push((c.px as i32, c.py as i32, c.w as i32, c.h as i32, c.heading, mc.cilia as u32));
        }
    }
    if beating.is_empty() {
        return;
    }
    let (mut tx, mut ty) = (0i64, 0i64);
    for (i, &(px, py, w, hh, heading, units)) in beating.iter().enumerate() {
        let n_adj = beating
            .iter()
            .enumerate()
            .filter(|&(j, &(qx, qy, qw, qh, _, _))| {
                j != i && px <= qx + qw && qx <= px + w && py <= qy + qh && qy <= py + hh
            })
            .count() as i64;
        let bonus_q2 = (4 + n_adj).min(8);
        let (dx, dy) = heading_delta(heading);
        tx += dx as i64 * units as i64 * bonus_q2;
        ty += dy as i64 * units as i64 * bonus_q2;
    }
    let (dir, mag_q2) = if tx.abs() >= ty.abs() {
        ((tx.signum() as i32, 0), tx.abs())
    } else {
        ((0, ty.signum() as i32), ty.abs())
    };
    let units = (mag_q2 / 4) as u32;
    if units == 0 || dir == (0, 0) {
        return;
    }
    let perimeter = (2 * ((x1 - x0) + (y1 - y0))).max(4) as u32;
    if h % thrust_period(units, perimeter) != 0 {
        return;
    }
    let seed = o.seed;
    if cells.move_org(pixels, org_id, dir.0, dir.1) {
        if let Some(c) = cells.get_mut(seed) {
            c.moved_dx = c.moved_dx.saturating_add(dir.0 as i8);
            c.moved_dy = c.moved_dy.saturating_add(dir.1 as i8);
        }
    }
}

// ---------------------------------------------------------------- per cell

fn metabolise(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    grid: &mut PhysicsGrid,
    pixels: &mut PixelLayer,
    coarse: &mut CoarseField,
    world_seed: u64,
    tick: u64,
    id: u32,
    t_tank: f32,
    lambda_scale_q8: u32,
    grav_x: f32,
    out: &mut BioStepResult,
) {
    let Some(cell) = cells.get(id).copied() else {
        return;
    };
    let mc = *genomes.coeffs(cell.genotype, cell.mode);
    // Phase 4: the cell reads `T_tank + T_local` — a THERMO halo speeds
    // up the warm cell and anything sitting inside it (§a.7).
    let ci = minerals::idx_of_pixel(coarse, cell.px, cell.py);
    let t_tank = t_tank + crate::thermal::local_c(coarse, ci);
    let humidity = coarse.humidity.get(ci).map_or(0, |&h| (h >> 8) as u8);
    let (metab_eff, temp_q, base_age, stress_def, wall_key, glue_key, resist_key, strategy_q8, lambda_m) = {
        let g = genomes.genome(cell.genotype);
        (
            g.metab_eff as i64,
            g.temp_eff_q8(t_tank) as i64,
            g.base_age,
            g.stress,
            g.wall_key,
            g.glue_key,
            g.resist_key,
            g.division_cost_q8(),
            g.lambda_milli(cell.stress, t_tank) * lambda_scale_q8 / 256,
        )
    };
    let area = cell.area();
    let idx = (cell.py as usize) * (pixels.w as usize) + cell.px as usize;
    let (light_sum, touch, glucose_sum) = sense(pixels, grid, cell.px, cell.py, cell.w, cell.h);
    let light_mean = (light_sum / area as u32) as u8;
    let kin = chem::kin_touch(cells, pixels, &cell);
    let sig = if mc.receptor > 0 {
        chem::receptor_read(pixels, &cell, mc.receptor_key)
    } else {
        0
    };
    let sense = if mc.sensor > 0 {
        let here_glucose = (glucose_sum / area as u32).min(255) as u8;
        sensor_read(
            cells,
            genomes,
            pixels,
            grid,
            &cell,
            &mc,
            light_mean,
            here_glucose,
            touch,
            humidity,
            t_tank,
            grav_x,
        )
    } else {
        0
    };

    // ---- feed
    // Yield × eff(T) × metab_eff; upkeep and ageing × Q10 rate ÷ metab_eff
    // (§4: all rates scale with temperature, lifespan ∝ 1/rate).
    let eff_q = metab_eff * temp_q / 255; // ≈128 = ×1
    let q10_q8 = q10_q8(t_tank);
    let mut photo_income: i64 = 0;
    if mc.photo > 0 {
        photo_income = PHOTO_YIELD_MFP * mc.photo as i64 * light_sum as i64 / (area as i64 * 255 * 1000);
    }
    // Baseline uptake drinks one unit per pixel per tick from the dissolved
    // field under the cell; UPTAKE(glucose) is 4× baseline per unit (§a.4).
    let mut glucose_income: i64 = 0;
    let store_cap = mc.store_cap_fp(area);
    if glucose_sum > 0 {
        let take = (1 + 3 * mc.uptake_glucose as u32).min(255) as u8;
        // Only what the store can hold is taken; the rest stays dissolved.
        let mut room = ((store_cap - cell.energy).max(0) / GLUCOSE_UNIT_FP) as u32;
        let pw = pixels.w as usize;
        let mut got = 0u32;
        'px: for dy in 0..cell.h as usize {
            for dx in 0..cell.w as usize {
                if room == 0 {
                    break 'px;
                }
                let i = (cell.py as usize + dy) * pw + cell.px as usize + dx;
                let t = (pixels.glucose[i].min(take) as u32).min(room) as u8;
                pixels.glucose[i] -= t;
                got += t as u32;
                room -= t as u32;
            }
        }
        glucose_income = got as i64 * GLUCOSE_UNIT_FP as i64;
    }
    // CHEMO (§a.4 #2): sediment nutrient in the sand / soil cells touching
    // the footprint → glucose, `units` nutrient units every 4th tick
    // (≈ PHOTO at half light), in the dark.
    let mut chemo_income: i64 = 0;
    if mc.chemo > 0 && (tick + id as u64) % 4 == 0 {
        let room = ((store_cap - cell.energy).max(0) / GLUCOSE_UNIT_FP) as u32;
        let got = chemo_feed(grid, coarse, &cell, (mc.chemo as u32).min(room));
        chemo_income = got as i64 * GLUCOSE_UNIT_FP as i64;
    }
    let income = (photo_income + glucose_income + chemo_income) * eff_q / 128;
    let glucose_income = glucose_income + chemo_income;
    if glucose_income > photo_income && glucose_income > 0 && (tick + id as u64) % 128 == 0 {
        out.events.push(SimEvent {
            kind: ev::HETEROTROPH,
            arg0: id,
        });
    }
    // Secreted enzyme / toxin lying on the cell (§b.3 field path).
    let exposure = chem::field_exposure(
        pixels,
        world_seed,
        tick,
        id,
        &cell,
        wall_key,
        resist_key,
        mc.birth_fp * area,
    );

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
    if energy > store_cap {
        energy = store_cap;
    }
    let mut hit = cell.hit;
    if exposure.lysed_units > 0 {
        add_plain(pixels, Field::Glucose, idx, exposure.lysed_units.min(255) as u8);
    }
    if exposure.enzyme_damage > 0 {
        hit |= HIT_ENZYME;
    }
    if exposure.toxin_damage > 0 {
        hit |= HIT_TOXIN;
    }
    let acc = cell.age_acc as u16 + act_q8 as u16;
    let age_acc = (acc & 0xFF) as u8;
    let age = cell.age.saturating_add(acc >> 8);
    let body_cost = cells.org_body_cost_fp(cell.org).max(mc.build_fp);
    let max_age = max_age_ticks(base_age, body_cost);

    let mut damage = cell
        .damage
        .saturating_add(exposure.enzyme_damage)
        .saturating_add(exposure.toxin_damage);
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
    // Desiccation on the air side (§h.1): reduced by wall hardness and by
    // the better of air humidity and the cell's own hydration.
    let (vx_here, vy_here) = pixel_to_phys(cell.px, cell.py, grid.visible_h() as u32);
    let seat_here = grid.get_visible(vx_here, vy_here);
    let on_land = !(seat_here.is_open() && seat_here.sat > 40);
    if on_land && tick % DESICCATE_EVERY == 0 {
        let wet = humidity.max(cell.hydration) as u32;
        let dry = 255u32.saturating_sub(wet);
        let soft = 255u32.saturating_sub(mc.wall_h as u32);
        let dmg = (DESICCATE_BASE as u32 * soft * dry / (255 * 255)) as u8;
        damage = damage.saturating_add(dmg);
        if cell.age < 4 {
            out.events.push(SimEvent {
                kind: ev::LIFE_ON_LAND,
                arg0: id,
            });
        }
        // A cutting that has rooted into moist sand/soil.
        if cell.depth == 0
            && mc.uptake_water >= 2
            && hydrate::is_rootable(seat_here)
            && cells.org(cell.org).is_some_and(|o| o.cut)
        {
            out.events.push(SimEvent {
                kind: ev::CUTTING_TOOK_ROOT,
                arg0: id,
            });
            if let Some(o) = cells.org_mut(cell.org) {
                o.cut = false; // fire once per cutting
            }
        }
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
        sig,
        kin,
        waste,
        temp_dev: ((t_tank - 22.0).abs() * 8.0).min(255.0) as u8,
        sense,
        humidity,
        hydration: cell.hydration,
    };

    // ---- act
    if mc.excrete > 0 && waste > 0 {
        let (dx, dy) = heading_delta(cell.heading.wrapping_add(4) % 8);
        let x = cell.px as i32 + dx;
        let y = cell.py as i32 + dy;
        if x >= 0 && y >= 0 && x < pixels.w as i32 && y < pixels.h as i32 {
            let j = (y as usize) * (pixels.w as usize) + x as usize;
            let push = mc.excrete.min(waste);
            add_plain(pixels, Field::Waste, j, push);
            waste -= push;
        }
    }
    let mut chem_out = chem::ChemOut::default();
    if mc.enzyme > 0 && mc.enzyme_gate.passes(&scalars) {
        chem::enzyme_act(
            cells, genomes, pixels, coarse, grid, world_seed, tick, id, &cell, &mc, &mut chem_out,
        );
        // Contact lysis of a dry soft-litter seat (litter band 0–63).
        if mc.enzyme_key < 64 {
            let (vx, vy) = pixel_to_phys(cell.px, cell.py, grid.visible_h() as u32);
            let seat = grid.get_visible(vx, vy);
            if seat.id() == MaterialId::Organic {
                grid.set_visible(vx, vy, Cell::air());
                add_plain(pixels, Field::Glucose, idx, LITTER_GLUCOSE);
            }
        }
    }
    if (mc.toxin > 0 || mc.needle > 0) && mc.toxin_gate.passes(&scalars) {
        chem::toxin_act(
            cells, genomes, pixels, grid, world_seed, tick, id, &cell, &mc, &mut chem_out,
        );
    }
    if mc.signal > 0 && mc.signal_gate.passes(&scalars) {
        chem::signal_act(pixels, grid, tick, &cell, &mc, &scalars, &mut chem_out);
    }
    energy -= chem_out.cost;
    // THERMO (§a.7): glucose → heat in the coarse cell, 0.04 g/s per unit,
    // while the gate holds and the cell can pay.
    let mut warm = cell.warm;
    let heating = mc.thermo > 0 && mc.thermo_gate.passes(&scalars) && energy > FP;
    if heating {
        energy -= THERMO_COST_FP_TICK_PER_UNIT * mc.thermo as i32;
        if (tick + id as u64) % THERMO_INJECT_EVERY == 0 {
            crate::thermal::inject(coarse, ci, mc.thermo);
        }
    }
    if mc.thermo > 0 {
        // `warm` ≥ 1 while heating or still in a halo (the render's warm
        // tint); it counts the ticks the halo has held ≥ +3 °C. A gated
        // heater thermostats on and off, so the count follows the halo,
        // not the burner; a dip costs four ticks rather than the run.
        let over_q2 = coarse.t_local.get(ci).copied().unwrap_or(0);
        if over_q2 >= WARM_BLOODED_Q2 {
            if warm <= WARM_BLOODED_TICKS {
                warm += 1;
                if warm == WARM_BLOODED_TICKS + 1 {
                    out.events.push(SimEvent {
                        kind: ev::WARM_BLOODED,
                        arg0: id,
                    });
                }
            }
        } else if warm <= WARM_BLOODED_TICKS {
            warm = warm.saturating_sub(4).max(u8::from(heating));
        }
    } else {
        warm = 0;
    }
    // Kin signalling: a receptor hears its own kind's signal (rising edge).
    if sig >= SIGNAL_HEARD && cell.sig < SIGNAL_HEARD && mc.signal > 0 {
        out.events.push(SimEvent {
            kind: ev::SIGNALLING,
            arg0: id,
        });
    }
    // Motor drive is shared by CILIA (thrust) and MUSCLE (spring rest).
    if mc.cilia > 0 || mc.muscle > 0 {
        let mut avg = cell.link_avg;
        let drive = motor_drive(&mc, &scalars, &mut avg) > 0;
        if drive {
            if mc.cilia > 0 {
                energy -= FP / 50 / TICK_HZ as i32 * mc.cilia as i32;
            }
            if mc.muscle > 0 {
                // Pay-per-use: ~0.05 g per full contraction / s → per tick.
                energy -= FP / 20 / TICK_HZ as i32 * mc.muscle as i32;
            }
        }
        if let Some(c) = cells.get_mut(id) {
            c.link_avg = avg;
            if mc.cilia > 0 && !drive {
                // Tumble: rotate 90° to a hash-chosen side, and keep
                // turning each tick until some heading gives a drive.
                let side = if hash_u64(world_seed, tick, id as u64, 0xB3_01) % 2 == 0 { 2 } else { 6 };
                c.heading = c.heading.wrapping_add(side) % 8;
            }
            c.running = drive;
        }
    }
    // "Something moved": net swim over the window, not drift. A body's
    // thrust is booked on its seed cell, which may itself bear no cilia.
    if (tick + id as u64) % MOVED_WINDOW == 0 {
        if let Some(c) = cells.get_mut(id) {
            if (c.moved_dx as i32).abs() + (c.moved_dy as i32).abs() >= MOVED_PX {
                out.events.push(SimEvent {
                    kind: ev::MOVED,
                    arg0: id,
                });
            }
            c.moved_dx = 0;
            c.moved_dy = 0;
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
        if cause == 2 {
            if hit & HIT_TOXIN != 0 {
                out.events.push(SimEvent {
                    kind: ev::POISON,
                    arg0: id,
                });
            } else if hit & HIT_ENZYME != 0 {
                out.events.push(SimEvent {
                    kind: ev::PREDATION,
                    arg0: id,
                });
            }
        }
        if let Some(c) = cells.get_mut(id) {
            c.energy = energy.max(0);
        }
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
        c.sig = sig;
        c.hit = 0;
        c.warm = warm;
        c.sense = sense;
    }

    // ---- develop: grow, transition, brood release, divide
    if age % T_GROW == 0 && (cell.w < mc.shape_w || cell.h < mc.shape_h) {
        let cost = mc.birth_fp * strategy_q8 as i32 / 256;
        if energy > cost + FP {
            // The step adds a column (h px) or a row (w px); hard walls pay
            // minerals per new pixel and wait when the water has none.
            let (nw, nh) = if cell.w < mc.shape_w {
                (cell.w + 1, cell.h)
            } else {
                (cell.w, cell.h + 1)
            };
            let new_px = (nw as u32 * nh as u32) - (cell.w as u32 * cell.h as u32);
            let need = mc.mineral_px as u32 * new_px;
            let ci = minerals::idx_of_pixel(coarse, cell.px, cell.py);
            if minerals::available(coarse, ci) >= need {
                if cells.blocked_by_foreign_shell(pixels, cell.org, cell.px, cell.py, nw, nh, id + OWNER_BIAS) {
                    cells.crack_shells(pixels, cell.org, cell.px, cell.py, nw, nh);
                }
                if cells.try_grow(pixels, id, mc.shape_w, mc.shape_h) {
                    minerals::take(coarse, ci, need);
                    if let Some(c) = cells.get_mut(id) {
                        c.energy -= cost;
                    }
                }
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

    if tick % 4 == (id as u64 % 4) && try_transition(cells, genomes, pixels, coarse, id, &scalars) {
        return;
    }

    if cell.brood {
        if cell.detach_gate.passes(&scalars) {
            if let Some(new_org) = cells.detach(genomes, id, tick) {
                cells.note_child(cell.org);
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
            cells, genomes, grid, pixels, coarse, world_seed, tick, id, &mc, strategy_q8,
            lambda_m, glue_key, out,
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
    // In dry air the corpse becomes an Organic seat; in water (free water
    // is conserved) its mass becomes pixel litter for the saprotrophs.
    let seat = grid.get_visible(vx, vy);
    let corpse = if seat.is_open() && seat.sat < 40 {
        grid.set_visible(vx, vy, Cell::solid(MaterialId::Organic));
        None
    } else {
        // Structure not already lysed away (damage 255 = all of it) + store.
        let mc = genomes.coeffs(cell.genotype, cell.mode);
        let structure = mc.birth_fp as i64 * cell.area() as i64 * (255 - cell.damage as i64) / 255;
        let mass = structure as i32 + cell.energy.max(0);
        Some(corpse_split(mc, cell.area(), mass))
    };
    let to_film = cell.film || age > FILM_STUCK_AGE && near_solid(grid, cell.px, cell.py);
    let fragments = cells.kill(genomes, pixels, id, to_film, tick, corpse);
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

/// Draw up to `want` sediment units from the sand / soil physics cells
/// under and beside the footprint (own cell first, then the cell below,
/// then left / right).
fn chemo_feed(grid: &PhysicsGrid, coarse: &mut CoarseField, c: &crate::bio::CellRec, want: u32) -> u32 {
    if want == 0 {
        return 0;
    }
    let vis_h = grid.visible_h() as u32;
    let (vx, vy) = pixel_to_phys(c.px, c.py + c.h as u16 - 1, vis_h);
    let mut got = 0u32;
    for (dx, dy) in [(0, 0), (0, -1), (-1, 0), (1, 0)] {
        if got >= want {
            break;
        }
        let (x, y) = (vx + dx, vy + dy);
        if x < 0 || y < 0 || x >= coarse.w as i32 || y >= coarse.h as i32 {
            continue;
        }
        let bed = grid.get_visible(x, y).id();
        if !matches!(bed, MaterialId::Sand | MaterialId::Gravel | MaterialId::Soil) {
            continue;
        }
        let ci = (y as usize) * (coarse.w as usize) + x as usize;
        let take = (coarse.sediment[ci] as u32).min(want - got);
        coarse.sediment[ci] -= take as u8;
        got += take;
    }
    got
}

/// Split a dead cell's mass (FP) into soft mass (FP) and bone litter (the
/// wall minerals, in field units). The glucose the bone band will later
/// recover from the bone comes out of the soft share so mass is conserved.
pub fn corpse_split(mc: &ModeCoeffs, area: i32, mass_fp: i32) -> (i32, u16) {
    let bone = mc.mineral_px as u32 * area as u32;
    let bone_fp = bone as i32 * minerals::BONE_GLUCOSE_PER_UNIT as i32 * GLUCOSE_UNIT_FP;
    ((mass_fp - bone_fp).max(0), bone.min(u16::MAX as u32) as u16)
}

/// First passing transition changes the mode in place (§b.4 item 5).
fn try_transition(
    cells: &mut CellStore,
    genomes: &GenomeStore,
    pixels: &mut PixelLayer,
    coarse: &mut CoarseField,
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
    // Harder walls draw minerals for the whole footprint (and wait for
    // them); softer ones give theirs back to the water.
    let m_delta = new.mineral_px as i32 - old.mineral_px as i32;
    let ci = minerals::idx_of_pixel(coarse, cell.px, cell.py);
    match m_delta.cmp(&0) {
        std::cmp::Ordering::Greater => {
            if !minerals::take(coarse, ci, m_delta as u32 * cell.area() as u32) {
                return false;
            }
        }
        std::cmp::Ordering::Less => {
            minerals::give(coarse, ci, (-m_delta) as u32 * cell.area() as u32);
        }
        std::cmp::Ordering::Equal => {}
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
    coarse: &mut CoarseField,
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
                && try_regrow(cells, genomes, pixels, coarse, parent, mc, strategy_q8, out);
        }
        (mode.children[child_idx as usize], child_idx, child_reps)
    };
    if spec.kind != ChildKind::Grow {
        return try_secrete(cells, genomes, grid, pixels, coarse, parent, spec.kind, spec.dir, child_idx, child_reps);
    }
    let immediate = matches!(spec.detach, Detach::Immediate);
    // Place the daughter adjacent to the footprint on the chosen side.
    // Habitable seats: free water, moist air, or (for roots) moist sand/soil
    // (§h.1). A propagule that finds a blocked seat tries the other sides.
    let vis_h = grid.visible_h() as u32;
    let child_preview = *genomes.coeffs(genotype, spec.mode);
    let seat_ok = |nx: i32, ny: i32| -> bool {
        hydrate::seat_ok(grid, coarse, pixels, nx, ny, &child_preview)
    };
    let (dx, dy) = spec.dir.delta(cell.heading);
    let mut nx = cell.px as i32 + side_offset(dx, cell.w);
    let mut ny = cell.py as i32 + side_offset(dy, cell.h);
    // A body part growing into another organism's shell cracks it (§d.8).
    let cracks = !immediate
        && nx >= 0
        && ny >= 0
        && cells.blocked_by_foreign_shell(pixels, cell.org, nx as u16, ny as u16, 1, 1, 0)
        && {
            let (vx, vy) = pixel_to_phys(nx as u16, ny as u16, vis_h);
            let seat = grid.get_visible(vx, vy);
            seat.is_open() && seat.sat > 40
        };
    if !cracks && !seat_ok(nx, ny) {
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
    // Hard walls pay minerals for the daughter's first pixel, or wait.
    let m_need = child_cc.mineral_px as u32;
    let ci = minerals::idx_of_pixel(coarse, nx as u16, ny as u16);
    if !minerals::take(coarse, ci, m_need) {
        return false;
    }
    if cracks {
        cells.crack_shells(pixels, cell.org, nx as u16, ny as u16, 1, 1);
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
    // Daughter heading: her mode's `heading_rule` relative to the parent's
    // heading (§b.4 item 3), whatever side she was placed on — so one side
    // mode grows both walls of a U (d.7) and flipping a rule mirrors a subtree.
    let heading = {
        let g = genomes.genome(genotype);
        g.mode(spec.mode).heading_rule.apply(cell.heading)
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
    if matches!(spec.detach, Detach::Immediate) && daughter.is_some() {
        cells.note_child(cell.org);
    }
    let Some(did) = daughter else {
        minerals::give(coarse, ci, m_need);
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

/// `Secrete(BONE | SHELL)` child entry (§b.3 dead matter): lay one dead
/// pixel on the chosen side, `1 g + 2 m` (bone) or `0.8 g + 3 m` (shell),
/// drawn from the local mineral field — the entry *waits* while the water
/// holds too little (K2#1). Repeats extend the line pixel by pixel; when
/// the line meets a cell of the same body (an egg), the pixels wrap that
/// cell instead. Nowhere left to lay → the entry is skipped.
fn try_secrete(
    cells: &mut CellStore,
    genomes: &GenomeStore,
    grid: &PhysicsGrid,
    pixels: &mut PixelLayer,
    coarse: &mut CoarseField,
    parent: u32,
    kind: ChildKind,
    dir: crate::bio::program::Dir,
    child_idx: u8,
    child_reps: u8,
) -> bool {
    let Some(cell) = cells.get(parent).copied() else {
        return false;
    };
    let vis_h = grid.visible_h() as u32;
    let pw = pixels.w as i32;
    let ph = pixels.h as i32;
    let water = |x: i32, y: i32| -> bool {
        let (vx, vy) = pixel_to_phys(x as u16, y as u16, vis_h);
        let seat = grid.get_visible(vx, vy);
        seat.is_open() && seat.sat > 40
    };
    let free = |pixels: &PixelLayer, x: i32, y: i32| -> bool {
        x >= 0
            && y >= 0
            && x < pw
            && y < ph
            && pixels.owner[(y * pw + x) as usize] == EMPTY_OWNER
            && water(x, y)
    };
    let (dx, dy) = dir.delta(cell.heading);
    let mut x = cell.px as i32 + side_offset(dx, cell.w);
    let mut y = cell.py as i32 + side_offset(dy, cell.h);
    let mut seat: Option<(i32, i32)> = None;
    for _ in 0..8 {
        if x < 0 || y < 0 || x >= pw || y >= ph {
            break;
        }
        let owner = pixels.owner[(y * pw + x) as usize];
        if owner == EMPTY_OWNER {
            if water(x, y) {
                seat = Some((x, y));
            }
            break;
        }
        if secretion_kind(owner).is_some() {
            if cells.secretion_org_at(x as u16, y as u16) != Some(cell.org) {
                break;
            }
            x += dx;
            y += dy;
            continue;
        }
        // A living cell: wrap it if it is ours.
        let Some(t) = cells.get(owner - OWNER_BIAS) else {
            break;
        };
        if t.org != cell.org {
            break;
        }
        let (x0, y0) = (t.px as i32, t.py as i32);
        let (x1, y1) = (x0 + t.w as i32 - 1, y0 + t.h as i32 - 1);
        let mut ring: Vec<(i32, i32)> = Vec::with_capacity(16);
        for xx in x0..=x1 {
            ring.push((xx, y0 - 1));
            ring.push((xx, y1 + 1));
        }
        for yy in y0..=y1 {
            ring.push((x0 - 1, yy));
            ring.push((x1 + 1, yy));
        }
        seat = ring.into_iter().find(|&(rx, ry)| free(pixels, rx, ry));
        break;
    }
    let advance = |cells: &mut CellStore, consumed: bool| {
        if let Some(c) = cells.get_mut(parent) {
            let left = if consumed { child_reps.saturating_sub(1) } else { 0 };
            if left == 0 {
                c.child_idx = child_idx.saturating_add(1);
                c.child_reps = genomes
                    .genome(cell.genotype)
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
    };
    let Some((sx, sy)) = seat else {
        advance(cells, false);
        return false;
    };
    let (cost, m_units) = match kind {
        ChildKind::SecreteShell => (FP * 4 / 5, minerals::SHELL_M_UNITS as u32),
        _ => (FP, minerals::BONE_M_UNITS as u32),
    };
    if cell.energy - cost < FP {
        return false;
    }
    let ci = minerals::idx_of_pixel(coarse, sx as u16, sy as u16);
    if !minerals::take(coarse, ci, m_units) {
        return false;
    }
    if !cells.add_secretion(pixels, cell.org, sx as u16, sy as u16, kind) {
        minerals::give(coarse, ci, m_units);
        advance(cells, false);
        return false;
    }
    if let Some(c) = cells.get_mut(parent) {
        c.energy -= cost;
        c.flash = 2;
    }
    advance(cells, true);
    true
}

/// Regrow the first dead child slot at full build cost.
fn try_regrow(
    cells: &mut CellStore,
    genomes: &mut GenomeStore,
    pixels: &mut PixelLayer,
    coarse: &mut CoarseField,
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
    let ci = minerals::idx_of_pixel(coarse, cell.px, cell.py);
    if !minerals::take(coarse, ci, cc.mineral_px as u32) {
        return false;
    }
    let Some(did) = cells.regrow_slot(genomes, pixels, org, slot, BIRTH_ENERGY_FP / 2) else {
        minerals::give(coarse, ci, cc.mineral_px as u32);
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
        let Some(v) = cells.get(vid).copied() else {
            continue;
        };
        let vmc = genomes.coeffs(v.genotype, v.mode);
        let refund = vmc.birth_fp / 2;
        // The other half of the structure, and her store, fall as litter.
        let mass = vmc.birth_fp * v.area() - refund + v.energy.max(0);
        let corpse = corpse_split(vmc, v.area(), mass);
        let _ = cells.kill(genomes, pixels, vid, false, tick, Some(corpse));
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
