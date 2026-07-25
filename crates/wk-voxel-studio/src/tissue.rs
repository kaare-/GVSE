//! Paint vocabulary for the studio body layer.
//!
//! RGB values for Bone / Muscle / Skin match `docs/organism/PALETTE.md`
//! reserved module slots so studio drawings and world modules share one
//! atlas. Fixture / joint / sensor kinds are studio-only.

use serde::{Deserialize, Serialize};

use crate::body::BodyGraph;
use crate::neural::StudioNet;

/// Default RGB mirrors (also used by tests for freeze checks).
pub const BONE_RGB: [u8; 3] = [0xEF, 0xE7, 0xDA];
pub const MUSCLE_RGB: [u8; 3] = [0xC3, 0x3C, 0x3C];
pub const SKIN_RGB: [u8; 3] = [0xFF, 0xDB, 0xAC];
pub const NERVE_RGB: [u8; 3] = [0xB0, 0x8A, 0x8A];
pub const NEURON_BLOB_RGB: [u8; 3] = [0x9A, 0x70, 0x70];
pub const FIXTURE_RGB: [u8; 3] = [0x2A, 0x2A, 0x2A];
pub const FORCE_SENSOR_RGB: [u8; 3] = [0x4A, 0x6F, 0xA5];

/// Joint hinge rotation limit as a fraction of a full turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum JointLimit {
    /// Free hinge (full rotation).
    Full = 0,
    ThreeQuarter = 1,
    Half = 2,
    Quarter = 3,
}

impl JointLimit {
    /// Max |angle| in turns (1.0 = 360°).
    pub fn max_turns(self) -> f32 {
        match self {
            Self::Full => 1.0,
            Self::ThreeQuarter => 0.75,
            Self::Half => 0.5,
            Self::Quarter => 0.25,
        }
    }
}

/// One painted / activated studio cell kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum TissueKind {
    #[default]
    Empty = 0,
    Bone = 1,
    Muscle = 2,
    Skin = 3,
    Nerve = 4,
    /// Nerve mass ≥2×2 — holds neurons / weights after activate.
    NeuronBlob = 5,
    /// Infinitely strong bench mount. Stripped on export.
    Fixture = 6,
    JointFull = 7,
    JointThreeQuarter = 8,
    JointHalf = 9,
    JointQuarter = 10,
    /// Uniaxial force sampler on a fixture. Stripped on export.
    ForceSensor = 11,
}

impl TissueKind {
    pub fn is_studio_only(self) -> bool {
        matches!(
            self,
            Self::Fixture
                | Self::JointFull
                | Self::JointThreeQuarter
                | Self::JointHalf
                | Self::JointQuarter
                | Self::ForceSensor
                | Self::Empty
        )
    }

    /// Joint kinds map to a rotation limit; others return `None`.
    pub fn joint_limit(self) -> Option<JointLimit> {
        match self {
            Self::JointFull => Some(JointLimit::Full),
            Self::JointThreeQuarter => Some(JointLimit::ThreeQuarter),
            Self::JointHalf => Some(JointLimit::Half),
            Self::JointQuarter => Some(JointLimit::Quarter),
            _ => None,
        }
    }

    /// Survives `.gvsebody` export (morphology + nerve tissue).
    pub fn exportable(self) -> bool {
        matches!(
            self,
            Self::Bone | Self::Muscle | Self::Skin | Self::Nerve | Self::NeuronBlob
        ) || self.joint_limit().is_some()
    }
}

/// Dense paint buffer in arena-local coordinates `(x, y)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TissuePaint {
    pub width: u32,
    pub height: u32,
    pub cells: Vec<TissueKind>,
}

impl TissuePaint {
    pub fn new(width: u32, height: u32) -> Self {
        let n = (width as usize).saturating_mul(height as usize);
        Self {
            width,
            height,
            cells: vec![TissueKind::Empty; n],
        }
    }

    pub fn index(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some((y as usize) * (self.width as usize) + (x as usize))
    }

    pub fn get(&self, x: u32, y: u32) -> TissueKind {
        self.index(x, y)
            .map(|i| self.cells[i])
            .unwrap_or(TissueKind::Empty)
    }

    pub fn set(&mut self, x: u32, y: u32, kind: TissueKind) {
        if let Some(i) = self.index(x, y) {
            self.cells[i] = kind;
        }
    }
}

/// Painted + optional activated body graph + optional neural controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioBody {
    pub paint: TissuePaint,
    /// True after a successful activate pass.
    pub activated: bool,
    /// Runtime rigid parts (S1+). Cleared when paint is edited.
    #[serde(default)]
    pub graph: Option<BodyGraph>,
    /// Frozen / trainable feed-forward controller (S4).
    #[serde(default)]
    pub net: Option<StudioNet>,
}

impl Default for StudioBody {
    fn default() -> Self {
        Self::from_paint(TissuePaint::new(0, 0))
    }
}

impl StudioBody {
    pub fn from_paint(paint: TissuePaint) -> Self {
        Self {
            paint,
            activated: false,
            graph: None,
            net: None,
        }
    }

    /// Editing paint invalidates the activated graph (keeps net weights
    /// until activate rebuilds topology).
    pub fn paint_set(&mut self, x: u32, y: u32, kind: TissueKind) {
        self.paint.set(x, y, kind);
        self.activated = false;
        self.graph = None;
    }
}

/// Force sensor sample (S3 wires this to fixture edges).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ForceSensor {
    pub x: i32,
    pub y: i32,
    /// Unit axis the sensor measures (arena space).
    pub dir_x: f32,
    pub dir_y: f32,
    pub last_force: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exportable_strips_fixture_and_sensor() {
        assert!(TissueKind::Bone.exportable());
        assert!(TissueKind::JointHalf.exportable());
        assert!(!TissueKind::Fixture.exportable());
        assert!(!TissueKind::ForceSensor.exportable());
        assert!(!TissueKind::Empty.exportable());
    }

    #[test]
    fn joint_limits_match_fractions() {
        assert_eq!(JointLimit::Full.max_turns(), 1.0);
        assert_eq!(JointLimit::Quarter.max_turns(), 0.25);
    }

    #[test]
    fn paint_round_trip_set_get() {
        let mut p = TissuePaint::new(8, 4);
        p.set(3, 1, TissueKind::Muscle);
        assert_eq!(p.get(3, 1), TissueKind::Muscle);
        assert_eq!(p.get(0, 0), TissueKind::Empty);
    }
}
