//! Muscle / Bone / Neural Test Studio — pixel arena UI.
//!
//! Isolation: wk-voxel + wk-voxel-studio + wk-material only.
//! See docs/organism/STUDIO.md.
//!
//! Layers: `T` tissue · `G` geology (all MaterialIds)
//! Physics: `F1` body-only · `F2` dry walk · `F3` hydro fin · `F4` full
//! Size: `[` / `]` width · `;` / `'` height
//! Sim: Space · Enter activate · W water · R reset · F5 fin · F6 rough terrain

use macroquad::prelude::*;
use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::Cell;
use wk_voxel_studio::{
    paint_fin_bench, paint_rough_terrain, tissue_rgb, ArenaConfig, StudioArena, StudioPhysicsConfig,
    TissueKind, ARENA_MAX, ARENA_MIN,
};

const CELL_PX: f32 = 3.0;
const WATER_FILM: [u8; 3] = [0xB8, 0xD4, 0xEE];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaintLayer {
    Tissue,
    Terrain,
}

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

fn tissue_from_digit(d: u8) -> Option<TissueKind> {
    Some(match d {
        1 => TissueKind::Bone,
        2 => TissueKind::Muscle,
        3 => TissueKind::Skin,
        4 => TissueKind::Nerve,
        5 => TissueKind::NeuronBlob,
        6 => TissueKind::Fixture,
        7 => TissueKind::ForceSensor,
        8 => TissueKind::JointFull,
        9 => TissueKind::JointThreeQuarter,
        0 => TissueKind::JointHalf,
        _ => return None,
    })
}

const TERRAIN_BRUSHES: [MaterialId; 10] = [
    MaterialId::Sand,
    MaterialId::Stone,
    MaterialId::Gravel,
    MaterialId::LooseRock,
    MaterialId::Clay,
    MaterialId::Limestone,
    MaterialId::Organic,
    MaterialId::Ice,
    MaterialId::Snow,
    MaterialId::Water,
];

fn overlay_rgb(arena: &StudioArena, x: i32, y: i32) -> Option<[u8; 3]> {
    if arena.body.activated {
        if let Some(graph) = arena.body.graph.as_ref() {
            if let Some(k) = graph.kind_at(x, y) {
                return tissue_rgb(k);
            }
        }
        let paint_k = arena.body.paint.get(x as u32, y as u32);
        if matches!(
            paint_k,
            TissueKind::Bone | TissueKind::Fixture | TissueKind::Muscle | TissueKind::Empty
        ) {
            return None;
        }
        return tissue_rgb(paint_k);
    }
    tissue_rgb(arena.body.paint.get(x as u32, y as u32))
}

fn preset_name(p: &StudioPhysicsConfig) -> &'static str {
    if !p.ca_enabled {
        "body_only"
    } else if p.water_flow && !p.grain && !p.failure {
        "hydro_fin"
    } else if p.grain && !p.water_flow {
        "dry_walk"
    } else if p.failure && p.seepage {
        "full"
    } else {
        "custom"
    }
}

fn digit_pressed() -> Option<u8> {
    if is_key_pressed(KeyCode::Key1) {
        Some(1)
    } else if is_key_pressed(KeyCode::Key2) {
        Some(2)
    } else if is_key_pressed(KeyCode::Key3) {
        Some(3)
    } else if is_key_pressed(KeyCode::Key4) {
        Some(4)
    } else if is_key_pressed(KeyCode::Key5) {
        Some(5)
    } else if is_key_pressed(KeyCode::Key6) {
        Some(6)
    } else if is_key_pressed(KeyCode::Key7) {
        Some(7)
    } else if is_key_pressed(KeyCode::Key8) {
        Some(8)
    } else if is_key_pressed(KeyCode::Key9) {
        Some(9)
    } else if is_key_pressed(KeyCode::Key0) {
        Some(0)
    } else {
        None
    }
}

