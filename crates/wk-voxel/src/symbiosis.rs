//! Plant ↔ fungus symbiotic exchange (opt-in Symbiont module).
//!
//! Both partners must paint [`ModuleId::Symbiont`]. The mutable treaty is
//! Genome `(sym_water, sym_energy)` — an agreed deal, not complementary
//! opposites. Similarity of the two vectors is match quality; a shared
//! lopsided vector is parasitism (high W/low E favours the plant; low W/high E
//! favours the fungus).
//!
//! Exchange is **moisture-directed** on root ↔ mycelium-cream contact:
//! - **Supply** (cream wetter): fungus pore water → plant; plant energy → sugar
//! - **Harvest** (root wetter): plant pore water → cream; network sugar → plant energy
//!
//! Plants keep a **reproduction reserve** ([`SYM_REPRO_RESERVE_FRAC`] of spawn
//! tank) that supply sugar pay cannot spend — so a wet network cannot hold a
//! grove at starvation energy and block sprouting. Water gifts still flow when
//! the plant is banking. Networks leave a small sugar floor when paying plants
//! or other strains ([`SYM_NET_SUGAR_PAY_RESERVE`]).
//!
//! Same-strain cream cells slowly equalize sugar + a trickle of pore water
//! ([`crate::fungi::equalize_mycelium_cargo`]), so wet-side harvests can feed
//! dry-side supply.
//!
//! **Strain↔strain** trade runs on adjacent cream cells with different
//! dominant strains: both need Symbiont + matching treaties
//! ([`World::mycelium_strain_lineage`]); wetter side gives water, drier side
//! pays sugar. Same-cell multi-share barter is deferred (shared sugar pool).
//!
//! Ledgers: plant Atom (both directions) and [`World::sym_net_flow`] by strain id.

use serde::{Deserialize, Serialize};

use crate::blueprint::Genome;
use crate::cell::water_capacity_cell;
#[cfg(test)]
use crate::cell::water_capacity;
use crate::fungi::{
    add_mycelium_energy, ensure_mycelium_strain, lineage_for_strain_at, mycelium_energy_at,
    mycelium_strain_at, nearest_mycelium_lineage, pull_mycelium_cargo_to, take_mycelium_energy,
    MYCELIUM_ENERGY_SIP_TO_ATOM, MYCELIUM_PROBE_SUGAR_RESERVE,
};
use crate::grid::World;
use crate::organism::{Atom, BodyModule, ModuleId};
use crate::plant::{is_land_plant, tank_ref, LAND_SPROUT_ENERGY_FRAC};

/// Soft cap on strain-keyed network flow ledger entries.
pub const SYM_NET_FLOW_MAP_MAX: usize = 8_192;

/// Minimum treaty similarity (0..1) before any exchange fires.
pub const SYM_MATCH_MIN: f32 = 0.55;
/// Max pore-sat units transferred per contact per tick (either direction).
pub const SYM_WATER_MAX_SAT: u8 = 2;
/// Max plant energy spent into network sugar per supply contact per tick.
pub const SYM_ENERGY_MAX: f32 = 0.35;
/// Soft cap on plant↔cream contacts resolved per organism tick.
/// Sized for dense groves (100+ symbiont plants) at one partner each.
pub const SYM_CONTACT_BUDGET: usize = 192;
/// Successful cream partners allowed per plant per tick.
///
/// Mature multi-root trees must not monopolize [`SYM_CONTACT_BUDGET`] and
/// starve spore / sprout clones that sit later in the atom list.
pub const SYM_PARTNERS_PER_PLANT: usize = 1;
/// Soft cap on strain↔strain frontier contacts per mycelium field pulse.
pub const SYM_STRAIN_CONTACT_BUDGET: usize = 48;
/// Max frontier edges considered per pulse (before the contact budget).
pub const SYM_STRAIN_EDGE_SCAN: usize = 256;
/// Min sugar gap for peer sugar trickle when moisture is nearly equal.
pub const SYM_STRAIN_SUGAR_PEER_MIN: u8 = 4;
/// Max sugar moved on an equal-moist peer contact per pulse.
pub const SYM_STRAIN_SUGAR_PEER_MAX: u8 = 2;
/// Treaty byte gap used to label plant- vs fungus-favoring deals.
pub const SYM_BIAS_GAP: u8 = 40;
/// Minimum cream−root moist-frac gap to pick supply vs harvest.
pub const SYM_MOIST_DELTA: f32 = 0.10;
/// Fraction of plant spawn tank that supply sugar trade cannot spend.
/// Matches rhizome sprout threshold so linked plants can still reproduce.
pub const SYM_REPRO_RESERVE_FRAC: f32 = LAND_SPROUT_ENERGY_FRAC;
/// Local network sugar left untouchable when paying plants / other strains.
pub const SYM_NET_SUGAR_PAY_RESERVE: u8 = MYCELIUM_PROBE_SUGAR_RESERVE;

/// Who the lived deal favours (same vector, lopsided rates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymBias {
    Mutual,
    /// High W / low E — fungus gives more water than it takes in sugar.
    PlantFavoring,
    /// Low W / high E — fungus takes more sugar than it offers in water.
    FungusFavoring,
}

impl SymBias {
    pub fn label(self) -> &'static str {
        match self {
            SymBias::Mutual => "mutual",
            SymBias::PlantFavoring => "plant-favoring",
            SymBias::FungusFavoring => "fungus-favoring",
        }
    }
}

/// Local trade direction from relative wetness at the contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymTradeMode {
    /// Cream wetter — network supplies water, plant pays sugar.
    Supply,
    /// Root wetter — plant supplies water, network pays sugar.
    Harvest,
}

impl SymTradeMode {
    pub fn label(self) -> &'static str {
        match self {
            SymTradeMode::Supply => "supply",
            SymTradeMode::Harvest => "harvest",
        }
    }
}

/// Actual exchange counters for one mycelium strain network (persisted).
///
/// Keyed on [`World::sym_net_flow`] by strain id — not by cell — so a network
/// that splits spatially and later reconnects keeps one continuous book.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SymNetFlow {
    /// Water donated to plants (supply).
    pub water_out_total: u32,
    /// Sugar received from plants (supply).
    pub sugar_in_total: u32,
    /// Water taken from plants (harvest).
    #[serde(default)]
    pub water_in_total: u32,
    /// Sugar paid to plants (harvest).
    #[serde(default)]
    pub sugar_out_total: u32,
    pub water_out_last: u8,
    pub sugar_in_last: u8,
    #[serde(default)]
    pub water_in_last: u8,
    #[serde(default)]
    pub sugar_out_last: u8,
    pub last_tick: u64,
}

/// Read-only inspector snapshot of a plant↔cream symbiotic link.
#[derive(Debug, Clone, Copy)]
pub struct SymProbe {
    /// Root touches cream with a Symbiont lineage (geometry only).
    pub touching: bool,
    /// Treaties similar enough to exchange ([`SYM_MATCH_MIN`]).
    pub linked: bool,
    /// Assortative match quality 0..1.
    pub match_q: f32,
    /// Living plant store index when known.
    pub plant_idx: Option<usize>,
    /// Agreed water byte (mean of partners).
    pub deal_w: u8,
    /// Agreed energy byte (mean of partners).
    pub deal_e: u8,
    /// Potential pore-sat units moved per tick (active mode direction).
    pub water_per_tick: u8,
    /// Potential plant energy spent (supply) or gained (harvest) per tick.
    pub energy_per_tick: f32,
    /// Potential network-sugar units moved per tick.
    pub sugar_per_tick: u8,
    pub bias: SymBias,
    /// Moisture-directed trade mode at this contact (when touching).
    pub trade_mode: SymTradeMode,
    /// Supply direction: water to plant / sugar from plant (last tick).
    pub water_last: u8,
    pub sugar_last: u8,
    pub water_total: u32,
    pub sugar_total: u32,
    /// Harvest direction: water from plant / sugar to plant (last tick).
    pub water_rev_last: u8,
    pub sugar_rev_last: u8,
    pub water_rev_total: u32,
    pub sugar_rev_total: u32,
    /// When set, counters are the **network** ledger for this strain.
    /// When `None`, counters are the **plant** Atom ledger.
    pub strain_id: Option<u32>,
    /// True when the plant touches same-strain cream nearby, not this cell.
    /// Cream inspectors use this so a conduit cell doesn't look "idle" while
    /// the strain is actively linked a few cells away.
    pub via_network: bool,
    /// Plant energy floor reserved for reproduction (0 when unknown).
    pub energy_reserve: f32,
    /// Supply sugar is paused — plant is banking toward the repro reserve.
    pub sugar_banking: bool,
    /// A living Root module sits on this cream cell (shared block).
    pub cohabit: bool,
}

/// Read-only inspector snapshot of a cream↔cream strain frontier.
#[derive(Debug, Clone, Copy)]
pub struct SymFrontierProbe {
    /// This cell's dominant strain.
    pub self_strain: u32,
    /// Neighbouring dominant strain across the frontier.
    pub peer_strain: u32,
    pub peer_x: i32,
    pub peer_y: i32,
    /// Assortative treaty match 0..1.
    pub match_q: f32,
    /// `moist(here) - moist(peer)`.
    pub moist_delta: f32,
    pub sugar_here: u8,
    pub sugar_peer: u8,
    pub deal_w: u8,
    pub deal_e: u8,
    /// Moisture gap large enough for water↔sugar trade.
    pub can_water: bool,
    /// Sugar gap large enough for equal-moist peer trickle.
    pub can_sugar_peer: bool,
    /// Why exchange cannot fire, if blocked.
    pub blocked: Option<&'static str>,
}

/// True when the body paints at least one Symbiont organ.
pub fn body_has_symbiont(body: &[BodyModule]) -> bool {
    body.iter().any(|(_, _, m)| *m == ModuleId::Symbiont)
}

