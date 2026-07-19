//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Deterministic continental worldgen for the voxel sim.
//!
//! Given a [`WorldgenParams`] descriptor and a [`World`], stamp a
//! continental profile — abyss → slope → shelf → coast → plains →
//! mountains — into the cell grid. Uses only [`wk_material::MaterialId`]
//! for cell materials and a deterministic hash for per-column noise.
//!
//! The profile intentionally mirrors column-GVSE's
//! `continental_surface_y` at a high level so gameplay comes out
//! recognisably GVSE-shaped, but the *implementation* here is
//! independent — this file MUST NOT reach into `wk_world`.

use wk_material::MaterialId;

use crate::cell::Cell;
use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// Params driving one worldgen pass.
///
/// Layout (world-x is horizontal, world-y is vertical; +y is up):
///
/// - x range: `0 .. width_cols`.
/// - y range: `bedrock_floor_y .. sky_ceiling_y`.
/// - `sea_level_y`: cells at or below this elevation that aren't
///   solid material get filled with water (Air cells with full sat).
#[derive(Debug, Clone, Copy)]
pub struct WorldgenParams {
    pub seed: u64,
    pub width_cols: i32,
    pub bedrock_floor_y: i32,
    pub sea_level_y: i32,
    pub sky_ceiling_y: i32,
    /// How many bedrock rows to stamp at the very bottom.
    pub bedrock_thickness: i32,
    /// Thickness of the stone body above bedrock.
    pub stone_thickness: i32,
    /// Thickness of the sand cap on top of stone / limestone.
    pub sand_cap_thickness: i32,
    /// When true, the "stone" body of columns in the shelf+coast
    /// zone is stamped as [`MaterialId::Limestone`] instead of
    /// [`MaterialId::Stone`]. Gives the karst dissolution rule
    /// something to eat through.
    pub limestone_in_shelf_and_coast: bool,
}

impl Default for WorldgenParams {
    fn default() -> Self {
        Self {
            seed: 0xDEAD_BEEF,
            width_cols: (CHUNK_CELLS_W as i32) * 8, // ~8 chunks wide
            bedrock_floor_y: 0,
            sea_level_y: 40,
            sky_ceiling_y: (CHUNK_CELLS_H as i32) * 2,
            bedrock_thickness: 3,
            stone_thickness: 8,
            sand_cap_thickness: 2,
            limestone_in_shelf_and_coast: true,
        }
    }
}

/// True if `world_x` falls in the karst-friendly stretch of the
/// continental profile — same shelf → coast x-range used by
/// [`continental_surface_y`].
pub fn is_karst_zone_x(world_x: i32) -> bool {
    let x = world_x as f32;
    // Matches shelf_end → coast_end in `continental_surface_y`.
    (180.0..=340.0).contains(&x)
}

