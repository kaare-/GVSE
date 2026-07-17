#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubsystemId {
    SurfaceWater = 0,
    Sediment = 1,
    RainInject = 2,
    Infiltration = 3,
    Evaporation = 4,
    LayerMerge = 5,
    Activity = 6,
    /// Gradually flattens each connected body of standing water toward its
    /// hydrostatic equilibrium level (a damped blend applied frequently,
    /// not an instant snap — see LAKE_LEVEL_BLEND). Real water pressure
    /// equalizes across a lake far faster than our per-tick neighbour
    /// diffusion can simulate one hop at a time — without this, a wide lake
    /// looks lumpy because infiltration (a fast, local, per-column process)
    /// wins the race against lateral leveling (a slow, column-by-column
    /// spreading process) long before the lake can go flat on its own.
    LakeLevel = 7,
    /// Slow lateral flow of underground moisture between neighbouring
    /// columns' water tables — a real (if simplified) groundwater layer.
    Groundwater = 8,
    /// Temperature-driven material transitions: snow melts to water,
    /// water freezes to ice, ice thaws back to water. Driven by each
    /// material's own `phase_change` property row rather than three
    /// separate hard-coded subsystems.
    PhaseChange = 9,
    /// Spawns/advances/rains from drifting clouds (the automatic weather
    /// layer, separate from the manual rain override).
    Weather = 10,
    /// Angle-of-repose slumping: any granular top layer sitting on a
    /// slope steeper than its material allows slides toward its lower
    /// neighbour. Post-barrier direct-mutation pass, like PhaseChange
    /// and LakeLevel.
    Slumping = 11,
    /// Thermal field diffusion (geothermal bottom + sky top). Writes
    /// only field state; runs after the material barrier so
    /// phase_change can sample the committed field.
    ThermalField = 12,
    /// Atmospheric relative-humidity diffusion. Reads open-water /
    /// evaporation source buffers; evaporation samples the field for
    /// its rate. Post-barrier like ThermalField.
    HumidityField = 13,
    /// Hydrostatic + buoyancy pressure field. Feeds WindField.
    PressureField = 14,
    /// Wind from −∇pressure (+ climate bias). Weather samples this.
    WindField = 15,
    /// Groundwater hydraulic-head Darcy diffusion. Moisture transfers
    /// still happen in `Groundwater`; this pass updates the head field.
    GroundwaterHeadField = 16,
    /// Dissolved-mineral concentration (kg/m³). Diffuses in wet cells;
    /// karst injects dissolved mass into this field.
    DissolvedField = 17,
    /// Flux-driven limestone dissolution + void growth.
    Karst = 18,
    /// Surface water capture into open voids + cave-river flow.
    VoidWater = 19,
    /// Roof collapse over voids wider than `roof_span_max_m`.
    RoofCollapse = 20,
    /// Speleothem reprecipitation (dissolved → Limestone inside voids).
    Speleogenesis = 21,
    /// Per-column plant growth / death / nutrient recycle.
    Ecology = 22,
    /// ECS creature behaviour (grazers).
    Agents = 23,
    /// Per-column air / dissolved CO₂ + O₂ mixing and exchange.
    Gas = 24,
    /// Wind stress + gravity-wave momentum on the free surface, plus tide.
    /// Post-barrier; runs before LakeLevel so deep water isn't flattened
    /// back to a sheet every tick.
    SurfaceWaves = 25,
}

#[derive(Debug, Clone, Copy)]
pub struct SubsystemSchedule {
    pub id: SubsystemId,
    pub period: u32,
    pub phase: u32,
}

