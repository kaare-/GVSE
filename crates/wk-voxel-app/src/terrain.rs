//! F3 terrain editor for wk-voxel-app.
//!
//! Isolation: wk-voxel + wk-material + macroquad only.
//!
//! Toggle with F3. Paint / erase every block type while the world stays
//! visible underneath. Free water is `Air + sat = FULL` (not solid Water).

use macroquad::prelude::*;
use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::{Cell, World};

/// Panel that steals clicks so world paint doesn't fire under UI.
const PANEL_W: f32 = 300.0;
const PANEL_PAD: f32 = 12.0;
const SWATCH: f32 = 28.0;
const SWATCH_GAP: f32 = 6.0;
/// Max brush radius in cells (diameter = 2·r+1).
pub const MAX_BRUSH_RADIUS: i32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainTool {
    Paint,
    Erase,
}

/// Brush that becomes a [`Cell`] when painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainBrush {
    /// Dry Air — also what Erase writes.
    Erase,
    /// Standing / free water (`Air` + full saturation).
    Water,
    Ice,
    Snow,
    Sand,
    Gravel,
    LooseRock,
    /// Broken limestone rubble (collapse / shear debris).
    LooseLimestone,
    /// Solid stone ("rock" in the UI).
    Rock,
    /// Clay — cohesive mineral fines (dry powder / plastic / mud by wetness).
    Clay,
    /// Humified soil (fungal compost end-product).
    Soil,
    /// Biological litter / compost.
    Organic,
    Limestone,
    Bedrock,
}

impl TerrainBrush {
    pub const ALL: [TerrainBrush; 14] = [
        TerrainBrush::Erase,
        TerrainBrush::Water,
        TerrainBrush::Ice,
        TerrainBrush::Snow,
        TerrainBrush::Sand,
        TerrainBrush::Gravel,
        TerrainBrush::LooseRock,
        TerrainBrush::LooseLimestone,
        TerrainBrush::Rock,
        TerrainBrush::Clay,
        TerrainBrush::Soil,
        TerrainBrush::Organic,
        TerrainBrush::Limestone,
        TerrainBrush::Bedrock,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TerrainBrush::Erase => "Erase (Air)",
            TerrainBrush::Water => "Water",
            TerrainBrush::Ice => "Ice",
            TerrainBrush::Snow => "Snow",
            TerrainBrush::Sand => "Sand",
            TerrainBrush::Gravel => "Gravel",
            TerrainBrush::LooseRock => "Loose rock",
            TerrainBrush::LooseLimestone => "Loose limestone",
            TerrainBrush::Rock => "Rock",
            TerrainBrush::Clay => "Clay",
            TerrainBrush::Soil => "Soil",
            TerrainBrush::Organic => "Biological",
            TerrainBrush::Limestone => "Limestone",
            TerrainBrush::Bedrock => "Bedrock",
        }
    }

    pub fn hotkey(self) -> Option<&'static str> {
        match self {
            TerrainBrush::Erase => Some("0"),
            TerrainBrush::Water => Some("1"),
            TerrainBrush::Ice => Some("2"),
            TerrainBrush::Snow => Some("3"),
            TerrainBrush::Sand => Some("4"),
            TerrainBrush::Gravel => Some("5"),
            TerrainBrush::LooseRock => Some("6"),
            TerrainBrush::Rock => Some("7"),
            TerrainBrush::Clay => Some("8"),
            TerrainBrush::Organic => Some("9"),
            TerrainBrush::LooseLimestone
            | TerrainBrush::Soil
            | TerrainBrush::Limestone
            | TerrainBrush::Bedrock => None,
        }
    }

    pub fn to_cell(self) -> Cell {
        match self {
            TerrainBrush::Erase => Cell::air(),
            TerrainBrush::Water => Cell::water(),
            TerrainBrush::Ice => Cell::solid(MaterialId::Ice),
            TerrainBrush::Snow => Cell::solid(MaterialId::Snow),
            TerrainBrush::Sand => Cell::solid(MaterialId::Sand),
            TerrainBrush::Gravel => Cell::solid(MaterialId::Gravel),
            TerrainBrush::LooseRock => Cell::solid(MaterialId::LooseRock),
            TerrainBrush::LooseLimestone => Cell::solid(MaterialId::LooseLimestone),
            TerrainBrush::Rock => Cell::solid(MaterialId::Stone),
            TerrainBrush::Clay => Cell::solid(MaterialId::Clay),
            TerrainBrush::Soil => Cell::solid(MaterialId::Soil),
            TerrainBrush::Organic => Cell::solid(MaterialId::Organic),
            TerrainBrush::Limestone => Cell::solid(MaterialId::Limestone),
            TerrainBrush::Bedrock => Cell::solid(MaterialId::Bedrock),
        }
    }

    pub fn swatch_rgb(self) -> [u8; 3] {
        match self {
            TerrainBrush::Erase => [40, 44, 56],
            TerrainBrush::Water => MaterialRegistry::colour_rgb(MaterialId::Water),
            other => {
                let c = other.to_cell();
                MaterialRegistry::colour_rgb(c.material)
            }
        }
    }
}

