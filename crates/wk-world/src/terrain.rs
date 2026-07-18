//! Procedural terrain generation (seeded D0).

use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M, CHUNK_W};

use crate::chunk::Chunk;
use crate::climate::biome_for;
use crate::column::{Column, Ecology};

/// Deepest bedrock reference elevation (metres). Ocean floor sits above this.
/// Deep enough for abyssal plains hundreds of metres below sea level while
/// still leaving headroom under kilometre-scale peaks.
pub const BEDROCK_FLOOR_M: f32 = -900.0;

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

/// Seed stage-8 ecology from biome at the column's climate elevation.
/// `sea_level` defaults to 0 when the caller doesn't care (flat test beds).
pub fn seed_column_ecology(col: &mut Column, sea_level: f32) {
    let biome = biome_for(col.climate_elevation(), sea_level);
    let rel = col.climate_elevation() - sea_level;
    col.ecology = Ecology::seed_from_biome(biome, rel);
}

/// Bulk-sediment budget for one column, split across the five stratigraphic
/// materials the terrain generator can lay down. Values are in kg; the
/// fill routine converts them to layer thicknesses through each
/// material's density. See `sediment_composition` for the elevation-
/// driven mix.
#[derive(Clone, Copy)]
struct SedimentMix {
    sand: f32,
    gravel: f32,
    looserock: f32,
    stone: f32,
    clay: f32,
}

impl SedimentMix {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        Self {
            sand: lerp(a.sand, b.sand, t),
            gravel: lerp(a.gravel, b.gravel, t),
            looserock: lerp(a.looserock, b.looserock, t),
            stone: lerp(a.stone, b.stone, t),
            clay: lerp(a.clay, b.clay, t),
        }
    }
}

