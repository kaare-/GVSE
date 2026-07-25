//! Force sensors on fixture edges (S3).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::body::{BodyGraph, PartKind};
use crate::tissue::{TissueKind, TissuePaint};

pub const SENSOR_HISTORY: usize = 64;

/// Uniaxial force sampler wired into [`BodyGraph`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForceSensorLink {
    pub x: i32,
    pub y: i32,
    /// Unit axis the sensor measures (arena space).
    pub dir_x: f32,
    pub dir_y: f32,
    pub last_force: f32,
    /// Ring buffer of recent samples (oldest → newest, max [`SENSOR_HISTORY`]).
    pub history: VecDeque<f32>,
}

impl ForceSensorLink {
    pub fn push_sample(&mut self, f: f32) {
        self.last_force = f;
        self.history.push_back(f);
        while self.history.len() > SENSOR_HISTORY {
            self.history.pop_front();
        }
    }

    pub fn mean_force(&self) -> f32 {
        if self.history.is_empty() {
            return 0.0;
        }
        self.history.iter().sum::<f32>() / self.history.len() as f32
    }
}

/// Discover ForceSensor paint on or adjacent to fixture cells.
pub fn find_sensors(
    paint: &TissuePaint,
    fixture_cells: &std::collections::HashSet<(i32, i32)>,
) -> Vec<ForceSensorLink> {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if paint.get(x as u32, y as u32) != TissueKind::ForceSensor {
                continue;
            }
            let on_or_near = fixture_cells.contains(&(x, y))
                || [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| {
                    fixture_cells.contains(&(x + dx, y + dy))
                });
            if !on_or_near {
                continue;
            }
            // Prefer horizontal sensing (thrust along +x) when fixture is to the left.
            let (dir_x, dir_y) = if fixture_cells.contains(&(x - 1, y)) {
                (1.0, 0.0)
            } else if fixture_cells.contains(&(x + 1, y)) {
                (-1.0, 0.0)
            } else if fixture_cells.contains(&(x, y - 1)) {
                (0.0, 1.0)
            } else if fixture_cells.contains(&(x, y + 1)) {
                (0.0, -1.0)
            } else {
                (1.0, 0.0)
            };
            out.push(ForceSensorLink {
                x,
                y,
                dir_x,
                dir_y,
                last_force: 0.0,
                history: VecDeque::with_capacity(SENSOR_HISTORY),
            });
        }
    }
    out.sort_by_key(|s| (s.y, s.x));
    out
}

/// Sample |muscle force proxy| and hinged-bone displacement along sensor dirs.
pub fn sample_sensors(graph: &mut BodyGraph) {
    if graph.sensors.is_empty() {
        return;
    }
    // Muscle force proxy: sum (actuation-0.5) * (rest - length) signed.
    let mut muscle_proxy = 0.0f32;
    for m in &graph.muscles {
        let a = graph.part_by_id(m.part_a);
        let b = graph.part_by_id(m.part_b);
        let (ax, ay) = match a {
            Some(p) => (m.attach_a.0 + p.offset_x, m.attach_a.1 + p.offset_y),
            None => m.attach_a,
        };
        let (bx, by) = match b {
            Some(p) => (m.attach_b.0 + p.offset_x, m.attach_b.1 + p.offset_y),
            None => m.attach_b,
        };
        let dx = (ax - bx) as f32;
        let dy = (ay - by) as f32;
        let len = (dx * dx + dy * dy).sqrt();
        muscle_proxy += (m.actuation - 0.5) * (m.rest_dist - len);
    }

    // Hinged bone displacement (offset projected).
    let mut bone_dx = 0.0f32;
    let mut bone_dy = 0.0f32;
    for p in &graph.parts {
        if p.kind == PartKind::Bone && !p.anchored && graph.is_hinged(p.id) {
            bone_dx += p.offset_x as f32;
            bone_dy += p.offset_y as f32;
        }
    }

    let sensors = graph.sensors.clone();
    for (i, s) in sensors.iter().enumerate() {
        let disp = bone_dx * s.dir_x + bone_dy * s.dir_y;
        let force = muscle_proxy.abs() + disp;
        if let Some(slot) = graph.sensors.get_mut(i) {
            slot.push_sample(force);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::activate;
    use crate::demo;

    #[test]
    fn fin_bench_has_sensor() {
        let paint = demo::fin_bench_paint(32, 32);
        let g = activate(&paint).unwrap();
        assert!(!g.sensors.is_empty());
    }
}