/// Assortative similarity of two treaties (1 = identical, 0 = maximally far).
pub fn treaty_match(a: Genome, b: Genome) -> f32 {
    let dw = (a.sym_water as i16 - b.sym_water as i16).unsigned_abs() as f32;
    let de = (a.sym_energy as i16 - b.sym_energy as i16).unsigned_abs() as f32;
    let dist = (dw * dw + de * de).sqrt();
    // Max Euclidean distance on the unit square [0,255]².
    const MAX: f32 = 360.624_6; // sqrt(2)*255
    (1.0 - (dist / MAX).clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// Mean treaty used as the lived deal once partners match.
fn agreed_treaty(a: Genome, b: Genome) -> (u8, u8) {
    let w = ((a.sym_water as u16 + b.sym_water as u16) / 2) as u8;
    let e = ((a.sym_energy as u16 + b.sym_energy as u16) / 2) as u8;
    (w, e)
}

fn bias_from_deal(w: u8, e: u8) -> SymBias {
    if w > e.saturating_add(SYM_BIAS_GAP) {
        SymBias::PlantFavoring
    } else if e > w.saturating_add(SYM_BIAS_GAP) {
        SymBias::FungusFavoring
    } else {
        SymBias::Mutual
    }
}

fn exchange_rates(match_q: f32, w: u8, e: u8) -> (u8, f32, u8) {
    let water_want = ((w as f32 / 255.0) * match_q * SYM_WATER_MAX_SAT as f32)
        .round()
        .clamp(0.0, SYM_WATER_MAX_SAT as f32) as u8;
    let energy_want = (e as f32 / 255.0) * match_q * SYM_ENERGY_MAX;
    let sugar = (energy_want / MYCELIUM_ENERGY_SIP_TO_ATOM.max(0.01))
        .round()
        .clamp(0.0, 8.0) as u8;
    (water_want, energy_want, sugar)
}

/// Spawn-tank energy that supply sugar pay must leave untouched.
pub fn plant_sym_energy_reserve(atom: &Atom) -> f32 {
    tank_ref(atom) * SYM_REPRO_RESERVE_FRAC
}

/// Surplus above the repro reserve that may be spent on supply sugar.
pub fn plant_sym_sugar_spendable(atom: &Atom) -> f32 {
    (atom.energy - plant_sym_energy_reserve(atom)).max(0.0)
}

fn apply_plant_reserve_to_probe(probe: &mut SymProbe, atom: &Atom) {
    let reserve = plant_sym_energy_reserve(atom);
    probe.energy_reserve = reserve;
    if probe.linked && probe.trade_mode == SymTradeMode::Supply {
        let want = probe.energy_per_tick;
        if want > 0.01 && plant_sym_sugar_spendable(atom) + 1e-4 < want {
            probe.sugar_banking = true;
            probe.energy_per_tick = 0.0;
            probe.sugar_per_tick = 0;
        }
    }
}

/// Local cream sugar available for outbound pay after the network reserve.
fn cream_sugar_payable(world: &World, gx: i32, gy: i32) -> u8 {
    mycelium_energy_at(world, gx, gy).saturating_sub(SYM_NET_SUGAR_PAY_RESERVE)
}

fn cell_moist_frac(world: &World, gx: i32, gy: i32) -> f32 {
    let gx = world.wrap_x(gx);
    let Some(c) = world.get_cell(gx, gy) else {
        return 0.0;
    };
    if c.material == wk_material::MaterialId::Air {
        return 0.0;
    }
    let cap = water_capacity_cell(c, &world.hydro);
    if cap == 0 {
        return 0.0;
    }
    c.sat.0 as f32 / cap as f32
}

/// Pick supply vs harvest from cream vs root bed wetness.
pub fn trade_mode_at(world: &World, root_x: i32, root_y: i32, cream_x: i32, cream_y: i32) -> SymTradeMode {
    let cream = cell_moist_frac(world, cream_x, cream_y);
    let mut root = cell_moist_frac(world, root_x, root_y);
    // Root modules often sit on the soil column just below the nucleus; also
    // sample one cell down so a soaked bed registers as harvestable.
    root = root.max(cell_moist_frac(world, root_x, root_y - 1));
    if root > cream + SYM_MOIST_DELTA {
        SymTradeMode::Harvest
    } else {
        SymTradeMode::Supply
    }
}

fn build_probe(
    touching: bool,
    match_q: f32,
    plant_idx: Option<usize>,
    plant_g: Genome,
    fungus_g: Genome,
    trade_mode: SymTradeMode,
    water_last: u8,
    sugar_last: u8,
    water_total: u32,
    sugar_total: u32,
    water_rev_last: u8,
    sugar_rev_last: u8,
    water_rev_total: u32,
    sugar_rev_total: u32,
    strain_id: Option<u32>,
    via_network: bool,
    cohabit: bool,
) -> SymProbe {
    let (deal_w, deal_e) = agreed_treaty(plant_g, fungus_g);
    let linked = touching && match_q >= SYM_MATCH_MIN;
    let (water_per_tick, energy_per_tick, sugar_per_tick) = if linked {
        exchange_rates(match_q, deal_w, deal_e)
    } else {
        (0, 0.0, 0)
    };
    SymProbe {
        touching,
        linked,
        match_q,
        plant_idx,
        deal_w,
        deal_e,
        water_per_tick,
        energy_per_tick,
        sugar_per_tick,
        bias: bias_from_deal(deal_w, deal_e),
        trade_mode,
        water_last,
        sugar_last,
        water_total,
        sugar_total,
        water_rev_last,
        sugar_rev_last,
        water_rev_total,
        sugar_rev_total,
        strain_id,
        via_network,
        energy_reserve: 0.0,
        sugar_banking: false,
        cohabit,
    }
}

fn net_flow_at(world: &World, strain: u32) -> SymNetFlow {
    world.sym_net_flow.get(&strain).copied().unwrap_or_default()
}

fn ensure_net_entry(world: &mut World, strain: u32) -> &mut SymNetFlow {
    if world.sym_net_flow.len() >= SYM_NET_FLOW_MAP_MAX && !world.sym_net_flow.contains_key(&strain)
    {
        if let Some(&key) = world.sym_net_flow.keys().next() {
            world.sym_net_flow.remove(&key);
        }
    }
    world.sym_net_flow.entry(strain).or_default()
}

fn record_net_supply(world: &mut World, strain: u32, water: u8, sugar: u8, tick: u64) {
    if water == 0 && sugar == 0 {
        return;
    }
    let e = ensure_net_entry(world, strain);
    e.water_out_total = e.water_out_total.saturating_add(water as u32);
    e.sugar_in_total = e.sugar_in_total.saturating_add(sugar as u32);
    e.water_out_last = e.water_out_last.saturating_add(water);
    e.sugar_in_last = e.sugar_in_last.saturating_add(sugar);
    e.last_tick = tick;
}

fn record_net_harvest(world: &mut World, strain: u32, water: u8, sugar: u8, tick: u64) {
    if water == 0 && sugar == 0 {
        return;
    }
    let e = ensure_net_entry(world, strain);
    e.water_in_total = e.water_in_total.saturating_add(water as u32);
    e.sugar_out_total = e.sugar_out_total.saturating_add(sugar as u32);
    e.water_in_last = e.water_in_last.saturating_add(water);
    e.sugar_out_last = e.sugar_out_last.saturating_add(sugar);
    e.last_tick = tick;
}

/// Clear plant Atom per-tick sym lasts (organism pulse).
pub fn clear_plant_sym_flow_lasts(atoms: &mut [Atom]) {
    for atom in atoms.iter_mut() {
        atom.sym_water_recv_last = 0;
        atom.sym_sugar_paid_last = 0;
        atom.sym_water_sent_last = 0;
        atom.sym_sugar_recv_last = 0;
    }
}

/// Clear network per-tick sym lasts (call once per world tick before
/// mycelium field + organism symbiosis so strain and plant writes share
/// one "last" window).
pub fn clear_sym_net_flow_lasts(world: &mut World) {
    for flow in world.sym_net_flow.values_mut() {
        flow.water_out_last = 0;
        flow.sugar_in_last = 0;
        flow.water_in_last = 0;
        flow.sugar_out_last = 0;
    }
}

/// Root↔cream contact: the root cell and its Moore neighbourhood.
const ROOT_CREAM_NEIGHBORS: [(i32, i32); 9] = [
    (0, 0),
    (0, -1),
    (0, 1),
    (1, 0),
    (-1, 0),
    (1, -1),
    (-1, -1),
    (1, 1),
    (-1, 1),
];
/// When inspecting cream that isn't the contact cell, still report a plant
/// linked to the same strain within this Chebyshev radius.
/// Sized for deep root spans (roots commonly dive 8–12 under the crown).
const CREAM_LINK_SCAN: i32 = 12;

fn plant_root_cells(world: &World, atom: &Atom) -> Vec<(i32, i32)> {
    atom.body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Root)
        .map(|&(dx, dy, _)| {
            (
                world.wrap_x(atom.gx + dx as i32),
                atom.gy + dy as i32,
            )
        })
        .collect()
}

fn cream_touches_root(world: &World, cx: i32, cy: i32, roots: &[(i32, i32)]) -> bool {
    for &(rx, ry) in roots {
        for (dx, dy) in ROOT_CREAM_NEIGHBORS {
            if world.wrap_x(rx + dx) == cx && ry + dy == cy {
                return true;
            }
        }
    }
    false
}

/// True when a living Root module occupies the cream cell itself.
fn root_cohabits_cream(world: &World, cx: i32, cy: i32, roots: &[(i32, i32)]) -> bool {
    let cx = world.wrap_x(cx);
    roots
        .iter()
        .any(|&(rx, ry)| world.wrap_x(rx) == cx && ry == cy)
}

/// True when `roots` touch this cream cell, or same-strain cream nearby.
///
/// Returns `(touching_here, via_network, cohabit)`.
fn cream_link_to_roots(
    world: &World,
    cx: i32,
    cy: i32,
    strain: Option<u32>,
    roots: &[(i32, i32)],
) -> (bool, bool, bool) {
    let cohabit = root_cohabits_cream(world, cx, cy, roots);
    if cream_touches_root(world, cx, cy, roots) {
        return (true, false, cohabit);
    }
    let Some(strain) = strain else {
        return (false, false, cohabit);
    };
    for dx in -CREAM_LINK_SCAN..=CREAM_LINK_SCAN {
        for dy in -CREAM_LINK_SCAN..=CREAM_LINK_SCAN {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = world.wrap_x(cx + dx);
            let ny = cy + dy;
            let Some(c) = world.get_cell(nx, ny) else {
                continue;
            };
            if c.mycelium() == 0 {
                continue;
            }
            if mycelium_strain_at(world, nx, ny) != Some(strain) {
                continue;
            }
            if cream_touches_root(world, nx, ny, roots) {
                return (true, true, cohabit);
            }
        }
    }
    (false, false, cohabit)
}

fn plant_ledger_probe(atom: &Atom) -> (u8, u8, u32, u32, u8, u8, u32, u32) {
    (
        atom.sym_water_recv_last,
        atom.sym_sugar_paid_last,
        atom.sym_water_recv_total,
        atom.sym_sugar_paid_total,
        atom.sym_water_sent_last,
        atom.sym_sugar_recv_last,
        atom.sym_water_sent_total,
        atom.sym_sugar_recv_total,
    )
}

fn net_ledger_probe(flow: SymNetFlow) -> (u8, u8, u32, u32, u8, u8, u32, u32) {
    (
        flow.water_out_last,
        flow.sugar_in_last,
        flow.water_out_total,
        flow.sugar_in_total,
        flow.water_in_last,
        flow.sugar_out_last,
        flow.water_in_total,
        flow.sugar_out_total,
    )
}

/// Probe cream at `(gx,gy)` for a nearby Symbiont plant partner.
///
/// Returns `None` when the cell has no Symbiont lineage (not a symbiont network).
pub fn probe_cream_link(world: &World, gx: i32, gy: i32, atoms: &[Atom]) -> Option<SymProbe> {
    probe_cream_link_preferring(world, gx, gy, atoms, None)
}

/// [`probe_cream_link`] but prefers a plant store index when ranking partners.
///
/// Used by the inspector so a cohabiting / clicked plant is not drowned out
/// by a better-matching plant elsewhere on the strain.
pub fn probe_cream_link_preferring(
    world: &World,
    gx: i32,
    gy: i32,
    atoms: &[Atom],
    prefer_plant: Option<usize>,
) -> Option<SymProbe> {
    let gx = world.wrap_x(gx);
    let c = world.get_cell(gx, gy)?;
    if c.mycelium() == 0 {
        return None;
    }
    let strain = mycelium_strain_at(world, gx, gy);
    let lin = match strain {
        Some(s) => lineage_for_strain_at(world, s, gx, gy)?,
        None => nearest_mycelium_lineage(world, gx, gy)?,
    };
    if !body_has_symbiont(&lin.body) {
        return None;
    }
    let flow = strain.map(|s| net_flow_at(world, s)).unwrap_or_default();
    let (wl, sl, wt, st, wrl, srl, wrt, srt) = net_ledger_probe(flow);
    let mut best: Option<SymProbe> = None;
    for (idx, atom) in atoms.iter().enumerate() {
        if !is_land_plant(atom) || !body_has_symbiont(&atom.body) {
            continue;
        }
        let roots = plant_root_cells(world, atom);
        if roots.is_empty() {
            continue;
        }
        let (touching, via_network, cohabit) =
            cream_link_to_roots(world, gx, gy, strain, &roots);
        let match_q = treaty_match(atom.genome, lin.genome);
        let mode = if touching && !via_network {
            let mut m = SymTradeMode::Supply;
            for &(rx, ry) in &roots {
                for (dx, dy) in ROOT_CREAM_NEIGHBORS {
                    if world.wrap_x(rx + dx) == gx && ry + dy == gy {
                        m = trade_mode_at(world, rx, ry, gx, gy);
                    }
                }
            }
            m
        } else if touching {
            // Contact is on nearby same-strain cream — sample moist there.
            let mut m = SymTradeMode::Supply;
            'scan: for dx in -CREAM_LINK_SCAN..=CREAM_LINK_SCAN {
                for dy in -CREAM_LINK_SCAN..=CREAM_LINK_SCAN {
                    let nx = world.wrap_x(gx + dx);
                    let ny = gy + dy;
                    if mycelium_strain_at(world, nx, ny) != strain {
                        continue;
                    }
                    if cream_touches_root(world, nx, ny, &roots) {
                        for &(rx, ry) in &roots {
                            for (ox, oy) in ROOT_CREAM_NEIGHBORS {
                                if world.wrap_x(rx + ox) == nx && ry + oy == ny {
                                    m = trade_mode_at(world, rx, ry, nx, ny);
                                    break 'scan;
                                }
                            }
                        }
                    }
                }
            }
            m
        } else {
            SymTradeMode::Supply
        };
        let mut probe = build_probe(
            touching,
            match_q,
            Some(idx),
            atom.genome,
            lin.genome,
            mode,
            wl,
            sl,
            wt,
            st,
            wrl,
            srl,
            wrt,
            srt,
            strain,
            via_network,
            cohabit,
        );
        apply_plant_reserve_to_probe(&mut probe, atom);
        let preferred = prefer_plant == Some(idx);
        let better = match best {
            None => true,
            Some(b) => {
                let b_pref = prefer_plant == b.plant_idx;
                (preferred && !b_pref && probe.touching)
                    || (probe.cohabit && !b.cohabit)
                    || (probe.linked && !b.linked)
                    || (probe.touching && !b.touching)
                    || (!probe.via_network && b.via_network && probe.touching == b.touching)
                    || (probe.match_q > b.match_q + 1e-4)
            }
        };
        if better {
            best = Some(probe);
        }
    }
    best.or_else(|| {
        Some(build_probe(
            false,
            0.0,
            None,
            lin.genome,
            lin.genome,
            SymTradeMode::Supply,
            wl,
            sl,
            wt,
            st,
            wrl,
            srl,
            wrt,
            srt,
            strain,
            false,
            false,
        ))
    })
}

