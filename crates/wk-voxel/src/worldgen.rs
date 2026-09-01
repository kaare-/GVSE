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

use crate::cell::{falls_through_empty_air, is_grain, Cell};
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
    hash_f32(seed, x.wrapping_add(y.wrapping_mul(0x9E37_79B9)), salt)
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
/// Topmost solid cell in a column of the **live** world, found by walking from a
/// hint.
///
/// [`continental_surface_y`] recomputes the *original* procedural profile from the
/// seed, so erosion, collapse, karst dissolution and hand edits are invisible to
/// it. Every weather consumer that asks where the ground is has been reading that
/// stale value.
///
/// Walking from a hint rather than caching is deliberate. A maintained cache needs
/// invalidation on every solidity change, and `set_cell` is hot enough that the
/// extra read was worth avoiding; terrain also moves *locally*, so the procedural
/// value stays a good hint indefinitely and the walk is a few cells. Same pattern
/// `clouds::cloud_floor_y` already uses.
///
/// Returns the hint unchanged when the column is not loaded, so callers degrade to
/// today's behaviour rather than to zero.
pub fn live_surface_y(world: &World, gx: i32, hint: i32, search: i32) -> i32 {
    let jx = world.wrap_x(gx);
    let solid_at = |y: i32| {
        matches!(world.get_cell(jx, y), Some(c) if c.material.is_solid())
    };
    if world.get_cell(jx, hint).is_none() {
        return hint;
    }
    let raw = if solid_at(hint) {
        // Ground has risen (deposition, F3 tower): climb to the top.
        // The short window used to stop at hint+64, so a stack built
        // toward the sky left a fake crest and a wind tunnel through
        // the extra rock (y≈250–260 on a 320 ceiling).
        let mut y = hint;
        for _ in 0..search {
            if !solid_at(y + 1) {
                break;
            }
            y += 1;
        }
        if solid_at(y + 1) {
            let extra = LIVE_SURFACE_DESCENT_MAX.saturating_sub(search);
            for _ in 0..extra {
                if !solid_at(y + 1) {
                    break;
                }
                y += 1;
            }
        }
        y
    } else {
        // Ground has fallen (erosion, dissolution): descend to the first solid.
        let mut y = hint;
        let mut found = None;
        for _ in 0..search {
            y -= 1;
            if solid_at(y) {
                found = Some(y);
                break;
            }
        }
        // A deleted hill can drop the crest by more than `search` (mountains
        // sit 80–150 cells above the bed). Giving up after 64 cells is what
        // left a ghost mountain in the ridge / wind maps after F3 erase.
        // Keep walking through loaded air for the remaining bed.
        if found.is_none() {
            let extra = LIVE_SURFACE_DESCENT_MAX.saturating_sub(search);
            for _ in 0..extra {
                y -= 1;
                if y < 0 {
                    break;
                }
                match world.get_cell(jx, y) {
                    None => break,
                    Some(c) if c.material.is_solid() => {
                        found = Some(y);
                        break;
                    }
                    _ => {}
                }
            }
        }
        // A created empty chunk (unstamped neighbor) is loaded air with no
        // solid — not a deleted hill. Keep the seed so wind / climate on a
        // one-column stamp still see the rest of the profile.
        found.unwrap_or(hint)
    };
    // Empty shafts and unloaded columns keep the hint. Peeling those
    // would walk the search window of air and invent a surface.
    if !solid_at(raw) {
        return raw;
    }
    peel_airborne_loose(world, jx, raw, search)
}

/// Snow is a solid, so a walk that stops on the first solid treats a falling
/// flake as the hill — the same needle the ridge plates used to draw. Seated
/// pack stays: that *is* the surface weather should see.
fn peel_airborne_loose(world: &World, gx: i32, start: i32, search: i32) -> i32 {
    let mut y = start;
    let max = search.max(LIVE_SURFACE_DESCENT_MAX);
    for _ in 0..max {
        let Some(c) = world.get_cell(gx, y) else {
            return y.max(0);
        };
        if c.material == MaterialId::Air || airborne_loose_at(world, gx, y, c) {
            y -= 1;
            if y < 0 {
                return 0;
            }
            continue;
        }
        return y;
    }
    y.max(0)
}

/// Loose pack (snow / ice / organic) with empty or haze air under it.
///
/// Full-sat air is a lake seat — floating ice and rafts stay a surface.
/// Anything else under a flake is sky, so the flake is not the hill.
pub fn airborne_loose_at(world: &World, gx: i32, y: i32, cell: Cell) -> bool {
    // Snow / ice / organic, and leftover grains (loose limestone scraps
    // after a hill erase) with empty air under them are not the hill.
    if !falls_through_empty_air(cell.material) && !is_grain(cell.material) {
        return false;
    }
    match world.get_cell(gx, y - 1) {
        Some(below) if below.material != MaterialId::Air => false,
        Some(below) if below.sat.is_full() => false,
        _ => true,
    }
}

