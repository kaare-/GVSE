//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Slow derived geotech stress map (docs/VOXEL_GEOTECH_MAP.md).
//!
//! Sweeps solid↔Air contacts and column overburden on a period-20
//! cadence. Stores sparse per-face stress + per-solid σᵥ for HUD
//! (`G` cycles shear / overburden / wetness) and F2/F3 modulators.
//! Does **not** write cells — overlays only.

use std::collections::HashMap;

use wk_material::{MaterialId, MaterialRegistry};

use crate::active::{plan_active, ActiveChunk};
use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::failure::{face_shear_demand, pore_wetness_with};
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
/// Max cells to walk upward when summing overburden.
pub const OVERBURDEN_MAX_UP: i32 = 96;
/// Force a full-world sweep every N smart rebuilds (S5).
pub const GEOTECH_FULL_EVERY: u32 = 8;

/// True when this tick should rebuild the geotech map.
pub fn geotech_map_due(tick: u64) -> bool {
    tick % GEOTECH_MAP_PERIOD == GEOTECH_MAP_PHASE
}

/// Which channel `G` is visualising in the demo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GeotechOverlayMode {
    #[default]
    Off,
    Shear,
    Overburden,
    Wetness,
}

impl GeotechOverlayMode {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Shear,
            Self::Shear => Self::Overburden,
            Self::Overburden => Self::Wetness,
            Self::Wetness => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Shear => "shear",
            Self::Overburden => "sigma_v",
            Self::Wetness => "wet",
        }
    }
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
    /// Combined score: `demand + weight * hydro/cap` (HUD + F2b).
    pub shear_score: f32,
    /// Relative overburden (Σ density above / 1000), including wet Air.
    pub overburden: f32,
}

impl FaceStress {
    pub fn from_parts(demand: i32, hydro_load: u16, wetness: f32, overburden: f32) -> Self {
        let hydro = (hydro_load as f32 / HYDRO_LOAD_CAP as f32).clamp(0.0, 1.0);
        let shear_score = demand as f32 + HYDRO_SCORE_WEIGHT * hydro;
        Self {
            demand,
            hydro_load,
            wetness,
            shear_score,
            overburden,
        }
    }
}

/// Sparse geotech overlay.
#[derive(Debug, Clone, Default)]
pub struct GeotechMap {
    /// `(gx, gy) → stress` for solid cells with open faces.
    pub faces: HashMap<(i32, i32), FaceStress>,
    /// `(gx, gy) → relative σᵥ` for every solid (F3 / HUD).
    pub overburden: HashMap<(i32, i32), f32>,
    /// Tick of last successful rebuild (`u64::MAX` = never).
    pub last_rebuild_tick: u64,
    /// Smart rebuilds since the last full sweep.
    pub rebuilds_since_full: u32,
    /// Last rebuild used the dirty-column path (S5).
    pub last_was_incremental: bool,
}

impl GeotechMap {
    pub fn new() -> Self {
        Self {
            faces: HashMap::new(),
            overburden: HashMap::new(),
            last_rebuild_tick: u64::MAX,
            rebuilds_since_full: 0,
            last_was_incremental: false,
        }
    }

    pub fn at_cell(&self, gx: i32, gy: i32) -> Option<FaceStress> {
        self.faces.get(&(gx, gy)).copied()
    }

    pub fn overburden_at(&self, gx: i32, gy: i32) -> f32 {
        self.overburden.get(&(gx, gy)).copied().unwrap_or(0.0)
    }

    pub fn clear(&mut self) {
        self.faces.clear();
        self.overburden.clear();
        self.last_rebuild_tick = u64::MAX;
        self.rebuilds_since_full = 0;
        self.last_was_incremental = false;
    }

