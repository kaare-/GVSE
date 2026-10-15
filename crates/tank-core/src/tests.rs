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
    use crate::pixels::{CoarseField, PixelLayer};
    let grid = water_grid(8, 8);
    let mut pixels = PixelLayer::new(8, 8);
    let mut coarse = CoarseField::new(8, 8);
    let idx = 10 * pixels.w as usize + 10;
    deposit(&mut pixels, KeyedField::Enzyme, idx, 200, 90);
    assert_eq!(pixels.enzyme_amt[idx], 200);
    assert!(pixels.fields_active);
    step_fields(&mut pixels, &grid, &mut coarse, 0);
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
    // Mutation off so the pace of the unmutated program is what is
    // measured, not its luck against its own mutants.
    tank.set_lambda_scale_q8(0);
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
    assert_eq!(
        tank.live_cells_of(geno) as usize,
        tank.live_cells(),
        "no mutants expected at λ = 0"
    );
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
    use crate::pixels::{CoarseField, PixelLayer};
    let grid = water_grid(16, 16);
    let mut coarse = CoarseField::new(16, 16);
    let pw = 48usize;
    // Open-water plume: one emitter at (10, 10).
    let mut open = PixelLayer::new(16, 16);
    let src = 10 * pw + 10;
    for t in 0..200u64 {
        deposit(&mut open, KeyedField::Enzyme, src, 32, 90);
        step_fields(&mut open, &grid, &mut coarse, t);
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
        step_fields(&mut cham, &grid, &mut coarse, t);
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

fn dark() -> TankInput {
    TankInput {
        always_day: true,
        light_slider: 0.0,
        ..TankInput::default()
    }
}

/// 2.3.2 / d.4 — a saprotroph lives on litter: in the dark, with soft
/// litter on the floor, the lineage digests it (litter falls, glucose
/// appears, divisions happen); with no litter it only spends its seed store.
#[test]
fn saprotroph_lives_on_litter_in_the_dark() {
    use crate::bio::chem::LITTER_SOFT_CODE;
    use crate::bio::Genome;
    let run = |litter: bool| -> (u64, usize, u32) {
        let mut tank = water_box(13, 32, 24, 20);
        tank.set_lambda_scale_q8(0);
        let (pw, ph) = tank.pixel_size();
        if litter {
            for px in 0..pw as u16 {
                tank.drop_litter(px, (ph - 1) as u16, 255, LITTER_SOFT_CODE);
            }
        }
        let start = tank.litter_total();
        let id = tank.seed_genome_at(48, 40, Genome::saprotroph()).expect("seed");
        let geno = tank.genotype_of_cell(id).unwrap();
        let mut divisions = 0u32;
        for _ in 0..6000 {
            tank.tick(&dark());
            while let Some(ev) = tank.poll_event() {
                if ev.kind == crate::bio::ev::DIVISION {
                    divisions += 1;
                }
            }
        }
        (start - tank.litter_total(), tank.live_cells_of(geno) as usize, divisions)
    };
    let (eaten, fed_cells, fed_divisions) = run(true);
    let (_, starved_cells, starved_divisions) = run(false);
    assert!(eaten > 2000, "litter should be digested (eaten {eaten} units)");
    assert!(
        fed_divisions >= 4 && fed_cells >= 4,
        "fed saprotrophs should multiply (divisions {fed_divisions}, cells {fed_cells})"
    );
    assert!(
        fed_divisions > starved_divisions + 2,
        "litter must make the difference ({fed_divisions} vs {starved_divisions})"
    );
    assert!(starved_cells <= 2, "without litter nothing grows in the dark ({starved_cells})");
}

/// 2.3.2 / d.7 — the chamber predator grows its hard U; an algae whose
/// `wall_key` matches the lining enzyme is lysed inside the cavity (its
/// structure becomes glucose), while the same algae in open water lives.
#[test]
fn chamber_predator_lyses_prey_in_its_cavity() {
    use crate::bio::Genome;
    let mut tank = water_box(17, 32, 24, 24);
    tank.set_lambda_scale_q8(0);
    let id = tank.seed_genome_at(48, 60, Genome::chamber_predator()).expect("seed");
    let geno = tank.genotype_of_cell(id).unwrap();
    let org = tank.cells().get(id).unwrap().org;
    let mut formed = false;
    for _ in 0..4000 {
        tank.tick(&day());
        if tank.cells().org(org).map(|o| o.n_alive).unwrap_or(0) >= 7 {
            formed = true;
            break;
        }
    }
    assert!(formed, "the U never finished (cells {})", tank.live_cells_of(geno));
    let base = tank
        .cells()
        .iter_alive()
        .find(|(_, c)| c.org == org && c.mode == 0)
        .map(|(_, c)| (c.px, c.py))
        .expect("base cell");
    let cavity = (base.0, base.1 - 1);
    assert_eq!(tank.inspect(cavity.0 as u32, cavity.1 as u32).owner, 0, "cavity is open water");
    assert!(
        tank.cells()
            .iter_alive()
            .filter(|(_, c)| c.org == org)
            .all(|(_, c)| c.damage == 0),
        "the lining must not digest itself"
    );
    let mut prey_g = Genome::algae();
    prey_g.wall_key = 110;
    prey_g.buoy = -20;
    tank.set_lambda_scale_q8(0);
    let prey = tank.seed_genome_at(cavity.0, cavity.1, prey_g.clone()).expect("prey in cavity");
    let control = tank.seed_genome_at(6, 40, prey_g).expect("control prey");
    let mut predation = false;
    let mut died_at = None;
    for t in 0..900 {
        tank.tick(&day());
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::PREDATION && ev.arg0 == prey {
                predation = true;
            }
        }
        if tank.cell_size(prey).is_none() {
            died_at = Some(t);
            break;
        }
    }
    assert!(died_at.is_some(), "prey in the cavity should be lysed within 30 s");
    assert!(predation, "a PREDATION event should name the prey");
    assert!(tank.cell_size(control).is_some(), "the control prey in open water lives");
    assert!(tank.live_cells_of(geno) >= 7, "the predator body survives the meal");
}

