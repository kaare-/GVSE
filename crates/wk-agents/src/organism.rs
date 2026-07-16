//! Set A organism behaviour (nucleus + photosystem).
//! See `docs/organism/CORE_FEATURES.md` Set A.
//!
//! Plankton float in the column's **flowable water** band (not a constant
//! `sea_level` line). Vertical preference is [`Genome::buoyancy_bias`]
//! (0 = free surface, 1 = water bed). At most one organism occupies a
//! given column at a time so blooms spread sideways instead of stacking.

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

/// Energy gained per photosystem per tick at full noon light.
pub const PHOTON_RATE: f32 = 1.8;

/// Baseline nucleus upkeep (added to metabolic_rate).
pub const NUCLEUS_UPKEEP: f32 = 0.03;

/// Upkeep per photosystem module.
pub const PHOTOSYSTEM_UPKEEP: f32 = 0.03;

/// kg dumped into `dead_biomass` on organism death.
pub const DEATH_LITTER_KG: i64 = 8;

/// How fast plankton Y lerps toward the buoyancy target each tick.
const BUOYANCY_LERP: f32 = 0.4;

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
                // Top of first solid substrate = water bed.
                return Some((top, y));
            }
        }
    }
    Some((top, y))
}

/// Target Y inside the water band. `bias` 0 = surface, 1 = bed.
pub fn buoyancy_target_y(col: &Column, bias: f32) -> Option<f32> {
    let (top, bed) = water_band(col)?;
    let t = bias.clamp(0.0, 1.0);
    Some(top + (bed - top) * t)
}

/// True when the column holds enough standing water that rooted plants
/// cannot establish (open water / lake / sea column).
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
    g.buoyancy_bias = jitter(g.buoyancy_bias, 0.0, 1.0);
    g
}

fn occupied_columns(store: &AgentStore) -> std::collections::HashSet<i32> {
    let mut set = std::collections::HashSet::new();
    for (_e, (pose, _)) in store.ecs.query::<(&Pose, &Organism)>().iter() {
        set.insert(pose.world_x());
    }
    set
}

/// Nearest free column to `prefer`, optionally requiring / forbidding deep water.
fn find_free_column(
    world: &World,
    prefer: i32,
    occupied: &std::collections::HashSet<i32>,
    want_water: Option<bool>,
) -> Option<i32> {
    let (lo, hi) = world.world_x_bounds()?;
    for d in 0..=(hi - lo).max(0) {
        for sign in [-1i32, 1] {
            if d == 0 && sign < 0 {
                continue;
            }
            let c = prefer.saturating_add(sign * d);
            if c < lo || c > hi {
                continue;
            }
            if occupied.contains(&c) {
                continue;
            }
            let Some(col) = world.column_at(c) else {
                continue;
            };
            match want_water {
                Some(true) if water_band(col).is_none() => continue,
                Some(false) if is_deep_water(col) => continue,
                _ => {}
            }
            return Some(c);
        }
    }
    None
}

impl AgentStore {
    pub fn organism_count(&self) -> usize {
        self.ecs.query::<&Organism>().iter().count()
    }

    /// Elevation for a newly spawned / cloned organism.
    /// Plankton ride the real free-water band (buoyancy gene); without
    /// water they rest on the sediment surface. Rooted sit on land surface.
    pub fn spawn_elevation(world: &World, world_x: i32, blueprint: &Blueprint) -> Option<f32> {
        let col = world.column_at(world_x)?;
        if blueprint.is_plankton() {
            if let Some(y) = buoyancy_target_y(col, blueprint.genome.buoyancy_bias) {
                return Some(y);
            }
            return Some(col.surface_y);
        }
        Some(col.surface_y)
    }

    /// Spawn a blueprint-backed organism at column `world_x`.
    /// Plankton may occupy any free column (preferring water when present);
    /// rooted designs need a free non-deep-water column. Never stacks on
    /// an already-occupied column.
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

        let occupied = occupied_columns(self);
        let want_water = if blueprint.is_plankton() {
            // Prefer water if the click/parent column has it; else any free col.
            world
                .column_at(world_x)
                .and_then(|c| water_band(c))
                .map(|_| true)
        } else {
            Some(false)
        };

        let world_x = if occupied.contains(&world_x) {
            find_free_column(world, world_x, &occupied, want_water)?
        } else if blueprint.is_rooted() {
            let col = world.column_at(world_x)?;
            if is_deep_water(col) {
                find_free_column(world, world_x, &occupied, Some(false))?
            } else {
                world_x
            }
        } else {
            world_x
        };

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

