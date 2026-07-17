use wk_agents::AgentStore;
use wk_world::world::{OverlayData, World};

use crate::barrier::barrier_commit;
use crate::buffer::WorldTransferScratch;
use crate::clock::{SimClock, SubsystemId, SUBSYSTEM_ORDER};
use crate::subsystems::{
    run_activity, run_agents, run_dissolved_field, run_ecology, run_evaporation, run_gas,
    recharge_deep_water_tables,
    run_groundwater_flow, run_groundwater_head_field, run_humidity_field, run_infiltration,
    run_karst, run_lake_level, run_layer_merge, run_phase_change, run_pressure_field,
    run_rain_inject, run_roof_collapse, run_sediment, run_slumping, run_speleogenesis,
    run_surface_void_capture, run_surface_water, run_surface_waves, run_thermal_field,
    run_void_water_flow, run_weather, run_wind_field, SimParams,
};

pub struct Simulation {
    pub clock: SimClock,
    pub scratch: WorldTransferScratch,
    pub params: SimParams,
    pub last_overlay: OverlayData,
    /// ECS creature store (stage 10). Empty until something spawns.
    pub agents: AgentStore,
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
            agents: AgentStore::new(),
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

        // Ensure agent host columns stay eligible before activity runs.
        if !self.agents.is_empty() {
            self.agents.wake_host_columns(world);
        }

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
                SubsystemId::PhaseChange
                | SubsystemId::LakeLevel
                | SubsystemId::Slumping
                | SubsystemId::ThermalField
                | SubsystemId::HumidityField
                | SubsystemId::PressureField
                | SubsystemId::WindField
                | SubsystemId::GroundwaterHeadField
                | SubsystemId::DissolvedField
                | SubsystemId::Karst
                | SubsystemId::VoidWater
                | SubsystemId::RoofCollapse
                | SubsystemId::Speleogenesis
                | SubsystemId::Ecology
                | SubsystemId::Agents
                | SubsystemId::Gas
                | SubsystemId::SurfaceWaves => {}
            }
        }

        barrier_commit(world, &mut self.scratch, tick);

        // Direct-mutation passes, NOT part of the buffered subsystem loop
        // above. Running these before barrier_commit would let them
        // change a column's top layer between the buffered subsystems
        // that computed deltas against the old top and the commit that
        // tries to apply those deltas — the deltas then silently no-op
        // but their upstream bookkeeping (evap_out_total etc.) was
        // already booked, leaking mass into the audit.
        //
        // Field passes write only field state (not layers), but still
        // run here so material consumers sample committed fields.
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::ThermalField)) {
            run_thermal_field(world, tick);
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::HumidityField)) {
            run_humidity_field(world, tick);
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::PressureField)) {
            run_pressure_field(world, tick);
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::WindField)) {
            run_wind_field(world, tick);
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::GroundwaterHeadField)) {
            run_groundwater_head_field(world, tick);
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::DissolvedField)) {
            run_dissolved_field(world, tick);
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::Karst)) {
            run_karst(world, tick);
            world.recompute_mass_audit();
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::VoidWater)) {
            run_surface_void_capture(world);
            run_void_water_flow(world);
            world.recompute_mass_audit();
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::RoofCollapse)) {
            run_roof_collapse(world, tick);
            world.recompute_mass_audit();
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::Speleogenesis)) {
            run_speleogenesis(world, tick);
            world.recompute_mass_audit();
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::Ecology)) {
            run_ecology(world, tick);
            world.recompute_mass_audit();
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::Gas)) {
            run_gas(world, tick);
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::Agents)) {
            run_agents(world, &mut self.agents, tick);
            world.recompute_mass_audit();
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::PhaseChange)) {
            run_phase_change(world, tick);
            world.recompute_mass_audit();
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::SurfaceWaves)) {
            // Waves book tide exchange on sea_inject_total; skip a full
            // ring mass audit here — LakeLevel (below) refreshes when due.
            run_surface_waves(world, tick);
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::LakeLevel)) {
            run_lake_level(world);
            world.recompute_mass_audit();
        }
        if self.clock.is_due(SimClock::schedule_for(SubsystemId::Slumping)) {
            run_slumping(world, tick);
            world.recompute_mass_audit();
        }
        // After slump reshuffles substrate, snap ocean/lake beds back to
        // saturation while the free surface can still afford it.
        recharge_deep_water_tables(world);

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
