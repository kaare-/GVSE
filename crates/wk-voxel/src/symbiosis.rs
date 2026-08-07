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
use crate::cell::water_capacity;
use crate::fungi::{
    add_mycelium_energy, ensure_mycelium_strain, lineage_for_strain_at, mycelium_energy_at,
    mycelium_strain_at, nearest_mycelium_lineage, pull_mycelium_cargo_to, take_mycelium_energy,
    MYCELIUM_CARGO_EQUALIZE_MAX, MYCELIUM_ENERGY_SIP_TO_ATOM,
};
use crate::grid::World;
use crate::organism::{Atom, BodyModule, ModuleId};
use crate::plant::is_land_plant;

/// Soft cap on strain-keyed network flow ledger entries.
pub const SYM_NET_FLOW_MAP_MAX: usize = 8_192;

/// Minimum treaty similarity (0..1) before any exchange fires.
pub const SYM_MATCH_MIN: f32 = 0.55;
/// Max pore-sat units transferred per contact per tick (either direction).
pub const SYM_WATER_MAX_SAT: u8 = 2;
/// Max plant energy spent into network sugar per supply contact per tick.
pub const SYM_ENERGY_MAX: f32 = 0.35;
/// Soft cap on plant↔cream contacts resolved per organism tick.
pub const SYM_CONTACT_BUDGET: usize = 48;
/// Soft cap on strain↔strain frontier contacts per mycelium field pulse.
pub const SYM_STRAIN_CONTACT_BUDGET: usize = 48;
/// Treaty byte gap used to label plant- vs fungus-favoring deals.
pub const SYM_BIAS_GAP: u8 = 40;
/// Minimum cream−root moist-frac gap to pick supply vs harvest.
pub const SYM_MOIST_DELTA: f32 = 0.10;

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

