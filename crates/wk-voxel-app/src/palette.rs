//! wk-voxel-app palette adapter.
//!
//! Isolation: this file depends only on `wk_material` and `wk_voxel`.
//! No imports from wk-world / wk-sim / wk-app.
//!
//! Maps a [`wk_voxel::Cell`] to an RGB triple using
//! [`wk_material::MaterialRegistry::colour_rgb`] as the ground truth.
//! Water is `Air + sat` in the voxel model. Dry Air keeps the sky colour;
//! meaningful sat blends from a soft film toward the `Water` palette.
//! Sub-haze films (≤ [`wk_voxel::GRAIN_REPOSE_HAZE_MAX`]) are not drawn
//! by the app — they stay atmospheric, not a bright ground outline.

use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::{Cell, GRAIN_REPOSE_HAZE_MAX};

/// Soft film colour once sat clears the atmospheric haze band.
const WATER_FILM_RGB: [u8; 3] = [0x9A, 0xC0, 0xD8];

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// Cream hypha tint blended onto colonized hosts (palette Hypha).
const MYCELIUM_THREAD_RGB: [u8; 3] = [0xF1, 0xE6, 0xC4];

/// Blend weight toward cream for a mycelium intensity on a given host.
///
/// Mineral corridors often sit at modest intensity (spread / compost
/// residual ~8–40). A pure `intensity/255` ramp made Sand/Soil look
/// unchanged — floor the blend so infected non-Organic cells read.
fn mycelium_blend(material: MaterialId, intensity: u8) -> f32 {
    if intensity == 0 {
        return 0.0;
    }
    let t = intensity as f32 / 255.0;
    if material == MaterialId::Organic {
        // Soft threads on litter — up to ~45% cream at full.
        (0.08 + t * 0.37).min(0.45)
    } else {
        // Mineral corridors: clearer chalky wash so tunnels show.
        (0.14 + t * 0.42).min(0.55)
    }
}

/// Wetness / porosity are quantized into this many levels.
///
/// Terrain draws as merged vertical runs keyed on colour, so a continuous tint
/// would break almost every run and give back the batching win. Buckets keep
/// neighbouring cells with similar values mergeable while still showing the
/// structure.
pub const TINT_LEVELS: u8 = 4;

/// How dark a fully waterlogged cell goes (fraction toward the wet tone).
pub const WET_DARKEN_DEFAULT: f32 = 0.45;

/// Wetness bucket `0..TINT_LEVELS-1` from **pore saturation over capacity**.
///
/// Capacity, not 255. Stone holds ~20 sat when completely full, so a
/// `sat / 255` ramp darkened saturated stone by under 3% — groundwater was
/// effectively invisible in the terrain colour, which is exactly the structure
/// the pore work needed to be able to see.
pub fn wetness_bucket(cell: Cell, hydro: &wk_material::HydroOverrides) -> u8 {
    let cap = wk_voxel::water_capacity_cell(cell, hydro);
    if cap == 0 || cell.sat.0 == 0 {
        return 0;
    }
    let frac = (cell.sat.0 as f32 / cap as f32).clamp(0.0, 1.0);
    ((frac * TINT_LEVELS as f32).floor() as u8).min(TINT_LEVELS - 1)
}

/// Aperture bucket `0..TINT_LEVELS-1` from the stored pore coordinate.
///
/// Only the upper buckets are stippled by the app. The fracture tail makes high
/// values genuinely rare, so marking just those is both cheap and points the eye
/// at the conduits rather than at ordinary rock.
pub fn pore_bucket(cell: Cell) -> u8 {
    if cell.material == MaterialId::Air {
        return 0;
    }
    ((cell.pore as u16 * TINT_LEVELS as u16) / 256) as u8
}

/// Pore coordinate at which a cell starts being drawn as porous.
///
/// Must sit clearly **above** the matrix boundary. `HydroRange::sample_fracture`
/// treats everything up to 128 as the material's matrix value, so marking at or
/// near 128 would stipple ordinary rock — including every `Cell::solid()`, which
/// defaults to exactly 128.
pub const PORE_STIPPLE_MIN: u8 = 176;

