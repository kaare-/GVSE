//! Column Beer–Lambert canopy shade (Set D light competition).
//!
//! Spec: `docs/organism/LIGHT.md`. Sky comes straight down. Each
//! Photosystem / Stem cell absorbs a fraction of light; samples at
//! `(x, y)` see only what remains after every foliage cell at greater
//! `y` (plus soft lateral bleed so neighbours cast sideways shade).
//! Lower leaves and plants under taller canopies get less light than
//! tips — self-shade and other-shade from the same rule.
//!
//! Cast / sample use *posed* draw cells (flop + pile) so dry mats
//! stacked on a beach shade the greens underneath.
//!
//! Implementation stores per-column `BTreeMap`s so `shade_transmit` only
//! walks occupied foliage rows above the sample (not every empty Y up to
//! a global max — that froze the frame loop / F2 editor on dense worlds).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::blueprint::Genome;
use crate::organism::{Atom, ModuleId};

fn atom_casts_canopy(atom: &Atom) -> bool {
    // Land plants (Root present). Avoid depending on `plant` to keep the
    // shade ↔ growth import graph acyclic.
    atom.body.iter().any(|(_, _, m)| *m == ModuleId::Root)
}

/// Columns left/right that bleed shade into a sample column.
pub const SHADE_RADIUS: i32 = 2;
/// Floor transmit so deep shade is dim, not black.
pub const SHADE_AMBIENT_FLOOR: f32 = 0.08;
/// Fixed olive (Stem) attenuation per cell — `s` in LIGHT.md.
pub const STEM_ABSORB: f32 = 0.10;
/// Cap stacked absorb in one cell (pile / overlap).
pub const MAX_CELL_ABSORB: f32 = 0.92;
/// Weight of neighbour-column absorb relative to own column (above).
pub const LATERAL_SHADE_SCALE: f32 = 0.45;
/// Same-height lateral peer carpet factor (meadows at equal tip height).
pub const PEER_LATERAL_SCALE: f32 = 0.55;

/// Sparse per-column foliage absorption (`0..MAX_CELL_ABSORB`).
#[derive(Debug, Clone, Default)]
pub struct CanopyIndex {
    /// `wx → (wy → absorb)`.
    cols: HashMap<i32, BTreeMap<i32, f32>>,
}

impl CanopyIndex {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.cols.is_empty()
    }

    #[inline]
    pub fn absorb_at(&self, x: i32, y: i32) -> f32 {
        self.cols
            .get(&x)
            .and_then(|c| c.get(&y).copied())
            .unwrap_or(0.0)
    }

    fn record(&mut self, wx: i32, wy: i32, absorb: f32) {
        let a = absorb.clamp(0.0, MAX_CELL_ABSORB);
        if a <= 0.0 {
            return;
        }
        let col = self.cols.entry(wx).or_default();
        let e = col.entry(wy).or_insert(0.0);
        *e = (*e + a).min(MAX_CELL_ABSORB);
    }
}

/// One resolved draw cell after flop + pile (see `resolve_organism_draw_cells`).
#[derive(Debug, Clone, Copy)]
pub struct PosedModule {
    pub atom_idx: usize,
    pub wx: i32,
    pub wy: i32,
    pub mid: ModuleId,
}

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

/// Group posed-module indices by `atom_idx` (one build per organism tick).
///
/// Hot path: [`posed_canopy_sample_of`] / [`sum_posed_photo_light_of`] walk
/// only this plant's modules instead of scanning the full posed list.
pub fn group_posed_by_atom(posed: &[PosedModule], n_atoms: usize) -> Vec<Vec<usize>> {
    let mut by = vec![Vec::new(); n_atoms];
    for (i, p) in posed.iter().enumerate() {
        if p.atom_idx < n_atoms {
            by[p.atom_idx].push(i);
        }
    }
    by
}

/// Highest Photosystem/Stem draw cell among `indices` into `posed`.
pub fn posed_canopy_sample_of(
    posed: &[PosedModule],
    indices: &[usize],
    fallback: (i32, i32),
) -> (i32, i32) {
    let mut best: Option<(i32, i32)> = None;
    for &i in indices {
        let Some(p) = posed.get(i) else {
            continue;
        };
        if !matches!(p.mid, ModuleId::Photosystem | ModuleId::Stem) {
            continue;
        }
        let replace = best
            .map(|(_, by)| p.wy > by || (p.wy == by && p.mid == ModuleId::Photosystem))
            .unwrap_or(true);
        if replace {
            best = Some((p.wx, p.wy));
        }
    }
    best.unwrap_or(fallback)
}

