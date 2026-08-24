//! Where does competent-fall time actually go on a quiet demo world?
//! `cargo test -p wk-voxel --release --test competent_phase_probe -- --nocapture`

use std::time::Instant;

use wk_voxel::{
    apply_competent_fall_regions, competent_probe, plan_active, stamp_world,
    CompetentFallConfig, World, WorldgenParams,
};

#[test]
fn probe_competent_phases() {
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let cfg = CompetentFallConfig::default();

    let loose0 = {
        let mut n = 0;
        for x in 0..params.width_cols {
            for y in params.bedrock_floor_y..params.sky_ceiling_y {
                if let Some(c) = world.get_cell(x, y) {
                    if matches!(
                        c.material,
                        wk_material::MaterialId::LooseRock | wk_material::MaterialId::LooseLimestone
                    ) {
                        n += 1;
                    }
                }
            }
        }
        n
    };
    println!("\n  loose debris in freshly stamped world: {loose0}  (baseline)");

    // Dirty state of a freshly stamped world, before competent fall runs.
    let fresh = plan_active(&world);
    let fresh_cells: usize = fresh
        .iter()
        .map(|a| {
            (a.rect.x1 as usize - a.rect.x0 as usize + 1)
                * (a.rect.y1 as usize - a.rect.y0 as usize + 1)
        })
        .sum();
    println!(
        "\n  fresh world: {} dirty regions, {} cells in rects",
        fresh.len(),
        fresh_cells
    );
    // Clear dirty so we measure a genuinely quiet world.
    wk_voxel::clear_all_dirty(&mut world);
    let after_clear = plan_active(&world);
    println!("  after clear_all_dirty: {} regions", after_clear.len());
    let st = apply_competent_fall_regions(&mut world, &after_clear, &cfg, true);
    let after_one = plan_active(&world);
    println!(
        "  after ONE competent pass on quiet world: {} regions  <-- self-dirty check",
        after_one.len()
    );
    println!(
        "  that pass: fall_moves={} impacts={} roll_moves={} embed={}",
        st.fall_moves, st.impacts, st.roll_moves, st.embed_cells
    );
    // Count loose debris created by a single pass over untouched terrain.
    let loose = |w: &World| -> usize {
        let mut n = 0;
        for x in 0..params.width_cols {
            for y in params.bedrock_floor_y..params.sky_ceiling_y {
                if let Some(c) = w.get_cell(x, y) {
                    if matches!(
                        c.material,
                        wk_material::MaterialId::LooseRock | wk_material::MaterialId::LooseLimestone
                    ) {
                        n += 1;
                    }
                }
            }
        }
        n
    };
    println!("  loose debris after 1 pass: {}", loose(&world));
    wk_voxel::clear_all_dirty(&mut world);
    for _ in 0..10 {
        let d = plan_active(&world);
        let _ = apply_competent_fall_regions(&mut world, &d, &cfg, true);
    }
    println!("  loose debris after 11 passes: {}", loose(&world));

    // Mirror the real tick loop: dirty is consumed each tick.
    const N: u32 = 30;
    wk_voxel::clear_all_dirty(&mut world);
    competent_probe::reset();
    let mut dirty_regions = 0usize;
    let mut empty_ticks = 0u32;
    let t0 = Instant::now();
    for t in 0..N {
        world.tick = t as u64;
        let dirty = plan_active(&world);
        dirty_regions += dirty.len();
        if dirty.is_empty() {
            empty_ticks += 1;
        }
        // Mirror tick_with_life: an empty dirty set means "nothing to do".
        // Passing `&[]` to apply_competent_fall_regions means "whole world".
        if !dirty.is_empty() {
            let _ = apply_competent_fall_regions(&mut world, &dirty, &cfg, true);
        }
        wk_voxel::clear_all_dirty(&mut world);
    }
    let wall = t0.elapsed();
    println!(
        "\n  plan_active regions {:.1}/tick, empty-dirty ticks {empty_ticks}/{N}",
        dirty_regions as f64 / N as f64
    );
    let s = competent_probe::snapshot();
    let per = |v: u64| v as f64 / N as f64;
    println!("\n=== competent fall phase probe ({N} ticks, quiet demo world) ===");
    println!("  wall                  {:>10.3} ms/tick", wall.as_secs_f64() * 1000.0 / N as f64);
    println!("  build_components calls{:>10.1} /tick", per(s.build_calls));
    println!("  seed candidates       {:>10.1} /tick", per(s.seed_candidates));
    println!("  seeds passing gate    {:>10.1} /tick", per(s.seeds_passed));
    println!("  floods started        {:>10.1} /tick", per(s.floods));
    println!("  cells visited (flood) {:>10.1} /tick", per(s.flood_cells));
    println!("  strata bailouts       {:>10.1} /tick", per(s.strata_bailouts));
    println!("  split_welded calls    {:>10.1} /tick", per(s.split_calls));
    println!("  split_welded cells    {:>10.1} /tick", per(s.split_cells));
    println!("  hang peel calls       {:>10.1} /tick", per(s.hang_calls));
    println!("  components emitted    {:>10.1} /tick", per(s.components));
    println!("  gather_cargo calls    {:>10.1} /tick", per(s.cargo_calls));
    println!("  gather_cargo cells    {:>10.1} /tick", per(s.cargo_cells));
    println!();
}
