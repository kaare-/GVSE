//! Dig F1 — Lab inter-process seam P0 mass (water + sediment + dissolved).
//!
//! See [`docs/HALO_SHARD.md`]. One host owns each column; mass that would
//! leave an open west/east face is booked here (absorbing edge). A live
//! neighbor drains outbound via [`take_outbound`] and paints inbound with
//! [`apply_inbound`]. Missing neighbor → backlog stays (same spirit as
//! [`crate::frozen_backlog`]).

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{Cell, Sat};
use crate::chunk::CHUNK_CELLS_H;
use crate::grid::World;
use crate::mineral::{add_dissolved, dissolved_at, take_dissolved};
use crate::sediment::{add_suspended, suspended_at, take_suspended};

/// Which vertical face of the owned strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SeamSide {
    West,
    East,
}

/// Owned column range + which faces are open to a neighbor process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeamConfig {
    /// Inclusive min world-x owned by this region.
    pub x_min: i32,
    /// Exclusive max world-x owned by this region.
    pub x_max: i32,
    pub open_west: bool,
    pub open_east: bool,
}

impl SeamConfig {
    /// Single open face on a `[0, width)` strip.
    pub fn open_side(width: i32, side: SeamSide) -> Self {
        let width = width.max(1);
        match side {
            SeamSide::West => Self {
                x_min: 0,
                x_max: width,
                open_west: true,
                open_east: false,
            },
            SeamSide::East => Self {
                x_min: 0,
                x_max: width,
                open_west: false,
                open_east: true,
            },
        }
    }

    pub fn contains_x(self, gx: i32) -> bool {
        gx >= self.x_min && gx < self.x_max
    }

    pub fn side_for_outside(self, gx: i32) -> Option<SeamSide> {
        if gx < self.x_min && self.open_west {
            Some(SeamSide::West)
        } else if gx >= self.x_max && self.open_east {
            Some(SeamSide::East)
        } else {
            None
        }
    }
}

/// Wire / exchange payload (P0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeamTransfer {
    pub water: i64,
    pub sediment: i64,
    pub dissolved: i64,
}

impl SeamTransfer {
    pub fn is_empty(self) -> bool {
        self.water == 0 && self.sediment == 0 && self.dissolved == 0
    }

    pub fn total_water(self) -> i64 {
        self.water
    }
}

/// Per-world seam bookkeeping (not saved).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeamState {
    pub config: Option<SeamConfig>,
    pub out_west: SeamTransfer,
    pub out_east: SeamTransfer,
    pub in_west: SeamTransfer,
    pub in_east: SeamTransfer,
    /// Neighbor silent ticks (Lab default absorb after 60 — host may bump).
    pub silent_ticks: u32,
}

impl SeamState {
    pub fn water_parked(&self) -> i64 {
        self.out_west.water
            + self.out_east.water
            + self.in_west.water
            + self.in_east.water
    }

    pub fn sediment_parked(&self) -> i64 {
        self.out_west.sediment
            + self.out_east.sediment
            + self.in_west.sediment
            + self.in_east.sediment
    }

    pub fn dissolved_parked(&self) -> i64 {
        self.out_west.dissolved
            + self.out_east.dissolved
            + self.in_west.dissolved
            + self.in_east.dissolved
    }
}

/// Water parked in seam buckets (for [`crate::audit::sat_totals`]).
pub fn seam_backlog_water_total(world: &World) -> i64 {
    world.seam.water_parked()
}

pub fn seam_backlog_sediment_total(world: &World) -> i64 {
    world.seam.sediment_parked()
}

pub fn seam_backlog_dissolved_total(world: &World) -> i64 {
    world.seam.dissolved_parked()
}

/// True when `to_gx` lies past an open seam face (destination not owned).
pub fn should_absorb_seam(world: &World, to_gx: i32) -> Option<SeamSide> {
    let cfg = world.seam.config?;
    // Toroidal rings own the wrap; seams are for open Lab strips.
    if world.wrap_width.is_some() {
        return None;
    }
    let gx = world.wrap_x(to_gx);
    cfg.side_for_outside(gx)
}

/// Book `amt` sat (+ pro-rata loads) into outbound seam for `side`.
pub fn absorb_seam_water(
    world: &mut World,
    from: (i32, i32),
    side: SeamSide,
    amt: i32,
    donor_sat_before: u8,
) {
    if amt <= 0 {
        return;
    }
    let mut sed_moved = 0i64;
    let mut diss_moved = 0i64;
    if donor_sat_before > 0 {
        let sed = suspended_at(world, from.0, from.1);
        if sed > 0 {
            let share = (sed as u32 * amt as u32 / donor_sat_before as u32) as u16;
            sed_moved = take_suspended(world, from.0, from.1, share) as i64;
        }
        let diss = dissolved_at(world, from.0, from.1);
        if diss > 0 {
            let share = ((diss as u32 * amt as u32) / donor_sat_before as u32) as u16;
            let share = share.max(1);
            diss_moved = take_dissolved(world, from.0, from.1, share) as i64;
        }
    }
    let slot = match side {
        SeamSide::West => &mut world.seam.out_west,
        SeamSide::East => &mut world.seam.out_east,
    };
    slot.water += amt as i64;
    slot.sediment += sed_moved;
    slot.dissolved += diss_moved;
}