/// Highest Photosystem/Stem draw cell for a plant (fallback sample).
///
/// Prefers [`posed_canopy_sample_of`] with a prebuilt [`group_posed_by_atom`]
/// index when stepping many plants.
pub fn posed_canopy_sample(
    posed: &[PosedModule],
    atom_idx: usize,
    fallback: (i32, i32),
) -> (i32, i32) {
    let mut best: Option<(i32, i32)> = None;
    for p in posed {
        if p.atom_idx != atom_idx {
            continue;
        }
        if !matches!(p.mid, ModuleId::Photosystem | ModuleId::Stem) {
            continue;
        }
        let replace = best
            .map(|(_, by)| p.wy > by || (p.wy == by && p.mid == ModuleId::Photosystem))
            .unwrap_or(true);
        if replace {
            best = Some((p.wx, p.wy));
        }
    }
    best.unwrap_or(fallback)
}

fn lateral_weight(dx: i32) -> f32 {
    match dx.abs() {
        1 => 0.70,
        2 => 0.35,
        _ => 0.0,
    }
}

/// Column-only transmit (no lateral bleed) — for sharp under-plant visual dim.
pub fn shade_transmit_column(index: &CanopyIndex, wx: i32, sample_y: i32) -> f32 {
    if index.is_empty() {
        return 1.0;
    }
    let Some(col) = index.cols.get(&wx) else {
        return 1.0;
    };
    let mut transmit = 1.0_f32;
    for (&_y, &optical) in col.range((sample_y + 1)..) {
        let optical = optical.min(MAX_CELL_ABSORB);
        if optical > 0.0 {
            transmit *= (1.0 - optical).clamp(0.0, 1.0);
        }
    }
    transmit.clamp(SHADE_AMBIENT_FLOOR, 1.0)
}

