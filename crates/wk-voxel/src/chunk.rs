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
///
/// [`DirtyBits`] packs one row into a `u64` — keep this 64.
pub const CHUNK_CELLS_W: usize = 64;
/// Chunk height in cells.
pub const CHUNK_CELLS_H: usize = 64;
/// Total cells per chunk.
pub const CHUNK_CELLS: usize = CHUNK_CELLS_W * CHUNK_CELLS_H;
/// Air sat at or above this is standing water / a full pipe film.
/// Rain and falling drizzle sit well below (typically ~33).
pub const STANDING_AIR_SAT: u8 = 160;

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

    pub fn area(self) -> usize {
        let w = (self.x1 as usize).saturating_sub(self.x0 as usize) + 1;
        let h = (self.y1 as usize).saturating_sub(self.y0 as usize) + 1;
        w.saturating_mul(h)
    }
}

/// Per-cell dirty mask for one chunk. One `u64` row (bit `x` in row `y`).
///
/// The dirty **rect** is the bounding box of writes. Planning dilates
/// these bits (not the box) so scattered rain does not scan the holes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyBits(pub [u64; CHUNK_CELLS_H]);

impl Default for DirtyBits {
    fn default() -> Self {
        Self([0; CHUNK_CELLS_H])
    }
}

impl DirtyBits {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(self) -> bool {
        self.0.iter().all(|&w| w == 0)
    }

    #[inline]
    pub fn set(&mut self, x: u8, y: u8) {
        debug_assert!((x as usize) < CHUNK_CELLS_W);
        debug_assert!((y as usize) < CHUNK_CELLS_H);
        self.0[y as usize] |= 1u64 << x;
    }

    #[inline]
    pub fn get(self, x: u8, y: u8) -> bool {
        self.0[y as usize] & (1u64 << x) != 0
    }

    pub fn or_assign(&mut self, other: Self) {
        for (a, b) in self.0.iter_mut().zip(other.0.iter()) {
            *a |= *b;
        }
    }

