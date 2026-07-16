//! Set A organism behaviour (nucleus + photosystem).
//! See `docs/organism/CORE_FEATURES.md` Set A.

use hecs::Entity;
use serde::{Deserialize, Serialize};
use wk_world::column::Activity;
use wk_world::terrain::hash_u64;
use wk_world::world::World;

use crate::blueprint::Blueprint;
use crate::{
    AgentStore, Energy, Genome, Pose, MAX_AGENTS, MUTATION_SIGMA, REPRO_COST_FRAC, REPRO_PERIOD,
};

/// Soft cap for module organisms (separate from grazers).
pub const MAX_ORGANISMS: usize = 256;

/// Energy gained per photosystem per tick at full noon light.
pub const PHOTON_RATE: f32 = 1.8;

/// Baseline nucleus upkeep (added to metabolic_rate).
pub const NUCLEUS_UPKEEP: f32 = 0.03;

/// Upkeep per photosystem module.
pub const PHOTOSYSTEM_UPKEEP: f32 = 0.03;

/// kg dumped into `dead_biomass` on organism death.
pub const DEATH_LITTER_KG: i64 = 8;

/// Marker: Set A (and later) module-pixel organism.
#[derive(Debug, Clone, Copy, Default)]
pub struct Organism;

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

/// Map climate day/night factor (−1..1) to sky light 0..1.
pub fn sky_light_l0(day_night_factor: f32) -> f32 {
    ((day_night_factor + 1.0) * 0.5).clamp(0.0, 1.0)
}

