# Legacy column stack

These crates (`wk-world`, `wk-sim`, `wk-agents`, `wk-io`, `wk-app`,
`wk-field`) are the **superseded** column-based sim. They remain in
the workspace as reference and for `tests/scenarios/`.

**Active development** is `crates/wk-voxel` + `crates/wk-voxel-app`.
See [`docs/VOXEL_MIGRATION.md`](../../docs/VOXEL_MIGRATION.md).

Do not add new features here. Do not depend on these crates from
`wk-voxel` / `wk-voxel-app`.
