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
//! Buoyancy is a slim port of column plankton physics: weight vs
//! float bias, circadian day-float / night-sink, fission jitter on
//! `buoyancy_bias`, and a light contact bounce so blooms don't stack
//! into one glued surface film.
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

/// Floater equilibrium depth below the free surface (cells).
const FLOAT_DEPTH: f32 = 1.5;
/// Column buoyancy constants (cell units / tick).
const GRAVITY: f32 = 0.08;
const WATER_DRAG: f32 = 0.25;
const AIR_DRAG: f32 = 0.05;
const EQ_SPRING: f32 = 0.12;
/// Gene jitter scale on fission — matches column `MUTATION_SIGMA`.
const MUTATION_SIGMA: f32 = 0.12;
/// Soft contact impulse when two Atoms share a cell.
const CONTACT_BOUNCE: f32 = 0.12;

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

    pub fn name(self) -> &'static str {
        match self {
            ModuleId::Nucleus => "Nucleus",
            ModuleId::Photosystem => "Photosystem",
        }
    }
}

/// Body module as offset from the nucleus anchor.
pub type BodyModule = (i16, i16, ModuleId);

fn default_atom_body() -> Vec<BodyModule> {
    vec![(0, 0, ModuleId::Nucleus), (1, 0, ModuleId::Photosystem)]
}

/// One living Set A organism in world cell space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Atom {
    /// Anchor cell (nucleus position).
    pub gx: i32,
    pub gy: i32,
    /// Continuous vertical pose (synced into `gy` each tick).
    pub fy: f32,
    pub vel_y: f32,
    pub energy: f32,
    pub energy_max: f32,
    pub age_ticks: u64,
    pub cooldown: u64,
    /// 0 = floater, 1 = sinker (column `Genome::buoyancy_bias`).
    pub buoyancy_bias: f32,
    /// High → children stay close to parent genes.
    pub clone_fidelity: f32,
    pub circadian_phase: f32,
    pub active_window: f32,
    /// Free-surface cell y last tick (ride rising / falling water).
    pub last_water_top: Option<i32>,
    /// Modules relative to `(gx, gy)`.
    pub body: Vec<BodyModule>,
}

impl Atom {
    pub fn new(gx: i32, gy: i32, energy_max: f32) -> Self {
        Self {
            gx,
            gy,
            fy: gy as f32,
            vel_y: 0.0,
            energy: energy_max * 0.6,
            energy_max,
            age_ticks: 0,
            cooldown: REPRO_PERIOD / 2,
            buoyancy_bias: 0.0,
            clone_fidelity: 0.9,
            circadian_phase: 0.25,
            active_window: 0.55,
            last_water_top: None,
            body: default_atom_body(),
        }
    }

    pub fn from_body(gx: i32, gy: i32, energy_max: f32, body: Vec<BodyModule>) -> Self {
        let mut a = Self::new(gx, gy, energy_max);
        if !body.is_empty() {
            a.body = body;
        }
        a
    }

    pub fn photosystem_count(&self) -> usize {
        self.body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .count()
    }

    pub fn occupies(&self, wx: i32, wy: i32) -> bool {
        self.body
            .iter()
            .any(|(dx, dy, _)| self.gx + *dx as i32 == wx && self.gy + *dy as i32 == wy)
    }