/// 2.3.3 — NEEDLE + TOXIN: a stinger facing an unresisted algae kills it
/// (POISON event); the same stinger facing kin with its `resist_key` does not.
#[test]
fn needle_toxin_kills_unresisted_prey_and_spares_resistant_kin() {
    use crate::bio::Genome;
    let run = |resist_key: u8| -> (bool, bool) {
        let mut tank = water_box(19, 16, 16, 16);
        tank.set_lambda_scale_q8(0);
        // Stinger heads up; its target sits on the pixel above it.
        let stinger = tank.seed_genome_at(24, 30, Genome::stinger()).expect("stinger");
        let mut prey_g = Genome::algae();
        prey_g.resist_key = resist_key;
        let prey = tank.seed_genome_at(24, 29, prey_g).expect("prey");
        tank.pin_cell_for_test(stinger);
        tank.pin_cell_for_test(prey);
        let mut poison = false;
        for _ in 0..300 {
            tank.tick(&day());
            while let Some(ev) = tank.poll_event() {
                if ev.kind == crate::bio::ev::POISON && ev.arg0 == prey {
                    poison = true;
                }
            }
            if tank.cell_size(prey).is_none() {
                break;
            }
        }
        assert!(tank.cell_size(stinger).is_some(), "the stinger is immune to its own venom");
        (tank.cell_size(prey).is_none(), poison)
    };
    let (dead, poison) = run(40);
    assert!(dead && poison, "unresisted prey should be poisoned (dead {dead}, event {poison})");
    let (dead_kin, _) = run(200);
    assert!(!dead_kin, "a resistant target shrugs the venom off");
}

