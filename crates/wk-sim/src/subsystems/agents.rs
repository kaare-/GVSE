//! ECS agent pass — wakes host columns and steps scripted grazers.

use wk_agents::AgentStore;
use wk_world::world::World;

/// Post-barrier: keep agent columns awake and run grazer behaviour.
pub fn run_agents(world: &mut World, agents: &mut AgentStore, tick: u64) {
    if agents.is_empty() {
        world.agent_keep_awake.clear();
        return;
    }
    agents.step_grazers(world, tick);
}
