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
    /// Melts a column's top Snow layer back into surface water once local
    /// temperature rises above the freeze point.
    SnowMelt = 9,
    /// Freezes standing liquid water into inert ice when cold, thaws it
    /// back when warm.
    FreezeThaw = 10,
    /// Spawns/advances/rains from drifting clouds (the automatic weather
    /// layer, separate from the manual rain override).
    Weather = 11,
}

#[derive(Debug, Clone, Copy)]
pub struct SubsystemSchedule {
    pub id: SubsystemId,
    pub period: u32,
    pub phase: u32,
}

pub const SUBSYSTEM_SCHEDULES: [SubsystemSchedule; 12] = [
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
        // Fires every tick (not every 6th) so rain is delivered smoothly
        // instead of as a periodic lump — a discrete "dump" of mass every
        // few ticks, combined with the also-periodic LakeLevel pass at a
        // *different* period, was creating a beat-frequency interference
        // pattern (their periods' LCM) that looked like waves periodically
        // appearing and disappearing.
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
        id: SubsystemId::SnowMelt,
        period: 1,
        phase: 0,
    },
    SubsystemSchedule {
        id: SubsystemId::FreezeThaw,
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
];

/// Fixed execution order for subsystems within one tick.
pub const SUBSYSTEM_ORDER: [SubsystemId; 11] = [
    SubsystemId::RainInject,
    SubsystemId::Weather,
    SubsystemId::SurfaceWater,
    SubsystemId::Sediment,
    SubsystemId::Infiltration,
    SubsystemId::Groundwater,
    SubsystemId::Evaporation,
    SubsystemId::LayerMerge,
    SubsystemId::Activity,
    SubsystemId::SnowMelt,
    SubsystemId::FreezeThaw,
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
