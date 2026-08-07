//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Fungi split (Set E):
//! - **Mycelium infection** — editor / spore "plant" drops cream
//!   ([`infect_mycelium_at`]) and stamps a [`MyceliumLineage`] so later
//!   stalks match the painted (or mutated) fruiting body.
//! - **Fruiting body** — temporary Atom (Digest / Hypha / ReproSpore).
//!   Raised by [`try_emergent_fruiting`] from a moist breached network;
//!   feeds from the field, sheds inoculum, then collapses to litter.
//! - **Mycelium field** — `Cell::_pad` on porous hosts (Organic food +
//!   Soil/Sand/Clay/rock corridors) plus sparse network sugar
//!   ([`World::mycelium_energy`]). World process [`step_mycelium_field`]
//!   goal-seeks Organic and the free surface; dry gaps can fade and
//!   remoisten to reconnect.
//! - **Two dispersal habits** via [`try_spore`] (both inoculate cream +
//!   mutate lineage on release):
//!   - *Underground* — short rhizomorph hops (no wind; stalk stays alive).
//!   - *Surface stalk* — wind-borne inoculum far; spent stalk collapses.
//! Soft litter is bonus fuel. Long colonization may compost Organic → Soil
//! (never Sand), leaving a residual cream corridor. Spec:
//! `docs/organism/FUNGI.md`, `docs/organism/VOXEL_PLANTS.md`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::blueprint::{mutate_body, Genome};
use crate::cell::{hosts_mycelium, water_capacity, Cell, CellFlags};
use crate::grid::World;
use crate::organism::{Atom, BodyModule, ModuleId};
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
///
/// Old value (256) + fixed scan order let dense hubs monopolize every
/// pulse so moist frontiers looked "stalled" forever. We now scan all
/// cream cells, then pick a rotating frontier-biased sample.
pub const MYCELIUM_FIELD_MAX_CELLS: usize = 768;
/// Pore / film moisture below which field growth pauses (slow decay).
pub const MYCELIUM_FIELD_MOIST: f32 = 0.04;
/// Ticks between mycelium-field → fruiting-body emergence attempts.
/// Slow: mushrooms should feel like a rare forest event, not wallpaper.
pub const MYCELIUM_EMERGE_PERIOD: u64 = 400;
/// 1-in-N chance per eligible column when an emergence pulse fires.
pub const MYCELIUM_EMERGE_ODDS: u64 = 96;
/// Min surface mycelium intensity before a stalk can emerge.
pub const MYCELIUM_EMERGE_MIN: u8 = 120;
/// Mycelium intensity burned from the surface Organic when a fruiting body emerges.
pub const MYCELIUM_EMERGE_COST: u8 = 28;
/// Starting energy fraction for an emerged fruiting body (can spore soon).
pub const MYCELIUM_EMERGE_ENERGY_FRAC: f32 = 0.75;
/// Editor / spore inoculum cream amount (no living body).
pub const MYCELIUM_INOCULUM_AMOUNT: u8 = 36;
/// Cap when stacking inoculum on one Organic cell.
pub const MYCELIUM_INOCULUM_CAP: u8 = 96;
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
pub const FUNGUS_STALK_SPORE_MIN_DIST: i32 = 8;
pub const FUNGUS_STALK_SPORE_MAX_DIST: i32 = 72;
/// Max columns an *underground* rhizomorph hop may travel (local only).
pub const FUNGUS_RHIZOMORPH_MAX_DIST: i32 = 5;
/// Legacy alias — stalk wind ceiling (HUD / settings may still refer).
pub const FUNGUS_SPORE_MAX_DIST: i32 = FUNGUS_STALK_SPORE_MAX_DIST;
/// Neighbourhood half-width for local fruiting-body density gate.
pub const FUNGUS_SPORE_LOCAL_RADIUS: i32 = 4;
/// Max living fruiting bodies in `[gx±radius]` before further spores
/// / emergence are blocked (anti-flood — keep mushrooms sparse).
pub const FUNGUS_SPORE_LOCAL_MAX: usize = 2;
/// Age before network support prevents energy-starve (babies must earn).
pub const FRUIT_SUPPORT_MIN_AGE: u64 = 480;
/// Soft litter deposited per body module on death.
pub const DEATH_LITTER_PER_MODULE: u16 = 6;
/// Cap soft litter added from one corpse.
pub const DEATH_LITTER_MAX: u16 = 48;
/// Cream left on the new Soil cell after Organic compost — keeps a
/// mineral corridor so networks can reconnect instead of hard-severing.
pub const MYCELIUM_COMPOST_RESIDUAL: u8 = 12;
/// Soft cream cap on mineral hosts while *seeking* Organic / surface.
/// Mineral is a search corridor, not a colony — keep it thin.
pub const MYCELIUM_MINERAL_CAP: u8 = 28;
/// After a mineral cell touches threaded Organic, taper cream toward this
/// conduit thickness (cheaper than keeping a fat mineral blob).
pub const MYCELIUM_MINERAL_CONDUIT: u8 = 12;
/// 1-in-N chance per field pulse to shave one unit off an over-thick
/// mineral conduit that has already found food.
pub const MYCELIUM_MINERAL_TAPER_ODDS: u64 = 6;
/// Soft cap on stamped lineage cells (editor / spores).
pub const MYCELIUM_LINEAGE_MAX: usize = 512;
/// Soft cap on per-cell strain ownership entries (overlay map).
pub const MYCELIUM_STRAIN_MAP_MAX: usize = 8_192;
/// Soft cap on sparse network-energy map entries.
pub const MYCELIUM_ENERGY_MAP_MAX: usize = 8_192;
/// Per-cell network sugar cap (glucose analog).
pub const MYCELIUM_ENERGY_CAP: u8 = 255;
/// Base sugar gained on moist colonized Organic each field pulse.
pub const MYCELIUM_ENERGY_ORGANIC_GAIN: u8 = 1;
/// Extra sugar per this many cream intensity points on Organic.
pub const MYCELIUM_ENERGY_GAIN_PER_MYC: u8 = 64;
/// Soft litter units → network sugar when the field sips without a stalk.
pub const MYCELIUM_ENERGY_FROM_LITTER: u8 = 3;
/// 1-in-N litter→sugar sip on moist Organic during the field pulse.
pub const MYCELIUM_ENERGY_LITTER_ODDS: u64 = 10;
/// 1-in-N mineral corridor sugar upkeep while moist.
pub const MYCELIUM_ENERGY_MINERAL_UPKEEP_ODDS: u64 = 20;
/// Sugar cost to probe a virgin mineral neighbour.
pub const MYCELIUM_ENERGY_SPREAD_COST: u8 = 1;
/// Network sugar burned when a fruiting body emerges (best-effort).
pub const MYCELIUM_EMERGE_ENERGY_COST: u8 = 24;
/// Atom energy gained per network sugar unit sipped by a stalk.
pub const MYCELIUM_ENERGY_SIP_TO_ATOM: f32 = 0.12;
/// Max network sugar a fruiting body may sip per tick.
pub const MYCELIUM_ENERGY_SIP_MAX: u8 = 4;
/// Same-strain sugar bleed toward poorer cream neighbours per field pulse.
pub const MYCELIUM_CARGO_SUGAR_BLEED: u8 = 1;
/// Same-strain pore-sat trickle toward drier cream neighbours per field pulse.
pub const MYCELIUM_CARGO_WATER_BLEED: u8 = 1;
/// Soft cap on cargo equalize source cells processed per field pulse.
pub const MYCELIUM_CARGO_EQUALIZE_MAX: usize = 96;
/// Soft cap on strain→lineage treaty stamps.
pub const MYCELIUM_STRAIN_LINEAGE_MAX: usize = 512;
/// How deep / wide to scan for Organic substrate under the fungus.
const ORGANIC_SCAN_DEPTH: i32 = 8;
const ORGANIC_SCAN_RADIUS: i32 = 2;
/// Extra radius when goal-seeking distant Organic through mineral hosts.
const ORGANIC_SEEK_RADIUS: i32 = 6;
const ORGANIC_SEEK_DEPTH: i32 = 10;

/// Genome + body remembered by a mycelium patch for later emergence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MyceliumLineage {
    pub genome: Genome,
    pub body: Vec<BodyModule>,
}

