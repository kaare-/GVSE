//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Slow derived geotech stress map (docs/VOXEL_GEOTECH_MAP.md).
//!
//! Sweeps solid↔Air contacts on a period-20 cadence. Stores sparse
//! per-face stress for HUD (`G`) and later F2/F3 modulators. Does
//! **not** write cells — overlays only.

use std::collections::HashMap;

use wk_material::{MaterialId, MaterialRegistry};

use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::failure::{face_shear_demand, pore_wetness};
use crate::grid::World;

/// Rebuild period (ticks), matching Temperature / humidity diffuse.
pub const GEOTECH_MAP_PERIOD: u64 = 20;
/// Phase offset so we don't pile on temp (0) / humidity (3).
pub const GEOTECH_MAP_PHASE: u64 = 7;
/// Cap wet-Air column height when scoring lateral hydro load.
pub const HYDRO_LOAD_CAP: i32 = 32;
/// How much a full hydro column adds on top of geometric demand.
pub const HYDRO_SCORE_WEIGHT: f32 = 2.0;
/// Air sat at/above this counts as standing water for hydro load.
pub const HYDRO_MIN_SAT: u8 = 200;

/// True when this tick should rebuild the geotech map.
pub fn geotech_map_due(tick: u64) -> bool {
    tick % GEOTECH_MAP_PERIOD == GEOTECH_MAP_PHASE
}

/// Per-face derived stress at a solid cell with an open Air contact.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceStress {
    /// Geometric face demand (0..2 from [`face_shear_demand`]).
    pub demand: i32,
    /// Contiguous wet-Air column height beside the face (cells).
    pub hydro_load: u16,
    /// Pore wetness 0..1.
    pub wetness: f32,
    /// Combined score: `demand + weight * hydro/cap` (HUD + later CA).
    pub shear_score: f32,
    /// Relative overburden (Σ density above / 1000). Filled in S2; 0 in S1.
    pub overburden: f32,
}

impl FaceStress {
    pub fn from_parts(demand: i32, hydro_load: u16, wetness: f32) -> Self {
        let hydro = (hydro_load as f32 / HYDRO_LOAD_CAP as f32).clamp(0.0, 1.0);
        let shear_score = demand as f32 + HYDRO_SCORE_WEIGHT * hydro;
        Self {
            demand,
            hydro_load,
            wetness,
            shear_score,
            overburden: 0.0,
        }
    }
}

/// Sparse geotech overlay — only solid cells with open faces.
#[derive(Debug, Clone, Default)]
pub struct GeotechMap {
    /// `(gx, gy) → stress` for face cells.
    pub faces: HashMap<(i32, i32), FaceStress>,
    /// Tick of last successful rebuild (`u64::MAX` = never).
    pub last_rebuild_tick: u64,
}

impl GeotechMap {
    pub fn new() -> Self {
        Self {
            faces: HashMap::new(),
            last_rebuild_tick: u64::MAX,
        }
    }

    pub fn at_cell(&self, gx: i32, gy: i32) -> Option<FaceStress> {
        self.faces.get(&(gx, gy)).copied()
    }

    pub fn clear(&mut self) {
        self.faces.clear();
        self.last_rebuild_tick = u64::MAX;
    }

    /// Full loaded-chunk contact sweep. Replaces previous contents.
    pub fn rebuild(&mut self, world: &World) {
        self.faces.clear();
        let mut coords: Vec<_> = world.chunks.keys().copied().collect();
        coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));

        for coord in coords {
            for ly in 0..CHUNK_CELLS_H {
                let gy = coord.cy * CHUNK_CELLS_H as i32 + ly as i32;
                if gy <= 0 {
                    continue;
                }
                for lx in 0..CHUNK_CELLS_W {
                    let gx = world.wrap_x(coord.cx * CHUNK_CELLS_W as i32 + lx as i32);
                    let Some(cell) = world.get_cell(gx, gy) else {
                        continue;
                    };
                    if cell.material == MaterialId::Air {
                        continue;
                    }
                    // Skip fluids / non-structural cards.
                    if matches!(
                        cell.material,
                        MaterialId::Water | MaterialId::Snow | MaterialId::Ice
                    ) {
                        continue;
                    }
                    let demand = face_shear_demand(world, gx, gy);
                    if demand < 1 {
                        continue;
                    }
                    let hydro = wet_air_column_beside(world, gx, gy);
                    let wet = pore_wetness(cell);
                    let stress = FaceStress::from_parts(demand, hydro, wet);
                    self.faces.insert((gx, gy), stress);
                }
            }
        }
        self.last_rebuild_tick = world.tick;
    }

    /// Rebuild when the period/phase gate says so.
    pub fn rebuild_if_due(&mut self, world: &World) -> bool {
        if !geotech_map_due(world.tick) {
            return false;
        }
        self.rebuild(world);
        true
    }
}

