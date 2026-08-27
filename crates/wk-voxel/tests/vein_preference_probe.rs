//! Does water prefer a high-pore vein, or does the *shading* only make it look
//! like it does not?
//!
//! Playtest, rock against rock: water appeared to choose unspeckled stone over
//! speckled stone. Permeability rises with pore, so that would be backwards.
//!
//! Two candidate explanations, and they need separating:
//!
//! - **Physics.** Water genuinely avoids high-pore cells, which would point at the
//!   head calculation equalising something other than head.
//! - **Shading.** Porosity uses a centred range on pore, so a high-pore cell has a
//!   *larger capacity*. Wet darkening draws `sat / capacity`, so a vein holding
//!   more water in a bigger container renders as a lower fraction — paler than the
//!   tight matrix beside it, while actually carrying the flow.
//!
//! This reports both quantities for the same cells, so whichever is true is
//! visible.
//!
//! ```text
//! cargo test -p wk-voxel --release --test vein_preference_probe -- --ignored --nocapture
//! ```

use wk_material::MaterialId;
use wk_voxel::{
    water_capacity_cell, Cell, ChunkCoord, PerfConfig, Sat, World,
};

/// Stone slab with a vertical high-pore vein, fed from a pool on top.
fn vein_world(vein_pore: u8, matrix_pore: u8) -> World {
    let mut w = World::new(7);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    let vein_x = 16;
    for x in 0..40 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..=60 {
            let mut c = Cell::solid(MaterialId::Stone);
            c.pore = if x == vein_x { vein_pore } else { matrix_pore };
            w.set_cell(x, y, c);
        }
    }
    // Feed the whole top so neither column is favoured by where water enters.
    for x in 0..40 {
        for y in 61..=68 {
            w.set_cell(x, y, Cell::water());
        }
    }
    w
}

fn report(label: &str, vein_pore: u8, matrix_pore: u8, ticks: u64) {
    let mut w = vein_world(vein_pore, matrix_pore);
    let perf = PerfConfig::default();
    for _ in 0..ticks {
        wk_voxel::tick_with_perf(&mut w, &perf);
    }
    let hydro = w.hydro;
    let vein_x = 16;
    let matrix_x = 10;

    let mut v_abs = 0u32;
    let mut v_cap = 0u32;
    let mut m_abs = 0u32;
    let mut m_cap = 0u32;
    for y in 1..=60 {
        let v = w.get_cell(vein_x, y).unwrap();
        let m = w.get_cell(matrix_x, y).unwrap();
        v_abs += v.sat.0 as u32;
        v_cap += water_capacity_cell(v, &hydro) as u32;
        m_abs += m.sat.0 as u32;
        m_cap += water_capacity_cell(m, &hydro) as u32;
    }
    let v_frac = v_abs as f32 / v_cap.max(1) as f32;
    let m_frac = m_abs as f32 / m_cap.max(1) as f32;

    println!("\n=== {label} (vein pore={vein_pore}, matrix pore={matrix_pore}, {ticks} ticks) ===");
    println!(
        "  vein   absolute water {v_abs:>6}   capacity {v_cap:>6}   full {:.3}",
        v_frac
    );
    println!(
        "  matrix absolute water {m_abs:>6}   capacity {m_cap:>6}   full {:.3}",
        m_frac
    );
    println!(
        "  → water prefers the {}   |   shading shows the {} as wetter",
        if v_abs > m_abs { "VEIN" } else { "MATRIX" },
        if v_frac > m_frac { "VEIN" } else { "MATRIX" }
    );
    if v_abs > m_abs && v_frac < m_frac {
        println!("  ** INVERTED: the vein carries more water but renders drier **");
    }
}

#[test]
#[ignore = "diagnostic; run with --release --ignored --nocapture"]
fn does_water_prefer_the_vein() {
    // Permeability of stone ramps only in the upper half of the pore domain, so
    // the matrix sits at matrix value and the vein well into the fracture tail.
    for ticks in [400, 2000] {
        report("high-pore vein in tight stone", 255, 100, ticks);
    }
    // Control: identical pore everywhere. Any difference here is fixture bias,
    // not a pore effect, and would invalidate the comparison above.
    report("control, uniform pore", 128, 128, 2000);
}
