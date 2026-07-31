//! E40b — high `attach_prefer` re-seats on a nearby host Stem (Wave AB).
//!
//! Product intent: sticky Holdfasts snag an adjacent olive when displaced;
//! `attach_prefer = 0` keeps the Wave U "die if unseated" clock.

use wk_material::MaterialId;
use wk_voxel::{
    apply_genome, collect_live_stem_world_cells, is_epiphyte, is_holdfast_anchored, BodyModule,
    Cell, ChunkCoord, Genome, ModuleId, OrganismStore, PixelTraits, World,
};

use crate::helpers::lay_bedrock_floor;

fn moist_floor(world: &mut World, width: i32) {
    lay_bedrock_floor(world, width);
    for x in 0..width {
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = 4;
        world.set_cell(x, 1, sand);
        for y in 2..=8 {
            world.set_cell(x, y, Cell::air());
        }
    }
}

fn host_body() -> Vec<BodyModule> {
    vec![
        (0, -1, ModuleId::Root),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Stem),
        (0, 2, ModuleId::Stem),
        (0, 3, ModuleId::Photosystem),
    ]
}

fn epi_body() -> Vec<BodyModule> {
    vec![
        (0, 0, ModuleId::Holdfast),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Photosystem),
    ]
}

fn epi_traits(attach: f32) -> Vec<PixelTraits> {
    let mut t = vec![PixelTraits::default(); 3];
    t[0].attach_prefer = attach; // Holdfast
    t
}

#[test]
fn e40b_high_attach_reseats_low_dies() {
    let width = 32;
    let mut world = World::new(9042);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 10;
    let host_gy = 2;
    assert!(
        orgs.spawn_blueprint(
            &world,
            hx,
            host_gy,
            host_body(),
            80.0,
            Genome::default(),
        ),
        "host plant must spawn"
    );

    // Upper stem world cell (hx, 4). Seat sticky epi, then a cling-free control
    // on a second host two columns over so both start anchored.
    let stem_y = host_gy + 2;
    assert!(
        orgs.spawn_blueprint_with_traits(
            &world,
            hx,
            stem_y,
            epi_body(),
            epi_traits(0.9),
            40.0,
            None,
        ),
        "sticky epiphyte must seat"
    );

    let hx2 = 14;
    assert!(
        orgs.spawn_blueprint(
            &world,
            hx2,
            host_gy,
            host_body(),
            80.0,
            Genome::default(),
        ),
        "second host must spawn"
    );
    assert!(
        orgs.spawn_blueprint_with_traits(
            &world,
            hx2,
            stem_y,
            epi_body(),
            epi_traits(0.0),
            40.0,
            None,
        ),
        "cling-free epiphyte must seat"
    );
    assert_eq!(orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(), 2);

    // Displace both epiphytes two cells left of their hosts — still within
    // sticky seek radius (1 + floor(0.9×4) = 4) but outside cling-free (0).
    for atom in orgs.atoms.iter_mut().filter(|a| is_epiphyte(a)) {
        atom.gx = world.wrap_x(atom.gx - 2);
        atom.fy = atom.gy as f32;
        atom.drought_ticks = 0;
    }
    let stems = collect_live_stem_world_cells(&world, &orgs.atoms);
    for epi in orgs.atoms.iter().filter(|a| is_epiphyte(a)) {
        assert!(
            !is_holdfast_anchored(epi, &stems, |x| world.wrap_x(x)),
            "displacement must unseat Holdfast"
        );
    }

    for _ in 0..12 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }

    let epis: Vec<_> = orgs.atoms.iter().filter(|a| is_epiphyte(a)).collect();
    assert_eq!(epis.len(), 1, "only the sticky epiphyte should survive");
    let sticky = epis[0];
    assert!(
        (sticky.body_plan.attach_prefer - 0.9).abs() < 1e-4,
        "survivor must be the high-attach lineage"
    );
    let stems = collect_live_stem_world_cells(&world, &orgs.atoms);
    assert!(
        is_holdfast_anchored(sticky, &stems, |x| world.wrap_x(x)),
        "sticky epiphyte must have re-seated on a host Stem"
    );
}

#[test]
fn e40b_habitat_spawn_snags_nearby_stem() {
    let width = 32;
    let mut world = World::new(9043);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 10;
    let host_gy = 2;
    assert!(orgs.spawn_blueprint(
        &world,
        hx,
        host_gy,
        host_body(),
        80.0,
        Genome::default(),
    ));

    // Click two cells beside the upper stem — default attach refuses;
    // sticky genome should snap onto the host.
    let stem_y = host_gy + 2;
    let mut cling = Genome::default();
    cling.attach_prefer = 0.9;
    assert!(
        !orgs.spawn_blueprint(
            &world,
            hx - 2,
            stem_y,
            epi_body(),
            40.0,
            Genome::default(),
        ),
        "default attach_prefer=0 must still refuse a near-miss"
    );
    assert!(
        orgs.spawn_blueprint(&world, hx - 2, stem_y, epi_body(), 40.0, cling),
        "high attach_prefer must snag the nearby Stem"
    );
    let epi = orgs.atoms.iter().find(|a| is_epiphyte(a)).expect("epi");
    let stems = collect_live_stem_world_cells(&world, &orgs.atoms);
    assert!(is_holdfast_anchored(epi, &stems, |x| world.wrap_x(x)));
}

#[test]
fn e40b_apply_genome_paints_holdfast_attach() {
    let mut atom = wk_voxel::Atom::from_body(0, 0, 20.0, epi_body());
    let mut g = Genome::default();
    g.attach_prefer = 0.85;
    apply_genome(&mut atom, g);
    assert!((atom.body_plan.attach_prefer - 0.85).abs() < 1e-4);
    assert!((atom.trait_at(0).attach_prefer - 0.85).abs() < 1e-4);
}