pub struct TerrainEditor {
    pub open: bool,
    pub tool: TerrainTool,
    pub brush: TerrainBrush,
    /// Extra cells beyond the clicked cell (0 = single cell).
    pub radius: i32,
    pub status: String,
    pub was_paused: bool,
    pub save_name: String,
    /// Set by UI / hotkeys; main loop consumes.
    pub request_save: bool,
    pub request_load: bool,
    /// True while dragging the brush-size slider.
    dragging_size: bool,
}

impl Default for TerrainEditor {
    fn default() -> Self {
        Self {
            open: false,
            tool: TerrainTool::Paint,
            brush: TerrainBrush::Sand,
            radius: 0,
            status: "F3 terrain — click world to paint".into(),
            was_paused: true,
            save_name: "world".into(),
            request_save: false,
            request_load: false,
            dragging_size: false,
        }
    }
}

impl TerrainEditor {
    pub fn toggle(&mut self, currently_paused: bool) {
        if self.open {
            self.open = false;
            self.dragging_size = false;
        } else {
            self.open = true;
            self.was_paused = currently_paused;
            self.status = "Paint world · size slider · 0–9 brushes · S/L save".into();
        }
    }

    /// Brush diameter in cells (`2·radius + 1`).
    pub fn brush_diameter(&self) -> i32 {
        self.radius.max(0) * 2 + 1
    }

    /// True if screen point lies over the left tool panel.
    pub fn hits_panel(&self, mx: f32, my: f32) -> bool {
        self.open && mx < PANEL_W && my < screen_height()
    }

    /// True while dragging the size slider (don't paint the world).
    pub fn blocks_world_paint(&self) -> bool {
        self.dragging_size
    }

    fn size_slider_rect() -> (f32, f32, f32, f32) {
        // x, y, w, h of the interactive track.
        let x = PANEL_PAD + 36.0;
        let y = 118.0;
        let w = PANEL_W - PANEL_PAD * 2.0 - 72.0;
        let h = 22.0;
        (x, y, w, h)
    }

    fn size_minus_rect() -> (f32, f32, f32, f32) {
        (PANEL_PAD, 112.0, 28.0, 28.0)
    }

    fn size_plus_rect() -> (f32, f32, f32, f32) {
        (PANEL_W - PANEL_PAD - 28.0, 112.0, 28.0, 28.0)
    }

    fn hit_rect(mx: f32, my: f32, r: (f32, f32, f32, f32)) -> bool {
        mx >= r.0 && mx < r.0 + r.2 && my >= r.1 && my < r.1 + r.3
    }

    fn set_radius_from_slider_x(&mut self, mx: f32) {
        let (x, _, w, _) = Self::size_slider_rect();
        let t = ((mx - x) / w).clamp(0.0, 1.0);
        self.radius = (t * MAX_BRUSH_RADIUS as f32).round() as i32;
    }

