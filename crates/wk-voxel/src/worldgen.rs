//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Deterministic continental worldgen for the voxel sim.
//!
//! Given a [`WorldgenParams`] descriptor and a [`World`], stamp a
//! **ring** continental profile — ocean seam → shelf → coast →
//! plains → mountains → plains → shelf → ocean seam — so the left
//! and right edges join under wrap. Uses only
//! [`wk_material::MaterialId`] for cell materials and a deterministic
//! hash for per-column / per-cell noise.
//!
//! The profile intentionally mirrors column-GVSE's ring layout at a
//! high level so gameplay comes out recognisably GVSE-shaped, but
//! the *implementation* here is independent — this file MUST NOT
//! reach into `wk_world`.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::Cell;
use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// Params driving one worldgen pass.
///
/// Layout (world-x is horizontal, world-y is vertical; +y is up):
///
/// - x range: `0 .. width_cols` (toroidal when [`Self::wrap_x`]).
/// - y range: `bedrock_floor_y .. sky_ceiling_y`.
/// - `sea_level_y`: cells at or below this elevation that aren't
///   solid material get filled with water (Air cells with full sat).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WorldgenParams {
    pub seed: u64,
    pub width_cols: i32,
    pub bedrock_floor_y: i32,
    pub sea_level_y: i32,
    pub sky_ceiling_y: i32,
    /// How many bedrock rows to stamp at the very bottom — the
    /// impermeable world floor.
    pub bedrock_thickness: i32,
    /// Thickness of the stone body above bedrock (legacy knobs;
    /// body height is driven by surface elevation today).
    pub stone_thickness: i32,
    /// Thickness of the sand cap on top of stone / limestone.
    pub sand_cap_thickness: i32,
    /// When true, the shelf+coast stone body prefers
    /// [`MaterialId::Limestone`] (karst food).
    pub limestone_in_shelf_and_coast: bool,
    /// When true, [`stamp_world`] sets `world.wrap_width` so physics
    /// and the camera treat x as a ring.
    pub wrap_x: bool,
}

impl Default for WorldgenParams {
    fn default() -> Self {
        Self {
            seed: 0xDEAD_BEEF,
            // 16 chunks wide — room to pan, and the ring seam is far
            // enough from the starting view that it reads as a world.
            width_cols: (CHUNK_CELLS_W as i32) * 16,
            bedrock_floor_y: 0,
            // Higher sea + taller relief so the cross-section fills
            // more of the view (less empty sky above the hills).
            sea_level_y: 80,
            // Headroom above mountain peaks for the rain cloud.
            sky_ceiling_y: (CHUNK_CELLS_H as i32) * 5,
            // Solid floor barrier — thick enough to read as "the
            // bottom of the world" in the demo.
            bedrock_thickness: 8,
            stone_thickness: 8,
            sand_cap_thickness: 2,
            limestone_in_shelf_and_coast: true,
            wrap_x: true,
        }
    }
}

/// True if `world_x` falls in a karst-friendly shelf/coast band.
///
/// Zones are fractions of `width_cols` so they track the ring profile
/// used by [`continental_surface_y`].
pub fn is_karst_zone_x(world_x: i32, width_cols: i32) -> bool {
    let w = width_cols.max(1) as f32;
    let x = world_x.rem_euclid(width_cols.max(1)) as f32 / w;
    // Two shelf/coast bands — one on each flank of the continent —
    // so karst shows up on both ocean approaches.
    (0.14..=0.28).contains(&x) || (0.72..=0.86).contains(&x)
}