/// Drain outbound for `side` into a transfer (zeros the out bucket).
pub fn take_outbound(world: &mut World, side: SeamSide) -> SeamTransfer {
    let slot = match side {
        SeamSide::West => &mut world.seam.out_west,
        SeamSide::East => &mut world.seam.out_east,
    };
    let t = *slot;
    *slot = SeamTransfer::default();
    world.seam.silent_ticks = 0;
    t
}

/// Queue inbound transfer; paint onto the owned edge column.
pub fn apply_inbound(world: &mut World, side: SeamSide, transfer: SeamTransfer) {
    if transfer.is_empty() {
        return;
    }
    let slot = match side {
        SeamSide::West => &mut world.seam.in_west,
        SeamSide::East => &mut world.seam.in_east,
    };
    slot.water += transfer.water;
    slot.sediment += transfer.sediment;
    slot.dissolved += transfer.dissolved;
    paint_inbound(world, side);
}

fn paint_inbound(world: &mut World, side: SeamSide) {
    let Some(cfg) = world.seam.config else {
        return;
    };
    let edge_x = match side {
        SeamSide::West => cfg.x_min,
        SeamSide::East => cfg.x_max - 1,
    };
    if edge_x < cfg.x_min || edge_x >= cfg.x_max {
        return;
    }
    let slot = match side {
        SeamSide::West => &mut world.seam.in_west,
        SeamSide::East => &mut world.seam.in_east,
    };
    // Paint water down the edge column (any loaded cy).
    let mut water = slot.water;
    let mut sediment = slot.sediment;
    let mut dissolved = slot.dissolved;
    // Scan a tall band of sky/ground Air on the edge.
    let y0 = -4 * CHUNK_CELLS_H as i32;
    let y1 = 8 * CHUNK_CELLS_H as i32;
    for gy in y0..y1 {
        if water <= 0 && sediment <= 0 && dissolved <= 0 {
            break;
        }
        let Some(cell) = world.get_cell(edge_x, gy) else {
            continue;
        };
        if cell.material != MaterialId::Air {
            continue;
        }
        if water > 0 {
            let free = (u8::MAX - cell.sat.0) as i64;
            if free > 0 {
                let give = water.min(free) as u8;
                world.set_cell(
                    edge_x,
                    gy,
                    Cell {
                        sat: Sat(cell.sat.0 + give),
                        ..cell
                    },
                );
                water -= give as i64;
            }
        }
        let Some(cell) = world.get_cell(edge_x, gy) else {
            continue;
        };
        if cell.material == MaterialId::Air && !cell.sat.is_empty() {
            if sediment > 0 {
                let give = sediment.min(u16::MAX as i64) as u16;
                add_suspended(world, edge_x, gy, give);
                sediment -= give as i64;
            }
            if dissolved > 0 {
                let give = dissolved.min(u16::MAX as i64) as u16;
                add_dissolved(world, edge_x, gy, give);
                dissolved -= give as i64;
            }
        }
    }
    let slot = match side {
        SeamSide::West => &mut world.seam.in_west,
        SeamSide::East => &mut world.seam.in_east,
    };
    slot.water = water;
    slot.sediment = sediment;
    slot.dissolved = dissolved;
}

/// Drain standing Air sat from an open seam face into outbound.
///
/// CA will not invent xfers into missing neighbour cells; Lab F1 uses this
/// face export (plus any commit-path absorbs) so mass can leave the strip.
pub fn export_seam_face(world: &mut World, side: SeamSide, max_water: i32) -> i64 {
    if max_water <= 0 {
        return 0;
    }
    let Some(cfg) = world.seam.config else {
        return 0;
    };
    let open = match side {
        SeamSide::West => cfg.open_west,
        SeamSide::East => cfg.open_east,
    };
    if !open || world.wrap_width.is_some() {
        return 0;
    }
    let edge_x = match side {
        SeamSide::West => cfg.x_min,
        SeamSide::East => cfg.x_max - 1,
    };
    let mut left = max_water;
    let mut moved = 0i64;
    let y0 = -4 * CHUNK_CELLS_H as i32;
    let y1 = 8 * CHUNK_CELLS_H as i32;
    for gy in y0..y1 {
        if left <= 0 {
            break;
        }
        let Some(cell) = world.get_cell(edge_x, gy) else {
            continue;
        };
        if cell.material != MaterialId::Air || cell.sat.0 == 0 {
            continue;
        }
        let give = (cell.sat.0 as i32).min(left);
        if give <= 0 {
            continue;
        }
        let before = cell.sat.0;
        world.set_cell(
            edge_x,
            gy,
            Cell {
                sat: Sat(cell.sat.0 - give as u8),
                ..cell
            },
        );
        absorb_seam_water(world, (edge_x, gy), side, give, before);
        left -= give;
        moved += give as i64;
    }
    moved
}

