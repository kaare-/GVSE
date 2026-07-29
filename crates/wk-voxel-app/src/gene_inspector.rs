//! Gene Inspector / Body Plan / Mutation Preview panels for the creature studio.
//! Wave K: every painted pixel is a gene; aggregates form the body plan.

use macroquad::prelude::*;
use wk_voxel::{Blueprint, ModuleId, PixelTraits};

/// Traits shown for a given module kind (hide the rest until wired).
fn visible_traits(module: ModuleId) -> &'static [&'static str] {
    match module {
        ModuleId::Bone => &["mass", "density", "stiffness", "upkeep_bias"],
        ModuleId::Muscle => &["mass", "density", "strength", "upkeep_bias"],
        ModuleId::Skin => &["mass", "density", "upkeep_bias", "buoyancy_bias"],
        ModuleId::Photosystem => &["mass", "absorb_bias", "upkeep_bias"],
        ModuleId::Root => &["mass", "drink_bias", "upkeep_bias"],
        ModuleId::Nucleus => &[
            "mass",
            "upkeep_bias",
            "clone_fidelity_bias",
            "reproduce_at_bias",
        ],
        ModuleId::Digest | ModuleId::Hypha | ModuleId::Stem => {
            &["mass", "density", "upkeep_bias"]
        }
    }
}

fn trait_get(t: &PixelTraits, name: &str) -> f32 {
    match name {
        "mass" => t.mass,
        "density" => t.density,
        "stiffness" => t.stiffness,
        "strength" => t.strength,
        "upkeep_bias" => t.upkeep_bias,
        "absorb_bias" => t.absorb_bias,
        "drink_bias" => t.drink_bias,
        "clone_fidelity_bias" => t.clone_fidelity_bias,
        "reproduce_at_bias" => t.reproduce_at_bias,
        "buoyancy_bias" => t.buoyancy_bias,
        _ => 0.0,
    }
}

fn trait_set(t: &mut PixelTraits, name: &str, v: f32) {
    let v = v.clamp(0.0, 4.0);
    match name {
        "mass" => t.mass = v,
        "density" => t.density = v,
        "stiffness" => t.stiffness = v,
        "strength" => t.strength = v,
        "upkeep_bias" => t.upkeep_bias = v,
        "absorb_bias" => t.absorb_bias = v,
        "drink_bias" => t.drink_bias = v,
        "clone_fidelity_bias" => t.clone_fidelity_bias = v.clamp(0.05, 1.0),
        "reproduce_at_bias" => t.reproduce_at_bias = v.clamp(0.05, 1.0),
        "buoyancy_bias" => t.buoyancy_bias = v.clamp(0.0, 1.0),
        _ => {}
    }
}

