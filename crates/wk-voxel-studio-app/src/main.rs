//! Muscle / Bone / Neural Test Studio — pixel arena UI.
//!
//! Isolation: wk-voxel + wk-voxel-studio + wk-material only.
//! See docs/organism/STUDIO.md.
//!
//! Left dock: clickable tissue + geology palette.
//! Space pause · Enter activate · W flood/drain · R reset
//! F1–F4 physics · F5 fin example · F6 rough terrain · [/] ;/' size

use macroquad::prelude::*;
use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::Cell;
use wk_voxel_studio::{
    paint_fin_bench, paint_rough_terrain, tissue_rgb, ArenaConfig, StudioArena, StudioPhysicsConfig,
    TissueKind, ARENA_MAX, ARENA_MIN,
};

const CELL_PX: f32 = 3.0;
const WATER_FILM: [u8; 3] = [0xB8, 0xD4, 0xEE];
const DOCK_W: f32 = 168.0;
const SWATCH: f32 = 22.0;
const ROW_H: f32 = 26.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaintLayer {
    Tissue,
    Terrain,
}

struct TissueEntry {
    kind: TissueKind,
    label: &'static str,
}

struct TerrainEntry {
    mat: MaterialId,
    label: &'static str,
}

const TISSUE_PALETTE: &[TissueEntry] = &[
    TissueEntry {
        kind: TissueKind::Bone,
        label: "Bone",
    },
    TissueEntry {
        kind: TissueKind::Muscle,
        label: "Muscle",
    },
    TissueEntry {
        kind: TissueKind::Skin,
        label: "Skin",
    },
    TissueEntry {
        kind: TissueKind::Nerve,
        label: "Nerve",
    },
    TissueEntry {
        kind: TissueKind::NeuronBlob,
        label: "Neuron",
    },
    TissueEntry {
        kind: TissueKind::Fixture,
        label: "Fixture",
    },
    TissueEntry {
        kind: TissueKind::JointFull,
        label: "Joint full",
    },
    TissueEntry {
        kind: TissueKind::JointThreeQuarter,
        label: "Joint 3/4",
    },
    TissueEntry {
        kind: TissueKind::JointHalf,
        label: "Joint 1/2",
    },
    TissueEntry {
        kind: TissueKind::JointQuarter,
        label: "Joint 1/4",
    },
    TissueEntry {
        kind: TissueKind::ForceSensor,
        label: "Force sense",
    },
];

const TERRAIN_PALETTE: &[TerrainEntry] = &[
    TerrainEntry {
        mat: MaterialId::Sand,
        label: "Sand",
    },
    TerrainEntry {
        mat: MaterialId::Stone,
        label: "Stone",
    },
    TerrainEntry {
        mat: MaterialId::Gravel,
        label: "Gravel",
    },
    TerrainEntry {
        mat: MaterialId::LooseRock,
        label: "Loose rock",
    },
    TerrainEntry {
        mat: MaterialId::Clay,
        label: "Clay",
    },
    TerrainEntry {
        mat: MaterialId::Limestone,
        label: "Limestone",
    },
    TerrainEntry {
        mat: MaterialId::Bedrock,
        label: "Bedrock",
    },
    TerrainEntry {
        mat: MaterialId::Organic,
        label: "Organic",
    },
    TerrainEntry {
        mat: MaterialId::Ice,
        label: "Ice",
    },
    TerrainEntry {
        mat: MaterialId::Snow,
        label: "Snow",
    },
    TerrainEntry {
        mat: MaterialId::Water,
        label: "Water",
    },
    TerrainEntry {
        mat: MaterialId::Air,
        label: "Air / erase",
    },
];

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

fn rgb_color(rgb: [u8; 3]) -> Color {
    Color::from_rgba(rgb[0], rgb[1], rgb[2], 255)
}

fn point_in(mx: f32, my: f32, x: f32, y: f32, w: f32, h: f32) -> bool {
    mx >= x && mx < x + w && my >= y && my < y + h
}

struct DockPick {
    tissue: Option<TissueKind>,
    terrain: Option<MaterialId>,
    layer: Option<PaintLayer>,
}

