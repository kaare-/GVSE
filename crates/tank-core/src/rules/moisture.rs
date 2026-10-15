//! Rock wet film. Sand already carries pore `sat` from seepage. Rock is
//! almost impermeable, so a thin film is copied from neighbouring water
//! or wet sand and fades slowly when those neighbours dry out.

use crate::cell::Cell;
use crate::consts::STANDING_AIR_SAT;
use crate::grid::PhysicsGrid;
use crate::rules::plan::ActiveRect;

const FILM_GAIN: u8 = 48;
const FILM_FADE: u8 = 2;

pub fn apply(grid: &mut PhysicsGrid, active: &ActiveRect) {
    let mut updates: Vec<(i32, i32, u8)> = Vec::new();
    for y in active.y0..=active.y1 {
        for x in active.x0..=active.x1 {
            let cell = grid.get(x, y);
            if !cell.is_rock() {
                continue;
            }
            let mut source = 0u8;
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let n = grid.get(x + dx, y + dy);
                source = source.max(neighbour_wetness(n));
            }
            let mut film = cell.pad;
            if source > 0 {
                film = film.saturating_add(FILM_GAIN).min(source.max(FILM_GAIN));
                if source >= STANDING_AIR_SAT {
                    film = film.max(200);
                }
            } else {
                film = film.saturating_sub(FILM_FADE);
            }
            if film != cell.pad {
                updates.push((x, y, film));
            }
        }
    }
    for (x, y, film) in updates {
        let mut cell = grid.get(x, y);
        cell.set_rock_film(film);
        grid.set(x, y, cell);
    }
}

fn neighbour_wetness(n: Cell) -> u8 {
    if n.is_air() {
        n.sat
    } else if n.is_rock() {
        n.pad
    } else {
        n.sat
    }
}
