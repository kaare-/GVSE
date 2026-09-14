//! Timed leftover / steam soak. Finds the 3 FPS grind.
//!
//! ```text
//! cargo test -p wk-voxel --test leftover_soak_profile --release -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use wk_material::MaterialId;
use wk_voxel::{
    leftover_soak_stats, stamp_world, step_world, water_capacity_cell, CarbonBudget, CarbonConfig,
    Cell, ClimateConfig, CloudConfig, CloudStore, CompetentFallConfig, CondensationConfig,
    EvapConfig, FailureConfig, FungiConfig, GrainConfig, Humidity, KarstConfig, LeftoverSoakStats,
    OrographicConfig, PerfConfig, PhaseConfig, Sat, SteamConfig, Temperature, Wind, World,
    WorldStep, WorldStepConfig, WorldStepTimings, WorldgenParams, CHUNK_CELLS_H, CHUNK_CELLS_W,
    PHASE_EXPANSION_DRIVE_MAX, STEAM_EVERY,
};

fn ms(d: Duration) -> f32 {
    d.as_secs_f32() * 1000.0
}

fn print_stats(label: &str, leftover: Duration, steam: Duration, stats: LeftoverSoakStats) {
    eprintln!(
        "{label} leftover={:.2}ms steam={:.2}ms zone={} pin={} map={} route={} reuse={} steam_c={} topo={} probe={}/{}/{} split={}/{}/{}",
        ms(leftover),
        ms(steam),
        stats.zone,
        stats.pin,
        stats.map,
        stats.route_set,
        stats.reused_pin,
        stats.steam_cells,
        stats.sky_topo,
        stats.probe_confined,
        stats.probe_open,
        stats.probe_boiler,
        stats.cands_us,
        stats.flood_us,
        stats.lock_us,
    );
}

fn compact_hot_hill() -> (World, Temperature, SteamConfig) {
    let mut w = World::new(0x50A1);
    w.ensure_chunk(wk_voxel::ChunkCoord::new(0, 0));
    let mut wet = Cell::solid(MaterialId::Stone);
    let cap = water_capacity_cell(wet, &w.hydro).max(1);
    wet.sat = Sat(cap);
    for x in 2..62 {
        for y in 0..62 {
            w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
        }
    }
    for x in 8..56 {
        for y in 1..28 {
            w.set_cell(x, y, wet);
        }
    }
    for x in 30..34 {
        for y in 28..56 {
            w.set_cell(x, y, wet);
        }
    }
    for y in 56..62 {
        w.set_cell(31, y, Cell::air());
    }
    let mut hot = Temperature::with_world_bounds(4, 0, 0, 64, 64, w.seed.0, 64, 20, false);
    for v in hot.cells.values_mut() {
        *v = 20.0;
    }
    for x in 8..56 {
        for y in 1..28 {
            let (hx, hy) = hot.tile_of(x, y);
            hot.set_tile_c(hx, hy, 150.0);
        }
    }
    let cfg = SteamConfig {
        enable_pore_boil: true,
        enable_escape: false,
        phase_expansion_drive: PHASE_EXPANSION_DRIVE_MAX,
        boil_point_c: 100.0,
        ..SteamConfig::default()
    };
    (w, hot, cfg)
}

#[test]
#[ignore]
fn leftover_compact_hill_soak() {
    let (mut w, mut hot, cfg) = compact_hot_hill();
    eprintln!("=== leftover compact hill 1400× ===");
    let mut leftover_acc = Duration::ZERO;
    let mut steam_acc = Duration::ZERO;
    let mut n = 0u64;
    for t in 1..=1_200 {
        w.tick = t;
        let t0 = Instant::now();
        wk_voxel::apply_steam(&mut w, &mut hot, &cfg);
        let dt = t0.elapsed();
        // apply_steam is leftover + cadence; cadence is cheap when steam is empty.
        leftover_acc += dt;
        n += 1;
        if t == 1 || t % 200 == 0 {
            print_stats(
                &format!("t={t:4}"),
                leftover_acc / n.max(1) as u32,
                Duration::ZERO,
                leftover_soak_stats(&w),
            );
            leftover_acc = Duration::ZERO;
            steam_acc = Duration::ZERO;
            n = 0;
        }
    }
    let _ = steam_acc;
}

fn short_sky_params() -> WorldgenParams {
    WorldgenParams {
        sky_ceiling_y: (CHUNK_CELLS_H as i32) * 5,
        width_cols: (CHUNK_CELLS_W as i32) * 8,
        ..WorldgenParams::default()
    }
}