    pub fn handle_input(&mut self) {
        if !self.open {
            return;
        }
        if is_key_pressed(KeyCode::Key0) {
            self.brush = TerrainBrush::Erase;
            self.tool = TerrainTool::Erase;
        }
        if is_key_pressed(KeyCode::Key1) {
            self.brush = TerrainBrush::Water;
            self.tool = TerrainTool::Paint;
        }
        if is_key_pressed(KeyCode::Key2) {
            self.brush = TerrainBrush::Ice;
            self.tool = TerrainTool::Paint;
        }
        if is_key_pressed(KeyCode::Key3) {
            self.brush = TerrainBrush::Snow;
            self.tool = TerrainTool::Paint;
        }
        if is_key_pressed(KeyCode::Key4) {
            self.brush = TerrainBrush::Sand;
            self.tool = TerrainTool::Paint;
        }
        if is_key_pressed(KeyCode::Key5) {
            self.brush = TerrainBrush::Gravel;
            self.tool = TerrainTool::Paint;
        }
        if is_key_pressed(KeyCode::Key6) {
            self.brush = TerrainBrush::LooseRock;
            self.tool = TerrainTool::Paint;
        }
        if is_key_pressed(KeyCode::Key7) {
            self.brush = TerrainBrush::Rock;
            self.tool = TerrainTool::Paint;
        }
        if is_key_pressed(KeyCode::Key8) {
            self.brush = TerrainBrush::Clay;
            self.tool = TerrainTool::Paint;
        }
        if is_key_pressed(KeyCode::Key9) {
            self.brush = TerrainBrush::Organic;
            self.tool = TerrainTool::Paint;
        }
        if is_key_pressed(KeyCode::E) {
            self.tool = TerrainTool::Erase;
            self.brush = TerrainBrush::Erase;
        }
        if is_key_pressed(KeyCode::P) {
            self.tool = TerrainTool::Paint;
            if self.brush == TerrainBrush::Erase {
                self.brush = TerrainBrush::Sand;
            }
        }
        if is_key_pressed(KeyCode::Equal) || is_key_pressed(KeyCode::KpAdd) {
            self.radius = (self.radius + 1).min(MAX_BRUSH_RADIUS);
        }
        if is_key_pressed(KeyCode::Minus) || is_key_pressed(KeyCode::KpSubtract) {
            self.radius = (self.radius - 1).max(0);
        }
        // Bracket keys also nudge size (common in paint apps).
        if is_key_pressed(KeyCode::RightBracket) {
            self.radius = (self.radius + 1).min(MAX_BRUSH_RADIUS);
        }
        if is_key_pressed(KeyCode::LeftBracket) {
            self.radius = (self.radius - 1).max(0);
        }
        if is_key_pressed(KeyCode::S) && !is_key_down(KeyCode::LeftControl) {
            self.request_save = true;
        }
        if is_key_pressed(KeyCode::L) {
            self.request_load = true;
        }

        let (mx, my) = mouse_position();
        if is_mouse_button_pressed(MouseButton::Left) {
            if Self::hit_rect(mx, my, Self::size_minus_rect()) {
                self.radius = (self.radius - 1).max(0);
            } else if Self::hit_rect(mx, my, Self::size_plus_rect()) {
                self.radius = (self.radius + 1).min(MAX_BRUSH_RADIUS);
            } else if Self::hit_rect(mx, my, Self::size_slider_rect()) {
                self.dragging_size = true;
                self.set_radius_from_slider_x(mx);
            } else if let Some(b) = self.swatch_at(mx, my) {
                self.brush = b;
                self.tool = if b == TerrainBrush::Erase {
                    TerrainTool::Erase
                } else {
                    TerrainTool::Paint
                };
            }
        }
        if self.dragging_size {
            if is_mouse_button_down(MouseButton::Left) {
                self.set_radius_from_slider_x(mx);
            } else {
                self.dragging_size = false;
            }
        }
    }

    fn swatch_origin() -> (f32, f32) {
        // Below the brush-size slider + preview.
        (PANEL_PAD, 168.0)
    }

