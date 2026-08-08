//! Simulation event + sample log for headless soaks and bug hunts.
//!
//! Records discrete events (birth, death, tip, geotech, …) and periodic
//! world samples (creatures, water, mycelium, carbon). Emit at orchestration
//! boundaries — never inside hot CA cell loops.
//!
//! Enable writing from soaks with `GVSE_SIM_LOG=<path>` or call
//! [`SimLog::write_ndjson`] / [`SimLog::print_ndjson`] directly.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use wk_material::MaterialId;

use crate::audit::sat_totals;
use crate::carbon::CarbonBudget;
use crate::cell::hosts_mycelium;
use crate::failure::FailureStats;
use crate::fungi::is_fungus;
use crate::grid::World;
use crate::organism::{ModuleId, OrganismStepStats, OrganismStore};
use crate::plant::{
    is_land_plant, leaves_bathing, plant_moisture_frac, stem_count, DROUGHT_DORMANT_FRAC,
    DROUGHT_STRESS_FRAC,
};
use crate::symbiosis::body_has_symbiont;

/// Default ring capacity (~a few minutes of dense events at 60 Hz).
pub const SIM_LOG_DEFAULT_CAP: usize = 50_000;
/// Default sample period for soaks (every 60 ticks ≈ 1 s of sim).
pub const SIM_LOG_DEFAULT_SAMPLE_PERIOD: u64 = 60;

/// One discrete simulation event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SimEventKind {
    Birth {
        habit: String,
        count: u32,
    },
    Death {
        habit: String,
        count: u32,
    },
    Tip {
        count: u32,
    },
    Spore {
        count: u32,
    },
    EmergentFruiting {
        count: u32,
    },
    SporeBankWake {
        count: u32,
    },
    Geotech {
        roof: u32,
        shear: u32,
        compaction: u32,
    },
    /// Free-form note for harness / scenario markers.
    Note {
        text: String,
    },
}

/// Timestamped event line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimEvent {
    pub tick: u64,
    #[serde(flatten)]
    pub kind: SimEventKind,
}

/// Periodic world sample (cheap aggregates).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimSample {
    pub tick: u64,
    pub plants: u32,
    pub fungi: u32,
    pub atoms: u32,
    pub corpses: u32,
    pub fallen_plants: u32,
    pub spores_bank: u32,
    pub sat_free: i64,
    pub sat_pore: i64,
    pub sat_total: i64,
    pub cream_cells: u32,
    pub cream_sum: u64,
    pub sugar_sum: u64,
    pub strain_cells: u32,
    pub carbon_atm: f32,
    pub carbon_dissolved: f32,
    pub mean_temp: Option<f32>,

    // --- Mycelium water support (plant↔cream Symbiont trade) ---
    /// Pore-sat units plants received from cream this tick (sum of lasts).
    pub sym_water_recv_tick: u32,
    /// Pore-sat units plants sent to cream this tick.
    pub sym_water_sent_tick: u32,
    /// Network-sugar units plants paid cream this tick.
    pub sym_sugar_paid_tick: u32,
    /// Network-sugar units plants received this tick.
    pub sym_sugar_recv_tick: u32,
    /// Living plants with any sym flow this tick.
    pub plants_sym_linked: u32,
    /// Living plants currently drought-stressed / dormant.
    pub plants_drought: u32,
    /// Drought plants that still received cream water this tick — desert support.
    pub plants_dry_sym_recv: u32,
    /// Plants whose body paints Symbiont.
    pub plants_with_symbiont: u32,
    /// Mean root/leaf moisture fraction across living plants (0..1).
    pub mean_root_moist: f32,
    /// Mean Organic stack depth under plant crowns (cream-buildup signal).
    pub mean_organic_depth: f32,
    /// Max Organic stack depth under any living plant crown.
    pub max_organic_depth: u32,

    // --- Plant evolution (means over living Root-bearing plants) ---
    pub stemless_plants: u32,
    pub mean_body_modules: f32,
    pub mean_roots: f32,
    pub mean_stems: f32,
    pub mean_photos: f32,
    pub mean_alloc_stem: f32,
    pub mean_alloc_leaf: f32,
    pub mean_alloc_root: f32,
    pub mean_root_depth_bias: f32,
    pub mean_clone_fidelity: f32,
    pub mean_leaf_absorb: f32,
    pub mean_shade_efficiency: f32,
    pub mean_sym_water: f32,
    pub mean_sym_energy: f32,

    // --- Habit cohorts (woody vs submerged seaweed vs stranded seaweed) ---
    /// Living plants with at least one Stem (true land/woody habit).
    pub woody_plants: u32,
    /// Stemless plants with leaves bathing in standing water (aquatic ribbons).
    pub stemless_wet: u32,
    /// Stemless plants not bathing — usually stranded seaweed on drying land,
    /// often sprouting long roots through dry periods.
    pub stemless_dry: u32,
    pub mean_roots_woody: f32,
    pub mean_roots_stemless_wet: f32,
    pub mean_roots_stemless_dry: f32,
    pub mean_moist_woody: f32,
    pub mean_moist_stemless_wet: f32,
    pub mean_moist_stemless_dry: f32,
    pub drought_woody: u32,
    pub drought_stemless_dry: u32,
    /// Mean root-depth bias of stranded stemless plants (dive signal).
    pub mean_depth_bias_stemless_dry: f32,
    pub fallen_woody: u32,
    pub fallen_stemless: u32,
    pub mean_org_depth_woody: f32,
}

