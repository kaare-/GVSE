//! Sealed-cave ambient humidity — sparse mini field, not sky weather.
//!
//! **Store:** [`World::cave_humidity`] is a capped sparse map of moist Air
//! mass inside **roofed / side-closed** voids. Same sat mass units as free
//! water. It is **not** sky [`Humidity`] and **not** pressurized
//! [`World::steam`].
//!
//! **Why:** Open caves / overhangs share sky Humidity (T5). Sealed voids under
//! rock need a second mini field for ordinary closed-cave air — any ambient
//! moisture, not a pressure or high-RH prerequisite — without minting geyser
//! steam or expanding the weather walk underground.
//!
//! **Motor (v1):** sealed-film evaporation deposits here; cool recondense and
//! diffusion come later. Never writes the rain lottery.

use crate::fasthash::FxHashMap;
use crate::grid::World;

/// Hard cap — sealed moist air is local, not world-filling.
pub const MAX_CAVE_HUMIDITY_CELLS: usize = 4096;

/// Units at `(gx, gy)`, or 0.
#[inline]
pub fn cave_humidity_at(world: &World, gx: i32, gy: i32) -> u8 {
    let gx = world.wrap_x(gx);
    world.cave_humidity.get(&(gx, gy)).copied().unwrap_or(0)
}

/// Total sealed-cave humidity mass (sat units).
pub fn cave_humidity_total(world: &World) -> u64 {
    world.cave_humidity.values().map(|&v| v as u64).sum()
}

/// Add up to `want` units into a sealed-cave seat. Returns accepted mass.
///
/// Refuses new seats once [`MAX_CAVE_HUMIDITY_CELLS`] is reached (existing
/// seats may still fill toward 255).
pub fn try_add_cave_humidity(world: &mut World, gx: i32, gy: i32, want: u8) -> u8 {
    if want == 0 {
        return 0;
    }
    let gx = world.wrap_x(gx);
    let key = (gx, gy);
    let cur = world.cave_humidity.get(&key).copied().unwrap_or(0);
    let room = 255u8.saturating_sub(cur);
    if room == 0 {
        return 0;
    }
    if cur == 0 && world.cave_humidity.len() >= MAX_CAVE_HUMIDITY_CELLS {
        return 0;
    }
    let take = want.min(room);
    world.cave_humidity.insert(key, cur + take);
    take
}

/// Debug / tests: replace the sparse map.
pub fn replace_cave_humidity(world: &mut World, map: FxHashMap<(i32, i32), u8>) {
    world.cave_humidity = map;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;
    use crate::grid::World;

    #[test]
    fn try_add_fills_and_caps_per_cell() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        assert_eq!(try_add_cave_humidity(&mut w, 2, 3, 200), 200);
        assert_eq!(cave_humidity_at(&w, 2, 3), 200);
        assert_eq!(try_add_cave_humidity(&mut w, 2, 3, 100), 55);
        assert_eq!(cave_humidity_at(&w, 2, 3), 255);
        assert_eq!(cave_humidity_total(&w), 255);
    }
}
