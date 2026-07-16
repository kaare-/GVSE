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
/// with painted modules — this is what the collision AABB uses.
pub const MODULE_CELL_COLS: f32 = 0.35;

/// Slop so settled bodies don't jitter on touching edges.
const COLLISION_EPS: f32 = 0.02;

/// Energy gained per photosystem per tick at full noon light.
pub const PHOTON_RATE: f32 = 1.8;

/// Baseline nucleus upkeep (added to metabolic_rate).
pub const NUCLEUS_UPKEEP: f32 = 0.03;

/// Upkeep per photosystem module.
pub const PHOTOSYSTEM_UPKEEP: f32 = 0.03;

/// kg dumped into `dead_biomass` on organism death.
pub const DEATH_LITTER_KG: i64 = 8;

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

/// Spawn / query helper — equilibrium if water exists, else sediment surface.
pub fn buoyancy_target_y(col: &Column, bias: f32) -> Option<f32> {
    let (top, bed) = water_band(col)?;
    Some(equilibrium_y(top, bed, bias))
}

/// Integrate one tick of weight vs buoyancy. Updates `y` and `state`.
pub fn step_buoyancy(y: &mut f32, state: &mut BuoyancyState, col: &Column, bias: f32) {
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
                let was_in_column = *y <= prev_top + 0.25 && *y >= bed - 0.25;
                if was_in_column {
                    if delta > 0.0 {
                        // Rising water lifts the body with the column.
                        *y += delta;
                    } else {
                        // Falling free surface: floaters (light) follow it down;
                        // heavy bodies keep their depth until buoyancy says otherwise.
                        let dens = relative_density(bias);
                        if dens < 1.0 {
                            let float_y = equilibrium_y(prev_top, bed, 0.0);
                            if (*y - float_y).abs() < FLOAT_DEPTH_M + 0.5 {
                                *y = (*y + delta).max(bed);
                            }
                        }
                    }
                }
            }
        }
        state.last_water_top = Some(top);

        let dens = relative_density(bias);
        let eq = equilibrium_y(top, bed, bias);

        if *y > top {
            // Air above the free surface — gravity wins.
            state.vel_y -= GRAVITY;
            state.vel_y *= 1.0 - AIR_DRAG;
            *y += state.vel_y;
            if *y < top {
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
            if dens < 1.0 && *y > top {
                *y = top;
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
    g
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
        self.min_x < other.max_x - COLLISION_EPS
            && self.max_x > other.min_x + COLLISION_EPS
            && self.min_y < other.max_y - COLLISION_EPS
            && self.max_y > other.min_y + COLLISION_EPS
    }

    pub fn center_x(self) -> f32 {
        (self.min_x + self.max_x) * 0.5
    }

    pub fn center_y(self) -> f32 {
        (self.min_y + self.max_y) * 0.5
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
        };
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
}

fn collect_bodies(store: &AgentStore) -> Vec<(Entity, Aabb)> {
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
    bodies.iter().any(|&(e, other)| {
        if ignore == Some(e) {
            return false;
        }
        aabb.overlaps(other)
    })
}

/// Search near `(x0, y0)` for a pose whose footprint clears existing bodies.
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

    // Prefer the requested point, then a small spiral in x/y so multiple
    // small creatures can pack into one column at different depths.
    let mut rad = 0.0f32;
    while rad <= 6.0 {
        let steps = if rad < 1e-6 { 1 } else { 12 };
        for i in 0..steps {
            let ang = std::f32::consts::TAU * (i as f32) / (steps as f32);
            let x = (x0 + rad * ang.cos()).clamp(lo_f, hi_f);
            let mut y = y0 + rad * ang.sin() * 0.5;
            let wx = x.floor() as i32;
            if let Some(col) = world.column_at(wx) {
                if blueprint.is_rooted() && is_deep_water(col) {
                    continue;
                }
                if let Some((top, bed)) = water_band(col) {
                    y = y.clamp(bed, top);
                } else if blueprint.is_plankton() {
                    y = col.surface_y;
                } else {
                    y = col.surface_y;
                }
            } else {
                continue;
            }
            let pose = Pose { x, y };
            let aabb = organism_aabb(&pose, blueprint);
            if !aabb_hits_any(bodies, aabb, None) {
                return Some((x, y));
            }
        }
        rad += 0.25;
    }
    None
}

