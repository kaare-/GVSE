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
pub const MAX_ORGANISMS: usize = 512;

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
/// Dense enough (see material table) to sink under water and build bed ooze.
pub const DEATH_ORGANIC_KG_PER_MODULE: i64 = 40;

/// Ticks a corpse rests on the bed before becoming sediment (~6–7 min at 60 Hz).
/// Long enough that death blooms leave a visible carpet of bodies first.
pub const CORPSE_SETTLE_TICKS: u32 = 24_000;

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

/// Marker: Set A (and later) module-pixel organism.
#[derive(Debug, Clone, Copy, Default)]
pub struct Organism;

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
    g
}

/// Soft cap so messy clone lineages don't paint an unbounded blob.
const MAX_PHOTOSYSTEM_MODULES: usize = 16;

/// Base chance of a morphology clone-error (add/remove a Photosystem),
/// scaled by `(1 - clone_fidelity)`. Perfect fidelity → never; fidelity 0
/// → this rate. Keeps green photosystems able to grow/shrink over gens.
const MORPH_ERROR_BASE: f32 = 0.40;

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
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for m in modules {
        let cx = pose.x + (m.x as f32 - mid_x) * MODULE_CELL_COLS;
        let cy = pose.y + m.y as f32 * MODULE_CELL_COLS;
        min_x = min_x.min(cx - half);
        max_x = max_x.max(cx + half);
        min_y = min_y.min(cy - half);
        max_y = max_y.max(cy + half);
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

fn collect_bodies(store: &AgentStore) -> Vec<(Entity, Aabb)> {
    // Keep ordering deterministic (by entity id) — resolve_collisions
    // uses it for stable tie-break when two AABBs share a center-x.
    let mut out: Vec<(Entity, Aabb)> = store
        .ecs
        .query::<(&Pose, &ModuleBody, &Organism)>()
        .iter()
        .map(|(e, (pose, body, _))| (e, organism_aabb(pose, &body.blueprint)))
        .collect();
    out.sort_by_key(|(e, _)| e.id());
    out
}

fn aabb_hits_any(bodies: &[(Entity, Aabb)], aabb: Aabb, ignore: Option<Entity>) -> bool {
    // Bodies are unsorted-by-x, so linear scan; but callers hit this only
    // per-birth (not per organism per tick), so it's cheap enough.
    bodies.iter().any(|&(e, other)| {
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
    if blueprint.is_rooted() && is_deep_water(col) {
        return None;
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
        return Some(col.surface_y - offset);
    }
    Some(col.surface_y)
}

/// Find a clear pose, scanning **outward horizontally** first (full map width)
/// so clones spread along the water instead of packing into a lens.
fn find_clear_pose(
    world: &World,
    bodies: &[(Entity, Aabb)],
    blueprint: &Blueprint,
    x0: f32,
    y0: f32,
) -> Option<(f32, f32)> {
    let (lo, hi) = world.world_x_bounds()?;
    let lo_f = lo as f32;
    let hi_f = hi as f32 + 0.99;
    let width = organism_width(blueprint);
    let step = (width + COLLISION_PAD).max(MODULE_CELL_COLS);
    let offset = blueprint_body_top_offset(blueprint);

    // Depth rows: primary equilibrium pose, then below/above if full.
    let mut depth_rows = vec![y0];
    if blueprint.is_plankton() {
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
            }
        }
    }

    for &row_y in &depth_rows {
        // k = 0, +1, -1, +2, -2, ... across the whole world.
        let max_k = ((hi_f - lo_f) / step).ceil() as i32 + 1;
        for k in 0..=max_k {
            for sign in [1i32, -1] {
                if k == 0 && sign < 0 {
                    continue;
                }
                let x = (x0 + sign as f32 * k as f32 * step).clamp(lo_f, hi_f);
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
        let mut bodies = collect_bodies(store);
        if bodies.len() < 2 {
            return;
        }
        // Sort by min_x for the sweep. `collect_bodies` sorts by id;
        // resort here since we need spatial order for the broad-phase.
        bodies.sort_by(|(_, a), (_, b)| {
            a.min_x.partial_cmp(&b.min_x).unwrap_or(std::cmp::Ordering::Equal)
        });
        dx.clear();
        dy.clear();
        let mut any = false;
        for i in 0..bodies.len() {
            let (ea, a) = bodies[i];
            for j in (i + 1)..bodies.len() {
                let (eb, b) = bodies[j];
                // Sweep termination: once the next body's min_x is
                // past this one's max_x, no further pair can overlap.
                if b.min_x >= a.max_x {
                    break;
                }
                if !a.overlaps(b) {
                    continue;
                }
                any = true;
                let overlap_x = (a.max_x.min(b.max_x) - a.min_x.max(b.min_x)).max(0.0);
                let overlap_y = (a.max_y.min(b.max_y) - a.min_y.max(b.min_y)).max(0.0);
                if overlap_x <= 0.0 || overlap_y <= 0.0 {
                    continue;
                }

                // Side-view: always shove on X. Use at least MIN_SEPARATION so
                // coincident clones don't stay stuck.
                let push = (overlap_x * 0.5).max(MIN_SEPARATION) + COLLISION_PAD;
                let (s_a, s_b) = if a.center_x() < b.center_x()
                    || (a.center_x() == b.center_x() && ea.id() < eb.id())
                {
                    (-push, push)
                } else {
                    (push, -push)
                };
                dx.push((ea.id(), s_a));
                dx.push((eb.id(), s_b));

                // If both are pinned against the same world edge, fall back to Y.
                let a_at_edge = a.center_x() <= lo_f + 0.1 || a.center_x() >= hi_f - 0.1;
                let b_at_edge = b.center_x() <= lo_f + 0.1 || b.center_x() >= hi_f - 0.1;
                if a_at_edge && b_at_edge && overlap_y > 0.0 {
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
        for (e, aabb) in &bodies {
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
        Some(col.surface_y)
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
        if !blueprint.is_valid_atom() {
            return None;
        }
        if world.column_at(world_x).is_none() {
            return None;
        }

        let y0 = Self::spawn_elevation(world, world_x, &blueprint)?;
        let x0 = world_x as f32 + 0.5;
        let bodies = collect_bodies(self);
        let (x, y) = find_clear_pose(world, &bodies, &blueprint, x0, y0)?;

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
            Organism,
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
            (c.settled_ticks, CORPSE_SETTLE_TICKS)
        });
        let dead = corpse_settle.is_some();
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

        let mut deaths: Vec<(Entity, i32)> = Vec::new();
        // x, y, blueprint, energy, max_e, parent, parent_generation, founder_id
        let mut births: Vec<(f32, f32, Blueprint, f32, f32, Entity, u32, u8)> = Vec::new();

        for (e, (pose, buoy, energy, genome, body, lineage, _)) in self
            .ecs
            .query::<(
                &mut Pose,
                &mut BuoyancyState,
                &mut Energy,
                &Genome,
                &mut ModuleBody,
                &mut Lineage,
                &Organism,
            )>()
            .iter()
        {
            let wx = pose.world_x();
            let n_photo = body.photosystem_count().max(1) as f32;
            let n_roots = body.blueprint.root_count().max(1) as f32;
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

            // Environment gates: water required, ice / freeze kills plankton.
            let (in_water, iced, water_co2, nutrient) = if let Some(col) = world.column_at(wx) {
                let wet = water_band(col).is_some();
                let ice = col.top_ice_mass() > 0;
                (
                    wet,
                    ice,
                    col.ecology.water_co2,
                    crate::root::column_nutrient_factor(col),
                )
            } else {
                (false, false, 0.0, 0.2)
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
                    pose.y = col.surface_y;
                    buoy.vel_y = 0.0;
                    buoy.last_water_top = None;
                }
            }

            if active && (!plankton || in_water) {
                let mut gain = organism_photo_gain(energy.max, l0, n_photo) * comfort;
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
                    gain *= nutrient.clamp(0.05, 1.6);
                }
                energy.current = (energy.current + gain).min(energy.max);
            }

            // Roots drink moisture from host + adjacent columns.
            if rooted && active {
                let moist = crate::root::adjacent_moisture_frac(world, wx);
                let budget = (1.5 + n_roots * 1.2).round() as i64;
                let drunk = crate::root::drink_adjacent(world, wx, budget);
                if drunk > 0 {
                    let sip = drunk as f32 * crate::root::ROOT_WATER_ENERGY * nutrient.max(0.2);
                    energy.current = (energy.current + sip).min(energy.max);
                }
                if moist < 0.08 {
                    energy.current -= crate::root::ROOT_DROUGHT_DRAIN;
                }
            }

            let upkeep = organism_upkeep(genome, energy.max, l0, n_photo);
            // Root tissue has a small upkeep tax (deeper trees cost more).
            let root_tax = if rooted {
                0.02 * body.blueprint.root_count() as f32
            } else {
                0.0
            };
            energy.current -= upkeep + root_tax;
            if energy.current <= 0.0 {
                deaths.push((e, wx));
                continue;
            }

            // Surplas → elongate roots / shoots (material-costed bore).
            if rooted && active && tick % crate::root::ROOT_GROW_PERIOD == (e.id() as u64 % crate::root::ROOT_GROW_PERIOD)
            {
                let (_, _, w_root) = genome.alloc_weights();
                if w_root >= 0.2 {
                    let _ = crate::root::try_elongate_root(
                        &mut body.blueprint,
                        energy,
                        world,
                        pose.x,
                        pose.y,
                        genome,
                        world.seed,
                        tick,
                        e.id(),
                    );
                } else {
                    let _ = crate::root::try_grow_shoot(
                        &mut body.blueprint,
                        energy,
                        genome,
                        world.seed,
                        tick,
                        e.id(),
                    );
                }
                // Occasional opposite allocation so plants aren't pure roots.
                if tick % (crate::root::ROOT_GROW_PERIOD * 3)
                    == (e.id() as u64 % (crate::root::ROOT_GROW_PERIOD * 3))
                {
                    let _ = crate::root::try_grow_shoot(
                        &mut body.blueprint,
                        energy,
                        genome,
                        world.seed,
                        tick,
                        e.id(),
                    );
                }
            }

            // Coarse ecology feedback from live modules.
            if rooted {
                if let Some(col) = world.column_at_mut(wx) {
                    let roots = body.blueprint.root_count() as f32;
                    let leaves = body.photosystem_count() as f32;
                    col.ecology.root_density =
                        (col.ecology.root_density.max(0.05) + roots * 0.04).clamp(0.0, 1.0);
                    col.ecology.leaf_area =
                        (col.ecology.leaf_area.max(0.05) + leaves * 0.05).clamp(0.0, 1.0);
                }
            }

            let phase_id = e.id() as u64 % REPRO_PERIOD;
            let threshold = genome.reproduce_at.clamp(0.2, 0.99);
            // Cold / hot water: no fission outside the comfort band.
            // Threshold is intentionally soft (was 0.35) so noon ocean under
            // thermal fields doesn't sterilise a full-energy Atom while still
            // blocking the E46d cold-but-unfrozen case.
            let can_repro = comfort >= 0.20;
            if population + births.len() < MAX_ORGANISMS
                && tick % REPRO_PERIOD == phase_id
                && energy.current >= energy.max * threshold
                && can_repro
            {
                // Low CloneFidelity → weaker kids, and density-dependent
                // stillbirths (messy booms then fails to replace itself at cap).
                let fidelity = genome.clone_fidelity.clamp(0.0, 1.0);
                let density = (population as f32 / MAX_ORGANISMS as f32).clamp(0.0, 1.0);
                let viability = (fidelity + (1.0 - density) * (1.0 - fidelity)).clamp(0.05, 1.0);
                let viab_h = hash_u64(world.seed, tick as i64, e.id() as i64, 0xB100_D5);
                let viable = (viab_h as f32 / u64::MAX as f32) < viability;
                let child_frac = REPRO_COST_FRAC * (0.35 + 0.65 * fidelity);
                let child_e = energy.current * child_frac;
                if child_e > 1.0 {
                    // Parent always pays the attempt; failed clones are the
                    // messy cost of low fidelity under crowding.
                    energy.current -= child_e;
                    if !viable {
                        continue;
                    }
                    let child_genome = mutate_organism(*genome, world.seed, tick, e.id());
                    let mut child_bp = mutate_blueprint_morphology(
                        body.blueprint.clone(),
                        genome.clone_fidelity,
                        world.seed,
                        tick,
                        e.id(),
                    );
                    child_bp.genome = child_genome;
                    let w = organism_width(&child_bp);
                    let side = if (tick + e.id() as u64) % 2 == 0 {
                        w
                    } else {
                        -w
                    };
                    births.push((
                        pose.x + side,
                        pose.y,
                        child_bp,
                        child_e,
                        energy.max,
                        e,
                        lineage.generation,
                        lineage.founder_id,
                    ));
                }
            }

            if let Some(col) = world.column_at_mut(wx) {
                col.activity = Activity::HydrologyActive;
            }
        }

        // Energy death → corpse (keeps drawing, sinks), litter on dissolve.
        for (e, _wx) in deaths {
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
            collect_bodies(self)
        } else {
            Vec::new()
        };
        for (x0, y0, blueprint, energy, max_e, parent, parent_gen, founder_id) in births {
            if self.organism_count() >= MAX_ORGANISMS {
                break;
            }
            let Some((x, y)) = find_clear_pose(world, &bodies, &blueprint, x0, y0) else {
                continue;
            };
            let host_x = x.floor() as i32;
            let last_water_top = world
                .column_at(host_x)
                .and_then(water_band)
                .map(|(top, _)| top);
            let new_pose = Pose { x, y };
            let new_aabb = organism_aabb(&new_pose, &blueprint);
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
                },
                Organism,
            ));
            bodies.push((new_entity, new_aabb));
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
                // Dry land: feet (y=0) on the ground.
                pose.y = col.surface_y;
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
                if corpse.settled_ticks >= CORPSE_SETTLE_TICKS {
                    dissolve.push((e, wx, n_modules));
                }
            } else {
                corpse.settled_ticks = 0;
            }
        }

        for (e, wx, n_modules) in dissolve {
            let n = n_modules.max(1) as i64;
            let organic_kg = DEATH_ORGANIC_KG_PER_MODULE.saturating_mul(n);
            let litter_kg = DEATH_LITTER_KG_PER_MODULE.saturating_mul(n);
            if let Some(col) = world.column_at_mut(wx) {
                // Stratigraphic Organic — denser than water, sinks to the bed.
                if organic_kg > 0 {
                    col.deposit_to_top(MaterialId::Organic, organic_kg, tick);
                    col.settle_by_density(tick);
                }
                col.ecology.dead_biomass =
                    col.ecology.dead_biomass.saturating_add(litter_kg);
                col.activity = Activity::HydrologyActive;
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
            for m in modules {
                let wx = pose.x + (m.x as f32 - mid_x) * MODULE_CELL_COLS;
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
        let surface = world.column_at(2).unwrap().surface_y;
        assert!(
            (y0 - surface).abs() < 0.05,
            "plant feet on ground y0={y0} surface={surface}"
        );
        let moist_before = world.column_at(2).unwrap().moisture;
        for t in 0..120 {
            store.step_organisms(&mut world, t);
        }
        let moist_after = world.column_at(2).unwrap().moisture;
        assert!(
            moist_after < moist_before,
            "roots should drink host moisture ({moist_before} → {moist_after})"
        );
        let info = store.inspect_organism(e).expect("plant still alive");
        assert!(!info.is_plankton);
        assert!(info.roots >= 1);
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
    fn footprint_collision_prevents_overlap() {
        let mut world = World::new(7);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        let mut store = AgentStore::new();
        let bp = Blueprint::atom(Genome::default());
        assert!(store.spawn_from_blueprint(&world, 4, bp.clone(), 50.0).is_some());
        assert!(store.spawn_from_blueprint(&world, 4, bp.clone(), 50.0).is_some());
        assert!(store.spawn_from_blueprint(&world, 4, bp, 50.0).is_some());
        let bodies = collect_bodies(&store);
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
        let bodies = collect_bodies(&store);
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                assert!(!bodies[i].1.overlaps(bodies[j].1));
            }
        }
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
}