/// True if the organism's circadian window is open this tick.
pub fn circadian_active(genome: &Genome, phase_fraction: f32) -> bool {
    let half = (genome.active_window * 0.5).clamp(0.0, 0.5);
    if half <= 0.0 {
        return false;
    }
    // Circular distance on [0,1).
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
    // Extra jitter on Set A genes proportional to infidelity.
    let strength = (1.0 - parent.clone_fidelity).clamp(0.0, 1.0) * MUTATION_SIGMA;
    if strength <= 1e-6 {
        return parent; // perfect clone
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
    g
}

impl AgentStore {
    pub fn organism_count(&self) -> usize {
        self.ecs.query::<&Organism>().iter().count()
    }

    /// Elevation for a newly spawned / cloned organism.
    /// Plankton (Atom without root/stem) sit in the lit water band near
    /// `sea_level`; rooted blueprints sit on the column surface.
    pub fn spawn_elevation(world: &World, world_x: i32, blueprint: &Blueprint) -> Option<f32> {
        let col = world.column_at(world_x)?;
        let submerged = col.surface_y <= world.sea_level;
        if blueprint.is_plankton() || submerged {
            // Float just under the free surface — algae lit-band default.
            // Full buoyancy (Set C) will steer this later.
            Some(world.sea_level - 0.35)
        } else if blueprint.is_rooted() || !submerged {
            Some(col.surface_y)
        } else {
            Some(world.sea_level - 0.35)
        }
    }

    /// Spawn a blueprint-backed organism at column `world_x`.
    /// Plankton Atoms may spawn in ocean columns; rooted designs need land.
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
        let col = world.column_at(world_x)?;
        let submerged = col.surface_y <= world.sea_level;
        if blueprint.is_rooted() && submerged {
            // Land plants can't establish on open water (yet).
            return None;
        }
        let y = Self::spawn_elevation(world, world_x, &blueprint)?;
        let max_e = energy.max(1.0).max(40.0);
        let e = self.ecs.spawn((
            Pose {
                x: world_x as f32 + 0.5,
                y,
            },
            Energy {
                current: energy.clamp(0.0, max_e),
                max: max_e,
            },
            blueprint.genome,
            ModuleBody { blueprint },
            Organism,
        ));
        Some(e)
    }

    /// Set A behaviour pass: harvest light, upkeep, reproduce, die → litter.
    pub fn step_organisms(&mut self, world: &mut World, tick: u64) {
        if self.organism_count() == 0 {
            return;
        }
        self.wake_host_columns(world);

        let l0 = sky_light_l0(world.climate.day_night_factor(tick));
        let phase = world.climate.phase_fraction(tick);
        let population = self.organism_count();

        let mut deaths: Vec<(Entity, i32)> = Vec::new();
        let mut births: Vec<(f32, f32, Blueprint, f32, f32)> = Vec::new();

        for (e, (pose, energy, genome, body, _)) in self
            .ecs
            .query::<(&Pose, &mut Energy, &Genome, &ModuleBody, &Organism)>()
            .iter()
        {
            let wx = pose.world_x();
            let n_photo = body.photosystem_count().max(1) as f32;
            let active = circadian_active(genome, phase);

            if active && l0 > 0.01 {
                let gain = PHOTON_RATE * l0 * n_photo;
                energy.current = (energy.current + gain).min(energy.max);
            }

            let upkeep = genome.metabolic_rate
                + NUCLEUS_UPKEEP
                + PHOTOSYSTEM_UPKEEP * n_photo;
            energy.current -= upkeep;
            if energy.current <= 0.0 {
                deaths.push((e, wx));
                continue;
            }

            let phase_id = e.id() as u64 % REPRO_PERIOD;
            let threshold = genome.reproduce_at.clamp(0.2, 0.99);
            if population + births.len() < MAX_ORGANISMS
                && tick % REPRO_PERIOD == phase_id
                && energy.current >= energy.max * threshold
            {
                let child_e = energy.current * REPRO_COST_FRAC;
                if child_e > 1.0 {
                    energy.current -= child_e;
                    let child_genome = mutate_organism(*genome, world.seed, tick, e.id());
                    let mut child_bp = body.blueprint.clone();
                    child_bp.genome = child_genome;
                    let child_x = pose.x
                        + if (tick + e.id() as u64) % 2 == 0 {
                            0.6
                        } else {
                            -0.6
                        };
                    births.push((child_x, pose.y, child_bp, child_e, energy.max));
                }
            }

            if let Some(col) = world.column_at_mut(wx) {
                col.activity = Activity::HydrologyActive;
            }
        }

        for (e, wx) in deaths {
            // Creature body was outside total_tracked; entering as litter is a
            // grow-side source (same bucket plants use when they create biomass).
            if let Some(col) = world.column_at_mut(wx) {
                col.ecology.dead_biomass =
                    col.ecology.dead_biomass.saturating_add(DEATH_LITTER_KG);
            }
            world.mass_audit.biomass_grow_total =
                world.mass_audit.biomass_grow_total.saturating_add(DEATH_LITTER_KG);
            let _ = self.ecs.despawn(e);
        }

        for (x, y, blueprint, energy, max_e) in births {
            if self.organism_count() >= MAX_ORGANISMS {
                break;
            }
            let mut child_x = x;
            let mut child_wx = child_x.floor() as i32;
            if world.column_at(child_wx).is_none() {
                child_x = x.clamp(
                    world.world_x_bounds().map(|(a, _)| a as f32).unwrap_or(0.0),
                    world
                        .world_x_bounds()
                        .map(|(_, b)| b as f32)
                        .unwrap_or(child_x),
                );
                child_wx = child_x.floor() as i32;
            }
            if world.column_at(child_wx).is_none() {
                continue;
            }
            // Keep plankton in the water column; rooted kids snap to surface.
            let child_y = Self::spawn_elevation(world, child_wx, &blueprint).unwrap_or(y);
            self.ecs.spawn((
                Pose {
                    x: child_x,
                    y: child_y,
                },
                Energy {
                    current: energy.clamp(0.0, max_e),
                    max: max_e,
                },
                blueprint.genome,
                ModuleBody { blueprint },
                Organism,
            ));
            self.births_total += 1;
        }

        self.wake_host_columns(world);
    }

    /// Collect drawable module quads for the renderer.
    /// Returns (world_x_frac, world_y_m, module_rgb).
    pub fn organism_draw_list(&self) -> Vec<(f32, f32, (u8, u8, u8))> {
        let mut out = Vec::new();
        for (_, (pose, body, _)) in self
            .ecs
            .query::<(&Pose, &ModuleBody, &Organism)>()
            .iter()
        {
            for m in &body.blueprint.modules {
                // Blueprint y: 0 = ground-relative; above-ground positive in editor.
                // World: pose.y is surface; module local y lifts above surface.
                let wx = pose.x + m.x as f32;
                let wy = pose.y + m.y as f32 * 0.25; // 0.25 m per module cell
                out.push((wx, wy, m.module.rgb()));
            }
        }
        out
    }
}