    fn body_top_offset(&self) -> f32 {
        self.body
            .iter()
            .map(|(_, dy, _)| *dy as f32)
            .fold(0.0f32, f32::max)
            .max(0.0)
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
            for &(dx, dy, mid) in &atom.body {
                out.push((atom.gx + dx as i32, atom.gy + dy as i32, mid.rgb()));
            }
        }
        out
    }

    /// Spawn a painted blueprint with nucleus at `(gx, gy)`.
    /// Prefers a wet cell at/near the click; returns false if none.
    pub fn spawn_blueprint(
        &mut self,
        world: &World,
        gx: i32,
        gy: i32,
        body: Vec<BodyModule>,
        energy_max: f32,
        buoyancy_bias: f32,
        clone_fidelity: f32,
    ) -> bool {
        if self.atoms.len() >= MAX_ATOMS || body.is_empty() {
            return false;
        }
        let gx = world.wrap_x(gx);
        let gy = if is_wet_air(world, gx, gy) {
            gy
        } else if let Some(slot) = find_wet_near(world, gx, gy) {
            slot
        } else {
            return false;
        };
        let mut atom = Atom::from_body(gx, gy, energy_max, body);
        atom.buoyancy_bias = buoyancy_bias.clamp(0.0, 1.0);
        atom.clone_fidelity = clone_fidelity.clamp(0.05, 1.0);
        if let Some((top, _)) = wet_band(world, gx, gy) {
            atom.last_water_top = Some(top);
        }
        self.atoms.push(atom);
        true
    }

    /// First organism occupying world cell `(gx, gy)`.
    pub fn pick_at(&self, gx: i32, gy: i32) -> Option<usize> {
        self.atoms.iter().position(|a| a.occupies(gx, gy))
    }

    /// One Set A step: buoyancy, light harvest, upkeep, fission, death,
    /// then a light contact bounce.
    pub fn step(&mut self, world: &mut World, tick: u64) {
        if self.atoms.is_empty() {
            return;
        }
        let day = day_factor(tick);
        let phase = phase_fraction(tick);
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

            // Drought gate: must still have a wet band nearby.
            if wet_band(world, atom.gx, atom.gy).is_none() {
                if !ensure_in_water(world, atom) {
                    deaths.push(i);
                    continue;
                }
            }

            let bias = circadian_buoyancy_bias(atom, phase);
            step_buoyancy(world, atom, bias);

            if !is_wet_air(world, atom.gx, atom.gy) {
                if !ensure_in_water(world, atom) {
                    deaths.push(i);
                    continue;
                }
            }

            let n_photo = atom.photosystem_count().max(1) as f32;
            let n_mod = atom.body.len().max(1) as f32;
            let light = column_light(world, atom.gx, atom.gy) * day;
            let harvest = PHOTON_RATE * light * n_photo;
            let upkeep = UPKEEP_PER_MODULE * n_mod * (0.45 + 0.55 * day);
            atom.energy = (atom.energy + harvest - upkeep).clamp(0.0, atom.energy_max);
            if atom.energy <= 0.0 {
                deaths.push(i);
                continue;
            }

            if atom.cooldown == 0
                && atom.energy >= atom.energy_max * REPRODUCE_AT
                && pop + births.len() < MAX_ATOMS
            {
                let cost = atom.energy_max * REPRO_COST_FRAC;
                atom.energy -= cost;
                atom.cooldown = REPRO_PERIOD;
                if let Some(child) = try_fission(world, atom, cost * 0.5, tick) {
                    births.push(child);
                } else {
                    atom.energy += cost;
                }
            }
        }

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
        resolve_contacts(world, &mut self.atoms);
    }
}

/// 1 at noon, ~0.08 at night — readable bloom / thin cycle.
pub fn day_factor(tick: u64) -> f32 {
    let t = phase_fraction(tick);
    let angle = t * std::f32::consts::TAU;
    (angle.cos() * 0.5 + 0.5).clamp(0.08, 1.0)
}

fn phase_fraction(tick: u64) -> f32 {
    (tick % DEMO_DAY_TICKS) as f32 / DEMO_DAY_TICKS as f32
}

/// Relative density: bias 0 → buoyant (0.55), bias 1 → heavy (1.45).
fn relative_density(bias: f32) -> f32 {
    0.55 + bias.clamp(0.0, 1.0) * 0.90
}

/// Day / active window → float side; night → deeper (column E33 style).
fn circadian_buoyancy_bias(atom: &Atom, phase: f32) -> f32 {
    let bias = atom.buoyancy_bias.clamp(0.0, 1.0);
    if circadian_active(atom.circadian_phase, atom.active_window, phase) {
        bias * 0.35
    } else {
        0.55 + bias * 0.45
    }
}

fn circadian_active(circadian_phase: f32, active_window: f32, phase: f32) -> bool {
    let window = active_window.clamp(0.05, 1.0);
    let mut d = (phase - circadian_phase).abs();
    if d > 0.5 {
        d = 1.0 - d;
    }
    d <= window * 0.5
}

fn equilibrium_y(top: i32, bed: i32, bias: f32) -> f32 {
    let top_f = top as f32;
    let bed_f = bed as f32;
    let float_y = (top_f - FLOAT_DEPTH).clamp(bed_f, top_f);
    float_y + (bed_f - float_y) * bias.clamp(0.0, 1.0)
}

/// Contiguous wet-Air band containing `hint_y` (or nearest wet cell).
fn wet_band(world: &World, gx: i32, hint_y: i32) -> Option<(i32, i32)> {
    let start = if is_wet_air(world, gx, hint_y) {
        hint_y
    } else {
        find_wet_near(world, gx, hint_y)?
    };
    let mut top = start;
    while is_wet_air(world, gx, top + 1) {
        top += 1;
        if top - start > 256 {
            break;
        }
    }
    let mut bed = start;
    while is_wet_air(world, gx, bed - 1) {
        bed -= 1;
        if start - bed > 256 {
            break;
        }
    }
    Some((top, bed))
}

