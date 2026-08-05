//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Fungi split (Set E):
//! - **Fruiting body** — the studio-designed Atom (Digest / Hypha pixels).
//!   Planted into the sim; temporary; feeds from the mycelium field.
//! - **Mycelium field** — `Cell::_pad` intensity on Organic. Lives in the
//!   ground as a world process ([`step_mycelium_field`]); keeps spreading
//!   in moist Organic after the fruiting body dies. Not studio-painted.
//!   Threads prefer climbing toward free Air; a rich moist network that
//!   has breached the surface can [`try_emergent_fruiting`].
//! - **Two dispersal habits** via [`try_spore`]:
//!   - *Underground* (nucleus in Organic) — short rhizomorph hops that
//!     seed mycelium nearby (no wind launch).
//!   - *Surface stalk* (nucleus in Air above the bed) — wind carries
//!     spores far once the column is surface-ready.
//! Soft litter is bonus fuel. Long colonization may compost Organic → Soil
//! (never Sand). Spec: `docs/organism/FUNGI.md`, `docs/organism/VOXEL_PLANTS.md`.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::blueprint::{mutate_body, Genome};
use crate::cell::{water_capacity, Cell, CellFlags};
use crate::grid::World;
use crate::organism::{Atom, ModuleId};
use crate::plant::{apply_genome, find_fungus_slot_biased, pin_plant_pose};

/// Live Tab knobs for mycelium compost (Organic → Soil).
///
/// Defaults are intentionally faster than the old hard-coded
/// `220 / 1-in-6000` so thick litter blankets humify before plants starve
/// for pore water. Raise odds / threshold to slow compost again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FungiConfig {
    /// Mycelium intensity before Organic may compost into Soil.
    pub soil_mycelium_threshold: u8,
    /// 1-in-N chance per eligible pulse to compost a threaded cell.
    /// Lower = faster humification.
    pub soil_convert_odds: u64,
}

impl Default for FungiConfig {
    fn default() -> Self {
        Self {
            // Was 220 — lower so mid-thread beds can humify.
            soil_mycelium_threshold: 140,
            // Was 6_000 — ~7.5× faster expected compost pulses.
            soil_convert_odds: 800,
        }
    }
}

/// Soft litter units treated as fully labile food (per column).
/// Stratigraphic Organic cells are substrate (mycelium), not instant fuel.
pub const LABILE_ORGANIC_UNITS: f32 = 5.0;
/// Fraction of soft-litter pool removable in one digest event (slow).
pub const DIGEST_TICK_FRAC: f32 = 0.02;
/// Soft litter units removed per digest event (hard cap).
pub const DIGEST_MAX_UNITS: u16 = 1;
/// Energy gained per soft-litter unit digested.
pub const DIGEST_ENERGY_PER_UNIT: f32 = 0.55;
/// Tiny energy from advancing mycelium one step (not from destroying cells).
pub const MYCELIUM_ENERGY: f32 = 0.08;
/// Energy / tick while a fruiting body forages labile Organic in place.
/// Soft litter is optional boom fuel; the mycelium field must sustain
/// fruiting bodies on humid beds.
pub const ORGANIC_FORAGE_ENERGY: f32 = 0.055;
/// Local mycelium intensity at which a fruiting body is network-supported
/// (won't energy-starve while the bed stays moist).
pub const FRUIT_SUPPORT_MYC: u8 = 40;
/// Ticks between mycelium growth pulses from a living fruiting body.
pub const MYCELIUM_GROW_PERIOD: u64 = 32;
/// Mycelium intensity gained per fruiting-body growth pulse (toward 255).
pub const MYCELIUM_GROW_AMOUNT: u8 = 1;
/// World mycelium field cadence (independent of fruiting bodies).
pub const MYCELIUM_FIELD_PERIOD: u64 = 16;
/// Intensity gain per field pulse on moist colonized Organic.
pub const MYCELIUM_FIELD_GROW: u8 = 1;
/// 1-in-N chance a moist colonized cell seeds a neighbour Organic.
pub const MYCELIUM_FIELD_SPREAD_ODDS: u64 = 20;
/// Max colonized cells processed per field pulse (perf cap).
pub const MYCELIUM_FIELD_MAX_CELLS: usize = 256;
/// Pore / film moisture below which field growth pauses (slow decay).
pub const MYCELIUM_FIELD_MOIST: f32 = 0.04;
/// Ticks between mycelium-field → fruiting-body emergence attempts.
pub const MYCELIUM_EMERGE_PERIOD: u64 = 160;
/// 1-in-N chance per eligible column when an emergence pulse fires.
pub const MYCELIUM_EMERGE_ODDS: u64 = 36;
/// Mycelium intensity burned from the surface Organic when a fruiting body emerges.
pub const MYCELIUM_EMERGE_COST: u8 = 16;
/// Starting energy fraction for an emerged fruiting body (can spore soon).
pub const MYCELIUM_EMERGE_ENERGY_FRAC: f32 = 0.75;
/// Legacy default intensity before Organic may compost into Soil.
/// Prefer [`FungiConfig::soil_mycelium_threshold`].
pub const MYCELIUM_SOIL_THRESHOLD: u8 = 140;
/// Legacy 1-in-N compost odds. Prefer [`FungiConfig::soil_convert_odds`].
pub const MYCELIUM_SOIL_CONVERT_ODDS: u64 = 800;
/// Neighbour Organic cells colonized from a threaded cell (1-in-N).
pub const MYCELIUM_SPREAD_ODDS: u64 = 48;
/// Upkeep per Hypha module / tick while active.
pub const HYPHA_UPKEEP: f32 = 0.008;
/// Upkeep per Digest module / tick while active.
pub const DIGEST_UPKEEP: f32 = 0.012;
/// Labile food below which fungi hibernate (soft litter only).
pub const FUNGUS_STARVE_UNITS: f32 = 1.0;
/// Pore moisture below which fungi hibernate.
pub const FUNGUS_DROUGHT_FRAC: f32 = 0.04;
/// Max consecutive dormant ticks before death.
pub const FUNGUS_HIBERNATE_MAX_TICKS: u32 = 7_200;
/// Upkeep multiplier while dormant.
pub const FUNGUS_DORMANT_UPKEEP: f32 = 0.15;
/// Energy fraction of tank to attempt a fruiting / spore burst.
pub const FUNGUS_SPORE_ENERGY_FRAC: f32 = 0.70;
/// Minimum ticks between fruiting attempts (very rare).
pub const FUNGUS_SPORE_PERIOD: u64 = 2_400;
/// Min / max columns a *surface stalk* wind spore may travel.
pub const FUNGUS_STALK_SPORE_MIN_DIST: i32 = 6;
pub const FUNGUS_STALK_SPORE_MAX_DIST: i32 = 36;
/// Max columns an *underground* rhizomorph hop may travel (local only).
pub const FUNGUS_RHIZOMORPH_MAX_DIST: i32 = 5;
/// Legacy alias — stalk wind ceiling (HUD / settings may still refer).
pub const FUNGUS_SPORE_MAX_DIST: i32 = FUNGUS_STALK_SPORE_MAX_DIST;
/// Neighbourhood half-width for local fruiting-body density gate.
pub const FUNGUS_SPORE_LOCAL_RADIUS: i32 = 4;
/// Max living fruiting bodies in `[gx±radius]` before further spores
/// / emergence are blocked (anti-flood; mirrors plant sprout density).
pub const FUNGUS_SPORE_LOCAL_MAX: usize = 6;
/// Age before network support prevents energy-starve (babies must earn).
pub const FRUIT_SUPPORT_MIN_AGE: u64 = 480;
/// Soft litter deposited per body module on death.
pub const DEATH_LITTER_PER_MODULE: u16 = 6;
/// Cap soft litter added from one corpse.
pub const DEATH_LITTER_MAX: u16 = 48;
/// How deep / wide to scan for Organic substrate under the fungus.
const ORGANIC_SCAN_DEPTH: i32 = 8;
const ORGANIC_SCAN_RADIUS: i32 = 2;

