//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Set A plankton + minimal Set D land plants — module pixel blobs.
//!
//! Mirrors the column-GVSE organism kernel (see `docs/organism/` and
//! `wk-agents`), but lives entirely inside `wk-voxel` so the isolation
//! contract holds. Life is the drawing: 1×1 modules, not a green
//! biomass wash over the terrain.
//!
//! **Set A (Atom):** Nucleus + Photosystem in wet Air; buoyancy,
//! circadian day-float / night-sink, fission.
//! **Set D (minimal plant):** Root + Stem + Photosystem on land; fixed
//! crown, drinks pore `sat` ([`crate::plant`]), column Beer–Lambert shade.
//!
//! Palette hex is frozen (`docs/organism/PALETTE.md`).

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::blueprint::Genome;
use crate::climate::{day_factor_cfg, phase_fraction_cfg, ClimateConfig, DEMO_DAY_TICKS};
use crate::fungi::{
    colonize_and_compost, digest_budget_units, digest_labile, dissolve_corpse_to_organic,
    forage_organic_energy, fruiting_body_supported, fungus_should_hibernate, fungus_upkeep,
    is_fungus, is_fungus_seated, try_emergent_fruiting, try_spore, FRUIT_SUPPORT_MIN_AGE,
    FUNGUS_HIBERNATE_MAX_TICKS,
};
use crate::grid::World;
use crate::humidity::Humidity;
use crate::plant::{
    apply_genome, collect_live_photo_world_cells, collect_live_root_world_cells,
    collect_trunk_world_cells, drink_plant, drought_band, drop_dead_leaves, find_fungus_slot,
    find_plant_slot, find_surface_air_slot, is_anchored, is_land_plant,
    leave_dead_roots_in_place, leaves_bathing, pin_plant_pose, plant_moisture_frac,
    shed_unproductive_woody_leaves, sync_root_storage, try_grow_plant, try_plant_wind_spore,
    try_vegetative_sprout, DroughtBand, PlantGrowthCaps, DROUGHT_DORMANT_UPKEEP,
    DROUGHT_HIBERNATE_MAX_TICKS, DROUGHT_STRESS_DRAIN, PLANT_UPKEEP_MULT,
};
use crate::shade::{
    build_canopy_index_posed, canopy_top_y, posed_canopy_sample, shade_transmit,
    sum_posed_photo_light, CanopyIndex, PosedModule,
};

/// Default **entity** ceiling (Tab → Creatures can raise/lower).
/// One Atom / plant / fungus = 1, not body pixels (roots, leaves, …).
pub const MAX_ATOMS: usize = 256;
/// Default lingering-corpse ceiling (same order as living pop).
pub const MAX_CORPSES: usize = 256;

fn default_max_atoms() -> usize {
    MAX_ATOMS
}
fn default_max_corpses() -> usize {
    MAX_CORPSES
}

/// Why [`OrganismStore::spawn_blueprint_free`] refused a placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnFail {
    /// Living entity count already at [`OrganismStore::max_atoms`].
    PopCap,
    /// Blueprint empty or missing a Nucleus.
    InvalidBody,
    /// No Air cell near the click.
    NoAir,
}

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
/// Age soft-cap for plankton (ticks).
const LIFE_TICKS: u64 = DEMO_DAY_TICKS * 4;
/// Land plants / fungi live longer — senescence is softer than plankton blooms.
const PLANT_LIFE_TICKS: u64 = DEMO_DAY_TICKS * 16;

/// Land / fungus corpses rest this long before becoming Organic (~0.75 demo day).
pub const CORPSE_SETTLE_LAND_TICKS: u32 = 900;
/// Plankton corpses linger longer so bloom deaths leave a visible carpet.
pub const CORPSE_SETTLE_WATER_TICKS: u32 = 2_400;

/// Floater equilibrium depth below the free surface (cells).
const FLOAT_DEPTH: f32 = 1.5;
/// Column buoyancy constants (cell units / tick).
const GRAVITY: f32 = 0.08;
const WATER_DRAG: f32 = 0.25;
const AIR_DRAG: f32 = 0.05;
const EQ_SPRING: f32 = 0.12;
/// Soft contact impulse when two Atoms share a cell.
const CONTACT_BOUNCE: f32 = 0.12;

/// Module IDs — values match `wk_agents::ModuleId` / PALETTE.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ModuleId {
    Nucleus = 0x00,
    Photosystem = 0x01,
    /// Brown-red — convert litter / Organic → energy (Set E).
    Digest = 0x0A,
    /// Cream — hypha thread extending digest reach (Set E).
    Hypha = 0x0B,
    /// Sienna — land plant anchor / moisture drink (Set D).
    Root = 0x0D,
    /// Olive — upright stack holding leaves (Set D).
    Stem = 0x0E,
    /// Lilac — wind-borne spore / seed packet (ferns, fruiting bodies).
    ReproSpore = 0x10,
}

/// Bright Photosystem green (full light) — `docs/organism/PALETTE.md`.
pub const PHOTO_RGB_ACTIVE: (u8, u8, u8) = (0x2E, 0xCC, 0x40);
/// Dim olive when a leaf is deeply shaded / light-starved (diagnostic).
/// Kept well away from active green so land canopies read at a glance.
pub const PHOTO_RGB_SHADED: (u8, u8, u8) = (0x3A, 0x4E, 0x22);

impl ModuleId {
    /// Frozen RGB from `docs/organism/PALETTE.md`.
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            ModuleId::Nucleus => (0x00, 0x00, 0x00),
            ModuleId::Photosystem => PHOTO_RGB_ACTIVE,
            ModuleId::Digest => (0x8B, 0x2E, 0x2E),
            ModuleId::Hypha => (0xF1, 0xE6, 0xC4),
            ModuleId::Root => (0x7A, 0x4B, 0x2A),
            ModuleId::Stem => (0x55, 0x6B, 0x2F),
            ModuleId::ReproSpore => (0xD0, 0xB0, 0xFF),
        }
    }

    /// Photosystem tint from light exposure `0..1` (active → shaded).
    ///
    /// Pass **raw** sky × canopy transmit — not harvest-remapped light —
    /// so `ShadeEfficiency` understory genes don't wash out the diagnostic.
    pub fn photosystem_rgb_for_light(light: f32) -> (u8, u8, u8) {
        let t = light.clamp(0.0, 1.0);
        // Slightly compress highlights so mid-shade still reads darker.
        let t = (t.powf(1.35)).clamp(0.0, 1.0);
        let (r0, g0, b0) = PHOTO_RGB_SHADED;
        let (r1, g1, b1) = PHOTO_RGB_ACTIVE;
        (
            (r0 as f32 + (r1 as f32 - r0 as f32) * t).round() as u8,
            (g0 as f32 + (g1 as f32 - g0 as f32) * t).round() as u8,
            (b0 as f32 + (b1 as f32 - b0 as f32) * t).round() as u8,
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            ModuleId::Nucleus => "Nucleus",
            ModuleId::Photosystem => "Photosystem",
            ModuleId::Digest => "Digest",
            ModuleId::Hypha => "Hypha",
            ModuleId::Root => "Root",
            ModuleId::Stem => "Stem",
            ModuleId::ReproSpore => "ReproSpore",
        }
    }
}

/// Body module as offset from the nucleus anchor.
pub type BodyModule = (i16, i16, ModuleId);

fn default_atom_body() -> Vec<BodyModule> {
    vec![(0, 0, ModuleId::Nucleus), (1, 0, ModuleId::Photosystem)]
}

/// One living organism in world cell space (Atom or land plant).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Atom {
    /// Anchor cell (nucleus position).
    pub gx: i32,
    pub gy: i32,
    /// Continuous vertical pose (synced into `gy` each tick).
    pub fy: f32,
    pub vel_y: f32,
    pub energy: f32,
    /// Current tank clamp (plants: may inflate from root starch storage).
    pub energy_max: f32,
    /// Spawn-tank size — photo / upkeep / growth floors key off this.
    /// Plants sync `energy_max` above this via root count (D4).
    #[serde(default)]
    pub energy_base_max: f32,
    pub age_ticks: u64,
    pub cooldown: u64,
    /// Consecutive ticks in drought dormancy (land plants). Resets when moist.
    #[serde(default)]
    pub drought_ticks: u32,
    /// Fractional water-sip accumulator (roots + bathing leaves).
    #[serde(default)]
    pub sip_acc: f32,
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
    /// Live genes (allocation, depth bias, shade knobs, …).
    #[serde(default)]
    pub genome: Genome,
    /// Woody Photosystem starve counters: `(local_x, local_y, ticks)`.
    /// Stemless seaweed ignores this. Cleared when a leaf is productive again.
    #[serde(default)]
    pub leaf_starve: Vec<(i16, i16, u16)>,
    /// Lost purchase / tippy raft — original body drawn tipped.
    #[serde(default)]
    pub fallen: bool,
    /// Body-local cells grown after tipping; drawn upright (new shoots).
    #[serde(default)]
    pub upright_growth: Vec<(i16, i16)>,
}

impl Atom {
    pub fn new(gx: i32, gy: i32, energy_max: f32) -> Self {
        let genome = Genome::default();
        let energy_max = energy_max.max(1.0);
        Self {
            gx,
            gy,
            fy: gy as f32,
            vel_y: 0.0,
            energy: energy_max * 0.6,
            energy_max,
            energy_base_max: energy_max,
            age_ticks: 0,
            cooldown: REPRO_PERIOD / 2,
            drought_ticks: 0,
            sip_acc: 0.0,
            buoyancy_bias: genome.buoyancy_bias,
            clone_fidelity: genome.clone_fidelity,
            circadian_phase: 0.25,
            active_window: 0.55,
            last_water_top: None,
            body: default_atom_body(),
            genome,
            leaf_starve: Vec::new(),
            fallen: false,
            upright_growth: Vec::new(),
        }
    }

    pub fn from_body(gx: i32, gy: i32, energy_max: f32, body: Vec<BodyModule>) -> Self {
        let mut a = Self::new(gx, gy, energy_max);
        if !body.is_empty() {
            a.body = body;
        }
        a
    }

    /// Record a shoot cell grown while tipped — draws upright, not tipped.
    ///
    /// Stemless tip is draw-only (body stays vertical); marking upright would
    /// detach new frond cells from the soft ribbon. Woody only.
    pub fn mark_upright_growth(&mut self, dx: i16, dy: i16) {
        if !self.fallen {
            return;
        }
        if !self.body.iter().any(|(_, _, m)| *m == ModuleId::Stem) {
            return;
        }
        if !self.upright_growth.iter().any(|&p| p == (dx, dy)) {
            self.upright_growth.push((dx, dy));
        }
    }

    /// Drop upright-growth entries whose body cells were removed (shed / prune).
    pub fn sync_upright_growth(&mut self) {
        self.upright_growth
            .retain(|&(x, y)| self.body.iter().any(|&(bx, by, _)| bx == x && by == y));
    }

    /// True when this body cell should skip the tip transform when drawn.
    pub fn draws_upright(&self, dx: i16, dy: i16) -> bool {
        !self.fallen || self.upright_growth.iter().any(|&p| p == (dx, dy))
    }

    /// Body → draw offset while tipped (pre-tip canopy flat; new shoots upright).
    pub fn fallen_draw_offset(&self, dx: i16, dy: i16) -> (i16, i16) {
        fallen_draw_offset(self.fallen, &self.upright_growth, &self.body, dx, dy)
    }

    pub fn photosystem_count(&self) -> usize {
        self.body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .count()
    }

    /// True when a posed (tipped / upright-mast) cell sits at `(wx, wy)`.
    /// Roots also match their body cells so substrate purchase stays pickable.
    pub fn occupies(&self, wx: i32, wy: i32) -> bool {
        self.body.iter().any(|(dx0, dy0, m)| {
            let (dx, dy) = self.fallen_draw_offset(*dx0, *dy0);
            if self.gx + dx as i32 == wx && self.gy + dy as i32 == wy {
                return true;
            }
            *m == ModuleId::Root
                && self.gx + *dx0 as i32 == wx
                && self.gy + *dy0 as i32 == wy
        })
    }

    fn body_top_offset(&self) -> f32 {
        self.body
            .iter()
            .map(|(_, dy, _)| *dy as f32)
            .fold(0.0f32, f32::max)
            .max(0.0)
    }
}

/// Dead body: keeps drawing (grey), sinks / rests, then becomes Organic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Corpse {
    pub gx: i32,
    pub gy: i32,
    pub fy: f32,
    pub vel_y: f32,
    pub body: Vec<BodyModule>,
    pub ticks: u32,
    pub settled_ticks: u32,
    /// Plant or fungus — pinned on land; plankton sinks in water.
    pub land: bool,
    pub last_water_top: Option<i32>,
    /// Land plant/fungus corpse floating tipped on open water.
    #[serde(default)]
    pub fallen: bool,
    /// Pre-death upright shoots — still drawn upright on a tipped corpse.
    #[serde(default)]
    pub upright_growth: Vec<(i16, i16)>,
}

impl Corpse {
    pub fn from_atom(atom: &Atom) -> Self {
        let land = is_land_plant(atom) || is_fungus(atom);
        // Roots → Organic in soil; leaves → falling Organic litter.
        // Lingering grey corpse keeps stem / nucleus / crown only.
        let body: Vec<BodyModule> = if is_land_plant(atom) {
            atom.body
                .iter()
                .copied()
                .filter(|(_, _, m)| {
                    *m != ModuleId::Root && *m != ModuleId::Photosystem
                })
                .collect()
        } else {
            atom.body.clone()
        };
        // Ghost leaf ranks from shed Photosystems must not gape the grey stem.
        let upright_growth: Vec<(i16, i16)> = atom
            .upright_growth
            .iter()
            .copied()
            .filter(|&(x, y)| body.iter().any(|&(bx, by, _)| bx == x && by == y))
            .collect();
        Self {
            gx: atom.gx,
            gy: atom.gy,
            fy: atom.fy,
            vel_y: if land { 0.0 } else { -0.15 },
            body,
            ticks: 0,
            settled_ticks: 0,
            land,
            last_water_top: atom.last_water_top,
            fallen: atom.fallen,
            upright_growth,
        }
    }

    fn fallen_draw_offset(&self, dx: i16, dy: i16) -> (i16, i16) {
        fallen_draw_offset(self.fallen, &self.upright_growth, &self.body, dx, dy)
    }

    pub fn occupies(&self, wx: i32, wy: i32) -> bool {
        self.body.iter().any(|(dx0, dy0, _)| {
            let (dx, dy) = self.fallen_draw_offset(*dx0, *dy0);
            self.gx + dx as i32 == wx && self.gy + dy as i32 == wy
        })
    }
}

/// Desaturated brown-grey — readable as dead tissue (column `corpse_rgb`).
pub fn corpse_rgb((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    let luma = (r as u16 * 3 + g as u16 * 6 + b as u16) / 10;
    (
        ((luma + 40) / 2).min(120) as u8,
        ((luma + 20) / 2).min(90) as u8,
        (luma / 3).min(70) as u8,
    )
}

/// Population of Set A Atoms (no `hecs` — keep the crate tiny).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganismStore {
    pub atoms: Vec<Atom>,
    #[serde(default)]
    pub corpses: Vec<Corpse>,
    /// Hard ceiling on living creatures (spawn / fission / sprout).
    #[serde(default = "default_max_atoms")]
    pub max_atoms: usize,
    /// Hard ceiling on lingering corpses before oldest are dropped.
    #[serde(default = "default_max_corpses")]
    pub max_corpses: usize,
    /// Per-plant Root / Stem / Photosystem pixel ceilings.
    #[serde(default)]
    pub growth_caps: PlantGrowthCaps,
}

impl Default for OrganismStore {
    fn default() -> Self {
        Self {
            atoms: Vec::new(),
            corpses: Vec::new(),
            max_atoms: MAX_ATOMS,
            max_corpses: MAX_CORPSES,
            growth_caps: PlantGrowthCaps::default(),
        }
    }
}