/// Max contiguous wet-Air column height adjacent to a solid face.
///
/// Checks ±x neighbours: if the neighbour is wet Air, walks upward
/// counting cells with `sat ≥ HYDRO_MIN_SAT`. Returns the max of the
/// two sides, capped at [`HYDRO_LOAD_CAP`].
pub fn wet_air_column_beside(world: &World, gx: i32, gy: i32) -> u16 {
    let mut best = 0u16;
    for &dx in &[-1i32, 1] {
        let nx = world.wrap_x(gx + dx);
        let Some(side) = world.get_cell(nx, gy) else {
            continue;
        };
        if side.material != MaterialId::Air || side.sat.0 < HYDRO_MIN_SAT {
            continue;
        }
        // Count this cell, then walk up.
        let mut h = 1i32;
        let mut y = gy + 1;
        while h < HYDRO_LOAD_CAP {
            match world.get_cell(nx, y) {
                Some(c) if c.material == MaterialId::Air && c.sat.0 >= HYDRO_MIN_SAT => {
                    h += 1;
                    y += 1;
                }
                _ => break,
            }
        }
        best = best.max(h as u16);
    }
    best
}

/// Relative overburden: sum of solid densities above `(gx, gy)` / 1000.
/// Exposed for S2; unused in S1 rebuild.
#[allow(dead_code)]
pub fn relative_overburden(world: &World, gx: i32, gy: i32, max_up: i32) -> f32 {
    let gx = world.wrap_x(gx);
    let mut sum = 0u64;
    for dy in 1..=max_up {
        match world.get_cell(gx, gy + dy) {
            Some(c) if c.material != MaterialId::Air => {
                sum += MaterialRegistry::props(c.material).density as u64;
            }
            Some(_) => {}
            None => break,
        }
    }
    (sum as f32) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Cell, Sat};
    use crate::chunk::ChunkCoord;

    fn bed(w: &mut World, x0: i32, x1: i32) {
        for x in x0..=x1 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
    }

    fn wet_air() -> Cell {
        let mut c = Cell::air();
        c.sat = Sat(255);
        c
    }

    #[test]
    fn geotech_map_due_phase() {
        assert!(geotech_map_due(GEOTECH_MAP_PHASE));
        assert!(!geotech_map_due(GEOTECH_MAP_PHASE + 1));
        assert!(geotech_map_due(GEOTECH_MAP_PHASE + GEOTECH_MAP_PERIOD));
    }

    #[test]
    fn dry_buried_stone_absent() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 8);
        // Buried: stone with stone neighbours, no Air face.
        w.set_cell(3, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(2, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(4, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(3, 2, Cell::solid(MaterialId::Stone));
        let mut map = GeotechMap::new();
        map.rebuild(&w);
        assert!(map.at_cell(3, 1).is_none());
    }

    #[test]
    fn vertical_cliff_face_recorded() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 8);
        for y in 1..=4 {
            w.set_cell(3, y, Cell::solid(MaterialId::Stone));
            w.set_cell(4, y, Cell::air());
        }
        let mut map = GeotechMap::new();
        map.rebuild(&w);
        let face = map.at_cell(3, 3).expect("cliff face");
        assert!(face.demand >= 1);
        assert_eq!(face.hydro_load, 0);
        assert!(face.shear_score >= 1.0);
    }

    #[test]
    fn tall_wet_column_raises_hydro_and_score() {
        let mut dry = World::new(1);
        dry.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut dry, 0, 8);
        for y in 1..=8 {
            dry.set_cell(3, y, Cell::solid(MaterialId::Stone));
            dry.set_cell(4, y, Cell::air());
        }
        let mut wet = dry.clone();
        for y in 1..=8 {
            wet.set_cell(4, y, wet_air());
        }

        let mut map_dry = GeotechMap::new();
        map_dry.rebuild(&dry);
        let mut map_wet = GeotechMap::new();
        map_wet.rebuild(&wet);

        let d = map_dry.at_cell(3, 4).expect("dry face");
        let ww = map_wet.at_cell(3, 4).expect("wet face");
        assert_eq!(d.hydro_load, 0);
        assert!(ww.hydro_load >= 4, "hydro_load={}", ww.hydro_load);
        assert!(
            ww.shear_score > d.shear_score,
            "wet score {} should exceed dry {}",
            ww.shear_score,
            d.shear_score
        );
    }

    #[test]
    fn rebuild_is_deterministic() {
        let mut w = World::new(7);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 10);
        for y in 1..=5 {
            w.set_cell(5, y, Cell::solid(MaterialId::Limestone));
            w.set_cell(6, y, wet_air());
        }
        let mut a = GeotechMap::new();
        let mut b = GeotechMap::new();
        a.rebuild(&w);
        b.rebuild(&w);
        assert_eq!(a.faces.len(), b.faces.len());
        for (k, va) in &a.faces {
            let vb = b.faces.get(k).unwrap();
            assert_eq!(va.demand, vb.demand);
            assert_eq!(va.hydro_load, vb.hydro_load);
            assert!((va.shear_score - vb.shear_score).abs() < 1e-5);
        }
    }
}
