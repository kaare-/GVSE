//! Episode evaluation + hill-climb training on muscle feedback (S4).

use crate::arena::StudioArena;
use crate::body::BodyGraph;
use crate::neural::StudioNet;

#[derive(Debug, Clone)]
pub struct EpisodeResult {
    pub fitness: f32,
    pub mean_tension: f32,
    pub bone_travel: f32,
    pub ticks: u64,
}

/// Run `ticks` with the net driving muscle actuation; fitness rewards
/// free-bone travel and mild tension (work), penalizes collapse to floor.
pub fn evaluate_net(arena: &mut StudioArena, net: &StudioNet, ticks: u64) -> EpisodeResult {
    arena.physics.scripted_muscle = false;
    let _ = arena.activate();
    let Some(graph) = arena.body.graph.as_ref() else {
        return EpisodeResult {
            fitness: -1.0e6,
            mean_tension: 0.0,
            bone_travel: 0.0,
            ticks: 0,
        };
    };
    if graph.muscles.is_empty() || net.n_out != graph.muscles.len() {
        return EpisodeResult {
            fitness: -1.0e6,
            mean_tension: 0.0,
            bone_travel: 0.0,
            ticks: 0,
        };
    }

    let free_ids: Vec<u32> = graph
        .parts
        .iter()
        .filter(|p| p.kind == crate::body::PartKind::Bone && !p.anchored)
        .map(|p| p.id)
        .collect();
    let start = centroids(graph, &free_ids);
    let mut tension_acc = 0.0;
    let mut samples = 0u64;

    for _ in 0..ticks {
        apply_net(arena, net);
        arena.tick();
        if let Some(g) = arena.body.graph.as_ref() {
            tension_acc += g.mean_tension();
            samples += 1;
        }
    }

    let end = arena
        .body
        .graph
        .as_ref()
        .map(|g| centroids(g, &free_ids))
        .unwrap_or_else(|| start.clone());
    let travel: f32 = start
        .iter()
        .zip(end.iter())
        .map(|(a, b)| (a.0 - b.0).hypot(a.1 - b.1))
        .sum();
    let mean_t = if samples > 0 {
        tension_acc / samples as f32
    } else {
        0.0
    };
    // Prefer motion; small tension bonus (muscles doing work).
    let fitness = travel * 2.0 + mean_t * 0.15;
    EpisodeResult {
        fitness,
        mean_tension: mean_t,
        bone_travel: travel,
        ticks,
    }
}

fn centroids(graph: &BodyGraph, ids: &[u32]) -> Vec<(f32, f32)> {
    ids.iter()
        .filter_map(|id| {
            graph
                .parts
                .iter()
                .find(|p| p.id == *id)
                .map(|p| p.centroid())
        })
        .collect()
}

fn apply_net(arena: &mut StudioArena, net: &StudioNet) {
    let Some(graph) = arena.body.graph.as_mut() else {
        return;
    };
    let fb = graph.muscle_feedback();
    if fb.is_empty() {
        return;
    }
    let input = StudioNet::encode_feedback(&fb);
    if input.len() != net.n_in {
        return;
    }
    let out = net.forward(&input);
    for (m, &a) in graph.muscles.iter_mut().zip(out.iter()) {
        m.actuation = a;
    }
}

/// Random-search / hill-climb for a few generations (deterministic seed).
pub fn hill_climb(
    make_arena: impl Fn() -> StudioArena,
    generations: u32,
    episode_ticks: u64,
    seed: u64,
) -> (StudioNet, EpisodeResult) {
    let mut probe = make_arena();
    let _ = probe.activate();
    let n_mus = probe
        .body
        .graph
        .as_ref()
        .map(|g| g.muscles.len())
        .unwrap_or(0)
        .max(1);
    let mut best_net = StudioNet::for_muscles(n_mus, seed);
    let mut best = {
        let mut a = make_arena();
        evaluate_net(&mut a, &best_net, episode_ticks)
    };
    for g in 0..generations {
        let cand = best_net.mutate(seed.wrapping_add(g as u64 + 1), 0.35);
        let mut a = make_arena();
        let r = evaluate_net(&mut a, &cand, episode_ticks);
        if r.fitness > best.fitness {
            best = r;
            best_net = cand;
        }
    }
    (best_net, best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::fin_hydro_arena;

    #[test]
    fn hill_climb_runs_fin_episode() {
        let (_net, best) = hill_climb(fin_hydro_arena, 4, 40, 7);
        // Smoke: finishes with finite fitness (may be low on tiny episodes).
        assert!(best.fitness.is_finite());
        assert_eq!(best.ticks, 40);
    }
}
