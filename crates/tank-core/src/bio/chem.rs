//! Contact-first chemistry (§a.4, §b.3): ENZYME and TOXIN acting on what a
//! cell touches, NEEDLE injection along the heading, the secreted
//! short-range variants, SIGNAL emission and RECEPTOR reading, and the
//! soft-litter substrate the litter-band enzymes digest.
//!
//! Every effect is keyed on the ring (`ring_match_q8`): enzyme key against
//! the target's substrate code (litter class, living `wall_key`, bone),
//! toxin key against the target's `resist_key`, signal key against the
//! receptor key. Fractional per-tick amounts are rounded with a hash so
//! the arithmetic stays integer and deterministic.
#![allow(clippy::too_many_arguments)]

use crate::bio::fields::{
    add_grit, add_litter, add_plain, credit_digestion, deposit, open_idx, tile_of, Field, KeyedField,
    BONE_GRIT_PER_UNIT, FIELD_WIDTH, GRIT_SHARE_Q8,
};
use crate::bio::genome::GenomeStore;
use crate::bio::profile::ModeCoeffs;
use crate::bio::program::{heading_delta, keyed_potency_q8, ring_match_q8, Scalars};
use crate::bio::minerals::{self, BONE_GLUCOSE_PER_UNIT};
use crate::bio::store::{secretion_kind, secretion_units, CellRec, CellStore, OWNER_BIAS};
use crate::consts::{FP, GLUCOSE_UNIT_FP, SHAPE_CAP, TICK_HZ};
use crate::grid::PhysicsGrid;
use crate::hash::hash_u64;
use crate::pixels::{CoarseField, PixelLayer};

/// Substrate codes (§b.3 key table): soft litter sits in the litter band
/// 0–63, bone and shell in 192–255.
pub const LITTER_SOFT_CODE: u8 = 32;
pub const LITTER_BONE_CODE: u8 = 220;
/// Accumulated Q8 contact damage that breaks one bone / shell pixel
/// (hardness 1: 16× a bare cell pixel). Applied statelessly as a per-tick
/// break chance of `potency / BONE_BREAK_Q8`.
pub const BONE_BREAK_Q8: u32 = 4096;

/// `CellRec::hit` flags: what damaged the cell since it last ran.
pub const HIT_ENZYME: u8 = 1;
pub const HIT_TOXIN: u8 = 2;

/// Match width for toxin vs `resist_key` and signal vs receptor key.
pub const RESIST_WIDTH: u8 = 32;
pub const RECEPTOR_WIDTH: u8 = 24;

/// Signal amplitude read by a RECEPTOR per unit of matching field.
const RECEPTOR_GAIN: u32 = 8;

const MAX_RING: usize = 4 * SHAPE_CAP as usize + 4;

/// What one cell's chemistry did this tick.
#[derive(Default, Clone, Copy)]
pub struct ChemOut {
    /// Glucose spent on secretion (FP).
    pub cost: i32,
    /// Dealt contact damage to a living cell of another organism.
    pub attacked: bool,
}

/// Exposure of a cell to the fields under its own pixels.
#[derive(Default, Clone, Copy)]
pub struct Exposure {
    pub enzyme_damage: u8,
    pub toxin_damage: u8,
    /// Glucose units of lysed structure to release at the cell.
    pub lysed_units: u32,
}

#[inline]
fn frac_round(v_q8: u32, h: u64) -> u32 {
    (v_q8 >> 8) + ((h & 0xFF) < (v_q8 & 0xFF) as u64) as u32
}