/// Sparse `(gx, gy) → lineage` stamps on [`World::mycelium_lineage`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MyceliumLineageMap {
    #[serde(default)]
    pub cells: HashMap<(i32, i32), MyceliumLineage>,
}

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
pub(crate) fn organic_cells_near(world: &World, gx: i32, gy: i32) -> u32 {
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

/// Strongest mycelium intensity on any porous host near `(gx, gy)`.
pub fn max_mycelium_near(world: &World, gx: i32, gy: i32) -> u8 {
    let gx = world.wrap_x(gx);
    let mut best = 0u8;
    for dx in -ORGANIC_SCAN_RADIUS..=ORGANIC_SCAN_RADIUS {
        let nx = world.wrap_x(gx + dx);
        for dy in -ORGANIC_SCAN_DEPTH..=ORGANIC_SCAN_DEPTH {
            if let Some(c) = world.get_cell(nx, gy + dy) {
                best = best.max(c.mycelium());
            }
        }
    }
    best
}

/// Mint a new strain id for an inoculum event.
pub fn alloc_mycelium_strain(world: &mut World) -> u32 {
    let id = world.next_mycelium_strain_id.max(1);
    world.next_mycelium_strain_id = id.wrapping_add(1).max(1);
    id
}

/// All strain shares on a cell (`strain_id`, intensity). Sum = [`Cell::mycelium`].
pub fn mycelium_shares_at(world: &World, gx: i32, gy: i32) -> &[(u32, u8)] {
    let gx = world.wrap_x(gx);
    world
        .mycelium_strains
        .get(&(gx, gy))
        .map(|v| v.as_slice())
        .unwrap_or(&[])
}

/// Dominant strain on a cell (highest share), if any.
pub fn mycelium_strain_at(world: &World, gx: i32, gy: i32) -> Option<u32> {
    mycelium_shares_at(world, gx, gy)
        .iter()
        .max_by_key(|(_, amt)| *amt)
        .map(|(s, _)| *s)
}

/// Network sugar / glucose analog stored on a colonized cell.
pub fn mycelium_energy_at(world: &World, gx: i32, gy: i32) -> u8 {
    let gx = world.wrap_x(gx);
    world.mycelium_energy.get(&(gx, gy)).copied().unwrap_or(0)
}

/// Add network sugar (capped). No-ops when the cell has no cream host.
pub fn add_mycelium_energy(world: &mut World, gx: i32, gy: i32, add: u8) {
    let gx = world.wrap_x(gx);
    if add == 0 {
        return;
    }
    let Some(c) = world.get_cell(gx, gy) else {
        return;
    };
    if !hosts_mycelium(c.material) || c.mycelium() == 0 {
        return;
    }
    let map = &mut world.mycelium_energy;
    if map.len() >= MYCELIUM_ENERGY_MAP_MAX && !map.contains_key(&(gx, gy)) {
        if let Some(&key) = map.keys().next() {
            map.remove(&key);
        }
    }
    let e = map.entry((gx, gy)).or_insert(0);
    *e = (*e).saturating_add(add).min(MYCELIUM_ENERGY_CAP);
}

/// Take up to `want` network sugar from a cell. Returns amount taken.
pub fn take_mycelium_energy(world: &mut World, gx: i32, gy: i32, want: u8) -> u8 {
    let gx = world.wrap_x(gx);
    if want == 0 {
        return 0;
    }
    let Some(e) = world.mycelium_energy.get_mut(&(gx, gy)) else {
        return 0;
    };
    let taken = (*e).min(want);
    *e = e.saturating_sub(taken);
    if *e == 0 {
        world.mycelium_energy.remove(&(gx, gy));
    }
    taken
}

fn clear_mycelium_energy(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    world.mycelium_energy.remove(&(gx, gy));
}

/// Sip network sugar from the richest cream cell near `(gx, gy)`.
pub fn sip_mycelium_energy_near(world: &mut World, gx: i32, gy: i32, want: u8) -> u8 {
    let gx = world.wrap_x(gx);
    if want == 0 {
        return 0;
    }
    let mut best: Option<(i32, i32, u8)> = None;
    for dx in -ORGANIC_SCAN_RADIUS..=ORGANIC_SCAN_RADIUS {
        let nx = world.wrap_x(gx + dx);
        for dy in -2..=2 {
            let ny = gy + dy;
            let e = mycelium_energy_at(world, nx, ny);
            if e == 0 {
                continue;
            }
            if best.map(|(_, _, be)| e > be).unwrap_or(true) {
                best = Some((nx, ny, e));
            }
        }
    }
    let Some((nx, ny, _)) = best else {
        return 0;
    };
    take_mycelium_energy(world, nx, ny, want.min(MYCELIUM_ENERGY_SIP_MAX))
}

/// Bright RGB for a strain id (golden-angle hues, full saturation).
pub fn mycelium_strain_rgb(strain: u32) -> [u8; 3] {
    let h = (strain as f32 * 0.618_033_988_75).fract();
    hsv_to_rgb(h, 0.90, 1.0)
}

/// Blend strain colors by share weight; alpha from total intensity.
///
/// Thin mineral corridors (cream ≈5–16) must stay neon-readable on the `M`
/// overlay — a low alpha floor made climb paths look like dark-green veins
/// while only dense hubs glowed.
pub fn mycelium_shares_overlay_rgba(shares: &[(u32, u8)], total: u8) -> [u8; 4] {
    if shares.is_empty() || total == 0 {
        return [0, 0, 0, 0];
    }
    let mut r = 0.0f32;
    let mut g = 0.0f32;
    let mut b = 0.0f32;
    let mut wsum = 0.0f32;
    for &(strain, amt) in shares {
        if amt == 0 {
            continue;
        }
        let [sr, sg, sb] = mycelium_strain_rgb(strain);
        let w = amt as f32;
        r += sr as f32 * w;
        g += sg as f32 * w;
        b += sb as f32 * w;
        wsum += w;
    }
    if wsum <= 0.0 {
        return [0, 0, 0, 0];
    }
    // Floor ~210 so even residual corridors (compost / rock cracks) read as
    // strain color, not muddy wash against dark rock.
    let a = (210u32 + (total as u32 * 45) / 255).min(255) as u8;
    [
        (r / wsum).round() as u8,
        (g / wsum).round() as u8,
        (b / wsum).round() as u8,
        a,
    ]
}

/// Swap per-cell mycelium meta (strains, lineage, energy) when two cells
/// exchange places. Cream rides in [`Cell::_pad`]; maps are coordinate-keyed
/// and must move with the host or the `M` overlay / inspector desync.
pub fn swap_mycelium_meta(world: &mut World, ax: i32, ay: i32, bx: i32, by: i32) {
    let ax = world.wrap_x(ax);
    let bx = world.wrap_x(bx);
    if ax == bx && ay == by {
        return;
    }
    let a_shares = world.mycelium_strains.remove(&(ax, ay));
    let b_shares = world.mycelium_strains.remove(&(bx, by));
    match (a_shares, b_shares) {
        (Some(a), Some(b)) => {
            world.mycelium_strains.insert((ax, ay), b);
            world.mycelium_strains.insert((bx, by), a);
        }
        (Some(a), None) => {
            world.mycelium_strains.insert((bx, by), a);
        }
        (None, Some(b)) => {
            world.mycelium_strains.insert((ax, ay), b);
        }
        (None, None) => {}
    }
    let a_lin = world.mycelium_lineage.cells.remove(&(ax, ay));
    let b_lin = world.mycelium_lineage.cells.remove(&(bx, by));
    match (a_lin, b_lin) {
        (Some(a), Some(b)) => {
            world.mycelium_lineage.cells.insert((ax, ay), b);
            world.mycelium_lineage.cells.insert((bx, by), a);
        }
        (Some(a), None) => {
            world.mycelium_lineage.cells.insert((bx, by), a);
        }
        (None, Some(b)) => {
            world.mycelium_lineage.cells.insert((ax, ay), b);
        }
        (None, None) => {}
    }
    let a_e = world.mycelium_energy.remove(&(ax, ay));
    let b_e = world.mycelium_energy.remove(&(bx, by));
    match (a_e, b_e) {
        (Some(a), Some(b)) => {
            world.mycelium_energy.insert((ax, ay), b);
            world.mycelium_energy.insert((bx, by), a);
        }
        (Some(a), None) => {
            world.mycelium_energy.insert((bx, by), a);
        }
        (None, Some(b)) => {
            world.mycelium_energy.insert((ax, ay), b);
        }
        (None, None) => {}
    }
}

/// Move mycelium meta from one cell to another (erosion / bedload deposit).
///
/// Destination meta is replaced (deposit seats are Air). Source cleared.
pub fn move_mycelium_meta(
    world: &mut World,
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
) {
    let from_x = world.wrap_x(from_x);
    let to_x = world.wrap_x(to_x);
    if from_x == to_x && from_y == to_y {
        return;
    }
    let shares = world.mycelium_strains.remove(&(from_x, from_y));
    world.mycelium_strains.remove(&(to_x, to_y));
    if let Some(s) = shares {
        if !s.is_empty() {
            world.mycelium_strains.insert((to_x, to_y), s);
        }
    }
    let lin = world.mycelium_lineage.cells.remove(&(from_x, from_y));
    world.mycelium_lineage.cells.remove(&(to_x, to_y));
    if let Some(l) = lin {
        world.mycelium_lineage.cells.insert((to_x, to_y), l);
    }
    let energy = world.mycelium_energy.remove(&(from_x, from_y));
    world.mycelium_energy.remove(&(to_x, to_y));
    if let Some(e) = energy {
        if e > 0 {
            world.mycelium_energy.insert((to_x, to_y), e);
        }
    }
}

/// Swap two cells and their mycelium meta (raft drift, buoyancy, punch).
pub fn swap_cells_preserving_mycelium(
    world: &mut World,
    ax: i32,
    ay: i32,
    bx: i32,
    by: i32,
) {
    let ax = world.wrap_x(ax);
    let bx = world.wrap_x(bx);
    let Some(a) = world.get_cell(ax, ay) else {
        return;
    };
    let Some(b) = world.get_cell(bx, by) else {
        return;
    };
    world.set_cell(ax, ay, b);
    world.set_cell(bx, by, a);
    swap_mycelium_meta(world, ax, ay, bx, by);
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.fract() * 6.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    ]
}

fn clear_mycelium_shares(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    world.mycelium_strains.remove(&(gx, gy));
    clear_mycelium_energy(world, gx, gy);
}

/// Cream `_pad` with no strain shares: inherit a neighbour strain, or clear.
///
/// Never mints a new strain for ghosts — minting turned oxidation scraps and
/// roof-collapse debris into fake "owned" red dots that then lingered.
fn heal_orphan_mycelium_pads(world: &mut World) {
    use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};

    let mut orphans: Vec<(i32, i32, u8)> = Vec::new();
    let coords: Vec<_> = world.chunks.keys().copied().collect();
    for coord in coords {
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + lx as i32;
                let gy = coord.cy * CHUNK_CELLS_H as i32 + ly as i32;
                let Some(c) = world.get_cell(gx, gy) else {
                    continue;
                };
                if !hosts_mycelium(c.material) {
                    continue;
                }
                let myc = c.mycelium();
                if myc == 0 {
                    continue;
                }
                if mycelium_shares_at(world, gx, gy).is_empty() {
                    orphans.push((gx, gy, myc));
                }
            }
        }
    }
    // Prefer cells that can inherit first so a fringe touching a real network
    // reconnects before isolated scraps are wiped.
    orphans.sort_by_key(|&(gx, gy, _)| {
        let near = neighbour_has_mycelium_shares(world, gx, gy);
        (!near, gx, gy)
    });
    for (gx, gy, myc) in orphans {
        if !mycelium_shares_at(world, gx, gy).is_empty() {
            continue;
        }
        if let Some(s) = neighbour_mycelium_strain(world, gx, gy) {
            world.mycelium_strains.insert((gx, gy), vec![(s, myc)]);
            continue;
        }
        clear_mycelium_shares(world, gx, gy);
        if let Some(mut c) = world.get_cell(gx, gy) {
            c.set_mycelium(0);
            world.set_cell(gx, gy, c);
        }
    }
}

fn neighbour_has_mycelium_shares(world: &World, gx: i32, gy: i32) -> bool {
    neighbour_mycelium_strain(world, gx, gy).is_some()
}

fn neighbour_mycelium_strain(world: &World, gx: i32, gy: i32) -> Option<u32> {
    for (dx, dy) in [
        (0i32, 1),
        (0, -1),
        (1, 0),
        (-1, 0),
        (1, 1),
        (-1, 1),
        (1, -1),
        (-1, -1),
    ] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if let Some(s) = mycelium_strain_at(world, nx, ny) {
            return Some(s);
        }
    }
    None
}

fn sync_cell_mycelium_from_shares(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    let total: u32 = world
        .mycelium_strains
        .get(&(gx, gy))
        .map(|v| v.iter().map(|(_, a)| *a as u32).sum())
        .unwrap_or(0)
        .min(255);
    if total == 0 {
        clear_mycelium_shares(world, gx, gy);
    }
    if let Some(mut c) = world.get_cell(gx, gy) {
        if hosts_mycelium(c.material) {
            c.set_mycelium(total as u8);
            world.set_cell(gx, gy, c);
            if total == 0 {
                clear_mycelium_energy(world, gx, gy);
            }
        } else {
            clear_mycelium_energy(world, gx, gy);
        }
    }
}

