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
    /// Solid stone ("rock" in the UI).
    Rock,
    /// Clay — best stand-in for packed soil.
    Clay,
    /// Biological litter / compost.
    Organic,
    Limestone,
    Bedrock,
}

impl TerrainBrush {
    pub const ALL: [TerrainBrush; 12] = [
        TerrainBrush::Erase,
        TerrainBrush::Water,
        TerrainBrush::Ice,
        TerrainBrush::Snow,
        TerrainBrush::Sand,
        TerrainBrush::Gravel,
        TerrainBrush::LooseRock,
        TerrainBrush::Rock,
        TerrainBrush::Clay,
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
            TerrainBrush::Rock => "Rock",
            TerrainBrush::Clay => "Clay (soil)",
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
            TerrainBrush::Limestone | TerrainBrush::Bedrock => None,
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
            TerrainBrush::Rock => Cell::solid(MaterialId::Stone),
            TerrainBrush::Clay => Cell::solid(MaterialId::Clay),
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
        }
    }
}

impl TerrainEditor {
    pub fn toggle(&mut self, currently_paused: bool) {
        if self.open {
            self.open = false;
        } else {
            self.open = true;
            self.was_paused = currently_paused;
            self.status = "Paint world · 0–9 brushes · E erase · S/L save/load".into();
        }
    }

    /// True if screen point lies over the left tool panel.
    pub fn hits_panel(&self, mx: f32, my: f32) -> bool {
        self.open && mx < PANEL_W && my < screen_height()
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
            self.radius = (self.radius + 1).min(6);
        }
        if is_key_pressed(KeyCode::Minus) || is_key_pressed(KeyCode::KpSubtract) {
            self.radius = (self.radius - 1).max(0);
        }
        if is_key_pressed(KeyCode::S) && !is_key_down(KeyCode::LeftControl) {
            self.request_save = true;
        }
        if is_key_pressed(KeyCode::L) {
            self.request_load = true;
        }

        // Click swatches in the panel.
        if is_mouse_button_pressed(MouseButton::Left) {
            let (mx, my) = mouse_position();
            if let Some(b) = self.swatch_at(mx, my) {
                self.brush = b;
                self.tool = if b == TerrainBrush::Erase {
                    TerrainTool::Erase
                } else {
                    TerrainTool::Paint
                };
            }
        }
    }

    fn swatch_origin() -> (f32, f32) {
        (PANEL_PAD, 110.0)
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
        let r = self.radius.max(0);
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                world.set_cell(gx + dx, gy + dy, cell);
            }
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
                "Radius {}  (+/-)  ·  LMB paint  RMB erase",
                self.radius
            ),
            PANEL_PAD,
            92.0,
            13.0,
            LIGHTGRAY,
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
                if *brush == self.brush { WHITE } else { LIGHTGRAY },
            );
        }

        let foot_y = oy + 6.0 * (SWATCH + SWATCH_GAP + 16.0) + 12.0;
        draw_text("S save  L load  → saves/*.gvsesim", PANEL_PAD, foot_y, 13.0, GRAY);
        draw_text(
            &format!("slot: {}", self.save_name),
            PANEL_PAD,
            foot_y + 20.0,
            13.0,
            LIGHTGRAY,
        );
        draw_text(&self.status, PANEL_PAD, foot_y + 44.0, 13.0, YELLOW);
        draw_text(
            "Also: Tab → World size · F5/F9 save/load",
            PANEL_PAD,
            foot_y + 68.0,
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
}