/// The 4-adjacent perimeter ring of a footprint as `(ring idx, nearest own idx)`.
fn ring(pixels: &PixelLayer, c: &CellRec) -> ([(usize, usize); MAX_RING], usize) {
    let pw = pixels.w as i32;
    let ph = pixels.h as i32;
    let (x0, y0) = (c.px as i32, c.py as i32);
    let (x1, y1) = (x0 + c.w as i32 - 1, y0 + c.h as i32 - 1);
    let mut out = [(0usize, 0usize); MAX_RING];
    let mut n = 0usize;
    let mut visit = |x: i32, y: i32| {
        if x < 0 || y < 0 || x >= pw || y >= ph || n >= MAX_RING {
            return;
        }
        let (ox, oy) = (x.clamp(x0, x1), y.clamp(y0, y1));
        out[n] = ((y * pw + x) as usize, (oy * pw + ox) as usize);
        n += 1;
    };
    for x in x0..=x1 {
        visit(x, y0 - 1);
        visit(x, y1 + 1);
    }
    for y in y0..=y1 {
        visit(x0 - 1, y);
        visit(x1 + 1, y);
    }
    (out, n)
}

/// Pixel just beyond the footprint on the heading side.
fn ahead_idx(pixels: &PixelLayer, c: &CellRec) -> Option<usize> {
    let (dx, dy) = heading_delta(c.heading);
    let x = match dx.cmp(&0) {
        std::cmp::Ordering::Greater => c.px as i32 + c.w as i32,
        std::cmp::Ordering::Less => c.px as i32 - 1,
        std::cmp::Ordering::Equal => c.px as i32 + (c.w as i32 - 1) / 2,
    };
    let y = match dy.cmp(&0) {
        std::cmp::Ordering::Greater => c.py as i32 + c.h as i32,
        std::cmp::Ordering::Less => c.py as i32 - 1,
        std::cmp::Ordering::Equal => c.py as i32 + (c.h as i32 - 1) / 2,
    };
    if x < 0 || y < 0 || x >= pixels.w as i32 || y >= pixels.h as i32 {
        return None;
    }
    Some((y as usize) * (pixels.w as usize) + x as usize)
}

/// Touching cells of the same genotype in other organisms, ×64 (the `kin`
/// scalar), over the four side midpoints `sense()` uses for `touch`.
pub fn kin_touch(cells: &CellStore, pixels: &PixelLayer, c: &CellRec) -> u8 {
    let pw = pixels.w as i32;
    let ph = pixels.h as i32;
    let mut kin = 0u8;
    for (dx, dy) in [(-1i32, 0i32), (c.w as i32, 0), (0, -1), (0, c.h as i32)] {
        let x = c.px as i32 + dx;
        let y = c.py as i32 + dy;
        if x < 0 || y < 0 || x >= pw || y >= ph {
            continue;
        }
        let owner = pixels.owner[(y * pw + x) as usize];
        if owner < OWNER_BIAS {
            continue;
        }
        if let Some(o) = cells.get(owner - OWNER_BIAS) {
            if o.org != c.org && o.genotype == c.genotype {
                kin = kin.saturating_add(64);
            }
        }
    }
    kin
}

/// RECEPTOR: the strongest matching signal under the cell's own pixels.
pub fn receptor_read(pixels: &PixelLayer, c: &CellRec, receptor_key: u8) -> u8 {
    let pw = pixels.w as usize;
    let mut best = 0u32;
    for dy in 0..c.h as usize {
        for dx in 0..c.w as usize {
            let i = (c.py as usize + dy) * pw + c.px as usize + dx;
            let a = pixels.signal_amt[i] as u32;
            if a == 0 {
                continue;
            }
            let m = ring_match_q8(pixels.signal_key[i], receptor_key, RECEPTOR_WIDTH) as u32;
            best = best.max(a * m / 255 * RECEPTOR_GAIN);
        }
    }
    best.min(255) as u8
}

