//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Set A organisms — Nucleus + Photosystem pixel blobs.
//!
//! Mirrors the column-GVSE organism kernel (see `docs/organism/` and
//! `wk-agents`), but lives entirely inside `wk-voxel` so the isolation
//! contract holds. Life is the drawing: two 1×1 modules, not a green
//! biomass wash over the terrain.
//!
//! Palette hex is frozen (`docs/organism/PALETTE.md`).

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::Cell;
use crate::grid::World;

/// Soft cap — blooms should stay readable at 1×.
pub const MAX_ATOMS: usize = 256;

/// Demo day length in ticks (shorter than column-GVSE so day/night
/// bloom is visible in a short play session).
pub const DEMO_DAY_TICKS: u64 = 1_200;

/// Energy gained per photosystem per tick at full noon light.
const PHOTON_RATE: f32 = 0.35;
/// Baseline upkeep per module per tick.
const UPKEEP_PER_MODULE: f32 = 0.04;
/// Fraction of tank spent to fission.
const REPRO_COST_FRAC: f32 = 0.45;
/// Minimum energy fraction of max to attempt fission.
const REPRODUCE_AT: f32 = 0.85;
/// Ticks between fission attempts.
const REPRO_PERIOD: u64 = 40;
/// Age soft-cap (ticks).
const LIFE_TICKS: u64 = DEMO_DAY_TICKS * 4;

/// Set A module IDs — values match `wk_agents::ModuleId` / PALETTE.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ModuleId {
    Nucleus = 0x00,
    Photosystem = 0x01,
}

impl ModuleId {
    /// Frozen RGB from `docs/organism/PALETTE.md`.
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            ModuleId::Nucleus => (0x00, 0x00, 0x00),
            ModuleId::Photosystem => (0x2E, 0xCC, 0x40),
        }
    }
}

/// One painted module relative to the organism pose (cell units).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedModule {
    pub dx: i16,
    pub dy: i16,
    pub module: ModuleId,
}

/// Canonical Atom blueprint: black nucleus + green photosystem.
pub fn atom_modules() -> [PlacedModule; 2] {
    [
        PlacedModule {
            dx: 0,
            dy: 0,
            module: ModuleId::Nucleus,
        },
        PlacedModule {
            dx: 1,
            dy: 0,
            module: ModuleId::Photosystem,
        },
    ]
}

/// One living Set A Atom in world cell space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Atom {
    /// Anchor cell (nucleus position).
    pub gx: i32,
    pub gy: i32,
    pub energy: f32,
    pub energy_max: f32,
    pub age_ticks: u64,
    pub cooldown: u64,
}

impl Atom {
    pub fn new(gx: i32, gy: i32, energy_max: f32) -> Self {
        Self {
            gx,
            gy,
            energy: energy_max * 0.6,
            energy_max,
            age_ticks: 0,
            cooldown: REPRO_PERIOD / 2,
        }
    }
}

/// Population of Set A Atoms (no `hecs` — keep the crate tiny).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrganismStore {
    pub atoms: Vec<Atom>,
}

impl OrganismStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.atoms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    /// Seed Atoms into wet Air cells near the free surface of each
    /// column in `[x0, x1)`. Deterministic on `(seed, gx)`.
    pub fn seed_coastal_atoms(
        &mut self,
        world: &World,
        seed: u64,
        x0: i32,
        x1: i32,
        y0: i32,
        y1: i32,
        stride: i32,
        energy_max: f32,
    ) {
        let stride = stride.max(1);
        let mut gx = x0;
        while gx < x1 {
            let gx_w = world.wrap_x(gx);
            if let Some(gy) = find_wet_slot(world, gx_w, y0, y1) {
                let h = hash_u64(seed, gx_w as u64, ATOM_SEED_SALT);
                if h % 3 == 0 && self.atoms.len() < MAX_ATOMS {
                    self.atoms.push(Atom::new(gx_w, gy, energy_max));
                }
            }
            gx += stride;
        }
    }

    /// Draw list: world cell + frozen module RGB.
    pub fn draw_list(&self) -> Vec<(i32, i32, (u8, u8, u8))> {
        let mut out = Vec::with_capacity(self.atoms.len() * 2);
        for atom in &self.atoms {
            for m in atom_modules() {
                out.push((
                    atom.gx + m.dx as i32,
                    atom.gy + m.dy as i32,
                    m.module.rgb(),
                ));
            }
        }
        out
    }

    /// One Set A step: light harvest, upkeep, drift, fission, death.
    ///
    /// Death deposits a small [`MaterialId::Organic`] speck at the
    /// nucleus cell when that cell is Air (otherwise the body simply
    /// vanishes — no column ecology litter bucket here).
    pub fn step(&mut self, world: &mut World, tick: u64) {
        if self.atoms.is_empty() {
            return;
        }
        let day = day_factor(tick);
        let mut births: Vec<Atom> = Vec::new();
        let mut deaths: Vec<usize> = Vec::new();
        let pop = self.atoms.len();

        for (i, atom) in self.atoms.iter_mut().enumerate() {
            atom.age_ticks = atom.age_ticks.saturating_add(1);
            atom.cooldown = atom.cooldown.saturating_sub(1);

            if atom.age_ticks >= LIFE_TICKS {
                deaths.push(i);
                continue;
            }

            // Stay in the wet column. The free-surface film often
            // drains under spill/seepage — snap back into water instead
            // of dying on a one-cell tide line.
            if !ensure_in_water(world, atom) {
                deaths.push(i);
                continue;
            }

            let light = column_light(world, atom.gx, atom.gy) * day;
            let harvest = PHOTON_RATE * light;
            let upkeep = UPKEEP_PER_MODULE * 2.0 * (0.45 + 0.55 * day);
            atom.energy = (atom.energy + harvest - upkeep).clamp(0.0, atom.energy_max);
            if atom.energy <= 0.0 {
                deaths.push(i);
                continue;
            }

            // Mild vertical drift inside the wet band only.
            drift_atom(world, atom);

            if atom.cooldown == 0
                && atom.energy >= atom.energy_max * REPRODUCE_AT
                && pop + births.len() < MAX_ATOMS
            {
                let cost = atom.energy_max * REPRO_COST_FRAC;
                atom.energy -= cost;
                atom.cooldown = REPRO_PERIOD;
                if let Some(child) = try_fission(world, atom, cost * 0.5) {
                    births.push(child);
                }
            }
        }

        // Apply deaths high-to-low index; deposit Organic speck.
        deaths.sort_unstable();
        deaths.dedup();
        for &i in deaths.iter().rev() {
            if let Some(dead) = self.atoms.get(i).cloned() {
                deposit_organic(world, dead.gx, dead.gy);
            }
            if i < self.atoms.len() {
                self.atoms.swap_remove(i);
            }
        }
        self.atoms.extend(births);
    }
}