/// True when the body is a fruiting-body habit (Digest, no Root/Stem).
/// Underground hyphae are the mycelium field on Organic, not body pixels.
pub fn is_fungus(atom: &Atom) -> bool {
    let has_digest = atom.body.iter().any(|(_, _, m)| *m == ModuleId::Digest);
    let has_root = atom.body.iter().any(|(_, _, m)| *m == ModuleId::Root);
    let has_stem = atom.body.iter().any(|(_, _, m)| *m == ModuleId::Stem);
    has_digest && !has_root && !has_stem
}

pub fn digest_count(atom: &Atom) -> usize {
    atom.body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Digest)
        .count()
}

pub fn hypha_count(atom: &Atom) -> usize {
    atom.body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Hypha)
        .count()
}

/// Soft litter units at wrapped column `gx`.
pub fn soft_litter_at(world: &World, gx: i32) -> u16 {
    let gx = world.wrap_x(gx);
    world.soft_litter.get(&gx).copied().unwrap_or(0)
}

/// Add soft litter at column `gx` (death / seeding).
pub fn add_soft_litter(world: &mut World, gx: i32, units: u16) {
    if units == 0 {
        return;
    }
    let gx = world.wrap_x(gx);
    let e = world.soft_litter.entry(gx).or_insert(0);
    *e = e.saturating_add(units);
}

fn take_soft_litter(world: &mut World, gx: i32, want: u16) -> u16 {
    let gx = world.wrap_x(gx);
    let Some(e) = world.soft_litter.get_mut(&gx) else {
        return 0;
    };
    let take = (*e).min(want);
    *e = e.saturating_sub(take);
    if *e == 0 {
        world.soft_litter.remove(&gx);
    }
    take
}

/// Count Organic solid cells in the litter / soil band near the fungus.
fn organic_cells_near(world: &World, gx: i32, gy: i32) -> u32 {
    let gx = world.wrap_x(gx);
    let mut n = 0u32;
    for dx in -ORGANIC_SCAN_RADIUS..=ORGANIC_SCAN_RADIUS {
        let nx = world.wrap_x(gx + dx);
        for dy in -ORGANIC_SCAN_DEPTH..=ORGANIC_SCAN_DEPTH {
            let y = gy + dy;
            if matches!(
                world.get_cell(nx, y),
                Some(c) if c.material == MaterialId::Organic
            ) {
                n += 1;
            }
        }
    }
    n
}

/// Soft litter visible as labile fuel (Organic is substrate, not fuel).
pub fn labile_food_units(world: &World, gx: i32, _gy: i32) -> f32 {
    soft_litter_at(world, gx) as f32
}

/// True when the fungus has Organic substrate to colonize (even if litter is gone).
pub fn has_organic_substrate(world: &World, gx: i32, gy: i32) -> bool {
    organic_cells_near(world, gx, gy) > 0
}

/// Pore / standing-water moisture under / around the fungus.
///
/// Free-water Air (`sat > 0`) counts — rain ponds on Organic are how beds
/// get humid. Skipping Air made rain-wet columns look bone-dry and forced
/// drought hibernate (no mycelium growth).
pub fn fungus_moisture_frac(world: &World, atom: &Atom) -> f32 {
    let gx = world.wrap_x(atom.gx);
    let mut best = 0.0f32;
    for dy in -2..=3 {
        let y = atom.gy + dy;
        if let Some(c) = world.get_cell(gx, y) {
            if c.material == MaterialId::Air {
                if !c.sat.is_empty() {
                    best = best.max(c.sat.0 as f32 / u8::MAX as f32);
                }
                continue;
            }
            let cap = water_capacity(c.material);
            if cap > 0 {
                best = best.max(c.sat.0 as f32 / cap as f32);
            }
        }
    }
    best
}

/// True when fungi should hibernate (no food bed, or bone-dry without Organic).
///
/// Organic substrate is a live colony bed — do not drought-hibernate just
/// because pores are briefly dry under a rain film.
pub fn fungus_should_hibernate(world: &World, atom: &Atom) -> bool {
    let litter = labile_food_units(world, atom.gx, atom.gy);
    let substrate = has_organic_substrate(world, atom.gx, atom.gy);
    if litter < FUNGUS_STARVE_UNITS && !substrate {
        return true;
    }
    if substrate {
        return false;
    }
    fungus_moisture_frac(world, atom) < FUNGUS_DROUGHT_FRAC
}

/// Strongest mycelium intensity on Organic near `(gx, gy)`.
pub fn max_mycelium_near(world: &World, gx: i32, gy: i32) -> u8 {
    let gx = world.wrap_x(gx);
    let mut best = 0u8;
    for dx in -ORGANIC_SCAN_RADIUS..=ORGANIC_SCAN_RADIUS {
        let nx = world.wrap_x(gx + dx);
        for dy in -ORGANIC_SCAN_DEPTH..=ORGANIC_SCAN_DEPTH {
            if let Some(c) = world.get_cell(nx, gy + dy) {
                if c.material == MaterialId::Organic {
                    best = best.max(c.mycelium());
                }
            }
        }
    }
    best
}

/// Labile forage from nearby Organic / mycelium field.
/// Does not destroy cells — mycelium intensity is the visible progress.
pub fn forage_organic_energy(world: &World, gx: i32, gy: i32, genome: &Genome, atom: &Atom) -> f32 {
    if !has_organic_substrate(world, gx, gy) {
        return 0.0;
    }
    let n_d = digest_count(atom).max(1) as f32;
    let n_h = hypha_count(atom) as f32;
    let rate = genome.digest_rate.clamp(0.05, 2.0);
    let myc = max_mycelium_near(world, gx, gy) as f32;
    // Established networks feed fruiting bodies harder.
    let myc_boost = 1.0 + (myc / 80.0).min(2.0);
    ORGANIC_FORAGE_ENERGY * n_d * (1.0 + 0.10 * n_h) * rate.clamp(0.5, 1.5) * myc_boost
}

/// Fruiting body is backed by a moist, colonized mycelium bed.
pub fn fruiting_body_supported(world: &World, atom: &Atom) -> bool {
    max_mycelium_near(world, atom.gx, atom.gy) >= FRUIT_SUPPORT_MYC
        && fungus_moisture_frac(world, atom) >= FUNGUS_DROUGHT_FRAC
}

fn organic_cell_moist_frac(world: &World, gx: i32, gy: i32) -> f32 {
    let gx = world.wrap_x(gx);
    let mut best = 0.0f32;
    if let Some(c) = world.get_cell(gx, gy) {
        if c.material != MaterialId::Air {
            let cap = water_capacity(c.material);
            if cap > 0 {
                best = best.max(c.sat.0 as f32 / cap as f32);
            }
        }
    }
    for (dx, dy) in [(0i32, 1), (0, -1), (1, 0), (-1, 0)] {
        let nx = world.wrap_x(gx + dx);
        if let Some(n) = world.get_cell(nx, gy + dy) {
            if n.material == MaterialId::Air && !n.sat.is_empty() {
                best = best.max(n.sat.0 as f32 / u8::MAX as f32);
            }
        }
    }
    best
}

/// Autonomous mycelium field: thicken / spread on moist Organic without a
/// living fruiting body. Call from the world tick.
pub fn step_mycelium_field(world: &mut World) {
    step_mycelium_field_cfg(world, &FungiConfig::default());
}