/// Damage and drain from the enzyme / toxin fields lying on the cell's own
/// pixels (the secreted path, §b.3: `amount/255` of contact strength).
pub fn field_exposure(
    pixels: &PixelLayer,
    world_seed: u64,
    tick: u64,
    id: u32,
    c: &CellRec,
    wall_key: u8,
    resist_key: u8,
    mass_fp: i32,
) -> Exposure {
    let mut ex = Exposure::default();
    if !pixels.fields_active {
        return ex;
    }
    let pw = pixels.w as usize;
    let first = (c.py as usize) * pw + c.px as usize;
    let last = first + (c.h as usize - 1) * pw + c.w as usize - 1;
    let want = Field::Enzyme.bit() | Field::Toxin.bit();
    if pixels.tile_active[tile_of(pixels, first)] & want == 0
        && pixels.tile_active[tile_of(pixels, last)] & want == 0
    {
        return ex;
    }
    let mut enz_q8 = 0u32;
    let mut tox_q8 = 0u32;
    for dy in 0..c.h as usize {
        for dx in 0..c.w as usize {
            let i = first + dy * pw + dx;
            let e = pixels.enzyme_amt[i] as u32;
            if e > 0 {
                let m = ring_match_q8(pixels.enzyme_key[i], wall_key, FIELD_WIDTH) as u32;
                enz_q8 += e * m / 64;
            }
            let tx = pixels.toxin_amt[i] as u32;
            if tx > 0 {
                let m = ring_match_q8(pixels.toxin_key[i], resist_key, RESIST_WIDTH) as u32;
                tox_q8 += tx * (255 - m) / 255 * 4;
            }
        }
    }
    if enz_q8 > 0 {
        let h = hash_u64(world_seed, tick, id as u64, 0xC4_11);
        ex.enzyme_damage = frac_round(enz_q8, h).min(255) as u8;
        ex.lysed_units = frac_round(enz_q8 * lysis_yield_q8(mass_fp) / 256, h >> 8);
    }
    if tox_q8 > 0 {
        let h = hash_u64(world_seed, tick, id as u64, 0xC4_12);
        ex.toxin_damage = frac_round(tox_q8, h).min(255) as u8;
    }
    ex
}

/// Spread `total` of a keyed field over the open, unowned ring pixels.
fn secrete_ring(pixels: &mut PixelLayer, grid: &PhysicsGrid, c: &CellRec, which: KeyedField, total: u32, key: u8) {
    if total == 0 {
        return;
    }
    let (r, n) = ring(pixels, c);
    let mut open = [0usize; MAX_RING];
    let mut m = 0usize;
    for &(j, _) in r.iter().take(n) {
        if pixels.owner[j] == 0 && open_idx(grid, pixels, j) {
            open[m] = j;
            m += 1;
        }
    }
    if m == 0 {
        return;
    }
    let per = (total / m as u32).clamp(1, 255) as u8;
    for &j in open.iter().take(m) {
        deposit(pixels, which, j, per, key);
    }
}

/// Contact lysis potency (255 per unit at full match) of `mc`'s enzyme on
/// a living target: walls shield (`1 − h`), TRANSPARENT tissue is 25 % softer.
fn living_potency(mc: &ModeCoeffs, target_wall_key: u8, tmc: &ModeCoeffs) -> u32 {
    let mut pot = keyed_potency_q8(mc.enzyme_key, target_wall_key, mc.enzyme_width) as u32 * mc.enzyme as u32;
    pot = pot * (255 - tmc.wall_h as u32) / 255;
    if tmc.transparent > 0 {
        pot = pot * 5 / 4;
    }
    pot
}

/// Glucose units released per damage point of a cell of `mass_fp`
/// structure (Q8): damage 255 is the whole cell lysed.
#[inline]
pub fn lysis_yield_q8(mass_fp: i32) -> u32 {
    (mass_fp.max(0) as u32 * 256) / (255 * GLUCOSE_UNIT_FP as u32)
}

/// Apply `pot` (255 = one unit at full match) of enzyme to living cell
/// `tid`: 1.25 damage per unit-tick, and the lysed structure appears as
/// glucose at `own` (the attacker's contact pixel). The store is not
/// touched — it falls with the corpse.
fn hit_living(
    cells: &mut CellStore,
    pixels: &mut PixelLayer,
    tid: u32,
    own: usize,
    pot: u32,
    mass_fp: i32,
    h: u64,
) -> bool {
    if pot == 0 {
        return false;
    }
    let dmg_q8 = pot * 5 / 4;
    let dmg = frac_round(dmg_q8, h).min(255) as u8;
    let units = frac_round(dmg_q8 * lysis_yield_q8(mass_fp) / 256, h >> 8);
    let Some(t) = cells.get_mut(tid) else {
        return false;
    };
    t.damage = t.damage.saturating_add(dmg);
    t.hit |= HIT_ENZYME;
    if units > 0 {
        add_plain(pixels, Field::Glucose, own, units.min(255) as u8);
    }
    true
}