    /// Full loaded-chunk sweep. Replaces previous contents.
    pub fn rebuild(&mut self, world: &World) {
        self.faces.clear();
        self.overburden.clear();
        let mut coords: Vec<_> = world.chunks.keys().copied().collect();
        coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));

        // Column σᵥ: walk each world-x top→bottom once per loaded chunk band.
        rebuild_overburden(world, &coords, &mut self.overburden);

        for coord in &coords {
            // Faces need a solid. Mid-ocean / empty sky never host one —
            // leftover 64×64 on the period-20 full sweep. Overburden still
            // walks those columns: sky wet Air loads σᵥ on land below.
            // Occupancy is the source of truth.
            if world
                .chunks
                .get(coord)
                .is_some_and(|c| !c.has_solid)
            {
                continue;
            }
            rescan_faces_in_chunk(world, *coord, None, &self.overburden, &mut self.faces);
        }
        self.last_rebuild_tick = world.tick;
        self.rebuilds_since_full = 0;
        self.last_was_incremental = false;
    }

    /// S5: prefer dirty-column update; fall back to full sweep periodically
    /// or when the map is empty / world is fully quiet after edits.
    pub fn rebuild_smart(&mut self, world: &World) {
        let active = plan_active(world);
        let force_full = self.faces.is_empty()
            || self.rebuilds_since_full + 1 >= GEOTECH_FULL_EVERY;
        if force_full {
            self.rebuild(world);
            return;
        }
        if active.is_empty() {
            // Nothing changed since last CA — keep map, mark due satisfied.
            self.last_rebuild_tick = world.tick;
            self.last_was_incremental = true;
            self.rebuilds_since_full = self.rebuilds_since_full.saturating_add(1);
            return;
        }
        self.rebuild_active(world, &active);
    }

    /// Rebuild only columns touched by `active` (inflated ±1 in x).
    pub fn rebuild_active(&mut self, world: &World, active: &[ActiveChunk]) {
        let mut columns: Vec<i32> = Vec::new();
        let mut y_lo = i32::MAX;
        let mut y_hi = i32::MIN;
        for ac in active {
            let base_x = ac.coord.cx * CHUNK_CELLS_W as i32;
            let base_y = ac.coord.cy * CHUNK_CELLS_H as i32;
            let x0 = world.wrap_x(base_x + ac.rect.x0 as i32 - 1);
            let x1 = world.wrap_x(base_x + ac.rect.x1 as i32 + 1);
            // Collect inclusive x range (handle wrap by scanning rect locals).
            for lx in ac.rect.x0.saturating_sub(1)..=(ac.rect.x1 + 1).min((CHUNK_CELLS_W - 1) as u8)
            {
                columns.push(world.wrap_x(base_x + lx as i32));
            }
            let _ = (x0, x1);
            y_lo = y_lo.min(base_y + ac.rect.y0 as i32 - 1);
            y_hi = y_hi.max(base_y + ac.rect.y1 as i32 + 1);
        }
        columns.sort_unstable();
        columns.dedup();
        y_lo = y_lo.max(0);
        if y_hi < y_lo {
            y_hi = y_lo;
        }

        for &gx in &columns {
            // Drop stale entries for this column.
            self.overburden.retain(|&(x, _), _| x != gx);
            self.faces.retain(|&(x, _), _| x != gx);
            rebuild_overburden_column(world, gx, &mut self.overburden);
            rescan_faces_column(world, gx, y_lo, y_hi, &self.overburden, &mut self.faces);
        }

        self.last_rebuild_tick = world.tick;
        self.rebuilds_since_full = self.rebuilds_since_full.saturating_add(1);
        self.last_was_incremental = true;
    }

    /// F3 / live-surface edit: rebuild σᵥ + faces for columns around `gx`.
    /// The period-20 smart path would keep the deleted-hill stamp until
    /// the next due tick.
    pub fn refresh_around(&mut self, world: &World, gx: i32, radius: i32) {
        let r = radius.max(0);
        let mut y_lo = i32::MAX;
        let mut y_hi = i32::MIN;
        for coord in world.chunks.keys() {
            let cy0 = coord.cy * CHUNK_CELLS_H as i32;
            let cy1 = cy0 + CHUNK_CELLS_H as i32 - 1;
            y_lo = y_lo.min(cy0);
            y_hi = y_hi.max(cy1);
        }
        if y_lo == i32::MAX {
            return;
        }
        y_lo = y_lo.max(0);
        for dx in -r..=r {
            let x = world.wrap_x(gx + dx);
            self.overburden.retain(|&(ox, _), _| ox != x);
            self.faces.retain(|&(ox, _), _| ox != x);
            rebuild_overburden_column(world, x, &mut self.overburden);
            rescan_faces_column(world, x, y_lo, y_hi, &self.overburden, &mut self.faces);
        }
        self.last_rebuild_tick = world.tick;
        self.last_was_incremental = true;
    }

    /// Rebuild when the period/phase gate says so (smart path).
    pub fn rebuild_if_due(&mut self, world: &World) -> bool {
        if !geotech_map_due(world.tick) {
            return false;
        }
        self.rebuild_smart(world);
        true
    }
}

