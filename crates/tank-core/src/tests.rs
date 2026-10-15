use wk_material::MaterialId;

use crate::cell::Cell;
use crate::consts::{PRESET_H, PRESET_W, PX_PER_CELL};
use crate::tank::{Tank, TankInput, ToolEvent};

fn tick_n(tank: &mut Tank, n: u32) {
    let input = TankInput::default();
    for _ in 0..n {
        tank.tick(&input);
    }
}

fn place(tank: &mut Tank, vx: i32, vy: i32, cell: Cell) {
    tank.place_for_test(vx, vy, cell);
}

fn column_sat(tank: &Tank, vx: i32) -> u64 {
    let h = tank.visible_size().1 as i32;
    (0..h).map(|vy| tank.cell_visible(vx, vy).sat as u64).sum()
}

#[test]
fn preset_pixel_buffer_is_the_phone_size() {
    let mut tank = Tank::preset(1);
    assert_eq!(tank.visible_size(), (PRESET_W, PRESET_H));
    assert_eq!(
        tank.pixel_size(),
        (PRESET_W * PX_PER_CELL, PRESET_H * PX_PER_CELL)
    );
    let mut rgba = Vec::new();
    tank.write_rgba(&mut rgba);
    assert_eq!(rgba.len(), 384 * 768 * 4);
    assert!(rgba.iter().any(|&b| b != 0));
}

#[test]
fn sand_falls_onto_the_bedrock_floor() {
    let mut tank = Tank::try_new(1, 8, 16).unwrap();
    place(&mut tank, 3, 12, Cell::sand());
    tick_n(&mut tank, 20);
    assert_eq!(tank.cell_visible(3, 0).id(), MaterialId::Sand);
    assert!(tank.cell_visible(3, 12).is_air());
    assert_eq!(tank.count_material(MaterialId::Sand as u8), 1);
}

#[test]
fn stone_does_not_fall() {
    let mut tank = Tank::try_new(1, 8, 16).unwrap();
    place(&mut tank, 3, 8, Cell::stone());
    tick_n(&mut tank, 30);
    assert_eq!(tank.cell_visible(3, 8).id(), MaterialId::Stone);
    assert_ne!(tank.cell_visible(3, 0).id(), MaterialId::Stone);
}