/// In-memory ring of events + samples.
#[derive(Debug, Clone)]
pub struct SimLog {
    pub events: Vec<SimEvent>,
    pub samples: Vec<SimSample>,
    cap: usize,
    sample_period: u64,
    path: Option<PathBuf>,
}

impl Default for SimLog {
    fn default() -> Self {
        Self::new(SIM_LOG_DEFAULT_CAP, SIM_LOG_DEFAULT_SAMPLE_PERIOD)
    }
}

impl SimLog {
    pub fn new(cap: usize, sample_period: u64) -> Self {
        Self {
            events: Vec::new(),
            samples: Vec::new(),
            cap: cap.max(1),
            sample_period: sample_period.max(1),
            path: None,
        }
    }

    /// Read `GVSE_SIM_LOG` — empty/unset → memory only; otherwise NDJSON path.
    pub fn from_env() -> Self {
        let mut log = Self::default();
        if let Ok(p) = std::env::var("GVSE_SIM_LOG") {
            let p = p.trim();
            if !p.is_empty() && p != "0" && p != "false" {
                log.path = Some(PathBuf::from(p));
            }
        }
        if let Ok(p) = std::env::var("GVSE_SIM_LOG_PERIOD") {
            if let Ok(n) = p.parse::<u64>() {
                log.sample_period = n.max(1);
            }
        }
        log
    }

    pub fn sample_period(&self) -> u64 {
        self.sample_period
    }

    pub fn set_path(&mut self, path: impl Into<PathBuf>) {
        self.path = Some(path.into());
    }

    pub fn record(&mut self, tick: u64, kind: SimEventKind) {
        self.events.push(SimEvent { tick, kind });
        if self.events.len() > self.cap {
            let drop = self.events.len() - self.cap;
            self.events.drain(0..drop);
        }
    }

    pub fn note(&mut self, tick: u64, text: impl Into<String>) {
        self.record(tick, SimEventKind::Note { text: text.into() });
    }

    /// Record organism step deltas (skips zero counters).
    pub fn record_organism(&mut self, tick: u64, stats: &OrganismStepStats) {
        for (habit, count) in [
            ("plant", stats.births_plant),
            ("fungus", stats.births_fungus),
            ("atom", stats.births_atom),
        ] {
            if count > 0 {
                self.record(
                    tick,
                    SimEventKind::Birth {
                        habit: habit.into(),
                        count,
                    },
                );
            }
        }
        for (habit, count) in [
            ("plant", stats.deaths_plant),
            ("fungus", stats.deaths_fungus),
            ("atom", stats.deaths_atom),
        ] {
            if count > 0 {
                self.record(
                    tick,
                    SimEventKind::Death {
                        habit: habit.into(),
                        count,
                    },
                );
            }
        }
        if stats.tips > 0 {
            self.record(tick, SimEventKind::Tip { count: stats.tips });
        }
        if stats.spores > 0 {
            self.record(tick, SimEventKind::Spore { count: stats.spores });
        }
        if stats.emergent_fruiting > 0 {
            self.record(
                tick,
                SimEventKind::EmergentFruiting {
                    count: stats.emergent_fruiting,
                },
            );
        }
        if stats.spore_bank_wakes > 0 {
            self.record(
                tick,
                SimEventKind::SporeBankWake {
                    count: stats.spore_bank_wakes,
                },
            );
        }
    }