    pub fn count(self) -> usize {
        self.0.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Inclusive bbox of set bits, or `None` if empty.
    pub fn bbox(self) -> Option<Rect> {
        let mut rect: Option<Rect> = None;
        for y in 0..CHUNK_CELLS_H as u8 {
            let row = self.0[y as usize];
            if row == 0 {
                continue;
            }
            let x0 = row.trailing_zeros() as u8;
            let x1 = 63 - row.leading_zeros() as u8;
            match &mut rect {
                Some(r) => {
                    r.expand_to_include(x0, y);
                    r.expand_to_include(x1, y);
                }
                None => {
                    rect = Some(Rect {
                        x0,
                        y0: y,
                        x1,
                        y1: y,
                    });
                }
            }
        }
        rect
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
    /// Per-cell dirty mask for the same writes as [`Self::dirty`].
    /// Runtime only — old saves have the rect and empty bits, so
    /// [`crate::active::plan_active`] falls back to the box.
    #[serde(default, skip)]
    pub dirty_bits: DirtyBits,
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
    /// never marks pure groundwater chunks). Cleared by
    /// [`crate::rules::seepage::wake_pore_weep_into_air`] when a scan
    /// finds no wet solid left.
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
    /// that never held litter. Cleared by
    /// [`crate::rules::grain::rise_and_soak_buoyant_litter`] when a
    /// buoyant collect finds no Organic left — a falling leaf otherwise
    /// keeps every sky chunk it passed through in the raft walk.
    #[serde(default)]
    pub has_organic: bool,
    /// Sticky occupancy: Organic / Snow / Ice (buoyant litter). Rise/soak
    /// scans skip sand-only loose chunks. Cleared when a buoyant collect
    /// finds none — otherwise a flake falling through the sky leaves those
    /// chunks in the rise/soak walk forever.
    #[serde(default)]
    pub has_buoyant: bool,
    /// Sticky occupancy: at least one `Snow` cell. Wind-drift skips
    /// organic rafts and ice sheets that have never held a flake.
    /// Cleared by [`crate::rules::grain::apply_snow_wind_drift`] when a
    /// scan finds no flake left.
    #[serde(default)]
    pub has_snow: bool,
    /// Sticky occupancy: at least one competent rock cell (stone /
    /// limestone / flowstone / sandstone / conglomerate). Floating-body
    /// wake skips empty sky and bedrock chunks. Cleared by
    /// [`crate::rules::competent_fall::wake_floating_competent`] when a
    /// scan finds none left.
    #[serde(default)]
    pub has_competent: bool,
    /// Sticky occupancy: at least one `Air` cell with
    /// `sat >= `[`STANDING_AIR_SAT`] (standing water / full pipe).
    /// Lake-bed and confined-head wakes skip rain-film sky. Cleared
    /// when a scan finds none left.
    #[serde(default)]
    pub has_standing_air: bool,
    /// Sticky occupancy: at least one non-`Air` cell. Confined-head
    /// wake skips mid-ocean chunks that are only water. Cleared when a
    /// scan finds none left.
    #[serde(default)]
    pub has_solid: bool,
    /// Sticky occupancy: at least one `Air` cell (wet or dry). Pore-weep
    /// on buried crust only scans the chunk perimeter — interior cells
    /// cannot face Air. Raised on any Air write; cleared when a scan
    /// finds none left.
    #[serde(default)]
    pub has_open_air: bool,
    /// Sticky occupancy: at least one porous solid below capacity.
    /// Lake-bed skip of a quiet saturated water table. Raised on any
    /// wet-solid write (capacity is not known at chunk level); cleared
    /// when a scan finds every pore full.
    #[serde(default)]
    pub has_unsaturated_pores: bool,
    /// Sticky occupancy: at least one `Clay` cell. Suspension scans skip
    /// rain-wet sand / soil / gravel that can never entrain. Raised on a
    /// Clay write; cleared by [`crate::sediment::apply_suspension`] when
    /// a scan finds none left. `true` on old saves (missing field) so
    /// they scan once, then the empty pass clears them.
    #[serde(default = "serde_flag_true")]
    pub has_clay: bool,
    /// Sticky occupancy: at least one soluble rock (limestone, flowstone,
    /// sandstone, conglomerate). Karst skips rain-soaked sand / soil that
    /// has no carbonate to dissolve. Raised on a soluble write; cleared
    /// when a karst scan finds none left. `true` on old saves so they
    /// scan once, then the empty pass clears them.
    #[serde(default = "serde_flag_true")]
    pub has_soluble: bool,
    /// Inclusive local-y band of standing Air. `y0 > y1` is unset
    /// (old saves / bootstrap) so confined still scans the full rect.
    /// Raised on a standing write; tightened by the evap occupancy walk.
    #[serde(default = "standing_band_unset_lo")]
    pub standing_air_y0: u8,
    #[serde(default = "standing_band_unset_hi")]
    pub standing_air_y1: u8,
}

fn serde_flag_true() -> bool {
    true
}

fn standing_band_unset_lo() -> u8 {
    255
}

fn standing_band_unset_hi() -> u8 {
    0
}

/// Materials that participate in grain settle / float / punch passes.
#[inline]
pub fn material_is_loose(material: MaterialId) -> bool {
    matches!(
        material,
        MaterialId::Sand
            | MaterialId::Gravel
            | MaterialId::Clay
            | MaterialId::Bentonite
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
            dirty_bits: DirtyBits::empty(),
            tick: 0,
            has_wet_air: false,
            has_wet_pores: false,
            has_limestone: false,
            has_loose: false,
            has_organic: false,
            has_buoyant: false,
            has_snow: false,
            has_competent: false,
            has_standing_air: false,
            has_solid: false,
            has_open_air: false,
            has_unsaturated_pores: false,
            has_clay: false,
            has_soluble: false,
            standing_air_y0: 255,
            standing_air_y1: 0,
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
        self.dirty_bits.set(xu, yu);
        if cell.material == MaterialId::Air && !cell.sat.is_empty() {
            self.has_wet_air = true;
        }
        // World hydrology overrides are not available at chunk level.
        // Mark every wet solid so a material whose range is changed from
        // 0–0 to permeable cannot miss quiet groundwater wakes. This may
        // over-wake a rare wet impermeable cell, but never misses water.
        if cell.material != MaterialId::Air && !cell.sat.is_empty() {
            self.has_wet_pores = true;
            self.has_unsaturated_pores = true;
        }
        if cell.material == MaterialId::Air {
            self.has_open_air = true;
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
        if cell.material == MaterialId::Snow {
            self.has_snow = true;
        }
        if crate::cell::is_competent_rock(cell.material) {
            self.has_competent = true;
        }
        if cell.material == MaterialId::Clay {
            self.has_clay = true;
        }
        if wk_material::MaterialRegistry::base_props(cell.material).solubility > 0 {
            self.has_soluble = true;
        }
        if cell.material == MaterialId::Air && cell.sat.0 >= STANDING_AIR_SAT {
            self.has_standing_air = true;
            self.note_standing_air_y(yu);
        }
        if cell.material != MaterialId::Air {
            self.has_solid = true;
        }
    }

    /// Wipe the dirty rectangle. Called at the start of each rule pass
    /// once the previous pass has been fully consumed.
    pub fn clear_dirty(&mut self) {
        self.dirty = None;
        self.dirty_bits = DirtyBits::empty();
    }

    /// Expand the standing-air y band to include `y`.
    pub fn note_standing_air_y(&mut self, y: u8) {
        if self.standing_air_y0 > self.standing_air_y1 {
            self.standing_air_y0 = y;
            self.standing_air_y1 = y;
            return;
        }
        self.standing_air_y0 = self.standing_air_y0.min(y);
        self.standing_air_y1 = self.standing_air_y1.max(y);
    }

    /// Clear standing occupancy and the y band (scan found none).
    pub fn clear_standing_air(&mut self) {
        self.has_standing_air = false;
        self.standing_air_y0 = 255;
        self.standing_air_y1 = 0;
    }

    /// Local y range to scan for confined rise, intersected with `rect`.
    ///
    /// The rising film sits on the standing column, so the band is
    /// expanded by one row. Unset bands (`y0 > y1`) keep the full rect.
    pub fn standing_scan_y(&self, rect: Rect) -> (u8, u8) {
        if self.standing_air_y0 > self.standing_air_y1 {
            return (rect.y0, rect.y1);
        }
        let lo = self.standing_air_y0.saturating_sub(1).max(rect.y0);
        let hi = (self.standing_air_y1.saturating_add(1)).min(rect.y1);
        if lo <= hi {
            (lo, hi)
        } else {
            (rect.y0, rect.y1)
        }
    }

    /// Inclusive standing-air rows only (no rising-film pad).
    /// `None` when the band is unset — caller keeps the full rect.
    pub fn standing_band_y(&self, rect: Rect) -> Option<(u8, u8)> {
        if self.standing_air_y0 > self.standing_air_y1 {
            return None;
        }
        let lo = self.standing_air_y0.max(rect.y0);
        let hi = self.standing_air_y1.min(rect.y1);
        if lo <= hi {
            Some((lo, hi))
        } else {
            None
        }
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
        self.dirty_bits.set(xu, yu);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Cell, Sat};
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
        assert!(c.dirty_bits.get(3, 5));
        assert!(c.dirty_bits.get(10, 2));
        assert!(!c.dirty_bits.get(6, 3), "AABB hole is not a write");
    }

    #[test]
    fn clear_dirty_resets() {
        let mut c = Chunk::new(ChunkCoord::new(0, 0));
        c.set(0, 0, Cell::solid(MaterialId::Sand));
        assert!(c.dirty.is_some());
        c.clear_dirty();
        assert!(c.dirty.is_none());
        assert!(c.dirty_bits.is_empty());
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
        assert!(!c.has_snow);
        assert!(!c.has_competent);
        assert!(!c.has_standing_air);
        assert!(!c.has_solid);
        assert!(!c.has_open_air);
        assert!(!c.has_unsaturated_pores);
        assert!(!c.has_clay);
        assert!(!c.has_soluble);
        let mut rain = Cell::air();
        rain.sat = Sat(33);
        c.set(0, 0, rain);
        assert!(c.has_wet_air);
        assert!(c.has_open_air);
        assert!(!c.has_standing_air);
        assert!(!c.has_solid);
        c.set(1, 1, Cell::water());
        assert!(c.has_wet_air);
        assert!(c.has_standing_air);
        assert_eq!(c.standing_air_y0, 1);
        assert_eq!(c.standing_air_y1, 1);
        assert!(c.has_open_air);
        c.set(2, 2, Cell::solid(MaterialId::Limestone));
        assert!(c.has_limestone);
        assert!(c.has_soluble);
        assert!(c.has_competent);
        assert!(c.has_solid);
        c.set(3, 3, Cell::solid(MaterialId::Sand));
        assert!(c.has_loose);
        assert!(!c.has_clay);
        c.set(7, 7, Cell::solid(MaterialId::Clay));
        assert!(c.has_clay);
        let mut wet_sand = Cell::solid(MaterialId::Sand);
        wet_sand.sat = Sat(8);
        c.set(6, 6, wet_sand);
        assert!(c.has_unsaturated_pores);
        c.set(4, 4, Cell::solid(MaterialId::Snow));
        assert!(c.has_snow);
        c.set(5, 5, Cell::solid(MaterialId::Stone));
        assert!(c.has_competent);
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

    #[test]
    fn standing_scan_y_covers_the_rising_film() {
        let mut c = Chunk::new(ChunkCoord::new(0, 0));
        let full = Rect::full();
        assert_eq!(c.standing_scan_y(full), (0, 63));
        c.set(4, 10, Cell::water());
        assert_eq!(c.standing_scan_y(full), (9, 11));
        assert_eq!(c.standing_band_y(full), Some((10, 10)));
        c.clear_standing_air();
        assert_eq!(c.standing_band_y(full), None);
        assert!(!c.has_standing_air);
        assert_eq!(c.standing_scan_y(full), (0, 63));
    }
}