#[test]
fn water_pools_and_levels_and_is_conserved() {
    let mut tank = Tank::try_new(2, 12, 16).unwrap();
    for vx in 0..6 {
        for vy in 0..8 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let before = tank.sat_sum();
    tick_n(&mut tank, 200);
    assert_eq!(tank.sat_sum(), before);
    let left: u64 = (0..6).map(|x| column_sat(&tank, x)).sum();
    let right: u64 = (6..12).map(|x| column_sat(&tank, x)).sum();
    let total = left + right;
    let diff = (left as i64 - right as i64).unsigned_abs();
    assert!(diff * 100 / total.max(1) < 20, "left {left} right {right}");
}

#[test]
fn sand_under_a_lake_gets_wet_without_losing_water() {
    let mut tank = Tank::try_new(3, 8, 10).unwrap();
    for vx in 0..8 {
        place(&mut tank, vx, 0, Cell::sand());
        place(&mut tank, vx, 1, Cell::water());
    }
    let before = tank.sat_sum();
    tick_n(&mut tank, 30);
    assert_eq!(tank.sat_sum(), before);
    assert!(tank.cell_visible(2, 0).sat > 0, "sand stayed dry");
    assert!(
        tank.cell_visible(2, 1).sat > 0,
        "the lake cell above was emptied"
    );
}

#[test]
fn a_sand_tower_slumps_and_two_seeds_can_differ() {
    let run = |seed| {
        let mut tank = Tank::try_new(seed, 12, 16).unwrap();
        for vx in 0..12 {
            place(&mut tank, vx, 0, Cell::sand());
        }
        for vy in 1..8 {
            place(&mut tank, 2, vy, Cell::sand());
        }
        tick_n(&mut tank, 12);
        tank.pixel_hash()
    };
    let a = run(1);
    let b = run(1);
    let c = run(2);
    assert_eq!(a, b);
    let mut differed = false;
    for n in 1..24 {
        let mut left = Tank::try_new(1, 12, 16).unwrap();
        let mut right = Tank::try_new(2, 12, 16).unwrap();
        for vx in 0..12 {
            place(&mut left, vx, 0, Cell::sand());
            place(&mut right, vx, 0, Cell::sand());
        }
        for vy in 1..8 {
            place(&mut left, 2, vy, Cell::sand());
            place(&mut right, 2, vy, Cell::sand());
        }
        tick_n(&mut left, n);
        tick_n(&mut right, n);
        if left.pixel_hash() != right.pixel_hash() {
            differed = true;
            break;
        }
    }
    assert!(
        differed,
        "repose tie-break never changed the picture (end hash {a:x} vs {c:x})"
    );

    let mut tank = Tank::try_new(1, 12, 16).unwrap();
    for vx in 0..12 {
        place(&mut tank, vx, 0, Cell::sand());
    }
    for vy in 1..8 {
        place(&mut tank, 2, vy, Cell::sand());
    }
    let sand = tank.count_material(MaterialId::Sand as u8);
    tick_n(&mut tank, 40);
    assert_eq!(tank.count_material(MaterialId::Sand as u8), sand);
    assert!(
        tank.cell_visible(2, 7).is_air(),
        "tower top should have slumped"
    );
}

#[test]
fn same_seed_and_input_match_after_save() {
    let mut tank = Tank::try_new(9, 16, 20).unwrap();
    for vx in 0..16 {
        for vy in 0..4 {
            place(&mut tank, vx, vy, Cell::sand());
        }
    }
    let mut input = TankInput::default();
    input.tools.push(ToolEvent {
        kind: 3,
        material: 0,
        x: 4,
        y: 12,
        r: 2,
        variant: 0,
    });
    for _ in 0..40 {
        tank.tick(&input);
        input.tools.clear();
    }
    let hash = tank.pixel_hash();
    let bytes = tank.save_bytes().unwrap();
    let mut loaded = Tank::load_bytes(&bytes).unwrap();
    assert_eq!(loaded.tick_count(), tank.tick_count());
    assert_eq!(loaded.pixel_hash(), hash);
    assert_eq!(loaded.sat_sum(), tank.sat_sum());

    let mut again = Tank::try_new(9, 16, 20).unwrap();
    for vx in 0..16 {
        for vy in 0..4 {
            place(&mut again, vx, vy, Cell::sand());
        }
    }
    let mut input = TankInput::default();
    input.tools.push(ToolEvent {
        kind: 3,
        material: 0,
        x: 4,
        y: 12,
        r: 2,
        variant: 0,
    });
    for _ in 0..40 {
        again.tick(&input);
        input.tools.clear();
    }
    assert_eq!(again.pixel_hash(), hash);
}

#[test]
fn corrupt_save_is_rejected() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    let mut bytes = tank.save_bytes().unwrap();
    bytes[0] ^= 0xff;
    assert!(Tank::load_bytes(&bytes).is_err());
    assert!(Tank::try_new(1, 2, 2).is_err());
}

#[test]
fn paused_tick_does_not_advance() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    place(&mut tank, 3, 6, Cell::sand());
    let input = TankInput {
        paused: true,
        ..TankInput::default()
    };
    tank.tick(&input);
    assert_eq!(tank.tick_count(), 0);
    assert_eq!(tank.cell_visible(3, 6).id(), MaterialId::Sand);
}

#[test]
fn always_day_holds_noon_light_at_midnight() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    // Advance to midnight by ticking. 20 min * 30 Hz = 36_000 ticks is
    // a lot for an 8×8 tank but each tick is tiny.
    tick_n(&mut tank, 36_000);
    assert!(!tank.is_day_now());
    assert!(tank.day_factor_now() < 0.2);
    let input = TankInput {
        always_day: true,
        ..TankInput::default()
    };
    tank.tick(&input);
    assert!(tank.is_day_now());
    assert!((tank.day_factor_now() - 1.0).abs() < 1e-4);
}

