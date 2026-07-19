//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Sparse scalar overlays keyed by chunk coordinate.
//!
//! Overlaid heatmaps (temperature, humidity, wind, chemical
//! concentrations, "underground flow") live on the same chunk grid as
//! the cells but at their own resolution (typically coarser). Each
//! layer is a generic `Heatmap<T>` — usually `f32`, occasionally `Vec2`
//! for vector-valued fields like wind.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};

/// One heatmap patch attached to a single chunk. `cells_per_side`
/// determines the sample resolution relative to the underlying cell
/// grid: `cells_per_side = 1` is one heatmap sample per cell,
/// `cells_per_side = 4` groups a 4×4 cell tile per sample.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatmapPatch<T> {
    pub coord: ChunkCoord,
    pub cells_per_side: u8,
    pub width: u16,
    pub height: u16,
    pub data: Vec<T>,
}

impl<T: Clone + Default> HeatmapPatch<T> {
    pub fn new(coord: ChunkCoord, cells_per_side: u8) -> Self {
        let cps = cells_per_side.max(1) as usize;
        let width = (CHUNK_CELLS_W / cps).max(1) as u16;
        let height = (CHUNK_CELLS_H / cps).max(1) as u16;
        Self {
            coord,
            cells_per_side,
            width,
            height,
            data: vec![T::default(); (width as usize) * (height as usize)],
        }
    }

    pub fn sample(&self, sx: usize, sy: usize) -> &T {
        &self.data[sy * self.width as usize + sx]
    }

    pub fn set(&mut self, sx: usize, sy: usize, value: T) {
        self.data[sy * self.width as usize + sx] = value;
    }
}

/// Sparse per-chunk heatmap: `chunk_coord → patch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heatmap<T> {
    /// Human-readable label ("temperature", "moisture", ...).
    /// Not required by physics; useful for debug overlays and tests.
    pub name: String,
    pub cells_per_side: u8,
    pub patches: HashMap<ChunkCoord, HeatmapPatch<T>>,
}

impl<T: Clone + Default> Heatmap<T> {
    pub fn new(name: impl Into<String>, cells_per_side: u8) -> Self {
        Self {
            name: name.into(),
            cells_per_side: cells_per_side.max(1),
            patches: HashMap::new(),
        }
    }

    pub fn ensure_patch(&mut self, coord: ChunkCoord) -> &mut HeatmapPatch<T> {
        let cps = self.cells_per_side;
        self.patches
            .entry(coord)
            .or_insert_with(|| HeatmapPatch::new(coord, cps))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_dimensions_match_resolution() {
        let coord = ChunkCoord::new(0, 0);
        let p = HeatmapPatch::<f32>::new(coord, 4);
        assert_eq!(p.width, (CHUNK_CELLS_W as u16) / 4);
        assert_eq!(p.height, (CHUNK_CELLS_H as u16) / 4);
        assert_eq!(p.data.len(), (p.width * p.height) as usize);
    }

    #[test]
    fn heatmap_lazy_patch_creation() {
        let mut h = Heatmap::<f32>::new("moisture", 2);
        let coord = ChunkCoord::new(3, -1);
        h.ensure_patch(coord).set(0, 0, 0.5);
        assert_eq!(h.patches.len(), 1);
        assert_eq!(*h.patches.get(&coord).unwrap().sample(0, 0), 0.5);
    }
}