/// How far [`live_surface_y`] walks for local erosion / deposition.
///
/// Generous enough for real collapse. A whole-hill F3 erase can drop the
/// crest farther than this — [`LIVE_SURFACE_DESCENT_MAX`] covers that.
pub const LIVE_SURFACE_SEARCH: i32 = 64;
/// Extra walk when the short window is not enough. Sky ceiling is 320:
/// a mountain wipe must reach the bed, an F3 tower must reach the crest.
pub const LIVE_SURFACE_DESCENT_MAX: i32 = 320;

/// Live top-of-column: procedural profile as the hint, then walk the world.
///
/// This is what every weather consumer should call instead of
/// [`continental_surface_y`]. An unloaded column degrades to the hint, so
/// callers that used to read the seed profile keep working.
#[inline]
pub fn live_surface_at(world: &World, seed: u64, gx: i32, sea: i32, width_cols: i32) -> i32 {
    let hint = continental_surface_y(seed, gx, sea, width_cols);
    live_surface_y(world, gx, hint, LIVE_SURFACE_SEARCH)
}

/// Skin the air actually sits on: rock bed, then standing water / ice.
///
/// [`live_surface_y`] stops on the first solid, so a pond reports its
/// excavated floor. Climate, couple, and free-air must sit on the
/// waterline — otherwise every inland lake is a fake ocean hole with
/// two "coast" columns.
///
/// `32` matches [`crate::GRAIN_REPOSE_HAZE_MAX`] (haze vs standing film)
/// without pulling `rules` into worldgen.
pub fn live_skin_y(world: &World, gx: i32, rock_y: i32) -> i32 {
    let jx = world.wrap_x(gx);
    let mut y = rock_y;
    for _ in 0..96 {
        match world.get_cell(jx, y + 1) {
            Some(c) if c.material != MaterialId::Air => y += 1,
            Some(c) if c.sat.0 > 32 => y += 1,
            _ => break,
        }
    }
    y
}

/// Fraction of the world occupied by the overturned block.
const TECTONIC_BLOCK_FRAC: f32 = 0.16;
/// Width of the blend at each edge of the block, as a fraction of its own width.
/// Without it the contact is a razor line, which reads as a rendering seam rather
/// than as geology.
const TECTONIC_EDGE_FRAC: f32 = 0.22;
/// How far the block's bands tilt, in cells of depth per cell of x.
const TECTONIC_DIP: f32 = 0.35;

/// The depth the strata sequence is indexed by.
///
/// Normally just the depth below the surface, so bands lie parallel to it. Inside
/// one seed-placed block the coordinate is **tilted and mirrored**, so the same
/// sequence appears dipping and overturned — deep rock carried up against shallow
/// beds, which is what a fault block or an overturned fold looks like in section.
///
/// A transform rather than new geology: every stratum rule, every material and
/// every contact behaves exactly as it does elsewhere, so the block gets limestone
/// bands, bentonite caps and clast sprinkles for free, just at the wrong angle and
/// in the wrong order. That is also why it stays honest — nothing here can produce
/// a material combination the rest of the world could not.
///
/// Blended in at the edges, so the contacts are transitional. A hard boundary
/// looked like a seam in the renderer rather than a fault.
fn tectonic_depth(seed: u64, x: i32, depth: f32, column_thickness: i32, p: &WorldgenParams) -> f32 {
    let w = p.width_cols.max(1) as f32;
    let block_w = (w * TECTONIC_BLOCK_FRAC).max(8.0);
    // Placed by seed, but away from the wrap seam so the block is not cut in half.
    let start = 0.10 * w + (hash_f32(seed, 0, 0x7EC7_0111) * 0.55 * w);
    let local = x as f32 - start;
    if local < 0.0 || local > block_w {
        return depth;
    }
    let edge = (block_w * TECTONIC_EDGE_FRAC).max(1.0);
    let blend = (local / edge).min((block_w - local) / edge).clamp(0.0, 1.0);
    if blend <= 0.0 {
        return depth;
    }
    let thickness = column_thickness.max(1) as f32;
    // Mirrored: the sequence runs the other way through the stack.
    let flipped = (thickness - depth).max(0.0);
    // ...and dipping, so the bands are not merely upside down but at an angle.
    let dipped = flipped + (local - block_w * 0.5) * TECTONIC_DIP;
    depth + (dipped - depth) * blend
}

