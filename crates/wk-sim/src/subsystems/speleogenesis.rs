//! Speleothem growth: reprecipitate the karst-dissolved mineral bank
//! into Limestone inside voids.
//!
//! Closes the karst mass loop: solid → dissolved-bank → solid (Limestone),
//! shrinking voids as floors/ceilings accrete. The old per-cell dissolved
//! concentration field is gone; we draw from
//! `MassAudit::dissolved_bank()` (which is
//! `dissolved_out_total − dissolved_return_total`) and route deposits
//! into ventilated voids on emergent land.

use wk_material::{CHUNK_W, MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::column::Activity;
use wk_world::world::World;

/// Fraction of the audit dissolved-bank returned per speleogenesis step.
///
/// Was 0.02 on the local concentration field; here we spend the same
/// slow rate against the global bank, split evenly across all eligible
/// voids so a single tick doesn't drain the bank into one cavity.
const SPELEO_BANK_FRAC: f32 = 0.02;

/// Minimum kg to bother precipitating for a single void.
const MIN_PRECIP_KG: i64 = 1;

pub fn run_speleogenesis(world: &mut World, tick: u64) {
    let bank = world.mass_audit.dissolved_bank();
    if bank <= 0 {
        return;
    }
    // Total to spend across all eligible voids this step.
    let budget = ((bank as f32) * SPELEO_BANK_FRAC).max(1.0) as i64;
    let budget = budget.min(bank);

    // Collect eligible (chunk, column, void_idx) targets — ventilated
    // (`light`) cavities on emergent land columns.
    let mut targets: Vec<(i32, usize, usize)> = Vec::new();
    for (&coord, chunk) in &world.chunks {
        for i in 0..CHUNK_W {
            let col = &chunk.columns[i];
            if col.activity == Activity::Dormant {
                continue;
            }
            for (vi, v) in col.voids.iter().enumerate() {
                if v.height_m <= 0.05 {
                    continue;
                }
                if v.light < 20 {
                    // No ventilation → no evaporation-driven precipitation.
                    continue;
                }
                targets.push((coord, i, vi));
            }
        }
    }
    if targets.is_empty() {
        return;
    }

    let per_void = (budget / targets.len() as i64).max(MIN_PRECIP_KG);
    let mut returned = 0i64;
    for (coord, i, vi) in targets {
        // Re-check void still exists (roof collapse could have removed it).
        let bedrock = world.chunks.get(&coord).map(|c| c.bedrock_y).unwrap_or(0.0);
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let col = &mut chunk.columns[i];
        if vi >= col.voids.len() {
            continue;
        }
        // Cap per-void by remaining budget and remaining bank.
        let bank_left = world.mass_audit.dissolved_bank() - returned;
        let take = per_void.min(bank_left).max(0);
        if take < MIN_PRECIP_KG {
            continue;
        }

        let density = MaterialRegistry::props(MaterialId::Limestone).density.max(1) as f32;
        let dh = (take as f32 / density) / SAMPLE_WIDTH_M;
        let dh = dh.min(col.voids[vi].height_m * 0.5);
        if dh <= 1e-5 {
            continue;
        }
        col.voids[vi].height_m = (col.voids[vi].height_m - dh).max(0.0);
        col.voids[vi].top_y -= dh * 0.5;
        col.deposit_to_top(MaterialId::Limestone, take, tick);
        col.activity = Activity::HydrologyActive;
        col.recompute_surface_y(bedrock);
        returned += take;
    }

    if returned > 0 {
        world.mass_audit.dissolved_return_total += returned;
    }
}
