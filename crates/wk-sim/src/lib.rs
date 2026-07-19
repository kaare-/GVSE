//! Simulation kernel: clock, transfer buffers, subsystems, barrier commit.

pub mod audit;
pub mod barrier;
pub mod buffer;
pub mod clock;
pub mod ports;
pub mod residual;
pub mod sim;
pub mod subsystems;

pub use audit::*;
pub use barrier::*;
pub use buffer::*;
pub use clock::*;
pub use residual::*;
pub use sim::*;
pub use hecs::Entity;
pub use wk_agents::{
    circadian_buoyancy_bias, temp_comfort_factor, Aabb, AgentStore, Blueprint, Energy, Genome,
    Grazer, LaneId, Lineage, ModuleBody, ModuleId, Organism, OrganismHabit, OrganismInspect,
    PlacedModule, PopCaps, Pose, Wire, WireKind, DROUGHT_HIBERNATE_MAX_TICKS, MAX_AGENTS,
};
