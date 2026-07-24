//! Periodic stratigraphic layer merging.

use wk_world::world::World;

pub fn run_layer_merge(world: &mut World, tick: u64) {
    for chunk in world.chunks.values_mut() {
        for col in &mut chunk.columns {
            col.merge_layers(true, tick);
        }
    }
}
