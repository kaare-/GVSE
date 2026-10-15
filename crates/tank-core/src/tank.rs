//! The tank: physics grid, pixel layer, clock, save, tools.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::bio::store::{secretion_kind, BONE_OWNER, OWNER_BIAS, SHELL_OWNER};
use crate::bio::{self, CellStore, Genome, GenomeStore};
use crate::cell::Cell;
use crate::clock::{self, day_factor};
use crate::consts::{
    MAX_VISIBLE_H, MAX_VISIBLE_W, MIN_VISIBLE, PRESET_H, PRESET_W, SAVE_VERSION, TISSUE_DAM_PIXELS,
};
use crate::grid::PhysicsGrid;
use crate::hash::hash_bytes;
use crate::milestones::{self, Milestones};
use crate::pixels::{self, CoarseField, PixelLayer};
use crate::rules;
use crate::telemetry::{self, TelEvent, Telemetry};
use crate::thermal;
use crate::tools::{self, UndoFrame, UNDO_CAP};
use wk_material::MaterialId;

pub use crate::tools::ToolEvent;

/// Light passing a corpse pixel (Q8): dead soft tissue, slightly murky.
const CORPSE_PASS_Q8: u8 = 192;

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

#[derive(Debug, Clone, Copy, Default)]
pub struct Inspect {
    pub hit: bool,
    pub material: u8,
    pub sat: u8,
    pub moisture: u8,
    pub variant: u8,
    pub owner: u32,
    pub vx: u32,
    pub vy: u32,
    /// Living cell under the pixel (`u32::MAX` = none) and its organism.
    pub cell: u32,
    pub org: u32,
    pub genotype: u32,
    pub mode: u8,
    pub generation: u32,
    /// Cells in the organism and kin organisms glued to it (§b.5 inspect).
    pub org_cells: u32,
    pub glued_kin: u32,
    pub age_ticks: u32,
    pub max_age_ticks: u32,
    pub energy_frac: u8,
    pub stress: u8,
    /// Species cluster of the genotype (`NO_SPECIES` when no cell).
    pub species: u16,
    /// Children this organism has released (propagules + broods).
    pub children: u32,
}

/// Phase 3 inspect lineage card (`roadmap.md` Phase 3): species, where
/// it sits in the tree, and who else is alive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineageCard {
    pub cell: u32,
    pub org: u32,
    pub genotype: u32,
    pub species: u16,
    pub species_name: String,
    pub parent_species_name: String,
    /// Organism divisions since the root organism.
    pub generation: u32,
    /// Mutation events between this genotype and its root.
    pub mutations_since_seed: u16,
    /// Species on the path from the root (the numeral).
    pub species_depth: u16,
    /// Live cells of the same species, this organism excluded.
    pub kin_alive: u32,
    /// Live cells of this genotype and everything descended from it.
    pub descendants_alive: u32,
    /// Archived species on the ancestor path, this species included.
    pub archived_relatives: u32,
    /// Children this organism has released.
    pub children: u32,
}

/// What the scoop holds (Phase 3): whole organisms with their records,
/// litter and grit by offset from the bucket centre, and water in
/// saturation units. Not part of the save — the shell releases or
/// discards before saving.
#[derive(Debug, Clone)]
pub struct Carried {
    pub cx: i32,
    pub cy: i32,
    pub r: i32,
    pub orgs: Vec<bio::OrgBundle>,
    /// `(dx, dy, units, litter code)`.
    pub litter: Vec<(i16, i16, u8, u8)>,
    /// `(dx, dy, units)`.
    pub grit: Vec<(i16, i16, u8)>,
    pub water: u32,
}

