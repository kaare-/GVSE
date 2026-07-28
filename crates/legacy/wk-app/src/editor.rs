//! MS-Paint creature editor tab (Organism Kernel Set A / D / E MVP).
//! Spec: `docs/organism/EDITOR.md`.

use macroquad::prelude::*;
use wk_sim::{Blueprint, LaneId, ModuleId, PlacedModule};

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
    /// Pause state to restore when closing.
    pub was_paused: bool,
    name_buf: String,
}

impl Default for CreatureEditor {
    fn default() -> Self {
        Self {
            open: false,
            blueprint: Blueprint::atom(Default::default()),
            tool: EditorTool::Paint,
            brush: ModuleId::Photosystem,
            status: "Paint Atom (black nucleus + green photosystem), then Spawn".into(),
            spawn_picker: false,
            was_paused: true,
            name_buf: "atom".into(),
        }
    }
}

fn brush_paintable(mid: ModuleId) -> bool {
    mid.set_d_paintable() || mid.set_e_paintable()
}

fn blueprint_spawnable(bp: &Blueprint) -> bool {
    bp.is_valid_atom() || bp.is_valid_fungus()
}

fn habit_label(bp: &Blueprint) -> &'static str {
    if bp.is_fungus() {
        "fungus"
    } else if bp.is_rooted() {
        "land plant"
    } else if bp.is_plankton() {
        "plankton"
    } else if bp.is_valid_atom() {
        "atom"
    } else {
        "incomplete"
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
                "1 nuc 2 photo 3 root 4 stem 5 digest 6 hypha | F=fungus template | Enter spawn"
                    .into();
        }
    }

    pub fn handle_input(&mut self) -> EditorAction {
        if !self.open {
            return EditorAction::None;
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
        // Quick litter-fungus starter (nucleus + digest + hypha thread).
        if is_key_pressed(KeyCode::F) {
            self.blueprint = Blueprint::minimal_fungus(self.blueprint.genome);
            self.name_buf = self.blueprint.name.clone();
            self.brush = ModuleId::Hypha;
            self.tool = EditorTool::Paint;
            self.status = "Loaded minimal fungus — paint hyphae, Enter to spawn on litter".into();
        }
        if is_key_pressed(KeyCode::E) {
            self.tool = EditorTool::Erase;
        }
        if is_key_pressed(KeyCode::P) {
            self.tool = EditorTool::Paint;
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
            if blueprint_spawnable(&self.blueprint) {
                self.spawn_picker = true;
                self.status = if self.blueprint.is_fungus() {
                    "Click litter / Organic land to spawn fungus — Esc cancel".into()
                } else if self.blueprint.is_plankton() {
                    "Click ocean or land to spawn algae Atom — Esc cancel".into()
                } else {
                    "Click a land column to spawn, Esc cancel".into()
                };
            } else {
                self.status =
                    "Need nucleus+photosystem (atom/plant) or nucleus+digest (fungus)".into();
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

        EditorAction::None
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
        // Editor y increases upward visually; store with y=0 near bottom.
        let row_from_top = ((my - oy) / CELL_PX).floor() as i16;
        let cy = (self.blueprint.canvas_h as i16 - 1) - row_from_top;
        Some((cx, cy))
    }

    fn apply_tool(&mut self, cx: i16, cy: i16) {
        self.blueprint
            .modules
            .retain(|m| !(m.x == cx && m.y == cy && m.lane == LaneId::Mid));
        if self.tool == EditorTool::Paint && brush_paintable(self.brush) {
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
        // Heavy dim so the editor is unmistakable against the world view.
        draw_rectangle(0.0, 0.0, sw, sh, Color::from_rgba(8, 10, 16, 210));
        draw_text(
            "CREATURE EDITOR  (C or F2 to close)",
            40.0,
            36.0,
            28.0,
            Color::from_rgba(255, 220, 80, 255),
        );

        let (ox, oy) = CANVAS_ORIGIN;
        let cw = self.blueprint.canvas_w as f32 * CELL_PX;
        let ch = self.blueprint.canvas_h as f32 * CELL_PX;
        draw_rectangle(ox - 4.0, oy - 4.0, cw + 8.0, ch + 8.0, Color::from_rgba(30, 34, 44, 255));
        draw_rectangle_lines(ox - 4.0, oy - 4.0, cw + 8.0, ch + 8.0, 2.0, Color::from_rgba(255, 220, 80, 255));

        // Grid + modules.
        for y in 0..self.blueprint.canvas_h {
            for x in 0..self.blueprint.canvas_w {
                let sx = ox + x as f32 * CELL_PX;
                let sy = oy + (self.blueprint.canvas_h - 1 - y) as f32 * CELL_PX;
                draw_rectangle_lines(sx, sy, CELL_PX, CELL_PX, 1.0, Color::from_rgba(70, 70, 80, 255));
            }
        }
        // Ground / deep line at y=0 (bottom). For plankton, the canvas top
        // is the water surface — spawn anchors the tallest module there.
        let ground_sy = oy + (self.blueprint.canvas_h - 1) as f32 * CELL_PX + CELL_PX;
        draw_line(ox, ground_sy, ox + cw, ground_sy, 2.0, Color::from_rgba(160, 120, 80, 255));
        draw_text(
            "bottom = ground/deep",
            ox,
            ground_sy + 14.0,
            12.0,
            Color::from_rgba(160, 120, 80, 200),
        );
        draw_text(
            "top = surface/sky (algae stay in water)",
            ox,
            oy - 6.0,
            12.0,
            Color::from_rgba(120, 180, 220, 220),
        );

        for m in &self.blueprint.modules {
            let sx = ox + m.x as f32 * CELL_PX;
            let sy = oy + (self.blueprint.canvas_h as i16 - 1 - m.y) as f32 * CELL_PX;
            let (r, g, b) = m.module.rgb();
            draw_rectangle(sx + 1.0, sy + 1.0, CELL_PX - 2.0, CELL_PX - 2.0, Color::from_rgba(r, g, b, 255));
        }

        // Side panel.
        let px = ox + cw + 24.0;
        draw_text("Creature editor (plants + fungi)", px, oy, 22.0, WHITE);
        draw_text(
            &format!("Tool: {:?}  Brush: {}", self.tool, self.brush.name()),
            px,
            oy + 28.0,
            16.0,
            LIGHTGRAY,
        );
        draw_text(
            "1 Nuc  2 Photo  3 Root  4 Stem  5 Digest  6 Hypha",
            px,
            oy + 52.0,
            14.0,
            GRAY,
        );
        draw_text(
            "E erase  F fungus template  S save  L load  Enter spawn",
            px,
            oy + 72.0,
            14.0,
            GRAY,
        );
        let habit = habit_label(&self.blueprint);
        let spawn_ok = blueprint_spawnable(&self.blueprint);
        draw_text(
            &format!(
                "modules={}  {}  spawn={}",
                self.blueprint.modules.len(),
                habit,
                spawn_ok
            ),
            px,
            oy + 100.0,
            14.0,
            WHITE,
        );
        draw_text(&self.status, px, oy + 130.0, 14.0, YELLOW);

        // Palette swatches (Set D plants + Set E fungi).
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

        if self.spawn_picker {
            draw_text(
                "SPAWN MODE — click a column in the world",
                40.0,
                sh - 40.0,
                20.0,
                GREEN,
            );
        }
    }
}

pub enum EditorAction {
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_sim::Genome;

    #[test]
    fn fungus_blueprint_is_editor_spawnable() {
        let bp = Blueprint::minimal_fungus(Genome::default());
        assert!(blueprint_spawnable(&bp));
        assert_eq!(habit_label(&bp), "fungus");
        assert!(brush_paintable(ModuleId::Digest));
        assert!(brush_paintable(ModuleId::Hypha));
    }

    #[test]
    fn atom_still_spawnable() {
        let bp = Blueprint::atom(Genome::default());
        assert!(blueprint_spawnable(&bp));
        assert_eq!(habit_label(&bp), "plankton");
    }
}
