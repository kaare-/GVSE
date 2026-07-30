//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Body-plan aggregates over a pixel-gene list (Wave K).
//!
//! Global organism scalars used to live on [`crate::blueprint::Genome`].
//! They now derive from per-pixel [`crate::blueprint::PixelTraits`] so
//! every painted module contributes to mass, metabolism, fidelity, and
//! (Wave O) plant/fungus habit knobs.

use crate::blueprint::{PixelTraits, PlacedModule};
use crate::organism::ModuleId;

/// Cached body-plan derived from a pixel list.
///
/// Formulas are first-cut gameplay knobs — expect a tuning wave when
/// Live physics reads these aggregates (not a global `Genome` bag).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BodyPlan {
    /// Σ (mass × density) over every pixel.
    pub total_mass: f32,
    /// Mean upkeep_bias × pixel count (bag-style cost).
    pub metabolic_rate: f32,
    /// Mean clone_fidelity_bias (mass-weighted).
    pub clone_fidelity: f32,
    /// Mean reproduce_at_bias (mass-weighted), clamped to 0..1.
    pub reproduce_at: f32,
    /// Mean buoyancy_bias (mass-weighted), clamped to 0..1.
    pub buoyancy_bias: f32,
    /// Σ absorb_bias over Photosystem pixels.
    pub photo_capacity: f32,
    /// Count of Nucleus pixels.
    pub nucleus_count: usize,
    /// True when at least one Nucleus is present (repro gate).
    pub has_repro_gate: bool,
    pub pixel_count: usize,
    /// Mean Nucleus `alloc_stem` (Wave O). Fallback: mean over all pixels.
    pub alloc_stem: f32,
    pub alloc_leaf: f32,
    pub alloc_root: f32,
    /// Mean Root `root_depth_bias` (default when no roots).
    pub root_depth_bias: f32,
    /// Mean Photosystem `shade_efficiency` (default when no leaves).
    pub shade_efficiency: f32,
    /// Mean Digest/Hypha `digest_rate` (default when none).
    pub digest_rate: f32,
}

impl Default for BodyPlan {
    fn default() -> Self {
        Self {
            total_mass: 0.0,
            metabolic_rate: 0.0,
            clone_fidelity: 0.9,
            reproduce_at: 0.85,
            buoyancy_bias: 0.0,
            photo_capacity: 0.0,
            nucleus_count: 0,
            has_repro_gate: false,
            pixel_count: 0,
            alloc_stem: 0.25,
            alloc_leaf: 0.45,
            alloc_root: 0.30,
            root_depth_bias: 0.55,
            shade_efficiency: 0.40,
            digest_rate: 0.8,
        }
    }
}

impl BodyPlan {
    /// Normalized `(stem, leaf, root)` surplus weights (sum to 1).
    pub fn alloc_weights(self) -> (f32, f32, f32) {
        let s = self.alloc_stem.max(0.0);
        let l = self.alloc_leaf.max(0.0);
        let r = self.alloc_root.max(0.0);
        let sum = (s + l + r).max(1e-6);
        (s / sum, l / sum, r / sum)
    }
}

/// One pixel contribution for aggregation (kind + traits).
pub trait PixelRef {
    fn module(&self) -> ModuleId;
    fn traits(&self) -> &PixelTraits;
}

impl PixelRef for PlacedModule {
    fn module(&self) -> ModuleId {
        self.module
    }
    fn traits(&self) -> &PixelTraits {
        &self.traits
    }
}

impl PixelRef for (ModuleId, PixelTraits) {
    fn module(&self) -> ModuleId {
        self.0
    }
    fn traits(&self) -> &PixelTraits {
        &self.1
    }
}

