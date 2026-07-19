//! Set A organism behaviour (nucleus + photosystem).
//! See `docs/organism/CORE_FEATURES.md` Set A.
//!
//! Plankton move under **weight vs buoyancy** inside each column's live
//! [`Column::flowable_water`] band (not a constant `sea_level` line).
//! [`Genome::buoyancy_bias`] `0` ≈ floater (~1 m under the free surface);
//! `1` ≈ sinker (water bed). Rising water lifts bodies in the column.
//!
//! Overlap uses **blueprint footprint AABBs** (creature size), not
//! one-per-column locks — small bodies may share a column; large ones
//! push apart across whatever space they need.

use hecs::Entity;
use serde::{Deserialize, Serialize};
use wk_material::MaterialId;
use wk_world::column::{Activity, Column};
use wk_world::terrain::hash_u64;
use wk_world::world::World;

use crate::blueprint::Blueprint;
use crate::{
    AgentStore, Energy, Genome, Pose, MAX_AGENTS, MUTATION_SIGMA, REPRO_COST_FRAC, REPRO_PERIOD,
};

/// Soft cap for module organisms (separate from grazers).
/// Absolute ECS ceiling — per-habit [`PopCaps`] must stay at or below this.
pub const MAX_ORGANISMS: usize = 512;

/// Habit bucket for population budgets (algae / rooted / hypha).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganismHabit {
    /// Nucleus + photosystem, no root/stem/digest (plankton Atom).
    Algae,
    /// Land plant — has Root or Stem.
    Plant,
    /// Litter fungus — Digest / Hypha chassis.
    Fungus,
}

impl OrganismHabit {
    pub fn from_blueprint(bp: &Blueprint) -> Self {
        if bp.is_fungus() {
            Self::Fungus
        } else if bp.is_rooted() {
            Self::Plant
        } else {
            Self::Algae
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Algae => "algae",
            Self::Plant => "plant",
            Self::Fungus => "fungus",
        }
    }
}

/// Tunable per-habit population ceilings (settings UI / scenarios).
///
/// Defaults split [`MAX_ORGANISMS`] so algae blooms, forests, and litter
/// fungi don't starve each other of the same global slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopCaps {
    pub algae: usize,
    pub plant: usize,
    pub fungus: usize,
}

impl Default for PopCaps {
    fn default() -> Self {
        Self {
            algae: 256,
            plant: 192,
            fungus: 64,
        }
    }
}

impl PopCaps {
    pub fn for_habit(self, habit: OrganismHabit) -> usize {
        match habit {
            OrganismHabit::Algae => self.algae,
            OrganismHabit::Plant => self.plant,
            OrganismHabit::Fungus => self.fungus,
        }
        .min(MAX_ORGANISMS)
    }

    pub fn clamp_each(self) -> Self {
        Self {
            algae: self.algae.min(MAX_ORGANISMS),
            plant: self.plant.min(MAX_ORGANISMS),
            fungus: self.fungus.min(MAX_ORGANISMS),
        }
    }
}

/// Blueprint pixel → world units (fraction of a column). Creature size scales
/// with painted modules — collision AABB uses this (kept ≥ draw cell size).
pub const MODULE_CELL_COLS: f32 = 0.45;

/// Extra inflate on collision boxes so drawn pixels don't visually merge.
const COLLISION_PAD: f32 = 0.06;

/// Minimum horizontal separation push when two bodies fully coincide.
const MIN_SEPARATION: f32 = 0.08;

/// Photosynthesis floor (energy/tick/photosystem at noon). Keeps short-day
/// scenarios (E30) fertile without one-tick fill-ups.
pub const PHOTON_RATE: f32 = 0.12;

/// Full energy tank lasts this many dark ticks at `metabolic_rate = 1.0`
/// (default night is 36 000 ticks — algae should outlast a normal night).
pub const DARK_ENDURANCE_TICKS: f32 = 42_000.0;

/// Night / low-light upkeep multiplier (dormant respiration).
const NIGHT_UPKEEP_MULT: f32 = 0.4;

/// kg of ecology litter (`dead_biomass`) per module when a corpse dissolves.
/// Small — nutrients recycle; the bulk of the body becomes Organic sediment.
pub const DEATH_LITTER_KG_PER_MODULE: i64 = 4;

/// kg of [`MaterialId::Organic`] sediment deposited per module on dissolve.
/// Kept small — living modules don't occupy stratigraphic volume, so large
/// per-module ooze "inflated" columns into beaver-dam spikes on death.
pub const DEATH_ORGANIC_KG_PER_MODULE: i64 = 10;
/// Soft cap on Organic kg from a single corpse dissolve (~0.35 m pile).
pub const DEATH_ORGANIC_KG_MAX: i64 = 80;

/// Ticks a corpse rests on the bed before becoming sediment (~6–7 min at 60 Hz).
/// Long enough that death blooms leave a visible carpet of bodies first.
pub const CORPSE_SETTLE_TICKS: u32 = 24_000;
/// Land-plant corpses rot faster (~80 s at 60 Hz). A slow carpet of trunks
/// packed every column and starved light/slots for the next cohort.
pub const CORPSE_SETTLE_LAND_TICKS: u32 = 4_800;

/// Floater depth below the live free-water surface (metres) when bias ≈ 0.
pub const FLOAT_DEPTH_M: f32 = 1.0;

/// Downward acceleration (world-Y decreases) when unsupported / denser than water.
const GRAVITY: f32 = 0.12;
/// Velocity damping while submerged.
const WATER_DRAG: f32 = 0.22;
/// Velocity damping in air.
const AIR_DRAG: f32 = 0.03;
/// Mild spring toward the gene equilibrium depth (keeps bias meaningful).
const EQ_SPRING: f32 = 0.08;

/// Marker: Set A / Set D module-pixel organism.
#[derive(Debug, Clone, Copy, Default)]
pub struct Organism {
    /// Consecutive ticks in drought dormancy (land plants). Resets when
    /// soil rewets; death after [`crate::root::DROUGHT_HIBERNATE_MAX_TICKS`].
    pub drought_ticks: u32,
    /// Fractional kg awaiting the next integer pore-water sip.
    pub sip_acc_kg: f32,
    /// Spawn / founder tank size. Land roots scale [`Energy::max`] above
    /// this as storage; photo, upkeep, and growth floors stay keyed here.
    pub energy_base_max: f32,
}

/// Vertical motion state for weight / buoyancy integration.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuoyancyState {
    /// Vertical velocity (m/tick); positive = up.
    pub vel_y: f32,
    /// Previous free-water top, used to ride rising/falling water.
    pub last_water_top: Option<f32>,
}

/// Per-organism lineage bookkeeping (not a gene — not mutated).
#[derive(Debug, Clone, Copy, Default)]
pub struct Lineage {
    /// `0` = editor / scenario founder; each fission adds one.
    pub generation: u32,
    /// Successful offspring this organism has produced.
    pub clones_produced: u32,
    /// Ticks since spawn (senescence clock).
    pub age_ticks: u64,
    /// Scenario / editor founder tag. Copied to children on fission.
    /// `0` = untagged; scenarios use non-zero ids to track competing lineages.
    pub founder_id: u8,
    /// Rhizome colony id. Land sprouts inherit the parent's id; ramets
    /// within range share moisture sips and equalize energy.
    /// `0` = unassigned / plankton (no sharing).
    pub genet_id: u32,
}

/// Default climate day+night length (ticks) — used as one "sim day".
const SIM_DAY_TICKS: f32 = 12.0 * 3_600.0 + 10.0 * 3_600.0; // 79_200

/// Nominal adult life at a reference genome (~3 sim-days for default algae).
const BASE_LIFE_SIM_DAYS: f32 = 3.0;

/// Dead body: sinks to the water bed (or rests on dry land), lingers, then
/// becomes Organic sediment (plus a little ecology litter for nutrient recycle).
#[derive(Debug, Clone, Copy, Default)]
pub struct Corpse {
    pub ticks: u32,
    pub settled_ticks: u32,
}

/// Snapshot for the click-to-inspect HUD.
#[derive(Debug, Clone)]
pub struct OrganismInspect {
    pub entity_id: u32,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub energy: f32,
    pub energy_max: f32,
    pub generation: u32,
    pub clones_produced: u32,
    pub age_ticks: u64,
    /// Scenario founder tag (`0` = untagged).
    pub founder_id: u8,
    /// Gene-derived expected lifespan (ticks), with per-entity jitter applied.
    pub life_expectancy_ticks: u64,
    pub genome: Genome,
    pub module_count: usize,
    pub photosystems: usize,
    pub roots: usize,
    pub stems: usize,
    pub is_plankton: bool,
    pub dead: bool,
    /// Land-plant drought dormancy ticks (0 = hydrated / not dormant).
    pub drought_ticks: u32,
    /// When dead: `(settled_ticks, CORPSE_SETTLE_TICKS)` while resting on bed/land.
    pub corpse_settle: Option<(u32, u32)>,
}

/// Life expectancy from existing genes (no extra gene).
///
/// Tradeoffs (live-fast / die-young style):
/// - higher `metabolic_rate` → shorter life
/// - longer `active_window` → more wear
/// - lower `reproduce_at` (earlier fission) → shorter life
/// - lower `clone_fidelity` → shorter life (messy lineages burn out)
///
/// Duration is measured in **climate cycles** (day+night). Default genome
/// lands near [`BASE_LIFE_SIM_DAYS`] on the default 12h+10h calendar; short
/// scenario cycles scale life down so senescence still fires in soak tests.
pub fn life_expectancy_ticks(genome: &Genome) -> u64 {
    life_expectancy_for_cycle(genome, SIM_DAY_TICKS as u64)
}

/// Life expectancy for a world whose day+night cycle is `cycle_ticks` long.
pub fn life_expectancy_for_cycle(genome: &Genome, cycle_ticks: u64) -> u64 {
    let m = genome.metabolic_rate.clamp(0.05, 1.5);
    let active = genome.active_window.clamp(0.1, 1.0);
    let early_repro = (0.95 - genome.reproduce_at.clamp(0.3, 0.95)) / 0.65;
    let fidelity = genome.clone_fidelity.clamp(0.0, 1.0);
    let cycle = cycle_ticks.max(1) as f32;

    // Wear relative to the default Genome so its pace stays ≈ 1.0.
    let wear = (1.0 + 1.1 * early_repro) * (1.0 + 0.9 * (1.0 - fidelity));
    const EARLY_REF: f32 = (0.95 - 0.7) / 0.65;
    const FID_REF: f32 = 0.6;
    let wear_ref = (1.0 + 1.1 * EARLY_REF) * (1.0 + 0.9 * (1.0 - FID_REF));
    let pace = (m / 0.35) * (0.85 + 0.30 * active) * (wear / wear_ref);
    let ticks = BASE_LIFE_SIM_DAYS * cycle / pace.max(0.25);
    // Floor scales with cycle so short-day scenarios can still senesce.
    let floor = (cycle * 0.35).clamp(40.0, 1_000.0);
    ticks.round().max(floor) as u64
}

/// Per-individual limit: expectancy ± ~10% from entity id (avoids sync die-offs).
pub fn life_limit_ticks(genome: &Genome, entity_id: u32) -> u64 {
    life_limit_for_cycle(genome, entity_id, SIM_DAY_TICKS as u64)
}

/// Per-individual limit for a specific climate cycle length.
pub fn life_limit_for_cycle(genome: &Genome, entity_id: u32, cycle_ticks: u64) -> u64 {
    let base = life_expectancy_for_cycle(genome, cycle_ticks);
    let jitter = 90 + (entity_id % 21); // 90..110 %
    let floor = (cycle_ticks.max(1) / 4).max(30);
    (base.saturating_mul(jitter as u64) / 100).max(floor)
}

/// Basal drain per tick. Scaled so a full tank outlasts a default night.
pub fn organism_upkeep(genome: &Genome, energy_max: f32, l0: f32, n_photo: f32) -> f32 {
    let m = genome.metabolic_rate.clamp(0.05, 1.5);
    let modules = 1.0 + 0.25 * n_photo.max(1.0);
    let day = (energy_max.max(1.0) / DARK_ENDURANCE_TICKS) * m * modules;
    if l0 < 0.1 {
        day * NIGHT_UPKEEP_MULT
    } else {
        day
    }
}

/// Photosynthetic gain per tick (0 when dark).
pub fn organism_photo_gain(energy_max: f32, l0: f32, n_photo: f32) -> f32 {
    if l0 <= 0.01 {
        return 0.0;
    }
    let n = n_photo.max(1.0);
    // Long-day fill + short-day floor so blooms still work on E30 cycles.
    let scaled = energy_max.max(1.0) / 10_000.0 * l0 * n;
    let floor = PHOTON_RATE * l0 * n;
    scaled.max(floor)
}

/// Unimodal comfort in `[0, 1]` around `temp_optimum` with half-width `temp_width`.
pub fn temp_comfort_factor(temp_c: f32, genome: &Genome) -> f32 {
    let width = genome.temp_width.max(1.0);
    let x = (temp_c - genome.temp_optimum) / width;
    (-(x * x)).exp().clamp(0.0, 1.0)
}

/// Dissolved-CO₂ half-saturation (relative units) for photo Michaelis curve.
pub const CO2_HALF_SAT: f32 = 0.25;
/// CO₂ drawn from the water column per unit photo gain.
/// Tuned so dense blooms measurably starve dissolved CO₂ against air↔water exchange.
pub const CO2_PER_ENERGY: f32 = 0.014;
/// O₂ emitted per unit photo gain.
pub const O2_PER_ENERGY: f32 = 0.009;
fn corpse_rgb((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    // Desaturated brown-grey — readable as dead tissue, not living chroma.
    let luma = (r as u16 * 3 + g as u16 * 6 + b as u16) / 10;
    (
        ((luma + 40) / 2).min(120) as u8,
        ((luma + 20) / 2).min(90) as u8,
        (luma / 3).min(70) as u8,
    )
}

/// Baked module body from a blueprint at spawn time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleBody {
    pub blueprint: Blueprint,
}

impl ModuleBody {
    pub fn photosystem_count(&self) -> usize {
        self.blueprint.photosystem_count()
    }
}

/// Top and bed of the free-water column (world Y; top ≥ bed).
/// `None` when the column has no flowable water.
pub fn water_band(col: &Column) -> Option<(f32, f32)> {
    let (top, mass) = col.flowable_water()?;
    if mass <= 0 {
        return None;
    }
    let mut y = col.surface_y;
    for j in 0..col.layer_count as usize {
        let m = col.layers[j].material;
        let h = col.mass_to_height_delta(m, col.layers[j].thickness);
        match m {
            MaterialId::Water | MaterialId::Snow | MaterialId::Ice => {
                y -= h;
            }
            _ => {
                return Some((top, y));
            }
        }
    }
    Some((top, y))
}

/// Density relative to water: `0` bias → buoyant (0.55), `1` → heavy (1.45).
pub fn relative_density(bias: f32) -> f32 {
    0.55 + bias.clamp(0.0, 1.0) * 0.90
}

/// Gene equilibrium depth: bias `0` → `top - FLOAT_DEPTH_M`, bias `1` → bed.
pub fn equilibrium_y(top: f32, bed: f32, bias: f32) -> f32 {
    let float_y = (top - FLOAT_DEPTH_M).clamp(bed, top);
    let t = bias.clamp(0.0, 1.0);
    float_y + (bed - float_y) * t
}

/// How far above `pose.y` the tallest blueprint module sits (world metres).
///
/// Editor y=0 is the ground/deep line; painting toward the top of the canvas
/// stores larger `m.y`. For plankton we anchor that top at the float line so
/// a creature drawn at the top of the editor doesn't spawn in the air.
pub fn blueprint_body_top_offset(blueprint: &Blueprint) -> f32 {
    if !blueprint.is_plankton() || blueprint.modules.is_empty() {
        return 0.0;
    }
    let max_y = blueprint.modules.iter().map(|m| m.y).max().unwrap_or(0);
    (max_y as f32) * MODULE_CELL_COLS
}

/// Blueprint Y of the land-plant ground crown (nucleus, else highest Root).
///
/// Editor y=0 is the ground line, but players often paint mid-canvas. We
/// treat the nucleus (root crown) as the surface contact so stems rise
/// above ground and roots with lower y dig below — matching the build.
pub fn blueprint_land_crown_y(blueprint: &Blueprint) -> i16 {
    if let Some(y) = blueprint
        .modules
        .iter()
        .filter(|m| m.module == crate::ModuleId::Nucleus)
        .map(|m| m.y)
        .min()
    {
        return y;
    }
    if let Some(y) = blueprint
        .modules
        .iter()
        .filter(|m| m.module == crate::ModuleId::Root)
        .map(|m| m.y)
        .max()
    {
        return y;
    }
    blueprint.modules.iter().map(|m| m.y).min().unwrap_or(0)
}

/// `pose.y` so the land-plant crown sits on the **solid bed**.
///
/// Pass [`Column::climate_elevation`], not [`Column::surface_y`]: flood
/// waves raise `surface_y` and would otherwise stretch/lift the whole
/// plant (roots and all) as if the water were pulling it out.
pub fn land_plant_pose_y(bed_y: f32, blueprint: &Blueprint) -> f32 {
    bed_y - blueprint_land_crown_y(blueprint) as f32 * MODULE_CELL_COLS
}

/// Crown on the column's solid bed (skips water / ice / snow caps).
pub fn land_plant_pose_y_on(col: &Column, blueprint: &Blueprint) -> f32 {
    land_plant_pose_y(col.climate_elevation(), blueprint)
}

/// Spawn / query helper — equilibrium if water exists, else sediment surface.
pub fn buoyancy_target_y(col: &Column, bias: f32) -> Option<f32> {
    let (top, bed) = water_band(col)?;
    Some(equilibrium_y(top, bed, bias))
}

/// Integrate one tick of weight vs buoyancy. Updates `y` and `state`.
///
/// `body_top_offset` is the distance from `pose.y` to the creature's highest
/// module (see [`blueprint_body_top_offset`]). Equilibrium tracks that top
/// so tall editor paints stay submerged instead of floating in air.
pub fn step_buoyancy(
    y: &mut f32,
    state: &mut BuoyancyState,
    col: &Column,
    bias: f32,
    body_top_offset: f32,
) {
    let offset = body_top_offset.max(0.0);
    let ground = {
        // Sediment contact: top of first solid under any fluid cap, else surface.
        water_band(col)
            .map(|(_, bed)| bed)
            .unwrap_or(col.surface_y)
    };

    if let Some((top, bed)) = water_band(col) {
        // Ride the free surface when water rises/falls and we were in the column.
        if let Some(prev_top) = state.last_water_top {
            let delta = top - prev_top;
            if delta.abs() > 1e-6 {
                let body_top = *y + offset;
                let was_in_column = body_top <= prev_top + 0.25 && *y >= bed - 0.25;
                if was_in_column {
                    if delta > 0.0 {
                        // Rising water lifts the body with the column.
                        *y += delta;
                    } else {
                        // Falling free surface: floaters (light) follow it down;
                        // heavy bodies keep their depth until buoyancy says otherwise.
                        let dens = relative_density(bias);
                        if dens < 1.0 {
                            let float_pose = equilibrium_y(prev_top, bed, 0.0) - offset;
                            if (*y - float_pose).abs() < FLOAT_DEPTH_M + 0.5 {
                                *y = (*y + delta).max(bed);
                            }
                        }
                    }
                }
            }
        }
        state.last_water_top = Some(top);

        let dens = relative_density(bias);
        // Pose equilibrium: body top sits on the gene float/sink line.
        let eq = equilibrium_y(top, bed, bias) - offset;
        let body_top = *y + offset;

        if body_top > top {
            // Air above the free surface — gravity wins.
            state.vel_y -= GRAVITY;
            state.vel_y *= 1.0 - AIR_DRAG;
            *y += state.vel_y;
            if *y + offset < top {
                // Splash: enter water, bleed downward speed.
                state.vel_y *= 0.4;
            }
        } else {
            // Submerged: buoyancy (− dens) vs weight, plus a mild pull to gene depth.
            // dens < 1 ⇒ net upward accel; dens > 1 ⇒ net downward.
            let accel = GRAVITY * (1.0 - dens) + (eq - *y) * EQ_SPRING;
            state.vel_y += accel;
            state.vel_y *= 1.0 - WATER_DRAG;
            *y += state.vel_y;

            if *y < bed {
                *y = bed;
                state.vel_y = state.vel_y.max(0.0);
            }
            // Don't let floaters launch far above the surface; park near float depth.
            if dens < 1.0 && *y + offset > top {
                *y = top - offset;
                if state.vel_y > 0.0 {
                    state.vel_y = 0.0;
                }
            }
        }

        // Soft settle near equilibrium when nearly still (stops endless bobbing).
        if state.vel_y.abs() < 0.01 && (*y - eq).abs() < 0.05 {
            *y = eq;
            state.vel_y = 0.0;
        }
    } else {
        state.last_water_top = None;
        // Dry column — fall onto sediment.
        state.vel_y -= GRAVITY;
        state.vel_y *= 1.0 - AIR_DRAG;
        *y += state.vel_y;
        if *y <= ground {
            *y = ground;
            state.vel_y = 0.0;
        }
    }
}

