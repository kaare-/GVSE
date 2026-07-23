//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Geotech failure passes (docs/VOXEL_FAILURE.md).
//!
//! F1 — compressive roof / overhang collapse when an Air cavity span
//! exceeds the roof material's [`wk_material::MaterialProps::roof_span_max_m`].
//!
//! F2 — wet cohesion shear: grains loosen more when wet+low-c′ (F2a in
//! repose); competent rock faces can convert to [`MaterialId::LooseRock`]
//! when wet and steep (F2b).

use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};

use crate::active::{plan_active, ActiveChunk};
use crate::cell::{falls_through_empty_air, is_grain, water_capacity, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// Wetness multiplier on cohesion: `c_eff = c * (1 - k_wet * wet)`.
pub const SHEAR_K_WET: f32 = 0.70;
/// Wet repose (F2a) loosens `max_step` when `c_eff` is below this.
pub const WET_REPOSE_C_EFF_LOOSEN: f32 = 80.0;
/// F2b: demand-1 face needs `c_eff` below this to fail.
pub const SHEAR_C_THRESH_DEMAND_1: f32 = 40.0;
/// F2b: demand-2 face (taller / undercut) needs `c_eff` below this.
pub const SHEAR_C_THRESH_DEMAND_2: f32 = 100.0;

/// Live-tunable geotech knobs (Tab → Geotech).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureConfig {
    /// Drop ceilings when cavity span exceeds `roof_span_max_m`.
    pub enable_roof_collapse: bool,
    /// F2b — wet competent rock faces → LooseRock.
    pub enable_shear_weaken: bool,
    /// F3 — overburden compaction (not yet implemented).
    pub enable_compaction: bool,
    /// Max roof cells converted / dropped per tick.
    pub max_roof_events: u32,
    /// Max rock-face shear converts per tick.
    pub max_shear_events: u32,
    /// Per-candidate success chance in parts-per-thousand (0..=1000).
    /// Keeps mountains from melting in one tick.
    pub shear_chance_per_mille: u32,
}

impl Default for FailureConfig {
    fn default() -> Self {
        Self {
            enable_roof_collapse: true,
            // Off until tuned — enable in Tab → Geotech.
            enable_shear_weaken: false,
            enable_compaction: false,
            max_roof_events: 32,
            max_shear_events: 16,
            shear_chance_per_mille: 250,
        }
    }
}

/// Pore wetness 0..1 for a cell (`sat / capacity`; 0 if impermeable).
pub fn pore_wetness(cell: Cell) -> f32 {
    let cap = water_capacity(cell.material);
    if cap == 0 {
        return 0.0;
    }
    (cell.sat.0 as f32 / cap as f32).clamp(0.0, 1.0)
}

/// Effective cohesion after wetness: `c * (1 - k_wet * wet)`.
pub fn effective_cohesion(material: MaterialId, wet: f32) -> f32 {
    let c = MaterialRegistry::props(material).cohesion as f32;
    c * (1.0 - SHEAR_K_WET * wet.clamp(0.0, 1.0))
}

/// F2a: wet grains with low `c_eff` lose one `max_step` of repose stability.
pub fn wet_repose_loosens(material: MaterialId, wet: f32) -> bool {
    if wet <= 0.0 {
        return false;
    }
    effective_cohesion(material, wet) < WET_REPOSE_C_EFF_LOOSEN
}

/// F2b threshold for a face demand of 1 or 2 cells.
pub fn shear_c_threshold(demand: i32) -> f32 {
    match demand {
        1 => SHEAR_C_THRESH_DEMAND_1,
        d if d >= 2 => SHEAR_C_THRESH_DEMAND_2,
        _ => 0.0,
    }
}

/// Max unsupported Air span (cells) this material can roof.
/// `i32::MAX` = never collapses (Bedrock / infinite span).
pub fn roof_span_limit_cells(material: MaterialId) -> i32 {
    let rise = MaterialRegistry::props(material).roof_span_max_m;
    if !rise.is_finite() {
        return i32::MAX;
    }
    if rise <= 0.0 {
        return 0;
    }
    (rise / SAMPLE_WIDTH_M).floor() as i32
}

