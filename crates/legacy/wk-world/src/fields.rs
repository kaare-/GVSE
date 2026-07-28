//! Typed wrappers around [`wk_field::FieldPatch`] for each physical field.
//!
//! Newtypes keep temperature / humidity / pressure / … distinct at
//! compile time. All fields are optional on [`crate::chunk::Chunk`] so
//! they can be rolled out one at a time; `None` means "field disabled,
//! fall back to the pre-field behaviour."

use serde::{Deserialize, Serialize};
use wk_field::FieldPatch;

/// Suggested default cell sizes (metres). Chosen once; do not change
/// between saves of the same schema without a migration.
pub const THERMAL_CELL_M: f32 = 0.5;
pub const HUMIDITY_CELL_M: f32 = 2.0;
pub const PRESSURE_CELL_M: f32 = 2.0;
pub const WIND_CELL_M: f32 = 2.0;
pub const GROUNDWATER_HEAD_CELL_M: f32 = 1.0;
pub const DISSOLVED_CELL_M: f32 = 0.5;

/// Vertical extent covered by fields: from a few metres below bedrock
/// floor up through the terrain into open air.
pub const FIELD_BELOW_BEDROCK_M: f32 = 5.0;
pub const FIELD_ABOVE_SEA_M: f32 = 30.0;
/// Cap how far below sea level field grids extend. Kilometre-deep
/// bedrock floors (dramatic relief) must not allocate ~2000-cell-tall
/// thermal/humidity patches per chunk — that made the live app crawl.
pub const FIELD_MAX_DEPTH_BELOW_SEA_M: f32 = 100.0;

/// Warm mixed layer thickness below the free surface / sea level (m).
pub const MIXED_LAYER_M: f32 = 8.0;
/// Depth span of the thermocline beneath the mixed layer (m).
pub const THERMOCLINE_M: f32 = 28.0;
/// How much cooler (°C) deep water sits vs the warm surface skin.
pub const DEEP_WATER_COOLING_C: f32 = 9.0;
/// Peak solar heating (°C) added to the mixed layer at solar noon.
pub const MIXED_LAYER_SOLAR_C: f32 = 3.5;

/// Stratified water-column temperature profile for seeding / restoring.
///
/// - Air (`y ≥ sea`): `sky`
/// - Mixed layer: warm skin (`sky + solar_heat_c`)
/// - Thermocline: cools toward `sky + solar − DEEP_WATER_COOLING`
/// - Below: blends toward geothermal at the field origin
pub fn stratified_water_temp(
    y_m: f32,
    sea_level: f32,
    origin_y_m: f32,
    sky_c: f32,
    geothermal_c: f32,
    solar_heat_c: f32,
) -> f32 {
    if y_m >= sea_level {
        return sky_c;
    }
    let surface = sky_c + solar_heat_c.max(0.0);
    let deep_water = surface - DEEP_WATER_COOLING_C;
    let depth = sea_level - y_m;
    if depth <= MIXED_LAYER_M {
        return surface;
    }
    if depth <= MIXED_LAYER_M + THERMOCLINE_M {
        let u = ((depth - MIXED_LAYER_M) / THERMOCLINE_M).clamp(0.0, 1.0);
        return surface + (deep_water - surface) * u;
    }
    let y_deep = sea_level - MIXED_LAYER_M - THERMOCLINE_M;
    let span = (y_deep - origin_y_m).max(1e-3);
    geothermal_c + (deep_water - geothermal_c) * ((y_m - origin_y_m) / span).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThermalField(pub FieldPatch);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HumidityField(pub FieldPatch);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PressureField(pub FieldPatch);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindField {
    pub vx: FieldPatch,
    pub vy: FieldPatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroundwaterHeadField(pub FieldPatch);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DissolvedField(pub FieldPatch);

/// Geometry helper: how many cells of size `cell_m` span `extent_m`.
pub fn cells_for_extent(extent_m: f32, cell_m: f32) -> u16 {
    ((extent_m / cell_m).ceil() as u16).max(1)
}

/// Horizontal cells covering one chunk width (`CHUNK_W * SAMPLE_WIDTH_M`).
pub fn chunk_width_cells(cell_m: f32) -> u16 {
    let width_m = crate::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
    cells_for_extent(width_m, cell_m)
}

/// Vertical cells from a capped floor up to `sea_level + FIELD_ABOVE_SEA_M`.
///
/// Floor is `max(bedrock − slack, sea − FIELD_MAX_DEPTH_BELOW_SEA)`. Deep
/// abyssal bedrock still gets a geothermal Dirichlet at the patch bottom;
/// we just don't simulate the whole mantle column at 0.5 m resolution.
pub fn vertical_cells(bedrock_y: f32, sea_level: f32, cell_m: f32) -> (u16, f32) {
    let rock_floor = bedrock_y - FIELD_BELOW_BEDROCK_M;
    let cap_floor = sea_level - FIELD_MAX_DEPTH_BELOW_SEA_M;
    let origin_y = rock_floor.max(cap_floor);
    let top_y = sea_level + FIELD_ABOVE_SEA_M;
    let extent = (top_y - origin_y).max(cell_m);
    (cells_for_extent(extent, cell_m), origin_y)
}

impl ThermalField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32, fill_c: f32) -> Self {
        let cell = THERMAL_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * crate::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self(FieldPatch::new(w, h, cell, origin_x, origin_y, fill_c))
    }
}