fn is_deep_water(col: &Column) -> bool {
    match water_band(col) {
        Some((top, bed)) => (top - bed) > 0.5,
        None => false,
    }
}

/// Map climate day/night factor (−1..1) to sky light 0..1.
pub fn sky_light_l0(day_night_factor: f32) -> f32 {
    ((day_night_factor + 1.0) * 0.5).clamp(0.0, 1.0)
}

/// Circadian depth migration on top of [`Genome::buoyancy_bias`].
///
/// Active window (typically day for default phase): prefer the upper
/// water column. Inactive (night): prefer deeper water. Gene still
/// distinguishes floaters from mid-water / sinkers within each band.
///
/// Interim Set-A stand-in for the planned Set-C soma wiring of day/night
/// → depth target (E33).
pub fn circadian_buoyancy_bias(genome: &Genome, phase_fraction: f32) -> f32 {
    let g = genome.buoyancy_bias.clamp(0.0, 1.0);
    if circadian_active(genome, phase_fraction) {
        g * 0.35
    } else {
        0.55 + g * 0.45
    }
}

/// True if the organism's circadian window is open this tick.
pub fn circadian_active(genome: &Genome, phase_fraction: f32) -> bool {
    let half = (genome.active_window * 0.5).clamp(0.0, 0.5);
    if half <= 0.0 {
        return false;
    }
    let mut d = (phase_fraction - genome.circadian_phase).abs();
    if d > 0.5 {
        d = 1.0 - d;
    }
    d <= half
}

/// Mutate with strength scaled by `(1 - clone_fidelity)`.
pub fn mutate_organism(
    parent: Genome,
    world_seed: u64,
    tick: u64,
    parent_id: u32,
) -> Genome {
    let mut g = Genome::mutate(parent, world_seed, tick, parent_id);
    let strength = (1.0 - parent.clone_fidelity).clamp(0.0, 1.0) * MUTATION_SIGMA;
    if strength <= 1e-6 {
        return parent;
    }
    let salt_base = tick
        .wrapping_mul(0xC0FF_EE11)
        .wrapping_add(parent_id as u64);
    let mut trait_i = 0u64;
    let mut jitter = |value: f32, lo: f32, hi: f32| -> f32 {
        trait_i += 1;
        let h = hash_u64(world_seed, salt_base as i64, trait_i as i64, 0xA70);
        let u = (h as f32 / u64::MAX as f32) * 2.0 - 1.0;
        (value * (1.0 + u * strength)).clamp(lo, hi)
    };
    g.metabolic_rate = jitter(g.metabolic_rate, 0.05, 1.2);
    g.reproduce_at = jitter(g.reproduce_at, 0.3, 0.95);
    g.clone_fidelity = jitter(g.clone_fidelity, 0.0, 1.0);
    g.circadian_phase = jitter(g.circadian_phase, 0.0, 1.0);
    g.active_window = jitter(g.active_window, 0.1, 1.0);
    g.buoyancy_bias = jitter(g.buoyancy_bias, 0.0, 1.0);
    g.temp_optimum = jitter(g.temp_optimum, -5.0, 40.0);
    g.temp_width = jitter(g.temp_width, 4.0, 25.0);
    g.root_depth_bias = jitter(g.root_depth_bias, 0.0, 1.0);
    g.alloc_stem = jitter(g.alloc_stem, 0.0, 1.0);
    g.alloc_leaf = jitter(g.alloc_leaf, 0.0, 1.0);
    g.alloc_root = jitter(g.alloc_root, 0.0, 1.0);
    g.leaf_absorb = jitter(g.leaf_absorb, 0.05, 1.0);
    g.shade_efficiency = jitter(g.shade_efficiency, 0.0, 1.0);
    g.digest_rate = jitter(g.digest_rate, 0.05, 2.0);
    g
}

/// Soft cap so messy clone lineages don't paint an unbounded blob.
const MAX_PHOTOSYSTEM_MODULES: usize = 16;

/// Base chance of a morphology clone-error (add/remove a Photosystem),
/// scaled by `(1 - clone_fidelity)`. Perfect fidelity → never; fidelity 0
/// → this rate. Keeps green photosystems able to grow/shrink over gens.
const MORPH_ERROR_BASE: f32 = 0.40;