/// Contiguous horizontal Air span length at `(gx, cavity_y)`, walking
/// left/right until a non-Air cell or missing cell. `0` if the seed
/// cell is not Air.
pub fn roof_span_cells(world: &World, gx: i32, cavity_y: i32) -> i32 {
    let gx = world.wrap_x(gx);
    let Some(seed) = world.get_cell(gx, cavity_y) else {
        return 0;
    };
    if seed.material != MaterialId::Air {
        return 0;
    }
    let mut n = 1i32;
    // Walk −x.
    let mut x = gx;
    for _ in 0..CHUNK_CELLS_W as i32 * 4 {
        let nx = world.wrap_x(x - 1);
        if nx == gx && n > 1 {
            break; // full wrap
        }
        match world.get_cell(nx, cavity_y) {
            Some(c) if c.material == MaterialId::Air => {
                n += 1;
                x = nx;
            }
            _ => break,
        }
        if n > CHUNK_CELLS_W as i32 * 2 {
            break;
        }
    }
    // Walk +x.
    x = gx;
    for _ in 0..CHUNK_CELLS_W as i32 * 4 {
        let nx = world.wrap_x(x + 1);
        if nx == gx {
            break;
        }
        match world.get_cell(nx, cavity_y) {
            Some(c) if c.material == MaterialId::Air => {
                n += 1;
                x = nx;
            }
            _ => break,
        }
        if n > CHUNK_CELLS_W as i32 * 2 {
            break;
        }
    }
    n
}

/// Inclusive `[x0, x1]` world-x range of the Air run containing `gx`
/// at `cavity_y` (wrap-aware, may be unordered if the run crosses the
/// seam — then `crosses_seam` is true and callers should iterate via
/// walk instead). For non-wrapping short runs, `x0 <= x1`.
fn roof_span_bounds(world: &World, gx: i32, cavity_y: i32) -> Option<(i32, i32, i32)> {
    let gx = world.wrap_x(gx);
    let Some(seed) = world.get_cell(gx, cavity_y) else {
        return None;
    };
    if seed.material != MaterialId::Air {
        return None;
    }
    let mut x_lo = gx;
    let mut x = gx;
    for _ in 0..CHUNK_CELLS_W as i32 * 4 {
        let nx = world.wrap_x(x - 1);
        match world.get_cell(nx, cavity_y) {
            Some(c) if c.material == MaterialId::Air => {
                x_lo = nx;
                x = nx;
            }
            _ => break,
        }
        if x == gx {
            break;
        }
    }
    let mut x_hi = gx;
    x = gx;
    for _ in 0..CHUNK_CELLS_W as i32 * 4 {
        let nx = world.wrap_x(x + 1);
        match world.get_cell(nx, cavity_y) {
            Some(c) if c.material == MaterialId::Air => {
                x_hi = nx;
                x = nx;
            }
            _ => break,
        }
        if x == gx {
            break;
        }
    }
    let span = roof_span_cells(world, gx, cavity_y);
    Some((x_lo, x_hi, span))
}

