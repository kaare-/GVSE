//! Toggleable living / dead creature roster for pop-cap debugging.
//! Isolation: wk-voxel + macroquad only.

use macroquad::prelude::*;
use wk_voxel::{Atom, Corpse, ModuleId, OrganismStore};

const PANEL_W: f32 = 340.0;
const PAD: f32 = 10.0;
const ROW_H: f32 = 16.0;
const TITLE_H: f32 = 44.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListSection {
    Living,
    Dead,
}

/// F4 creature list overlay.
pub struct CreatureList {
    pub open: bool,
    scroll: f32,
    /// Selected living atom index, if any.
    pub selected_live: Option<usize>,
    /// Selected corpse index, if any.
    pub selected_dead: Option<usize>,
    section: ListSection,
}

impl Default for CreatureList {
    fn default() -> Self {
        Self {
            open: false,
            scroll: 0.0,
            selected_live: None,
            selected_dead: None,
            section: ListSection::Living,
        }
    }
}

impl CreatureList {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.selected_live = None;
            self.selected_dead = None;
        }
    }

    pub fn hits_panel(&self, mx: f32, my: f32) -> bool {
        if !self.open {
            return false;
        }
        let sh = screen_height();
        let panel_h = sh - 8.0;
        mx >= 8.0 && mx < 8.0 + PANEL_W && my >= 8.0 && my < 8.0 + panel_h
    }

    /// Handle input while open. Returns nucleus cell to focus inspector on.
    pub fn handle_input(&mut self, store: &OrganismStore) -> Option<(i32, i32)> {
        if !self.open {
            return None;
        }
        let (mx, my) = mouse_position();
        let sh = screen_height();
        let panel_h = sh - 8.0;
        let x0 = 8.0;
        let y0 = 8.0;

        // Scroll when cursor is over the panel.
        if self.hits_panel(mx, my) {
            let wheel = mouse_wheel().1;
            if wheel != 0.0 {
                self.scroll = (self.scroll - wheel * 36.0).max(0.0);
            }
        }

        // Tab living / dead.
        if is_key_pressed(KeyCode::Q) {
            self.section = match self.section {
                ListSection::Living => ListSection::Dead,
                ListSection::Dead => ListSection::Living,
            };
            self.scroll = 0.0;
        }

        if !is_mouse_button_pressed(MouseButton::Left) {
            return None;
        }
        if mx < x0 || mx >= x0 + PANEL_W || my < y0 || my >= y0 + panel_h {
            return None;
        }

        let rows_top = y0 + TITLE_H;
        let idx = ((my - rows_top + self.scroll) / ROW_H).floor() as i32;
        if idx < 0 {
            return None;
        }
        let idx = idx as usize;

        match self.section {
            ListSection::Living => {
                if idx >= store.atoms.len() {
                    return None;
                }
                self.selected_live = Some(idx);
                self.selected_dead = None;
                let a = &store.atoms[idx];
                Some((a.gx, a.gy))
            }
            ListSection::Dead => {
                if idx >= store.corpses.len() {
                    return None;
                }
                self.selected_dead = Some(idx);
                self.selected_live = None;
                let c = &store.corpses[idx];
                Some((c.gx, c.gy))
            }
        }
    }

    pub fn draw(&self, store: &OrganismStore) {
        if !self.open {
            return;
        }
        let sh = screen_height();
        let panel_h = sh - 8.0;
        let x0 = 8.0;
        let y0 = 8.0;

        draw_rectangle(x0, y0, PANEL_W, panel_h, Color::from_rgba(10, 12, 18, 235));
        draw_rectangle_lines(
            x0,
            y0,
            PANEL_W,
            panel_h,
            1.5,
            Color::from_rgba(120, 160, 200, 255),
        );

        let live_n = store.len();
        let dead_n = store.corpse_count();
        let (p, f, a) = store.habit_counts();
        let title = match self.section {
            ListSection::Living => {
                format!("CREATURES (living {live_n}  p={p} f={f} a={a})  [Q]=dead")
            }
            ListSection::Dead => format!("CREATURES (dead {dead_n})  [Q]=living"),
        };
        draw_text(&title, x0 + PAD, y0 + 18.0, 16.0, Color::from_rgba(180, 210, 255, 255));
        draw_text(
            "F4 close · click row → inspect · wheel scroll",
            x0 + PAD,
            y0 + 34.0,
            13.0,
            GRAY,
        );

        let rows_top = y0 + TITLE_H;
        let rows_bot = y0 + panel_h - PAD;
        let view_h = (rows_bot - rows_top).max(0.0);
        let count = match self.section {
            ListSection::Living => store.atoms.len(),
            ListSection::Dead => store.corpses.len(),
        };
        let content_h = count as f32 * ROW_H;
        let max_scroll = (content_h - view_h).max(0.0);
        let scroll = self.scroll.clamp(0.0, max_scroll);
        let first = (scroll / ROW_H).floor().max(0.0) as usize;
        let visible = (view_h / ROW_H).ceil() as usize + 1;

        for off in 0..visible {
            let i = first + off;
            if i >= count {
                break;
            }
            let row_y = rows_top + i as f32 * ROW_H - scroll;
            if row_y + ROW_H < rows_top || row_y > rows_bot {
                continue;
            }
            let selected = match self.section {
                ListSection::Living => self.selected_live == Some(i),
                ListSection::Dead => self.selected_dead == Some(i),
            };
            if selected {
                draw_rectangle(
                    x0 + 2.0,
                    row_y,
                    PANEL_W - 4.0,
                    ROW_H,
                    Color::from_rgba(40, 70, 110, 220),
                );
            }
            let line = match self.section {
                ListSection::Living => format_live(i, &store.atoms[i]),
                ListSection::Dead => format_dead(i, &store.corpses[i]),
            };
            draw_text(&line, x0 + PAD, row_y + 12.0, 14.0, WHITE);
        }

        if count == 0 {
            draw_text(
                "(empty)",
                x0 + PAD,
                rows_top + 16.0,
                14.0,
                Color::from_rgba(140, 140, 140, 255),
            );
        }
    }
}