/// Draw left dock and optionally consume a click on a swatch/tab.
fn draw_palette_dock(
    mx: f32,
    my: f32,
    click: bool,
    layer: PaintLayer,
    tissue: TissueKind,
    terrain: MaterialId,
) -> DockPick {
    draw_rectangle(0.0, 0.0, DOCK_W, screen_height(), Color::from_rgba(28, 32, 40, 255));
    draw_rectangle(
        DOCK_W - 1.0,
        0.0,
        1.0,
        screen_height(),
        Color::from_rgba(60, 66, 78, 255),
    );

    let mut pick = DockPick {
        tissue: None,
        terrain: None,
        layer: None,
    };

    let mut y = 10.0;
    draw_text("STUDIO", 12.0, y + 12.0, 18.0, WHITE);
    y += 28.0;

    let tab_w = (DOCK_W - 24.0) * 0.5;
    let tissue_tab = (10.0, y, tab_w, 22.0);
    let geo_tab = (14.0 + tab_w, y, tab_w, 22.0);
    for (x, ty, w, h, label, active, set) in [
        (
            tissue_tab.0,
            tissue_tab.1,
            tissue_tab.2,
            tissue_tab.3,
            "Tissue",
            layer == PaintLayer::Tissue,
            PaintLayer::Tissue,
        ),
        (
            geo_tab.0,
            geo_tab.1,
            geo_tab.2,
            geo_tab.3,
            "Geology",
            layer == PaintLayer::Terrain,
            PaintLayer::Terrain,
        ),
    ] {
        let bg = if active {
            Color::from_rgba(70, 90, 120, 255)
        } else {
            Color::from_rgba(40, 44, 54, 255)
        };
        draw_rectangle(x, ty, w, h, bg);
        draw_text(label, x + 8.0, ty + 15.0, 14.0, WHITE);
        if click && point_in(mx, my, x, ty, w, h) {
            pick.layer = Some(set);
        }
    }
    y += 34.0;

    match layer {
        PaintLayer::Tissue => {
            draw_text(
                "CREATURE",
                12.0,
                y + 10.0,
                13.0,
                Color::from_rgba(180, 190, 210, 255),
            );
            y += 18.0;
            for entry in TISSUE_PALETTE {
                let selected = tissue == entry.kind;
                let rgb = tissue_rgb(entry.kind).unwrap_or([80, 80, 80]);
                if selected {
                    draw_rectangle(
                        6.0,
                        y - 2.0,
                        DOCK_W - 12.0,
                        ROW_H,
                        Color::from_rgba(50, 60, 80, 255),
                    );
                }
                draw_rectangle(12.0, y + 2.0, SWATCH, SWATCH, rgb_color(rgb));
                draw_rectangle_lines(12.0, y + 2.0, SWATCH, SWATCH, 1.0, WHITE);
                draw_text(entry.label, 42.0, y + 17.0, 15.0, WHITE);
                if click && point_in(mx, my, 6.0, y - 2.0, DOCK_W - 12.0, ROW_H) {
                    pick.tissue = Some(entry.kind);
                }
                y += ROW_H;
            }
        }
        PaintLayer::Terrain => {
            draw_text(
                "WORLD MATERIALS",
                12.0,
                y + 10.0,
                13.0,
                Color::from_rgba(180, 190, 210, 255),
            );
            y += 18.0;
            for entry in TERRAIN_PALETTE {
                let selected = terrain == entry.mat;
                let rgb = if entry.mat == MaterialId::Air {
                    [40, 48, 60]
                } else {
                    MaterialRegistry::colour_rgb(entry.mat)
                };
                if selected {
                    draw_rectangle(
                        6.0,
                        y - 2.0,
                        DOCK_W - 12.0,
                        ROW_H,
                        Color::from_rgba(50, 60, 80, 255),
                    );
                }
                draw_rectangle(12.0, y + 2.0, SWATCH, SWATCH, rgb_color(rgb));
                draw_rectangle_lines(12.0, y + 2.0, SWATCH, SWATCH, 1.0, WHITE);
                draw_text(entry.label, 42.0, y + 17.0, 15.0, WHITE);
                if click && point_in(mx, my, 6.0, y - 2.0, DOCK_W - 12.0, ROW_H) {
                    pick.terrain = Some(entry.mat);
                }
                y += ROW_H;
            }
        }
    }

    let foot = screen_height() - 70.0;
    draw_text(
        "LMB paint  RMB erase",
        10.0,
        foot,
        13.0,
        Color::from_rgba(160, 168, 180, 255),
    );
    draw_text(
        "Enter activate  Space run",
        10.0,
        foot + 16.0,
        13.0,
        Color::from_rgba(160, 168, 180, 255),
    );
    draw_text(
        "W water fill  R reset",
        10.0,
        foot + 32.0,
        13.0,
        Color::from_rgba(160, 168, 180, 255),
    );

    pick
}

