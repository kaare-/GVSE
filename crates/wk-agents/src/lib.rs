//! ECS creature layer (stages 10–11 + Organism Kernel Set A).
//!
//! Agents live in a [`hecs::World`] beside the column stack. They read
//! world state and call [`wk_world::World`] APIs (`dig`, `eat_biomass`,
//! `drink_water`). Stage 11 adds reproduction with deterministic genome
//! mutation and light environmental stress. Set A adds blueprint-backed
//! photo-organisms. See `docs/AGENTS.md`, `docs/EVOLUTION.md`, and
//! `docs/organism/`.

pub mod blueprint;
pub mod module;
pub mod organism;

pub use blueprint::{Blueprint, PlacedModule, Wire, WireKind, BLUEPRINT_DIR};
pub use module::{LaneId, ModuleId};
pub use organism::{
    Aabb, Corpse, Lineage, ModuleBody, Organism, OrganismInspect, MAX_ORGANISMS, MODULE_CELL_COLS,
    PHOTON_RATE,
};

use hecs::{Entity, World as EcsWorld};
use serde::{Deserialize, Serialize};
use wk_world::column::Activity;
use wk_world::terrain::hash_u64;
use wk_world::world::World;

/// Soft population ceiling — keeps soak / scenarios bounded.
pub const MAX_AGENTS: usize = 64;

/// Reproduce when current energy ≥ this fraction of max.
pub const REPRO_ENERGY_FRAC: f32 = 0.82;

/// Fraction of current energy transferred to the offspring.
pub const REPRO_COST_FRAC: f32 = 0.45;

/// Minimum ticks between reproduction attempts for one agent
/// (phase-staggered by entity id).
pub const REPRO_PERIOD: u64 = 90;

/// Relative mutation amplitude per trait (± this fraction).
pub const MUTATION_SIGMA: f32 = 0.12;

/// Extra energy drain when the host column is dry.
pub const DESICCATION_DRAIN: f32 = 0.15;

/// Extra energy drain when host temperature is below freezing.
pub const COLD_DRAIN: f32 = 0.25;

/// Horizontal position in column units (fractional) and elevation (m).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Pose {
    pub x: f32,
    pub y: f32,
}

impl Pose {
    pub fn world_x(self) -> i32 {
        self.x.floor() as i32
    }
}

/// Metabolic energy pool. `current <= 0` means the agent dies this step.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Energy {
    pub current: f32,
    pub max: f32,
}

/// Genome trait vector — mutated on reproduction (stage 11 / Set A).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Genome {
    /// Columns stepped toward forage per agent tick (typical 0.2–1.0).
    pub move_speed: f32,
    /// kg biomass taken per successful graze.
    pub graze_rate: f32,
    /// kg water taken per successful drink.
    pub drink_rate: f32,
    /// 0..1 chance-like drive to dig when energy is middling.
    pub dig_drive: f32,
    /// Energy gained per kg biomass eaten.
    pub graze_efficiency: f32,
    /// Basal energy drain per agent tick (renamed from `metabolism`).
    #[serde(alias = "metabolism", default = "default_metabolic_rate")]
    pub metabolic_rate: f32,
    /// 0..1 gate on reproduction attempts (0 disables fission).
    pub repro_drive: f32,
    /// Energy fraction of max required to attempt fission (Set A).
    #[serde(default = "default_reproduce_at")]
    pub reproduce_at: f32,
    /// 1 = exact clone; 0 = full mutation strength (Set A).
    #[serde(default = "default_clone_fidelity")]
    pub clone_fidelity: f32,
    /// Preferred phase within the day cycle (0..1) (Set A).
    #[serde(default = "default_circadian_phase")]
    pub circadian_phase: f32,
    /// Fraction of the day the organism is active (0..1) (Set A).
    #[serde(default = "default_active_window")]
    pub active_window: f32,
    /// Relative sink tendency for plankton (0..1).
    /// `0` = buoyant floater (rides ~1 m under the live free-water surface);
    /// `1` = heavy (settles on the water bed). Motion is weight vs buoyancy,
    /// not a snap to a fixed ocean line.
    #[serde(default = "default_buoyancy_bias")]
    pub buoyancy_bias: f32,
}

fn default_metabolic_rate() -> f32 {
    0.35
}
fn default_reproduce_at() -> f32 {
    0.7
}
fn default_clone_fidelity() -> f32 {
    0.6
}
fn default_circadian_phase() -> f32 {
    0.25
}
fn default_active_window() -> f32 {
    0.55
}
fn default_buoyancy_bias() -> f32 {
    0.0 // floater: ~1 m under the live free-water surface
}

