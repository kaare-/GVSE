//! Plant ↔ fungus symbiotic exchange (opt-in Symbiont module).
//!
//! Both partners must paint [`ModuleId::Symbiont`]. The mutable treaty is
//! Genome `(sym_water, sym_energy)` — an agreed deal, not complementary
//! opposites. Similarity of the two vectors is match quality; a shared
//! lopsided vector is parasitism (high W/low E favours the plant; low W/high E
//! favours the fungus).
//!
//! Exchange runs on root ↔ mycelium-cream contact: fungus pore water → plant
//! root pores; plant `Atom.energy` → network sugar on the cream cell.

use crate::blueprint::Genome;
use crate::cell::water_capacity;
use crate::fungi::{
    add_mycelium_energy, nearest_mycelium_lineage, MYCELIUM_ENERGY_SIP_TO_ATOM,
};
use crate::grid::World;
use crate::organism::{Atom, BodyModule, ModuleId};
use crate::plant::is_land_plant;

/// Minimum treaty similarity (0..1) before any exchange fires.
pub const SYM_MATCH_MIN: f32 = 0.55;
/// Max pore-sat units transferred fungus → plant per contact per tick.
pub const SYM_WATER_MAX_SAT: u8 = 2;
/// Max plant energy spent into network sugar per contact per tick.
pub const SYM_ENERGY_MAX: f32 = 0.35;
/// Soft cap on plant↔cream contacts resolved per organism tick.
pub const SYM_CONTACT_BUDGET: usize = 48;
/// Treaty byte gap used to label plant- vs fungus-favoring deals.
pub const SYM_BIAS_GAP: u8 = 40;

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
    /// Potential pore-sat units fungus → plant per tick.
    pub water_per_tick: u8,
    /// Potential plant energy spent per tick.
    pub energy_per_tick: f32,
    /// Potential network-sugar units banked per tick.
    pub sugar_per_tick: u8,
    pub bias: SymBias,
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

fn build_probe(
    touching: bool,
    match_q: f32,
    plant_idx: Option<usize>,
    plant_g: Genome,
    fungus_g: Genome,
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

/// Probe cream at `(gx,gy)` for a nearby Symbiont plant partner.
///
/// Returns `None` when the cell has no Symbiont lineage (not a symbiont network).
pub fn probe_cream_link(world: &World, gx: i32, gy: i32, atoms: &[Atom]) -> Option<SymProbe> {
    let gx = world.wrap_x(gx);
    let c = world.get_cell(gx, gy)?;
    if c.mycelium() == 0 {
        return None;
    }
    let lin = nearest_mycelium_lineage(world, gx, gy)?;
    if !body_has_symbiont(&lin.body) {
        return None;
    }
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
        let probe = build_probe(touching, match_q, Some(idx), atom.genome, lin.genome);
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
        // Symbiont network with no plant in range — idle treaty readout.
        Some(build_probe(false, 0.0, None, lin.genome, lin.genome))
    })
}

/// Probe a Symbiont plant for cream contact / exchange potential.
pub fn probe_plant_link(world: &World, atom: &Atom) -> Option<SymProbe> {
    if !is_land_plant(atom) || !body_has_symbiont(&atom.body) {
        return None;
    }
    let roots = plant_root_cells(world, atom);
    if roots.is_empty() {
        return Some(build_probe(false, 0.0, None, atom.genome, atom.genome));
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
            let Some(lin) = nearest_mycelium_lineage(world, cx, cy) else {
                continue;
            };
            if !body_has_symbiont(&lin.body) {
                continue;
            }
            let match_q = treaty_match(atom.genome, lin.genome);
            let probe = build_probe(true, match_q, None, atom.genome, lin.genome);
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
    best.or_else(|| Some(build_probe(false, 0.0, None, atom.genome, atom.genome)))
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

/// Run one symbiotic exchange pulse for all eligible land plants.
pub fn step(world: &mut World, atoms: &mut [Atom], _tick: u64) {
    let mut budget = SYM_CONTACT_BUDGET;
    for atom in atoms.iter_mut() {
        if budget == 0 {
            break;
        }
        if !is_land_plant(atom) || !body_has_symbiont(&atom.body) {
            continue;
        }
        if atom.energy < 0.5 {
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
                let Some(lin) = nearest_mycelium_lineage(world, cx, cy) else {
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
                let (water_want, energy_want, sugar) = exchange_rates(match_q, w, e);

                if water_want > 0 {
                    let taken = take_pore_sat(world, cx, cy, water_want);
                    if taken > 0 {
                        let mut deposited = give_pore_sat(world, rx, ry, taken);
                        if deposited < taken {
                            deposited += give_pore_sat(world, rx, ry - 1, taken - deposited);
                        }
                        // Leftover stays nowhere — rare full-pore case; mass
                        // already left the cream cell intentionally as gift.
                        let _ = deposited;
                    }
                }

                if sugar > 0 && energy_want > 0.01 && atom.energy > energy_want {
                    atom.energy = (atom.energy - energy_want).max(0.0);
                    add_mycelium_energy(world, cx, cy, sugar);
                }

                budget = budget.saturating_sub(1);
                // One cream partner per root per tick keeps the pipe thin.
                break;
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
    use crate::fungi::{mycelium_energy_at, stamp_mycelium_lineage};
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
    fn exchange_moves_water_and_sugar_on_matching_symbionts() {
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
    }

    #[test]
    fn no_exchange_without_symbiont_module() {
        let mut w = moist_bed();
        let fungus_body = vec![
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
        ];
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
        let plant_p = probe_plant_link(&w, &plant).expect("symbiont plant");
        assert!(plant_p.linked);
        assert_eq!(plant_p.bias, SymBias::PlantFavoring);
    }
}