fn rescan_faces_in_chunk(
    world: &World,
    coord: crate::chunk::ChunkCoord,
    y_clamp: Option<(i32, i32)>,
    overburden: &HashMap<(i32, i32), f32>,
    faces: &mut HashMap<(i32, i32), FaceStress>,
) {
    if world
        .chunks
        .get(&coord)
        .is_some_and(|c| !c.has_solid)
    {
        return;
    }
    for ly in 0..CHUNK_CELLS_H {
        let gy = coord.cy * CHUNK_CELLS_H as i32 + ly as i32;
        if gy <= 0 {
            continue;
        }
        if let Some((lo, hi)) = y_clamp {
            if gy < lo || gy > hi {
                continue;
            }
        }
        for lx in 0..CHUNK_CELLS_W {
            let gx = world.wrap_x(coord.cx * CHUNK_CELLS_W as i32 + lx as i32);
            maybe_insert_face(world, gx, gy, overburden, faces);
        }
    }
}

fn rescan_faces_column(
    world: &World,
    gx: i32,
    y_lo: i32,
    y_hi: i32,
    overburden: &HashMap<(i32, i32), f32>,
    faces: &mut HashMap<(i32, i32), FaceStress>,
) {
    // Scan a generous band — hydro faces need neighbours; overburden
    // already covers the full column.
    let y0 = y_lo.saturating_sub(2).max(1);
    let y1 = y_hi + HYDRO_LOAD_CAP;
    for gy in y0..=y1 {
        let (coord, _, _) = World::split(gx, gy);
        if world
            .chunks
            .get(&coord)
            .is_some_and(|c| !c.has_solid)
        {
            continue;
        }
        maybe_insert_face(world, gx, gy, overburden, faces);
    }
}

fn maybe_insert_face(
    world: &World,
    gx: i32,
    gy: i32,
    overburden: &HashMap<(i32, i32), f32>,
    faces: &mut HashMap<(i32, i32), FaceStress>,
) {
    let Some(cell) = world.get_cell(gx, gy) else {
        return;
    };
    if cell.material == MaterialId::Air {
        return;
    }
    if matches!(
        cell.material,
        MaterialId::Water | MaterialId::Snow | MaterialId::Ice
    ) {
        return;
    }
    let demand = face_shear_demand(world, gx, gy);
    if demand < 1 {
        return;
    }
    let hydro = wet_air_column_beside(world, gx, gy);
    let wet = pore_wetness_with(cell, &world.hydro);
    let sigma = overburden.get(&(gx, gy)).copied().unwrap_or(0.0);
    faces.insert(
        (gx, gy),
        FaceStress::from_parts(demand, hydro, wet, sigma),
    );
}