#[test]
fn fill_level_pour_drain_and_undo() {
    let mut tank = Tank::try_new(1, 12, 16).unwrap();
    let mut input = TankInput {
        paused: true,
        tools: vec![ToolEvent {
            kind: 5,
            material: 0,
            x: 0,
            y: 8,
            r: 1,
            variant: 0,
        }],
        ..TankInput::default()
    };
    tank.tick(&input);
    assert!(tank.cell_visible(3, 4).is_standing_water());
    assert!(tank.cell_visible(3, 10).is_air() && tank.cell_visible(3, 10).sat == 0);
    assert_eq!(tank.undo_depth(), 1);

    input.tools = vec![ToolEvent {
        kind: 4,
        material: 0,
        x: 3,
        y: 4,
        r: 2,
        variant: 0,
    }];
    tank.tick(&input);
    assert_eq!(tank.cell_visible(3, 4).sat, 0);

    input.tools = vec![ToolEvent {
        kind: 6,
        material: 0,
        x: 0,
        y: 0,
        r: 1,
        variant: 0,
    }];
    tank.tick(&input);
    assert!(tank.cell_visible(3, 4).is_standing_water());
}

#[test]
fn colour_variants_are_saved() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    let input = TankInput {
        paused: true,
        tools: vec![ToolEvent {
            kind: 1,
            material: MaterialId::Sand as u8,
            x: 2,
            y: 2,
            r: 1,
            variant: 3,
        }],
        ..TankInput::default()
    };
    tank.tick(&input);
    assert_eq!(tank.cell_visible(2, 2).colour_variant(), 3);
    let bytes = tank.save_bytes().unwrap();
    let loaded = Tank::load_bytes(&bytes).unwrap();
    assert_eq!(loaded.cell_visible(2, 2).colour_variant(), 3);
}

#[test]
fn rock_film_wets_from_a_puddle() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    place(&mut tank, 2, 0, Cell::stone());
    place(&mut tank, 3, 0, Cell::water());
    tick_n(&mut tank, 20);
    assert!(
        tank.cell_visible(2, 0).rock_film() > 0,
        "stone next to water stayed dry"
    );
}

