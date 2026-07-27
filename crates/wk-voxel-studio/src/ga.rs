//! Morphology GA — mutate tissue paint, keep best by episode fitness (S5).

use crate::arena::StudioArena;
use crate::neural::{SensorCounts, StudioNet};
use crate::tissue::{TissueKind, TissuePaint};
use crate::train::{evaluate_net, EpisodeResult};

#[derive(Debug, Clone)]
pub struct GaIndividual {
    pub paint: TissuePaint,
    pub net: StudioNet,
    pub fitness: f32,
}

fn hash(x: u64) -> u64 {
    x.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1)
}

/// Nudge a few bone/muscle cells (deterministic).
pub fn mutate_paint(paint: &TissuePaint, seed: u64) -> TissuePaint {
    let mut out = paint.clone();
    let mut s = seed;
    let n = (out.width as usize).saturating_mul(out.height as usize);
    if n == 0 {
        return out;
    }
    for _ in 0..6 {
        s = hash(s);
        let i = (s as usize) % n;
        let kind = out.cells[i];
        if matches!(
            kind,
            TissueKind::Bone | TissueKind::Muscle | TissueKind::Empty
        ) {
            s = hash(s);
            out.cells[i] = match s % 3 {
                0 => TissueKind::Bone,
                1 => TissueKind::Muscle,
                _ => TissueKind::Empty,
            };
        }
    }
    out
}

/// Population loop: evaluate paint+net pairs, elite + mutate.
pub fn evolve_morphology(
    base: impl Fn() -> StudioArena,
    population: usize,
    generations: u32,
    episode_ticks: u64,
    seed: u64,
) -> (GaIndividual, Vec<f32>) {
    let mut history = Vec::new();
    let template = base();
    let paint0 = template.body.paint.clone();
    let n_mus = {
        let mut a = base();
        a.body.paint = paint0.clone();
        a.activate()
            .map(|g| g.muscles.len().max(1))
            .unwrap_or(1)
    };

    let mut pop: Vec<GaIndividual> = (0..population)
        .map(|i| {
            let paint = if i == 0 {
                paint0.clone()
            } else {
                mutate_paint(&paint0, seed.wrapping_add(i as u64 * 17))
            };
            let net = StudioNet::for_body(n_mus, SensorCounts::default(), seed.wrapping_add(i as u64));
            GaIndividual {
                paint,
                net,
                fitness: f32::NEG_INFINITY,
            }
        })
        .collect();

    for g in 0..generations {
        for (i, ind) in pop.iter_mut().enumerate() {
            let mut arena = base();
            arena.body.paint = ind.paint.clone();
            // Resize net if muscle/sensor counts changed after paint mutate.
            let _ = arena.activate();
            let (m, sensors) = arena
                .body
                .graph
                .as_ref()
                .map(|g| (g.muscles.len(), g.sensor_counts()))
                .unwrap_or((0, SensorCounts::default()));
            if m == 0 {
                ind.fitness = -1.0e5;
                continue;
            }
            if !ind.net.matches_body(m, sensors) {
                ind.net =
                    StudioNet::for_body(m, sensors, seed.wrapping_add(g as u64 * 100 + i as u64));
            }
            let r: EpisodeResult = evaluate_net(&mut arena, &ind.net, episode_ticks);
            ind.fitness = r.fitness;
        }
        pop.sort_by(|a, b| {
            b.fitness
                .partial_cmp(&a.fitness)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        history.push(pop[0].fitness);
        // Replace bottom half with mutants of elite.
        let elite = pop[0].clone();
        for i in (population / 2)..population {
            let mut child = elite.clone();
            child.paint = mutate_paint(
                &elite.paint,
                seed.wrapping_add(g as u64 * 999 + i as u64),
            );
            child.net = elite.net.mutate(
                seed.wrapping_add(g as u64 * 77 + i as u64),
                0.25,
            );
            pop[i] = child;
        }
    }
    pop.sort_by(|a, b| {
        b.fitness
            .partial_cmp(&a.fitness)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    (pop[0].clone(), history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::fin_hydro_arena;

    #[test]
    fn ga_smoke_fin() {
        let (best, hist) = evolve_morphology(fin_hydro_arena, 4, 2, 24, 3);
        assert_eq!(hist.len(), 2);
        assert!(best.fitness.is_finite());
    }
}