/// Aggregate a body plan from any pixel list.
pub fn body_plan_from<'a, I, P>(pixels: I) -> BodyPlan
where
    I: IntoIterator<Item = &'a P>,
    P: PixelRef + 'a,
{
    let mut total_mass = 0.0f32;
    let mut upkeep_sum = 0.0f32;
    let mut fidelity_acc = 0.0f32;
    let mut repro_acc = 0.0f32;
    let mut buoy_acc = 0.0f32;
    let mut photo_capacity = 0.0f32;
    let mut nucleus_count = 0usize;
    let mut pixel_count = 0usize;

    let mut alloc_stem_n = 0.0f32;
    let mut alloc_leaf_n = 0.0f32;
    let mut alloc_root_n = 0.0f32;
    let mut nucleus_alloc_n = 0usize;
    let mut alloc_stem_all = 0.0f32;
    let mut alloc_leaf_all = 0.0f32;
    let mut alloc_root_all = 0.0f32;

    let mut root_depth_acc = 0.0f32;
    let mut root_n = 0usize;
    let mut shade_acc = 0.0f32;
    let mut shade_n = 0usize;
    let mut digest_acc = 0.0f32;
    let mut digest_n = 0usize;

    for p in pixels {
        let t = p.traits();
        let mass = (t.mass * t.density).max(0.0);
        total_mass += mass;
        upkeep_sum += t.upkeep_bias.max(0.0);
        fidelity_acc += t.clone_fidelity_bias.clamp(0.0, 1.0) * mass.max(1e-6);
        repro_acc += t.reproduce_at_bias.clamp(0.0, 1.0) * mass.max(1e-6);
        buoy_acc += t.buoyancy_bias.clamp(0.0, 1.0) * mass.max(1e-6);

        alloc_stem_all += t.alloc_stem.max(0.0);
        alloc_leaf_all += t.alloc_leaf.max(0.0);
        alloc_root_all += t.alloc_root.max(0.0);

        match p.module() {
            ModuleId::Photosystem => {
                photo_capacity += t.absorb_bias.max(0.0);
                shade_acc += t.shade_efficiency.clamp(0.0, 1.0);
                shade_n += 1;
            }
            ModuleId::Nucleus => {
                nucleus_count += 1;
                alloc_stem_n += t.alloc_stem.max(0.0);
                alloc_leaf_n += t.alloc_leaf.max(0.0);
                alloc_root_n += t.alloc_root.max(0.0);
                nucleus_alloc_n += 1;
            }
            ModuleId::Root => {
                root_depth_acc += t.root_depth_bias.clamp(0.0, 1.0);
                root_n += 1;
            }
            ModuleId::Digest | ModuleId::Hypha => {
                digest_acc += t.digest_rate.clamp(0.05, 2.0);
                digest_n += 1;
            }
            _ => {}
        }
        pixel_count += 1;
    }

    if pixel_count == 0 {
        return BodyPlan::default();
    }

    let mass_norm = total_mass.max(1e-6);
    let (alloc_stem, alloc_leaf, alloc_root) = if nucleus_alloc_n > 0 {
        let n = nucleus_alloc_n as f32;
        (alloc_stem_n / n, alloc_leaf_n / n, alloc_root_n / n)
    } else {
        let n = pixel_count as f32;
        (alloc_stem_all / n, alloc_leaf_all / n, alloc_root_all / n)
    };
    let defaults = BodyPlan::default();
    BodyPlan {
        total_mass,
        metabolic_rate: upkeep_sum.max(0.05),
        clone_fidelity: (fidelity_acc / mass_norm).clamp(0.05, 1.0),
        reproduce_at: (repro_acc / mass_norm).clamp(0.05, 0.99),
        buoyancy_bias: (buoy_acc / mass_norm).clamp(0.0, 1.0),
        photo_capacity,
        nucleus_count,
        has_repro_gate: nucleus_count > 0,
        pixel_count,
        alloc_stem,
        alloc_leaf,
        alloc_root,
        root_depth_bias: if root_n > 0 {
            (root_depth_acc / root_n as f32).clamp(0.0, 1.0)
        } else {
            defaults.root_depth_bias
        },
        shade_efficiency: if shade_n > 0 {
            (shade_acc / shade_n as f32).clamp(0.0, 1.0)
        } else {
            defaults.shade_efficiency
        },
        digest_rate: if digest_n > 0 {
            (digest_acc / digest_n as f32).clamp(0.05, 2.0)
        } else {
            defaults.digest_rate
        },
    }
}