fn step_buoyancy(world: &World, atom: &mut Atom, bias: f32) {
    let Some((top, bed)) = wet_band(world, atom.gx, atom.gy) else {
        atom.last_water_top = None;
        return;
    };
    let dens = relative_density(bias);
    let offset = atom.body_top_offset();
    let eq = equilibrium_y(top, bed, bias) - offset;

    // Ride free-surface change (rising tide lifts floaters with it).
    if let Some(prev) = atom.last_water_top {
        let delta = (top - prev) as f32;
        if delta != 0.0 {
            let body_top = atom.fy + offset;
            let was_in = body_top <= prev as f32 + 0.5 && atom.fy >= bed as f32 - 0.5;
            if was_in {
                if delta > 0.0 {
                    atom.fy += delta;
                } else if dens < 1.0 {
                    // Falling surface: floaters follow down a little.
                    let near_float = (atom.fy - (prev as f32 - FLOAT_DEPTH)).abs() < 2.0;
                    if near_float {
                        atom.fy = (atom.fy + delta).max(bed as f32);
                    }
                }
            }
        }
    }
    atom.last_water_top = Some(top);

    let body_top = atom.fy + offset;
    if body_top > top as f32 + 0.05 {
        // In air — fall back in.
        atom.vel_y -= GRAVITY;
        atom.vel_y *= 1.0 - AIR_DRAG;
        atom.fy += atom.vel_y;
        if atom.fy + offset <= top as f32 {
            atom.vel_y *= 0.4; // splash damping
        }
    } else {
        let accel = GRAVITY * (1.0 - dens) + (eq - atom.fy) * EQ_SPRING;
        atom.vel_y += accel;
        atom.vel_y *= 1.0 - WATER_DRAG;
        atom.fy += atom.vel_y;
        if atom.fy < bed as f32 {
            atom.fy = bed as f32;
            atom.vel_y = atom.vel_y.max(0.0);
        }
        if dens < 1.0 && atom.fy + offset > top as f32 {
            atom.fy = top as f32 - offset;
            atom.vel_y = atom.vel_y.min(0.0);
        }
    }

    // Soft settle near equilibrium so floaters don't jitter.
    if atom.vel_y.abs() < 0.02 && (atom.fy - eq).abs() < 0.08 {
        atom.fy = eq;
        atom.vel_y = 0.0;
    }

    atom.fy = atom.fy.clamp(bed as f32, top as f32);
    atom.gy = atom.fy.round() as i32;
    // Keep nucleus on a wet cell after rounding.
    if !is_wet_air(world, atom.gx, atom.gy) {
        atom.gy = atom.gy.clamp(bed, top);
        if !is_wet_air(world, atom.gx, atom.gy) {
            atom.gy = ((eq).round() as i32).clamp(bed, top);
        }
        atom.fy = atom.gy as f32;
    }
}

/// Prefer a one-cell horizontal shove; tiny vertical bounce if stuck.
fn resolve_contacts(world: &World, atoms: &mut [Atom]) {
    let n = atoms.len();
    if n < 2 {
        return;
    }
    for _ in 0..4 {
        for i in 0..n {
            for j in (i + 1)..n {
                if !bodies_overlap(&atoms[i], &atoms[j]) {
                    continue;
                }
                let dir = if atoms[j].gx >= atoms[i].gx { 1 } else { -1 };
                let try_x = world.wrap_x(atoms[j].gx + dir);
                if is_wet_air(world, try_x, atoms[j].gy)
                    && !occupied_by_other(atoms, j, try_x, atoms[j].gy)
                {
                    atoms[j].gx = try_x;
                    continue;
                }
                let try_x2 = world.wrap_x(atoms[i].gx - dir);
                if is_wet_air(world, try_x2, atoms[i].gy)
                    && !occupied_by_other(atoms, i, try_x2, atoms[i].gy)
                {
                    atoms[i].gx = try_x2;
                    continue;
                }
                // Tiny buoyancy bounce so stacked floaters separate in y.
                atoms[i].vel_y -= CONTACT_BOUNCE;
                atoms[j].vel_y += CONTACT_BOUNCE;
                atoms[i].fy -= CONTACT_BOUNCE * 0.5;
                atoms[j].fy += CONTACT_BOUNCE * 0.5;
                if let Some((top, bed)) = wet_band(world, atoms[i].gx, atoms[i].gy) {
                    atoms[i].fy = atoms[i].fy.clamp(bed as f32, top as f32);
                    atoms[i].gy = atoms[i].fy.round() as i32;
                }
                if let Some((top, bed)) = wet_band(world, atoms[j].gx, atoms[j].gy) {
                    atoms[j].fy = atoms[j].fy.clamp(bed as f32, top as f32);
                    atoms[j].gy = atoms[j].fy.round() as i32;
                }
            }
        }
    }
}

