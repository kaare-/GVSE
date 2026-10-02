//! Save/load sat audit: cell inventory round-trips; HUD pipe sat must not
//! jump when Tab expand (unsaved) differs from the saved book scale.
use wk_voxel::{
    apply_pipe_motor, pipe_mass_sat, sat_totals, stamp_world, CarbonBudget, CloudStore, Humidity,
    OrganismStore, SimSnapshot, SteamConfig, Temperature, Wind, World, WorldgenParams,
    CHUNK_CELLS_W, ChunkCoord,
};

fn climate(params: &WorldgenParams) -> (Humidity, Wind, Temperature) {
    let humidity = Humidity::with_world_bounds(
        4,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
    );
    let wind = Wind::climate(
        4,
        0.05,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.bedrock_floor_y,
        params.sky_ceiling_y,
        params.wrap_x,
    );
    let temperature = Temperature::with_world_bounds(
        4,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.wrap_x,
    );
    (humidity, wind, temperature)
}

#[test]
fn cell_sat_postcard_roundtrip_is_exact() {
    let params = WorldgenParams {
        seed: 3,
        width_cols: CHUNK_CELLS_W as i32 * 2,
        sea_level_y: 30,
        sky_ceiling_y: 64,
        wrap_x: true,
        ..WorldgenParams::default()
    };
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    world.steam.insert((1, 1), 10);
    world.cave_humidity.insert((2, 2), 20);
    world.pipe_steam.insert((3, 3), 960);
    let before = sat_totals(&world);
    let (h, w, t) = climate(&params);
    let loaded = SimSnapshot::from_bytes(
        &SimSnapshot::new(
            params,
            world,
            h,
            w,
            t,
            CloudStore::new(),
            OrganismStore::new(),
            CarbonBudget::default(),
        )
        .to_bytes()
        .unwrap(),
    )
    .unwrap();
    let after = sat_totals(&loaded.world);
    assert_eq!(after.free_air, before.free_air);
    assert_eq!(after.pore, before.pore);
    assert_eq!(after.cell_total, before.cell_total);
}

#[test]
fn hud_pipe_sat_stays_flat_when_settings_expand_halves_after_load() {
    // Play at expand=192, save, "restart" with default settings expand=96,
    // load, run one pipe motor tick — HUD sat= must stay flat (was ~2×).
    let params = WorldgenParams {
        width_cols: 64,
        sky_ceiling_y: 64,
        sea_level_y: 8,
        ..WorldgenParams::default()
    };
    let mut world = World::new(1);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    world.pipe_expand = 192;
    world.pipe_res.insert((5, 5), 192 * 8_000_000);
    let hud_before = pipe_mass_sat(&world);
    assert_eq!(hud_before, 8_000_000);

    let (humidity, wind, temperature) = climate(&params);
    let snap = SimSnapshot::new(
        params,
        world,
        humidity,
        wind,
        temperature,
        CloudStore::new(),
        OrganismStore::new(),
        CarbonBudget::default(),
    );
    let mut loaded = SimSnapshot::from_bytes(&snap.to_bytes().unwrap())
        .unwrap()
        .world;
    assert_eq!(loaded.pipe_expand, 192);
    assert_eq!(pipe_mass_sat(&loaded), 8_000_000);

    let cfg = SteamConfig {
        enable_pipe: true,
        phase_expansion_drive: 96,
        pipe_beat: 1,
        ..SteamConfig::default()
    };
    let mut temp = Temperature::with_world_bounds(4, 0, 0, 64, 64, 1, 64, 8, false);
    loaded.tick = 0;
    apply_pipe_motor(&mut loaded, &mut temp, &cfg, None);

    let hud_after = pipe_mass_sat(&loaded);
    eprintln!(
        "expand →{} hud {}→{} (ratio {:.4})",
        loaded.pipe_expand,
        hud_before,
        hud_after,
        hud_after as f64 / hud_before as f64
    );
    assert_eq!(loaded.pipe_expand, 96);
    let ratio = hud_after as f64 / hud_before as f64;
    // Orphan reclaim may move a little book into cell sat (mass-flat overall).
    // The bug was a ~2× HUD jump from reinterpretation — that must not return.
    assert!(
        (0.95..1.05).contains(&ratio),
        "HUD sat jumped on expand change: {hud_before}→{hud_after} (ratio {ratio})"
    );
}