/// Probe a Symbiont plant for cream contact / exchange potential.
pub fn probe_plant_link(world: &World, atom: &Atom) -> Option<SymProbe> {
    if !is_land_plant(atom) || !body_has_symbiont(&atom.body) {
        return None;
    }
    let (wl, sl, wt, st, wrl, srl, wrt, srt) = plant_ledger_probe(atom);
    let roots = plant_root_cells(world, atom);
    if roots.is_empty() {
        let mut probe = build_probe(
            false,
            0.0,
            None,
            atom.genome,
            atom.genome,
            SymTradeMode::Supply,
            wl,
            sl,
            wt,
            st,
            wrl,
            srl,
            wrt,
            srt,
            None,
            false,
            false,
        );
        apply_plant_reserve_to_probe(&mut probe, atom);
        return Some(probe);
    }
    let mut best: Option<SymProbe> = None;
    for &(rx, ry) in &roots {
        for (dx, dy) in ROOT_CREAM_NEIGHBORS {
            let cx = world.wrap_x(rx + dx);
            let cy = ry + dy;
            let Some(c) = world.get_cell(cx, cy) else {
                continue;
            };
            if c.mycelium() == 0 {
                continue;
            }
            let strain = mycelium_strain_at(world, cx, cy);
            let Some(lin) = (match strain {
                Some(s) => lineage_for_strain_at(world, s, cx, cy),
                None => nearest_mycelium_lineage(world, cx, cy),
            }) else {
                continue;
            };
            if !body_has_symbiont(&lin.body) {
                continue;
            }
            let match_q = treaty_match(atom.genome, lin.genome);
            let mode = trade_mode_at(world, rx, ry, cx, cy);
            let cohabit = dx == 0 && dy == 0;
            let mut probe = build_probe(
                true,
                match_q,
                None,
                atom.genome,
                lin.genome,
                mode,
                wl,
                sl,
                wt,
                st,
                wrl,
                srl,
                wrt,
                srt,
                None,
                false,
                cohabit,
            );
            apply_plant_reserve_to_probe(&mut probe, atom);
            let better = match best {
                None => true,
                Some(b) => {
                    (probe.cohabit && !b.cohabit)
                        || (probe.linked && !b.linked)
                        || (probe.match_q > b.match_q + 1e-4)
                }
            };
            if better {
                best = Some(probe);
            }
        }
    }
    best.or_else(|| {
        let mut probe = build_probe(
            false,
            0.0,
            None,
            atom.genome,
            atom.genome,
            SymTradeMode::Supply,
            wl,
            sl,
            wt,
            st,
            wrl,
            srl,
            wrt,
            srt,
            None,
            false,
            false,
        );
        apply_plant_reserve_to_probe(&mut probe, atom);
        Some(probe)
    })
}

fn give_pore_sat(world: &mut World, gx: i32, gy: i32, amount: u8) -> u8 {
    if amount == 0 {
        return 0;
    }
    let gx = world.wrap_x(gx);
    let Some(mut c) = world.get_cell(gx, gy) else {
        return 0;
    };
    if c.material == wk_material::MaterialId::Air {
        return 0;
    }
    let cap = water_capacity_cell(c, &world.hydro);
    if cap == 0 || c.sat.0 >= cap {
        return 0;
    }
    let room = cap - c.sat.0;
    let give = amount.min(room);
    c.sat.0 = c.sat.0.saturating_add(give);
    world.set_cell(gx, gy, c);
    give
}