fn cell_moist_frac(world: &World, gx: i32, gy: i32) -> f32 {
    let gx = world.wrap_x(gx);
    let Some(c) = world.get_cell(gx, gy) else {
        return 0.0;
    };
    if c.material == wk_material::MaterialId::Air {
        return 0.0;
    }
    let cap = water_capacity(c.material);
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

/// Clear per-tick lasts at the start of an organism symbiosis pulse.
fn clear_sym_flow_lasts(world: &mut World, atoms: &mut [Atom]) {
    for atom in atoms.iter_mut() {
        atom.sym_water_recv_last = 0;
        atom.sym_sugar_paid_last = 0;
        atom.sym_water_sent_last = 0;
        atom.sym_sugar_recv_last = 0;
    }
    for flow in world.sym_net_flow.values_mut() {
        flow.water_out_last = 0;
        flow.sugar_in_last = 0;
        flow.water_in_last = 0;
        flow.sugar_out_last = 0;
    }
}

const ROOT_CREAM_NEIGHBORS: [(i32, i32); 7] = [
    (0, 0),
    (0, -1),
    (0, 1),
    (1, 0),
    (-1, 0),
    (1, -1),
    (-1, -1),
];

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
        let touching = cream_touches_root(world, gx, gy, &roots);
        let match_q = treaty_match(atom.genome, lin.genome);
        let mode = if touching {
            let mut m = SymTradeMode::Supply;
            for &(rx, ry) in &roots {
                for (dx, dy) in ROOT_CREAM_NEIGHBORS {
                    if world.wrap_x(rx + dx) == gx && ry + dy == gy {
                        m = trade_mode_at(world, rx, ry, gx, gy);
                    }
                }
            }
            m
        } else {
            SymTradeMode::Supply
        };
        let probe = build_probe(
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
        );
        let better = match best {
            None => true,
            Some(b) => {
                (probe.linked && !b.linked)
                    || (probe.touching && !b.touching)
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
        return Some(build_probe(
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
        ));
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
            let probe = build_probe(
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
            );
            let better = match best {
                None => true,
                Some(b) => {
                    (probe.linked && !b.linked) || (probe.match_q > b.match_q + 1e-4)
                }
            };
            if better {
                best = Some(probe);
            }
        }
    }
    best.or_else(|| {
        Some(build_probe(
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
        ))
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
    let cap = water_capacity(c.material);
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
    let cap = water_capacity(c.material);
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
pub fn step(world: &mut World, atoms: &mut [Atom], tick: u64) {
    clear_sym_flow_lasts(world, atoms);
    let mut budget = SYM_CONTACT_BUDGET;
    for atom in atoms.iter_mut() {
        if budget == 0 {
            break;
        }
        if !is_land_plant(atom) || !body_has_symbiont(&atom.body) {
            continue;
        }
        let plant_g = atom.genome;
        let roots = plant_root_cells(world, atom);
        if roots.is_empty() {
            continue;
        }

        'roots: for &(rx, ry) in &roots {
            if budget == 0 {
                break;
            }
            for (dx, dy) in ROOT_CREAM_NEIGHBORS {
                if budget == 0 {
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

                match mode {
                    SymTradeMode::Supply => {
                        // Plant must have a little energy to pay.
                        if atom.energy < 0.5 {
                            budget = budget.saturating_sub(1);
                            break;
                        }
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

                        let mut sugar_moved = 0u8;
                        if sugar_want > 0 && energy_want > 0.01 && atom.energy > energy_want {
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
                            let available = mycelium_energy_at(world, cx, cy);
                            if available < sugar_want {
                                let _ = pull_mycelium_cargo_to(
                                    world,
                                    cx,
                                    cy,
                                    sugar_want - available,
                                    0,
                                );
                            }
                            let pay = sugar_want.min(mycelium_energy_at(world, cx, cy));
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
                        }
                    }
                }

                budget = budget.saturating_sub(1);
                // One cream partner per root per tick keeps the pipe thin.
                break;
            }
        }
    }
}

/// Bidirectional strain↔strain trade on adjacent cream frontiers.
///
/// Wetter dominant strain gives pore water; drier pays network sugar.
/// Both sides need Symbiont + [`treaty_match`] ≥ [`SYM_MATCH_MIN`].
/// Hooked from the mycelium field pulse after same-strain cargo equalize.
pub fn step_strain_trade(world: &mut World, tick: u64) {
    // Sample from the strain map (already colonized cells).
    let mut cells: Vec<(i32, i32)> = world.mycelium_strains.keys().copied().collect();
    if cells.is_empty() {
        return;
    }
    // Rotate by tick so frontiers share the budget.
    let rot = (tick as usize).wrapping_mul(17);
    cells.sort_unstable_by_key(|&(x, y)| {
        (x.wrapping_add(rot as i32), y.wrapping_add((rot >> 3) as i32))
    });
    let n = cells.len().min(MYCELIUM_CARGO_EQUALIZE_MAX);
    let mut budget = SYM_STRAIN_CONTACT_BUDGET;

    for &(ax, ay) in cells.iter().take(n) {
        if budget == 0 {
            break;
        }
        let Some(sa) = mycelium_strain_at(world, ax, ay) else {
            continue;
        };
        let Some(lin_a) = lineage_for_strain_at(world, sa, ax, ay) else {
            continue;
        };
        if !body_has_symbiont(&lin_a.body) {
            continue;
        }
        for (dx, dy) in [(1i32, 0), (0, 1)] {
            // Cardinal + process each undirected edge once (east / south).
            if budget == 0 {
                break;
            }
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
            if water_want == 0 && sugar_want == 0 {
                continue;
            }

            // Wetter gives water; drier pays sugar (same rule as plant trade).
            let (wet_xy, dry_xy, wet_strain, dry_strain) =
                if moist_a > moist_b + SYM_MOIST_DELTA {
                    ((ax, ay), (bx, by), sa, sb)
                } else if moist_b > moist_a + SYM_MOIST_DELTA {
                    ((bx, by), (ax, ay), sb, sa)
                } else {
                    continue;
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
                // Paying (drier) strain may pull sugar from its wider network.
                let available = mycelium_energy_at(world, dry_xy.0, dry_xy.1);
                if available < sugar_want {
                    let _ = pull_mycelium_cargo_to(
                        world,
                        dry_xy.0,
                        dry_xy.1,
                        sugar_want - available,
                        0,
                    );
                }
                let pay = sugar_want.min(mycelium_energy_at(world, dry_xy.0, dry_xy.1));
                if pay > 0 {
                    let taken = take_mycelium_energy(world, dry_xy.0, dry_xy.1, pay);
                    if taken > 0 {
                        add_mycelium_energy(world, wet_xy.0, wet_xy.1, taken);
                        sugar_moved = taken;
                    }
                }
            }

            if water_moved > 0 || sugar_moved > 0 {
                // Wet strain: supply side (water out, sugar in).
                record_net_supply(world, wet_strain, water_moved, sugar_moved, tick);
                // Dry strain: harvest side (water in, sugar out).
                record_net_harvest(world, dry_strain, water_moved, sugar_moved, tick);
                budget = budget.saturating_sub(1);
            }
        }
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
        plant.energy = 30.0;
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
        assert!(plant.energy < 30.0, "plant should pay energy");
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
        p1.energy = 30.0;
        let mut p2 = Atom::from_body(8, 3, 40.0, plant_body);
        p2.genome.sym_water = 200;
        p2.genome.sym_energy = 80;
        p2.energy = 30.0;

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