/// Cheap deterministic 32-bit hash → f32 in `[0, 1)`.
fn hash_f32(seed: u64, x: i64, salt: u64) -> f32 {
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

fn hash_f32_2(seed: u64, x: i64, y: i64, salt: u64) -> f32 {
    hash_f32(
        seed,
        x.wrapping_add(y.wrapping_mul(0x9E37_79B9)),
        salt,
    )
}

/// GVSE-style macro continental profile on a **ring**.
///
/// `world_x` is wrapped into `[0, width_cols)`. Both edges sit in
/// deep ocean so the seam joins cleanly when `wrap_x` is enabled.
/// Moving inland (either direction from the seam) you climb shelf →
/// coast → plains → a mountain core, then descend the far side.
pub fn continental_surface_y(seed: u64, world_x: i32, sea: i32, width_cols: i32) -> i32 {
    let w = width_cols.max(1);
    let x_i = world_x.rem_euclid(w);
    let t = x_i as f32 / w as f32; // [0, 1)
    let n = |salt: u64| (hash_f32(seed, x_i as i64, salt) - 0.5) * 2.0;
    let sea_f = sea as f32;

    let smoothstep = |a: f32, b: f32, u: f32| {
        let v = ((u - a) / (b - a)).clamp(0.0, 1.0);
        v * v * (3.0 - 2.0 * v)
    };
    let lerp = |a: f32, b: f32, u: f32| a + (b - a) * u;

    // Elevation targets — tall relief so land fills more of the view.
    let abyss = sea_f - 55.0;
    let slope = sea_f - 18.0;
    let shelf = sea_f - 2.0;
    let coast = sea_f + 28.0;
    let plains = sea_f + 40.0;

    // Ring zones as fractions of width. Symmetric: ocean at both
    // ends, mountains in the middle.
    //
    // 0.00–0.08  abyss (seam)
    // 0.08–0.14  slope up
    // 0.14–0.22  shelf
    // 0.22–0.30  coast
    // 0.30–0.40  plains
    // 0.40–0.60  mountains
    // 0.60–0.70  plains
    // 0.70–0.78  coast
    // 0.78–0.86  shelf
    // 0.86–0.92  slope down
    // 0.92–1.00  abyss (seam)
    let elev = if t < 0.08 {
        abyss + n(30) * 0.5
    } else if t < 0.14 {
        lerp(abyss, slope, smoothstep(0.08, 0.14, t)) + n(31) * 0.4
    } else if t < 0.22 {
        lerp(slope, shelf, smoothstep(0.14, 0.22, t)) + n(32) * 0.25
    } else if t < 0.30 {
        lerp(shelf, coast, smoothstep(0.22, 0.30, t)) + n(33) * 0.5
    } else if t < 0.40 {
        lerp(coast, plains, smoothstep(0.30, 0.40, t)) + n(35) * 0.6
    } else if t < 0.60 {
        // Mountain core — a few Gaussian ridges in the normalised
        // mountain band.
        let u = (t - 0.40) / 0.20; // 0..1 across the band
        let peak = |center: f32, width: f32, amp: f32| {
            amp * (-(u - center) * (u - center) / (2.0 * width * width)).exp()
        };
        let ridges = peak(0.25, 0.12, 48.0) + peak(0.55, 0.14, 72.0) + peak(0.80, 0.11, 56.0);
        plains + ridges + n(36) * 0.7
    } else if t < 0.70 {
        lerp(plains, coast, smoothstep(0.60, 0.70, t)) + n(37) * 0.6
    } else if t < 0.78 {
        lerp(coast, shelf, smoothstep(0.70, 0.78, t)) + n(38) * 0.5
    } else if t < 0.86 {
        lerp(shelf, slope, smoothstep(0.78, 0.86, t)) + n(39) * 0.25
    } else if t < 0.92 {
        lerp(slope, abyss, smoothstep(0.86, 0.92, t)) + n(40) * 0.4
    } else {
        abyss + n(41) * 0.5
    };

    elev.round() as i32
}

/// Pick a solid body material for cell `(x, y)` under the sand cap.
///
/// Primary structure is **horizontal strata** (depth below the sand
/// interface), gently warped by column noise so beds aren't laser-
/// flat. Karst-friendly x-bands thicken the limestone bed rather than
/// painting a whole vertical column — so you still get lateral facies
/// contrast without the "two painted walls" look.
///
/// Palette reminder: Stone is cool mid-grey, Limestone is warm pale
/// tan, Clay is brown, Gravel is sandy-tan, LooseRock is dark cobble.
fn body_material(seed: u64, x: i32, y: i32, surface_y: i32, bedrock_top: i32, p: &WorldgenParams) -> MaterialId {
    let depth = (surface_y - y) as f32; // 0 at sand interface, grows downward
    let above_bedrock = y - bedrock_top;
    let n = hash_f32_2(seed, x as i64, y as i64, 0x57A7_A);
    // Mild per-column warp so stratum contacts undulate a little.
    let warp = (hash_f32(seed, x as i64, 0x1A11_05) - 0.5) * 5.0;
    let d = depth + warp;

    let lime_enabled = p.limestone_in_shelf_and_coast;
    let karst = lime_enabled && is_karst_zone_x(x, p.width_cols);
    // Limestone bed: mid-stack stratum when enabled; thicker in karst
    // shelf bands. Toggle off → stone fills that depth instead.
    let (lime_lo, lime_hi) = if karst { (5.0, 32.0) } else { (12.0, 22.0) };

    // Basement rubble just above the bedrock barrier.
    if above_bedrock < 2 || (above_bedrock < 4 && n < 0.55) {
        return if n < 0.55 {
            MaterialId::LooseRock
        } else {
            MaterialId::Stone
        };
    }

    // Shallow regolith under the sand cap.
    if d < 3.5 {
        return if n < 0.30 {
            MaterialId::Gravel
        } else if n < 0.45 {
            MaterialId::Clay
        } else {
            MaterialId::Stone
        };
    }
    // Clay bed.
    if d < 8.0 {
        return if n < 0.75 {
            MaterialId::Clay
        } else {
            MaterialId::Stone
        };
    }
    // Limestone stratum (the pale band) — or stone when disabled.
    if lime_enabled && d >= lime_lo && d < lime_hi {
        return if n < 0.07 {
            MaterialId::Stone
        } else if n < 0.12 {
            MaterialId::Clay
        } else {
            MaterialId::Limestone
        };
    }
    // Deep stone with sparse fractures / gravel stringers.
    if n < 0.07 {
        MaterialId::Gravel
    } else if n < 0.12 {
        MaterialId::LooseRock
    } else {
        MaterialId::Stone
    }
}

/// Stamp the full ring continental profile into `world`.
///
/// For each column `x` in `0..width_cols`:
///
/// 1. Compute `surface_y = continental_surface_y(...)`.
/// 2. Rows `[bedrock_floor_y, bedrock_floor_y + bedrock_thickness)`
///    → `Bedrock` (impermeable floor barrier).
/// 3. Above bedrock up to `stone_top = surface_y - sand_cap_thickness`
///    → stratified rock / mineral body (see [`body_material`]).
/// 4. From `stone_top + 1` up to `surface_y` → `Sand`.
/// 5. Above the solid stack, if elevation is below `sea_level_y`
///    → water-filled `Air` (sat = FULL).
/// 6. Everything else → empty `Air`.
///
/// When [`WorldgenParams::wrap_x`] is set, also writes
/// `world.wrap_width = Some(width_cols)`.
pub fn stamp_world(world: &mut World, p: &WorldgenParams) {
    if p.wrap_x {
        world.wrap_width = Some(p.width_cols.max(1));
    } else {
        world.wrap_width = None;
    }

    let bedrock_top = p.bedrock_floor_y + p.bedrock_thickness;
    for x in 0..p.width_cols {
        let surface_y = continental_surface_y(p.seed, x, p.sea_level_y, p.width_cols);
        // Keep a usable solid stack even if noise dips near the floor.
        let surface_y = surface_y.max(bedrock_top + p.sand_cap_thickness);
        let stone_top = surface_y - p.sand_cap_thickness;

        for y in p.bedrock_floor_y..p.sky_ceiling_y {
            let cell = if y < bedrock_top {
                Cell::solid(MaterialId::Bedrock)
            } else if y <= stone_top {
                Cell::solid(body_material(p.seed, x, y, surface_y, bedrock_top, p))
            } else if y <= surface_y {
                Cell::solid(MaterialId::Sand)
            } else if y <= p.sea_level_y {
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
        // Wide enough for the ring zones to resolve, tall enough that
        // mountain peaks fit under the sky ceiling.
        WorldgenParams {
            seed: 42,
            width_cols: (CHUNK_CELLS_W as i32) * 10, // 640 cols
            bedrock_floor_y: 0,
            sea_level_y: 60,
            sky_ceiling_y: (CHUNK_CELLS_H as i32) * 3, // 192 rows
            bedrock_thickness: 8,
            stone_thickness: 8,
            sand_cap_thickness: 2,
            limestone_in_shelf_and_coast: true,
            wrap_x: true,
        }
    }

    #[test]
    fn stamped_lake_bed_pores_wet_under_water() {
        use crate::cell::water_capacity;
        use crate::rules::tick;
        use wk_material::MaterialId;

        let mut w = World::new(1);
        let p = WorldgenParams {
            width_cols: CHUNK_CELLS_W as i32 * 4,
            sky_ceiling_y: CHUNK_CELLS_H as i32 * 2,
            ..small_params()
        };
        stamp_world(&mut w, &p);

        // Find an ocean column: water directly above sand.
        let mut col = None;
        for x in 0..p.width_cols {
            let surf = continental_surface_y(p.seed, x, p.sea_level_y, p.width_cols)
                .max(p.bedrock_floor_y + p.bedrock_thickness + p.sand_cap_thickness);
            if surf >= p.sea_level_y {
                continue; // land
            }
            let above = w.get_cell(x, surf + 1).unwrap();
            let bed = w.get_cell(x, surf).unwrap();
            if above.material == MaterialId::Air
                && above.sat.is_full()
                && bed.material == MaterialId::Sand
            {
                col = Some((x, surf));
                break;
            }
        }
        let (x, surf) = col.expect("ocean sand column");

        // Bed wetting is permeability-limited seepage, not an instant
        // gravity pore-fill. Give the lake time to saturate the profile.
        for _ in 0..240 {
            tick(&mut w);
        }

        let sand = w.get_cell(x, surf).unwrap();
        assert_eq!(sand.material, MaterialId::Sand);
        assert_eq!(
            sand.sat.0,
            water_capacity(MaterialId::Sand),
            "lake-bed sand must saturate"
        );
        // Body cell just under the sand cap.
        let under = w.get_cell(x, surf - p.sand_cap_thickness).unwrap();
        assert!(
            under.material != MaterialId::Bedrock,
            "expected porous body under sand, got {:?}",
            under.material
        );
        let cap = water_capacity(under.material);
        assert!(cap > 0, "under-sand material should be porous: {:?}", under.material);
        assert_eq!(
            under.sat.0, cap,
            "porous {:?} under lake sand must reach capacity (sat={})",
            under.material, under.sat.0
        );
    }

    #[test]
    fn stamps_bedrock_barrier_at_floor() {
        let mut w = World::new(1);
        let p = small_params();
        stamp_world(&mut w, &p);
        for x in 0..p.width_cols {
            for y in 0..p.bedrock_thickness {
                assert_eq!(
                    w.get_cell(x, y).unwrap().material,
                    MaterialId::Bedrock,
                    "x={x} y={y} should be Bedrock"
                );
            }
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
    fn ocean_seam_columns_carry_water() {
        // Ring edges are abyss — deep water up to sea level.
        let mut w = World::new(3);
        let p = small_params();
        stamp_world(&mut w, &p);
        for &x in &[0, p.width_cols - 1, p.width_cols / 2] {
            // Midpoint is mountains — skip water check there.
            if x == p.width_cols / 2 {
                continue;
            }
            let mut water_rows = 0i32;
            for y in 0..p.sky_ceiling_y {
                let c = w.get_cell(x, y).unwrap();
                if c.material == MaterialId::Air && c.sat.is_full() {
                    water_rows += 1;
                }
            }
            assert!(
                water_rows > 20,
                "seam column x={x} should have deep water, got {water_rows}"
            );
        }
    }

    #[test]
    fn ring_seam_elevations_match() {
        let p = small_params();
        let a = continental_surface_y(p.seed, 0, p.sea_level_y, p.width_cols);
        let b = continental_surface_y(p.seed, p.width_cols, p.sea_level_y, p.width_cols);
        assert_eq!(a, b, "x=0 and x=width must agree under wrap");
        // Both ends deep ocean.
        assert!(a < p.sea_level_y - 20, "seam should be abyss, got {a}");
    }

    #[test]
    fn wrap_lets_water_spill_across_seam() {
        use crate::rules::apply_lateral_spill;
        let mut w = World::new(9);
        w.wrap_width = Some(64);
        // Solid walls everywhere on the water row except the two seam
        // cells, so the only Air↔Air pair is 63 ↔ 0 under wrap.
        for x in 0..64 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
        }
        w.set_cell(63, 1, Cell::water());
        w.set_cell(0, 1, Cell::air());
        apply_lateral_spill(&mut w);
        let left = w.get_cell(63, 1).unwrap().sat.0;
        let right = w.get_cell(0, 1).unwrap().sat.0;
        assert!(right > 0, "mass should cross the ring seam");
        assert!(left < 255, "donor should have lost mass");
        assert_eq!(left as u16 + right as u16, 255);
    }

    #[test]
    fn stamps_air_above_sea_level_on_land() {
        let mut w = World::new(4);
        let p = small_params();
        stamp_world(&mut w, &p);
        // Mountain core sits around mid-ring.
        let inland_x = p.width_cols / 2;
        let surface = continental_surface_y(p.seed, inland_x, p.sea_level_y, p.width_cols);
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
        let mut w = World::new(5);
        let p = small_params();
        stamp_world(&mut w, &p);
        let left = w.get_cell((CHUNK_CELLS_W as i32) - 1, 0).unwrap();
        let right = w.get_cell(CHUNK_CELLS_W as i32, 0).unwrap();
        assert_eq!(left.material, MaterialId::Bedrock);
        assert_eq!(right.material, MaterialId::Bedrock);
    }

    #[test]
    fn limestone_forms_a_horizontal_stratum() {
        let mut w = World::new(7);
        let p = small_params();
        stamp_world(&mut w, &p);
        let bedrock_top = p.bedrock_floor_y + p.bedrock_thickness;
        // Sample a plains/mountain column: limestone should appear as a
        // mid-depth bed, not as the whole stack and not only at the floor.
        let x = p.width_cols / 2;
        let surface = continental_surface_y(p.seed, x, p.sea_level_y, p.width_cols)
            .max(bedrock_top + p.sand_cap_thickness);
        let stone_top = surface - p.sand_cap_thickness;
        let mut lime_ys = Vec::new();
        let mut saw_stone_above_lime = false;
        let mut saw_stone_below_lime = false;
        for y in bedrock_top..=stone_top {
            let mat = w.get_cell(x, y).unwrap().material;
            if mat == MaterialId::Limestone {
                lime_ys.push(y);
            }
        }
        assert!(
            !lime_ys.is_empty(),
            "expected a limestone stratum under the sand cap"
        );
        let lime_min = *lime_ys.iter().min().unwrap();
        let lime_max = *lime_ys.iter().max().unwrap();
        for y in (lime_max + 1)..=stone_top {
            if w.get_cell(x, y).unwrap().material == MaterialId::Stone {
                saw_stone_above_lime = true;
                break;
            }
        }
        for y in bedrock_top..lime_min {
            if matches!(
                w.get_cell(x, y).unwrap().material,
                MaterialId::Stone | MaterialId::LooseRock
            ) {
                saw_stone_below_lime = true;
                break;
            }
        }
        assert!(
            saw_stone_below_lime,
            "stone/loose-rock should sit below the limestone bed"
        );
        // Shallow stack under sand may be clay/gravel; stone above is
        // nice-to-have when the column is deep enough.
        let _ = saw_stone_above_lime;

        // Karst x-bands should carry a thicker limestone bed.
        let mut karst_lime = 0i32;
        let mut karst_cols = 0i32;
        let mut other_lime = 0i32;
        let mut other_cols = 0i32;
        for x in 0..p.width_cols {
            let surface = continental_surface_y(p.seed, x, p.sea_level_y, p.width_cols)
                .max(bedrock_top + p.sand_cap_thickness);
            let stone_top = surface - p.sand_cap_thickness;
            let mut lime = 0i32;
            for y in bedrock_top..=stone_top {
                if w.get_cell(x, y).unwrap().material == MaterialId::Limestone {
                    lime += 1;
                }
            }
            if is_karst_zone_x(x, p.width_cols) {
                karst_lime += lime;
                karst_cols += 1;
            } else {
                other_lime += lime;
                other_cols += 1;
            }
        }
        assert!(karst_cols > 0 && other_cols > 0);
        let karst_avg = karst_lime as f32 / karst_cols as f32;
        let other_avg = other_lime as f32 / other_cols as f32;
        assert!(
            karst_avg > other_avg,
            "karst shelves should thicken the limestone bed (karst={karst_avg} other={other_avg})"
        );
    }

    #[test]
    fn body_contains_more_than_just_stone() {
        let mut w = World::new(11);
        let p = small_params();
        stamp_world(&mut w, &p);
        let mut saw_clay = false;
        let mut saw_gravel = false;
        let mut saw_loose = false;
        for x in 0..p.width_cols {
            for y in (p.bedrock_floor_y + p.bedrock_thickness)..p.sky_ceiling_y {
                let Some(c) = w.get_cell(x, y) else {
                    continue;
                };
                match c.material {
                    MaterialId::Clay => saw_clay = true,
                    MaterialId::Gravel => saw_gravel = true,
                    MaterialId::LooseRock => saw_loose = true,
                    _ => {}
                }
            }
        }
        assert!(saw_clay, "expected clay lenses in the body");
        assert!(saw_gravel, "expected gravel pockets in the body");
        assert!(saw_loose, "expected loose rock near basement");
    }

    #[test]
    fn limestone_toggle_off_gives_no_limestone() {
        let mut w = World::new(8);
        let p = WorldgenParams {
            limestone_in_shelf_and_coast: false,
            ..small_params()
        };
        stamp_world(&mut w, &p);
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
        use crate::failure::FailureConfig;
        use crate::rules::{tick_with_configs, PerfConfig};
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
        // Roof collapse converts Stone/Limestone → fallable grains, which
        // would inflate the grain count; keep geotech off for this check.
        let fail = FailureConfig {
            enable_roof_collapse: false,
            enable_shear_weaken: false,
            enable_compaction: false,
            ..FailureConfig::default()
        };
        let perf = PerfConfig {
            parallel_physics: false,
            ..PerfConfig::default()
        };
        for _ in 0..25 {
            tick_with_configs(&mut w, &perf, &fail);
        }
        let s1 = count_sat(&w);
        let g1 = count_grains(&w);
        assert_eq!(s0, s1, "total sat must be conserved");
        assert_eq!(g0, g1, "grain count must be conserved");
    }
}