fn bodies_overlap(a: &Atom, b: &Atom) -> bool {
    for &(dx, dy, _) in &a.body {
        let ax = a.gx + dx as i32;
        let ay = a.gy + dy as i32;
        if b.occupies(ax, ay) {
            return true;
        }
    }
    false
}

fn occupied_by_other(atoms: &[Atom], self_i: usize, gx: i32, gy: i32) -> bool {
    atoms
        .iter()
        .enumerate()
        .any(|(k, a)| k != self_i && a.occupies(gx, gy))
}

fn column_light(world: &World, gx: i32, gy: i32) -> f32 {
    let mut light = 1.0f32;
    let mut y = gy + 1;
    let mut steps = 0;
    while steps < 64 {
        match world.get_cell(gx, y) {
            None => return light,
            Some(c) if c.material == MaterialId::Air => {
                if !c.sat.is_empty() {
                    light *= 0.97;
                }
            }
            Some(_) => return light * 0.15,
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
    // Seed at floater equilibrium depth, not the draining film.
    let target = top - FLOAT_DEPTH.round() as i32;
    for d in 0..=4 {
        for gy in [target - d, target + d] {
            if gy >= y0 && gy <= top && is_wet_air(world, gx, gy) {
                return Some(gy);
            }
        }
    }
    Some(top)
}

fn ensure_in_water(world: &World, atom: &mut Atom) -> bool {
    if is_wet_air(world, atom.gx, atom.gy) {
        return true;
    }
    if let Some(ny) = find_wet_near(world, atom.gx, atom.gy) {
        atom.gy = ny;
        atom.fy = ny as f32;
        atom.vel_y = 0.0;
        return true;
    }
    false
}

fn try_fission(world: &World, parent: &Atom, child_energy: f32, tick: u64) -> Option<Atom> {
    for (dx, dy) in [(2, 0), (-2, 0), (0, 1), (0, -1), (3, 0), (-1, 0)] {
        let nx = world.wrap_x(parent.gx + dx);
        let ny = parent.gy + dy;
        if is_wet_air(world, nx, ny) {
            let mut child =
                Atom::from_body(nx, ny, parent.energy_max, parent.body.clone());
            child.energy = child_energy.clamp(1.0, parent.energy_max);
            child.cooldown = REPRO_PERIOD;
            child.circadian_phase = parent.circadian_phase;
            child.active_window = parent.active_window;
            child.last_water_top = parent.last_water_top;
            // Mutate buoyancy (and fidelity a little) on clone.
            let strength = (1.0 - parent.clone_fidelity.clamp(0.0, 1.0)) * MUTATION_SIGMA;
            let j_b = hash_signed(tick, parent.gx as u64, parent.gy as u64, 0xB0A7);
            let j_f = hash_signed(tick, parent.gx as u64, parent.age_ticks, 0xF1DE);
            child.buoyancy_bias =
                (parent.buoyancy_bias + j_b * strength * 2.0).clamp(0.0, 1.0);
            child.clone_fidelity =
                (parent.clone_fidelity + j_f * strength).clamp(0.05, 1.0);
            return Some(child);
        }
    }
    None
}

fn find_wet_near(world: &World, gx: i32, gy: i32) -> Option<i32> {
    if is_wet_air(world, gx, gy) {
        return Some(gy);
    }
    for dy in [-1, 1, -2, 2, -3, 3, -4, 4, -5, 5, -8, 8] {
        let ny = gy + dy;
        if is_wet_air(world, gx, ny) {
            return Some(ny);
        }
    }
    None
}

fn deposit_organic(world: &mut World, gx: i32, gy: i32) {
    let mut y = gy;
    for _ in 0..64 {
        match world.get_cell(gx, y) {
            Some(c) if c.material != MaterialId::Air => {
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

/// Deterministic signed noise in [-1, 1].
fn hash_signed(a: u64, b: u64, c: u64, salt: u64) -> f32 {
    let h = hash_u64(a ^ b, c, salt);
    let u = (h >> 40) as f32 / ((1u64 << 24) as f32);
    u * 2.0 - 1.0
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
        assert!(list.contains(&(4, 5, (0, 0, 0))));
        assert!(list.contains(&(5, 5, (0x2E, 0xCC, 0x40))));
    }

    #[test]
    fn floater_settles_below_free_surface() {
        let mut w = wet_column();
        let mut store = OrganismStore::new();
        // Start glued to the surface film (y=7 is top wet).
        store.atoms.push(Atom::new(4, 7, 50.0));
        for t in 0..40 {
            store.step(&mut w, t);
        }
        let a = &store.atoms[0];
        assert!(
            a.gy < 7,
            "floater should leave the surface film, gy={}",
            a.gy
        );
        assert!(a.gy >= 1, "still in the wet column");
    }

    #[test]
    fn sinker_goes_deeper_than_floater() {
        let mut w = wet_column();
        let mut store = OrganismStore::new();
        let mut floater = Atom::new(4, 7, 50.0);
        floater.buoyancy_bias = 0.0;
        let mut sinker = Atom::new(8, 7, 50.0);
        sinker.buoyancy_bias = 1.0;
        store.atoms.push(floater);
        store.atoms.push(sinker);
        // Night phase → sinkers even deeper; use inactive phase.
        for t in 600..680 {
            store.step(&mut w, t);
        }
        assert!(
            store.atoms[1].gy < store.atoms[0].gy,
            "sinker gy={} should be below floater gy={}",
            store.atoms[1].gy,
            store.atoms[0].gy
        );
    }

    #[test]
    fn fission_can_jitter_buoyancy_bias() {
        let mut w = wet_column();
        let mut store = OrganismStore::new();
        let mut parent = Atom::new(4, 5, 40.0);
        parent.energy = 40.0;
        parent.cooldown = 0;
        parent.clone_fidelity = 0.2; // strong mutation
        parent.buoyancy_bias = 0.5;
        store.atoms.push(parent);
        for t in 0..120 {
            store.step(&mut w, t);
            if store.len() >= 2 {
                break;
            }
        }
        assert!(store.len() >= 2, "expected a child");
        let child_bias = store.atoms[1].buoyancy_bias;
        // With low fidelity, bias should usually move — allow equal
        // only if hash happened to be ~0 (rare); check genes copied path.
        assert!((0.0..=1.0).contains(&child_bias));
        assert!(
            (child_bias - 0.5).abs() > 1e-6 || store.atoms[1].clone_fidelity != 0.2,
            "fission should jitter buoyancy or fidelity"
        );
    }

    #[test]
    fn atoms_harvest_and_can_fission_in_lit_water() {
        let mut w = wet_column();
        let mut store = OrganismStore::new();
        store.atoms.push(Atom::new(4, 6, 20.0));
        store.atoms[0].energy = 20.0;
        store.atoms[0].cooldown = 0;
        for t in 0..80 {
            store.step(&mut w, t);
        }
        assert!(!store.is_empty(), "founder should survive in lit water");
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
            w.set_cell(4, y, Cell::air());
        }
        let mut store = OrganismStore::new();
        store.atoms.push(Atom::new(4, 6, 50.0));
        store.step(&mut w, 0);
        assert!(store.is_empty());
    }

    #[test]
    fn death_can_deposit_organic_above_bed() {
        let mut w = wet_column();
        w.set_cell(4, 1, Cell::air());
        let mut store = OrganismStore::new();
        let mut a = Atom::new(4, 6, 10.0);
        a.age_ticks = LIFE_TICKS;
        store.atoms.push(a);
        store.step(&mut w, 0);
        assert!(store.is_empty());
        assert_eq!(
            w.get_cell(4, 1).map(|c| c.material),
            Some(MaterialId::Organic),
            "corpse residue should sit on the bed, not replace water"
        );
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
            !store.is_empty(),
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

    #[test]
    fn contact_nudge_separates_stacked_atoms() {
        let mut w = wet_column();
        let mut store = OrganismStore::new();
        store.atoms.push(Atom::new(4, 5, 50.0));
        store.atoms.push(Atom::new(4, 5, 50.0)); // same cell
        store.step(&mut w, 0);
        let same = store.atoms[0].gx == store.atoms[1].gx
            && store.atoms[0].gy == store.atoms[1].gy;
        assert!(
            !same || (store.atoms[0].vel_y - store.atoms[1].vel_y).abs() > 0.01,
            "contact should shove apart in x or bounce in y"
        );
    }
}