fn take_pore_sat(world: &mut World, gx: i32, gy: i32, amount: u8) -> u8 {
    if amount == 0 {
        return 0;
    }
    let gx = world.wrap_x(gx);
    let Some(mut c) = world.get_cell(gx, gy) else {
        return 0;
    };
    if c.material == wk_material::MaterialId::Air || c.sat.0 == 0 {
        return 0;
    }
    let cap = water_capacity_cell(c, &world.hydro);
    if cap == 0 {
        return 0;
    }
    let take = amount.min(c.sat.0);
    c.sat.0 = c.sat.0.saturating_sub(take);
    world.set_cell(gx, gy, c);
    take
}

fn take_root_pore_sat(world: &mut World, rx: i32, ry: i32, amount: u8) -> u8 {
    let mut taken = take_pore_sat(world, rx, ry, amount);
    if taken < amount {
        taken = taken.saturating_add(take_pore_sat(world, rx, ry - 1, amount - taken));
    }
    taken
}

/// Run one symbiotic exchange pulse for all eligible land plants.
///
/// Clears plant lasts only — network lasts are cleared once per world tick
/// via [`clear_sym_net_flow_lasts`] before the mycelium field pulse so
/// strain↔strain and plant↔cream share one inspector "last" window.
///
/// Plants are visited in tick-rotated order so early atom indices (editor
/// plantings) cannot permanently starve later spore / sprout clones when the
/// contact budget is tight. Each plant gets at most
/// [`SYM_PARTNERS_PER_PLANT`] successful cream trades per pulse.
pub fn step(world: &mut World, atoms: &mut [Atom], tick: u64) {
    clear_plant_sym_flow_lasts(atoms);
    let n = atoms.len();
    if n == 0 {
        return;
    }
    let mut budget = SYM_CONTACT_BUDGET;
    let start = (tick as usize) % n;
    for k in 0..n {
        if budget == 0 {
            break;
        }
        let atom = &mut atoms[(start + k) % n];
        if !is_land_plant(atom) || !body_has_symbiont(&atom.body) {
            continue;
        }
        let plant_g = atom.genome;
        let roots = plant_root_cells(world, atom);
        if roots.is_empty() {
            continue;
        }

        let mut partners = 0usize;
        'roots: for &(rx, ry) in &roots {
            if budget == 0 || partners >= SYM_PARTNERS_PER_PLANT {
                break;
            }
            for (dx, dy) in ROOT_CREAM_NEIGHBORS {
                if budget == 0 || partners >= SYM_PARTNERS_PER_PLANT {
                    break 'roots;
                }
                let cx = world.wrap_x(rx + dx);
                let cy = ry + dy;
                let Some(c) = world.get_cell(cx, cy) else {
                    continue;
                };
                if c.mycelium() == 0 {
                    continue;
                }
                let strain = match mycelium_strain_at(world, cx, cy) {
                    Some(s) => s,
                    None => ensure_mycelium_strain(world, cx, cy),
                };
                let Some(lin) = lineage_for_strain_at(world, strain, cx, cy)
                    .or_else(|| nearest_mycelium_lineage(world, cx, cy))
                else {
                    continue;
                };
                if !body_has_symbiont(&lin.body) {
                    continue;
                }
                let match_q = treaty_match(plant_g, lin.genome);
                if match_q < SYM_MATCH_MIN {
                    continue;
                }
                let (w, e) = agreed_treaty(plant_g, lin.genome);
                let (water_want, energy_want, sugar_want) = exchange_rates(match_q, w, e);
                let mode = trade_mode_at(world, rx, ry, cx, cy);

                let mut moved = false;
                match mode {
                    SymTradeMode::Supply => {
                        // Desert contact: pull water from the wider same-strain
                        // network into the cream tip before gifting.
                        if water_want > 0 {
                            let local = world
                                .get_cell(cx, cy)
                                .map(|c| c.sat.0)
                                .unwrap_or(0);
                            if local < water_want {
                                let _ = pull_mycelium_cargo_to(
                                    world,
                                    cx,
                                    cy,
                                    0,
                                    water_want.saturating_sub(local).saturating_add(1),
                                );
                            }
                        }
                        let mut water_moved = 0u8;
                        if water_want > 0 {
                            let taken = take_pore_sat(world, cx, cy, water_want);
                            if taken > 0 {
                                let mut deposited = give_pore_sat(world, rx, ry, taken);
                                if deposited < taken {
                                    deposited +=
                                        give_pore_sat(world, rx, ry - 1, taken - deposited);
                                }
                                water_moved = taken;
                                let _ = deposited;
                            }
                        }

                        // Sugar pay only from surplus above the repro reserve so
                        // a wet network cannot pin plants at starvation energy.
                        let mut sugar_moved = 0u8;
                        if sugar_want > 0
                            && energy_want > 0.01
                            && plant_sym_sugar_spendable(atom) + 1e-4 >= energy_want
                        {
                            atom.energy = (atom.energy - energy_want).max(0.0);
                            add_mycelium_energy(world, cx, cy, sugar_want);
                            sugar_moved = sugar_want;
                        }

                        if water_moved > 0 || sugar_moved > 0 {
                            atom.sym_water_recv_last =
                                atom.sym_water_recv_last.saturating_add(water_moved);
                            atom.sym_sugar_paid_last =
                                atom.sym_sugar_paid_last.saturating_add(sugar_moved);
                            atom.sym_water_recv_total =
                                atom.sym_water_recv_total.saturating_add(water_moved as u32);
                            atom.sym_sugar_paid_total =
                                atom.sym_sugar_paid_total.saturating_add(sugar_moved as u32);
                            record_net_supply(world, strain, water_moved, sugar_moved, tick);
                            moved = true;
                        }
                    }
                    SymTradeMode::Harvest => {
                        // Plant → cream water; network pays sugar into plant energy.
                        let mut water_moved = 0u8;
                        if water_want > 0 {
                            let taken = take_root_pore_sat(world, rx, ry, water_want);
                            if taken > 0 {
                                let deposited = give_pore_sat(world, cx, cy, taken);
                                // Count what left the plant bed.
                                water_moved = taken;
                                // Refund undeliverable water so we don't evaporate it.
                                if deposited < taken {
                                    let _ = give_pore_sat(world, rx, ry, taken - deposited);
                                    water_moved = deposited;
                                }
                            }
                        }

                        let mut sugar_moved = 0u8;
                        if sugar_want > 0 {
                            let available = cream_sugar_payable(world, cx, cy);
                            if available < sugar_want {
                                let _ = pull_mycelium_cargo_to(
                                    world,
                                    cx,
                                    cy,
                                    sugar_want - available,
                                    0,
                                );
                            }
                            let pay = sugar_want.min(cream_sugar_payable(world, cx, cy));
                            if pay > 0 {
                                let taken = take_mycelium_energy(world, cx, cy, pay);
                                atom.energy = (atom.energy
                                    + taken as f32 * MYCELIUM_ENERGY_SIP_TO_ATOM)
                                    .min(atom.energy_max);
                                sugar_moved = taken;
                            }
                        }

                        if water_moved > 0 || sugar_moved > 0 {
                            atom.sym_water_sent_last =
                                atom.sym_water_sent_last.saturating_add(water_moved);
                            atom.sym_sugar_recv_last =
                                atom.sym_sugar_recv_last.saturating_add(sugar_moved);
                            atom.sym_water_sent_total =
                                atom.sym_water_sent_total.saturating_add(water_moved as u32);
                            atom.sym_sugar_recv_total =
                                atom.sym_sugar_recv_total.saturating_add(sugar_moved as u32);
                            record_net_harvest(world, strain, water_moved, sugar_moved, tick);
                            moved = true;
                        }
                    }
                }

                // Empty contacts (saturated pores / banking sugar) must not
                // consume the plant's partner slot or the global budget —
                // keep scanning neighbours for a cream that can actually trade.
                if !moved {
                    continue;
                }
                budget = budget.saturating_sub(1);
                partners = partners.saturating_add(1);
                // One successful cream partner per root; plant cap enforced above.
                break;
            }
        }
    }
}

/// Probe cream at `(gx,gy)` for an adjacent different-strain Symbiont frontier.
///
/// Returns the best cardinal peer (highest treaty match). `None` when this
/// cell has no dominant strain / Symbiont lineage, or no different-strain
/// neighbour. Same-cell multi-share blends are **not** frontiers (deferred).
pub fn probe_strain_frontier(world: &World, gx: i32, gy: i32) -> Option<SymFrontierProbe> {
    let gx = world.wrap_x(gx);
    let c = world.get_cell(gx, gy)?;
    if c.mycelium() == 0 {
        return None;
    }
    let self_strain = mycelium_strain_at(world, gx, gy)?;
    let lin_self = lineage_for_strain_at(world, self_strain, gx, gy)?;
    if !body_has_symbiont(&lin_self.body) {
        return None;
    }
    let moist_self = cell_moist_frac(world, gx, gy);
    let sugar_here = mycelium_energy_at(world, gx, gy);
    let mut best: Option<SymFrontierProbe> = None;
    for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1)] {
        let px = world.wrap_x(gx + dx);
        let py = gy + dy;
        let Some(pc) = world.get_cell(px, py) else {
            continue;
        };
        if pc.mycelium() == 0 {
            continue;
        }
        let Some(peer_strain) = mycelium_strain_at(world, px, py) else {
            continue;
        };
        if peer_strain == self_strain {
            continue;
        }
        let Some(lin_peer) = lineage_for_strain_at(world, peer_strain, px, py) else {
            continue;
        };
        let match_q = if body_has_symbiont(&lin_peer.body) {
            treaty_match(lin_self.genome, lin_peer.genome)
        } else {
            0.0
        };
        let (deal_w, deal_e) = agreed_treaty(lin_self.genome, lin_peer.genome);
        let moist_peer = cell_moist_frac(world, px, py);
        let moist_delta = moist_self - moist_peer;
        let sugar_peer = mycelium_energy_at(world, px, py);
        let can_water = moist_delta.abs() > SYM_MOIST_DELTA;
        let can_sugar_peer = sugar_here.abs_diff(sugar_peer) >= SYM_STRAIN_SUGAR_PEER_MIN;
        let blocked = if !body_has_symbiont(&lin_peer.body) {
            Some("peer no Symbiont")
        } else if match_q < SYM_MATCH_MIN {
            Some("treaty mismatch")
        } else if !can_water && !can_sugar_peer {
            Some("no moist/sugar gradient")
        } else {
            None
        };
        let probe = SymFrontierProbe {
            self_strain,
            peer_strain,
            peer_x: px,
            peer_y: py,
            match_q,
            moist_delta,
            sugar_here,
            sugar_peer,
            deal_w,
            deal_e,
            can_water,
            can_sugar_peer,
            blocked,
        };
        let better = match best {
            None => true,
            Some(b) => {
                (probe.blocked.is_none() && b.blocked.is_some())
                    || (probe.match_q > b.match_q + 1e-4)
            }
        };
        if better {
            best = Some(probe);
        }
    }
    best
}

