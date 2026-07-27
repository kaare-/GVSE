//! Episode evaluation + hill-climb training on muscle feedback (S4).

use crate::arena::StudioArena;
use crate::body::BodyGraph;
use crate::neural::{SensorCounts, StudioNet};

#[derive(Debug, Clone)]
pub struct EpisodeResult {
    pub fitness: f32,
    pub mean_tension: f32,
    pub bone_travel: f32,
    pub ticks: u64,
}

/// Incremental hill-climb state for live studio / continuous regimes.
#[derive(Debug, Clone)]
pub struct TrainingSession {
    pub best_net: StudioNet,
    pub best: EpisodeResult,
    pub generation: u32,
    pub seed: u64,
    pub episode_ticks: u64,
}

impl TrainingSession {
    /// Build from an activated arena (needs ≥1 muscle).
    pub fn new(arena: &StudioArena, seed: u64, episode_ticks: u64) -> Option<Self> {
        let g = arena.body.graph.as_ref()?;
        let n_mus = g.muscles.len();
        if n_mus == 0 {
            return None;
        }
        let sensors = g.sensor_counts();
        let best_net = arena
            .body
            .net
            .clone()
            .unwrap_or_else(|| StudioNet::for_body(n_mus, sensors, seed));
        Some(Self {
            best_net,
            best: EpisodeResult {
                fitness: f32::NEG_INFINITY,
                mean_tension: 0.0,
                bone_travel: 0.0,
                ticks: 0,
            },
            generation: 0,
            seed,
            episode_ticks,
        })
    }

    /// One mutate → evaluate generation. Returns whether fitness improved.
    pub fn step(&mut self, make_arena: impl Fn() -> StudioArena) -> bool {
        let cand = if self.generation == 0 && self.best.fitness.is_infinite() {
            self.best_net.clone()
        } else {
            self.best_net
                .mutate(self.seed.wrapping_add(self.generation as u64 + 1), 0.35)
        };
        let mut a = make_arena();
        let r = evaluate_net(&mut a, &cand, self.episode_ticks);
        self.generation += 1;
        let improved = r.fitness > self.best.fitness;
        if improved {
            self.best = r;
            self.best_net = cand;
        }
        improved
    }
}

/// Run `ticks` with the net driving muscle actuation; fitness rewards
/// free-bone travel and mild tension (work), penalizes collapse to floor.
pub fn evaluate_net(arena: &mut StudioArena, net: &StudioNet, ticks: u64) -> EpisodeResult {
    arena.physics.scripted_muscle = false;
    arena.body.net = Some(net.clone());
    let _ = arena.activate();
    // Re-install after activate (activate may rebuild a mismatched net).
    arena.body.net = Some(net.clone());
    arena.physics.scripted_muscle = false;
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

/// Write net outputs into muscle actuation from current feedback + sensors.
pub fn apply_net(arena: &mut StudioArena, net: &StudioNet) {
    let Some(graph) = arena.body.graph.as_mut() else {
        return;
    };
    let fb = graph.muscle_feedback();
    if fb.is_empty() {
        return;
    }
    let sensors = graph.sensor_frame();
    let input = StudioNet::encode_inputs(&fb, &sensors);
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
    let (n_mus, sensors) = probe
        .body
        .graph
        .as_ref()
        .map(|g| (g.muscles.len().max(1), g.sensor_counts()))
        .unwrap_or((1, SensorCounts::default()));
    let mut session = TrainingSession {
        best_net: StudioNet::for_body(n_mus, sensors, seed),
        best: EpisodeResult {
            fitness: f32::NEG_INFINITY,
            mean_tension: 0.0,
            bone_travel: 0.0,
            ticks: 0,
        },
        generation: 0,
        seed,
        episode_ticks,
    };
    for _ in 0..generations {
        let _ = session.step(&make_arena);
    }
    (session.best_net, session.best)
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

    #[test]
    fn net_drive_applies_muscle_forces() {
        let mut arena = fin_hydro_arena();
        arena.physics = crate::physics::StudioPhysicsConfig::body_only();
        arena.physics.scripted_muscle = false;
        let g = arena.activate().unwrap();
        let n = g.muscles.len();
        assert!(n >= 1);
        let mut net = StudioNet::for_muscles(n, 11);
        // Bias outputs high so actuation crosses the 0.55 pull threshold.
        for v in net.b2.iter_mut() {
            *v = 3.0;
        }
        arena.body.net = Some(net.clone());
        let free_id = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .find(|p| p.kind == crate::body::PartKind::Bone && !p.anchored)
            .map(|p| p.id)
            .expect("free bone");
        let x0 = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .find(|p| p.id == free_id)
            .unwrap()
            .offset_x;
        for _ in 0..60 {
            arena.tick();
        }
        let g = arena.body.graph.as_ref().unwrap();
        let x1 = g.parts.iter().find(|p| p.id == free_id).unwrap().offset_x;
        let act: f32 = g.muscles.iter().map(|m| m.actuation).sum::<f32>() / g.muscles.len() as f32;
        assert!(
            act > 0.5 || x1 != x0 || g.mean_tension() > 0.01,
            "net should drive actuation/motion (act={act}, x {x0}→{x1})"
        );
    }

    #[test]
    fn training_session_steps() {
        let mut probe = fin_hydro_arena();
        probe.activate().unwrap();
        let mut session = TrainingSession::new(&probe, 3, 24).unwrap();
        let _ = session.step(fin_hydro_arena);
        assert_eq!(session.generation, 1);
        assert!(session.best.fitness.is_finite());
    }
}