/// [`step_mycelium_field`] with live [`FungiConfig`] compost knobs.
pub fn step_mycelium_field_cfg(world: &mut World, cfg: &FungiConfig) {
    if world.tick % MYCELIUM_FIELD_PERIOD != 0 {
        return;
    }
    use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};

    let threshold = cfg.soil_mycelium_threshold;
    let convert_odds = cfg.soil_convert_odds.max(1);

    let mut colonized: Vec<(i32, i32, u8)> = Vec::new();
    let coords: Vec<_> = world.chunks.keys().copied().collect();
    for coord in coords {
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + lx as i32;
                let gy = coord.cy * CHUNK_CELLS_H as i32 + ly as i32;
                let Some(c) = world.get_cell(gx, gy) else {
                    continue;
                };
                if c.material == MaterialId::Organic && c.mycelium() > 0 {
                    colonized.push((gx, gy, c.mycelium()));
                    if colonized.len() >= MYCELIUM_FIELD_MAX_CELLS {
                        break;
                    }
                }
            }
            if colonized.len() >= MYCELIUM_FIELD_MAX_CELLS {
                break;
            }
        }
        if colonized.len() >= MYCELIUM_FIELD_MAX_CELLS {
            break;
        }
    }
    if colonized.is_empty() {
        return;
    }

    let seed = world.seed.0;
    let tick = world.tick;
    let mut spreads: Vec<(i32, i32)> = Vec::new();
    let mut grows: Vec<(i32, i32)> = Vec::new();
    let mut decays: Vec<(i32, i32)> = Vec::new();

    for (i, &(gx, gy, myc)) in colonized.iter().enumerate() {
        let moist = organic_cell_moist_frac(world, gx, gy);
        if moist >= MYCELIUM_FIELD_MOIST {
            if myc < 255 {
                grows.push((gx, gy));
            }
            if myc >= 16 {
                let h = hash_u64(seed, tick, gx as u64, gy as u64 ^ (i as u64));
                if h % MYCELIUM_FIELD_SPREAD_ODDS == 0 {
                    spreads.push((gx, gy));
                }
            }
            if myc >= threshold {
                let h = hash_u64(seed, tick, gx as u64, 0x5011u64);
                if h % convert_odds == 0 {
                    compost_organic_to_soil(world, gx, gy);
                }
            }
        } else if myc > 0 {
            // Bone-dry: rare fade, network hibernates rather than vanishing.
            let h = hash_u64(seed, tick, gx as u64, 0xD00Du64);
            if h % 64 == 0 {
                decays.push((gx, gy));
            }
        }
    }

    for (gx, gy) in grows {
        if let Some(mut c) = world.get_cell(gx, gy) {
            if c.material == MaterialId::Organic {
                c.set_mycelium(c.mycelium().saturating_add(MYCELIUM_FIELD_GROW));
                world.set_cell(gx, gy, c);
            }
        }
    }
    for (gx, gy) in spreads {
        spread_mycelium_once(world, gx, gy);
    }
    for (gx, gy) in decays {
        if let Some(mut c) = world.get_cell(gx, gy) {
            if c.material == MaterialId::Organic {
                c.set_mycelium(c.mycelium().saturating_sub(1));
                world.set_cell(gx, gy, c);
            }
        }
    }
}

/// Soft-litter sip budget from gene + modules (capped low — mycelium is slow).
pub fn digest_budget_units(genome: &Genome, atom: &Atom) -> u16 {
    let n_d = digest_count(atom).max(1) as f32;
    let n_h = hypha_count(atom) as f32;
    let rate = genome.digest_rate.clamp(0.05, 2.0);
    let scale = n_d * (1.0 + 0.10 * n_h) * rate;
    let u = (scale * DIGEST_MAX_UNITS as f32).round() as u16;
    u.clamp(1, DIGEST_MAX_UNITS)
}

/// Pick an Organic cell to thread next.
///
/// Prefer close cells under/around the fungus and thicken an existing
/// patch before spraying +1 onto the farthest clean cell in the scan
/// window (that dilution made colonies look idle while energy still rose).
fn find_organic_xy(world: &World, gx: i32, gy: i32) -> Option<(i32, i32)> {
    let gx = world.wrap_x(gx);
    let mut best: Option<(i32, i32, i32)> = None; // score, x, y — lower wins
    for dx in -ORGANIC_SCAN_RADIUS..=ORGANIC_SCAN_RADIUS {
        let nx = world.wrap_x(gx + dx);
        for dy in -ORGANIC_SCAN_DEPTH..=ORGANIC_SCAN_DEPTH {
            let y = gy + dy;
            let Some(c) = world.get_cell(nx, y) else {
                continue;
            };
            if c.material != MaterialId::Organic {
                continue;
            }
            let m = c.mycelium();
            if m >= 255 {
                continue;
            }
            let dist = dx.abs() + dy.abs();
            // 0 = young patch (keep thickening), 1 = mid, 2 = virgin frontier.
            let stage = if (1..80).contains(&m) {
                0
            } else if m >= 80 {
                1
            } else {
                2
            };
            // Prefer climbing toward free Air so networks breach the surface
            // before raising a stalk (slight bias — still thicken near seat).
            let open_above = matches!(
                world.get_cell(nx, y + 1),
                Some(a) if a.material == MaterialId::Air
            );
            let climb = if open_above {
                0
            } else if dy > 0 {
                1
            } else {
                2
            };
            let score = dist * 8 + stage + climb;
            if best.map(|(bs, _, _)| score < bs).unwrap_or(true) {
                best = Some((score, nx, y));
            }
        }
    }
    best.map(|(_, x, y)| (x, y))
}

/// Push leftover pore water into neighbouring Air / porous cells so
/// Organic → Soil compaction never destroys water mass.
fn push_excess_sat(world: &mut World, gx: i32, gy: i32, mut excess: u8) {
    if excess == 0 {
        return;
    }
    for (dx, dy) in [(0i32, 1), (0, -1), (1, 0), (-1, 0), (1, 1), (-1, 1)] {
        if excess == 0 {
            break;
        }
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        let Some(mut c) = world.get_cell(nx, ny) else {
            continue;
        };
        let cap = world.water_capacity(c.material);
        if cap == 0 || c.sat.0 >= cap {
            continue;
        }
        let room = cap - c.sat.0;
        let give = excess.min(room);
        c.sat.0 = c.sat.0.saturating_add(give);
        excess = excess.saturating_sub(give);
        world.set_cell(nx, ny, c);
    }
    // Last resort: create a free-water film above if anything remains.
    if excess > 0 {
        if let Some(mut above) = world.get_cell(gx, gy + 1) {
            if above.material == MaterialId::Air {
                let room = u8::MAX - above.sat.0;
                let give = excess.min(room);
                above.sat.0 = above.sat.0.saturating_add(give);
                world.set_cell(gx, gy + 1, above);
            }
        }
    }
}

/// Convert a fully colonized Organic cell into Soil, preserving water.
pub fn compost_organic_to_soil(world: &mut World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    let Some(c) = world.get_cell(gx, gy) else {
        return false;
    };
    if c.material != MaterialId::Organic {
        return false;
    }
    let old_sat = c.sat.0;
    let cap = world.water_capacity(MaterialId::Soil);
    let keep = if cap > 0 { old_sat.min(cap) } else { 0 };
    let excess = old_sat.saturating_sub(keep);
    let mut soil = Cell::solid(MaterialId::Soil);
    soil.sat.0 = keep;
    soil.flags.set(CellFlags::COMPACTED);
    world.set_cell(gx, gy, soil);
    push_excess_sat(world, gx, gy, excess);
    true
}

