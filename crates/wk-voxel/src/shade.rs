//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Cheap column canopy shade (Set D light competition, lite).
//! Spec: `docs/organism/LIGHT.md`. Sparse `wx → canopy` map from living
//! plants, then O(1) neighbour sample per plant.

use std::collections::HashMap;

use crate::blueprint::Genome;
use crate::organism::{is_fallen_log, Atom, Corpse, ModuleId};
use crate::plant::{is_epiphyte, is_land_plant};

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
/// Standing-dead olive cast vs living (Wave AF — grey trunks still shade).
pub const STANDING_DEAD_CAST_SCALE: f32 = 0.55;
/// Ticks a canopy-gap flash lingers after topple (Wave AF / PLANTS.md).
pub const GAP_FLASH_TICKS: u32 = 90;
/// Peak light multiplier bonus at a flashing column (fades with remaining ticks).
pub const GAP_FLASH_LIGHT_BONUS: f32 = 0.35;
/// Floor-fungus energy sip from a full gap flash (Wave AF).
pub const GAP_FLASH_FUNGUS_ENERGY: f32 = 0.10;

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
    build_canopy_index_full(atoms, &[])
}

/// Living canopy plus standing-dead Stem shade (Wave AF). Fallen logs do not cast.
pub fn build_canopy_index_full(atoms: &[Atom], corpses: &[Corpse]) -> CanopyIndex {
    let mut index = CanopyIndex::default();
    for (id, atom) in atoms.iter().enumerate() {
        if !is_land_plant(atom) && !is_epiphyte(atom) {
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
    record_standing_dead_canopy(&mut index, corpses);
    index
}

/// World-Y of the highest Stem on a grey corpse (leaves already stripped).
pub fn corpse_canopy_top_y(corpse: &Corpse) -> i32 {
    let mut max_y = i32::MIN;
    for &(dx, dy, mid) in &corpse.body {
        let _ = dx;
        if mid == ModuleId::Stem {
            max_y = max_y.max(corpse.gy + dy as i32);
        }
    }
    if max_y == i32::MIN {
        corpse.gy
    } else {
        max_y
    }
}

/// Standing-dead trunks cast stem-only shade until they topple (Wave AF).
pub fn record_standing_dead_canopy(index: &mut CanopyIndex, corpses: &[Corpse]) {
    for (i, corpse) in corpses.iter().enumerate() {
        if !corpse.land || is_fallen_log(corpse) {
            continue;
        }
        let n_stem = corpse
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Stem)
            .count();
        if n_stem == 0 {
            continue;
        }
        let absorb = (n_stem as f32 * SHADE_PER_STEM * STANDING_DEAD_CAST_SCALE)
            .clamp(0.0, SHADE_CAST_CAP);
        // Unique non-living entity ids so plants never treat a corpse as self.
        let entity_id = u32::MAX.saturating_sub(i as u32);
        record_canopy(
            index,
            corpse.gx,
            corpse_canopy_top_y(corpse),
            absorb,
            0,
            entity_id,
        );
    }
}

/// Register / refresh gap-flash columns after a topple (Wave AF).
pub fn register_gap_flash(gap_flash: &mut HashMap<i32, u32>, columns: impl IntoIterator<Item = i32>) {
    for wx in columns {
        let entry = gap_flash.entry(wx).or_insert(0);
        *entry = (*entry).max(GAP_FLASH_TICKS);
    }
}

/// Decay gap-flash timers; drop spent columns.
pub fn tick_gap_flash(gap_flash: &mut HashMap<i32, u32>) {
    gap_flash.retain(|_, ticks| {
        *ticks = ticks.saturating_sub(1);
        *ticks > 0
    });
}

/// Light multiplier from nearby canopy-gap flash (`1.0` = none).
pub fn gap_flash_transmit(gap_flash: &HashMap<i32, u32>, wx: i32) -> f32 {
    let mut best = 0u32;
    for dx in -SHADE_RADIUS..=SHADE_RADIUS {
        if let Some(&t) = gap_flash.get(&(wx + dx)) {
            best = best.max(t);
        }
    }
    if best == 0 {
        return 1.0;
    }
    let frac = (best as f32 / GAP_FLASH_TICKS as f32).clamp(0.0, 1.0);
    1.0 + GAP_FLASH_LIGHT_BONUS * frac
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

/// Same-column epiphyte steal for modules below `sample_y` (Wave Y).
///
/// `HostLeaveFraction` leaves light for the host: steal scales with
/// `(1 − leave) × cast_strength`. Gentle riders (`leave → 1`) barely
/// shade the landlord; smotherers (`leave = 0`) take hard.
pub fn epiphyte_rider_transmit(atoms: &[Atom], wx: i32, sample_y: i32, self_entity: u32) -> f32 {
    let mut transmit = 1.0f32;
    for (id, atom) in atoms.iter().enumerate() {
        if id as u32 == self_entity {
            continue;
        }
        if !is_epiphyte(atom) {
            continue;
        }
        if atom.gx != wx {
            continue;
        }
        // Shade host / understory at or below the rider canopy (equal height
        // still steals — freeloader greens share the crown cell).
        if canopy_top_y(atom) < sample_y {
            continue;
        }
        let leave = atom.body_plan.host_leave_fraction.clamp(0.0, 1.0);
        let n_photo = atom.photosystem_count();
        let steal = (1.0 - leave)
            * cast_strength(n_photo, 0, atom.leaf_absorb_effective())
            * 0.90;
        transmit *= (1.0 - steal.clamp(0.0, 0.92)).clamp(0.0, 1.0);
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
    rider_transmit: f32,
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
        rider_transmit,
    )
}

/// Like [`effective_photo_light`] with explicit absorb (Wave M body plan).
///
/// `rider_transmit` is the Wave Y same-column epiphyte factor
/// ([`epiphyte_rider_transmit`]); pass `1.0` when no riders apply.
pub fn effective_photo_light_absorb(
    index: &CanopyIndex,
    wx: i32,
    sample_y: i32,
    sky_l0: f32,
    self_entity: u32,
    self_n_photo: usize,
    leaf_absorb: f32,
    shade_efficiency: f32,
    rider_transmit: f32,
) -> f32 {
    if sky_l0 <= 0.01 {
        return 0.0;
    }
    let mut transmit = shade_transmit(
        index,
        wx,
        sample_y,
        self_entity,
        self_n_photo,
        leaf_absorb,
    );
    transmit *= rider_transmit.clamp(SHADE_AMBIENT_FLOOR, 1.0);
    transmit = transmit.clamp(SHADE_AMBIENT_FLOOR, 1.0);
    let attenuated = (sky_l0 * transmit).clamp(0.0, 1.0);
    shade_harvest_light(attenuated, shade_efficiency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::Genome;
    use crate::organism::{Atom, ModuleId};
    use crate::plant::apply_genome;

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
        apply_genome(&mut a, genome);
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
        let lit = effective_photo_light(&index, 5, sample_y, 1.0, 0, 2, &short_g, 1.0);
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

    #[test]
    fn standing_dead_stems_cast_shade() {
        let short = plant_at(5, 2, 1, 2, Genome::default());
        let tall_live = plant_at(6, 2, 4, 2, Genome::default());
        let corpse = Corpse::from_atom(&tall_live);
        assert!(corpse.body.iter().any(|(_, _, m)| *m == ModuleId::Stem));
        let open = build_canopy_index_full(&[short.clone()], &[]);
        let shaded = build_canopy_index_full(&[short.clone()], &[corpse]);
        let sample_y = canopy_top_y(&short);
        let lit_open = effective_photo_light(&open, 5, sample_y, 1.0, 0, 2, &Genome::default(), 1.0);
        let lit_dead =
            effective_photo_light(&shaded, 5, sample_y, 1.0, 0, 2, &Genome::default(), 1.0);
        assert!(
            lit_dead < lit_open,
            "standing-dead neighbour should shade (dead={lit_dead} open={lit_open})"
        );
    }

    #[test]
    fn gap_flash_boosts_then_fades() {
        let mut flash = HashMap::new();
        register_gap_flash(&mut flash, [6]);
        let near = gap_flash_transmit(&flash, 5);
        let at = gap_flash_transmit(&flash, 6);
        assert!(at > 1.0 && (at - near).abs() < 1e-5, "radius shares full flash");
        assert_eq!(gap_flash_transmit(&flash, 6 + SHADE_RADIUS + 1), 1.0);
        for _ in 0..GAP_FLASH_TICKS {
            tick_gap_flash(&mut flash);
        }
        assert!(flash.is_empty());
        assert_eq!(gap_flash_transmit(&flash, 6), 1.0);
    }

    #[test]
    fn smother_epiphyte_shades_host_more_than_gentle() {
        let host_body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
            (0, 3, ModuleId::Photosystem),
        ];
        let host = Atom::from_body(8, 2, 80.0, host_body);
        let epi_body = vec![
            (0, 0, ModuleId::Holdfast),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Photosystem),
        ];
        let mut smother_g = Genome::default();
        smother_g.leaf_absorb = 0.95;
        smother_g.host_leave_fraction = 0.0;
        let mut gentle_g = Genome::default();
        gentle_g.leaf_absorb = 0.95;
        gentle_g.host_leave_fraction = 0.85;

        // Holdfast on upper stem (y=5); epi leaf at y=6 matches host crown.
        let mut smother = Atom::from_body(8, 5, 40.0, epi_body.clone());
        apply_genome(&mut smother, smother_g);
        let mut gentle = Atom::from_body(8, 5, 40.0, epi_body);
        apply_genome(&mut gentle, gentle_g);

        let smother_atoms = [host.clone(), smother];
        let gentle_atoms = [host.clone(), gentle];
        let idx_s = build_canopy_index(&smother_atoms);
        let idx_g = build_canopy_index(&gentle_atoms);
        let sample_y = canopy_top_y(&host);
        let rt_s = epiphyte_rider_transmit(&smother_atoms, 8, sample_y, 0);
        let rt_g = epiphyte_rider_transmit(&gentle_atoms, 8, sample_y, 0);
        let lit_s = effective_photo_light_absorb(
            &idx_s,
            8,
            sample_y,
            1.0,
            0,
            host.photosystem_count(),
            host.leaf_absorb_effective(),
            host.body_plan.shade_efficiency,
            rt_s,
        );
        let lit_g = effective_photo_light_absorb(
            &idx_g,
            8,
            sample_y,
            1.0,
            0,
            host.photosystem_count(),
            host.leaf_absorb_effective(),
            host.body_plan.shade_efficiency,
            rt_g,
        );
        assert!(
            lit_s < lit_g,
            "smotherer should leave less light for host (smother={lit_s} gentle={lit_g})"
        );
    }
}
