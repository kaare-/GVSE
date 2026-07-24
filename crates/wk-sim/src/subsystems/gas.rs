//! Per-column CO₂ / O₂ (air + dissolved water).
//!
//! Air mixes toward ambient; dissolved gas exchanges with air at the free
//! surface (Henry-style relaxation) and diffuses sideways through wet
//! neighbours. Algae consume dissolved CO₂ / emit O₂ in `step_organisms`.

use wk_material::CHUNK_W;
use wk_world::column::{
    AMBIENT_AIR_CO2, AMBIENT_AIR_O2, EQUIL_WATER_CO2, EQUIL_WATER_O2,
};
use wk_world::world::World;

/// Fraction of the air↔ambient gap closed each gas step.
const AIR_MIX: f32 = 0.08;
/// Fraction of the dissolved↔Henry gap closed each step when water is present.
const EXCHANGE: f32 = 0.12;
/// Lateral blend of dissolved gas with wet neighbours.
const LATERAL: f32 = 0.15;

fn has_standing_water(col: &wk_world::column::Column) -> bool {
    col.flowable_water().map(|(_, m)| m > 0).unwrap_or(false)
}

/// Post-barrier gas mixing + air↔water exchange + wet lateral diffusion.
pub fn run_gas(world: &mut World, _tick: u64) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();

    // --- Pass 1: air restore + air↔water exchange (local) ---------------
    for &coord in &coords {
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        for i in 0..CHUNK_W {
            let wet = has_standing_water(&chunk.columns[i]);
            let eco = &mut chunk.columns[i].ecology;
            eco.air_co2 += (AMBIENT_AIR_CO2 - eco.air_co2) * AIR_MIX;
            eco.air_o2 += (AMBIENT_AIR_O2 - eco.air_o2) * AIR_MIX;

            if wet {
                // Henry targets scale with current air concentration.
                let target_co2 = EQUIL_WATER_CO2 * (eco.air_co2 / AMBIENT_AIR_CO2.max(1e-3));
                let target_o2 = EQUIL_WATER_O2 * (eco.air_o2 / AMBIENT_AIR_O2.max(1e-3));
                let d_co2 = (target_co2 - eco.water_co2) * EXCHANGE;
                let d_o2 = (target_o2 - eco.water_o2) * EXCHANGE;
                eco.water_co2 += d_co2;
                eco.water_o2 += d_o2;
                // Weak equal-and-opposite pull on the air film (lake outgassing).
                eco.air_co2 -= d_co2 * 0.05;
                eco.air_o2 -= d_o2 * 0.05;
            } else {
                // Dry: dissolved pool slowly relaxes toward equilibrium (pore film).
                eco.water_co2 += (EQUIL_WATER_CO2 - eco.water_co2) * 0.02;
                eco.water_o2 += (EQUIL_WATER_O2 - eco.water_o2) * 0.02;
            }
            eco.air_co2 = eco.air_co2.clamp(0.0, 3.0);
            eco.air_o2 = eco.air_o2.clamp(0.0, 3.0);
            eco.water_co2 = eco.water_co2.clamp(0.0, 3.0);
            eco.water_o2 = eco.water_o2.clamp(0.0, 3.0);
        }
    }

    // --- Pass 2: lateral dissolved diffusion (snapshot then write) ------
    for &coord in &coords {
        let base = world.chunks.get(&coord).map(|c| c.world_x_base()).unwrap_or(0);
        let mut next_co2 = [0.0f32; CHUNK_W];
        let mut next_o2 = [0.0f32; CHUNK_W];
        let mut wet = [false; CHUNK_W];
        {
            let Some(chunk) = world.chunks.get(&coord) else {
                continue;
            };
            for i in 0..CHUNK_W {
                wet[i] = has_standing_water(&chunk.columns[i]);
                next_co2[i] = chunk.columns[i].ecology.water_co2;
                next_o2[i] = chunk.columns[i].ecology.water_o2;
            }
        }
        for i in 0..CHUNK_W {
            if !wet[i] {
                continue;
            }
            let mut sum_c = next_co2[i];
            let mut sum_o = next_o2[i];
            let mut n = 1.0f32;
            for d in [-1i32, 1] {
                let j = i as i32 + d;
                if j >= 0 && (j as usize) < CHUNK_W && wet[j as usize] {
                    sum_c += next_co2[j as usize];
                    sum_o += next_o2[j as usize];
                    n += 1.0;
                } else {
                    // Cross-chunk neighbour
                    let wx = base + j;
                    if let Some(ncol) = world.column_at(wx) {
                        if has_standing_water(ncol) {
                            sum_c += ncol.ecology.water_co2;
                            sum_o += ncol.ecology.water_o2;
                            n += 1.0;
                        }
                    }
                }
            }
            let avg_c = sum_c / n;
            let avg_o = sum_o / n;
            next_co2[i] += (avg_c - next_co2[i]) * LATERAL;
            next_o2[i] += (avg_o - next_o2[i]) * LATERAL;
        }
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            for i in 0..CHUNK_W {
                if wet[i] {
                    chunk.columns[i].ecology.water_co2 = next_co2[i].clamp(0.0, 3.0);
                    chunk.columns[i].ecology.water_o2 = next_o2[i].clamp(0.0, 3.0);
                }
            }
        }
    }
}