    pub fn record_geotech(&mut self, tick: u64, stats: FailureStats) {
        if stats.total() == 0 {
            return;
        }
        self.record(
            tick,
            SimEventKind::Geotech {
                roof: stats.roof,
                shear: stats.shear,
                compaction: stats.compaction,
            },
        );
    }

    pub fn maybe_sample(
        &mut self,
        tick: u64,
        world: &World,
        organisms: &OrganismStore,
        carbon: Option<&CarbonBudget>,
        mean_temp: Option<f32>,
    ) {
        if tick % self.sample_period != 0 {
            return;
        }
        self.push_sample(tick, world, organisms, carbon, mean_temp);
    }

    pub fn push_sample(
        &mut self,
        tick: u64,
        world: &World,
        organisms: &OrganismStore,
        carbon: Option<&CarbonBudget>,
        mean_temp: Option<f32>,
    ) {
        let (p, f, a) = organisms.habit_counts();
        let fallen_plants = organisms
            .atoms
            .iter()
            .filter(|atom| is_land_plant(atom) && atom.fallen)
            .count() as u32;
        let sat = sat_totals(world);
        let myc = mycelium_totals(world);
        let life = plant_life_totals(world, organisms);
        let (atm, dissolved) = carbon
            .map(|c| (c.atmosphere, c.dissolved))
            .unwrap_or((0.0, 0.0));
        self.samples.push(SimSample {
            tick,
            plants: p as u32,
            fungi: f as u32,
            atoms: a as u32,
            corpses: organisms.corpse_count() as u32,
            fallen_plants,
            spores_bank: crate::spore_bank::spore_bank_len(world) as u32,
            sat_free: sat.free_air,
            sat_pore: sat.pore,
            sat_total: sat.cell_total,
            cream_cells: myc.cream_cells,
            cream_sum: myc.cream_sum,
            sugar_sum: myc.sugar_sum,
            strain_cells: myc.strain_cells,
            carbon_atm: atm,
            carbon_dissolved: dissolved,
            mean_temp,
            sym_water_recv_tick: life.sym_water_recv_tick,
            sym_water_sent_tick: life.sym_water_sent_tick,
            sym_sugar_paid_tick: life.sym_sugar_paid_tick,
            sym_sugar_recv_tick: life.sym_sugar_recv_tick,
            plants_sym_linked: life.plants_sym_linked,
            plants_drought: life.plants_drought,
            plants_dry_sym_recv: life.plants_dry_sym_recv,
            plants_with_symbiont: life.plants_with_symbiont,
            mean_root_moist: life.mean_root_moist,
            mean_organic_depth: life.mean_organic_depth,
            max_organic_depth: life.max_organic_depth,
            stemless_plants: life.stemless_plants,
            mean_body_modules: life.mean_body_modules,
            mean_roots: life.mean_roots,
            mean_stems: life.mean_stems,
            mean_photos: life.mean_photos,
            mean_alloc_stem: life.mean_alloc_stem,
            mean_alloc_leaf: life.mean_alloc_leaf,
            mean_alloc_root: life.mean_alloc_root,
            mean_root_depth_bias: life.mean_root_depth_bias,
            mean_clone_fidelity: life.mean_clone_fidelity,
            mean_leaf_absorb: life.mean_leaf_absorb,
            mean_shade_efficiency: life.mean_shade_efficiency,
            mean_sym_water: life.mean_sym_water,
            mean_sym_energy: life.mean_sym_energy,
            woody_plants: life.woody_plants,
            stemless_wet: life.stemless_wet,
            stemless_dry: life.stemless_dry,
            mean_roots_woody: life.mean_roots_woody,
            mean_roots_stemless_wet: life.mean_roots_stemless_wet,
            mean_roots_stemless_dry: life.mean_roots_stemless_dry,
            mean_moist_woody: life.mean_moist_woody,
            mean_moist_stemless_wet: life.mean_moist_stemless_wet,
            mean_moist_stemless_dry: life.mean_moist_stemless_dry,
            drought_woody: life.drought_woody,
            drought_stemless_dry: life.drought_stemless_dry,
            mean_depth_bias_stemless_dry: life.mean_depth_bias_stemless_dry,
            fallen_woody: life.fallen_woody,
            fallen_stemless: life.fallen_stemless,
            mean_org_depth_woody: life.mean_org_depth_woody,
        });
        if self.samples.len() > self.cap / 4 {
            let drop = self.samples.len() - self.cap / 4;
            self.samples.drain(0..drop);
        }
    }

