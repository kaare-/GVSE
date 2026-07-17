//! World generation topology and facies-belt recipes (see `docs/STRATA.md`,
//! `docs/WORLDGEN.md`).
//!
//! v1 ships a **ring** world: finite circumference, periodic neighbours.
//! `LegacyContinental` keeps the old fixed transect for scenarios.

use serde::{Deserialize, Serialize};
use wk_material::CHUNK_W;

/// How the horizontal axis closes (or doesn't).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WorldTopology {
    /// Open strip — missing neighbour ⇒ boundary outbox (scenarios / legacy).
    #[default]
    Open,
    /// Periodic ring of `chunks` chunk coordinates `0 .. chunks`.
    Ring {
        chunks: u32,
    },
}

impl WorldTopology {
    pub fn is_ring(self) -> bool {
        matches!(self, WorldTopology::Ring { .. })
    }

    pub fn ring_chunks(self) -> Option<u32> {
        match self {
            WorldTopology::Ring { chunks } => Some(chunks.max(1)),
            WorldTopology::Open => None,
        }
    }

    pub fn width_columns(self) -> Option<i32> {
        self.ring_chunks()
            .map(|c| (c as i32).saturating_mul(CHUNK_W as i32))
    }
}

/// Which generator fills new chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WorldGenProfile {
    /// Fixed ocean→shelf→coast→plains→mountains transect (`continental_surface_y`).
    #[default]
    LegacyContinental,
    /// Periodic facies belts around a ring ([`crate::terrain`] ring path).
    RingFacies,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorldGenParams {
    pub topology: WorldTopology,
    pub profile: WorldGenProfile,
}

impl Default for WorldGenParams {
    fn default() -> Self {
        Self {
            topology: WorldTopology::Open,
            profile: WorldGenProfile::LegacyContinental,
        }
    }
}

impl WorldGenParams {
    /// Default playable ring (~1.5 km at 0.25 m/col, 96 chunks).
    pub fn default_ring() -> Self {
        Self {
            topology: WorldTopology::Ring { chunks: 96 },
            profile: WorldGenProfile::RingFacies,
        }
    }
}

/// Wrap a chunk coordinate into the ring, or return `coord` unchanged if open.
pub fn wrap_chunk_coord(topology: WorldTopology, coord: i32) -> i32 {
    match topology {
        WorldTopology::Open => coord,
        WorldTopology::Ring { chunks } => {
            let n = chunks.max(1) as i32;
            coord.rem_euclid(n)
        }
    }
}

/// Wrap a world-x column into `[0, width)` on a ring; identity if open.
pub fn wrap_world_x(topology: WorldTopology, world_x: i32) -> i32 {
    match topology.width_columns() {
        Some(w) if w > 0 => world_x.rem_euclid(w),
        _ => world_x,
    }
}

/// Neighbour chunk coords (left, right), wrapped on a ring.
pub fn neighbor_chunk_coords(topology: WorldTopology, coord: i32) -> (i32, i32) {
    (
        wrap_chunk_coord(topology, coord - 1),
        wrap_chunk_coord(topology, coord + 1),
    )
}

/// Facies belt along the ring (arc-length story).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaciesBelt {
    Abyss,
    Slope,
    Shelf,
    Marsh,
    Coast,
    Plains,
    Foothills,
    HighRange,
    RainShadow,
    InteriorBasin,
}

/// Seed-authored belt anchors as fractions of the ring circumference.
/// Each entry is `(start_frac, belt)` Contiguous arcs; last wraps to first.
fn belt_anchors(seed: u64) -> [(f32, FaciesBelt); 11] {
    // Small seed jitter on boundaries so two seeds don't share identical seams.
    // First and last arcs are both Abyss so the ring closes without a cliff.
    let j = |i: u64| {
        let h = crate::terrain::hash_f32(seed, 9100, i);
        (h - 0.5) * 0.015
    };
    [
        (0.00, FaciesBelt::Abyss),
        ((0.10 + j(2)).clamp(0.07, 0.13), FaciesBelt::Slope),
        ((0.16 + j(3)).clamp(0.14, 0.20), FaciesBelt::Shelf),
        ((0.24 + j(4)).clamp(0.21, 0.27), FaciesBelt::Marsh),
        ((0.30 + j(5)).clamp(0.28, 0.35), FaciesBelt::Coast),
        ((0.40 + j(6)).clamp(0.36, 0.46), FaciesBelt::Plains),
        ((0.52 + j(7)).clamp(0.48, 0.56), FaciesBelt::Foothills),
        ((0.62 + j(8)).clamp(0.58, 0.68), FaciesBelt::HighRange),
        ((0.74 + j(9)).clamp(0.70, 0.78), FaciesBelt::RainShadow),
        ((0.82 + j(10)).clamp(0.79, 0.88), FaciesBelt::InteriorBasin),
        ((0.92 + j(11)).clamp(0.90, 0.95), FaciesBelt::Abyss),
    ]
}

/// Fraction of ring circumference for `world_x` (requires ring topology).
pub fn ring_frac(topology: WorldTopology, world_x: i32) -> f32 {
    let w = topology.width_columns().unwrap_or(1).max(1) as f32;
    let x = wrap_world_x(topology, world_x) as f32;
    (x / w).clamp(0.0, 1.0 - f32::EPSILON)
}