    fn swatch_at(&self, mx: f32, my: f32) -> Option<TerrainBrush> {
        let (ox, oy) = Self::swatch_origin();
        let cols = 2;
        for (i, brush) in TerrainBrush::ALL.iter().enumerate() {
            let col = (i % cols) as f32;
            let row = (i / cols) as f32;
            let x = ox + col * (PANEL_W - PANEL_PAD * 2.0) * 0.5;
            let y = oy + row * (SWATCH + SWATCH_GAP + 16.0);
            let w = (PANEL_W - PANEL_PAD * 2.0) * 0.5 - 4.0;
            let h = SWATCH + 14.0;
            if mx >= x && mx < x + w && my >= y && my < y + h {
                return Some(*brush);
            }
        }
        None
    }

    /// Paint or erase a disk of cells around `(gx, gy)`.
    pub fn apply_at(&self, world: &mut World, gx: i32, gy: i32) {
        let cell = match self.tool {
            TerrainTool::Erase => Cell::air(),
            TerrainTool::Paint => self.brush.to_cell(),
        };
        let r = self.radius.clamp(0, MAX_BRUSH_RADIUS);
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                world.set_cell(gx + dx, gy + dy, cell);
            }
        }
        if matches!(self.tool, TerrainTool::Erase) {
            wk_voxel::wake_floating_competent(world);
        }
    }

    pub fn draw(&self) {
        if !self.open {
            return;
        }
        let sh = screen_height();
        draw_rectangle(0.0, 0.0, PANEL_W, sh, Color::from_rgba(12, 14, 20, 230));
        draw_rectangle(
            PANEL_W - 2.0,
            0.0,
            2.0,
            sh,
            Color::from_rgba(90, 140, 200, 255),
        );
        draw_text(
            "TERRAIN EDITOR",
            PANEL_PAD,
            28.0,
            22.0,
            Color::from_rgba(160, 210, 255, 255),
        );
        draw_text("(F3 close)", PANEL_PAD, 48.0, 14.0, GRAY);
        draw_text(
            &format!(
                "Tool: {:?}  Brush: {}",
                self.tool,
                self.brush.label()
            ),
            PANEL_PAD,
            72.0,
            14.0,
            WHITE,
        );
        draw_text(
            &format!(
                "Paint size: {}×{} cells  ([ ] or +/-)",
                self.brush_diameter(),
                self.brush_diameter()
            ),
            PANEL_PAD,
            94.0,
            13.0,
            LIGHTGRAY,
        );

        // − / slider / +
        let minus = Self::size_minus_rect();
        let plus = Self::size_plus_rect();
        let (sx, sy, sw, sh_track) = Self::size_slider_rect();
        draw_rectangle(
            minus.0,
            minus.1,
            minus.2,
            minus.3,
            Color::from_rgba(40, 48, 64, 255),
        );
        draw_text("−", minus.0 + 8.0, minus.1 + 20.0, 22.0, WHITE);
        draw_rectangle(
            plus.0,
            plus.1,
            plus.2,
            plus.3,
            Color::from_rgba(40, 48, 64, 255),
        );
        draw_text("+", plus.0 + 7.0, plus.1 + 20.0, 22.0, WHITE);

        draw_rectangle(
            sx,
            sy + sh_track * 0.35,
            sw,
            sh_track * 0.30,
            Color::from_rgba(50, 58, 74, 255),
        );
        let t = self.radius as f32 / MAX_BRUSH_RADIUS as f32;
        let thumb_x = sx + t * sw;
        draw_circle(
            thumb_x,
            sy + sh_track * 0.5,
            9.0,
            Color::from_rgba(160, 210, 255, 255),
        );
        // Filled portion of the track.
        draw_rectangle(
            sx,
            sy + sh_track * 0.35,
            (thumb_x - sx).max(0.0),
            sh_track * 0.30,
            Color::from_rgba(90, 140, 200, 200),
        );

        // Tiny brush preview (disk).
        let prev_cx = PANEL_W * 0.5;
        let prev_cy = 152.0;
        let prev_r = (2.0 + self.radius as f32 * 1.1).min(14.0);
        let [pr, pg, pb] = self.brush.swatch_rgb();
        draw_circle(
            prev_cx,
            prev_cy,
            prev_r,
            Color::from_rgba(pr, pg, pb, 220),
        );
        draw_circle_lines(
            prev_cx,
            prev_cy,
            prev_r,
            1.0,
            Color::from_rgba(255, 255, 255, 180),
        );

        let (ox, oy) = Self::swatch_origin();
        let cols = 2;
        for (i, brush) in TerrainBrush::ALL.iter().enumerate() {
            let col = (i % cols) as f32;
            let row = (i / cols) as f32;
            let x = ox + col * (PANEL_W - PANEL_PAD * 2.0) * 0.5;
            let y = oy + row * (SWATCH + SWATCH_GAP + 16.0);
            let [r, g, b] = brush.swatch_rgb();
            draw_rectangle(x, y, SWATCH, SWATCH, Color::from_rgba(r, g, b, 255));
            if *brush == self.brush {
                draw_rectangle_lines(x, y, SWATCH, SWATCH, 2.0, WHITE);
            }
            let key = brush.hotkey().unwrap_or("");
            let label = if key.is_empty() {
                brush.label().to_string()
            } else {
                format!("{key} {}", brush.label())
            };
            draw_text(
                &label,
                x + SWATCH + 6.0,
                y + SWATCH * 0.7,
                13.0,
                if *brush == self.brush {
                    WHITE
                } else {
                    LIGHTGRAY
                },
            );
        }

        let foot_y = oy + 6.0 * (SWATCH + SWATCH_GAP + 16.0) + 12.0;
        draw_text(
            "LMB paint  RMB erase  ·  drag size slider",
            PANEL_PAD,
            foot_y,
            13.0,
            GRAY,
        );
        draw_text("S save  L load  → saves/*.gvsesim", PANEL_PAD, foot_y + 18.0, 13.0, GRAY);
        draw_text(
            &format!("slot: {}", self.save_name),
            PANEL_PAD,
            foot_y + 38.0,
            13.0,
            LIGHTGRAY,
        );
        draw_text(&self.status, PANEL_PAD, foot_y + 60.0, 13.0, YELLOW);
        draw_text(
            "Also: Tab → World size · F5/F9 save/load",
            PANEL_PAD,
            foot_y + 84.0,
            12.0,
            GRAY,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_voxel::Sat;

    #[test]
    fn water_brush_is_wet_air_not_solid_water() {
        let c = TerrainBrush::Water.to_cell();
        assert_eq!(c.material, MaterialId::Air);
        assert_eq!(c.sat, Sat::FULL);
    }

    #[test]
    fn rock_brush_is_stone() {
        assert_eq!(
            TerrainBrush::Rock.to_cell().material,
            MaterialId::Stone
        );
    }

    #[test]
    fn paint_disk_writes_cells() {
        let mut w = World::new(1);
        let ed = TerrainEditor {
            brush: TerrainBrush::Sand,
            tool: TerrainTool::Paint,
            radius: 1,
            ..TerrainEditor::default()
        };
        ed.apply_at(&mut w, 10, 10);
        assert_eq!(
            w.get_cell(10, 10).map(|c| c.material),
            Some(MaterialId::Sand)
        );
        assert_eq!(
            w.get_cell(11, 10).map(|c| c.material),
            Some(MaterialId::Sand)
        );
    }

    #[test]
    fn brush_diameter_matches_radius() {
        let mut ed = TerrainEditor::default();
        assert_eq!(ed.brush_diameter(), 1);
        ed.radius = 3;
        assert_eq!(ed.brush_diameter(), 7);
        ed.radius = MAX_BRUSH_RADIUS;
        assert_eq!(ed.brush_diameter(), MAX_BRUSH_RADIUS * 2 + 1);
    }

    #[test]
    fn large_brush_fills_disk() {
        let mut w = World::new(1);
        let ed = TerrainEditor {
            brush: TerrainBrush::Clay,
            tool: TerrainTool::Paint,
            radius: 4,
            ..TerrainEditor::default()
        };
        ed.apply_at(&mut w, 20, 20);
        assert_eq!(
            w.get_cell(20, 20).map(|c| c.material),
            Some(MaterialId::Clay)
        );
        assert_eq!(
            w.get_cell(24, 20).map(|c| c.material),
            Some(MaterialId::Clay)
        );
        // Corner of bounding box is outside the disk.
        assert_ne!(
            w.get_cell(24, 24).map(|c| c.material),
            Some(MaterialId::Clay)
        );
    }
}