/// Sediment composition as a *continuous* function of elevation relative to
/// sea level. Every regime (abyss/shelf/coast/plains/mountain) blends
/// smoothly into its neighbour via smoothstep, so noisy elevation near
/// a boundary changes composition gradually — never flips a whole
/// column's dominant material from one single-column step to the next.
///
/// Regimes:
/// - abyss: mostly clay (fine sediment settles far offshore) + some sand
/// - shelf: sand + stone with a light gravel mix
/// - coast (low emergent): sand dominant, gravel present (beach cobbles)
/// - plains: sand + stone with a bit of clay
/// - mountain (>30m above sea): stone + LooseRock talus + coarse gravel
fn sediment_composition(surface_y: f32, sea_level: f32) -> SedimentMix {
    let depth = sea_level - surface_y; // positive = underwater
    let rel = surface_y - sea_level;

    let abyss = SedimentMix {
        sand: 1500.0,
        gravel: 0.0,
        looserock: 0.0,
        stone: 0.0,
        clay: 700.0,
    };
    let shelf = SedimentMix {
        sand: 3200.0,
        gravel: 400.0,
        looserock: 0.0,
        stone: 2500.0,
        clay: 100.0,
    };
    let coast = SedimentMix {
        sand: 4200.0,
        gravel: 900.0,
        looserock: 0.0,
        stone: 6000.0,
        clay: 200.0,
    };
    let plains = SedimentMix {
        sand: 3800.0,
        gravel: 500.0,
        looserock: 0.0,
        stone: 9000.0,
        clay: 400.0,
    };
    let mountain = SedimentMix {
        sand: 1200.0,
        gravel: 1200.0,
        looserock: 5000.0,
        stone: 15000.0,
        clay: 0.0,
    };

    // abyss ← shelf as depth shrinks
    let t_abyss = smoothstep(14.0, 22.0, depth);
    let underwater = SedimentMix::lerp(shelf, abyss, t_abyss);

    // underwater ← coast as we cross the shoreline
    let t_coast = smoothstep(-2.0, 2.0, rel);
    let low = SedimentMix::lerp(underwater, coast, t_coast);

    // coast ← plains as elevation climbs a bit
    let t_plains = smoothstep(6.0, 18.0, rel);
    let base = SedimentMix::lerp(low, plains, t_plains);

    // plains ← mountain at higher elevations
    let t_mountain = smoothstep(30.0, 55.0, rel);
    SedimentMix::lerp(base, mountain, t_mountain)
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

    let mix = sediment_composition(surface_y, sea_level);
    let sand_mass = mix.sand as i64;
    let gravel_mass = mix.gravel as i64;
    let looserock_mass = mix.looserock as i64;
    let stone_mass = mix.stone as i64;
    let clay_mass = mix.clay as i64;

    let sediment_h = mass_to_height_m(MaterialId::Sand, sand_mass)
        + mass_to_height_m(MaterialId::Gravel, gravel_mass)
        + mass_to_height_m(MaterialId::LooseRock, looserock_mass)
        + mass_to_height_m(MaterialId::Stone, stone_mass)
        + mass_to_height_m(MaterialId::Clay, clay_mass);
    let bedrock_h = (surface_y - bedrock_y - sediment_h).max(2.0);

    // Deposit bottom-up. Density settling isn't strictly needed here
    // since we already lay materials in a roughly correct order, but
    // the clamp/settle at the end of the next simulation tick will
    // sort out any minor inversions anyway.
    col.deposit_to_top(MaterialId::Bedrock, mass_for_height(MaterialId::Bedrock, bedrock_h), tick);
    if stone_mass > 0 {
        col.deposit_to_top(MaterialId::Stone, stone_mass, tick);
    }
    if looserock_mass > 0 {
        col.deposit_to_top(MaterialId::LooseRock, looserock_mass, tick);
    }
    if clay_mass > 0 {
        col.deposit_to_top(MaterialId::Clay, clay_mass, tick);
    }
    if gravel_mass > 0 {
        col.deposit_to_top(MaterialId::Gravel, gravel_mass, tick);
    }
    if sand_mass > 0 {
        col.deposit_to_top(MaterialId::Sand, sand_mass, tick);
    }

    // Vegetation is not an Organic stratigraphic layer (density settling
    // would float it). Stage 8 seeds a per-column Ecology bucket instead.
    let _ = seed;
    let _ = world_x;

    col.recompute_surface_y(bedrock_y);
    col.activity = crate::column::Activity::HydrologyActive;

    // Fill submerged columns with a Water layer up to sea level so the
    // ocean is just the top of the layer stack, not a separate ad-hoc
    // rendering pass or column-state flag. `deposit_to_top` handles the
    // surface_y bookkeeping for us.
    fill_up_to_sea_level(col, sea_level, tick);
    seed_column_water_table(col, sea_level);
    seed_column_ecology(col, sea_level);
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

/// Seed pore moisture so the regional water table sits near `sea_level`
/// from the first tick — ocean beds fully saturated, coastal land wet
/// up to the table, high ground holding a modest base saturation.
///
/// Without this, continental gen left `moisture = 0` everywhere and the
/// ocean spent the early sim soaking into dry sand (and looking empty
/// underground on the water-table overlay).
pub fn seed_column_water_table(col: &mut Column, sea_level: f32) {
    col.moisture = col.target_moisture_for_table(sea_level);
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

/// Conventional margin profile: deep plain → slope → shelf → coast → plains → mountains.
/// Profile spans ~500 m of world-x; repeat subtle variation for maps beyond that.
pub fn continental_surface_y(seed: u64, world_x: i32, sea_level: f32) -> f32 {
    let xm = world_x as f32 * SAMPLE_WIDTH_M;
    // Per-column noise dampened relative to earlier revisions — real
    // adjacent-column height variance is a lot smaller than ±2 m, and
    // strong independent-per-column jitter is what created isolated
    // 1–2 m sand spikes that then survived generation because slumping
    // hadn't yet caught up.
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
        abyss + n(30) * 0.6
    } else if macro_x < 180.0 {
        let t = smoothstep(100.0, 180.0, macro_x);
        lerp(abyss, slope_end, t) + n(31) * 0.4
    } else if macro_x < 260.0 {
        let t = smoothstep(180.0, 260.0, macro_x);
        lerp(slope_end, shelf_end, t) + n(32) * 0.25
    } else if macro_x < 340.0 {
        let t = smoothstep(260.0, 340.0, macro_x);
        lerp(shelf_end, coast_end, t) + n(33) * 0.5 + land_ripple(seed, xm) * 0.4
    } else if macro_x < 420.0 {
        let t = smoothstep(340.0, 420.0, macro_x);
        lerp(coast_end, plains_base, t) + n(35) * 0.6 + land_ripple(seed, xm)
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

        // Extended range: 8 named peaks going progressively higher, with
        // one dominant peak in the middle so there's a clear "highest"
        // point to hike toward. Amplitudes are bigger than before so sand
        // has genuine slopes to erode across.
        let ridges = peak(40.0, 20.0, 55.0)
            + peak(105.0, 22.0, 78.0)
            + peak(175.0, 22.0, 65.0)
            + peak(250.0, 26.0, 110.0)     // main summit
            + peak(330.0, 22.0, 82.0)
            + peak(405.0, 20.0, 60.0)
            + peak(475.0, 24.0, 88.0)
            + peak(555.0, 22.0, 70.0);

        // Basins sit between ridges and are wide/flat enough at the bottom
        // to hold a real lake, not just a narrow notch. Slightly deeper
        // valleys now so lakes have room to fill.
        let valleys = peak(70.0, 14.0, 40.0)
            + peak(140.0, 14.0, 42.0)
            + peak(210.0, 14.0, 38.0)
            + peak(285.0, 14.0, 50.0)
            + peak(365.0, 14.0, 44.0)
            + peak(440.0, 14.0, 38.0)
            + peak(515.0, 14.0, 46.0);

        plains_base
            + ramp_in * (ridges - valleys)
            + n(36) * 0.7
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

/// Ring facies chunk: periodic belts + stratigraphic recipes (`docs/STRATA.md`).
pub fn generate_chunk_ring_facies(
    coord: i32,
    seed: u64,
    bedrock_y: f32,
    sea_level: f32,
    topology: crate::worldgen::WorldTopology,
) -> Chunk {
    let coord = crate::worldgen::wrap_chunk_coord(topology, coord);
    let mut chunk = Chunk::new(coord, bedrock_y);
    let base = chunk.world_x_base();
    for i in 0..CHUNK_W {
        let wx = crate::worldgen::wrap_world_x(topology, base + i as i32);
        let surface =
            crate::worldgen::facies_surface_y(seed, topology, wx, sea_level);
        fill_facies_column(
            &mut chunk.columns[i],
            surface,
            bedrock_y,
            sea_level,
            seed,
            topology,
            wx,
            0,
        );
    }
    chunk
}

/// Dispatch chunk generation from [`crate::worldgen::WorldGenParams`].
pub fn generate_chunk(
    coord: i32,
    seed: u64,
    bedrock_y: f32,
    sea_level: f32,
    params: crate::worldgen::WorldGenParams,
) -> Chunk {
    match params.profile {
        crate::worldgen::WorldGenProfile::LegacyContinental => {
            generate_chunk_continental(coord, seed, bedrock_y, sea_level)
        }
        crate::worldgen::WorldGenProfile::RingFacies => {
            generate_chunk_ring_facies(coord, seed, bedrock_y, sea_level, params.topology)
        }
    }
}

/// Facies-aware stack: named packages with limestone shelves, clay basins,
/// and talus on high ground — still ≤7 solid layers before sea fill.
pub fn fill_facies_column(
    col: &mut Column,
    surface_y: f32,
    bedrock_y: f32,
    sea_level: f32,
    seed: u64,
    topology: crate::worldgen::WorldTopology,
    world_x: i32,
    tick: u64,
) {
    use crate::worldgen::{facies_at, FaciesBelt};

    col.layer_count = 0;
    col.surface_y = bedrock_y;

    let belt = facies_at(seed, topology, world_x);
    let n = |salt: u64| hash_f32(seed, world_x as i64, salt);

    // Package thicknesses (metres of solid above basement fill).
    let (stone_m, mid_mat, mid_m, cover_mat, cover_m) = match belt {
        FaciesBelt::Abyss => (6.0, MaterialId::Clay, 3.0 + n(11) * 1.0, MaterialId::Sand, 0.8),
        FaciesBelt::Slope => (
            12.0,
            MaterialId::LooseRock,
            5.0 + n(12) * 1.5,
            MaterialId::Gravel,
            2.0,
        ),
        FaciesBelt::Shelf => (
            8.0,
            MaterialId::Limestone,
            8.0 + n(13) * 1.5,
            MaterialId::Sand,
            2.0,
        ),
        FaciesBelt::Marsh => (4.0, MaterialId::Clay, 3.5 + n(14) * 0.8, MaterialId::Sand, 1.0),
        FaciesBelt::Coast => (8.0, MaterialId::Sand, 0.0, MaterialId::Sand, 5.0 + n(15) * 1.2),
        FaciesBelt::Plains => (
            14.0,
            MaterialId::Clay,
            2.5 + n(16) * 0.8,
            MaterialId::Sand,
            3.5,
        ),
        FaciesBelt::Foothills => (
            28.0,
            MaterialId::Gravel,
            6.0 + n(17) * 2.0,
            MaterialId::LooseRock,
            4.0,
        ),
        FaciesBelt::HighRange => (
            // Tall peaks are mostly Bedrock fill; keep a thick stone sleeve
            // so x-ray still reads as mountain rock, not a paper-thin crust.
            80.0,
            MaterialId::Stone,
            0.0,
            MaterialId::LooseRock,
            3.0 + n(18) * 2.0,
        ),
        FaciesBelt::RainShadow => (
            18.0,
            MaterialId::Sand,
            4.0 + n(19) * 1.2,
            MaterialId::LooseRock,
            2.5,
        ),
        FaciesBelt::InteriorBasin => (
            8.0,
            MaterialId::Clay,
            5.0 + n(20) * 1.2,
            MaterialId::Sand,
            2.0,
        ),
    };

    let mut sediment_h = stone_m + mid_m + cover_m;
    // Pinch mid package when very thin — frees a layer slot (unconformity cue).
    let use_mid = mid_m > 0.35 && mid_mat != cover_mat;
    if !use_mid {
        sediment_h = stone_m + cover_m;
    }
    let bedrock_h = (surface_y - bedrock_y - sediment_h).max(2.0);

    col.deposit_to_top(
        MaterialId::Bedrock,
        mass_for_height(MaterialId::Bedrock, bedrock_h),
        tick,
    );
    if stone_m > 0.05 {
        col.deposit_to_top(
            MaterialId::Stone,
            mass_for_height(MaterialId::Stone, stone_m),
            tick,
        );
    }
    if use_mid {
        col.deposit_to_top(mid_mat, mass_for_height(mid_mat, mid_m), tick);
    }
    if cover_m > 0.05 {
        col.deposit_to_top(cover_mat, mass_for_height(cover_mat, cover_m), tick);
    }

    col.recompute_surface_y(bedrock_y);
    col.activity = crate::column::Activity::HydrologyActive;
    fill_up_to_sea_level(col, sea_level, tick);
    seed_column_water_table(col, sea_level);
    // Wet belts get a slightly richer ecology seed via elevation proxy.
    seed_column_ecology(col, sea_level);
    let _ = topology;
}

pub fn generate_chunk_hill(coord: i32, seed: u64, center: i32, bedrock_y: f32) -> Chunk {
    let mut chunk = Chunk::new(coord, bedrock_y);
    let base = chunk.world_x_base();
    for i in 0..CHUNK_W {
        let wx = base + i as i32;
        let surface = gaussian_hill_surface(seed, wx, center, 30.0, bedrock_y + 5.0, 25.0);
        let col = &mut chunk.columns[i];
        fill_column_strata(col, surface, bedrock_y, 5000, 8000, 0);
        // Scenario worlds usually use sea_level≈0; biome seed follows that.
        seed_column_ecology(col, 0.0);
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
        seed_column_ecology(col, 0.0);
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
        seed_column_ecology(col, 0.0);
    }
    chunk
}

pub fn generate_flat_sand(coord: i32, bedrock_y: f32, surface_y: f32) -> Chunk {
    let mut chunk = Chunk::new(coord, bedrock_y);
    for col in &mut chunk.columns {
        fill_column_strata(col, surface_y, bedrock_y, 8000, 5000, 0);
        seed_column_ecology(col, 0.0);
    }
    chunk
}

/// Flat limestone bed under a thin sand cap — stage 7 karst scenarios.
/// Stack (top→bottom after settle): Sand cap, Limestone body, Stone base.
pub fn generate_limestone_bed(
    coord: i32,
    bedrock_y: f32,
    stone_h: f32,
    limestone_h: f32,
    sand_h: f32,
) -> Chunk {
    let mut chunk = Chunk::new(coord, bedrock_y);
    for col in &mut chunk.columns {
        col.layer_count = 0;
        col.surface_y = bedrock_y;
        let stone_m = mass_for_height(MaterialId::Stone, stone_h);
        let lime_m = mass_for_height(MaterialId::Limestone, limestone_h);
        let sand_m = mass_for_height(MaterialId::Sand, sand_h);
        // Deposit bottom-first via repeated top deposits + settle.
        col.deposit_to_top(MaterialId::Stone, stone_m, 0);
        col.deposit_to_top(MaterialId::Limestone, lime_m, 0);
        col.deposit_to_top(MaterialId::Sand, sand_m, 0);
        col.settle_by_density(0);
        col.recompute_surface_y(bedrock_y);
    }
    chunk
}