/// Draw Gene Inspector + Body Plan + Mutation Preview to the right of the canvas.
///
/// Returns `true` if a slider click mutated a trait this frame.
pub fn draw_gene_panels(
    blueprint: &mut Blueprint,
    selected: Option<usize>,
    preview_child: &Option<Blueprint>,
    origin_x: f32,
    origin_y: f32,
) -> GenePanelAction {
    let mut action = GenePanelAction::default();
    let px = origin_x;
    let mut y = origin_y;

    draw_text("GENE INSPECTOR", px, y, 18.0, Color::from_rgba(255, 220, 80, 255));
    y += 22.0;

    if let Some(i) = selected {
        if let Some(m) = blueprint.modules.get_mut(i) {
            draw_text(
                &format!("#{} {}", i, m.module.name()),
                px,
                y,
                14.0,
                WHITE,
            );
            y += 18.0;
            let names = visible_traits(m.module);
            for name in names {
                let val = trait_get(&m.traits, name);
                draw_text(
                    &format!("{name}: {val:.2}"),
                    px,
                    y,
                    13.0,
                    LIGHTGRAY,
                );
                // Click-drag strip: left = decrease, right = increase.
                let bar_x = px + 160.0;
                let bar_w = 120.0;
                let bar_y = y - 10.0;
                draw_rectangle(bar_x, bar_y, bar_w, 12.0, Color::from_rgba(40, 44, 56, 255));
                let fill = (val / 4.0).clamp(0.0, 1.0) * bar_w;
                draw_rectangle(
                    bar_x,
                    bar_y,
                    fill,
                    12.0,
                    Color::from_rgba(120, 180, 220, 255),
                );
                if is_mouse_button_down(MouseButton::Left) {
                    let (mx, my) = mouse_position();
                    if mx >= bar_x && mx <= bar_x + bar_w && my >= bar_y && my <= bar_y + 12.0 {
                        let t = ((mx - bar_x) / bar_w).clamp(0.0, 1.0) * 4.0;
                        trait_set(&mut m.traits, name, t);
                        action.traits_changed = true;
                    }
                }
                y += 16.0;
            }
        }
    } else {
        draw_text("Click a painted pixel to inspect", px, y, 13.0, GRAY);
        y += 18.0;
    }

    y += 12.0;
    draw_text("BODY PLAN", px, y, 18.0, Color::from_rgba(255, 220, 80, 255));
    y += 20.0;
    let plan = blueprint.body_plan();
    let lines = [
        format!("pixels={}", plan.pixel_count),
        format!("total_mass={:.2}", plan.total_mass),
        format!("metabolic={:.2}", plan.metabolic_rate),
        format!("clone_fidelity={:.2}", plan.clone_fidelity),
        format!("reproduce_at={:.2}", plan.reproduce_at),
        format!("photo_cap={:.2}", plan.photo_capacity),
        format!(
            "repro_gate={} (nuclei={})",
            plan.has_repro_gate, plan.nucleus_count
        ),
    ];
    for line in lines {
        draw_text(&line, px, y, 13.0, WHITE);
        y += 15.0;
    }

    y += 10.0;
    draw_text("MUTATION PREVIEW", px, y, 18.0, Color::from_rgba(255, 220, 80, 255));
    y += 20.0;

    // Roll button
    let btn = Rect {
        x: px,
        y: y - 12.0,
        w: 140.0,
        h: 22.0,
    };
    draw_rectangle(btn.x, btn.y, btn.w, btn.h, Color::from_rgba(60, 90, 70, 255));
    draw_text("Roll child (seed=0)", px + 6.0, y + 4.0, 13.0, WHITE);
    if is_mouse_button_pressed(MouseButton::Left) {
        let (mx, my) = mouse_position();
        if mx >= btn.x && mx <= btn.x + btn.w && my >= btn.y && my <= btn.y + btn.h {
            action.roll_preview = true;
        }
    }
    y += 28.0;

    if let Some(child) = preview_child {
        let parent_plan = blueprint.body_plan();
        let child_plan = child.body_plan();
        let d_pix = child_plan.pixel_count as i32 - parent_plan.pixel_count as i32;
        let d_mass = child_plan.total_mass - parent_plan.total_mass;
        let d_meta = child_plan.metabolic_rate - parent_plan.metabolic_rate;
        draw_text(
            &format!("Δpixels={d_pix:+}  Δmass={d_mass:+.2}  Δmeta={d_meta:+.2}"),
            px,
            y,
            13.0,
            Color::from_rgba(180, 220, 160, 255),
        );
        y += 18.0;
        // Half-size child glyph strip.
        let cell = 8.0;
        for m in &child.modules {
            let (r, g, b) = m.module.rgb();
            draw_rectangle(
                px + m.x as f32 * cell,
                y + (16.0 - m.y as f32) * cell,
                cell - 1.0,
                cell - 1.0,
                Color::from_rgba(r, g, b, 220),
            );
        }
    } else {
        draw_text("No preview yet — click Roll", px, y, 13.0, GRAY);
    }

    action
}

#[derive(Debug, Default)]
pub struct GenePanelAction {
    pub traits_changed: bool,
    pub roll_preview: bool,
}
