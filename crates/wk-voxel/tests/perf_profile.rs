//! Headless perf profile for the wk-voxel demo stack.
//!
//! Ignored by default. Run on a laptop before threading work:
//!
//! ```text
//! cargo test -p wk-voxel --test perf_profile --release -- --ignored --nocapture
//! ```
//!
//! Prints ms/tick for the full app-like stack and a per-pass breakdown
//! (rain, evap→humidity, condensation, karst, gravity, lateral spill,
//! grain fall, humidity.diffuse). Two world sizes: demo-default and a
//! 2× wider / taller stress size.

use std::time::{Duration, Instant};

use wk_voxel::{
    apply_condensation_rain, apply_evaporation_into_humidity, apply_grain_fall,
    apply_gravity_fall, apply_karst_dissolution, apply_lateral_spill, apply_rain, stamp_world,
    CondensationConfig, EvapConfig, Humidity, KarstConfig, RainConfig, World, WorldgenParams,
    CHUNK_CELLS_H, CHUNK_CELLS_W,
};

const HUMIDITY_TILE_COLS: i32 = 4;
const HUMIDITY_DIFFUSION_ALPHA: f32 = 0.15;
const WARMUP_TICKS: u64 = 40;
const MEASURE_TICKS: u64 = 200;

struct PassAccum {
    rain: Duration,
    evap: Duration,
    condensation: Duration,
    karst: Duration,
    gravity: Duration,
    spill: Duration,
    grain: Duration,
    humidity_diffuse: Duration,
    /// Bookkeeping that `tick()` does after the three physics passes
    /// (tick bump + clear_dirty). Kept separate so physics numbers
    /// stay comparable to a future threaded checkerboard.
    finish: Duration,
}

impl PassAccum {
    fn zero() -> Self {
        Self {
            rain: Duration::ZERO,
            evap: Duration::ZERO,
            condensation: Duration::ZERO,
            karst: Duration::ZERO,
            gravity: Duration::ZERO,
            spill: Duration::ZERO,
            grain: Duration::ZERO,
            humidity_diffuse: Duration::ZERO,
            finish: Duration::ZERO,
        }
    }

    fn total(&self) -> Duration {
        self.rain
            + self.evap
            + self.condensation
            + self.karst
            + self.gravity
            + self.spill
            + self.grain
            + self.humidity_diffuse
            + self.finish
    }
}

fn demo_params() -> WorldgenParams {
    WorldgenParams::default()
}

/// Roughly 2× the demo footprint in each axis (16×4 chunks vs 8×2).
fn stress_params() -> WorldgenParams {
    WorldgenParams {
        width_cols: (CHUNK_CELLS_W as i32) * 16,
        sky_ceiling_y: (CHUNK_CELLS_H as i32) * 4,
        ..WorldgenParams::default()
    }
}

fn stamp_scene(params: WorldgenParams) -> (World, Humidity, WorldgenParams) {
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let humidity = Humidity::new(HUMIDITY_TILE_COLS);
    (world, humidity, params)
}

fn configs(params: &WorldgenParams) -> (RainConfig, EvapConfig, CondensationConfig, KarstConfig) {
    let rain = RainConfig {
        top_y: params.sky_ceiling_y - 2,
        x_range: (0, params.width_cols - 1),
        prob_per_col_per_tick: 0.02,
        droplet_sat: 64,
        seed_salt: 0xC10D_5EED,
    };
    let evap = EvapConfig::default();
    let cond = CondensationConfig {
        top_y: params.sky_ceiling_y - 3,
        ..CondensationConfig::default()
    };
    let karst = KarstConfig::default();
    (rain, evap, cond, karst)
}

/// Mirror `rules::tick`'s post-pass bookkeeping without re-running
/// the three physics passes.
fn finish_tick(world: &mut World) {
    world.tick = world.tick.wrapping_add(1);
    for chunk in world.chunks.values_mut() {
        chunk.tick = chunk.tick.wrapping_add(1);
        chunk.clear_dirty();
    }
}

fn cell_count(params: &WorldgenParams) -> i64 {
    let h = (params.sky_ceiling_y - params.bedrock_floor_y) as i64;
    params.width_cols as i64 * h
}

fn ms_per(d: Duration, n: u64) -> f32 {
    d.as_secs_f32() * 1000.0 / n as f32
}

