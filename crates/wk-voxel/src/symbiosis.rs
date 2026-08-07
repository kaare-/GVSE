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
        let roots: Vec<(i32, i32)> = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Root)
            .map(|&(dx, dy, _)| {
                (
                    world.wrap_x(atom.gx + dx as i32),
                    atom.gy + dy as i32,
                )
            })
            .collect();
        if roots.is_empty() {
            continue;
        }

        'roots: for &(rx, ry) in &roots {
            if budget == 0 {
                break;
            }
            for (dx, dy) in [
                (0i32, 0),
                (0, -1),
                (0, 1),
                (1, 0),
                (-1, 0),
                (1, -1),
                (-1, -1),
            ] {
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
                // Water offer / energy ask scale with treaty bytes × match.
                let water_want = ((w as f32 / 255.0) * match_q * SYM_WATER_MAX_SAT as f32)
                    .round()
                    .clamp(0.0, SYM_WATER_MAX_SAT as f32) as u8;
                let energy_want =
                    (e as f32 / 255.0) * match_q * SYM_ENERGY_MAX;

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

                if energy_want > 0.01 && atom.energy > energy_want {
                    let sugar = (energy_want / MYCELIUM_ENERGY_SIP_TO_ATOM.max(0.01))
                        .round()
                        .clamp(0.0, 8.0) as u8;
                    if sugar > 0 {
                        atom.energy = (atom.energy - energy_want).max(0.0);
                        add_mycelium_energy(world, cx, cy, sugar);
                    }
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
}