/// Default face export budget per exchange (one full Air cell).
pub const SEAM_FACE_EXPORT_MAX: i32 = 255;

/// In-process Lab smoke: `west`'s east face ↔ `east`'s west face.
pub fn exchange_seam_p0(west: &mut World, east: &mut World) {
    export_seam_face(west, SeamSide::East, SEAM_FACE_EXPORT_MAX);
    export_seam_face(east, SeamSide::West, SEAM_FACE_EXPORT_MAX);
    let to_east = take_outbound(west, SeamSide::East);
    let to_west = take_outbound(east, SeamSide::West);
    apply_inbound(east, SeamSide::West, to_east);
    apply_inbound(west, SeamSide::East, to_west);
}

/// Attach an open Lab seam on a non-wrapping strip `[0, width)`.
pub fn attach_open_seam(world: &mut World, width: i32, side: SeamSide) {
    world.wrap_width = None;
    world.seam.config = Some(SeamConfig::open_side(width, side));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{sat_totals, sediment_total};
    use crate::chunk::CHUNK_CELLS_W;

    fn slab_pair() -> (World, World) {
        let w = CHUNK_CELLS_W as i32;
        let mut west = World::new(1);
        let mut east = World::new(2);
        for x in 0..w {
            west.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            east.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            west.set_cell(x, 1, Cell::air());
            east.set_cell(x, 1, Cell::air());
        }
        attach_open_seam(&mut west, w, SeamSide::East);
        attach_open_seam(&mut east, w, SeamSide::West);
        (west, east)
    }

    #[test]
    fn absorb_then_exchange_preserves_pair_sat() {
        let (mut west, mut east) = slab_pair();
        let edge = CHUNK_CELLS_W as i32 - 1;
        west.set_cell(edge, 1, Cell::water());
        let pair0 = sat_totals(&west).cell_total + sat_totals(&east).cell_total;
        let amt = west.get_cell(edge, 1).unwrap().sat.0 as i64;

        // Neighbor down: face export books outbound (absorbing).
        export_seam_face(&mut west, SeamSide::East, SEAM_FACE_EXPORT_MAX);
        assert_eq!(west.seam.out_east.water, amt);
        assert_eq!(
            sat_totals(&west).cell_total + sat_totals(&east).cell_total,
            pair0
        );

        exchange_seam_p0(&mut west, &mut east);
        assert_eq!(west.seam.out_east.water, 0);
        let pair1 = sat_totals(&west).cell_total + sat_totals(&east).cell_total;
        assert_eq!(pair1, pair0);
        let east_edge_sat: i64 = (0..4)
            .filter_map(|y| east.get_cell(0, y).map(|c| c.sat.0 as i64))
            .sum();
        assert_eq!(east_edge_sat, amt);
    }

    #[test]
    fn neighbor_down_keeps_outbound_booked() {
        let (mut west, mut _east) = slab_pair();
        let edge = CHUNK_CELLS_W as i32 - 1;
        west.set_cell(edge, 1, Cell::water());
        let pair0 = sat_totals(&west).cell_total;
        export_seam_face(&mut west, SeamSide::East, SEAM_FACE_EXPORT_MAX);
        // No exchange — absorbing edge holds mass in seam backlog.
        assert!(west.seam.out_east.water > 0);
        assert_eq!(sat_totals(&west).cell_total, pair0);
    }

    #[test]
    fn wrap_ring_ignores_seam() {
        let mut w = World::new(3);
        w.wrap_width = Some(64);
        attach_open_seam(&mut w, 64, SeamSide::East);
        // attach clears wrap; restore ring + seam to prove guard
        w.wrap_width = Some(64);
        w.seam.config = Some(SeamConfig::open_side(64, SeamSide::East));
        assert_eq!(should_absorb_seam(&w, 64), None);
    }

    #[test]
    fn sediment_rides_seam_transfer() {
        let (mut west, mut east) = slab_pair();
        let edge = CHUNK_CELLS_W as i32 - 1;
        west.set_cell(edge, 1, Cell::water());
        add_suspended(&mut west, edge, 1, 40);
        let sed0 = sediment_total(&west) + sediment_total(&east);
        exchange_seam_p0(&mut west, &mut east);
        assert_eq!(sediment_total(&west) + sediment_total(&east), sed0);
    }
}