fn body_material(
    seed: u64,
    x: i32,
    y: i32,
    surface_y: i32,
    bedrock_top: i32,
    p: &WorldgenParams,
) -> MaterialId {
    let depth = (surface_y - y) as f32; // 0 at sand interface, grows downward
    let above_bedrock = y - bedrock_top;
    let n = hash_f32_2(seed, x as i64, y as i64, 0x57A7_A);
    // Coherent lens noise: picks *regions*, not speckle, so permeable and
    // porous material forms connected bodies. Groundwater then prefers those
    // paths, and since wetter pores conduct faster that choice reinforces
    // itself. Two octaves — broad lenses plus smaller pockets.
    // Horizontally stretched: sampling y at a shorter wavelength than x makes the
    // bodies long and flat, which is what a bed *is*. Isotropic noise gives round
    // blobs, and blobs read as pockets rather than strata however large they are.
    let lens = 0.65 * lens_noise(seed, x, y * 3, 40, 10, 0x1E_1501)
        + 0.35 * lens_noise(seed, x, y * 2, 14, 4, 0x1E_1502);
    // Mild per-column warp so stratum contacts undulate a little.
    let warp = (hash_f32(seed, x as i64, 0x1A11_05) - 0.5) * 5.0;
    let d = tectonic_depth(seed, x, depth + warp, surface_y - bedrock_top, p);

    let lime_enabled = p.limestone_in_shelf_and_coast;
    let karst = lime_enabled && is_karst_zone_x(x, p.width_cols);
    // Limestone bed: mid-stack stratum when enabled; thicker in karst
    // shelf bands. Toggle off → stone fills that depth instead.
    // Thicker than it was (was 12..22 outside karst): playtest wanted more
    // limestone, and a thin bed is also hydraulically uninteresting — the
    // limestone/stone contact is where the best perching was observed, so a taller
    // band gives more of it.
    let (lime_lo, lime_hi) = if karst { (5.0, 38.0) } else { (10.0, 30.0) };

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
        return if lens < 0.30 {
            MaterialId::Gravel
        } else if lens < 0.45 {
            MaterialId::Clay
        } else {
            MaterialId::Stone
        };
    }
    // Bentonite aquitard capping the limestone aquifer.
    //
    // A confined aquifer needs a seal, and clay is not one: at permeability 10
    // against limestone's 140 it is only ~14× tighter, which still equalises
    // over a geological cadence. Without a real cap there is no confined head,
    // which is why a hand-dug well found no pressure.
    //
    // Deliberately *not* continuous. A perfect seal would also block recharge
    // and the aquifer beneath would never fill; real confined aquifers take
    // their water where the aquitard is absent. The gaps are those windows.
    if lime_enabled {
        let cap_lo = (lime_lo - 2.0f32).max(3.5);
        if d >= cap_lo && d < lime_lo && lens >= 0.15 {
            return MaterialId::Bentonite;
        }
    }
    // Clay bed.
    if d < 8.0 {
        return if lens < 0.75 {
            MaterialId::Clay
        } else {
            MaterialId::Stone
        };
    }
    // Limestone stratum (the pale band) — or stone when disabled.
    if lime_enabled && d >= lime_lo && d < lime_hi {
        return if lens < 0.07 {
            MaterialId::Stone
        } else if lens < 0.12 {
            MaterialId::Clay
        } else {
            MaterialId::Limestone
        };
    }
    // A second carbonate band well below the first, so the deep stack is not one
    // uniform stone mass. Real sections repeat: sequences of beds, not a single
    // sandwich. Its contacts give another perching horizon far from the surface.
    if lime_enabled && d >= lime_hi + 22.0 && d < lime_hi + 40.0 && lens > 0.30 {
        return MaterialId::Limestone;
    }
    // Deep stone cut by connected gravel / fractured stringers — the
    // preferential flow paths for groundwater.
    if lens < 0.14 {
        return MaterialId::Gravel;
    }
    if lens < 0.24 {
        return MaterialId::LooseRock;
    }
    // Scattered clasts through the rock mass.
    //
    // The lens stringers above are large and coherent by design, so they read as
    // beds rather than as the sprinkle of loose material a real rock mass carries.
    // A second, much finer noise adds isolated pockets — small enough to stay
    // inclusions rather than becoming a second set of beds, and they matter
    // hydraulically as well as visually: an isolated permeable pocket is a
    // retention site, which is the counterpart to the conduits.
    let speck = lens_noise(seed, x, y, 5, 3, 0xA0_2E_2001);
    if speck < 0.055 {
        return MaterialId::Sand;
    }
    if speck < 0.10 {
        return MaterialId::Gravel;
    }
    if speck < 0.145 {
        return MaterialId::LooseRock;
    }
    // Conglomerate and flowstone as *native* rock, not only as cementation and
    // precipitate products. Both occur geologically without a simulated history,
    // and seeding them means a fresh world already shows the materials the water
    // cycle can otherwise only make over a long soak.
    if speck > 0.955 {
        return MaterialId::Conglomerate;
    }
    // Flowstone lines fractures, so it follows the ridged vein locus rather than a
    // blob — the same field the pore veins use, thresholded near its crest.
    let vein = lens_noise(seed, x, y * 3, 24, 9, 0xA0_2E_1003);
    // Rare. At 0.94 flowstone was everywhere, which both misrepresents it — it is
    // a fracture lining, not a rock type — and, being pale, read as cavities.
    if ridged(vein) > 0.988 {
        return MaterialId::Flowstone;
    }
    MaterialId::Stone
}