#[macroquad::main("GVSE Studio — muscle / bone / neural")]
async fn main() {
    let mut arena = StudioArena::new(ArenaConfig::default());
    let mut paused = true;
    let mut layer = PaintLayer::Tissue;
    let mut tissue = TissueKind::Bone;
    let mut terrain_idx = 0usize;
    let mut water_on = arena.cfg.water_to_y.is_some();
    let mut status = String::from("T tissue · G geology · F1–F4 physics · Enter activate");

    loop {
        if is_key_pressed(KeyCode::Escape) {
            break;
        }
        if is_key_pressed(KeyCode::Space) {
            paused = !paused;
        }
        if is_key_pressed(KeyCode::T) {
            layer = PaintLayer::Tissue;
        }
        if is_key_pressed(KeyCode::G) {
            layer = PaintLayer::Terrain;
        }
        if is_key_pressed(KeyCode::F1) {
            arena.physics = StudioPhysicsConfig::body_only();
            status = "physics: body_only".into();
        }
        if is_key_pressed(KeyCode::F2) {
            arena.physics = StudioPhysicsConfig::dry_walk();
            status = "physics: dry_walk".into();
        }
        if is_key_pressed(KeyCode::F3) {
            arena.physics = StudioPhysicsConfig::hydro_fin();
            status = "physics: hydro_fin".into();
        }
        if is_key_pressed(KeyCode::F4) {
            arena.physics = StudioPhysicsConfig::full();
            status = "physics: full".into();
        }
        if is_key_pressed(KeyCode::F5) {
            paint_fin_bench(&mut arena);
            status = "loaded fin bench tissue".into();
        }
        if is_key_pressed(KeyCode::F6) {
            paint_rough_terrain(&mut arena);
            status = "painted rough terrain".into();
        }
        if is_key_pressed(KeyCode::Minus) && layer == PaintLayer::Tissue {
            tissue = TissueKind::JointQuarter;
        }
        if let Some(d) = digit_pressed() {
            match layer {
                PaintLayer::Tissue => {
                    if let Some(b) = tissue_from_digit(d) {
                        tissue = b;
                    }
                }
                PaintLayer::Terrain => {
                    terrain_idx = if d == 0 { 9 } else { (d as usize - 1).min(9) };
                }
            }
        }

        if is_key_pressed(KeyCode::LeftBracket) {
            arena.resize((arena.cfg.width - 32).max(ARENA_MIN), arena.cfg.height);
            status = format!("size {}x{}", arena.cfg.width, arena.cfg.height);
        }
        if is_key_pressed(KeyCode::RightBracket) {
            arena.resize((arena.cfg.width + 32).min(ARENA_MAX), arena.cfg.height);
            status = format!("size {}x{}", arena.cfg.width, arena.cfg.height);
        }
        if is_key_pressed(KeyCode::Semicolon) {
            arena.resize(arena.cfg.width, (arena.cfg.height - 32).max(ARENA_MIN));
            status = format!("size {}x{}", arena.cfg.width, arena.cfg.height);
        }
        if is_key_pressed(KeyCode::Apostrophe) {
            arena.resize(arena.cfg.width, (arena.cfg.height + 32).min(ARENA_MAX));
            status = format!("size {}x{}", arena.cfg.width, arena.cfg.height);
        }

        if is_key_pressed(KeyCode::Enter) {
            match arena.activate() {
                Ok(g) => {
                    status = format!(
                        "active bones={} muscles={} hung={}",
                        g.bone_count(),
                        g.muscles.len(),
                        g.anchored_bone_count()
                    );
                    paused = false;
                }
                Err(_) => status = "activate failed — need bone/fixture".into(),
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
            let phys = arena.physics;
            let w = arena.cfg.width;
            let h = arena.cfg.height;
            arena = StudioArena::new(ArenaConfig {
                width: w,
                height: h,
                water_to_y: if water_on { Some(h / 2) } else { None },
                ..ArenaConfig::default()
            });
            arena.physics = phys;
            status = "reset".into();
            paused = true;
        }

        let (mx, my) = mouse_position();
        let gx = (mx / CELL_PX).floor() as i32;
        let gy = ((screen_height() - my) / CELL_PX).floor() as i32;
        if gx >= 0 && gy >= 0 && gx < arena.cfg.width && gy < arena.cfg.height {
            if is_mouse_button_down(MouseButton::Left) {
                match layer {
                    PaintLayer::Tissue => arena.body.paint_set(gx as u32, gy as u32, tissue),
                    PaintLayer::Terrain => {
                        arena.paint_terrain(gx, gy, TERRAIN_BRUSHES[terrain_idx]);
                    }
                }
            } else if is_mouse_button_down(MouseButton::Right) {
                match layer {
                    PaintLayer::Tissue => {
                        arena
                            .body
                            .paint_set(gx as u32, gy as u32, TissueKind::Empty);
                    }
                    PaintLayer::Terrain => arena.paint_terrain(gx, gy, MaterialId::Air),
                }
            }
        }

        if !paused {
            arena.tick();
        }

        clear_background(Color::from_rgba(0x1A, 0x1E, 0x24, 255));
        let max_x = ((screen_width() / CELL_PX) as i32).min(arena.cfg.width);
        let max_y = ((screen_height() / CELL_PX) as i32).min(arena.cfg.height);
        for y in 0..max_y {
            for x in 0..max_x {
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

        let tension = arena
            .body
            .graph
            .as_ref()
            .map(|g| g.mean_tension())
            .unwrap_or(0.0);
        let brush = match layer {
            PaintLayer::Tissue => format!("{tissue:?}"),
            PaintLayer::Terrain => format!("{:?}", TERRAIN_BRUSHES[terrain_idx]),
        };
        let hud = format!(
            "STUDIO  {}x{}  tick={}  {}  {}  phys={}  layer={layer:?}\n\
             brush={brush}  muscleT={tension:.2}  {status}\n\
             F1 body  F2 dry_walk  F3 hydro  F4 full  F5 fin  F6 rough  [/] ;/' size",
            arena.cfg.width,
            arena.cfg.height,
            arena.world.tick,
            if paused { "PAUSED" } else { "RUN" },
            if arena.body.activated {
                "ACTIVE"
            } else {
                "PAINT"
            },
            preset_name(&arena.physics),
        );
        draw_text(&hud, 8.0, 16.0, 14.0, WHITE);

        next_frame().await;
    }
}