impl Default for Genome {
    fn default() -> Self {
        Self {
            move_speed: 0.35,
            graze_rate: 40.0,
            drink_rate: 30.0,
            dig_drive: 0.15,
            graze_efficiency: 0.08,
            metabolic_rate: default_metabolic_rate(),
            repro_drive: 0.55,
            reproduce_at: default_reproduce_at(),
            clone_fidelity: default_clone_fidelity(),
            circadian_phase: default_circadian_phase(),
            active_window: default_active_window(),
            buoyancy_bias: default_buoyancy_bias(),
        }
    }
}

impl Genome {
    /// Deterministic per-trait mutation of `parent`.
    pub fn mutate(parent: Genome, world_seed: u64, tick: u64, parent_id: u32) -> Genome {
        let mut g = parent;
        let salt_base = tick
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add(parent_id as u64);
        let mut trait_i = 0u64;
        let mut jitter = |value: f32, lo: f32, hi: f32| -> f32 {
            trait_i += 1;
            let h = hash_u64(world_seed, salt_base as i64, trait_i as i64, 0xE11);
            // Map to [-1, 1].
            let u = (h as f32 / u64::MAX as f32) * 2.0 - 1.0;
            (value * (1.0 + u * MUTATION_SIGMA)).clamp(lo, hi)
        };
        g.move_speed = jitter(g.move_speed, 0.05, 1.5);
        g.graze_rate = jitter(g.graze_rate, 5.0, 120.0);
        g.drink_rate = jitter(g.drink_rate, 5.0, 100.0);
        g.dig_drive = jitter(g.dig_drive, 0.0, 1.0);
        g.graze_efficiency = jitter(g.graze_efficiency, 0.01, 0.25);
        g.metabolic_rate = jitter(g.metabolic_rate, 0.05, 1.2);
        g.repro_drive = jitter(g.repro_drive, 0.0, 1.0);
        g.reproduce_at = jitter(g.reproduce_at, 0.3, 0.95);
        g.clone_fidelity = jitter(g.clone_fidelity, 0.0, 1.0);
        g.circadian_phase = jitter(g.circadian_phase, 0.0, 1.0);
        g.active_window = jitter(g.active_window, 0.1, 1.0);
        g.buoyancy_bias = jitter(g.buoyancy_bias, 0.0, 1.0);
        g
    }

    /// True if any trait differs beyond a tiny epsilon.
    pub fn differs_from(self, other: Genome) -> bool {
        const EPS: f32 = 1e-4;
        (self.move_speed - other.move_speed).abs() > EPS
            || (self.graze_rate - other.graze_rate).abs() > EPS
            || (self.drink_rate - other.drink_rate).abs() > EPS
            || (self.dig_drive - other.dig_drive).abs() > EPS
            || (self.graze_efficiency - other.graze_efficiency).abs() > EPS
            || (self.metabolic_rate - other.metabolic_rate).abs() > EPS
            || (self.repro_drive - other.repro_drive).abs() > EPS
            || (self.reproduce_at - other.reproduce_at).abs() > EPS
            || (self.clone_fidelity - other.clone_fidelity).abs() > EPS
            || (self.circadian_phase - other.circadian_phase).abs() > EPS
            || (self.active_window - other.active_window).abs() > EPS
            || (self.buoyancy_bias - other.buoyancy_bias).abs() > EPS
    }
}

/// Marker: scripted surface grazer.
#[derive(Debug, Clone, Copy, Default)]
pub struct Grazer;

/// All agent entities for one simulation.
#[derive(Default)]
pub struct AgentStore {
    pub ecs: EcsWorld,
    /// Cumulative successful births (stage 11 bookkeeping / tests).
    pub births_total: u64,
}

impl AgentStore {
    pub fn new() -> Self {
        Self {
            ecs: EcsWorld::new(),
            births_total: 0,
        }
    }