    /// One-line human summary for CI / agent logs.
    pub fn summary(&self) -> String {
        let mut births = 0u32;
        let mut deaths = 0u32;
        let mut tips = 0u32;
        let mut spores = 0u32;
        let mut geotech = 0u32;
        for e in &self.events {
            match &e.kind {
                SimEventKind::Birth { count, .. } => births += count,
                SimEventKind::Death { count, .. } => deaths += count,
                SimEventKind::Tip { count } => tips += count,
                SimEventKind::Spore { count }
                | SimEventKind::EmergentFruiting { count }
                | SimEventKind::SporeBankWake { count } => spores += count,
                SimEventKind::Geotech {
                    roof,
                    shear,
                    compaction,
                } => geotech += roof + shear + compaction,
                SimEventKind::Note { .. } => {}
            }
        }
        let last = self.samples.last();
        format!(
            "sim_log: events={} samples={} births={} deaths={} tips={} spores+={} geotech={} | \
             last_pop p/f/a={}/{}/{} woody/wet/dry={}/{}/{} sat={} cream={} \
             dry_sym={}/{} moist_w/d={:.2}/{:.2} stranded_roots={:.1} \
             alloc_r={:.2} fid={:.2}",
            self.events.len(),
            self.samples.len(),
            births,
            deaths,
            tips,
            spores,
            geotech,
            last.map(|s| s.plants).unwrap_or(0),
            last.map(|s| s.fungi).unwrap_or(0),
            last.map(|s| s.atoms).unwrap_or(0),
            last.map(|s| s.woody_plants).unwrap_or(0),
            last.map(|s| s.stemless_wet).unwrap_or(0),
            last.map(|s| s.stemless_dry).unwrap_or(0),
            last.map(|s| s.sat_total).unwrap_or(0),
            last.map(|s| s.cream_cells).unwrap_or(0),
            last.map(|s| s.plants_dry_sym_recv).unwrap_or(0),
            last.map(|s| s.plants_drought).unwrap_or(0),
            last.map(|s| s.mean_moist_woody).unwrap_or(0.0),
            last.map(|s| s.mean_moist_stemless_dry).unwrap_or(0.0),
            last.map(|s| s.mean_roots_stemless_dry).unwrap_or(0.0),
            last.map(|s| s.mean_alloc_root).unwrap_or(0.0),
            last.map(|s| s.mean_clone_fidelity).unwrap_or(0.0),
        )
    }