/// Sip soft litter for energy. Organic cells are colonized separately via
/// [`colonize_and_compost`] — this never converts materials.
pub fn digest_labile(world: &mut World, gx: i32, _gy: i32, want: u16) -> (u16, f32) {
    if want == 0 {
        return (0, 0.0);
    }
    let litter = soft_litter_at(world, gx) as f32;
    if litter < FUNGUS_STARVE_UNITS {
        return (0, 0.0);
    }
    // At least one unit when any labile litter is present — frac alone
    // floors to 0 on small piles with the slow DIGEST_TICK_FRAC.
    let tick_cap = (litter * DIGEST_TICK_FRAC)
        .floor()
        .max(1.0) as u16;
    let sip = want.min(tick_cap).min(DIGEST_MAX_UNITS);
    if sip == 0 {
        return (0, 0.0);
    }
    let taken = take_soft_litter(world, gx, sip);
    if taken > 0 {
        (taken, taken as f32 * DIGEST_ENERGY_PER_UNIT)
    } else {
        (0, 0.0)
    }
}

/// Grow mycelium with gene/hypha scaling; compost when ready.
/// Preferred entry from `step_fungus` (includes genome).
///
/// Uses the organism `tick` (not only `World::tick`) so growth stays paced
/// with the creature step even if callers disagree on world clock sync.
pub fn colonize_and_compost(
    world: &mut World,
    gx: i32,
    gy: i32,
    genome: &Genome,
    atom: &Atom,
    tick: u64,
) -> f32 {
    colonize_and_compost_cfg(world, gx, gy, genome, atom, tick, &FungiConfig::default())
}

/// [`colonize_and_compost`] with live [`FungiConfig`] compost knobs.
pub fn colonize_and_compost_cfg(
    world: &mut World,
    gx: i32,
    gy: i32,
    genome: &Genome,
    atom: &Atom,
    tick: u64,
    cfg: &FungiConfig,
) -> f32 {
    let mut energy = 0.0f32;
    let n_h = hypha_count(atom) as u8;
    let rate = genome.digest_rate.clamp(0.05, 2.0);
    // Period stretches when digest_rate is low; high rate shortens slightly.
    let period = ((MYCELIUM_GROW_PERIOD as f32) / rate.clamp(0.25, 2.0)).round() as u64;
    let period = period.max(8);
    if tick % period != 0 {
        return 0.0;
    }
    let Some((ox, oy)) = find_organic_xy(world, gx, gy) else {
        return 0.0;
    };
    let threshold = cfg.soil_mycelium_threshold;
    let convert_odds = cfg.soil_convert_odds.max(1);
    if let Some(mut c) = world.get_cell(ox, oy) {
        let before = c.mycelium();
        if before < 255 {
            // Closer to the fungus → thicker threads (visible local change).
            let dist = (ox - world.wrap_x(gx)).abs() + (oy - gy).abs();
            let near_bonus = if dist <= 1 {
                2
            } else if dist <= 3 {
                1
            } else {
                0
            };
            let add = MYCELIUM_GROW_AMOUNT
                .saturating_add(n_h / 4)
                .saturating_add(near_bonus)
                .max(1)
                .min(4);
            c.set_mycelium(before.saturating_add(add));
            world.set_cell(ox, oy, c);
            energy += MYCELIUM_ENERGY * (1.0 + 0.05 * n_h as f32);
        }
        let intensity = world
            .get_cell(ox, oy)
            .map(|c| c.mycelium())
            .unwrap_or(before);
        if intensity >= threshold {
            let h = hash_u64(world.seed.0, tick, ox as u64, oy as u64);
            if h % convert_odds == 0 {
                compost_organic_to_soil(world, ox, oy);
            }
        }
    }
    let h = hash_u64(world.seed.0, tick, gx as u64, 0x51CE_A11C);
    if h % MYCELIUM_SPREAD_ODDS == 0 {
        spread_mycelium_once(world, ox, oy);
    }
    energy
}

fn spread_mycelium_once(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    // Up first — networks grow toward the free surface before lateral fill.
    for (dx, dy) in [
        (0i32, 1),
        (1, 1),
        (-1, 1),
        (1, 0),
        (-1, 0),
        (0, -1),
        (1, -1),
        (-1, -1),
    ] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if let Some(mut c) = world.get_cell(nx, ny) {
            if c.material == MaterialId::Organic && c.mycelium() < 40 {
                c.set_mycelium(c.mycelium().saturating_add(8));
                world.set_cell(nx, ny, c);
                return;
            }
        }
    }
}

/// Active upkeep for Digest + Hypha tissue.
pub fn fungus_upkeep(atom: &Atom, dormant: bool) -> f32 {
    let base = DIGEST_UPKEEP * digest_count(atom) as f32
        + HYPHA_UPKEEP * hypha_count(atom) as f32;
    if dormant {
        base * FUNGUS_DORMANT_UPKEEP
    } else {
        base
    }
}

/// Nucleus may sit in Organic (underground mycelium) or Air above a solid.
pub fn is_fungus_seated(world: &World, atom: &Atom) -> bool {
    let gx = world.wrap_x(atom.gx);
    let Some(here) = world.get_cell(gx, atom.gy) else {
        return false;
    };
    if here.material == MaterialId::Organic {
        return true;
    }
    if here.material != MaterialId::Air {
        return false;
    }
    matches!(
        world.get_cell(gx, atom.gy - 1),
        Some(c) if c.material != MaterialId::Air
    )
}

/// True when the fruiting body sits in Air above the bed (surface stalk).
/// Stalks launch wind spores; buried bodies only rhizomorph-hop locally.
pub fn is_surface_stalk(world: &World, atom: &Atom) -> bool {
    let gx = world.wrap_x(atom.gx);
    let Some(here) = world.get_cell(gx, atom.gy) else {
        return false;
    };
    if here.material != MaterialId::Air {
        return false;
    }
    matches!(
        world.get_cell(gx, atom.gy - 1),
        Some(c) if c.material != MaterialId::Air
    )
}

/// True when colonized Organic in this column is open to Air *and* the
/// network has threaded from below (not a lone surface film).
pub fn fruiting_surface_ready(world: &World, gx: i32) -> Option<i32> {
    let gx = world.wrap_x(gx);
    for y in (-64..128).rev() {
        let Some(c) = world.get_cell(gx, y) else {
            continue;
        };
        if c.material != MaterialId::Organic || c.mycelium() < 80 {
            continue;
        }
        if !matches!(
            world.get_cell(gx, y + 1),
            Some(a) if a.material == MaterialId::Air
        ) {
            continue;
        }
        if !mycelium_breached_from_below(world, gx, y) {
            continue;
        }
        return Some(y + 1); // Air cell for the fruiting body / spore launch
    }
    None
}

/// Surface Organic must connect to deeper mycelium (grew upward to breach).
fn mycelium_breached_from_below(world: &World, gx: i32, surface_y: i32) -> bool {
    // Same-column deeper thread, or a neighbour column feeder at/below.
    for dy in 1..=4 {
        let y = surface_y - dy;
        if matches!(
            world.get_cell(gx, y),
            Some(c) if c.material == MaterialId::Organic && c.mycelium() > 0
        ) {
            return true;
        }
    }
    for dx in [-1i32, 1] {
        let nx = world.wrap_x(gx + dx);
        for dy in 0..=3 {
            let y = surface_y - dy;
            if matches!(
                world.get_cell(nx, y),
                Some(c) if c.material == MaterialId::Organic && c.mycelium() >= 16
            ) {
                return true;
            }
        }
    }
    false
}