fn profile_label(label: &str, params: &WorldgenParams, chunks: usize) {
    eprintln!(
        "=== {label} ===  seed={:#x}  {}×{} cells (~{})  chunks={}  warm={} measure={}",
        params.seed,
        params.width_cols,
        params.sky_ceiling_y - params.bedrock_floor_y,
        cell_count(params),
        chunks,
        WARMUP_TICKS,
        MEASURE_TICKS
    );
}

fn one_stack_tick(
    world: &mut World,
    humidity: &mut Humidity,
    rain: &RainConfig,
    evap: &EvapConfig,
    cond: &CondensationConfig,
    karst: &KarstConfig,
    accum: Option<&mut PassAccum>,
) {
    match accum {
        None => {
            apply_rain(world, rain);
            apply_evaporation_into_humidity(world, humidity, evap);
            apply_condensation_rain(world, humidity, cond);
            apply_karst_dissolution(world, karst);
            apply_gravity_fall(world);
            apply_lateral_spill(world);
            apply_grain_fall(world);
            finish_tick(world);
            humidity.diffuse(HUMIDITY_DIFFUSION_ALPHA);
        }
        Some(a) => {
            let t0 = Instant::now();
            apply_rain(world, rain);
            a.rain += t0.elapsed();

            let t0 = Instant::now();
            apply_evaporation_into_humidity(world, humidity, evap);
            a.evap += t0.elapsed();

            let t0 = Instant::now();
            apply_condensation_rain(world, humidity, cond);
            a.condensation += t0.elapsed();

            let t0 = Instant::now();
            apply_karst_dissolution(world, karst);
            a.karst += t0.elapsed();

            let t0 = Instant::now();
            apply_gravity_fall(world);
            a.gravity += t0.elapsed();

            let t0 = Instant::now();
            apply_lateral_spill(world);
            a.spill += t0.elapsed();

            let t0 = Instant::now();
            apply_grain_fall(world);
            a.grain += t0.elapsed();

            let t0 = Instant::now();
            finish_tick(world);
            a.finish += t0.elapsed();

            let t0 = Instant::now();
            humidity.diffuse(HUMIDITY_DIFFUSION_ALPHA);
            a.humidity_diffuse += t0.elapsed();
        }
    }
}

fn run_profile(label: &str, params: WorldgenParams) {
    let (mut world, mut humidity, params) = stamp_scene(params);
    let (rain, evap, cond, karst) = configs(&params);
    let chunks = world.chunks.len();
    profile_label(label, &params, chunks);

    for _ in 0..WARMUP_TICKS {
        one_stack_tick(
            &mut world,
            &mut humidity,
            &rain,
            &evap,
            &cond,
            &karst,
            None,
        );
    }

    let mut accum = PassAccum::zero();
    let wall = Instant::now();
    for _ in 0..MEASURE_TICKS {
        one_stack_tick(
            &mut world,
            &mut humidity,
            &rain,
            &evap,
            &cond,
            &karst,
            Some(&mut accum),
        );
    }
    let wall = wall.elapsed();

    let n = MEASURE_TICKS;
    eprintln!("  wall                 {:>8.3} ms/tick  (total {:?})", ms_per(wall, n), wall);
    eprintln!("  sum(passes)          {:>8.3} ms/tick", ms_per(accum.total(), n));
    eprintln!("  ------------------------------------------------------------");
    eprintln!("  rain                 {:>8.3} ms/tick", ms_per(accum.rain, n));
    eprintln!("  evap→humidity        {:>8.3} ms/tick", ms_per(accum.evap, n));
    eprintln!("  condensation         {:>8.3} ms/tick", ms_per(accum.condensation, n));
    eprintln!("  karst                {:>8.3} ms/tick", ms_per(accum.karst, n));
    eprintln!("  gravity_fall         {:>8.3} ms/tick", ms_per(accum.gravity, n));
    eprintln!("  lateral_spill        {:>8.3} ms/tick", ms_per(accum.spill, n));
    eprintln!("  grain_fall           {:>8.3} ms/tick", ms_per(accum.grain, n));
    eprintln!("  humidity.diffuse     {:>8.3} ms/tick", ms_per(accum.humidity_diffuse, n));
    eprintln!("  finish (dirty clear) {:>8.3} ms/tick", ms_per(accum.finish, n));
    eprintln!(
        "  humidity tiles={}  humidity_mass={:.1}",
        humidity.cells.len(),
        humidity.total_mass()
    );
    eprintln!();
}

#[test]
#[ignore]
fn perf_profile_demo_and_stress() {
    run_profile("demo (WorldgenParams::default)", demo_params());
    run_profile("stress (16×4 chunks)", stress_params());
}