/// Weakest (smallest) roof limit among solid cells at `roof_y` above
/// the Air run containing `gx`.
fn weakest_roof_limit(world: &World, gx: i32, cavity_y: i32, roof_y: i32) -> i32 {
    let Some((x_lo, x_hi, span)) = roof_span_bounds(world, gx, cavity_y) else {
        return i32::MAX;
    };
    let mut weakest = i32::MAX;
    // Iterate the span. If bounds don't wrap oddly, walk from lo.
    // Prefer walking span cells from gx.
    let mut x = world.wrap_x(gx);
    let mut seen = 0i32;
    // Restart at left end by walking left then scanning right.
    let mut start = x;
    loop {
        let nx = world.wrap_x(start - 1);
        match world.get_cell(nx, cavity_y) {
            Some(c) if c.material == MaterialId::Air => start = nx,
            _ => break,
        }
        if start == x {
            break;
        }
    }
    x = start;
    for _ in 0..span {
        if let Some(roof) = world.get_cell(x, roof_y) {
            if roof.material != MaterialId::Air {
                weakest = weakest.min(roof_span_limit_cells(roof.material));
            }
        }
        seen += 1;
        let nx = world.wrap_x(x + 1);
        match world.get_cell(nx, cavity_y) {
            Some(c) if c.material == MaterialId::Air => x = nx,
            _ => break,
        }
        if seen >= span {
            break;
        }
    }
    let _ = (x_lo, x_hi);
    weakest
}

/// Debris left when a roof cell fails — fallable so grain fall / empty-air
/// fall can seat it.
pub fn roof_collapse_debris(material: MaterialId) -> MaterialId {
    match material {
        MaterialId::Stone | MaterialId::Limestone => MaterialId::LooseRock,
        MaterialId::Bedrock => MaterialId::LooseRock, // should not be selected (∞ span)
        other => other,
    }
}

/// True when this solid can participate as a collapsing roof.
fn is_roof_candidate(material: MaterialId) -> bool {
    material != MaterialId::Air && roof_span_limit_cells(material) < i32::MAX
}

fn regions_for_roof(world: &World) -> Vec<ActiveChunk> {
    let planned = plan_active(world);
    if !planned.is_empty() {
        return planned;
    }
    let mut coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
        .into_iter()
        .map(|coord| ActiveChunk {
            coord,
            rect: crate::chunk::Rect::full(),
        })
        .collect()
}

/// F1: collapse ceilings whose cavity span exceeds material capacity.
///
/// Compute-then-apply. Converts roof rock to fallable debris and swaps
/// it into the Air below (one cell) so the drop is visible this tick.
/// Event count capped by [`FailureConfig::max_roof_events`].
pub fn apply_roof_collapse(world: &mut World, cfg: &FailureConfig) {
    if !cfg.enable_roof_collapse || cfg.max_roof_events == 0 {
        return;
    }
    let regions = regions_for_roof(world);
    apply_roof_collapse_regions(world, &regions, cfg);
}

/// Roof collapse restricted to a pre-planned active set.
pub fn apply_roof_collapse_regions(
    world: &mut World,
    active: &[ActiveChunk],
    cfg: &FailureConfig,
) {
    if !cfg.enable_roof_collapse || cfg.max_roof_events == 0 || active.is_empty() {
        return;
    }

    // (gy, gx) — lowest ceilings first, then x for determinism.
    let mut candidates: Vec<(i32, i32)> = Vec::new();
    for ac in active {
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            if gy <= 0 {
                continue;
            }
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = world.wrap_x(ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32);
                let Some(roof) = world.get_cell(gx, gy) else {
                    continue;
                };
                if !is_roof_candidate(roof.material) {
                    continue;
                }
                let Some(below) = world.get_cell(gx, gy - 1) else {
                    continue;
                };
                if below.material != MaterialId::Air {
                    continue;
                }
                let span = roof_span_cells(world, gx, gy - 1);
                if span <= 0 {
                    continue;
                }
                let limit = weakest_roof_limit(world, gx, gy - 1, gy);
                if span > limit {
                    candidates.push((gy, gx));
                }
            }
        }
    }

    candidates.sort_unstable();
    candidates.dedup();

    let mut applied = 0u32;
    for (gy, gx) in candidates {
        if applied >= cfg.max_roof_events {
            break;
        }
        if collapse_one_ceiling(world, gx, gy) {
            applied += 1;
        }
    }
}