/// Collect undirected cardinal edges between different dominant strains.
fn collect_strain_frontier_edges(world: &World, tick: u64) -> Vec<(i32, i32, i32, i32, u32, u32)> {
    let mut edges = Vec::new();
    for &(ax, ay) in world.mycelium_strains.keys() {
        let Some(sa) = mycelium_strain_at(world, ax, ay) else {
            continue;
        };
        for (dx, dy) in [(1i32, 0), (0, 1)] {
            let bx = world.wrap_x(ax + dx);
            let by = ay + dy;
            let Some(c_b) = world.get_cell(bx, by) else {
                continue;
            };
            if c_b.mycelium() == 0 {
                continue;
            }
            let Some(sb) = mycelium_strain_at(world, bx, by) else {
                continue;
            };
            if sa == sb {
                continue;
            }
            edges.push((ax, ay, bx, by, sa, sb));
        }
    }
    if edges.is_empty() {
        return edges;
    }
    // Rotate by tick so frontiers share the contact budget over time.
    let rot = (tick as usize).wrapping_mul(17);
    edges.sort_unstable_by_key(|&(ax, ay, bx, by, _, _)| {
        (
            ax.wrapping_add(rot as i32),
            ay.wrapping_add((rot >> 3) as i32),
            bx,
            by,
        )
    });
    edges.truncate(SYM_STRAIN_EDGE_SCAN);
    edges
}

