//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Fixed-size chunk of cells, keyed by an `(i32, i32)` chunk
//! coordinate. Chunk size follows Noita's Falling Everything engine
//! (Purho GDC 2019): 64×64 keeps each chunk small enough for a single
//! thread to own during a checkerboard sub-tick and keeps the dirty
//! rectangle cheap.

use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::Cell;

/// Chunk width in cells.
pub const CHUNK_CELLS_W: usize = 64;
/// Chunk height in cells.
pub const CHUNK_CELLS_H: usize = 64;
/// Total cells per chunk.
pub const CHUNK_CELLS: usize = CHUNK_CELLS_W * CHUNK_CELLS_H;

/// Chunk coordinate in world-chunk space. `(cx, cy)` — positive `cy`
/// is up (sky), negative `cy` is down (bedrock).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkCoord {
    pub cx: i32,
    pub cy: i32,
}

impl ChunkCoord {
    pub fn new(cx: i32, cy: i32) -> Self {
        Self { cx, cy }
    }

    /// Pack into one word so HashMaps hash a single `u64` instead of two `i32`s.
    #[inline]
    pub fn pack(self) -> u64 {
        ((self.cx as u32 as u64) << 32) | (self.cy as u32 as u64)
    }
}

impl Hash for ChunkCoord {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.pack());
    }
}

/// Inclusive axis-aligned rectangle in local chunk-cell space.
/// Follows Noita's per-chunk "dirty rectangle" trick: only cells that
/// changed since the last tick need to be visited again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x0: u8,
    pub y0: u8,
    pub x1: u8,
    pub y1: u8,
}

impl Rect {
    pub fn full() -> Self {
        Self {
            x0: 0,
            y0: 0,
            x1: (CHUNK_CELLS_W - 1) as u8,
            y1: (CHUNK_CELLS_H - 1) as u8,
        }
    }

    pub fn empty() -> Option<Self> {
        None
    }

    pub fn contains(self, x: u8, y: u8) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    pub fn expand_to_include(&mut self, x: u8, y: u8) {
        self.x0 = self.x0.min(x);
        self.y0 = self.y0.min(y);
        self.x1 = self.x1.max(x);
        self.y1 = self.y1.max(y);
    }
}

/// One `CHUNK_CELLS_W × CHUNK_CELLS_H` slab. Row-major, `y = 0` at the
/// bottom row so gravity rules can walk bottom-up cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub coord: ChunkCoord,
    /// Flat cell storage. Index with [`Chunk::idx`].
    pub cells: Vec<Cell>,
    /// Bounding box around cells touched last tick. `None` = quiescent.
    pub dirty: Option<Rect>,
    /// Local tick counter (wraps freely — used for RNG salting only).
    pub tick: u64,
    /// Sticky occupancy: at least one `Air` cell with `sat > 0` was
    /// written since the flag was last cleared. Evaporation uses this
    /// to skip empty sky chunks while still visiting quiescent lakes
    /// (which dirty-rects alone would miss).
    #[serde(default)]
    pub has_wet_air: bool,
    /// Sticky occupancy: at least one permeable solid with `sat > 0`.
    /// Lake-bed / seam wakes use this so underground pore columns keep
    /// visiting after the free surface goes quiet (`has_wet_air` alone
    /// never marks pure groundwater chunks).
    #[serde(default)]
    pub has_wet_pores: bool,
    /// Sticky occupancy: at least one `Limestone` cell was written
    /// since the flag was last cleared. Karst skips chunks that never
    /// held limestone.
    #[serde(default)]
    pub has_limestone: bool,
    /// Sticky occupancy: at least one loose / buoyant cell (sand, soil,
    /// gravel, clay, loose rock/limestone, snow, ice, organic) was
    /// written since the flag was last cleared. Grain wake / punch /
    /// litter scans skip pure water / sky / stone chunks.
    #[serde(default)]
    pub has_loose: bool,
    /// Sticky occupancy: at least one `Organic` cell was written since
    /// the flag was last cleared. Float-column / raft scans skip chunks
    /// that never held litter.
    #[serde(default)]
    pub has_organic: bool,
    /// Sticky occupancy: Organic / Snow / Ice (buoyant litter). Rise/soak
    /// scans skip sand-only loose chunks.
    #[serde(default)]
    pub has_buoyant: bool,
}

/// Materials that participate in grain settle / float / punch passes.
#[inline]
pub fn material_is_loose(material: MaterialId) -> bool {
    matches!(
        material,
        MaterialId::Sand
            | MaterialId::Gravel
            | MaterialId::Clay
            | MaterialId::Soil
            | MaterialId::LooseRock
            | MaterialId::LooseLimestone
            | MaterialId::Snow
            | MaterialId::Ice
            | MaterialId::Organic
    )
}

impl Chunk {
    pub fn new(coord: ChunkCoord) -> Self {
        Self {
            coord,
            cells: vec![Cell::default(); CHUNK_CELLS],
            dirty: None,
            tick: 0,
            has_wet_air: false,
            has_wet_pores: false,
            has_limestone: false,
            has_loose: false,
            has_organic: false,
            has_buoyant: false,
        }
    }