fn heat_wet_stone(world: &World, temp: &mut Temperature, celsius: f32) -> usize {
    let mut n = 0usize;
    for (coord, chunk) in &world.chunks {
        if !chunk.has_wet_pores {
            continue;
        }
        let x0 = coord.cx * CHUNK_CELLS_W as i32;
        let y0 = coord.cy * CHUNK_CELLS_H as i32;
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let cell = chunk.get(lx, ly);
                if cell.material == MaterialId::Air || cell.sat.0 == 0 {
                    continue;
                }
                if !wk_voxel::cell::is_competent_rock(cell.material)
                    && cell.material != MaterialId::Stone
                {
                    continue;
                }
                let gx = world.wrap_x(x0 + lx as i32);
                let gy = y0 + ly as i32;
                let (hx, hy) = temp.tile_of(gx, gy);
                temp.set_tile_c(hx, hy, celsius);
                n += 1;
            }
        }
    }
    n
}

#[test]
#[ignore]
fn leftover_short_sky_world_step_soak() {
    let params = short_sky_params();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let mut humidity = Humidity::with_world_bounds(
        4,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
    );
    humidity.wrap_x = params.wrap_x;
    let mut wind = Wind::climate(
        4,
        0.05,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.bedrock_floor_y,
        params.sky_ceiling_y,
        params.wrap_x,
    );
    let mut temperature = Temperature::with_world_bounds(
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
    let heated = heat_wet_stone(&world, &mut temperature, 150.0);
    let steam = SteamConfig {
        phase_expansion_drive: PHASE_EXPANSION_DRIVE_MAX,
        ..SteamConfig::default()
    };
    let mut clouds = CloudStore::new();
    let mut carbon = CarbonBudget::default();
    let climate = ClimateConfig::default();
    let perf = PerfConfig::default();
    let failure = FailureConfig::default();
    let evap = EvapConfig::default();
    let cond = CondensationConfig {
        top_y: params.sky_ceiling_y - 2,
        ..CondensationConfig::default()
    };
    let oro = OrographicConfig {
        width_cols: params.width_cols,
        sea_level_y: params.sea_level_y,
        ..OrographicConfig::default()
    };
    let karst = KarstConfig::default();
    let cloud = CloudConfig::default();
    let phase = PhaseConfig::default();
    let carbon_cfg = CarbonConfig::default();
    let grain = GrainConfig::default();
    let fungi = FungiConfig::default();
    let competent = CompetentFallConfig::default();

    eprintln!("=== short-sky step_world 1400× (heated {heated} wet stone cells) ===");
    let mut leftover_acc = Duration::ZERO;
    let mut steam_acc = Duration::ZERO;
    let mut phase_acc = Duration::ZERO;
    let mut phys_acc = Duration::ZERO;
    let mut wall_acc = Duration::ZERO;
    let mut n = 0u64;
    for t in 1..=400 {
        let cfg = WorldStepConfig {
            perf: &perf,
            failure: &failure,
            evap: &evap,
            cond: &cond,
            oro: Some(&oro),
            karst: &karst,
            cloud: &cloud,
            phase: &phase,
            steam: &steam,
            climate: &climate,
            carbon: &carbon_cfg,
            grain: &grain,
            fungi: &fungi,
            competent: &competent,
            humidity_diffusion_alpha: 0.15,
            sea_level_y: params.sea_level_y,
            sky_ceiling_y: params.sky_ceiling_y,
            evap_on: true,
            cond_rain_on: true,
            karst_on: true,
            organisms_on: false,
        };
        let mut timings = WorldStepTimings::default();
        let t0 = Instant::now();
        let _ = step_world(
            WorldStep {
                world: &mut world,
                humidity: &mut humidity,
                wind: &mut wind,
                temperature: &mut temperature,
                clouds: &mut clouds,
                carbon: &mut carbon,
                organisms: None,
                landscape: None,
                geotech: None,
                support: None,
            },
            &cfg,
            Some(&mut timings),
        );
        wall_acc += t0.elapsed();
        leftover_acc += timings.leftover;
        steam_acc += timings.steam;
        phase_acc += timings.phase;
        phys_acc += timings.physics_tick;
        n += 1;
        if t == 1 || t % 50 == 0 || t % STEAM_EVERY == 0 && t <= 20 {
            let stats = leftover_soak_stats(&world);
            eprintln!(
                "t={t:4} wall={:.1} leftover={:.2} steam={:.2} phase={:.2} phys={:.1} zone={} pin={} steam_c={} topo={} probe={}/{}/{} reuse={} split={}/{}/{}",
                ms(wall_acc / n.max(1) as u32),
                ms(leftover_acc / n.max(1) as u32),
                ms(steam_acc / n.max(1) as u32),
                ms(phase_acc / n.max(1) as u32),
                ms(phys_acc / n.max(1) as u32),
                stats.zone,
                stats.pin,
                stats.steam_cells,
                stats.sky_topo,
                stats.probe_confined,
                stats.probe_open,
                stats.probe_boiler,
                stats.reused_pin,
                stats.cands_us,
                stats.flood_us,
                stats.lock_us,
            );
            leftover_acc = Duration::ZERO;
            steam_acc = Duration::ZERO;
            phase_acc = Duration::ZERO;
            phys_acc = Duration::ZERO;
            wall_acc = Duration::ZERO;
            n = 0;
        }
    }
    let _ = steam;
}