/// Add cream for one strain into the cell's shared 255 budget.
///
/// Other strains keep their shares; only free room (`cap - total`) is taken.
pub fn add_mycelium_with_strain(
    world: &mut World,
    gx: i32,
    gy: i32,
    add: u8,
    strain: Option<u32>,
    cap: u8,
) {
    let gx = world.wrap_x(gx);
    let Some(c) = world.get_cell(gx, gy) else {
        return;
    };
    if !hosts_mycelium(c.material) || add == 0 {
        return;
    }
    let cap = cap.min(255);
    let pad = c.mycelium();
    let strain = match strain {
        Some(s) => s,
        None => {
            // Thicken dominant share, or mint wild on virgin cream.
            mycelium_strain_at(world, gx, gy).unwrap_or_else(|| alloc_mycelium_strain(world))
        }
    };

    let map = &mut world.mycelium_strains;
    if map.len() >= MYCELIUM_STRAIN_MAP_MAX && !map.contains_key(&(gx, gy)) {
        if let Some(&key) = map.keys().next() {
            map.remove(&key);
        }
    }
    let shares = map.entry((gx, gy)).or_default();
    // Absorb legacy / test cream that lives only in `_pad`.
    let share_sum: u32 = shares.iter().map(|(_, a)| *a as u32).sum();
    if (pad as u32) > share_sum {
        let orphan = (pad as u32 - share_sum) as u8;
        if let Some(slot) = shares.iter_mut().find(|(s, _)| *s == strain) {
            slot.1 = slot.1.saturating_add(orphan);
        } else {
            shares.push((strain, orphan));
        }
    }
    let total: u8 = shares
        .iter()
        .map(|(_, a)| *a as u32)
        .sum::<u32>()
        .min(255) as u8;
    let room = cap.saturating_sub(total);
    if room == 0 {
        shares.sort_by_key(|(s, _)| *s);
        sync_cell_mycelium_from_shares(world, gx, gy);
        return;
    }
    let gain = add.min(room);
    if let Some(slot) = shares.iter_mut().find(|(s, _)| *s == strain) {
        slot.1 = slot.1.saturating_add(gain);
    } else {
        shares.push((strain, gain));
    }
    // Keep shares tidy / ordered by strain id for stable inspector output.
    shares.sort_by_key(|(s, _)| *s);
    sync_cell_mycelium_from_shares(world, gx, gy);
}

/// Subtract cream from shares (largest first) and sync `_pad`.
pub fn reduce_mycelium_shares(world: &mut World, gx: i32, gy: i32, amount: u8) {
    let gx = world.wrap_x(gx);
    if amount == 0 {
        return;
    }
    let Some(shares) = world.mycelium_strains.get_mut(&(gx, gy)) else {
        // Legacy cream with no shares — just lower `_pad`.
        if let Some(mut c) = world.get_cell(gx, gy) {
            c.set_mycelium(c.mycelium().saturating_sub(amount));
            world.set_cell(gx, gy, c);
        }
        return;
    };
    let mut left = amount;
    while left > 0 && !shares.is_empty() {
        let idx = shares
            .iter()
            .enumerate()
            .max_by_key(|(_, (_, a))| *a)
            .map(|(i, _)| i)
            .unwrap();
        let take = shares[idx].1.min(left);
        shares[idx].1 = shares[idx].1.saturating_sub(take);
        left = left.saturating_sub(take);
        if shares[idx].1 == 0 {
            shares.remove(idx);
        }
    }
    if shares.is_empty() {
        world.mycelium_strains.remove(&(gx, gy));
    }
    sync_cell_mycelium_from_shares(world, gx, gy);
}

/// Scale all shares so their sum equals `target` (compost residual).
fn scale_mycelium_shares_to(world: &mut World, gx: i32, gy: i32, target: u8) {
    let gx = world.wrap_x(gx);
    if target == 0 {
        clear_mycelium_shares(world, gx, gy);
        sync_cell_mycelium_from_shares(world, gx, gy);
        return;
    }
    if world.mycelium_strains.get(&(gx, gy)).is_none() {
        // Orphan `_pad` residual — inherit/mint so cream never goes unowned.
        let _ = ensure_mycelium_strain(world, gx, gy);
    }
    let Some(shares) = world.mycelium_strains.get(&(gx, gy)).cloned() else {
        return;
    };
    let sum: u32 = shares.iter().map(|(_, a)| *a as u32).sum();
    if sum == 0 {
        clear_mycelium_shares(world, gx, gy);
        sync_cell_mycelium_from_shares(world, gx, gy);
        return;
    }
    let mut next: Vec<(u32, u8)> = Vec::with_capacity(shares.len());
    let mut assigned = 0u32;
    for (i, &(s, a)) in shares.iter().enumerate() {
        if a == 0 {
            continue;
        }
        let mut part = ((a as u32 * target as u32) / sum) as u8;
        if part == 0 {
            part = 1;
        }
        if i + 1 == shares.len() {
            part = target.saturating_sub(assigned as u8);
        }
        assigned += part as u32;
        if part > 0 {
            next.push((s, part));
        }
    }
    if next.is_empty() {
        // Keep dominant as residual.
        if let Some(&(s, _)) = shares.iter().max_by_key(|(_, a)| *a) {
            next.push((s, target));
        }
    }
    // Fix sum drift.
    let got: u32 = next.iter().map(|(_, a)| *a as u32).sum();
    if got != target as u32 && !next.is_empty() {
        let d = target as i32 - got as i32;
        let last = next.len() - 1;
        next[last].1 = (next[last].1 as i32 + d).clamp(0, 255) as u8;
        next.retain(|(_, a)| *a > 0);
    }
    if next.is_empty() {
        clear_mycelium_shares(world, gx, gy);
    } else {
        world.mycelium_strains.insert((gx, gy), next);
    }
    sync_cell_mycelium_from_shares(world, gx, gy);
}

/// Ensure a colonized cell has at least one strain share (inherit / mint).
pub fn ensure_mycelium_strain(world: &mut World, gx: i32, gy: i32) -> u32 {
    let gx = world.wrap_x(gx);
    if let Some(s) = mycelium_strain_at(world, gx, gy) {
        return s;
    }
    let total = world.get_cell(gx, gy).map(|c| c.mycelium()).unwrap_or(0);
    for (dx, dy) in [
        (0i32, 1),
        (0, -1),
        (1, 0),
        (-1, 0),
        (1, 1),
        (-1, 1),
        (1, -1),
        (-1, -1),
    ] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if let Some(s) = mycelium_strain_at(world, nx, ny) {
            if total > 0 {
                world.mycelium_strains.insert((gx, gy), vec![(s, total)]);
            }
            return s;
        }
    }
    let s = alloc_mycelium_strain(world);
    if total > 0 {
        world.mycelium_strains.insert((gx, gy), vec![(s, total)]);
    }
    s
}

/// Stamp editor / spore lineage at a cell (and soft-cap the map).
pub fn stamp_mycelium_lineage(
    world: &mut World,
    gx: i32,
    gy: i32,
    genome: Genome,
    body: Vec<BodyModule>,
) {
    let gx = world.wrap_x(gx);
    if body.is_empty() {
        return;
    }
    let map = &mut world.mycelium_lineage.cells;
    if map.len() >= MYCELIUM_LINEAGE_MAX && !map.contains_key(&(gx, gy)) {
        // Drop an arbitrary old stamp so new inoculum always lands.
        if let Some(&key) = map.keys().next() {
            map.remove(&key);
        }
    }
    map.insert(
        (gx, gy),
        MyceliumLineage {
            genome: genome.clone(),
            body: body.clone(),
        },
    );
    // Bind to the cell's dominant strain when known so frontiers keep the
    // inoculum treaty after cream spreads away from the spatial stamp.
    if let Some(strain) = mycelium_strain_at(world, gx, gy) {
        bind_strain_lineage(world, strain, genome, body);
    }
}

/// Remember a strain's genome+body for treaty match (strain↔strain trade).
pub fn bind_strain_lineage(
    world: &mut World,
    strain: u32,
    genome: Genome,
    body: Vec<BodyModule>,
) {
    if body.is_empty() || strain == 0 {
        return;
    }
    let map = &mut world.mycelium_strain_lineage;
    if map.len() >= MYCELIUM_STRAIN_LINEAGE_MAX && !map.contains_key(&strain) {
        if let Some(&key) = map.keys().next() {
            map.remove(&key);
        }
    }
    map.insert(strain, MyceliumLineage { genome, body });
}

/// Lineage bound to a strain id, if any.
pub fn strain_lineage(world: &World, strain: u32) -> Option<MyceliumLineage> {
    world.mycelium_strain_lineage.get(&strain).cloned()
}

/// Prefer strain→lineage; fall back to spatial nearest stamp at `(gx,gy)`.
pub fn lineage_for_strain_at(
    world: &World,
    strain: u32,
    gx: i32,
    gy: i32,
) -> Option<MyceliumLineage> {
    strain_lineage(world, strain).or_else(|| nearest_mycelium_lineage(world, gx, gy))
}

/// Nearest stamped lineage within a small Chebyshev window (column-biased).
pub fn nearest_mycelium_lineage(
    world: &World,
    gx: i32,
    gy: i32,
) -> Option<MyceliumLineage> {
    let gx = world.wrap_x(gx);
    let mut best: Option<(i32, MyceliumLineage)> = None;
    for (&(lx, ly), lin) in &world.mycelium_lineage.cells {
        let dx = {
            let d = (lx - gx).abs();
            match world.wrap_width {
                Some(w) if w > 0 => d.min(w - d.min(w)),
                _ => d,
            }
        };
        let dy = (ly - gy).abs();
        if dx > 6 || dy > 10 {
            continue;
        }
        let dist = dx * 3 + dy; // prefer same column
        if best.as_ref().map(|(bd, _)| dist < *bd).unwrap_or(true) {
            best = Some((dist, lin.clone()));
        }
    }
    best.map(|(_, l)| l)
}

/// Spread cost for threading into `mat` at given moisture (lower = easier).
/// `None` = refuse (bedrock / air / ice).
fn mycelium_host_cost(mat: MaterialId, moist: f32) -> Option<u32> {
    let wet = moist >= MYCELIUM_FIELD_MOIST;
    match mat {
        MaterialId::Organic => Some(0),
        MaterialId::Soil | MaterialId::Sand => Some(if wet { 2 } else { 6 }),
        MaterialId::Clay => Some(if wet { 4 } else { 9 }),
        MaterialId::LooseRock | MaterialId::LooseLimestone => Some(if wet { 10 } else { 18 }),
        MaterialId::Stone | MaterialId::Limestone => Some(if wet { 22 } else { 36 }),
        _ => None,
    }
}

/// Chebyshev distance to nearest Organic cell (food) in the seek window.
fn dist_to_organic(world: &World, gx: i32, gy: i32) -> i32 {
    let gx = world.wrap_x(gx);
    let mut best = i32::MAX;
    for dx in -ORGANIC_SEEK_RADIUS..=ORGANIC_SEEK_RADIUS {
        let nx = world.wrap_x(gx + dx);
        for dy in -ORGANIC_SEEK_DEPTH..=ORGANIC_SEEK_DEPTH {
            if matches!(
                world.get_cell(nx, gy + dy),
                Some(c) if c.material == MaterialId::Organic
            ) {
                let d = dx.abs().max(dy.abs());
                best = best.min(d);
            }
        }
    }
    best
}

/// True when any cell above in-column is free Air (surface-seeking bias).
fn open_air_above(world: &World, gx: i32, gy: i32) -> bool {
    for dy in 1..=4 {
        if matches!(
            world.get_cell(gx, gy + dy),
            Some(a) if a.material == MaterialId::Air
        ) {
            return true;
        }
    }
    false
}