fn rebuild_overburden_column(world: &World, gx: i32, out: &mut HashMap<(i32, i32), f32>) {
    let gx = world.wrap_x(gx);
    // Determine y span from loaded chunks covering this x.
    let mut y_lo = i32::MAX;
    let mut y_hi = i32::MIN;
    for coord in world.chunks.keys() {
        let x0 = coord.cx * CHUNK_CELLS_W as i32;
        let x1 = x0 + CHUNK_CELLS_W as i32 - 1;
        // Wrap-aware: crude check — if gx falls in this chunk's x band.
        let mut hit = false;
        for x in x0..=x1 {
            if world.wrap_x(x) == gx {
                hit = true;
                break;
            }
        }
        if !hit {
            continue;
        }
        let cy0 = coord.cy * CHUNK_CELLS_H as i32;
        let cy1 = cy0 + CHUNK_CELLS_H as i32 - 1;
        y_lo = y_lo.min(cy0);
        y_hi = y_hi.max(cy1);
    }
    if y_lo == i32::MAX {
        return;
    }
    let mut above = 0.0f32;
    let mut gy = y_hi;
    while gy >= y_lo && gy >= 0 {
        match world.get_cell(gx, gy) {
            Some(c) if c.material != MaterialId::Air => {
                out.insert((gx, gy), above);
                above += MaterialRegistry::props(c.material).density as f32 / 1000.0;
            }
            Some(c) => {
                if c.sat.0 > 0 {
                    above += (c.sat.0 as f32 / 255.0) * 1.0;
                }
            }
            None => {}
        }
        gy -= 1;
    }
}

fn rebuild_overburden(
    world: &World,
    coords: &[crate::chunk::ChunkCoord],
    out: &mut HashMap<(i32, i32), f32>,
) {
    // Collect unique cx values, then for each column walk top→bottom.
    let mut cxs: Vec<i32> = coords.iter().map(|c| c.cx).collect();
    cxs.sort_unstable();
    cxs.dedup();
    let mut cys: Vec<i32> = coords.iter().map(|c| c.cy).collect();
    cys.sort_unstable();
    cys.dedup();
    let y_lo = cys.first().copied().unwrap_or(0) * CHUNK_CELLS_H as i32;
    let y_hi = (cys.last().copied().unwrap_or(0) + 1) * CHUNK_CELLS_H as i32 - 1;

    for cx in cxs {
        for lx in 0..CHUNK_CELLS_W {
            let gx = world.wrap_x(cx * CHUNK_CELLS_W as i32 + lx as i32);
            let mut above = 0.0f32;
            let mut gy = y_hi;
            while gy >= y_lo && gy >= 0 {
                match world.get_cell(gx, gy) {
                    Some(c) if c.material != MaterialId::Air => {
                        out.insert((gx, gy), above);
                        above += MaterialRegistry::props(c.material).density as f32 / 1000.0;
                    }
                    Some(c) => {
                        // Wet Air contributes water load (density 1000).
                        if c.sat.0 > 0 {
                            above += (c.sat.0 as f32 / 255.0) * 1.0;
                        }
                    }
                    None => {}
                }
                gy -= 1;
            }
        }
    }
}

/// Contiguous wet-Air column height beside a solid face (full stack).
///
/// From a wet-Air neighbour, walks **down and up** so mid-wall cells
/// see the whole reservoir height (hydrostatic proxy), not only water
/// above them.
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
        let mut h = 1i32;
        // Down.
        let mut y = gy - 1;
        while h < HYDRO_LOAD_CAP {
            match world.get_cell(nx, y) {
                Some(c) if c.material == MaterialId::Air && c.sat.0 >= HYDRO_MIN_SAT => {
                    h += 1;
                    y -= 1;
                }
                _ => break,
            }
        }
        // Up.
        y = gy + 1;
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
pub fn relative_overburden(world: &World, gx: i32, gy: i32, max_up: i32) -> f32 {
    let gx = world.wrap_x(gx);
    let mut sum = 0.0f32;
    for dy in 1..=max_up {
        match world.get_cell(gx, gy + dy) {
            Some(c) if c.material != MaterialId::Air => {
                sum += MaterialRegistry::props(c.material).density as f32 / 1000.0;
            }
            Some(c) if c.sat.0 > 0 => {
                sum += c.sat.0 as f32 / 255.0;
            }
            Some(_) => {}
            None => break,
        }
    }
    sum
}

