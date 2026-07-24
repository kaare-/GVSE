//! Quit confirmation dialog for wk-voxel-app.
//!
//! Isolation: macroquad only. Save is performed by the main loop.

use macroquad::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitChoice {
    /// Persist then exit.
    SaveAndQuit,
    /// Exit without writing a save.
    QuitWithoutSave,
    /// Dismiss the dialog; keep running.
    Cancel,
}

pub struct QuitDialog {
    pub open: bool,
    /// Shown under the prompt (e.g. `saves/world.gvsesim`).
    pub save_hint: String,
    pub status: String,
}

struct DialogLayout {
    panel: (f32, f32, f32, f32),
    save: (f32, f32, f32, f32),
    quit: (f32, f32, f32, f32),
    cancel: (f32, f32, f32, f32),
}

impl Default for QuitDialog {
    fn default() -> Self {
        Self {
            open: false,
            save_hint: "saves/world.gvsesim".into(),
            status: String::new(),
        }
    }
}

impl QuitDialog {
    pub fn open_with_slot(&mut self, slot: &str) {
        self.open = true;
        self.save_hint = format!("saves/{slot}.gvsesim");
        self.status.clear();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.status.clear();
    }

    /// Keyboard + click handling. Returns a choice when the user decides.
    pub fn handle_input(&mut self) -> Option<QuitChoice> {
        if !self.open {
            return None;
        }
        if is_key_pressed(KeyCode::Escape) || is_key_pressed(KeyCode::N) {
            return Some(QuitChoice::Cancel);
        }
        if is_key_pressed(KeyCode::Y) || is_key_pressed(KeyCode::S) || is_key_pressed(KeyCode::Enter)
        {
            return Some(QuitChoice::SaveAndQuit);
        }
        if is_key_pressed(KeyCode::Q) {
            return Some(QuitChoice::QuitWithoutSave);
        }

        if is_mouse_button_pressed(MouseButton::Left) {
            let (mx, my) = mouse_position();
            let layout = Self::layout();
            if Self::hit(mx, my, layout.save) {
                return Some(QuitChoice::SaveAndQuit);
            }
            if Self::hit(mx, my, layout.quit) {
                return Some(QuitChoice::QuitWithoutSave);
            }
            if Self::hit(mx, my, layout.cancel) {
                return Some(QuitChoice::Cancel);
            }
        }
        None
    }

    pub fn draw(&self) {
        if !self.open {
            return;
        }
        let sw = screen_width();
        let sh = screen_height();
        draw_rectangle(0.0, 0.0, sw, sh, Color::from_rgba(0, 0, 0, 160));

        let layout = Self::layout();
        let (px, py, pw, ph) = layout.panel;
        draw_rectangle(px, py, pw, ph, Color::from_rgba(18, 22, 30, 245));
        draw_rectangle_lines(px, py, pw, ph, 2.0, Color::from_rgba(160, 210, 255, 255));

        draw_text(
            "Quit simulation?",
            px + 24.0,
            py + 40.0,
            28.0,
            Color::from_rgba(255, 230, 160, 255),
        );
        draw_text(
            "Do you want to save the project before quitting?",
            px + 24.0,
            py + 72.0,
            18.0,
            WHITE,
        );
        draw_text(
            &format!("Slot: {}", self.save_hint),
            px + 24.0,
            py + 98.0,
            15.0,
            LIGHTGRAY,
        );
        if !self.status.is_empty() {
            draw_text(&self.status, px + 24.0, py + 122.0, 14.0, YELLOW);
        }

        Self::draw_button(layout.save, "Save & quit  (Y / Enter)", Color::from_rgba(40, 110, 70, 255));
        Self::draw_button(
            layout.quit,
            "Quit without saving  (Q)",
            Color::from_rgba(120, 50, 50, 255),
        );
        Self::draw_button(layout.cancel, "Cancel  (Esc / N)", Color::from_rgba(50, 58, 74, 255));
    }

    fn layout() -> DialogLayout {
        let sw = screen_width();
        let sh = screen_height();
        let pw = 520.0_f32.min(sw - 40.0).max(360.0);
        let ph = 290.0_f32;
        let px = (sw - pw) * 0.5;
        let py = (sh - ph) * 0.5;
        let btn_w = pw - 48.0;
        let btn_h = 36.0;
        let bx = px + 24.0;
        DialogLayout {
            panel: (px, py, pw, ph),
            save: (bx, py + 140.0, btn_w, btn_h),
            quit: (bx, py + 186.0, btn_w, btn_h),
            cancel: (bx, py + 232.0, btn_w, btn_h),
        }
    }

    fn hit(mx: f32, my: f32, r: (f32, f32, f32, f32)) -> bool {
        mx >= r.0 && mx < r.0 + r.2 && my >= r.1 && my < r.1 + r.3
    }

    fn draw_button(r: (f32, f32, f32, f32), label: &str, fill: Color) {
        draw_rectangle(r.0, r.1, r.2, r.3, fill);
        draw_rectangle_lines(r.0, r.1, r.2, r.3, 1.5, Color::from_rgba(220, 220, 230, 200));
        draw_text(label, r.0 + 14.0, r.1 + 24.0, 18.0, WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_starts_closed() {
        assert!(!QuitDialog::default().open);
    }
}
