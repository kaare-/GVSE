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
    let width_m = wk_material::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
    cells_for_extent(width_m, cell_m)
}

/// Vertical cells from `bedrock_y - FIELD_BELOW_BEDROCK_M` up to
/// `sea_level + FIELD_ABOVE_SEA_M`.
pub fn vertical_cells(bedrock_y: f32, sea_level: f32, cell_m: f32) -> (u16, f32) {
    let origin_y = bedrock_y - FIELD_BELOW_BEDROCK_M;
    let top_y = sea_level + FIELD_ABOVE_SEA_M;
    let extent = (top_y - origin_y).max(cell_m);
    (cells_for_extent(extent, cell_m), origin_y)
}

impl ThermalField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32, fill_c: f32) -> Self {
        let cell = THERMAL_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * wk_material::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self(FieldPatch::new(w, h, cell, origin_x, origin_y, fill_c))
    }
}

impl HumidityField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32, fill: f32) -> Self {
        let cell = HUMIDITY_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * wk_material::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self(FieldPatch::new(w, h, cell, origin_x, origin_y, fill))
    }
}

impl PressureField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32, fill: f32) -> Self {
        let cell = PRESSURE_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * wk_material::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self(FieldPatch::new(w, h, cell, origin_x, origin_y, fill))
    }
}

impl WindField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32) -> Self {
        let cell = WIND_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * wk_material::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
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
        let origin_x = coord as f32 * wk_material::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self(FieldPatch::new(w, h, cell, origin_x, origin_y, fill_m))
    }
}

impl DissolvedField {
    pub fn new_for_chunk(coord: i32, bedrock_y: f32, sea_level: f32) -> Self {
        let cell = DISSOLVED_CELL_M;
        let w = chunk_width_cells(cell);
        let (h, origin_y) = vertical_cells(bedrock_y, sea_level, cell);
        let origin_x = coord as f32 * wk_material::CHUNK_W as f32 * wk_material::SAMPLE_WIDTH_M;
        Self(FieldPatch::new(w, h, cell, origin_x, origin_y, 0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::{CHUNK_W, SAMPLE_WIDTH_M};

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
    fn wind_components_share_geometry() {
        let w = WindField::new_for_chunk(3, -45.0, 12.0);
        assert_eq!(w.vx.width_cells, w.vy.width_cells);
        assert_eq!(w.vx.height_cells, w.vy.height_cells);
        assert!((w.vx.origin_x_m - w.vy.origin_x_m).abs() < 1e-5);
    }
}