/// Digest litter at `j` with `pot`; glucose appears at `own`. Bone litter
/// (code ≥ 192) returns its minerals to the physics cell under `j` and a
/// little glucose from the organic matrix.
fn lyse_litter_px(
    pixels: &mut PixelLayer,
    grid: &PhysicsGrid,
    coarse: &mut CoarseField,
    j: usize,
    own: usize,
    pot: u32,
    h: u64,
) {
    let l = pixels.litter[j];
    if l == 0 || pot == 0 {
        return;
    }
    let room = 255 - pixels.glucose[own] as u32;
    let bone = pixels.litter_key[j] >= 192;
    let mut take = frac_round(pot * 2, h).min(l as u32);
    if bone {
        take = take.min(room / BONE_GLUCOSE_PER_UNIT as u32);
    } else {
        take = take.min(room);
    }
    if take == 0 {
        return;
    }
    pixels.litter[j] -= take as u8;
    if bone {
        let pw = pixels.w as usize;
        let ci = minerals::idx_of_pixel(coarse, (j % pw) as u16, (j / pw) as u16);
        minerals::give(coarse, ci, take);
        add_plain(pixels, Field::Glucose, own, (take * BONE_GLUCOSE_PER_UNIT as u32).min(255) as u8);
        add_grit(pixels, j, (take * BONE_GRIT_PER_UNIT as u32).min(255) as u8);
    } else {
        // 15 % of soft litter is left as inert grit (§d.4); the digestion
        // counts toward fungus-made soil under the litter.
        let grit = frac_round(take * GRIT_SHARE_Q8, h >> 24);
        add_plain(pixels, Field::Glucose, own, (take - grit) as u8);
        add_grit(pixels, j, grit as u8);
        credit_digestion(pixels, grid, coarse, j, take);
    }
}

/// Contact enzyme on a corpse (soft-litter band): its soft mass is eaten
/// like litter — glucose at `own`, a grit share, digestion credited to
/// the bed under the corpse pixel. The corpse crumbles when it is empty.
fn eat_corpse(
    cells: &mut CellStore,
    pixels: &mut PixelLayer,
    grid: &PhysicsGrid,
    coarse: &mut CoarseField,
    tid: u32,
    j: usize,
    own: usize,
    pot: u32,
    h: u64,
) {
    if pot == 0 {
        return;
    }
    let room = 255 - pixels.glucose[own] as u32;
    let want = frac_round(pot * 2, h).min(room);
    let got = cells.corpse_eat(tid, want);
    if got == 0 {
        return;
    }
    let grit = frac_round(got * GRIT_SHARE_Q8, h >> 24);
    add_plain(pixels, Field::Glucose, own, (got - grit) as u8);
    add_grit(pixels, j, grit as u8);
    credit_digestion(pixels, grid, coarse, j, got);
}

/// Contact enzyme on a dead bone / shell pixel: a stateless break chance
/// per tick; the broken pixel falls as bone litter where it stood.
fn hit_secretion(cells: &mut CellStore, pixels: &mut PixelLayer, j: usize, pot: u32, h: u64) {
    if pot == 0 || (h % BONE_BREAK_Q8 as u64) >= pot as u64 {
        return;
    }
    let pw = pixels.w as usize;
    let (x, y) = ((j % pw) as u16, (j / pw) as u16);
    if let Some(kind) = cells.remove_secretion_at(pixels, x, y) {
        add_litter(pixels, j, secretion_units(kind), LITTER_BONE_CODE);
    }
}

fn litter_potency(mc: &ModeCoeffs, code: u8) -> u32 {
    keyed_potency_q8(mc.enzyme_key, code, mc.enzyme_width) as u32 * mc.enzyme as u32
}

