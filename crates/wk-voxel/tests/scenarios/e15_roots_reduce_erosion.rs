//! E15 — roots reduce erosion / slumping (voxel port).
//!
//! Legacy oracle: `tests/scenarios/e15_roots_reduce_erosion.rs`.
//! Product intent: sand bound by living Root modules retains more
//! height under repose than a bare twin (column `root_density`).

use wk_material::MaterialId;
use wk_voxel::{
    apply_grain_fall, apply_grain_repose_bound, collect_live_root_world_cells, BodyModule, Cell,
    ChunkCoord, Genome, ModuleId, OrganismStore, World,
};

use crate::helpers::lay_bedrock_floor;

fn tower_peak_y(world: &World, cx: i32) -> i32 {
    (1..=16)
        .rev()
        .find(|&y| world.get_cell(cx, y).map(|c| c.material) == Some(MaterialId::Sand))
        .unwrap_or(0)
}

fn stamp_sand_tower(world: &mut World, cx: i32, height: i32) {
    for y in 1..=height {
        world.set_cell(cx, y, Cell::solid(MaterialId::Sand));
    }
}

fn rooted_tower_body(height: i32) -> Vec<BodyModule> {
    // Nucleus in Air above the tower; roots through every sand cell.
    let nuc_y = height + 1;
    let mut body = vec![
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Photosystem),
    ];
    for y in 1..=height {
        let dy = (y - nuc_y) as i16;
        body.push((0, dy, ModuleId::Root));
    }
    body
}

#[test]
fn e15_roots_reduce_erosion() {
    let height = 6;
    let bare_cx = 20;
    let rooted_cx = 40;
    let width = 64;

    let mut world = World::new(8015);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut world, width);
    stamp_sand_tower(&mut world, bare_cx, height);
    stamp_sand_tower(&mut world, rooted_cx, height);

    let mut orgs = OrganismStore::new();
    assert!(
        orgs.spawn_blueprint_free(
            &world,
            rooted_cx,
            height + 1,
            rooted_tower_body(height),
            80.0,
            Genome::default(),
        )
        .is_ok(),
        "rooted tower plant must spawn"
    );
    let rooted = collect_live_root_world_cells(&orgs.atoms);
    assert_eq!(rooted.len(), height as usize);

    let h0_bare = tower_peak_y(&world, bare_cx);
    let h0_rooted = tower_peak_y(&world, rooted_cx);
    assert_eq!(h0_bare, height);
    assert_eq!(h0_rooted, height);

    // Dry slump — no rain / flow erosion (matches legacy E15).
    for _ in 0..80 {
        apply_grain_fall(&mut world);
        apply_grain_repose_bound(&mut world, Some(&rooted));
    }

    let h1_bare = tower_peak_y(&world, bare_cx);
    let h1_rooted = tower_peak_y(&world, rooted_cx);
    let drop_bare = h0_bare - h1_bare;
    let drop_rooted = h0_rooted - h1_rooted;

    assert!(
        drop_bare > 2,
        "bare tower should slump hard: {h0_bare}→{h1_bare}"
    );
    assert!(
        drop_rooted < drop_bare,
        "rooted tower should hold better: bare_drop={drop_bare} rooted_drop={drop_rooted} \
         (bare {h0_bare}→{h1_bare}, rooted {h0_rooted}→{h1_rooted})"
    );
    assert!(
        h1_rooted > h1_bare,
        "rooted remnant should stay taller (bare={h1_bare} rooted={h1_rooted})"
    );

    eprintln!(
        "E15: bare {h0_bare}→{h1_bare} (drop {drop_bare}) rooted {h0_rooted}→{h1_rooted} (drop {drop_rooted})"
    );
}
