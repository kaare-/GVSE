//! Cheap column canopy shade (Set D light competition, lite).
//!
//! Full [`docs/organism/LIGHT.md`](../../../docs/organism/LIGHT.md) wants a
//! per-pixel top-down scan every tick. This module instead:
//!
//! 1. Builds a sparse `wx → canopy` map from living photosynthesizers (**O(n)**).
//! 2. Samples neighbour columns within [`SHADE_RADIUS`] (**O(1)** per plant).
//!
//! Taller neighbours attenuate sky light; stacked own leaves self-shade;
//! [`Genome::shade_efficiency`] remaps the resulting `L` (sun vs understory).

use std::collections::HashMap;

use crate::blueprint::Blueprint;
use crate::module::ModuleId;
use crate::organism::MODULE_CELL_COLS;
use crate::Genome;

/// Columns left/right that can cast shade onto a plant.
pub const SHADE_RADIUS: i32 = 3;
/// Floor transmit so deep shade is dim, not black (no bounce light sim).
pub const SHADE_AMBIENT_FLOOR: f32 = 0.08;
/// Per-photosystem contribution to cast strength (scaled by `leaf_absorb`).
pub const SHADE_PER_LEAF: f32 = 0.16;
/// Per-stem contribution to cast strength (olive attenuation).
pub const SHADE_PER_STEM: f32 = 0.07;
/// Soft cap on how hard one column can shade its neighbours.
pub const SHADE_CAST_CAP: f32 = 0.88;
/// Self-shade per extra photosystem above the first (× `leaf_absorb`).
pub const SHADE_SELF_PER_EXTRA_LEAF: f32 = 0.10;

/// One column's shade-casting canopy (tallest / strongest plant wins).
#[derive(Debug, Clone, Copy)]
pub struct CanopyColumn {
    /// World Y of the highest Photosystem or Stem module.
    pub top_y: f32,
    /// 0..[`SHADE_CAST_CAP`] attenuation strength cast sideways.
    pub absorb: f32,
    pub n_photo: u16,
    pub entity_id: u32,
}

/// Sparse canopy index: world column → shade caster.
pub type CanopyIndex = HashMap<i32, CanopyColumn>;

/// World-Y of the highest Stem / Photosystem on this body (shade + harvest height).
pub fn canopy_top_y(pose_y: f32, blueprint: &Blueprint) -> f32 {
    let mut max_y = i16::MIN;
    for m in &blueprint.modules {
        if matches!(m.module, ModuleId::Photosystem | ModuleId::Stem) {
            max_y = max_y.max(m.y);
        }
    }
    if max_y == i16::MIN {
        return pose_y;
    }
    pose_y + max_y as f32 * MODULE_CELL_COLS
}

/// How hard this plant shades neighbours (0..[`SHADE_CAST_CAP`]).
pub fn cast_strength(n_photo: usize, n_stem: usize, leaf_absorb: f32) -> f32 {
    let a = leaf_absorb.clamp(0.0, 1.0);
    let raw = a * n_photo as f32 * SHADE_PER_LEAF + n_stem as f32 * SHADE_PER_STEM;
    raw.clamp(0.0, SHADE_CAST_CAP)
}

/// Insert / reinforce a column's canopy entry (keep taller, else stronger).
pub fn record_canopy(
    index: &mut CanopyIndex,
    wx: i32,
    top_y: f32,
    absorb: f32,
    n_photo: usize,
    entity_id: u32,
) {
    match index.get_mut(&wx) {
        Some(c) => {
            if top_y > c.top_y + 1e-3 || (top_y >= c.top_y - 1e-3 && absorb > c.absorb) {
                *c = CanopyColumn {
                    top_y,
                    absorb,
                    n_photo: n_photo.min(u16::MAX as usize) as u16,
                    entity_id,
                };
            }
        }
        None => {
            index.insert(
                wx,
                CanopyColumn {
                    top_y,
                    absorb,
                    n_photo: n_photo.min(u16::MAX as usize) as u16,
                    entity_id,
                },
            );
        }
    }
}

fn neighbour_weight(dx: i32) -> f32 {
    match dx.abs() {
        1 => 0.55,
        2 => 0.30,
        3 => 0.16,
        _ => 0.0,
    }
}