#[cfg(test)]
mod lens_tests {
    use super::*;

    #[test]
    fn lens_noise_is_coherent_not_speckle() {
        // Neighbouring cells must agree far more than white noise does,
        // otherwise permeable material is salt-and-pepper and groundwater has
        // no connected path to prefer.
        let seed = 12345u64;
        let mut lens_delta = 0.0f32;
        let mut white_delta = 0.0f32;
        let n = 400;
        for i in 0..n {
            let (x, y) = (i % 40, i / 40);
            lens_delta +=
                (lens_noise(seed, x, y, 24, 10, 7) - lens_noise(seed, x + 1, y, 24, 10, 7)).abs();
            white_delta += (hash_f32_2(seed, x as i64, y as i64, 7)
                - hash_f32_2(seed, x as i64 + 1, y as i64, 7))
            .abs();
        }
        assert!(
            lens_delta * 4.0 < white_delta,
            "lens noise must be much smoother than white noise \
             (lens={lens_delta:.2} white={white_delta:.2})"
        );
    }

    #[test]
    fn the_live_surface_follows_erosion_and_deposition() {
        // continental_surface_y recomputes the *original* profile, so every weather
        // consumer that asks where the ground is has been reading a map from
        // worldgen. This is the replacement they need.
        let p = WorldgenParams::default();
        let mut w = World::new(p.seed);
        stamp_world(&mut w, &p);
        let x = 300;
        let hint = continental_surface_y(p.seed, x, p.sea_level_y, p.width_cols);
        assert_eq!(
            live_surface_y(&w, x, hint, LIVE_SURFACE_SEARCH),
            hint,
            "an untouched column should agree with the procedural profile"
        );
        assert_eq!(
            live_surface_at(&w, p.seed, x, p.sea_level_y, p.width_cols),
            hint,
            "live_surface_at is the hint-then-walk helper weather uses"
        );

        // Erode the top eight cells away.
        for dy in 0..8 {
            w.set_cell(x, hint - dy, Cell::air());
        }
        assert_eq!(
            live_surface_y(&w, x, hint, LIVE_SURFACE_SEARCH),
            hint - 8,
            "the surface should descend with erosion"
        );

        // Pile five cells back on top of the original level.
        for dy in 1..=5 {
            w.set_cell(x, hint + dy, Cell::solid(MaterialId::Sand));
        }
        // The hint is now buried, so this exercises the climbing branch.
        assert_eq!(
            live_surface_y(&w, x, hint + 1, LIVE_SURFACE_SEARCH),
            hint + 5,
            "the surface should rise with deposition"
        );
    }