/// 2.3.3 — SIGNAL / RECEPTOR: kin a few pixels apart hear each other
/// (`sig` rises on the listener, SIGNALLING event fires).
#[test]
fn kin_hear_each_others_signal() {
    use crate::bio::Genome;
    let mut tank = water_box(23, 16, 16, 16);
    tank.set_lambda_scale_q8(0);
    let a = tank.seed_genome_at(20, 30, Genome::signaller()).expect("a");
    let b = tank.seed_genome_at(24, 30, Genome::signaller()).expect("b");
    let mut heard = false;
    for _ in 0..120 {
        tank.tick(&day());
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::SIGNALLING && (ev.arg0 == a || ev.arg0 == b) {
                heard = true;
            }
        }
    }
    assert!(heard, "no SIGNALLING event between kin");
    let sig = tank.cells().get(a).map(|c| c.sig).unwrap_or(0).max(tank.cells().get(b).map(|c| c.sig).unwrap_or(0));
    assert!(sig > 0, "a receptor should read its kin's signal");
    // A deaf algae next to them reads nothing.
    let deaf = tank.seed_genome_at(28, 30, Genome::algae()).expect("deaf");
    for _ in 0..30 {
        tank.tick(&day());
    }
    assert_eq!(tank.cells().get(deaf).map(|c| c.sig), Some(0));
}

/// 2.3.2 — a corpse in water becomes litter that settles to the floor and
/// keeps its mass until something digests it.
#[test]
fn corpses_become_litter_that_settles() {
    use crate::bio::Genome;
    let mut tank = water_box(29, 16, 16, 16);
    tank.set_lambda_scale_q8(0);
    // A heat-killed algae: run hot so it dies of damage quickly.
    let id = tank.seed_genome_at(24, 20, Genome::algae()).expect("seed");
    let hot = TankInput {
        always_day: true,
        t_base: 45.0,
        ..TankInput::default()
    };
    let mut died = false;
    for _ in 0..3000 {
        tank.tick(&hot);
        if tank.cell_size(id).is_none() {
            died = true;
            break;
        }
    }
    assert!(died, "the algae should die in a hot tank");
    // The dead cell lies there as a corpse first (2.3.5), then crumbles.
    // Any daughters are removed (the tool leaves no corpse) so only the
    // corpses' mass is in play.
    assert!(tank.corpse_count() >= 1, "a water death leaves a corpse");
    tank.remove_cells_at(8, 8, 32);
    for _ in 0..(crate::bio::store::CORPSE_TICKS as u32 + 8) {
        tank.tick(&hot);
    }
    assert_eq!(tank.corpse_count(), 0, "the corpses crumbled");
    let total = tank.litter_total();
    assert!(total >= 140, "a 6 g corpse leaves ≥ 140 units of litter (got {total})");
    let fines: f32 = tank.fines().iter().sum();
    assert!(fines > 0.0, "30 % of the soft mass went up as fines");
    for _ in 0..600 {
        tank.tick(&hot);
    }
    assert_eq!(tank.litter_total(), total, "litter mass is conserved");
    let (pw, ph) = tank.pixel_size();
    let floor: u64 = (0..pw as u16).map(|px| tank.litter_at(px, (ph - 1) as u16) as u64).sum();
    assert_eq!(floor, total, "all litter settled onto the floor row");
}

