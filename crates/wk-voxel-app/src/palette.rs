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
    // **Only** conglomerate, and on identity rather than any rate: *clasts in a
    // matrix* is what the material is, and without the marks it reads as plain
    // grey and is indistinguishable from stone.
    //
    // Permeability used to be stippled here and no longer is. Speckling sand said
    // nothing — sand is visibly permeable already — and the marks competed with
    // the thing that actually needed distinguishing: rock from rock. That job now
    // belongs to [`permeability_tint`], which uses hue, leaving brightness free
    // for water content.
    let _ = hydro;
    cell.material == MaterialId::Conglomerate
}

/// Hue pull toward ochre for conductive rock, quantized to keep runs mergeable.
///
/// Rock is otherwise grey on grey, so a fractured vein is invisible next to its
/// matrix even though water treats them completely differently. Hue is the right
/// axis because **wetness already owns brightness** — darkening for water content
/// and tinting for permeability can then be read independently instead of
/// fighting.
///
/// Square-rooted on purpose. Permeability spans 5 (tight stone) to 255
/// (fractured limestone), and a linear map spends almost all its range on
/// carbonate while leaving stone's 5..40 — the band the pore field actually
/// varies — indistinguishable. The curve gives the tight end real resolution.
fn permeability_tint(cell: Cell, hydro: &wk_material::HydroOverrides) -> f32 {
    if !wk_voxel::cell::is_competent_rock(cell.material) {
        return 0.0;
    }
    let p = wk_voxel::permeability_cell(cell, hydro) as f32 / 255.0;
    let t = p.sqrt().clamp(0.0, 1.0);
    // Quantized like the wetness bucket, or every cell becomes its own draw call.
    let step = (t * (TINT_LEVELS - 1) as f32).round();
    step / (TINT_LEVELS - 1) as f32
}

/// Ochre: warm and desaturated, so it reads as a property of the rock rather than
/// as a highlight, and stays distinguishable once the cell darkens with water.
const PERMEABLE_ROCK_RGB: [u8; 3] = [0xD8, 0xC0, 0x60];

/// How far the ochre pull may go. Deliberately partial: this is a readable hint
/// about the rock, not a heatmap replacing its material colour.
const PERM_TINT_STRENGTH: f32 = 0.55;

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
        // Conductive rock pulls toward ochre. After darkening so a wet permeable
        // rock still reads as permeable, and hue-only so the two channels do not
        // compete.
        let perm = permeability_tint(cell, hydro);
        if perm > 0.0 {
            rgb = [
                lerp_u8(rgb[0], PERMEABLE_ROCK_RGB[0], perm * PERM_TINT_STRENGTH),
                lerp_u8(rgb[1], PERMEABLE_ROCK_RGB[1], perm * PERM_TINT_STRENGTH),
                lerp_u8(rgb[2], PERMEABLE_ROCK_RGB[2], perm * PERM_TINT_STRENGTH),
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
    fn only_conglomerate_is_stippled_now() {
        // Speckling every permeable material said nothing — sand is visibly
        // permeable already — and competed with the thing that needed
        // distinguishing: rock from rock. Permeability moved to hue.
        let h = wk_material::HydroOverrides::default();
        let mut congl = Cell::solid(MaterialId::Conglomerate);
        congl.pore = 0;
        assert!(
            shows_pore_stipple(congl, &h),
            "conglomerate is clasts in a matrix; that is what the marks say"
        );
        for m in [
            MaterialId::Sand,
            MaterialId::Gravel,
            MaterialId::Limestone,
            MaterialId::Stone,
        ] {
            let mut c = Cell::solid(m);
            c.pore = u8::MAX;
            assert!(
                !shows_pore_stipple(c, &h),
                "{m:?} should carry permeability in its tint, not in speckles"
            );
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
    fn the_permeability_tint_leaves_brightness_to_water() {
        // The two channels must stay readable together: hue for permeability,
        // brightness for water. If the tint also brightened, a dry tight rock and
        // a wet permeable one would converge.
        let h = wk_material::HydroOverrides::default();
        let mut dry = Cell::solid(MaterialId::Limestone);
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
        let mut seen = std::collections::HashSet::new();
        for pore in 0..=255u8 {
            let mut c = Cell::solid(MaterialId::Stone);
            c.pore = pore;
            seen.insert(cell_color_with(c, &h, 0.0));
        }
        assert!(
            seen.len() <= TINT_LEVELS as usize,
            "256 pore values must collapse to at most {TINT_LEVELS} colours, got {}",
            seen.len()
        );
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