    #[test]
    fn live_skin_sits_on_the_pond_not_the_excavated_bed() {
        let mut w = World::new(1);
        let x: i32 = 4;
        for y in 0i32..=8 {
            w.ensure_chunk(crate::chunk::ChunkCoord::new(
                x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
            ));
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
        for y in 9i32..=12 {
            w.ensure_chunk(crate::chunk::ChunkCoord::new(
                x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
            ));
            w.set_cell(x, y, Cell::water());
        }
        assert_eq!(live_surface_y(&w, x, 8, LIVE_SURFACE_SEARCH), 8);
        assert_eq!(live_skin_y(&w, x, 8), 12, "air sits on the waterline");
    }

    #[test]
    fn the_live_surface_keeps_the_hint_when_it_cannot_do_better() {
        // Degrading to today's behaviour beats inventing a surface: an unloaded
        // column must not report the bottom of the search window, which would put
        // weather underground.
        let p = WorldgenParams::default();
        let w = World::new(p.seed); // nothing stamped
        assert_eq!(live_surface_y(&w, 10, 50, LIVE_SURFACE_SEARCH), 50);

        // A created empty column is an unstamped neighbor, not a deleted
        // hill. Keep the seed. A real F3 wipe still has a bed the extra
        // walk finds (see `live_surface_follows_a_deleted_hill`).
        let mut w2 = World::new(p.seed);
        w2.ensure_chunk(crate::chunk::ChunkCoord::new(0, 0));
        for y in 0..64 {
            w2.set_cell(5, y, Cell::air());
        }
        assert_eq!(
            live_surface_y(&w2, 5, 60, 8),
            60,
            "an empty created column keeps the seed — that is not a deleted hill"
        );
    }

    #[test]
    fn live_surface_follows_a_built_tower_past_the_search_window() {
        // Seed crest + 64 used to be the climb cap. F3-stacking toward
        // the sky left a fake surface and wind through the extra rock.
        let mut w = World::new(1);
        let x = 8;
        let hint = 40;
        let crest = 160;
        for cy in 0..=3 {
            w.ensure_chunk(crate::chunk::ChunkCoord::new(0, cy));
        }
        for y in 0..=crest {
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
        assert_eq!(
            live_surface_y(&w, x, hint, LIVE_SURFACE_SEARCH),
            crest,
            "a tower 120 cells above the hint must not stop at hint+64"
        );
    }

    #[test]
    fn live_surface_follows_a_deleted_hill_not_the_seed_crest() {
        // Mountains sit well above the 64-cell local window. After the hill
        // is erased, weather / ridges / wind must sit on the remaining bed.
        let mut w = World::new(1);
        let x = 8;
        let bed = 20;
        let crest = 120;
        for cy in 0..=2 {
            w.ensure_chunk(crate::chunk::ChunkCoord::new(0, cy));
        }
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..=bed {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
        for y in (bed + 1)..=crest {
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
        assert_eq!(live_surface_y(&w, x, crest, LIVE_SURFACE_SEARCH), crest);
        for y in (bed + 1)..=crest {
            w.set_cell(x, y, Cell::air());
        }
        // Leftover loose scrap at the old crest must not pin the surface.
        w.set_cell(x, crest - 4, Cell::solid(MaterialId::LooseLimestone));
        assert_eq!(
            live_surface_y(&w, x, crest, LIVE_SURFACE_SEARCH),
            bed,
            "erased hill must drop the live surface to the remaining bed"
        );
    }

    #[test]
    fn airborne_snow_is_not_the_live_surface() {
        // Weather used the same "first solid" walk as the old ridge scan.
        // A flake in the sky became the hill: orographic dump, cloud floor
        // and frost all sat on needles that were gone a few ticks later.
        // The tests that landed the helper never put snow in the column,
        // so they could not catch it.
        let p = WorldgenParams::default();
        let mut w = World::new(p.seed);
        let x = 8;
        let hint = 20;
        w.ensure_chunk(crate::chunk::ChunkCoord::new(0, 0));
        w.set_cell(x, hint, Cell::solid(MaterialId::Stone));
        w.set_cell(x, hint + 30, Cell::solid(MaterialId::Snow));
        assert_eq!(
            live_surface_y(&w, x, hint, LIVE_SURFACE_SEARCH),
            hint,
            "a flake above the stone is not the hill"
        );
        // Walking *down* from a high hint is the dangerous branch: the
        // first solid is the flake.
        assert_eq!(
            live_surface_y(&w, x, hint + 40, LIVE_SURFACE_SEARCH),
            hint,
            "descending onto a flake must keep walking to the stone"
        );
    }

    #[test]
    fn seated_snowpack_is_the_live_surface() {
        let p = WorldgenParams::default();
        let mut w = World::new(p.seed);
        let x = 8;
        let hint = 20;
        w.ensure_chunk(crate::chunk::ChunkCoord::new(0, 0));
        w.set_cell(x, hint, Cell::solid(MaterialId::Stone));
        w.set_cell(x, hint + 1, Cell::solid(MaterialId::Snow));
        w.set_cell(x, hint + 2, Cell::solid(MaterialId::Snow));
        assert_eq!(
            live_surface_y(&w, x, hint, LIVE_SURFACE_SEARCH),
            hint + 2,
            "landed pack is the surface weather should see"
        );
    }

    #[test]
    fn a_tectonic_block_overturns_the_strata() {
        // Playtest asked for "a section where plate tectonics has flipped the
        // stratas". Implemented as a coordinate transform, so every stratum rule,
        // material and contact behaves as it does elsewhere — the block just gets
        // them at the wrong angle and in the wrong order.
        let p = WorldgenParams::default();
        let thickness = 60;
        // Find the block by scanning for columns whose structural depth departs
        // from the plain depth.
        let mut inside = Vec::new();
        for x in 0..p.width_cols {
            let d = tectonic_depth(p.seed, x, 20.0, thickness, &p);
            if (d - 20.0).abs() > 1.0 {
                inside.push(x);
            }
        }
        assert!(
            !inside.is_empty(),
            "there should be an overturned block somewhere in the world"
        );
        let frac = inside.len() as f32 / p.width_cols as f32;
        assert!(
            frac < 0.35,
            "the block is a section, not the world ({:.0}%)",
            frac * 100.0
        );

        // Inside it, the sequence must actually differ at the same depth, or the
        // transform is decoration.
        let mid = inside[inside.len() / 2];
        let outside = (0..p.width_cols)
            .find(|x| (tectonic_depth(p.seed, *x, 20.0, thickness, &p) - 20.0).abs() < 0.01)
            .expect("some column should be untransformed");
        let surface_in = continental_surface_y(p.seed, mid, p.sea_level_y, p.width_cols);
        let surface_out = continental_surface_y(p.seed, outside, p.sea_level_y, p.width_cols);
        let bedrock_top = p.bedrock_floor_y + p.bedrock_thickness;
        let mut differs = 0;
        for d in 6..30 {
            let a = body_material(p.seed, mid, surface_in - d, surface_in, bedrock_top, &p);
            let b = body_material(p.seed, outside, surface_out - d, surface_out, bedrock_top, &p);
            if a != b {
                differs += 1;
            }
        }
        assert!(
            differs > 4,
            "the overturned block should present a different sequence at the same \
             depth (only {differs} of 24 depths differed)"
        );
    }

    #[test]
    fn the_tectonic_contact_is_gradual_not_a_seam() {
        // A hard boundary read as a renderer seam rather than a fault.
        let p = WorldgenParams::default();
        let thickness = 60;
        let ds: Vec<f32> = (0..p.width_cols)
            .map(|x| tectonic_depth(p.seed, x, 20.0, thickness, &p))
            .collect();
        let worst = ds
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 6.0,
            "structural depth should change smoothly between columns, worst jump {worst}"
        );
    }

    #[test]
    fn a_groundwater_table_is_stamped_at_sea_level() {
        // Soaking a dry world to a full table took a whole night to reach a state
        // we already know the answer to: below the table, everything is saturated
        // to capacity — that is what a water table *is*. The interesting behaviour
        // is the vadose zone above it, so the soak should start there.
        let p = WorldgenParams::default();
        let mut w = World::new(p.seed);
        stamp_world(&mut w, &p);

        let mut below_full = 0u32;
        let mut below_total = 0u32;
        let mut above_wet = 0u32;
        let mut above_total = 0u32;
        for x in (0..p.width_cols).step_by(17) {
            for y in (p.bedrock_floor_y + 4)..(p.sea_level_y + 30) {
                let Some(c) = w.get_cell(x, y) else { continue };
                if !c.material.is_solid() {
                    continue;
                }
                let cap = crate::cell::water_capacity_cell(c, &w.hydro);
                if cap == 0 {
                    continue;
                }
                if y <= p.sea_level_y {
                    below_total += 1;
                    if c.sat.0 >= cap {
                        below_full += 1;
                    }
                } else {
                    above_total += 1;
                    if c.sat.0 > 0 {
                        above_wet += 1;
                    }
                }
            }
        }
        assert!(below_total > 100 && above_total > 100, "fixture should span the table");
        assert_eq!(
            below_full, below_total,
            "every porous cell below the table must start at capacity"
        );
        assert_eq!(
            above_wet, 0,
            "the vadose zone above the table must start dry — that is the part the \
             soak is for"
        );
    }

    #[test]
    fn rock_carries_scattered_clasts_and_the_new_rock_types() {
        // Playtest missed "the cake sprinkle of sand, loose rock and gravel
        // throughout the rock formations", and asked for conglomerate and flowstone
        // to exist in a fresh world rather than only as products of a long soak.
        let p = WorldgenParams::default();
        let mut counts = std::collections::HashMap::new();
        for x in 0..600 {
            let surface = continental_surface_y(p.seed, x, p.sea_level_y, p.width_cols);
            for y in (p.bedrock_floor_y + 6)..(surface - 12).max(p.bedrock_floor_y + 7) {
                let m = body_material(p.seed, x, y, surface, p.bedrock_floor_y + p.bedrock_thickness, &p);
                *counts.entry(m).or_insert(0u32) += 1;
            }
        }
        let total: u32 = counts.values().sum();
        assert!(total > 1000, "fixture should sample a real rock mass");
        for m in [
            MaterialId::Sand,
            MaterialId::Gravel,
            MaterialId::LooseRock,
            MaterialId::Conglomerate,
            MaterialId::Flowstone,
        ] {
            let n = counts.get(&m).copied().unwrap_or(0);
            assert!(n > 0, "{m:?} should appear in the rock mass");
            // Inclusions, not beds: each stays a small minority so the rock is
            // still rock.
            assert!(
                (n as f32 / total as f32) < 0.25,
                "{m:?} is {:.1}% of the mass — that is a stratum, not a sprinkle",
                100.0 * n as f32 / total as f32
            );
        }
        // Stone must still dominate.
        let stone = counts.get(&MaterialId::Stone).copied().unwrap_or(0);
        assert!(
            stone as f32 / total as f32 > 0.35,
            "stone should still be the rock mass ({:.1}%)",
            100.0 * stone as f32 / total as f32
        );
    }

    #[test]
    fn ridged_peaks_on_a_locus_not_at_the_extremes() {
        // The property that turns blobs into veins: a ridged field is maximal
        // where the underlying noise crosses its midpoint, and the level set of a
        // continuous field is a *curve* — long, thin and connected. Ordinary noise
        // peaks in patches, and a patch of permeable rock is a lens that water
        // equalises through rather than a conduit it can deepen.
        assert!((ridged(0.5) - 1.0).abs() < 1e-6, "crest at the midpoint");
        assert!(ridged(0.0).abs() < 1e-6, "trough at the low extreme");
        assert!(ridged(1.0).abs() < 1e-6, "trough at the high extreme");
        // Symmetric about the crest.
        assert!((ridged(0.3) - ridged(0.7)).abs() < 1e-6);
    }

    #[test]
    fn the_pore_field_has_narrow_high_veins_not_broad_lenses() {
        // High-pore cells should be a small minority (crests, not regions) while
        // still being present. A broad ridge would just be another lens.
        let seed = 99u64;
        let mut high = 0u32;
        let mut total = 0u32;
        for y in 10..70 {
            for x in 0..400 {
                if pore_coordinate(seed, x, y, 100) >= 200 {
                    high += 1;
                }
                total += 1;
            }
        }
        let frac = high as f32 / total as f32;
        assert!(
            frac > 0.001,
            "veins must actually exist (high-pore fraction {frac})"
        );
        assert!(
            frac < 0.30,
            "veins must be narrow, not most of the rock (high-pore fraction {frac})"
        );
    }

    #[test]
    fn pore_coordinate_is_coherent_and_not_material_noise() {
        let seed = 12345u64;
        let mut neighbour_delta = 0.0f32;
        let mut broad_span = 0u8;
        let mut min = u8::MAX;
        let mut max = 0u8;
        for y in 20..40 {
            for x in 0..80 {
                let a = pore_coordinate(seed, x, y, 100);
                let b = pore_coordinate(seed, x + 1, y, 100);
                neighbour_delta += (a as f32 - b as f32).abs();
                min = min.min(a);
                max = max.max(a);
            }
        }
        broad_span = broad_span.max(max.saturating_sub(min));
        assert!(
            neighbour_delta / (20.0 * 80.0) < 12.0,
            "neighbour pore values should vary smoothly"
        );
        assert!(
            broad_span > 60,
            "worldgen pore field needs useful variation"
        );
    }
}

/// Coherent value noise in `0..1`, bilinear over a lattice of
/// `period_x × period_y` cells. Cheap stand-in for Perlin: the hash is the
/// same one worldgen already uses, just sampled at lattice corners and
/// smoothstep-interpolated so neighbouring cells agree.
fn lens_noise(seed: u64, x: i32, y: i32, period_x: i32, period_y: i32, salt: u64) -> f32 {
    let px = period_x.max(1);
    let py = period_y.max(1);
    let x0 = x.div_euclid(px);
    let y0 = y.div_euclid(py);
    let fx = x.rem_euclid(px) as f32 / px as f32;
    let fy = y.rem_euclid(py) as f32 / py as f32;
    let corner = |ix: i32, iy: i32| hash_f32_2(seed, ix as i64, iy as i64, salt);
    let ease = |t: f32| t * t * (3.0 - 2.0 * t);
    let u = ease(fx);
    let v = ease(fy);
    let top = corner(x0, y0) + (corner(x0 + 1, y0) - corner(x0, y0)) * u;
    let bot = corner(x0, y0 + 1) + (corner(x0 + 1, y0 + 1) - corner(x0, y0 + 1)) * u;
    (top + (bot - top) * v).clamp(0.0, 1.0)
}

/// Stored position inside a material's hydrology ranges. Uses salts
/// independent from material-choice lenses so one limestone body can
/// contain a permeable core and tighter margins. Depth adds mild
/// compaction without erasing the coherent pattern.
/// Fold a 0..1 field so its maximum lies along a **locus** rather than at its
/// extremes: `1 - |2n - 1|`.
///
/// This is what turns blobs into veins. Ordinary noise peaks in patches, and a
/// patch of permeable rock is a lens, not a conduit — water spreads through it
/// and equalises. Ridged noise peaks where the underlying field crosses its
/// midpoint, and a level set of a continuous field is a *curve*: long, thin and
/// connected. That is the shape a channel needs before flow can find and deepen
/// it.
fn ridged(n: f32) -> f32 {
    1.0 - (2.0 * n - 1.0).abs()
}

fn pore_coordinate(seed: u64, x: i32, y: i32, surface_y: i32) -> u8 {
    let broad = lens_noise(seed, x, y, 32, 14, 0xA0_2E_1001);
    let fine = lens_noise(seed, x, y, 11, 6, 0xA0_2E_1002);
    // Veins: narrow, connected, and following their own contour rather than
    // sitting in a blob. Anisotropic on purpose — stretched along x by sampling
    // y at a shorter wavelength — because bedded rock fractures along its
    // bedding, so a conduit should run with the strata rather than across them.
    let vein = ridged(lens_noise(seed, x, y * 3, 24, 9, 0xA0_2E_1003)).powi(3);
    let depth = (surface_y - y).max(0) as f32;
    let compaction = (depth / 180.0).min(0.20);
    // Left coherent and roughly centred on purpose. The fracture weighting
    // lives in `HydroRange::sample_fracture`, which treats the lower half of
    // this domain as matrix and ramps only the upper half — so the *field*
    // stays a readable lens pattern (and porosity stays centred on it) while
    // permeability still ends up tight almost everywhere.
    // Veins *add* to the lens field rather than replacing it, so the readable
    // lens pattern survives and conduits sit on top of it as the fast paths.
    // Cubed above, so only the ridge crest reaches high pore and the flanks stay
    // matrix — a wide soft ridge would be another lens.
    let base = 0.72 * broad + 0.28 * fine - compaction;
    (((base + 0.55 * vein).clamp(0.0, 1.0) * 255.0).round()) as u8
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
            let mut cell = if y < bedrock_top {
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
            if cell.material.is_solid() {
                cell.pore = pore_coordinate(p.seed, x, y, surface_y);
                // Start with a groundwater table already in place.
                //
                // Below the table everything is saturated to capacity — that is
                // what a water table *is* — and soaking there from dry took a
                // whole night of simulation to reach a state we know the answer
                // to. The interesting behaviour is the vadose zone *above* it:
                // drainage capacity growing as apertures open, and conduits
                // forming over long stretches of time. Starting dry spent the
                // soak on the boring half.
                //
                // The table sits at sea level, which is where it belongs on a
                // coastal world: hydrostatically continuous with the ocean, so it
                // is a boundary condition rather than an arbitrary fill.
                if y <= p.sea_level_y {
                    cell.sat = crate::cell::Sat(crate::cell::water_capacity_cell(
                        cell,
                        &world.hydro,
                    ));
                }
            }
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
        use crate::cell::water_capacity_cell;
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

        // The bed may have taken rock debris in those 240 ticks: bodies now
        // hold a terminal velocity and are re-dirtied while in flight, so a
        // hanging boulder finishes its fall into the sea and shatters. What
        // matters here is that the lake bed is porous and saturates.
        let sand = w.get_cell(x, surf).unwrap();
        let bed_cap = water_capacity_cell(sand, &w.hydro);
        assert!(
            bed_cap > 0,
            "lake bed must stay porous, got {:?}",
            sand.material
        );
        assert_eq!(
            sand.sat.0, bed_cap,
            "lake-bed {:?} must saturate (sat={})",
            sand.material, sand.sat.0
        );
        // Body cell just under the sand cap.
        let under = w.get_cell(x, surf - p.sand_cap_thickness).unwrap();
        assert!(
            under.material != MaterialId::Bedrock,
            "expected porous body under sand, got {:?}",
            under.material
        );
        let cap = water_capacity_cell(under, &w.hydro);
        assert!(
            cap > 0,
            "under-sand material should be porous: {:?}",
            under.material
        );
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
            enable_competent_fall: false,
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
