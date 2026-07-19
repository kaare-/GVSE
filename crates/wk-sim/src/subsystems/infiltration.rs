//! Surface water soaking into the top porous solid layer, plus continuous
//! water-table / shore recharge so lakes and sea level keep the ground wet.

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

/// Shore / lake fringe: dry land next to free water aims for this
/// saturation. One tree cannot empty a lake fringe in a few ticks.
const SHORE_TARGET_SAT: f32 = 0.90;

/// Max kg moved from a free-water column into one shore neighbour per
/// recharge pass (keeps mass moves visible but not catastrophic).
const SHORE_SEEP_PER_PASS: i64 = 2_500;

/// Post-slump / post-barrier: keep aquifers under free water saturated,
/// wet lake/sea shores by capillary seepage, and hold a regional floor
/// near sea level (same rule as worldgen `seed_column_water_table`).
pub fn recharge_deep_water_tables(world: &mut World) {
    let sea = world.sea_level;
    let coords: Vec<i32> = world.chunks.keys().copied().collect();

    // Pass 1 — own free water → own pores.
    //
    // Only *submerged* beds get the instant "top up pores to full" snap:
    // a lake or ocean column is hydraulically clamped to the aquifer
    // beneath it, so any tick where moisture < cap must equalize.
    // On emergent land a rain puddle is *not* clamped to the aquifer —
    // it's just wet material sitting on top waiting for [`run_infiltration`]
    // to slowly draw it in. Aggressively draining every kg of top water
    // into moisture on emergent columns was the reason light rain
    // never rendered as standing water: it was siphoned into pore
    // space on the same tick it landed.
    for &coord in &coords {
        for i in 0..CHUNK_W {
            let col = &mut world.chunks.get_mut(&coord).unwrap().columns[i];
            let available = col.top_water_mass();
            if available <= 0 {
                continue;
            }
            if col.solid_bed_y() >= sea - 0.05 {
                continue;
            }
            let target = col
                .target_moisture_for_table(sea)
                .max(col.moisture_cap());
            let need = target.saturating_sub(col.moisture);
            if need <= 0 {
                continue;
            }
            let took = col.take_water_from_cap(need.min(available));
            col.moisture += took;
        }
    }

    // Pass 2 — free-water columns seep into dry shore neighbours.
    // Collect donor indices first so we can mutate both sides safely.
    let mut donors: Vec<(i32, usize)> = Vec::new();
    for &coord in &coords {
        for i in 0..CHUNK_W {
            let col = &world.chunks.get(&coord).unwrap().columns[i];
            if col.top_water_mass() > HYDRAULIC_CONTACT_MIN_KG {
                donors.push((coord, i));
            }
        }
    }

    for (coord, i) in donors {
        for &di in &[-1i32, 1] {
            let ni = i as i32 + di;
            let (n_coord, n_local) = if ni < 0 {
                (coord - 1, (CHUNK_W as i32 + ni) as usize)
            } else if ni >= CHUNK_W as i32 {
                (coord + 1, (ni - CHUNK_W as i32) as usize)
            } else {
                (coord, ni as usize)
            };
            if !world.chunks.contains_key(&n_coord) {
                continue;
            }

            // Snapshot donor free water and neighbour need.
            let (donor_water, need) = {
                let donor = &world.chunks.get(&coord).unwrap().columns[i];
                let avail = donor.top_water_mass();
                if avail <= HYDRAULIC_CONTACT_MIN_KG {
                    continue;
                }
                let neigh = &world.chunks.get(&n_coord).unwrap().columns[n_local];
                // Interior of a water body — already handled in pass 1.
                if neigh.top_water_mass() > HYDRAULIC_CONTACT_MIN_KG {
                    continue;
                }
                let cap = neigh.moisture_cap();
                if cap <= 0 {
                    continue;
                }
                let shore_floor = ((cap as f32) * SHORE_TARGET_SAT).round() as i64;
                let table_floor = neigh.target_moisture_for_table(sea);
                let target = shore_floor.max(table_floor);
                let need = target.saturating_sub(neigh.moisture);
                (avail, need)
            };
            if need <= 0 {
                continue;
            }
            let want = need.min(SHORE_SEEP_PER_PASS).min(donor_water / 4);
            if want <= 0 {
                continue;
            }

            let took = {
                let donor = &mut world.chunks.get_mut(&coord).unwrap().columns[i];
                donor.take_water_from_cap(want)
            };
            if took > 0 {
                let neigh = &mut world.chunks.get_mut(&n_coord).unwrap().columns[n_local];
                let cap = neigh.moisture_cap();
                neigh.moisture = (neigh.moisture + took).min(cap);
            }
        }
    }
}

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
    fn lake_seeps_into_dry_shore() {
        let mut world = World::new(1);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        world.wake_all();

        // Lake in columns 0..8, dry sand shore to the right.
        for i in 0..8 {
            let col = world.column_at_mut(i).unwrap();
            col.moisture = 0;
            col.deposit_to_top(MaterialId::Water, 20_000, 0);
        }
        for i in 8..16 {
            let col = world.column_at_mut(i).unwrap();
            col.moisture = 0;
        }

        let shore_before = world.column_at(8).unwrap().moisture;
        assert_eq!(shore_before, 0);

        recharge_deep_water_tables(&mut world);

        let lake_moist = world.column_at(0).unwrap().moisture;
        let shore_moist = world.column_at(8).unwrap().moisture;
        let shore_cap = world.column_at(8).unwrap().moisture_cap();
        assert!(lake_moist > 0, "lake bed should saturate from free water");
        assert!(
            shore_moist > 0,
            "dry shore next to lake must receive capillary seepage"
        );
        assert!(
            shore_moist as f32 / shore_cap.max(1) as f32 > 0.2,
            "shore should hold meaningful water (got {} / {})",
            shore_moist,
            shore_cap
        );
    }

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
        let booked: i64 = scratch.buffers.get(&0).map(|b| b.infil_delta[..8].iter().sum()).unwrap_or(0);
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
