//! Ugly mass-budget ledger overlay (`B`).
//!
//! Not a heatmap. Snapshot stores at a mark and dump deltas so a pretty
//! cliff cannot impersonate physics. `N` remakes the mark while on —
//! that is not the retired humidity-bank overlay.

use macroquad::prelude::*;
use wk_voxel::{BudgetLedger, Humidity, World};

const LINE_H: f32 = 15.0;
const PAD: f32 = 8.0;

/// Play-app wrapper: ledger + named cheats since the mark.
#[derive(Debug, Clone, Default)]
pub struct BudgetHud {
    pub ledger: BudgetLedger,
    cheats: Vec<String>,
    last_event: String,
}

impl BudgetHud {
    pub fn is_on(&self) -> bool {
        self.ledger.is_on()
    }

    pub fn toggle(&mut self, world: &World, humidity: &Humidity) {
        if self.ledger.is_on() {
            self.ledger.disable();
            self.cheats.clear();
            self.last_event.clear();
        } else {
            self.ledger.enable(world, humidity);
            self.cheats.clear();
            self.last_event = format!("mark t={}", world.tick);
        }
    }

    /// Regen / load / `N` — pin a new mark on the current inventory.
    pub fn remake(&mut self, world: &World, humidity: &Humidity, why: &str) {
        if !self.ledger.is_on() {
            return;
        }
        self.ledger.enable(world, humidity);
        self.cheats.clear();
        self.last_event = format!("{why} t={}", world.tick);
    }

    pub fn note_cheat(&mut self, tag: &str) {
        if !self.ledger.is_on() {
            return;
        }
        if !self.cheats.iter().any(|c| c == tag) {
            self.cheats.push(tag.to_string());
        }
    }

    pub fn sample_if_due(&mut self, world: &World, humidity: &Humidity) {
        self.ledger.sample_if_due(world, humidity);
    }

    pub fn refresh(&mut self, world: &World, humidity: &Humidity) {
        self.ledger.refresh(world, humidity);
    }

    pub fn draw(&self) {
        if !self.ledger.is_on() {
            return;
        }
        let (Some(mark), Some(now)) = (self.ledger.mark(), self.ledger.now()) else {
            return;
        };
        let d = now.delta(mark);
        let mut flags = String::new();
        if d.soak_like() {
            flags.push_str("SOAK ");
        }
        if d.evap_like() {
            flags.push_str("EVAP ");
        }
        if d.rain_like() {
            flags.push_str("RAIN ");
        }
        if d.freeze_like() {
            flags.push_str("FREEZE ");
        }
        if d.unexplained_water() {
            flags.push_str("UNEXPL-W ");
        }
        if d.unexplained_mineral() {
            flags.push_str("UNEXPL-M ");
        }
        if !self.cheats.is_empty() {
            flags.push_str("CHEAT ");
        }
        if flags.is_empty() {
            flags.push_str("(flat)");
        }

        let cheats = if self.cheats.is_empty() {
            "(none)".into()
        } else {
            self.cheats.join(", ")
        };
        let event = if self.last_event.is_empty() {
            "—"
        } else {
            self.last_event.as_str()
        };

        let lines = [
            format!("BUDGET  B=on  N=remake  scan/{}t", self.ledger.period),
            format!("mark t={}  now t={}  dt={}", mark.tick, now.tick, d.ticks),
            format!("event  {event}"),
            "store          mark          now            d".into(),
            row_i("free", mark.free_air, now.free_air, d.d_free),
            row_i("pore", mark.pore, now.pore, d.d_pore),
            row_i("steam", mark.steam, now.steam, d.d_steam),
            row_i("cave_h", mark.cave_h, now.cave_h, d.d_cave),
            row_i("pipe", mark.pipe, now.pipe, d.d_pipe),
            row_f(
                "hum",
                f64::from(mark.humidity),
                f64::from(now.humidity),
                d.d_humidity,
            ),
            row_i("ice", mark.ice, now.ice, d.d_ice),
            row_i("snow", mark.snow, now.snow, d.d_snow),
            row_i(
                "cells",
                mark.cell_total(),
                now.cell_total(),
                now.cell_total() - mark.cell_total(),
            ),
            row_f("TRACKED", mark.tracked(), now.tracked(), d.d_tracked),
            row_i(
                "min.sol",
                mark.mineral_solid,
                now.mineral_solid,
                d.d_min_solid,
            ),
            row_i(
                "min.load",
                mark.mineral_load,
                now.mineral_load,
                d.d_min_load,
            ),
            row_i(
                "min.tot",
                mark.mineral_total(),
                now.mineral_total(),
                d.d_min_total,
            ),
            format!("read   {flags}"),
            format!("cheats {cheats}"),
            "TRACKED leftover = mint/cull/OOB (or uncredited sink)".into(),
            "SOAK=free→pore  EVAP=cell→hum  RAIN=hum→free/ice".into(),
        ];

        let w = 420.0;
        let h = PAD * 2.0 + lines.len() as f32 * LINE_H + 4.0;
        let x = 8.0;
        let y = 8.0;
        draw_rectangle(x, y, w, h, Color::from_rgba(18, 0, 18, 230));
        draw_rectangle_lines(x, y, w, h, 2.0, Color::from_rgba(255, 0, 180, 255));
        for (i, line) in lines.iter().enumerate() {
            let color = if i == 0 {
                Color::from_rgba(255, 255, 0, 255)
            } else if line.starts_with("TRACKED leftover") || line.starts_with("SOAK=") {
                Color::from_rgba(180, 180, 180, 255)
            } else if line.starts_with("read") && (d.unexplained_water() || d.unexplained_mineral())
            {
                Color::from_rgba(255, 80, 80, 255)
            } else if line.starts_with("TRACKED") {
                if d.unexplained_water() {
                    Color::from_rgba(255, 80, 80, 255)
                } else {
                    Color::from_rgba(80, 255, 80, 255)
                }
            } else {
                Color::from_rgba(255, 220, 80, 255)
            };
            draw_text(
                line,
                x + PAD,
                y + PAD + 12.0 + i as f32 * LINE_H,
                14.0,
                color,
            );
        }
    }
}

fn row_i(name: &str, mark: i64, now: i64, d: i64) -> String {
    format!("{name:<8} {mark:>12} {now:>12} {d:>+12}")
}

fn row_f(name: &str, mark: f64, now: f64, d: f64) -> String {
    format!("{name:<8} {mark:>12.0} {now:>12.0} {d:>+12.0}")
}