/// ENZYME (§a.4): contact lysis of everything on the perimeter ring and of
/// litter under the cell; optional secretion of `8·units` into the ring.
pub fn enzyme_act(
    cells: &mut CellStore,
    genomes: &GenomeStore,
    pixels: &mut PixelLayer,
    coarse: &mut CoarseField,
    grid: &PhysicsGrid,
    world_seed: u64,
    tick: u64,
    id: u32,
    c: &CellRec,
    mc: &ModeCoeffs,
    out: &mut ChemOut,
) {
    let (r, n) = ring(pixels, c);
    for (k, &(j, own)) in r.iter().take(n).enumerate() {
        let h = hash_u64(world_seed, tick, id as u64 ^ ((k as u64) << 40), 0xC4_01);
        let owner = pixels.owner[j];
        if secretion_kind(owner).is_some() {
            hit_secretion(cells, pixels, j, litter_potency(mc, LITTER_BONE_CODE), h);
        } else if owner >= OWNER_BIAS {
            let tid = owner - OWNER_BIAS;
            let Some(t) = cells.get(tid).copied() else {
                if cells.corpse(tid).is_some() {
                    eat_corpse(cells, pixels, grid, coarse, tid, j, own, litter_potency(mc, LITTER_SOFT_CODE), h);
                }
                continue;
            };
            if t.org == c.org {
                continue;
            }
            let tmc = genomes.coeffs(t.genotype, t.mode);
            let pot = living_potency(mc, genomes.genome(t.genotype).wall_key, tmc);
            let mass = tmc.birth_fp * t.area();
            if hit_living(cells, pixels, tid, own, pot, mass, h) {
                out.attacked = true;
            }
        }
        if pixels.litter[j] > 0 {
            let pot = litter_potency(mc, pixels.litter_key[j]);
            lyse_litter_px(pixels, grid, coarse, j, own, pot, h >> 16);
        }
    }
    // Sitting on litter is contact too.
    let pw = pixels.w as usize;
    for dy in 0..c.h as usize {
        for dx in 0..c.w as usize {
            let i = (c.py as usize + dy) * pw + c.px as usize + dx;
            if pixels.litter[i] > 0 {
                let pot = litter_potency(mc, pixels.litter_key[i]);
                let h = hash_u64(world_seed, tick, id as u64 ^ ((i as u64) << 32), 0xC4_02);
                lyse_litter_px(pixels, grid, coarse, i, i, pot, h);
            }
        }
    }
    if mc.enzyme_secrete {
        secrete_ring(pixels, grid, c, KeyedField::Enzyme, 8 * mc.enzyme as u32, mc.enzyme_key);
        out.cost += secretion_cost(mc.enzyme, tick);
    }
}

/// Secretion costs 0.002 g per unit per second on top of upkeep (§a.4),
/// charged once a second.
#[inline]
fn secretion_cost(units: u8, tick: u64) -> i32 {
    if tick % TICK_HZ == 0 {
        FP / 500 * units as i32
    } else {
        0
    }
}