/// Permeability at or above which a cell is marked as a water path.
///
/// Absolute, unlike `pore`. Gravel sits at 160 and sand at 96, while even heavily
/// fractured stone only reaches ~40 (matrix 5, fracture tail ~8x).
pub const PATH_STIPPLE_MIN: u8 = 64;

/// True when water can actually move through this cell fast enough to matter.
///
/// Keyed on **permeability**, not on raw `pore`. Those are different questions and
/// stippling the wrong one was actively misleading: `pore` is *relative* to a
/// material — "how fractured is this cell for the rock it is" — while water follows
/// the absolute rate. A fractured stone vein (perm ~40) drew speckles while a
/// plain gravel band (perm 160) drew none, so water appeared to avoid the marked
/// rock and prefer the gaps. It was the overlay that had it backwards.
pub fn shows_pore_stipple(cell: Cell, hydro: &wk_material::HydroOverrides) -> bool {
    if matches!(
        cell.material,
        MaterialId::Air | MaterialId::Water | MaterialId::Ice | MaterialId::Snow
    ) {
        return false;
    }
    // Nothing is stippled any more.
    //
    // Dots came to mean two different things — porosity on some materials and
    // "this is conglomerate" on another — which broke the visual language. Both
    // jobs moved to colour: permeability to a hue pull on stone, and conglomerate
    // to its own red-leaning grey. Kept as a function because the render path asks
    // the question, and a future overlay may want to answer it again.
    let _ = (cell, hydro);
    false
}

/// Hue pull for conductive rock, quantized to keep runs mergeable.
///
/// Rock is otherwise grey on grey, so a fractured vein is invisible next to its
/// matrix even though water treats them completely differently. Hue is the right
/// axis because **wetness already owns brightness** — darkening for water content
/// and tinting for permeability can then be read independently instead of
/// fighting.
///
/// Each material is normalised against **its own** permeability range, not the
/// 0..255 byte. Stone spans roughly 5..40; a global scale would confine it to
/// the bottom sixth of the ramp. Limestone's fracture tail (140..255) gets the
/// same treatment, then a different hue so the two rocks do not share a signal.
fn permeability_tint(cell: Cell, hydro: &wk_material::HydroOverrides) -> f32 {
    // Stone, conglomerate, and limestone. Stone is most of the map; conglomerate
    // lost its speckle and needs a pore-true signal; limestone is the karst
    // rock, so open beds should read as open. Flowstone / sandstone stay put —
    // their material colour already says enough, and pulling them too drowned
    // the one signal that mattered.
    if !matches!(
        cell.material,
        MaterialId::Stone | MaterialId::Conglomerate | MaterialId::Limestone
    ) {
        return 0.0;
    }
    let range = MaterialRegistry::hydrology_with(cell.material, hydro).permeability;
    let (lo, hi) = (range.min as f32, range.max as f32);
    let span = (hi - lo).max(1.0);
    let p = wk_voxel::permeability_cell(cell, hydro) as f32;
    let t = ((p - lo) / span).clamp(0.0, 1.0);
    // Quantized like the wetness bucket, or every cell becomes its own draw call.
    let step = (t * (TINT_LEVELS - 1) as f32).round();
    step / (TINT_LEVELS - 1) as f32
}

/// Muted orange: warm enough to register, desaturated enough to read as a property
/// of the rock rather than a highlight, and still distinguishable once the cell
/// darkens with water. Pulled toward orange and away from chroma from an earlier
/// ochre, which was louder than the signal warranted.
const PERMEABLE_ROCK_RGB: [u8; 3] = [0xC0, 0x9A, 0x76];

