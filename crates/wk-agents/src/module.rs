//! Frozen module IDs and palette hex (see `docs/organism/PALETTE.md`).

use serde::{Deserialize, Serialize};

/// Depth lane for occupancy / draw order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum LaneId {
    Fore = 0,
    #[default]
    Mid = 1,
    Back = 2,
}

/// Atomic organelle / body part. `#[repr(u8)]` values are save-stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ModuleId {
    Nucleus = 0x00,
    Photosystem = 0x01,
    Chemosystem = 0x02,
    ChemoSensor = 0x03,
    ChemoEmitter = 0x04,
    NeuralSoma = 0x05,
    Axon = 0x06,
    Buoyancy = 0x07,
    TempTolerance = 0x08,
    Store = 0x09,
    Digest = 0x0A,
    Hypha = 0x0B,
    Motility = 0x0C,
    Root = 0x0D,
    Stem = 0x0E,
    Holdfast = 0x0F,
    // Reserved slots (not used in Set A).
    ReproSpore = 0x10,
    Fruit = 0x11,
    Bark = 0x12,
    Skin = 0x13,
    Muscle = 0x14,
    Bone = 0x15,
}

impl ModuleId {
    /// Modules the Set A editor may paint.
    pub fn set_a_paintable(self) -> bool {
        matches!(self, ModuleId::Nucleus | ModuleId::Photosystem)
    }

    /// Set A + land-plant modules (Root / Stem).
    pub fn set_d_paintable(self) -> bool {
        matches!(
            self,
            ModuleId::Nucleus | ModuleId::Photosystem | ModuleId::Root | ModuleId::Stem
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            ModuleId::Nucleus => "Nucleus",
            ModuleId::Photosystem => "Photosystem",
            ModuleId::Chemosystem => "Chemosystem",
            ModuleId::ChemoSensor => "ChemoSensor",
            ModuleId::ChemoEmitter => "ChemoEmitter",
            ModuleId::NeuralSoma => "NeuralSoma",
            ModuleId::Axon => "Axon",
            ModuleId::Buoyancy => "Buoyancy",
            ModuleId::TempTolerance => "TempTolerance",
            ModuleId::Store => "Store",
            ModuleId::Digest => "Digest",
            ModuleId::Hypha => "Hypha",
            ModuleId::Motility => "Motility",
            ModuleId::Root => "Root",
            ModuleId::Stem => "Stem",
            ModuleId::Holdfast => "Holdfast",
            ModuleId::ReproSpore => "ReproSpore",
            ModuleId::Fruit => "Fruit",
            ModuleId::Bark => "Bark",
            ModuleId::Skin => "Skin",
            ModuleId::Muscle => "Muscle",
            ModuleId::Bone => "Bone",
        }
    }

    /// Frozen RGB from `docs/organism/PALETTE.md`.
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            ModuleId::Nucleus => (0x00, 0x00, 0x00),
            ModuleId::Photosystem => (0x2E, 0xCC, 0x40),
            ModuleId::Chemosystem => (0xB5, 0x89, 0x00),
            ModuleId::ChemoSensor => (0x0A, 0x6C, 0x74),
            ModuleId::ChemoEmitter => (0x39, 0xCC, 0xCC),
            ModuleId::NeuralSoma => (0x7F, 0x7F, 0x7F),
            ModuleId::Axon => (0xAA, 0xAA, 0xAA),
            ModuleId::Buoyancy => (0x7F, 0xDB, 0xFF),
            ModuleId::TempTolerance => (0xFF, 0x85, 0x1B),
            ModuleId::Store => (0xEF, 0xEF, 0xEF),
            ModuleId::Digest => (0x8B, 0x2E, 0x2E),
            ModuleId::Hypha => (0xF1, 0xE6, 0xC4),
            ModuleId::Motility => (0xB1, 0x0D, 0xC9),
            ModuleId::Root => (0x7A, 0x4B, 0x2A),
            ModuleId::Stem => (0x55, 0x6B, 0x2F),
            ModuleId::Holdfast => (0xFF, 0x3D, 0x9A),
            ModuleId::ReproSpore => (0xD0, 0xB0, 0xFF),
            ModuleId::Fruit => (0xE8, 0x5D, 0x75),
            ModuleId::Bark => (0x3E, 0x2E, 0x1F),
            ModuleId::Skin => (0xFF, 0xDB, 0xAC),
            ModuleId::Muscle => (0xC3, 0x3C, 0x3C),
            ModuleId::Bone => (0xEF, 0xE7, 0xDA),
        }
    }
}