    pub fn idx(x: usize, y: usize) -> usize {
        debug_assert!(x < CHUNK_CELLS_W);
        debug_assert!(y < CHUNK_CELLS_H);
        y * CHUNK_CELLS_W + x
    }

    pub fn get(&self, x: usize, y: usize) -> Cell {
        self.cells[Self::idx(x, y)]
    }

    /// Write a cell and mark the chunk's dirty rectangle so the next
    /// tick knows which region to re-scan. Also raises occupancy
    /// flags (cleared only by the passes that scan for absence).
    ///
    /// No-op when the stored cell already equals `cell` — avoids
    /// inflating dirty rects on redundant writes.
    pub fn set(&mut self, x: usize, y: usize, cell: Cell) {
        let idx = Self::idx(x, y);
        if self.cells[idx] == cell {
            return;
        }
        self.cells[idx] = cell;
        let xu = x as u8;
        let yu = y as u8;
        match &mut self.dirty {
            Some(r) => r.expand_to_include(xu, yu),
            None => {
                self.dirty = Some(Rect {
                    x0: xu,
                    y0: yu,
                    x1: xu,
                    y1: yu,
                });
            }
        }
        if cell.material == MaterialId::Air && !cell.sat.is_empty() {
            self.has_wet_air = true;
        }
        if cell.material != MaterialId::Air
            && !cell.sat.is_empty()
            && wk_material::MaterialRegistry::props(cell.material).permeability > 0
        {
            self.has_wet_pores = true;
        }
        if cell.material == MaterialId::Limestone {
            self.has_limestone = true;
        }
        if material_is_loose(cell.material) {
            self.has_loose = true;
        }
        if cell.material == MaterialId::Organic {
            self.has_organic = true;
        }
        if matches!(
            cell.material,
            MaterialId::Organic | MaterialId::Snow | MaterialId::Ice
        ) {
            self.has_buoyant = true;
        }
    }

    /// Wipe the dirty rectangle. Called at the start of each rule pass
    /// once the previous pass has been fully consumed.
    pub fn clear_dirty(&mut self) {
        self.dirty = None;
    }

    /// Mark a local cell dirty without changing its contents.
    /// Used to re-wake stranded slope films that went quiescent.
    pub fn touch_dirty(&mut self, x: usize, y: usize) {
        let xu = x as u8;
        let yu = y as u8;
        match &mut self.dirty {
            Some(r) => r.expand_to_include(xu, yu),
            None => {
                self.dirty = Some(Rect {
                    x0: xu,
                    y0: yu,
                    x1: xu,
                    y1: yu,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::MaterialId;

    #[test]
    fn set_marks_dirty_rect() {
        let mut c = Chunk::new(ChunkCoord::new(0, 0));
        assert!(c.dirty.is_none());
        c.set(3, 5, Cell::solid(MaterialId::Sand));
        c.set(10, 2, Cell::solid(MaterialId::Sand));
        let r = c.dirty.expect("dirty after two writes");
        assert_eq!(r.x0, 3);
        assert_eq!(r.x1, 10);
        assert_eq!(r.y0, 2);
        assert_eq!(r.y1, 5);
    }

    #[test]
    fn clear_dirty_resets() {
        let mut c = Chunk::new(ChunkCoord::new(0, 0));
        c.set(0, 0, Cell::solid(MaterialId::Sand));
        assert!(c.dirty.is_some());
        c.clear_dirty();
        assert!(c.dirty.is_none());
    }

    #[test]
    fn full_rect_covers_grid() {
        let r = Rect::full();
        assert!(r.contains(0, 0));
        assert!(r.contains((CHUNK_CELLS_W - 1) as u8, (CHUNK_CELLS_H - 1) as u8));
    }

    #[test]
    fn set_raises_occupancy_flags() {
        let mut c = Chunk::new(ChunkCoord::new(0, 0));
        assert!(!c.has_wet_air);
        assert!(!c.has_limestone);
        assert!(!c.has_loose);
        c.set(1, 1, Cell::water());
        assert!(c.has_wet_air);
        c.set(2, 2, Cell::solid(MaterialId::Limestone));
        assert!(c.has_limestone);
        c.set(3, 3, Cell::solid(MaterialId::Sand));
        assert!(c.has_loose);
        // Dry air / stone do not clear sticky flags.
        c.set(1, 1, Cell::air());
        assert!(c.has_wet_air);
        c.set(3, 3, Cell::air());
        assert!(c.has_loose);
    }

    #[test]
    fn set_same_cell_does_not_dirty() {
        let mut c = Chunk::new(ChunkCoord::new(0, 0));
        c.set(3, 4, Cell::water());
        c.clear_dirty();
        c.set(3, 4, Cell::water());
        assert!(c.dirty.is_none(), "identical rewrite must not wake physics");
    }
}