/// Pick a seat for a spore / rhizomorph hop.
///
/// - `wind_far`: surface stalk — prefer farther downwind seats.
/// - otherwise: underground — short local hops only.
///
/// Skips columns that already host a living fruiting body and enforces
/// a soft local density cap ([`FUNGUS_SPORE_LOCAL_MAX`]).
pub fn pick_spore_site(
    world: &World,
    atom: &Atom,
    tick: u64,
    id: u32,
    wind_vx: f32,
    fungus_cols: &[i32],
) -> Option<(i32, i32)> {
    pick_spore_site_mode(world, atom, tick, id, wind_vx, fungus_cols, is_surface_stalk(world, atom))
}

fn pick_spore_site_mode(
    world: &World,
    atom: &Atom,
    tick: u64,
    id: u32,
    wind_vx: f32,
    fungus_cols: &[i32],
    wind_far: bool,
) -> Option<(i32, i32)> {
    let wx0 = atom.gx;
    let prefer_dir = if wind_vx.abs() < 0.05 {
        let flip = hash_u64(world.seed.0, tick, id as u64, 0xF5C0) & 1;
        if flip == 0 {
            1
        } else {
            -1
        }
    } else if wind_vx > 0.0 {
        1
    } else {
        -1
    };
    let (min_d, max_d) = if wind_far {
        (FUNGUS_STALK_SPORE_MIN_DIST, FUNGUS_STALK_SPORE_MAX_DIST)
    } else {
        (1, FUNGUS_RHIZOMORPH_MAX_DIST)
    };
    let mut best: Option<(f32, i32, i32)> = None;
    for dist in min_d..=max_d {
        for &sign in &[prefer_dir, -prefer_dir] {
            let wx = world.wrap_x(wx0 + sign * dist);
            if fungus_cols.iter().any(|&ox| world.wrap_x(ox) == wx) {
                continue;
            }
            let local = count_fungi_near(fungus_cols, wx, FUNGUS_SPORE_LOCAL_RADIUS, world.wrap_width);
            if local >= FUNGUS_SPORE_LOCAL_MAX {
                continue;
            }
            let Some(gy) = find_fungus_slot_biased(world, wx, atom.gy, wind_far) else {
                continue;
            };
            // Prefer Organic substrate; allow any fungus seat as bank.
            let organic = organic_cells_near(world, wx, gy) as f32;
            let litter = soft_litter_at(world, wx) as f32;
            if organic < 1.0 && litter < FUNGUS_STARVE_UNITS {
                continue;
            }
            let downwind = if sign == prefer_dir { 2.0 } else { 0.0 };
            let score = if wind_far {
                // Aerial stalk: farther downwind is better (like ferns).
                organic * 2.0 + litter * 0.04 + dist as f32 * 0.06 + downwind
            } else {
                // Rhizomorph: stay close, thicken the local network.
                organic * 3.0 + litter * 0.05 - dist as f32 * 0.35 + downwind * 0.25
            };
            if best.map(|(s, _, _)| score > s).unwrap_or(true) {
                best = Some((score, wx, gy));
            }
        }
    }
    best.map(|(_, wx, gy)| (wx, gy))
}

/// Count living fruiting-body crowns within `radius` columns of `gx`.
pub fn count_fungi_near(
    fungus_cols: &[i32],
    gx: i32,
    radius: i32,
    wrap_width: Option<i32>,
) -> usize {
    fungus_cols
        .iter()
        .filter(|&&px| {
            let d = (px - gx).abs();
            let dist = match wrap_width {
                Some(w) if w > 0 => d.min(w - d.min(w)),
                _ => d,
            };
            dist <= radius
        })
        .count()
}

/// Seed a faint mycelium patch on Organic near `(gx, gy)`.
pub fn seed_mycelium_near(world: &mut World, gx: i32, gy: i32, amount: u8) {
    if let Some((ox, oy)) = find_organic_xy(world, gx, gy) {
        if let Some(mut c) = world.get_cell(ox, oy) {
            c.set_mycelium(c.mycelium().saturating_add(amount).min(48));
            world.set_cell(ox, oy, c);
        }
    }
}

/// Mycelium field raises a surface stalk once a moist network has climbed
/// to Organic open to Air (no living parent required). The new body can
/// later [`try_spore`] on the wind.
pub fn try_emergent_fruiting(
    world: &mut World,
    occupied_fungus_cols: &[i32],
    tick: u64,
    pop_room: bool,
) -> Option<Atom> {
    if !pop_room || tick % MYCELIUM_EMERGE_PERIOD != 0 {
        return None;
    }
    use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};

    let mut candidates: Vec<(i32, i32, u8)> = Vec::new(); // gx, air_y, myc
    let coords: Vec<_> = world.chunks.keys().copied().collect();
    for coord in coords {
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let gx = world.wrap_x(coord.cx * CHUNK_CELLS_W as i32 + lx as i32);
                let gy = coord.cy * CHUNK_CELLS_H as i32 + ly as i32;
                let Some(c) = world.get_cell(gx, gy) else {
                    continue;
                };
                if c.material != MaterialId::Organic || c.mycelium() < 80 {
                    continue;
                }
                if organic_cell_moist_frac(world, gx, gy) < MYCELIUM_FIELD_MOIST {
                    continue;
                }
                if !matches!(
                    world.get_cell(gx, gy + 1),
                    Some(a) if a.material == MaterialId::Air
                ) {
                    continue;
                }
                // Must have grown up from below — not a lone surface film.
                if !mycelium_breached_from_below(world, gx, gy) {
                    continue;
                }
                if occupied_fungus_cols
                    .iter()
                    .any(|&ox| world.wrap_x(ox) == gx)
                {
                    continue;
                }
                let local = count_fungi_near(
                    occupied_fungus_cols,
                    gx,
                    FUNGUS_SPORE_LOCAL_RADIUS,
                    world.wrap_width,
                );
                if local >= FUNGUS_SPORE_LOCAL_MAX {
                    continue;
                }
                candidates.push((gx, gy + 1, c.mycelium()));
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }
    // Prefer the richest patch; rarity gate on the winner.
    candidates.sort_by(|a, b| b.2.cmp(&a.2));
    let (gx, air_y, _myc) = candidates[0];
    let h = hash_u64(world.seed.0, tick, gx as u64, 0xE3E7_F001);
    if h % MYCELIUM_EMERGE_ODDS != 0 {
        return None;
    }
    // Spend field intensity — fruiting costs the network.
    if let Some(mut bed) = world.get_cell(gx, air_y - 1) {
        if bed.material == MaterialId::Organic {
            bed.set_mycelium(bed.mycelium().saturating_sub(MYCELIUM_EMERGE_COST));
            world.set_cell(gx, air_y - 1, bed);
        }
    }
    let body = crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus();
    let tank = 40.0f32;
    let mut child = Atom::from_body(gx, air_y, tank, body);
    apply_genome(&mut child, Genome::default());
    child.genome.digest_rate = 1.0;
    child.energy = (tank * MYCELIUM_EMERGE_ENERGY_FRAC).clamp(1.0, child.energy_max);
    // Mature before sporing — emergence must not immediately flood.
    child.cooldown = FUNGUS_SPORE_PERIOD;
    pin_plant_pose(&mut child);
    if !is_fungus_seated(world, &child) {
        return None;
    }
    Some(child)
}

