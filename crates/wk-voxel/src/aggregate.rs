//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Body-plan aggregates over a pixel-gene list (Wave K).
//!
//! Global organism scalars used to live on [`crate::blueprint::Genome`].
//! They now derive from per-pixel [`crate::blueprint::PixelTraits`] so
//! every painted module contributes to mass, metabolism, and fidelity.

use crate::blueprint::{PixelTraits, PlacedModule};
use crate::organism::ModuleId;

/// Cached body-plan derived from a pixel list.
///
/// Formulas are first-cut gameplay knobs — expect a tuning wave when
/// physics starts reading these instead of the vestigial `Genome`.
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
        }
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

    for p in pixels {
        let t = p.traits();
        let mass = (t.mass * t.density).max(0.0);
        total_mass += mass;
        upkeep_sum += t.upkeep_bias.max(0.0);
        fidelity_acc += t.clone_fidelity_bias.clamp(0.0, 1.0) * mass.max(1e-6);
        repro_acc += t.reproduce_at_bias.clamp(0.0, 1.0) * mass.max(1e-6);
        buoy_acc += t.buoyancy_bias.clamp(0.0, 1.0) * mass.max(1e-6);
        if p.module() == ModuleId::Photosystem {
            photo_capacity += t.absorb_bias.max(0.0);
        }
        if p.module() == ModuleId::Nucleus {
            nucleus_count += 1;
        }
        pixel_count += 1;
    }

    if pixel_count == 0 {
        return BodyPlan::default();
    }

    let mass_norm = total_mass.max(1e-6);
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
}
