//! The tank: physics grid, pixel layer, clock, save, tools.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::bio::store::OWNER_BIAS;
use crate::bio::{self, CellStore, GenomeStore};
use crate::cell::Cell;
use crate::clock::{self, day_factor};
use crate::consts::{MAX_VISIBLE_H, MAX_VISIBLE_W, MIN_VISIBLE, PRESET_H, PRESET_W, SAVE_VERSION};
use crate::grid::PhysicsGrid;
use crate::hash::hash_bytes;
use crate::pixels::{self, CoarseField, PixelLayer};
use crate::rules;
use crate::telemetry::{self, TelEvent, Telemetry};
use crate::tools::{self, UndoFrame, UNDO_CAP};

pub use crate::tools::ToolEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TankError {
    Size,
    Version,
    Shape,
    Postcard,
}

#[derive(Debug, Clone)]
pub struct TankInput {
    pub grav_x: f32,
    pub grav_y: f32,
    pub paused: bool,
    pub always_day: bool,
    /// When false (default), gyro tilt is ignored. Phase 1.5.
    pub swirl: bool,
    pub light_slider: f32,
    pub t_base: f32,
    /// Flashlight cone centre in visible physics cells; radius 0 = off.
    pub flash_x: i32,
    pub flash_y: i32,
    pub flash_r: i32,
    pub tools: Vec<ToolEvent>,
}

impl Default for TankInput {
    fn default() -> Self {
        Self {
            grav_x: 0.0,
            grav_y: -1.0,
            paused: false,
            always_day: false,
            swirl: false,
            light_slider: 1.0,
            t_base: crate::consts::T_BASE_DEFAULT,
            flash_x: 0,
            flash_y: 0,
            flash_r: 0,
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
    pub moisture: u8,
    pub variant: u8,
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
    swirl: bool,
    grav_x: f32,
    grav_y: f32,
    grid: PhysicsGrid,
    pixels: PixelLayer,
    coarse: CoarseField,
    fines: Vec<f32>,
    events: VecDeque<SimEvent>,
    telemetry: Telemetry,
    undo: Vec<UndoFrame>,
    cells: CellStore,
    genomes: GenomeStore,
    flash_x: i32,
    flash_y: i32,
    flash_r: i32,
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
            swirl: false,
            grav_x: 0.0,
            grav_y: -1.0,
            grid,
            pixels,
            coarse,
            fines,
            events: VecDeque::new(),
            telemetry: Telemetry::default(),
            undo: Vec::new(),
            cells: CellStore::new(),
            genomes: GenomeStore::new(),
            flash_x: 0,
            flash_y: 0,
            flash_r: 0,
        };
        tank.rebuild_light();
        Ok(tank)
    }

    /// Empty tank with a thin sand bed — the "new tank" starting point.
    pub fn empty(seed: u64) -> Self {
        let mut tank = Self::try_new(seed, PRESET_W, PRESET_H).expect("preset size");
        let w = tank.grid.visible_w();
        let bed = 4;
        for vy in 0..bed {
            for vx in 0..w {
                tank.grid.set_visible(vx, vy, Cell::sand());
            }
        }
        tank.rebuild_light();
        tank
    }

