//! Smoke test for the wk-voxel scaffold.
//!
//! No physics rules exist yet — this test just verifies:
//!   - the data structures wire up
//!   - dirty-rectangle bookkeeping is coherent under writes
//!   - `tick()` advances the counters and does not corrupt state
//!
//! When the real cellular rules land, we'll add scenarios that drop
//! rain and expect it to accumulate on the sand and infiltrate at the
//! porosity rate. Right now: no rules, no expected water motion.

use wk_material::MaterialId;
use wk_voxel::{tick, Cell, Sat, World};

#[test]
fn scaffold_wires_up() {
    let mut w = World::new(1234);
    // Build a single chunk of flat sand at y = 0, air above.
    for x in 0..64 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Sand));
    }
    // Drop a rain "film" into the row of air just above the sand.
    for x in 0..64 {
        let mut air = Cell::solid(MaterialId::Water);
        air.sat = Sat::FULL;
        w.set_cell(x, 1, air);
    }

    let cell = w.get_cell(4, 0).unwrap();
    assert_eq!(cell.material, MaterialId::Sand);
    let wet = w.get_cell(4, 1).unwrap();
    assert_eq!(wet.material, MaterialId::Water);
    assert_eq!(wet.sat, Sat::FULL);

    // Dirty rectangle covers the touched cells.
    let (coord, _, _) = World::split(4, 0);
    let chunk = w.chunks.get(&coord).unwrap();
    let dirty = chunk.dirty.expect("dirty after writes");
    assert!(dirty.contains(0, 0));
    assert!(dirty.contains(63, 1));

    // Ticking is a no-op today but must advance counters and clear
    // dirty rectangles.
    let tick0 = w.tick;
    tick(&mut w);
    assert_eq!(w.tick, tick0 + 1);
    let chunk = w.chunks.get(&coord).unwrap();
    assert!(chunk.dirty.is_none());
    // Cell state must survive ticks unchanged (no rules yet).
    assert_eq!(w.get_cell(4, 1).unwrap().sat, Sat::FULL);

    // Second tick: no writes since previous tick → still no dirty rect.
    tick(&mut w);
    let chunk = w.chunks.get(&coord).unwrap();
    assert!(chunk.dirty.is_none());
    assert_eq!(w.tick, tick0 + 2);
}