    /// Set A behaviour pass: harvest light, buoyancy, exclusion, reproduce, die.
    pub fn step_organisms(&mut self, world: &mut World, tick: u64) {
        if self.organism_count() == 0 {
            return;
        }
        self.wake_host_columns(world);

        let l0 = sky_light_l0(world.climate.day_night_factor(tick));
        let phase = world.climate.phase_fraction(tick);
        let population = self.organism_count();

        // --- Spatial exclusion: one organism per column -----------------
        {
            let mut rows: Vec<(Entity, i32, bool)> = self
                .ecs
                .query::<(&Pose, &ModuleBody, &Organism)>()
                .iter()
                .map(|(e, (pose, body, _))| (e, pose.world_x(), body.blueprint.is_plankton()))
                .collect();
            rows.sort_by_key(|(e, ..)| e.id());

            let mut claimed = std::collections::HashSet::new();
            let mut moves: Vec<(Entity, i32)> = Vec::new();
            for &(e, wx, plankton) in &rows {
                if claimed.insert(wx) {
                    continue;
                }
                let want = if plankton {
                    world
                        .column_at(wx)
                        .and_then(water_band)
                        .map(|_| true)
                } else {
                    Some(false)
                };
                if let Some(free) = find_free_column(world, wx, &claimed, want) {
                    claimed.insert(free);
                    moves.push((e, free));
                }
            }
            for (e, new_wx) in moves {
                if let Ok(mut pose) = self.ecs.get::<&mut Pose>(e) {
                    pose.x = new_wx as f32 + 0.5;
                }
            }
        }

        let mut deaths: Vec<(Entity, i32)> = Vec::new();
        let mut births: Vec<(i32, Blueprint, f32, f32)> = Vec::new();

        for (e, (pose, energy, genome, body, _)) in self
            .ecs
            .query::<(&mut Pose, &mut Energy, &Genome, &ModuleBody, &Organism)>()
            .iter()
        {
            let wx = pose.world_x();
            let n_photo = body.photosystem_count().max(1) as f32;
            let active = circadian_active(genome, phase);

            // Buoyancy / grounding against the live water column.
            if let Some(col) = world.column_at(wx) {
                if body.blueprint.is_plankton() {
                    if let Some(target) = buoyancy_target_y(col, genome.buoyancy_bias) {
                        pose.y += (target - pose.y) * BUOYANCY_LERP;
                        if let Some((top, bed)) = water_band(col) {
                            pose.y = pose.y.clamp(bed.min(top), bed.max(top));
                        }
                    } else {
                        // Water gone — rest on sediment (no constant ocean line).
                        pose.y = col.surface_y;
                    }
                } else {
                    pose.y = col.surface_y;
                }
            }

            if active && l0 > 0.01 {
                let gain = PHOTON_RATE * l0 * n_photo;
                energy.current = (energy.current + gain).min(energy.max);
            }

            let upkeep = genome.metabolic_rate + NUCLEUS_UPKEEP + PHOTOSYSTEM_UPKEEP * n_photo;
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
                    births.push((wx, child_bp, child_e, energy.max));
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

        for (parent_wx, blueprint, energy, max_e) in births {
            if self.organism_count() >= MAX_ORGANISMS {
                break;
            }
            let occupied = occupied_columns(self);
            let want = if blueprint.is_plankton() {
                world
                    .column_at(parent_wx)
                    .and_then(water_band)
                    .map(|_| true)
            } else {
                Some(false)
            };
            // Always take a free neighbour — never stack on the parent column.
            let Some(child_wx) = find_free_column(world, parent_wx, &occupied, want)
                .or_else(|| find_free_column(world, parent_wx, &occupied, None))
            else {
                continue;
            };
            let child_y = Self::spawn_elevation(world, child_wx, &blueprint).unwrap_or_else(|| {
                world
                    .column_at(child_wx)
                    .map(|c| c.surface_y)
                    .unwrap_or(0.0)
            });
            self.ecs.spawn((
                Pose {
                    x: child_wx as f32 + 0.5,
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

#[cfg(test)]
mod tests {
    use super::*;
    use wk_world::terrain::generate_flat_sand;
    use wk_world::world::World;

    fn world_with_water() -> World {
        let mut world = World::new(42);
        world.sea_level = 10.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 5.0));
        // Flood a mid column with standing water.
        if let Some(col) = world.column_at_mut(8) {
            col.deposit_to_top(MaterialId::Water, 2_000, 0);
        }
        world
    }

    #[test]
    fn buoyancy_zero_near_free_surface() {
        let world = world_with_water();
        let col = world.column_at(8).unwrap();
        let (top, bed) = water_band(col).expect("water");
        assert!(top > bed);
        let y = buoyancy_target_y(col, 0.0).unwrap();
        assert!((y - top).abs() < 1e-3);
    }

    #[test]
    fn buoyancy_one_near_bed() {
        let world = world_with_water();
        let col = world.column_at(8).unwrap();
        let (_top, bed) = water_band(col).expect("water");
        let y = buoyancy_target_y(col, 1.0).unwrap();
        assert!((y - bed).abs() < 1e-3);
    }

    #[test]
    fn no_stacking_same_column() {
        let mut world = World::new(7);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        let mut store = AgentStore::new();
        let bp = Blueprint::atom(Genome::default());
        let a = store.spawn_from_blueprint(&world, 4, bp.clone(), 50.0);
        let b = store.spawn_from_blueprint(&world, 4, bp, 50.0);
        assert!(a.is_some() && b.is_some());
        let cols: Vec<i32> = store
            .ecs
            .query::<(&Pose, &Organism)>()
            .iter()
            .map(|(_, (p, _))| p.world_x())
            .collect();
        assert_eq!(cols.len(), 2);
        assert_ne!(cols[0], cols[1]);
    }

    #[test]
    fn spawn_ignores_constant_sea_level() {
        let mut world = World::new(9);
        // Constant ocean line would put plankton at sea_level - 0.35 = 9.65,
        // but the free water surface on this flooded column is much lower.
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
        assert!(
            (y - top).abs() < 0.5,
            "expected near free-water top {top}, got {y} (sea_level={})",
            world.sea_level
        );
        assert!((y - (world.sea_level - 0.35)).abs() > 1.0);
    }
}