impl OrganismStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.atoms.len()
    }

    /// Living habit breakdown: `(plants, fungi, atoms)`.
    /// HUD uses this so a fungus spore bloom isn't mistaken for "invisible plants".
    pub fn habit_counts(&self) -> (usize, usize, usize) {
        let mut plants = 0usize;
        let mut fungi = 0usize;
        let mut atoms = 0usize;
        for a in &self.atoms {
            if is_land_plant(a) {
                plants += 1;
            } else if is_fungus(a) {
                fungi += 1;
            } else {
                atoms += 1;
            }
        }
        (plants, fungi, atoms)
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    pub fn corpse_count(&self) -> usize {
        self.corpses.len()
    }

    /// Living pop ceiling (at least 1 so a lone spawn can land).
    pub fn atom_cap(&self) -> usize {
        self.max_atoms.max(1)
    }

    /// Corpse list ceiling (at least 1).
    pub fn corpse_cap(&self) -> usize {
        self.max_corpses.max(1)
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
                if h % 3 == 0 && self.atoms.len() < self.atom_cap() {
                    self.atoms.push(Atom::new(gx_w, gy, energy_max));
                }
            }
            gx += stride;
        }
    }

    /// Draw list: world cell + frozen module RGB (living + grey corpses).
    ///
    /// Soft fronds (Photosystem / Stem) sway under water and lay along the
    /// free surface when taller than the water column. Flopped greens **pile**
    /// when they would share a cell. Photosystems tint bright→dim green by
    /// effective light at their posed cell (shade / depth diagnostic).
    /// Pass `wind_vx` for lean direction; `tick` phases the underwater wave.
    pub fn draw_list(
        &self,
        world: &World,
        tick: u64,
        wind_vx: f32,
    ) -> Vec<(i32, i32, (u8, u8, u8))> {
        let posed = resolve_organism_draw_cells(world, &self.atoms, tick, wind_vx);
        let canopy = build_canopy_index_posed(&self.atoms, &posed);
        // Cache transmit per posed cell — many leaves share a flop pile cell.
        let mut transmit_cache: std::collections::HashMap<(i32, i32), f32> =
            std::collections::HashMap::with_capacity(posed.len());
        let mut out = Vec::with_capacity(posed.len() + self.corpses.len() * 2);
        for p in &posed {
            let rgb = if p.mid == ModuleId::Photosystem {
                // Raw exposure (sky × column Beer–Lambert), land and water —
                // not the harvest remap, which understory genes wash toward green.
                let sky = column_light(world, p.wx, p.wy);
                let transmit = *transmit_cache
                    .entry((p.wx, p.wy))
                    .or_insert_with(|| shade_transmit(&canopy, p.wx, p.wy));
                let exposure = (sky * transmit).clamp(0.0, 1.0);
                ModuleId::photosystem_rgb_for_light(exposure)
            } else {
                p.mid.rgb()
            };
            out.push((p.wx, p.wy, rgb));
        }
        for corpse in &self.corpses {
            for &(dx0, dy0, mid) in &corpse.body {
                let (dx, dy) = corpse.fallen_draw_offset(dx0, dy0);
                let rgb = if mid == ModuleId::Photosystem {
                    // Corpses keep the shaded leaf tone (no harvest).
                    PHOTO_RGB_SHADED
                } else {
                    mid.rgb()
                };
                out.push((
                    corpse.gx + dx as i32,
                    corpse.gy + dy as i32,
                    corpse_rgb(rgb),
                ));
            }
        }
        out
    }

    /// Spawn a painted blueprint with nucleus at `(gx, gy)`.
    /// Atoms need wet Air; land plants need Air above porous solid.
    ///
    /// Habitat-aware seating for sim / tests. Editor free placement uses
    /// [`Self::spawn_blueprint_free`].
    pub fn spawn_blueprint(
        &mut self,
        world: &World,
        gx: i32,
        gy: i32,
        body: Vec<BodyModule>,
        energy_max: f32,
        genome: Genome,
    ) -> bool {
        if self.atoms.len() >= self.atom_cap() || body.is_empty() {
            return false;
        }
        let gx = world.wrap_x(gx);
        let plant = body.iter().any(|(_, _, m)| *m == ModuleId::Root);
        let fungus = body.iter().any(|(_, _, m)| *m == ModuleId::Digest) && !plant;
        let gy = if plant {
            let Some(slot) = find_plant_slot(world, gx, gy) else {
                return false;
            };
            slot
        } else if fungus {
            let Some(slot) = find_fungus_slot(world, gx, gy) else {
                return false;
            };
            slot
        } else if is_wet_air(world, gx, gy) {
            gy
        } else if let Some(slot) = find_wet_near(world, gx, gy) {
            slot
        } else {
            return false;
        };
        let mut atom = Atom::from_body(gx, gy, energy_max, body);
        apply_genome(&mut atom, genome);
        if plant {
            pin_plant_pose(&mut atom);
            if !is_anchored(world, &atom) {
                return false;
            }
        } else if fungus {
            pin_plant_pose(&mut atom);
            if !is_fungus_seated(world, &atom) {
                return false;
            }
        } else if let Some((top, _)) = wet_band(world, gx, gy) {
            atom.last_water_top = Some(top);
        }
        self.atoms.push(atom);
        true
    }

    /// Editor spawn: any module mix near the click.
    ///
    /// Land plants / fungi snap to an Air-above-solid crown under the
    /// column (so canopy clicks don't hang a Root in mid-air). Plankton
    /// still only need Air. Thriving remains the creature's problem.
    ///
    /// Counts **entities** (one plant = one slot), not body pixels.
    pub fn spawn_blueprint_free(
        &mut self,
        world: &World,
        gx: i32,
        gy: i32,
        body: Vec<BodyModule>,
        energy_max: f32,
        genome: Genome,
    ) -> Result<(), SpawnFail> {
        if self.atoms.len() >= self.atom_cap() {
            return Err(SpawnFail::PopCap);
        }
        if body.is_empty() || !body.iter().any(|(_, _, m)| *m == ModuleId::Nucleus) {
            return Err(SpawnFail::InvalidBody);
        }
        let gx = world.wrap_x(gx);
        let plant = body.iter().any(|(_, _, m)| *m == ModuleId::Root);
        let fungus = body.iter().any(|(_, _, m)| *m == ModuleId::Digest) && !plant;
        let gy = if plant {
            find_surface_air_slot(world, gx, gy)
                .or_else(|| find_air_near(world, gx, gy))
                .ok_or(SpawnFail::NoAir)?
        } else if fungus {
            find_fungus_slot(world, gx, gy)
                .or_else(|| find_surface_air_slot(world, gx, gy))
                .or_else(|| find_air_near(world, gx, gy))
                .ok_or(SpawnFail::NoAir)?
        } else {
            find_air_near(world, gx, gy).ok_or(SpawnFail::NoAir)?
        };
        let mut atom = Atom::from_body(gx, gy, energy_max, body);
        apply_genome(&mut atom, genome);
        if is_land_plant(&atom) || is_fungus(&atom) {
            pin_plant_pose(&mut atom);
        } else if let Some((top, _)) = wet_band(world, gx, gy) {
            atom.last_water_top = Some(top);
        }
        self.atoms.push(atom);
        Ok(())
    }

    /// First living organism occupying world cell `(gx, gy)`.
    pub fn pick_at(&self, gx: i32, gy: i32) -> Option<usize> {
        self.atoms.iter().position(|a| a.occupies(gx, gy))
    }

    /// First corpse occupying world cell `(gx, gy)`.
    pub fn pick_corpse_at(&self, gx: i32, gy: i32) -> Option<usize> {
        self.corpses.iter().position(|c| c.occupies(gx, gy))
    }

    /// One organism step: plankton buoyancy / plant drink, light,
    /// upkeep, fission (Atoms only), death → corpse, corpse settle → Organic.
    pub fn step(&mut self, world: &mut World, tick: u64) {
        let _ = self.step_with_climate(world, tick, &ClimateConfig::default(), None);
    }

    /// Like [`Self::step_with_climate`] but without humidity bookkeeping.
    pub fn step_with_climate(
        &mut self,
        world: &mut World,
        tick: u64,
        climate: &ClimateConfig,
        humidity: Option<&mut Humidity>,
    ) {
        let _ = self.step_with_climate_wind(world, tick, climate, humidity, 0.0);
    }

    /// Like [`Self::step_with_climate`] with horizontal wind for spores
    /// (fungi fruiting bodies + plants with [`ModuleId::ReproSpore`]).
    /// Returns spore-release events for the renderer (not saved).
    pub fn step_with_climate_wind(
        &mut self,
        world: &mut World,
        tick: u64,
        climate: &ClimateConfig,
        humidity: Option<&mut Humidity>,
        wind_vx: f32,
    ) -> Vec<SporeRelease> {
        let day = day_factor_cfg(tick, climate);
        let phase = phase_fraction_cfg(tick, climate);
        // Posed draw cells (flop + pile) feed canopy shade so dry mats and
        // equal-height meadows compete for light where they actually sit.
        let posed = resolve_organism_draw_cells(world, &self.atoms, tick, wind_vx);
        let canopy = build_canopy_index_posed(&self.atoms, &posed);
        // Live + grey-corpse Stem cells — shoot growth keeps a trunk gap.
        let trunks = collect_trunk_world_cells(&self.atoms, &self.corpses);
        // All living Root / Photosystem cells — Moore spacing across plants.
        let live_roots = collect_live_root_world_cells(&self.atoms);
        let live_photos = collect_live_photo_world_cells(&self.atoms);
        let mut births: Vec<Atom> = Vec::new();
        let mut deaths: Vec<usize> = Vec::new();
        let mut spore_releases: Vec<SporeRelease> = Vec::new();
        let pop = self.atoms.len();
        let atom_cap = self.atom_cap();
        let growth_caps = self.growth_caps.clamp();
        // One crown per column: destack any pre-existing overlaps first.
        reseat_stacked_land_plants(world, &mut self.atoms);
        reseat_stacked_fungi(world, &mut self.atoms);
        // Crown columns of living land plants — density + occupancy gate.
        // Mutated as sprouts birth so same-tick siblings can't share a seat.
        let mut plant_cols: Vec<i32> = self
            .atoms
            .iter()
            .filter(|a| is_land_plant(a))
            .map(|a| a.gx)
            .collect();
        let mut fungus_cols: Vec<i32> = self
            .atoms
            .iter()
            .filter(|a| is_fungus(a))
            .map(|a| a.gx)
            .collect();
        // Transpiration return: (gx, gy, sat_units) → humidity mass.
        let mut transpired: Vec<(i32, i32, u32)> = Vec::new();
        // Floating-Organic raft columns: once per organism tick. Seat / tip /
        // holdfast helpers used to re-scan the whole world per plant (and
        // several times each) — that was O(plants × cells) and crushed FPS.
        let float_columns = if plant_cols.is_empty() {
            std::collections::HashMap::new()
        } else {
            crate::rules::collect_floating_organic_columns(world)
        };

        // Empty store: still allow mycelium field → fruiting body emergence
        // (and corpse settle). Spores need a living body afterward.
        if self.atoms.is_empty() {
            if self.corpses.is_empty() {
                let room = births.len() < atom_cap;
                if let Some(child) =
                    try_emergent_fruiting(world, &fungus_cols, tick, room)
                {
                    self.atoms.push(child);
                }
                return spore_releases;
            }
            self.step_corpses(world);
            let room = self.atoms.len() < atom_cap;
            if let Some(child) = try_emergent_fruiting(world, &[], tick, room) {
                self.atoms.push(child);
            }
            return spore_releases;
        }

        for (i, atom) in self.atoms.iter_mut().enumerate() {
            atom.age_ticks = atom.age_ticks.saturating_add(1);
            atom.cooldown = atom.cooldown.saturating_sub(1);

            let life_cap = if is_land_plant(atom) || is_fungus(atom) {
                PLANT_LIFE_TICKS
            } else {
                LIFE_TICKS
            };
            if atom.age_ticks >= life_cap {
                deaths.push(i);
                continue;
            }

            if is_land_plant(atom) {
                let room = pop + births.len() < atom_cap;
                let parent_gx = atom.gx;
                let parent_gy = atom.gy;
                match step_land_plant(
                    world,
                    atom,
                    day,
                    tick,
                    &canopy,
                    &posed,
                    i,
                    &trunks,
                    &live_roots,
                    &live_photos,
                    i as u32,
                    room,
                    &growth_caps,
                    &plant_cols,
                    wind_vx,
                    &float_columns,
                ) {
                    PlantStep::Dead => deaths.push(i),
                    PlantStep::Alive { sat, at } => {
                        if sat > 0 {
                            transpired.push((at.0, at.1, sat));
                        }
                    }
                    PlantStep::Sprout(child) => {
                        plant_cols.push(child.gx);
                        births.push(child);
                    }
                    PlantStep::Spore(child) => {
                        spore_releases.push(SporeRelease {
                            from_gx: parent_gx,
                            from_gy: parent_gy,
                            to_gx: child.gx,
                            to_gy: child.gy,
                        });
                        plant_cols.push(child.gx);
                        births.push(child);
                    }
                }
                continue;
            }

            if is_fungus(atom) {
                let room = pop + births.len() < atom_cap;
                let parent_gx = atom.gx;
                let parent_gy = atom.gy;
                match step_fungus(
                    world,
                    atom,
                    day,
                    tick,
                    i as u32,
                    room,
                    wind_vx,
                    &fungus_cols,
                ) {
                    PlantStep::Dead => deaths.push(i),
                    PlantStep::Alive { .. } => {}
                    PlantStep::Sprout(child) => {
                        fungus_cols.push(child.gx);
                        births.push(child);
                    }
                    PlantStep::Spore(child) => {
                        spore_releases.push(SporeRelease {
                            from_gx: parent_gx,
                            from_gy: parent_gy,
                            to_gx: child.gx,
                            to_gy: child.gy,
                        });
                        fungus_cols.push(child.gx);
                        births.push(child);
                    }
                }
                continue;
            }

            // Plankton drought gate: must still have a wet band nearby.
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
                && pop + births.len() < atom_cap
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
                // Land plants: roots stay as Organic in soil; leaves drop
                // as falling Organic; stems linger grey until dissolve.
                if is_land_plant(&dead) {
                    let _ = leave_dead_roots_in_place(world, &dead);
                    let _ = drop_dead_leaves(world, &dead);
                }
                self.push_corpse(world, Corpse::from_atom(&dead));
            }
            if i < self.atoms.len() {
                self.atoms.swap_remove(i);
            }
        }
        self.atoms.extend(births);
        // Cream network → new fruiting body (may later shed spores).
        let mut fungus_cols_now: Vec<i32> = self
            .atoms
            .iter()
            .filter(|a| is_fungus(a))
            .map(|a| a.gx)
            .collect();
        if self.atoms.len() < atom_cap {
            if let Some(child) =
                try_emergent_fruiting(world, &fungus_cols_now, tick, true)
            {
                fungus_cols_now.push(child.gx);
                self.atoms.push(child);
            }
        }
        let _ = fungus_cols_now;
        resolve_contacts(world, &mut self.atoms);
        self.step_corpses(world);

        // Return drunk pore sat to atmospheric humidity (mass conservation).
        if let Some(hum) = humidity {
            for (gx, gy, sat) in transpired {
                if sat > 0 {
                    hum.add(gx, gy, sat as f32);
                }
            }
        }
        spore_releases
    }

    fn push_corpse(&mut self, world: &mut World, corpse: Corpse) {
        if self.corpses.len() >= self.corpse_cap() {
            // Cap pressure: dissolve oldest immediately into Organic.
            if let Some(old) = self.corpses.first().cloned() {
                dissolve_corpse_to_organic(world, old.gx, old.gy, &old.body);
            }
            self.corpses.remove(0);
        }
        self.corpses.push(corpse);
    }

    /// Sink / pin corpses; after settle, paint Organic + soft litter.
    fn step_corpses(&mut self, world: &mut World) {
        let mut dissolve: Vec<usize> = Vec::new();
        for (i, corpse) in self.corpses.iter_mut().enumerate() {
            corpse.ticks = corpse.ticks.saturating_add(1);
            if corpse.land {
                if let Some((top, _bed)) = wet_band(world, corpse.gx, corpse.gy) {
                    // Dead land plant on open water — float tipped at the
                    // free surface until compost (don't pin to the bed).
                    corpse.fallen = true;
                    corpse.gy = top;
                    corpse.fy = top as f32;
                    corpse.vel_y = 0.0;
                    corpse.last_water_top = Some(top);
                } else {
                    // Reseat on beach/bed — don't leave a grey stem at the
                    // vanished waterline. Woody bake already flattened the
                    // body; keep fallen so upright mast ranks still draw.
                    if let Some(slot) = find_surface_air_slot(world, corpse.gx, corpse.gy) {
                        corpse.gy = slot;
                    }
                    pin_corpse_land(corpse);
                }
                corpse.settled_ticks = corpse.settled_ticks.saturating_add(1);
                if corpse.settled_ticks >= CORPSE_SETTLE_LAND_TICKS {
                    dissolve.push(i);
                }
                continue;
            }

            // Plankton corpse: heavy sink toward the wet-band bed.
            corpse.fallen = false;
            step_corpse_buoyancy(world, corpse);
            let on_bed = match wet_band(world, corpse.gx, corpse.gy) {
                Some((_top, bed)) => corpse.gy <= bed + 1,
                None => true, // stranded — count as settled
            };
            if on_bed {
                corpse.vel_y = 0.0;
                corpse.settled_ticks = corpse.settled_ticks.saturating_add(1);
                if corpse.settled_ticks >= CORPSE_SETTLE_WATER_TICKS {
                    dissolve.push(i);
                }
            } else {
                corpse.settled_ticks = 0;
            }
        }

        dissolve.sort_unstable();
        dissolve.dedup();
        for &i in dissolve.iter().rev() {
            if let Some(c) = self.corpses.get(i).cloned() {
                dissolve_corpse_to_organic(world, c.gx, c.gy, &c.body);
            }
            if i < self.corpses.len() {
                self.corpses.swap_remove(i);
            }
        }
    }
}

fn pin_corpse_land(corpse: &mut Corpse) {
    corpse.fy = corpse.gy as f32;
    corpse.vel_y = 0.0;
    corpse.last_water_top = None;
}

fn step_corpse_buoyancy(world: &World, corpse: &mut Corpse) {
    let Some((top, bed)) = wet_band(world, corpse.gx, corpse.gy) else {
        // Dry out — rest where we are.
        corpse.vel_y = 0.0;
        return;
    };
    // Heavy: sink with extra pull (column corpse path).
    corpse.vel_y -= GRAVITY + 0.04;
    corpse.vel_y *= 1.0 - WATER_DRAG;
    corpse.fy += corpse.vel_y;
    if corpse.fy < bed as f32 {
        corpse.fy = bed as f32;
        corpse.vel_y = 0.0;
    }
    if corpse.fy > top as f32 {
        corpse.fy = top as f32;
    }
    corpse.gy = corpse.fy.round() as i32;
    corpse.last_water_top = Some(top);
}

enum PlantStep {
    Dead,
    Alive { sat: u32, at: (i32, i32) },
    /// Vegetative rhizome child (local).
    Sprout(Atom),
    /// Wind-borne spore child (fern / fruiting body) — emit VFX.
    Spore(Atom),
}

/// One wind-spore launch for the renderer (ephemeral; not saved).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SporeRelease {
    pub from_gx: i32,
    pub from_gy: i32,
    pub to_gx: i32,
    pub to_gy: i32,
}

/// Oldest land plant keeps its column; younger stack-mates reseat nearby.
/// Fixes ghost-looking overlays from rhizome sprouts sharing one crown cell.
fn reseat_stacked_land_plants(world: &World, atoms: &mut [Atom]) {
    let mut land_idx: Vec<usize> = atoms
        .iter()
        .enumerate()
        .filter(|(_, a)| is_land_plant(a) && !a.fallen)
        .map(|(i, _)| i)
        .collect();
    if land_idx.len() < 2 {
        return;
    }
    // Oldest first — they claim the column.
    land_idx.sort_by_key(|&i| std::cmp::Reverse(atoms[i].age_ticks));
    let mut claimed = std::collections::HashSet::new();
    for i in land_idx {
        let gx = atoms[i].gx;
        if claimed.insert(gx) {
            continue;
        }
        let gy = atoms[i].gy;
        let mut moved = false;
        for dist in 1..=16 {
            for sign in [1i32, -1] {
                let nx = world.wrap_x(gx + sign * dist);
                if claimed.contains(&nx) {
                    continue;
                }
                let Some(ny) = find_plant_slot(world, nx, gy) else {
                    continue;
                };
                atoms[i].gx = nx;
                atoms[i].gy = ny;
                pin_plant_pose(&mut atoms[i]);
                claimed.insert(nx);
                moved = true;
                break;
            }
            if moved {
                break;
            }
        }
    }
}

