//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Cheap column canopy shade (Set D light competition, lite).
//! Spec: `docs/organism/LIGHT.md`. Sparse `wx → canopy` map from living
//! plants, then O(1) neighbour sample per plant.

use std::collections::HashMap;

use crate::blueprint::Genome;
use crate::organism::{Atom, ModuleId};
use crate::plant::is_land_plant;

/// Columns left/right that can cast shade onto a plant.
pub const SHADE_RADIUS: i32 = 3;
/// Floor transmit so deep shade is dim, not black.
pub const SHADE_AMBIENT_FLOOR: f32 = 0.08;
/// Per-photosystem contribution to cast strength (× `leaf_absorb`).
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
    pub top_y: i32,
    /// 0..[`SHADE_CAST_CAP`] attenuation strength cast sideways.
    pub absorb: f32,
    pub n_photo: u16,
    pub entity_id: u32,
}

/// Sparse canopy index: world column → shade caster.
pub type CanopyIndex = HashMap<i32, CanopyColumn>;

/// World-Y of the highest Stem / Photosystem on this body.
pub fn canopy_top_y(atom: &Atom) -> i32 {
    let mut max_y = i32::MIN;
    for &(dx, dy, mid) in &atom.body {
        let _ = dx;
        if matches!(mid, ModuleId::Photosystem | ModuleId::Stem) {
            max_y = max_y.max(atom.gy + dy as i32);
        }
    }
    if max_y == i32::MIN {
        atom.gy
    } else {
        max_y
    }
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
    top_y: i32,
    absorb: f32,
    n_photo: usize,
    entity_id: u32,
) {
    match index.get_mut(&wx) {
        Some(c) => {
            if top_y > c.top_y || (top_y == c.top_y && absorb > c.absorb) {
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

/// Build canopy index from all living land plants in the store.
pub fn build_canopy_index(atoms: &[Atom]) -> CanopyIndex {
    let mut index = CanopyIndex::default();
    for (id, atom) in atoms.iter().enumerate() {
        if !is_land_plant(atom) {
            continue;
        }
        let n_photo = atom.photosystem_count();
        let n_stem = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Stem)
            .count();
        if n_photo == 0 && n_stem == 0 {
            continue;
        }
        let absorb = cast_strength(n_photo, n_stem, atom.leaf_absorb_effective());
        record_canopy(
            &mut index,
            atom.gx,
            canopy_top_y(atom),
            absorb,
            n_photo,
            id as u32,
        );
    }
    index
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
    sample_y: i32,
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
        if c.top_y <= sample_y {
            continue;
        }
        let rise = ((c.top_y - sample_y) as f32 / 2.5).clamp(0.0, 1.0);
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
pub fn shade_harvest_light(light: f32, shade_eff: f32) -> f32 {
    let l = light.clamp(0.0, 1.0);
    let se = shade_eff.clamp(0.0, 1.0);
    let sun = l;
    let under = (l / (l + 0.22)) * (0.62 + 0.38 * l);
    sun * (1.0 - se) + under * se
}

/// Effective light for photosynthesis after canopy shade + gene remap.
pub fn effective_photo_light(
    index: &CanopyIndex,
    wx: i32,
    sample_y: i32,
    sky_l0: f32,
    self_entity: u32,
    self_n_photo: usize,
    genome: &Genome,
) -> f32 {
    effective_photo_light_absorb(
        index,
        wx,
        sample_y,
        sky_l0,
        self_entity,
        self_n_photo,
        genome.leaf_absorb,
        genome.shade_efficiency,
    )
}

/// Like [`effective_photo_light`] with explicit absorb (Wave M body plan).
pub fn effective_photo_light_absorb(
    index: &CanopyIndex,
    wx: i32,
    sample_y: i32,
    sky_l0: f32,
    self_entity: u32,
    self_n_photo: usize,
    leaf_absorb: f32,
    shade_efficiency: f32,
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
        leaf_absorb,
    );
    let attenuated = (sky_l0 * transmit).clamp(0.0, 1.0);
    shade_harvest_light(attenuated, shade_efficiency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::Genome;
    use crate::organism::{Atom, ModuleId};

    fn plant_at(gx: i32, gy: i32, stems: i16, photos: i16, genome: Genome) -> Atom {
        let mut body = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
        ];
        for s in 1..=stems {
            body.push((0, s, ModuleId::Stem));
        }
        let top = stems.max(0);
        for p in 0..photos {
            body.push((p, top + 1, ModuleId::Photosystem));
        }
        let mut a = Atom::from_body(gx, gy, 40.0, body);
        a.genome = genome;
        a
    }

    #[test]
    fn taller_neighbour_reduces_short_plant_light() {
        let mut tall_g = Genome::default();
        tall_g.leaf_absorb = 0.95;
        tall_g.shade_efficiency = 0.05;
        let mut short_g = Genome::default();
        short_g.leaf_absorb = 0.3;
        short_g.shade_efficiency = 0.15;

        // Short crown at y=4, tall crown at y=4 with much taller canopy.
        let short = plant_at(5, 4, 1, 2, short_g);
        let tall = plant_at(6, 4, 6, 3, tall_g);
        let atoms = [short, tall];
        let index = build_canopy_index(&atoms);
        let sample_y = canopy_top_y(&atoms[0]);
        let lit = effective_photo_light(&index, 5, sample_y, 1.0, 0, 2, &short_g);
        assert!(
            lit < 0.85,
            "tall neighbour should shade short plant (lit={lit})"
        );
        let open = shade_harvest_light(1.0, short_g.shade_efficiency);
        assert!(lit < open * 0.95);
    }

    #[test]
    fn understory_gene_beats_sun_thug_in_dim_light() {
        let dim = 0.15f32;
        let sun_thug = shade_harvest_light(dim, 0.05);
        let understory = shade_harvest_light(dim, 0.95);
        assert!(
            understory > sun_thug,
            "high shade_efficiency should harvest better in dim light"
        );
    }
}