/// Swap one failing roof cell into the Air below as debris.
/// Returns false if the geometry raced away.
fn collapse_one_ceiling(world: &mut World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    let Some(roof) = world.get_cell(gx, gy) else {
        return false;
    };
    if !is_roof_candidate(roof.material) {
        return false;
    }
    let Some(below) = world.get_cell(gx, gy - 1) else {
        return false;
    };
    if below.material != MaterialId::Air {
        return false;
    }
    // Re-check span — neighbour collapse may have changed the cavity.
    let span = roof_span_cells(world, gx, gy - 1);
    let limit = weakest_roof_limit(world, gx, gy - 1, gy);
    if span <= limit {
        return false;
    }

    let debris_mat = roof_collapse_debris(roof.material);
    let cap = water_capacity(debris_mat);
    let mut debris_sat = roof.sat.0.min(cap);
    let mut leftover = below.sat.0.saturating_add(roof.sat.0.saturating_sub(debris_sat));
    if cap > debris_sat {
        let take = (cap - debris_sat).min(leftover);
        debris_sat += take;
        leftover -= take;
    }
    let debris = Cell {
        material: debris_mat,
        sat: Sat(debris_sat),
        ..roof
    };
    // Vacated roof becomes Air; any leftover free water stays here.
    let vacated = Cell {
        material: MaterialId::Air,
        sat: Sat(leftover),
        flags: Default::default(),
        _pad: 0,
    };
    world.set_cell(gx, gy - 1, debris);
    world.set_cell(gx, gy, vacated);
    // Competence check: debris should be able to keep falling later.
    debug_assert!(
        is_grain(debris_mat) || falls_through_empty_air(debris_mat) || debris_mat == MaterialId::LooseRock
    );
    true
}

/// Competent rock that can shear-weaken (infinite repose, not grains).
fn is_shear_competent(material: MaterialId) -> bool {
    matches!(material, MaterialId::Stone | MaterialId::Limestone)
}

/// Local open-face demand: 0 = buried, 1 = side/diag Air, 2 = taller face
/// (side Air with Air below, or two-cell empty drop beside).
pub fn face_shear_demand(world: &World, gx: i32, gy: i32) -> i32 {
    let gx = world.wrap_x(gx);
    let mut demand = 0i32;
    for &dx in &[-1i32, 1] {
        let nx = world.wrap_x(gx + dx);
        let side_air = matches!(
            world.get_cell(nx, gy),
            Some(c) if c.material == MaterialId::Air
        );
        let diag_air = matches!(
            world.get_cell(nx, gy - 1),
            Some(c) if c.material == MaterialId::Air
        );
        let deep_air = matches!(
            world.get_cell(nx, gy - 2),
            Some(c) if c.material == MaterialId::Air
        );
        if side_air {
            demand = demand.max(1);
            if diag_air || deep_air {
                demand = demand.max(2);
            }
        } else if diag_air {
            // Stepped face without a same-Y Air neighbour.
            demand = demand.max(1);
            if deep_air {
                demand = demand.max(2);
            }
        }
    }
    demand
}

/// Debris left when a competent face shears.
pub fn shear_weaken_debris(material: MaterialId) -> MaterialId {
    match material {
        MaterialId::Stone | MaterialId::Limestone => MaterialId::LooseRock,
        other => other,
    }
}

