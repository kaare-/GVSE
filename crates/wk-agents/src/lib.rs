//! ECS creature layer (stage 10).
//!
//! Agents live in a [`hecs::World`] beside the column stack. They read
//! world state and call [`wk_world::World`] APIs (`dig`, `eat_biomass`,
//! `drink_water`). See `docs/AGENTS.md`.

use hecs::{Entity, World as EcsWorld};
use serde::{Deserialize, Serialize};
use wk_material::CHUNK_W;
use wk_world::column::Activity;
use wk_world::world::World;

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

/// Genome trait vector — stage 10 stores it; stage 11 mutates it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
    /// Basal energy drain per agent tick.
    pub metabolism: f32,
}

impl Default for Genome {
    fn default() -> Self {
        Self {
            move_speed: 0.35,
            graze_rate: 40.0,
            drink_rate: 30.0,
            dig_drive: 0.15,
            graze_efficiency: 0.08,
            metabolism: 0.35,
        }
    }
}

/// Marker: scripted surface grazer.
#[derive(Debug, Clone, Copy, Default)]
pub struct Grazer;

/// All agent entities for one simulation.
#[derive(Default)]
pub struct AgentStore {
    pub ecs: EcsWorld,
}

impl AgentStore {
    pub fn new() -> Self {
        Self {
            ecs: EcsWorld::new(),
        }
    }

    pub fn len(&self) -> u32 {
        self.ecs.iter().count() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Spawn a scripted grazer at column `world_x`, surface elevation.
    pub fn spawn_grazer(
        &mut self,
        world: &World,
        world_x: i32,
        genome: Genome,
        energy: f32,
    ) -> Option<Entity> {
        let col = world.column_at(world_x)?;
        let y = col.surface_y;
        let max_e = energy.max(1.0);
        let e = self
            .ecs
            .spawn((
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

    /// One scripted-grazer behaviour pass.
    pub fn step_grazers(&mut self, world: &mut World, tick: u64) {
        self.wake_host_columns(world);

        let mut moves: Vec<(Entity, f32, f32)> = Vec::new();
        let mut digs: Vec<(Entity, i32, f32, i64)> = Vec::new();
        let mut deaths: Vec<Entity> = Vec::new();

        for (e, (pose, energy, genome, _)) in self
            .ecs
            .query::<(&Pose, &mut Energy, &Genome, &Grazer)>()
            .iter()
        {
            let wx = pose.world_x();
            let hungry = energy.current < energy.max * 0.85;
            let thirsty = energy.current < energy.max * 0.95;

            if thirsty {
                let drank = world.drink_water(wx, genome.drink_rate as i64);
                if drank > 0 {
                    energy.current = (energy.current + drank as f32 * 0.01).min(energy.max);
                }
            }
            if hungry {
                let eaten = world.eat_biomass(wx, genome.graze_rate as i64);
                if eaten > 0 {
                    energy.current = (energy.current
                        + eaten as f32 * genome.graze_efficiency)
                        .min(energy.max);
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

            energy.current -= genome.metabolism;
            if energy.current <= 0.0 {
                deaths.push(e);
                continue;
            }

            let new_x = pose.x + dx;
            let new_wx = new_x.floor() as i32;
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

        self.wake_host_columns(world);
        let _ = CHUNK_W;
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
}
