//! Procedural terrain generation (seeded D0).

use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M, CHUNK_W};

use crate::chunk::Chunk;
use crate::column::Column;

/// Deepest bedrock reference elevation (metres). Ocean floor sits above this.
pub const BEDROCK_FLOOR_M: f32 = -45.0;

pub fn hash_u64(seed: u64, x: i64, y: i64, salt: u64) -> u64 {
    let mut z = seed.wrapping_add(salt);
    z = z.wrapping_add((x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = z.wrapping_add((y as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
    z ^= z >> 30;
    z = z.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z ^= z >> 27;
    z = z.wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub fn hash_f32(seed: u64, x: i64, salt: u64) -> f32 {
    let h = hash_u64(seed, x, 0, salt);
    (h as f32) / (u64::MAX as f32)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge0 == edge1 {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Convert kg mass in a column to height in metres.
pub fn mass_to_height_m(material: MaterialId, mass: i64) -> f32 {
    let density = MaterialRegistry::props(material).density.max(1) as f32;
    (mass as f32 / density) / SAMPLE_WIDTH_M
}

pub fn mass_for_height(material: MaterialId, height_m: f32) -> i64 {
    let density = MaterialRegistry::props(material).density as i64;
    let volume = (height_m * SAMPLE_WIDTH_M * 1.0) as f64;
    (volume * density as f64) as i64
}

/// Gaussian hill centered at world x.
pub fn gaussian_hill_surface(
    seed: u64,
    world_x: i32,
    center: i32,
    width: f32,
    base_y: f32,
    peak: f32,
) -> f32 {
    let dx = (world_x - center) as f32 * SAMPLE_WIDTH_M;
    let w = width.max(1.0);
    let g = (-dx * dx / (2.0 * w * w)).exp();
    base_y + peak * g + (hash_f32(seed, world_x as i64, 1) - 0.5) * 0.5
}

/// Bowl-shaped basin.
pub fn basin_surface(
    seed: u64,
    world_x: i32,
    center: i32,
    width: f32,
    rim_y: f32,
    depth: f32,
) -> f32 {
    let dx = (world_x - center) as f32 * SAMPLE_WIDTH_M;
    let w = width.max(1.0);
    let norm = (dx / w).clamp(-1.5, 1.5);
    let bowl = norm * norm;
    rim_y - depth * bowl + (hash_f32(seed, world_x as i64, 2) - 0.5) * 0.2
}

pub fn tilted_surface(world_x: i32, start_y: f32, slope: f32) -> f32 {
    start_y - world_x as f32 * SAMPLE_WIDTH_M * slope
}

/// Build strata from bedrock up; `surface_y` is the target top elevation (not pre-set).
pub fn fill_column_strata(
    col: &mut Column,
    surface_y: f32,
    bedrock_y: f32,
    sand_mass: i64,
    stone_mass: i64,
    tick: u64,
) {
    col.layer_count = 0;
    col.surface_y = bedrock_y;

    let sand_h = mass_to_height_m(MaterialId::Sand, sand_mass);
    let stone_h = mass_to_height_m(MaterialId::Stone, stone_mass);
    let sediment_h = sand_h + stone_h;
    let bedrock_h = (surface_y - bedrock_y - sediment_h).max(2.0);

    col.deposit_to_top(MaterialId::Bedrock, mass_for_height(MaterialId::Bedrock, bedrock_h), tick);
    if stone_mass > 0 {
        col.deposit_to_top(MaterialId::Stone, stone_mass, tick);
    }
    if sand_mass > 0 {
        col.deposit_to_top(MaterialId::Sand, sand_mass, tick);
    }
    col.recompute_surface_y(bedrock_y);
    col.activity = crate::column::Activity::HydrologyActive;
}

fn lerp3(a: (f32, f32, f32), b: (f32, f32, f32), t: f32) -> (f32, f32, f32) {
    (lerp(a.0, b.0, t), lerp(a.1, b.1, t), lerp(a.2, b.2, t))
}

/// Sediment composition as a *continuous* function of elevation relative to
/// sea level. Every regime (abyss/shelf/land/mountain) blends smoothly into
/// its neighbour via smoothstep, so noisy elevation near a boundary changes
/// composition gradually — never flips a whole column's dominant material
/// from one single-column step to the next.
fn sediment_composition(surface_y: f32, sea_level: f32) -> (i64, i64, i64) {
    let depth = sea_level - surface_y; // positive = underwater

    let abyss = (1800.0_f32, 0.0, 600.0);
    let shelf = (3500.0_f32, 2500.0, 0.0);
    let land = (4500.0_f32, 9000.0, 0.0);
    let mountain = (2500.0_f32, 17_000.0, 0.0);

    let t_abyss = smoothstep(14.0, 22.0, depth);
    let underwater = lerp3(shelf, abyss, t_abyss);

    let t_land = smoothstep(-2.0, 2.0, -depth);
    let base = lerp3(underwater, land, t_land);

    let t_mountain = smoothstep(20.0, 34.0, surface_y - sea_level);
    let final_mix = lerp3(base, mountain, t_mountain);

    (final_mix.0 as i64, final_mix.1 as i64, final_mix.2 as i64)
}

/// Bathymetry-aware fill: abyssal plain, shelf, or land cover.
pub fn fill_bathymetry_column(
    col: &mut Column,
    surface_y: f32,
    bedrock_y: f32,
    sea_level: f32,
    seed: u64,
    world_x: i32,
    tick: u64,
) {
    col.layer_count = 0;
    col.surface_y = bedrock_y;

    let (sand_mass, stone_mass, clay_mass) = sediment_composition(surface_y, sea_level);

    let sand_h = mass_to_height_m(MaterialId::Sand, sand_mass);
    let stone_h = mass_to_height_m(MaterialId::Stone, stone_mass);
    let clay_h = mass_to_height_m(MaterialId::Clay, clay_mass);
    let sediment_h = sand_h + stone_h + clay_h;
    let bedrock_h = (surface_y - bedrock_y - sediment_h).max(2.0);

    col.deposit_to_top(MaterialId::Bedrock, mass_for_height(MaterialId::Bedrock, bedrock_h), tick);
    if stone_mass > 0 {
        col.deposit_to_top(MaterialId::Stone, stone_mass, tick);
    }
    if clay_mass > 0 {
        col.deposit_to_top(MaterialId::Clay, clay_mass, tick);
    }
    if sand_mass > 0 {
        col.deposit_to_top(MaterialId::Sand, sand_mass, tick);
    }

    // Vegetation cover uses coherent low-frequency patchiness, scaled smoothly
    // in by how far above sea level we are, rather than an independent
    // per-column coin flip or a hard elevation cutoff.
    let veg_t = smoothstep(0.0, 6.0, surface_y - sea_level);
    if veg_t > 0.0 {
        let xm = world_x as f32 * SAMPLE_WIDTH_M;
        if cover_patchiness(seed, xm) > 0.05 {
            let mass = (400.0 * veg_t) as i64;
            col.deposit_to_top(MaterialId::Organic, mass, tick);
        }
    }

    col.recompute_surface_y(bedrock_y);
    col.activity = crate::column::Activity::HydrologyActive;

    // Fill submerged columns with a Water layer up to sea level so the
    // ocean is just the top of the layer stack, not a separate ad-hoc
    // rendering pass or column-state flag. `deposit_to_top` handles the
    // surface_y bookkeeping for us.
    fill_up_to_sea_level(col, sea_level, tick);
}

/// If this column's natural terrain surface sits below sea level, tops
/// it up with a `Water` layer of exactly the mass needed to reach sea
/// level. No-op otherwise. Ocean, lakebed, and puddle all end up as
/// the same "top layer is Water" state, which lets one rendering path
/// and one flow path handle everything.
pub fn fill_up_to_sea_level(col: &mut Column, sea_level: f32, tick: u64) {
    let deficit_m = sea_level - col.surface_y;
    if deficit_m <= 0.0 {
        return;
    }
    let density = wk_material::MaterialRegistry::props(MaterialId::Water)
        .density
        .max(1) as f32;
    let mass = (deficit_m * SAMPLE_WIDTH_M * density) as i64;
    if mass > 0 {
        col.deposit_to_top(MaterialId::Water, mass, tick);
    }
}

/// Low-frequency rolling ripple layered onto emergent land so there are
/// visible dips (crevices) for rainwater to pool in, not just a smooth ramp.
fn land_ripple(seed: u64, xm: f32) -> f32 {
    let phase = hash_f32(seed, 777, 41) * std::f32::consts::TAU;
    let a = (xm * 0.045 + phase).sin() * 2.6;
    let b = (xm * 0.11 + phase * 1.3).sin() * 1.2;
    let c = (xm * 0.23 + phase * 0.7).sin() * 0.5;
    a + b + c
}

/// Coherent (multi-column) patchiness for surface cover, in roughly [-1, 1].
/// Deliberately low frequency so vegetation/rock patches span tens of
/// columns instead of flickering column-to-column like independent noise
/// would (that was the cause of the single-column checkerboard artifact).
fn cover_patchiness(seed: u64, xm: f32) -> f32 {
    let phase = hash_f32(seed, 888, 51) * std::f32::consts::TAU;
    let a = (xm * 0.018 + phase).sin();
    let b = (xm * 0.037 + phase * 1.7).sin() * 0.5;
    (a + b) / 1.5
}

/// Conventional margin profile: deep plain → slope → shelf → coast → plains → mountains.
/// Profile spans ~500 m of world-x; repeat subtle variation for maps beyond that.
pub fn continental_surface_y(seed: u64, world_x: i32, sea_level: f32) -> f32 {
    let xm = world_x as f32 * SAMPLE_WIDTH_M;
    let n = |salt: u64| (hash_f32(seed, world_x as i64, salt) - 0.5) * 2.0;

    // Macro zones (metres along the margin)
    let abyss = sea_level - 42.0;
    let slope_end = sea_level - 14.0;
    let shelf_end = sea_level - 2.0;
    let coast_end = sea_level + 18.0;
    let plains_base = sea_level + 20.0;

    // Not wrapped: the loaded map is finite, and modulo-wrapping this profile
    // would otherwise jump straight from mountain peaks back to open ocean
    // the instant it looped, which is just as bad a cliff as the one below.
    let macro_x = xm;

    if macro_x < 100.0 {
        abyss + n(30) * 1.5
    } else if macro_x < 180.0 {
        let t = smoothstep(100.0, 180.0, macro_x);
        lerp(abyss, slope_end, t) + n(31)
    } else if macro_x < 260.0 {
        let t = smoothstep(180.0, 260.0, macro_x);
        lerp(slope_end, shelf_end, t) + n(32) * 0.5
    } else if macro_x < 340.0 {
        let t = smoothstep(260.0, 340.0, macro_x);
        lerp(shelf_end, coast_end, t) + n(33) * 1.2 + land_ripple(seed, xm) * 0.4
    } else if macro_x < 420.0 {
        let t = smoothstep(340.0, 420.0, macro_x);
        lerp(coast_end, plains_base, t) + n(35) * 1.5 + land_ripple(seed, xm)
    } else {
        // Mountain cordillera: distinct peaks with genuine enclosed valleys
        // (basins) carved between them, not just gentler saddles. Each bump
        // is centred well inland and ramped in from zero at the boundary,
        // so it starts flush with the plains elevation and rises gradually —
        // no instant step at the seam.
        let inland = macro_x - 420.0;
        let ramp_in = smoothstep(0.0, 30.0, inland);

        let peak = |center: f32, width: f32, amp: f32| {
            amp * (-(inland - center) * (inland - center) / (2.0 * width * width)).exp()
        };

        let ridges = peak(40.0, 18.0, 42.0)
            + peak(95.0, 20.0, 58.0)
            + peak(155.0, 18.0, 48.0)
            + peak(215.0, 20.0, 52.0);

        // Basins sit between ridges and are wide/flat enough at the bottom
        // to hold a real lake, not just a narrow notch.
        let valleys = peak(67.0, 13.0, 34.0)
            + peak(125.0, 13.0, 36.0)
            + peak(185.0, 13.0, 32.0);

        plains_base
            + ramp_in * (ridges - valleys)
            + n(36) * 1.5
            + land_ripple(seed, xm) * 1.2
    }
}

/// Fill a large map: `chunk_min..chunk_max` half-open.
pub fn generate_map_continental(
    chunk_min: i32,
    chunk_max: i32,
    seed: u64,
    bedrock_y: f32,
    sea_level: f32,
) -> Vec<Chunk> {
    (chunk_min..chunk_max)
        .map(|c| generate_chunk_continental(c, seed, bedrock_y, sea_level))
        .collect()
}

pub fn generate_chunk_continental(
    coord: i32,
    seed: u64,
    bedrock_y: f32,
    sea_level: f32,
) -> Chunk {
    let mut chunk = Chunk::new(coord, bedrock_y);
    let base = chunk.world_x_base();
    for i in 0..CHUNK_W {
        let wx = base + i as i32;
        let surface = continental_surface_y(seed, wx, sea_level);
        fill_bathymetry_column(
            &mut chunk.columns[i],
            surface,
            bedrock_y,
            sea_level,
            seed,
            wx,
            0,
        );
    }
    chunk
}

pub fn generate_chunk_hill(coord: i32, seed: u64, center: i32, bedrock_y: f32) -> Chunk {
    let mut chunk = Chunk::new(coord, bedrock_y);
    let base = chunk.world_x_base();
    for i in 0..CHUNK_W {
        let wx = base + i as i32;
        let surface = gaussian_hill_surface(seed, wx, center, 30.0, bedrock_y + 5.0, 25.0);
        let col = &mut chunk.columns[i];
        fill_column_strata(col, surface, bedrock_y, 5000, 8000, 0);
    }
    chunk
}

pub fn generate_chunk_basin(coord: i32, seed: u64, center: i32, bedrock_y: f32) -> Chunk {
    let mut chunk = Chunk::new(coord, bedrock_y);
    let base = chunk.world_x_base();
    for i in 0..CHUNK_W {
        let wx = base + i as i32;
        let surface = basin_surface(seed, wx, center, 40.0, bedrock_y + 20.0, 15.0);
        let col = &mut chunk.columns[i];
        fill_column_strata(col, surface, bedrock_y, 1000, 5000, 0);
        col.deposit_to_top(MaterialId::Clay, 3000, 0);
    }
    chunk
}

pub fn generate_chunk_stratified_tilt(
    coord: i32,
    seed: u64,
    bedrock_y: f32,
    slope: f32,
) -> Chunk {
    let mut chunk = Chunk::new(coord, bedrock_y);
    let base = chunk.world_x_base();
    for i in 0..CHUNK_W {
        let wx = base + i as i32;
        let surface = tilted_surface(wx, bedrock_y + 12.0, slope)
            + (hash_f32(seed, wx as i64, 3) - 0.5) * 1.0;
        let col = &mut chunk.columns[i];
        fill_column_strata(col, surface, bedrock_y, 6000, 10000, 0);
    }
    chunk
}

pub fn generate_flat_sand(coord: i32, bedrock_y: f32, surface_y: f32) -> Chunk {
    let mut chunk = Chunk::new(coord, bedrock_y);
    for col in &mut chunk.columns {
        fill_column_strata(col, surface_y, bedrock_y, 8000, 5000, 0);
    }
    chunk
}
