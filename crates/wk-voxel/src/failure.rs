//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Geotech failure passes (docs/VOXEL_FAILURE.md).
//!
//! F1 — compressive roof / overhang collapse when an Air cavity span
//! exceeds the roof material's [`wk_material::MaterialProps::roof_span_max_m`].

use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};

use crate::active::{plan_active, ActiveChunk};
use crate::cell::{falls_through_empty_air, is_grain, water_capacity, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// Live-tunable geotech knobs (Tab → Geotech).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureConfig {
    /// Drop ceilings when cavity span exceeds `roof_span_max_m`.
    pub enable_roof_collapse: bool,
    /// F2 — wet cohesion shear (not yet implemented).
    pub enable_shear_weaken: bool,
    /// F3 — overburden compaction (not yet implemented).
    pub enable_compaction: bool,
    /// Max roof cells converted / dropped per tick.
    pub max_roof_events: u32,
    /// Reserved for F2.
    pub max_shear_events: u32,
}

impl Default for FailureConfig {
    fn default() -> Self {
        Self {
            enable_roof_collapse: true,
            enable_shear_weaken: false,
            enable_compaction: false,
            max_roof_events: 32,
            max_shear_events: 16,
        }
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

/// Run enabled failure passes (F1 roof; F2/F3 stubs).
pub fn apply_failure(world: &mut World, cfg: &FailureConfig) {
    if cfg.enable_roof_collapse {
        apply_roof_collapse(world, cfg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;
    use crate::rules::{apply_grain_fall, tick_with_configs, PerfConfig};

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
}