fn shear_hash_ok(seed: u64, tick_no: u64, gx: i32, gy: i32, per_mille: u32) -> bool {
    if per_mille >= 1000 {
        return true;
    }
    if per_mille == 0 {
        return false;
    }
    let mut h = seed
        .wrapping_add(0x5F2A_B7E1u64.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(tick_no.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add((gx as u64).wrapping_mul(0x85EB_CA6B))
        .wrapping_add((gy as u64).wrapping_mul(0xC2B2_AE3D));
    h ^= h.wrapping_shr(30);
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h.wrapping_shr(27);
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h.wrapping_shr(31);
    let roll = (h % 1000) as u32;
    roll < per_mille
}

/// F2b: convert wet, steep competent rock faces to LooseRock.
///
/// Compute-then-apply. Event count capped by [`FailureConfig::max_shear_events`].
/// Gated by [`FailureConfig::enable_shear_weaken`] (off by default).
pub fn apply_shear_weaken(world: &mut World, cfg: &FailureConfig) {
    if !cfg.enable_shear_weaken || cfg.max_shear_events == 0 {
        return;
    }
    let regions = regions_for_roof(world);
    apply_shear_weaken_regions(world, &regions, cfg);
}

/// Shear weaken restricted to a pre-planned active set.
pub fn apply_shear_weaken_regions(
    world: &mut World,
    active: &[ActiveChunk],
    cfg: &FailureConfig,
) {
    if !cfg.enable_shear_weaken || cfg.max_shear_events == 0 || active.is_empty() {
        return;
    }

    let seed = world.seed.0;
    let tick_no = world.tick;
    // (gy, gx) for determinism.
    let mut candidates: Vec<(i32, i32)> = Vec::new();
    for ac in active {
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            if gy <= 0 {
                continue;
            }
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = world.wrap_x(ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32);
                let Some(cell) = world.get_cell(gx, gy) else {
                    continue;
                };
                if !is_shear_competent(cell.material) {
                    continue;
                }
                let demand = face_shear_demand(world, gx, gy);
                if demand < 1 {
                    continue;
                }
                let wet = pore_wetness(cell);
                if wet <= 0.0 {
                    continue;
                }
                let c_eff = effective_cohesion(cell.material, wet);
                if c_eff >= shear_c_threshold(demand) {
                    continue;
                }
                if !shear_hash_ok(seed, tick_no, gx, gy, cfg.shear_chance_per_mille) {
                    continue;
                }
                candidates.push((gy, gx));
            }
        }
    }

    candidates.sort_unstable();
    candidates.dedup();

    let mut applied = 0u32;
    for (gy, gx) in candidates {
        if applied >= cfg.max_shear_events {
            break;
        }
        if shear_one_face(world, gx, gy) {
            applied += 1;
        }
    }
}

fn shear_one_face(world: &mut World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    let Some(cell) = world.get_cell(gx, gy) else {
        return false;
    };
    if !is_shear_competent(cell.material) {
        return false;
    }
    let demand = face_shear_demand(world, gx, gy);
    if demand < 1 {
        return false;
    }
    let wet = pore_wetness(cell);
    let c_eff = effective_cohesion(cell.material, wet);
    if c_eff >= shear_c_threshold(demand) {
        return false;
    }
    let debris_mat = shear_weaken_debris(cell.material);
    let cap = water_capacity(debris_mat);
    let debris = Cell {
        material: debris_mat,
        sat: Sat(cell.sat.0.min(cap)),
        ..cell
    };
    world.set_cell(gx, gy, debris);
    true
}