/// 1 at noon, ~0.08 at night — readable bloom / thin cycle.
pub fn day_factor(tick: u64) -> f32 {
    let t = (tick % DEMO_DAY_TICKS) as f32 / DEMO_DAY_TICKS as f32;
    let angle = t * std::f32::consts::TAU;
    // Raised cosine: day half bright, night dim but not zero.
    (angle.cos() * 0.5 + 0.5).clamp(0.08, 1.0)
}

fn column_light(world: &World, gx: i32, gy: i32) -> f32 {
    // Walk up; each wet/air cell transmits, solids block.
    let mut light = 1.0f32;
    let mut y = gy + 1;
    let mut steps = 0;
    while steps < 64 {
        match world.get_cell(gx, y) {
            None => return light,
            Some(c) if c.material == MaterialId::Air => {
                // Turbid water attenuates a little.
                if !c.sat.is_empty() {
                    light *= 0.97;
                }
            }
            Some(_) => return light * 0.15, // buried / under rock
        }
        y += 1;
        steps += 1;
    }
    light
}

fn is_wet_air(world: &World, gx: i32, gy: i32) -> bool {
    match world.get_cell(gx, gy) {
        Some(c) => c.material == MaterialId::Air && !c.sat.is_empty(),
        None => false,
    }
}

fn find_wet_slot(world: &World, gx: i32, y0: i32, y1: i32) -> Option<i32> {
    // Top of the wet column, then step down a few cells so we don't
    // sit on the free-surface film that spill/seepage drains first.
    let mut surface = None;
    let mut y = y1 - 1;
    while y >= y0 {
        if is_wet_air(world, gx, y) {
            surface = Some(y);
            break;
        }
        y -= 1;
    }
    let top = surface?;
    const DEPTH: i32 = 3;
    for d in (0..=DEPTH).rev() {
        let gy = top - d;
        if gy >= y0 && is_wet_air(world, gx, gy) {
            return Some(gy);
        }
    }
    Some(top)
}

/// If the atom's cell dried, move to a nearby wet Air cell. Returns
/// false only when no water remains in the local column.
fn ensure_in_water(world: &World, atom: &mut Atom) -> bool {
    if is_wet_air(world, atom.gx, atom.gy) {
        return true;
    }
    for dy in [-1, 1, -2, 2, -3, 3, -4, 4] {
        let ny = atom.gy + dy;
        if is_wet_air(world, atom.gx, ny) {
            atom.gy = ny;
            return true;
        }
    }
    false
}

fn drift_atom(world: &World, atom: &mut Atom) {
    // Bias one cell toward brighter wet neighbour; never leave water.
    let up = atom.gy + 1;
    let down = atom.gy - 1;
    if is_wet_air(world, atom.gx, up) {
        let light_up = column_light(world, atom.gx, up);
        let light_here = column_light(world, atom.gx, atom.gy);
        if light_up >= light_here {
            atom.gy = up;
            return;
        }
    }
    if !is_wet_air(world, atom.gx, atom.gy) && is_wet_air(world, atom.gx, down) {
        atom.gy = down;
    }
}

