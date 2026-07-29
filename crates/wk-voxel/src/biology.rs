//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Biological world materials (Wave L): ModuleId → MaterialId corpse
//! routing when a body dissolves into the terrain.

use wk_material::MaterialId;

use crate::organism::ModuleId;

/// World material left when a module dies / dissolves.
///
/// Plant tissues and metabolic organs still compost to Organic.
/// Bone / Muscle / Skin keep their identity so differential decay
/// (see [`crate::rules::apply_biological_decay`]) can act on them.
pub fn module_death_material(module: ModuleId) -> MaterialId {
    match module {
        ModuleId::Bone => MaterialId::Bone,
        ModuleId::Muscle => MaterialId::Muscle,
        ModuleId::Skin => MaterialId::Skin,
        ModuleId::Nucleus
        | ModuleId::Photosystem
        | ModuleId::Digest
        | ModuleId::Hypha
        | ModuleId::Root
        | ModuleId::Stem => MaterialId::Organic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpse_of_bone_muscle_leaves_kind_specific_cells() {
        assert_eq!(module_death_material(ModuleId::Bone), MaterialId::Bone);
        assert_eq!(module_death_material(ModuleId::Muscle), MaterialId::Muscle);
        assert_eq!(module_death_material(ModuleId::Skin), MaterialId::Skin);
        assert_eq!(module_death_material(ModuleId::Root), MaterialId::Organic);
        assert_eq!(module_death_material(ModuleId::Stem), MaterialId::Organic);
        assert_eq!(
            module_death_material(ModuleId::Photosystem),
            MaterialId::Organic
        );
    }
}
