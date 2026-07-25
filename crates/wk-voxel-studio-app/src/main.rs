//! Muscle / Bone / Neural Test Studio — pixel arena UI.
//!
//! Isolation: wk-voxel + wk-voxel-studio + wk-material only.
//! See docs/organism/STUDIO.md.
//!
//! Controls (S1):
//! - `Space` — pause / resume (CA + body)
//! - `Enter` — activate paint → body graph (bones hang on fixtures)
//! - `1`–`7` — brush: bone, muscle, skin, nerve, neuron, fixture, sensor
//! - `8`–`0` / `-` — joints (full / 3/4 / 1/2 / 1/4)
//! - LMB paint, RMB erase (invalidates activate)
//! - `W` — toggle water fill
//! - `R` — reset arena
//! - `Esc` — quit

use macroquad::prelude::*;
use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::Cell;
use wk_voxel_studio::{tissue_rgb, ArenaConfig, StudioArena, TissueKind};

const CELL_PX: f32 = 4.0;
const WATER_FILM: [u8; 3] = [0xB8, 0xD4, 0xEE];

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

fn cell_color(cell: Cell) -> [u8; 3] {
    let base = MaterialRegistry::colour_rgb(cell.material);
    let t = cell.sat.as_f32();
    if cell.material == MaterialId::Air {
        if cell.sat.is_empty() {
            return base;
        }
        let water = MaterialRegistry::colour_rgb(MaterialId::Water);
        let blend = if t >= 0.55 {
            (0.55 + (t - 0.55) * 1.8).clamp(0.75, 1.0)
        } else {
            (t / 0.55) * 0.75
        };
        [
            lerp_u8(WATER_FILM[0], water[0], blend),
            lerp_u8(WATER_FILM[1], water[1], blend),
            lerp_u8(WATER_FILM[2], water[2], blend),
        ]
    } else {
        base
    }
}

fn brush_from_key() -> Option<TissueKind> {
    if is_key_pressed(KeyCode::Key1) {
        Some(TissueKind::Bone)
    } else if is_key_pressed(KeyCode::Key2) {
        Some(TissueKind::Muscle)
    } else if is_key_pressed(KeyCode::Key3) {
        Some(TissueKind::Skin)
    } else if is_key_pressed(KeyCode::Key4) {
        Some(TissueKind::Nerve)
    } else if is_key_pressed(KeyCode::Key5) {
        Some(TissueKind::NeuronBlob)
    } else if is_key_pressed(KeyCode::Key6) {
        Some(TissueKind::Fixture)
    } else if is_key_pressed(KeyCode::Key7) {
        Some(TissueKind::ForceSensor)
    } else if is_key_pressed(KeyCode::Key8) {
        Some(TissueKind::JointFull)
    } else if is_key_pressed(KeyCode::Key9) {
        Some(TissueKind::JointThreeQuarter)
    } else if is_key_pressed(KeyCode::Key0) {
        Some(TissueKind::JointHalf)
    } else if is_key_pressed(KeyCode::Minus) {
        Some(TissueKind::JointQuarter)
    } else {
        None
    }
}

fn brush_label(k: TissueKind) -> &'static str {
    match k {
        TissueKind::Empty => "erase",
        TissueKind::Bone => "bone",
        TissueKind::Muscle => "muscle",
        TissueKind::Skin => "skin",
        TissueKind::Nerve => "nerve",
        TissueKind::NeuronBlob => "neuron blob",
        TissueKind::Fixture => "fixture",
        TissueKind::ForceSensor => "force sensor",
        TissueKind::JointFull => "joint full",
        TissueKind::JointThreeQuarter => "joint 3/4",
        TissueKind::JointHalf => "joint 1/2",
        TissueKind::JointQuarter => "joint 1/4",
    }
}

/// Overlay colour: live body graph for bone/fixture when activated,
/// otherwise paint (and non-rigid paint kinds always from paint).
fn overlay_rgb(arena: &StudioArena, x: i32, y: i32) -> Option<[u8; 3]> {
    if arena.body.activated {
        if let Some(graph) = arena.body.graph.as_ref() {
            if let Some(k) = graph.kind_at(x, y) {
                return tissue_rgb(k);
            }
        }
        let paint_k = arena.body.paint.get(x as u32, y as u32);
        // Hide rest-pose bone/fixture (moved into graph); keep other paint.
        if matches!(paint_k, TissueKind::Bone | TissueKind::Fixture | TissueKind::Empty) {
            return None;
        }
        return tissue_rgb(paint_k);
    }
    tissue_rgb(arena.body.paint.get(x as u32, y as u32))
}