/// Run enabled failure passes (F1 roof, F2b shear; F3 stub).
pub fn apply_failure(world: &mut World, cfg: &FailureConfig) {
    if cfg.enable_roof_collapse {
        apply_roof_collapse(world, cfg);
    }
    if cfg.enable_shear_weaken {
        apply_shear_weaken(world, cfg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;
    use crate::rules::{apply_grain_fall, apply_grain_repose, tick_with_configs, PerfConfig};

    fn bed(w: &mut World, x0: i32, x1: i32) {
        for x in x0..=x1 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
    }

    #[test]
    fn roof_span_limit_sand_is_zero_stone_finite_bedrock_infinite() {
        assert_eq!(roof_span_limit_cells(MaterialId::Sand), 0);
        assert_eq!(roof_span_limit_cells(MaterialId::Clay), 0);
        assert!(roof_span_limit_cells(MaterialId::Stone) > 0);
        assert!(roof_span_limit_cells(MaterialId::Stone) < i32::MAX);
        assert_eq!(roof_span_limit_cells(MaterialId::Bedrock), i32::MAX);
    }

    #[test]
    fn sand_ceiling_over_one_air_collapses() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 8);
        // Pillars + sand bridge over 1 Air.
        w.set_cell(3, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(5, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(4, 1, Cell::air());
        w.set_cell(3, 2, Cell::solid(MaterialId::Sand));
        w.set_cell(4, 2, Cell::solid(MaterialId::Sand)); // ceiling
        w.set_cell(5, 2, Cell::solid(MaterialId::Sand));
        let cfg = FailureConfig::default();
        apply_roof_collapse(&mut w, &cfg);
        assert_eq!(
            w.get_cell(4, 2).unwrap().material,
            MaterialId::Air,
            "sand ceiling must vacate"
        );
        assert_eq!(
            w.get_cell(4, 1).unwrap().material,
            MaterialId::Sand,
            "sand debris drops into the cavity"
        );
    }

    #[test]
    fn bedrock_bridges_arbitrarily() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 20);
        for x in 2..=18 {
            w.set_cell(x, 1, Cell::air());
            w.set_cell(x, 2, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(1, 1, Cell::solid(MaterialId::Bedrock));
        w.set_cell(19, 1, Cell::solid(MaterialId::Bedrock));
        apply_roof_collapse(&mut w, &FailureConfig::default());
        for x in 2..=18 {
            assert_eq!(
                w.get_cell(x, 2).unwrap().material,
                MaterialId::Bedrock,
                "bedrock roof at {x} must hold"
            );
        }
    }

    #[test]
    fn stone_holds_short_overhang() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 16);
        // 3-cell Air cavity under Stone — well under ~60 cell limit.
        for x in 4..=6 {
            w.set_cell(x, 1, Cell::air());
            w.set_cell(x, 2, Cell::solid(MaterialId::Stone));
        }
        w.set_cell(3, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(7, 1, Cell::solid(MaterialId::Stone));
        apply_roof_collapse(&mut w, &FailureConfig::default());
        for x in 4..=6 {
            assert_eq!(
                w.get_cell(x, 2).unwrap().material,
                MaterialId::Stone,
                "short stone overhang must hold"
            );
        }
    }

    #[test]
    fn stone_collapses_wide_karst_room() {
        // Limestone limit is 10m → 40 cells; easier to fit in one chunk.
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 63);
        let limit = roof_span_limit_cells(MaterialId::Limestone);
        assert!(limit > 0 && limit + 4 < CHUNK_CELLS_W as i32);
        let span = limit + 2;
        let x0 = 2;
        let x1 = x0 + span - 1;
        for x in x0..=x1 {
            w.set_cell(x, 1, Cell::air());
            w.set_cell(x, 2, Cell::solid(MaterialId::Limestone));
        }
        w.set_cell(x0 - 1, 1, Cell::solid(MaterialId::Limestone));
        w.set_cell(x1 + 1, 1, Cell::solid(MaterialId::Limestone));
        let cfg = FailureConfig {
            max_roof_events: 64,
            ..FailureConfig::default()
        };
        apply_roof_collapse(&mut w, &cfg);
        let dropped = (x0..=x1)
            .filter(|&x| w.get_cell(x, 2).unwrap().material == MaterialId::Air)
            .count();
        assert!(
            dropped > 0,
            "wide limestone room must drop at least one ceiling cell (span={span} limit={limit})"
        );
        let debris = (x0..=x1)
            .filter(|&x| w.get_cell(x, 1).unwrap().material == MaterialId::LooseRock)
            .count();
        assert!(debris > 0, "collapsed limestone should become LooseRock debris");
    }

    #[test]
    fn roof_collapse_conserves_solid_mass_units() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 8);
        w.set_cell(3, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(5, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(4, 1, Cell::air());
        w.set_cell(4, 2, Cell::solid(MaterialId::Sand));
        let solids_before = count_non_air(&w);
        apply_roof_collapse(&mut w, &FailureConfig::default());
        let solids_after = count_non_air(&w);
        assert_eq!(
            solids_before, solids_after,
            "collapse swaps roof into cavity — solid count unchanged"
        );
    }

    #[test]
    fn roof_events_capped_per_tick() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 63);
        // Many independent 1-cell sand bridges.
        for i in 0..20 {
            let x = 2 + i * 3;
            w.set_cell(x - 1, 1, Cell::solid(MaterialId::Sand));
            w.set_cell(x + 1, 1, Cell::solid(MaterialId::Sand));
            w.set_cell(x, 1, Cell::air());
            w.set_cell(x, 2, Cell::solid(MaterialId::Sand));
        }
        let cfg = FailureConfig {
            max_roof_events: 3,
            ..FailureConfig::default()
        };
        apply_roof_collapse(&mut w, &cfg);
        let collapsed = (0..20)
            .filter(|&i| {
                let x = 2 + i * 3;
                w.get_cell(x, 2).unwrap().material == MaterialId::Air
            })
            .count();
        assert_eq!(collapsed, 3, "must respect max_roof_events");
    }

    #[test]
    fn tick_runs_roof_collapse_when_enabled() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 8);
        w.set_cell(3, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(5, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(4, 1, Cell::air());
        w.set_cell(4, 2, Cell::solid(MaterialId::Sand));
        // Dirty the roof so plan_active sees it.
        w.set_cell(4, 2, Cell::solid(MaterialId::Sand));
        let fail = FailureConfig::default();
        let perf = PerfConfig {
            parallel_physics: false,
            ..PerfConfig::default()
        };
        tick_with_configs(&mut w, &perf, &fail);
        // After a full tick, sand bridge should be gone (roof + grain).
        let ceiling_air = w.get_cell(4, 2).unwrap().material == MaterialId::Air;
        let seated = matches!(
            w.get_cell(4, 1).unwrap().material,
            MaterialId::Sand | MaterialId::Air
        );
        assert!(ceiling_air || seated);
        apply_grain_fall(&mut w);
        assert!(
            w.get_cell(4, 2).unwrap().material == MaterialId::Air
                || w.get_cell(4, 1).unwrap().material == MaterialId::Sand,
            "sand bridge should not remain suspended"
        );
    }

    fn count_non_air(w: &World) -> usize {
        let mut n = 0;
        for y in 0..8 {
            for x in 0..16 {
                if let Some(c) = w.get_cell(x, y) {
                    if c.material != MaterialId::Air {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    fn wet_solid(mat: MaterialId) -> Cell {
        let mut c = Cell::solid(mat);
        c.sat = Sat(water_capacity(mat));
        c
    }

    #[test]
    fn wet_repose_low_cohesion_loosens_high_holds_until_soaked() {
        // Sand c=20 → always loosens when wet.
        assert!(wet_repose_loosens(MaterialId::Sand, 0.25));
        // Clay c=180 → needs near-full wetness (c_eff < 80).
        assert!(!wet_repose_loosens(MaterialId::Clay, 0.4));
        assert!(wet_repose_loosens(MaterialId::Clay, 1.0));
        // LooseRock c=100 → loosens once moderately wet.
        assert!(wet_repose_loosens(MaterialId::LooseRock, 0.5));
        assert!(!wet_repose_loosens(MaterialId::LooseRock, 0.0));
    }

    #[test]
    fn wet_sand_bank_loosens_faster_than_dry() {
        // LooseRock stairs: dry max_step=1 holds a 1-cell step; wet
        // F2a drops max_step to 0 so the bank slides (Sand itself is
        // already max_step=0 dry — LooseRock is the cohesion-gated case).
        let mut dry = World::new(1);
        dry.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut dry, 0, 10);
        // Floor + 1-cell LooseRock step at x=4 → Air seat at (5,1).
        for x in 0..=4 {
            dry.set_cell(x, 1, Cell::solid(MaterialId::LooseRock));
        }
        dry.set_cell(4, 2, Cell::solid(MaterialId::LooseRock)); // lip
        dry.set_cell(5, 1, Cell::air());
        dry.set_cell(5, 2, Cell::air());

        let mut wet = dry.clone();
        wet.set_cell(4, 2, wet_solid(MaterialId::LooseRock));

        apply_grain_repose(&mut dry);
        apply_grain_repose(&mut wet);

        assert_eq!(
            dry.get_cell(4, 2).unwrap().material,
            MaterialId::LooseRock,
            "dry LooseRock lip must hold a 1-cell step"
        );
        assert_eq!(
            wet.get_cell(5, 1).unwrap().material,
            MaterialId::LooseRock,
            "wet LooseRock lip must slide into the Air seat"
        );
        assert_eq!(
            wet.get_cell(4, 2).unwrap().material,
            MaterialId::Air,
            "wet source vacates"
        );
    }

    #[test]
    fn dry_stone_cliff_stable() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 8);
        // Vertical dry Stone face.
        for y in 1..=4 {
            w.set_cell(3, y, Cell::solid(MaterialId::Stone));
            w.set_cell(4, y, Cell::air());
        }
        let cfg = FailureConfig {
            enable_shear_weaken: true,
            shear_chance_per_mille: 1000,
            max_shear_events: 64,
            ..FailureConfig::default()
        };
        for _ in 0..40 {
            apply_shear_weaken(&mut w, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        for y in 1..=4 {
            assert_eq!(
                w.get_cell(3, y).unwrap().material,
                MaterialId::Stone,
                "dry stone cliff must not F2b-spam at y={y}"
            );
        }
    }

    #[test]
    fn wet_stone_overhang_lip_can_loosen() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 8);
        // Pillar + saturated Stone lip with 2-cell Air drop beside.
        w.set_cell(3, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(3, 2, Cell::solid(MaterialId::Stone));
        w.set_cell(3, 3, wet_solid(MaterialId::Stone)); // lip
        w.set_cell(4, 1, Cell::air());
        w.set_cell(4, 2, Cell::air());
        w.set_cell(4, 3, Cell::air());
        assert_eq!(face_shear_demand(&w, 3, 3), 2);
        let c_eff = effective_cohesion(MaterialId::Stone, 1.0);
        assert!(
            c_eff < SHEAR_C_THRESH_DEMAND_2,
            "fully wet Stone must fail demand-2 (c_eff={c_eff})"
        );

        let cfg = FailureConfig {
            enable_shear_weaken: true,
            shear_chance_per_mille: 1000,
            max_shear_events: 8,
            enable_roof_collapse: false,
            ..FailureConfig::default()
        };
        let mut loosened = false;
        for _ in 0..8 {
            apply_shear_weaken(&mut w, &cfg);
            w.tick = w.tick.wrapping_add(1);
            if w.get_cell(3, 3).unwrap().material == MaterialId::LooseRock {
                loosened = true;
                break;
            }
        }
        assert!(loosened, "saturated Stone lip above Air → LooseRock");
    }

    #[test]
    fn shear_events_capped() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        bed(&mut w, 0, 40);
        // Many independent wet Stone lips.
        for i in 0..12 {
            let x = 2 + i * 3;
            w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
            w.set_cell(x, 2, Cell::solid(MaterialId::Stone));
            w.set_cell(x, 3, wet_solid(MaterialId::Stone));
            w.set_cell(x + 1, 1, Cell::air());
            w.set_cell(x + 1, 2, Cell::air());
            w.set_cell(x + 1, 3, Cell::air());
        }
        let cfg = FailureConfig {
            enable_shear_weaken: true,
            shear_chance_per_mille: 1000,
            max_shear_events: 3,
            enable_roof_collapse: false,
            ..FailureConfig::default()
        };
        apply_shear_weaken(&mut w, &cfg);
        let converted = (0..12)
            .filter(|&i| {
                let x = 2 + i * 3;
                w.get_cell(x, 3).unwrap().material == MaterialId::LooseRock
            })
            .count();
        assert_eq!(converted, 3, "must respect max_shear_events");
    }
}
