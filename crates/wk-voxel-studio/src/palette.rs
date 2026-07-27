//! Studio tissue colours — must stay aligned with
//! `docs/organism/PALETTE.md` for Bone / Muscle / Skin.

use crate::tissue::{
    TissueKind, BONE_RGB, FIXTURE_RGB, FORCE_SENSOR_RGB, JOINT_RGB, MUSCLE_RGB, NERVE_RGB,
    NEURON_BLOB_RGB, PRESSURE_ENDING_RGB, SKIN_RGB,
};

/// Joint overlay glyph id (drawn as a 1× tick mark in the app).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JointSymbol {
    Full,
    ThreeQuarter,
    Half,
    Quarter,
}

pub const JOINT_SYMBOL: &[(TissueKind, JointSymbol)] = &[
    (TissueKind::JointFull, JointSymbol::Full),
    (TissueKind::JointThreeQuarter, JointSymbol::ThreeQuarter),
    (TissueKind::JointHalf, JointSymbol::Half),
    (TissueKind::JointQuarter, JointSymbol::Quarter),
];

pub fn tissue_rgb(kind: TissueKind) -> Option<[u8; 3]> {
    match kind {
        TissueKind::Empty => None,
        TissueKind::Bone => Some(BONE_RGB),
        TissueKind::Muscle => Some(MUSCLE_RGB),
        TissueKind::Skin => Some(SKIN_RGB),
        TissueKind::Nerve => Some(NERVE_RGB),
        TissueKind::NeuronBlob => Some(NEURON_BLOB_RGB),
        TissueKind::Fixture => Some(FIXTURE_RGB),
        TissueKind::JointFull
        | TissueKind::JointThreeQuarter
        | TissueKind::JointHalf
        | TissueKind::JointQuarter => Some(JOINT_RGB),
        TissueKind::ForceSensor => Some(FORCE_SENSOR_RGB),
        TissueKind::PressureEnding => Some(PRESSURE_ENDING_RGB),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bone_muscle_skin_match_organism_palette_hex() {
        // docs/organism/PALETTE.md reserved slots 0x13–0x15.
        assert_eq!(tissue_rgb(TissueKind::Skin).unwrap(), [0xFF, 0xDB, 0xAC]);
        assert_eq!(tissue_rgb(TissueKind::Muscle).unwrap(), [0xC3, 0x3C, 0x3C]);
        assert_eq!(tissue_rgb(TissueKind::Bone).unwrap(), [0xEF, 0xE7, 0xDA]);
    }
}