#[macroquad::main("GVSE Studio — muscle / bone / neural")]
async fn main() {
    let mut arena = StudioArena::new(ArenaConfig::default());
    let mut paused = true;
    let mut brush = TissueKind::Bone;
    let mut water_on = arena.cfg.water_to_y.is_some();
    let mut status = String::from("paint a fixture + bone, then Enter");

    loop {
        if is_key_pressed(KeyCode::Escape) {
            break;
        }
        if is_key_pressed(KeyCode::Space) {
            paused = !paused;
        }
        if let Some(b) = brush_from_key() {
            brush = b;
        }
        if is_key_pressed(KeyCode::Enter) {
            match arena.activate() {
                Ok(g) => {
                    status = format!(
                        "activated: {} fixture parts, {} bones ({} hung)",
                        g.fixture_count(),
                        g.bone_count(),
                        g.anchored_bone_count()
                    );
                    paused = false;
                }
                Err(_) => status = "activate failed — paint bone or fixture first".into(),
            }
        }
        if is_key_pressed(KeyCode::W) {
            water_on = !water_on;
            arena.set_water_to(if water_on {
                Some(arena.cfg.height / 2)
            } else {
                None
            });
        }
        if is_key_pressed(KeyCode::R) {
            arena = StudioArena::new(ArenaConfig {
                water_to_y: if water_on {
                    Some(arena.cfg.height / 2)
                } else {
                    None
                },
                ..ArenaConfig::default()
            });
            status = "reset — paint, then Enter".into();
            paused = true;
        }

        let (mx, my) = mouse_position();
        let gx = (mx / CELL_PX).floor() as i32;
        let gy = ((screen_height() - my) / CELL_PX).floor() as i32;
        if gx >= 0 && gy >= 0 && gx < arena.cfg.width && gy < arena.cfg.height {
            if is_mouse_button_down(MouseButton::Left) {
                arena.body.paint_set(gx as u32, gy as u32, brush);
            } else if is_mouse_button_down(MouseButton::Right) {
                arena.body.paint_set(gx as u32, gy as u32, TissueKind::Empty);
            }
        }

        if !paused {
            arena.tick();
        }

        clear_background(Color::from_rgba(0x1A, 0x1E, 0x24, 255));
        for y in 0..arena.cfg.height {
            for x in 0..arena.cfg.width {
                let Some(cell) = arena.world.get_cell(x, y) else {
                    continue;
                };
                let mut rgb = cell_color(cell);
                if let Some(tr) = overlay_rgb(&arena, x, y) {
                    rgb = tr;
                }
                let sx = x as f32 * CELL_PX;
                let sy = screen_height() - (y as f32 + 1.0) * CELL_PX;
                draw_rectangle(
                    sx,
                    sy,
                    CELL_PX,
                    CELL_PX,
                    Color::from_rgba(rgb[0], rgb[1], rgb[2], 255),
                );
            }
        }

        let mode = if arena.body.activated {
            "ACTIVE"
        } else {
            "PAINT"
        };
        let hud = format!(
            "STUDIO S1  tick={}  {}  {}  brush={}\n\
             [Enter] activate  [Space] pause  [W] water  [R] reset\n\
             1 bone  2 muscle  3 skin  4 nerve  5 neuron  6 fixture  7 sensor  8–0/- joints\n\
             {status}",
            arena.world.tick,
            if paused { "PAUSED" } else { "RUN" },
            mode,
            brush_label(brush),
        );
        draw_text(&hud, 8.0, 18.0, 16.0, WHITE);

        let swatches = [
            TissueKind::Bone,
            TissueKind::Muscle,
            TissueKind::Skin,
            TissueKind::Nerve,
            TissueKind::NeuronBlob,
            TissueKind::Fixture,
        ];
        for (i, kind) in swatches.iter().enumerate() {
            let rgb = tissue_rgb(*kind).unwrap_or([0, 0, 0]);
            draw_rectangle(
                8.0 + i as f32 * 28.0,
                72.0,
                24.0,
                16.0,
                Color::from_rgba(rgb[0], rgb[1], rgb[2], 255),
            );
        }

        next_frame().await;
    }
}
