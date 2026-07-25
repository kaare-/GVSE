//! Nerve flood-fill components (S3).

use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::tissue::{TissueKind, TissuePaint};

/// Connected Nerve / NeuronBlob component after activate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NerveComponent {
    pub id: u32,
    /// True when the component contains any NeuronBlob cell.
    pub is_blob: bool,
    pub cells: Vec<(i32, i32)>,
}

/// Directed-ish nerve map used to decide whether a neural controller exists.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NerveGraph {
    pub components: Vec<NerveComponent>,
}

impl NerveGraph {
    pub fn blob_count(&self) -> usize {
        self.components.iter().filter(|c| c.is_blob).count()
    }

    pub fn has_controller(&self) -> bool {
        self.blob_count() > 0
    }
}

/// Flood-fill Nerve + NeuronBlob (treated as one tissue family).
pub fn build_nerve_graph(paint: &TissuePaint) -> NerveGraph {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut seen = HashSet::new();
    let mut components = Vec::new();
    let mut next_id = 0u32;

    for y in 0..h {
        for x in 0..w {
            let k = paint.get(x as u32, y as u32);
            if !matches!(k, TissueKind::Nerve | TissueKind::NeuronBlob) {
                continue;
            }
            if !seen.insert((x, y)) {
                continue;
            }
            let (cells, is_blob) = flood_nerve(paint, x, y, &mut seen);
            if cells.is_empty() {
                continue;
            }
            components.push(NerveComponent {
                id: next_id,
                is_blob,
                cells,
            });
            next_id += 1;
        }
    }
    NerveGraph { components }
}

fn is_nerve_family(k: TissueKind) -> bool {
    matches!(k, TissueKind::Nerve | TissueKind::NeuronBlob)
}

fn flood_nerve(
    paint: &TissuePaint,
    sx: i32,
    sy: i32,
    seen: &mut HashSet<(i32, i32)>,
) -> (Vec<(i32, i32)>, bool) {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut out = Vec::new();
    let mut is_blob = false;
    let mut q = VecDeque::new();
    q.push_back((sx, sy));
    while let Some((x, y)) = q.pop_front() {
        let k = paint.get(x as u32, y as u32);
        if k == TissueKind::NeuronBlob {
            is_blob = true;
        }
        out.push((x, y));
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x + dx;
            let ny = y + dy;
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                continue;
            }
            if !is_nerve_family(paint.get(nx as u32, ny as u32)) {
                continue;
            }
            if seen.insert((nx, ny)) {
                q.push_back((nx, ny));
            }
        }
    }
    (out, is_blob)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_marks_controller() {
        let mut paint = TissuePaint::new(8, 8);
        paint.set(2, 2, TissueKind::Nerve);
        paint.set(3, 2, TissueKind::Nerve);
        paint.set(2, 3, TissueKind::NeuronBlob);
        paint.set(3, 3, TissueKind::NeuronBlob);
        let g = build_nerve_graph(&paint);
        assert_eq!(g.components.len(), 1);
        assert!(g.has_controller());
    }
}