#[macroquad::main("GVSE Studio — muscle / bone / neural")]
async fn main() {
    let mut arena = StudioArena::new(ArenaConfig::default());
    arena.physics = StudioPhysicsConfig::dry_walk();
    let mut paused = true;
    let mut layer = PaintLayer::Tissue;
    let mut tissue = TissueKind::Bone;
    let mut terrain = MaterialId::Sand;
    let mut water_on = false;
    let mut status = String::from("dry arena — pick a brush on the left");

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
            status = "example: fin tissue (optional)".into();
        }
        if is_key_pressed(KeyCode::F6) {
            paint_rough_terrain(&mut arena);
            status = "example: rough terrain".into();
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
                Err(_) => status = "activate failed — paint bone/fixture first".into(),
            }
        }
        if is_key_pressed(KeyCode::W) {
            water_on = !water_on;
            arena.set_water_to(if water_on {
                Some(arena.cfg.height / 2)
            } else {
                None
            });
            status = if water_on {
                "water fill on".into()
            } else {
                "water drained".into()
            };
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

        if !paused {
            arena.tick();
        }

        clear_background(Color::from_rgba(0x1A, 0x1E, 0x24, 255));

        let (mx, my) = mouse_position();
        let click = is_mouse_button_pressed(MouseButton::Left);
        let pick = draw_palette_dock(mx, my, click, layer, tissue, terrain);
        if let Some(l) = pick.layer {
            layer = l;
        }
        if let Some(t) = pick.tissue {
            tissue = t;
            layer = PaintLayer::Tissue;
        }
        if let Some(g) = pick.terrain {
            terrain = g;
            layer = PaintLayer::Terrain;
        }

        let over_dock = mx < DOCK_W;
        let palette_consumed =
            pick.tissue.is_some() || pick.terrain.is_some() || pick.layer.is_some();
        let gx = ((mx - DOCK_W) / CELL_PX).floor() as i32;
        let gy = ((screen_height() - my) / CELL_PX).floor() as i32;
        if !over_dock
            && !palette_consumed
            && gx >= 0
            && gy >= 0
            && gx < arena.cfg.width
            && gy < arena.cfg.height
        {
            if is_mouse_button_down(MouseButton::Left) {
                match layer {
                    PaintLayer::Tissue => arena.body.paint_set(gx as u32, gy as u32, tissue),
                    PaintLayer::Terrain => arena.paint_terrain(gx, gy, terrain),
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

        let max_x = (((screen_width() - DOCK_W) / CELL_PX) as i32).min(arena.cfg.width);
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
                let sx = DOCK_W + x as f32 * CELL_PX;
                let sy = screen_height() - (y as f32 + 1.0) * CELL_PX;
                draw_rectangle(sx, sy, CELL_PX, CELL_PX, rgb_color(rgb));
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
            PaintLayer::Terrain => format!("{terrain:?}"),
        };
        let hud = format!(
            "{}x{}  tick={}  {}  {}  phys={}  brush={brush}  T={tension:.2}  {status}",
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
        draw_text(&hud, DOCK_W + 10.0, 18.0, 15.0, WHITE);

        next_frame().await;
    }
}