/// Cheap deterministic 32-bit hash → f32 in `[0, 1)`.
fn hash_f32(seed: u64, x: i64, salt: u64) -> f32 {
    // Wyhash-lite mixing. Simple, deterministic, no external deps.
    let mut h = seed
        .wrapping_add(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(x as u64);
    h ^= h.wrapping_shr(30);
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h.wrapping_shr(27);
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h.wrapping_shr(31);
    (h as u32 as f32) / (u32::MAX as f32 + 1.0)
}

/// GVSE-style macro continental profile.
///
/// Maps world-x to solid-surface elevation. Six named zones stitched
/// with `smoothstep` blends — matches the column-GVSE zone layout
/// (abyss → slope → shelf → coast → plains → mountains) so play tests
/// port over intuitively, but the implementation lives here in the
/// voxel crate.
pub fn continental_surface_y(seed: u64, world_x: i32, sea: i32) -> i32 {
    let x = world_x as f32;
    let n = |salt: u64| (hash_f32(seed, world_x as i64, salt) - 0.5) * 2.0;
    let sea_f = sea as f32;

    // Zone anchors along world-x. Scaled versions of column-GVSE's
    // continental band widths.
    let abyss_end = 100.0f32;
    let slope_end = 180.0f32;
    let shelf_end = 260.0f32;
    let coast_end = 340.0f32;
    let plains_end = 420.0f32;

    let smoothstep = |a: f32, b: f32, t: f32| {
        let u = ((t - a) / (b - a)).clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    };
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;

    let abyss = sea_f - 42.0;
    let slope_out = sea_f - 14.0;
    let shelf_out = sea_f - 2.0;
    let coast_out = sea_f + 18.0;
    let plains_base = sea_f + 20.0;

    let elev = if x < abyss_end {
        abyss + n(30) * 0.6
    } else if x < slope_end {
        lerp(abyss, slope_out, smoothstep(abyss_end, slope_end, x)) + n(31) * 0.4
    } else if x < shelf_end {
        lerp(slope_out, shelf_out, smoothstep(slope_end, shelf_end, x)) + n(32) * 0.25
    } else if x < coast_end {
        lerp(shelf_out, coast_out, smoothstep(shelf_end, coast_end, x)) + n(33) * 0.5
    } else if x < plains_end {
        lerp(coast_out, plains_base, smoothstep(coast_end, plains_end, x)) + n(35) * 0.6
    } else {
        // Mountains: a couple of Gaussian ridges.
        let inland = x - plains_end;
        let ramp_in = smoothstep(0.0, 30.0, inland);
        let peak = |center: f32, width: f32, amp: f32| {
            amp * (-(inland - center) * (inland - center) / (2.0 * width * width)).exp()
        };
        let ridges = peak(40.0, 20.0, 30.0) + peak(120.0, 22.0, 45.0) + peak(200.0, 22.0, 38.0);
        plains_base + ramp_in * ridges + n(36) * 0.7
    };
    elev.round() as i32
}

/// Stamp the full continental profile into `world`.
///
/// For each column `x` in `0..width_cols`:
///
/// 1. Compute the solid-surface elevation `surface_y = continental_surface_y(...)`.
/// 2. Rows `[bedrock_floor_y, bedrock_floor_y + bedrock_thickness)`
///    → `Bedrock`.
/// 3. Above bedrock up to `stone_top = surface_y - sand_cap_thickness`
///    → `Stone`.
/// 4. From `stone_top + 1` up to `surface_y` → `Sand` (the near-surface
///    cap). Sand porosity carries pore water once fluid rules run.
/// 5. Above the solid stack, if elevation is below `sea_level_y`
///    → water-filled `Air` (sat = FULL).
/// 6. Everything else → empty `Air`.
///
/// Chunks materialise on demand as cells are written. Deterministic
/// for a given `(seed, world_x)`.
pub fn stamp_world(world: &mut World, p: &WorldgenParams) {
    let bedrock_top = p.bedrock_floor_y + p.bedrock_thickness;
    for x in 0..p.width_cols {
        let surface_y = continental_surface_y(p.seed, x, p.sea_level_y);
        let stone_top = surface_y - p.sand_cap_thickness;
        // Zone-based stone material substitution. Shelf + coast get
        // Limestone — the karst dissolution rule eats through it.
        let stone_material = if p.limestone_in_shelf_and_coast && is_karst_zone_x(x) {
            MaterialId::Limestone
        } else {
            MaterialId::Stone
        };

        for y in p.bedrock_floor_y..p.sky_ceiling_y {
            let cell = if y < bedrock_top {
                Cell::solid(MaterialId::Bedrock)
            } else if y <= stone_top {
                Cell::solid(stone_material)
            } else if y <= surface_y {
                // Sand cap. Dry at generation — infiltration wets it later.
                Cell::solid(MaterialId::Sand)
            } else if y <= p.sea_level_y {
                // Submerged air = free water.
                Cell::water()
            } else {
                Cell::air()
            };
            world.set_cell(x, y, cell);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};

    fn small_params() -> WorldgenParams {
        // Wide enough to include all six zones (abyss → mountains,
        // ~620 world-x) and tall enough that mountain peaks fit under
        // sky_ceiling. Sea level lifted so abyss surface stays above
        // bedrock floor.
        WorldgenParams {
            seed: 42,
            width_cols: (CHUNK_CELLS_W as i32) * 10, // 640 cols
            bedrock_floor_y: 0,
            sea_level_y: 60,
            sky_ceiling_y: (CHUNK_CELLS_H as i32) * 3, // 192 rows
            bedrock_thickness: 3,
            stone_thickness: 8,
            sand_cap_thickness: 2,
            limestone_in_shelf_and_coast: true,
        }
    }

    #[test]
    fn stamps_bedrock_at_floor() {
        let mut w = World::new(1);
        let p = small_params();
        stamp_world(&mut w, &p);
        for x in 0..p.width_cols {
            assert_eq!(
                w.get_cell(x, 0).unwrap().material,
                MaterialId::Bedrock,
                "x={x} y=0 should be Bedrock"
            );
        }
    }

    #[test]
    fn surface_is_deterministic_for_seed() {
        let mut w1 = World::new(1);
        let mut w2 = World::new(1);
        let p = small_params();
        stamp_world(&mut w1, &p);
        stamp_world(&mut w2, &p);
        for x in 0..p.width_cols {
            for y in 0..p.sky_ceiling_y {
                assert_eq!(w1.get_cell(x, y), w2.get_cell(x, y), "diverged at {x},{y}");
            }
        }
    }

    #[test]
    fn different_seeds_produce_different_surfaces() {
        let mut w1 = World::new(1);
        let mut w2 = World::new(2);
        let p_a = WorldgenParams {
            seed: 1000,
            ..small_params()
        };
        let p_b = WorldgenParams {
            seed: 9999,
            ..small_params()
        };
        stamp_world(&mut w1, &p_a);
        stamp_world(&mut w2, &p_b);
        // At least *some* column should differ — the two profiles use
        // different mountain / plains noise seeds.
        let mut differs = false;
        for x in 0..p_a.width_cols {
            for y in 0..p_a.sky_ceiling_y {
                if w1.get_cell(x, y) != w2.get_cell(x, y) {
                    differs = true;
                    break;
                }
            }
            if differs {
                break;
            }
        }
        assert!(differs, "different seeds must produce different worlds");
    }

    #[test]
    fn ocean_columns_carry_water() {
        // At x=0 (deep abyss), surface is ~sea-42, well below sea.
        // Column should be Bedrock at the very bottom, Stone above,
        // Sand cap, then a Water column up to sea level.
        let mut w = World::new(3);
        let p = small_params();
        stamp_world(&mut w, &p);
        let mut water_rows = 0i32;
        for y in 0..p.sky_ceiling_y {
            let c = w.get_cell(0, y).unwrap();
            if c.material == MaterialId::Air && c.sat.is_full() {
                water_rows += 1;
            }
        }
        assert!(
            water_rows > 20,
            "abyss column should have deep water, got {water_rows}"
        );
    }

    #[test]
    fn stamps_air_above_sea_level_on_land() {
        // Well inland (past plains_end=420) the elevation sits above
        // sea level, so cells just above the surface should be Air
        // with sat=0.
        let mut w = World::new(4);
        let p = small_params();
        stamp_world(&mut w, &p);
        let inland_x = 500;
        let surface = continental_surface_y(p.seed, inland_x, p.sea_level_y);
        assert!(
            surface > p.sea_level_y,
            "inland must be above sea level (got surface={surface}, sea={})",
            p.sea_level_y
        );
        let above = w.get_cell(inland_x, surface + 2).unwrap();
        assert_eq!(above.material, MaterialId::Air);
        assert!(above.sat.is_empty(), "sky cells are dry");
    }

    #[test]
    fn stamps_across_chunk_boundaries() {
        // Stamp then verify a cell in each of the two chunks that
        // straddle x=CHUNK_CELLS_W.
        let mut w = World::new(5);
        let p = small_params();
        stamp_world(&mut w, &p);
        let left = w.get_cell((CHUNK_CELLS_W as i32) - 1, 0).unwrap();
        let right = w.get_cell(CHUNK_CELLS_W as i32, 0).unwrap();
        assert_eq!(left.material, MaterialId::Bedrock);
        assert_eq!(right.material, MaterialId::Bedrock);
    }

    #[test]
    fn shelf_stamps_limestone_when_enabled() {
        let mut w = World::new(7);
        let p = small_params();
        stamp_world(&mut w, &p);
        // A column in the shelf zone (x ≈ 220) should have Limestone
        // in its stone body, and a column well past the shelf (x=500,
        // inland plains) should have Stone.
        let shelf_x = 220;
        let inland_x = 500;
        let surface_shelf = continental_surface_y(p.seed, shelf_x, p.sea_level_y);
        let surface_inland = continental_surface_y(p.seed, inland_x, p.sea_level_y);
        let just_above_bedrock = p.bedrock_floor_y + p.bedrock_thickness + 1;
        // Only inspect a row that lies inside the stone body of both
        // columns (below the sand cap).
        let inspect_y = just_above_bedrock
            .min(surface_shelf - p.sand_cap_thickness)
            .min(surface_inland - p.sand_cap_thickness);
        assert!(inspect_y >= just_above_bedrock);
        assert_eq!(
            w.get_cell(shelf_x, inspect_y).unwrap().material,
            MaterialId::Limestone,
            "shelf column at y={inspect_y} should be Limestone"
        );
        assert_eq!(
            w.get_cell(inland_x, inspect_y).unwrap().material,
            MaterialId::Stone,
            "inland column at y={inspect_y} should be plain Stone"
        );
    }

    #[test]
    fn limestone_toggle_off_gives_all_stone() {
        let mut w = World::new(8);
        let p = WorldgenParams {
            limestone_in_shelf_and_coast: false,
            ..small_params()
        };
        stamp_world(&mut w, &p);
        // Every non-Bedrock cell that isn't Sand should be Stone.
        for x in 0..p.width_cols {
            for y in (p.bedrock_floor_y + p.bedrock_thickness)..p.sky_ceiling_y {
                let Some(c) = w.get_cell(x, y) else {
                    continue;
                };
                assert_ne!(
                    c.material,
                    MaterialId::Limestone,
                    "no Limestone should appear with the toggle off ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn stamped_world_is_stable_across_ticks() {
        // Sanity-check that the fluid + grain rules don't corrupt a
        // freshly-stamped world in the first few ticks. Total sat +
        // grain count should stay constant.
        use crate::rules::tick;
        let mut w = World::new(6);
        let p = small_params();
        stamp_world(&mut w, &p);

        let count_sat = |w: &World| -> i64 {
            let mut sum = 0i64;
            for x in 0..p.width_cols {
                for y in 0..p.sky_ceiling_y {
                    if let Some(c) = w.get_cell(x, y) {
                        sum += c.sat.0 as i64;
                    }
                }
            }
            sum
        };
        let count_grains = |w: &World| -> i64 {
            let mut sum = 0i64;
            for x in 0..p.width_cols {
                for y in 0..p.sky_ceiling_y {
                    if let Some(c) = w.get_cell(x, y) {
                        if crate::cell::is_grain(c.material) {
                            sum += 1;
                        }
                    }
                }
            }
            sum
        };

        let s0 = count_sat(&w);
        let g0 = count_grains(&w);
        assert!(s0 > 0);
        assert!(g0 > 0);
        for _ in 0..25 {
            tick(&mut w);
        }
        let s1 = count_sat(&w);
        let g1 = count_grains(&w);
        assert_eq!(s0, s1, "total sat must be conserved");
        assert_eq!(g0, g1, "grain count must be conserved");
    }
}