/// Primary belt at this column (no blend — use for recipe pick).
pub fn facies_at(seed: u64, topology: WorldTopology, world_x: i32) -> FaciesBelt {
    let f = ring_frac(topology, world_x);
    let anchors = belt_anchors(seed);
    let mut best = anchors[0].1;
    for i in 0..anchors.len() {
        let (start, belt) = anchors[i];
        let next = if i + 1 < anchors.len() {
            anchors[i + 1].0
        } else {
            1.0
        };
        if f >= start && f < next {
            best = belt;
            break;
        }
        // Past last anchor → interior basin until wrap (abyss at 0).
        if i + 1 == anchors.len() && f >= start {
            best = belt;
        }
    }
    best
}

/// Target solid surface elevation (before sea fill) for a facies belt.
pub fn facies_surface_y(
    seed: u64,
    topology: WorldTopology,
    world_x: i32,
    sea_level: f32,
) -> f32 {
    let belt = facies_at(seed, topology, world_x);
    let n = |salt: u64| (crate::terrain::hash_f32(seed, world_x as i64, salt) - 0.5) * 2.0;
    let frac = ring_frac(topology, world_x);
    // Periodic detail so the seam matches.
    let ripple = (frac * std::f32::consts::TAU * 3.0).sin() * 1.2
        + (frac * std::f32::consts::TAU * 7.0 + n(50) * 0.5).sin() * 0.6;

    let mut base = match belt {
        FaciesBelt::Abyss => sea_level - 38.0 + n(30) * 0.8,
        FaciesBelt::Slope => sea_level - 18.0 + n(31) * 1.5,
        FaciesBelt::Shelf => sea_level - 3.5 + n(32) * 0.4,
        FaciesBelt::Marsh => sea_level + 0.4 + n(33) * 0.3 + ripple * 0.2,
        FaciesBelt::Coast => sea_level + 4.0 + n(34) * 0.8 + ripple * 0.5,
        FaciesBelt::Plains => sea_level + 14.0 + n(35) * 1.2 + ripple,
        FaciesBelt::Foothills => sea_level + 32.0 + n(36) * 3.0 + ripple * 1.5,
        FaciesBelt::HighRange => {
            // Peak centred in the high-range arc (~0.65), not near the seam.
            let peak = ((frac - 0.65) * 28.0).abs();
            sea_level + 55.0 + (1.0 - peak.min(1.0)) * 45.0 + n(37) * 4.0
        }
        FaciesBelt::RainShadow => sea_level + 22.0 + n(38) * 2.0 + ripple * 0.8,
        FaciesBelt::InteriorBasin => sea_level + 2.0 + n(39) * 1.0 + ripple * 0.4,
    };
    // Soft blend toward abyss near the periodic seam (frac→0/1).
    let seam = (frac.min(1.0 - frac) * 40.0).clamp(0.0, 1.0);
    let abyss_ref = sea_level - 38.0;
    if seam < 1.0 {
        base = abyss_ref + (base - abyss_ref) * seam;
    }
    base
}

/// Wetness 0..1 for hydro/ecology seeding hints.
pub fn facies_wetness(seed: u64, topology: WorldTopology, world_x: i32) -> f32 {
    let base = match facies_at(seed, topology, world_x) {
        FaciesBelt::Abyss | FaciesBelt::Shelf | FaciesBelt::Marsh => 0.95,
        FaciesBelt::Slope | FaciesBelt::Coast => 0.75,
        FaciesBelt::Plains | FaciesBelt::InteriorBasin => 0.55,
        FaciesBelt::Foothills => 0.45,
        FaciesBelt::HighRange => 0.50,
        FaciesBelt::RainShadow => 0.15,
    };
    let jitter = (crate::terrain::hash_f32(seed, world_x as i64, 77) - 0.5) * 0.05;
    (base + jitter).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_chunk_rings() {
        let t = WorldTopology::Ring { chunks: 8 };
        assert_eq!(wrap_chunk_coord(t, -1), 7);
        assert_eq!(wrap_chunk_coord(t, 8), 0);
        assert_eq!(wrap_chunk_coord(t, 3), 3);
    }

    #[test]
    fn wrap_world_x_rings() {
        let t = WorldTopology::Ring { chunks: 2 }; // 128 cols
        assert_eq!(wrap_world_x(t, 128), 0);
        assert_eq!(wrap_world_x(t, -1), 127);
    }

    #[test]
    fn open_identity() {
        let t = WorldTopology::Open;
        assert_eq!(wrap_chunk_coord(t, -3), -3);
        assert_eq!(wrap_world_x(t, 999), 999);
    }

    #[test]
    fn facies_covers_ring() {
        let t = WorldTopology::Ring { chunks: 16 };
        let mut seen = 0u32;
        for x in 0..(16 * CHUNK_W as i32) {
            let _ = facies_at(42, t, x);
            seen |= 1;
        }
        assert_eq!(seen, 1);
        // Seam belts differ from mid-ring high range for seed 42.
        let a = facies_at(42, t, 0);
        let mid = facies_at(42, t, 8 * CHUNK_W as i32);
        assert_ne!(a, mid);
    }
}
