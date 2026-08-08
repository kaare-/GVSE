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

use crate::audit::sat_totals;
use crate::carbon::CarbonBudget;
use crate::cell::hosts_mycelium;
use crate::failure::FailureStats;
use crate::fungi::is_fungus;
use crate::grid::World;
use crate::organism::{OrganismStepStats, OrganismStore};
use crate::plant::is_land_plant;

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
            "sim_log: events={} samples={} births={} deaths={} tips={} spores+={} geotech={} | last_pop p/f/a={}/{}/{} sat={} cream_cells={}",
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
            last.map(|s| s.sat_total).unwrap_or(0),
            last.map(|s| s.cream_cells).unwrap_or(0),
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
        let nd = log.to_ndjson();
        assert!(nd.contains("\"type\":\"event\""));
        assert!(nd.contains("\"type\":\"sample\""));
        assert!(nd.contains("\"plants\":1"));
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
