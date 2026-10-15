//! The tank: physics grid, pixel layer, clock, save, tools.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::Cell;
use crate::clock::{self, day_factor};
use crate::consts::{MAX_VISIBLE_H, MAX_VISIBLE_W, MIN_VISIBLE, PRESET_H, PRESET_W, SAVE_VERSION};
use crate::grid::PhysicsGrid;
use crate::hash::hash_bytes;
use crate::pixels::{self, CoarseField, PixelLayer};
use crate::rules;
use crate::telemetry::{self, TelEvent, Telemetry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TankError {
    Size,
    Version,
    Shape,
    Postcard,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolEvent {
    /// 1 paint material, 2 erase to air, 3 pour water.
    pub kind: u8,
    pub material: u8,
    /// Visible physics cell, origin at the bottom-left of the interior.
    pub x: u16,
    pub y: u16,
    pub r: u16,
}

#[derive(Debug, Clone)]
pub struct TankInput {
    pub grav_x: f32,
    pub grav_y: f32,
    pub paused: bool,
    pub always_day: bool,
    pub light_slider: f32,
    pub t_base: f32,
    pub tools: Vec<ToolEvent>,
}

impl Default for TankInput {
    fn default() -> Self {
        Self {
            grav_x: 0.0,
            grav_y: -1.0,
            paused: false,
            always_day: false,
            light_slider: 1.0,
            t_base: crate::consts::T_BASE_DEFAULT,
            tools: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SimEvent {
    pub kind: u32,
    pub arg0: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Inspect {
    pub hit: bool,
    pub material: u8,
    pub sat: u8,
    pub owner: u32,
    pub vx: u32,
    pub vy: u32,
}

pub struct Tank {
    seed: u64,
    tick_count: u64,
    t_base: f32,
    light_slider: f32,
    stored_heat: f32,
    always_day: bool,
    grav_x: f32,
    grav_y: f32,
    grid: PhysicsGrid,
    pixels: PixelLayer,
    coarse: CoarseField,
    fines: Vec<f32>,
    events: VecDeque<SimEvent>,
    telemetry: Telemetry,
}

impl Tank {
    pub fn try_new(seed: u64, visible_w: u32, visible_h: u32) -> Result<Self, TankError> {
        if visible_w < MIN_VISIBLE
            || visible_h < MIN_VISIBLE
            || visible_w > MAX_VISIBLE_W
            || visible_h > MAX_VISIBLE_H
        {
            return Err(TankError::Size);
        }
        let grid = PhysicsGrid::new(visible_w, visible_h);
        let pixels = PixelLayer::new(visible_w, visible_h);
        let coarse = CoarseField::new(visible_w, visible_h);
        let fines = vec![0.0; pixels.w as usize];
        let mut tank = Self {
            seed,
            tick_count: 0,
            t_base: crate::consts::T_BASE_DEFAULT,
            light_slider: 1.0,
            stored_heat: 0.0,
            always_day: false,
            grav_x: 0.0,
            grav_y: -1.0,
            grid,
            pixels,
            coarse,
            fines,
            events: VecDeque::new(),
            telemetry: Telemetry::default(),
        };
        tank.rebuild_light();
        Ok(tank)
    }

    /// Phone preset: bedrock frame outside the picture, a sand bed,
    /// water to a bit over half, one static stone, and a sand column
    /// that collapses on the first ticks.
    pub fn preset(seed: u64) -> Self {
        let mut tank = Self::try_new(seed, PRESET_W, PRESET_H).expect("preset size");
        let w = tank.grid.visible_w();
        let h = tank.grid.visible_h();
        let sand_top = (h as f32 * 0.18) as i32;
        let water_top = (h as f32 * 0.55) as i32;
        for vy in 0..sand_top {
            for vx in 0..w {
                tank.grid.set_visible(vx, vy, Cell::sand());
            }
        }
        for vy in sand_top..water_top {
            for vx in 0..w {
                tank.grid.set_visible(vx, vy, Cell::water());
            }
        }
        let stone_y = sand_top + 4;
        if stone_y < h {
            tank.grid.set_visible(16, stone_y, Cell::stone());
        }
        let cx = w / 2;
        for dy in 0..24 {
            let vy = water_top + dy;
            if vy < h - 1 {
                tank.grid.set_visible(cx, vy, Cell::sand());
            }
        }
        tank.rebuild_light();
        tank
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }

    pub fn t_base(&self) -> f32 {
        self.t_base
    }

    pub fn light_slider(&self) -> f32 {
        self.light_slider
    }

    pub fn stored_heat(&self) -> f32 {
        self.stored_heat
    }

    pub fn always_day(&self) -> bool {
        self.always_day
    }

    pub fn gravity(&self) -> (f32, f32) {
        (self.grav_x, self.grav_y)
    }

    pub fn visible_size(&self) -> (u32, u32) {
        (self.grid.visible_w() as u32, self.grid.visible_h() as u32)
    }

    pub fn pixel_size(&self) -> (u32, u32) {
        (self.pixels.w, self.pixels.h)
    }

    pub fn day_factor_now(&self) -> f32 {
        if self.always_day {
            1.0
        } else {
            day_factor(self.tick_count)
        }
    }

    pub fn is_day_now(&self) -> bool {
        self.always_day || clock::is_day(self.tick_count)
    }

    pub fn phase_name(&self) -> &'static str {
        if self.is_day_now() {
            "day"
        } else {
            "night"
        }
    }

    pub fn sat_sum(&self) -> u64 {
        self.grid.sat_sum()
    }

    pub fn count_material(&self, material: u8) -> usize {
        self.grid.count_material(material)
    }

    pub fn cell_visible(&self, vx: i32, vy: i32) -> Cell {
        self.grid.get_visible(vx, vy)
    }

    #[cfg(test)]
    pub(crate) fn place_for_test(&mut self, vx: i32, vy: i32, cell: Cell) {
        self.grid.set_visible(vx, vy, cell);
    }

    pub fn tick(&mut self, input: &TankInput) {
        self.t_base = input.t_base;
        self.light_slider = input.light_slider.clamp(0.0, 1.0);
        self.always_day = input.always_day;
        self.grav_x = input.grav_x;
        self.grav_y = input.grav_y;
        if input.paused {
            self.rebuild_light();
            return;
        }
        self.apply_tools(&input.tools);
        // grav_y is stored for the Phase 1 tilt spike. This core always
        // steps downward; the value is part of the save so a host can
        // round-trip the last input.
        rules::step(&mut self.grid, self.seed, self.tick_count);
        self.tick_count = self.tick_count.wrapping_add(1);
        // Incident light is a pure function of the grid and the clock.
        // It is rebuilt in `write_rgba` / `save_bytes` (and on a paused
        // tick, so a slider change shows up) rather than on every step,
        // which keeps the 128×256 physics tick inside its budget.
    }

    pub fn poll_event(&mut self) -> Option<SimEvent> {
        self.events.pop_front()
    }

    /// Pixel coordinate, origin top-left of the RGBA buffer.
    pub fn inspect(&self, px: u32, py: u32) -> Inspect {
        if px >= self.pixels.w || py >= self.pixels.h {
            return Inspect {
                hit: false,
                material: 0,
                sat: 0,
                owner: 0,
                vx: 0,
                vy: 0,
            };
        }
        let vx = px / crate::consts::PX_PER_CELL;
        let vy_top = py / crate::consts::PX_PER_CELL;
        let vy = self.grid.visible_h() as u32 - 1 - vy_top;
        let cell = self.grid.get_visible(vx as i32, vy as i32);
        let owner = self.pixels.owner[(py as usize) * (self.pixels.w as usize) + px as usize];
        Inspect {
            hit: true,
            material: cell.material,
            sat: cell.sat,
            owner,
            vx,
            vy,
        }
    }

    pub fn write_rgba(&mut self, out: &mut Vec<u8>) {
        self.rebuild_light();
        pixels::write_rgba(&self.grid, &self.pixels, out);
    }

    pub fn pixel_hash(&mut self) -> u64 {
        let mut buf = Vec::new();
        self.write_rgba(&mut buf);
        hash_bytes(&buf)
    }

    pub fn save_bytes(&mut self) -> Result<Vec<u8>, TankError> {
        self.rebuild_light();
        let blob = SaveBlob {
            version: SAVE_VERSION,
            seed: self.seed,
            tick_count: self.tick_count,
            w: self.grid.width() as u32,
            h: self.grid.height() as u32,
            t_base: self.t_base,
            light_slider: self.light_slider,
            stored_heat: self.stored_heat,
            always_day: self.always_day,
            grav_x: self.grav_x,
            grav_y: self.grav_y,
            cells: self.grid.cells().to_vec(),
            pix_w: self.pixels.w,
            pix_h: self.pixels.h,
            owner: self.pixels.owner.clone(),
            light: self.pixels.light.clone(),
            glucose: self.pixels.glucose.clone(),
            waste: self.pixels.waste.clone(),
            enzyme_amt: self.pixels.enzyme_amt.clone(),
            enzyme_key: self.pixels.enzyme_key.clone(),
            toxin_amt: self.pixels.toxin_amt.clone(),
            toxin_key: self.pixels.toxin_key.clone(),
            signal_amt: self.pixels.signal_amt.clone(),
            signal_key: self.pixels.signal_key.clone(),
            film: self.pixels.film.clone(),
            fines: self.fines.clone(),
            coarse_w: self.coarse.w,
            coarse_h: self.coarse.h,
            minerals: self.coarse.minerals.clone(),
            t_local: self.coarse.t_local.clone(),
            humidity: self.coarse.humidity.clone(),
        };
        postcard::to_allocvec(&blob).map_err(|_| TankError::Postcard)
    }

    pub fn load_bytes(bytes: &[u8]) -> Result<Self, TankError> {
        let blob: SaveBlob = postcard::from_bytes(bytes).map_err(|_| TankError::Postcard)?;
        if blob.version != SAVE_VERSION {
            return Err(TankError::Version);
        }
        let visible_w = blob.w.checked_sub(2).ok_or(TankError::Shape)?;
        let visible_h = blob.h.checked_sub(2).ok_or(TankError::Shape)?;
        let mut tank = Self::try_new(blob.seed, visible_w, visible_h)?;
        if blob.cells.len() != (blob.w as usize) * (blob.h as usize) {
            return Err(TankError::Shape);
        }
        let n = tank.pixels.len();
        if blob.pix_w != tank.pixels.w
            || blob.pix_h != tank.pixels.h
            || blob.owner.len() != n
            || blob.light.len() != n
            || blob.glucose.len() != n
            || blob.waste.len() != n
            || blob.enzyme_amt.len() != n
            || blob.enzyme_key.len() != n
            || blob.toxin_amt.len() != n
            || blob.toxin_key.len() != n
            || blob.signal_amt.len() != n
            || blob.signal_key.len() != n
            || blob.film.len() != n
            || blob.fines.len() != tank.pixels.w as usize
            || blob.coarse_w != tank.coarse.w
            || blob.coarse_h != tank.coarse.h
            || blob.minerals.len() != tank.coarse.minerals.len()
            || blob.t_local.len() != tank.coarse.t_local.len()
            || blob.humidity.len() != tank.coarse.humidity.len()
        {
            return Err(TankError::Shape);
        }
        tank.tick_count = blob.tick_count;
        tank.t_base = blob.t_base;
        tank.light_slider = blob.light_slider;
        tank.stored_heat = blob.stored_heat;
        tank.always_day = blob.always_day;
        tank.grav_x = blob.grav_x;
        tank.grav_y = blob.grav_y;
        tank.grid.cells_mut().copy_from_slice(&blob.cells);
        // The frame is part of the blob. Re-assert bedrock so a corrupt
        // interior save cannot delete the tank walls.
        reinforce_border(&mut tank.grid);
        tank.pixels.owner = blob.owner;
        tank.pixels.light = blob.light;
        tank.pixels.glucose = blob.glucose;
        tank.pixels.waste = blob.waste;
        tank.pixels.enzyme_amt = blob.enzyme_amt;
        tank.pixels.enzyme_key = blob.enzyme_key;
        tank.pixels.toxin_amt = blob.toxin_amt;
        tank.pixels.toxin_key = blob.toxin_key;
        tank.pixels.signal_amt = blob.signal_amt;
        tank.pixels.signal_key = blob.signal_key;
        tank.pixels.film = blob.film;
        tank.fines = blob.fines;
        tank.coarse.minerals = blob.minerals;
        tank.coarse.t_local = blob.t_local;
        tank.coarse.humidity = blob.humidity;
        Ok(tank)
    }

    pub fn telemetry_mut(&mut self) -> &mut Telemetry {
        &mut self.telemetry
    }

    pub fn format_telemetry(&self, events: &[TelEvent], anon: &str) -> String {
        telemetry::format_batch(events, anon)
    }

    fn rebuild_light(&mut self) {
        pixels::rebuild_light(
            &self.grid,
            &mut self.pixels,
            self.tick_count,
            self.always_day,
            self.light_slider,
        );
    }

    fn apply_tools(&mut self, tools: &[ToolEvent]) {
        for tool in tools {
            let r = tool.r.max(1) as i32;
            let cx = tool.x as i32;
            let cy = tool.y as i32;
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy > r * r {
                        continue;
                    }
                    let vx = cx + dx;
                    let vy = cy + dy;
                    if vx < 0
                        || vy < 0
                        || vx >= self.grid.visible_w()
                        || vy >= self.grid.visible_h()
                    {
                        continue;
                    }
                    match tool.kind {
                        1 => {
                            let id = MaterialId::from_u8(tool.material).unwrap_or(MaterialId::Sand);
                            if id == MaterialId::Bedrock {
                                continue;
                            }
                            let mut cell = Cell::solid(id);
                            if id == MaterialId::Air {
                                cell = Cell::air();
                            }
                            self.grid.set_visible(vx, vy, cell);
                        }
                        2 => self.grid.set_visible(vx, vy, Cell::air()),
                        3 => self.grid.set_visible(vx, vy, Cell::water()),
                        _ => {}
                    }
                }
            }
        }
    }
}

fn reinforce_border(grid: &mut PhysicsGrid) {
    let w = grid.width();
    let h = grid.height();
    for y in 0..h {
        for x in 0..w {
            if grid.is_border(x, y) {
                let idx = (y * w + x) as usize;
                grid.cells_mut()[idx] = Cell::bedrock();
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SaveBlob {
    version: u32,
    seed: u64,
    #[serde(default)]
    tick_count: u64,
    w: u32,
    h: u32,
    #[serde(default = "default_t_base")]
    t_base: f32,
    #[serde(default = "default_slider")]
    light_slider: f32,
    #[serde(default)]
    stored_heat: f32,
    #[serde(default)]
    always_day: bool,
    #[serde(default)]
    grav_x: f32,
    #[serde(default = "default_grav_y")]
    grav_y: f32,
    cells: Vec<Cell>,
    pix_w: u32,
    pix_h: u32,
    owner: Vec<u32>,
    light: Vec<u8>,
    glucose: Vec<u8>,
    waste: Vec<u8>,
    enzyme_amt: Vec<u8>,
    enzyme_key: Vec<u8>,
    toxin_amt: Vec<u8>,
    toxin_key: Vec<u8>,
    signal_amt: Vec<u8>,
    signal_key: Vec<u8>,
    film: Vec<u8>,
    fines: Vec<f32>,
    coarse_w: u32,
    coarse_h: u32,
    minerals: Vec<u8>,
    t_local: Vec<i8>,
    humidity: Vec<u8>,
}

fn default_t_base() -> f32 {
    crate::consts::T_BASE_DEFAULT
}

fn default_slider() -> f32 {
    1.0
}

fn default_grav_y() -> f32 {
    -1.0
}