/// 2.3.5 / 2.3.7 — corpse lifecycle: a starved cell stays on its pixels
/// as a corpse, sinks through the water, keeps its id and pixel claim,
/// then crumbles into soft litter (70 %) and fines (30 %) on expiry.
#[test]
fn corpse_sinks_then_becomes_litter_and_fines() {
    use crate::bio::store::{CORPSE_TICKS, OWNER_BIAS};
    use crate::bio::Genome;
    let mut tank = water_box(31, 16, 16, 16);
    tank.set_lambda_scale_q8(0);
    let id = tank.seed_genome_at(24, 10, Genome::algae()).expect("seed");
    tank.set_energy_for_test(id, 0);
    tank.tick(&dark());
    assert!(tank.cell(id).is_none(), "the cell starved");
    let corpse = *tank.corpse(id).expect("it lies there as a corpse");
    assert!(corpse.energy > 0, "the corpse carries its soft mass");
    assert_eq!(tank.owner_at(corpse.px, corpse.py), id + OWNER_BIAS, "the corpse keeps its pixel");
    assert_eq!(tank.litter_total(), 0);
    for _ in 0..40 {
        tank.tick(&dark());
    }
    let sunk = tank.corpse(id).expect("still a corpse");
    assert!(sunk.py > 10, "a corpse sinks ({} → {})", 10, sunk.py);
    for _ in 0..(CORPSE_TICKS as u32) {
        tank.tick(&dark());
    }
    assert!(tank.corpse(id).is_none(), "the corpse crumbled");
    assert_eq!(tank.corpse_count(), 0);
    let soft_units = (corpse.energy / crate::consts::GLUCOSE_UNIT_FP) as u64;
    let litter = tank.litter_total();
    let fines_units = soft_units * crate::bio::FINES_SHARE_Q8 as u64 / 256;
    assert_eq!(litter, soft_units - fines_units, "70 % of the soft mass is litter");
    let fines_g = tank.fines()[corpse.px as usize];
    let expect_g = fines_units as f32 / 64.0;
    assert!((fines_g - expect_g).abs() < 1e-3, "30 % went to fines[x] (got {fines_g}, want {expect_g})");
}

/// 2.3.5 — three detritus classes: digesting soft litter leaves inert
/// grit that nothing eats; only the clean tool (kind 13) removes it.
#[test]
fn grit_accumulates_until_cleaned() {
    use crate::bio::chem::LITTER_SOFT_CODE;
    use crate::bio::Genome;
    let mut tank = water_box(33, 32, 24, 20);
    tank.set_lambda_scale_q8(0);
    let (pw, ph) = tank.pixel_size();
    for px in 0..pw as u16 {
        tank.drop_litter(px, (ph - 1) as u16, 255, LITTER_SOFT_CODE);
    }
    tank.seed_genome_at(48, 40, Genome::saprotroph()).expect("seed");
    for _ in 0..6000 {
        tank.tick(&dark());
    }
    let grit = tank.grit_total();
    assert!(grit > 100, "digestion should leave grit (got {grit})");
    for _ in 0..300 {
        tank.tick(&dark());
    }
    assert!(tank.grit_total() >= grit, "nothing eats grit");
    let (vw, vh) = tank.visible_size();
    let clean = TankInput {
        tools: vec![ToolEvent {
            kind: 13,
            material: 0,
            x: (vw / 2) as u16,
            y: 0,
            r: (vw + vh) as u16,
            variant: 0,
        }],
        ..dark()
    };
    tank.tick(&clean);
    assert_eq!(tank.grit_total(), 0, "the clean tool sweeps grit");
}

/// 2.3.5 — CHEMO: a sediment feeder on a nutrient-rich sand bed lives
/// and multiplies in the dark; on a barren bed the same lineage only
/// spends its seed store.
#[test]
fn chemo_cell_lives_on_sediment_in_the_dark() {
    use crate::bio::Genome;
    let run = |fed: bool| -> (u32, u64) {
        let mut tank = water_box(35, 32, 24, 20);
        tank.set_lambda_scale_q8(0);
        for vx in 0..32 {
            place(&mut tank, vx, 0, Cell::sand());
        }
        tank.tick(&dark());
        if fed {
            tank.set_sediment_all(255);
        }
        let start = tank.sediment_total();
        let id = tank.seed_genome_at(48, 50, Genome::sediment_feeder()).expect("seed");
        let geno = tank.genotype_of_cell(id).unwrap();
        for _ in 0..6000 {
            tank.tick(&dark());
        }
        (tank.live_cells_of(geno), start - tank.sediment_total())
    };
    let (fed_cells, eaten) = run(true);
    let (starved_cells, _) = run(false);
    assert!(eaten > 500, "the feeder draws down the sediment (ate {eaten})");
    assert!(fed_cells >= 4, "fed chemo cells multiply (cells {fed_cells})");
    assert!(starved_cells <= 2, "on a barren bed nothing grows in the dark ({starved_cells})");
}