/// Fraction of overhead sky remaining at `(wx, sample_y)` after Beer–Lambert
/// through foliage **above** this cell, plus soft lateral bleed.
///
/// Own-cell absorb does not shade the sample (light arrives, then the leaf
/// harvests). Same-height neighbours still compete via peer lateral.
/// Used by plant growth; visuals prefer [`shade_transmit_column`] + cast rays.
pub fn shade_transmit(index: &CanopyIndex, wx: i32, sample_y: i32) -> f32 {
    if index.is_empty() {
        return 1.0;
    }
    // Only occupied foliage rows above the sample (plus lateral columns).
    let mut ys: BTreeSet<i32> = BTreeSet::new();
    for dx in -SHADE_RADIUS..=SHADE_RADIUS {
        let Some(col) = index.cols.get(&(wx + dx)) else {
            continue;
        };
        for &y in col.range((sample_y + 1)..).map(|(y, _)| y) {
            ys.insert(y);
        }
    }

    let mut transmit = 1.0_f32;
    for y in ys {
        let mut optical = index.absorb_at(wx, y);
        if SHADE_RADIUS > 0 {
            let mut side = 0.0_f32;
            for dx in -SHADE_RADIUS..=SHADE_RADIUS {
                if dx == 0 {
                    continue;
                }
                side += index.absorb_at(wx + dx, y) * lateral_weight(dx);
            }
            optical += LATERAL_SHADE_SCALE * side;
        }
        optical = optical.min(MAX_CELL_ABSORB);
        if optical > 0.0 {
            transmit *= (1.0 - optical).clamp(0.0, 1.0);
        }
    }
    // Equal-height meadow: neighbours at the same Y still steal light.
    if SHADE_RADIUS > 0 && PEER_LATERAL_SCALE > 0.0 {
        let mut peer = 0.0_f32;
        for dx in -SHADE_RADIUS..=SHADE_RADIUS {
            if dx == 0 {
                continue;
            }
            peer += index.absorb_at(wx + dx, sample_y) * lateral_weight(dx);
        }
        let peer = (peer * PEER_LATERAL_SCALE).min(MAX_CELL_ABSORB);
        if peer > 0.0 {
            transmit *= (1.0 - peer).clamp(0.0, 1.0);
        }
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

/// Effective light for one Photosystem cell after column shade + gene remap.
pub fn effective_photo_light(
    index: &CanopyIndex,
    wx: i32,
    sample_y: i32,
    sky_l0: f32,
    genome: &Genome,
) -> f32 {
    if sky_l0 <= 0.01 {
        return 0.0;
    }
    let transmit = shade_transmit(index, wx, sample_y);
    let attenuated = (sky_l0 * transmit).clamp(0.0, 1.0);
    shade_harvest_light(attenuated, genome.shade_efficiency)
}

/// Sum harvest-remapped light over posed Photosystems listed in `indices`.
///
/// Each leaf samples its own column exposure — lower leaves self-shade.
pub fn sum_posed_photo_light_of(
    index: &CanopyIndex,
    posed: &[PosedModule],
    indices: &[usize],
    sky_at: &mut dyn FnMut(i32, i32) -> f32,
    genome: &Genome,
) -> f32 {
    let mut sum = 0.0_f32;
    let mut any = false;
    for &i in indices {
        let Some(p) = posed.get(i) else {
            continue;
        };
        if p.mid != ModuleId::Photosystem {
            continue;
        }
        any = true;
        let sky = sky_at(p.wx, p.wy);
        sum += effective_photo_light(index, p.wx, p.wy, sky, genome);
    }
    if any {
        sum
    } else {
        0.0
    }
}

/// Sum harvest-remapped light over every posed Photosystem of one plant.
///
/// Prefer [`sum_posed_photo_light_of`] with [`group_posed_by_atom`] in the
/// full-pop hot path.
pub fn sum_posed_photo_light(
    index: &CanopyIndex,
    posed: &[PosedModule],
    atom_idx: usize,
    sky_at: &mut dyn FnMut(i32, i32) -> f32,
    genome: &Genome,
) -> f32 {
    let mut sum = 0.0_f32;
    let mut any = false;
    for p in posed {
        if p.atom_idx != atom_idx || p.mid != ModuleId::Photosystem {
            continue;
        }
        any = true;
        let sky = sky_at(p.wx, p.wy);
        sum += effective_photo_light(index, p.wx, p.wy, sky, genome);
    }
    if any {
        sum
    } else {
        0.0
    }
}

/// Build canopy from upright body modules (tests / fallback).
pub fn build_canopy_index(atoms: &[Atom]) -> CanopyIndex {
    let mut index = CanopyIndex::default();
    for atom in atoms {
        if !atom_casts_canopy(atom) {
            continue;
        }
        for &(dx, dy, mid) in &atom.body {
            let absorb = match mid {
                ModuleId::Photosystem => atom.genome.leaf_absorb.clamp(0.0, 1.0),
                ModuleId::Stem => STEM_ABSORB,
                _ => continue,
            };
            index.record(atom.gx + dx as i32, atom.gy + dy as i32, absorb);
        }
    }
    index
}

/// Build canopy from posed (draw) modules so flopped mats shade each other.
pub fn build_canopy_index_posed(atoms: &[Atom], posed: &[PosedModule]) -> CanopyIndex {
    let mut index = CanopyIndex::default();
    for p in posed {
        if !matches!(p.mid, ModuleId::Photosystem | ModuleId::Stem) {
            continue;
        }
        let Some(atom) = atoms.get(p.atom_idx) else {
            continue;
        };
        if !atom_casts_canopy(atom) {
            continue;
        }
        let absorb = if p.mid == ModuleId::Photosystem {
            atom.genome.leaf_absorb.clamp(0.0, 1.0)
        } else {
            STEM_ABSORB
        };
        index.record(p.wx, p.wy, absorb);
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::Genome;
    use crate::organism::{Atom, ModuleId};

    fn plant_at(gx: i32, gy: i32, stems: i16, photos: i16, genome: Genome) -> Atom {
        let mut body = vec![(0, 0, ModuleId::Nucleus), (0, -1, ModuleId::Root)];
        for s in 1..=stems {
            body.push((0, s, ModuleId::Stem));
        }
        let top = stems.max(0);
        for p in 0..photos {
            // Stack photosystems in one column so self-shade is visible.
            body.push((0, top + 1 + p, ModuleId::Photosystem));
        }
        let mut a = Atom::from_body(gx, gy, 40.0, body);
        a.genome = genome;
        a
    }

    #[test]
    fn tip_brighter_than_lower_leaf_same_column() {
        let mut g = Genome::default();
        g.leaf_absorb = 0.45;
        g.shade_efficiency = 0.1;
        let plant = plant_at(10, 4, 2, 3, g);
        let index = build_canopy_index(&[plant]);
        // Photos at y = 4+2+1=7, 8, 9
        let tip = shade_transmit(&index, 10, 9);
        let mid = shade_transmit(&index, 10, 8);
        let low = shade_transmit(&index, 10, 7);
        assert!(tip > mid && mid > low, "tip={tip} mid={mid} low={low}");
        assert!(tip > 0.95, "open tip should be ~1, got {tip}");
    }

    #[test]
    fn taller_neighbour_reduces_short_plant_light() {
        let mut tall_g = Genome::default();
        tall_g.leaf_absorb = 0.85;
        tall_g.shade_efficiency = 0.05;
        let mut short_g = Genome::default();
        short_g.leaf_absorb = 0.3;
        short_g.shade_efficiency = 0.15;

        let short = plant_at(5, 4, 1, 1, short_g);
        let tall = plant_at(6, 4, 6, 3, tall_g);
        let atoms = [short, tall];
        let index = build_canopy_index(&atoms);
        let sample_y = canopy_top_y(&atoms[0]);
        let lit = effective_photo_light(&index, 5, sample_y, 1.0, &short_g);
        assert!(
            lit < 0.85,
            "tall neighbour should shade short plant (lit={lit})"
        );
        let open = shade_harvest_light(1.0, short_g.shade_efficiency);
        assert!(lit < open * 0.95);
    }

    #[test]
    fn equal_height_peers_shade_each_other() {
        let mut g = Genome::default();
        g.leaf_absorb = 0.7;
        g.shade_efficiency = 0.2;
        let a = plant_at(5, 2, 0, 1, g);
        let b = plant_at(6, 2, 0, 1, g);
        let c = plant_at(4, 2, 0, 1, g);
        let alone = [plant_at(5, 2, 0, 1, g)];
        let meadow = [a, b, c];
        let open = effective_photo_light(
            &build_canopy_index(&alone),
            5,
            canopy_top_y(&alone[0]),
            1.0,
            &g,
        );
        let crowded = effective_photo_light(
            &build_canopy_index(&meadow),
            5,
            canopy_top_y(&meadow[0]),
            1.0,
            &g,
        );
        assert!(
            crowded < open * 0.90,
            "peer meadow should cut light (crowded={crowded}, open={open})"
        );
    }

    #[test]
    fn piled_same_column_shades_lower_leaf() {
        let mut g = Genome::default();
        g.leaf_absorb = 0.8;
        let atoms = [plant_at(5, 2, 0, 1, g), plant_at(6, 2, 0, 1, g)];
        let posed = [
            PosedModule {
                atom_idx: 0,
                wx: 5,
                wy: 3,
                mid: ModuleId::Photosystem,
            },
            PosedModule {
                atom_idx: 1,
                wx: 5,
                wy: 5,
                mid: ModuleId::Photosystem,
            },
        ];
        let index = build_canopy_index_posed(&atoms, &posed);
        let low = shade_transmit(&index, 5, 3);
        let high = shade_transmit(&index, 5, 5);
        assert!(
            low < high * 0.85,
            "lower pile leaf should be shaded (low={low}, high={high})"
        );
    }

    #[test]
    fn group_posed_sample_matches_scan() {
        let posed = vec![
            PosedModule {
                atom_idx: 0,
                wx: 1,
                wy: 4,
                mid: ModuleId::Stem,
            },
            PosedModule {
                atom_idx: 1,
                wx: 5,
                wy: 8,
                mid: ModuleId::Photosystem,
            },
            PosedModule {
                atom_idx: 0,
                wx: 1,
                wy: 6,
                mid: ModuleId::Photosystem,
            },
        ];
        let by = group_posed_by_atom(&posed, 2);
        let a0 = posed_canopy_sample_of(&posed, &by[0], (0, 0));
        let a1 = posed_canopy_sample_of(&posed, &by[1], (0, 0));
        assert_eq!(a0, posed_canopy_sample(&posed, 0, (0, 0)));
        assert_eq!(a1, posed_canopy_sample(&posed, 1, (0, 0)));
        assert_eq!(a0, (1, 6));
        assert_eq!(a1, (5, 8));
    }

    #[test]
    fn sum_posed_counts_self_shade() {
        let mut g = Genome::default();
        g.leaf_absorb = 0.5;
        g.shade_efficiency = 0.0;
        let plant = plant_at(3, 2, 1, 3, g);
        let index = build_canopy_index(std::slice::from_ref(&plant));
        let posed: Vec<PosedModule> = plant
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|&(dx, dy, mid)| PosedModule {
                atom_idx: 0,
                wx: plant.gx + dx as i32,
                wy: plant.gy + dy as i32,
                mid,
            })
            .collect();
        let sum = sum_posed_photo_light(&index, &posed, 0, &mut |_, _| 1.0, &g);
        // Tip ~1, mid ~(1-a), low ~(1-a)^2 → sum < 3
        assert!(sum < 2.6, "stacked leaves should self-shade (sum={sum})");
        assert!(sum > 1.5, "still some harvest (sum={sum})");
    }

    #[test]
    fn sparse_tall_canopy_transmit_stays_cheap() {
        // One leaf at y=5 and one tip at y=500 — must not scan 495 empty rows.
        let mut index = CanopyIndex::default();
        index.record(0, 5, 0.4);
        index.record(0, 500, 0.4);
        let t0 = std::time::Instant::now();
        let mut acc = 0.0f32;
        for _ in 0..20_000 {
            acc += shade_transmit(&index, 0, 5);
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert!(acc > 0.0);
        assert!(
            ms < 500.0,
            "occupied-row scan should stay cheap (got {ms:.1}ms for 20k queries)"
        );
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
