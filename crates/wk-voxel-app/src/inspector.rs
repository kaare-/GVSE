//! Click-to-inspect panel for world cells and Set A organisms.
//! Isolation: wk-voxel + wk-material only (no column-stack imports).

use macroquad::prelude::*;
use wk_material::MaterialId;
use wk_voxel::{Atom, Cell, Humidity};

fn material_name(mat: MaterialId) -> &'static str {
    match mat {
        MaterialId::Bedrock => "bedrock",
        MaterialId::Stone => "stone",
        MaterialId::Limestone => "limestone",
        MaterialId::LooseRock => "looserock",
        MaterialId::Gravel => "gravel",
        MaterialId::Sand => "sand",
        MaterialId::Clay => "clay",
        MaterialId::Organic => "organic",
        MaterialId::Water => "water",
        MaterialId::Air => "air",
        MaterialId::Snow => "snow",
        MaterialId::Ice => "ice",
    }
}

/// Screen → world cell under the cursor (accounts for camera + ring wrap).
pub fn screen_to_world(
    mx: f32,
    my: f32,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    width_cols: i32,
    bedrock_floor_y: i32,
    sky_ceiling_y: i32,
    wrap_x: bool,
) -> Option<(i32, i32)> {
    if cell_px <= 0.0 {
        return None;
    }
    let gx_raw = ((mx - origin_x) / cell_px).floor() as i32;
    let gy = bedrock_floor_y + ((origin_y - my) / cell_px).floor() as i32;
    if gy < bedrock_floor_y || gy >= sky_ceiling_y {
        return None;
    }
    let gx = if wrap_x {
        if width_cols <= 0 {
            return None;
        }
        gx_raw.rem_euclid(width_cols)
    } else if gx_raw >= 0 && gx_raw < width_cols {
        gx_raw
    } else {
        return None;
    };
    Some((gx, gy))
}

pub fn draw_block_inspector(
    gx: i32,
    gy: i32,
    cell: Option<Cell>,
    humidity: &Humidity,
    organism: Option<(usize, &Atom)>,
    sw: f32,
) {
    let hum = humidity.at_cell(gx, gy);
    let (hx, hy) = humidity.tile_of(gx, gy);
    let mut lines = vec![format!("Block ({gx}, {gy})")];
    match cell {
        Some(c) => {
            let kind = if c.material == MaterialId::Air && c.sat.is_full() {
                "water (Air+FULL sat)"
            } else if c.material == MaterialId::Air && !c.sat.is_empty() {
                "wet air"
            } else {
                material_name(c.material)
            };
            lines.push(format!("material={kind}"));
            lines.push(format!(
                "sat={}/{} ({:.0}%)",
                c.sat.0,
                u8::MAX,
                c.sat.as_f32() * 100.0
            ));
            lines.push(format!("flags=0x{:02X}", c.flags.0));
        }
        None => lines.push("cell: (empty / unstamped)".into()),
    }
    lines.push(format!("humidity={hum:.1}  tile=({hx},{hy})"));

    if let Some((id, atom)) = organism {
        lines.push("--- organism ---".into());
        lines.push(format!("Atom #{id}  anchor=({}, {})", atom.gx, atom.gy));
        lines.push(format!(
            "energy={:.1}/{:.0}  age={}  mods={}",
            atom.energy,
            atom.energy_max,
            atom.age_ticks,
            atom.body.len()
        ));
        lines.push(format!(
            "photosystems={}  cooldown={}",
            atom.photosystem_count(),
            atom.cooldown
        ));
        lines.push(format!(
            "buoyancy={:.2}  vel_y={:.2}  fy={:.1}",
            atom.buoyancy_bias, atom.vel_y, atom.fy
        ));
        lines.push(format!(
            "clone_fid={:.2}  circadian={:.2}/{:.2}",
            atom.clone_fidelity, atom.circadian_phase, atom.active_window
        ));
    }

    let panel_w = 280.0;
    let panel_h = 16.0 + lines.len() as f32 * 15.0 + 10.0;
    let x0 = sw - panel_w - 8.0;
    let y0 = 10.0;
    draw_rectangle(x0, y0, panel_w, panel_h, Color::from_rgba(0, 0, 0, 220));
    draw_rectangle_lines(
        x0,
        y0,
        panel_w,
        panel_h,
        1.0,
        Color::from_rgba(200, 200, 200, 255),
    );
    for (i, line) in lines.iter().enumerate() {
        draw_text(line, x0 + 8.0, y0 + 16.0 + i as f32 * 15.0, 14.0, WHITE);
    }
}

pub fn draw_selection_outline(
    gx: i32,
    gy: i32,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    width_cols: i32,
    wrap_x: bool,
    sw: f32,
    sh: f32,
) {
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    for &x_copy in x_copies {
        let sx = origin_x + (gx + x_copy * width_cols) as f32 * cell_px;
        let sy = origin_y - (gy - bedrock_floor_y) as f32 * cell_px;
        if sx + cell_px < 0.0 || sx > sw || sy < 0.0 || sy - cell_px > sh {
            continue;
        }
        draw_rectangle_lines(
            sx,
            sy - cell_px,
            cell_px,
            cell_px,
            1.5,
            Color::from_rgba(255, 220, 80, 255),
        );
    }
}
