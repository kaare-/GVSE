//! E39 — litter bloom: soft litter invites fungi, then boom → crash (Wave AD).
//!
//! Product intent: after a plant die-off deposits soft litter past
//! `LITTER_BLOOM_THRESHOLD`, cream hyphae seed spontaneously, spike in
//! count, then starve as the litter bank is exhausted.

use wk_material::MaterialId;
use wk_voxel::{
    add_soft_litter, is_fungus, soft_litter_at, Cell, ChunkCoord, OrganismStore, World,
    LITTER_BLOOM_THRESHOLD,
};

use crate::helpers::lay_bedrock_floor;

fn litter_bed(world: &mut World, width: i32) {
    lay_bedrock_floor(world, width);
    for x in 0..width {
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = 8;
        world.set_cell(x, 1, sand);
        for y in 2..=8 {
            world.set_cell(x, y, Cell::air());
        }
    }
}

#[test]
fn e39_litter_bloom_seeds_then_crashes() {
    let width = 32;
    let mut world = World::new(9039);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    litter_bed(&mut world, width);

    // Simulate a canopy die-off: rich litter on several columns.
    for x in [6, 10, 14, 18, 22] {
        add_soft_litter(&mut world, x, LITTER_BLOOM_THRESHOLD.saturating_add(20));
    }
    let litter0: u32 = [6, 10, 14, 18, 22]
        .iter()
        .map(|&x| soft_litter_at(&world, x) as u32)
        .sum();
    assert!(litter0 >= LITTER_BLOOM_THRESHOLD as u32 * 5);

    let mut orgs = OrganismStore::new();
    let mut peak_fungi = 0usize;
    let mut saw_seed = false;
    for _ in 0..400 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
        let n = orgs.atoms.iter().filter(|a| is_fungus(a)).count();
        peak_fungi = peak_fungi.max(n);
        if n > 0 {
            saw_seed = true;
        }
    }
    assert!(saw_seed, "litter bloom must seed at least one fungus");
    assert!(peak_fungi >= 1, "peak fungi should spike");

    let litter1: u32 = [6, 10, 14, 18, 22]
        .iter()
        .map(|&x| soft_litter_at(&world, x) as u32)
        .sum();
    assert!(
        litter1 < litter0,
        "fungi should draw down the litter bank (before={litter0} after={litter1})"
    );

    // Starve the remaining bank and soak — population should crash.
    for x in 0..width {
        world.soft_litter.remove(&x);
    }
    for _ in 0..900 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    let final_fungi = orgs.atoms.iter().filter(|a| is_fungus(a)).count();
    assert!(
        final_fungi < peak_fungi,
        "fungi should crash after substrate exhaustion (peak={peak_fungi} final={final_fungi})"
    );
}
