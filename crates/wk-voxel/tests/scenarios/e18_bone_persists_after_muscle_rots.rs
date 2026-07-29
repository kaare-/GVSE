//! E18 — Bone persists after Muscle / Skin rot (Wave L).
//!
//! Spawn a small creature (Nucleus + Bone + Muscle + Skin), kill it,
//! dissolve the corpse into kind-specific materials, run biological
//! decay: Muscle/Skin → Organic within a few hundred ticks; Bone
//! survives that window and eventually becomes Sand.

use wk_material::MaterialId;
use wk_voxel::{
    apply_biological_decay, dissolve_corpse_to_organic, BiologicalDecayConfig, BodyModule, Cell,
    ChunkCoord, Genome, ModuleId, OrganismStore, World, CORPSE_SETTLE_LAND_TICKS,
};

use crate::helpers::lay_bedrock_floor;

fn animal_body() -> Vec<BodyModule> {
    vec![
        (0, 0, ModuleId::Nucleus),
        (1, 0, ModuleId::Bone),
        (2, 0, ModuleId::Muscle),
        (3, 0, ModuleId::Skin),
    ]
}

#[test]
fn e18_bone_persists_after_muscle_rots() {
    let mut world = World::new(1818);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut world, 64);
    // Dry Air shelf for dissolve painting.
    for x in 8..16 {
        world.set_cell(x, 1, Cell::solid(MaterialId::Sand));
        world.set_cell(x, 2, Cell::air());
    }

    let mut orgs = OrganismStore::new();
    assert!(
        orgs.spawn_blueprint_free(
            &world,
            10,
            2,
            animal_body(),
            40.0,
            Genome::default(),
        )
        .is_ok()
    );
    assert_eq!(orgs.len(), 1);

    // Starve → corpse.
    orgs.atoms[0].energy = 0.0;
    for _ in 0..5 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    assert_eq!(orgs.len(), 0, "starved animal should die");
    assert!(orgs.corpse_count() >= 1, "should leave a corpse");

    // Fast-forward land settle and dissolve.
    orgs.corpses[0].settled_ticks = CORPSE_SETTLE_LAND_TICKS;
    let c = orgs.corpses[0].clone();
    dissolve_corpse_to_organic(&mut world, c.gx, c.gy, &c.body);
    orgs.corpses.clear();

    let bone_xy = (c.gx + 1, c.gy);
    let muscle_xy = (c.gx + 2, c.gy);
    let skin_xy = (c.gx + 3, c.gy);
    assert_eq!(
        world.get_cell(bone_xy.0, bone_xy.1).map(|x| x.material),
        Some(MaterialId::Bone)
    );
    assert_eq!(
        world.get_cell(muscle_xy.0, muscle_xy.1).map(|x| x.material),
        Some(MaterialId::Muscle)
    );
    assert_eq!(
        world.get_cell(skin_xy.0, skin_xy.1).map(|x| x.material),
        Some(MaterialId::Skin)
    );

    // Accelerated decay for the mid-window check (still deterministic).
    let mid = BiologicalDecayConfig {
        muscle_prob: 0.05,
        skin_prob: 0.05,
        bone_prob: 0.0, // hold bone for this phase
        ..BiologicalDecayConfig::default()
    };
    for _ in 0..500 {
        apply_biological_decay(&mut world, &mid);
        world.tick = world.tick.wrapping_add(1);
    }
    assert_eq!(
        world.get_cell(muscle_xy.0, muscle_xy.1).map(|x| x.material),
        Some(MaterialId::Organic),
        "muscle should rot to Organic"
    );
    assert_eq!(
        world.get_cell(skin_xy.0, skin_xy.1).map(|x| x.material),
        Some(MaterialId::Organic),
        "skin should rot to Organic"
    );
    assert_eq!(
        world.get_cell(bone_xy.0, bone_xy.1).map(|x| x.material),
        Some(MaterialId::Bone),
        "bone should still be Bone after soft-tissue window"
    );

    // Long bone meal phase.
    let late = BiologicalDecayConfig {
        muscle_prob: 0.0,
        skin_prob: 0.0,
        bone_prob: 0.01,
        ..BiologicalDecayConfig::default()
    };
    let mut became_sand = false;
    for _ in 0..2_000 {
        apply_biological_decay(&mut world, &late);
        world.tick = world.tick.wrapping_add(1);
        if world.get_cell(bone_xy.0, bone_xy.1).map(|x| x.material) == Some(MaterialId::Sand)
        {
            became_sand = true;
            break;
        }
    }
    assert!(became_sand, "bone should eventually convert to Sand");
}