/// Clone-time fungus morphology: add/remove a Hypha pixel (never the last
/// Digest, never the Nucleus). Scaled by `(1 - fidelity)`.
pub fn mutate_blueprint_fungus_morphology(
    mut bp: Blueprint,
    parent_fidelity: f32,
    world_seed: u64,
    tick: u64,
    parent_id: u32,
) -> Blueprint {
    let error_p = (1.0 - parent_fidelity.clamp(0.0, 1.0)) * MORPH_ERROR_BASE;
    if error_p <= 1e-6 {
        return bp;
    }
    let roll =
        hash_u64(world_seed, tick as i64, parent_id as i64, 0xF401) as f32 / u64::MAX as f32;
    if roll >= error_p {
        return bp;
    }
    let occupied: std::collections::HashSet<(i16, i16)> =
        bp.modules.iter().map(|m| (m.x, m.y)).collect();
    let n_h = bp.hypha_count();
    let can_add = n_h < crate::fungi::MAX_HYPHA_MODULES;
    let can_remove = n_h > 0;
    if !can_add && !can_remove {
        return bp;
    }
    let prefer_add = hash_u64(world_seed, tick as i64, parent_id as i64, 0xF402) & 1 == 0;
    let do_add = if can_add && can_remove {
        prefer_add
    } else {
        can_add
    };
    if do_add {
        const DIRS: [(i16, i16); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        let mut candidates: Vec<(i16, i16)> = Vec::new();
        for m in &bp.modules {
            for &(dx, dy) in &DIRS {
                let nx = m.x + dx;
                let ny = m.y + dy;
                if nx.abs() > 10 || ny.abs() > 6 {
                    continue;
                }
                if !occupied.contains(&(nx, ny)) {
                    candidates.push((nx, ny));
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        if candidates.is_empty() {
            return bp;
        }
        let pick =
            hash_u64(world_seed, tick as i64, parent_id as i64, 0xF403) as usize % candidates.len();
        let (x, y) = candidates[pick];
        let lane = bp
            .modules
            .first()
            .map(|m| m.lane)
            .unwrap_or(crate::LaneId::Mid);
        bp.modules.push(crate::blueprint::PlacedModule {
            x,
            y,
            lane,
            module: crate::ModuleId::Hypha,
        });
    } else {
        let idxs: Vec<usize> = bp
            .modules
            .iter()
            .enumerate()
            .filter(|(_, m)| m.module == crate::ModuleId::Hypha)
            .map(|(i, _)| i)
            .collect();
        if idxs.is_empty() {
            return bp;
        }
        let pick =
            hash_u64(world_seed, tick as i64, parent_id as i64, 0xF404) as usize % idxs.len();
        bp.modules.remove(idxs[pick]);
    }
    bp
}

/// Clone-time morphology mutation: with probability scaled by
/// `(1 - fidelity)`, either place a Photosystem on an empty 4-neighbour of
/// an existing module, or delete one Photosystem (never the last one, never
/// the Nucleus). Lets algae visibly grow or shed green units across gens.
pub fn mutate_blueprint_morphology(
    mut bp: Blueprint,
    parent_fidelity: f32,
    world_seed: u64,
    tick: u64,
    parent_id: u32,
) -> Blueprint {
    let error_p = (1.0 - parent_fidelity.clamp(0.0, 1.0)) * MORPH_ERROR_BASE;
    if error_p <= 1e-6 {
        return bp;
    }
    let roll =
        hash_u64(world_seed, tick as i64, parent_id as i64, 0x4D01) as f32 / u64::MAX as f32;
    if roll >= error_p {
        return bp;
    }

    let occupied: std::collections::HashSet<(i16, i16)> =
        bp.modules.iter().map(|m| (m.x, m.y)).collect();
    let n_photo = bp.photosystem_count();
    let can_add = n_photo < MAX_PHOTOSYSTEM_MODULES;
    let can_remove = n_photo > 1;
    if !can_add && !can_remove {
        return bp;
    }

    let prefer_add = hash_u64(world_seed, tick as i64, parent_id as i64, 0x4D02) & 1 == 0;
    let do_add = if can_add && can_remove {
        prefer_add
    } else {
        can_add
    };

    if do_add {
        // Candidate empty cells adjacent to any existing module.
        const DIRS: [(i16, i16); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        let mut candidates: Vec<(i16, i16)> = Vec::new();
        for m in &bp.modules {
            for &(dx, dy) in &DIRS {
                let nx = m.x + dx;
                let ny = m.y + dy;
                if nx.abs() > 8 || ny.abs() > 8 {
                    continue;
                }
                if !occupied.contains(&(nx, ny)) {
                    candidates.push((nx, ny));
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        if candidates.is_empty() {
            return bp;
        }
        let pick =
            hash_u64(world_seed, tick as i64, parent_id as i64, 0x4D03) as usize % candidates.len();
        let (x, y) = candidates[pick];
        let lane = bp
            .modules
            .first()
            .map(|m| m.lane)
            .unwrap_or(crate::LaneId::Mid);
        bp.modules.push(crate::blueprint::PlacedModule {
            x,
            y,
            lane,
            module: crate::ModuleId::Photosystem,
        });
    } else {
        let photo_idxs: Vec<usize> = bp
            .modules
            .iter()
            .enumerate()
            .filter(|(_, m)| m.module == crate::ModuleId::Photosystem)
            .map(|(i, _)| i)
            .collect();
        if photo_idxs.len() <= 1 {
            return bp;
        }
        let pick =
            hash_u64(world_seed, tick as i64, parent_id as i64, 0x4D04) as usize % photo_idxs.len();
        bp.modules.remove(photo_idxs[pick]);
    }
    bp
}

/// Axis-aligned body bounds in world space (from blueprint modules).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min_x: f32,
    pub max_x: f32,
    pub min_y: f32,
    pub max_y: f32,
}

impl Aabb {
    pub fn overlaps(self, other: Self) -> bool {
        self.min_x < other.max_x
            && self.max_x > other.min_x
            && self.min_y < other.max_y
            && self.max_y > other.min_y
    }

    pub fn width(self) -> f32 {
        (self.max_x - self.min_x).max(0.0)
    }

    pub fn center_x(self) -> f32 {
        (self.min_x + self.max_x) * 0.5
    }

    pub fn center_y(self) -> f32 {
        (self.min_y + self.max_y) * 0.5
    }

    fn inflated(self, pad: f32) -> Self {
        Self {
            min_x: self.min_x - pad,
            max_x: self.max_x + pad,
            min_y: self.min_y - pad,
            max_y: self.max_y + pad,
        }
    }
}

/// World-space AABB for a posed blueprint. Size grows with painted modules.
///
/// Land plants occupy **one column** of X so neighbours on cliffs don't
/// block each other (wide AABBs were preventing root sprouts entirely).
pub fn organism_aabb(pose: &Pose, blueprint: &Blueprint) -> Aabb {
    let modules = &blueprint.modules;
    if modules.is_empty() {
        let half = MODULE_CELL_COLS * 0.5;
        return Aabb {
            min_x: pose.x - half,
            max_x: pose.x + half,
            min_y: pose.y - half,
            max_y: pose.y + half,
        }
        .inflated(COLLISION_PAD);
    }
    let min_mx = modules.iter().map(|m| m.x).min().unwrap_or(0);
    let max_mx = modules.iter().map(|m| m.x).max().unwrap_or(0);
    let mid_x = (min_mx as f32 + max_mx as f32) * 0.5;
    let half = MODULE_CELL_COLS * 0.5;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for m in modules {
        let cy = pose.y + m.y as f32 * MODULE_CELL_COLS;
        min_y = min_y.min(cy - half);
        max_y = max_y.max(cy + half);
    }
    if blueprint.is_rooted() {
        let col0 = pose.x.floor();
        return Aabb {
            min_x: col0 + 0.02,
            max_x: col0 + 0.98,
            min_y,
            max_y,
        }
        .inflated(COLLISION_PAD * 0.25);
    }
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    for m in modules {
        let cx = pose.x + (m.x as f32 - mid_x) * MODULE_CELL_COLS;
        min_x = min_x.min(cx - half);
        max_x = max_x.max(cx + half);
    }
    Aabb {
        min_x,
        max_x,
        min_y,
        max_y,
    }
    .inflated(COLLISION_PAD)
}

/// Horizontal body width used when spacing clones apart.
pub fn organism_width(blueprint: &Blueprint) -> f32 {
    organism_aabb(&Pose { x: 0.0, y: 0.0 }, blueprint)
        .width()
        .max(MODULE_CELL_COLS)
}

/// Living body footprint: `(entity, aabb, immobile)`.
///
/// Land plants (`is_rooted`) are always immobile under collision — trees
/// must not shove each other sideways into neighbouring soil columns.
/// Algae / plankton remain mobile.
fn collect_bodies(store: &AgentStore, _world: &World) -> Vec<(Entity, Aabb, bool)> {
    // Keep ordering deterministic (by entity id) — resolve_collisions
    // uses it for stable tie-break when two AABBs share a center-x.
    let mut out: Vec<(Entity, Aabb, bool)> = store
        .ecs
        .query::<(&Pose, &ModuleBody, &Organism)>()
        .iter()
        .map(|(e, (pose, body, _))| {
            (
                e,
                organism_aabb(pose, &body.blueprint),
                body.blueprint.is_rooted() || body.blueprint.is_fungus(),
            )
        })
        .collect();
    out.sort_by_key(|(e, _, _)| e.id());
    out
}

fn aabb_hits_any(bodies: &[(Entity, Aabb, bool)], aabb: Aabb, ignore: Option<Entity>) -> bool {
    // Bodies are unsorted-by-x, so linear scan; but callers hit this only
    // per-birth (not per organism per tick), so it's cheap enough.
    bodies.iter().any(|&(e, other, _)| {
        if ignore == Some(e) {
            return false;
        }
        aabb.overlaps(other)
    })
}

/// Prefer the gene equilibrium depth (float line / sink depth) at column `x`.
/// Returns a **pose.y** (body top = pose + [`blueprint_body_top_offset`]).
fn depth_for_pose(world: &World, blueprint: &Blueprint, x: f32, y_hint: f32) -> Option<f32> {
    let wx = x.floor() as i32;
    let col = world.column_at(wx)?;
    if blueprint.is_rooted() || blueprint.is_fungus() {
        // Seat on the solid bed / organic skin. No plantable gate — wrong
        // niches (deep water, cavity roofs) are allowed and simply struggle.
        return Some(land_plant_pose_y_on(col, blueprint));
    }
    if blueprint.is_plankton() {
        let offset = blueprint_body_top_offset(blueprint);
        if let Some((top, bed)) = water_band(col) {
            let eq_pose = equilibrium_y(top, bed, blueprint.genome.buoyancy_bias) - offset;
            // Honour hint only as a secondary depth band (packing rows).
            if (y_hint - eq_pose).abs() < 0.05 {
                return Some(eq_pose);
            }
            return Some(y_hint.clamp(bed, (top - offset).max(bed)));
        }
        // Dry column: only used for an explicit editor drop (clone search
        // skips dry land when k > 0 — see find_clear_pose).
        return Some(col.surface_y - offset);
    }
    Some(land_plant_pose_y_on(col, blueprint))
}

/// Seat a root sprout on `prefer_x` (from [`pick_root_sprout_x`]) or a
/// close rescue column. Does **not** walk another full sprout radius —
/// that stacked with the picker and teleported kids off cliffs.
fn find_land_sprout_pose(
    world: &World,
    occupied: &std::collections::HashSet<i32>,
    blueprint: &Blueprint,
    prefer_x: f32,
) -> Option<(f32, f32)> {
    let prefer_wx = prefer_x.floor() as i32;
    let prefer_bed = world
        .column_at(prefer_wx)
        .map(|c| c.climate_elevation())
        .unwrap_or(0.0);
    let reach = crate::root::root_reach_m(blueprint);
    let try_col = |wx: i32| -> Option<(f32, f32)> {
        if occupied.contains(&wx) {
            return None;
        }
        if !crate::root::column_can_host_sprout_for_reach(world, wx, reach) {
            return None;
        }
        let col = world.column_at(wx)?;
        let bed = col.climate_elevation();
        if (bed - prefer_bed).abs() > 10.0 {
            return None;
        }
        Some((wx as f32 + 0.5, land_plant_pose_y_on(col, blueprint)))
    };
    if let Some(p) = try_col(prefer_wx) {
        return Some(p);
    }
    // Rescue within ±2 columns of the preferred seat.
    for dist in 1..=2 {
        for sign in [1i32, -1] {
            if let Some(p) = try_col(prefer_wx + sign * dist) {
                return Some(p);
            }
        }
    }
    None
}

fn rooted_occupied_columns(bodies: &[(Entity, Aabb, bool)]) -> std::collections::HashSet<i32> {
    bodies
        .iter()
        .filter(|(_, _, immobile)| *immobile)
        .map(|(_, aabb, _)| aabb.center_x().floor() as i32)
        .collect()
}

/// Move water displaced by under-lake sediment onto neighbouring columns.
fn spill_displaced_water(world: &mut World, wx: i32, mut amount: i64, tick: u64) {
    if amount <= 0 {
        return;
    }
    let host_bed = world
        .column_at(wx)
        .map(|c| c.climate_elevation())
        .unwrap_or(0.0);
    // Prefer already-wet or lower-bed neighbours (lake / downhill).
    let mut order: Vec<(i64, i32)> = Vec::new(); // score, x
    for dist in 1..=4 {
        for sign in [1i32, -1] {
            let x = wx + sign * dist;
            let Some(col) = world.column_at(x) else {
                continue;
            };
            let wet = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
            let bed = col.climate_elevation();
            let score = wet.saturating_mul(2)
                + ((host_bed - bed).max(0.0) * 100.0) as i64
                - dist as i64;
            order.push((score, x));
        }
    }
    order.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, x) in order {
        if amount <= 0 {
            break;
        }
        if let Some(col) = world.column_at_mut(x) {
            let put = amount;
            col.adjust_top_water(put, tick);
            col.settle_by_density(tick);
            amount -= put;
        }
    }
    // Leftover (no neighbours) — book as audit so we don't invent mass later.
    if amount > 0 {
        world.mass_audit.evap_out_total = world
            .mass_audit
            .evap_out_total
            .saturating_add(amount);
    }
}

/// Host columns a ramet may drink from: self + same-genet neighbours in range.
fn genet_drink_hosts(
    wx: i32,
    genet_id: u32,
    genet_members: &std::collections::HashMap<u32, Vec<(Entity, i32)>>,
) -> Vec<i32> {
    let mut hosts = vec![wx];
    if genet_id == 0 {
        return hosts;
    }
    let Some(members) = genet_members.get(&genet_id) else {
        return hosts;
    };
    for &(_, x) in members {
        if (x - wx).abs() <= crate::root::GENET_SHARE_MAX_DIST && !hosts.contains(&x) {
            hosts.push(x);
        }
    }
    hosts
}

/// Soft-equalize energy fractions within each connected genet component.
/// Conserves total energy: targets are `mean_frac * max` per ramet.
fn equalize_genet_energy(
    store: &mut AgentStore,
    genet_members: &std::collections::HashMap<u32, Vec<(Entity, i32)>>,
) {
    let rate = crate::root::GENET_ENERGY_EQUALIZE.clamp(0.0, 1.0);
    if rate <= 0.0 {
        return;
    }
    for members in genet_members.values() {
        if members.len() < 2 {
            continue;
        }
        // Union-find by column proximity (rhizome reach).
        let n = members.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut [usize], i: usize) -> usize {
            let mut i = i;
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        for i in 0..n {
            for j in (i + 1)..n {
                if (members[i].1 - members[j].1).abs() <= crate::root::GENET_SHARE_MAX_DIST {
                    let a = find(&mut parent, i);
                    let b = find(&mut parent, j);
                    if a != b {
                        parent[b] = a;
                    }
                }
            }
        }
        let mut comps: std::collections::HashMap<usize, Vec<Entity>> =
            std::collections::HashMap::new();
        for i in 0..n {
            let r = find(&mut parent, i);
            comps.entry(r).or_default().push(members[i].0);
        }
        for ents in comps.values() {
            if ents.len() < 2 {
                continue;
            }
            let mut total = 0.0f32;
            let mut sum_max = 0.0f32;
            let mut snap: Vec<(Entity, f32, f32)> = Vec::with_capacity(ents.len());
            for &e in ents {
                let Ok(energy) = store.ecs.get::<&Energy>(e) else {
                    continue;
                };
                total += energy.current;
                sum_max += energy.max.max(1.0);
                snap.push((e, energy.current, energy.max.max(1.0)));
            }
            if snap.len() < 2 || sum_max <= 0.0 {
                continue;
            }
            let mean_frac = (total / sum_max).clamp(0.0, 1.0);
            for (e, cur, max_e) in snap {
                let target = mean_frac * max_e;
                let next = cur + (target - cur) * rate;
                if let Ok(mut energy) = store.ecs.get::<&mut Energy>(e) {
                    energy.current = next.clamp(0.0, energy.max);
                }
            }
        }
    }
}

/// Find a clear pose, scanning **outward horizontally** first.
///
/// `max_steps` limits how far the search walks (`None` = whole map —
/// used by plankton clones). Root sprouts pass a small radius so
/// suckers stay near the parent root system.
fn find_clear_pose(
    world: &World,
    bodies: &[(Entity, Aabb, bool)],
    blueprint: &Blueprint,
    x0: f32,
    y0: f32,
) -> Option<(f32, f32)> {
    find_clear_pose_limited(world, bodies, blueprint, x0, y0, None)
}

fn find_clear_pose_limited(
    world: &World,
    bodies: &[(Entity, Aabb, bool)],
    blueprint: &Blueprint,
    x0: f32,
    y0: f32,
    max_steps: Option<i32>,
) -> Option<(f32, f32)> {
    let (lo, hi) = world.world_x_bounds()?;
    let lo_f = lo as f32;
    let hi_f = hi as f32 + 0.99;
    let width = organism_width(blueprint);
    let step = (width + COLLISION_PAD).max(MODULE_CELL_COLS);
    let offset = blueprint_body_top_offset(blueprint);
    let plankton = blueprint.is_plankton();

    // Depth rows: primary equilibrium pose, then below/above if full.
    let mut depth_rows = vec![y0];
    if plankton {
        if let Some(col) = world.column_at(x0.floor() as i32) {
            if let Some((top, bed)) = water_band(col) {
                let eq_pose = equilibrium_y(top, bed, blueprint.genome.buoyancy_bias) - offset;
                depth_rows = vec![eq_pose];
                let mut d = width;
                while eq_pose - d >= bed {
                    depth_rows.push(eq_pose - d);
                    d += width;
                }
                d = width;
                while eq_pose + d + offset <= top {
                    depth_rows.push(eq_pose + d);
                    d += width;
                }
            } else {
                // Dry column: keep `y0` so an explicit editor drop still works.
                // Outward clone search (k>0) skips further dry columns below.
            }
        }
    }

    for &row_y in &depth_rows {
        // k = 0, +1, -1, +2, -2, ...
        let full_k = ((hi_f - lo_f) / step).ceil() as i32 + 1;
        let max_k = max_steps.unwrap_or(full_k).clamp(0, full_k);
        for k in 0..=max_k {
            for sign in [1i32, -1] {
                if k == 0 && sign < 0 {
                    continue;
                }
                let x = (x0 + sign as f32 * k as f32 * step).clamp(lo_f, hi_f);
                // Algae clones must not spread onto dry beach (k>0). An
                // explicit editor drop at k==0 on land is still allowed.
                if plankton && k > 0 {
                    let wx = x.floor() as i32;
                    if world.column_at(wx).and_then(water_band).is_none() {
                        continue;
                    }
                }
                let Some(y) = depth_for_pose(world, blueprint, x, row_y) else {
                    continue;
                };
                let pose = Pose { x, y };
                let aabb = organism_aabb(&pose, blueprint);
                if !aabb_hits_any(bodies, aabb, None) {
                    return Some((x, y));
                }
                if k == 0 {
                    break;
                }
            }
        }
    }
    None
}

/// Push overlapping footprints apart. Prefer **horizontal** separation so
/// buoyancy (same depth) doesn't re-form a vertical lens every tick.
///
/// **Rooted land plants are immobile** — algae bounce off; trees never
/// shove each other into neighbouring soil. Two overlapping plants stay
/// put (clone search should avoid packing them).
///
/// Broad-phase: sort by AABB `min_x`, then compare only bodies whose
/// x-extents overlap. Reduces the O(N²) pair scan to O(N + K) where K
/// is the actual overlap count. At pop=500 this is the difference
/// between ~5 ms and ~0.1 ms for the collision pass.
fn resolve_collisions(store: &mut AgentStore, world: &World) {
    let (lo, hi) = match world.world_x_bounds() {
        Some(b) => b,
        None => return,
    };
    let lo_f = lo as f32;
    let hi_f = hi as f32 + 0.99;

    let mut dx = Vec::<(u32, f32)>::new();
    let mut dy = Vec::<(u32, f32)>::new();
    for _ in 0..12 {
        let mut bodies = collect_bodies(store, world);
        if bodies.len() < 2 {
            return;
        }
        // Sort by min_x for the sweep. `collect_bodies` sorts by id;
        // resort here since we need spatial order for the broad-phase.
        bodies.sort_by(|(_, a, _), (_, b, _)| {
            a.min_x.partial_cmp(&b.min_x).unwrap_or(std::cmp::Ordering::Equal)
        });
        dx.clear();
        dy.clear();
        let mut any = false;
        for i in 0..bodies.len() {
            let (ea, a, a_anchored) = bodies[i];
            for j in (i + 1)..bodies.len() {
                let (eb, b, b_anchored) = bodies[j];
                // Sweep termination: once the next body's min_x is
                // past this one's max_x, no further pair can overlap.
                if b.min_x >= a.max_x {
                    break;
                }
                if !a.overlaps(b) {
                    continue;
                }
                // Two land plants: both fixed — never sideways-bury each other.
                if a_anchored && b_anchored {
                    continue;
                }
                any = true;
                let overlap_x = (a.max_x.min(b.max_x) - a.min_x.max(b.min_x)).max(0.0);
                let overlap_y = (a.max_y.min(b.max_y) - a.min_y.max(b.min_y)).max(0.0);
                if overlap_x <= 0.0 || overlap_y <= 0.0 {
                    continue;
                }

                // Side-view: shove on X. Rooted body never moves; only the
                // mobile partner (plankton) yields.
                let push = (overlap_x * 0.5).max(MIN_SEPARATION) + COLLISION_PAD;
                let a_left = a.center_x() < b.center_x()
                    || (a.center_x() == b.center_x() && ea.id() < eb.id());
                let (s_a, s_b) = match (a_anchored, b_anchored) {
                    (true, false) => {
                        let dir = if a_left { push * 2.0 } else { -push * 2.0 };
                        (0.0, dir)
                    }
                    (false, true) => {
                        let dir = if a_left { -push * 2.0 } else { push * 2.0 };
                        (dir, 0.0)
                    }
                    (false, false) => {
                        if a_left {
                            (-push, push)
                        } else {
                            (push, -push)
                        }
                    }
                    (true, true) => (0.0, 0.0), // unreachable — continued above
                };
                if s_a != 0.0 {
                    dx.push((ea.id(), s_a));
                }
                if s_b != 0.0 {
                    dx.push((eb.id(), s_b));
                }

                // Edge fallback Y — mobile bodies only.
                let a_at_edge = a.center_x() <= lo_f + 0.1 || a.center_x() >= hi_f - 0.1;
                let b_at_edge = b.center_x() <= lo_f + 0.1 || b.center_x() >= hi_f - 0.1;
                if a_at_edge && b_at_edge && overlap_y > 0.0 && !a_anchored && !b_anchored {
                    let y_push = overlap_y * 0.5 + COLLISION_PAD;
                    let (ya, yb) = if a.center_y() <= b.center_y() {
                        (-y_push, y_push)
                    } else {
                        (y_push, -y_push)
                    };
                    dy.push((ea.id(), ya));
                    dy.push((eb.id(), yb));
                }
            }
        }
        if !any {
            return;
        }
        // Sum per-entity pushes (short vecs — no HashMap overhead).
        dx.sort_by_key(|&(id, _)| id);
        dy.sort_by_key(|&(id, _)| id);
        for (e, aabb, anchored) in &bodies {
            // Hard rule: only unanchored bodies move from collision.
            if *anchored {
                continue;
            }
            let id = e.id();
            let px: f32 = dx
                .iter()
                .filter_map(|&(k, v)| if k == id { Some(v) } else { None })
                .sum();
            let py: f32 = dy
                .iter()
                .filter_map(|&(k, v)| if k == id { Some(v) } else { None })
                .sum();
            if px == 0.0 && py == 0.0 {
                continue;
            }
            if let Ok(mut pose) = store.ecs.get::<&mut Pose>(*e) {
                let mut x = pose.x + px;
                if world.topology().is_ring() {
                    let wx = world.resolve_world_x(x.floor() as i32);
                    let frac = x - x.floor();
                    x = wx as f32 + frac;
                } else {
                    x = x.clamp(lo_f, hi_f);
                }
                pose.x = x;
                pose.y += py;
                let _ = aabb;
            }
        }
    }
}

impl AgentStore {
    /// Living Set A organisms (excludes sinking corpses).
    pub fn organism_count(&self) -> usize {
        self.ecs.query::<&Organism>().iter().count()
    }

    pub fn corpse_count(&self) -> usize {
        self.ecs.query::<&Corpse>().iter().count()
    }

    /// Living counts by habit: `(algae, plant, fungus)`.
    pub fn count_by_habit(&self) -> (usize, usize, usize) {
        let mut algae = 0usize;
        let mut plant = 0usize;
        let mut fungus = 0usize;
        for (_, (body, _)) in self.ecs.query::<(&ModuleBody, &Organism)>().iter() {
            match OrganismHabit::from_blueprint(&body.blueprint) {
                OrganismHabit::Algae => algae += 1,
                OrganismHabit::Plant => plant += 1,
                OrganismHabit::Fungus => fungus += 1,
            }
        }
        (algae, plant, fungus)
    }

    /// True if another organism of this habit may spawn / birth.
    ///
    /// `pending` counts same-tick deferred births already queued (and, for
    /// the global ceiling, all deferred births).
    pub fn habit_has_room(
        &self,
        habit: OrganismHabit,
        pending_same_habit: usize,
        pending_total: usize,
    ) -> bool {
        let (algae, plant, fungus) = self.count_by_habit();
        let n = match habit {
            OrganismHabit::Algae => algae,
            OrganismHabit::Plant => plant,
            OrganismHabit::Fungus => fungus,
        };
        let cap = self.pop_caps.for_habit(habit);
        n + pending_same_habit < cap && self.organism_count() + pending_total < MAX_ORGANISMS
    }

    /// Elevation for a newly spawned / cloned organism (`pose.y`).
    pub fn spawn_elevation(world: &World, world_x: i32, blueprint: &Blueprint) -> Option<f32> {
        let col = world.column_at(world_x)?;
        let offset = blueprint_body_top_offset(blueprint);
        if blueprint.is_plankton() {
            if let Some(eq) = buoyancy_target_y(col, blueprint.genome.buoyancy_bias) {
                // Anchor the tallest painted module at the float line so
                // editor-top paints spawn in water, not in the air.
                return Some(eq - offset);
            }
            return Some(col.surface_y - offset);
        }
        // Land plant: nucleus (crown) on the solid bed — never on the
        // free-water top (flood waves must not stretch rooted plants).
        let _ = world_x;
        Some(land_plant_pose_y_on(col, blueprint))
    }

    /// Spawn a blueprint-backed organism near column `world_x`.
    /// Placement clears existing **footprint AABBs** (size from blueprint).
    pub fn spawn_from_blueprint(
        &mut self,
        world: &World,
        world_x: i32,
        blueprint: Blueprint,
        energy: f32,
    ) -> Option<Entity> {
        if self.organism_count() >= MAX_ORGANISMS {
            return None;
        }
        if self.len() as usize >= MAX_AGENTS + MAX_ORGANISMS {
            return None;
        }
        if !blueprint.is_valid_atom() && !blueprint.is_valid_fungus() {
            return None;
        }
        let habit = OrganismHabit::from_blueprint(&blueprint);
        if !self.habit_has_room(habit, 0, 0) {
            return None;
        }
        if world.column_at(world_x).is_none() {
            return None;
        }

        let y0 = Self::spawn_elevation(world, world_x, &blueprint)?;
        let x0 = world_x as f32 + 0.5;
        let bodies = collect_bodies(self, world);
        let (x, y) = if blueprint.is_rooted() || blueprint.is_fungus() {
            // Prefer the clicked / requested column if free; else a nearby
            // rescue seat. Plantable gates no longer block editor drops —
            // cavity roofs, deep water, and dry rock are all allowed.
            let mut occupied = rooted_occupied_columns(&bodies);
            if !occupied.contains(&world_x) {
                (x0, y0)
            } else {
                occupied.insert(world_x);
                find_land_sprout_pose(world, &occupied, &blueprint, x0)?
            }
        } else {
            find_clear_pose(world, &bodies, &blueprint, x0, y0)?
        };

        let max_e = energy.max(1.0).max(40.0);
        let host_x = x.floor() as i32;
        let last_water_top = world
            .column_at(host_x)
            .and_then(water_band)
            .map(|(top, _)| top);
        let e = self.ecs.spawn((
            Pose { x, y },
            BuoyancyState {
                vel_y: 0.0,
                last_water_top,
            },
            Energy {
                current: energy.clamp(0.0, max_e),
                max: max_e,
            },
            blueprint.genome,
            ModuleBody { blueprint },
            Lineage::default(),
            Organism {
                energy_base_max: max_e,
                ..Organism::default()
            },
        ));
        Some(e)
    }

    /// Tag a living organism as belonging to scenario founder `founder_id`.
    /// Children inherit the tag on fission.
    pub fn tag_founder(&mut self, entity: Entity, founder_id: u8) {
        if let Ok(mut lin) = self.ecs.get::<&mut Lineage>(entity) {
            lin.founder_id = founder_id;
        }
    }

    /// Living organisms whose lineage carries `founder_id`.
    pub fn count_living_by_founder(&self, founder_id: u8) -> usize {
        self.ecs
            .query::<(&Lineage, &Organism)>()
            .iter()
            .filter(|(_, (lin, _))| lin.founder_id == founder_id)
            .count()
    }

    /// True if `entity` still exists as a living organism or a corpse.
    pub fn organism_alive(&self, entity: Entity) -> bool {
        self.ecs.get::<&Organism>(entity).is_ok() || self.ecs.get::<&Corpse>(entity).is_ok()
    }

    /// Pick the smallest-footprint living body or corpse at `(wx, wy)`.
    pub fn pick_organism_at(&self, wx: f32, wy: f32) -> Option<Entity> {
        let mut best: Option<(Entity, f32)> = None;
        let mut consider = |e: Entity, pose: &Pose, body: &ModuleBody| {
            let aabb = organism_aabb(pose, &body.blueprint);
            if wx < aabb.min_x || wx > aabb.max_x || wy < aabb.min_y || wy > aabb.max_y {
                return;
            }
            let area = aabb.width() * (aabb.max_y - aabb.min_y).max(0.01);
            match best {
                Some((_, a)) if a <= area => {}
                _ => best = Some((e, area)),
            }
        };
        for (e, (pose, body, _)) in self
            .ecs
            .query::<(&Pose, &ModuleBody, &Organism)>()
            .iter()
        {
            consider(e, pose, body);
        }
        for (e, (pose, body, _)) in self.ecs.query::<(&Pose, &ModuleBody, &Corpse)>().iter() {
            consider(e, pose, body);
        }
        best.map(|(e, _)| e)
    }

    /// Live inspect snapshot, or `None` if the entity is gone.
    pub fn inspect_organism(&self, entity: Entity) -> Option<OrganismInspect> {
        let pose = *self.ecs.get::<&Pose>(entity).ok()?;
        let body = self.ecs.get::<&ModuleBody>(entity).ok()?;
        let genome = self
            .ecs
            .get::<&Genome>(entity)
            .map(|g| *g)
            .unwrap_or_else(|_| body.blueprint.genome);
        let lineage = self
            .ecs
            .get::<&Lineage>(entity)
            .map(|l| *l)
            .unwrap_or_default();
        let (energy, energy_max) = self
            .ecs
            .get::<&Energy>(entity)
            .map(|e| (e.current, e.max))
            .unwrap_or((0.0, 0.0));
        let corpse_settle = self.ecs.get::<&Corpse>(entity).ok().map(|c| {
            let land = body.blueprint.is_rooted() || body.blueprint.is_fungus();
            let need = if land {
                CORPSE_SETTLE_LAND_TICKS
            } else {
                CORPSE_SETTLE_TICKS
            };
            (c.settled_ticks, need)
        });
        let dead = corpse_settle.is_some();
        let drought_ticks = self
            .ecs
            .get::<&Organism>(entity)
            .map(|o| o.drought_ticks)
            .unwrap_or(0);
        Some(OrganismInspect {
            entity_id: entity.id(),
            name: body.blueprint.name.clone(),
            x: pose.x,
            y: pose.y,
            energy,
            energy_max,
            generation: lineage.generation,
            clones_produced: lineage.clones_produced,
            age_ticks: lineage.age_ticks,
            founder_id: lineage.founder_id,
            life_expectancy_ticks: life_limit_ticks(&genome, entity.id()),
            genome,
            module_count: body.blueprint.modules.len(),
            photosystems: body.photosystem_count(),
            roots: body.blueprint.root_count(),
            stems: body.blueprint.stem_count(),
            is_plankton: body.blueprint.is_plankton(),
            dead,
            drought_ticks,
            corpse_settle,
        })
    }

    /// World-space AABB for highlighting a selected organism or corpse.
    pub fn organism_highlight_aabb(&self, entity: Entity) -> Option<Aabb> {
        let pose = *self.ecs.get::<&Pose>(entity).ok()?;
        let body = self.ecs.get::<&ModuleBody>(entity).ok()?;
        Some(organism_aabb(&pose, &body.blueprint))
    }

    /// Set A behaviour pass: light, buoyancy, AABB collision, reproduce, die.
    pub fn step_organisms(&mut self, world: &mut World, tick: u64) {
        if self.organism_count() == 0 {
            return;
        }
        self.wake_host_columns(world);

        let l0 = sky_light_l0(world.climate.day_night_factor(tick));
        let phase = world.climate.phase_fraction(tick);
        let cycle_ticks = world.climate.cycle_length_ticks();
        let population = self.organism_count();

        // Land plants join a genet (rhizome colony) and share along it.
        for (e, (body, lineage)) in self
            .ecs
            .query::<(&ModuleBody, &mut Lineage)>()
            .iter()
        {
            if body.blueprint.is_rooted() && lineage.genet_id == 0 {
                lineage.genet_id = e.id().saturating_add(1);
            }
        }
        // genet_id → (entity, host column) for living rooted ramets.
        let mut genet_members: std::collections::HashMap<u32, Vec<(Entity, i32)>> =
            std::collections::HashMap::new();
        for (e, (pose, body, lineage, _)) in self
            .ecs
            .query::<(&Pose, &ModuleBody, &Lineage, &Organism)>()
            .iter()
        {
            if body.blueprint.is_rooted() && lineage.genet_id != 0 {
                genet_members
                    .entry(lineage.genet_id)
                    .or_default()
                    .push((e, pose.world_x()));
            }
        }
        // Soft energy equalize from last tick's tanks (conserves total energy).
        equalize_genet_energy(self, &genet_members);

        // Sparse canopy index for cheap neighbour shade (O(plants), not O(world)).
        let mut canopy = crate::shade::CanopyIndex::default();
        for (e, (pose, body, genome, _)) in self
            .ecs
            .query::<(&Pose, &ModuleBody, &Genome, &Organism)>()
            .iter()
        {
            if !body.blueprint.is_rooted() {
                continue;
            }
            let n_photo = body.photosystem_count();
            if n_photo == 0 {
                continue;
            }
            let n_stem = body.blueprint.stem_count();
            let top = crate::shade::canopy_top_y(pose.y, &body.blueprint);
            let absorb =
                crate::shade::cast_strength(n_photo, n_stem, genome.leaf_absorb);
            crate::shade::record_canopy(
                &mut canopy,
                pose.world_x(),
                top,
                absorb,
                n_photo,
                e.id(),
            );
        }

        let mut deaths: Vec<(Entity, i32)> = Vec::new();
        // x, y, blueprint, energy, max_e, parent, parent_generation, founder_id, genet_id
        let mut births: Vec<(f32, f32, Blueprint, f32, f32, Entity, u32, u8, u32)> = Vec::new();
        let (n_algae0, n_plant0, n_fungus0) = self.count_by_habit();
        let pop_caps = self.pop_caps;

        for (e, (pose, buoy, energy, genome, body, lineage, organism)) in self
            .ecs
            .query::<(
                &mut Pose,
                &mut BuoyancyState,
                &mut Energy,
                &Genome,
                &mut ModuleBody,
                &mut Lineage,
                &mut Organism,
            )>()
            .iter()
        {
            let wx = pose.world_x();
            let n_photo = body.photosystem_count().max(1) as f32;
            let n_roots = body.blueprint.root_count() as f32;
            let active = circadian_active(genome, phase);

            lineage.age_ticks = lineage.age_ticks.saturating_add(1);
            let life_limit = life_limit_for_cycle(genome, e.id(), cycle_ticks);
            if lineage.age_ticks >= life_limit {
                deaths.push((e, wx));
                continue;
            }

            let temp_c = world.temperature_at_point(wx, pose.y, tick);
            let comfort = temp_comfort_factor(temp_c, genome);
            let plankton = body.blueprint.is_plankton();
            let rooted = body.blueprint.is_rooted();
            let fungus = body.blueprint.is_fungus();

            // Environment gates: water required, ice / freeze kills plankton.
            // Snow/ice cover dims light for everyone (cover_light_factor).
            let (in_water, iced, water_co2, nutrient, cover) =
                if let Some(col) = world.column_at(wx) {
                    let wet = water_band(col).is_some();
                    let ice = col.top_ice_mass() > 0;
                    (
                        wet,
                        ice,
                        col.ecology.water_co2,
                        crate::root::column_nutrient_factor(col),
                        col.cover_light_factor(),
                    )
                } else {
                    (false, false, 0.0, 0.2, 1.0)
                };
            // Plankton need free water; ice around them is lethal.
            if plankton && (!in_water || iced) {
                deaths.push((e, wx));
                continue;
            }

            if let Some(col) = world.column_at(wx) {
                if plankton {
                    let bias = circadian_buoyancy_bias(genome, phase);
                    let offset = blueprint_body_top_offset(&body.blueprint);
                    step_buoyancy(&mut pose.y, buoy, col, bias, offset);
                } else {
                    // Pin to column centre + solid bed so cliff neighbours
                    // never leave the crown floating or buried mid-face.
                    pose.x = wx as f32 + 0.5;
                    pose.y = land_plant_pose_y_on(col, &body.blueprint);
                    buoy.vel_y = 0.0;
                    buoy.last_water_top = None;
                }
            }

            // Land-plant drought / fungus starve+dry: hibernate for a limited window.
            let moist = if rooted || fungus {
                crate::root::adjacent_moisture_frac(world, wx)
            } else {
                1.0
            };
            let drought = if rooted {
                crate::root::drought_band(moist)
            } else {
                crate::root::DroughtBand::Hydrated
            };

            // Roots bank surplus above the spawn tank (starch analogy).
            // Photo / upkeep / growth floors stay on energy_base_max.
            if organism.energy_base_max < 1.0 {
                organism.energy_base_max = energy.max.max(1.0);
            }
            let tank_ref = organism.energy_base_max.max(1.0);
            if rooted {
                let cap = crate::root::energy_capacity(tank_ref, body.blueprint.root_count());
                energy.max = cap;
                if energy.current > cap {
                    energy.current = cap;
                }
            }
            let fungus_dormant = fungus && crate::fungi::fungus_should_hibernate(world, wx);
            let dormant = matches!(drought, crate::root::DroughtBand::Dormant) || fungus_dormant;
            if rooted || fungus {
                let hib_max = if fungus {
                    crate::fungi::FUNGUS_HIBERNATE_MAX_TICKS
                } else {
                    crate::root::DROUGHT_HIBERNATE_MAX_TICKS
                };
                if dormant {
                    organism.drought_ticks = organism.drought_ticks.saturating_add(1);
                    if organism.drought_ticks >= hib_max {
                        deaths.push((e, wx));
                        continue;
                    }
                } else {
                    organism.drought_ticks = 0;
                }
            }

            let plant_active = active && !dormant;

            // Fungi: digest labile litter / Organic → energy + soil nutrient.
            if fungus && !dormant {
                let budget = crate::fungi::digest_budget_kg(genome, &body.blueprint);
                let (_kg, gained, _nut) = crate::fungi::digest_labile(world, wx, budget);
                if gained > 0.0 {
                    energy.current = (energy.current + gained * comfort.max(0.2)).min(energy.max);
                }
            }

            if plant_active && !fungus && (!plankton || in_water) {
                // Land plants sample neighbour canopy shade; plankton keep sky L0
                // (water depth / CO₂ already gate blooms).
                let mut photo_l0 = if rooted {
                    let sample_y = crate::shade::canopy_top_y(pose.y, &body.blueprint);
                    crate::shade::effective_photo_light(
                        &canopy,
                        wx,
                        sample_y,
                        l0,
                        e.id(),
                        n_photo as usize,
                        genome,
                    )
                } else {
                    l0
                };
                // Submerged canopy: water above the leaf crown dims photo —
                // reed payoff is grow stems above the free surface while roots
                // drink the wet column.
                if rooted {
                    if let Some(col) = world.column_at(wx) {
                        if let Some((wtop, _)) = water_band(col) {
                            let leaf_top =
                                crate::shade::canopy_top_y(pose.y, &body.blueprint);
                            if wtop > leaf_top + 0.05 {
                                let cover_m = (wtop - leaf_top).clamp(0.0, 4.0);
                                photo_l0 *= (1.0 - 0.22 * cover_m).clamp(0.12, 1.0);
                            }
                        }
                    }
                }
                // Snow/ice pack Beer-Lambert attenuation (after shade / water).
                photo_l0 *= cover;
                let mut gain = organism_photo_gain(tank_ref, photo_l0, n_photo) * comfort;
                if plankton {
                    // Michaelis–Menten on dissolved CO₂ — blooms can starve the water.
                    let co2_factor = water_co2 / (water_co2 + CO2_HALF_SAT);
                    gain *= co2_factor;
                    if gain > 0.0 {
                        if let Some(col) = world.column_at_mut(wx) {
                            let take = (gain * CO2_PER_ENERGY).min(col.ecology.water_co2);
                            col.ecology.water_co2 =
                                (col.ecology.water_co2 - take).clamp(0.0, 3.0);
                            col.ecology.water_o2 =
                                (col.ecology.water_o2 + gain * O2_PER_ENERGY).clamp(0.0, 3.0);
                        }
                    }
                } else if rooted {
                    // Land plants: photo gated by soil / dissolved nutrients.
                    // Mild leaf bonus when hydrated — sand plains otherwise
                    // can't recover after a sprout payment.
                    gain *= nutrient.clamp(0.05, 1.6);
                    if matches!(drought, crate::root::DroughtBand::Hydrated) {
                        gain *= 1.45;
                    } else if matches!(drought, crate::root::DroughtBand::Stressed) {
                        gain *= 0.45; // stomata closing
                    }
                }
                energy.current = (energy.current + gain).min(energy.max);
            }

            // Roots sip moisture gently — accumulate fractional kg so we
            // never ceil to 1 kg/tick (that flash-dried hills in seconds).
            // Genet ramets may sip through siblings' columns (rhizome pipe).
            if rooted
                && plant_active
                && n_roots > 0.0
                && !matches!(drought, crate::root::DroughtBand::Dormant)
            {
                organism.sip_acc_kg = (organism.sip_acc_kg
                    + n_roots * crate::root::ROOT_SIP_KG_PER_ROOT)
                    .min(2.0);
                let budget = organism.sip_acc_kg.floor() as i64;
                if budget >= 1 {
                    let want = budget.min(crate::root::ROOT_SIP_MAX_KG_PER_TICK);
                    let hosts = genet_drink_hosts(wx, lineage.genet_id, &genet_members);
                    let drunk = crate::root::drink_from_hosts(world, &hosts, want);
                    organism.sip_acc_kg = (organism.sip_acc_kg - drunk as f32).max(0.0);
                    if drunk > 0 {
                        let sip =
                            drunk as f32 * crate::root::ROOT_WATER_ENERGY * nutrient.max(0.2);
                        energy.current = (energy.current + sip).min(energy.max);
                    }
                }
                if matches!(drought, crate::root::DroughtBand::Stressed) {
                    energy.current -= crate::root::ROOT_DROUGHT_STRESS_DRAIN;
                }
            }

            let mut upkeep = if fungus {
                // Fungi don't photosynthesize — basal drain tracks metabolic_rate
                // lightly, plus Digest/Hypha tissue tax.
                let basal = (tank_ref / DARK_ENDURANCE_TICKS)
                    * genome.metabolic_rate.clamp(0.05, 1.5);
                basal + crate::fungi::fungus_upkeep(&body.blueprint, dormant)
            } else {
                // Buried under snow is dark — attenuate light for upkeep too.
                let mut u = organism_upkeep(genome, tank_ref, l0 * cover, n_photo);
                if dormant {
                    u *= crate::root::DROUGHT_DORMANT_UPKEEP;
                }
                u
            };
            // Root tissue maintenance — must stay ≪ photo or deep-rooted
            // trees starve overnight on wet fertile ground (see ROOT_UPKEEP).
            // Scales down at night with basal respiration (woody roots don't
            // burn day-rate sugar in the dark).
            let mut root_tax = if rooted && !dormant {
                crate::root::ROOT_UPKEEP_PER_MODULE * body.blueprint.root_count() as f32
            } else {
                0.0
            };
            if l0 < 0.1 {
                root_tax *= NIGHT_UPKEEP_MULT;
                if fungus {
                    upkeep *= NIGHT_UPKEEP_MULT;
                }
            }
            energy.current -= upkeep + root_tax;
            if energy.current <= 0.0 {
                deaths.push((e, wx));
                continue;
            }

            // Coarse ecology feedback — raise toward cover, don't stack every tick
            // (stacking drove leaf_area→1 and flash-dried soil via ET).
            if rooted {
                if let Some(col) = world.column_at_mut(wx) {
                    let roots = body.blueprint.root_count() as f32;
                    let leaves = body.photosystem_count() as f32;
                    let cover_r = (roots * 0.08).clamp(0.0, 1.0);
                    let cover_l = if dormant {
                        (leaves * 0.04).clamp(0.0, 0.35) // wilted canopy
                    } else {
                        (leaves * 0.12).clamp(0.0, 1.0)
                    };
                    col.ecology.root_density = col.ecology.root_density.max(cover_r);
                    col.ecology.leaf_area = if dormant {
                        col.ecology.leaf_area.min(cover_l.max(0.05))
                    } else {
                        col.ecology.leaf_area.max(cover_l)
                    };
                }
            }

            // Vegetative sprouts / fission / spores BEFORE elongation so growth
            // can't permanently spend the tank below the repro gate.
            let repro_period = if fungus {
                crate::fungi::FUNGUS_SPORE_PERIOD
            } else if rooted {
                crate::root::LAND_SPROUT_PERIOD
            } else {
                REPRO_PERIOD
            };
            let phase_id = e.id() as u64 % repro_period;
            let threshold = if fungus {
                genome
                    .reproduce_at
                    .clamp(0.25, 0.95)
                    .min(crate::fungi::FUNGUS_SPORE_ENERGY_FRAC)
            } else if rooted {
                genome
                    .reproduce_at
                    .clamp(0.25, 0.95)
                    .min(crate::root::LAND_SPROUT_ENERGY_FRAC)
            } else {
                genome.reproduce_at.clamp(0.2, 0.99)
            };
            // Plankton: comfort + circadian. Land suckers need roots first.
            // Fungi: spore when active, fed, and not dormant.
            let can_repro = if fungus {
                !dormant && comfort >= 0.05
            } else if rooted {
                plant_active
                    && comfort >= 0.05
                    && body.blueprint.root_count() >= crate::root::LAND_SPROUT_MIN_ROOTS
            } else {
                plant_active && comfort >= 0.20
            };
            let parent_habit = OrganismHabit::from_blueprint(&body.blueprint);
            let pending_same = births
                .iter()
                .filter(|b| OrganismHabit::from_blueprint(&b.2) == parent_habit)
                .count();
            let habit_n0 = match parent_habit {
                OrganismHabit::Algae => n_algae0,
                OrganismHabit::Plant => n_plant0,
                OrganismHabit::Fungus => n_fungus0,
            };
            let habit_cap = pop_caps.for_habit(parent_habit);
            let habit_room = habit_n0 + pending_same < habit_cap
                && population + births.len() < MAX_ORGANISMS;
            if habit_room
                && tick % repro_period == phase_id
                && energy.current >= tank_ref * threshold
                && can_repro
            {
                // Land: runner sucker. Fungi: spore to nearby litter. Plankton: fission.
                let sprout_x = if rooted {
                    crate::root::pick_root_sprout_x(
                        world,
                        pose.x,
                        pose.y,
                        &body.blueprint,
                        world.seed,
                        tick,
                        e.id(),
                    )
                } else if fungus {
                    crate::fungi::pick_spore_site(world, pose.x, world.seed, tick, e.id())
                } else {
                    let w = organism_width(&body.blueprint);
                    let side = if (tick + e.id() as u64) % 2 == 0 {
                        w
                    } else {
                        -w
                    };
                    Some(pose.x + side)
                };
                if let Some(child_x0) = sprout_x {
                    // Low CloneFidelity → weaker kids, and density-dependent
                    // stillbirths (messy booms then fails to replace itself at cap).
                    // Density is per-habit so an algae bloom doesn't sterilize plants.
                    let fidelity = genome.clone_fidelity.clamp(0.0, 1.0);
                    let density = ((habit_n0 + pending_same) as f32
                        / habit_cap.max(1) as f32)
                        .clamp(0.0, 1.0);
                    let viability =
                        (fidelity + (1.0 - density) * (1.0 - fidelity)).clamp(0.05, 1.0);
                    let viab_h = hash_u64(world.seed, tick as i64, e.id() as i64, 0xB100_D5);
                    let viable = (viab_h as f32 / u64::MAX as f32) < viability;
                    let child_frac = REPRO_COST_FRAC * (0.35 + 0.65 * fidelity);
                    let mut child_e = energy.current * child_frac;
                    // Land / fungus parents keep a reserve after paying.
                    // Land retain uses base tank so root-store surplus can fund
                    // the sprout payment and still leave bore headroom.
                    if rooted || fungus {
                        let retain = tank_ref
                            * if fungus {
                                0.25
                            } else {
                                crate::root::LAND_GROW_ENERGY_FRAC
                            };
                        if energy.current - child_e < retain {
                            child_e = (energy.current - retain).max(0.0);
                        }
                    }
                    if child_e > 1.0 {
                        // Parent always pays the attempt; failed clones are the
                        // messy cost of low fidelity under crowding.
                        energy.current -= child_e;
                        if viable {
                            let child_genome =
                                mutate_organism(*genome, world.seed, tick, e.id());
                            // Same clone pipeline: gene jitter + morphology mut.
                            // Fungi morph targets Hypha; plants/algae Photosystem.
                            let mut child_bp = if fungus {
                                mutate_blueprint_fungus_morphology(
                                    body.blueprint.clone(),
                                    genome.clone_fidelity,
                                    world.seed,
                                    tick,
                                    e.id(),
                                )
                            } else {
                                mutate_blueprint_morphology(
                                    body.blueprint.clone(),
                                    genome.clone_fidelity,
                                    world.seed,
                                    tick,
                                    e.id(),
                                )
                            };
                            child_bp.genome = child_genome;
                            if rooted {
                                child_bp.name = format!("{}-sprout", body.blueprint.name);
                            } else if fungus {
                                child_bp.name = format!("{}-spore", body.blueprint.name);
                            }
                            births.push((
                                child_x0,
                                pose.y,
                                child_bp,
                                child_e,
                                tank_ref,
                                e,
                                lineage.generation,
                                lineage.founder_id,
                                lineage.genet_id,
                            ));
                        }
                    }
                }
            }

            // Surplus above the *growth* floor → elongate. Sprout gate is
            // higher, so roots deepen while the tank banks toward a sucker.
            let growth_reserve = if rooted {
                crate::root::growth_energy_floor(tank_ref)
            } else {
                0.0
            };
            let bore_headroom = crate::root::ROOT_ELONGATE_BASE_COST * 2.0;
            if rooted
                && plant_active
                && energy.current >= growth_reserve + bore_headroom
                && tick % crate::root::ROOT_GROW_PERIOD
                    == (e.id() as u64 % crate::root::ROOT_GROW_PERIOD)
            {
                let (_, _, w_root) = genome.alloc_weights();
                // Shoot a horizontal runner before suckering — sprouts only
                // emerge from painted lateral Root tips.
                let need_runner = !crate::root::has_lateral_runner(pose.x, &body.blueprint)
                    && body.blueprint.root_count()
                        >= crate::root::LAND_SPROUT_MIN_ROOTS.saturating_sub(1);
                let roots_ample =
                    crate::root::roots_past_soft_budget_for(&body.blueprint, drought);
                // Prefer canopy once the soft root budget is met — otherwise
                // high alloc_root genomes bore until night upkeep kills them.
                // Drought lifts the budget so storage / deep water can pay.
                let grow_roots = (w_root >= 0.2 || need_runner) && (!roots_ample || need_runner);
                if grow_roots {
                    let _ = crate::root::try_elongate_root(
                        &mut body.blueprint,
                        energy,
                        world,
                        pose.x,
                        pose.y,
                        genome,
                        tank_ref,
                        world.seed,
                        tick,
                        e.id(),
                    );
                } else {
                    let _ = crate::root::try_grow_shoot(
                        &mut body.blueprint,
                        energy,
                        genome,
                        tank_ref,
                        world.seed,
                        tick,
                        e.id(),
                    );
                }
                // Occasional opposite allocation so plants aren't pure roots.
                if energy.current >= growth_reserve + bore_headroom
                    && tick % (crate::root::ROOT_GROW_PERIOD * 3)
                        == (e.id() as u64 % (crate::root::ROOT_GROW_PERIOD * 3))
                {
                    let _ = crate::root::try_grow_shoot(
                        &mut body.blueprint,
                        energy,
                        genome,
                        tank_ref,
                        world.seed,
                        tick,
                        e.id(),
                    );
                }
            }

            if let Some(col) = world.column_at_mut(wx) {
                col.activity = Activity::HydrologyActive;
            }
        }

        // Energy death → corpse (keeps drawing, sinks), litter on dissolve.
        for (e, wx) in deaths {
            // Standing dead shouldn't keep full canopy ET / shade cover.
            if let Some(col) = world.column_at_mut(wx) {
                col.ecology.leaf_area *= 0.15;
                col.ecology.root_density *= 0.35;
            }
            let _ = self.ecs.remove_one::<Organism>(e);
            let _ = self.ecs.insert_one(
                e,
                Corpse {
                    ticks: 0,
                    settled_ticks: 0,
                },
            );
            if let Ok(mut buoy) = self.ecs.get::<&mut BuoyancyState>(e) {
                // Dead tissue is heavy — start a downward drift.
                buoy.vel_y = -0.15;
            }
            if let Ok(mut energy) = self.ecs.get::<&mut Energy>(e) {
                energy.current = 0.0;
            }
            // Switch from float-line (top) anchoring to bed (bottom) anchoring
            // without teleporting the crest: living pose was `float - extent`,
            // so the feet are already at `pose.y`; corpse buoyancy uses
            // extent=0 and sinks those feet down to the bed.
        }

        // Collect once, extend as we spawn — the old code re-queried every
        // living organism through hecs per birth (O(N²) at MAX_ORGANISMS).
        let mut bodies = if !births.is_empty() {
            collect_bodies(self, world)
        } else {
            Vec::new()
        };
        let mut occupied = rooted_occupied_columns(&bodies);
        for (x0, y0, blueprint, energy, max_e, parent, parent_gen, founder_id, genet_id) in births
        {
            if self.organism_count() >= MAX_ORGANISMS {
                break;
            }
            let habit = OrganismHabit::from_blueprint(&blueprint);
            if !self.habit_has_room(habit, 0, 0) {
                if let Ok(mut e) = self.ecs.get::<&mut Energy>(parent) {
                    e.current = (e.current + energy).min(e.max);
                }
                continue;
            }
            let placed = if blueprint.is_rooted() || blueprint.is_fungus() {
                find_land_sprout_pose(world, &occupied, &blueprint, x0)
            } else {
                find_clear_pose_limited(world, &bodies, &blueprint, x0, y0, None)
            };
            let Some((x, y)) = placed else {
                // Failed seat — refund the parent so cliffs don't drain trees.
                if let Ok(mut e) = self.ecs.get::<&mut Energy>(parent) {
                    e.current = (e.current + energy).min(e.max);
                }
                continue;
            };
            let host_x = x.floor() as i32;
            let last_water_top = world
                .column_at(host_x)
                .and_then(water_band)
                .map(|(top, _)| top);
            let new_pose = Pose { x, y };
            let new_aabb = organism_aabb(&new_pose, &blueprint);
            let immobile = blueprint.is_rooted() || blueprint.is_fungus();
            if immobile {
                occupied.insert(x.floor() as i32);
            }
            // Land sprouts inherit the parent's genet; assign a fresh id if
            // the parent somehow lacked one (shouldn't happen after ensure).
            let child_genet = if blueprint.is_rooted() {
                if genet_id != 0 {
                    genet_id
                } else {
                    parent.id().saturating_add(1)
                }
            } else {
                0
            };
            let new_entity = self.ecs.spawn((
                new_pose,
                BuoyancyState {
                    vel_y: 0.0,
                    last_water_top,
                },
                Energy {
                    current: energy.clamp(0.0, max_e),
                    max: max_e,
                },
                blueprint.genome,
                ModuleBody { blueprint },
                Lineage {
                    generation: parent_gen.saturating_add(1),
                    clones_produced: 0,
                    age_ticks: 0,
                    founder_id,
                    genet_id: child_genet,
                },
                Organism {
                    energy_base_max: max_e,
                    ..Organism::default()
                },
            ));
            bodies.push((new_entity, new_aabb, immobile));
            if let Ok(mut lin) = self.ecs.get::<&mut Lineage>(parent) {
                lin.clones_produced = lin.clones_produced.saturating_add(1);
            }
            self.births_total += 1;
        }

        // After buoyancy + births: separate any overlapping footprints.
        resolve_collisions(self, world);
        self.step_corpses(world, tick);

        self.wake_host_columns(world);
    }

    /// Sink corpses to the bed (or rest on dry land); after a long settle,
    /// deposit Organic sediment (and a little ecology litter). Bodies stay
    /// visible for minutes so blooms leave a carpet before becoming ooze.
    fn step_corpses(&mut self, world: &mut World, tick: u64) {
        let mut dissolve: Vec<(Entity, i32, usize)> = Vec::new();
        for (e, (pose, buoy, corpse, body)) in self
            .ecs
            .query::<(&mut Pose, &mut BuoyancyState, &mut Corpse, &ModuleBody)>()
            .iter()
        {
            corpse.ticks = corpse.ticks.saturating_add(1);
            let wx = pose.world_x();
            let n_modules = body.blueprint.modules.len().max(1);
            let Some(col) = world.column_at(wx) else {
                dissolve.push((e, wx, n_modules));
                continue;
            };
            // Dead bodies are bottom-anchored (editor y=0 on the bed), unlike
            // living floaters whose tallest module sits on the float line.
            // Passing the float offset here used to park pose at `bed - extent`
            // or leave the crest sticking into the air in shallow water.
            let extent = blueprint_body_top_offset(&body.blueprint);
            if body.blueprint.is_plankton() || water_band(col).is_some() {
                step_buoyancy(&mut pose.y, buoy, col, 1.0, 0.0);
                buoy.vel_y -= 0.04; // extra dead-weight pull
                if let Some((top, bed)) = water_band(col) {
                    // Keep the crest at/under the free surface when the body
                    // is taller than the water column (bottom may sit a bit
                    // into the bed — better than a tower in the sky).
                    if pose.y + extent > top {
                        pose.y = (top - extent).min(bed);
                    }
                    if pose.y < bed - extent {
                        pose.y = bed - extent;
                    }
                }
            } else {
                // Dry land: crown on the solid bed (same anchor as living plants).
                pose.y = land_plant_pose_y_on(col, &body.blueprint);
                buoy.vel_y = 0.0;
            }

            let on_bed = match water_band(col) {
                Some((top, bed)) => {
                    let deep = pose.y <= bed + 0.15;
                    // Shallow: already as low as crest-at-surface allows.
                    let shallow = extent > (top - bed) - 0.2
                        && pose.y <= (top - extent) + 0.15;
                    deep || shallow
                }
                None => true,
            };
            if on_bed {
                buoy.vel_y = 0.0;
                corpse.settled_ticks = corpse.settled_ticks.saturating_add(1);
                let land_corpse = (body.blueprint.is_rooted() || body.blueprint.is_fungus())
                    && water_band(col).is_none();
                let settle_need = if land_corpse {
                    CORPSE_SETTLE_LAND_TICKS
                } else {
                    CORPSE_SETTLE_TICKS
                };
                if corpse.settled_ticks >= settle_need {
                    dissolve.push((e, wx, n_modules));
                }
            } else {
                corpse.settled_ticks = 0;
            }
        }

        for (e, wx, n_modules) in dissolve {
            let n = n_modules.max(1) as i64;
            let organic_kg = DEATH_ORGANIC_KG_PER_MODULE
                .saturating_mul(n)
                .min(DEATH_ORGANIC_KG_MAX);
            let litter_kg = DEATH_LITTER_KG_PER_MODULE.saturating_mul(n);
            let mut displaced_water = 0i64;
            if let Some(col) = world.column_at_mut(wx) {
                // Settled Organic under water displaces free-surface mass
                // (avoids the beaver-dam water spike). Dry land still piles.
                if organic_kg > 0 {
                    displaced_water =
                        col.deposit_sediment_settled(MaterialId::Organic, organic_kg, tick);
                }
                col.ecology.dead_biomass =
                    col.ecology.dead_biomass.saturating_add(litter_kg);
                col.activity = Activity::HydrologyActive;
            }
            // Spill displaced lake water into neighbouring wet/low columns
            // so mass is conserved and the sill doesn't keep a spike.
            if displaced_water > 0 {
                spill_displaced_water(world, wx, displaced_water, tick);
            }
            // Creatures aren't in the mass audit; booking the deposit as
            // biomass growth keeps conservation bookkeeping balanced.
            world.mass_audit.biomass_grow_total = world
                .mass_audit
                .biomass_grow_total
                .saturating_add(organic_kg.saturating_add(litter_kg));
            let _ = self.ecs.despawn(e);
        }
    }

    /// Collect drawable module quads for the renderer (living + grey corpses).
    /// Returns (world_x_frac, world_y_m, module_rgb).
    pub fn organism_draw_list(&self) -> Vec<(f32, f32, (u8, u8, u8))> {
        let mut out = Vec::new();
        let mut push_body = |pose: &Pose, body: &ModuleBody, dead: bool| {
            let modules = &body.blueprint.modules;
            if modules.is_empty() {
                return;
            }
            let min_x = modules.iter().map(|m| m.x).min().unwrap_or(0);
            let max_x = modules.iter().map(|m| m.x).max().unwrap_or(0);
            let mid_x = (min_x as f32 + max_x as f32) * 0.5;
            let host = pose.x.floor() + 0.5;
            let rooted = body.blueprint.is_rooted();
            for m in modules {
                let mut wx = pose.x + (m.x as f32 - mid_x) * MODULE_CELL_COLS;
                // Keep land-plant pixels inside the host column so cliff
                // faces don't show stems floating in open air beside the wall.
                if rooted {
                    wx = wx.clamp(host - 0.45, host + 0.45);
                }
                let wy = pose.y + m.y as f32 * MODULE_CELL_COLS;
                let rgb = if dead {
                    corpse_rgb(m.module.rgb())
                } else {
                    m.module.rgb()
                };
                out.push((wx, wy, rgb));
            }
        };
        for (_, (pose, body, _)) in self
            .ecs
            .query::<(&Pose, &ModuleBody, &Organism)>()
            .iter()
        {
            push_body(pose, body, false);
        }
        for (_, (pose, body, _)) in self.ecs.query::<(&Pose, &ModuleBody, &Corpse)>().iter() {
            push_body(pose, body, true);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::PlacedModule;
    use crate::module::{LaneId, ModuleId};
    use wk_world::column::{EQUIL_WATER_CO2, EQUIL_WATER_O2};
    use wk_world::terrain::generate_flat_sand;
    use wk_world::world::World;

    fn world_with_water() -> World {
        let mut world = World::new(42);
        world.sea_level = 10.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 5.0));
        if let Some(col) = world.column_at_mut(8) {
            col.deposit_to_top(MaterialId::Water, 2_000, 0);
        }
        world
    }

    #[test]
    fn faster_metabolism_shortens_life() {
        let slow = Genome {
            metabolic_rate: 0.2,
            ..Genome::default()
        };
        let fast = Genome {
            metabolic_rate: 1.0,
            ..Genome::default()
        };
        assert!(life_expectancy_ticks(&fast) < life_expectancy_ticks(&slow));
        let def = life_expectancy_ticks(&Genome::default());
        // ~3 sim-days ballpark for default algae.
        assert!(def > 150_000 && def < 400_000, "default life={def}");
    }

    #[test]
    fn senescence_kills_and_leaves_corpse() {
        let mut world = World::new(99);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        if let Some(col) = world.column_at_mut(4) {
            col.deposit_to_top(MaterialId::Water, 2_000, 0);
        }
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(&world, 4, Blueprint::atom(Genome::default()), 50.0)
            .expect("spawn");
        {
            let mut lin = store.ecs.get::<&mut Lineage>(e).unwrap();
            lin.age_ticks = life_limit_ticks(&Genome::default(), e.id());
        }
        store.step_organisms(&mut world, 1);
        assert_eq!(store.organism_count(), 0);
        assert_eq!(store.corpse_count(), 1);
    }

    #[test]
    fn plankton_dies_without_water() {
        let mut world = World::new(101);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        // Dry column — no standing water.
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(&world, 4, Blueprint::atom(Genome::default()), 50.0)
            .expect("spawn");
        assert!(store.ecs.get::<&Organism>(e).is_ok());
        store.step_organisms(&mut world, 1);
        assert_eq!(store.organism_count(), 0, "dry land should kill plankton");
        assert_eq!(store.corpse_count(), 1);
    }

    #[test]
    fn plankton_dies_under_ice_cap() {
        let mut world = World::new(102);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 18.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        if let Some(col) = world.column_at_mut(4) {
            col.deposit_to_top(MaterialId::Water, 2_000, 0);
            col.deposit_to_top(MaterialId::Ice, 400, 1);
            col.settle_by_density(2);
            assert!(col.top_ice_mass() > 0, "ice must float as top cap");
            assert!(water_band(col).is_some(), "water still under ice");
        }
        let mut store = AgentStore::new();
        store
            .spawn_from_blueprint(&world, 4, Blueprint::atom(Genome::default()), 50.0)
            .expect("spawn");
        store.step_organisms(&mut world, 1);
        assert_eq!(store.organism_count(), 0, "ice should kill plankton");
        assert_eq!(store.corpse_count(), 1);
    }

    #[test]
    fn circadian_buoyancy_floats_by_day_sinks_by_night() {
        let g = Genome {
            buoyancy_bias: 0.0,
            circadian_phase: 0.25,
            active_window: 0.55,
            ..Genome::default()
        };
        let day = circadian_buoyancy_bias(&g, 0.25);
        let night = circadian_buoyancy_bias(&g, 0.75);
        assert!(day < 0.2, "day bias should be near surface (got {day})");
        assert!(night > 0.5, "night bias should be deeper (got {night})");
        assert!(night > day + 0.3);
    }

    #[test]
    fn temp_comfort_blocks_cold_and_hot() {
        let g = Genome {
            temp_optimum: 18.0,
            temp_width: 10.0,
            ..Genome::default()
        };
        assert!(temp_comfort_factor(18.0, &g) > 0.95);
        assert!(
            temp_comfort_factor(-5.0, &g) < 0.20,
            "cold should be below repro threshold"
        );
        assert!(
            temp_comfort_factor(45.0, &g) < 0.20,
            "hot should be below repro threshold"
        );
    }

    #[test]
    fn cold_climate_blocks_plankton_reproduction() {
        let mut world = World::new(103);
        world.sea_level = 0.0;
        world.climate.base_temp_c = -8.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_length_ticks = 10_000;
        world.climate.night_length_ticks = 1;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.deposit_to_top(MaterialId::Water, 2_000, 0);
            }
        }
        let genome = Genome {
            metabolic_rate: 0.1,
            reproduce_at: 0.4,
            temp_optimum: 18.0,
            temp_width: 8.0,
            active_window: 1.0,
            circadian_phase: 0.0,
            ..Genome::default()
        };
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(&world, 8, Blueprint::atom(genome), 50.0)
            .expect("spawn");
        // Keep energy topped so only temperature can block fission.
        for tick in 0..400 {
            if let Ok(mut energy) = store.ecs.get::<&mut Energy>(e) {
                energy.current = energy.max;
            }
            store.step_organisms(&mut world, tick);
        }
        assert_eq!(
            store.births_total, 0,
            "cold water must block fission (comfort < 0.20)"
        );
        assert_eq!(store.organism_count(), 1);
    }

    #[test]
    fn photo_drawdown_lowers_dissolved_co2() {
        let mut world = World::new(104);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 18.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_length_ticks = 10_000;
        world.climate.night_length_ticks = 1;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.deposit_to_top(MaterialId::Water, 2_000, 0);
                col.ecology.water_co2 = EQUIL_WATER_CO2;
                col.ecology.water_o2 = EQUIL_WATER_O2;
            }
        }
        let co2_before: f32 = (0..64)
            .map(|x| world.column_at(x).unwrap().ecology.water_co2)
            .sum::<f32>()
            / 64.0;

        let genome = Genome {
            metabolic_rate: 0.15,
            reproduce_at: 0.99, // no fission — pure drawdown
            temp_optimum: 18.0,
            temp_width: 20.0,
            active_window: 1.0,
            circadian_phase: 0.0,
            ..Genome::default()
        };
        let mut store = AgentStore::new();
        for x in 2..62 {
            let _ = store.spawn_from_blueprint(&world, x, Blueprint::atom(genome), 50.0);
        }
        let n = store.organism_count();
        assert!(n >= 24, "need a dense patch for drawdown, got {n}");

        // Daylight ticks only — no gas exchange here (AgentStore path).
        for tick in 0..200 {
            store.step_organisms(&mut world, tick);
        }
        let co2_min = (0..64)
            .map(|x| world.column_at(x).unwrap().ecology.water_co2)
            .fold(f32::INFINITY, f32::min);
        let o2_max = (0..64)
            .map(|x| world.column_at(x).unwrap().ecology.water_o2)
            .fold(0.0f32, f32::max);
        assert!(
            co2_min < co2_before - 0.15,
            "bloom should draw CO₂ hard in occupied columns (before={co2_before:.3} min={co2_min:.3})"
        );
        assert!(
            o2_max > EQUIL_WATER_O2 + 0.08,
            "photosynthesis should emit O₂ (o2_max={o2_max:.3})"
        );
    }

    #[test]
    fn full_tank_outlasts_default_night() {
        let g = Genome {
            metabolic_rate: 0.35,
            ..Genome::default()
        };
        let max = 50.0;
        let mut e = max;
        // Default climate night = 10h * 60 * 60 ticks.
        for _ in 0..36_000 {
            e -= organism_upkeep(&g, max, 0.0, 1.0);
        }
        assert!(
            e > 5.0,
            "algae should still have reserves after a night (left={e})"
        );
    }

    #[test]
    fn corpse_sinks_instead_of_vanishing() {
        let mut world = world_with_water();
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(
                &world,
                8,
                Blueprint::atom(Genome {
                    buoyancy_bias: 0.0,
                    ..Genome::default()
                }),
                50.0,
            )
            .expect("spawn");
        let y0 = store.ecs.get::<&Pose>(e).unwrap().y;
        // Force energy death on next step.
        if let Ok(mut energy) = store.ecs.get::<&mut Energy>(e) {
            energy.current = 0.0;
        }
        // Manually convert like step_organisms death path.
        let _ = store.ecs.remove_one::<Organism>(e);
        let _ = store.ecs.insert_one(
            e,
            Corpse {
                ticks: 0,
                settled_ticks: 0,
            },
        );
        for _ in 0..40 {
            store.step_corpses(&mut world, 0);
        }
        assert!(store.ecs.get::<&Corpse>(e).is_ok(), "corpse should remain");
        let y1 = store.ecs.get::<&Pose>(e).unwrap().y;
        assert!(y1 < y0 - 0.2, "corpse should sink (y0={y0} y1={y1})");
        assert_eq!(store.organism_count(), 0);
        assert_eq!(store.corpse_count(), 1);
    }

    #[test]
    fn tall_corpse_crest_stays_out_of_the_air() {
        // Living floaters top-anchor; corpses must bottom-anchor + clamp so a
        // body taller than the water column doesn't stick into the sky.
        let mut world = World::new(404);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        // Shallow puddle — deeper than FLOAT_DEPTH but shallower than a
        // y=12 editor paint (~5.4 m).
        if let Some(col) = world.column_at_mut(8) {
            col.deposit_to_top(MaterialId::Water, 600, 0); // ~2.4 m
        }
        let mut bp = Blueprint::atom(Genome::default());
        bp.modules.push(crate::blueprint::PlacedModule {
            x: 0,
            y: 12,
            lane: crate::LaneId::Mid,
            module: crate::ModuleId::Photosystem,
        });
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(&world, 8, bp, 50.0)
            .expect("spawn");
        let extent = {
            let body = store.ecs.get::<&ModuleBody>(e).unwrap();
            blueprint_body_top_offset(&body.blueprint)
        };
        let _ = store.ecs.remove_one::<Organism>(e);
        let _ = store.ecs.insert_one(
            e,
            Corpse {
                ticks: 0,
                settled_ticks: 0,
            },
        );
        for _ in 0..80 {
            store.step_corpses(&mut world, 0);
        }
        let pose_y = store.ecs.get::<&Pose>(e).unwrap().y;
        let col = world.column_at(8).unwrap();
        let (top, bed) = water_band(col).unwrap();
        let crest = pose_y + extent;
        assert!(
            crest <= top + 0.2,
            "corpse crest must not float in air (crest={crest:.2} top={top:.2} pose={pose_y:.2} bed={bed:.2} extent={extent:.2})"
        );
        assert!(
            store.ecs.get::<&Corpse>(e).is_ok(),
            "corpse should still be settling"
        );
    }

    #[test]
    fn corpse_becomes_organic_sediment_after_settle() {
        let mut world = world_with_water();
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(&world, 8, Blueprint::atom(Genome::default()), 50.0)
            .expect("spawn");
        let _ = store.ecs.remove_one::<Organism>(e);
        let _ = store.ecs.insert_one(
            e,
            Corpse {
                ticks: 0,
                settled_ticks: CORPSE_SETTLE_TICKS, // already rested
            },
        );
        // Park on the bed so dissolve fires this tick.
        if let (Ok(mut pose), Some(col)) = (store.ecs.get::<&mut Pose>(e), world.column_at(8)) {
            let (_top, bed) = water_band(col).unwrap();
            pose.y = bed;
        }
        let organic_before = world
            .column_at(8)
            .map(|c| {
                (0..c.layer_count as usize)
                    .filter(|&i| c.layers[i].material == MaterialId::Organic)
                    .map(|i| c.layers[i].thickness)
                    .sum::<i64>()
            })
            .unwrap_or(0);
        store.step_corpses(&mut world, 99);
        assert_eq!(store.corpse_count(), 0, "corpse should have dissolved");
        let organic_after = world
            .column_at(8)
            .map(|c| {
                (0..c.layer_count as usize)
                    .filter(|&i| c.layers[i].material == MaterialId::Organic)
                    .map(|i| c.layers[i].thickness)
                    .sum::<i64>()
            })
            .unwrap_or(0);
        let expected = DEATH_ORGANIC_KG_PER_MODULE * 2; // Atom = nucleus + photosystem
        assert!(
            organic_after >= organic_before + expected,
            "expected Organic sediment ≥{expected}, before={organic_before} after={organic_after}"
        );
        // Organic must sit under water (denser waterlogged detritus).
        let col = world.column_at(8).unwrap();
        let top = col.layers[0].material;
        assert_eq!(top, MaterialId::Water, "water should remain on top");
        assert!(
            (0..col.layer_count as usize).any(|i| col.layers[i].material == MaterialId::Organic),
            "organic layer present in stack"
        );
    }

    #[test]
    fn corpse_dissolve_under_water_does_not_spike_surface() {
        let mut world = World::new(42);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        // Deep puddle on the host; wet neighbour receives displaced spill.
        for x in [7, 8, 9] {
            if let Some(col) = world.column_at_mut(x) {
                col.deposit_to_top(MaterialId::Water, 3_000, 0);
            }
        }
        let surface_before = world.column_at(8).unwrap().surface_y;
        let mut store = AgentStore::new();
        // Many-module floater → hits DEATH_ORGANIC_KG_MAX (~0.35 m bed rise).
        let mut bp = Blueprint::atom(Genome::default());
        for y in 1i16..=20 {
            bp.modules.push(crate::blueprint::PlacedModule {
                x: 0,
                y,
                lane: crate::module::LaneId::Mid,
                module: crate::module::ModuleId::Photosystem,
            });
        }
        let e = store
            .spawn_from_blueprint(&world, 8, bp, 80.0)
            .expect("spawn");
        let _ = store.ecs.remove_one::<Organism>(e);
        let _ = store.ecs.insert_one(
            e,
            Corpse {
                ticks: 0,
                settled_ticks: CORPSE_SETTLE_TICKS,
            },
        );
        if let (Ok(mut pose), Some(col)) = (store.ecs.get::<&mut Pose>(e), world.column_at(8)) {
            let (_top, bed) = water_band(col).unwrap();
            pose.y = bed;
        }
        store.step_corpses(&mut world, 50);
        assert_eq!(store.corpse_count(), 0);
        let surface_after = world.column_at(8).unwrap().surface_y;
        assert!(
            (surface_after - surface_before).abs() < 0.15,
            "corpse ooze must not spike the free surface (before={surface_before:.3} after={surface_after:.3})"
        );
    }

    #[test]
    fn corpse_on_dry_land_becomes_organic_sediment() {
        let mut world = World::new(42);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        // Dry coast: no water layer.
        assert!(
            world
                .column_at(8)
                .map(|c| !c.layers[..c.layer_count as usize]
                    .iter()
                    .any(|l| l.material == MaterialId::Water))
                .unwrap_or(false),
            "fixture must be dry land"
        );
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(&world, 8, Blueprint::atom(Genome::default()), 50.0)
            .expect("spawn");
        let _ = store.ecs.remove_one::<Organism>(e);
        let _ = store.ecs.insert_one(
            e,
            Corpse {
                ticks: 0,
                settled_ticks: CORPSE_SETTLE_TICKS,
            },
        );
        if let (Ok(mut pose), Some(col)) = (store.ecs.get::<&mut Pose>(e), world.column_at(8)) {
            pose.y = col.surface_y;
        }
        let organic_before = world
            .column_at(8)
            .map(|c| {
                (0..c.layer_count as usize)
                    .filter(|&i| c.layers[i].material == MaterialId::Organic)
                    .map(|i| c.layers[i].thickness)
                    .sum::<i64>()
            })
            .unwrap_or(0);
        store.step_corpses(&mut world, 77);
        assert_eq!(store.corpse_count(), 0, "land corpse should dissolve");
        let col = world.column_at(8).unwrap();
        let organic_after: i64 = (0..col.layer_count as usize)
            .filter(|&i| col.layers[i].material == MaterialId::Organic)
            .map(|i| col.layers[i].thickness)
            .sum();
        let expected = DEATH_ORGANIC_KG_PER_MODULE * 2;
        assert!(
            organic_after >= organic_before + expected,
            "land Organic ≥{expected}, before={organic_before} after={organic_after}"
        );
        // Lighter than sand → Organic sits as the top solid layer.
        assert_eq!(
            col.layers[0].material,
            MaterialId::Organic,
            "organic should cap the dry land stack"
        );
    }

    #[test]
    fn mid_canvas_plant_spawns_with_crown_on_ground() {
        // Same shape as a typical editor paint: plant drawn mid-grid, not on y=0.
        let mut world = World::new(3);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
        let mut bp = Blueprint::empty("hover");
        bp.modules = vec![
            crate::blueprint::PlacedModule {
                x: 0,
                y: 6,
                lane: crate::LaneId::Mid,
                module: crate::ModuleId::Photosystem,
            },
            crate::blueprint::PlacedModule {
                x: 1,
                y: 6,
                lane: crate::LaneId::Mid,
                module: crate::ModuleId::Photosystem,
            },
            crate::blueprint::PlacedModule {
                x: 0,
                y: 5,
                lane: crate::LaneId::Mid,
                module: crate::ModuleId::Stem,
            },
            crate::blueprint::PlacedModule {
                x: 0,
                y: 4,
                lane: crate::LaneId::Mid,
                module: crate::ModuleId::Nucleus,
            },
            crate::blueprint::PlacedModule {
                x: 0,
                y: 3,
                lane: crate::LaneId::Mid,
                module: crate::ModuleId::Root,
            },
        ];
        bp.genome = Genome::default();
        let surface = world.column_at(4).unwrap().surface_y;
        let pose_y = AgentStore::spawn_elevation(&world, 4, &bp).unwrap();
        // Nucleus at blueprint y=4 must sit on the surface.
        let nucleus_wy = pose_y + 4.0 * MODULE_CELL_COLS;
        assert!(
            (nucleus_wy - surface).abs() < 0.02,
            "nucleus should be on ground (wy={nucleus_wy} surface={surface} pose={pose_y})"
        );
        let root_wy = pose_y + 3.0 * MODULE_CELL_COLS;
        assert!(
            root_wy < surface - 0.1,
            "root should be below ground (root={root_wy} surface={surface})"
        );
        let leaf_wy = pose_y + 6.0 * MODULE_CELL_COLS;
        assert!(
            leaf_wy > surface + 0.5,
            "leaves should be above ground (leaf={leaf_wy} surface={surface})"
        );
    }

    #[test]
    fn land_plant_spawns_on_surface_and_drinks_moisture() {
        let mut world = World::new(3);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        if let Some(col) = world.column_at_mut(2) {
            col.moisture = 800;
            col.ecology.nutrient = 0.6;
            col.deposit_to_top(MaterialId::Organic, 300, 0);
        }
        let mut store = AgentStore::new();
        let bp = Blueprint::minimal_plant(Genome {
            alloc_root: 0.7,
            root_depth_bias: 0.8,
            ..Genome::default()
        });
        assert!(bp.is_rooted());
        assert!(!bp.is_plankton());
        let e = store
            .spawn_from_blueprint(&world, 2, bp, 80.0)
            .expect("spawn plant");
        let y0 = store.ecs.get::<&Pose>(e).unwrap().y;
        let bed = world.column_at(2).unwrap().climate_elevation();
        assert!(
            (y0 - bed).abs() < 0.05,
            "plant crown on solid bed y0={y0} bed={bed}"
        );
        let moist_before = world.column_at(2).unwrap().moisture;
        // Fractional sip needs ~1/ROOT_SIP ticks to take 1 kg; run long enough
        // to observe a drink without emptying the column.
        for t in 0..8_000 {
            store.step_organisms(&mut world, t);
        }
        let moist_after = world.column_at(2).unwrap().moisture;
        assert!(
            moist_after < moist_before,
            "roots should drink host moisture ({moist_before} → {moist_after})"
        );
        // Must leave the bulk of the aquifer — recharge should win.
        assert!(
            moist_after > moist_before * 3 / 4,
            "sip too aggressive ({moist_before} → {moist_after})"
        );
        let info = store.inspect_organism(e).expect("plant still alive");
        assert!(!info.is_plankton);
        assert!(info.roots >= 1);
    }

    #[test]
    fn land_plant_can_spawn_in_cliff_notch() {
        let mut world = World::new(21);
        world.sea_level = 0.0;
        // U-notch: high | low | high — placement is allowed (won't thrive well).
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
            }
        }
        for x in [9, 11] {
            if let Some(col) = world.column_at_mut(x) {
                col.deposit_to_top(MaterialId::Stone, 80_000, 0);
            }
        }
        assert!(crate::root::column_is_plantable(&world, 10));
        let mut store = AgentStore::new();
        assert!(
            store
                .spawn_from_blueprint(
                    &world,
                    10,
                    Blueprint::minimal_plant(Genome::default()),
                    80.0,
                )
                .is_some(),
            "cliff notch must not hard-block editor / sucker placement"
        );
    }

    #[test]
    fn flood_wave_does_not_stretch_land_plant() {
        let mut world = World::new(17);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(
                &world,
                10,
                Blueprint::minimal_plant(Genome::default()),
                80.0,
            )
            .expect("plant");
        let y_before = store.ecs.get::<&Pose>(e).unwrap().y;
        let bed_before = world.column_at(10).unwrap().climate_elevation();
        // Flood wave — raises surface_y a lot, bed unchanged.
        if let Some(col) = world.column_at_mut(10) {
            col.deposit_to_top(MaterialId::Water, 8_000, 0);
        }
        let surface_after = world.column_at(10).unwrap().surface_y;
        assert!(
            surface_after > bed_before + 1.0,
            "flood should lift free surface"
        );
        for t in 0..5 {
            store.step_organisms(&mut world, t);
        }
        let y_after = store.ecs.get::<&Pose>(e).unwrap().y;
        assert!(
            (y_after - y_before).abs() < 0.05,
            "rooted plant must stay on the bed through a flood wave (before={y_before} after={y_after} surface={surface_after})"
        );
    }

    #[test]
    fn land_plant_hibernates_through_short_drought() {
        let mut world = World::new(5);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        // Bone-dry soil.
        if let Some(col) = world.column_at_mut(2) {
            col.moisture = 0;
            col.ecology.nutrient = 0.5;
        }
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(
                &world,
                2,
                Blueprint::minimal_plant(Genome::default()),
                80.0,
            )
            .expect("spawn");
        // Survive well past the old "seconds" kill window.
        for t in 0..2_000 {
            store.step_organisms(&mut world, t);
        }
        let info = store.inspect_organism(e).expect("should still be hibernating");
        assert!(info.drought_ticks > 0, "should be in drought dormancy");
        assert!(info.energy > 0.0);
        // Eventually dies if drought never breaks.
        if let Ok(mut org) = store.ecs.get::<&mut Organism>(e) {
            org.drought_ticks = crate::root::DROUGHT_HIBERNATE_MAX_TICKS - 5;
        }
        for t in 2_000..2_020 {
            store.step_organisms(&mut world, t);
        }
        assert!(
            store.ecs.get::<&Organism>(e).is_err(),
            "prolonged drought should end living Organism after hibernate max"
        );
    }

    #[test]
    fn land_plant_elongates_roots_while_banking_for_sprout() {
        // Mid-tank (above grow floor, below sprout gate) must deepen roots —
        // the old shared 0.52 gate left forests as 1-pixel stubs.
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 22.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.nutrient = 0.85;
                col.deposit_to_top(MaterialId::Organic, 800, 0);
            }
        }
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(
                &world,
                16,
                Blueprint::minimal_plant(Genome {
                    alloc_root: 0.85,
                    alloc_stem: 0.1,
                    alloc_leaf: 0.05,
                    root_depth_bias: 0.85,
                    reproduce_at: 0.95, // don't sprout during this test
                    ..Genome::default()
                }),
                200.0,
            )
            .expect("plant");
        let roots0 = store
            .inspect_organism(e)
            .map(|i| i.roots)
            .unwrap_or(0);
        if let Ok(mut energy) = store.ecs.get::<&mut Energy>(e) {
            energy.current = energy.max * 0.42;
        }
        for t in 0..6_000 {
            // Hold the tank in the grow band so we don't starve or sprout.
            if let Ok(mut energy) = store.ecs.get::<&mut Energy>(e) {
                energy.current = (energy.max * 0.42).max(energy.current);
                energy.current = energy.current.min(energy.max * 0.50);
            }
            store.step_organisms(&mut world, t);
        }
        let roots1 = store
            .inspect_organism(e)
            .map(|i| i.roots)
            .unwrap_or(0);
        assert!(
            roots1 > roots0,
            "roots should elongate while banking (before={roots0} after={roots1})"
        );
        assert!(
            roots1 >= crate::root::LAND_SPROUT_MIN_ROOTS,
            "should reach sprout-ready root count (got {roots1})"
        );
    }

    #[test]
    fn rhizome_sprout_inherits_genet_and_shares_energy() {
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 22.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.nutrient = 0.8;
                col.deposit_to_top(MaterialId::Organic, 600, 0);
            }
        }
        let mut store = AgentStore::new();
        let parent = store
            .spawn_from_blueprint(
                &world,
                16,
                Blueprint::minimal_plant(Genome {
                    reproduce_at: 0.99,
                    metabolic_rate: 0.15,
                    active_window: 0.9,
                    ..Genome::default()
                }),
                200.0,
            )
            .expect("parent");
        // Assign genet + spawn a sibling ramet by hand (same genet, nearby).
        store.step_organisms(&mut world, 0); // ensure genet_id
        let genet = store
            .ecs
            .get::<&Lineage>(parent)
            .map(|l| l.genet_id)
            .unwrap_or(0);
        assert!(genet != 0, "rooted plant must join a genet");
        let child = store
            .spawn_from_blueprint(
                &world,
                18,
                Blueprint::minimal_plant(Genome {
                    reproduce_at: 0.99,
                    metabolic_rate: 0.15,
                    ..Genome::default()
                }),
                200.0,
            )
            .expect("child");
        store.step_organisms(&mut world, 1);
        // Force same genet (spawn assigns its own).
        if let Ok(mut lin) = store.ecs.get::<&mut Lineage>(child) {
            lin.genet_id = genet;
        }
        if let Ok(mut e) = store.ecs.get::<&mut Energy>(parent) {
            e.current = e.max * 0.90;
        }
        if let Ok(mut e) = store.ecs.get::<&mut Energy>(child) {
            e.current = e.max * 0.20;
        }
        let before_p = store.ecs.get::<&Energy>(parent).unwrap().current;
        let before_c = store.ecs.get::<&Energy>(child).unwrap().current;
        store.step_organisms(&mut world, 2); // equalize at start
        let after_p = store.ecs.get::<&Energy>(parent).unwrap().current;
        let after_c = store.ecs.get::<&Energy>(child).unwrap().current;
        assert!(
            after_p < before_p && after_c > before_c,
            "genet should move energy parent→child (p {before_p:.1}→{after_p:.1}, c {before_c:.1}→{after_c:.1})"
        );
        let total_before = before_p + before_c;
        let total_after = after_p + after_c;
        assert!(
            (total_before - total_after).abs() < 1.0,
            "equalize must conserve energy (before={total_before} after={total_after})"
        );
    }

    #[test]
    fn genet_ramet_drinks_through_sibling_column() {
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        // Parent column wet; child column bone-dry.
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = 0;
                col.ecology.nutrient = 0.6;
            }
        }
        for x in 15..=17 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
            }
        }
        let mut store = AgentStore::new();
        let parent = store
            .spawn_from_blueprint(
                &world,
                16,
                Blueprint::minimal_plant(Genome {
                    circadian_phase: 0.25,
                    active_window: 1.0,
                    metabolic_rate: 0.1,
                    reproduce_at: 0.99,
                    ..Genome::default()
                }),
                100.0,
            )
            .expect("parent");
        let child = store
            .spawn_from_blueprint(
                &world,
                20,
                Blueprint::minimal_plant(Genome {
                    circadian_phase: 0.25,
                    active_window: 1.0,
                    metabolic_rate: 0.1,
                    reproduce_at: 0.99,
                    ..Genome::default()
                }),
                100.0,
            )
            .expect("child");
        store.step_organisms(&mut world, 0);
        let genet = store.ecs.get::<&Lineage>(parent).unwrap().genet_id;
        if let Ok(mut lin) = store.ecs.get::<&mut Lineage>(child) {
            lin.genet_id = genet;
        }
        // Force a drink attempt on the dry child via shared hosts.
        if let Ok(mut org) = store.ecs.get::<&mut Organism>(child) {
            org.sip_acc_kg = 1.5;
        }
        if let Ok(mut e) = store.ecs.get::<&mut Energy>(child) {
            e.current = 40.0;
        }
        let moist_parent_before = world.column_at(16).unwrap().moisture;
        let e_before = store.ecs.get::<&Energy>(child).unwrap().current;
        // Noon-ish tick so circadian is active (phase 0.25 window full).
        let noon = world.climate.day_length_ticks / 4;
        store.step_organisms(&mut world, noon);
        let moist_parent_after = world.column_at(16).unwrap().moisture;
        let e_after = store
            .ecs
            .get::<&Energy>(child)
            .map(|e| e.current)
            .unwrap_or(0.0);
        assert!(
            moist_parent_after < moist_parent_before || e_after > e_before,
            "dry ramet should pull moisture/energy via wet sibling (moist {moist_parent_before}→{moist_parent_after}, e {e_before}→{e_after})"
        );
    }

    #[test]
    fn land_plant_corpse_settles_faster_than_bloom_carpet() {
        let mut world = World::new(42);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(
                &world,
                8,
                Blueprint::minimal_plant(Genome::default()),
                80.0,
            )
            .expect("spawn");
        let _ = store.ecs.remove_one::<Organism>(e);
        let _ = store.ecs.insert_one(
            e,
            Corpse {
                ticks: 0,
                settled_ticks: CORPSE_SETTLE_LAND_TICKS,
            },
        );
        store.step_corpses(&mut world, 1);
        assert_eq!(
            store.corpse_count(),
            0,
            "land plant corpse should dissolve at land settle ticks"
        );
        assert!(CORPSE_SETTLE_LAND_TICKS < CORPSE_SETTLE_TICKS);
    }

    #[test]
    fn land_plant_propagates_by_root_sprout() {
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.nutrient = 0.7;
                col.deposit_to_top(MaterialId::Organic, 600, 0);
            }
        }
        let mut store = AgentStore::new();
        let parent = store
            .spawn_from_blueprint(
                &world,
                16,
                Blueprint::minimal_plant(Genome {
                    reproduce_at: 0.35,
                    clone_fidelity: 0.9,
                    metabolic_rate: 0.2,
                    alloc_root: 0.85,
                    alloc_stem: 0.1,
                    alloc_leaf: 0.05,
                    root_depth_bias: 0.25, // sprawl rhizomes
                    active_window: 0.85,
                    ..Genome::default()
                }),
                200.0,
            )
            .expect("parent");
        let x0 = store.ecs.get::<&Pose>(parent).unwrap().x;
        // Grow a lateral runner, then sprout from its tip (no teleport).
        for t in 0..20_000 {
            if let Ok(mut e) = store.ecs.get::<&mut Energy>(parent) {
                // Keep a working tank so runners can paint, then bank for sprout.
                e.current = e.current.max(e.max * 0.55).min(e.max);
            }
            store.step_organisms(&mut world, t);
            if store.organism_count() > 1 {
                break;
            }
        }
        assert!(
            store.organism_count() > 1,
            "root sprout should emerge from a painted runner"
        );
        let parent_has_runner = store
            .ecs
            .get::<&ModuleBody>(parent)
            .map(|b| crate::root::has_lateral_runner(x0, &b.blueprint))
            .unwrap_or(false);
        assert!(
            parent_has_runner,
            "parent should have shot a horizontal runner before suckering"
        );
        let mut xs: Vec<f32> = store
            .ecs
            .query::<(&Pose, &Organism)>()
            .iter()
            .map(|(_, (p, _))| p.x)
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let max_dx = xs
            .iter()
            .map(|x| (x - x0).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_dx <= crate::root::ROOT_SPROUT_MAX_DIST as f32 + 1.5,
            "sprouts must stay near the parent root system (max_dx={max_dx})"
        );
    }

    #[test]
    fn founder_lineage_starts_at_zero() {
        let mut world = World::new(3);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(&world, 2, Blueprint::atom(Genome::default()), 50.0)
            .expect("spawn");
        let info = store.inspect_organism(e).expect("inspect");
        assert_eq!(info.generation, 0);
        assert_eq!(info.clones_produced, 0);
        assert_eq!(info.founder_id, 0);
        assert!(info.energy > 0.0);
    }

    #[test]
    fn founder_tag_inherited_on_fission() {
        let mut world = World::new(31);
        world.sea_level = 0.0;
        world.climate.day_length_ticks = 60;
        world.climate.night_length_ticks = 60;
        world.climate.day_night_amplitude_c = 0.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.base_temp_c = 18.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.deposit_to_top(MaterialId::Water, 2_000, 0);
            }
        }
        let genome = Genome {
            metabolic_rate: 0.15,
            reproduce_at: 0.35,
            clone_fidelity: 1.0,
            circadian_phase: 0.25,
            active_window: 0.9,
            repro_drive: 0.0,
            temp_optimum: 18.0,
            temp_width: 25.0,
            ..Genome::default()
        };
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(&world, 8, Blueprint::atom(genome), 50.0)
            .expect("spawn");
        store.tag_founder(e, 7);
        for tick in 0..800 {
            store.step_organisms(&mut world, tick);
            if store.count_living_by_founder(7) > 1 {
                break;
            }
        }
        assert!(
            store.count_living_by_founder(7) > 1,
            "expected tagged offspring, living={}",
            store.count_living_by_founder(7)
        );
        assert_eq!(
            store.organism_count(),
            store.count_living_by_founder(7),
            "all living organisms should carry founder tag"
        );
    }

    #[test]
    fn floater_equilibrium_is_one_metre_under_surface() {
        let world = world_with_water();
        let col = world.column_at(8).unwrap();
        let (top, bed) = water_band(col).expect("water");
        assert!(top - bed > FLOAT_DEPTH_M);
        let y = buoyancy_target_y(col, 0.0).unwrap();
        assert!((y - (top - FLOAT_DEPTH_M)).abs() < 1e-3);
    }

    #[test]
    fn sinker_equilibrium_is_bed() {
        let world = world_with_water();
        let col = world.column_at(8).unwrap();
        let (_top, bed) = water_band(col).expect("water");
        let y = buoyancy_target_y(col, 1.0).unwrap();
        assert!((y - bed).abs() < 1e-3);
    }

    #[test]
    fn rising_water_lifts_floater() {
        let mut world = world_with_water();
        let col = world.column_at(8).unwrap();
        let (top0, _) = water_band(col).unwrap();
        let mut y = top0 - FLOAT_DEPTH_M;
        let mut state = BuoyancyState {
            vel_y: 0.0,
            last_water_top: Some(top0),
        };
        // Add more water → free surface rises.
        if let Some(c) = world.column_at_mut(8) {
            c.deposit_to_top(MaterialId::Water, 1_500, 1);
        }
        let col = world.column_at(8).unwrap();
        let (top1, _) = water_band(col).unwrap();
        assert!(top1 > top0, "expected water top to rise");
        step_buoyancy(&mut y, &mut state, col, 0.0, 0.0);
        assert!(
            y > top0 - FLOAT_DEPTH_M + 0.1,
            "floater should rise with water: y={y} old_float={} new_top={top1}",
            top0 - FLOAT_DEPTH_M
        );
    }

    #[test]
    fn tall_editor_paint_spawns_submerged() {
        // Painting near the top of the 16-tall editor canvas used to put
        // modules in the air because pose.y sat at the float line and
        // m.y extended upward. Body-top anchoring keeps the crest wet.
        let world = world_with_water();
        let mut bp = Blueprint::atom(Genome::default());
        bp.modules.push(crate::blueprint::PlacedModule {
            x: 0,
            y: 12,
            lane: crate::LaneId::Mid,
            module: crate::ModuleId::Photosystem,
        });
        let y = AgentStore::spawn_elevation(&world, 8, &bp).unwrap();
        let col = world.column_at(8).unwrap();
        let (top, _) = water_band(col).unwrap();
        let offset = blueprint_body_top_offset(&bp);
        assert!(offset > 5.0, "tall paint should produce a body offset");
        let body_top = y + offset;
        assert!(
            body_top <= top + 0.05,
            "body top must sit at/under the free surface (top={top:.2} body_top={body_top:.2})"
        );
        assert!(
            (body_top - (top - FLOAT_DEPTH_M)).abs() < 0.2,
            "body top should sit on the float line"
        );
    }

    #[test]
    fn rooted_plant_stays_put_when_algae_overlaps() {
        let mut world = World::new(9);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        // Wet column for algae spawn; plant on dry land beside it.
        if let Some(col) = world.column_at_mut(4) {
            col.deposit_to_top(MaterialId::Water, 2_000, 0);
        }
        let mut store = AgentStore::new();
        let plant = store
            .spawn_from_blueprint(
                &world,
                8,
                Blueprint::minimal_plant(Genome::default()),
                80.0,
            )
            .expect("plant");
        let x_before = store.ecs.get::<&Pose>(plant).unwrap().x;
        let y_plant = store.ecs.get::<&Pose>(plant).unwrap().y;
        let algae = store
            .spawn_from_blueprint(&world, 4, Blueprint::atom(Genome::default()), 50.0)
            .expect("algae");
        // Force the algae onto the plant footprint.
        if let Ok(mut pose) = store.ecs.get::<&mut Pose>(algae) {
            pose.x = x_before;
            pose.y = y_plant;
        }
        let ax_before = store.ecs.get::<&Pose>(algae).unwrap().x;
        resolve_collisions(&mut store, &world);
        let x_after = store.ecs.get::<&Pose>(plant).unwrap().x;
        assert!(
            (x_after - x_before).abs() < 1e-4,
            "rooted plant must not move (before={x_before} after={x_after})"
        );
        let ax_after = store.ecs.get::<&Pose>(algae).unwrap().x;
        assert!(
            (ax_after - ax_before).abs() > 0.01,
            "algae should be shoved off the immobile plant (before={ax_before} after={ax_after})"
        );
    }

    #[test]
    fn footprint_collision_prevents_overlap() {
        let mut world = World::new(7);
        world.sea_level = 10.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 5.0));
        for x in 0..16 {
            if let Some(col) = world.column_at_mut(x) {
                col.deposit_to_top(MaterialId::Water, 2_000, 0);
            }
        }
        let mut store = AgentStore::new();
        let bp = Blueprint::atom(Genome::default());
        assert!(store.spawn_from_blueprint(&world, 4, bp.clone(), 50.0).is_some());
        assert!(store.spawn_from_blueprint(&world, 4, bp.clone(), 50.0).is_some());
        assert!(store.spawn_from_blueprint(&world, 4, bp, 50.0).is_some());
        let bodies = collect_bodies(&store, &world);
        assert_eq!(bodies.len(), 3);
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                assert!(
                    !bodies[i].1.overlaps(bodies[j].1),
                    "bodies {i} and {j} still overlap"
                );
            }
        }
    }

    #[test]
    fn clones_spread_horizontally_not_lens() {
        let mut world = World::new(21);
        world.sea_level = 10.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 5.0));
        // Flood many columns so the float line is wide.
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.deposit_to_top(MaterialId::Water, 2_000, 0);
            }
        }
        let mut store = AgentStore::new();
        let bp = Blueprint::atom(Genome {
            buoyancy_bias: 0.0,
            ..Genome::default()
        });
        let origin = 32;
        for _ in 0..24 {
            assert!(
                store
                    .spawn_from_blueprint(&world, origin, bp.clone(), 50.0)
                    .is_some(),
                "spawn should find a clear slot along the water"
            );
        }
        let mut xs: Vec<f32> = store
            .ecs
            .query::<(&Pose, &Organism)>()
            .iter()
            .map(|(_, (p, _))| p.x)
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let span = xs[xs.len() - 1] - xs[0];
        assert!(
            span > 8.0,
            "expected clones to spread along X (span={span}), not pack into a lens"
        );
        let bodies = collect_bodies(&store, &world);
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                assert!(!bodies[i].1.overlaps(bodies[j].1));
            }
        }
    }

    #[test]
    fn rooted_plants_do_not_shove_each_other() {
        let mut world = World::new(9);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        let mut store = AgentStore::new();
        let a = store
            .spawn_from_blueprint(
                &world,
                8,
                Blueprint::minimal_plant(Genome::default()),
                80.0,
            )
            .expect("plant a");
        let b = store
            .spawn_from_blueprint(
                &world,
                10,
                Blueprint::minimal_plant(Genome::default()),
                80.0,
            )
            .expect("plant b");
        // Force overlap — trees must stay put, not bury sideways.
        let xa = store.ecs.get::<&Pose>(a).unwrap().x;
        let ya = store.ecs.get::<&Pose>(a).unwrap().y;
        if let Ok(mut pb) = store.ecs.get::<&mut Pose>(b) {
            pb.x = xa;
            pb.y = ya;
        }
        let xb_before = store.ecs.get::<&Pose>(b).unwrap().x;
        resolve_collisions(&mut store, &world);
        let xa_after = store.ecs.get::<&Pose>(a).unwrap().x;
        let xb_after = store.ecs.get::<&Pose>(b).unwrap().x;
        assert!(
            (xa_after - xa).abs() < 1e-4,
            "plant A must not move"
        );
        assert!(
            (xb_after - xb_before).abs() < 1e-4,
            "plant B must not move (before={xb_before} after={xb_after})"
        );
    }

    #[test]
    fn collapsed_soil_unanchors_purchase_but_plant_stays_put() {
        use wk_world::column::VoidOrigin;
        let mut world = World::new(9);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        if let Some(col) = world.column_at_mut(4) {
            col.deposit_to_top(MaterialId::Water, 2_000, 0);
        }
        let mut store = AgentStore::new();
        let plant = store
            .spawn_from_blueprint(
                &world,
                8,
                Blueprint::minimal_plant(Genome::default()),
                80.0,
            )
            .expect("plant");
        let pose = *store.ecs.get::<&Pose>(plant).unwrap();
        let bp = store.ecs.get::<&ModuleBody>(plant).unwrap().blueprint.clone();
        assert!(
            crate::root::plant_is_anchored(&world, pose.x, pose.y, &bp),
            "fresh plant should have root purchase"
        );
        let tip_y = pose.y - MODULE_CELL_COLS; // root at blueprint y=-1
        let wx = pose.world_x();
        if let Some(col) = world.column_at_mut(wx) {
            col.grow_void_at(tip_y, 4.0, MaterialId::Sand, VoidOrigin::Collapse);
        }
        assert!(
            !crate::root::plant_is_anchored(&world, pose.x, pose.y, &bp),
            "collapsed soil should lose root purchase"
        );
        let x_before = pose.x;
        let algae = store
            .spawn_from_blueprint(&world, 4, Blueprint::atom(Genome::default()), 50.0)
            .expect("algae");
        if let Ok(mut ap) = store.ecs.get::<&mut Pose>(algae) {
            ap.x = x_before;
            ap.y = pose.y;
        }
        resolve_collisions(&mut store, &world);
        let x_after = store.ecs.get::<&Pose>(plant).unwrap().x;
        assert!(
            (x_after - x_before).abs() < 1e-4,
            "rooted plant stays immobile even without purchase (before={x_before} after={x_after})"
        );
    }

    #[test]
    fn larger_blueprint_has_bigger_aabb() {
        let pose = Pose { x: 0.0, y: 0.0 };
        let small = Blueprint::atom(Genome::default());
        let mut big = small.clone();
        // Widen the paint: modules at x=0..3.
        big.modules.push(crate::blueprint::PlacedModule {
            x: 2,
            y: 0,
            lane: crate::LaneId::Mid,
            module: crate::ModuleId::Photosystem,
        });
        big.modules.push(crate::blueprint::PlacedModule {
            x: 3,
            y: 0,
            lane: crate::LaneId::Mid,
            module: crate::ModuleId::Photosystem,
        });
        let a = organism_aabb(&pose, &small);
        let b = organism_aabb(&pose, &big);
        assert!(
            (b.max_x - b.min_x) > (a.max_x - a.min_x) + 0.1,
            "wider paint must widen collision box"
        );
    }

    #[test]
    fn morphology_clone_error_can_add_or_remove_photosystem() {
        let atom = Blueprint::atom(Genome::default());
        let n0 = atom.photosystem_count();
        assert_eq!(n0, 1);
        // Perfect fidelity → never mutate morphology.
        let same = mutate_blueprint_morphology(atom.clone(), 1.0, 99, 1, 7);
        assert_eq!(same.photosystem_count(), n0);
        // Messy fidelity: scan ticks until we see both an add and a remove
        // (or at least one change). Deterministic hash so this is stable.
        let mut saw_add = false;
        let mut saw_remove = false;
        for tick in 0..400u64 {
            let mut parent = atom.clone();
            // Seed a second photosystem so remove is possible.
            parent.modules.push(crate::blueprint::PlacedModule {
                x: 0,
                y: 1,
                lane: crate::LaneId::Mid,
                module: crate::ModuleId::Photosystem,
            });
            let child = mutate_blueprint_morphology(parent, 0.0, 42, tick, 3);
            let n = child.photosystem_count();
            assert!(n >= 1, "must keep at least one photosystem");
            assert!(child.nucleus_count() >= 1, "must keep nucleus");
            if n > 2 {
                saw_add = true;
            }
            if n < 2 {
                saw_remove = true;
            }
            if saw_add && saw_remove {
                break;
            }
        }
        assert!(saw_add, "expected at least one adjacent photosystem add");
        assert!(saw_remove, "expected at least one photosystem remove");
    }

    #[test]
    fn atom_draw_stays_in_one_column() {
        let mut world = World::new(11);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        let mut store = AgentStore::new();
        let bp = Blueprint::atom(Genome::default());
        store
            .spawn_from_blueprint(&world, 10, bp, 50.0)
            .expect("spawn");
        let quads = store.organism_draw_list();
        assert_eq!(quads.len(), 2);
        for &(wx, _, _) in &quads {
            assert!(
                (wx - 10.5).abs() < 0.5,
                "module wx={wx} spilled outside host column 10"
            );
        }
    }

    #[test]
    fn spawn_ignores_constant_sea_level() {
        let mut world = World::new(9);
        world.sea_level = 10.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 2.0));
        if let Some(col) = world.column_at_mut(3) {
            col.deposit_to_top(MaterialId::Water, 1_500, 0);
        }
        let col = world.column_at(3).unwrap();
        let (top, _) = water_band(col).unwrap();
        let bp = Blueprint::atom(Genome {
            buoyancy_bias: 0.0,
            ..Genome::default()
        });
        let y = AgentStore::spawn_elevation(&world, 3, &bp).unwrap();
        assert!((y - (top - FLOAT_DEPTH_M)).abs() < 0.5);
        assert!((y - (world.sea_level - 0.35)).abs() > 1.0);
    }

    #[test]
    fn default_atom_reproduces_with_thermal_fields() {
        // Shallow bedrock used to seed ~38 °C surface water (geo→sky
        // anchored at the air top). That hard-gated fission while energy
        // could still fill — the playtest "peak energy, never clones" bug.
        let mut world = World::new(42);
        world.sea_level = 12.0;
        world.rain_enabled = false;
        world.weather.weather_enabled = false;
        for c in -1..=1 {
            world.insert_chunk(generate_flat_sand(c, -20.0, 8.0));
        }
        let sea = world.sea_level;
        for x in -64..128 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                let (eta, mass) = col.flowable_water().unwrap_or((col.surface_y, 0));
                let bed = eta - mass as f32 / 250.0;
                let target = ((sea - bed).max(2.0) * 250.0) as i64;
                let need = target - mass;
                if need > 0 {
                    col.deposit_to_top(MaterialId::Water, need, 0);
                }
            }
        }
        world.wake_all();
        world.enable_thermal_fields();
        world.recompute_mass_audit();

        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(&world, 32, Blueprint::atom(Genome::default()), 50.0)
            .expect("spawn");

        let mut temps = Vec::new();
        let mut blocked = 0u32;
        let mut ok = 0u32;
        for tick in 0..5_000u64 {
            if tick % 200 == 0 {
                if let Ok(pose) = store.ecs.get::<&Pose>(e) {
                    let t = world.temperature_at_point(pose.x.floor() as i32, pose.y, tick);
                    let c = temp_comfort_factor(t, &Genome::default());
                    if c >= 0.20 {
                        ok += 1;
                    } else {
                        blocked += 1;
                    }
                    if temps.len() < 12 {
                        temps.push((tick, t, c));
                    }
                }
            }
            store.step_organisms(&mut world, tick);
            for (_ent, energy) in store.ecs.query_mut::<&mut Energy>() {
                energy.current = energy.max;
            }
        }
        eprintln!("temps_sample={temps:?} comfort_ok={ok} blocked={blocked}");
        eprintln!(
            "births={} count={} clones={}",
            store.births_total,
            store.organism_count(),
            store
                .inspect_organism(e)
                .map(|i| i.clones_produced)
                .unwrap_or(0)
        );
        assert!(
            store.births_total > 0,
            "default Atom with peak energy should fission under app-like thermal ocean (comfort_ok={ok} blocked={blocked})"
        );
    }

    #[test]
    fn coastal_plain_plant_sprouts_from_half_tank() {
        // Fertile wet flat — the "best coastal plain" case. Default genome,
        // no force-fed energy: elongation must not sterilise vegetative spread.
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 22.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_night_amplitude_c = 4.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.nutrient = 0.85;
                // Thin organic topsoil (coastal plain, not bare dune).
                col.deposit_to_top(MaterialId::Organic, 800, 0);
            }
        }
        let mut store = AgentStore::new();
        let parent = store
            .spawn_from_blueprint(
                &world,
                16,
                Blueprint::minimal_plant(Genome {
                    clone_fidelity: 0.85,
                    metabolic_rate: 0.25,
                    active_window: 0.75,
                    alloc_root: 0.55,
                    ..Genome::default()
                }),
                200.0,
            )
            .expect("parent");
        // Mid-life tank — not force-fed every tick.
        if let Ok(mut e) = store.ecs.get::<&mut Energy>(parent) {
            e.current = e.max * 0.55;
        }
        let x0 = store.ecs.get::<&Pose>(parent).unwrap().x;
        for t in 0..60_000 {
            store.step_organisms(&mut world, t);
            if store.organism_count() > 1 {
                let max_dx = store
                    .ecs
                    .query::<(&Pose, &Organism)>()
                    .iter()
                    .map(|(_, (p, _))| (p.x - x0).abs())
                    .fold(0.0f32, f32::max);
                assert!(
                    max_dx <= crate::root::ROOT_SPROUT_MAX_DIST as f32 + 1.5,
                    "coastal sprouts must stay local (max_dx={max_dx})"
                );
                return;
            }
        }
        panic!(
            "expected root sprout on wet coastal plain; living={}",
            store.organism_count()
        );
    }

    #[test]
    fn deep_rooted_plant_survives_default_night() {
        // Regresses the 0.015/root/tick tax: 24 roots × night emptied a
        // full 200 tank before dawn even on wet fertile ground.
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 22.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.nutrient = 0.9;
                col.deposit_to_top(MaterialId::Organic, 900, 0);
            }
        }
        let mut store = AgentStore::new();
        let mut bp = Blueprint::minimal_plant(Genome {
            metabolic_rate: 0.35,
            alloc_root: 0.7,
            ..Genome::default()
        });
        // Pre-paint a heavy root system (soft budget normally prevents this).
        for i in 0..20i16 {
            bp.modules.push(crate::blueprint::PlacedModule {
                x: (i % 5) - 2,
                y: -1 - (i / 5),
                lane: crate::module::LaneId::Mid,
                module: crate::module::ModuleId::Root,
            });
        }
        let e = store
            .spawn_from_blueprint(&world, 16, bp, 200.0)
            .expect("plant");
        assert!(
            store.inspect_organism(e).unwrap().roots >= 20,
            "fixture needs a deep root system"
        );
        if let Ok(mut energy) = store.ecs.get::<&mut Energy>(e) {
            energy.current = energy.max;
        }
        // Skip the day; run the whole default night (10 h).
        let night0 = world.climate.day_length_ticks;
        let night1 = night0 + world.climate.night_length_ticks;
        for t in night0..night1 {
            store.step_organisms(&mut world, t);
        }
        assert!(
            store.ecs.get::<&Organism>(e).is_ok(),
            "deep-rooted plant must survive one night on a full tank"
        );
        let info = store.inspect_organism(e).unwrap();
        assert!(
            info.energy > 1.0,
            "should keep reserves through the night (energy={})",
            info.energy
        );
    }

    #[test]
    fn land_plant_roots_expand_energy_storage() {
        // Deep roots raise Energy.max (starch bank) without raising the
        // spawn tank that keys photo / upkeep / growth floors.
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 22.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.nutrient = 0.9;
                col.deposit_to_top(MaterialId::Organic, 900, 0);
            }
        }
        let mut store = AgentStore::new();
        let mut bp = Blueprint::minimal_plant(Genome {
            metabolic_rate: 0.35,
            alloc_root: 0.7,
            ..Genome::default()
        });
        for i in 0..15i16 {
            bp.modules.push(crate::blueprint::PlacedModule {
                x: (i % 5) - 2,
                y: -1 - (i / 5),
                lane: crate::module::LaneId::Mid,
                module: crate::module::ModuleId::Root,
            });
        }
        let e = store
            .spawn_from_blueprint(&world, 16, bp, 200.0)
            .expect("plant");
        let base = store.ecs.get::<&Organism>(e).unwrap().energy_base_max;
        assert!((base - 200.0).abs() < 1e-3, "spawn tank is 200");
        let roots = store.inspect_organism(e).unwrap().roots;
        store.step_organisms(&mut world, 0);
        let expect = crate::root::energy_capacity(base, roots);
        {
            let energy = store.ecs.get::<&Energy>(e).unwrap();
            assert!(
                (energy.max - expect).abs() < 1e-2,
                "max should track root storage (max={} expect={expect} roots={roots})",
                energy.max
            );
            assert!(
                energy.max > base + 10.0,
                "deep roots must expand capacity above spawn tank"
            );
        }
        // Surplus can sit above the spawn tank — growth floor stays on base.
        if let Ok(mut energy) = store.ecs.get::<&mut Energy>(e) {
            energy.current = energy.max;
        }
        let floor = crate::root::growth_energy_floor(base);
        let energy = store.ecs.get::<&Energy>(e).unwrap();
        assert!(
            energy.current > floor * 2.0,
            "root bank should leave headroom above the base growth floor"
        );
    }

    #[test]
    fn hydrated_plant_stops_rooting_past_soft_budget() {
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 22.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.nutrient = 0.9;
                col.deposit_to_top(MaterialId::Organic, 900, 0);
            }
        }
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(
                &world,
                16,
                Blueprint::minimal_plant(Genome {
                    metabolic_rate: 0.25,
                    alloc_root: 0.9,
                    alloc_stem: 0.05,
                    alloc_leaf: 0.05,
                    root_depth_bias: 0.9,
                    reproduce_at: 0.99,
                    clone_fidelity: 0.05, // almost never viable sprouts
                    ..Genome::default()
                }),
                200.0,
            )
            .expect("plant");
        for t in 0..20_000 {
            if let Some(col) = world.column_at_mut(16) {
                col.moisture = col.moisture_cap();
            }
            // Keep the tank high so only the soft budget (not energy) gates roots.
            if let Ok(mut energy) = store.ecs.get::<&mut Energy>(e) {
                energy.current = energy.max * 0.85;
            }
            store.step_organisms(&mut world, t);
        }
        let info = store.inspect_organism(e).expect("alive");
        let budget = crate::root::useful_root_budget(
            &store.ecs.get::<&ModuleBody>(e).unwrap().blueprint,
        );
        assert!(
            info.roots <= budget + 6,
            "hydrated plant should stop near soft budget (roots={} budget={budget}; +6 allows one rhizome runner)",
            info.roots
        );
        assert!(
            info.roots >= crate::root::LAND_SPROUT_MIN_ROOTS,
            "should still reach sprout-ready roots"
        );
    }

    #[test]
    fn plant_sprout_uses_morphology_mutation_like_algae() {
        // Child body is a mutated clone of the parent — not a fresh minimal_plant.
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 22.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.nutrient = 0.9;
                col.deposit_to_top(MaterialId::Organic, 700, 0);
            }
        }
        let mut parent_bp = Blueprint::minimal_plant(Genome {
            clone_fidelity: 0.0, // always morph-error when rolled
            metabolic_rate: 0.15,
            alloc_root: 0.7,
            reproduce_at: 0.4,
            ..Genome::default()
        });
        // Extra photosystems so remove-or-add is visible; paint a runner tip.
        for y in 4i16..=6 {
            parent_bp.modules.push(crate::blueprint::PlacedModule {
                x: 0,
                y,
                lane: crate::module::LaneId::Mid,
                module: crate::module::ModuleId::Photosystem,
            });
        }
        parent_bp.modules.push(crate::blueprint::PlacedModule {
            x: 4, // ~1.8 m → neighbour column
            y: -1,
            lane: crate::module::LaneId::Mid,
            module: crate::module::ModuleId::Root,
        });
        let parent_photos = parent_bp.photosystem_count();
        let mut store = AgentStore::new();
        let parent = store
            .spawn_from_blueprint(&world, 16, parent_bp, 200.0)
            .expect("parent");
        if let Ok(mut e) = store.ecs.get::<&mut Energy>(parent) {
            e.current = e.max;
        }
        let mut child_photos = None;
        for t in 0..20_000 {
            store.step_organisms(&mut world, t);
            if store.organism_count() > 1 {
                child_photos = store
                    .ecs
                    .query::<(&ModuleBody, &Organism)>()
                    .iter()
                    .filter(|(e, _)| *e != parent)
                    .map(|(_, (b, _))| b.photosystem_count())
                    .next();
                break;
            }
        }
        let child_n = child_photos.expect("expected a sprout child");
        // Parent starts with 5 photosystems; morph mut ±1. minimal_plant is 2.
        assert!(
            (4..=6).contains(&child_n),
            "sprout should clone parent morphology (±1 photo), got {child_n} from parent {parent_photos}"
        );
    }

    #[test]
    fn fungus_digests_litter_and_raises_nutrient() {
        let mut world = World::new(9);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 22.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.dead_biomass = 120;
                col.ecology.nutrient = 0.1;
                col.deposit_to_top(MaterialId::Organic, 800, 0);
            }
        }
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(
                &world,
                16,
                Blueprint::minimal_fungus(Genome {
                    digest_rate: 1.2,
                    metabolic_rate: 0.15,
                    reproduce_at: 0.99,
                    ..Genome::default()
                }),
                80.0,
            )
            .expect("fungus");
        let nut0 = world.column_at(16).unwrap().ecology.nutrient;
        let litter0 = world.column_at(16).unwrap().ecology.dead_biomass;
        let e0 = store.ecs.get::<&Energy>(e).unwrap().current;
        for t in 0..400 {
            store.step_organisms(&mut world, t);
        }
        let nut1 = world.column_at(16).unwrap().ecology.nutrient;
        let litter1 = world.column_at(16).unwrap().ecology.dead_biomass;
        let e1 = store.ecs.get::<&Energy>(e).unwrap().current;
        assert!(litter1 < litter0, "should consume soft litter");
        assert!(nut1 > nut0, "digest should mineralize nutrient");
        assert!(e1 > e0 - 1.0, "fungus should gain or hold energy from digest");
        assert!(store.ecs.get::<&Organism>(e).is_ok(), "should stay alive");
    }

    #[test]
    fn fungus_hibernates_without_litter_then_dies() {
        let mut world = World::new(9);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        // Bone-dry, no litter.
        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(
                &world,
                8,
                Blueprint::minimal_fungus(Genome::default()),
                40.0,
            )
            .expect("fungus");
        for t in 0..200 {
            store.step_organisms(&mut world, t);
        }
        let ticks = store.inspect_organism(e).unwrap().drought_ticks;
        assert!(ticks > 0, "should enter fungus dormancy");
        if let Ok(mut org) = store.ecs.get::<&mut Organism>(e) {
            org.drought_ticks = crate::fungi::FUNGUS_HIBERNATE_MAX_TICKS - 3;
        }
        for t in 200..210 {
            store.step_organisms(&mut world, t);
        }
        assert!(
            store.ecs.get::<&Organism>(e).is_err(),
            "prolonged starve dormancy should kill"
        );
    }

    #[test]
    fn fungus_spores_onto_nearby_litter() {
        let mut world = World::new(9);
        world.sea_level = 0.0;
        world.climate.base_temp_c = 22.0;
        world.climate.lapse_rate_c_per_m = 0.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.dead_biomass = 200;
                col.ecology.nutrient = 0.5;
                col.deposit_to_top(MaterialId::Organic, 600, 0);
            }
        }
        let mut store = AgentStore::new();
        let parent = store
            .spawn_from_blueprint(
                &world,
                16,
                Blueprint::minimal_fungus(Genome {
                    digest_rate: 1.0,
                    metabolic_rate: 0.1,
                    clone_fidelity: 0.8,
                    reproduce_at: 0.4,
                    ..Genome::default()
                }),
                200.0,
            )
            .expect("parent");
        if let Ok(mut e) = store.ecs.get::<&mut Energy>(parent) {
            e.current = e.max;
        }
        for t in 0..30_000 {
            store.step_organisms(&mut world, t);
            if store.organism_count() > 1 {
                return;
            }
        }
        panic!(
            "expected spore burst on rich litter; living={}",
            store.organism_count()
        );
    }

    #[test]
    fn deep_snow_blocks_photosynthesis() {
        let mut world = World::new(202);
        world.sea_level = 0.0;
        world.climate.day_night_amplitude_c = 0.0;
        world.climate.base_temp_c = 18.0;
        // Noon: force day factor via tick in first half of day.
        world.climate.day_length_ticks = 1_000;
        world.climate.night_length_ticks = 1_000;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        // Rooted land plant under a deep snowpack.
        let mut bp = Blueprint::atom(Genome {
            circadian_phase: 0.25,
            active_window: 1.0,
            metabolic_rate: 0.2,
            ..Genome::default()
        });
        bp.modules.push(PlacedModule {
            x: 0,
            y: -1,
            lane: LaneId::Back,
            module: ModuleId::Root,
        });
        {
            let col = world.column_at_mut(4).unwrap();
            col.moisture = col.moisture_cap();
            col.ecology.nutrient = 0.8;
            col.deposit_to_top(MaterialId::Snow, 8_000, 0);
            col.clamp_state();
        }
        assert!(
            world.column_at(4).unwrap().cover_light_factor() < 0.02,
            "8t snow should be near-dark"
        );

        let mut store = AgentStore::new();
        let e = store
            .spawn_from_blueprint(&world, 4, bp, 50.0)
            .expect("spawn");
        if let Ok(mut energy) = store.ecs.get::<&mut Energy>(e) {
            energy.current = 40.0;
        }
        let before = store.ecs.get::<&Energy>(e).unwrap().current;
        // Mid-day tick.
        store.step_organisms(&mut world, 500);
        let after = store.ecs.get::<&Energy>(e).unwrap().current;
        assert!(
            after <= before,
            "buried plant must not gain energy under deep snow (before={before} after={after})"
        );
    }

    #[test]
    fn taller_neighbour_shades_short_plant_photo() {
        // End-to-end: canopy built from spawned bodies → effective_photo_light
        // for a short plant drops when a tall high-absorb neighbour is present.
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
            }
        }

        let short_g = Genome {
            leaf_absorb: 0.3,
            shade_efficiency: 0.15,
            ..Genome::default()
        };
        let tall_g = Genome {
            leaf_absorb: 0.95,
            shade_efficiency: 0.05,
            ..Genome::default()
        };
        let short_bp = Blueprint::minimal_plant(short_g);
        let mut tall_bp = Blueprint::minimal_plant(tall_g);
        for y in 4i16..=10 {
            tall_bp.modules.push(crate::blueprint::PlacedModule {
                x: 0,
                y,
                lane: crate::module::LaneId::Mid,
                module: if y == 10 {
                    crate::module::ModuleId::Photosystem
                } else {
                    crate::module::ModuleId::Stem
                },
            });
        }

        let mut store = AgentStore::new();
        let short = store
            .spawn_from_blueprint(&world, 20, short_bp, 80.0)
            .expect("short");
        let tall = store
            .spawn_from_blueprint(&world, 21, tall_bp, 200.0)
            .expect("tall");

        let mut canopy = crate::shade::CanopyIndex::default();
        for (e, (pose, body, genome, _)) in store
            .ecs
            .query::<(&Pose, &ModuleBody, &Genome, &Organism)>()
            .iter()
        {
            let n_photo = body.photosystem_count();
            let absorb = crate::shade::cast_strength(
                n_photo,
                body.blueprint.stem_count(),
                genome.leaf_absorb,
            );
            crate::shade::record_canopy(
                &mut canopy,
                pose.world_x(),
                crate::shade::canopy_top_y(pose.y, &body.blueprint),
                absorb,
                n_photo,
                e.id(),
            );
        }

        let short_pose = *store.ecs.get::<&Pose>(short).unwrap();
        let short_body = store.ecs.get::<&ModuleBody>(short).unwrap();
        let short_genome = *store.ecs.get::<&Genome>(short).unwrap();
        let sample_y = crate::shade::canopy_top_y(short_pose.y, &short_body.blueprint);
        let shaded = crate::shade::effective_photo_light(
            &canopy,
            short_pose.world_x(),
            sample_y,
            1.0,
            short.id(),
            short_body.photosystem_count(),
            &short_genome,
        );
        // Control: same short plant, empty canopy.
        let open = crate::shade::effective_photo_light(
            &crate::shade::CanopyIndex::default(),
            short_pose.world_x(),
            sample_y,
            1.0,
            short.id(),
            short_body.photosystem_count(),
            &short_genome,
        );
        let tall_top = crate::shade::canopy_top_y(
            store.ecs.get::<&Pose>(tall).unwrap().y,
            &store.ecs.get::<&ModuleBody>(tall).unwrap().blueprint,
        );
        assert!(
            tall_top > sample_y + 0.5,
            "fixture: tall canopy must clear short (tall={tall_top:.2} short={sample_y:.2})"
        );
        assert!(
            shaded < open - 0.12,
            "tall neighbour must cut effective light (open={open:.3} shaded={shaded:.3})"
        );
    }

    #[test]
    fn habit_pop_caps_isolate_algae_from_plants() {
        let mut world = World::new(13);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        // Dry fertile land for plants; a wet band for algae.
        for x in 0..64 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.nutrient = 0.8;
                col.deposit_to_top(MaterialId::Organic, 400, 0);
            }
        }
        for x in 40..56 {
            if let Some(col) = world.column_at_mut(x) {
                col.deposit_to_top(MaterialId::Water, 2_000, 0);
            }
        }
        let mut store = AgentStore::new();
        store.pop_caps = PopCaps {
            algae: 8,
            plant: 1,
            fungus: 0,
        };
        let plant = store
            .spawn_from_blueprint(
                &world,
                10,
                Blueprint::minimal_plant(Genome::default()),
                80.0,
            )
            .expect("first plant");
        assert!(store.ecs.get::<&Organism>(plant).is_ok());
        // Plant cap full — second plant refused.
        assert!(
            store
                .spawn_from_blueprint(
                    &world,
                    12,
                    Blueprint::minimal_plant(Genome::default()),
                    80.0,
                )
                .is_none(),
            "plant cap should block a second land plant"
        );
        // Algae still have room on the wet band.
        assert!(
            store
                .spawn_from_blueprint(&world, 48, Blueprint::atom(Genome::default()), 40.0)
                .is_some(),
            "algae cap is independent of plant cap"
        );
        let (a, p, f) = store.count_by_habit();
        assert_eq!((a, p, f), (1, 1, 0));
        assert!(!store.habit_has_room(OrganismHabit::Fungus, 0, 0));
        assert!(!store.habit_has_room(OrganismHabit::Plant, 0, 0));
        assert!(store.habit_has_room(OrganismHabit::Algae, 0, 0));
    }

}