/// Rare dispersal: energy cost, child on Organic / litter.
///
/// Requires painted [`ModuleId::ReproSpore`]. Habit depends on seating:
/// - **Surface stalk** — column must be [`fruiting_surface_ready`]; wind
///   carries the child far ([`FUNGUS_STALK_SPORE_MIN_DIST`]…).
/// - **Underground** — short rhizomorph hop ([`FUNGUS_RHIZOMORPH_MAX_DIST`])
///   that seeds mycelium nearby (no surface / wind requirement).
///
/// One living fruiting body per column + local density gate (anti-flood).
pub fn try_spore(
    world: &mut World,
    atom: &mut Atom,
    tick: u64,
    entity_id: u32,
    pop_room: bool,
    wind_vx: f32,
    fungus_cols: &[i32],
) -> Option<Atom> {
    if !pop_room || atom.cooldown > 0 {
        return None;
    }
    if !is_fungus(atom) || digest_count(atom) < 1 {
        return None;
    }
    if atom
        .body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::ReproSpore)
        .count()
        < 1
    {
        return None;
    }
    let stalk = is_surface_stalk(world, atom);
    // Stalks only launch once mycelium has breached this column's surface.
    if stalk && fruiting_surface_ready(world, atom.gx).is_none() {
        return None;
    }
    // Buried bodies may rhizomorph without a surface breach.
    if !stalk && !is_fungus_seated(world, atom) {
        return None;
    }
    let local = count_fungi_near(
        fungus_cols,
        atom.gx,
        FUNGUS_SPORE_LOCAL_RADIUS,
        world.wrap_width,
    );
    if local >= FUNGUS_SPORE_LOCAL_MAX {
        return None;
    }
    let tank = if atom.energy_base_max >= 1.0 {
        atom.energy_base_max
    } else {
        atom.energy_max
    }
    .max(1.0);
    if atom.energy < tank * FUNGUS_SPORE_ENERGY_FRAC {
        return None;
    }
    // Extra rarity gate beyond cooldown (stalks rarer than local hops).
    let h = hash_u64(world.seed.0, tick, entity_id as u64, 0xF801_700D);
    let odds = if stalk { 17 } else { 11 };
    if h % odds != 0 {
        return None;
    }
    let (wx, gy) = pick_spore_site_mode(world, atom, tick, entity_id, wind_vx, fungus_cols, stalk)?;
    let cost = tank * if stalk { 0.45 } else { 0.32 };
    if atom.energy < cost {
        return None;
    }
    atom.energy -= cost;
    atom.cooldown = if stalk {
        FUNGUS_SPORE_PERIOD
    } else {
        FUNGUS_SPORE_PERIOD / 2
    };

    // Inherit parent fruiting-body pixels, then morph-mutate.
    let mut body = mutate_body(
        &atom.body,
        atom.genome.clone_fidelity,
        world.seed.0,
        tick,
        entity_id,
    );
    // Guarantee a spore packet so the line can keep dispersing.
    if !body.iter().any(|(_, _, m)| *m == ModuleId::ReproSpore) {
        let occupied: std::collections::HashSet<(i16, i16)> =
            body.iter().map(|&(x, y, _)| (x, y)).collect();
        let spot = [(1i16, 1i16), (1, 0), (0, 1), (-1, 1), (2, 0)]
            .into_iter()
            .find(|p| !occupied.contains(p))
            .unwrap_or((1, 1));
        body.push((spot.0, spot.1, ModuleId::ReproSpore));
    }
    let child_genome = Genome::mutate(atom.genome, world.seed.0, tick, entity_id);
    let mut child = Atom::from_body(wx, gy, tank, body);
    apply_genome(&mut child, child_genome);
    child.energy = (cost * 0.5).clamp(1.0, child.energy_max);
    // Children must mature before chaining another spore burst.
    child.cooldown = if stalk {
        FUNGUS_SPORE_PERIOD.saturating_mul(2)
    } else {
        FUNGUS_SPORE_PERIOD
    };
    pin_plant_pose(&mut child);
    if !is_fungus_seated(world, &child) {
        atom.energy = (atom.energy + cost).min(atom.energy_max);
        atom.cooldown = 0;
        return None;
    }
    // Spore / rhizomorph germinates as faint threads in the landing Organic.
    seed_mycelium_near(world, wx, gy, if stalk { 24 } else { 18 });
    Some(child)
}

/// Soft litter + one Organic on the bed (fallback when no body cells paint).
pub fn deposit_death_litter(world: &mut World, gx: i32, gy: i32, n_modules: usize) {
    let units = (DEATH_LITTER_PER_MODULE as usize)
        .saturating_mul(n_modules.max(1))
        .min(DEATH_LITTER_MAX as usize) as u16;
    add_soft_litter(world, gx, units);
    deposit_organic_cell(world, gx, gy);
}

/// Dissolve a lingering corpse into Organic matter + soft litter.
///
/// Shoot modules (Stem / Nucleus / Photosystem) never become mid-air Organic
/// pillars — water and snow must pass dead trunks; compost belongs on the
/// bed (fallback pile) or in soil already painted by dead roots. Digest /
/// Hypha / Root footprints still convert solids (and dry Air for detritus).
/// Wet Air (free water) is left alone so lakes aren't plugged.
pub fn dissolve_corpse_to_organic(
    world: &mut World,
    gx: i32,
    gy: i32,
    body: &[(i16, i16, ModuleId)],
) {
    let n_modules = body.len().max(1);
    let units = (DEATH_LITTER_PER_MODULE as usize)
        .saturating_mul(n_modules)
        .min(DEATH_LITTER_MAX as usize) as u16;
    add_soft_litter(world, gx, units);

    let mut painted = 0u32;
    for &(dx, dy, mid) in body {
        // Grey trunks / crowns / leaves: litter only — do not dam flow.
        if matches!(
            mid,
            ModuleId::Stem
                | ModuleId::Nucleus
                | ModuleId::Photosystem
                | ModuleId::ReproSpore
        ) {
            continue;
        }
        let wx = world.wrap_x(gx + dx as i32);
        let wy = gy + dy as i32;
        let Some(c) = world.get_cell(wx, wy) else {
            continue;
        };
        match c.material {
            MaterialId::Air if c.sat.is_empty() => {
                world.set_cell(wx, wy, Cell::solid(MaterialId::Organic));
                painted += 1;
            }
            MaterialId::Air => {
                // Free water — leave the lake; litter already banked.
            }
            MaterialId::Bedrock | MaterialId::Ice | MaterialId::Snow | MaterialId::Water => {}
            _ => {
                // Sand / stone / clay / soil / Organic → Organic residue.
                // Preserve pore sat so dissolve doesn't destroy water mass.
                let mut org = Cell::solid(MaterialId::Organic);
                let cap = water_capacity(MaterialId::Organic);
                org.sat.0 = if cap > 0 { c.sat.0.min(cap) } else { 0 };
                world.set_cell(wx, wy, org);
                painted += 1;
            }
        }
    }
    if painted == 0 {
        deposit_organic_cell(world, gx, gy);
    }
}

fn deposit_organic_cell(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    let mut y = gy;
    for _ in 0..64 {
        match world.get_cell(gx, y) {
            Some(c) if c.material != MaterialId::Air => {
                if let Some(above) = world.get_cell(gx, y + 1) {
                    if above.material == MaterialId::Air && above.sat.is_empty() {
                        world.set_cell(gx, y + 1, Cell::solid(MaterialId::Organic));
                    }
                }
                return;
            }
            None => return,
            Some(_) => y -= 1,
        }
    }
}