/// Oldest fruiting body keeps its column; younger stack-mates reseat.
/// Spore floods used to pile hundreds of `N1D1H2Sp1` on one Organic seat.
fn reseat_stacked_fungi(world: &World, atoms: &mut [Atom]) {
    let mut fungus_idx: Vec<usize> = atoms
        .iter()
        .enumerate()
        .filter(|(_, a)| is_fungus(a))
        .map(|(i, _)| i)
        .collect();
    if fungus_idx.len() < 2 {
        return;
    }
    fungus_idx.sort_by_key(|&i| std::cmp::Reverse(atoms[i].age_ticks));
    let mut claimed = std::collections::HashSet::new();
    for i in fungus_idx {
        let gx = atoms[i].gx;
        if claimed.insert(gx) {
            continue;
        }
        let gy = atoms[i].gy;
        let mut moved = false;
        for dist in 1..=24 {
            for sign in [1i32, -1] {
                let nx = world.wrap_x(gx + sign * dist);
                if claimed.contains(&nx) {
                    continue;
                }
                let Some(ny) = find_fungus_slot(world, nx, gy)
                    .or_else(|| find_surface_air_slot(world, nx, gy))
                else {
                    continue;
                };
                atoms[i].gx = nx;
                atoms[i].gy = ny;
                pin_plant_pose(&mut atoms[i]);
                claimed.insert(nx);
                moved = true;
                break;
            }
            if moved {
                break;
            }
        }
    }
}

/// Land plant tick: drink, shade photo, grow, maybe rhizome sprout or
/// wind spore (needs painted [`ModuleId::ReproSpore`], fern-style).
/// D4: root starch tank + drought bands (stress / hibernate).
fn step_land_plant(
    world: &mut World,
    atom: &mut Atom,
    day: f32,
    tick: u64,
    canopy: &CanopyIndex,
    posed: &[PosedModule],
    atom_idx: usize,
    trunks: &std::collections::HashSet<(i32, i32)>,
    live_roots: &std::collections::HashSet<(i32, i32)>,
    live_photos: &std::collections::HashSet<(i32, i32)>,
    entity_id: u32,
    pop_room: bool,
    growth_caps: &PlantGrowthCaps,
    plant_cols: &[i32],
    wind_vx: f32,
    float_columns: &std::collections::HashMap<i32, (i32, i32)>,
) -> PlantStep {
    // Pose / seat:
    // - Sand/rock purchase wins: once grounded, organics and water never
    //   hoist the crown (shore mats / rising lake only settle downward).
    // - Crown-column substrate holdfast: reseat when not grounded.
    // - Floating-Organic raft holdfast (only): tip check; ride the surface.
    // - No holdfast over full-sat water: tip and free-float.
    // - Woody tip always bakes into body (plan survives as horizontal trunk);
    //   stemless tips for draw only.
    // - Sand undercut with no water yet: woody tip-bakes, then reseats.
    // - Shore: stay tipped; grow roots into the beach; new shoots upright.
    let on_float_raft = rooted_in_floating_organic(world, atom, float_columns);
    let holdfast_solid = crown_holdfast_solid_y(world, atom, float_columns);
    let stemless = crate::plant::stem_count(atom) == 0;
    let grounded = grounded_substrate_anchor(world, atom, float_columns);
    if grounded {
        // Fixed in sand/rock/grounded compost. Stemless clears a stale tip
        // when a short crown grip is still present; woody stays tipped after
        // shore re-root. Never raise gy onto organic piles or the waterline.
        if stemless && holdfast_solid.is_some() {
            atom.fallen = false;
            atom.upright_growth.clear();
        }
        if let Some(solid_y) = holdfast_solid {
            let seat = solid_y + 1;
            if seat < atom.gy {
                atom.gy = seat;
            }
        }
        pin_plant_pose(atom);
    } else if let Some(solid_y) = holdfast_solid {
        // Local ground under the crown (no sand purchase yet). Stemless
        // seaweed drops a stale tip once the holdfast is back; woody plants
        // stay tipped so only upright_growth shoots stand up after re-root.
        if stemless {
            atom.fallen = false;
            atom.upright_growth.clear();
        }
        atom.gy = solid_y + 1;
        pin_plant_pose(atom);
    } else if on_float_raft {
        let water_top = column_standing_surface(world, atom.gx, atom.gy);
        apply_raft_tip(world, atom, float_columns);
        if atom.fallen {
            if let Some(top) = water_top {
                atom.gy = top;
                atom.last_water_top = Some(top);
            }
            atom.fy = atom.gy as f32;
            atom.vel_y = 0.0;
        } else {
            pin_plant_pose(atom);
        }
    } else if let Some(top) = column_standing_surface(world, atom.gx, atom.gy) {
        if stemless {
            // Detached seaweed: ride the surface; keep ribbon body offsets.
            atom.fallen = true;
            atom.upright_growth.clear();
        } else if !atom.fallen {
            bake_tip_into_body(atom);
        } else if upright_mast_tippy(atom, 1) {
            bake_tip_into_body(atom);
        } else {
            atom.fallen = true;
        }
        atom.gy = top;
        atom.fy = top as f32;
        atom.vel_y = 0.0;
        atom.last_water_top = Some(top);
    } else if atom.fallen {
        // Shore (rooted or not): stay tipped; seat on the beach surface.
        if let Some(slot) = find_surface_air_slot(world, atom.gx, atom.gy) {
            atom.gy = slot;
        }
        atom.fy = atom.gy as f32;
        atom.vel_y = 0.0;
        atom.last_water_top = None;
    } else if !stemless {
        // Substrate gone (sand eroded) with no standing water: bake the tip
        // so the chassis survives, then drop onto the local surface tipped.
        bake_tip_into_body(atom);
        if let Some(slot) = find_surface_air_slot(world, atom.gx, atom.gy) {
            atom.gy = slot;
        }
        atom.fy = atom.gy as f32;
        atom.vel_y = 0.0;
        atom.last_water_top = None;
    } else if let Some(slot) = find_surface_air_slot(world, atom.gx, atom.gy) {
        atom.gy = slot;
        pin_plant_pose(atom);
    } else {
        pin_plant_pose(atom);
    }
    // Roots bank surplus above the spawn tank (starch analogy).
    sync_root_storage(atom);
    // Pore moisture under roots, or free standing water on leaves.
    let moist = plant_moisture_frac(world, atom);
    let bathing = leaves_bathing(world, atom);
    let drought = drought_band(moist);
    let dormant = matches!(drought, DroughtBand::Dormant);
    if dormant {
        atom.drought_ticks = atom.drought_ticks.saturating_add(1);
        if atom.drought_ticks >= DROUGHT_HIBERNATE_MAX_TICKS {
            return PlantStep::Dead;
        }
    } else {
        atom.drought_ticks = 0;
    }

    let n_photo = atom.photosystem_count();
    let n_mod = atom.body.len().max(1) as f32;
    // Plants respire less than plankton blooms.
    let mut upkeep = UPKEEP_PER_MODULE * PLANT_UPKEEP_MULT * n_mod * (0.45 + 0.55 * day);

    if dormant {
        upkeep *= DROUGHT_DORMANT_UPKEEP;
        atom.energy = (atom.energy - upkeep).clamp(0.0, atom.energy_max);
        return if atom.energy <= 0.0 {
            PlantStep::Dead
        } else {
            PlantStep::Alive {
                sat: 0,
                at: (atom.gx, atom.gy),
            }
        };
    }

    let (drink_e, sat_taken, drink_at) = drink_plant(world, atom);
    // Per-leaf column Beer–Lambert at posed cells (flop/pile). Lower leaves
    // and plants under taller neighbours harvest less than open tips.
    let (tip_x, tip_y) = posed_canopy_sample(posed, atom_idx, (atom.gx, canopy_top_y(atom)));
    let tip_x = world.wrap_x(tip_x);
    let submerged = is_wet_air(world, tip_x, tip_y);
    let mut light_sum = sum_posed_photo_light(
        canopy,
        posed,
        atom_idx,
        &|wx, wy| column_light(world, world.wrap_x(wx), wy) * day,
        &atom.genome,
    );
    // Fallback when pose missed leaves: tip sample × count (pre-pose path).
    if light_sum <= 0.0 && n_photo > 0 {
        let sky = column_light(world, tip_x, tip_y) * day;
        let tip = crate::shade::effective_photo_light(canopy, tip_x, tip_y, sky, &atom.genome);
        light_sum = tip * n_photo as f32;
    }
    // Mean leaf light for submerged stem-urge threshold.
    let light = if n_photo > 0 {
        light_sum / n_photo as f32
    } else {
        0.0
    };
    let photo_scale = match drought {
        DroughtBand::Hydrated => 1.25, // mild bonus so moist sand recovers
        DroughtBand::Stressed => 0.55,
        DroughtBand::Dormant => 0.0,
    };
    let harvest = PHOTON_RATE * light_sum * photo_scale;
    let stress = match drought {
        DroughtBand::Stressed => DROUGHT_STRESS_DRAIN,
        DroughtBand::Hydrated | DroughtBand::Dormant => 0.0,
    };
    atom.energy = (atom.energy + harvest + drink_e - upkeep - stress).clamp(0.0, atom.energy_max);
    if atom.energy <= 0.0 {
        return PlantStep::Dead;
    }
    // Woody plants shed leaves that stay dim (self/neighbour shade). Seaweed
    // ribbons keep their frond — no abscission on stemless bodies.
    let _ = shed_unproductive_woody_leaves(world, atom, canopy, day, tick);
    // Surplus → tissue. Submerged + dim light: race toward brighter water.
    // Stemmed plants urge olive upward; stemless seaweed elongates the
    // Photosystem ribbon instead (no trunk invent). When leaves bathe in
    // standing water, root urge collapses — holdfast is enough.
    let genome_save = atom.genome;
    let lake_surface = column_standing_surface(world, atom.gx, atom.gy);
    if atom.fallen {
        if lake_surface.is_some() {
            // Lake float: elongate dangling roots into wet void / raft, and
            // grow a fresh upright mast (body tip is baked flat).
            atom.genome.alloc_root = atom.genome.alloc_root.max(0.20).min(0.40);
            atom.genome.alloc_stem = atom.genome.alloc_stem.max(0.28);
            atom.genome.alloc_leaf = atom.genome.alloc_leaf.max(0.28);
        } else {
            // Shore strand: root into the beach; new stems still grow upright.
            atom.genome.alloc_root = (atom.genome.alloc_root + 0.30).min(0.75);
            atom.genome.alloc_stem = atom.genome.alloc_stem.max(0.22);
        }
    } else if bathing {
        atom.genome.alloc_root = atom.genome.alloc_root.min(0.06);
        if crate::plant::stem_count(atom) == 0 && n_photo > 0 {
            atom.genome.alloc_leaf = (atom.genome.alloc_leaf + 0.20).min(1.0);
            atom.genome.alloc_stem = 0.0;
        }
    }
    if !atom.fallen && submerged && light < SUBMERGED_STEM_URGE_LIGHT {
        if crate::plant::stem_count(atom) > 0 {
            atom.genome.alloc_stem = (atom.genome.alloc_stem + 0.40).min(1.0);
            atom.genome.alloc_root = (atom.genome.alloc_root * 0.55).max(0.05);
            atom.genome.alloc_leaf = (atom.genome.alloc_leaf * 0.85).max(0.05);
        } else if n_photo > 0 {
            atom.genome.alloc_leaf = (atom.genome.alloc_leaf + 0.40).min(1.0);
            atom.genome.alloc_root = (atom.genome.alloc_root * 0.55).max(0.05);
            atom.genome.alloc_stem = 0.0;
        }
    }
    let _ = try_grow_plant(
        world,
        atom,
        tick,
        trunks,
        live_roots,
        live_photos,
        growth_caps,
        canopy,
        entity_id,
    );
    atom.genome = genome_save;
    sync_root_storage(atom);
    // Fern-style wind spores before local rhizome (longer range, needs ReproSpore).
    if let Some(child) =
        try_plant_wind_spore(world, atom, tick, entity_id, pop_room, plant_cols, wind_vx)
    {
        return PlantStep::Spore(child);
    }
    if let Some(child) =
        try_vegetative_sprout(world, atom, tick, entity_id, pop_room, plant_cols)
    {
        return PlantStep::Sprout(child);
    }
    PlantStep::Alive {
        sat: sat_taken,
        at: drink_at,
    }
}