impl HumidityField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32, fill: f32) -> Self {
        let cell = HUMIDITY_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * crate::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self(FieldPatch::new(w, h, cell, origin_x, origin_y, fill))
    }
}

impl PressureField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32, fill: f32) -> Self {
        let cell = PRESSURE_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * crate::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self(FieldPatch::new(w, h, cell, origin_x, origin_y, fill))
    }
}

impl WindField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32) -> Self {
        let cell = WIND_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * crate::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self {
            vx: FieldPatch::new(w, h, cell, origin_x, origin_y, 0.0),
            vy: FieldPatch::new(w, h, cell, origin_x, origin_y, 0.0),
        }
    }
}

impl GroundwaterHeadField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32, fill_m: f32) -> Self {
        let cell = GROUNDWATER_HEAD_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * crate::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self(FieldPatch::new(w, h, cell, origin_x, origin_y, fill_m))
    }
}

impl DissolvedField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32) -> Self {
        let cell = DISSOLVED_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * crate::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self(FieldPatch::new(w, h, cell, origin_x, origin_y, 0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::{SAMPLE_WIDTH_M};
use crate::{CHUNK_W};

    #[test]
    fn thermal_geometry_covers_chunk_width() {
        let f = ThermalField::new_for_chunk(0, -45.0, 12.0, 15.0);
        let expected_w = (CHUNK_W as f32 * SAMPLE_WIDTH_M / THERMAL_CELL_M).ceil() as u16;
        assert_eq!(f.0.width_cells, expected_w);
        assert!(f.0.height_cells > 1);
        assert!((f.0.origin_x_m - 0.0).abs() < 1e-5);
        assert!((f.0.origin_y_m - (-45.0 - FIELD_BELOW_BEDROCK_M)).abs() < 1e-5);
    }

    #[test]
    fn deep_bedrock_does_not_inflate_field_height() {
        let (h_shallow, origin_s) = vertical_cells(-45.0, 12.0, THERMAL_CELL_M);
        let (h_deep, origin_d) = vertical_cells(-900.0, 12.0, THERMAL_CELL_M);
        let uncapped_h = {
            let origin = -900.0 - FIELD_BELOW_BEDROCK_M;
            let extent = (12.0 + FIELD_ABOVE_SEA_M) - origin;
            cells_for_extent(extent, THERMAL_CELL_M)
        };
        assert!(
            origin_d > -200.0,
            "deep bedrock should clamp field floor (origin={origin_d})"
        );
        assert!(
            h_deep < uncapped_h / 2,
            "cap must cut deep grids (capped={h_deep} uncapped={uncapped_h})"
        );
        assert!(h_deep < 400, "capped thermal height, got {h_deep}");
        // Shallow bedrock still uses the rock floor (above the sea-depth cap).
        assert!((origin_s - (-45.0 - FIELD_BELOW_BEDROCK_M)).abs() < 1e-3);
        assert!(h_shallow < h_deep || h_shallow + 5 >= h_deep);
    }

    #[test]
    fn stratified_profile_has_warm_skin_and_cool_deep() {
        let sea = 12.0;
        let origin = -200.0;
        let sky = 22.0;
        let geo = 55.0;
        let surface = stratified_water_temp(sea - 1.0, sea, origin, sky, geo, 3.0);
        let mid = stratified_water_temp(sea - 20.0, sea, origin, sky, geo, 3.0);
        let deep = stratified_water_temp(sea - 60.0, sea, origin, sky, geo, 3.0);
        assert!(surface > sky, "mixed layer should be at least sky (+ solar)");
        assert!(
            mid < surface - 2.0,
            "thermocline should cool vs skin (skin={surface:.1} mid={mid:.1})"
        );
        assert!(
            deep < mid + 1.0,
            "deep water should stay cool vs mid (mid={mid:.1} deep={deep:.1})"
        );
        let air = stratified_water_temp(sea + 5.0, sea, origin, sky, geo, 3.0);
        assert!((air - sky).abs() < 1e-3);
    }

    #[test]
    fn wind_components_share_geometry() {
        let w = WindField::new_for_chunk(3, -45.0, 12.0);
        assert_eq!(w.vx.width_cells, w.vy.width_cells);
        assert_eq!(w.vx.height_cells, w.vy.height_cells);
        assert!((w.vx.origin_x_m - w.vy.origin_x_m).abs() < 1e-5);
    }
}
