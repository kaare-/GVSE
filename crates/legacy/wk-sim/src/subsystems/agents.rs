//! ECS agent pass — wakes host columns and steps grazers + organisms.

use wk_agents::AgentStore;
use wk_world::world::World;

/// Post-barrier: keep agent columns awake; run grazer + Set A organism behaviour.
pub fn run_agents(world: &mut World, agents: &mut AgentStore, tick: u64) {
    if agents.is_empty() {
        world.agent_keep_awake.clear();
        return;
    }
    agents.step_grazers(world, tick);
    agents.step_organisms(world, tick);
}
