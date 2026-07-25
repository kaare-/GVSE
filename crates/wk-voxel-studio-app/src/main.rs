//! Muscle / Bone / Neural Test Studio — pixel arena UI.
//!
//! Isolation: wk-voxel + wk-voxel-studio + wk-material only.
//! See docs/organism/STUDIO.md.
//!
//! Left dock: clickable tissue + geology palette.
//! Space pause · Enter activate · W flood/drain · R reset
//! N net/scripted · H hill-climb · C continuous train · E export
//! F1–F4 physics · F5 fin · F6 terrain · F7 vertical arm · F8 quarter-gate arm

use macroquad::prelude::*;
use std::path::PathBuf;
use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::Cell;
use wk_voxel_studio::{
    encode_body, export_body_with_net, evolve_morphology, force_sensors_bridging_parts,
    paint_fin_bench, paint_rough_terrain, paint_vertical_arm, tissue_rgb, ArenaConfig,
    JointLimit, StudioArena, StudioPhysicsConfig, TissueKind, TrainingSession, ARENA_MAX,
    ARENA_MIN, JOINT_RGB,
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
        label: "Joint full (cyan)",
    },
    TissueEntry {
        kind: TissueKind::JointThreeQuarter,
        label: "Joint 3/4",
    },
    TissueEntry {
        kind: TissueKind::JointHalf,
        label: "Joint 1/2 (cyan)",
    },
    TissueEntry {
        kind: TissueKind::JointQuarter,
        label: "Joint 1/4",
    },
    TissueEntry {
        kind: TissueKind::ForceSensor,
        label: "Force sense (blue)",
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

    let foot = screen_height() - 88.0;
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
        "W water  R reset  N net",
        10.0,
        foot + 32.0,
        13.0,
        Color::from_rgba(160, 168, 180, 255),
    );
    draw_text(
        "H train  C cont  E export",
        10.0,
        foot + 48.0,
        13.0,
        Color::from_rgba(160, 168, 180, 255),
    );

    pick
}