/// Sage on limestone. Same job as the ochre pull — high perm / porosity in the
/// fracture tail — but a different hue so open carbonate does not look like
/// open granite. Kept close to the warm pale matrix so it stays a hint.
const PERMEABLE_LIMESTONE_RGB: [u8; 3] = [0xB0, 0xC4, 0xA4];

/// How far the ochre pull may go. Deliberately partial: this is a readable hint
/// about the rock, not a heatmap replacing its material colour.
const PERM_TINT_STRENGTH: f32 = 0.55;

/// Limestone's green is quieter than stone's ochre. The bed is already pale;
/// a full-strength wash would read as moss, not as open rock.
const PERM_LIME_TINT_STRENGTH: f32 = 0.40;

pub fn cell_color(cell: Cell) -> [u8; 3] {
    cell_color_with(cell, &wk_material::HydroOverrides::default(), WET_DARKEN_DEFAULT)
}

/// [`cell_color`] with the world's hydrology and a wetness-darkening budget.
pub fn cell_color_with(
    cell: Cell,
    hydro: &wk_material::HydroOverrides,
    wet_darken: f32,
) -> [u8; 3] {
    let base = MaterialRegistry::colour_rgb(cell.material);
    if cell.material == MaterialId::Air {
        if cell.sat.is_empty() || cell.sat.0 <= GRAIN_REPOSE_HAZE_MAX {
            // Dry or atmospheric film — sky (app skips drawing these).
            return base;
        }
        let water = MaterialRegistry::colour_rgb(MaterialId::Water);
        // Remap sat above haze band onto 0..1 for the film→lake ramp.
        let t_vis = ((cell.sat.0 - GRAIN_REPOSE_HAZE_MAX) as f32
            / (255 - GRAIN_REPOSE_HAZE_MAX) as f32)
            .clamp(0.0, 1.0);
        let blend = if t_vis >= 0.55 {
            (0.55 + (t_vis - 0.55) * 1.8).clamp(0.75, 1.0)
        } else {
            t_vis / 0.55 * 0.75
        };
        [
            lerp_u8(WATER_FILM_RGB[0], water[0], blend),
            lerp_u8(WATER_FILM_RGB[1], water[1], blend),
            lerp_u8(WATER_FILM_RGB[2], water[2], blend),
        ]
    } else {
        // Porous solids darken as pore water rises — the convention (wet rock
        // is darker, not bluer). Quantized so merged runs survive, and measured
        // against the cell's own capacity so a fully saturated stone actually
        // reads as saturated.
        let bucket = wetness_bucket(cell, hydro);
        let darken = wet_darken.clamp(0.0, 1.0) * bucket as f32 / (TINT_LEVELS - 1) as f32;
        let mut rgb = [
            lerp_u8(base[0], 40, darken),
            lerp_u8(base[1], 55, darken),
            lerp_u8(base[2], 85, darken),
        ];
        // Conductive rock pulls on hue. After darkening so a wet permeable
        // cell still reads as permeable, and hue-only so the two channels do
        // not compete. Stone / conglomerate go ochre; limestone goes sage.
        let perm = permeability_tint(cell, hydro);
        if perm > 0.0 {
            let (target, strength) = if cell.material == MaterialId::Limestone {
                (PERMEABLE_LIMESTONE_RGB, PERM_LIME_TINT_STRENGTH)
            } else {
                (PERMEABLE_ROCK_RGB, PERM_TINT_STRENGTH)
            };
            let w = perm * strength;
            rgb = [
                lerp_u8(rgb[0], target[0], w),
                lerp_u8(rgb[1], target[1], w),
                lerp_u8(rgb[2], target[2], w),
            ];
        }
        // Organic + mineral hosts with mycelium: cream wash (minerals
        // get a stronger floor so Sand/Soil corridors are readable).
        let myc = mycelium_blend(cell.material, cell.mycelium());
        if myc > 0.0 {
            rgb = [
                lerp_u8(rgb[0], MYCELIUM_THREAD_RGB[0], myc),
                lerp_u8(rgb[1], MYCELIUM_THREAD_RGB[1], myc),
                lerp_u8(rgb[2], MYCELIUM_THREAD_RGB[2], myc),
            ];
        }
        rgb
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_voxel::{Sat, Cell};

    #[test]
    fn saturated_stone_reads_as_wet_not_dry() {
        // Regression: wetness was measured against 255 rather than the cell's
        // own capacity. Stone holds ~20 sat when completely full, so a fully
        // saturated stone cell darkened by under 3% and groundwater was
        // effectively invisible in the terrain colour.
        let hydro = wk_material::HydroOverrides::default();
        let dry = Cell::solid(MaterialId::Stone);
        let mut full = dry;
        full.sat = Sat(wk_voxel::water_capacity_cell(dry, &hydro));
        assert!(full.sat.0 > 0 && full.sat.0 < 64, "precondition: stone's capacity is small");

        assert_eq!(
            wetness_bucket(full, &hydro),
            TINT_LEVELS - 1,
            "a cell at capacity must read as the wettest bucket"
        );
        let dry_rgb = cell_color_with(dry, &hydro, WET_DARKEN_DEFAULT);
        let wet_rgb = cell_color_with(full, &hydro, WET_DARKEN_DEFAULT);
        let lum = |c: [u8; 3]| c[0] as i32 + c[1] as i32 + c[2] as i32;
        assert!(
            lum(dry_rgb) - lum(wet_rgb) > 60,
            "saturated stone must be visibly darker (dry {dry_rgb:?} wet {wet_rgb:?})"
        );
    }

    #[test]
    fn tints_are_quantized_so_runs_still_merge() {
        // Terrain batches on colour equality. Neighbouring cells with slightly
        // different sat must land on the same bucket, or the merged-run win goes
        // away.
        let hydro = wk_material::HydroOverrides::default();
        let cap = wk_voxel::water_capacity_cell(Cell::solid(MaterialId::Sand), &hydro);
        let at = |s: u8| {
            let mut c = Cell::solid(MaterialId::Sand);
            c.sat = Sat(s);
            cell_color_with(c, &hydro, WET_DARKEN_DEFAULT)
        };
        // Two cells a single sat unit apart, mid-bucket.
        let s = cap / 2;
        assert_eq!(at(s), at(s + 1), "a one-unit difference must not split a run");
    }

    #[test]
    fn nothing_is_stippled_any_more() {
        // Dots came to mean two different things — porosity on some materials and
        // "this is conglomerate" on another — which broke the visual language.
        // Both jobs moved to colour.
        let h = wk_material::HydroOverrides::default();
        for m in [
            MaterialId::Conglomerate,
            MaterialId::Sand,
            MaterialId::Gravel,
            MaterialId::Limestone,
            MaterialId::Stone,
        ] {
            let mut c = Cell::solid(m);
            c.pore = u8::MAX;
            assert!(!shows_pore_stipple(c, &h), "{m:?} should carry its story in colour");
        }
    }

    #[test]
    fn conductive_rock_tints_warmer_than_tight_rock() {
        // Rock is grey on grey, so a fractured vein is invisible beside its matrix
        // even though water treats them completely differently.
        let h = wk_material::HydroOverrides::default();
        let mut tight = Cell::solid(MaterialId::Stone);
        tight.pore = 0;
        let mut fractured = Cell::solid(MaterialId::Stone);
        fractured.pore = u8::MAX;
        let a = cell_color_with(tight, &h, WET_DARKEN_DEFAULT);
        let b = cell_color_with(fractured, &h, WET_DARKEN_DEFAULT);
        // Warmer means red rises faster than blue.
        let warm = |c: [u8; 3]| c[0] as i32 - c[2] as i32;
        assert!(
            warm(b) > warm(a),
            "fractured stone should read warmer than tight stone ({a:?} vs {b:?})"
        );
    }

    #[test]
    fn narrow_band_rocks_carry_no_permeability_tint() {
        // Stone / conglomerate / limestone carry the pore story in hue.
        // Flowstone and sandstone arrive as deposits whose own colour says
        // enough, and tinting them drowned out the bed signal.
        let h = wk_material::HydroOverrides::default();
        for m in [
            MaterialId::Flowstone,
            MaterialId::Sandstone,
        ] {
            let mut tight = Cell::solid(m);
            tight.pore = 0;
            let mut open = Cell::solid(m);
            open.pore = u8::MAX;
            assert_eq!(
                cell_color_with(tight, &h, 0.0),
                cell_color_with(open, &h, 0.0),
                "{m:?} must not shift with permeability"
            );
        }
    }

    #[test]
    fn stone_spans_a_readable_permeability_range() {
        // Normalised against stone's own 5..40 range rather than the 0..255 byte:
        // a global scale would confine stone to the bottom sixth of the ramp and
        // waste the resolution exactly where it is wanted.
        let h = wk_material::HydroOverrides::default();
        let mut tight = Cell::solid(MaterialId::Stone);
        tight.pore = 0;
        let mut open = Cell::solid(MaterialId::Stone);
        open.pore = u8::MAX;
        let warm = |c: Cell| {
            let rgb = cell_color_with(c, &h, 0.0);
            rgb[0] as i32 - rgb[2] as i32
        };
        let spread = warm(open) - warm(tight);
        // Deliberately modest: playtest asked for "not a large contrast, just
        // enough to register", so this pins that the signal *exists* and is
        // monotone, not that it shouts.
        assert!(
            spread > 25,
            "stone should span a readable permeability range, got {spread}"
        );
    }

    #[test]
    fn the_permeability_tint_leaves_brightness_to_water() {
        // The two channels must stay readable together: hue for permeability,
        // brightness for water. If the tint also brightened, a dry tight rock and
        // a wet permeable one would converge.
        let h = wk_material::HydroOverrides::default();
        let mut dry = Cell::solid(MaterialId::Stone);
        dry.pore = u8::MAX;
        let mut wet = dry;
        wet.sat = wk_voxel::Sat(wk_voxel::water_capacity_cell(wet, &h));
        let d = cell_color_with(dry, &h, WET_DARKEN_DEFAULT);
        let w = cell_color_with(wet, &h, WET_DARKEN_DEFAULT);
        let lum = |c: [u8; 3]| c[0] as i32 + c[1] as i32 + c[2] as i32;
        assert!(
            lum(w) < lum(d),
            "wet rock must still be darker than dry rock at the same permeability"
        );
    }

    #[test]
    fn permeability_tints_are_quantized_so_runs_still_merge() {
        let h = wk_material::HydroOverrides::default();
        for m in [MaterialId::Stone, MaterialId::Limestone] {
            let mut seen = std::collections::HashSet::new();
            for pore in 0..=255u8 {
                let mut c = Cell::solid(m);
                c.pore = pore;
                seen.insert(cell_color_with(c, &h, 0.0));
            }
            assert!(
                seen.len() <= TINT_LEVELS as usize,
                "{m:?}: 256 pore values must collapse to at most {TINT_LEVELS} colours, got {}",
                seen.len()
            );
        }
    }

    #[test]
    fn conductive_limestone_tints_greener_than_tight_limestone() {
        // Same treatment as stone: the fracture tail shifts hue. Sage, not
        // ochre, so open carbonate does not look like open granite.
        let h = wk_material::HydroOverrides::default();
        let mut tight = Cell::solid(MaterialId::Limestone);
        tight.pore = 0;
        let mut open = Cell::solid(MaterialId::Limestone);
        open.pore = u8::MAX;
        let a = cell_color_with(tight, &h, 0.0);
        let b = cell_color_with(open, &h, 0.0);
        let green = |c: [u8; 3]| c[1] as i32 - c[0] as i32;
        assert!(
            green(b) > green(a),
            "open limestone should read greener than tight ({a:?} vs {b:?})"
        );
        let warm = |c: [u8; 3]| c[0] as i32 - c[2] as i32;
        assert!(
            warm(b) <= warm(a),
            "limestone must not pick up stone's ochre ({a:?} vs {b:?})"
        );
        assert_ne!(a, b, "the limestone span must be visible");
    }

    #[test]
    fn dry_air_stays_sky_blue() {
        let c = Cell::air();
        let rgb = cell_color(c);
        let sky = MaterialRegistry::colour_rgb(MaterialId::Air);
        assert_eq!(rgb, sky);
    }

    #[test]
    fn full_saturated_air_matches_water() {
        let c = Cell::water();
        let rgb = cell_color(c);
        let water = MaterialRegistry::colour_rgb(MaterialId::Water);
        assert_eq!(rgb, water);
    }

    #[test]
    fn half_saturated_air_is_between_film_and_water() {
        let mut c = Cell::air();
        c.sat = Sat(128);
        let rgb = cell_color(c);
        let water = MaterialRegistry::colour_rgb(MaterialId::Water);
        for i in 0..3 {
            let lo = WATER_FILM_RGB[i].min(water[i]);
            let hi = WATER_FILM_RGB[i].max(water[i]);
            assert!(rgb[i] >= lo && rgb[i] <= hi, "component {i}: {}", rgb[i]);
        }
    }

    #[test]
    fn haze_band_sat_reads_as_sky_not_film() {
        let mut c = Cell::air();
        c.sat = Sat(1);
        let rgb = cell_color(c);
        let sky = MaterialRegistry::colour_rgb(MaterialId::Air);
        assert_eq!(rgb, sky, "1/255 wet-air film must not paint as water");
        c.sat = Sat(GRAIN_REPOSE_HAZE_MAX);
        assert_eq!(
            cell_color(c),
            sky,
            "haze-band sat must stay sky-coloured"
        );
    }

    #[test]
    fn just_above_haze_band_is_film_not_sky() {
        let mut c = Cell::air();
        c.sat = Sat(GRAIN_REPOSE_HAZE_MAX.saturating_add(1));
        let rgb = cell_color(c);
        let sky = MaterialRegistry::colour_rgb(MaterialId::Air);
        assert_ne!(rgb, sky, "puddle-threshold sat should leave the sky colour");
    }

    #[test]
    fn wet_sand_darker_than_dry_sand() {
        let dry = Cell::solid(MaterialId::Sand);
        let mut wet = Cell::solid(MaterialId::Sand);
        wet.sat = Sat::FULL;
        let dry_rgb = cell_color(dry);
        let wet_rgb = cell_color(wet);
        let dry_lum = dry_rgb[0] as i32 + dry_rgb[1] as i32 + dry_rgb[2] as i32;
        let wet_lum = wet_rgb[0] as i32 + wet_rgb[1] as i32 + wet_rgb[2] as i32;
        assert!(wet_lum < dry_lum, "wet sand must be darker");
    }

    #[test]
    fn mineral_mycelium_shifts_sand_toward_cream() {
        let bare = Cell::solid(MaterialId::Sand);
        let mut infected = Cell::solid(MaterialId::Sand);
        infected.set_mycelium(24); // typical thin corridor / residual
        let bare_rgb = cell_color(bare);
        let myc_rgb = cell_color(infected);
        assert_ne!(
            bare_rgb, myc_rgb,
            "infected Sand must visually differ from bare Sand"
        );
        // Cream is warmer/lighter — green+blue channels move toward Hypha.
        let toward_cream = (myc_rgb[1] as i16 - bare_rgb[1] as i16).abs()
            + (myc_rgb[2] as i16 - bare_rgb[2] as i16).abs()
            + (myc_rgb[0] as i16 - bare_rgb[0] as i16).abs();
        assert!(
            toward_cream >= 12,
            "mycelium wash on Sand should be obvious (delta={toward_cream})"
        );
    }
}