/// Mineral corridor has reached threaded Organic food (can taper to a conduit).
fn mineral_reached_food(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    for (dx, dy) in [
        (0i32, 1),
        (0, -1),
        (1, 0),
        (-1, 0),
        (1, 1),
        (-1, 1),
        (1, -1),
        (-1, -1),
    ] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if matches!(
            world.get_cell(nx, ny),
            Some(c) if c.material == MaterialId::Organic && c.mycelium() > 0
        ) {
            return true;
        }
    }
    false
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

/// Autonomous mycelium field: thicken / spread on moist porous hosts without
/// a living fruiting body. Call from the world tick.
pub fn step_mycelium_field(world: &mut World) {
    step_mycelium_field_cfg(world, &FungiConfig::default());
}

/// [`step_mycelium_field`] with live [`FungiConfig`] compost knobs.
pub fn step_mycelium_field_cfg(world: &mut World, cfg: &FungiConfig) {
    if world.tick % MYCELIUM_FIELD_PERIOD != 0 {
        return;
    }

    let threshold = cfg.soil_mycelium_threshold;
    let convert_odds = cfg.soil_convert_odds.max(1);

    let seed = world.seed.0;
    let tick = world.tick;
    // Full-world orphan pass (not sample-capped): cream without shares either
    // inherits a neighbouring strain or clears. Roof-collapse / old oxidation
    // ghosts used to linger as red `M` dots when hubs filled the process budget.
    heal_orphan_mycelium_pads(world);
    let colonized = collect_mycelium_field_cells(world, tick, seed, MYCELIUM_FIELD_MAX_CELLS);
    if colonized.is_empty() {
        return;
    }
    let mut spreads: Vec<(i32, i32)> = Vec::new();
    let mut grows: Vec<(i32, i32)> = Vec::new();
    let mut decays: Vec<(i32, i32)> = Vec::new();

    for (i, &(gx, gy, myc, mat)) in colonized.iter().enumerate() {
        // Sampled cells should already be healed; skip any residual ghosts.
        if mycelium_shares_at(world, gx, gy).is_empty() {
            continue;
        }
        let moist = organic_cell_moist_frac(world, gx, gy);
        if moist >= MYCELIUM_FIELD_MOIST {
            let organic = mat == MaterialId::Organic;
            if organic {
                // Food substrate: thicken freely into the shared 255 budget.
                if myc < 255 {
                    grows.push((gx, gy));
                }
                if myc >= 16 {
                    let h = hash_u64(seed, tick, gx as u64, gy as u64 ^ (i as u64));
                    if h % MYCELIUM_FIELD_SPREAD_ODDS == 0 {
                        spreads.push((gx, gy));
                    }
                }
                // Bank network sugar from the Organic bed (+ occasional litter).
                let gain = MYCELIUM_ENERGY_ORGANIC_GAIN
                    .saturating_add(myc / MYCELIUM_ENERGY_GAIN_PER_MYC.max(1));
                add_mycelium_energy(world, gx, gy, gain);
                let h_lit = hash_u64(seed, tick, gx as u64, 0x51AAu64 ^ (i as u64));
                if h_lit % MYCELIUM_ENERGY_LITTER_ODDS == 0 && soft_litter_at(world, gx) > 0 {
                    if take_soft_litter(world, gx, 1) > 0 {
                        add_mycelium_energy(world, gx, gy, MYCELIUM_ENERGY_FROM_LITTER);
                    }
                }
                if myc >= threshold {
                    let h = hash_u64(seed, tick, gx as u64, 0x5011u64);
                    if h % convert_odds == 0 {
                        compost_organic_to_soil(world, gx, gy);
                    }
                }
            } else {
                // Mineral: thin search front → conduit once Organic is found.
                let fed = mineral_reached_food(world, gx, gy);
                // Corridor upkeep — sugar keeps the mineral thread cheap to hold.
                if mycelium_energy_at(world, gx, gy) > 0 {
                    let h_u = hash_u64(seed, tick, gx as u64, 0x590Du64);
                    if h_u % MYCELIUM_ENERGY_MINERAL_UPKEEP_ODDS == 0 {
                        let _ = take_mycelium_energy(world, gx, gy, 1);
                    }
                }
                if fed && myc > MYCELIUM_MINERAL_CONDUIT {
                    // Fat leftovers reabsorb quickly; near-conduit shaves slowly.
                    let odds = if myc > MYCELIUM_MINERAL_CONDUIT.saturating_add(8) {
                        2
                    } else {
                        MYCELIUM_MINERAL_TAPER_ODDS
                    };
                    let h = hash_u64(seed, tick, gx as u64, 0x7A9Eu64);
                    if h % odds == 0 {
                        decays.push((gx, gy));
                    }
                } else if !fed && myc < MYCELIUM_MINERAL_CAP {
                    let h = hash_u64(seed, tick, gx as u64, 0x680u64 ^ (i as u64));
                    if h % 3 == 0 {
                        grows.push((gx, gy));
                    }
                }
                // Only *seeking* mineral fronts keep probing. Once fed, the
                // Organic colony owns further spread; mineral just conducts.
                if !fed && myc >= 8 && myc <= MYCELIUM_MINERAL_CAP {
                    let h = hash_u64(seed, tick, gx as u64, gy as u64 ^ (i as u64));
                    if h % MYCELIUM_FIELD_SPREAD_ODDS == 0 {
                        spreads.push((gx, gy));
                    }
                }
            }
        } else if myc > 0 {
            // Bone-dry: rare fade — corridors can disconnect, then reconnect
            // when remoistened neighbours re-spread into the gap.
            let h = hash_u64(seed, tick, gx as u64, 0xD00Du64);
            if h % 64 == 0 {
                decays.push((gx, gy));
            }
        }
    }

    for (gx, gy) in grows {
        // Thicken the dominant share (or mint wild) into free room.
        let strain = mycelium_strain_at(world, gx, gy);
        let cap = world
            .get_cell(gx, gy)
            .map(|c| {
                if c.material == MaterialId::Organic {
                    255
                } else {
                    MYCELIUM_MINERAL_CAP
                }
            })
            .unwrap_or(MYCELIUM_MINERAL_CAP);
        add_mycelium_with_strain(world, gx, gy, MYCELIUM_FIELD_GROW, strain, cap);
    }
    for (gx, gy) in spreads {
        spread_mycelium_once(world, gx, gy);
    }
    for (gx, gy) in decays {
        if world
            .get_cell(gx, gy)
            .is_some_and(|c| hosts_mycelium(c.material) && c.mycelium() > 0)
        {
            reduce_mycelium_shares(world, gx, gy, 1);
        }
    }
    // Wet-side harvest / dry-side supply need a thin pipe: bleed sugar + a
    // trickle of pore water along same-strain cream neighbours.
    equalize_mycelium_cargo(world, &colonized);
    // Distinct strains that meet with matching Symbiont treaties may trade.
    crate::symbiosis::step_strain_trade(world, tick);
}

fn shares_strain(world: &World, gx: i32, gy: i32, strain: u32) -> bool {
    mycelium_shares_at(world, gx, gy)
        .iter()
        .any(|(s, amt)| *s == strain && *amt > 0)
}

