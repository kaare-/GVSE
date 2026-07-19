//! Surface water soaking into the top porous solid layer.
//!
//! The old `recharge_deep_water_tables` "instant top-up" for submerged
//! and shore columns was removed after it kept eating rain films on
//! emergent land in ways the throttled per-tick infiltration path
//! never would. Submerged beds still saturate — the free-water column
//! above them keeps `run_infiltration` fed every tick — it just takes
//! several ticks rather than one snap.

use wk_material::{CHUNK_W, MaterialRegistry};
use wk_world::column::Activity;
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;
use crate::residual::ResidualAccumulator;

/// Fraction of standing water that soaks per infiltration tick.
/// Was 0.01 with a 60-tick period — rain puddles ran off before any
/// meaningful pore fill. With period 5 this lands ~1–2 orders faster.
const INFILTRATION_COEFF: f32 = 0.12;

/// Standing water above this instantly fills the remaining pore deficit
/// (hydraulic contact). Was 500 kg — shallow rain films never qualified.
pub const HYDRAULIC_CONTACT_MIN_KG: i64 = 80;

pub fn run_infiltration(world: &mut World, scratch: &mut WorldTransferScratch) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        for i in 0..CHUNK_W {
            let (activity, available, moisture, cap, perm) = {
                let col = &world.chunks.get(&coord).unwrap().columns[i];
                let base_perm = col
                    .top_porous_layer()
                    .map(|l| MaterialRegistry::props(l.material).permeability as f32 / 255.0)
                    .unwrap_or(0.0);
                let root = col.ecology.root_density.clamp(0.0, 1.0);
                let perm = base_perm * (1.0 + 0.8 * root);
                (
                    col.activity,
                    col.top_water_mass(),
                    col.moisture,
                    col.moisture_cap(),
                    perm,
                )
            };
            if activity == Activity::Dormant || available <= 0 || perm <= 0.0 {
                continue;
            }
            let need = cap.saturating_sub(moisture);
            if need == 0 {
                continue;
            }
            if available > HYDRAULIC_CONTACT_MIN_KG && available > need {
                scratch.buffer_mut(coord).infil_delta[i] += need;
                continue;
            }
            let rate = available as f32 * perm * INFILTRATION_COEFF;
            let col = world.chunks.get_mut(&coord).unwrap();
            let transfer =
                ResidualAccumulator::drain(&mut col.columns[i].residual.infiltration, rate);
            let actual = transfer.min(available).min(need);
            if actual > 0 {
                scratch.buffer_mut(coord).infil_delta[i] += actual;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::MaterialId;
    use wk_world::terrain::generate_flat_sand;

    #[test]
    fn rain_film_infiltrates_into_dry_sand() {
        let mut world = World::new(1);
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for i in 0..8 {
            let col = world.column_at_mut(i).unwrap();
            col.moisture = 0;
            col.deposit_to_top(MaterialId::Water, 200, 0);
        }
        world.wake_all();

        let mut scratch = WorldTransferScratch::default();
        // Several infiltration ticks accumulate residual → real kg.
        for _ in 0..20 {
            run_infiltration(&mut world, &mut scratch);
        }
        let booked: i64 = scratch
            .buffers
            .get(&0)
            .map(|b| b.infil_delta[..8].iter().sum())
            .unwrap_or(0);
        assert!(
            booked > 50,
            "shallow rain film must book infiltration (booked={booked})"
        );
    }

    #[test]
    fn submerged_bed_targets_full_saturation() {
        let mut world = World::new(1);
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        let col = world.column_at_mut(0).unwrap();
        // Standing water on a bed at/below the table → saturated target.
        col.deposit_to_top(MaterialId::Water, 10_000, 0);
        let bed = col.climate_elevation();
        let target = col.target_moisture_for_table(bed + 0.5);
        assert_eq!(target, col.moisture_cap());
    }
}