impl Carried {
    pub fn empty() -> Self {
        Self {
            cx: 0,
            cy: 0,
            r: 0,
            orgs: Vec::new(),
            litter: Vec::new(),
            grit: Vec::new(),
            water: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.orgs.is_empty() && self.litter.is_empty() && self.grit.is_empty() && self.water == 0
    }

    pub fn cell_count(&self) -> u32 {
        self.orgs.iter().map(|b| b.cells.len() as u32).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseError {
    /// Some organism would land on tissue, a solid or outside the tank.
    NoRoom,
    OutOfBounds,
}

pub struct Tank {
    seed: u64,
    tick_count: u64,
    t_base: f32,
    light_slider: f32,
    stored_heat: f32,
    /// Stored-heat time constant (ticks), re-measured every
    /// `THERMAL_MASS_EVERY` ticks from the material fractions.
    thermal_tau: f32,
    /// `T_tank` of the last tick (the thermometer).
    t_tank: f32,
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
    light_dirty: bool,
    /// Cumulative deaths by cause: [starved, old age, damage, shed].
    death_causes: [u64; 4],
    /// Physics cells currently flagged as hard tissue (visible coords).
    tissue_marks: Vec<(i32, i32)>,
    /// Mutation-rate multiplier (Q8, 256 = ×1); harness lever.
    lambda_scale_q8: u16,
    /// 2.3.8 once-per-tank milestone detectors.
    milestones: Milestones,
    /// Lineage roots seeded from the archive: `(genotype, specimen id)`.
    archive_roots: Vec<(u32, u64)>,
    /// Last "water is cloudy" announcement (Phase 4).
    cloudy_last: Option<u64>,
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
        let mut coarse = CoarseField::new(visible_w, visible_h);
        coarse.minerals.fill(bio::minerals::MINERAL_DEFAULT);
        let fines = vec![0.0; pixels.w as usize];
        let mut tank = Self {
            seed,
            tick_count: 0,
            t_base: crate::consts::T_BASE_DEFAULT,
            light_slider: 1.0,
            stored_heat: 0.0,
            thermal_tau: 0.0,
            t_tank: crate::consts::T_BASE_DEFAULT,
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
            light_dirty: true,
            death_causes: [0; 4],
            tissue_marks: Vec::new(),
            milestones: Milestones::default(),
            lambda_scale_q8: 256,
            archive_roots: Vec::new(),
            cloudy_last: None,
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
        tank.light_dirty = true;
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
        let py = ((h - water_top + 1) as u32) * 3;
        let _ = tank.seed_algae(px as u16, py as u16);
        tank.light_dirty = true;
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

    /// The thermometer: `T_tank` of the last tick, °C.
    pub fn t_tank(&self) -> f32 {
        self.t_tank
    }

    /// Degrees over ambient at a visible physics cell (THERMO halo).
    pub fn t_local_at(&self, vx: i32, vy: i32) -> f32 {
        if vx < 0 || vy < 0 || vx >= self.coarse.w as i32 || vy >= self.coarse.h as i32 {
            return 0.0;
        }
        thermal::local_c(
            &self.coarse,
            (vy as usize) * self.coarse.w as usize + vx as usize,
        )
    }

    /// Material fractions by thermal class (harness / tests).
    pub fn thermal_mass(&self) -> thermal::ThermalMass {
        thermal::ThermalMass::measure(&self.grid, self.cells.len())
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

    /// Live cells per living genotype (harness diversity metrics).
    pub fn genotype_live_counts(&self) -> Vec<u32> {
        (0..self.genomes.node_count() as u32)
            .filter_map(|id| self.genomes.get(id))
            .filter(|n| n.live_cells > 0 && !n.tombstone)
            .map(|n| n.live_cells)
            .collect()
    }

    pub fn seed_algae(&mut self, px: u16, py: u16) -> Option<u32> {
        let id = self.cells.seed_algae(
            &mut self.genomes,
            &mut self.pixels,
            px,
            py,
            self.tick_count,
            self.seed,
        )?;
        self.events.push_back(SimEvent {
            kind: bio::ev::SEED,
            arg0: id,
        });
        self.light_dirty = true;
        Some(id)
    }

    /// Seed a cell whose mode targets `shape_w × shape_h` (growth tests).
    pub fn seed_shaped_algae(&mut self, px: u16, py: u16, shape_w: u8, shape_h: u8) -> Option<u32> {
        let mut g = Genome::algae();
        let m = &mut g.modes[0];
        let old = m.capacity();
        m.shape_w = shape_w.clamp(1, crate::consts::SHAPE_CAP);
        m.shape_h = shape_h.clamp(1, crate::consts::SHAPE_CAP);
        let new = m.capacity();
        m.rescale_units(old, new);
        self.seed_genome_at(px, py, g)
    }

    pub fn cell_size(&self, id: u32) -> Option<(u8, u8)> {
        self.cells.get(id).map(|c| (c.w, c.h))
    }

    pub fn cell(&self, id: u32) -> Option<&bio::CellRec> {
        self.cells.get(id)
    }

    pub fn org_of_cell(&self, id: u32) -> Option<&bio::OrgRec> {
        self.cells.get(id).and_then(|c| self.cells.org(c.org))
    }

    pub fn cells(&self) -> &CellStore {
        &self.cells
    }

    pub fn genomes(&self) -> &GenomeStore {
        &self.genomes
    }

    /// Scale every genome's mutation rate (Q8; 0 = breed true, 256 = ×1).
    pub fn set_lambda_scale_q8(&mut self, q8: u16) {
        self.lambda_scale_q8 = q8;
    }

    /// Cumulative deaths by cause: [starved, old age, damage, shed].
    pub fn death_causes(&self) -> [u64; 4] {
        self.death_causes
    }

    /// Material id of the physics cell under a pixel (harness probe).
    pub fn material_at(&self, px: u16, py: u16) -> u8 {
        let (vx, vy) = bio::pixel_to_phys(px, py, self.grid.visible_h() as u32);
        self.grid.get_visible(vx, vy).material
    }

    /// Water saturation of the physics cell under a pixel (harness probe).
    pub fn water_sat_at(&self, px: u16, py: u16) -> u8 {
        let (vx, vy) = bio::pixel_to_phys(px, py, self.grid.visible_h() as u32);
        let c = self.grid.get_visible(vx, vy);
        if c.is_open() {
            c.sat
        } else {
            0
        }
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

    /// Living organisms, and how many have at least `min_cells` cells.
    pub fn org_counts(&self, min_cells: u16) -> (u32, u32) {
        let mut n = 0;
        let mut big = 0;
        for (_, o) in self.cells.iter_orgs() {
            n += 1;
            if o.n_alive >= min_cells {
                big += 1;
            }
        }
        (n, big)
    }

    /// Highest organism generation alive for a genotype.
    pub fn max_generation_of(&self, genotype: u32) -> u32 {
        let mut best = 0;
        for (_, o) in self.cells.iter_orgs() {
            if let Some(c) = self.cells.get(o.seed) {
                if c.genotype == genotype {
                    best = best.max(o.generation);
                }
            }
        }
        best
    }

    /// Live cells of `genotype` and all genotypes descended from it.
    pub fn lineage_cells_of(&self, genotype: u32) -> u32 {
        self.genomes.lineage_live_cells(genotype)
    }

    /// Live cells expressing `genotype`.
    pub fn live_cells_of(&self, genotype: u32) -> u32 {
        self.genomes
            .get(genotype)
            .map(|n| n.live_cells)
            .unwrap_or(0)
    }

    pub fn seed_genome_at(&mut self, px: u16, py: u16, genome: Genome) -> Option<u32> {
        let seed_mode = genome.seed_mode;
        let geno = self.genomes.intern_root(genome);
        let id = self.cells.spawn(
            &mut self.genomes,
            &mut self.pixels,
            px,
            py,
            geno,
            seed_mode,
            0,
            12 * crate::consts::FP,
            2,
            self.tick_count,
            self.seed,
        );
        self.light_dirty = true;
        id
    }

    pub fn genotype_of_cell(&self, id: u32) -> Option<u32> {
        self.cells.get(id).map(|c| c.genotype)
    }

    pub fn genomes_mut(&mut self) -> &mut GenomeStore {
        &mut self.genomes
    }

    /// Spawn a generation-0 seed cell of an interned genotype (archive).
    pub(crate) fn spawn_root(&mut self, geno: u32, seed_mode: u8, px: u16, py: u16) -> Option<u32> {
        let id = self.cells.spawn(
            &mut self.genomes,
            &mut self.pixels,
            px,
            py,
            geno,
            seed_mode,
            0,
            12 * crate::consts::FP,
            2,
            self.tick_count,
            self.seed,
        );
        self.light_dirty = true;
        id
    }

    pub(crate) fn note_archive_root(&mut self, genotype: u32, specimen: u64) {
        if !self.archive_roots.iter().any(|(g, _)| *g == genotype) {
            self.archive_roots.push((genotype, specimen));
        }
    }

    /// The archived specimen a genotype's lineage root was seeded from.
    pub fn archive_parent_of(&self, genotype: u32) -> Option<u64> {
        let root = self.genomes.root_of(genotype);
        self.archive_roots.iter().find(|(g, _)| *g == root).map(|(_, id)| *id)
    }

    /// Fill every cell of an organism to its store cap (portrait sandbox).
    pub(crate) fn feed_org_full(&mut self, org: u32) {
        let ids: Vec<u32> = match self.cells.org(org) {
            Some(o) => o.slots.iter().map(|s| s.cell).filter(|&c| c != bio::store::NONE).collect(),
            None => return,
        };
        for id in ids {
            let Some(c) = self.cells.get(id) else {
                continue;
            };
            let cap = self.genomes.coeffs(c.genotype, c.mode).store_cap_fp(c.area());
            if let Some(c) = self.cells.get_mut(id) {
                c.energy = cap.max(c.energy);
                c.stress = 0;
            }
        }
    }

    /// Kill one cell directly (harness "bite").
    pub fn kill_cell(&mut self, id: u32) -> bool {
        if self.cells.get(id).is_none() {
            return false;
        }
        let _ = self
            .cells
            .kill(&mut self.genomes, &mut self.pixels, id, false, self.tick_count, None);
        self.light_dirty = true;
        true
    }

    /// Clean tool (K2#5): sweep litter, grit and corpses from the physics
    /// cells within `r` of `(vx, vy)`. Returns pixels cleared.
    pub fn clean_at(&mut self, vx: i32, vy: i32, r: i32) -> u32 {
        let r = r.max(0);
        let vis_h = self.grid.visible_h();
        let pw = self.pixels.w as i32;
        let mut cleared = 0u32;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                let (x, y) = (vx + dx, vy + dy);
                if x < 0 || y < 0 || x >= self.grid.visible_w() || y >= vis_h {
                    continue;
                }
                let py_top = vis_h - 1 - y;
                for sy in 0..3 {
                    for sx in 0..3 {
                        let idx = ((py_top * 3 + sy) * pw + x * 3 + sx) as usize;
                        if self.pixels.litter[idx] > 0 || self.pixels.grit[idx] > 0 {
                            self.pixels.litter[idx] = 0;
                            self.pixels.litter_key[idx] = 0;
                            self.pixels.grit[idx] = 0;
                            cleared += 1;
                        }
                        let owner = self.pixels.owner[idx];
                        if owner >= OWNER_BIAS && self.cells.corpse(owner - OWNER_BIAS).is_some() {
                            self.cells.expire_corpse(&mut self.pixels, owner - OWNER_BIAS);
                            cleared += 1;
                        }
                    }
                }
            }
        }
        self.light_dirty = true;
        cleared
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
                let _ = self.cells.kill(
                    &mut self.genomes,
                    &mut self.pixels,
                    id,
                    false,
                    self.tick_count,
                    None,
                );
            }
        }
        self.light_dirty = true;
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
                        bio::fields::add_plain(
                            &mut self.pixels,
                            bio::fields::Field::Glucose,
                            idx,
                            amount,
                        );
                    }
                }
            }
        }
    }

    /// Drop soft litter (or bone with `code ≥ 192`) at a pixel: `units` of
    /// `GLUCOSE_UNIT_FP` (1/64 g). Feed hook for the harness and the player.
    pub fn drop_litter(&mut self, px: u16, py: u16, units: u8, code: u8) {
        if px as u32 >= self.pixels.w || py as u32 >= self.pixels.h {
            return;
        }
        let idx = (py as usize) * (self.pixels.w as usize) + px as usize;
        bio::fields::add_litter(&mut self.pixels, idx, units, code);
    }

    /// Fines settle to the floor of their column as soft litter (2.3.9):
    /// whole glucose units move; the fraction below a unit stays suspended.
    fn settle_fines(&mut self) {
        use crate::bio::chem::LITTER_SOFT_CODE;
        let pw = self.pixels.w as usize;
        let ph = self.pixels.h as usize;
        let vis_h = self.grid.visible_h();
        for px in 0..pw {
            let f = self.fines[px];
            if f <= 0.0 {
                continue;
            }
            let units = (f / crate::consts::FINES_SETTLE_DIV * 64.0).floor().min(255.0);
            if units < 1.0 {
                continue;
            }
            // Lowest open pixel of the column with room (water above the
            // bed); a thick fall piles up to `FINES_PILE_PX` deep.
            let mut floor = None;
            let mut depth = 0usize;
            let vx = (px / crate::consts::PX_PER_CELL as usize) as i32;
            for py in (0..ph).rev() {
                let vy = vis_h - 1 - (py / crate::consts::PX_PER_CELL as usize) as i32;
                if !self.grid.get_visible(vx, vy).is_open() {
                    continue;
                }
                if self.pixels.litter[py * pw + px] < 255 {
                    floor = Some(py);
                    break;
                }
                depth += 1;
                if depth >= crate::consts::FINES_PILE_PX {
                    break;
                }
            }
            let Some(py) = floor else {
                continue;
            };
            let idx = py * pw + px;
            let room = 255u32.saturating_sub(self.pixels.litter[idx] as u32).min(units as u32);
            if room == 0 {
                continue;
            }
            bio::fields::add_litter(&mut self.pixels, idx, room as u8, LITTER_SOFT_CODE);
            self.fines[px] = (f - room as f32 / 64.0).max(0.0);
        }
    }

    /// Harness: stir `grams` of fines into pixel column `px` (a bloom
    /// crash in one call).
    pub fn add_fines(&mut self, px: usize, grams: f32) {
        if let Some(f) = self.fines.get_mut(px) {
            *f += grams.max(0.0);
        }
    }

    /// Harness: kill every `n`-th living cell (deterministic, id order),
    /// leaving corpses — a bloom crash. Returns how many died.
    pub fn cull(&mut self, every: u32) -> u32 {
        let every = every.max(1);
        let ids = self.cells.live_ids();
        let mut n = 0;
        for (k, id) in ids.into_iter().enumerate() {
            if k as u32 % every != 0 {
                continue;
            }
            let Some(c) = self.cells.get(id).copied() else {
                continue;
            };
            let mc = self.genomes.coeffs(c.genotype, c.mode);
            let structure = mc.birth_fp as i64 * c.area() as i64 * (255 - c.damage as i64) / 255;
            let corpse = bio::corpse_split(mc, c.area(), structure as i32 + c.energy.max(0));
            let _ = self.cells.kill(
                &mut self.genomes,
                &mut self.pixels,
                id,
                false,
                self.tick_count,
                Some(corpse),
            );
            n += 1;
        }
        n
    }

    /// 3-tap blur across neighbouring columns (no stripes), conservative.
    fn blur_fines(&mut self) {
        let n = self.fines.len();
        if n < 3 || self.fines.iter().all(|&f| f <= 0.0) {
            return;
        }
        let src = self.fines.clone();
        for x in 0..n {
            let l = if x > 0 { src[x - 1] } else { src[x] };
            let r = if x + 1 < n { src[x + 1] } else { src[x] };
            self.fines[x] = (l + 2.0 * src[x] + r) * 0.25;
        }
    }

    /// Gyro swirl stirs the floor litter back up: `0.02 · |tilt| ·
    /// litter_g` per second per column (§4), litter → fines.
    fn resuspend_fines(&mut self) {
        let tilt = self.grav_x.abs().min(1.0);
        if tilt < 0.05 {
            return;
        }
        let pw = self.pixels.w as usize;
        let ph = self.pixels.h as usize;
        let per_sec = crate::consts::SWIRL_RESUSPEND * tilt;
        let frac =
            per_sec * crate::consts::FINES_SETTLE_EVERY as f32 / crate::consts::TICK_HZ as f32;
        let vis_h = self.grid.visible_h();
        for px in 0..pw {
            // Soft litter resting on this column's floor.
            let mut total = 0u32;
            let mut idxs: Vec<usize> = Vec::new();
            let vx = (px / crate::consts::PX_PER_CELL as usize) as i32;
            for py in 0..ph {
                let idx = py * pw + px;
                let vy = vis_h - 1 - (py / crate::consts::PX_PER_CELL as usize) as i32;
                let c = self.grid.get_visible(vx, vy);
                if c.is_open()
                    && c.sat > 40
                    && self.pixels.litter[idx] > 0
                    && self.pixels.litter_key[idx] < 192
                {
                    total += self.pixels.litter[idx] as u32;
                    idxs.push(idx);
                }
            }
            if total == 0 {
                continue;
            }
            let lift = ((total as f32 * frac).floor() as u32).min(total);
            if lift == 0 {
                continue;
            }
            let mut left = lift;
            for idx in idxs {
                if left == 0 {
                    break;
                }
                let t = (self.pixels.litter[idx] as u32).min(left);
                self.pixels.litter[idx] -= t as u8;
                left -= t;
            }
            self.fines[px] += (lift - left) as f32 / 64.0;
        }
    }

    /// "Water is cloudy" (§12): mean cloud under `CLOUDY_MEAN`, repeatable
    /// after a cooldown.
    fn check_cloudy(&mut self) {
        let mean = self.cloud_mean();
        if mean < crate::consts::CLOUDY_MEAN {
            let due = self.cloudy_last.is_none_or(|t| {
                self.tick_count.saturating_sub(t) >= crate::consts::CLOUDY_COOLDOWN
            });
            if due {
                self.cloudy_last = Some(self.tick_count);
                self.events.push_back(SimEvent {
                    kind: bio::ev::CLOUDY,
                    arg0: ((1.0 - mean) * 255.0) as u32,
                });
            }
        } else if mean > crate::consts::CLOUDY_MEAN + 0.15 {
            self.cloudy_last = None;
        }
    }

    /// Phase 3 scoop: lift everything inside the O of radius `r` pixels
    /// around pixel `(cx, cy)` — organisms whose seed cell lies inside
    /// travel whole (glue breaks, division bonds hold), plus the litter,
    /// grit and water inside the ring. Corpses stay. Nothing is lost:
    /// the bucket holds it until `release` or `discard`.
    pub fn scoop_region(&mut self, cx: i32, cy: i32, r: i32) -> Carried {
        let r = r.max(1);
        let pw = self.pixels.w as i32;
        let ph = self.pixels.h as i32;
        let inside = |x: i32, y: i32| (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r;
        let mut carried = Carried {
            cx,
            cy,
            r,
            orgs: Vec::new(),
            litter: Vec::new(),
            grit: Vec::new(),
            water: 0,
        };
        let org_ids: Vec<u32> = self
            .cells
            .iter_orgs()
            .filter(|(_, o)| {
                self.cells
                    .get(o.seed)
                    .is_some_and(|c| inside(c.px as i32 + c.w as i32 / 2, c.py as i32 + c.h as i32 / 2))
            })
            .map(|(id, _)| id)
            .collect();
        for org in org_ids {
            if let Some(b) = self.cells.lift_org(&mut self.pixels, org) {
                carried.orgs.push(b);
            }
        }
        let vis_h = self.grid.visible_h();
        for y in (cy - r).max(0)..=(cy + r).min(ph - 1) {
            for x in (cx - r).max(0)..=(cx + r).min(pw - 1) {
                if !inside(x, y) {
                    continue;
                }
                let idx = (y * pw + x) as usize;
                if self.pixels.litter[idx] > 0 {
                    carried.litter.push((
                        (x - cx) as i16,
                        (y - cy) as i16,
                        self.pixels.litter[idx],
                        self.pixels.litter_key[idx],
                    ));
                    self.pixels.litter[idx] = 0;
                    self.pixels.litter_key[idx] = 0;
                }
                if self.pixels.grit[idx] > 0 {
                    carried.grit.push(((x - cx) as i16, (y - cy) as i16, self.pixels.grit[idx]));
                    self.pixels.grit[idx] = 0;
                }
            }
        }
        // Water: every open physics cell whose centre pixel is inside.
        let ppc = crate::consts::PX_PER_CELL as i32;
        for vy_top in ((cy - r) / ppc).max(0)..=((cy + r) / ppc).min(vis_h - 1) {
            for vx in ((cx - r) / ppc).max(0)..=((cx + r) / ppc).min(self.grid.visible_w() - 1) {
                if !inside(vx * ppc + 1, vy_top * ppc + 1) {
                    continue;
                }
                let vy = vis_h - 1 - vy_top;
                let c = self.grid.get_visible(vx, vy);
                if c.is_open() && c.sat > 0 {
                    carried.water += c.sat as u32;
                    let mut dry = c;
                    dry.sat = 0;
                    self.grid.set_visible(vx, vy, dry);
                }
            }
        }
        bio::fields::rescan_tiles(&mut self.pixels);
        self.light_dirty = true;
        carried
    }

    /// Put a bucket down with its centre at pixel `(x, y)`. Every
    /// organism must fit on free open pixels or nothing moves
    /// (`Err(ReleaseError::NoRoom)`); litter, grit and water then land
    /// as near as they can to where they were in the ring.
    pub fn release(&mut self, carried: Carried, x: i32, y: i32) -> Result<(), (Carried, ReleaseError)> {
        let dx = x - carried.cx;
        let dy = y - carried.cy;
        let pw = self.pixels.w as i32;
        let ph = self.pixels.h as i32;
        if x < 0 || y < 0 || x >= pw || y >= ph {
            return Err((carried, ReleaseError::OutOfBounds));
        }
        let vis_h = self.grid.visible_h() as u32;
        let open = |px: i32, py: i32| -> bool {
            let (vx, vy) = bio::pixel_to_phys(px as u16, py as u16, vis_h);
            self.grid.get_visible(vx, vy).is_open()
        };
        for b in &carried.orgs {
            if !self.cells.bundle_fits(&self.pixels, b, dx, dy, &open) {
                return Err((carried, ReleaseError::NoRoom));
            }
        }
        // Two bodies in one bucket may not overlap each other either.
        for (i, a) in carried.orgs.iter().enumerate() {
            for b in carried.orgs.iter().skip(i + 1) {
                for (_, ca) in &a.cells {
                    for (_, cb) in &b.cells {
                        if ca.px < cb.px + cb.w as u16
                            && cb.px < ca.px + ca.w as u16
                            && ca.py < cb.py + cb.h as u16
                            && cb.py < ca.py + ca.h as u16
                        {
                            return Err((carried, ReleaseError::NoRoom));
                        }
                    }
                }
            }
        }
        for b in &carried.orgs {
            self.cells.place_bundle(&mut self.pixels, b, dx, dy);
        }
        for &(ox, oy, units, code) in &carried.litter {
            if let Some(idx) = self.landing_pixel(carried.cx + ox as i32 + dx, carried.cy + oy as i32 + dy) {
                bio::fields::add_litter(&mut self.pixels, idx, units, code);
            }
        }
        for &(ox, oy, units) in &carried.grit {
            if let Some(idx) = self.landing_pixel(carried.cx + ox as i32 + dx, carried.cy + oy as i32 + dy) {
                self.pixels.grit[idx] = self.pixels.grit[idx].saturating_add(units);
            }
        }
        self.pour_water_near(x, y, carried.water);
        bio::fields::rescan_tiles(&mut self.pixels);
        self.light_dirty = true;
        Ok(())
    }

    /// Throw the bucket away: the organisms die without a corpse and the
    /// matter leaves the tank (this is the "remove" reading of a scoop).
    pub fn discard(&mut self, carried: Carried) {
        for b in &carried.orgs {
            for (_, c) in &b.cells {
                self.genomes.on_death(c.genotype);
            }
        }
    }

    /// Remove tool (Phase 3, §2.4 #5): anything under the brush leaves
    /// the tank — organisms whose cells are hit (whole, no corpse, no
    /// fragments), corpses, litter and grit. Returns cells removed.
    pub fn remove_at(&mut self, vx: i32, vy: i32, r: i32) -> u32 {
        let r = r.max(0);
        let vis_h = self.grid.visible_h();
        let mut orgs: Vec<u32> = Vec::new();
        for (_, c) in self.cells.iter_alive() {
            let cx = c.px as i32 / 3;
            let cy = vis_h - 1 - c.py as i32 / 3;
            let (dx, dy) = (cx - vx, cy - vy);
            if dx * dx + dy * dy <= r * r && !orgs.contains(&c.org) {
                orgs.push(c.org);
            }
        }
        let mut removed = 0;
        for org in orgs {
            if let Some(b) = self.cells.lift_org(&mut self.pixels, org) {
                removed += b.cells.len() as u32;
                for (_, c) in &b.cells {
                    self.genomes.on_death(c.genotype);
                }
            }
        }
        let _ = self.clean_at(vx, vy, r);
        self.light_dirty = true;
        removed
    }

    /// The pixel where loose matter put down at `(x, y)` comes to rest:
    /// the pixel itself when open, else the lowest open pixel of that
    /// column, else the nearest open pixel anywhere.
    fn landing_pixel(&self, x: i32, y: i32) -> Option<usize> {
        let pw = self.pixels.w as i32;
        let ph = self.pixels.h as i32;
        let vis_h = self.grid.visible_h() as u32;
        let open = |px: i32, py: i32| -> bool {
            px >= 0 && py >= 0 && px < pw && py < ph && {
                let (vx, vy) = bio::pixel_to_phys(px as u16, py as u16, vis_h);
                self.grid.get_visible(vx, vy).is_open()
            }
        };
        let x = x.clamp(0, pw - 1);
        let y = y.clamp(0, ph - 1);
        if open(x, y) {
            return Some((y * pw + x) as usize);
        }
        if let Some(py) = (0..ph).rev().find(|&py| open(x, py)) {
            return Some((py * pw + x) as usize);
        }
        let mut best: Option<(i32, usize)> = None;
        for py in 0..ph {
            for px in 0..pw {
                if open(px, py) {
                    let d = (px - x) * (px - x) + (py - y) * (py - y);
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, (py * pw + px) as usize));
                    }
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// Pour `sat` units of water into the open physics cells nearest to
    /// pixel `(x, y)` (breadth-first), filling each to full.
    fn pour_water_near(&mut self, x: i32, y: i32, mut sat: u32) {
        if sat == 0 {
            return;
        }
        let vis_h = self.grid.visible_h();
        let vis_w = self.grid.visible_w();
        let (sx, sy) = bio::pixel_to_phys(
            x.clamp(0, self.pixels.w as i32 - 1) as u16,
            y.clamp(0, self.pixels.h as i32 - 1) as u16,
            vis_h as u32,
        );
        let mut seen = vec![false; (vis_w * vis_h) as usize];
        let mut queue = VecDeque::new();
        queue.push_back((sx, sy));
        seen[(sy * vis_w + sx) as usize] = true;
        while let Some((vx, vy)) = queue.pop_front() {
            let c = self.grid.get_visible(vx, vy);
            if c.is_open() && c.sat < 255 {
                let take = (255 - c.sat as u32).min(sat);
                let mut w = c;
                w.sat += take as u8;
                self.grid.set_visible(vx, vy, w);
                sat -= take;
                if sat == 0 {
                    return;
                }
            }
            for (nx, ny) in [(vx - 1, vy), (vx + 1, vy), (vx, vy - 1), (vx, vy + 1)] {
                if nx < 0 || ny < 0 || nx >= vis_w || ny >= vis_h {
                    continue;
                }
                let i = (ny * vis_w + nx) as usize;
                if seen[i] {
                    continue;
                }
                seen[i] = true;
                queue.push_back((nx, ny));
            }
        }
    }

    /// Total litter mass in units (harness probe).
    pub fn litter_total(&self) -> u64 {
        self.pixels.litter.iter().map(|&l| l as u64).sum()
    }

    /// Total dissolved glucose in units (harness probe).
    pub fn glucose_total(&self) -> u64 {
        self.pixels.glucose.iter().map(|&g| g as u64).sum()
    }

    /// Litter units at a pixel.
    pub fn litter_at(&self, px: u16, py: u16) -> u8 {
        let idx = (py as usize) * (self.pixels.w as usize) + px as usize;
        self.pixels.litter.get(idx).copied().unwrap_or(0)
    }

    /// Total inert grit in units (harness probe).
    pub fn grit_total(&self) -> u64 {
        self.pixels.grit.iter().map(|&g| g as u64).sum()
    }

    /// Total sediment nutrient in the beds, in units (harness probe).
    pub fn sediment_total(&self) -> u64 {
        self.coarse.sediment.iter().map(|&s| s as u64).sum()
    }

    /// Set the sediment nutrient of every sand / gravel / soil cell.
    pub fn set_sediment_all(&mut self, units: u8) {
        let (vw, vh) = (self.grid.visible_w(), self.grid.visible_h());
        for vy in 0..vh {
            for vx in 0..vw {
                let bed = self.grid.get_visible(vx, vy).id();
                if matches!(bed, MaterialId::Sand | MaterialId::Gravel | MaterialId::Soil) {
                    self.coarse.sediment[(vy as usize) * (vw as usize) + vx as usize] = units;
                }
            }
        }
    }

    /// Dead cells still lying on their pixels.
    pub fn corpse_count(&self) -> usize {
        self.cells.corpse_count()
    }

    /// Corpse record by cell id (tests / inspect).
    pub fn corpse(&self, id: u32) -> Option<&bio::CellRec> {
        self.cells.corpse(id)
    }

    /// Suspended fines per pixel column, in grams (Phase 4 consumes them).
    pub fn fines(&self) -> &[f32] {
        &self.fines
    }

    /// Calcium feed (K2#1): raise the dissolved-mineral field by `amount`
    /// field units (`bio::minerals::MINERAL_Q` per m) in the water within
    /// `r` physics cells of `(vx, vy)`.
    pub fn add_calcium(&mut self, vx: i32, vy: i32, r: i32, amount: u8) {
        let r = r.max(1);
        let w = self.grid.visible_w();
        let h = self.grid.visible_h();
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                let (x, y) = (vx + dx, vy + dy);
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let c = self.grid.get_visible(x, y);
                if !(c.is_open() && c.sat > 40) {
                    continue;
                }
                let i = (y as usize) * (w as usize) + x as usize;
                self.coarse.minerals[i] = self.coarse.minerals[i].saturating_add(amount);
            }
        }
    }

    /// Set the whole mineral field (harness / tests: `0` = mineral-free water).
    pub fn set_minerals_all(&mut self, units: u8) {
        self.coarse.minerals.fill(units);
    }

    /// Total dissolved minerals in field units.
    pub fn mineral_total(&self) -> u64 {
        bio::minerals::total(&self.coarse)
    }

    /// Mineral field units in the physics cell under a pixel.
    pub fn minerals_at_pixel(&self, px: u16, py: u16) -> u8 {
        self.coarse.minerals[bio::minerals::idx_of_pixel(&self.coarse, px, py)]
    }

    /// Dead secretion pixels: `(bone, shell)` counts.
    pub fn secretion_counts(&self) -> (u32, u32) {
        let mut bone = 0;
        let mut shell = 0;
        for &o in &self.pixels.owner {
            if o == BONE_OWNER {
                bone += 1;
            } else if o == SHELL_OWNER {
                shell += 1;
            }
        }
        (bone, shell)
    }

    /// Owner value at a pixel (`BONE_OWNER` / `SHELL_OWNER` for secretions).
    pub fn owner_at(&self, px: u16, py: u16) -> u32 {
        let idx = (py as usize) * (self.pixels.w as usize) + px as usize;
        self.pixels.owner.get(idx).copied().unwrap_or(0)
    }

    /// Test rig: a full-size 3×3 cell pinned to the pane (film) so it
    /// cannot drift; `hard` gives it a WALL h 0.9 (dam rule).
    #[cfg(test)]
    pub(crate) fn seed_fixed_plate_for_test(&mut self, px: u16, py: u16, hard: bool) -> u32 {
        use crate::bio::{Block, Entry, Mode};
        let mut g = Genome::base();
        let mut m = Mode::blank();
        m.shape_w = 3;
        m.shape_h = 3;
        m.push_entry(Entry::new(Block::Photo, 2));
        if hard {
            m.push_entry(Entry::with_params(Block::Wall, 2, 230, 0));
        }
        g.modes.push(m);
        let id = self.seed_genome_at(px, py, g).expect("seed plate");
        while self.cells.try_grow(&mut self.pixels, id, 3, 3) {}
        if let Some(c) = self.cells.get_mut(id) {
            c.film = true;
            c.energy = 24 * crate::consts::FP;
        }
        self.light_dirty = true;
        id
    }

    /// Test rig: pin a cell to the pane (film) so it cannot drift.
    #[cfg(test)]
    pub(crate) fn pin_cell_for_test(&mut self, id: u32) {
        if let Some(c) = self.cells.get_mut(id) {
            c.film = true;
        }
    }

    /// Test rig: set a living cell's store (FP).
    #[cfg(test)]
    pub(crate) fn set_energy_for_test(&mut self, id: u32, energy: i32) {
        if let Some(c) = self.cells.get_mut(id) {
            c.energy = energy;
        }
    }

    #[cfg(test)]
    pub(crate) fn place_for_test(&mut self, vx: i32, vy: i32, cell: Cell) {
        self.grid.set_visible(vx, vy, cell);
        self.light_dirty = true;
    }

    /// Place a physics cell (harness scenarios).
    pub fn place(&mut self, vx: i32, vy: i32, cell: Cell) {
        self.grid.set_visible(vx, vy, cell);
        self.light_dirty = true;
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
            self.light_dirty = true;
            self.rebuild_light();
            return;
        }
        self.apply_tools(&input.tools);
        self.light_dirty = true;
        self.rebuild_light();
        // Thermal cycle (§4): base + solar heat + stored heat that carries
        // into the night; the flashlight does not heat (K4#7).
        let day = if self.always_day {
            1.0
        } else {
            crate::clock::day_factor(self.tick_count)
        };
        let t_direct = thermal::t_direct(self.light_slider.clamp(0.0, 1.0) * day);
        if self.thermal_tau <= 0.0 {
            // A fresh tank has been sitting in its light all along: the
            // stored heat starts in equilibrium rather than at zero.
            self.stored_heat = t_direct;
        }
        if self.tick_count % crate::consts::THERMAL_MASS_EVERY == 0 || self.thermal_tau <= 0.0 {
            self.thermal_tau =
                thermal::ThermalMass::measure(&self.grid, self.cells.len()).tau_ticks();
        }
        self.stored_heat = thermal::relax_stored(self.stored_heat, t_direct, self.thermal_tau);
        let t_tank = thermal::t_tank(self.t_base, t_direct, self.stored_heat);
        self.t_tank = t_tank;
        let tilt = if self.swirl { self.grav_x } else { 0.0 };
        self.mark_tissue();
        rules::step(&mut self.grid, self.seed, self.tick_count, tilt);
        let bio = bio::step(
            &mut self.cells,
            &mut self.genomes,
            &mut self.grid,
            &mut self.pixels,
            &mut self.coarse,
            self.seed,
            self.tick_count,
            t_tank,
            self.lambda_scale_q8 as u32,
        );
        for ev in &bio.events {
            self.events.push_back(*ev);
        }
        let mut bio = bio;
        for sid in self.genomes.drain_new_species() {
            let e = SimEvent {
                kind: bio::ev::NEW_SPECIES,
                arg0: sid as u32,
            };
            self.events.push_back(e);
            bio.events.push(e);
        }
        for (acc, n) in self.death_causes.iter_mut().zip(bio.death_causes) {
            *acc += n as u64;
        }
        thermal::step_local(&mut self.coarse, self.tick_count);
        // 2.3.7: crumbling corpses send 30 % of their soft mass up as fines.
        for (px, units) in bio.fines {
            if let Some(f) = self.fines.get_mut(px as usize) {
                *f += units as f32 * crate::consts::GLUCOSE_UNIT_FP as f32 / crate::consts::FP as f32;
            }
        }
        for (px, units) in self.pixels.lysis_fines.drain(..) {
            if let Some(f) = self.fines.get_mut(px as usize) {
                *f +=
                    units as f32 * crate::consts::GLUCOSE_UNIT_FP as f32 / crate::consts::FP as f32;
            }
        }
        if self.tick_count % crate::consts::FINES_SETTLE_EVERY == 0 {
            if self.swirl {
                self.resuspend_fines();
            }
            self.settle_fines();
        }
        if self.tick_count % crate::consts::FINES_BLUR_EVERY == 3 {
            self.blur_fines();
        }
        if self.tick_count % milestones::GLASS_EVERY == 150 {
            self.check_cloudy();
        }
        let probe = milestones::Probe {
            film_n: if self.tick_count % milestones::GLASS_EVERY == 0 {
                self.pixels.film.iter().filter(|&&f| f > 80).count()
            } else {
                0
            },
            film_len: self.pixels.len(),
        };
        self.milestones.run(
            self.tick_count,
            &bio.events,
            &self.cells,
            &self.genomes,
            &probe,
            &mut self.events,
        );
        self.light_dirty = true;
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

    /// Whether a once-per-tank milestone (`ev::MS_*`) has been announced.
    pub fn milestone_fired(&self, kind: u32) -> bool {
        self.milestones.has_fired(kind)
    }

    pub fn poll_event(&mut self) -> Option<SimEvent> {
        self.events.pop_front()
    }

    /// Pixel coordinate, origin top-left of the RGBA buffer.
    pub fn inspect(&self, px: u32, py: u32) -> Inspect {
        if px >= self.pixels.w || py >= self.pixels.h {
            return Inspect {
                cell: u32::MAX,
                species: bio::NO_SPECIES,
                ..Inspect::default()
            };
        }
        let vx = px / crate::consts::PX_PER_CELL;
        let vy_top = py / crate::consts::PX_PER_CELL;
        let vy = self.grid.visible_h() as u32 - 1 - vy_top;
        let cell = self.grid.get_visible(vx as i32, vy as i32);
        let owner = self.pixels.owner[(py as usize) * (self.pixels.w as usize) + px as usize];
        let mut rec = Inspect {
            species: bio::NO_SPECIES,
            hit: true,
            material: cell.material,
            sat: cell.sat,
            moisture: cell.moisture(),
            variant: cell.colour_variant(),
            owner,
            vx,
            vy,
            cell: u32::MAX,
            ..Inspect::default()
        };
        if owner >= OWNER_BIAS {
            let id = owner - OWNER_BIAS;
            if let Some(c) = self.cells.get(id) {
                let mc = self.genomes.coeffs(c.genotype, c.mode);
                let g = self.genomes.genome(c.genotype);
                let body = self.cells.org_body_cost_fp(c.org).max(mc.build_fp);
                rec.cell = id;
                rec.org = c.org;
                rec.genotype = c.genotype;
                rec.mode = c.mode;
                rec.generation = self.cells.org_generation(c.org);
                rec.org_cells = self.cells.org_slot_count(c.org) as u32;
                rec.glued_kin = self.cells.glued_kin(c.org);
                rec.age_ticks = c.age as u32;
                rec.max_age_ticks = bio::max_age_ticks(g.base_age, body);
                let cap = mc.store_cap_fp(c.area()).max(1);
                rec.energy_frac = (c.energy.max(0) as i64 * 255 / cap as i64).min(255) as u8;
                rec.stress = c.stress;
                rec.species = self.genomes.species_of(c.genotype);
                rec.children = self.cells.org(c.org).map(|o| o.children).unwrap_or(0);
            }
        }
        rec
    }

    /// The lineage card of a living cell's organism.
    pub fn lineage_card(&self, cell: u32) -> Option<LineageCard> {
        let c = self.cells.get(cell)?;
        let species = self.genomes.species_of(c.genotype);
        let rec = self.genomes.species(species);
        let parent_species_name = rec
            .and_then(|r| self.genomes.species(r.parent))
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let mut archived = 0;
        let mut cur = species;
        let mut hops = 0;
        while let Some(r) = self.genomes.species(cur) {
            if r.archived {
                archived += 1;
            }
            if r.parent == bio::NO_SPECIES || hops > self.genomes.species_count() {
                break;
            }
            cur = r.parent;
            hops += 1;
        }
        let own = self.cells.org(c.org).map(|o| o.n_alive as u32).unwrap_or(1);
        Some(LineageCard {
            cell,
            org: c.org,
            genotype: c.genotype,
            species,
            species_name: rec.map(|r| r.name.clone()).unwrap_or_default(),
            parent_species_name,
            generation: self.cells.org_generation(c.org),
            mutations_since_seed: self.genomes.get(c.genotype).map(|n| n.depth).unwrap_or(0),
            species_depth: rec.map(|r| r.depth).unwrap_or(0),
            kin_alive: self.genomes.species_live_cells(species).saturating_sub(own),
            descendants_alive: self.genomes.lineage_live_cells(c.genotype),
            archived_relatives: archived,
            children: self.cells.org(c.org).map(|o| o.children).unwrap_or(0),
        })
    }

    /// Name of a species (`""` when unknown).
    pub fn species_name(&self, species: u16) -> &str {
        self.genomes.species(species).map(|s| s.name.as_str()).unwrap_or("")
    }

    pub fn species_count(&self) -> usize {
        self.genomes.species_count()
    }

    pub fn species_parent(&self, species: u16) -> Option<u16> {
        self.genomes.species(species).map(|s| s.parent).filter(|&p| p != bio::NO_SPECIES)
    }

    /// Pixel bounding box `(x0, y0, x1, y1)` (inclusive) of a living
    /// organism's cells and secretions — the inspect tracking rectangle.
    pub fn org_bbox(&self, org: u32) -> Option<(u16, u16, u16, u16)> {
        let o = self.cells.org(org)?;
        let mut bb: Option<(u16, u16, u16, u16)> = None;
        let mut add = |x0: u16, y0: u16, x1: u16, y1: u16| {
            bb = Some(match bb {
                None => (x0, y0, x1, y1),
                Some(b) => (b.0.min(x0), b.1.min(y0), b.2.max(x1), b.3.max(y1)),
            });
        };
        for s in o.slots.iter().filter(|s| s.cell != bio::store::NONE) {
            if let Some(c) = self.cells.get(s.cell) {
                add(c.px, c.py, c.px + c.w as u16 - 1, c.py + c.h as u16 - 1);
            }
        }
        for sec in &o.secretions {
            add(sec.px, sec.py, sec.px, sec.py);
        }
        bb
    }

    /// `cloud[x] = exp(−k·fines[x])`, Q8 per pixel column (§4 cloudiness).
    fn cloud_q8(&self) -> Vec<u8> {
        self.fines
            .iter()
            .map(|&f| ((-crate::consts::CLOUD_K * f).exp() * 255.0).clamp(0.0, 255.0) as u8)
            .collect()
    }

    /// Mean `cloud[x]` over the water columns (1.0 = clear).
    pub fn cloud_mean(&self) -> f32 {
        if self.fines.is_empty() {
            return 1.0;
        }
        let sum: f32 = self
            .fines
            .iter()
            .map(|&f| (-crate::consts::CLOUD_K * f).exp())
            .sum();
        sum / self.fines.len() as f32
    }

    pub fn write_rgba(&mut self, out: &mut Vec<u8>) {
        self.rebuild_light();
        let cloud = self.cloud_q8();
        let cells = &self.cells;
        let genomes = &self.genomes;
        let tick = self.tick_count;
        pixels::write_rgba(
            &self.grid,
            &self.pixels,
            out,
            &|owner| {
                if owner < OWNER_BIAS {
                    return None;
                }
                // Dead secretions (§a.4 S1/S2): BONE ivory, SHELL gray-blue.
                match owner {
                    BONE_OWNER => return Some([0xEF, 0xE7, 0xDA]),
                    SHELL_OWNER => return Some([0x9F, 0xB3, 0xC8]),
                    _ => {}
                }
                let id = owner - OWNER_BIAS;
                let Some(cell) = cells.get(id) else {
                    // A corpse fades from its living colour to gray-brown.
                    let c = cells.corpse(id)?;
                    let live = genomes.coeffs(c.genotype, c.mode).colour;
                    let t = 255 - (c.corpse as u16 * 255 / bio::store::CORPSE_TICKS as u16);
                    let dead = [0x6Bu8, 0x55, 0x45];
                    let mut rgb = [0u8; 3];
                    for k in 0..3 {
                        rgb[k] = ((live[k] as u16 * (255 - t) + dead[k] as u16 * t) / 255) as u8;
                    }
                    return Some(rgb);
                };
                let mc = genomes.coeffs(cell.genotype, cell.mode);
                let mut rgb = mc.colour;
                if cell.flash > 0 {
                    return Some([255, 255, 255]);
                }
                // Brightness tracks light received (shaded cells go dull).
                let cap = mc.store_cap_fp(cell.area()).max(1);
                let e = (cell.energy.max(0) as i64 * 24 / cap as i64) as u8;
                rgb[1] = (rgb[1] as u16 + e as u16).min(255) as u8;
                // Desaturate under stress (pull toward gray).
                if cell.stress > 32 {
                    let s = cell.stress as u16;
                    let gray = (rgb[0] as u16 + rgb[1] as u16 + rgb[2] as u16) / 3;
                    for c in &mut rgb {
                        *c = ((*c as u16 * (255 - s) + gray * s) / 255) as u8;
                    }
                }
                // Damage: dark stipple (every other pixel of the footprint).
                if cell.damage > 64 && (cell.px as u64 + cell.py as u64 + tick / 8) % 2 == 0 {
                    for c in &mut rgb {
                        *c /= 2;
                    }
                }
                // THERMO running: ≤ 15 % toward coral (§a.7 warm tint).
                if cell.warm > 0 {
                    let coral = [0xE3u16, 0x6A, 0x50];
                    for (c, w) in rgb.iter_mut().zip(coral) {
                        *c = ((*c as u16 * 217 + w * 38) / 255) as u8;
                    }
                }
                Some(rgb)
            },
            &pixels::Overlay {
                cloud_q8: &cloud,
                t_local: &self.coarse.t_local,
                coarse_w: self.coarse.w,
            },
        );
        // Visible correlates drawn over the footprint colours (§a.5):
        // CILIA fringe (1–2 px trailing the heading, pale pink, never
        // blended; while beating it alternates two frames at ≈ 4 Hz) and
        // the SENSOR(light) pupil (one dark pixel on the heading side).
        let pw = self.pixels.w as i32;
        let ph = self.pixels.h as i32;
        let put = |out: &mut [u8], i: usize, rgb: [u8; 3], light: u8| {
            let o = i * 4;
            for k in 0..3 {
                out[o + k] = (rgb[k] as u32 * light as u32 / 255) as u8;
            }
        };
        for (_, c) in cells.iter_alive() {
            let mc = genomes.coeffs(c.genotype, c.mode);
            if mc.cilia > 0 {
                let (bx, by) = bio::program::heading_delta(c.heading.wrapping_add(4) % 8);
                let beat = c.running && (tick / 4) % 2 == 1;
                let n = if bx != 0 { c.h } else { c.w } as i32;
                for k in 0..n {
                    // Every other pixel trails one further on the beat frame.
                    let depth = if beat && k % 2 == 1 { 2 } else { 1 };
                    let (x, y) = if bx != 0 {
                        (c.px as i32 + bio::step::side_offset(bx, c.w) + bx * (depth - 1), c.py as i32 + k)
                    } else {
                        (c.px as i32 + k, c.py as i32 + bio::step::side_offset(by, c.h) + by * (depth - 1))
                    };
                    if x < 0 || y < 0 || x >= pw || y >= ph {
                        continue;
                    }
                    let i = (y * pw + x) as usize;
                    if self.pixels.owner[i] != 0 {
                        continue;
                    }
                    let (vx, vy) = bio::pixel_to_phys(x as u16, y as u16, self.grid.visible_h() as u32);
                    if !self.grid.get_visible(vx, vy).is_open() {
                        continue;
                    }
                    put(out, i, [0xF0, 0xA6, 0xC0], self.pixels.light[i]);
                }
            }
            if mc.sensor > 0 && mc.sensor_kind == bio::profile::SENSOR_LIGHT {
                let (dx, dy) = bio::program::heading_delta(c.heading);
                let (x, y) = if dx != 0 {
                    (c.px as i32 + if dx > 0 { c.w as i32 - 1 } else { 0 }, c.py as i32 + c.h as i32 / 2)
                } else {
                    (c.px as i32 + c.w as i32 / 2, c.py as i32 + if dy > 0 { c.h as i32 - 1 } else { 0 })
                };
                if x >= 0 && y >= 0 && x < pw && y < ph {
                    let i = (y * pw + x) as usize;
                    put(out, i, [0x10, 0x14, 0x20], self.pixels.light[i]);
                }
            }
        }
    }

    pub fn pixel_hash(&mut self) -> u64 {
        let mut buf = Vec::new();
        self.write_rgba(&mut buf);
        hash_bytes(&buf)
    }

    pub fn save_bytes(&mut self) -> Result<Vec<u8>, TankError> {
        self.light_dirty = true;
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
            lambda_scale_q8: self.lambda_scale_q8,
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
            litter: self.pixels.litter.clone(),
            litter_key: self.pixels.litter_key.clone(),
            grit: self.pixels.grit.clone(),
            film: self.pixels.film.clone(),
            fines: self.fines.clone(),
            coarse_w: self.coarse.w,
            coarse_h: self.coarse.h,
            minerals: self.coarse.minerals.clone(),
            sediment: self.coarse.sediment.clone(),
            digested: self.coarse.digested.clone(),
            t_local: self.coarse.t_local.clone(),
            humidity: self.coarse.humidity.clone(),
            genomes: self.genomes.export(),
            bio_cells: self.cells.raw_cells().to_vec(),
            orgs: self.cells.raw_orgs().to_vec(),
            milestones: self.milestones.clone(),
            species: self.genomes.species_table().to_vec(),
            node_species: self.genomes.export_species(),
            archive_roots: self.archive_roots.clone(),
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
            || blob.litter.len() != n
            || blob.litter_key.len() != n
            || blob.grit.len() != n
            || blob.film.len() != n
            || blob.fines.len() != tank.pixels.w as usize
            || blob.coarse_w != tank.coarse.w
            || blob.coarse_h != tank.coarse.h
            || blob.minerals.len() != tank.coarse.minerals.len()
            || blob.sediment.len() != tank.coarse.sediment.len()
            || blob.digested.len() != tank.coarse.digested.len()
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
        tank.lambda_scale_q8 = blob.lambda_scale_q8;
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
        tank.pixels.litter = blob.litter;
        tank.pixels.litter_key = blob.litter_key;
        tank.pixels.grit = blob.grit;
        tank.pixels.film = blob.film;
        bio::fields::rescan_tiles(&mut tank.pixels);
        tank.fines = blob.fines;
        tank.coarse.minerals = blob.minerals;
        tank.coarse.sediment = blob.sediment;
        tank.coarse.digested = blob.digested;
        tank.coarse.t_local = blob.t_local;
        tank.coarse.humidity = blob.humidity;
        // Life: genotypes first (cells index them), then live counts.
        let mut genomes = GenomeStore::import(&blob.genomes);
        genomes.import_species(blob.species, &blob.node_species);
        genomes.set_tick(blob.tick_count);
        for c in blob.bio_cells.iter().filter(|c| c.alive) {
            if genomes.get(c.genotype).is_none() {
                return Err(TankError::Shape);
            }
            genomes.on_birth(c.genotype);
        }
        tank.genomes = genomes;
        tank.cells = CellStore::from_raw(blob.bio_cells, blob.orgs);
        tank.milestones = blob.milestones;
        tank.archive_roots = blob.archive_roots;
        let (vw, vh) = (tank.grid.visible_w(), tank.grid.visible_h());
        for vy in 0..vh {
            for vx in 0..vw {
                if tank.grid.get_visible(vx, vy).is_tissue() {
                    tank.tissue_marks.push((vx, vy));
                }
            }
        }
        tank.light_dirty = true;
        tank.rebuild_light();
        // A loaded tank keeps its saved heat: τ known ⇒ no re-equilibration.
        tank.thermal_tau = thermal::ThermalMass::measure(&tank.grid, tank.cells.len()).tau_ticks();
        Ok(tank)
    }

    pub fn telemetry_mut(&mut self) -> &mut Telemetry {
        &mut self.telemetry
    }

    pub fn format_telemetry(&self, events: &[TelEvent], anon: &str) -> String {
        telemetry::format_batch(events, anon)
    }

    /// Solid-to-flow rule (§b.3): a physics cell whose 3×3 pixels hold
    /// ≥ 5 pixels of hard-walled living tissue is a solid for water and
    /// grains this tick. Soft tissue dams nothing.
    fn mark_tissue(&mut self) {
        for &(vx, vy) in &self.tissue_marks {
            let mut c = self.grid.get_visible(vx, vy);
            c.flags &= !Cell::TISSUE;
            self.grid.set_visible(vx, vy, c);
        }
        self.tissue_marks.clear();
        let vis_h = self.grid.visible_h() as u32;
        // (vx, vy) → hard pixel count; cells are visited in id order so the
        // result is deterministic.
        let mut counts: Vec<((i32, i32), u8)> = Vec::new();
        for (_, c) in self.cells.iter_alive() {
            if !self.genomes.coeffs(c.genotype, c.mode).is_hard {
                continue;
            }
            for dy in 0..c.h as i32 {
                for dx in 0..c.w as i32 {
                    let px = c.px as i32 + dx;
                    let py = c.py as i32 + dy;
                    let (vx, vy) = bio::pixel_to_phys(px as u16, py as u16, vis_h);
                    match counts.iter_mut().find(|(k, _)| *k == (vx, vy)) {
                        Some((_, n)) => *n += 1,
                        None => counts.push(((vx, vy), 1)),
                    }
                }
            }
        }
        for (_, o) in self.cells.iter_orgs() {
            for sec in &o.secretions {
                let (vx, vy) = bio::pixel_to_phys(sec.px, sec.py, vis_h);
                match counts.iter_mut().find(|(k, _)| *k == (vx, vy)) {
                    Some((_, n)) => *n += 1,
                    None => counts.push(((vx, vy), 1)),
                }
            }
        }
        for ((vx, vy), n) in counts {
            if n < TISSUE_DAM_PIXELS {
                continue;
            }
            let mut c = self.grid.get_visible(vx, vy);
            if !c.is_open() {
                continue;
            }
            c.flags |= Cell::TISSUE;
            self.grid.set_visible(vx, vy, c);
            self.tissue_marks.push((vx, vy));
        }
    }

    fn rebuild_light(&mut self) {
        if !self.light_dirty {
            return;
        }
        self.light_dirty = false;
        let flash = if self.flash_r > 0 {
            Some(pixels::Flashlight {
                vx: self.flash_x,
                radius: self.flash_r,
                boost: 512, // ×2, does not heat (K4#7)
            })
        } else {
            None
        };
        let cloud = self.cloud_q8();
        let cells = &self.cells;
        let genomes = &self.genomes;
        pixels::rebuild_light(
            &self.grid,
            &mut self.pixels,
            self.tick_count,
            self.always_day,
            self.light_slider,
            flash,
            &cloud,
            &|owner| {
                if owner < OWNER_BIAS {
                    return 255;
                }
                if secretion_kind(owner).is_some() {
                    return crate::consts::TISSUE_PASS_OTHER_Q8 as u8 / 2;
                }
                match cells.get(owner - OWNER_BIAS) {
                    Some(c) => genomes.coeffs(c.genotype, c.mode).light_pass_q8,
                    None if cells.corpse(owner - OWNER_BIAS).is_some() => CORPSE_PASS_Q8,
                    None => 255,
                }
            },
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
                // Remove anything under the brush (Phase 3, §2.4 #5).
                let _ = self.remove_at(tool.x as i32, tool.y as i32, tool.r as i32);
                continue;
            }
            if tool.kind == 10 {
                // Glucose drip into the dissolved field.
                self.drip_glucose(tool.x as i32, tool.y as i32, tool.r as i32, 180);
                continue;
            }
            if tool.kind == 11 {
                // Calcium feed: raise the local mineral field (K2#1).
                self.add_calcium(tool.x as i32, tool.y as i32, tool.r as i32, 96);
                continue;
            }
            if tool.kind == 12 {
                // Detritus feed: a pinch of soft litter at the brush centre.
                let vis_h = self.grid.visible_h() as u32;
                let py_top = vis_h.saturating_sub(1).saturating_sub(tool.y as u32);
                for sx in 0..3u16 {
                    self.drop_litter(
                        (tool.x as u32 * 3) as u16 + sx,
                        (py_top * 3 + 1) as u16,
                        128,
                        bio::chem::LITTER_SOFT_CODE,
                    );
                }
                continue;
            }
            if tool.kind == 13 {
                // Clean: sweep litter, grit and corpses under the brush.
                let _ = self.clean_at(tool.x as i32, tool.y as i32, tool.r as i32);
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
    #[serde(default = "default_lambda_scale")]
    lambda_scale_q8: u16,
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
    litter: Vec<u8>,
    litter_key: Vec<u8>,
    grit: Vec<u8>,
    film: Vec<u8>,
    fines: Vec<f32>,
    coarse_w: u32,
    coarse_h: u32,
    minerals: Vec<u8>,
    sediment: Vec<u8>,
    digested: Vec<u8>,
    t_local: Vec<i8>,
    humidity: Vec<u8>,
    #[serde(default)]
    genomes: Vec<(u32, u32, u16, Genome)>,
    #[serde(default)]
    bio_cells: Vec<bio::CellRec>,
    #[serde(default)]
    orgs: Vec<bio::OrgRec>,
    #[serde(default)]
    milestones: Milestones,
    #[serde(default)]
    species: Vec<bio::SpeciesRec>,
    #[serde(default)]
    node_species: Vec<(u32, u16)>,
    archive_roots: Vec<(u32, u64)>,
}

fn default_t_base() -> f32 {
    crate::consts::T_BASE_DEFAULT
}

fn default_slider() -> f32 {
    1.0
}

fn default_lambda_scale() -> u16 {
    256
}

fn default_grav_y() -> f32 {
    -1.0
}