/// Aggregate from kinds alone (default traits). Used by living atoms
/// that still carry the legacy `(dx, dy, ModuleId)` body shape.
pub fn body_plan_from_kinds<'a, I>(kinds: I) -> BodyPlan
where
    I: IntoIterator<Item = &'a ModuleId>,
{
    let pairs: Vec<(ModuleId, PixelTraits)> = kinds
        .into_iter()
        .map(|m| (*m, PixelTraits::default()))
        .collect();
    body_plan_from(&pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::LaneId;

    fn placed(x: i16, y: i16, m: ModuleId, t: PixelTraits) -> PlacedModule {
        PlacedModule {
            x,
            y,
            lane: LaneId::Mid,
            module: m,
            traits: t,
        }
    }

    #[test]
    fn body_plan_metabolic_matches_hand_math() {
        let a = PixelTraits {
            upkeep_bias: 1.0,
            ..PixelTraits::default()
        };
        let b = PixelTraits {
            upkeep_bias: 2.0,
            ..PixelTraits::default()
        };
        let body = [
            placed(0, 0, ModuleId::Nucleus, a),
            placed(1, 0, ModuleId::Photosystem, b),
        ];
        let plan = body_plan_from(&body);
        assert!((plan.metabolic_rate - 3.0).abs() < 1e-5);
        assert_eq!(plan.pixel_count, 2);
        assert_eq!(plan.nucleus_count, 1);
        assert!(plan.has_repro_gate);
        assert!((plan.photo_capacity - 1.0).abs() < 1e-5);
    }

    #[test]
    fn body_plan_total_mass_uses_density() {
        let dense = PixelTraits {
            mass: 1.0,
            density: 2.5,
            ..PixelTraits::default()
        };
        let body = [placed(0, 0, ModuleId::Bone, dense)];
        let plan = body_plan_from(&body);
        assert!((plan.total_mass - 2.5).abs() < 1e-5);
    }

    #[test]
    fn body_plan_reports_no_repro_gate_without_nucleus() {
        let body = [placed(
            0,
            0,
            ModuleId::Bone,
            PixelTraits::default(),
        )];
        let plan = body_plan_from(&body);
        assert!(!plan.has_repro_gate);
        assert_eq!(plan.nucleus_count, 0);
    }

    #[test]
    fn body_plan_from_kinds_defaults() {
        let kinds = [ModuleId::Nucleus, ModuleId::Photosystem];
        let plan = body_plan_from_kinds(&kinds);
        assert!(plan.has_repro_gate);
        assert!((plan.metabolic_rate - 2.0).abs() < 1e-5);
        assert!((plan.photo_capacity - 1.0).abs() < 1e-5);
    }

    #[test]
    fn body_plan_plant_knobs_from_kinded_traits() {
        let nuc = PixelTraits {
            alloc_stem: 0.1,
            alloc_leaf: 0.2,
            alloc_root: 0.7,
            ..PixelTraits::default()
        };
        let root = PixelTraits {
            root_depth_bias: 0.9,
            ..PixelTraits::default()
        };
        let leaf = PixelTraits {
            shade_efficiency: 0.7,
            absorb_bias: 1.5,
            ..PixelTraits::default()
        };
        let dig = PixelTraits {
            digest_rate: 1.4,
            ..PixelTraits::default()
        };
        let body = [
            placed(0, 0, ModuleId::Nucleus, nuc),
            placed(0, -1, ModuleId::Root, root),
            placed(0, 1, ModuleId::Photosystem, leaf),
            placed(1, 0, ModuleId::Digest, dig),
        ];
        let plan = body_plan_from(&body);
        assert!((plan.alloc_root - 0.7).abs() < 1e-5);
        assert!((plan.root_depth_bias - 0.9).abs() < 1e-5);
        assert!((plan.shade_efficiency - 0.7).abs() < 1e-5);
        assert!((plan.digest_rate - 1.4).abs() < 1e-5);
        assert!((plan.photo_capacity - 1.5).abs() < 1e-5);
        let (s, l, r) = plan.alloc_weights();
        assert!((s + l + r - 1.0).abs() < 1e-5);
        assert!(r > s && r > l);
    }
}
