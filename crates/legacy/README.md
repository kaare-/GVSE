# Legacy column stack (archive)

These crates (`wk-world`, `wk-sim`, `wk-agents`, `wk-io`, `wk-app`,
`wk-field`) are the **superseded** column-based sim. They remain in
the workspace as reference and for the column scenario suite in
`tests/scenarios/` (including scripted grazer E16/E17).

**Active development** is `crates/wk-voxel` + `crates/wk-voxel-app`.
See [`docs/VOXEL_MIGRATION.md`](../../docs/VOXEL_MIGRATION.md).

Related design records marked archive-only:

- [`docs/AGENTS.md`](../../docs/AGENTS.md) — scripted grazer ECS
- [`docs/ECOLOGY.md`](../../docs/ECOLOGY.md) — per-column `Ecology` bucket
- [`docs/EVOLUTION.md`](../../docs/EVOLUTION.md) — grazer fission / mutation
- [`docs/BURROWS.md`](../../docs/BURROWS.md) — dig / void API

Organism Kernel specs under [`docs/organism/`](../../docs/organism/)
point here for historical hooks; the live petri is `wk-voxel-app`.

Do not add new features here. Do not depend on these crates from
`wk-voxel` / `wk-voxel-app`.