/// Bidirectional strain↔strain trade on adjacent cream frontiers.
///
/// Wetter dominant strain gives pore water; drier pays network sugar.
/// When moisture is nearly equal, a sugar-rich side may trickle sugar to a
/// poorer matching peer (so soaked beds still show frontier exchange).
/// Both sides need Symbiont + [`treaty_match`] ≥ [`SYM_MATCH_MIN`].
/// Hooked from the mycelium field pulse after same-strain cargo equalize.
pub fn step_strain_trade(world: &mut World, tick: u64) {
    let edges = collect_strain_frontier_edges(world, tick);
    if edges.is_empty() {
        return;
    }
    let mut budget = SYM_STRAIN_CONTACT_BUDGET;

    for &(ax, ay, bx, by, sa, sb) in &edges {
        if budget == 0 {
            break;
        }
        let Some(lin_a) = lineage_for_strain_at(world, sa, ax, ay) else {
            continue;
        };
        if !body_has_symbiont(&lin_a.body) {
            continue;
        }
        let Some(lin_b) = lineage_for_strain_at(world, sb, bx, by) else {
            continue;
        };
        if !body_has_symbiont(&lin_b.body) {
            continue;
        }
        let match_q = treaty_match(lin_a.genome, lin_b.genome);
        if match_q < SYM_MATCH_MIN {
            continue;
        }
        let moist_a = cell_moist_frac(world, ax, ay);
        let moist_b = cell_moist_frac(world, bx, by);
        let (w, e) = agreed_treaty(lin_a.genome, lin_b.genome);
        let (water_want, _energy_want, sugar_want) = exchange_rates(match_q, w, e);

        // Wetter gives water; drier pays sugar (same rule as plant trade).
        if moist_a > moist_b + SYM_MOIST_DELTA || moist_b > moist_a + SYM_MOIST_DELTA {
            if water_want == 0 && sugar_want == 0 {
                continue;
            }
            let (wet_xy, dry_xy, wet_strain, dry_strain) = if moist_a > moist_b {
                ((ax, ay), (bx, by), sa, sb)
            } else {
                ((bx, by), (ax, ay), sb, sa)
            };

            let mut water_moved = 0u8;
            if water_want > 0 {
                let local = world
                    .get_cell(wet_xy.0, wet_xy.1)
                    .map(|c| c.sat.0)
                    .unwrap_or(0);
                if local < water_want {
                    let _ = pull_mycelium_cargo_to(
                        world,
                        wet_xy.0,
                        wet_xy.1,
                        0,
                        water_want.saturating_sub(local).saturating_add(1),
                    );
                }
                let taken = take_pore_sat(world, wet_xy.0, wet_xy.1, water_want);
                if taken > 0 {
                    let deposited = give_pore_sat(world, dry_xy.0, dry_xy.1, taken);
                    if deposited < taken {
                        let _ = give_pore_sat(world, wet_xy.0, wet_xy.1, taken - deposited);
                        water_moved = deposited;
                    } else {
                        water_moved = taken;
                    }
                }
            }

            let mut sugar_moved = 0u8;
            if sugar_want > 0 {
                // Paying (drier) strain may pull sugar from its wider network,
                // but keeps a local floor for probes / further trade.
                let available = cream_sugar_payable(world, dry_xy.0, dry_xy.1);
                if available < sugar_want {
                    let _ = pull_mycelium_cargo_to(
                        world,
                        dry_xy.0,
                        dry_xy.1,
                        sugar_want - available,
                        0,
                    );
                }
                let pay = sugar_want.min(cream_sugar_payable(world, dry_xy.0, dry_xy.1));
                if pay > 0 {
                    let taken = take_mycelium_energy(world, dry_xy.0, dry_xy.1, pay);
                    if taken > 0 {
                        add_mycelium_energy(world, wet_xy.0, wet_xy.1, taken);
                        sugar_moved = taken;
                    }
                }
            }

            if water_moved > 0 || sugar_moved > 0 {
                record_net_supply(world, wet_strain, water_moved, sugar_moved, tick);
                record_net_harvest(world, dry_strain, water_moved, sugar_moved, tick);
                budget = budget.saturating_sub(1);
            }
            continue;
        }

        // Near-equal moisture (common in soaked Organic): peer sugar trickle
        // so frontiers still exchange when plant hubs bank sugar unevenly.
        let sug_a = mycelium_energy_at(world, ax, ay);
        let sug_b = mycelium_energy_at(world, bx, by);
        if sug_a.abs_diff(sug_b) < SYM_STRAIN_SUGAR_PEER_MIN {
            continue;
        }
        let (rich_xy, poor_xy, rich_strain, poor_strain) = if sug_a > sug_b {
            ((ax, ay), (bx, by), sa, sb)
        } else {
            ((bx, by), (ax, ay), sb, sa)
        };
        let want = SYM_STRAIN_SUGAR_PEER_MAX
            .min(sug_a.abs_diff(sug_b) / 2)
            .max(1);
        let available = cream_sugar_payable(world, rich_xy.0, rich_xy.1);
        if available < want {
            let _ = pull_mycelium_cargo_to(
                world,
                rich_xy.0,
                rich_xy.1,
                want - available,
                0,
            );
        }
        let pay = want.min(cream_sugar_payable(world, rich_xy.0, rich_xy.1));
        if pay == 0 {
            continue;
        }
        let taken = take_mycelium_energy(world, rich_xy.0, rich_xy.1, pay);
        if taken == 0 {
            continue;
        }
        add_mycelium_energy(world, poor_xy.0, poor_xy.1, taken);
        // Rich pays sugar (harvest book); poor receives (supply book).
        record_net_harvest(world, rich_strain, 0, taken, tick);
        record_net_supply(world, poor_strain, 0, taken, tick);
        budget = budget.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::Genome;
    use crate::cell::{Cell, Sat};
    use crate::chunk::ChunkCoord;
    use crate::fungi::{
        bind_strain_lineage, ensure_mycelium_strain, mycelium_energy_at, mycelium_strain_at,
        stamp_mycelium_lineage,
    };
    use crate::organism::Atom;
    use wk_material::MaterialId;

    fn moist_bed() -> World {
        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(180);
            org.set_mycelium(80);
            w.set_cell(x, 1, org);
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(40);
            w.set_cell(x, 2, sand);
            w.set_cell(x, 3, Cell::air());
        }
        w
    }

    fn wet_root_dry_cream() -> World {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(20); // dry cream
            org.set_mycelium(80);
            w.set_cell(x, 1, org);
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(200); // wet root bed
            w.set_cell(x, 2, sand);
            w.set_cell(x, 3, Cell::air());
        }
        w
    }

    #[test]
    fn identical_treaties_match_fully() {
        let g = Genome::default();
        assert!((treaty_match(g, g) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn opposite_treaties_are_weak() {
        let mut a = Genome::default();
        a.sym_water = 255;
        a.sym_energy = 0;
        let mut b = Genome::default();
        b.sym_water = 0;
        b.sym_energy = 255;
        assert!(treaty_match(a, b) < SYM_MATCH_MIN);
    }

    #[test]
    fn supply_moves_water_and_sugar_on_matching_symbionts() {
        let mut w = moist_bed();
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        let fungus_body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        stamp_mycelium_lineage(&mut w, 4, 1, fungus_g, fungus_body);

        let plant_body = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, 1, ModuleId::Photosystem),
            (1, -1, ModuleId::Symbiont),
        ];
        let mut plant = Atom::from_body(4, 3, 40.0, plant_body);
        plant.genome.sym_water = 200;
        plant.genome.sym_energy = 80;
        // Surplus above repro reserve so sugar pay is allowed.
        plant.energy = 36.0;
        let cream_sat0 = w.get_cell(4, 1).unwrap().sat.0;
        let root_sat0 = w.get_cell(4, 2).unwrap().sat.0;
        let sugar0 = mycelium_energy_at(&w, 4, 1);

        step(&mut w, std::slice::from_mut(&mut plant), 0);

        let cream_sat1 = w.get_cell(4, 1).unwrap().sat.0;
        let root_sat1 = w.get_cell(4, 2).unwrap().sat.0;
        let sugar1 = mycelium_energy_at(&w, 4, 1);
        assert!(
            cream_sat1 < cream_sat0,
            "fungus cream should donate pore water"
        );
        assert!(
            root_sat1 > root_sat0,
            "plant root bed should receive water"
        );
        assert!(sugar1 > sugar0, "cream should bank plant-paid sugar");
        assert!(plant.energy < 36.0, "plant should pay energy");
        assert!(
            plant.sym_water_recv_last > 0 || plant.sym_sugar_paid_last > 0,
            "plant should count actual last-tick flow"
        );
        let strain = mycelium_strain_at(&w, 4, 1).expect("exchange seats a strain");
        let flow = w.sym_net_flow.get(&strain).copied().unwrap_or_default();
        assert!(
            flow.water_out_total > 0 || flow.sugar_in_total > 0,
            "strain network should count supply flow"
        );
        assert_eq!(
            trade_mode_at(&w, 4, 2, 4, 1),
            SymTradeMode::Supply,
            "wet cream vs dry root → supply"
        );
    }

    #[test]
    fn harvest_moves_water_to_cream_and_sugar_to_plant() {
        let mut w = wet_root_dry_cream();
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        stamp_mycelium_lineage(
            &mut w,
            4,
            1,
            fungus_g,
            vec![
                (0, 0, ModuleId::Nucleus),
                (1, 0, ModuleId::Digest),
                (2, 0, ModuleId::Symbiont),
            ],
        );
        add_mycelium_energy(&mut w, 4, 1, 40);
        let mut plant = Atom::from_body(
            4,
            3,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Photosystem),
                (1, -1, ModuleId::Symbiont),
            ],
        );
        plant.genome.sym_water = 200;
        plant.genome.sym_energy = 80;
        plant.energy = 5.0;
        assert_eq!(trade_mode_at(&w, 4, 2, 4, 1), SymTradeMode::Harvest);

        let cream_sat0 = w.get_cell(4, 1).unwrap().sat.0;
        let root_sat0 = w.get_cell(4, 2).unwrap().sat.0;
        let sugar0 = mycelium_energy_at(&w, 4, 1);
        let e0 = plant.energy;

        step(&mut w, std::slice::from_mut(&mut plant), 0);

        assert!(
            w.get_cell(4, 1).unwrap().sat.0 > cream_sat0,
            "cream should receive plant water"
        );
        assert!(
            w.get_cell(4, 2).unwrap().sat.0 < root_sat0,
            "wet root bed should donate water"
        );
        assert!(
            mycelium_energy_at(&w, 4, 1) < sugar0,
            "network should pay sugar"
        );
        assert!(plant.energy > e0, "plant should receive sugar as energy");
        assert!(
            plant.sym_water_sent_total > 0 || plant.sym_sugar_recv_total > 0,
            "plant harvest ledger should move"
        );
        let strain = mycelium_strain_at(&w, 4, 1).expect("strain");
        let flow = w.sym_net_flow.get(&strain).copied().unwrap_or_default();
        assert!(flow.water_in_total > 0 || flow.sugar_out_total > 0);
    }

    #[test]
    fn same_strain_keeps_one_ledger_across_split_cells() {
        let mut w = moist_bed();
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        let fungus_body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        stamp_mycelium_lineage(&mut w, 4, 1, fungus_g, fungus_body.clone());
        stamp_mycelium_lineage(&mut w, 8, 1, fungus_g, fungus_body);
        let strain = ensure_mycelium_strain(&mut w, 4, 1);
        w.mycelium_strains.insert((8, 1), vec![(strain, 80)]);

        let plant_body = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, 1, ModuleId::Photosystem),
            (1, -1, ModuleId::Symbiont),
        ];
        let mut p1 = Atom::from_body(4, 3, 40.0, plant_body.clone());
        p1.genome.sym_water = 200;
        p1.genome.sym_energy = 80;
        p1.energy = 36.0;
        let mut p2 = Atom::from_body(8, 3, 40.0, plant_body);
        p2.genome.sym_water = 200;
        p2.genome.sym_energy = 80;
        p2.energy = 36.0;

        let mut plants = [p1, p2];
        step(&mut w, &mut plants, 0);
        let flow = w.sym_net_flow.get(&strain).copied().unwrap_or_default();
        let plant_w = plants[0].sym_water_recv_total + plants[1].sym_water_recv_total;
        let plant_s = plants[0].sym_sugar_paid_total + plants[1].sym_sugar_paid_total;
        assert_eq!(flow.water_out_total, plant_w, "network water-out = sum plant recv");
        assert_eq!(flow.sugar_in_total, plant_s, "network sugar-in = sum plant paid");
        assert!(flow.water_out_total > 0 || flow.sugar_in_total > 0);
    }

    #[test]
    fn no_exchange_without_symbiont_module() {
        let mut w = moist_bed();
        let fungus_body = vec![(0, 0, ModuleId::Nucleus), (1, 0, ModuleId::Digest)];
        stamp_mycelium_lineage(&mut w, 4, 1, Genome::default(), fungus_body);

        let plant_body = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, 1, ModuleId::Photosystem),
            (1, -1, ModuleId::Symbiont),
        ];
        let mut plant = Atom::from_body(4, 3, 40.0, plant_body);
        plant.energy = 30.0;
        let sugar0 = mycelium_energy_at(&w, 4, 1);
        let e0 = plant.energy;
        step(&mut w, std::slice::from_mut(&mut plant), 0);
        assert_eq!(mycelium_energy_at(&w, 4, 1), sugar0);
        assert_eq!(plant.energy, e0);
    }

    #[test]
    fn probe_reports_linked_exchange_on_matching_contact() {
        let mut w = moist_bed();
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        stamp_mycelium_lineage(
            &mut w,
            4,
            1,
            fungus_g,
            vec![
                (0, 0, ModuleId::Nucleus),
                (1, 0, ModuleId::Digest),
                (2, 0, ModuleId::Symbiont),
            ],
        );
        let mut plant = Atom::from_body(
            4,
            3,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Photosystem),
                (1, -1, ModuleId::Symbiont),
            ],
        );
        plant.genome.sym_water = 200;
        plant.genome.sym_energy = 80;
        let cream = probe_cream_link(&w, 4, 1, std::slice::from_ref(&plant)).expect("symbiont cream");
        assert!(cream.touching);
        assert!(cream.linked);
        assert!(cream.match_q >= SYM_MATCH_MIN);
        assert!(cream.water_per_tick > 0 || cream.sugar_per_tick > 0);
        assert_eq!(cream.bias, SymBias::PlantFavoring);
        assert_eq!(cream.trade_mode, SymTradeMode::Supply);
        let plant_p = probe_plant_link(&w, &plant).expect("symbiont plant");
        assert!(plant_p.linked);
        assert_eq!(plant_p.bias, SymBias::PlantFavoring);
    }

    #[test]
    fn empty_cream_contact_does_not_block_later_trade() {
        let mut w = moist_bed();
        // Only two cream tips under the root: first neighbour (0,-1) is an
        // empty Supply contact (equal-dry → no water/sugar move); the side
        // tip can still gift. Wipe other cream so the scan can't cheat.
        for x in 0..16 {
            let mut c = w.get_cell(x, 1).unwrap();
            c.sat = Sat(0);
            c.set_mycelium(0);
            w.set_cell(x, 1, c);
        }
        w.set_cell(4, 2, {
            let mut c = w.get_cell(4, 2).unwrap();
            c.sat = Sat(0);
            c
        });
        w.set_cell(4, 1, {
            let mut c = w.get_cell(4, 1).unwrap();
            c.sat = Sat(0);
            c.set_mycelium(80);
            c
        });
        w.set_cell(5, 1, {
            let mut c = w.get_cell(5, 1).unwrap();
            c.sat = Sat(200);
            c.set_mycelium(80);
            c
        });
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        let body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        stamp_mycelium_lineage(&mut w, 4, 1, fungus_g, body.clone());
        stamp_mycelium_lineage(&mut w, 5, 1, fungus_g, body);
        let strain = ensure_mycelium_strain(&mut w, 4, 1);
        w.mycelium_strains.insert((5, 1), vec![(strain, 80)]);
        bind_strain_lineage(&mut w, strain, fungus_g, vec![(0, 0, ModuleId::Symbiont)]);

        let mut plant = Atom::from_body(
            4,
            3,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Photosystem),
                (1, -1, ModuleId::Symbiont),
            ],
        );
        plant.genome.sym_water = 200;
        plant.genome.sym_energy = 80;
        // Banking — no sugar pay; water gift must still find the wet cream.
        plant.energy = 0.5;
        let root_sat0 = w.get_cell(4, 2).unwrap().sat.0;

        step(&mut w, std::slice::from_mut(&mut plant), 0);

        assert!(
            w.get_cell(4, 2).unwrap().sat.0 > root_sat0
                || plant.sym_water_recv_last > 0,
            "plant should keep scanning after an empty cream contact; last={}",
            plant.sym_water_recv_last
        );
        assert!(
            plant.sym_water_recv_last > 0 || plant.sym_sugar_paid_last > 0,
            "successful neighbour trade must show on sym plant last"
        );
    }

    #[test]
    fn multi_root_plant_takes_only_one_partner_slot() {
        let mut w = moist_bed();
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        let body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        // Wide cream under a fan of roots.
        for x in 2..10 {
            let mut c = w.get_cell(x, 1).unwrap();
            c.sat = Sat(200);
            c.set_mycelium(80);
            w.set_cell(x, 1, c);
            stamp_mycelium_lineage(&mut w, x, 1, fungus_g, body.clone());
        }
        let strain = ensure_mycelium_strain(&mut w, 4, 1);
        for x in 2..10 {
            w.mycelium_strains.insert((x, 1), vec![(strain, 80)]);
        }
        bind_strain_lineage(&mut w, strain, fungus_g, vec![(0, 0, ModuleId::Symbiont)]);

        let mut roots = vec![(0i16, 0, ModuleId::Nucleus), (0, 1, ModuleId::Photosystem)];
        for dx in -3i16..=3 {
            roots.push((dx, -1, ModuleId::Root));
        }
        roots.push((1, -1, ModuleId::Symbiont));
        let mut plant = Atom::from_body(5, 3, 40.0, roots);
        plant.genome.sym_water = 200;
        plant.genome.sym_energy = 80;
        plant.energy = 0.5; // banking — water gift only
        step(&mut w, std::slice::from_mut(&mut plant), 0);
        assert!(
            plant.sym_water_recv_last > 0,
            "multi-root plant should still trade once"
        );
        assert!(
            plant.sym_water_recv_last <= SYM_WATER_MAX_SAT,
            "one partner/plant: last water {} must fit a single contact",
            plant.sym_water_recv_last
        );
    }

    #[test]
    fn later_clone_still_trades_when_early_plant_is_multi_rooted() {
        let mut w = moist_bed();
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        let body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        for x in 2..12 {
            let mut c = w.get_cell(x, 1).unwrap();
            c.sat = Sat(200);
            c.set_mycelium(80);
            w.set_cell(x, 1, c);
            stamp_mycelium_lineage(&mut w, x, 1, fungus_g, body.clone());
        }
        let strain = ensure_mycelium_strain(&mut w, 4, 1);
        for x in 2..12 {
            w.mycelium_strains.insert((x, 1), vec![(strain, 80)]);
        }
        bind_strain_lineage(&mut w, strain, fungus_g, vec![(0, 0, ModuleId::Symbiont)]);

        let mut early_body = vec![
            (0i16, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Photosystem),
            (4, -1, ModuleId::Symbiont),
        ];
        for dx in -4i16..=4 {
            early_body.push((dx, -1, ModuleId::Root));
        }
        let mut early = Atom::from_body(4, 3, 40.0, early_body);
        early.genome.sym_water = 200;
        early.genome.sym_energy = 80;
        early.energy = 0.5;

        let mut clone = Atom::from_body(
            10,
            3,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Photosystem),
                (1, -1, ModuleId::Symbiont),
            ],
        );
        clone.genome.sym_water = 200;
        clone.genome.sym_energy = 80;
        clone.energy = 0.5;

        let mut plants = [early, clone];
        step(&mut w, &mut plants, 0);
        assert!(
            plants[1].sym_water_recv_last > 0 || plants[1].sym_sugar_paid_last > 0,
            "spore-clone index must not be starved by a multi-root earlier plant"
        );
    }

    #[test]
    fn contact_budget_rotates_start_index_by_tick() {
        // More eligible plants than the old tiny budget would allow; with
        // tick rotation a late index eventually gets a turn even if the
        // global budget is smaller than the grove (simulate via many plants
        // and check a high start tick serves the late clone).
        let mut w = moist_bed();
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        let body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        let n = 8usize;
        for i in 0..n {
            let x = 2 + i as i32;
            let mut c = w.get_cell(x, 1).unwrap();
            c.sat = Sat(200);
            c.set_mycelium(80);
            w.set_cell(x, 1, c);
            stamp_mycelium_lineage(&mut w, x, 1, fungus_g, body.clone());
        }
        let strain = ensure_mycelium_strain(&mut w, 2, 1);
        for i in 0..n {
            let x = 2 + i as i32;
            w.mycelium_strains.insert((x, 1), vec![(strain, 80)]);
        }
        bind_strain_lineage(&mut w, strain, fungus_g, vec![(0, 0, ModuleId::Symbiont)]);

        let plant_body = vec![
            (0i16, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, 1, ModuleId::Photosystem),
            (1, -1, ModuleId::Symbiont),
        ];
        let mut plants: Vec<Atom> = (0..n)
            .map(|i| {
                let mut p = Atom::from_body(2 + i as i32, 3, 40.0, plant_body.clone());
                p.genome.sym_water = 200;
                p.genome.sym_energy = 80;
                p.energy = 0.5;
                p
            })
            .collect();

        // Start at index 5 — that plant must be among the first served.
        step(&mut w, &mut plants, 5);
        assert!(
            plants[5].sym_water_recv_last > 0,
            "tick-rotated start should serve plant[5] on tick 5"
        );
    }

    #[test]
    fn organism_sym_step_preserves_network_lasts() {
        let mut w = moist_bed();
        let strain = 7u32;
        w.sym_net_flow.insert(
            strain,
            SymNetFlow {
                water_out_last: 9,
                sugar_in_last: 3,
                water_in_last: 2,
                sugar_out_last: 4,
                water_out_total: 9,
                sugar_in_total: 3,
                water_in_total: 2,
                sugar_out_total: 4,
                last_tick: 1,
            },
        );
        // No land plants — step only clears Atom lasts.
        step(&mut w, &mut [], 2);
        let flow = w.sym_net_flow.get(&strain).copied().unwrap();
        assert_eq!(flow.water_out_last, 9);
        assert_eq!(flow.sugar_in_last, 3);
        assert_eq!(flow.water_in_last, 2);
        assert_eq!(flow.sugar_out_last, 4);
        clear_sym_net_flow_lasts(&mut w);
        let flow = w.sym_net_flow.get(&strain).copied().unwrap();
        assert_eq!(flow.water_out_last, 0);
        assert_eq!(flow.sugar_in_last, 0);
        assert_eq!(flow.water_out_total, 9, "totals must survive last clear");
    }

    #[test]
    fn supply_skips_sugar_when_plant_below_repro_reserve() {
        let mut w = moist_bed();
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        stamp_mycelium_lineage(
            &mut w,
            4,
            1,
            fungus_g,
            vec![
                (0, 0, ModuleId::Nucleus),
                (1, 0, ModuleId::Digest),
                (2, 0, ModuleId::Symbiont),
            ],
        );
        let mut plant = Atom::from_body(
            4,
            3,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Photosystem),
                (1, -1, ModuleId::Symbiont),
            ],
        );
        plant.genome.sym_water = 200;
        plant.genome.sym_energy = 80;
        // Well below 0.72 * 40 = 28.8 reserve — classic "drained by network".
        plant.energy = 0.5;
        let sugar0 = mycelium_energy_at(&w, 4, 1);
        let root_sat0 = w.get_cell(4, 2).unwrap().sat.0;

        step(&mut w, std::slice::from_mut(&mut plant), 0);

        assert_eq!(plant.energy, 0.5, "banking plant must not pay sugar");
        assert_eq!(
            mycelium_energy_at(&w, 4, 1),
            sugar0,
            "network must not receive sugar from a banking plant"
        );
        assert!(
            w.get_cell(4, 2).unwrap().sat.0 >= root_sat0,
            "water gift may still arrive while plant banks"
        );
        assert_eq!(plant.sym_sugar_paid_total, 0);
        let probe = probe_plant_link(&w, &plant).expect("symbiont plant");
        assert!(probe.sugar_banking);
        assert!(probe.energy_reserve > 20.0);
        assert_eq!(probe.sugar_per_tick, 0);
    }

    #[test]
    fn supply_never_drains_plant_below_repro_reserve() {
        let mut w = moist_bed();
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 200; // greedy sugar ask
        stamp_mycelium_lineage(
            &mut w,
            4,
            1,
            fungus_g,
            vec![
                (0, 0, ModuleId::Nucleus),
                (1, 0, ModuleId::Digest),
                (2, 0, ModuleId::Symbiont),
            ],
        );
        let mut plant = Atom::from_body(
            4,
            3,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Photosystem),
                (1, -1, ModuleId::Symbiont),
            ],
        );
        plant.genome.sym_water = 200;
        plant.genome.sym_energy = 200;
        plant.energy = 36.0;
        let reserve = plant_sym_energy_reserve(&plant);
        for tick in 0..200 {
            step(&mut w, std::slice::from_mut(&mut plant), tick);
        }
        assert!(
            plant.energy + 1e-3 >= reserve,
            "after sustained supply, energy {e} must stay at/above reserve {reserve}",
            e = plant.energy
        );
    }

    #[test]
    fn cream_probe_reports_cohabit_when_root_shares_cell() {
        let mut w = moist_bed();
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        stamp_mycelium_lineage(
            &mut w,
            4,
            1,
            fungus_g,
            vec![
                (0, 0, ModuleId::Nucleus),
                (1, 0, ModuleId::Digest),
                (2, 0, ModuleId::Symbiont),
            ],
        );
        // Nucleus above cream; Root painted ON the cream cell (same block).
        let mut plant = Atom::from_body(
            4,
            2,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root), // world (4,1) = cream
                (0, 1, ModuleId::Photosystem),
                (1, -1, ModuleId::Symbiont),
            ],
        );
        plant.genome.sym_water = 200;
        plant.genome.sym_energy = 80;
        let cream = probe_cream_link(&w, 4, 1, std::slice::from_ref(&plant)).expect("cream");
        assert!(cream.touching);
        assert!(cream.linked);
        assert!(cream.cohabit, "shared root+cream cell must flag cohabit");
        assert!(!cream.via_network);
        let plant_p = probe_plant_link(&w, &plant).expect("plant");
        assert!(plant_p.cohabit);
    }

    #[test]
    fn cream_probe_via_network_reaches_deep_same_strain_cream() {
        let mut w = moist_bed();
        // Extend moist bed upward so a deep cream column exists.
        for y in 4..=10 {
            for x in 0..16 {
                let mut org = Cell::solid(MaterialId::Organic);
                org.sat = Sat(180);
                org.set_mycelium(80);
                w.set_cell(x, y, org);
            }
        }
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        let body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        stamp_mycelium_lineage(&mut w, 4, 1, fungus_g, body);
        let strain = ensure_mycelium_strain(&mut w, 4, 1);
        // Same strain cream 8 cells away (beyond the old scan of 4).
        w.set_cell(4, 9, {
            let mut c = w.get_cell(4, 9).unwrap();
            c.set_mycelium(80);
            c
        });
        w.mycelium_strains.insert((4, 9), vec![(strain, 80)]);
        bind_strain_lineage(
            &mut w,
            strain,
            fungus_g,
            vec![(0, 0, ModuleId::Symbiont)],
        );

        let mut plant = Atom::from_body(
            4,
            3,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root), // contacts cream at (4,1)/(4,2)
                (0, 1, ModuleId::Photosystem),
                (1, -1, ModuleId::Symbiont),
            ],
        );
        plant.genome.sym_water = 200;
        plant.genome.sym_energy = 80;

        let deep = probe_cream_link(&w, 4, 9, std::slice::from_ref(&plant)).expect("deep cream");
        assert!(deep.linked, "deep same-strain cream should report linked");
        assert!(deep.via_network);
        assert!(!deep.cohabit);
    }

    #[test]
    fn cream_probe_reports_via_network_when_contact_is_nearby() {
        let mut w = moist_bed();
        let mut fungus_g = Genome::default();
        fungus_g.sym_water = 200;
        fungus_g.sym_energy = 80;
        let body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        stamp_mycelium_lineage(&mut w, 4, 1, fungus_g, body);
        let strain = ensure_mycelium_strain(&mut w, 4, 1);
        // Same strain cream two cells away from the root contact.
        w.set_cell(6, 1, {
            let mut c = w.get_cell(6, 1).unwrap();
            c.set_mycelium(80);
            c
        });
        w.mycelium_strains.insert((6, 1), vec![(strain, 80)]);
        bind_strain_lineage(&mut w, strain, fungus_g, vec![(0, 0, ModuleId::Symbiont)]);

        let mut plant = Atom::from_body(
            4,
            3,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Photosystem),
                (1, -1, ModuleId::Symbiont),
            ],
        );
        plant.genome.sym_water = 200;
        plant.genome.sym_energy = 80;

        let contact = probe_cream_link(&w, 4, 1, std::slice::from_ref(&plant)).expect("contact");
        assert!(contact.linked);
        assert!(!contact.via_network);

        let nearby = probe_cream_link(&w, 6, 1, std::slice::from_ref(&plant)).expect("nearby");
        assert!(nearby.linked, "same-strain cream near contact should report linked");
        assert!(nearby.via_network, "non-contact cream should flag via_network");
    }

    #[test]
    fn matching_strains_trade_water_for_sugar_across_frontier() {
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 3, Cell::air());
        }
        // Wet cream A | dry cream B — different strains, matching treaties.
        let mut wet = Cell::solid(MaterialId::Organic);
        wet.sat = Sat(200);
        wet.set_mycelium(80);
        w.set_cell(4, 2, wet);
        let mut dry = Cell::solid(MaterialId::Organic);
        dry.sat = Sat(20);
        dry.set_mycelium(80);
        w.set_cell(5, 2, dry);

        let sa = ensure_mycelium_strain(&mut w, 4, 2);
        let sb = {
            let id = w.next_mycelium_strain_id.max(1);
            w.next_mycelium_strain_id = id.wrapping_add(1).max(1);
            w.mycelium_strains.insert((5, 2), vec![(id, 80)]);
            id
        };
        assert_ne!(sa, sb);

        let mut g = Genome::default();
        g.sym_water = 200;
        g.sym_energy = 80;
        let body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        bind_strain_lineage(&mut w, sa, g, body.clone());
        bind_strain_lineage(&mut w, sb, g, body);
        stamp_mycelium_lineage(&mut w, 4, 2, g, vec![(0, 0, ModuleId::Symbiont)]);
        stamp_mycelium_lineage(&mut w, 5, 2, g, vec![(0, 0, ModuleId::Symbiont)]);
        add_mycelium_energy(&mut w, 5, 2, 40); // dry side can pay

        let wet0 = w.get_cell(4, 2).unwrap().sat.0;
        let dry0 = w.get_cell(5, 2).unwrap().sat.0;
        let sugar_wet0 = mycelium_energy_at(&w, 4, 2);
        let sugar_dry0 = mycelium_energy_at(&w, 5, 2);

        step_strain_trade(&mut w, 0);

        assert!(
            w.get_cell(4, 2).unwrap().sat.0 < wet0,
            "wet strain should donate water"
        );
        assert!(
            w.get_cell(5, 2).unwrap().sat.0 > dry0,
            "dry strain should receive water"
        );
        assert!(
            mycelium_energy_at(&w, 5, 2) < sugar_dry0,
            "dry strain should pay sugar"
        );
        assert!(
            mycelium_energy_at(&w, 4, 2) > sugar_wet0,
            "wet strain should receive sugar"
        );
        let fa = w.sym_net_flow.get(&sa).copied().unwrap_or_default();
        let fb = w.sym_net_flow.get(&sb).copied().unwrap_or_default();
        assert!(fa.water_out_total > 0 || fa.sugar_in_total > 0);
        assert!(fb.water_in_total > 0 || fb.sugar_out_total > 0);
    }

    #[test]
    fn equal_moist_strains_peer_sugar_across_frontier() {
        let mut w = World::new(11);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 4..=5 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(180); // equal moisture — old path skipped entirely
            org.set_mycelium(80);
            w.set_cell(x, 2, org);
        }
        let sa = ensure_mycelium_strain(&mut w, 4, 2);
        let sb = {
            let id = w.next_mycelium_strain_id.max(1);
            w.next_mycelium_strain_id = id.wrapping_add(1).max(1);
            w.mycelium_strains.insert((5, 2), vec![(id, 80)]);
            id
        };
        let mut g = Genome::default();
        g.sym_water = 200;
        g.sym_energy = 80;
        let body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        bind_strain_lineage(&mut w, sa, g, body.clone());
        bind_strain_lineage(&mut w, sb, g, body);
        add_mycelium_energy(&mut w, 4, 2, 40); // rich
        add_mycelium_energy(&mut w, 5, 2, 4); // poor
        let probe0 = probe_strain_frontier(&w, 4, 2).expect("frontier");
        assert_eq!(probe0.peer_strain, sb);
        assert!(probe0.can_sugar_peer);
        assert!(probe0.blocked.is_none());
        let rich0 = mycelium_energy_at(&w, 4, 2);
        let poor0 = mycelium_energy_at(&w, 5, 2);

        step_strain_trade(&mut w, 0);

        assert!(
            mycelium_energy_at(&w, 4, 2) < rich0,
            "rich equal-moist strain should trickle sugar"
        );
        assert!(
            mycelium_energy_at(&w, 5, 2) > poor0,
            "poor equal-moist strain should receive sugar"
        );
    }

    #[test]
    fn probe_strain_frontier_reports_blocked_treaty() {
        let mut w = World::new(12);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 4..=5 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(100);
            org.set_mycelium(80);
            w.set_cell(x, 2, org);
        }
        let sa = ensure_mycelium_strain(&mut w, 4, 2);
        let sb = {
            let id = w.next_mycelium_strain_id.max(1);
            w.next_mycelium_strain_id = id.wrapping_add(1).max(1);
            w.mycelium_strains.insert((5, 2), vec![(id, 80)]);
            id
        };
        let mut ga = Genome::default();
        ga.sym_water = 255;
        ga.sym_energy = 0;
        let mut gb = Genome::default();
        gb.sym_water = 0;
        gb.sym_energy = 255;
        let body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        bind_strain_lineage(&mut w, sa, ga, body.clone());
        bind_strain_lineage(&mut w, sb, gb, body);
        let probe = probe_strain_frontier(&w, 4, 2).expect("frontier geometry");
        assert_eq!(probe.peer_strain, sb);
        assert_eq!(probe.blocked, Some("treaty mismatch"));
    }

    #[test]
    fn mismatched_strain_treaties_do_not_trade() {
        let mut w = World::new(10);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 4..=5 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = if x == 4 { Sat(200) } else { Sat(20) };
            org.set_mycelium(80);
            w.set_cell(x, 2, org);
        }
        let sa = ensure_mycelium_strain(&mut w, 4, 2);
        let sb = {
            let id = w.next_mycelium_strain_id.max(1);
            w.next_mycelium_strain_id = id.wrapping_add(1).max(1);
            w.mycelium_strains.insert((5, 2), vec![(id, 80)]);
            id
        };
        let mut ga = Genome::default();
        ga.sym_water = 255;
        ga.sym_energy = 0;
        let mut gb = Genome::default();
        gb.sym_water = 0;
        gb.sym_energy = 255;
        let body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (2, 0, ModuleId::Symbiont),
        ];
        bind_strain_lineage(&mut w, sa, ga, body.clone());
        bind_strain_lineage(&mut w, sb, gb, body);
        add_mycelium_energy(&mut w, 5, 2, 40);
        let sugar0 = mycelium_energy_at(&w, 5, 2);
        step_strain_trade(&mut w, 0);
        assert_eq!(mycelium_energy_at(&w, 5, 2), sugar0);
        assert!(w.sym_net_flow.get(&sa).is_none());
        assert!(w.sym_net_flow.get(&sb).is_none());
    }
}