/// Fruiting-body tick: feed from mycelium field / litter, seed local threads.
/// Underground network persistence is [`crate::fungi::step_mycelium_field`].
fn step_fungus(
    world: &mut World,
    atom: &mut Atom,
    day: f32,
    tick: u64,
    entity_id: u32,
    pop_room: bool,
    wind_vx: f32,
    fungus_cols: &[i32],
) -> PlantStep {
    pin_plant_pose(atom);
    // Prefer Organic seats; fall back to Air-above-solid crown.
    if !is_fungus_seated(world, atom) {
        if let Some(slot) = find_fungus_slot(world, atom.gx, atom.gy)
            .or_else(|| find_surface_air_slot(world, atom.gx, atom.gy))
        {
            atom.gy = slot;
            pin_plant_pose(atom);
        }
    }
    let dormant = fungus_should_hibernate(world, atom);
    if dormant {
        atom.drought_ticks = atom.drought_ticks.saturating_add(1);
        if atom.drought_ticks >= FUNGUS_HIBERNATE_MAX_TICKS {
            return PlantStep::Dead;
        }
    } else {
        atom.drought_ticks = 0;
    }

    let network = fruiting_body_supported(world, atom);
    let mut upkeep = fungus_upkeep(atom, dormant || network);
    // Light basal tax so empty chassis still burns sugar.
    upkeep += UPKEEP_PER_MODULE * 0.35 * (0.45 + 0.55 * day);
    if network {
        // Established moist mycelium carries most of the metabolic load.
        upkeep *= 0.35;
    }

    if !dormant {
        let want = digest_budget_units(&atom.genome, atom);
        let (_taken, from_litter) = digest_labile(world, atom.gx, atom.gy, want);
        let from_organic = forage_organic_energy(world, atom.gx, atom.gy, &atom.genome, atom);
        let from_myc = colonize_and_compost(world, atom.gx, atom.gy, &atom.genome, atom, tick);
        atom.energy =
            (atom.energy + from_litter + from_organic + from_myc).min(atom.energy_max);
    }

    atom.energy = (atom.energy - upkeep).clamp(0.0, atom.energy_max);
    // Mature network-backed fruiting bodies don't energy-starve — babies
    // must still earn (anti-flood: sporelings can't pad the pop forever).
    if network && atom.age_ticks >= FRUIT_SUPPORT_MIN_AGE {
        atom.energy = atom.energy.max(1.0);
    } else if atom.energy <= 0.0 {
        return PlantStep::Dead;
    }
    if !dormant {
        if let Some(child) =
            try_spore(world, atom, tick, entity_id, pop_room, wind_vx, fungus_cols)
        {
            return PlantStep::Spore(child);
        }
    }
    PlantStep::Alive {
        sat: 0,
        at: (atom.gx, atom.gy),
    }
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

/// Horizontal drive below which submerged fronds hang still (no idle wave).
/// Climate wind and local water-sat shear both count as drive.
pub const FROND_STILL_WIND: f32 = 0.08;

/// Cells of soft leaf a dry *stemless* ribbon can hold before it hangs.
pub const LEAF_SUPPORT_DRY: i32 = 1;
/// Woody canopy: petiole length held stiff before a small tip nod (cells).
/// Leaves on a trunk/branch never flatten into a ground mat.
pub const LEAF_SUPPORT_WOODY: i32 = 3;

/// Resolve every living module to a world draw cell (flop + pile).
///
/// Soft Photosystems that would share a cell stack upward so dry mats
/// pile instead of overwriting. Same pose feeds canopy shade.
pub fn resolve_organism_draw_cells(
    world: &World,
    atoms: &[Atom],
    tick: u64,
    wind_vx: f32,
) -> Vec<PosedModule> {
    use std::collections::HashSet;
    let mut occupied: HashSet<(i32, i32)> = HashSet::new();
    let mut out = Vec::with_capacity(atoms.iter().map(|a| a.body.len()).sum());
    for (atom_idx, atom) in atoms.iter().enumerate() {
        for &(dx0, dy0, mid) in &atom.body {
            let (dx, dy) = atom.fallen_draw_offset(dx0, dy0);
            let (wx0, wy0) = frond_draw_cell(world, atom, dx, dy, mid, tick, wind_vx);
            // Keep unwrapped draw X for the renderer (it paints wrap copies).
            // World queries use wrap_x.
            let wx = wx0;
            let mut wy = wy0;
            let qx = world.wrap_x(wx);
            // Stemless soft mats pile in free Air so flopped greens don't
            // overwrite. Woody canopy leaves stay on their stem pose —
            // piling would float them a cell off the petiole.
            if mid == ModuleId::Photosystem && crate::plant::stem_count(atom) == 0 {
                for _ in 0..24 {
                    // Missing world cells (tests / unloaded) count as free air.
                    let blocked = match world.get_cell(qx, wy) {
                        Some(c) => c.material != MaterialId::Air,
                        None => false,
                    };
                    if !blocked && !occupied.contains(&(wx, wy)) {
                        break;
                    }
                    wy += 1;
                }
            }
            occupied.insert((wx, wy));
            out.push(PosedModule {
                atom_idx,
                wx,
                wy,
                mid,
            });
        }
    }
    out
}

/// Tip pose for free-float / unstable-raft draw.
///
/// Canopy (dy ≥ 0) lays along +x at the waterline; roots (dy < 0) hang
/// straight down into the water — never into the air.
pub fn fallen_body_offset(dx: i16, dy: i16) -> (i16, i16) {
    if dy < 0 {
        (dx, dy)
    } else {
        (dx + dy, 0)
    }
}

/// Bake the tip into body offsets so growth reorients.
///
/// After this, the old canopy lies on `dy == 0` and new Stem elongation
/// (`ay + 1`) grows **up** from the waterline instead of continuing the
/// phantom pre-tip vertical axis. Clears `upright_growth`.
pub fn bake_tip_into_body(atom: &mut Atom) {
    use std::collections::HashSet;

    atom.fallen = true;
    let mut next: Vec<BodyModule> = Vec::with_capacity(atom.body.len());
    let mut used: HashSet<(i16, i16)> = HashSet::new();

    // Roots keep hanging; nucleus stays at the crown.
    for &(dx, dy, m) in &atom.body {
        if m == ModuleId::Nucleus {
            if used.insert((0, 0)) {
                next.push((0, 0, ModuleId::Nucleus));
            }
            continue;
        }
        if m == ModuleId::Root || dy < 0 {
            let mut place = (dx, dy);
            let mut guard = 0;
            while !used.insert(place) && guard < 24 {
                place.0 += if place.0 >= 0 { 1 } else { -1 };
                guard += 1;
            }
            if guard < 24 {
                next.push((place.0, place.1, m));
            }
        }
    }

    let mut canopy: Vec<BodyModule> = atom
        .body
        .iter()
        .copied()
        .filter(|&(_, dy, m)| m != ModuleId::Nucleus && m != ModuleId::Root && dy >= 0)
        .collect();
    // Lower body-Y claims the waterline first (stable tip order).
    canopy.sort_by_key(|&(dx, dy, _)| (dy, dx));
    for (dx, dy, m) in canopy {
        let (mut nx, ny) = fallen_body_offset(dx, dy);
        let mut guard = 0;
        while !used.insert((nx, ny)) && guard < 32 {
            nx += 1;
            guard += 1;
        }
        if guard < 32 {
            next.push((nx, ny, m));
        }
    }

    atom.body = next;
    atom.upright_growth.clear();
    atom.leaf_starve.clear();
}

/// Draw offset for a tipped plant cell.
///
/// Pre-tip canopy uses [`fallen_body_offset`]. Cells in `upright_growth`
/// stack upright from the waterline with no gaps: visual Y is `1 +` the
/// number of distinct upright body-Y values below this cell. (Body Y still
/// includes the tipped trunk, so a raw `dy - tipped_top` remap can skip a
/// row when trunk/leaf heights don't line up.)
pub fn fallen_draw_offset(
    fallen: bool,
    upright_growth: &[(i16, i16)],
    body: &[BodyModule],
    dx: i16,
    dy: i16,
) -> (i16, i16) {
    if !fallen {
        return (dx, dy);
    }
    if !upright_growth.iter().any(|&p| p == (dx, dy)) {
        return fallen_body_offset(dx, dy);
    }
    // Distinct upright body-Y values strictly below `dy` that still exist on
    // the body (shed leaves must not leave phantom ranks / gaps).
    let mut seen: Vec<i16> = Vec::with_capacity(upright_growth.len());
    for &(ux, uy) in upright_growth {
        if uy < dy
            && !seen.contains(&uy)
            && body.iter().any(|&(bx, by, _)| bx == ux && by == uy)
        {
            seen.push(uy);
        }
    }
    (dx, 1 + seen.len() as i16)
}

/// Soft frond draw pose. Roots / Nucleus / Digest stay rigid. Cosmetic —
/// body cells for physics stay upright.
///
/// - **Stemless Photosystem** (seaweed ribbon): soft. Underwater lean with
///   local water flow only (not air wind); emerged tips lay on the free
///   surface; dry mats hang past [`LEAF_SUPPORT_DRY`] and settle onto
///   terrain (pile via [`resolve_organism_draw_cells`]).
/// - **Photosystem on Stem / branch**: stays in the canopy. Long tips may
///   nod a little ([`LEAF_SUPPORT_WOODY`]) but never flatten to the ground
///   or waterline — wood holds the leaf up.
/// - **Stem** (woody): upright on land; underwater leans with flow only.
pub fn frond_draw_cell(
    world: &World,
    atom: &Atom,
    dx: i16,
    dy: i16,
    mid: ModuleId,
    tick: u64,
    wind_vx: f32,
) -> (i32, i32) {
    let base_x = atom.gx + dx as i32;
    let base_y = atom.gy + dy as i32;
    // Free-floating plants already tip via [`fallen_body_offset`]; no lean.
    if atom.fallen {
        return (base_x, base_y);
    }
    let woody = mid == ModuleId::Stem;
    let soft_leaf = mid == ModuleId::Photosystem;
    if !soft_leaf && !woody {
        return (base_x, base_y);
    }

    let flow = local_water_drive(world, base_x, base_y);
    let wind = wind_vx.abs();
    // Emerged tissue feels air wind; submerged tissue only follows water flow
    // so seabed / drowned plants don't "sail" visually with the breeze.
    let band = wet_band(world, atom.gx, atom.gy);
    let submerged = band
        .map(|(top, bed)| base_y >= bed && base_y <= top)
        .unwrap_or(false);
    let drive = if submerged { flow } else { wind.max(flow) };
    let dir = if submerged {
        local_water_dir(world, base_x, base_y)
    } else if flow > wind {
        local_water_dir(world, base_x, base_y)
    } else if wind_vx >= 0.0 {
        1
    } else {
        -1
    };
    let lean_dir = if drive < FROND_STILL_WIND { 1 } else { dir };
    let cant = leaf_cantilever(atom, dx, dy);
    let on_wood = soft_leaf
        && atom
            .body
            .iter()
            .any(|(_, _, m)| *m == ModuleId::Stem);

    if woody {
        let Some((top, bed)) = band else {
            return (base_x, base_y);
        };
        if base_y > top || base_y < bed || drive < FROND_STILL_WIND {
            return (base_x, base_y);
        }
        let tip_w = (dy.max(0) as f32) * 0.30;
        let lean_mag = (drive * (1.2 + tip_w)).ceil().clamp(1.0, 2.0) as i32;
        return (base_x + dir * lean_mag, base_y);
    }

    // Leaves attached to a trunk/branch stay in the canopy.
    if on_wood {
        let nod = (cant - LEAF_SUPPORT_WOODY).max(0).min(2);
        let wy = base_y - nod;
        if let Some((top, bed)) = band {
            if base_y >= bed && base_y <= top && drive >= FROND_STILL_WIND {
                let tip_w = (dy.max(0) as f32) * 0.20;
                let lean_mag = (drive * (0.8 + tip_w)).ceil().clamp(1.0, 2.0) as i32;
                return (base_x + dir * lean_mag, wy);
            }
        }
        return (base_x, wy);
    }

    let outward = if dx == 0 {
        lean_dir
    } else if dx > 0 {
        1
    } else {
        -1
    };

    if let Some((top, bed)) = band {
        if base_y > top {
            // Stemless emerged tip: flop onto the waterline, then lift clear
            // of solid so shore ribbons rest on the beach.
            let overhang = base_y - top;
            let wx = atom.gx + dx as i32 + lean_dir * overhang;
            let wy = rest_soft_leaf_y(world, wx, top);
            return (wx, wy);
        }
        if base_y >= bed {
            // Stemless underwater: lean / ripple with local flow only.
            if drive < FROND_STILL_WIND {
                return (base_x, base_y);
            }
            let tip_w = (dy.max(0) as f32).max(cant as f32 * 0.5) * 0.30;
            let phase = tick as f32 * (0.04 + drive * 0.10)
                + atom.gx as f32 * 0.19
                + dy as f32 * 0.85;
            let amp = (drive * 0.9 * (0.5 + tip_w)).clamp(0.0, 1.4);
            let sway = (phase.sin() * amp).round() as i32;
            let lean_mag = (drive * (1.2 + tip_w)).ceil().clamp(1.0, 2.0) as i32;
            return (base_x + dir * lean_mag + sway, base_y);
        }
    }

    // Stemless dry air: hang into a mat on the terrain surface.
    let hang = (cant - LEAF_SUPPORT_DRY).max(0);
    let (sx, sy) = nearest_leaf_support(atom, dx, dy);
    let wx = atom.gx + sx as i32 + outward * (LEAF_SUPPORT_DRY.min(cant) + hang);
    let preferred = (atom.gy + sy as i32 - hang).max(atom.gy - 1);
    let wy = if hang > 0 {
        rest_soft_leaf_y(world, wx, preferred)
    } else {
        clear_solid_y(world, wx, preferred)
    };
    (wx, wy)
}

/// Manhattan distance from a Photosystem to the nearest Stem, else Nucleus.
fn leaf_cantilever(atom: &Atom, dx: i16, dy: i16) -> i32 {
    let mut best = i32::MAX;
    for &(x, y, m) in &atom.body {
        if m != ModuleId::Stem && m != ModuleId::Nucleus {
            continue;
        }
        let d = (x - dx).abs() as i32 + (y - dy).abs() as i32;
        best = best.min(d);
    }
    if best == i32::MAX {
        dx.abs() as i32 + dy.max(0) as i32
    } else {
        best
    }
}

fn nearest_leaf_support(atom: &Atom, dx: i16, dy: i16) -> (i16, i16) {
    let mut best: Option<(i32, i16, i16)> = None;
    for &(x, y, m) in &atom.body {
        if m != ModuleId::Stem && m != ModuleId::Nucleus {
            continue;
        }
        let d = (x - dx).abs() as i32 + (y - dy).abs() as i32;
        if best.map(|(bd, _, _)| d < bd).unwrap_or(true) {
            best = Some((d, x, y));
        }
    }
    best.map(|(_, x, y)| (x, y)).unwrap_or((0, 0))
}

fn air_sat(world: &World, gx: i32, gy: i32) -> i16 {
    match world.get_cell(world.wrap_x(gx), gy) {
        Some(c) if c.material == MaterialId::Air => c.sat.0 as i16,
        _ => 0,
    }
}

/// Lateral sat shear near a cell — proxy for surface / column flow.
fn local_water_drive(world: &World, gx: i32, gy: i32) -> f32 {
    let mut best = 0i16;
    for y in [gy - 1, gy, gy + 1] {
        let l = air_sat(world, gx - 1, y);
        let c = air_sat(world, gx, y);
        let r = air_sat(world, gx + 1, y);
        best = best.max((r - l).abs()).max((c - l).abs()).max((c - r).abs());
    }
    (best as f32 / 255.0) * 0.65
}

fn local_water_dir(world: &World, gx: i32, gy: i32) -> i32 {
    let l = air_sat(world, gx - 1, gy) + air_sat(world, gx - 1, gy + 1);
    let r = air_sat(world, gx + 1, gy) + air_sat(world, gx + 1, gy + 1);
    if r >= l {
        1
    } else {
        -1
    }
}

fn clear_solid_y(world: &World, gx: i32, preferred_y: i32) -> i32 {
    let gx = world.wrap_x(gx);
    let mut y = preferred_y.max(0);
    for _ in 0..64 {
        match world.get_cell(gx, y) {
            Some(c) if c.material != MaterialId::Air => y += 1,
            // Missing / unloaded cells: leave the pose alone.
            _ => break,
        }
    }
    y
}

/// Lift out of solid, then (when dry) drop onto the ground so flopped
/// leaves rest on the terrain surface instead of painting through it.
fn rest_soft_leaf_y(world: &World, gx: i32, preferred_y: i32) -> i32 {
    let gx = world.wrap_x(gx);
    let mut y = clear_solid_y(world, gx, preferred_y);
    if !is_wet_air(world, gx, y) {
        for _ in 0..64 {
            match world.get_cell(gx, y - 1) {
                Some(c) if c.material == MaterialId::Air && c.sat.is_empty() => y -= 1,
                _ => break,
            }
        }
    }
    y
}

/// True when a Root/Nucleus sits in or on Organic (raft / compost holdfast).
/// Holdfast on a *floating* Organic column (raft), not grounded compost.
fn rooted_in_floating_organic(
    world: &World,
    atom: &Atom,
    columns: &std::collections::HashMap<i32, (i32, i32)>,
) -> bool {
    use crate::plant::holdfast_on_float_column;

    if columns.is_empty() {
        return false;
    }
    for &(dx, dy, m) in &atom.body {
        if m != ModuleId::Root && m != ModuleId::Nucleus {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        if holdfast_on_float_column(columns, m, wx, wy) {
            return true;
        }
    }
    false
}

fn organic_in_floating_column(
    columns: &std::collections::HashMap<i32, (i32, i32)>,
    wx: i32,
    wy: i32,
) -> bool {
    let Some(&(bottom, height)) = columns.get(&wx) else {
        return false;
    };
    wy >= bottom && wy < bottom + height
}

/// Root purchase in sand/rock/grounded organic — not a floating raft cell.
///
/// - Near-crown roots (`dy >= -6`): grip the root cell or the cell below
///   (root in Air above sand).
/// - Deeper roots embedded in mineral count only when the crown is **not**
///   over standing water — so land plants keep deep sand purchase, but a
///   free-floater tendril scraping the lake bed cannot pin the crown mid-air.
fn grounded_substrate_anchor(
    world: &World,
    atom: &Atom,
    columns: &std::collections::HashMap<i32, (i32, i32)>,
) -> bool {
    const NEAR_CROWN: i16 = 6;
    let over_water = column_standing_surface(world, atom.gx, atom.gy).is_some();
    for &(dx, dy, m) in &atom.body {
        if m != ModuleId::Root {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        if substrate_purchase_at(world, columns, wx, wy) {
            if dy >= -NEAR_CROWN || !over_water {
                return true;
            }
        }
        if dy >= -NEAR_CROWN && substrate_purchase_at(world, columns, wx, wy - 1) {
            return true;
        }
    }
    false
}

fn substrate_purchase_at(
    world: &World,
    columns: &std::collections::HashMap<i32, (i32, i32)>,
    wx: i32,
    wy: i32,
) -> bool {
    let Some(c) = world.get_cell(wx, wy) else {
        return false;
    };
    if c.material == MaterialId::Air {
        return false;
    }
    if c.material == MaterialId::Organic && organic_in_floating_column(columns, wx, wy) {
        return false;
    }
    true
}

/// Solid Y of the local ground under the crown, if short roots still grip it.
///
/// Only roots in the crown columns (`|dx| <= 1`, `dy >= -6`) count. Lateral
/// rhizomes upslope must not hoist `gy` — that was lifting shoreline plants
/// with the waterline. Deep free-floater tendrils are ignored so castaways
/// stay on the surface.
///
/// **Organic never counts** — floating mats and beach litter must not become
/// a holdfast seat that pumps sand-rooted crowns upward. Raft seating uses
/// [`rooted_in_floating_organic`] instead.
fn crown_holdfast_solid_y(
    world: &World,
    atom: &Atom,
    _columns: &std::collections::HashMap<i32, (i32, i32)>,
) -> Option<i32> {
    const MAX_ROOT_LEN: i16 = 6;
    let mut best: Option<i32> = None;
    for &(dx, dy, m) in &atom.body {
        if m != ModuleId::Root || dy < -MAX_ROOT_LEN || dx.abs() > 1 {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        for y in (wy - 1)..=(wy + 1) {
            let Some(c) = world.get_cell(wx, y) else {
                continue;
            };
            if c.material == MaterialId::Air || c.material == MaterialId::Organic {
                continue;
            }
            // Ground surface under the crown (highest mineral solid).
            best = Some(best.map_or(y, |b| b.max(y)));
        }
    }
    best
}

/// Top Y of a full standing-water column near `hint_y`.
///
/// Only **full-sat** wet Air counts — damp seepage films must not tip land
/// plants or lift holdfast seaweed. (`is_standing_water` alone is too loose:
/// any non-empty sat on solid would look like a stream.)
fn column_standing_surface(world: &World, gx: i32, hint_y: i32) -> Option<i32> {
    let gx = world.wrap_x(gx);
    let is_body = |world: &World, x: i32, y: i32| {
        matches!(
            world.get_cell(x, y),
            Some(c) if c.material == MaterialId::Air && c.sat.is_full()
        )
    };
    let mut start = None;
    if is_body(world, gx, hint_y) {
        start = Some(hint_y);
    } else {
        for dy in 1..=48 {
            for y in [hint_y - dy, hint_y + dy] {
                if is_body(world, gx, y) {
                    start = Some(y);
                    break;
                }
            }
            if start.is_some() {
                break;
            }
        }
    }
    let start = start?;
    let mut top = start;
    while is_body(world, gx, top + 1) {
        top += 1;
        if top - start > 256 {
            break;
        }
    }
    Some(top)
}

/// First tip, or re-tip an upright mast that grew too tall on a skinny raft.
///
/// Woody plants bake the flop into body offsets so the next stem elongates
/// upward. Stemless seaweed only sets `fallen` (soft draw) — baking would
/// turn the ribbon into a permanent sideways trunk.
///
/// Effective beam = floating-Organic footprint + root keel (extra dangling
/// roots under the raft make tip less likely).
fn apply_raft_tip(
    world: &World,
    atom: &mut Atom,
    columns: &std::collections::HashMap<i32, (i32, i32)>,
) {
    let Some(support) = raft_tip_support(world, atom, columns) else {
        return;
    };
    let stemless = crate::plant::stem_count(atom) == 0;
    if stemless {
        if upright_body_tippy(atom, support) {
            atom.fallen = true;
            atom.upright_growth.clear();
        }
        return;
    }
    if !atom.fallen {
        if upright_body_tippy(atom, support) {
            bake_tip_into_body(atom);
        }
        return;
    }
    if upright_mast_tippy(atom, support) {
        bake_tip_into_body(atom);
    }
}

/// Raft beam for tip checks: Organic footprint width plus dangling-root keel.
fn raft_tip_support(
    world: &World,
    atom: &Atom,
    columns: &std::collections::HashMap<i32, (i32, i32)>,
) -> Option<i32> {
    let width = raft_support_width(world, atom, columns)?;
    let keel = raft_root_keel(atom);
    Some(width + keel)
}

/// Extra roots hanging under the crown act as a keel against tip.
///
/// The first root is the mount; each additional `dy < 0` Root adds one
/// effective support column so a heavily rooted skinny raft stays upright.
fn raft_root_keel(atom: &Atom) -> i32 {
    let roots = atom
        .body
        .iter()
        .filter(|(_, dy, m)| *m == ModuleId::Root && *dy < 0)
        .count() as i32;
    roots.saturating_sub(1)
}

fn raft_support_width(
    world: &World,
    atom: &Atom,
    columns: &std::collections::HashMap<i32, (i32, i32)>,
) -> Option<i32> {
    use crate::plant::holdfast_on_float_column;

    if columns.is_empty() {
        return None;
    }
    // Body-local span — never `wrap_x` min..=max (ring seams explode that).
    let mut min_dx = i16::MAX;
    let mut max_dx = i16::MIN;
    let mut has_holdfast = false;
    for &(dx, dy, m) in &atom.body {
        if m != ModuleId::Root && m != ModuleId::Nucleus {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        min_dx = min_dx.min(dx);
        max_dx = max_dx.max(dx);
        if holdfast_on_float_column(columns, m, wx, wy) {
            has_holdfast = true;
        }
    }
    if !has_holdfast || min_dx > max_dx {
        return None;
    }
    let support = (min_dx..=max_dx)
        .filter(|&d| columns.contains_key(&world.wrap_x(atom.gx + d as i32)))
        .count() as i32;
    (support > 0).then_some(support)
}

/// True when a Root has purchase in sand/rock/grounded compost (not a raft).
pub(crate) fn plant_grounded_in_substrate(
    world: &World,
    atom: &Atom,
    columns: &std::collections::HashMap<i32, (i32, i32)>,
) -> bool {
    grounded_substrate_anchor(world, atom, columns)
}

/// Upright (pre-tip) body sail height vs raft footprint.
fn upright_body_tippy(atom: &Atom, support: i32) -> bool {
    let height = atom
        .body
        .iter()
        .filter(|(_, _, m)| {
            matches!(
                m,
                ModuleId::Stem | ModuleId::Photosystem | ModuleId::Nucleus
            )
        })
        .map(|(_, dy, _)| i32::from(*dy))
        .max()
        .unwrap_or(0);
    sail_tippy(height, support)
}

/// Post-tip upright mast height (draw stack) vs support footprint.
fn upright_mast_tippy(atom: &Atom, support: i32) -> bool {
    if atom.upright_growth.is_empty() {
        return false;
    }
    let mut seen: Vec<i16> = Vec::new();
    for &(_, y) in &atom.upright_growth {
        if !seen.contains(&y) {
            seen.push(y);
        }
    }
    sail_tippy(seen.len() as i32, support)
}

fn sail_tippy(height: i32, support: i32) -> bool {
    let support = support.max(1);
    height >= 3 && height >= support * 2
}

/// Contiguous wet-Air band containing `hint_y` (or nearest wet cell).
/// Returns `(top, bed)` free-surface and bed Y.
pub fn wet_band(world: &World, gx: i32, hint_y: i32) -> Option<(i32, i32)> {
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
                // Land plants / fungi stay pinned — only plankton shove apart.
                if is_land_plant(&atoms[i])
                    || is_land_plant(&atoms[j])
                    || is_fungus(&atoms[i])
                    || is_fungus(&atoms[j])
                {
                    continue;
                }
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

/// Beer–Lambert-ish transmittance per standing-water (wet Air) cell
/// when scanning skyward from a photosystem. ~0.85^10 ≈ 0.20 at 10 deep.
pub const WATER_LIGHT_TRANSMIT: f32 = 0.85;
/// One-time loss when light crosses the free-water surface into dry air.
pub const WATER_SURFACE_TRANSMIT: f32 = 0.90;
/// Below this effective light, submerged stemmed plants urge toward the
/// surface (seaweed race) while surplus still exists.
pub const SUBMERGED_STEM_URGE_LIGHT: f32 = 0.42;

/// Sky light remaining at `(gx, gy)` after water / solid occlusion.
/// Dry air is clear; each wet Air cell attenuates; solids nearly black out.
pub fn column_sky_light(world: &World, gx: i32, gy: i32) -> f32 {
    let mut light = 1.0f32;
    let mut under_water = is_wet_air(world, gx, gy);
    let mut applied_surface = false;
    let mut y = gy + 1;
    let mut steps = 0;
    while steps < 96 {
        match world.get_cell(gx, y) {
            None => break,
            Some(c) if c.material == MaterialId::Air => {
                if !c.sat.is_empty() {
                    light *= WATER_LIGHT_TRANSMIT;
                    under_water = true;
                } else if under_water {
                    if !applied_surface {
                        light *= WATER_SURFACE_TRANSMIT;
                        applied_surface = true;
                    }
                    under_water = false;
                }
            }
            Some(c)
                if matches!(
                    c.material,
                    MaterialId::Ice | MaterialId::Snow
                ) =>
            {
                light *= 0.35;
            }
            Some(_) => {
                // Buried under rock / soil / Organic — only a trickle.
                return (light * 0.12).clamp(0.0, 1.0);
            }
        }
        y += 1;
        steps += 1;
    }
    light.clamp(0.0, 1.0)
}

fn column_light(world: &World, gx: i32, gy: i32) -> f32 {
    column_sky_light(world, gx, gy)
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
            let body = crate::blueprint::mutate_body(
                &parent.body,
                parent.clone_fidelity,
                world.seed.0,
                tick,
                parent.age_ticks as u32,
            );
            let mut child = Atom::from_body(nx, ny, parent.energy_max, body);
            child.energy = child_energy.clamp(1.0, parent.energy_max);
            child.cooldown = REPRO_PERIOD;
            child.circadian_phase = parent.circadian_phase;
            child.active_window = parent.active_window;
            child.last_water_top = parent.last_water_top;
            let g = Genome::mutate(parent.genome, world.seed.0, tick, parent.age_ticks as u32);
            apply_genome(&mut child, g);
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

fn is_air(world: &World, gx: i32, gy: i32) -> bool {
    matches!(
        world.get_cell(gx, gy),
        Some(c) if c.material == MaterialId::Air
    )
}

/// Prefer the clicked Air cell; otherwise nearest Air in the column.
fn find_air_near(world: &World, gx: i32, gy: i32) -> Option<i32> {
    let gx = world.wrap_x(gx);
    if is_air(world, gx, gy) {
        return Some(gy);
    }
    for dy in [1, -1, 2, -2, 3, -3, 4, -4, 5, -5, 8, -8, 12, -12, 16, -16] {
        let ny = gy + dy;
        if is_air(world, gx, ny) {
            return Some(ny);
        }
    }
    for y in (gy - 48..=gy + 48).rev() {
        if is_air(world, gx, y) {
            return Some(y);
        }
    }
    None
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
    use crate::blueprint::Genome;
    use crate::cell::{Cell, Sat};
    use crate::chunk::ChunkCoord;
    use crate::plant::LAND_GROW_PERIOD;

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
    fn photosystem_tint_darkens_under_shade() {
        assert_eq!(
            ModuleId::photosystem_rgb_for_light(1.0),
            PHOTO_RGB_ACTIVE
        );
        assert_eq!(
            ModuleId::photosystem_rgb_for_light(0.0),
            PHOTO_RGB_SHADED
        );
        let mid = ModuleId::photosystem_rgb_for_light(0.35);
        assert!(
            mid.1 < PHOTO_RGB_ACTIVE.1 && mid.1 > PHOTO_RGB_SHADED.1,
            "mid light should sit between active and shaded ({mid:?})"
        );

        let mut w = moist_sand_plot();
        let mut open_g = Genome::default();
        open_g.leaf_absorb = 0.2;
        let mut thug_g = Genome::default();
        thug_g.leaf_absorb = 0.95;
        let short = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut tall = short.clone();
        tall.push((0, 3, ModuleId::Stem));
        tall.push((0, 4, ModuleId::Stem));
        tall.push((0, 5, ModuleId::Stem));
        tall.push((0, 6, ModuleId::Photosystem));

        let mut alone = OrganismStore::new();
        assert!(alone.spawn_blueprint(&w, 3, 2, short.clone(), 40.0, open_g));
        let mut shaded = OrganismStore::new();
        assert!(shaded.spawn_blueprint(&w, 3, 2, short, 40.0, open_g));
        assert!(shaded.spawn_blueprint(&w, 4, 2, tall, 40.0, thug_g));

        let alone_g = alone
            .draw_list(&w, 0, 0.0)
            .into_iter()
            .filter(|(_, _, (r, g, b))| *g > *r && *g > *b)
            .map(|(_, _, rgb)| rgb.1)
            .max()
            .unwrap();
        let shaded_g = shaded
            .draw_list(&w, 0, 0.0)
            .into_iter()
            .filter(|&(x, _, (r, g, b))| x == 3 && g > r && g > b)
            .map(|(_, _, rgb)| rgb.1)
            .max()
            .unwrap_or(0);
        assert!(
            shaded_g < alone_g,
            "shaded plant greens should draw darker (shaded g={shaded_g}, alone g={alone_g})"
        );
    }

    #[test]
    fn stacked_leaves_self_shade_in_draw_tint() {
        let mut w = moist_sand_plot();
        let mut g = Genome::default();
        g.leaf_absorb = 0.55;
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
            (0, 3, ModuleId::Photosystem),
            (0, 4, ModuleId::Photosystem),
        ];
        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(&w, 4, 2, body, 40.0, g));
        let greens: Vec<(i32, u8)> = store
            .draw_list(&w, 0, 0.0)
            .into_iter()
            .filter(|&(x, _, (r, g, b))| x == 4 && g > r && g > b)
            .map(|(_, y, rgb)| (y, rgb.1))
            .collect();
        let tip_g = greens.iter().max_by_key(|(y, _)| *y).map(|(_, g)| *g);
        let low_g = greens.iter().min_by_key(|(y, _)| *y).map(|(_, g)| *g);
        assert!(
            tip_g.unwrap() > low_g.unwrap(),
            "tip leaf should draw brighter than lower leaf (tip={tip_g:?}, low={low_g:?})"
        );
    }

    #[test]
    fn atom_draw_list_is_black_and_green_pixels() {
        let mut store = OrganismStore::new();
        store.atoms.push(Atom::new(4, 5, 50.0));
        let w = World::new(3);
        let list = store.draw_list(&w, 0, 0.0);
        assert_eq!(list.len(), 2);
        assert!(list.contains(&(4, 5, (0, 0, 0))));
        let green = list
            .iter()
            .find(|&&(x, y, _)| x == 5 && y == 5)
            .map(|&(_, _, rgb)| rgb)
            .expect("photosystem pixel");
        // Open-sky leaf should sit near the active palette green.
        assert!(
            green.1 >= 0xA0 && green.2 >= 0x30,
            "lit photosystem should read bright green, got {green:?}"
        );
    }

    #[test]
    fn frond_lays_along_free_surface_when_taller_than_water() {
        let w = wet_column();
        // wet_column free surface ~y=7; seat high so the ribbon tip clears it.
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let atom = Atom::from_body(4, 4, 40.0, body);
        let tip_dy = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|(_, y, _)| *y)
            .max()
            .unwrap();
        let tip_world_y = atom.gy + tip_dy as i32;
        let (top, _) = wet_band(&w, 4, 4).expect("wet column");
        assert!(
            tip_world_y > top,
            "fixture tip should clear the free surface (tip={tip_world_y} top={top})"
        );
        let (dx, dy, mid) = atom
            .body
            .iter()
            .copied()
            .find(|(_, y, m)| *m == ModuleId::Photosystem && *y == tip_dy)
            .unwrap();
        let (wx, wy) = frond_draw_cell(&w, &atom, dx, dy, mid, 0, 0.5);
        assert_eq!(wy, top, "tip draws on the waterline");
        assert!(wx > atom.gx, "positive wind lays the tip downwind");
    }

    #[test]
    fn dry_stemless_frond_flops_into_a_mat() {
        let mut w = moist_sand_plot(); // no standing water column
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let atom = Atom::from_body(4, 2, 40.0, body);
        let tip_dy = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|(_, y, _)| *y)
            .max()
            .unwrap();
        let (dx, dy, mid) = atom
            .body
            .iter()
            .copied()
            .find(|(_, y, m)| *m == ModuleId::Photosystem && *y == tip_dy)
            .unwrap();
        let (wx, wy) = frond_draw_cell(&w, &atom, dx, dy, mid, 0, 0.0);
        assert!(
            tip_dy as i32 > LEAF_SUPPORT_DRY,
            "fixture tip must be long enough to hang"
        );
        assert!(
            wy < atom.gy + tip_dy as i32,
            "long dry ribbon tip hangs below body Y (wy={wy} body={})",
            atom.gy + tip_dy as i32
        );
        assert!(
            wx != atom.gx + dx as i32 || tip_dy == 0,
            "dry ribbon should flop laterally (wx={wx})"
        );
        assert!(wx > atom.gx, "flops into a mat beside the holdfast");
    }

    #[test]
    fn roots_draw_inside_substrate_not_on_surface() {
        let mut w = moist_sand_plot(); // sand at y=1, air above
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let atom = Atom::from_body(4, 2, 40.0, body);
        // Template root is at (0,-1) → world (4,1) inside sand.
        let root_body_y = atom.gy
            + atom
                .body
                .iter()
                .find(|(_, _, m)| *m == ModuleId::Root)
                .map(|(_, y, _)| *y)
                .unwrap() as i32;
        assert_eq!(root_body_y, 1, "fixture root sits in the sand row");
        let mut store = OrganismStore::new();
        store.atoms.push(atom);
        let list = store.draw_list(&w, 0, 0.0);
        let root_rgb = ModuleId::Root.rgb();
        let root_draws: Vec<(i32, i32)> = list
            .iter()
            .filter(|(_, _, rgb)| *rgb == root_rgb)
            .map(|&(x, y, _)| (x, y))
            .collect();
        assert!(
            !root_draws.is_empty(),
            "plant should draw at least one Root"
        );
        for &(wx, wy) in &root_draws {
            let cell = w.get_cell(wx, wy).expect("root draw in world");
            assert_ne!(
                cell.material,
                MaterialId::Air,
                "root must not be lifted onto the surface ({wx},{wy})"
            );
            assert_eq!(wy, root_body_y, "root draw stays on body Y");
        }
    }

    #[test]
    fn woody_canopy_leaves_stay_on_the_branch() {
        let mut w = moist_sand_plot();
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
            (1, 2, ModuleId::Photosystem),
            (2, 2, ModuleId::Photosystem),
            (3, 2, ModuleId::Photosystem),
            (4, 2, ModuleId::Photosystem),
        ];
        let atom = Atom::from_body(4, 2, 40.0, body);
        let base_y = atom.gy + 2;
        for &(dx, dy, mid) in &atom.body {
            if mid != ModuleId::Photosystem {
                continue;
            }
            let (wx, wy) = frond_draw_cell(&w, &atom, dx, dy, mid, 0, 0.0);
            assert_eq!(wx, atom.gx + dx as i32, "branch leaf keeps its column");
            assert!(
                wy >= base_y - 2,
                "woody leaf must stay in the canopy, not flatten (wy={wy})"
            );
            assert!(
                wy > atom.gy,
                "trunk leaf must not settle onto the crown/ground (wy={wy})"
            );
        }
        // Long tip may nod a little past woody support length.
        let (_, tip_y) = frond_draw_cell(&w, &atom, 4, 2, ModuleId::Photosystem, 0, 0.0);
        let (_, near_y) = frond_draw_cell(&w, &atom, 1, 2, ModuleId::Photosystem, 0, 0.0);
        assert_eq!(near_y, base_y, "short petiole stays level with the branch");
        assert!(
            tip_y <= near_y,
            "long tip may nod at most a little (tip={tip_y} near={near_y})"
        );
    }

    #[test]
    fn submerged_frond_still_when_water_is_calm() {
        let w = wet_column();
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let atom = Atom::from_body(4, 2, 40.0, body);
        let (dx, dy, mid) = atom
            .body
            .iter()
            .copied()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .max_by_key(|(_, y, _)| *y)
            .unwrap();
        let base_x = atom.gx + dx as i32;
        let base_y = atom.gy + dy as i32;
        for tick in [0u64, 17, 99, 400] {
            let (wx, wy) = frond_draw_cell(&w, &atom, dx, dy, mid, tick, 0.05);
            assert_eq!(
                (wx, wy),
                (base_x, base_y),
                "calm water must not idle-wave (tick={tick})"
            );
        }
        // Atmospheric wind must not bend submerged fronds (flow-only lean).
        for tick in [0u64, 11, 23, 40, 70] {
            let (wx, wy) = frond_draw_cell(&w, &atom, dx, dy, mid, tick, 0.40);
            assert_eq!(
                (wx, wy),
                (base_x, base_y),
                "air wind must not lean submerged fronds (tick={tick})"
            );
        }
    }

    #[test]
    fn submerged_frond_leans_with_local_water_flow() {
        let mut w = wet_column();
        // Shear sat across the tip column so flow drive exceeds the still gate.
        for y in 1..8 {
            let mut left = Cell::air();
            left.sat = Sat(40);
            w.set_cell(3, y, left);
            let mut right = Cell::air();
            right.sat = Sat(255);
            w.set_cell(5, y, right);
        }
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let atom = Atom::from_body(4, 2, 40.0, body);
        let (dx, dy, mid) = atom
            .body
            .iter()
            .copied()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .max_by_key(|(_, y, _)| *y)
            .unwrap();
        let base_x = atom.gx + dx as i32;
        let bent = [0u64, 11, 23, 40, 70].iter().any(|&tick| {
            let (wx, _) = frond_draw_cell(&w, &atom, dx, dy, mid, tick, 0.0);
            wx != base_x
        });
        assert!(bent, "sat shear (flowing water) should bend the frond");
    }

    #[test]
    fn flopped_fronds_pile_instead_of_sharing_one_cell() {
        let mut w = moist_sand_plot();
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        // Adjacent holdfasts — dry flop leans the same way and would overlap.
        store.atoms.push(Atom::from_body(4, 2, 40.0, body.clone()));
        store.atoms.push(Atom::from_body(5, 2, 40.0, body.clone()));
        store.atoms.push(Atom::from_body(6, 2, 40.0, body));
        let list = store.draw_list(&w, 0, 0.0);
        let n_photo: usize = store.atoms.iter().map(|a| a.photosystem_count()).sum();
        let mut greens: Vec<(i32, i32)> = list
            .iter()
            .filter(|(_, _, (r, g, b))| {
                // Any photosystem tint (active‥shaded), not roots/stems.
                *g > *r && *g > *b && *g >= 0x50
            })
            .map(|&(x, y, _)| (x, y))
            .collect();
        assert_eq!(greens.len(), n_photo, "all photosystems should draw");
        let before = greens.len();
        greens.sort_unstable();
        greens.dedup();
        assert_eq!(
            greens.len(),
            before,
            "flopped Photosystems must pile to unique cells, not overwrite"
        );
        let max_y = greens.iter().map(|(_, y)| *y).max().unwrap();
        let min_y = greens.iter().map(|(_, y)| *y).min().unwrap();
        assert!(
            max_y > min_y,
            "dry meadow should stack into a pile height (min={min_y} max={max_y})"
        );
    }

    #[test]
    fn flopped_frond_rests_on_terrain_not_inside_solid() {
        let mut w = moist_sand_plot();
        // Rising beach to the right of the holdfast.
        for x in 5..10 {
            let h = 1 + (x - 4);
            for y in 1..=h {
                let mut sand = Cell::solid(MaterialId::Sand);
                sand.sat = Sat(80);
                w.set_cell(x, y, sand);
            }
            for y in (h + 1)..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let atom = Atom::from_body(4, 2, 40.0, body);
        for &(dx, dy, mid) in &atom.body {
            if mid != ModuleId::Photosystem {
                continue;
            }
            let (wx, wy) = frond_draw_cell(&w, &atom, dx, dy, mid, 0, 0.0);
            let cell = w.get_cell(wx, wy).expect("draw cell in world");
            assert_eq!(
                cell.material,
                MaterialId::Air,
                "flopped leaf must not paint inside solid at ({wx},{wy})"
            );
            if wx >= 5 {
                let below = w.get_cell(wx, wy - 1).unwrap();
                assert_ne!(
                    below.material,
                    MaterialId::Air,
                    "on the beach, mat should rest on sand ({wx},{wy})"
                );
            }
        }
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
        assert_eq!(store.corpse_count(), 1, "stranded plankton becomes a corpse");
    }

    #[test]
    fn death_leaves_grey_corpse_then_organic() {
        let mut w = wet_column();
        w.set_cell(4, 1, Cell::air());
        let mut store = OrganismStore::new();
        let mut a = Atom::new(4, 6, 10.0);
        a.age_ticks = LIFE_TICKS;
        store.atoms.push(a);
        store.step(&mut w, 0);
        assert!(store.is_empty(), "living pop should be gone");
        assert_eq!(store.corpse_count(), 1, "death should leave a lingering corpse");
        assert!(
            crate::fungi::soft_litter_at(&w, 4) == 0,
            "litter waits until dissolve, not instant death"
        );
        // Grey corpse still drawable.
        let draw = store.draw_list(&w, 0, 0.0);
        assert!(!draw.is_empty());
        let (_, _, rgb) = draw[0];
        assert!(rgb.0 <= 120 && rgb.2 <= 70, "corpse should be desaturated brown-grey");

        // Fast-forward settle.
        store.corpses[0].settled_ticks = CORPSE_SETTLE_WATER_TICKS - 1;
        // Pin on bed so settle counts.
        store.corpses[0].gy = 1;
        store.corpses[0].fy = 1.0;
        store.step(&mut w, 1);
        assert_eq!(store.corpse_count(), 0, "settled corpse should dissolve");
        assert!(
            crate::fungi::soft_litter_at(&w, 4) > 0,
            "dissolve should bank soft litter for fungi"
        );
        // Body modules that sat in dry Air become Organic (nucleus+photo at bed).
        let organic_cells = (0..10)
            .filter(|&y| {
                matches!(
                    w.get_cell(4, y).map(|c| c.material),
                    Some(MaterialId::Organic)
                )
            })
            .count();
        assert!(
            organic_cells >= 1,
            "dissolve should leave MaterialId::Organic in the world"
        );
    }

    #[test]
    fn land_plant_corpse_leaves_roots_as_organic_immediately() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            minimal_plant_body(),
            40.0,
            Genome::default(),
        ));
        // Age past plant life (longer than plankton LIFE_TICKS).
        store.atoms[0].age_ticks = super::PLANT_LIFE_TICKS;
        store.atoms[0].energy = 40.0;
        let root_cell = {
            let a = &store.atoms[0];
            a.body
                .iter()
                .find(|(_, _, m)| *m == ModuleId::Root)
                .map(|&(dx, dy, _)| (a.gx + dx as i32, a.gy + dy as i32))
                .expect("plant has a root")
        };
        let sat_before = w.get_cell(root_cell.0, root_cell.1).unwrap().sat.0;
        store.step(&mut w, 0);
        assert!(store.is_empty());
        assert_eq!(store.corpse_count(), 1);
        assert!(store.corpses[0].land);
        // Roots stripped from corpse body — already in the ground.
        assert!(
            store.corpses[0]
                .body
                .iter()
                .all(|(_, _, m)| *m != ModuleId::Root),
            "root modules should leave the grey corpse"
        );
        assert_eq!(
            w.get_cell(root_cell.0, root_cell.1).map(|c| c.material),
            Some(MaterialId::Organic),
            "dead roots should already be Organic in place"
        );
        // Pore water preserved through the conversion.
        assert_eq!(
            w.get_cell(root_cell.0, root_cell.1).unwrap().sat.0,
            sat_before.min(crate::cell::water_capacity(MaterialId::Organic)),
            "Organic conversion must not destroy pore sat"
        );
    }

    #[test]
    fn land_plant_death_drops_leaves_keeps_stem_corpse() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            minimal_plant_body(),
            40.0,
            Genome::default(),
        ));
        let leaf_cells: Vec<(i32, i32)> = store.atoms[0]
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|&(dx, dy, _)| {
                (
                    store.atoms[0].gx + dx as i32,
                    store.atoms[0].gy + dy as i32,
                )
            })
            .collect();
        assert!(!leaf_cells.is_empty());
        store.atoms[0].age_ticks = super::PLANT_LIFE_TICKS;
        store.atoms[0].energy = 40.0;
        store.step(&mut w, 0);
        assert_eq!(store.corpse_count(), 1);
        assert!(
            store.corpses[0]
                .body
                .iter()
                .all(|(_, _, m)| *m != ModuleId::Photosystem),
            "leaves leave the grey corpse"
        );
        assert!(
            store.corpses[0]
                .body
                .iter()
                .any(|(_, _, m)| *m == ModuleId::Stem),
            "stems linger on the corpse until dissolve"
        );
        for &(lx, ly) in &leaf_cells {
            assert_eq!(
                w.get_cell(lx, ly).map(|c| c.material),
                Some(MaterialId::Organic),
                "leaf cell ({lx},{ly}) should be falling Organic litter"
            );
        }
    }

    #[test]
    fn dissolve_does_not_erect_organic_stem_pillars() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            minimal_plant_body(),
            40.0,
            Genome::default(),
        ));
        let stem_cells: Vec<(i32, i32)> = store.atoms[0]
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Stem)
            .map(|&(dx, dy, _)| {
                (
                    store.atoms[0].gx + dx as i32,
                    store.atoms[0].gy + dy as i32,
                )
            })
            .collect();
        assert!(!stem_cells.is_empty());
        store.atoms[0].age_ticks = super::PLANT_LIFE_TICKS;
        store.atoms[0].energy = 40.0;
        store.step(&mut w, 0);
        assert_eq!(store.corpse_count(), 1);
        store.corpses[0].settled_ticks = CORPSE_SETTLE_LAND_TICKS;
        store.step(&mut w, 1);
        assert_eq!(store.corpse_count(), 0);
        for &(sx, sy) in &stem_cells {
            assert_eq!(
                w.get_cell(sx, sy).map(|c| c.material),
                Some(MaterialId::Air),
                "dead trunk at ({sx},{sy}) must not become Organic (water/snow pass)"
            );
        }
        // Compost still lands on the bed via fallback / dead roots.
        let bed_organic = (0..6).any(|y| {
            matches!(
                w.get_cell(4, y).map(|c| c.material),
                Some(MaterialId::Organic)
            )
        });
        assert!(bed_organic, "dissolve should still leave bed Organic / root residue");
    }

    #[test]
    fn fungus_spawns_on_bare_stone() {
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
            for y in 2..8 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut store = OrganismStore::new();
        assert!(
            store.spawn_blueprint(
                &w,
                4,
                2,
                crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus(),
                40.0,
                Genome::default(),
            ),
            "fungus should place on Air above any solid"
        );
    }

    #[test]
    fn pop_cap_blocks_editor_spawn() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            for y in 0..8 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut store = OrganismStore::new();
        store.max_atoms = 1;
        let body = crate::blueprint::Blueprint::atom().modules_relative_to_nucleus();
        assert!(store
            .spawn_blueprint_free(&w, 2, 3, body.clone(), 40.0, Genome::default())
            .is_ok());
        assert_eq!(
            store.spawn_blueprint_free(&w, 4, 3, body, 40.0, Genome::default()),
            Err(SpawnFail::PopCap),
            "second spawn must fail at max_atoms=1"
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn growth_caps_block_root_elongation() {
        let mut w = moist_sand_plot();
        let mut atom = Atom::from_body(4, 2, 80.0, minimal_plant_body());
        atom.genome.alloc_root = 1.0;
        atom.genome.alloc_stem = 0.0;
        atom.genome.alloc_leaf = 0.0;
        atom.energy = 80.0;
        let roots = crate::plant::collect_live_root_world_cells(std::slice::from_ref(&atom));
        let tight = PlantGrowthCaps {
            max_roots: 1,
            max_stems: 10,
            max_photos: 12,
        };
        let spent = crate::plant::try_elongate_root(&mut w, &mut atom, &roots, &tight);
        assert_eq!(spent, 0.0, "max_roots=1 must refuse further elongation");
        assert_eq!(crate::plant::root_count(&atom), 1);
    }

    #[test]
    fn editor_free_spawn_places_atom_on_dry_air() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
            for y in 2..8 {
                w.set_cell(x, y, Cell::air()); // dry — habitat Atom spawn would fail
            }
        }
        let mut store = OrganismStore::new();
        let body = crate::blueprint::Blueprint::atom().modules_relative_to_nucleus();
        assert!(
            !store.spawn_blueprint(&w, 4, 3, body.clone(), 40.0, Genome::default()),
            "habitat spawn still requires wet Air"
        );
        assert!(
            store
                .spawn_blueprint_free(&w, 4, 3, body, 40.0, Genome::default())
                .is_ok(),
            "editor free spawn may place Atom on dry Air"
        );
        assert_eq!(store.len(), 1);
        assert_eq!(store.atoms[0].gx, 4);
        assert_eq!(store.atoms[0].gy, 3);
    }

    #[test]
    fn editor_free_spawn_many_plants_survives_steps() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        let body = minimal_plant_body();
        // Place five plants across the plot — entity pop must not stop at 2.
        for (i, x) in [2, 4, 6, 8, 10].into_iter().enumerate() {
            assert!(
                store
                    .spawn_blueprint_free(&w, x, 2, body.clone(), 40.0, Genome::default())
                    .is_ok(),
                "plant {} should free-spawn",
                i + 1
            );
        }
        assert_eq!(store.len(), 5);
        for t in 0..90 {
            store.step(&mut w, t);
        }
        assert_eq!(
            store.len(),
            5,
            "free-spawned plants must not vanish after a few ticks"
        );
    }

    #[test]
    fn editor_free_spawn_plant_snaps_down_from_canopy_click() {
        let mut w = moist_sand_plot();
        // Tall empty air column — click high like on a neighbour's leaves.
        for y in 2..20 {
            w.set_cell(5, y, Cell::air());
        }
        let mut store = OrganismStore::new();
        assert!(store
            .spawn_blueprint_free(&w, 5, 18, minimal_plant_body(), 40.0, Genome::default())
            .is_ok());
        assert_eq!(store.len(), 1);
        assert_eq!(
            store.atoms[0].gy, 2,
            "plant must seat on Air above sand, not hang at canopy y=18"
        );
        assert!(is_anchored(&w, &store.atoms[0]));
        store.step(&mut w, 1);
        assert_eq!(store.len(), 1, "seated plant must survive the next tick");
    }

    #[test]
    fn editor_free_spawn_allows_odd_module_mix() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        // Root + Digest is neither a valid plant nor fungus habit.
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (1, 0, ModuleId::Digest),
            (0, 1, ModuleId::Photosystem),
        ];
        assert!(
            store
                .spawn_blueprint_free(&w, 4, 2, body, 40.0, Genome::default())
                .is_ok(),
            "sandbox should accept any nucleus-bearing mix"
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn fungus_digests_soft_litter_over_time() {
        let mut w = moist_sand_plot();
        crate::fungi::add_soft_litter(&mut w, 4, 80);
        // Neighbour litter so a spore has somewhere to go later.
        crate::fungi::add_soft_litter(&mut w, 5, 40);
        crate::fungi::add_soft_litter(&mut w, 3, 40);
        let mut store = OrganismStore::new();
        let mut g = Genome::default();
        g.digest_rate = 1.2;
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus(),
            40.0,
            g,
        ));
        let litter0 = crate::fungi::soft_litter_at(&w, 4);
        store.atoms[0].energy = 20.0;
        for t in 0..120 {
            store.step(&mut w, t);
        }
        assert!(!store.is_empty());
        let litter1 = crate::fungi::soft_litter_at(&w, 4);
        assert!(
            litter1 < litter0,
            "fungus should digest soft litter ({litter1} < {litter0})"
        );
        assert!(store.atoms[0].energy > 0.0);
    }

    #[test]
    fn fungus_hibernates_without_litter_then_dies() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus(),
            40.0,
            Genome::default(),
        ));
        store.atoms[0].energy = 30.0;
        for t in 0..200 {
            store.step(&mut w, t);
        }
        assert!(!store.is_empty(), "short starve should hibernate");
        assert!(store.atoms[0].drought_ticks > 0);
        store.atoms[0].drought_ticks = crate::fungi::FUNGUS_HIBERNATE_MAX_TICKS - 3;
        for t in 200..220 {
            store.step(&mut w, t);
        }
        assert!(store.is_empty());
        assert_eq!(
            store.corpse_count(),
            1,
            "prolonged starve should leave a corpse after hibernate max"
        );
    }

    #[test]
    fn mycelium_field_keeps_spreading_after_fruiting_body_dies() {
        use crate::failure::FailureConfig;
        use crate::rules::{tick_with_life, PerfConfig};
        let mut w = moist_sand_plot();
        for x in 3..=5 {
            for y in 1..=3 {
                let mut org = Cell::solid(MaterialId::Organic);
                org.sat = Sat(180);
                w.set_cell(x, y, org);
            }
        }
        let mut store = OrganismStore::new();
        let g = Genome {
            digest_rate: 1.0,
            ..Genome::default()
        };
        assert!(store.spawn_blueprint(
            &w,
            4,
            3,
            crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus(),
            40.0,
            g,
        ));
        crate::fungi::seed_mycelium_near(&mut w, 4, 3, 48);
        let perf = PerfConfig::default();
        let fail = FailureConfig::default();
        for _ in 0..120 {
            tick_with_life(&mut w, &perf, &fail, None, None);
            let tick = w.tick;
            store.step(&mut w, tick);
        }
        // Kill the fruiting body; mycelium field must continue.
        store.atoms.clear();
        let myc0 = crate::fungi::max_mycelium_near(&w, 4, 2);
        assert!(myc0 >= 40, "need an established network before death");
        for _ in 0..200 {
            tick_with_life(&mut w, &perf, &fail, None, None);
        }
        let myc1 = crate::fungi::max_mycelium_near(&w, 4, 2);
        assert!(
            myc1 >= myc0,
            "mycelium field must persist after fruiting body dies (was {myc0}, now {myc1})"
        );
        assert!(
            myc1 > myc0 || (3..=5).any(|x| {
                (1..=3).any(|y| {
                    w.get_cell(x, y)
                        .map(|c| c.material == MaterialId::Organic && c.mycelium() > 0)
                        .unwrap_or(false)
                })
            }),
            "field should keep living on moist Organic without a fruiting body"
        );
    }

    #[test]
    fn fungus_mycelium_thickens_near_seat_under_physics() {
        use crate::failure::FailureConfig;
        use crate::rules::{tick_with_life, PerfConfig};
        let mut w = moist_sand_plot();
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(200);
            w.set_cell(4, y, org);
        }
        let mut store = OrganismStore::new();
        let g = Genome {
            digest_rate: 1.0,
            ..Genome::default()
        };
        let mut body =
            crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus();
        // This test is about local mycelium growth, not wind spores.
        body.retain(|(_, _, m)| *m != ModuleId::ReproSpore);
        assert!(store.spawn_blueprint(&w, 4, 3, body, 40.0, g));
        let (fx, fy) = (store.atoms[0].gx, store.atoms[0].gy);
        crate::fungi::seed_mycelium_near(&mut w, fx, fy, 32);
        let myc_near0 = (-1..=1)
            .flat_map(|dy| {
                w.get_cell(fx, fy + dy)
                    .map(|c| {
                        if c.material == MaterialId::Organic {
                            c.mycelium()
                        } else {
                            0
                        }
                    })
            })
            .max()
            .unwrap_or(0);
        let perf = PerfConfig::default();
        let fail = FailureConfig::default();
        for _ in 0..400 {
            tick_with_life(&mut w, &perf, &fail, None, None);
            let tick = w.tick;
            store.step(&mut w, tick);
        }
        assert_eq!(store.len(), 1);
        let (fx, fy) = (store.atoms[0].gx, store.atoms[0].gy);
        let myc_near1 = (-2..=2)
            .flat_map(|dy| {
                w.get_cell(fx, fy + dy)
                    .map(|c| {
                        if c.material == MaterialId::Organic {
                            c.mycelium()
                        } else {
                            0
                        }
                    })
            })
            .max()
            .unwrap_or(0);
        assert!(
            myc_near1 > myc_near0,
            "mycelium must thicken near the fungus (was {myc_near0}, now {myc_near1})"
        );
    }

    #[test]
    fn fungus_on_humid_organic_without_litter_grows_mycelium() {
        // Rain-soaked Organic, no soft litter: must stay active, hold energy,
        // and advance mycelium (the reported "dies in humid organic" case).
        let mut w = moist_sand_plot();
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(200);
            w.set_cell(4, y, org);
        }
        w.set_cell(4, 4, Cell::water()); // ponded rain above
        let mut store = OrganismStore::new();
        let g = Genome {
            digest_rate: 0.8,
            ..Genome::default()
        };
        assert!(store.spawn_blueprint(
            &w,
            4,
            3,
            crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus(),
            40.0,
            g,
        ));
        assert!(
            !crate::fungi::fungus_should_hibernate(&w, &store.atoms[0]),
            "humid Organic must not hibernate"
        );
        store.atoms[0].energy = 20.0;
        let e0 = store.atoms[0].energy;
        for t in 1..=320 {
            w.tick = t;
            store.step(&mut w, t);
        }
        assert_eq!(store.len(), 1, "must not starve on humid Organic");
        assert_eq!(store.atoms[0].drought_ticks, 0);
        assert!(
            store.atoms[0].energy + 1e-3 >= e0,
            "Organic forage should hold energy (was {e0}, now {})",
            store.atoms[0].energy
        );
        let threaded = (1..=3).any(|y| {
            w.get_cell(4, y)
                .map(|c| c.material == MaterialId::Organic && c.mycelium() > 0)
                .unwrap_or(false)
        });
        assert!(threaded, "mycelium must advance on humid Organic");
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
    fn water_column_light_fades_with_depth() {
        // Free surface at y=20; bed wet down to y=1.
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(4, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..=20 {
            let mut wet = Cell::air();
            wet.sat = Sat(255);
            w.set_cell(4, y, wet);
        }
        for y in 21..28 {
            w.set_cell(4, y, Cell::air());
        }
        let surface = column_sky_light(&w, 4, 19); // 1 wet cell above + surface film
        let mid = column_sky_light(&w, 4, 12);
        let deep = column_sky_light(&w, 4, 3);
        assert!(
            surface > mid && mid > deep,
            "deeper must be darker (surface={surface}, mid={mid}, deep={deep})"
        );
        assert!(
            deep < 0.15,
            "deep water must be near-dark for cost/benefit cliff (deep={deep})"
        );
        let land = column_sky_light(&w, 4, 22); // dry air, clear sky
        assert!(
            land > 0.95,
            "dry air above the lake must stay bright (land={land})"
        );
    }

    #[test]
    fn raising_canopy_toward_surface_recovers_light() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(5, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..=16 {
            let mut wet = Cell::air();
            wet.sat = Sat(255);
            w.set_cell(5, y, wet);
        }
        for y in 17..24 {
            w.set_cell(5, y, Cell::air());
        }
        let deep = column_sky_light(&w, 5, 4);
        let taller = column_sky_light(&w, 5, 14);
        assert!(
            taller > deep * 2.0,
            "stem-racing toward the surface must reclaim light ({taller} vs {deep})"
        );
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

    fn moist_sand_plot() -> World {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(120);
            w.set_cell(x, 1, sand);
            for y in 2..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w
    }

    fn minimal_plant_body() -> Vec<BodyModule> {
        crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus()
    }

    #[test]
    fn plant_spawns_on_moist_sand_and_stays_put() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            minimal_plant_body(),
            40.0,
            Genome::default(),
        ));
        assert!(is_land_plant(&store.atoms[0]));
        let gx = store.atoms[0].gx;
        let gy = store.atoms[0].gy;
        for t in 0..30 {
            store.step(&mut w, t);
        }
        assert_eq!(store.len(), 1);
        assert_eq!(store.atoms[0].gx, gx);
        assert_eq!(store.atoms[0].gy, gy, "plant crown must stay pinned");
    }

    #[test]
    fn plant_hibernates_on_bone_dry_bedrock_then_dies() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Bedrock));
            for y in 2..8 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut store = OrganismStore::new();
        // Force-place on bedrock (not plantable via spawn helper).
        let mut plant = Atom::from_body(4, 2, 40.0, minimal_plant_body());
        plant.energy = 30.0;
        store.atoms.push(plant);
        for t in 0..200 {
            store.step(&mut w, t);
        }
        assert!(!store.is_empty(), "short drought should hibernate, not kill");
        assert!(
            store.atoms[0].drought_ticks > 0,
            "should accumulate drought dormancy ticks"
        );
        assert!(store.atoms[0].energy > 0.0);
        // Prolonged drought ends the plant after hibernate max.
        store.atoms[0].drought_ticks = crate::plant::DROUGHT_HIBERNATE_MAX_TICKS - 3;
        for t in 200..220 {
            store.step(&mut w, t);
        }
        assert!(store.is_empty());
        assert_eq!(
            store.corpse_count(),
            1,
            "prolonged drought should leave a corpse after hibernate max"
        );
    }

    #[test]
    fn plant_drinks_pore_water_over_time() {
        let mut w = moist_sand_plot();
        let sat0 = w.get_cell(4, 1).unwrap().sat.0;
        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            minimal_plant_body(),
            40.0,
            Genome::default(),
        ));
        // Slow sip accumulator — need enough ticks to cross 1 sat unit.
        for t in 0..200 {
            store.step(&mut w, t);
        }
        let sat1 = w.get_cell(4, 1).unwrap().sat.0;
        assert!(sat1 < sat0, "roots should sip pore sat ({sat1} < {sat0})");
        assert!(!store.is_empty());
    }

    #[test]
    fn drink_does_not_touch_free_air_water() {
        let mut w = moist_sand_plot();
        // Standing water film above the crown — plants must not drink this.
        let mut wet = Cell::air();
        wet.sat = Sat::FULL;
        w.set_cell(4, 3, wet);
        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            minimal_plant_body(),
            40.0,
            Genome::default(),
        ));
        // Force a large sip budget so we'd notice if Air were drained.
        store.atoms[0].sip_acc = 5.0;
        let _ = crate::plant::drink_roots(&mut w, &mut store.atoms[0]);
        assert!(
            w.get_cell(4, 3).unwrap().sat.is_full(),
            "roots must never drink free Air water"
        );
    }

    fn deep_moist_sand() -> World {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                let mut sand = Cell::solid(MaterialId::Sand);
                // Wetter at depth so RootDepthBias dive pays.
                sand.sat = Sat(40 + y as u8 * 30);
                w.set_cell(x, y, sand);
            }
            for y in 6..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w
    }

    #[test]
    fn root_heavy_plant_elongates_downward() {
        let mut w = deep_moist_sand();
        let mut store = OrganismStore::new();
        let mut g = Genome::default();
        g.alloc_root = 1.0;
        g.alloc_stem = 0.05;
        g.alloc_leaf = 0.05;
        g.root_depth_bias = 0.95;
        assert!(store.spawn_blueprint(&w, 4, 6, minimal_plant_body(), 80.0, g));
        let roots0 = crate::plant::root_count(&store.atoms[0]);
        store.atoms[0].energy = 80.0;
        for t in 0..400 {
            store.step(&mut w, t);
            if store.is_empty() {
                break;
            }
            // Keep tank topped so growth isn't starved by night.
            if let Some(a) = store.atoms.first_mut() {
                a.energy = a.energy.max(50.0);
            }
        }
        assert!(!store.is_empty());
        let roots1 = crate::plant::root_count(&store.atoms[0]);
        assert!(
            roots1 > roots0,
            "root-heavy alloc should elongate (had {roots0}, now {roots1})"
        );
        let min_dy = store.atoms[0]
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Root)
            .map(|(_, dy, _)| *dy)
            .min()
            .unwrap();
        assert!(min_dy < -1, "new roots should dive below the crown root");
    }

    #[test]
    fn root_elongation_refuses_cell_beside_other_live_root() {
        let mut w = deep_moist_sand();
        let mut g = Genome::default();
        g.alloc_root = 1.0;
        g.alloc_stem = 0.05;
        g.alloc_leaf = 0.05;
        g.root_depth_bias = 0.95;
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, -2, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
        ];
        let mut atom = Atom::from_body(4, 6, 80.0, body);
        atom.genome = g;
        atom.energy = 80.0;
        let roots = crate::plant::collect_live_root_world_cells(std::slice::from_ref(&atom));
        let spent = crate::plant::try_elongate_root(&mut w, &mut atom, &roots, &crate::plant::PlantGrowthCaps::default());
        assert!(spent > 0.0, "vertical dive past the tip should still be legal");
        let added = atom
            .body
            .iter()
            .find(|&&(x, y, m)| {
                m == ModuleId::Root && !matches!((x, y), (0, -1) | (0, -2))
            })
            .map(|&(x, y, _)| (x, y))
            .expect("should add one root");
        // Forbidden blob fills: cells Moore-adjacent to *both* existing roots.
        assert!(
            !matches!(added, (1, -2) | (-1, -2) | (1, -1) | (-1, -1)),
            "must not pack beside the live stack, got {added:?}"
        );
        let live_neighbors = atom
            .body
            .iter()
            .filter(|&&(rx, ry, m)| {
                m == ModuleId::Root
                    && (rx, ry) != added
                    && (rx - added.0).abs() <= 1
                    && (ry - added.1).abs() <= 1
            })
            .count();
        assert_eq!(
            live_neighbors, 1,
            "new root may touch only its parent tip, got {live_neighbors} neighbors at {added:?}"
        );
    }

    #[test]
    fn root_elongation_refuses_cell_beside_other_plant_root() {
        let mut w = deep_moist_sand();
        // Seal every step except the one hugging the foreign root.
        w.set_cell(4, 3, Cell::solid(MaterialId::Bedrock)); // (0,-3)
        w.set_cell(3, 3, Cell::solid(MaterialId::Bedrock)); // (-1,-3)
        w.set_cell(3, 4, Cell::solid(MaterialId::Bedrock)); // (-1,-2)
        w.set_cell(5, 3, Cell::solid(MaterialId::Bedrock)); // (1,-3)
        w.set_cell(3, 5, Cell::solid(MaterialId::Bedrock)); // (-1,-1)
        w.set_cell(5, 5, Cell::solid(MaterialId::Bedrock)); // (1,-1)
        let mut g = Genome::default();
        g.alloc_root = 1.0;
        g.alloc_stem = 0.05;
        g.alloc_leaf = 0.05;
        g.root_depth_bias = 0.5;
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, -2, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
        ];
        let mut atom = Atom::from_body(4, 6, 80.0, body);
        atom.genome = g;
        atom.energy = 80.0;
        // Only open sand left beside tip is (1,-2)=(5,4); foreign root at (5,3).
        let mut roots = crate::plant::collect_live_root_world_cells(std::slice::from_ref(&atom));
        roots.insert((5, 3));
        let spent = crate::plant::try_elongate_root(&mut w, &mut atom, &roots, &crate::plant::PlantGrowthCaps::default());
        assert_eq!(spent, 0.0, "must not elongate beside another plant's live root");
        assert_eq!(crate::plant::root_count(&atom), 2);
    }

    #[test]
    fn root_growth_prefers_branch_over_deep_pipe() {
        let mut w = deep_moist_sand();
        let mut g = Genome::default();
        g.alloc_root = 1.0;
        g.alloc_stem = 0.05;
        g.alloc_leaf = 0.05;
        g.root_depth_bias = 0.45; // balanced — should fork, not only drill
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, -2, ModuleId::Root),
            (0, -3, ModuleId::Root),
            (0, -4, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
        ];
        let mut atom = Atom::from_body(4, 6, 80.0, body);
        atom.genome = g;
        atom.energy = 80.0;
        let roots = crate::plant::collect_live_root_world_cells(std::slice::from_ref(&atom));
        let spent = crate::plant::try_elongate_root(&mut w, &mut atom, &roots, &crate::plant::PlantGrowthCaps::default());
        assert!(spent > 0.0, "pipe tip should still be able to fork");
        let added = atom
            .body
            .iter()
            .find(|&&(x, y, m)| {
                m == ModuleId::Root && !matches!((x, y), (0, -1) | (0, -2) | (0, -3) | (0, -4))
            })
            .map(|&(x, y, _)| (x, y))
            .expect("should add a root");
        assert_ne!(
            added.0, 0,
            "after a deep pipe, next step should branch sideways, got {added:?}"
        );
    }

    #[test]
    fn root_can_bud_sideways_from_mid_pipe() {
        let mut w = deep_moist_sand();
        // Block further vertical under the tip so growth must bud sideways.
        w.set_cell(4, 2, Cell::solid(MaterialId::Bedrock)); // (0,-4)
        w.set_cell(3, 2, Cell::solid(MaterialId::Bedrock)); // (-1,-4)
        w.set_cell(5, 2, Cell::solid(MaterialId::Bedrock)); // (1,-4)
        let mut g = Genome::default();
        g.alloc_root = 1.0;
        g.alloc_stem = 0.05;
        g.alloc_leaf = 0.05;
        g.root_depth_bias = 0.2;
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, -2, ModuleId::Root),
            (0, -3, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
        ];
        let mut atom = Atom::from_body(4, 6, 80.0, body);
        atom.genome = g;
        atom.energy = 80.0;
        let roots = crate::plant::collect_live_root_world_cells(std::slice::from_ref(&atom));
        let spent = crate::plant::try_elongate_root(&mut w, &mut atom, &roots, &crate::plant::PlantGrowthCaps::default());
        assert!(spent > 0.0, "mid-pipe site should be able to bud");
        let added = atom
            .body
            .iter()
            .find(|&&(x, y, m)| {
                m == ModuleId::Root && !matches!((x, y), (0, -1) | (0, -2) | (0, -3))
            })
            .map(|&(x, y, _)| (x, y))
            .expect("should add a lateral bud");
        assert_ne!(added.0, 0, "bud should leave the pipe column, got {added:?}");
    }

    #[test]
    fn root_transport_prefers_short_branch_over_long_tendril() {
        // Vertical pipe + long diagonal tendril. Transport tax should make
        // a mid-pipe bud beat extending the tendril tip another step.
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=10 {
                let mut sand = Cell::solid(MaterialId::Sand);
                sand.sat = Sat(120); // uniform — no deep-moisture lure
                w.set_cell(x, y, sand);
            }
            for y in 11..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut g = Genome::default();
        g.alloc_root = 1.0;
        g.alloc_stem = 0.05;
        g.alloc_leaf = 0.05;
        g.root_depth_bias = 0.55;
        // Two photosystems → soft root budget 9, room for one more grow.
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, -2, ModuleId::Root),
            (0, -3, ModuleId::Root),
            (1, -4, ModuleId::Root),
            (2, -5, ModuleId::Root),
            (3, -6, ModuleId::Root),
            (4, -7, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
            (1, 2, ModuleId::Photosystem),
        ];
        // Nucleus at y=12 so roots sit in sand (y=11..5).
        let mut atom = Atom::from_body(4, 12, 80.0, body);
        atom.genome = g;
        atom.energy = 80.0;
        let tip_hops = crate::plant::root_transport_hops(&atom, 5, -8);
        let bud_hops = crate::plant::root_transport_hops(&atom, -1, -2);
        assert!(
            tip_hops > bud_hops + 2,
            "tendril tip should be much farther from crown ({tip_hops} vs {bud_hops})"
        );
        let roots = crate::plant::collect_live_root_world_cells(std::slice::from_ref(&atom));
        let spent = crate::plant::try_elongate_root(&mut w, &mut atom, &roots, &crate::plant::PlantGrowthCaps::default());
        assert!(spent > 0.0, "should still grow somewhere");
        let prior = [
            (0, -1),
            (0, -2),
            (0, -3),
            (1, -4),
            (2, -5),
            (3, -6),
            (4, -7),
        ];
        let added = atom
            .body
            .iter()
            .find(|&&(x, y, m)| m == ModuleId::Root && !prior.contains(&(x, y)))
            .map(|&(x, y, _)| (x, y))
            .expect("should add a root");
        let added_hops = crate::plant::root_transport_hops(&atom, added.0, added.1);
        assert!(
            added_hops <= 4,
            "transport cost should pick a short branch near the crown, got {added:?} hops={added_hops}"
        );
        assert_ne!(
            added,
            (5, -8),
            "must not extend the long diagonal tendril tip"
        );
    }

    #[test]
    fn root_dir_preference_does_not_always_pick_diagonal() {
        // Short plant, uniform moisture: high depth bias should dive
        // cardinal down — not get forced onto a diagonal stair by the
        // old double-counted dir score. Keep energy below sprout-banking
        // so rhizome urge doesn't yank sideways.
        let mut w = deep_moist_sand();
        for x in 0..12 {
            for y in 1..=5 {
                let mut sand = Cell::solid(MaterialId::Sand);
                sand.sat = Sat(100);
                w.set_cell(x, y, sand);
            }
        }
        let mut g = Genome::default();
        g.alloc_root = 1.0;
        g.alloc_stem = 0.05;
        g.alloc_leaf = 0.05;
        g.root_depth_bias = 0.85;
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
        ];
        let mut atom = Atom::from_body(4, 6, 80.0, body);
        atom.genome = g;
        // Above grow floor (0.30·tank) but below sprout-banking (~0.44·tank).
        atom.energy = 32.0;
        let roots = crate::plant::collect_live_root_world_cells(std::slice::from_ref(&atom));
        let spent = crate::plant::try_elongate_root(&mut w, &mut atom, &roots, &crate::plant::PlantGrowthCaps::default());
        assert!(spent > 0.0);
        let added = atom
            .body
            .iter()
            .find(|&&(x, y, m)| m == ModuleId::Root && (x, y) != (0, -1))
            .map(|&(x, y, _)| (x, y))
            .expect("added root");
        assert_eq!(
            added,
            (0, -2),
            "high depth bias should dive cardinal, not diagonal; got {added:?}"
        );
    }

    #[test]
    fn root_elongation_allows_cell_beside_organic_compost() {
        let mut w = deep_moist_sand();
        // Seal the dive so the only open step is lateral Sand next to
        // Organic compost (transformed dead root).
        w.set_cell(4, 4, Cell::solid(MaterialId::Bedrock));
        w.set_cell(3, 5, Cell::solid(MaterialId::Bedrock));
        w.set_cell(6, 5, Cell::solid(MaterialId::Organic));
        let mut g = Genome::default();
        g.alloc_root = 1.0;
        g.alloc_stem = 0.05;
        g.alloc_leaf = 0.05;
        g.root_depth_bias = 0.1; // prefer the lateral runner
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
        ];
        let mut atom = Atom::from_body(4, 6, 80.0, body);
        atom.genome = g;
        atom.energy = 80.0;
        let roots = crate::plant::collect_live_root_world_cells(std::slice::from_ref(&atom));
        let spent = crate::plant::try_elongate_root(&mut w, &mut atom, &roots, &crate::plant::PlantGrowthCaps::default());
        assert!(
            spent > 0.0,
            "Organic compost is transformed dead root — OK to sit beside"
        );
        assert!(
            atom.body
                .iter()
                .any(|&(x, y, m)| m == ModuleId::Root && (x, y) == (1, -1)),
            "should step into Sand beside Organic compost"
        );
    }

    #[test]
    fn stem_heavy_plant_grows_upward() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        let mut g = Genome::default();
        g.alloc_stem = 1.0;
        g.alloc_leaf = 0.05;
        g.alloc_root = 0.05;
        assert!(store.spawn_blueprint(&w, 4, 2, minimal_plant_body(), 80.0, g));
        let stem0 = crate::plant::stem_count(&store.atoms[0]);
        store.atoms[0].energy = 80.0;
        for t in 0..400 {
            store.step(&mut w, t);
            if let Some(a) = store.atoms.first_mut() {
                a.energy = a.energy.max(50.0);
            }
        }
        assert!(!store.is_empty());
        let stem1 = crate::plant::stem_count(&store.atoms[0]);
        assert!(
            stem1 > stem0,
            "stem-heavy alloc should stack olive (had {stem0}, now {stem1})"
        );
    }

    #[test]
    fn stem_growth_keeps_gap_from_neighbour_trunk() {
        let mut atom = Atom::from_body(
            4,
            2,
            80.0,
            vec![
                (0, -1, ModuleId::Root),
                (0, 0, ModuleId::Nucleus),
                (0, 1, ModuleId::Stem),
            ],
        );
        atom.genome.alloc_stem = 1.0;
        atom.genome.alloc_leaf = 0.05;
        atom.genome.alloc_root = 0.05;
        atom.energy = 80.0;
        // Foreign live trunk one cell beside the tip column.
        // Tip is at (0,1) → candidate (0,2) world (4,4). Neighbour (5,4) is Moore.
        let mut trunks = std::collections::HashSet::new();
        trunks.insert((5, 4));
        assert!(
            !crate::plant::stem_spacing_ok(&atom, 0, 2, &trunks),
            "must not elongate into a cell beside another trunk"
        );
        let empty = std::collections::HashSet::new();
        assert!(
            crate::plant::stem_spacing_ok(&atom, 0, 2, &empty),
            "solo trunk may elongate upward"
        );
        // Growth may still place a leaf, but must not add Stem beside the gap.
        atom.age_ticks = LAND_GROW_PERIOD;
        let mut w = moist_sand_plot();
        let _ = crate::plant::try_grow_shoot(
            &w,
            &mut atom,
            1,
            &trunks,
            &std::collections::HashSet::new(),
            &crate::plant::PlantGrowthCaps::default(),
            &CanopyIndex::default(),
            0,
        );
        assert_eq!(crate::plant::stem_count(&atom), 1);
    }

    #[test]
    fn leaves_may_touch_each_other() {
        let empty = std::collections::HashSet::new();
        let mut w = moist_sand_plot();
        let mut atom = Atom::from_body(
            4,
            2,
            80.0,
            vec![
                (0, -1, ModuleId::Root),
                (0, 0, ModuleId::Nucleus),
                (0, 1, ModuleId::Stem),
                (1, 1, ModuleId::Photosystem),
            ],
        );
        atom.genome.alloc_stem = 0.05;
        atom.genome.alloc_leaf = 1.0;
        atom.genome.alloc_root = 0.05;
        atom.energy = 80.0;
        let n0 = atom.photosystem_count();
        for t in 0..40u64 {
            atom.age_ticks = t * LAND_GROW_PERIOD;
            let _ = crate::plant::try_grow_shoot(
                &w,
                &mut atom,
                t,
                &empty,
                &std::collections::HashSet::new(),
                &crate::plant::PlantGrowthCaps::default(),
                &CanopyIndex::default(),
                0,
            );
        }
        assert!(
            atom.photosystem_count() > n0,
            "leaf-heavy shoot should add leaves even when they touch"
        );
    }

    fn root_nucleus_leaf_body() -> Vec<BodyModule> {
        vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Photosystem),
        ]
    }

    #[test]
    fn stemless_chassis_does_not_invent_trunk() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        let mut g = Genome::default();
        // Even max alloc_stem cannot invent olive — must paint Stem.
        g.alloc_stem = 1.0;
        g.alloc_leaf = 0.5;
        g.alloc_root = 0.05;
        assert!(store.spawn_blueprint(&w, 4, 2, root_nucleus_leaf_body(), 80.0, g));
        store.atoms[0].energy = 80.0;
        for t in 0..400 {
            store.step(&mut w, t);
            if let Some(a) = store.atoms.first_mut() {
                a.energy = a.energy.max(50.0);
            }
        }
        assert!(!store.is_empty());
        assert_eq!(
            crate::plant::stem_count(&store.atoms[0]),
            0,
            "Root+Nucleus+Leaf chassis must not grow olive stems"
        );
    }

    #[test]
    fn stemless_parent_sprouts_stemless_child() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        let mut g = Genome::default();
        g.clone_fidelity = 0.5;
        g.alloc_root = 0.8;
        g.alloc_stem = 1.0; // would invent under the old threshold
        g.alloc_leaf = 0.2;
        assert!(store.spawn_blueprint(&w, 4, 2, root_nucleus_leaf_body(), 60.0, g));
        let a = &mut store.atoms[0];
        a.body.push((-1, -1, ModuleId::Root));
        a.body.push((-2, -1, ModuleId::Root));
        a.body.push((1, -1, ModuleId::Root));
        a.body.push((2, -1, ModuleId::Root));
        a.energy = 60.0;
        a.cooldown = 0;
        let n0 = store.len();
        for t in 0..200u64 {
            store.step(&mut w, t);
            if store.len() > n0 {
                break;
            }
            if let Some(p) = store.atoms.first_mut() {
                p.energy = p.energy.max(55.0);
                p.cooldown = 0;
            }
        }
        assert!(store.len() > n0, "stemless parent should still rhizome-sprout");
        for child in store.atoms.iter().skip(1) {
            assert_eq!(
                crate::plant::stem_count(child),
                0,
                "stemless habit must pass to vegetative sprouts"
            );
        }
    }

    #[test]
    fn stem_does_not_stack_on_photosystem() {
        // Proper chassis: root / nucleus / stem, leaf to the side.
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (1, 1, ModuleId::Photosystem),
        ];
        let mut atom = Atom::from_body(4, 2, 80.0, body);
        atom.genome.alloc_stem = 1.0;
        atom.genome.alloc_leaf = 0.5;
        atom.genome.alloc_root = 0.05;
        atom.energy = 80.0;
        let empty = std::collections::HashSet::new();
        let mut w = moist_sand_plot();
        for t in 0..30u64 {
            atom.age_ticks = t * LAND_GROW_PERIOD;
            let _ = crate::plant::try_grow_shoot(
                &w,
                &mut atom,
                t,
                &empty,
                &std::collections::HashSet::new(),
                &crate::plant::PlantGrowthCaps::default(),
                &CanopyIndex::default(),
                0,
            );
        }
        let stem_on_leaf = atom.body.iter().any(|&(x, y, m)| {
            m == ModuleId::Stem
                && atom.body.iter().any(|&(lx, ly, lm)| {
                    lm == ModuleId::Photosystem && lx == x && ly == y - 1
                })
        });
        assert!(
            !stem_on_leaf,
            "stem must not grow directly on top of a leaf"
        );
        assert!(
            crate::plant::stem_count(&atom) > 1,
            "stem-heavy shoot should still elongate the trunk"
        );
    }

    #[test]
    fn plant_with_lateral_runner_can_sprout_child() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        let mut g = Genome::default();
        g.clone_fidelity = 0.5;
        g.alloc_root = 0.8;
        g.alloc_stem = 0.1;
        g.alloc_leaf = 0.1;
        assert!(store.spawn_blueprint(&w, 4, 2, minimal_plant_body(), 60.0, g));
        // Paint a lateral rhizome + extra roots so sprout gate opens.
        let a = &mut store.atoms[0];
        a.body.push((-1, -1, ModuleId::Root));
        a.body.push((-2, -1, ModuleId::Root));
        a.body.push((1, -1, ModuleId::Root));
        a.body.push((2, -1, ModuleId::Root));
        a.energy = 60.0;
        a.cooldown = 0;
        let n0 = store.len();
        for t in 0..200u64 {
            store.step(&mut w, t);
            if store.len() > n0 {
                break;
            }
            if let Some(p) = store.atoms.first_mut() {
                p.energy = p.energy.max(55.0);
                p.cooldown = 0;
            }
        }
        assert!(
            store.len() > n0,
            "lateral runner + energy should fire a vegetative sprout"
        );
        assert!(store.atoms.iter().all(is_land_plant));
        // Child should sit on a different column.
        let cols: std::collections::HashSet<i32> =
            store.atoms.iter().map(|a| a.gx).collect();
        assert!(cols.len() >= 2, "sprout should emerge on a neighbour column");
    }

    #[test]
    fn stacked_crowns_are_reseated_to_distinct_columns() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            minimal_plant_body(),
            40.0,
            Genome::default(),
        ));
        let body = store.atoms[0].body.clone();
        let gx = store.atoms[0].gx;
        let gy = store.atoms[0].gy;
        store.atoms[0].age_ticks = 500;
        // Two younger clones stacked on the same crown cell.
        for _ in 0..2 {
            let mut twin = Atom::from_body(gx, gy, 40.0, body.clone());
            twin.age_ticks = 10;
            apply_genome(&mut twin, Genome::default());
            pin_plant_pose(&mut twin);
            store.atoms.push(twin);
        }
        assert_eq!(
            store.atoms.iter().filter(|a| a.gx == gx && a.gy == gy).count(),
            3
        );
        store.step(&mut w, 0);
        let cols: std::collections::HashSet<i32> =
            store.atoms.iter().map(|a| a.gx).collect();
        assert_eq!(
            cols.len(),
            store.len(),
            "living land crowns must not share a column after reseat"
        );
    }

    #[test]
    fn vegetative_sprout_skips_occupied_columns() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        let mut g = Genome::default();
        g.clone_fidelity = 0.5;
        g.alloc_root = 0.8;
        assert!(store.spawn_blueprint(&w, 4, 2, minimal_plant_body(), 60.0, g));
        // Neighbour columns already claimed.
        for &(x, age) in &[(3, 200u64), (5, 200)] {
            let body = store.atoms[0].body.clone();
            let mut other = Atom::from_body(x, 2, 40.0, body);
            other.age_ticks = age;
            apply_genome(&mut other, Genome::default());
            pin_plant_pose(&mut other);
            store.atoms.push(other);
        }
        let a = &mut store.atoms[0];
        a.body.push((-1, -1, ModuleId::Root));
        a.body.push((-2, -1, ModuleId::Root));
        a.body.push((1, -1, ModuleId::Root));
        a.body.push((2, -1, ModuleId::Root));
        a.energy = 60.0;
        a.cooldown = 0;
        let n0 = store.len();
        let cols0: std::collections::HashSet<i32> =
            store.atoms.iter().map(|a| a.gx).collect();
        for t in 0..80u64 {
            store.step(&mut w, t);
            if let Some(p) = store.atoms.first_mut() {
                p.energy = p.energy_max;
                p.cooldown = 0;
            }
        }
        // May or may not sprout farther out, but must never stack on 3/4/5.
        for a in &store.atoms {
            let same = store
                .atoms
                .iter()
                .filter(|b| b.gx == a.gx)
                .count();
            assert_eq!(same, 1, "column {} has {same} crowns", a.gx);
        }
        let _ = (n0, cols0);
    }

    #[test]
    fn single_plant_does_not_rhizome_flood_pop_cap() {
        // Regression: one F2 plant template used to fill max_atoms with
        // short-lived root sprouts (brown underground pepper, HUD at cap).
        let mut w = moist_sand_plot();
        for x in 0..12 {
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(160);
            w.set_cell(x, 1, sand);
        }
        let mut store = OrganismStore::new();
        store.max_atoms = 64;
        let mut g = Genome::default();
        g.alloc_root = 0.9;
        g.alloc_stem = 0.05;
        g.alloc_leaf = 0.05;
        assert!(store.spawn_blueprint(&w, 4, 2, minimal_plant_body(), 80.0, g));
        // Natural cooldowns — do not cheat period to zero.
        // ~3–4 max sprout windows in 4k ticks at period 720; must stay
        // far below a pop-cap carpet even with energy cheated full.
        for t in 0..4_000u64 {
            for p in &mut store.atoms {
                p.energy = p.energy_max;
            }
            store.step(&mut w, t);
        }
        let n = store.len();
        assert!(
            n < 16,
            "one founder must not carpet the plot (creatures={n})"
        );
        assert!(n >= 1);
    }

    #[test]
    fn rooted_plant_gains_energy_capacity_from_roots() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            minimal_plant_body(),
            50.0,
            Genome::default(),
        ));
        let a = &mut store.atoms[0];
        assert!((a.energy_base_max - 50.0).abs() < 1e-3);
        let cap0 = a.energy_max;
        // Paint extra roots → starch tank grows on next sync / step.
        a.body.push((0, -2, ModuleId::Root));
        a.body.push((0, -3, ModuleId::Root));
        a.body.push((1, -1, ModuleId::Root));
        a.body.push((-1, -1, ModuleId::Root));
        crate::plant::sync_root_storage(a);
        let cap1 = a.energy_max;
        assert!(
            cap1 > cap0,
            "more roots should raise energy_max (was {cap0}, now {cap1})"
        );
        assert!(
            (cap1 - crate::plant::energy_capacity(50.0, crate::plant::root_count(a))).abs() < 1e-3
        );
        // Growth floor still keys off spawn tank, not inflated max.
        assert!(
            (crate::plant::tank_ref(a) - 50.0).abs() < 1e-3,
            "tank_ref must stay on energy_base_max"
        );
    }

    #[test]
    fn hydrated_plant_stops_rooting_past_soft_budget() {
        let mut w = moist_sand_plot();
        let mut store = OrganismStore::new();
        let mut g = Genome::default();
        g.alloc_root = 1.0;
        g.alloc_stem = 0.05;
        g.alloc_leaf = 0.05;
        assert!(store.spawn_blueprint(&w, 4, 2, minimal_plant_body(), 80.0, g));
        // Minimal plant: 1 photo → budget = 3 + 3 = 6 roots.
        let a = &mut store.atoms[0];
        let budget = crate::plant::useful_root_budget(a, &crate::plant::PlantGrowthCaps::default());
        while crate::plant::root_count(a) < budget {
            a.body.push((crate::plant::root_count(a) as i16, -1, ModuleId::Root));
        }
        a.energy = 80.0;
        let roots0 = crate::plant::root_count(a);
        assert!(crate::plant::roots_past_soft_budget_for(
            a,
            DroughtBand::Hydrated,
            &crate::plant::PlantGrowthCaps::default(),
            false,
        ));
        for t in 0..200 {
            store.step(&mut w, t);
            if let Some(p) = store.atoms.first_mut() {
                p.energy = p.energy.max(70.0);
            }
        }
        assert!(!store.is_empty());
        let roots1 = crate::plant::root_count(&store.atoms[0]);
        let budget_now = crate::plant::useful_root_budget_for(
            &store.atoms[0],
            DroughtBand::Hydrated,
            &crate::plant::PlantGrowthCaps::default(),
            false,
        );
        assert!(
            roots1 <= budget_now.max(roots0) + 1,
            "hydrated plant should stay near soft budget (had {roots0}, now {roots1}, budget={budget_now})"
        );
    }

    #[test]
    fn drought_stress_lifts_soft_root_budget() {
        let mut store = OrganismStore::new();
        let mut w = moist_sand_plot();
        assert!(store.spawn_blueprint(
            &w,
            4,
            2,
            minimal_plant_body(),
            40.0,
            Genome::default(),
        ));
        let a = &store.atoms[0];
        let hydrated = crate::plant::useful_root_budget_for(
            a,
            DroughtBand::Hydrated,
            &crate::plant::PlantGrowthCaps::default(),
            false,
        );
        let stressed = crate::plant::useful_root_budget_for(
            a,
            DroughtBand::Stressed,
            &crate::plant::PlantGrowthCaps::default(),
            false,
        );
        assert!(
            stressed > hydrated,
            "stress should allow deeper boring (hydrated={hydrated} stressed={stressed})"
        );
    }

    #[test]
    fn tall_neighbour_shades_short_plant_energy() {
        let mut w = moist_sand_plot();
        // Short alone vs short next to a tall canopy thug.
        let mut short_g = Genome::default();
        short_g.leaf_absorb = 0.25;
        short_g.shade_efficiency = 0.2;
        short_g.alloc_stem = 0.1;
        short_g.alloc_leaf = 0.1;
        short_g.alloc_root = 0.8;

        let mut tall_g = Genome::default();
        tall_g.leaf_absorb = 0.95;
        tall_g.shade_efficiency = 0.05;
        tall_g.alloc_stem = 0.1;
        tall_g.alloc_leaf = 0.1;
        tall_g.alloc_root = 0.8;

        let short_body = minimal_plant_body();
        let mut tall_body = minimal_plant_body();
        // Extra stem + leaf so canopy clears the short neighbour.
        tall_body.push((0, 3, ModuleId::Stem));
        tall_body.push((0, 4, ModuleId::Stem));
        tall_body.push((0, 5, ModuleId::Stem));
        tall_body.push((0, 6, ModuleId::Photosystem));

        let mut alone = OrganismStore::new();
        assert!(alone.spawn_blueprint(&w, 3, 2, short_body.clone(), 40.0, short_g));

        let mut shaded = OrganismStore::new();
        assert!(shaded.spawn_blueprint(&w, 3, 2, short_body, 40.0, short_g));
        assert!(shaded.spawn_blueprint(&w, 4, 2, tall_body, 40.0, tall_g));

        // Reset energy each tick so we measure harvest rate, not the tank cap.
        let mut gain_alone = 0.0f32;
        let mut gain_shaded = 0.0f32;
        for t in 0..60u64 {
            alone.atoms[0].energy = 10.0;
            shaded.atoms[0].energy = 10.0;
            if let Some(a) = shaded.atoms.get_mut(1) {
                a.energy = 10.0;
            }
            alone.step(&mut w, t);
            shaded.step(&mut w, t);
            gain_alone += alone.atoms[0].energy - 10.0;
            gain_shaded += shaded.atoms[0].energy - 10.0;
        }
        assert!(!alone.is_empty() && !shaded.is_empty());
        assert!(
            gain_shaded < gain_alone * 0.95,
            "shaded short plant should harvest less (shaded={gain_shaded}, alone={gain_alone})"
        );
    }

    #[test]
    fn woody_understory_leaf_drops_after_sustained_shade() {
        let mut w = moist_sand_plot();
        let mut short_g = Genome::default();
        short_g.leaf_absorb = 0.35;
        short_g.shade_efficiency = 0.15;
        short_g.alloc_stem = 0.05;
        short_g.alloc_leaf = 0.05;
        short_g.alloc_root = 0.9;
        let mut tall_g = Genome::default();
        tall_g.leaf_absorb = 0.95;
        tall_g.shade_efficiency = 0.05;
        tall_g.alloc_stem = 0.05;
        tall_g.alloc_leaf = 0.05;
        tall_g.alloc_root = 0.9;

        let short_body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
            (0, 3, ModuleId::Photosystem),
            (0, 4, ModuleId::Photosystem),
        ];
        let mut tall_body = minimal_plant_body();
        for s in 3..=8 {
            tall_body.push((0, s, ModuleId::Stem));
        }
        tall_body.push((0, 9, ModuleId::Photosystem));

        let mut store = OrganismStore::new();
        assert!(store.spawn_blueprint(&w, 3, 2, short_body, 40.0, short_g));
        assert!(store.spawn_blueprint(&w, 4, 2, tall_body, 40.0, tall_g));
        let n0 = store.atoms[0].photosystem_count();
        assert!(n0 >= 3);
        for t in 0..(crate::plant::WOODY_LEAF_STARVE_TICKS as u64 + 120) {
            store.atoms[0].energy = 30.0;
            if let Some(a) = store.atoms.get_mut(1) {
                a.energy = 30.0;
            }
            // Lock shoot growth so the short plant can't replace dropped leaves.
            store.atoms[0].genome.alloc_leaf = 0.0;
            store.atoms[0].genome.alloc_stem = 0.0;
            store.step(&mut w, t);
            // Keep age off LAND_GROW_PERIOD (step increments age_ticks).
            if let Some(a) = store.atoms.get_mut(0) {
                a.age_ticks = 1;
                a.genome.alloc_leaf = 0.0;
                a.genome.alloc_stem = 0.0;
            }
        }
        let n1 = store.atoms[0].photosystem_count();
        assert!(
            n1 < n0,
            "chronically shaded woody leaves should abscise (had {n0}, now {n1})"
        );
        assert!(n1 >= 1, "keep at least one Photosystem");
    }
}