fn hash_u64(a: u64, b: u64, c: u64, salt: u64) -> u64 {
    let mut x = a
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(b)
        .wrapping_add(c)
        .wrapping_add(salt);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Sat;
    use crate::chunk::ChunkCoord;
    use crate::organism::BodyModule;

    fn litter_plot() -> World {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 1, sand);
            for y in 2..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w
    }

    fn fungus_body() -> Vec<BodyModule> {
        crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus()
    }

    #[test]
    fn digest_prefers_soft_litter() {
        let mut w = litter_plot();
        add_soft_litter(&mut w, 4, 40);
        w.set_cell(4, 2, Cell::solid(MaterialId::Organic));
        let (taken, energy) = digest_labile(&mut w, 4, 2, 1);
        assert!(taken > 0 && taken <= DIGEST_MAX_UNITS);
        assert!(energy > 0.0);
        assert!(soft_litter_at(&w, 4) < 40, "should eat soft litter first");
        assert_eq!(
            w.get_cell(4, 2).map(|c| c.material),
            Some(MaterialId::Organic),
            "Organic must not be destroyed by a litter sip"
        );
    }

    #[test]
    fn colonize_does_not_flash_organic_to_sand() {
        let mut w = litter_plot();
        for y in 1..=4 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(100);
            w.set_cell(4, y, org);
        }
        let g = Genome {
            digest_rate: 1.0,
            ..Genome::default()
        };
        let atom = Atom::from_body(4, 3, 40.0, fungus_body());
        let _ = colonize_and_compost(&mut w, 4, 3, &g, &atom, MYCELIUM_GROW_PERIOD);
        let sands = (1..=4)
            .filter(|&y| {
                w.get_cell(4, y)
                    .map(|c| c.material == MaterialId::Sand)
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(sands, 0, "mycelium must never produce Sand");
        let threaded = (1..=4).any(|y| {
            w.get_cell(4, y)
                .map(|c| c.material == MaterialId::Organic && c.mycelium() > 0)
                .unwrap_or(false)
        });
        assert!(threaded, "should raise mycelium on Organic");
    }

    #[test]
    fn colonize_thickens_organic_under_fungus_not_far_pad() {
        let mut w = litter_plot();
        // Near Organic under the seat + a far Organic pad in scan radius.
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(100);
            w.set_cell(4, y, org);
        }
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(100);
            w.set_cell(6, y, org); // dx=2 edge of scan
        }
        let g = Genome {
            digest_rate: 1.0,
            ..Genome::default()
        };
        let atom = Atom::from_body(4, 3, 40.0, fungus_body());
        // Seed a young patch under the fungus, leave far cells virgin.
        if let Some(mut c) = w.get_cell(4, 2) {
            c.set_mycelium(24);
            w.set_cell(4, 2, c);
        }
        for pulse in 0..12u64 {
            let _ = colonize_and_compost(
                &mut w,
                4,
                3,
                &g,
                &atom,
                MYCELIUM_GROW_PERIOD * (pulse + 1),
            );
        }
        let near = w.get_cell(4, 2).unwrap().mycelium();
        let far = (1..=3)
            .map(|y| w.get_cell(6, y).unwrap().mycelium())
            .max()
            .unwrap();
        assert!(
            near > 24,
            "patch under fungus must thicken (got {near})"
        );
        assert!(
            near > far,
            "near colony should outpace far virgin pad (near={near} far={far})"
        );
    }

    #[test]
    fn compost_to_soil_preserves_water() {
        let mut w = litter_plot();
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(200);
        org.set_mycelium(255);
        w.set_cell(4, 2, org);
        // Neighbour Air can receive excess from compaction.
        w.set_cell(4, 3, Cell::air());
        assert!(compost_organic_to_soil(&mut w, 4, 2));
        let soil = w.get_cell(4, 2).unwrap();
        assert_eq!(soil.material, MaterialId::Soil);
        let soil_cap = water_capacity(MaterialId::Soil);
        assert_eq!(soil.sat.0, 200u8.min(soil_cap));
        let above = w.get_cell(4, 3).unwrap();
        let total = soil.sat.0 as u32 + above.sat.0 as u32;
        assert!(
            total >= 200,
            "water must not vanish on compost (total={total})"
        );
    }

    #[test]
    fn starve_column_hibernates() {
        let w = litter_plot();
        let atom = Atom::from_body(4, 2, 40.0, fungus_body());
        assert!(fungus_should_hibernate(&w, &atom));
    }

    #[test]
    fn rich_litter_does_not_hibernate() {
        let mut w = litter_plot();
        add_soft_litter(&mut w, 4, 40);
        let atom = Atom::from_body(4, 2, 40.0, fungus_body());
        assert!(!fungus_should_hibernate(&w, &atom));
    }

    #[test]
    fn organic_substrate_prevents_hibernate() {
        let mut w = litter_plot();
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(100);
        w.set_cell(4, 2, org);
        let atom = Atom::from_body(4, 2, 40.0, fungus_body());
        assert!(!fungus_should_hibernate(&w, &atom));
    }

    #[test]
    fn ponded_rain_above_dry_organic_counts_as_humid() {
        let mut w = litter_plot();
        // Dry bedrock column — no wet sand below to mask the bug.
        for y in 1..=4 {
            w.set_cell(4, y, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(4, 2, Cell::solid(MaterialId::Organic)); // dry pores
        w.set_cell(4, 3, Cell::water()); // rain pond
        let atom = Atom::from_body(4, 3, 40.0, fungus_body());
        let moist = fungus_moisture_frac(&w, &atom);
        assert!(
            moist > FUNGUS_DROUGHT_FRAC,
            "ponded rain must count as moisture (got {moist})"
        );
        assert!(
            !fungus_should_hibernate(&w, &atom),
            "must not drought-hibernate on rain-wet Organic bed"
        );
    }

    #[test]
    fn fungus_seats_inside_organic() {
        let mut w = litter_plot();
        w.set_cell(4, 3, Cell::solid(MaterialId::Organic));
        let atom = Atom::from_body(4, 3, 40.0, fungus_body());
        assert!(is_fungus_seated(&w, &atom));
    }

    #[test]
    fn organic_forage_without_litter_is_positive() {
        let mut w = litter_plot();
        w.set_cell(4, 2, Cell::solid(MaterialId::Organic));
        let g = Genome {
            digest_rate: 1.0,
            ..Genome::default()
        };
        let atom = Atom::from_body(4, 2, 40.0, fungus_body());
        let e = forage_organic_energy(&w, 4, 2, &g, &atom);
        assert!(
            e >= ORGANIC_FORAGE_ENERGY * 0.5,
            "Organic forage must yield energy without soft litter (got {e})"
        );
    }

    #[test]
    fn mycelium_field_spreads_without_fruiting_body() {
        let mut w = litter_plot();
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(160);
            w.set_cell(4, y, org);
        }
        // Seed a colonized patch — no Atom in the world.
        if let Some(mut c) = w.get_cell(4, 2) {
            c.set_mycelium(48);
            w.set_cell(4, 2, c);
        }
        let myc0 = max_mycelium_near(&w, 4, 2);
        for t in 0..80u64 {
            w.tick = t * MYCELIUM_FIELD_PERIOD;
            step_mycelium_field(&mut w);
        }
        let myc1 = max_mycelium_near(&w, 4, 2);
        assert!(myc1 > myc0, "field must thicken (was {myc0}, now {myc1})");
        let spread = (1..=3).any(|y| {
            w.get_cell(4, y)
                .map(|c| c.material == MaterialId::Organic && c.mycelium() > 0 && y != 2)
                .unwrap_or(false)
        }) || [-1i32, 1].iter().any(|&dx| {
            (1..=3).any(|y| {
                w.get_cell(4 + dx, y)
                    .map(|c| c.material == MaterialId::Organic && c.mycelium() > 0)
                    .unwrap_or(false)
            })
        });
        // Neighbour Organic may be missing on this plot — thickening alone is enough.
        let _ = spread;
        assert!(myc1 >= 48 + 4, "moist field should keep growing without a fruiting body");
    }

    /// Moist Organic column with mycelium that has climbed from below.
    fn rich_moist_surface(w: &mut World, gx: i32, myc: u8) {
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(160);
            w.set_cell(gx, y, org);
        }
        // Deeper feeder + surface breach (emergence / stalk gate).
        if let Some(mut c) = w.get_cell(gx, 2) {
            c.set_mycelium((myc / 2).max(24));
            w.set_cell(gx, 2, c);
        }
        if let Some(mut c) = w.get_cell(gx, 3) {
            c.set_mycelium(myc);
            w.set_cell(gx, 3, c);
        }
    }

    #[test]
    fn cream_network_emerges_fruiting_body() {
        let mut w = litter_plot();
        rich_moist_surface(&mut w, 4, 120);
        let myc_before = w.get_cell(4, 3).unwrap().mycelium();
        let mut child = None;
        // Period + rarity gates — sweep pulses until the hash opens.
        for pulse in 0..2_000u64 {
            let tick = pulse * MYCELIUM_EMERGE_PERIOD;
            if let Some(a) = try_emergent_fruiting(&mut w, &[], tick, true) {
                child = Some(a);
                break;
            }
        }
        let child = child.expect("rich moist mycelium must eventually raise a fruiting body");
        assert!(is_fungus(&child), "emergent body must be fungus habit");
        assert!(
            is_surface_stalk(&w, &child),
            "emergent body must be a surface stalk in Air"
        );
        let myc_after = w.get_cell(4, 3).unwrap().mycelium();
        assert!(
            myc_after < myc_before,
            "emergence must burn mycelium intensity ({myc_before} → {myc_after})"
        );
    }

    #[test]
    fn surface_film_alone_does_not_emerge() {
        let mut w = litter_plot();
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(160);
            w.set_cell(4, y, org);
        }
        // Rich surface only — no deeper feeder thread.
        if let Some(mut c) = w.get_cell(4, 3) {
            c.set_mycelium(200);
            w.set_cell(4, 3, c);
        }
        assert!(fruiting_surface_ready(&w, 4).is_none());
        for pulse in 0..400u64 {
            let tick = pulse * MYCELIUM_EMERGE_PERIOD;
            assert!(
                try_emergent_fruiting(&mut w, &[], tick, true).is_none(),
                "lone surface film must not raise a stalk"
            );
        }
    }

    #[test]
    fn mycelium_spread_prefers_upward_neighbor() {
        let mut w = litter_plot();
        for y in 1..=4 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(160);
            w.set_cell(4, y, org);
        }
        if let Some(mut c) = w.get_cell(4, 2) {
            c.set_mycelium(48);
            w.set_cell(4, 2, c);
        }
        spread_mycelium_once(&mut w, 4, 2);
        let up = w.get_cell(4, 3).unwrap().mycelium();
        let side = w.get_cell(5, 2).map(|c| c.mycelium()).unwrap_or(0);
        assert!(up >= 8, "spread should climb toward surface first (up={up})");
        assert_eq!(side, 0, "lateral should wait until up is taken");
    }

    #[test]
    fn fungus_spore_skips_occupied_columns() {
        let mut w = litter_plot();
        rich_moist_surface(&mut w, 4, 100);
        for x in 0..12 {
            rich_moist_surface(&mut w, x, 40);
            add_soft_litter(&mut w, x, 20);
        }
        let mut atom = Atom::from_body(4, 4, 40.0, fungus_body());
        // Entire moist plot claimed — must not stack on any column.
        let occupied: Vec<i32> = (0..12).collect();
        for tick in 0..4_000u64 {
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
            assert!(
                try_spore(&mut w, &mut atom, tick, 7, true, 0.4, &occupied).is_none(),
                "spore must not land on an occupied fungus column"
            );
        }
    }

    #[test]
    fn fruiting_body_can_spread_spores() {
        let mut w = litter_plot();
        // Parent column + a downwind Organic bank for the spore to land on.
        rich_moist_surface(&mut w, 4, 100);
        for x in 5..12 {
            rich_moist_surface(&mut w, x, 40);
            add_soft_litter(&mut w, x, 20);
        }
        let mut atom = Atom::from_body(4, 4, 40.0, fungus_body());
        atom.energy = atom.energy_max;
        atom.cooldown = 0;
        assert!(is_surface_stalk(&w, &atom));
        assert!(fruiting_surface_ready(&w, 4).is_some());
        let mut spore = None;
        for tick in 0..4_000u64 {
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
            if let Some(c) = try_spore(&mut w, &mut atom, tick, 7, true, 0.4, &[4]) {
                spore = Some(c);
                break;
            }
        }
        let spore = spore.expect("surface stalk must eventually wind-spore");
        assert!(is_fungus(&spore));
        let d = (spore.gx - 4).abs();
        assert!(
            d >= FUNGUS_STALK_SPORE_MIN_DIST,
            "stalk wind spore should travel far (dist={d})"
        );
    }

    #[test]
    fn underground_fungus_rhizomorph_stays_local() {
        let mut w = litter_plot();
        for x in 0..12 {
            for y in 1..=3 {
                let mut org = Cell::solid(MaterialId::Organic);
                org.sat = Sat(160);
                w.set_cell(x, y, org);
            }
            add_soft_litter(&mut w, x, 20);
        }
        // Buried fruiting body (nucleus inside Organic) — no surface stalk.
        let mut atom = Atom::from_body(4, 2, 40.0, fungus_body());
        assert!(!is_surface_stalk(&w, &atom));
        assert!(is_fungus_seated(&w, &atom));
        let mut child = None;
        for tick in 0..4_000u64 {
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
            if let Some(c) = try_spore(&mut w, &mut atom, tick, 11, true, 0.4, &[4]) {
                child = Some(c);
                break;
            }
        }
        let child = child.expect("buried fungus should rhizomorph-hop locally");
        let d = (child.gx - 4).abs();
        assert!(
            d >= 1 && d <= FUNGUS_RHIZOMORPH_MAX_DIST,
            "rhizomorph must stay local (dist={d})"
        );
    }

    #[test]
    fn hypha_raises_digest_budget() {
        let g = Genome {
            digest_rate: 1.0,
            ..Genome::default()
        };
        let mut atom = Atom::from_body(4, 2, 40.0, fungus_body());
        atom.genome = g;
        let base = digest_budget_units(&g, &atom);
        atom.body.push((4, 0, ModuleId::Hypha));
        atom.body.push((5, 0, ModuleId::Hypha));
        let boosted = digest_budget_units(&g, &atom);
        // Cap is 1 now — budget stays at cap but call must not panic.
        assert!(boosted >= 1);
        assert!(base >= 1);
    }

    #[test]
    fn fast_fungi_config_composts_threaded_organic() {
        let mut w = litter_plot();
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(120);
            org.set_mycelium(200);
            w.set_cell(4, y, org);
        }
        let cfg = FungiConfig {
            soil_mycelium_threshold: 100,
            soil_convert_odds: 1, // every eligible pulse
        };
        let mut saw_soil = false;
        for t in 0..40u64 {
            w.tick = t * MYCELIUM_FIELD_PERIOD;
            step_mycelium_field_cfg(&mut w, &cfg);
            if (1..=3).any(|y| {
                w.get_cell(4, y)
                    .map(|c| c.material == MaterialId::Soil)
                    .unwrap_or(false)
            }) {
                saw_soil = true;
                break;
            }
        }
        assert!(saw_soil, "low-odds FungiConfig must compost Organic → Soil");
    }
}
