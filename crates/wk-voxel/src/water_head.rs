//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse **regional water-table head**. Derived from cells, never a
//! second water store. Throughflow / confined rise may *scale* their
//! existing rates when a column sits under a higher nearby table
//! (mountain recharge → valley spring). Pore-pore seepage is unchanged.
//!
//! Step 3 (bias near-surface wind from this field) is not wired.

use crate::cell::water_capacity_cell;
use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::fasthash::FxHashMap;
use crate::grid::World;
use wk_material::MaterialId;

/// Rebuild this often. Cell head still moves water every tick.
pub const WATER_HEAD_PERIOD: u64 = 24;

const SAMPLE_STRIDE: i32 = 4;
const SMOOTH_PASSES: u32 = 4;
const NEAR_HEAD_CELLS: i32 = 24;
const STANDING_HAZE: u8 = 32;

/// Sparse per-column water-table elevation (world cells), Darcy-smoothed.
#[derive(Debug, Clone, Default)]
pub struct WaterHead {
    head: FxHashMap<i32, f32>,
    last_tick: u64,
    built: bool,
}

impl WaterHead {
    pub fn maybe_refresh(&mut self, world: &World) {
        if self.built && world.tick.saturating_sub(self.last_tick) < WATER_HEAD_PERIOD {
            return;
        }
        self.rebuild(world);
    }

    fn rebuild(&mut self, world: &World) {
        self.head.clear();
        let cw = CHUNK_CELLS_W as i32;
        let mut columns: Vec<i32> = Vec::new();
        for coord in world.chunks.keys() {
            let x0 = coord.cx * cw;
            let mut x = x0;
            while x < x0 + cw {
                columns.push(world.wrap_x(x));
                x += SAMPLE_STRIDE;
            }
        }
        columns.sort_unstable();
        columns.dedup();
        for gx in columns {
            if let Some(y) = column_table_y(world, gx) {
                self.head.insert(gx, y as f32);
            }
        }
        self.smooth();
        self.last_tick = world.tick;
        self.built = true;
    }

    fn smooth(&mut self) {
        for _ in 0..SMOOTH_PASSES {
            let snap = self.head.clone();
            for (&gx, &h) in &snap {
                let mut s = h * 2.0;
                let mut w = 2.0;
                for dx in [-SAMPLE_STRIDE, SAMPLE_STRIDE] {
                    if let Some(&n) = snap.get(&(gx + dx)) {
                        s += n;
                        w += 1.0;
                    }
                }
                self.head.insert(gx, s / w);
            }
        }
    }

    fn head_at(&self, gx: i32) -> Option<f32> {
        if let Some(&h) = self.head.get(&gx) {
            return Some(h);
        }
        let mut best: Option<(i32, f32)> = None;
        for (&x, &h) in &self.head {
            let d = (x - gx).abs();
            if d > NEAR_HEAD_CELLS {
                continue;
            }
            if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                best = Some((d, h));
            }
        }
        best.map(|(_, h)| h)
    }

    /// `1` when this cell is at or above the regional table.
    /// Up to `1.4` under a high nearby table. Never below 1 — we only
    /// boost exits, we do not throttle the CA.
    pub fn rate_scale(&self, gx: i32, gy: i32) -> f32 {
        let Some(h) = self.head_at(gx) else {
            return 1.0;
        };
        let over = (h - gy as f32).max(0.0);
        (1.0 + 0.012 * over.min(40.0)).min(1.40)
    }
}

fn column_table_y(world: &World, gx: i32) -> Option<i32> {
    let jx = world.wrap_x(gx);
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let cx = jx.div_euclid(cw);
    let mut cys: Vec<i32> = world
        .chunks
        .keys()
        .filter(|c| c.cx == cx)
        .map(|c| c.cy)
        .collect();
    if cys.is_empty() {
        return None;
    }
    cys.sort_unstable();
    for &cy in cys.iter().rev() {
        let y0 = cy * ch;
        let y1 = y0 + ch - 1;
        for y in (y0..=y1).rev() {
            let Some(c) = world.get_cell(jx, y) else {
                continue;
            };
            if c.material == MaterialId::Air {
                if c.sat.0 > STANDING_HAZE {
                    return Some(y);
                }
                continue;
            }
            let cap = water_capacity_cell(c, &world.hydro);
            if cap > 0 && (c.sat.0 as u32) * 5 >= (cap as u32) * 4 {
                return Some(y);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Cell, Sat};
    use crate::chunk::ChunkCoord;
    use wk_material::MaterialId;

    #[test]
    fn empty_world_does_not_scale() {
        let w = World::new(1);
        let head = WaterHead::default();
        assert_eq!(head.rate_scale(4, 8), 1.0);
        let _ = w;
    }

    #[test]
    fn high_lake_boosts_a_lower_spring_column() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Stone));
            for y in 1..=24 {
                w.set_cell(
                    x,
                    y,
                    Cell {
                        material: MaterialId::Air,
                        sat: Sat::FULL,
                        ..Cell::air()
                    },
                );
            }
        }
        let mut head = WaterHead::default();
        head.maybe_refresh(&w);
        let scale = head.rate_scale(20, 10);
        assert!(
            scale > 1.05,
            "a lake at y=24 should over-pressure a spring at y=10 (scale={scale:.3})"
        );
        assert!(scale <= 1.40);
        let sat_sum = |w: &World| {
            let mut n = 0i64;
            for x in 0..12 {
                for y in 1..=24 {
                    if let Some(c) = w.get_cell(x, y) {
                        n += c.sat.0 as i64;
                    }
                }
            }
            n
        };
        let sat_before = sat_sum(&w);
        head.maybe_refresh(&w);
        assert_eq!(sat_before, sat_sum(&w), "head overlay must not write sat");
    }
}