/// Snapshot paint + physics into a fresh arena for episode evaluation.
fn clone_bench(arena: &StudioArena) -> StudioArena {
    let mut a = StudioArena::new(ArenaConfig {
        width: arena.cfg.width,
        height: arena.cfg.height,
        seed: arena.cfg.seed,
        water_to_y: arena.cfg.water_to_y,
    });
    a.physics = arena.physics;
    a.body.paint = arena.body.paint.clone();
    a.body.net = arena.body.net.clone();
    a
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
    let mut train: Option<TrainingSession> = None;
    let mut continuous = false;
    let mut train_seed = 0x71A1_u64;

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
            status = "example: fin + nerve + neuron (Enter)".into();
        }
        if is_key_pressed(KeyCode::F6) {
            paint_rough_terrain(&mut arena);
            status = "example: rough terrain".into();
        }
        if is_key_pressed(KeyCode::F7) {
            paint_vertical_arm(&mut arena, JointLimit::Half);
            status = "example: vertical arm JointHalf + antagonists (Enter)".into();
        }
        if is_key_pressed(KeyCode::F8) {
            paint_vertical_arm(&mut arena, JointLimit::Quarter);
            status = "example: vertical arm JointQuarter gate (Enter)".into();
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
            let fs_bridge = force_sensors_bridging_parts(&arena.body.paint);
            match arena.activate() {
                Ok(g) => {
                    let bones = g.bone_count();
                    let mus = g.muscles.len();
                    let joints = g.joints.len();
                    let hinged = g.hinged_bone_count();
                    let nerves = g.nerves.len();
                    let ctrl = g.has_controller;
                    let drive = if arena.physics.scripted_muscle {
                        "scripted"
                    } else {
                        "neural"
                    };
                    let mut msg = format!(
                        "active bones={bones} joints={joints} hinged={hinged} mus={mus} nerves={nerves} ctrl={ctrl} drive={drive}"
                    );
                    if joints == 0 && fs_bridge > 0 {
                        msg.push_str(" | need cyan Joint between the two bones (ForceSense on fixture is fine)");
                    } else if joints == 0 {
                        msg.push_str(" | no joint — cyan Joint* between the two bones");
                    } else if mus == 0 {
                        msg.push_str(" | no muscle link — red must touch both bones or the joint");
                    } else if hinged == 0 && bones >= 2 {
                        msg.push_str(" | tip: fixture must touch the root bone");
                    } else if ctrl && arena.physics.scripted_muscle {
                        msg.push_str(" | net ready — flapping scripted; N=neural H=train");
                    }
                    status = msg;
                    paused = false;
                    train = None;
                    continuous = false;
                }
                Err(_) => status = "activate failed — paint bone/fixture first".into(),
            }
        }

        // Toggle neural vs scripted muscle drive.
        if is_key_pressed(KeyCode::N) {
            if !arena.body.activated {
                status = "activate first (Enter)".into();
            } else if arena.ensure_net(train_seed).is_none() {
                status = "need muscles for a net".into();
            } else {
                arena.physics.scripted_muscle = !arena.physics.scripted_muscle;
                status = if arena.physics.scripted_muscle {
                    "drive: scripted sinusoid".into()
                } else {
                    "drive: StudioNet (muscle feedback)".into()
                };
            }
        }

        // Burst hill-climb on current morphology.
        if is_key_pressed(KeyCode::H) {
            if !arena.body.activated {
                status = "activate first (Enter)".into();
            } else if arena
                .body
                .graph
                .as_ref()
                .map(|g| g.muscles.is_empty())
                .unwrap_or(true)
            {
                let fs = force_sensors_bridging_parts(&arena.body.paint);
                let joints = arena
                    .body
                    .graph
                    .as_ref()
                    .map(|g| g.joints.len())
                    .unwrap_or(0);
                status = if joints == 0 && fs > 0 {
                    "H: no muscles — replace blue ForceSense with cyan Joint 1/2, then muscle".into()
                } else if joints == 0 {
                    "H: no muscles — need cyan Joint + red Muscle spanning the hinge".into()
                } else {
                    "H: no muscle link — paint Muscle touching both bones or the joint".into()
                };
            } else if arena.ensure_net(train_seed).is_none() {
                status = "need muscles to train".into();
            } else {
                let paint = arena.body.paint.clone();
                let cfg = arena.cfg;
                let phys = arena.physics;
                let seed = train_seed;
                train_seed = train_seed.wrapping_add(1);
                let make = || {
                    let mut a = StudioArena::new(cfg);
                    a.physics = phys;
                    a.physics.scripted_muscle = false;
                    a.body.paint = paint.clone();
                    a
                };
                let (net, best) = wk_voxel_studio::hill_climb(make, 12, 48, seed);
                arena.physics.scripted_muscle = false;
                arena.body.net = Some(net.clone());
                let _ = arena.activate();
                arena.body.net = Some(net.clone());
                continuous = false;
                train = TrainingSession::new(&arena, seed, 48);
                if let Some(s) = train.as_mut() {
                    s.best_net = net;
                    s.best = best.clone();
                    s.generation = 12;
                }
                status = format!(
                    "hill-climb fit={:.2} travel={:.1} T={:.2} gen=12",
                    best.fitness, best.bone_travel, best.mean_tension
                );
                paused = false;
            }
        }

        // Morphology GA burst (heavier).
        if is_key_pressed(KeyCode::M) {
            if arena.body.paint.cells.iter().all(|k| *k == TissueKind::Empty) {
                status = "paint a body first (or F5)".into();
            } else {
                let paint = arena.body.paint.clone();
                let cfg = arena.cfg;
                let phys = arena.physics;
                let seed = train_seed;
                train_seed = train_seed.wrapping_add(17);
                let make = || {
                    let mut a = StudioArena::new(cfg);
                    a.physics = phys;
                    a.body.paint = paint.clone();
                    a
                };
                let (best, hist) = evolve_morphology(make, 4, 3, 32, seed);
                let hist_s = hist
                    .iter()
                    .map(|v| format!("{v:.1}"))
                    .collect::<Vec<_>>()
                    .join(",");
                arena.body.paint = best.paint;
                arena.physics.scripted_muscle = false;
                arena.body.net = Some(best.net.clone());
                let _ = arena.activate();
                arena.body.net = Some(best.net);
                continuous = false;
                train = None;
                status = format!("GA morph fit={:.2} hist=[{hist_s}]", best.fitness);
                paused = false;
            }
        }

        if is_key_pressed(KeyCode::C) {
            if !arena.body.activated {
                status = "activate first (Enter)".into();
            } else if arena.ensure_net(train_seed).is_none() {
                status = "need muscles to train".into();
            } else {
                continuous = !continuous;
                if continuous {
                    arena.physics.scripted_muscle = false;
                    train = TrainingSession::new(&arena, train_seed, 40);
                    train_seed = train_seed.wrapping_add(1);
                    status = "continuous train ON (C to stop)".into();
                    paused = true; // episodes run on clones; live arena shows best
                } else {
                    status = "continuous train OFF".into();
                }
            }
        }

        if is_key_pressed(KeyCode::E) {
            match export_body_with_net(&arena.body, arena.body.net.clone()) {
                Ok(exp) => match encode_body(&exp) {
                    Ok(bytes) => {
                        let path = PathBuf::from("export.gvsebody");
                        match std::fs::write(&path, &bytes) {
                            Ok(()) => {
                                status = format!(
                                    "exported {} bytes → {}",
                                    bytes.len(),
                                    path.display()
                                );
                            }
                            Err(e) => status = format!("export write failed: {e}"),
                        }
                    }
                    Err(e) => status = format!("encode failed: {e}"),
                },
                Err(_) => status = "export failed — empty body".into(),
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
            train = None;
            continuous = false;
            status = "reset".into();
            paused = true;
        }

        // Continuous regime: one generation per frame on a paint clone.
        if continuous {
            if let Some(session) = train.as_mut() {
                let bench = clone_bench(&arena);
                let paint = bench.body.paint.clone();
                let cfg = bench.cfg;
                let phys = bench.physics;
                let improved = session.step(|| {
                    let mut a = StudioArena::new(cfg);
                    a.physics = phys;
                    a.physics.scripted_muscle = false;
                    a.body.paint = paint.clone();
                    a
                });
                arena.body.net = Some(session.best_net.clone());
                arena.physics.scripted_muscle = false;
                if improved {
                    status = format!(
                        "train gen={} fit={:.2} travel={:.1}",
                        session.generation, session.best.fitness, session.best.bone_travel
                    );
                } else if session.generation % 5 == 0 {
                    status = format!(
                        "train gen={} best={:.2}",
                        session.generation, session.best.fitness
                    );
                }
            }
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

        // Always stamp active joint pivots (cyan) so hinges stay visible after Enter.
        if let Some(graph) = arena.body.graph.as_ref() {
            for (px, py) in graph.joint_world_pivots() {
                if px < 0 || py < 0 || px >= max_x || py >= max_y {
                    continue;
                }
                let sx = DOCK_W + px as f32 * CELL_PX;
                let sy = screen_height() - (py as f32 + 1.0) * CELL_PX;
                draw_rectangle(sx, sy, CELL_PX, CELL_PX, rgb_color(JOINT_RGB));
                // Tiny cross so hinges read even at 3px.
                draw_rectangle(
                    sx + CELL_PX * 0.35,
                    sy,
                    CELL_PX * 0.3,
                    CELL_PX,
                    Color::from_rgba(10, 40, 50, 220),
                );
                draw_rectangle(
                    sx,
                    sy + CELL_PX * 0.35,
                    CELL_PX,
                    CELL_PX * 0.3,
                    Color::from_rgba(10, 40, 50, 220),
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
            PaintLayer::Terrain => format!("{terrain:?}"),
        };
        let drive = if arena.physics.scripted_muscle {
            "script"
        } else if arena.body.net.is_some() {
            "net"
        } else {
            "idle"
        };
        let fit = train
            .as_ref()
            .map(|s| format!("  fit={:.2} g={}", s.best.fitness, s.generation))
            .unwrap_or_default();
        let hinge = arena
            .body
            .graph
            .as_ref()
            .and_then(|g| {
                let d = g.joint_angle_delta(0)?;
                let lim = g.joints.first()?.limit.max_turns();
                Some(format!("  θ={:.0}°/±{:.0}°", d.to_degrees(), lim * 180.0))
            })
            .unwrap_or_default();
        let hud = format!(
            "{}x{}  tick={}  {}  {}  phys={}  {drive}{fit}{hinge}  brush={brush}  T={tension:.2}  {status}",
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