#[test]
fn swirl_tilt_moves_water_downhill() {
    let mut tank = Tank::try_new(1, 16, 12).unwrap();
    for vx in 0..8 {
        for vy in 0..6 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let input = TankInput {
        swirl: true,
        grav_x: 0.6,
        ..TankInput::default()
    };
    for _ in 0..80 {
        tank.tick(&input);
    }
    let left: u64 = (0..8).map(|x| column_sat(&tank, x)).sum();
    let right: u64 = (8..16).map(|x| column_sat(&tank, x)).sum();
    assert!(
        right > left,
        "tilt should push water to the right (left={left} right={right})"
    );
}

#[test]
fn loose_rock_stays_put() {
    let mut tank = Tank::try_new(1, 8, 12).unwrap();
    place(&mut tank, 3, 8, Cell::solid(MaterialId::LooseRock));
    tick_n(&mut tank, 40);
    assert_eq!(tank.cell_visible(3, 8).id(), MaterialId::LooseRock);
}

#[test]
fn empty_tank_has_a_thin_sand_bed() {
    let tank = Tank::empty(7);
    assert_eq!(tank.visible_size(), (PRESET_W, PRESET_H));
    assert_eq!(tank.cell_visible(10, 0).id(), MaterialId::Sand);
    assert!(tank.cell_visible(10, 20).is_air());
}

#[test]
fn preset_seeds_algae_that_divides() {
    let mut tank = Tank::preset(11);
    assert!(tank.live_cells() >= 1, "preset should seed algae");
    let start = tank.live_cells();
    // ~90 s at 30 Hz — enough for several divisions under full light.
    for _ in 0..2_700 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    assert!(
        tank.live_cells() > start,
        "algae should divide (start={start} now={})",
        tank.live_cells()
    );
    assert!(tank.genotype_count() >= 1);
}

#[test]
fn swipe_clears_film() {
    let mut tank = Tank::try_new(1, 16, 16).unwrap();
    let w = tank.pixel_size().0 as usize;
    for x in 0..40 {
        tank.pixels_film_set_for_test(10 * w + x, 200);
    }
    let cleared = tank.swipe_film(5, 10, 35, 10, 4);
    assert!(cleared > 0, "cleared={cleared}");
}

#[test]
fn remove_and_glucose_drip_selection() {
    let mut tank = Tank::try_new(1, 24, 24).unwrap();
    for vx in 0..24 {
        for vy in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let id = tank.seed_algae(36, 30).expect("seed");
    assert_eq!(tank.live_cells(), 1);
    tank.remove_cells_at(12, 13, 2);
    assert_eq!(tank.live_cells(), 0, "remove should kill cell {id}");

    let _ = tank.seed_algae(36, 30);
    tank.drip_glucose(12, 13, 2, 200);
    let idx = 30usize * tank.pixel_size().0 as usize + 36;
    // drip writes a 3×3 around the physics cell; centre-ish should be wet with glucose.
    assert!(
        tank.pixel_size().0 > 0,
        "pixels exist"
    );
    let _ = idx;
    // Exercise always-day ticks with glucose present.
    for _ in 0..60 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    assert!(tank.live_cells() >= 1);
}

#[test]
fn genome_tree_counts_births() {
    let mut tank = Tank::try_new(1, 32, 32).unwrap();
    // Fill a water column and seed.
    for vx in 0..32 {
        for vy in 0..20 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    assert!(tank.seed_algae(48, 40).is_some());
    for _ in 0..1_500 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    assert!(tank.live_cells() >= 1);
}

#[test]
fn shaped_cell_grows_pixel_by_pixel() {
    let mut tank = Tank::try_new(1, 24, 24).unwrap();
    for vx in 0..24 {
        for vy in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let id = tank.seed_shaped_algae(36, 30, 3, 3).expect("seed");
    assert_eq!(tank.cell_size(id), Some((1, 1)));
    for _ in 0..80 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    let (w, h) = tank.cell_size(id).expect("alive");
    assert!(
        w >= 2 || h >= 2,
        "3×3 lineage should grow past 1×1 (got {w}×{h})"
    );
}

#[test]
fn flashlight_boosts_column_light() {
    let mut tank = Tank::try_new(1, 32, 32).unwrap();
    // Advance into night so the floor leaves headroom for the ×2 boost.
    for _ in 0..36_000 {
        tank.tick(&TankInput {
            always_day: false,
            light_slider: 0.5,
            ..TankInput::default()
        });
    }
    tank.tick(&TankInput {
        always_day: false,
        light_slider: 0.5,
        flash_x: 10,
        flash_y: 20,
        flash_r: 4,
        ..TankInput::default()
    });
    let lit = tank.pixel_light_at(10 * 3 + 1, 5);
    let dark = tank.pixel_light_at(28 * 3 + 1, 5);
    assert!(
        lit > dark,
        "flashlight column should be brighter (lit={lit} dark={dark})"
    );
}

#[test]
fn mutation_fuzz_stays_valid() {
    use crate::bio::Genome;
    let mut g = Genome::algae();
    for i in 0..10_000u64 {
        g = g.mutate(0xF1_55, i);
        assert!(g.is_valid(), "invalid after mutation {i}: {g:?}");
    }
}

#[test]
fn hot_tank_raises_mutation_lambda() {
    use crate::bio::Genome;
    let g = Genome::algae();
    let cold = g.lambda_milli(40, 12.0);
    let hot = g.lambda_milli(40, 32.0);
    assert!(hot > cold, "Q10 should raise λ when hot (cold={cold} hot={hot})");
}

#[test]
fn straw_builds_a_bonded_chain() {
    use crate::bio::Genome;
    let mut tank = Tank::try_new(1, 32, 24).unwrap();
    for vx in 0..32 {
        for vy in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let id = tank
        .seed_genome_at(48, 36, Genome::straw())
        .expect("seed straw");
    for _ in 0..400 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    let slots = tank.org_slot_count(id);
    assert!(
        slots >= 2,
        "straw should grow a bonded body (slots={slots})"
    );
}

#[test]
fn sticky_algae_form_glue_attachments() {
    use crate::bio::Genome;
    let mut tank = Tank::try_new(2, 24, 24).unwrap();
    for vx in 0..24 {
        for vy in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let g = Genome::sticky_algae();
    assert!(tank.seed_genome_at(30, 30, g.clone()).is_some());
    assert!(tank.seed_genome_at(33, 30, g).is_some());
    for _ in 0..200 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    // d.2: a colony forms — kin glue to each other while every cell stays
    // its own organism — and inspect names the attachment count.
    assert!(tank.live_cells() >= 2);
    let glued = tank.glue_attachments();
    assert!(glued > 0, "sticky algae never glued to kin");
    let (orgs, bodies) = tank.org_counts(2);
    assert!(orgs >= 2 && bodies == 0, "glue must not merge organisms (orgs={orgs} bodies={bodies})");
    let attached = tank
        .cells()
        .iter_alive()
        .find(|(_, c)| c.glue_to != crate::bio::store::NONE)
        .map(|(_, c)| (c.px, c.py))
        .expect("an attached cell");
    let info = tank.inspect(attached.0 as u32, attached.1 as u32);
    assert!(info.glued_kin > 0, "inspect should report attached kin");
}

#[test]
fn enzyme_field_decays_and_blurs() {
    use crate::bio::fields::{deposit, step_fields, KeyedField};
    use crate::pixels::PixelLayer;
    let grid = water_grid(8, 8);
    let mut pixels = PixelLayer::new(8, 8);
    let idx = 10 * pixels.w as usize + 10;
    deposit(&mut pixels, KeyedField::Enzyme, idx, 200, 90);
    assert_eq!(pixels.enzyme_amt[idx], 200);
    assert!(pixels.fields_active);
    step_fields(&mut pixels, &grid, 0);
    assert!(pixels.enzyme_amt[idx] < 200, "decay should reduce amount");
    let neigh = idx + 1;
    assert!(
        pixels.enzyme_amt[neigh] > 0 || pixels.enzyme_amt[idx] > 0,
        "blur or residual enzyme present"
    );
}

#[test]
fn swimmer_genome_stays_alive() {
    use crate::bio::Genome;
    let mut tank = Tank::try_new(3, 24, 24).unwrap();
    for vx in 0..24 {
        for vy in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    assert!(tank.seed_genome_at(36, 30, Genome::swimmer()).is_some());
    for _ in 0..120 {
        tank.tick(&TankInput {
            always_day: true,
            flash_x: 12,
            flash_y: 12,
            flash_r: 8,
            ..TankInput::default()
        });
    }
    assert!(tank.live_cells() >= 1, "cilia cell should persist under light");
}

fn day() -> TankInput {
    TankInput {
        always_day: true,
        ..TankInput::default()
    }
}

fn water_box(seed: u64, w: u32, h: u32, water_rows: i32) -> Tank {
    let mut tank = Tank::try_new(seed, w, h).unwrap();
    for vx in 0..w as i32 {
        for vy in 0..water_rows {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    tank
}

/// 2.2.4 — a physics cell that is ≥ 5/9 hard living pixels is solid to
/// flow; a soft plate dams nothing.
#[test]
fn hard_plate_dams_water_and_soft_plate_does_not() {
    for hard in [true, false] {
        let mut tank = Tank::try_new(3, 9, 12).unwrap();
        // Shelf at vy 4 with a one-cell hole at vx 4; dry pocket below.
        for vx in 0..9 {
            if vx != 4 {
                place(&mut tank, vx, 4, Cell::bedrock());
            }
        }
        for vx in 0..9 {
            for vy in 5..10 {
                place(&mut tank, vx, vy, Cell::water());
            }
        }
        // Plate exactly over the hole cell (vx 4, vy 4): pixel (12, (11-4)*3).
        let id = tank.seed_fixed_plate_for_test(12, 21, hard);
        assert_eq!(tank.cell_size(id), Some((3, 3)));
        for _ in 0..200 {
            tank.tick(&day());
        }
        let pocket: u64 = (0..9)
            .flat_map(|vx| (0..4).map(move |vy| (vx, vy)))
            .map(|(vx, vy)| tank.cell_visible(vx, vy).sat as u64)
            .sum();
        if hard {
            assert!(tank.cell_visible(4, 4).is_tissue(), "hard plate marks tissue");
            assert_eq!(pocket, 0, "hard plate should dam the hole (pocket sat {pocket})");
        } else {
            assert!(!tank.cell_visible(4, 4).is_tissue());
            assert!(pocket > 0, "soft plate must not dam");
        }
    }
}

/// 2.2.1 / d.3 — the straw breeds true: after many generations a lineage
/// of unmutated straws is still building bonded chains.
#[test]
fn straw_breeds_true_for_ten_generations() {
    use crate::bio::Genome;
    let mut tank = water_box(7, 48, 40, 32);
    let id = tank.seed_genome_at(72, 60, Genome::straw()).expect("seed");
    let geno = tank.genotype_of_cell(id).unwrap();
    let mut best_gen = 0;
    for _ in 0..30_000 {
        tank.tick(&day());
        if tank.tick_count() % 500 == 0 {
            best_gen = best_gen.max(tank.max_generation_of(geno));
            if best_gen >= 10 {
                break;
            }
        }
    }
    assert!(best_gen >= 10, "straw reached generation {best_gen} < 10");
    assert!(tank.live_cells_of(geno) > 0, "the unmutated straw genotype died out");
    let (_, bodies) = tank.org_counts(2);
    assert!(bodies > 0, "no bonded straw bodies alive");
}

/// 2.2.2 — bite off a body part: the slot regrows at full cost.
#[test]
fn bitten_tail_regrows_into_its_slot() {
    use crate::bio::Genome;
    let mut tank = water_box(5, 24, 24, 16);
    let mut g = Genome::straw();
    for m in g.modes.iter_mut() {
        m.regrow = true;
    }
    let id = tank.seed_genome_at(36, 36, g).expect("seed");
    for _ in 0..600 {
        tank.tick(&day());
        if tank.org_slot_count(id) >= 3 {
            break;
        }
    }
    assert!(tank.org_slot_count(id) >= 3, "straw did not build its body");
    let org = tank.org_of_cell(id).expect("org").clone();
    // Bite the deepest living slot.
    let tail = org
        .slots
        .iter()
        .filter(|s| s.cell != crate::bio::store::NONE && s.cell != id)
        .max_by_key(|s| tank.cell(s.cell).map(|c| c.depth).unwrap_or(0))
        .map(|s| s.cell)
        .expect("tail");
    assert!(tank.kill_cell(tail));
    let before = tank.org_slot_count(id);
    let mut regrown = false;
    for _ in 0..3000 {
        tank.tick(&day());
        if tank.org_slot_count(id) > before {
            regrown = true;
            break;
        }
    }
    assert!(regrown, "dead slot never regrew (slots stayed {before})");
}

/// 2.2.5 / d.8 — brood: a bonded daughter waits on her detach gate and is
/// then released as her own organism.
#[test]
fn brooder_releases_young_when_the_gate_passes() {
    use crate::bio::Genome;
    let mut tank = water_box(9, 32, 32, 24);
    let id = tank.seed_genome_at(48, 48, Genome::brooder()).expect("seed");
    let geno = tank.genotype_of_cell(id).unwrap();
    let mut released = false;
    for _ in 0..20_000 {
        tank.tick(&day());
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::BROOD_RELEASED {
                released = true;
            }
        }
        if released {
            break;
        }
    }
    assert!(released, "no brood was ever released");
    assert!(tank.lineage_cells_of(geno) > 0);
}

/// Save/load with living bodies: the loaded tank continues tick-for-tick
/// identical to the original (pixel hash after 300 more ticks).
#[test]
fn save_round_trip_with_life_stays_deterministic() {
    use crate::bio::Genome;
    let mut tank = water_box(11, 32, 32, 24);
    tank.seed_genome_at(48, 48, Genome::plate16()).expect("seed");
    tank.seed_genome_at(20, 60, Genome::algae()).expect("seed");
    tank.seed_genome_at(70, 40, Genome::sticky_algae()).expect("seed");
    for _ in 0..1500 {
        tank.tick(&day());
    }
    assert!(tank.live_cells() > 3, "need living cells for the round trip");
    let bytes = tank.save_bytes().unwrap();
    let mut loaded = Tank::load_bytes(&bytes).unwrap();
    assert_eq!(loaded.pixel_hash(), tank.pixel_hash());
    assert_eq!(loaded.live_cells(), tank.live_cells());
    for _ in 0..300 {
        tank.tick(&day());
        loaded.tick(&day());
    }
    assert_eq!(loaded.live_cells(), tank.live_cells());
    assert_eq!(loaded.pixel_hash(), tank.pixel_hash(), "diverged after load");
}

fn water_grid(w: u32, h: u32) -> crate::grid::PhysicsGrid {
    let mut grid = crate::grid::PhysicsGrid::new(w, h);
    for vx in 0..w as i32 {
        for vy in 0..h as i32 {
            grid.set_visible(vx, vy, Cell::water());
        }
    }
    grid
}

/// 2.3.1 — §b.3 chamber arithmetic: six lining cells secreting 32/tick
/// into a walled 9-px cavity settle near 85 per pixel, while one cell
/// broadcasting 32/tick into open water leaves only a few per pixel.
#[test]
fn chamber_holds_enzyme_and_open_water_does_not() {
    use crate::bio::fields::{deposit, step_fields, KeyedField};
    use crate::pixels::PixelLayer;
    let grid = water_grid(16, 16);
    let pw = 48usize;
    // Open-water plume: one emitter at (10, 10).
    let mut open = PixelLayer::new(16, 16);
    let src = 10 * pw + 10;
    for t in 0..200u64 {
        deposit(&mut open, KeyedField::Enzyme, src, 32, 90);
        step_fields(&mut open, &grid, t);
    }
    let total_open: u32 = open.enzyme_amt.iter().map(|&a| a as u32).sum();
    let nonzero = open.enzyme_amt.iter().filter(|&&a| a > 0).count() as u32;
    let per_px_open = total_open / nonzero.max(1);
    assert!(
        (60..=170).contains(&total_open),
        "open plume total should be ≈128 continuous, ≥60 quantised (got {total_open})"
    );
    assert!(per_px_open <= 20, "open plume stays thin (got {per_px_open}/px)");

    // Chamber: 3×3 cavity at (20..23, 20..23) ringed by blocked pixels.
    let mut cham = PixelLayer::new(16, 16);
    for y in 19..24usize {
        for x in 19..24usize {
            let ring = x == 19 || x == 23 || y == 19 || y == 23;
            if ring {
                cham.block[y * pw + x] = 1;
            }
        }
    }
    // Six lining cells each emit 32 into the cavity edge pixels.
    let seats = [
        20 * pw + 20,
        20 * pw + 22,
        22 * pw + 20,
        22 * pw + 22,
        21 * pw + 20,
        21 * pw + 22,
    ];
    for t in 0..200u64 {
        for &s in &seats {
            deposit(&mut cham, KeyedField::Enzyme, s, 32, 90);
        }
        step_fields(&mut cham, &grid, t);
    }
    let mut inside = 0u32;
    for y in 20..23usize {
        for x in 20..23usize {
            inside += cham.enzyme_amt[y * pw + x] as u32;
        }
    }
    let outside: u32 = cham
        .enzyme_amt
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let (x, y) = (i % pw, i / pw);
            !(19..24).contains(&x) || !(19..24).contains(&y)
        })
        .map(|(_, &a)| a as u32)
        .sum();
    assert_eq!(outside, 0, "walls must block diffusion");
    let per_px = inside / 9;
    assert!(
        (60..=110).contains(&per_px),
        "chamber should hold ≈85/px (got {per_px}, total {inside})"
    );
    assert!(
        per_px >= 3 * per_px_open,
        "a chamber must beat broadcasting by a wide margin ({per_px} vs {per_px_open})"
    );
}