    /// Phone preset: sand bed, water, one static stone, a sand column.
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
            let mut stone = Cell::stone();
            stone.set_colour_variant(0);
            tank.grid.set_visible(16, stone_y, stone);
        }
        let cx = w / 2;
        for dy in 0..24 {
            let vy = water_top + dy;
            if vy < h - 1 {
                tank.grid.set_visible(cx, vy, Cell::sand());
            }
        }
        // Seed one algae near the waterline, centre.
        let px = (w as u32 / 2) * 3;
        let py = ((h - water_top - 2) as u32) * 3;
        let _ = tank.seed_algae(px as u16, py as u16);
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

    pub fn swirl(&self) -> bool {
        self.swirl
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

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn live_cells(&self) -> usize {
        self.cells.len()
    }

    pub fn genotype_count(&self) -> usize {
        self.genomes.len()
    }

    pub fn seed_algae(&mut self, px: u16, py: u16) -> Option<u32> {
        let id = self
            .cells
            .seed_algae(&mut self.genomes, &mut self.pixels, px, py, self.seed)?;
        self.events.push_back(SimEvent {
            kind: 1, // first life / seed
            arg0: id,
        });
        Some(id)
    }

    /// Seed a cell whose mode targets `shape_w × shape_h` (growth tests).
    pub fn seed_shaped_algae(&mut self, px: u16, py: u16, shape_w: u8, shape_h: u8) -> Option<u32> {
        use crate::bio::GenomeV0;
        let mut g = GenomeV0::algae();
        g.mode.shape_w = shape_w.clamp(1, crate::consts::SHAPE_CAP);
        g.mode.shape_h = shape_h.clamp(1, crate::consts::SHAPE_CAP);
        let parent = self.genomes.seed_algae();
        let geno = self.genomes.intern_child(parent, g);
        self.cells.spawn(
            &mut self.genomes,
            &mut self.pixels,
            px,
            py,
            geno,
            0,
            12 * 256,
            2,
            self.seed,
        )
    }

    pub fn cell_size(&self, id: u32) -> Option<(u8, u8)> {
        self.cells.get(id).map(|c| (c.w, c.h))
    }

    pub fn pixel_light_at(&self, px: u16, py: u16) -> u8 {
        let idx = (py as usize) * (self.pixels.w as usize) + px as usize;
        self.pixels.light.get(idx).copied().unwrap_or(0)
    }

    pub fn org_slot_count(&self, cell_id: u32) -> u8 {
        self.cells
            .get(cell_id)
            .map(|c| self.cells.org_slot_count(c.org))
            .unwrap_or(0)
    }

    pub fn glue_attachments(&self) -> u32 {
        self.cells.glue_attachments()
    }

    pub fn seed_genome_at(
        &mut self,
        px: u16,
        py: u16,
        genome: crate::bio::GenomeV0,
    ) -> Option<u32> {
        let parent = self.genomes.seed_algae();
        let geno = self.genomes.intern_child(parent, genome);
        self.cells.spawn(
            &mut self.genomes,
            &mut self.pixels,
            px,
            py,
            geno,
            0,
            12 * 256,
            2,
            self.seed,
        )
    }

    /// Clear front-pane film along a stroke (pixel space, top-left origin).
    pub fn swipe_film(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, radius: i32) -> u32 {
        bio::swipe_film(&mut self.pixels, x0, y0, x1, y1, radius)
    }

    pub fn remove_cells_at(&mut self, vx: i32, vy: i32, r: i32) {
        let r = r.max(0);
        let ids: Vec<u32> = self.cells.iter_alive().map(|(id, _)| id).collect();
        for id in ids {
            let Some(c) = self.cells.get(id).copied() else {
                continue;
            };
            let cx = c.px as i32 / 3;
            let cy_top = c.py as i32 / 3;
            let cy = self.grid.visible_h() - 1 - cy_top;
            let dx = cx - vx;
            let dy = cy - vy;
            if dx * dx + dy * dy <= r * r {
                self.cells
                    .kill(&mut self.genomes, &mut self.pixels, id, false);
            }
        }
    }

    pub fn drip_glucose(&mut self, vx: i32, vy: i32, r: i32, amount: u8) {
        let r = r.max(1);
        let vis_h = self.grid.visible_h();
        let pw = self.pixels.w as i32;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                let x = vx + dx;
                let y = vy + dy;
                if x < 0 || y < 0 || x >= self.grid.visible_w() || y >= vis_h {
                    continue;
                }
                let py_top = vis_h - 1 - y;
                for sy in 0..3 {
                    for sx in 0..3 {
                        let px = x * 3 + sx;
                        let py = py_top * 3 + sy;
                        let idx = (py * pw + px) as usize;
                        if idx < self.pixels.glucose.len() {
                            self.pixels.glucose[idx] =
                                self.pixels.glucose[idx].saturating_add(amount);
                        }
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn place_for_test(&mut self, vx: i32, vy: i32, cell: Cell) {
        self.grid.set_visible(vx, vy, cell);
    }

    #[cfg(test)]
    pub(crate) fn pixels_film_set_for_test(&mut self, idx: usize, v: u8) {
        if idx < self.pixels.film.len() {
            self.pixels.film[idx] = v;
        }
    }

    pub fn tick(&mut self, input: &TankInput) {
        self.t_base = input.t_base;
        self.light_slider = input.light_slider.clamp(0.0, 1.0);
        self.always_day = input.always_day;
        self.swirl = input.swirl;
        self.grav_x = input.grav_x;
        self.grav_y = input.grav_y;
        self.flash_x = input.flash_x;
        self.flash_y = input.flash_y;
        self.flash_r = input.flash_r;
        if input.paused {
            self.apply_tools(&input.tools);
            self.rebuild_light();
            return;
        }
        self.apply_tools(&input.tools);
        self.rebuild_light();
        // Tank temperature: base + solar heat (flashlight does not heat, K4#7).
        let day = if self.always_day {
            1.0
        } else {
            crate::clock::day_factor(self.tick_count)
        };
        let t_tank = self.t_base + 10.0 * self.light_slider.clamp(0.0, 1.0) * day;
        let tilt = if self.swirl { self.grav_x } else { 0.0 };
        rules::step(&mut self.grid, self.seed, self.tick_count, tilt);
        let bio = bio::step(
            &mut self.cells,
            &mut self.genomes,
            &mut self.grid,
            &mut self.pixels,
            self.seed,
            self.tick_count,
            t_tank,
        );
        for ev in bio.events {
            self.events.push_back(ev);
        }
        if bio.deaths > 0 && self.cells.is_empty() {
            self.events.push_back(SimEvent {
                kind: 10, // lifeless
                arg0: 0,
            });
        }
        // "glass needs cleaning" when film coverage is high.
        let film_n = self.pixels.film.iter().filter(|&&f| f > 80).count();
        if film_n > (self.pixels.len() / 40) && self.tick_count % 300 == 0 {
            self.events.push_back(SimEvent {
                kind: 11, // glass dirty
                arg0: film_n as u32,
            });
        }
        self.tick_count = self.tick_count.wrapping_add(1);
    }

    /// Restore the previous build snapshot. Returns false when empty.
    pub fn undo(&mut self) -> bool {
        let Some(frame) = self.undo.pop() else {
            return false;
        };
        tools::restore(&mut self.grid, &frame);
        true
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
                moisture: 0,
                variant: 0,
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
            moisture: cell.moisture(),
            variant: cell.colour_variant(),
            owner,
            vx,
            vy,
        }
    }

    pub fn write_rgba(&mut self, out: &mut Vec<u8>) {
        self.rebuild_light();
        let cells = &self.cells;
        let genomes = &self.genomes;
        pixels::write_rgba(&self.grid, &self.pixels, out, &|owner| {
            if owner < OWNER_BIAS {
                return None;
            }
            let id = owner - OWNER_BIAS;
            let cell = cells.get(id)?;
            if !cell.alive {
                return None;
            }
            let g = genomes.genome(cell.genotype);
            let mut rgb = g.mode.colour(g.metab_eff);
            // Energy brightens a little so hungry cells read duller.
            let e = (cell.energy / 256).min(8);
            rgb[1] = (rgb[1] as u16 + e * 2).min(255) as u8;
            // Desaturate under stress (pull toward gray).
            if cell.stress > 32 {
                let s = cell.stress as u16;
                let gray = (rgb[0] as u16 + rgb[1] as u16 + rgb[2] as u16) / 3;
                for c in &mut rgb {
                    *c = ((*c as u16 * (255 - s) + gray * s) / 255) as u8;
                }
            }
            Some(rgb)
        });
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
            swirl: self.swirl,
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
        tank.swirl = blob.swirl;
        tank.grav_x = blob.grav_x;
        tank.grav_y = blob.grav_y;
        tank.grid.cells_mut().copy_from_slice(&blob.cells);
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
        let flash = if self.flash_r > 0 {
            Some(pixels::Flashlight {
                vx: self.flash_x,
                radius: self.flash_r,
                boost: 512, // ×2, does not heat (K4#7)
            })
        } else {
            None
        };
        pixels::rebuild_light(
            &self.grid,
            &mut self.pixels,
            self.tick_count,
            self.always_day,
            self.light_slider,
            flash,
        );
    }

    fn apply_tools(&mut self, tools: &[ToolEvent]) {
        if tools.is_empty() {
            return;
        }
        let mut need_push = false;
        for tool in tools {
            if tool.kind == 6 {
                let _ = self.undo();
                continue;
            }
            if tool.kind == 7 {
                // Swipe-clean film: (x,y) start, r = radius; end ≈ start for a dab.
                let _ = self.swipe_film(
                    tool.x as i32,
                    tool.y as i32,
                    tool.x as i32,
                    tool.y as i32,
                    tool.r.max(2) as i32,
                );
                continue;
            }
            if tool.kind == 8 {
                // Seed algae at a physics cell → pixel centre.
                let px = tool.x as u32 * 3 + 1;
                let py_phys = tool.y as u32;
                let vis_h = self.grid.visible_h() as u32;
                let py_top = vis_h.saturating_sub(1).saturating_sub(py_phys);
                let py = py_top * 3 + 1;
                let _ = self.seed_algae(px as u16, py as u16);
                continue;
            }
            if tool.kind == 9 {
                // Remove living cell under the brush (thin selection).
                self.remove_cells_at(tool.x as i32, tool.y as i32, tool.r as i32);
                continue;
            }
            if tool.kind == 10 {
                // Glucose drip into the dissolved field.
                self.drip_glucose(tool.x as i32, tool.y as i32, tool.r as i32, 180);
                continue;
            }
            if !need_push {
                self.undo.push(tools::capture(&self.grid));
                if self.undo.len() > UNDO_CAP {
                    self.undo.remove(0);
                }
                need_push = true;
            }
            let _ = tools::apply_one(&mut self.grid, tool);
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
    swirl: bool,
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