/// Sky-light transmit through taller neighbours + own leaf stack (`0..1`).
pub fn shade_transmit(
    index: &CanopyIndex,
    wx: i32,
    sample_y: f32,
    self_entity: u32,
    self_n_photo: usize,
    self_leaf_absorb: f32,
) -> f32 {
    let mut transmit = 1.0f32;
    for dx in -SHADE_RADIUS..=SHADE_RADIUS {
        if dx == 0 {
            continue;
        }
        let Some(c) = index.get(&(wx + dx)) else {
            continue;
        };
        if c.entity_id == self_entity {
            continue;
        }
        // Only columns that rise above our harvest height cast shade.
        if c.top_y <= sample_y + 0.05 {
            continue;
        }
        let rise = ((c.top_y - sample_y) / 2.5).clamp(0.0, 1.0);
        let w = neighbour_weight(dx);
        transmit *= (1.0 - c.absorb * w * (0.40 + 0.60 * rise)).clamp(0.0, 1.0);
    }
    if self_n_photo > 1 {
        let stack = (self_n_photo - 1) as f32
            * self_leaf_absorb.clamp(0.0, 1.0)
            * SHADE_SELF_PER_EXTRA_LEAF;
        transmit *= (1.0 - stack.clamp(0.0, 0.55)).clamp(0.0, 1.0);
    }
    transmit.clamp(SHADE_AMBIENT_FLOOR, 1.0)
}

/// Remap attenuated light through `ShadeEfficiency` (sun thug vs understory).
///
/// - `shade_eff ≈ 0`: harvest tracks `L` (full-sun specialist).
/// - `shade_eff ≈ 1`: Michaelis dim curve with a lower sun peak.
pub fn shade_harvest_light(light: f32, shade_eff: f32) -> f32 {
    let l = light.clamp(0.0, 1.0);
    let se = shade_eff.clamp(0.0, 1.0);
    let sun = l;
    // Half-sat keeps scraps usable; ×(0.62+0.38L) softens the noon peak.
    let under = (l / (l + 0.22)) * (0.62 + 0.38 * l);
    sun * (1.0 - se) + under * se
}

/// Effective `l0` for photosynthesis after canopy shade + gene remap.
pub fn effective_photo_light(
    index: &CanopyIndex,
    wx: i32,
    sample_y: f32,
    sky_l0: f32,
    self_entity: u32,
    self_n_photo: usize,
    genome: &Genome,
) -> f32 {
    if sky_l0 <= 0.01 {
        return 0.0;
    }
    let transmit = shade_transmit(
        index,
        wx,
        sample_y,
        self_entity,
        self_n_photo,
        genome.leaf_absorb,
    );
    let attenuated = (sky_l0 * transmit).clamp(0.0, 1.0);
    shade_harvest_light(attenuated, genome.shade_efficiency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::{Blueprint, PlacedModule};
    use crate::module::{LaneId, ModuleId};

    fn tall_plant(genome: Genome) -> Blueprint {
        let mut bp = Blueprint::minimal_plant(genome);
        // Extra stem + high leaf so canopy clears a short neighbour.
        bp.modules.push(PlacedModule {
            x: 0,
            y: 4,
            lane: LaneId::Mid,
            module: ModuleId::Stem,
        });
        bp.modules.push(PlacedModule {
            x: 0,
            y: 5,
            lane: LaneId::Mid,
            module: ModuleId::Photosystem,
        });
        bp
    }

    #[test]
    fn taller_neighbour_reduces_transmit() {
        let mut idx = CanopyIndex::default();
        record_canopy(&mut idx, 10, 12.0, 0.7, 4, 1);
        let open = shade_transmit(&idx, 20, 8.0, 2, 1, 0.5);
        let shaded = shade_transmit(&idx, 11, 8.0, 2, 1, 0.5);
        assert!(open > 0.95, "far column stays open (got {open})");
        assert!(
            shaded < open - 0.15,
            "adjacent taller canopy must cut light (open={open} shaded={shaded})"
        );
    }

    #[test]
    fn shorter_neighbour_does_not_shade() {
        let mut idx = CanopyIndex::default();
        record_canopy(&mut idx, 10, 7.0, 0.8, 4, 1);
        let t = shade_transmit(&idx, 11, 9.0, 2, 1, 0.5);
        assert!(t > 0.95, "shorter neighbour must not shade (got {t})");
    }

    #[test]
    fn shade_efficiency_helps_in_dim_light() {
        let dim = 0.15f32;
        let sun_spec = shade_harvest_light(dim, 0.0);
        let shade_spec = shade_harvest_light(dim, 1.0);
        assert!(
            shade_spec > sun_spec,
            "understory gene should harvest more in dim light ({shade_spec} vs {sun_spec})"
        );
        let noon = 1.0f32;
        assert!(
            shade_harvest_light(noon, 0.0) >= shade_harvest_light(noon, 1.0),
            "sun specialist should match or beat understory at full sun"
        );
    }

    #[test]
    fn canopy_top_tracks_highest_leaf() {
        let bp = tall_plant(Genome::default());
        let top = canopy_top_y(10.0, &bp);
        assert!((top - (10.0 + 5.0 * MODULE_CELL_COLS)).abs() < 1e-3);
    }

    #[test]
    fn high_leaf_absorb_casts_harder() {
        let soft = cast_strength(3, 2, 0.2);
        let hard = cast_strength(3, 2, 0.9);
        assert!(hard > soft + 0.2);
    }
}