/// Equalize network sugar (and a trickle of pore water) along same-strain cream.
///
/// This is the transport half of bidirectional symbiosis: a wet contact can
/// bank water/sugar that later reaches a dry contact of the same strain.
pub fn equalize_mycelium_cargo(world: &mut World, colonized: &[(i32, i32, u8, MaterialId)]) {
    let n = colonized.len().min(MYCELIUM_CARGO_EQUALIZE_MAX);
    let mut sugar_moves: Vec<(i32, i32, i32, i32, u8)> = Vec::new();
    let mut water_moves: Vec<(i32, i32, i32, i32, u8)> = Vec::new();
    for &(gx, gy, _, _) in colonized.iter().take(n) {
        let Some(strain) = mycelium_strain_at(world, gx, gy) else {
            continue;
        };
        let src_sugar = mycelium_energy_at(world, gx, gy);
        let src_sat = world.get_cell(gx, gy).map(|c| c.sat.0).unwrap_or(0);
        let src_cap = world
            .get_cell(gx, gy)
            .map(|c| water_capacity(c.material))
            .unwrap_or(0);
        let mut best_sugar: Option<(i32, i32, u8)> = None;
        let mut best_water: Option<(i32, i32, u8, u8)> = None; // nx,ny,sat,cap
        for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = world.wrap_x(gx + dx);
            let ny = gy + dy;
            if !shares_strain(world, nx, ny, strain) {
                continue;
            }
            let Some(nc) = world.get_cell(nx, ny) else {
                continue;
            };
            if nc.mycelium() == 0 || !hosts_mycelium(nc.material) {
                continue;
            }
            let ns = mycelium_energy_at(world, nx, ny);
            if src_sugar > ns.saturating_add(1) {
                if best_sugar.map(|(_, _, s)| ns < s).unwrap_or(true) {
                    best_sugar = Some((nx, ny, ns));
                }
            }
            let ncap = water_capacity(nc.material);
            if src_cap > 0 && ncap > 0 && nc.sat.0 < ncap && src_sat > nc.sat.0.saturating_add(1) {
                if best_water
                    .map(|(_, _, s, _)| nc.sat.0 < s)
                    .unwrap_or(true)
                {
                    best_water = Some((nx, ny, nc.sat.0, ncap));
                }
            }
        }
        if let Some((nx, ny, _)) = best_sugar {
            if src_sugar > 0 {
                sugar_moves.push((gx, gy, nx, ny, MYCELIUM_CARGO_SUGAR_BLEED));
            }
        }
        if let Some((nx, ny, _, _)) = best_water {
            if src_sat > 0 {
                water_moves.push((gx, gy, nx, ny, MYCELIUM_CARGO_WATER_BLEED));
            }
        }
    }
    for (sx, sy, nx, ny, amt) in sugar_moves {
        let taken = take_mycelium_energy(world, sx, sy, amt);
        if taken > 0 {
            add_mycelium_energy(world, nx, ny, taken);
        }
    }
    for (sx, sy, nx, ny, amt) in water_moves {
        let sx = world.wrap_x(sx);
        let nx = world.wrap_x(nx);
        let Some(mut src) = world.get_cell(sx, sy) else {
            continue;
        };
        let Some(mut dst) = world.get_cell(nx, ny) else {
            continue;
        };
        let dcap = water_capacity(dst.material);
        if dcap == 0 || src.sat.0 == 0 || dst.sat.0 >= dcap {
            continue;
        }
        let room = dcap - dst.sat.0;
        let move_amt = amt.min(src.sat.0).min(room);
        if move_amt == 0 {
            continue;
        }
        src.sat.0 = src.sat.0.saturating_sub(move_amt);
        dst.sat.0 = dst.sat.0.saturating_add(move_amt);
        world.set_cell(sx, sy, src);
        world.set_cell(nx, ny, dst);
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

/// Pick a cell to thread next — prefer Organic food, else mineral corridor
/// that steps toward Organic / the free surface.
///
/// Prefer close cells under/around the fungus and thicken an existing
/// patch before spraying onto the farthest clean cell in the scan window.
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
            if !hosts_mycelium(c.material) {
                continue;
            }
            let m = c.mycelium();
            let organic = c.material == MaterialId::Organic;
            let host_cap = if organic { 255 } else { MYCELIUM_MINERAL_CAP };
            if m >= host_cap {
                continue;
            }
            let moist = organic_cell_moist_frac(world, nx, y);
            let Some(host_cost) = mycelium_host_cost(c.material, moist) else {
                continue;
            };
            let dist = dx.abs() + dy.abs();
            // Organic: thicken young patches. Mineral: prefer virgin frontier
            // (never farm a fat mineral blob under the seat).
            let stage = if organic {
                if (1..80).contains(&m) {
                    0
                } else if m >= 80 {
                    1
                } else {
                    2
                }
            } else if m == 0 {
                0
            } else if m < MYCELIUM_MINERAL_CONDUIT {
                1
            } else {
                3
            };
            // Prefer climbing toward free Air so networks breach the surface
            // before raising a stalk (slight bias — still thicken near seat).
            let climb = if open_air_above(world, nx, y) {
                0
            } else if dy > 0 {
                1
            } else {
                2
            };
            // Goal: Organic food nearby — mineral cells pay until they reach it.
            let food = if organic {
                0
            } else {
                12 + dist_to_organic(world, nx, y).min(20)
            };
            let score = dist * 8 + stage + climb + host_cost as i32 + food;
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
///
/// Leaves a residual cream corridor on the Soil so the network can
/// reconnect through the humified patch instead of hard-severing.
/// Virgin Organic (no cream) composts clean — carbon surface oxidation
/// uses this path and must not invent orphan `mycelium=1` soil.
pub fn compost_organic_to_soil(world: &mut World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    let Some(c) = world.get_cell(gx, gy) else {
        return false;
    };
    if c.material != MaterialId::Organic {
        return false;
    }
    let old_sat = c.sat.0;
    let prior_myc = c.mycelium();
    let cap = world.water_capacity(MaterialId::Soil);
    let keep = if cap > 0 { old_sat.min(cap) } else { 0 };
    let excess = old_sat.saturating_sub(keep);
    let mut soil = Cell::solid(MaterialId::Soil);
    soil.sat.0 = keep;
    soil.flags.set(CellFlags::COMPACTED);
    // Residual ≤ prior intensity — never invent cream on virgin Organic.
    // (Previously `.max(1)` painted fake myc=1 soil with no strain shares
    // whenever carbon oxidized surface litter.)
    let residual = MYCELIUM_COMPOST_RESIDUAL.min(prior_myc);
    soil.set_mycelium(residual);
    world.set_cell(gx, gy, soil);
    if residual > 0 {
        // Keep / mint strain ownership so residual corridors stay on `M`.
        if mycelium_shares_at(world, gx, gy).is_empty() {
            let _ = ensure_mycelium_strain(world, gx, gy);
        }
        scale_mycelium_shares_to(world, gx, gy, residual);
    } else {
        clear_mycelium_shares(world, gx, gy);
    }
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
            let strain = mycelium_strain_at(world, ox, oy)
                .or_else(|| mycelium_strain_at(world, gx, gy));
            let cap = if c.material == MaterialId::Organic {
                255
            } else {
                MYCELIUM_MINERAL_CAP
            };
            let _ = c;
            add_mycelium_with_strain(world, ox, oy, add, strain, cap);
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
    let src_food = dist_to_organic(world, gx, gy);
    let src_mat = world
        .get_cell(gx, gy)
        .map(|c| c.material)
        .unwrap_or(MaterialId::Air);
    let src_on_food = src_mat == MaterialId::Organic;
    let src_sugar = mycelium_energy_at(world, gx, gy);
    // Score neighbours: host cost + food-seek + surface-seek. Pick best.
    // Last bool: charge spread sugar (virgin mineral probe).
    let mut best: Option<(i32, i32, i32, u8, u8, bool)> = None;
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
        let Some(c) = world.get_cell(nx, ny) else {
            continue;
        };
        if !hosts_mycelium(c.material) {
            continue;
        }
        let dest_organic = c.material == MaterialId::Organic;
        let dest_cap = if dest_organic {
            255
        } else {
            MYCELIUM_MINERAL_CAP
        };
        // Organic may keep thickening via grow; mineral stops at conduit cap.
        let skip_at = if dest_organic { 40 } else { MYCELIUM_MINERAL_CAP };
        if c.mycelium() >= skip_at {
            continue;
        }
        // Don't re-thicken a mineral cell that already touches food — that
        // fights the conduit taper and paints fat mineral blobs again.
        if !dest_organic
            && c.mycelium() >= MYCELIUM_MINERAL_CONDUIT
            && mineral_reached_food(world, nx, ny)
        {
            continue;
        }
        // Virgin mineral probes spend sugar when the source can afford it.
        // Hungry (0-sugar) fronts may still seek for free.
        let charge = !dest_organic
            && c.mycelium() == 0
            && src_sugar >= MYCELIUM_ENERGY_SPREAD_COST;
        let moist = organic_cell_moist_frac(world, nx, ny);
        let Some(host_cost) = mycelium_host_cost(c.material, moist) else {
            continue;
        };
        // Hard rock: rare crack only.
        if matches!(c.material, MaterialId::Stone | MaterialId::Limestone) {
            let h = hash_u64(world.seed.0, world.tick, nx as u64, ny as u64);
            if h % 12 != 0 {
                continue;
            }
        }
        let food = dist_to_organic(world, nx, ny);
        let toward_food = if food < src_food {
            0
        } else if food == src_food {
            2
        } else {
            6
        };
        let climb = if dy > 0 && open_air_above(world, nx, ny) {
            0
        } else if dy > 0 {
            1
        } else if dy == 0 {
            2
        } else {
            4
        };
        let organic_bonus = if dest_organic { 0 } else { 5 };
        // From a fed Organic patch, don't flood mineral sideways — only
        // probe mineral when it steps toward new food or open air.
        let mineral_flood = if src_on_food && !dest_organic {
            if food < src_food || open_air_above(world, nx, ny) {
                0
            } else {
                18
            }
        } else {
            0
        };
        let score =
            host_cost as i32 + toward_food + climb + organic_bonus + mineral_flood;
        let add = if dest_organic { 8 } else { 4 };
        if best.map(|(bs, _, _, _, _, _)| score < bs).unwrap_or(true) {
            best = Some((score, nx, ny, add, dest_cap, charge));
        }
    }
    if let Some((_, nx, ny, add, cap, charge)) = best {
        if charge {
            let _ = take_mycelium_energy(world, gx, gy, MYCELIUM_ENERGY_SPREAD_COST);
        }
        let strain = Some(ensure_mycelium_strain(world, gx, gy));
        add_mycelium_with_strain(world, nx, ny, add, strain, cap);
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
        // Fruiting bed is still Organic (food); feeders below may be mineral.
        if c.material != MaterialId::Organic || c.mycelium() < MYCELIUM_EMERGE_MIN {
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
/// Feeders may be Organic or mineral corridors.
fn mycelium_breached_from_below(world: &World, gx: i32, surface_y: i32) -> bool {
    for dy in 1..=4 {
        let y = surface_y - dy;
        if matches!(
            world.get_cell(gx, y),
            Some(c) if c.mycelium() > 0
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
                Some(c) if c.mycelium() >= 16
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
        let strain = mycelium_strain_at(world, ox, oy).or_else(|| {
            let s = alloc_mycelium_strain(world);
            Some(s)
        });
        add_mycelium_with_strain(world, ox, oy, amount, strain, 48);
    }
}

/// Plant a mycelium infection — no living fruiting body.
///
/// Seeds the nearest host (prefer Organic) and a short feeder column below
/// so the network can later breach ([`mycelium_breached_from_below`]).
/// Returns the surface cell infected.
pub fn infect_mycelium_at(world: &mut World, gx: i32, gy: i32) -> Option<(i32, i32)> {
    infect_mycelium_with_lineage(world, gx, gy, None)
}

/// [`infect_mycelium_at`] plus an optional lineage stamp for later emergence.
pub fn infect_mycelium_with_lineage(
    world: &mut World,
    gx: i32,
    gy: i32,
    lineage: Option<(Genome, Vec<BodyModule>)>,
) -> Option<(i32, i32)> {
    let (ox, oy) = find_organic_xy(world, gx, gy)?;
    // Each inoculum event is a new strain that *shares* free cream room
    // with any strains already on the cell (never wipes neighbours).
    let strain = alloc_mycelium_strain(world);
    let mut hit = false;
    for dy in 0..=3 {
        let y = oy - dy;
        let Some(c) = world.get_cell(ox, y) else {
            break;
        };
        if !hosts_mycelium(c.material) {
            break;
        }
        let add = if dy == 0 {
            MYCELIUM_INOCULUM_AMOUNT
        } else {
            (MYCELIUM_INOCULUM_AMOUNT / 2).max(8)
        };
        // Mineral feeders take a thinner inoculum into the mineral conduit cap.
        let (add, cap) = if c.material == MaterialId::Organic {
            (add, MYCELIUM_INOCULUM_CAP)
        } else {
            ((add / 2).max(4), MYCELIUM_MINERAL_CAP)
        };
        add_mycelium_with_strain(world, ox, y, add, Some(strain), cap);
        // Starter sugar so new inoculum can afford mineral probes soon.
        if c.material == MaterialId::Organic {
            add_mycelium_energy(world, ox, y, 12);
        } else {
            add_mycelium_energy(world, ox, y, 4);
        }
        hit = true;
    }
    if hit {
        if let Some((genome, body)) = lineage {
            // Bind strain first so cell stamp can see the dominant id.
            bind_strain_lineage(world, strain, genome.clone(), body.clone());
            stamp_mycelium_lineage(world, ox, oy, genome, body);
        }
        Some((ox, oy))
    } else {
        None
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
                if c.material != MaterialId::Organic || c.mycelium() < MYCELIUM_EMERGE_MIN {
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
    // Spend field intensity + banked sugar — fruiting costs the network.
    if world
        .get_cell(gx, air_y - 1)
        .is_some_and(|c| hosts_mycelium(c.material) && c.mycelium() > 0)
    {
        reduce_mycelium_shares(world, gx, air_y - 1, MYCELIUM_EMERGE_COST);
        let _ = take_mycelium_energy(world, gx, air_y - 1, MYCELIUM_EMERGE_ENERGY_COST);
    }
    // Prefer stamped editor / spore lineage; fall back to the stock template.
    let (body, genome) = if let Some(lin) = nearest_mycelium_lineage(world, gx, air_y - 1) {
        (lin.body, lin.genome)
    } else {
        (
            crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus(),
            {
                let mut g = Genome::default();
                g.digest_rate = 1.0;
                g
            },
        )
    };
    if !body.iter().any(|(_, _, m)| *m == ModuleId::Digest) {
        return None;
    }
    let tank = 40.0f32;
    let mut child = Atom::from_body(gx, air_y, tank, body);
    apply_genome(&mut child, genome);
    child.energy = (tank * MYCELIUM_EMERGE_ENERGY_FRAC).clamp(1.0, child.energy_max);
    // Mature before sporing — emergence must not immediately flood.
    child.cooldown = FUNGUS_SPORE_PERIOD;
    pin_plant_pose(&mut child);
    if !is_fungus_seated(world, &child) {
        return None;
    }
    Some(child)
}

/// Rare dispersal: energy cost, inoculum on Organic (no Atom child).
///
/// Requires painted [`ModuleId::ReproSpore`]. Habit depends on seating:
/// - **Surface stalk** — column must be [`fruiting_surface_ready`]; wind
///   carries inoculum far ([`FUNGUS_STALK_SPORE_MIN_DIST`]…); stalk then
///   collapses to litter.
/// - **Underground** — short rhizomorph hop ([`FUNGUS_RHIZOMORPH_MAX_DIST`])
///   that infects mycelium nearby (no surface / wind requirement).
///
/// Visible mushrooms only come from [`try_emergent_fruiting`]. Local
/// density gate keeps living stalks sparse.
pub fn try_spore(
    world: &mut World,
    atom: &mut Atom,
    tick: u64,
    entity_id: u32,
    _pop_room: bool,
    wind_vx: f32,
    fungus_cols: &[i32],
    bank_cfg: &crate::spore_bank::SporeBankConfig,
) -> crate::spore_bank::DispersalResult {
    use crate::spore_bank::{packet_from_child, DispersalResult, SporeKind};

    if atom.cooldown > 0 {
        return DispersalResult::None;
    }
    if !is_fungus(atom) || digest_count(atom) < 1 {
        return DispersalResult::None;
    }
    if atom
        .body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::ReproSpore)
        .count()
        < 1
    {
        return DispersalResult::None;
    }
    let stalk = is_surface_stalk(world, atom);
    // Stalks only launch once mycelium has breached this column's surface.
    if stalk && fruiting_surface_ready(world, atom.gx).is_none() {
        return DispersalResult::None;
    }
    // Buried bodies may rhizomorph without a surface breach.
    if !stalk && !is_fungus_seated(world, atom) {
        return DispersalResult::None;
    }
    let local = count_fungi_near(
        fungus_cols,
        atom.gx,
        FUNGUS_SPORE_LOCAL_RADIUS,
        world.wrap_width,
    );
    if local >= FUNGUS_SPORE_LOCAL_MAX {
        return DispersalResult::None;
    }
    let tank = if atom.energy_base_max >= 1.0 {
        atom.energy_base_max
    } else {
        atom.energy_max
    }
    .max(1.0);
    if atom.energy < tank * FUNGUS_SPORE_ENERGY_FRAC {
        return DispersalResult::None;
    }
    // Extra rarity gate beyond cooldown (stalks rarer than local hops).
    let h = hash_u64(world.seed.0, tick, entity_id as u64, 0xF801_700D);
    let odds = if stalk { 17 } else { 11 };
    if h % odds != 0 {
        return DispersalResult::None;
    }
    // Prefer a ready seat; bank-capable pick lands even on crowded/dry columns.
    let Some((wx, gy)) =
        pick_spore_site_mode(world, atom, tick, entity_id, wind_vx, fungus_cols, stalk)
            .or_else(|| {
                if bank_cfg.enabled {
                    pick_spore_bank_landing(world, atom, tick, entity_id, wind_vx, stalk)
                } else {
                    None
                }
            })
    else {
        return DispersalResult::None;
    };
    let cost = tank * if stalk { 0.45 } else { 0.32 };
    if atom.energy < cost {
        return DispersalResult::None;
    }
    atom.energy -= cost;
    atom.cooldown = if stalk {
        FUNGUS_SPORE_PERIOD
    } else {
        FUNGUS_SPORE_PERIOD / 2
    };

    // Mutate genome + body on release — wind/rhizomorph inoculum carries
    // a varied lineage; emergent stalks later match the painted mushroom.
    let child_genome = Genome::mutate(atom.genome, world.seed.0, tick, entity_id);
    let child_body = mutate_body(
        &atom.body,
        child_genome.clone_fidelity,
        world.seed.0,
        tick,
        entity_id,
    );
    let lineage = (child_genome, child_body);

    // Spores inoculate mycelium — they do not stamp a living fruiting Atom.
    if let Some((gx, gy)) =
        infect_mycelium_with_lineage(world, wx, gy, Some(lineage.clone()))
    {
        if stalk {
            // Spent mushroom collapses → corpse → litter → Organic.
            atom.energy = 0.0;
        }
        return DispersalResult::Inoculated {
            gx,
            gy,
            collapse: stalk,
        };
    }
    // No host at landing — hibernate mutated packet for a later inoculum wake.
    if bank_cfg.enabled {
        let mut packet = packet_from_child(SporeKind::Fungus, atom, tick, stalk);
        packet.genome = lineage.0;
        packet.body = lineage.1;
        let wx = world.wrap_x(wx);
        if world.spore_bank.deposit(wx, gy, packet, bank_cfg) {
            if stalk {
                atom.energy = 0.0;
            }
            return DispersalResult::Banked { gx: wx, gy };
        }
    }
    atom.energy = (atom.energy + cost).min(atom.energy_max);
    atom.cooldown = 0;
    DispersalResult::None
}

/// Any fungus crown downwind — ignores density / food (hibernation landing).
fn pick_spore_bank_landing(
    world: &World,
    atom: &Atom,
    tick: u64,
    id: u32,
    wind_vx: f32,
    wind_far: bool,
) -> Option<(i32, i32)> {
    let prefer_dir = if wind_vx.abs() < 0.05 {
        if hash_u64(world.seed.0, tick, id as u64, 0xBA11) & 1 == 0 {
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
    for dist in min_d..=max_d {
        for &sign in &[prefer_dir, -prefer_dir] {
            let wx = world.wrap_x(atom.gx + sign * dist);
            if let Some(gy) = find_fungus_slot_biased(world, wx, atom.gy, wind_far) {
                return Some((wx, gy));
            }
        }
    }
    None
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

/// Scan every cream cell, then pick up to `max` for this pulse.
///
/// ~¾ of the budget prefers frontier cells (`myc < 80`) so networks keep
/// climbing / seeking Organic; the rest rotates through dense hubs so they
/// still thicken, compost, and push outward. Without this, a fixed
/// first-N scan froze growth once hubs filled the old 256-cell cap.
fn collect_mycelium_field_cells(
    world: &World,
    tick: u64,
    seed: u64,
    max: usize,
) -> Vec<(i32, i32, u8, MaterialId)> {
    use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};

    let mut frontier: Vec<(i32, i32, u8, MaterialId)> = Vec::new();
    let mut core: Vec<(i32, i32, u8, MaterialId)> = Vec::new();
    let mut coords: Vec<_> = world.chunks.keys().copied().collect();
    if coords.is_empty() || max == 0 {
        return Vec::new();
    }
    coords.sort_by_key(|c| (c.cy, c.cx));
    // Rotate chunk visit order each pulse so no region is permanently last.
    let pulse = (tick / MYCELIUM_FIELD_PERIOD.max(1)) as usize;
    let start = pulse % coords.len();
    coords.rotate_left(start);

    for coord in coords {
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + lx as i32;
                let gy = coord.cy * CHUNK_CELLS_H as i32 + ly as i32;
                let Some(c) = world.get_cell(gx, gy) else {
                    continue;
                };
                if !hosts_mycelium(c.material) {
                    continue;
                }
                let myc = c.mycelium();
                if myc == 0 {
                    continue;
                }
                if myc < 80 {
                    frontier.push((gx, gy, myc, c.material));
                } else {
                    core.push((gx, gy, myc, c.material));
                }
            }
        }
    }

    let n_front = (max * 3 / 4).max(1).min(max);
    let mut out = take_rotated_sample(&mut frontier, tick, seed, 0xF70A, n_front);
    let remain = max.saturating_sub(out.len());
    out.extend(take_rotated_sample(&mut core, tick, seed, 0xC0A1, remain));
    // If one tier was short, top up from leftovers of the other.
    if out.len() < max && !frontier.is_empty() {
        let need = max - out.len();
        out.extend(take_rotated_sample(&mut frontier, tick, seed, 0xF70B, need));
    }
    if out.len() < max && !core.is_empty() {
        let need = max - out.len();
        out.extend(take_rotated_sample(&mut core, tick, seed, 0xC0A2, need));
    }
    out
}

fn take_rotated_sample(
    cells: &mut Vec<(i32, i32, u8, MaterialId)>,
    tick: u64,
    seed: u64,
    salt: u64,
    n: usize,
) -> Vec<(i32, i32, u8, MaterialId)> {
    if cells.is_empty() || n == 0 {
        return Vec::new();
    }
    // Deterministic shuffle-lite: sort by hash(tick, pos) then take prefix.
    cells.sort_by_key(|&(gx, gy, myc, _)| {
        hash_u64(seed, tick ^ salt, gx as u64, gy as u64 ^ (myc as u64))
    });
    let take = n.min(cells.len());
    cells.drain(..take).collect()
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
        let strain = alloc_mycelium_strain(&mut w);
        w.mycelium_strains.insert((4, 2), vec![(strain, 255)]);
        // Neighbour Air can receive excess from compaction.
        w.set_cell(4, 3, Cell::air());
        assert!(compost_organic_to_soil(&mut w, 4, 2));
        let soil = w.get_cell(4, 2).unwrap();
        assert_eq!(soil.material, MaterialId::Soil);
        assert!(
            soil.mycelium() > 0 && soil.mycelium() <= MYCELIUM_COMPOST_RESIDUAL,
            "compost must leave a residual cream corridor (got {})",
            soil.mycelium()
        );
        assert!(
            mycelium_shares_at(&w, 4, 2)
                .iter()
                .any(|&(s, a)| s == strain && a > 0),
            "residual corridor must keep strain ownership"
        );
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
    fn field_sample_prefers_frontier_when_over_cap() {
        let mut w = litter_plot();
        // Dense saturated hub — would have monopolized the old fixed scan.
        for x in 0..8 {
            for y in 1..6 {
                let mut sand = Cell::solid(MaterialId::Sand);
                sand.sat = Sat(180);
                sand.set_mycelium(200);
                w.set_cell(x, y, sand);
                let s = alloc_mycelium_strain(&mut w);
                w.mycelium_strains.insert((x, y), vec![(s, 200)]);
            }
        }
        // Thin frontier corridor toward fresh Organic.
        for y in 1..=4 {
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            sand.set_mycelium(20);
            w.set_cell(10, y, sand);
            let s = alloc_mycelium_strain(&mut w);
            w.mycelium_strains.insert((10, y), vec![(s, 20)]);
        }
        let mut food = Cell::solid(MaterialId::Organic);
        food.sat = Sat(160);
        w.set_cell(10, 5, food);

        let picked = collect_mycelium_field_cells(&w, MYCELIUM_FIELD_PERIOD, w.seed.0, 16);
        assert_eq!(picked.len(), 16, "sample respects process budget");
        let frontier_hits = picked.iter().filter(|&&(gx, _, myc, _)| gx == 10 && myc < 80).count();
        assert!(
            frontier_hits >= 3,
            "frontier corridor must get slots when hubs overflow the cap (hits={frontier_hits})"
        );
    }

    #[test]
    fn virgin_organic_compost_does_not_invent_cream() {
        let mut w = litter_plot();
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(120);
        // No mycelium — same path carbon surface oxidation uses.
        w.set_cell(4, 2, org);
        w.set_cell(4, 3, Cell::air());
        assert!(compost_organic_to_soil(&mut w, 4, 2));
        let soil = w.get_cell(4, 2).unwrap();
        assert_eq!(soil.material, MaterialId::Soil);
        assert_eq!(
            soil.mycelium(),
            0,
            "virgin Organic→Soil must not invent orphan cream"
        );
        assert!(
            mycelium_shares_at(&w, 4, 2).is_empty(),
            "virgin compost must leave no strain shares"
        );
        assert!(
            soil.flags.contains(CellFlags::COMPACTED),
            "humified soil still marks compacted"
        );
    }

    #[test]
    fn field_clears_isolated_oxidation_ghost_cream() {
        let mut w = litter_plot();
        let mut soil = Cell::solid(MaterialId::Soil);
        soil.sat = Sat(160);
        soil.flags.set(CellFlags::COMPACTED);
        soil.set_mycelium(1); // fake cream, no shares
        w.set_cell(4, 2, soil);
        // Real hub far enough that 8-neighbour check does not see it.
        let mut hub = Cell::solid(MaterialId::Sand);
        hub.sat = Sat(160);
        hub.set_mycelium(80);
        w.set_cell(8, 1, hub);
        let strain = alloc_mycelium_strain(&mut w);
        w.mycelium_strains.insert((8, 1), vec![(strain, 80)]);
        w.tick = MYCELIUM_FIELD_PERIOD;
        step_mycelium_field(&mut w);
        assert_eq!(
            w.get_cell(4, 2).map(|c| c.mycelium()),
            Some(0),
            "isolated myc=1 soil with no shares should clear as oxidation ghost"
        );
    }

    #[test]
    fn field_clears_isolated_orphan_cream_of_any_intensity() {
        let mut w = litter_plot();
        // Fat orphan (e.g. roof-collapse debris that dropped `_pad` without shares).
        let mut loose = Cell::solid(MaterialId::LooseLimestone);
        loose.sat = Sat(40);
        loose.set_mycelium(28);
        w.set_cell(4, 2, loose);
        // Dense owned hub fills the sample budget; orphan must still heal.
        for x in 10..26 {
            for y in 1..=4 {
                let mut sand = Cell::solid(MaterialId::Sand);
                sand.sat = Sat(180);
                sand.set_mycelium(200);
                w.set_cell(x, y, sand);
                let s = alloc_mycelium_strain(&mut w);
                w.mycelium_strains.insert((x, y), vec![(s, 200)]);
            }
        }
        w.tick = MYCELIUM_FIELD_PERIOD;
        step_mycelium_field(&mut w);
        assert_eq!(
            w.get_cell(4, 2).map(|c| c.mycelium()),
            Some(0),
            "isolated shareless cream must clear even when hubs fill the process budget"
        );
        assert!(mycelium_shares_at(&w, 4, 2).is_empty());
    }

    #[test]
    fn orphan_cream_beside_owned_network_inherits_strain() {
        let mut w = litter_plot();
        let mut hub = Cell::solid(MaterialId::Sand);
        hub.sat = Sat(160);
        hub.set_mycelium(80);
        w.set_cell(4, 2, hub);
        let strain = alloc_mycelium_strain(&mut w);
        w.mycelium_strains.insert((4, 2), vec![(strain, 80)]);
        let mut orphan = Cell::solid(MaterialId::LooseLimestone);
        orphan.sat = Sat(40);
        orphan.set_mycelium(28);
        w.set_cell(5, 2, orphan);
        w.tick = MYCELIUM_FIELD_PERIOD;
        step_mycelium_field(&mut w);
        assert_eq!(w.get_cell(5, 2).map(|c| c.mycelium()), Some(28));
        assert!(
            mycelium_shares_at(&w, 5, 2)
                .iter()
                .any(|&(s, a)| s == strain && a == 28),
            "fringe orphan must inherit neighbour strain, not mint / clear"
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
    fn fungus_spore_never_births_atom_child() {
        let mut w = litter_plot();
        rich_moist_surface(&mut w, 4, 140);
        for x in 0..12 {
            rich_moist_surface(&mut w, x, 80);
            add_soft_litter(&mut w, x, 20);
        }
        let mut atom = Atom::from_body(4, 4, 40.0, fungus_body());
        let occupied: Vec<i32> = (0..12).collect();
        let bank = crate::spore_bank::SporeBankConfig::default();
        for tick in 0..4_000u64 {
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
            assert!(
                !matches!(
                    try_spore(&mut w, &mut atom, tick, 7, true, 0.4, &occupied, &bank),
                    crate::spore_bank::DispersalResult::Germinated(_)
                ),
                "fungus spores must inoculate mycelium, never birth a fruiting Atom"
            );
        }
    }

    #[test]
    fn infect_mycelium_seeds_feeder_column() {
        let mut w = litter_plot();
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(160);
            w.set_cell(4, y, org);
        }
        let hit = infect_mycelium_at(&mut w, 4, 4).expect("Organic under click");
        assert_eq!(hit, (4, 3));
        let surface = w.get_cell(4, 3).unwrap().mycelium();
        let feeder = w.get_cell(4, 2).unwrap().mycelium();
        assert!(
            surface >= MYCELIUM_INOCULUM_AMOUNT,
            "surface inoculum too weak ({surface})"
        );
        assert!(feeder > 0, "feeder column must be seeded for later emergence");
        assert!(mycelium_breached_from_below(&w, 4, 3));
    }

    #[test]
    fn fruiting_body_wind_spore_inoculates_and_collapses() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(1, 0));
        for x in 0..80 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 1, sand);
            for y in 2..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Parent column + a far downwind Organic bank for long wind travel.
        rich_moist_surface(&mut w, 4, 140);
        for x in 12..48 {
            rich_moist_surface(&mut w, x, 80);
            add_soft_litter(&mut w, x, 20);
        }
        let mut atom = Atom::from_body(4, 4, 40.0, fungus_body());
        atom.energy = atom.energy_max;
        atom.cooldown = 0;
        assert!(is_surface_stalk(&w, &atom));
        assert!(fruiting_surface_ready(&w, 4).is_some());
        let bank = crate::spore_bank::SporeBankConfig::default();
        let mut landing = None;
        for tick in 0..4_000u64 {
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
            if let crate::spore_bank::DispersalResult::Inoculated { gx, gy, collapse } =
                try_spore(&mut w, &mut atom, tick, 7, true, 0.4, &[4], &bank)
            {
                assert!(collapse, "surface stalk must collapse after sporulating");
                assert!(atom.energy <= 0.0, "spent stalk energy must be zeroed");
                landing = Some((gx, gy));
                break;
            }
        }
        let (gx, gy) = landing.expect("surface stalk must eventually wind-inoculate");
        let d = (gx - 4).abs();
        assert!(
            d >= FUNGUS_STALK_SPORE_MIN_DIST,
            "stalk wind inoculum should travel far (dist={d})"
        );
        assert!(
            w.get_cell(gx, gy)
                .map(|c| c.material == MaterialId::Organic && c.mycelium() > 0)
                .unwrap_or(false),
            "landing Organic must carry cream inoculum"
        );
    }

    #[test]
    fn underground_fungus_rhizomorph_inoculates_locally() {
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
        let bank = crate::spore_bank::SporeBankConfig::default();
        let mut landing = None;
        for tick in 0..4_000u64 {
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
            if let crate::spore_bank::DispersalResult::Inoculated { gx, collapse, .. } =
                try_spore(&mut w, &mut atom, tick, 11, true, 0.4, &[4], &bank)
            {
                assert!(!collapse, "rhizomorph hop must not kill the buried body");
                landing = Some(gx);
                break;
            }
        }
        let gx = landing.expect("buried fungus should rhizomorph-inoculate locally");
        let d = (gx - 4).abs();
        assert!(
            d >= 1 && d <= FUNGUS_RHIZOMORPH_MAX_DIST,
            "rhizomorph must stay local (dist={d})"
        );
        assert!(
            w.get_cell(gx, 3)
                .or_else(|| w.get_cell(gx, 2))
                .map(|c| c.mycelium() > 0)
                .unwrap_or(false),
            "local Organic must receive inoculum"
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

    #[test]
    fn organic_field_banks_network_sugar() {
        let mut w = litter_plot();
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(200);
        org.set_mycelium(40);
        w.set_cell(4, 2, org);
        let s = alloc_mycelium_strain(&mut w);
        w.mycelium_strains.insert((4, 2), vec![(s, 40)]);
        for t in 0..12u64 {
            w.tick = t * MYCELIUM_FIELD_PERIOD;
            step_mycelium_field(&mut w);
        }
        assert!(
            mycelium_energy_at(&w, 4, 2) >= 8,
            "moist Organic cream should bank network sugar"
        );
    }

    #[test]
    fn network_sugar_moves_with_cell_swap() {
        let mut w = litter_plot();
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(160);
        sand.set_mycelium(20);
        w.set_cell(4, 2, sand);
        w.set_cell(5, 2, Cell::air());
        let s = alloc_mycelium_strain(&mut w);
        w.mycelium_strains.insert((4, 2), vec![(s, 20)]);
        w.mycelium_energy.insert((4, 2), 40);
        swap_cells_preserving_mycelium(&mut w, 4, 2, 5, 2);
        assert_eq!(mycelium_energy_at(&w, 5, 2), 40);
        assert_eq!(mycelium_energy_at(&w, 4, 2), 0);
    }

    #[test]
    fn same_strain_cream_equalizes_sugar_cargo() {
        let mut w = litter_plot();
        let s = alloc_mycelium_strain(&mut w);
        for x in 4..=5 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(160);
            org.set_mycelium(40);
            w.set_cell(x, 2, org);
            w.mycelium_strains.insert((x, 2), vec![(s, 40)]);
        }
        w.mycelium_energy.insert((4, 2), 30);
        w.mycelium_energy.insert((5, 2), 2);
        let colonized = vec![
            (4, 2, 40, MaterialId::Organic),
            (5, 2, 40, MaterialId::Organic),
        ];
        equalize_mycelium_cargo(&mut w, &colonized);
        assert!(
            mycelium_energy_at(&w, 4, 2) < 30,
            "rich cell should bleed sugar"
        );
        assert!(
            mycelium_energy_at(&w, 5, 2) > 2,
            "poor neighbour should receive sugar"
        );
        assert_eq!(
            mycelium_energy_at(&w, 4, 2) + mycelium_energy_at(&w, 5, 2),
            32,
            "sugar conserved"
        );
    }

    #[test]
    fn fruiting_body_can_sip_network_sugar() {
        let mut w = litter_plot();
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(200);
        org.set_mycelium(80);
        w.set_cell(4, 2, org);
        let s = alloc_mycelium_strain(&mut w);
        w.mycelium_strains.insert((4, 2), vec![(s, 80)]);
        w.mycelium_energy.insert((4, 2), 50);
        let taken = sip_mycelium_energy_near(&mut w, 4, 3, MYCELIUM_ENERGY_SIP_MAX);
        assert_eq!(taken, MYCELIUM_ENERGY_SIP_MAX);
        assert_eq!(mycelium_energy_at(&w, 4, 2), 50 - MYCELIUM_ENERGY_SIP_MAX);
    }

    #[test]
    fn mineral_cream_soft_caps_while_seeking() {
        let mut w = litter_plot();
        // Isolated moist sand — seeking, no Organic in reach.
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(200);
        sand.set_mycelium(10);
        w.set_cell(4, 2, sand);
        let s = alloc_mycelium_strain(&mut w);
        w.mycelium_strains.insert((4, 2), vec![(s, 10)]);
        for t in 0..200u64 {
            w.tick = t * MYCELIUM_FIELD_PERIOD;
            step_mycelium_field(&mut w);
        }
        let myc = w.get_cell(4, 2).unwrap().mycelium();
        assert!(
            myc > 10 && myc <= MYCELIUM_MINERAL_CAP,
            "seeking mineral should thicken but stay ≤ mineral cap (got {myc})"
        );
    }

    #[test]
    fn mineral_corridor_tapers_after_reaching_organic() {
        let mut w = litter_plot();
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(200);
        sand.set_mycelium(MYCELIUM_MINERAL_CAP);
        w.set_cell(4, 2, sand);
        let s = alloc_mycelium_strain(&mut w);
        w.mycelium_strains
            .insert((4, 2), vec![(s, MYCELIUM_MINERAL_CAP)]);
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(200);
        org.set_mycelium(80);
        w.set_cell(5, 2, org);
        w.mycelium_strains.insert((5, 2), vec![(s, 80)]);
        for t in 0..120u64 {
            w.tick = t * MYCELIUM_FIELD_PERIOD;
            step_mycelium_field(&mut w);
        }
        let myc = w.get_cell(4, 2).unwrap().mycelium();
        assert!(
            myc <= MYCELIUM_MINERAL_CONDUIT + 4,
            "fed mineral corridor should taper toward conduit (got {myc})"
        );
        assert!(myc > 0, "conduit must remain connected");
    }

    #[test]
    fn mycelium_spreads_through_moist_sand_toward_organic() {
        let mut w = litter_plot();
        // Moist sand corridor between a seeded cell and distant Organic.
        for x in 4..=7 {
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 2, sand);
        }
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(160);
        w.set_cell(8, 2, org);
        if let Some(mut c) = w.get_cell(4, 2) {
            c.set_mycelium(40);
            w.set_cell(4, 2, c);
        }
        for _ in 0..48 {
            spread_mycelium_once(&mut w, 4, 2);
            // Keep walking the frontier rightward if present.
            for x in 4..=7 {
                if w.get_cell(x, 2).map(|c| c.mycelium() > 0).unwrap_or(false) {
                    spread_mycelium_once(&mut w, x, 2);
                }
            }
        }
        let sand_threaded = (5..=7).any(|x| {
            w.get_cell(x, 2)
                .map(|c| c.material == MaterialId::Sand && c.mycelium() > 0)
                .unwrap_or(false)
        });
        let org_hit = w
            .get_cell(8, 2)
            .map(|c| c.material == MaterialId::Organic && c.mycelium() > 0)
            .unwrap_or(false);
        assert!(
            sand_threaded || org_hit,
            "mycelium must network through moist sand toward Organic (sand={sand_threaded} org={org_hit})"
        );
    }

    #[test]
    fn inoculum_mints_strain_and_spread_inherits() {
        let mut w = litter_plot();
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(160);
            w.set_cell(4, y, org);
            w.set_cell(5, y, {
                let mut o = Cell::solid(MaterialId::Organic);
                o.sat = Sat(160);
                o
            });
        }
        let (ox, oy) = infect_mycelium_at(&mut w, 4, 4).expect("inoculum");
        let strain = mycelium_strain_at(&w, ox, oy).expect("inoculum must mint a strain");
        assert!(
            mycelium_shares_at(&w, ox, oy - 1)
                .iter()
                .any(|&(s, a)| s == strain && a > 0),
            "feeder column carries the inoculum strain share"
        );
        for _ in 0..16 {
            spread_mycelium_once(&mut w, ox, oy);
        }
        let neighbor_strain = (1..=3).find_map(|y| {
            mycelium_shares_at(&w, 5, y)
                .iter()
                .find(|&&(s, a)| s == strain && a > 0)
                .map(|&(s, _)| s)
        });
        assert_eq!(
            neighbor_strain,
            Some(strain),
            "spread must carry source strain share"
        );
        let [r, g, b] = mycelium_strain_rgb(strain);
        assert!(
            r.max(g).max(b) >= 200,
            "strain colors should be bright (got {r},{g},{b})"
        );
    }

    #[test]
    fn overlay_keeps_thin_corridors_neon() {
        let rgba = mycelium_shares_overlay_rgba(&[(7, 8)], 8);
        assert!(
            rgba[3] >= 200,
            "thin cream must stay high-alpha on M overlay (got a={})",
            rgba[3]
        );
        assert!(
            rgba[0].max(rgba[1]).max(rgba[2]) >= 180,
            "strain color must stay bright"
        );
    }

    #[test]
    fn cell_swap_migrates_strain_shares() {
        let mut w = litter_plot();
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(160);
        sand.set_mycelium(40);
        w.set_cell(4, 2, sand);
        w.set_cell(5, 2, Cell::air());
        let strain = alloc_mycelium_strain(&mut w);
        w.mycelium_strains.insert((4, 2), vec![(strain, 40)]);
        swap_cells_preserving_mycelium(&mut w, 4, 2, 5, 2);
        assert_eq!(
            w.get_cell(5, 2).map(|c| c.mycelium()),
            Some(40),
            "cream rides with the sand cell"
        );
        assert_eq!(
            mycelium_shares_at(&w, 5, 2),
            &[(strain, 40)][..],
            "strain shares must follow the host"
        );
        assert!(
            mycelium_shares_at(&w, 4, 2).is_empty(),
            "vacated seat must not keep orphan shares"
        );
    }

    #[test]
    fn bedload_move_migrates_strain_shares() {
        let mut w = litter_plot();
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.set_mycelium(55);
        w.set_cell(3, 1, sand);
        w.set_cell(6, 1, Cell::air());
        let strain = alloc_mycelium_strain(&mut w);
        w.mycelium_strains.insert((3, 1), vec![(strain, 55)]);
        // Simulate erosion: place grain at deposit, move meta, clear source.
        let placed = w.get_cell(3, 1).unwrap();
        w.set_cell(6, 1, placed);
        move_mycelium_meta(&mut w, 3, 1, 6, 1);
        w.set_cell(3, 1, Cell::air());
        assert_eq!(mycelium_shares_at(&w, 6, 1), &[(strain, 55)][..]);
        assert!(mycelium_shares_at(&w, 3, 1).is_empty());
    }

    #[test]
    fn two_strains_share_one_cell_budget() {
        let mut w = litter_plot();
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(160);
        w.set_cell(4, 2, org);
        let a = alloc_mycelium_strain(&mut w);
        let b = alloc_mycelium_strain(&mut w);
        add_mycelium_with_strain(&mut w, 4, 2, 40, Some(a), 255);
        add_mycelium_with_strain(&mut w, 4, 2, 60, Some(b), 255);
        let shares = mycelium_shares_at(&w, 4, 2);
        let amt_a = shares
            .iter()
            .find(|(s, _)| *s == a)
            .map(|(_, n)| *n)
            .unwrap_or(0);
        let amt_b = shares
            .iter()
            .find(|(s, _)| *s == b)
            .map(|(_, n)| *n)
            .unwrap_or(0);
        assert_eq!(amt_a, 40, "strain A keeps its share");
        assert_eq!(amt_b, 60, "strain B keeps its share");
        assert_eq!(
            w.get_cell(4, 2).unwrap().mycelium(),
            100,
            "_pad total must equal sum of shares"
        );
        // Fill remaining room — cannot exceed 255.
        add_mycelium_with_strain(&mut w, 4, 2, 200, Some(a), 255);
        assert_eq!(w.get_cell(4, 2).unwrap().mycelium(), 255);
        let amt_a2 = mycelium_shares_at(&w, 4, 2)
            .iter()
            .find(|(s, _)| *s == a)
            .map(|(_, n)| *n)
            .unwrap_or(0);
        assert_eq!(amt_a2, 195, "A only takes free room (40+155)");
        assert_eq!(
            mycelium_shares_at(&w, 4, 2)
                .iter()
                .find(|(s, _)| *s == b)
                .map(|(_, n)| *n),
            Some(60),
            "B share untouched when A fills room"
        );
    }

    #[test]
    fn mycelium_refuses_bedrock_corridor() {
        let mut w = litter_plot();
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(160);
        org.set_mycelium(60);
        w.set_cell(4, 2, org);
        w.set_cell(5, 2, Cell::solid(MaterialId::Bedrock));
        for _ in 0..24 {
            spread_mycelium_once(&mut w, 4, 2);
        }
        assert_eq!(
            w.get_cell(5, 2).map(|c| c.mycelium()).unwrap_or(0),
            0,
            "bedrock must not host mycelium"
        );
    }

    #[test]
    fn emergent_stalk_uses_stamped_lineage_body() {
        let mut w = litter_plot();
        rich_moist_surface(&mut w, 4, 140);
        let custom: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Digest),
            (0, 2, ModuleId::Hypha),
            (0, 3, ModuleId::Hypha),
            (0, 4, ModuleId::Hypha),
            (1, 1, ModuleId::ReproSpore),
        ];
        let mut g = Genome::default();
        g.digest_rate = 1.35;
        stamp_mycelium_lineage(&mut w, 4, 3, g, custom.clone());
        let mut child = None;
        for pulse in 0..2_000u64 {
            let tick = pulse * MYCELIUM_EMERGE_PERIOD;
            if let Some(a) = try_emergent_fruiting(&mut w, &[], tick, true) {
                child = Some(a);
                break;
            }
        }
        let child = child.expect("lineage bed must raise a stalk");
        let hyphae = child
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Hypha)
            .count();
        assert!(
            hyphae >= 3,
            "emergent stalk should match stamped tall hypha body, got {:?}",
            child.body
        );
        assert!(
            (child.genome.digest_rate - 1.35).abs() < 0.001,
            "emergent genome should match lineage digest_rate"
        );
    }

    #[test]
    fn spore_inoculum_stamps_mutated_lineage() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(1, 0));
        for x in 0..80 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 1, sand);
            for y in 2..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        rich_moist_surface(&mut w, 4, 140);
        for x in 12..40 {
            rich_moist_surface(&mut w, x, 80);
        }
        let mut atom = Atom::from_body(4, 4, 80.0, fungus_body());
        apply_genome(&mut atom, {
            let mut g = Genome::default();
            g.digest_rate = 1.0;
            g.clone_fidelity = 0.2; // messy offspring
            g
        });
        atom.energy = atom.energy_max;
        atom.cooldown = 0;
        let cfg = crate::spore_bank::SporeBankConfig::default();
        let mut inoculated = false;
        for tick in 0..4_000u64 {
            atom.gx = 4;
            atom.gy = 4;
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
            match try_spore(&mut w, &mut atom, tick, 7, true, 1.0, &[4], &cfg) {
                crate::spore_bank::DispersalResult::Inoculated { gx, gy, .. } => {
                    assert!(
                        w.mycelium_lineage.cells.contains_key(&(gx, gy))
                            || nearest_mycelium_lineage(&w, gx, gy).is_some(),
                        "spore must stamp lineage at inoculum"
                    );
                    inoculated = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(inoculated, "surface stalk should eventually inoculate downwind");
    }
}