/// TOXIN (§a.4): with a NEEDLE it is injected on contact along the heading
/// (and so is this cell's ENZYME, bypassing the wall); without one it is
/// secreted into the toxin field. Damage × (1 − resist match).
pub fn toxin_act(
    cells: &mut CellStore,
    genomes: &GenomeStore,
    pixels: &mut PixelLayer,
    grid: &PhysicsGrid,
    world_seed: u64,
    tick: u64,
    id: u32,
    c: &CellRec,
    mc: &ModeCoeffs,
    out: &mut ChemOut,
) {
    if mc.needle == 0 {
        if mc.toxin > 0 {
            secrete_ring(pixels, grid, c, KeyedField::Toxin, 8 * mc.toxin as u32, mc.toxin_key);
            out.cost += secretion_cost(mc.toxin, tick);
        }
        return;
    }
    let Some(j) = ahead_idx(pixels, c) else {
        return;
    };
    let owner = pixels.owner[j];
    if owner < OWNER_BIAS {
        return;
    }
    let tid = owner - OWNER_BIAS;
    let Some(t) = cells.get(tid).copied() else {
        return;
    };
    if t.org == c.org {
        return;
    }
    let tmc = *genomes.coeffs(t.genotype, t.mode);
    if tmc.wall_h >= mc.needle_s {
        return;
    }
    let tg = genomes.genome(t.genotype);
    let h = hash_u64(world_seed, tick, id as u64, 0xC4_03);
    if mc.toxin > 0 {
        let m = ring_match_q8(mc.toxin_key, tg.resist_key, RESIST_WIDTH) as u32;
        let q8 = mc.toxin as u32 * 256 * 3 * (255 - m) / 255;
        let dmg = frac_round(q8, h).min(255) as u8;
        if dmg > 0 {
            if let Some(tc) = cells.get_mut(tid) {
                tc.damage = tc.damage.saturating_add(dmg);
                tc.hit |= HIT_TOXIN;
            }
            out.attacked = true;
        }
    }
    if mc.enzyme > 0 {
        // Injected enzyme ignores the wall and works at double strength.
        let pot = keyed_potency_q8(mc.enzyme_key, tg.wall_key, mc.enzyme_width) as u32 * mc.enzyme as u32 * 2;
        let (x, y) = (j % pixels.w as usize, j / pixels.w as usize);
        let own = (y.clamp(c.py as usize, c.py as usize + c.h as usize - 1)) * pixels.w as usize
            + x.clamp(c.px as usize, c.px as usize + c.w as usize - 1);
        if hit_living(cells, pixels, tid, own, pot, tmc.birth_fp * t.area(), h >> 16) {
            out.attacked = true;
        }
    }
}

/// SIGNAL (§a.4): source scalar → keyed amplitude `8·units·s/255` into the ring.
pub fn signal_act(
    pixels: &mut PixelLayer,
    grid: &PhysicsGrid,
    tick: u64,
    c: &CellRec,
    mc: &ModeCoeffs,
    scalars: &Scalars,
    out: &mut ChemOut,
) {
    let s = scalars.read(mc.signal_source) as u32;
    let amp = 8 * mc.signal as u32 * s / 255;
    if amp == 0 {
        return;
    }
    secrete_ring(pixels, grid, c, KeyedField::Signal, amp, mc.signal_key);
    out.cost += secretion_cost(mc.signal, tick) / 2;
}

/// A dead cell's soft mass becomes soft litter on its footprint, spilling
/// onto the unowned ring when a pixel is full; the minerals of its walls
/// become bone litter the same way (§a.3 detritus classes).
pub fn deposit_corpse(pixels: &mut PixelLayer, c: &CellRec, soft_units: u32, bone_units: u32) {
    deposit_litter(pixels, c, soft_units, LITTER_SOFT_CODE);
    deposit_litter(pixels, c, bone_units, LITTER_BONE_CODE);
}

fn deposit_litter(pixels: &mut PixelLayer, c: &CellRec, mut units: u32, code: u8) {
    if units == 0 {
        return;
    }
    let pw = pixels.w as usize;
    let put = |pixels: &mut PixelLayer, i: usize, units: &mut u32| {
        if *units == 0 {
            return;
        }
        let cur = pixels.litter[i];
        let room = if cur == 0 || pixels.litter_key[i] == code {
            255 - cur as u32
        } else {
            0
        };
        let add = room.min(*units);
        if add > 0 {
            add_litter(pixels, i, add as u8, code);
            *units -= add;
        }
    };
    for dy in 0..c.h as usize {
        for dx in 0..c.w as usize {
            put(pixels, (c.py as usize + dy) * pw + c.px as usize + dx, &mut units);
        }
    }
    if units == 0 {
        return;
    }
    let (r, n) = ring(pixels, c);
    for &(j, _) in r.iter().take(n) {
        if pixels.owner[j] == 0 {
            put(pixels, j, &mut units);
        }
    }
}