fn module_tally(body: &[(i16, i16, ModuleId)]) -> String {
    let mut n = 0usize;
    let mut p = 0usize;
    let mut r = 0usize;
    let mut s = 0usize;
    let mut d = 0usize;
    let mut h = 0usize;
    let mut sp = 0usize;
    for (_, _, m) in body {
        match *m {
            ModuleId::Nucleus => n += 1,
            ModuleId::Photosystem => p += 1,
            ModuleId::Root => r += 1,
            ModuleId::Stem => s += 1,
            ModuleId::Digest => d += 1,
            ModuleId::Hypha => h += 1,
            ModuleId::ReproSpore => sp += 1,
        }
    }
    let mut parts = Vec::new();
    if n > 0 {
        parts.push(format!("N{n}"));
    }
    if r > 0 {
        parts.push(format!("R{r}"));
    }
    if s > 0 {
        parts.push(format!("S{s}"));
    }
    if p > 0 {
        parts.push(format!("P{p}"));
    }
    if d > 0 {
        parts.push(format!("D{d}"));
    }
    if h > 0 {
        parts.push(format!("H{h}"));
    }
    if sp > 0 {
        parts.push(format!("Sp{sp}"));
    }
    if parts.is_empty() {
        "∅".into()
    } else {
        parts.join("")
    }
}

fn format_live(id: usize, atom: &Atom) -> String {
    let mods = module_tally(&atom.body);
    format!(
        "#{id} ({},{}) E{:.0}/{:.0} age{} {}",
        atom.gx, atom.gy, atom.energy, atom.energy_max, atom.age_ticks, mods
    )
}

fn format_dead(id: usize, corpse: &Corpse) -> String {
    let mods = module_tally(&corpse.body);
    format!(
        "#{id} ({},{}) settled={} {}",
        corpse.gx, corpse.gy, corpse.settled_ticks, mods
    )
}
