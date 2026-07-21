//! Click-to-inspect panel for world cells and organisms.
//! Isolation: wk-voxel + wk-material only (no column-stack imports).

use macroquad::prelude::*;
use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::{
    is_fungus, is_land_plant, soft_litter_at, water_capacity, Atom, Cell, Humidity, Temperature,
    World,
};

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
    temperature: &Temperature,
    world: &World,
    organism: Option<(usize, &Atom)>,
    sw: f32,
) {
    let hum = humidity.at_cell(gx, gy);
    let temp_c = temperature.at_cell(gx, gy);
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
            // Pore fill is relative to material capacity (porosity for
            // solids, 255 for Air) — not always /255. Stone at sat=20
            // with porosity 20 is fully saturated, not "8% wet".
            let cap = water_capacity(c.material);
            let pct = if cap > 0 {
                (c.sat.0 as f32 / cap as f32) * 100.0
            } else {
                0.0
            };
            lines.push(format!("sat={}/{cap} ({pct:.0}% of capacity)", c.sat.0));
            let props = MaterialRegistry::props(c.material);
            lines.push(format!(
                "porosity={}  permeability={}",
                props.porosity, props.permeability
            ));
            lines.push(format!("flags=0x{:02X}", c.flags.0));
        }
        None => lines.push("cell: (empty / unstamped)".into()),
    }
    lines.push(format!("temp={temp_c:.1}C  humidity={hum:.1}  tile=({hx},{hy})"));
    let litter = soft_litter_at(world, gx);
    if litter > 0 {
        lines.push(format!("soft_litter={litter}"));
    }

    if let Some((id, atom)) = organism {
        lines.push("--- organism ---".into());
        let kind = if is_fungus(atom) {
            "Fungus"
        } else if is_land_plant(atom) {
            "Plant"
        } else {
            "Atom"
        };
        lines.push(format!("{kind} #{id}  anchor=({}, {})", atom.gx, atom.gy));
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
        if is_fungus(atom) {
            lines.push("habit=fungus (digest litter / Organic)".into());
            lines.push(format!(
                "digest_rate={:.2}  drought_ticks={}",
                atom.genome.digest_rate, atom.drought_ticks
            ));
            let digests = atom
                .body
                .iter()
                .filter(|(_, _, m)| *m == wk_voxel::ModuleId::Digest)
                .count();
            let hyphae = atom
                .body
                .iter()
                .filter(|(_, _, m)| *m == wk_voxel::ModuleId::Hypha)
                .count();
            lines.push(format!("digest={digests}  hypha={hyphae}"));
        } else if is_land_plant(atom) {
            let (s, l, r) = atom.genome.alloc_weights();
            lines.push("habit=land (fixed crown, root drink)".into());
            lines.push(format!(
                "alloc S/L/R={s:.2}/{l:.2}/{r:.2}  depth={:.2}",
                atom.genome.root_depth_bias
            ));
            lines.push(format!(
                "leaf_abs={:.2}  shade_eff={:.2}",
                atom.genome.leaf_absorb, atom.genome.shade_efficiency
            ));
            let roots = atom
                .body
                .iter()
                .filter(|(_, _, m)| *m == wk_voxel::ModuleId::Root)
                .count();
            let stems = atom
                .body
                .iter()
                .filter(|(_, _, m)| *m == wk_voxel::ModuleId::Stem)
                .count();
            lines.push(format!("roots={roots}  stems={stems}"));
        } else {
            lines.push(format!(
                "buoyancy={:.2}  vel_y={:.2}  fy={:.1}",
                atom.buoyancy_bias, atom.vel_y, atom.fy
            ));
        }
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