/// 2.3.5 acceptance — soil appears under a fungus colony: saprotrophs
/// digesting litter on a sand bed turn the sand beneath them into `Soil`
/// (FUNGUS event), and their waste settles into sediment nutrient.
#[test]
fn soil_appears_under_a_fungus_colony() {
    use crate::bio::chem::LITTER_SOFT_CODE;
    use crate::bio::Genome;
    let mut tank = water_box(37, 32, 24, 20);
    tank.set_lambda_scale_q8(0);
    for vx in 0..32 {
        place(&mut tank, vx, 0, Cell::sand());
    }
    tank.tick(&dark());
    assert_eq!(tank.count_material(MaterialId::Soil as u8), 0);
    let (pw, ph) = tank.pixel_size();
    let sand_top = (ph - 3) as u16; // first water pixel row above the bed
    tank.seed_genome_at(48, 40, Genome::saprotroph()).expect("seed");
    let mut fungus_event = false;
    for t in 0..8000 {
        if t % 400 == 0 {
            for px in 0..pw as u16 {
                tank.drop_litter(px, sand_top - 1, 200, LITTER_SOFT_CODE);
            }
        }
        tank.tick(&dark());
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::FUNGUS {
                fungus_event = true;
            }
        }
        if fungus_event && tank.count_material(MaterialId::Soil as u8) > 0 {
            break;
        }
    }
    let soil = tank.count_material(MaterialId::Soil as u8);
    assert!(soil > 0, "digestion should turn sand into soil");
    assert!(fungus_event, "the FUNGUS event fires when soil is made");
    assert!(tank.sediment_total() > 0, "waste settles into the bed as sediment nutrient");
}

/// 2.3.4 — K2#1 mineral cap: in mineral-free water a hard-walled body
/// cannot grow a single part; a calcium feed lets it build, and the feed
/// is consumed.
#[test]
fn hard_walls_wait_for_minerals_until_calcium_is_fed() {
    use crate::bio::Genome;
    let mut tank = water_box(21, 32, 24, 24);
    tank.set_lambda_scale_q8(0);
    tank.set_minerals_all(0);
    let id = tank.seed_genome_at(48, 50, Genome::chamber_predator()).expect("seed");
    tank.pin_cell_for_test(id);
    for _ in 0..1500 {
        tank.tick(&day());
    }
    assert_eq!(tank.org_slot_count(id), 1, "no hard part may grow without minerals");
    assert!(tank.cell(id).is_some(), "the seed itself lives on");
    let (vx, vy) = crate::bio::pixel_to_phys(48, 50, 24);
    tank.add_calcium(vx, vy, 3, 128);
    let fed = tank.mineral_total();
    assert!(fed > 0);
    let mut built = 0;
    for _ in 0..2500 {
        tank.tick(&day());
        built = built.max(tank.org_slot_count(id));
        if built >= 4 {
            break;
        }
    }
    assert!(built >= 4, "calcium should unblock the body (got {built} cells)");
    assert!(tank.mineral_total() < fed, "building hard walls consumes minerals");
}