/// Push overlapping footprints apart (minimum-translation, deterministic).
fn resolve_collisions(store: &mut AgentStore, world: &World) {
    // A few iterations so chains of overlaps settle.
    for _ in 0..4 {
        let bodies = collect_bodies(store);
        if bodies.len() < 2 {
            return;
        }
        let mut pushes: Vec<(Entity, f32, f32)> = Vec::new();
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                let (ea, a) = bodies[i];
                let (eb, b) = bodies[j];
                if !a.overlaps(b) {
                    continue;
                }
                let overlap_x = (a.max_x.min(b.max_x) - a.min_x.max(b.min_x)).max(0.0);
                let overlap_y = (a.max_y.min(b.max_y) - a.min_y.max(b.min_y)).max(0.0);
                if overlap_x <= 0.0 || overlap_y <= 0.0 {
                    continue;
                }
                // Separate on the smaller axis (classic AABB MTV).
                if overlap_x <= overlap_y {
                    let push = overlap_x * 0.5 + COLLISION_EPS;
                    if a.center_x() <= b.center_x() {
                        pushes.push((ea, -push, 0.0));
                        pushes.push((eb, push, 0.0));
                    } else {
                        pushes.push((ea, push, 0.0));
                        pushes.push((eb, -push, 0.0));
                    }
                } else {
                    let push = overlap_y * 0.5 + COLLISION_EPS;
                    if a.center_y() <= b.center_y() {
                        pushes.push((ea, 0.0, -push));
                        pushes.push((eb, 0.0, push));
                    } else {
                        pushes.push((ea, 0.0, push));
                        pushes.push((eb, 0.0, -push));
                    }
                }
            }
        }
        if pushes.is_empty() {
            return;
        }
        let (lo, hi) = match world.world_x_bounds() {
            Some(b) => b,
            None => return,
        };
        let lo_f = lo as f32;
        let hi_f = hi as f32 + 0.99;
        for (e, dx, dy) in pushes {
            if let Ok(mut pose) = store.ecs.get::<&mut Pose>(e) {
                pose.x = (pose.x + dx).clamp(lo_f, hi_f);
                pose.y += dy;
            }
        }
    }
}

impl AgentStore {
    pub fn organism_count(&self) -> usize {
        self.ecs.query::<&Organism>().iter().count()
    }

    /// Elevation for a newly spawned / cloned organism.
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
            Organism,
        ));
        Some(e)
    }

    /// Set A behaviour pass: light, buoyancy, AABB collision, reproduce, die.
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

        for (e, (pose, buoy, energy, genome, body, _)) in self
            .ecs
            .query::<(
                &mut Pose,
                &mut BuoyancyState,
                &mut Energy,
                &Genome,
                &ModuleBody,
                &Organism,
            )>()
            .iter()
        {
            let wx = pose.world_x();
            let n_photo = body.photosystem_count().max(1) as f32;
            let active = circadian_active(genome, phase);

            if let Some(col) = world.column_at(wx) {
                if body.blueprint.is_plankton() {
                    step_buoyancy(&mut pose.y, buoy, col, genome.buoyancy_bias);
                } else {
                    pose.y = col.surface_y;
                    buoy.vel_y = 0.0;
                    buoy.last_water_top = None;
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
                    // Nudge birth sideways; AABB search / collision finishes packing.
                    let side = if (tick + e.id() as u64) % 2 == 0 {
                        MODULE_CELL_COLS
                    } else {
                        -MODULE_CELL_COLS
                    };
                    births.push((pose.x + side, pose.y, child_bp, child_e, energy.max));
                }
            }

            if let Some(col) = world.column_at_mut(wx) {
                col.activity = Activity::HydrologyActive;
            }
        }

        for (e, wx) in deaths {
            if let Some(col) = world.column_at_mut(wx) {
                col.ecology.dead_biomass =
                    col.ecology.dead_biomass.saturating_add(DEATH_LITTER_KG);
            }
            world.mass_audit.biomass_grow_total =
                world.mass_audit.biomass_grow_total.saturating_add(DEATH_LITTER_KG);
            let _ = self.ecs.despawn(e);
        }

        for (x0, y0, blueprint, energy, max_e) in births {
            if self.organism_count() >= MAX_ORGANISMS {
                break;
            }
            let bodies = collect_bodies(self);
            let Some((x, y)) = find_clear_pose(world, &bodies, &blueprint, x0, y0) else {
                continue;
            };
            let host_x = x.floor() as i32;
            let last_water_top = world
                .column_at(host_x)
                .and_then(water_band)
                .map(|(top, _)| top);
            self.ecs.spawn((
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
                Organism,
            ));
            self.births_total += 1;
        }

        // After buoyancy + births: separate any overlapping footprints.
        resolve_collisions(self, world);

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
            let modules = &body.blueprint.modules;
            if modules.is_empty() {
                continue;
            }
            let min_x = modules.iter().map(|m| m.x).min().unwrap_or(0);
            let max_x = modules.iter().map(|m| m.x).max().unwrap_or(0);
            let mid_x = (min_x as f32 + max_x as f32) * 0.5;
            for m in modules {
                let wx = pose.x + (m.x as f32 - mid_x) * MODULE_CELL_COLS;
                let wy = pose.y + m.y as f32 * MODULE_CELL_COLS;
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
        if let Some(col) = world.column_at_mut(8) {
            col.deposit_to_top(MaterialId::Water, 2_000, 0);
        }
        world
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
        step_buoyancy(&mut y, &mut state, col, 0.0);
        assert!(
            y > top0 - FLOAT_DEPTH_M + 0.1,
            "floater should rise with water: y={y} old_float={} new_top={top1}",
            top0 - FLOAT_DEPTH_M
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
}