    pub fn len(&self) -> u32 {
        self.ecs.iter().count() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Spawn a scripted grazer at column `world_x`, surface elevation.
    /// `energy` is both the starting pool and the max (see
    /// [`spawn_grazer_energy`] to set them separately).
    pub fn spawn_grazer(
        &mut self,
        world: &World,
        world_x: i32,
        genome: Genome,
        energy: f32,
    ) -> Option<Entity> {
        let max_e = energy.max(1.0);
        self.spawn_grazer_energy(world, world_x, genome, energy, max_e)
    }

    /// Spawn with explicit current / max energy.
    pub fn spawn_grazer_energy(
        &mut self,
        world: &World,
        world_x: i32,
        genome: Genome,
        energy: f32,
        max_energy: f32,
    ) -> Option<Entity> {
        if self.grazer_count() >= MAX_AGENTS {
            return None;
        }
        let col = world.column_at(world_x)?;
        let y = col.surface_y;
        let max_e = max_energy.max(1.0);
        let e = self.ecs.spawn((
            Pose {
                x: world_x as f32 + 0.5,
                y,
            },
            Energy {
                current: energy.clamp(0.0, max_e),
                max: max_e,
            },
            genome,
            Grazer,
        ));
        Some(e)
    }

    /// Collect host column indices so hydrology stays awake under agents.
    pub fn collect_keep_awake(&self) -> Vec<i32> {
        let mut xs = Vec::new();
        for (_, pose) in self.ecs.query::<&Pose>().iter() {
            xs.push(pose.world_x());
        }
        xs.sort_unstable();
        xs.dedup();
        xs
    }

    /// Apply `world.agent_keep_awake` and force those columns active.
    pub fn wake_host_columns(&self, world: &mut World) {
        let xs = self.collect_keep_awake();
        world.agent_keep_awake = xs.clone();
        for wx in xs {
            if let Some(col) = world.column_at_mut(wx) {
                col.activity = Activity::HydrologyActive;
            }
            // Soft-wake the whole host chunk so neighbour flow still runs.
            let coord = World::chunk_coord_for_world_x(wx);
            if let Some(chunk) = world.chunks.get_mut(&coord) {
                chunk.set_all_active();
            }
        }
    }

    /// One scripted-grazer behaviour pass (forage, stress, reproduce).
    pub fn step_grazers(&mut self, world: &mut World, tick: u64) {
        self.wake_host_columns(world);

        let mut moves: Vec<(Entity, f32, f32)> = Vec::new();
        let mut digs: Vec<(Entity, i32, f32, i64)> = Vec::new();
        let mut deaths: Vec<Entity> = Vec::new();
        // (x, y, genome, energy, max_energy)
        let mut births: Vec<(f32, f32, Genome, f32, f32)> = Vec::new();
        let population = self.grazer_count();

        for (e, (pose, energy, genome, _)) in self
            .ecs
            .query::<(&Pose, &mut Energy, &Genome, &Grazer)>()
            .iter()
        {
            let wx = pose.world_x();
            // Grazers forage whenever they have headroom — drinking alone
            // used to keep energy above a tight "hungry" band forever, so
            // biomass was never taken.
            let can_graze = energy.current < energy.max;
            let thirsty = energy.current < energy.max * 0.9;

            if can_graze {
                let eaten = world.eat_biomass(wx, genome.graze_rate as i64);
                if eaten > 0 {
                    energy.current = (energy.current
                        + eaten as f32 * genome.graze_efficiency)
                        .min(energy.max);
                }
            }
            if thirsty {
                let drank = world.drink_water(wx, genome.drink_rate as i64);
                if drank > 0 {
                    energy.current = (energy.current + drank as f32 * 0.01).min(energy.max);
                }
            }

            // Opportunistic dig when middling energy and drive is on.
            let dig_roll = ((tick.wrapping_add(e.id() as u64).wrapping_mul(1103515245)) % 1000)
                as f32
                / 1000.0;
            if energy.current > energy.max * 0.25
                && energy.current < energy.max * 0.7
                && dig_roll < genome.dig_drive
            {
                let y = pose.y - 1.0;
                digs.push((e, wx, y, 80));
            }

            // Forage: step toward richer neighbour alive biomass.
            let here = world
                .column_at(wx)
                .map(|c| c.ecology.alive_biomass)
                .unwrap_or(0);
            let left = world
                .column_at(wx - 1)
                .map(|c| c.ecology.alive_biomass)
                .unwrap_or(0);
            let right = world
                .column_at(wx + 1)
                .map(|c| c.ecology.alive_biomass)
                .unwrap_or(0);
            let mut dx = 0.0f32;
            if left > here && left >= right {
                dx = -genome.move_speed;
            } else if right > here {
                dx = genome.move_speed;
            } else if left == 0 && right == 0 && here == 0 {
                // Wander deterministically when the local patch is bare.
                dx = if (tick + e.id() as u64) % 2 == 0 {
                    genome.move_speed * 0.5
                } else {
                    -genome.move_speed * 0.5
                };
            }

            // Environmental stress (stage 11).
            let mut drain = genome.metabolic_rate;
            let dry = world
                .column_at(wx)
                .map(|c| c.top_water_mass() <= 0 && c.moisture <= 0)
                .unwrap_or(true);
            if dry {
                drain += DESICCATION_DRAIN;
            }
            let temp = world.temperature_at_point(wx, pose.y, tick);
            if temp < 0.0 {
                drain += COLD_DRAIN;
            }
            energy.current -= drain;
            if energy.current <= 0.0 {
                deaths.push(e);
                continue;
            }

            // Reproduction when well-fed, under the soft cap, and due.
            let phase = e.id() as u64 % REPRO_PERIOD;
            let repro_roll =
                ((tick.wrapping_add(e.id() as u64).wrapping_mul(48271)) % 1000) as f32 / 1000.0;
            if population + births.len() < MAX_AGENTS
                && genome.repro_drive > 0.0
                && tick % REPRO_PERIOD == phase
                && repro_roll < genome.repro_drive
                && energy.current >= energy.max * REPRO_ENERGY_FRAC
            {
                let child_e = energy.current * REPRO_COST_FRAC;
                if child_e > 1.0 {
                    energy.current -= child_e;
                    let child_genome = Genome::mutate(*genome, world.seed, tick, e.id());
                    let child_x = pose.x
                        + if (tick + e.id() as u64) % 2 == 0 {
                            0.35
                        } else {
                            -0.35
                        };
                    births.push((child_x, pose.y, child_genome, child_e, energy.max));
                }
            }

            let mut new_x = pose.x + dx;
            let mut new_wx = new_x.floor() as i32;
            // Stay inside loaded columns — wandering off-map starves agents.
            if world.column_at(new_wx).is_none() {
                new_x = pose.x;
                new_wx = wx;
            }
            let new_y = world
                .column_at(new_wx)
                .map(|c| c.surface_y)
                .unwrap_or(pose.y);
            moves.push((e, new_x, new_y));
        }

        for (e, wx, y, kg) in digs {
            if self.ecs.contains(e) {
                let _ = world.dig(wx, y, kg);
            }
        }
        for (e, x, y) in moves {
            if let Ok(mut pose) = self.ecs.get::<&mut Pose>(e) {
                pose.x = x;
                pose.y = y;
            }
        }
        for e in deaths {
            let _ = self.ecs.despawn(e);
        }
        for (x, y, genome, energy, max_e) in births {
            if self.grazer_count() >= MAX_AGENTS {
                break;
            }
            let mut child_x = x;
            let child_wx = child_x.floor() as i32;
            if world.column_at(child_wx).is_none() {
                // Snap back onto a loaded column if the offset walked off-map.
                if let Some((_, pose)) = self.ecs.query::<&Pose>().iter().next() {
                    child_x = pose.x;
                } else {
                    continue;
                }
            }
            let surface_y = world
                .column_at(child_x.floor() as i32)
                .map(|c| c.surface_y)
                .unwrap_or(y);
            self.ecs.spawn((
                Pose {
                    x: child_x,
                    y: surface_y,
                },
                Energy {
                    current: energy.clamp(0.0, max_e),
                    max: max_e,
                },
                genome,
                Grazer,
            ));
            self.births_total += 1;
        }

        self.wake_host_columns(world);
    }

    /// Alive grazer count (for tests / HUD).
    pub fn grazer_count(&self) -> usize {
        self.ecs.query::<&Grazer>().iter().count()
    }

    /// Sum of current energy across grazers.
    pub fn total_energy(&self) -> f32 {
        self.ecs
            .query::<&Energy>()
            .iter()
            .map(|(_, e)| e.current.max(0.0))
            .sum()
    }

    /// Collect all grazer genomes (deterministic entity iteration order).
    pub fn genomes(&self) -> Vec<Genome> {
        self.ecs
            .query::<&Genome>()
            .iter()
            .map(|(_, g)| *g)
            .collect()
    }

    /// Mean metabolism across living grazers (0 if empty).
    pub fn mean_metabolism(&self) -> f32 {
        let gs = self.genomes();
        if gs.is_empty() {
            return 0.0;
        }
        gs.iter().map(|g| g.metabolic_rate).sum::<f32>() / gs.len() as f32
    }
}