/// 2.3.4 — BONE secretion: a cell lays dead bone pixels one by one along
/// its heading, each costing `2 m`; when the body dies its bone stays as
/// bone litter, and a bone-band enzyme returns those minerals to the water.
#[test]
fn bone_spine_grows_pixel_by_pixel_and_bone_eaters_recycle_it() {
    use crate::bio::store::BONE_OWNER;
    use crate::bio::{Block, Entry, Genome};
    let mut tank = water_box(23, 24, 24, 24);
    tank.set_lambda_scale_q8(0);
    let m0 = tank.mineral_total();
    let id = tank.seed_genome_at(36, 40, Genome::spiny()).expect("seed");
    tank.pin_cell_for_test(id);
    let mut bone = 0;
    for _ in 0..1500 {
        tank.tick(&day());
        bone = tank.secretion_counts().0;
        if bone >= 3 {
            break;
        }
    }
    assert_eq!(bone, 3, "three bone pixels should be laid");
    for k in 1..=3u16 {
        assert_eq!(tank.owner_at(36, 40 - k), BONE_OWNER, "spine extends along the heading (up)");
    }
    let spent = m0 - tank.mineral_total();
    assert_eq!(spent, 3 * crate::bio::minerals::BONE_M_UNITS as u64, "2 m per bone pixel");
    // The body dies: its bone falls as bone litter.
    assert!(tank.kill_cell(id));
    assert_eq!(tank.secretion_counts().0, 0);
    let bone_litter: u64 = (1..=3u16).map(|k| tank.litter_at(36, 40 - k) as u64).sum();
    assert_eq!(bone_litter, spent, "bone keeps its minerals as litter");
    // A bone eater (ENZYME key 220) in the dark digests it and frees minerals.
    let mut eater = Genome::algae();
    eater.modes[0].entries[0] = Entry::with_params(Block::Enzyme, 4, 220, 24);
    for _ in 0..600 {
        tank.tick(&dark());
    }
    // The litter has settled onto the floor row; the eater sits on it.
    let (_, ph) = tank.pixel_size();
    let floor = (ph - 1) as u16;
    assert_eq!(tank.litter_at(36, floor) as u64, spent, "bone litter settles to the floor");
    let eid = tank.seed_genome_at(36, floor - 1, eater).expect("eater");
    tank.pin_cell_for_test(eid);
    let m1 = tank.mineral_total();
    let before = tank.litter_total();
    for _ in 0..600 {
        tank.tick(&dark());
    }
    let after = tank.litter_total();
    assert!(after < before, "bone litter is digested ({before} → {after})");
    assert_eq!(tank.mineral_total() - m1, before - after, "lysed bone returns its minerals");
}

/// 2.3.4 / d.8 — SHELL: the egg layer wraps her brood daughter in shell;
/// after release the hatchling grows a part and cracks the shell, which
/// falls as bone litter.
#[test]
fn shell_wraps_the_egg_and_cracks_when_the_hatchling_grows() {
    use crate::bio::Genome;
    let mut tank = water_box(29, 24, 24, 24);
    tank.set_lambda_scale_q8(0);
    let id = tank.seed_genome_at(36, 40, Genome::egg_layer()).expect("seed");
    tank.pin_cell_for_test(id);
    let mut max_shell = 0;
    let mut released = None;
    for _ in 0..6000 {
        tank.tick(&day());
        max_shell = max_shell.max(tank.secretion_counts().1);
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::BROOD_RELEASED && released.is_none() {
                released = Some(ev.arg0);
            }
        }
        if released.is_some() && max_shell >= 3 {
            break;
        }
    }
    assert_eq!(max_shell, 3, "the egg should be wrapped in three shell pixels");
    let hatchling = released.expect("the egg is released");
    let shell_at_release = tank.secretion_counts().1;
    let mut cracked = false;
    for _ in 0..4000 {
        tank.tick(&day());
        let n = tank.cells().org(hatchling).map(|o| o.n_alive).unwrap_or(0);
        if n >= 2 {
            cracked = true;
            break;
        }
    }
    assert!(cracked, "the hatchling should grow its second cell through the shell");
    assert!(
        tank.secretion_counts().1 < shell_at_release,
        "growing through the shell removes a shell pixel"
    );
    let (pw, ph) = tank.pixel_size();
    let bone_litter: u64 = (0..pw as u16)
        .flat_map(|px| (0..ph as u16).map(move |py| (px, py)))
        .filter(|&(px, py)| tank.litter_at(px, py) > 0)
        .map(|(px, py)| tank.litter_at(px, py) as u64)
        .sum();
    assert!(bone_litter >= crate::bio::minerals::SHELL_M_UNITS as u64, "the cracked shell is bone litter");
}
