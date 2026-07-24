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
    /// Default playable ring (~3 km at 0.25 m/col, 192 chunks).
    pub fn default_ring() -> Self {
        Self {
            topology: WorldTopology::Ring { chunks: 192 },
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

/// Periodic multi-scale height noise in metres (seam-safe via `frac`).
fn periodic_relief(seed: u64, frac: f32, salt: u64) -> f32 {
    let tau = std::f32::consts::TAU;
    let phase = crate::terrain::hash_f32(seed, salt as i64, 1) * tau;
    let a = (frac * tau * 2.0 + phase).sin();
    let b = (frac * tau * 5.0 + phase * 1.3).sin();
    let c = (frac * tau * 11.0 + phase * 0.7).sin();
    let d = (frac * tau * 23.0 + phase * 2.1).sin();
    a * 1.0 + b * 0.55 + c * 0.28 + d * 0.12
}

/// Narrow island / stack bump in `[0, 1]` centred at `center` frac.
fn peak_bump(frac: f32, center: f32, width: f32) -> f32 {
    let mut d = (frac - center).abs();
    d = d.min(1.0 - d); // periodic distance on the ring
    (-(d * d) / (2.0 * width * width)).exp()
}

/// Target solid surface elevation (before sea fill) for a facies belt.
///
/// Amplitudes are intentionally large: the camera pans vertically, so
/// peaks of hundreds–thousands of metres and deep abyssal plains are fine.
pub fn facies_surface_y(
    seed: u64,
    topology: WorldTopology,
    world_x: i32,
    sea_level: f32,
) -> f32 {
    let belt = facies_at(seed, topology, world_x);
    let n = |salt: u64| (crate::terrain::hash_f32(seed, world_x as i64, salt) - 0.5) * 2.0;
    let frac = ring_frac(topology, world_x);
    let relief = periodic_relief(seed, frac, 50);
    let local = periodic_relief(seed, frac, 51) * 0.35 + n(52) * 0.15;

    let mut base = match belt {
        FaciesBelt::Abyss => {
            // Deep ocean floor: ~250–550 m below sea, rolling abyssal hills.
            sea_level - 400.0 + relief * 80.0 + local * 40.0 + n(30) * 12.0
        }
        FaciesBelt::Slope => {
            // Steep continental slope — big drop from shelf to abyss.
            sea_level - 120.0 + relief * 90.0 + local * 50.0 + n(31) * 20.0
        }
        FaciesBelt::Shelf => {
            // Shallow shelf with occasional islands that breach sea level.
            let island = peak_bump(frac, 0.19, 0.008) * 55.0
                + peak_bump(frac, 0.21, 0.005) * 35.0;
            // High-frequency terms used to be independent per column
            // (`n * 3` + `local * 6`) and cut single-column trenches the
            // flat-sea overlay hides at the free surface but still show as
            // limestone notches / taller "pipe" water bands (x≈2655).
            // 3-tap smooth the HF so shelf beds stay continuous.
            let hf = |wx: i32| {
                let f = ring_frac(topology, wx);
                let n_col = (crate::terrain::hash_f32(seed, wx as i64, 32) - 0.5) * 2.0;
                let local_col = periodic_relief(seed, f, 51) * 0.35
                    + (crate::terrain::hash_f32(seed, wx as i64, 52) - 0.5) * 2.0 * 0.15;
                local_col * 2.0 + n_col * 0.35
            };
            let smooth_hf =
                (hf(world_x - 1) + hf(world_x) + hf(world_x + 1)) / 3.0;
            sea_level - 18.0 + relief * 10.0 + island + smooth_hf
        }
        FaciesBelt::Marsh => sea_level + 0.6 + relief * 2.0 + local * 1.5 + n(33) * 0.8,
        FaciesBelt::Coast => {
            // Beaches, dunes, rocky headlands.
            let headland = peak_bump(frac, 0.33, 0.01) * 40.0;
            sea_level + 6.0 + relief * 12.0 + local * 8.0 + headland + n(34) * 4.0
        }
        FaciesBelt::Plains => {
            // Rolling plains cut by river valleys (negative bumps).
            let valley = peak_bump(frac, 0.43, 0.012) * -35.0
                + peak_bump(frac, 0.47, 0.01) * -22.0;
            sea_level + 45.0 + relief * 35.0 + local * 18.0 + valley + n(35) * 8.0
        }
        FaciesBelt::Foothills => {
            sea_level + 180.0 + relief * 120.0 + local * 60.0 + n(36) * 25.0
        }
        FaciesBelt::HighRange => {
            // Cordillera: several named peaks, main summit ~1–2 km a.s.l.
            let p1 = peak_bump(frac, 0.63, 0.014) * 900.0;
            let p2 = peak_bump(frac, 0.66, 0.012) * 1400.0; // main
            let p3 = peak_bump(frac, 0.69, 0.011) * 750.0;
            let valley = peak_bump(frac, 0.645, 0.006) * -180.0
                + peak_bump(frac, 0.675, 0.006) * -160.0;
            sea_level + 220.0 + p1 + p2 + p3 + valley + relief * 80.0 + n(37) * 40.0
        }
        FaciesBelt::RainShadow => {
            // High dry plateau dropping into canyons.
            let canyon = peak_bump(frac, 0.77, 0.01) * -120.0;
            sea_level + 160.0 + relief * 70.0 + local * 40.0 + canyon + n(38) * 20.0
        }
        FaciesBelt::InteriorBasin => {
            // Endorheic lowland — can sit near or below sea level.
            let playa = peak_bump(frac, 0.85, 0.02) * -40.0;
            sea_level - 5.0 + relief * 25.0 + local * 12.0 + playa + n(39) * 8.0
        }
    };
    // Soft blend toward abyss near the periodic seam (frac→0/1).
    let seam = (frac.min(1.0 - frac) * 25.0).clamp(0.0, 1.0);
    let abyss_ref = sea_level - 400.0;
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

    #[test]
    fn shelf_bathymetry_has_no_single_column_trenches() {
        // Regresses the ±3 m per-column shelf hash that cut limestone
        // notches under the flat-sea overlay (visible around x≈2655).
        let t = WorldTopology::Ring { chunks: 192 };
        let sea = 12.0f32;
        let mut max_jump = 0.0f32;
        let mut prev: Option<f32> = None;
        let width = 192 * CHUNK_W as i32;
        for x in 0..width {
            if facies_at(42, t, x) != FaciesBelt::Shelf {
                prev = None;
                continue;
            }
            let y = facies_surface_y(42, t, x, sea);
            if let Some(p) = prev {
                max_jump = max_jump.max((y - p).abs());
            }
            prev = Some(y);
        }
        assert!(
            max_jump < 2.0,
            "shelf neighbour jump {max_jump:.2} m too large (single-column trench)"
        );
        // Spot-check the old glitch stripe: no multi-metre bed cliff.
        let y0 = facies_surface_y(42, t, 2655, sea);
        let y1 = facies_surface_y(42, t, 2656, sea);
        assert!(
            (y0 - y1).abs() < 2.0,
            "x=2655/2656 bed jump too large ({y0:.2} → {y1:.2})"
        );
    }
}
