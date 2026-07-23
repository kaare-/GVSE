//! MS-Paint creature editor for wk-voxel-app.
//! Mirrors the column `wk-app` editor (docs/organism/EDITOR.md):
//! Set A Atom + Set D plant + Set E fungus — no wk-agents / wk-app imports.

use macroquad::prelude::*;
use wk_voxel::{Blueprint, LaneId, ModuleId, PlacedModule};

const CELL_PX: f32 = 22.0;
const CANVAS_ORIGIN: (f32, f32) = (40.0, 80.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorTool {
    Paint,
    Erase,
}

pub struct CreatureEditor {
    pub open: bool,
    pub blueprint: Blueprint,
    pub tool: EditorTool,
    pub brush: ModuleId,
    pub status: String,
    pub spawn_picker: bool,
    pub was_paused: bool,
    name_buf: String,
}

impl Default for CreatureEditor {
    fn default() -> Self {
        Self {
            open: false,
            blueprint: Blueprint::atom(),
            tool: EditorTool::Paint,
            brush: ModuleId::Photosystem,
            status: "Paint Atom / Plant / Fungus, then Enter + click to spawn".into(),
            spawn_picker: false,
            was_paused: true,
            name_buf: "atom".into(),
        }
    }
}

impl CreatureEditor {
    pub fn toggle(&mut self, currently_paused: bool) {
        if self.open {
            self.open = false;
            self.spawn_picker = false;
        } else {
            self.open = true;
            self.was_paused = currently_paused;
            self.spawn_picker = false;
            self.status =
                "1-6 modules | A Atom  T Plant  F Fungus | Enter then click spawn site"
                    .into();
        }
    }

    pub fn handle_input(&mut self) {
        if !self.open {
            return;
        }
        if is_key_pressed(KeyCode::Key1) {
            self.brush = ModuleId::Nucleus;
            self.tool = EditorTool::Paint;
        }
        if is_key_pressed(KeyCode::Key2) {
            self.brush = ModuleId::Photosystem;
            self.tool = EditorTool::Paint;
        }
        if is_key_pressed(KeyCode::Key3) {
            self.brush = ModuleId::Root;
            self.tool = EditorTool::Paint;
        }
        if is_key_pressed(KeyCode::Key4) {
            self.brush = ModuleId::Stem;
            self.tool = EditorTool::Paint;
        }
        if is_key_pressed(KeyCode::Key5) {
            self.brush = ModuleId::Digest;
            self.tool = EditorTool::Paint;
        }
        if is_key_pressed(KeyCode::Key6) {
            self.brush = ModuleId::Hypha;
            self.tool = EditorTool::Paint;
        }
        if is_key_pressed(KeyCode::E) {
            self.tool = EditorTool::Erase;
        }
        if is_key_pressed(KeyCode::P) {
            self.tool = EditorTool::Paint;
        }
        if is_key_pressed(KeyCode::A) {
            self.blueprint = Blueprint::atom();
            self.name_buf = "atom".into();
            self.status = "Reset to Atom template (spawn on wet Air)".into();
        }
        if is_key_pressed(KeyCode::T) {
            self.blueprint = Blueprint::minimal_plant();
            self.name_buf = "plant".into();
            self.status = "Minimal plant template (spawn on moist sand/soil)".into();
        }
        if is_key_pressed(KeyCode::F) {
            self.blueprint = Blueprint::minimal_fungus();
            self.name_buf = "fungus".into();
            self.status =
                "Minimal fungus template (spawn on Organic / wet sand / any solid)".into();
        }
        if is_key_pressed(KeyCode::S) && !is_key_down(KeyCode::LeftControl) {
            self.blueprint.name = self.name_buf.clone();
            match self.blueprint.save_to_disk() {
                Ok(p) => self.status = format!("Saved {}", p.display()),
                Err(e) => self.status = format!("Save failed: {e}"),
            }
        }
        if is_key_pressed(KeyCode::L) {
            if let Some(path) = Blueprint::list_disk().into_iter().next() {
                match Blueprint::load_from_disk(&path) {
                    Ok(bp) => {
                        self.name_buf = bp.name.clone();
                        self.blueprint = bp;
                        self.status = format!("Loaded {}", path.display());
                    }
                    Err(e) => self.status = format!("Load failed: {e}"),
                }
            } else {
                self.status = "No blueprints/ yet — press S to save one".into();
            }
        }
        if is_key_pressed(KeyCode::Enter) {
            if self.blueprint.can_editor_spawn() {
                self.spawn_picker = true;
                self.status =
                    "SPAWN — click any Air cell (Esc cancel); odd mixes may not thrive".into();
            } else {
                self.status = "Need at least one Nucleus on the canvas".into();
            }
        }
        if is_key_pressed(KeyCode::Escape) && self.spawn_picker {
            self.spawn_picker = false;
            self.status = "Spawn cancelled".into();
        }

        if !self.spawn_picker && is_mouse_button_down(MouseButton::Left) {
            if let Some((cx, cy)) = self.mouse_to_cell() {
                self.apply_tool(cx, cy);
            }
        }
    }

    fn mouse_to_cell(&self) -> Option<(i16, i16)> {
        let (mx, my) = mouse_position();
        let (ox, oy) = CANVAS_ORIGIN;
        let w = self.blueprint.canvas_w as f32 * CELL_PX;
        let h = self.blueprint.canvas_h as f32 * CELL_PX;
        if mx < ox || my < oy || mx >= ox + w || my >= oy + h {
            return None;
        }
        let cx = ((mx - ox) / CELL_PX).floor() as i16;
        let row_from_top = ((my - oy) / CELL_PX).floor() as i16;
        let cy = (self.blueprint.canvas_h as i16 - 1) - row_from_top;
        Some((cx, cy))
    }

    fn apply_tool(&mut self, cx: i16, cy: i16) {
        self.blueprint
            .modules
            .retain(|m| !(m.x == cx && m.y == cy && m.lane == LaneId::Mid));
        if self.tool == EditorTool::Paint {
            self.blueprint.modules.push(PlacedModule {
                x: cx,
                y: cy,
                lane: LaneId::Mid,
                module: self.brush,
            });
        }
    }

    pub fn draw(&self) {
        if !self.open {
            return;
        }
        let sw = screen_width();
        let sh = screen_height();
        if self.spawn_picker {
            draw_rectangle(0.0, 0.0, sw, 36.0, Color::from_rgba(8, 10, 16, 200));
            let msg =
                "SPAWN MODE — click ground/Air (plants snap to surface)  |  Esc cancel  |  F2 close";
            draw_text(msg, 16.0, 24.0, 20.0, GREEN);
            return;
        }
        draw_rectangle(0.0, 0.0, sw, sh, Color::from_rgba(8, 10, 16, 210));
        draw_text(
            "CREATURE EDITOR  (F2 to close)",
            40.0,
            36.0,
            28.0,
            Color::from_rgba(255, 220, 80, 255),
        );

        let (ox, oy) = CANVAS_ORIGIN;
        let cw = self.blueprint.canvas_w as f32 * CELL_PX;
        let ch = self.blueprint.canvas_h as f32 * CELL_PX;
        draw_rectangle(
            ox - 4.0,
            oy - 4.0,
            cw + 8.0,
            ch + 8.0,
            Color::from_rgba(30, 34, 44, 255),
        );
        draw_rectangle_lines(
            ox - 4.0,
            oy - 4.0,
            cw + 8.0,
            ch + 8.0,
            2.0,
            Color::from_rgba(255, 220, 80, 255),
        );

        for y in 0..self.blueprint.canvas_h {
            for x in 0..self.blueprint.canvas_w {
                let sx = ox + x as f32 * CELL_PX;
                let sy = oy + (self.blueprint.canvas_h - 1 - y) as f32 * CELL_PX;
                draw_rectangle_lines(
                    sx,
                    sy,
                    CELL_PX,
                    CELL_PX,
                    1.0,
                    Color::from_rgba(70, 70, 80, 255),
                );
            }
        }

        for m in &self.blueprint.modules {
            let sx = ox + m.x as f32 * CELL_PX;
            let sy = oy + (self.blueprint.canvas_h as i16 - 1 - m.y) as f32 * CELL_PX;
            let (r, g, b) = m.module.rgb();
            draw_rectangle(
                sx + 1.0,
                sy + 1.0,
                CELL_PX - 2.0,
                CELL_PX - 2.0,
                Color::from_rgba(r, g, b, 255),
            );
        }

        let px = ox + cw + 24.0;
        let kind = if self.blueprint.is_valid_fungus() {
            "Set E fungus"
        } else if self.blueprint.is_valid_plant() {
            "Set D plant"
        } else if self.blueprint.is_valid_atom() {
            "Set A Atom"
        } else {
            "incomplete"
        };
        draw_text(&format!("Creature editor ({kind})"), px, oy, 22.0, WHITE);
        draw_text(
            &format!("Tool: {:?}  Brush: {}", self.tool, self.brush.name()),
            px,
            oy + 28.0,
            16.0,
            LIGHTGRAY,
        );
        draw_text(
            "1 Nucleus  2 Photo  3 Root  4 Stem  5 Digest  6 Hypha  | E erase  P paint",
            px,
            oy + 52.0,
            14.0,
            GRAY,
        );
        draw_text(
            "A Atom  T Plant  F Fungus  | S save  L load  | Enter spawn",
            px,
            oy + 72.0,
            14.0,
            GRAY,
        );
        draw_text(
            &format!(
                "modules={}  valid={}  name={}",
                self.blueprint.modules.len(),
                self.blueprint.is_valid_creature(),
                self.name_buf
            ),
            px,
            oy + 100.0,
            14.0,
            WHITE,
        );
        draw_text(&self.status, px, oy + 130.0, 14.0, YELLOW);

        for (i, mid) in [
            ModuleId::Nucleus,
            ModuleId::Photosystem,
            ModuleId::Root,
            ModuleId::Stem,
            ModuleId::Digest,
            ModuleId::Hypha,
        ]
        .iter()
        .enumerate()
        {
            let (r, g, b) = mid.rgb();
            let sx = px + i as f32 * 36.0;
            let sy = oy + 160.0;
            draw_rectangle(sx, sy, 28.0, 28.0, Color::from_rgba(r, g, b, 255));
            if *mid == self.brush {
                draw_rectangle_lines(sx, sy, 28.0, 28.0, 2.0, WHITE);
            }
        }
    }
}