    pub fn to_ndjson(&self) -> String {
        let mut out = String::new();
        for e in &self.events {
            if let Ok(line) = serde_json::to_string(&LogLine::Event(e.clone())) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        for s in &self.samples {
            if let Ok(line) = serde_json::to_string(&LogLine::Sample(s.clone())) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }

    pub fn write_ndjson(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut w = BufWriter::new(File::create(path)?);
        w.write_all(self.to_ndjson().as_bytes())?;
        w.flush()
    }

    pub fn print_ndjson(&self) {
        print!("{}", self.to_ndjson());
    }

    /// Flush to `GVSE_SIM_LOG` path when set.
    pub fn flush_env(&self) -> std::io::Result<()> {
        if let Some(ref p) = self.path {
            self.write_ndjson(p)?;
        }
        Ok(())
    }

    pub fn event_count_matching<F>(&self, mut f: F) -> usize
    where
        F: FnMut(&SimEvent) -> bool,
    {
        self.events.iter().filter(|e| f(e)).count()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LogLine {
    Event(SimEvent),
    Sample(SimSample),
}

#[derive(Debug, Clone, Copy, Default)]
struct MycTotals {
    cream_cells: u32,
    cream_sum: u64,
    sugar_sum: u64,
    strain_cells: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct CohortAcc {
    n: u32,
    roots: u32,
    moist: f32,
    drought: u32,
    org: u32,
    depth_bias: f32,
    fallen: u32,
}

impl CohortAcc {
    fn push(&mut self, roots: u32, moist: f32, drought: bool, org: u32, depth_bias: f32, fallen: bool) {
        self.n += 1;
        self.roots += roots;
        self.moist += moist;
        if drought {
            self.drought += 1;
        }
        self.org += org;
        self.depth_bias += depth_bias;
        if fallen {
            self.fallen += 1;
        }
    }

    fn mean_roots(&self) -> f32 {
        if self.n == 0 {
            0.0
        } else {
            self.roots as f32 / self.n as f32
        }
    }

    fn mean_moist(&self) -> f32 {
        if self.n == 0 {
            0.0
        } else {
            self.moist / self.n as f32
        }
    }

    fn mean_org(&self) -> f32 {
        if self.n == 0 {
            0.0
        } else {
            self.org as f32 / self.n as f32
        }
    }

    fn mean_depth_bias(&self) -> f32 {
        if self.n == 0 {
            0.0
        } else {
            self.depth_bias / self.n as f32
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct PlantLifeTotals {
    sym_water_recv_tick: u32,
    sym_water_sent_tick: u32,
    sym_sugar_paid_tick: u32,
    sym_sugar_recv_tick: u32,
    plants_sym_linked: u32,
    plants_drought: u32,
    plants_dry_sym_recv: u32,
    plants_with_symbiont: u32,
    mean_root_moist: f32,
    mean_organic_depth: f32,
    max_organic_depth: u32,
    stemless_plants: u32,
    mean_body_modules: f32,
    mean_roots: f32,
    mean_stems: f32,
    mean_photos: f32,
    mean_alloc_stem: f32,
    mean_alloc_leaf: f32,
    mean_alloc_root: f32,
    mean_root_depth_bias: f32,
    mean_clone_fidelity: f32,
    mean_leaf_absorb: f32,
    mean_shade_efficiency: f32,
    mean_sym_water: f32,
    mean_sym_energy: f32,
    woody_plants: u32,
    stemless_wet: u32,
    stemless_dry: u32,
    mean_roots_woody: f32,
    mean_roots_stemless_wet: f32,
    mean_roots_stemless_dry: f32,
    mean_moist_woody: f32,
    mean_moist_stemless_wet: f32,
    mean_moist_stemless_dry: f32,
    drought_woody: u32,
    drought_stemless_dry: u32,
    mean_depth_bias_stemless_dry: f32,
    fallen_woody: u32,
    fallen_stemless: u32,
    mean_org_depth_woody: f32,
}

/// Consecutive Organic cells downward from the solid under the crown.
fn organic_stack_depth(world: &World, gx: i32, crown_y: i32) -> u32 {
    let mut depth = 0u32;
    let mut y = crown_y - 1;
    for _ in 0..64 {
        let Some(c) = world.get_cell(gx, y) else {
            break;
        };
        if c.material != MaterialId::Organic {
            break;
        }
        depth += 1;
        y -= 1;
    }
    depth
}

fn plant_life_totals(world: &World, organisms: &OrganismStore) -> PlantLifeTotals {
    let mut t = PlantLifeTotals::default();
    let mut n = 0u32;
    let mut moist_sum = 0.0f32;
    let mut org_sum = 0u32;
    let mut body_sum = 0u32;
    let mut root_sum = 0u32;
    let mut stem_sum = 0u32;
    let mut photo_sum = 0u32;
    let mut alloc_s = 0.0f32;
    let mut alloc_l = 0.0f32;
    let mut alloc_r = 0.0f32;
    let mut depth_bias = 0.0f32;
    let mut fidelity = 0.0f32;
    let mut leaf_abs = 0.0f32;
    let mut shade = 0.0f32;
    let mut sym_w = 0.0f32;
    let mut sym_e = 0.0f32;
    let mut woody = CohortAcc::default();
    let mut wet = CohortAcc::default();
    let mut dry_stemless = CohortAcc::default();

    for atom in &organisms.atoms {
        if !is_land_plant(atom) {
            continue;
        }
        n += 1;
        let recv = u32::from(atom.sym_water_recv_last);
        let sent = u32::from(atom.sym_water_sent_last);
        let paid = u32::from(atom.sym_sugar_paid_last);
        let got = u32::from(atom.sym_sugar_recv_last);
        t.sym_water_recv_tick += recv;
        t.sym_water_sent_tick += sent;
        t.sym_sugar_paid_tick += paid;
        t.sym_sugar_recv_tick += got;
        if recv + sent + paid + got > 0 {
            t.plants_sym_linked += 1;
        }
        let moist = plant_moisture_frac(world, atom);
        moist_sum += moist;
        // Soft stress + hard dormancy (dormancy alone misses brief dry dips).
        let drought = moist < DROUGHT_STRESS_FRAC
            || moist < DROUGHT_DORMANT_FRAC
            || atom.drought_ticks > 0;
        if drought {
            t.plants_drought += 1;
            if recv > 0 {
                t.plants_dry_sym_recv += 1;
            }
        }
        if body_has_symbiont(&atom.body) {
            t.plants_with_symbiont += 1;
        }
        let stems_n = stem_count(atom) as u32;
        let stemless = stems_n == 0;
        if stemless {
            t.stemless_plants += 1;
        }
        let org = organic_stack_depth(world, atom.gx, atom.gy);
        org_sum += org;
        t.max_organic_depth = t.max_organic_depth.max(org);

        let mut roots = 0u32;
        let mut photos = 0u32;
        for &(_, _, m) in &atom.body {
            match m {
                ModuleId::Root => roots += 1,
                ModuleId::Photosystem => photos += 1,
                _ => {}
            }
        }
        body_sum += atom.body.len() as u32;
        root_sum += roots;
        stem_sum += stems_n;
        photo_sum += photos;

        let (as_, al, ar) = atom.genome.alloc_weights();
        alloc_s += as_;
        alloc_l += al;
        alloc_r += ar;
        depth_bias += atom.genome.root_depth_bias;
        fidelity += atom.genome.clone_fidelity;
        leaf_abs += atom.genome.leaf_absorb;
        shade += atom.genome.shade_efficiency;
        sym_w += f32::from(atom.genome.sym_water);
        sym_e += f32::from(atom.genome.sym_energy);

        // Cohort split: woody trunk vs bathing seaweed vs stranded seaweed.
        if !stemless {
            woody.push(
                roots,
                moist,
                drought,
                org,
                atom.genome.root_depth_bias,
                atom.fallen,
            );
        } else if leaves_bathing(world, atom) {
            wet.push(
                roots,
                moist,
                drought,
                org,
                atom.genome.root_depth_bias,
                atom.fallen,
            );
        } else {
            dry_stemless.push(
                roots,
                moist,
                drought,
                org,
                atom.genome.root_depth_bias,
                atom.fallen,
            );
        }
    }

    t.woody_plants = woody.n;
    t.stemless_wet = wet.n;
    t.stemless_dry = dry_stemless.n;
    t.mean_roots_woody = woody.mean_roots();
    t.mean_roots_stemless_wet = wet.mean_roots();
    t.mean_roots_stemless_dry = dry_stemless.mean_roots();
    t.mean_moist_woody = woody.mean_moist();
    t.mean_moist_stemless_wet = wet.mean_moist();
    t.mean_moist_stemless_dry = dry_stemless.mean_moist();
    t.drought_woody = woody.drought;
    t.drought_stemless_dry = dry_stemless.drought;
    t.mean_depth_bias_stemless_dry = dry_stemless.mean_depth_bias();
    t.fallen_woody = woody.fallen;
    t.fallen_stemless = wet.fallen + dry_stemless.fallen;
    t.mean_org_depth_woody = woody.mean_org();

    if n > 0 {
        let nf = n as f32;
        t.mean_root_moist = moist_sum / nf;
        t.mean_organic_depth = org_sum as f32 / nf;
        t.mean_body_modules = body_sum as f32 / nf;
        t.mean_roots = root_sum as f32 / nf;
        t.mean_stems = stem_sum as f32 / nf;
        t.mean_photos = photo_sum as f32 / nf;
        t.mean_alloc_stem = alloc_s / nf;
        t.mean_alloc_leaf = alloc_l / nf;
        t.mean_alloc_root = alloc_r / nf;
        t.mean_root_depth_bias = depth_bias / nf;
        t.mean_clone_fidelity = fidelity / nf;
        t.mean_leaf_absorb = leaf_abs / nf;
        t.mean_shade_efficiency = shade / nf;
        t.mean_sym_water = sym_w / nf;
        t.mean_sym_energy = sym_e / nf;
    }
    t
}

fn mycelium_totals(world: &World) -> MycTotals {
    let mut t = MycTotals::default();
    for (&coord, chunk) in &world.chunks {
        let x0 = coord.cx * crate::chunk::CHUNK_CELLS_W as i32;
        let y0 = coord.cy * crate::chunk::CHUNK_CELLS_H as i32;
        for ly in 0..crate::chunk::CHUNK_CELLS_H {
            for lx in 0..crate::chunk::CHUNK_CELLS_W {
                let cell = chunk.get(lx, ly);
                if !hosts_mycelium(cell.material) {
                    continue;
                }
                let myc = cell.mycelium();
                if myc == 0 {
                    continue;
                }
                t.cream_cells += 1;
                t.cream_sum += u64::from(myc);
                let gx = x0 + lx as i32;
                let gy = y0 + ly as i32;
                if let Some(&sugar) = world.mycelium_energy.get(&(gx, gy)) {
                    t.sugar_sum += u64::from(sugar);
                }
                if world.mycelium_strains.contains_key(&(gx, gy)) {
                    t.strain_cells += 1;
                }
            }
        }
    }
    t
}

/// Habit label for logging.
pub fn habit_label(atom: &crate::organism::Atom) -> &'static str {
    if is_land_plant(atom) {
        "plant"
    } else if is_fungus(atom) {
        "fungus"
    } else {
        "atom"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::{Blueprint, Genome};
    use crate::cell::Cell;
    use crate::chunk::ChunkCoord;
    use wk_material::MaterialId;

    fn moist_plot() -> World {
        let mut w = World::new(7);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = crate::cell::Sat(80);
            w.set_cell(x, 1, sand);
            for y in 2..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w
    }

    #[test]
    fn records_organism_stats_and_samples() {
        let mut log = SimLog::new(1000, 1);
        let mut w = moist_plot();
        let mut store = OrganismStore::new();
        let body = Blueprint::minimal_plant().modules_relative_to_nucleus();
        assert!(store.spawn_blueprint(&w, 4, 2, body, 40.0, Genome::default()));
        log.note(0, "start");
        log.record_organism(
            1,
            &OrganismStepStats {
                births_plant: 1,
                tips: 1,
                ..OrganismStepStats::default()
            },
        );
        log.push_sample(1, &w, &store, None, Some(12.0));
        assert_eq!(log.events.len(), 3); // note + birth + tip
        assert_eq!(log.samples.len(), 1);
        assert!(log.summary().contains("births=1"));
        let s = &log.samples[0];
        assert!(s.mean_body_modules >= 1.0);
        assert!(s.mean_roots >= 1.0);
        assert!(s.mean_clone_fidelity > 0.0);
        assert_eq!(s.woody_plants, 1, "minimal_plant is woody");
        assert_eq!(s.stemless_wet + s.stemless_dry, 0);
        let nd = log.to_ndjson();
        assert!(nd.contains("\"type\":\"event\""));
        assert!(nd.contains("\"type\":\"sample\""));
        assert!(nd.contains("\"plants\":1"));
        assert!(nd.contains("\"plants_dry_sym_recv\""));
        assert!(nd.contains("\"mean_alloc_root\""));
        assert!(nd.contains("\"stemless_dry\""));
        assert!(nd.contains("\"woody_plants\""));
    }

    #[test]
    fn ring_caps_events() {
        let mut log = SimLog::new(8, 60);
        for t in 0..20 {
            log.note(t, "x");
        }
        assert!(log.events.len() <= 8);
    }
}