/// Map shear_score → cohesion threshold for F2b (S3).
///
/// Higher score (steep + tall hydro) needs less effective cohesion to fail.
pub fn shear_score_c_threshold(score: f32) -> f32 {
    // score 1 → 40 (old demand-1); score 2 → 100 (old demand-2);
    // score 3+ → 160 (thin wet dams / tall reservoirs).
    if score < 1.5 {
        40.0
    } else if score < 2.5 {
        100.0
    } else {
        160.0
    }
}

/// Effective wetness for strength: pore fill, or hydro wetting proxy.
pub fn face_strength_wetness(pore: f32, hydro_load: u16) -> f32 {
    let hydro_frac = (hydro_load as f32 / HYDRO_LOAD_CAP as f32).clamp(0.0, 1.0);
    // Standing water beside the face counts as partial wetting even if
    // pores are still dry (seepage lag).
    pore.max(hydro_frac * 0.65)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Cell, Sat};
    use crate::chunk::ChunkCoord;
    use crate::failure::effective_cohesion;

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
    fn overlay_mode_cycles() {
        assert_eq!(GeotechOverlayMode::Off.next(), GeotechOverlayMode::Shear);
        assert_eq!(
            GeotechOverlayMode::Wetness.next(),
            GeotechOverlayMode::Off
        );
    }

    #[test]
    fn dry_buried_stone_absent_from_faces_but_has_overburden() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 8);
        w.set_cell(3, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(2, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(4, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(3, 2, Cell::solid(MaterialId::Stone));
        let mut map = GeotechMap::new();
        map.rebuild(&w);
        assert!(map.at_cell(3, 1).is_none());
        // Cap stone has Air above → overburden ~0; buried has load.
        assert!(map.overburden_at(3, 1) > map.overburden_at(3, 2));
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
    fn deep_stack_has_higher_overburden_than_surface() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 4);
        for y in 1..=6 {
            w.set_cell(2, y, Cell::solid(MaterialId::Stone));
            w.set_cell(3, y, Cell::air()); // give a face so we can read FaceStress too
        }
        let mut map = GeotechMap::new();
        map.rebuild(&w);
        let deep = map.overburden_at(2, 1);
        let mid = map.overburden_at(2, 3);
        let top = map.overburden_at(2, 6);
        assert!(top < 0.1, "surface overburden should be ~0, got {top}");
        assert!(deep > mid && mid > top, "deep={deep} mid={mid} top={top}");
        let face = map.at_cell(2, 1).expect("face");
        assert!((face.overburden - deep).abs() < 1e-4);
    }

    #[test]
    fn refresh_around_drops_overburden_after_a_hill_wipe() {
        let mut w = World::new(1);
        for cy in 0..=2 {
            w.ensure_chunk(ChunkCoord::new(0, cy));
        }
        bed(&mut w, 0, 4);
        for y in 1..=80 {
            w.set_cell(2, y, Cell::solid(MaterialId::Stone));
        }
        let mut map = GeotechMap::new();
        map.rebuild(&w);
        let under_hill = map.overburden_at(2, 8);
        assert!(
            under_hill > 5.0,
            "intact stack must load the buried cell (sv={under_hill})"
        );
        for y in 21..=80 {
            w.set_cell(2, y, Cell::air());
        }
        map.refresh_around(&w, 2, 0);
        let after = map.overburden_at(2, 8);
        assert!(
            after + 2.0 < under_hill,
            "live overburden must follow the wiped surface ({under_hill} → {after})"
        );
        assert!(
            map.overburden.get(&(2, 50)).is_none(),
            "erased cells must leave the overburden stamp"
        );
    }

    #[test]
    fn wet_dam_score_exceeds_stone_strength_threshold() {
        // Tall wet column beside a 1-wide stone wall → score ≥ 2.5 → thresh 160.
        // Dry stone c_eff=200 still holds; hydro wetting proxy drops c_eff.
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 6);
        for y in 1..=20 {
            w.set_cell(2, y, Cell::solid(MaterialId::Stone));
            w.set_cell(3, y, wet_air());
        }
        let mut map = GeotechMap::new();
        map.rebuild(&w);
        let face = map.at_cell(2, 10).expect("dam face");
        assert!(face.shear_score >= 2.5, "score={}", face.shear_score);
        let thresh = shear_score_c_threshold(face.shear_score);
        assert!(thresh >= 160.0);
        let wet = face_strength_wetness(face.wetness, face.hydro_load);
        let c_eff = effective_cohesion(MaterialId::Stone, wet);
        assert!(
            c_eff < thresh,
            "hydro-wetted stone dam should be below score threshold (c_eff={c_eff} thresh={thresh})"
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
        assert_eq!(a.overburden.len(), b.overburden.len());
        for (k, va) in &a.faces {
            let vb = b.faces.get(k).unwrap();
            assert_eq!(va.demand, vb.demand);
            assert_eq!(va.hydro_load, vb.hydro_load);
            assert!((va.shear_score - vb.shear_score).abs() < 1e-5);
            assert!((va.overburden - vb.overburden).abs() < 1e-4);
        }
    }

    #[test]
    fn rebuild_smart_incremental_updates_dirty_column_hydro() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 8);
        for y in 1..=6 {
            w.set_cell(3, y, Cell::solid(MaterialId::Stone));
            w.set_cell(4, y, Cell::air());
        }
        let mut map = GeotechMap::new();
        map.rebuild(&w);
        assert_eq!(map.at_cell(3, 3).unwrap().hydro_load, 0);

        // Fill the reservoir — dirties those cells.
        for y in 1..=6 {
            w.set_cell(4, y, wet_air());
        }
        map.rebuild_smart(&w);
        assert!(
            map.last_was_incremental,
            "dirty halo should take the incremental path"
        );
        let face = map.at_cell(3, 3).expect("face");
        assert!(
            face.hydro_load >= 4,
            "incremental rebuild must see wet column (hydro={})",
            face.hydro_load
        );
    }

    #[test]
    fn rebuild_smart_skips_when_world_quiet() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 4);
        w.set_cell(2, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(3, 1, Cell::air());
        let mut map = GeotechMap::new();
        map.rebuild(&w);
        let faces_before = map.faces.len();
        crate::active::clear_all_dirty(&mut w);
        map.rebuild_smart(&w);
        assert!(map.last_was_incremental);
        assert_eq!(map.faces.len(), faces_before);
    }

    #[test]
    fn face_rescan_skips_water_only_chunks() {
        let mut w = World::new(128);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(1, 0));
        for x in 0..8 {
            w.set_cell(x, 2, Cell::water());
        }
        w.set_cell(68, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..=4 {
            w.set_cell(68, y, Cell::solid(MaterialId::Stone));
            w.set_cell(69, y, Cell::air());
        }
        let mut map = GeotechMap::new();
        map.rebuild(&w);
        assert!(
            (0..8).all(|x| (0..8).all(|y| map.at_cell(x, y).is_none())),
            "mid-ocean must not mint faces"
        );
        let face = map.at_cell(68, 3).expect("cliff beside ocean still maps");
        assert!(face.demand >= 1);
    }

    #[test]
    fn overburden_still_counts_sky_wet_air() {
        let mut w = World::new(8);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(0, 1));
        w.set_cell(3, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(3, 1, Cell::solid(MaterialId::Stone));
        for y in 66..72 {
            w.set_cell(3, y, Cell::water());
        }
        let mut map = GeotechMap::new();
        map.rebuild(&w);
        assert!(
            map.overburden_at(3, 1) > 0.0,
            "sky wet Air in a !has_solid chunk must still load σᵥ"
        );
        assert!(
            !w.chunks[&ChunkCoord::new(0, 1)].has_solid,
            "precondition: water column did not raise has_solid"
        );
    }
}
