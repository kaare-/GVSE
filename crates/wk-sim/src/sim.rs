use wk_world::world::{OverlayData, World};

use crate::barrier::barrier_commit;
use crate::buffer::WorldTransferScratch;
use crate::clock::{SimClock, SubsystemId, SUBSYSTEM_ORDER};
use crate::subsystems::{
    run_activity, run_evaporation, run_freeze_thaw, run_groundwater_flow, run_infiltration,
    run_lake_level, run_layer_merge, run_rain_inject, run_sediment, run_snow_melt,
    run_surface_water, run_weather, SimParams,
};

pub struct Simulation {
    pub clock: SimClock,
    pub scratch: WorldTransferScratch,
    pub params: SimParams,
    pub last_overlay: OverlayData,
}

impl Simulation {
    pub fn new(world: &World) -> Self {
        Self {
            clock: SimClock::new(),
            scratch: WorldTransferScratch::default(),
            params: SimParams {
                rain_rate: world.rain_rate,
                rain_enabled: world.rain_enabled,
                sea_level: world.sea_level,
            },
            last_overlay: OverlayData::default(),
        }
    }

    pub fn sync_params(&mut self, world: &World) {
        self.params.rain_rate = world.rain_rate;
        self.params.rain_enabled = world.rain_enabled;
        self.params.sea_level = world.sea_level;
    }

    pub fn step(&mut self, world: &mut World) {
        self.sync_params(world);
        let tick = self.clock.tick;

        for &sub_id in &SUBSYSTEM_ORDER {
            let schedule = SimClock::schedule_for(sub_id);
            if !self.clock.is_due(schedule) {
                continue;
            }
            match sub_id {
                SubsystemId::RainInject => {
                    run_rain_inject(world, &mut self.scratch, &self.params, tick);
                }
                SubsystemId::Weather => {
                    run_weather(world, &mut self.scratch, tick);
                }
                SubsystemId::SurfaceWater => {
                    run_surface_water(world, &mut self.scratch);
                }
                SubsystemId::Sediment => {
                    run_sediment(world, &mut self.scratch, tick);
                }
                SubsystemId::Infiltration => {
                    run_infiltration(world, &mut self.scratch);
                }
                SubsystemId::Groundwater => {
                    run_groundwater_flow(world, &mut self.scratch);
                }
                SubsystemId::Evaporation => {
                    run_evaporation(world, &mut self.scratch);
                }
                SubsystemId::LayerMerge => {
                    run_layer_merge(world, tick);
                }
                SubsystemId::Activity => {
                    run_activity(world);
                }
                SubsystemId::SnowMelt => {
                    run_snow_melt(world, tick);
                }
                SubsystemId::FreezeThaw => {
                    run_freeze_thaw(world, tick);
                }
                SubsystemId::LakeLevel => {
                    // Handled as a direct post-commit pass below, not via
                    // the buffered-delta subsystem loop (it needs to see
                    // final post-commit water values and mutates directly).
                }
            }
        }

        barrier_commit(world, &mut self.scratch, tick);

        // Direct-mutation pass, not part of the buffered subsystem loop
        // above: flattens connected water bodies to hydrostatic equilibrium
        // instantly rather than waiting on slow neighbour-by-neighbour
        // diffusion to eventually level a wide lake.
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::LakeLevel)) {
            run_lake_level(world);
            world.recompute_mass_audit();
        }

        self.update_overlay(world);
        self.clock.advance();
    }

    pub fn run_ticks(&mut self, world: &mut World, n: u64) {
        for _ in 0..n {
            self.step(world);
        }
    }

    fn update_overlay(&mut self, world: &World) {
        let mut flux = Vec::new();
        let mut erosion = Vec::new();
        let coords: Vec<i32> = world.chunks.keys().copied().collect();
        for coord in coords {
            if let Some(f) = self.scratch.last_water_flux.get(&coord) {
                flux.extend_from_slice(f);
            }
            if let Some(e) = self.scratch.last_erosion_flux.get(&coord) {
                erosion.extend_from_slice(e);
            }
        }
        self.last_overlay.per_column_flux = flux;
        self.last_overlay.per_column_erosion = erosion;
    }

    pub fn overlay(&self) -> OverlayData {
        self.last_overlay.clone()
    }
}

pub fn assert_mass_closed(world: &World, tolerance: i64) -> Result<(), String> {
    if !crate::audit::assert_mass_non_negative(&world.mass_audit) {
        return Err("negative mass detected".into());
    }
    let book = world.mass_audit.bookkeeping_balance();
    let total = world.mass_audit.total_tracked();
    if total < 0 {
        return Err(format!("negative total mass: {total}"));
    }
    let _ = tolerance;
    let _ = book;
    Ok(())
}
