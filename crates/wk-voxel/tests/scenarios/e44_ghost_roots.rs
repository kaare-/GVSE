//! E44 — ghost roots: Rock → Organic → Void → Loose preferential path (Wave AC).
//!
//! Product intent: dead-root Organic digested by fungi opens a cavity;
//! loose fill collapses in; PreferentialRootPath makes the next plant
//! re-root faster than a virgin neighbour.

use wk_material::MaterialId;
use wk_voxel::{
    apply_genome, collect_live_root_world_cells, digest_labile, fill_ghost_root_voids,
    leave_dead_roots_in_place, try_elongate_root, Atom, BodyModule, Cell, CellFlags, ChunkCoord,
    Genome, ModuleId, OrganismStore, PlantGrowthCaps, World,
};

use crate::helpers::lay_bedrock_floor;

fn dive_body() -> Vec<BodyModule> {
    vec![
        (0, -1, ModuleId::Root),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Stem),
        (0, 2, ModuleId::Photosystem),
    ]
}

fn deepest_root_y(atom: &Atom) -> i32 {
    atom.body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Root)
        .map(|&(_, dy, _)| atom.gy + dy as i32)
        .min()
        .unwrap_or(atom.gy)
}

fn wet_sand_column(world: &mut World, cx: i32, crown_y: i32) {
    for y in 1..crown_y {
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = if y <= 2 { 220 } else { 40 };
        world.set_cell(cx, y, sand);
    }
    world.set_cell(cx, crown_y, Cell::air());
}

/// Energy spent elongating until a Root reaches `target_y` (or `None`).
fn energy_to_depth(world: &mut World, cx: i32, crown_y: i32, target_y: i32) -> Option<f32> {
    let mut atom = Atom::from_body(cx, crown_y, 200.0, dive_body());
    let mut g = Genome::default();
    g.root_depth_bias = 1.0;
    g.alloc_root = 0.9;
    g.alloc_stem = 0.05;
    g.alloc_leaf = 0.05;
    apply_genome(&mut atom, g);
    atom.energy = 180.0;
    let caps = PlantGrowthCaps {
        max_roots: 32,
        max_stems: 4,
        max_photos: 4,
    };
    let mut spent_total = 0.0f32;
    for _ in 0..64 {
        atom.energy = 180.0;
        let roots = collect_live_root_world_cells(std::slice::from_ref(&atom));
        let spent = try_elongate_root(world, &mut atom, &roots, &caps);
        spent_total += spent;
        if deepest_root_y(&atom) <= target_y {
            return Some(spent_total);
        }
        if spent <= 0.0 {
            break;
        }
    }
    None
}

#[test]
fn e44_ghost_root_lifecycle_rock_organic_void_loose() {
    let width = 32;
    let crown_y = 8;
    let mut world = World::new(9044);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut world, width);

    let cx = 12;
    // Competent Stone under a Sand cap — Rock → Organic → Void → Loose.
    for y in 2..=6 {
        world.set_cell(cx, y, Cell::solid(MaterialId::Stone));
    }
    world.set_cell(cx, 7, Cell::solid(MaterialId::Sand));
    world.set_cell(cx, crown_y, Cell::air());
    let mut wet = Cell::solid(MaterialId::Sand);
    wet.sat.0 = 200;
    world.set_cell(cx, 1, wet);

    let founder = Atom::from_body(
        cx,
        crown_y,
        80.0,
        vec![
            (0, -1, ModuleId::Root),
            (0, -2, ModuleId::Root),
            (0, -3, ModuleId::Root),
            (0, -4, ModuleId::Root),
            (0, -5, ModuleId::Root),
            (0, -6, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
        ],
    );
    let painted = leave_dead_roots_in_place(&mut world, &founder);
    assert!(painted >= 4, "founder roots must paint Organic residue");
    assert!(
        (2..=7).any(|y| {
            world
                .get_cell(cx, y)
                .map(|c| {
                    c.material == MaterialId::Organic && c.flags.contains(CellFlags::ROOT_RESIDUE)
                })
                .unwrap_or(false)
        }),
        "Organic cells must carry ROOT_RESIDUE"
    );

    for _ in 0..48 {
        let _ = digest_labile(&mut world, cx, crown_y, 4);
    }
    let voids: Vec<i32> = (1..=7)
        .filter(|&y| {
            world
                .get_cell(cx, y)
                .map(|c| c.material == MaterialId::Air)
                .unwrap_or(false)
                && world.is_preferential_root(cx, y)
        })
        .collect();
    assert!(!voids.is_empty(), "digest must open preferential Void");

    // Keep feeding Sand from above so the pipe can fill.
    for _ in 0..20 {
        if world
            .get_cell(cx, 7)
            .map(|c| c.material == MaterialId::Air)
            .unwrap_or(false)
        {
            world.set_cell(cx, 7, Cell::solid(MaterialId::Sand));
        }
        let _ = fill_ghost_root_voids(&mut world);
    }
    let loose = (1..=7)
        .filter(|&y| {
            world.is_preferential_root(cx, y)
                && world
                    .get_cell(cx, y)
                    .map(|c| matches!(c.material, MaterialId::Sand | MaterialId::Clay))
                    .unwrap_or(false)
        })
        .count();
    assert!(
        loose >= 1,
        "preferential Void should fill to Loose Sand"
    );
}

#[test]
fn e44_follower_beats_virgin_on_preferential_path() {
    let width = 32;
    let crown_y = 8;
    let target_y = 2;
    let mut world = World::new(9046);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut world, width);

    let virgin_cx = 8;
    let ghost_cx = 16;
    wet_sand_column(&mut world, virgin_cx, crown_y);
    wet_sand_column(&mut world, ghost_cx, crown_y);

    // Stamp preferential memory down the ghost column (post fill state).
    for y in 1..crown_y {
        world.mark_preferential_root(ghost_cx, y);
    }

    let founder_e = energy_to_depth(&mut world, virgin_cx, crown_y, target_y)
        .expect("virgin plant must reach the wet table");
    let follower_e = energy_to_depth(&mut world, ghost_cx, crown_y, target_y)
        .expect("ghost-path plant must reach the wet table");
    assert!(
        follower_e + 0.05 < founder_e,
        "ghost path should cost less energy: follower={follower_e:.2} founder={founder_e:.2}"
    );
}

#[test]
fn e44_organism_step_fills_preferential_voids() {
    let mut world = World::new(9045);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut world, 16);
    world.set_cell(4, 2, Cell::air());
    world.mark_preferential_root(4, 2);
    world.set_cell(4, 3, Cell::solid(MaterialId::Sand));

    let mut orgs = OrganismStore::new();
    orgs.step(&mut world, 0);
    assert_eq!(
        world.get_cell(4, 2).map(|c| c.material),
        Some(MaterialId::Sand)
    );
    assert!(world.is_preferential_root(4, 2));
}