fn try_fission(world: &World, parent: &Atom, child_energy: f32) -> Option<Atom> {
    // Place offspring on a free wet neighbour.
    for (dx, dy) in [(2, 0), (-2, 0), (0, 1), (0, -1), (3, 0), (-1, 0)] {
        let nx = world.wrap_x(parent.gx + dx);
        let ny = parent.gy + dy;
        let Some(c) = world.get_cell(nx, ny) else {
            continue;
        };
        if c.material == MaterialId::Air && !c.sat.is_empty() {
            let mut child = Atom::new(nx, ny, parent.energy_max);
            child.energy = child_energy.clamp(1.0, parent.energy_max);
            child.cooldown = REPRO_PERIOD;
            return Some(child);
        }
    }
    None
}

fn deposit_organic(world: &mut World, gx: i32, gy: i32) {
    // Drop residue on the first solid below — never replace a water
    // cell with Organic (that was deleting the whole plankton band).
    let mut y = gy;
    for _ in 0..64 {
        match world.get_cell(gx, y) {
            Some(c) if c.material != MaterialId::Air => {
                // Speck in the Air cell just above the bed, if empty.
                if let Some(above) = world.get_cell(gx, y + 1) {
                    if above.material == MaterialId::Air && above.sat.is_empty() {
                        world.set_cell(gx, y + 1, Cell::solid(MaterialId::Organic));
                    }
                }
                return;
            }
            None => return,
            Some(_) => y -= 1,
        }
    }
}

const ATOM_SEED_SALT: u64 = 0xA701_5EED;

fn hash_u64(seed: u64, a: u64, salt: u64) -> u64 {
    let mut x = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(a)
        .wrapping_add(salt);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Sat;
    use crate::chunk::ChunkCoord;

    fn wet_column() -> World {
        let mut w = World::new(7);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..8 {
                let mut water = Cell::air();
                water.sat = Sat(255);
                w.set_cell(x, y, water);
            }
            // Open air above.
            for y in 8..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w
    }

    #[test]
    fn atom_draw_list_is_black_and_green_pixels() {
        let mut store = OrganismStore::new();
        store.atoms.push(Atom::new(4, 5, 50.0));
        let list = store.draw_list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], (4, 5, (0, 0, 0)));
        assert_eq!(list[1], (5, 5, (0x2E, 0xCC, 0x40)));
    }

    #[test]
    fn atoms_harvest_and_can_fission_in_lit_water() {
        let mut w = wet_column();
        let mut store = OrganismStore::new();
        store.atoms.push(Atom::new(4, 6, 20.0));
        store.atoms[0].energy = 20.0;
        store.atoms[0].cooldown = 0;
        // Noon-ish ticks.
        for t in 0..80 {
            store.step(&mut w, t);
        }
        assert!(
            store.len() >= 1,
            "founder should survive in lit water"
        );
        // With full tank + repro, expect at least one child eventually.
        assert!(
            store.len() >= 2 || store.atoms[0].energy < 20.0,
            "should spend energy on life / fission"
        );
    }

    #[test]
    fn dry_column_with_no_water_kills_atom() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for y in 0..12 {
            w.set_cell(4, y, Cell::air()); // fully dry column
        }
        let mut store = OrganismStore::new();
        store.atoms.push(Atom::new(4, 6, 50.0));
        store.step(&mut w, 0);
        assert!(store.is_empty());
    }

    #[test]
    fn death_can_deposit_organic_above_bed() {
        let mut w = wet_column();
        // Dry air pocket just above bedrock so residue has a place.
        w.set_cell(4, 1, Cell::air());
        let mut store = OrganismStore::new();
        let mut a = Atom::new(4, 6, 10.0);
        a.age_ticks = LIFE_TICKS; // force age death
        store.atoms.push(a);
        store.step(&mut w, 0);
        assert!(store.is_empty());
        assert_eq!(
            w.get_cell(4, 1).map(|c| c.material),
            Some(MaterialId::Organic),
            "corpse residue should sit on the bed, not replace water"
        );
        // Water column where the atom lived stays wet Air.
        assert_eq!(
            w.get_cell(4, 6).map(|c| c.material),
            Some(MaterialId::Air)
        );
    }

    #[test]
    fn demo_atoms_survive_physics_ticks() {
        use crate::worldgen::{stamp_world, WorldgenParams};
        let params = WorldgenParams::default();
        let mut world = World::new(params.seed);
        stamp_world(&mut world, &params);
        let mut store = OrganismStore::new();
        store.seed_coastal_atoms(
            &world,
            params.seed,
            0,
            params.width_cols,
            params.bedrock_floor_y,
            params.sky_ceiling_y,
            4,
            40.0,
        );
        let n0 = store.len();
        assert!(n0 > 0);
        for t in 0..180u64 {
            store.step(&mut world, t);
            crate::rules::tick(&mut world);
        }
        assert!(
            store.len() > 0,
            "Atoms must survive free-surface spill; started with {n0}"
        );
    }

    #[test]
    fn seed_places_atoms_in_wet_cells() {
        let w = wet_column();
        let mut store = OrganismStore::new();
        store.seed_coastal_atoms(&w, 1, 0, 16, 0, 12, 2, 40.0);
        assert!(!store.is_empty());
        for a in &store.atoms {
            let c = w.get_cell(a.gx, a.gy).unwrap();
            assert_eq!(c.material, MaterialId::Air);
            assert!(!c.sat.is_empty());
        }
    }
}