pub const SUBSYSTEM_SCHEDULES: [SubsystemSchedule; 26] = [
    SubsystemSchedule {
        id: SubsystemId::SurfaceWater,
        period: 1,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::Sediment,
        period: 2,
        phase: 1,
    },
    SubsystemSchedule {
        id: SubsystemId::RainInject,
        // Every tick so rain is smooth. (A past RainInject×LakeLevel period
        // mismatch made beat-frequency "fake waves"; real free-surface
        // motion now lives in SurfaceWaves.)
        period: 1,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::Infiltration,
        period: 60,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::Evaporation,
        period: 60,
        phase: 30,
    },
    SubsystemSchedule {
        id: SubsystemId::LayerMerge,
        period: 3600,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::Activity,
        period: 60,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::LakeLevel,
        // Every tick, smoothly (see LAKE_LEVEL_BLEND) — matches RainInject's
        // period so there's no beat-frequency interference between the two.
        period: 1,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::Groundwater,
        // Every tick too — same reasoning as RainInject/LakeLevel, avoids
        // introducing a new periodic driver that could beat against them.
        period: 1,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::PhaseChange,
        period: 1,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::Weather,
        // Every tick too, same reasoning as the other precipitation/flow
        // subsystems — a periodic driver here would beat against RainInject.
        period: 1,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::Slumping,
        // Every tick, but the transfer per tick is small (SLUMP_RELAXATION
        // = 0.35 of the excess). A big cliff collapses over a few frames
        // rather than instantly, which reads as a natural slide.
        period: 1,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::ThermalField,
        // Every 10 ticks — heat diffusion is slow vs hydrology; period
        // chosen against α·Δt/Δx² stability at 0.5 m cells.
        period: 10,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::HumidityField,
        // Same cadence as thermal, phase-staggered so the two field
        // passes don't always fire on the same tick.
        period: 10,
        phase: 3,
    },
    SubsystemSchedule {
        id: SubsystemId::PressureField,
        period: 30,
        phase: 5,
    },
    SubsystemSchedule {
        id: SubsystemId::WindField,
        // One tick after pressure so wind samples the committed field.
        period: 30,
        phase: 6,
    },
    SubsystemSchedule {
        id: SubsystemId::GroundwaterHeadField,
        period: 30,
        phase: 10,
    },
    SubsystemSchedule {
        id: SubsystemId::DissolvedField,
        period: 6,
        phase: 2,
    },
    SubsystemSchedule {
        id: SubsystemId::Karst,
        // Slow vs hydrology; caves develop over many ticks.
        period: 6,
        phase: 4,
    },
    SubsystemSchedule {
        id: SubsystemId::VoidWater,
        period: 1,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::RoofCollapse,
        period: 10,
        phase: 7,
    },
    SubsystemSchedule {
        id: SubsystemId::Speleogenesis,
        period: 30,
        phase: 15,
    },
    SubsystemSchedule {
        id: SubsystemId::Ecology,
        // Plants are slow vs hydrology; keep the tick budget light.
        period: 10,
        phase: 8,
    },
    SubsystemSchedule {
        id: SubsystemId::Agents,
        period: 1,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::Gas,
        period: 4,
        phase: 1,
    },
    SubsystemSchedule {
        id: SubsystemId::SurfaceWaves,
        // Every tick — free-surface momentum needs a steady integrate step.
        period: 1,
        phase: 0,
    },
];

/// Fixed execution order for subsystems within one tick. PhaseChange,
/// LakeLevel, Slumping, and the field passes are deliberately NOT here
/// — they're direct-mutation passes that run *after* barrier_commit so
/// they operate on already-committed column (and field) state.
pub const SUBSYSTEM_ORDER: [SubsystemId; 9] = [
    SubsystemId::RainInject,
    SubsystemId::Weather,
    SubsystemId::SurfaceWater,
    SubsystemId::Sediment,
    SubsystemId::Infiltration,
    SubsystemId::Groundwater,
    SubsystemId::Evaporation,
    SubsystemId::LayerMerge,
    SubsystemId::Activity,
];

#[derive(Debug, Clone, Default)]
pub struct SimClock {
    pub tick: u64,
}

impl SimClock {
    pub fn new() -> Self {
        Self { tick: 0 }
    }

    pub fn advance(&mut self) {
        self.tick += 1;
    }

    pub fn is_due(&self, schedule: &SubsystemSchedule) -> bool {
        let p = schedule.period.max(1);
        let t = self.tick % p as u64;
        t == schedule.phase as u64
    }

    pub fn schedule_for(id: SubsystemId) -> &'static SubsystemSchedule {
        SUBSYSTEM_SCHEDULES
            .iter()
            .find(|s| s.id == id)
            .expect("schedule exists")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_6_fires_at_0_6_12() {
        let sched = SubsystemSchedule {
            id: SubsystemId::RainInject,
            period: 6,
            phase: 0,
        };
        let mut clock = SimClock::new();
        let mut fired = Vec::new();
        for _ in 0..13 {
            if clock.is_due(&sched) {
                fired.push(clock.tick);
            }
            clock.advance();
        }
        assert_eq!(fired, vec![0, 6, 12]);
    }
}
