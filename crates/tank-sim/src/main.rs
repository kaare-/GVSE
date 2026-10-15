//! Headless harness.
//!
//! ```text
//! tank-sim tank --ticks 1000 [--seed 1] [--csv out.csv]
//! tank-sim clock --tick 0
//! tank-sim determinism --ticks 100 --seed 1
//! ```
//!
//! `determinism` runs the preset twice and exits non-zero if the pixel
//! hashes diverge. Same seed + same (empty) input log must match.

use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::process::ExitCode;
use std::time::Instant;

use tank_core::{bio::Genome, day_factor, is_day, phase_name, Cell, Tank, TankInput};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("tank") => cmd_tank(args),
        Some("clock") => cmd_clock(args),
        Some("determinism") => cmd_determinism(args),
        Some("trace") => cmd_trace(args),
        Some("giant-vs-body") => cmd_giant_vs_body(args),
        Some("fuzz") => cmd_fuzz(args),
        _ => {
            eprintln!(
                "usage:\n  tank-sim tank --ticks N [--seed N] [--csv path]\n  tank-sim clock --tick N\n  tank-sim determinism --ticks N --seed N\n  tank-sim trace --ticks N [--seed N] [--every N] [--genome algae|straw|giant|plate16|brooder|sticky|swimmer|saprotroph|predator|stinger|signaller|spiny|egg|chemo] [--cycle] [--dark] [--verbose] [--litter N] [--lambda-q8 N] [--minerals N]\n  tank-sim giant-vs-body --ticks N --seeds N\n  tank-sim fuzz --n N [--seed N]"
            );
            ExitCode::from(2)
        }
    }
}

fn water_tank(seed: u64, w: u32, h: u32, water_rows: i32) -> Tank {
    let mut tank = Tank::try_new(seed, w, h).expect("size");
    for vy in 0..2 {
        for vx in 0..w as i32 {
            tank.place(vx, vy, Cell::sand());
        }
    }
    for vy in 2..water_rows {
        for vx in 0..w as i32 {
            tank.place(vx, vy, Cell::water());
        }
    }
    tank
}

fn genome_by_name(name: &str) -> Genome {
    match name {
        "straw" => Genome::straw(),
        "giant" => Genome::giant(4),
        "plate16" => Genome::plate16(),
        "brooder" => Genome::brooder(),
        "sticky" => Genome::sticky_algae(),
        "swimmer" => Genome::swimmer(),
        "saprotroph" => Genome::saprotroph(),
        "predator" => Genome::chamber_predator(),
        "stinger" => Genome::stinger(),
        "signaller" => Genome::signaller(),
        "spiny" => Genome::spiny(),
        "egg" => Genome::egg_layer(),
        "chemo" => Genome::sediment_feeder(),
        _ => Genome::algae(),
    }
}

/// Population trace for one genome in a 48×64 water tank.
fn cmd_trace(args: impl Iterator<Item = String>) -> ExitCode {
    let args: Vec<String> = args.collect();
    let ticks = flag_u64(args.iter().cloned(), "--ticks").unwrap_or(3000);
    let seed = flag_u64(args.iter().cloned(), "--seed").unwrap_or(1);
    let every = flag_u64(args.iter().cloned(), "--every").unwrap_or(300);
    let name = flag_str(&args, "--genome").unwrap_or_else(|| "algae".into());
    let verbose = args.iter().any(|a| a == "--verbose");
    let cycle = args.iter().any(|a| a == "--cycle");
    let dark = args.iter().any(|a| a == "--dark");
    let litter = flag_u64(args.iter().cloned(), "--litter").unwrap_or(0);
    let lambda = flag_u64(args.iter().cloned(), "--lambda-q8").unwrap_or(256);
    let minerals = flag_u64(args.iter().cloned(), "--minerals");
    let mut tank = water_tank(seed, 48, 64, 50);
    tank.set_lambda_scale_q8(lambda as u16);
    // `--minerals N`: whole-tank mineral field in quarter-m units (0 = none).
    if let Some(m) = minerals {
        tank.set_minerals_all(m.min(255) as u8);
    }
    let g = genome_by_name(&name);
    let id = tank.seed_genome_at(72, 60, g).expect("seed");
    let geno = tank.genotype_of_cell(id).unwrap_or(0);
    // `--litter N`: N pixels of full soft litter on the floor under the seed.
    for k in 0..litter as u16 {
        let px = 60 + (k % 24);
        let py = 60 + 3 + k / 24;
        tank.drop_litter(px, py, 255, tank_core::bio::chem::LITTER_SOFT_CODE);
    }
    let input = TankInput {
        always_day: !cycle,
        light_slider: if dark { 0.0 } else { 1.0 },
        ..TankInput::default()
    };
    println!("tick,cells,orgs,orgs_ge2,genotypes,founder_cells,lineage_cells,max_gen,litter,glucose,minerals,bone,ms_per_tick");
    let mut start = Instant::now();
    for i in 0..ticks {
        tank.tick(&input);
        if (i + 1) % every == 0 || i + 1 == ticks {
            let (orgs, big) = tank.org_counts(2);
            let ms = start.elapsed().as_secs_f64() * 1000.0 / every as f64;
            let (bone, shell) = tank.secretion_counts();
            println!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{ms:.3}",
                tank.tick_count(),
                tank.live_cells(),
                orgs,
                big,
                tank.genotype_count(),
                tank.live_cells_of(geno),
                tank.lineage_cells_of(geno),
                tank.max_generation_of(geno),
                tank.litter_total(),
                tank.glucose_total(),
                tank.mineral_total(),
                bone + shell,
            );
            if verbose {
                let col: Vec<String> = (24..48u16)
                    .map(|py| format!("{}:m{}s{}", py, tank.material_at(72, py), tank.water_sat_at(72, py)))
                    .collect();
                println!("  water column x=72 (py:sat) {}", col.join(" "));
                let dc = tank.death_causes();
                let mut hist = [0u32; 8];
                for (_, c) in tank.cells().iter_alive() {
                    hist[(tank.pixel_light_at(c.px, c.py) / 32) as usize] += 1;
                }
                println!(
                    "  deaths starved={} age={} damage={} shed={}  light histogram (per 32): {:?}",
                    dc[0], dc[1], dc[2], dc[3], hist
                );
                for (id, c) in tank.cells().iter_alive().take(24) {
                    println!(
                        "  cell {id} org={} mode={} depth={} pos=({},{}) {}x{} e={:.2}g age={} stress={} light={}",
                        c.org,
                        c.mode,
                        c.depth,
                        c.px,
                        c.py,
                        c.w,
                        c.h,
                        c.energy as f32 / tank_core::consts::FP as f32,
                        c.age,
                        c.stress,
                        tank.pixel_light_at(c.px, c.py)
                    );
                }
            }
            start = Instant::now();
        }
    }
    ExitCode::SUCCESS
}

/// 2.2.7 gate: a 4×4 lineage and a 16-cell lineage each persist `ticks`
/// in ≥ 50 % of seeds (each lineage alone in its own tank).
fn cmd_giant_vs_body(args: impl Iterator<Item = String>) -> ExitCode {
    let args: Vec<String> = args.collect();
    let ticks = flag_u64(args.iter().cloned(), "--ticks").unwrap_or(100_000);
    let seeds = flag_u64(args.iter().cloned(), "--seeds").unwrap_or(4);
    let mut ok_all = true;
    for name in ["giant", "plate16"] {
        let mut survived = 0;
        for seed in 1..=seeds {
            let mut tank = water_tank(seed, 48, 64, 50);
            let g = genome_by_name(name);
            let id = tank.seed_genome_at(72, 60, g).expect("seed");
            let geno = tank.genotype_of_cell(id).unwrap_or(0);
            let input = TankInput::default();
            for _ in 0..ticks {
                tank.tick(&input);
            }
            let alive = tank.lineage_cells_of(geno);
            let (_, big) = tank.org_counts(2);
            println!(
                "{name} seed={seed} lineage_cells={alive} founder_cells={} cells={} orgs_ge2={big} gen={}",
                tank.live_cells_of(geno),
                tank.live_cells(),
                tank.max_generation_of(geno)
            );
            if alive > 0 {
                survived += 1;
            }
        }
        let pass = survived * 2 >= seeds;
        println!("{name}: survived {survived}/{seeds} → {}", if pass { "PASS" } else { "FAIL" });
        ok_all &= pass;
    }
    if ok_all {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// 2.1.5 / 2.2.6 gate: N chained mutations from every preset genome never
/// produce an invalid genome.
fn cmd_fuzz(args: impl Iterator<Item = String>) -> ExitCode {
    let args: Vec<String> = args.collect();
    let n = flag_u64(args.iter().cloned(), "--n").unwrap_or(1_000_000);
    let seed = flag_u64(args.iter().cloned(), "--seed").unwrap_or(0xF1_55);
    let names = ["algae", "straw", "giant", "plate16", "brooder", "sticky", "swimmer"];
    let per = n / names.len() as u64;
    let start = Instant::now();
    let mut ops = 0u64;
    for (k, name) in names.iter().enumerate() {
        let mut g = genome_by_name(name);
        for i in 0..per {
            g = g.mutate(seed ^ (k as u64) << 32, i);
            ops += 1;
            if !g.is_valid() {
                println!("INVALID after {i} mutations of {name}: {g:?}");
                return ExitCode::from(1);
            }
        }
    }
    println!(
        "fuzz: {ops} mutations across {} genomes valid in {:.1}s → PASS",
        names.len(),
        start.elapsed().as_secs_f64()
    );
    ExitCode::SUCCESS
}

fn cmd_clock(args: impl Iterator<Item = String>) -> ExitCode {
    let tick = flag_u64(args, "--tick").unwrap_or(0);
    println!(
        "tick={tick} phase={} day_factor={:.4} day={}",
        phase_name(tick),
        day_factor(tick),
        is_day(tick)
    );
    ExitCode::SUCCESS
}

fn cmd_tank(args: impl Iterator<Item = String>) -> ExitCode {
    let args: Vec<String> = args.collect();
    let ticks = flag_u64(args.iter().cloned(), "--ticks").unwrap_or(1000);
    let seed = flag_u64(args.iter().cloned(), "--seed").unwrap_or(1);
    let csv = flag_str(&args, "--csv");
    let mut tank = Tank::preset(seed);
    let input = TankInput::default();
    let mut csv_file = csv.map(|path| BufWriter::new(File::create(path).expect("csv")));
    if let Some(file) = csv_file.as_mut() {
        writeln!(file, "tick,phase,day_factor,pixel_hash").ok();
    }
    let start = Instant::now();
    let mut firsts: Vec<(u32, u64)> = Vec::new();
    for i in 0..ticks {
        tank.tick(&input);
        while let Some(ev) = tank.poll_event() {
            if milestone_name(ev.kind).is_some() && !firsts.iter().any(|(k, _)| *k == ev.kind) {
                firsts.push((ev.kind, tank.tick_count()));
            }
        }
        if let Some(file) = csv_file.as_mut() {
            if i % 30 == 29 || i + 1 == ticks {
                writeln!(
                    file,
                    "{},{},{:.4},{:016x}",
                    tank.tick_count(),
                    tank.phase_name(),
                    tank.day_factor_now(),
                    tank.pixel_hash()
                )
                .ok();
            }
        }
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / ticks.max(1) as f64;
    println!(
        "seed={seed} tick={} phase={} day_factor={:.4} hash={:016x} ms_per_tick={ms:.3} sat={} sand={} cells={} genotypes={}",
        tank.tick_count(),
        tank.phase_name(),
        tank.day_factor_now(),
        tank.pixel_hash(),
        tank.sat_sum(),
        tank.count_material(2),
        tank.live_cells(),
        tank.genotype_count()
    );
    let firsts: Vec<String> = firsts
        .iter()
        .map(|(k, t)| format!("{}@{t}", milestone_name(*k).unwrap_or("?")))
        .collect();
    println!("milestones: {}", if firsts.is_empty() { "-".to_string() } else { firsts.join(" ") });
    ExitCode::SUCCESS
}

/// Player-facing milestone names (2.3.8), for the harness summary.
fn milestone_name(kind: u32) -> Option<&'static str> {
    use tank_core::bio::ev;
    Some(match kind {
        ev::LIFELESS => "lifeless",
        ev::GLASS => "glass",
        ev::MS_FIRST_LIFE => "first_life",
        ev::MS_FIRST_DIVISION => "division",
        ev::MS_FIRST_DEATH => "death",
        ev::MS_NEW_COLOUR => "new_colour",
        ev::MS_MULTICELLULAR => "multicellular",
        ev::MS_GIANT => "giant",
        ev::MS_FUNGUS => "fungus",
        ev::MS_HETEROTROPH => "heterotroph",
        ev::MS_PREDATION => "predation",
        ev::MS_POISON => "poison",
        ev::MS_SIGNALLING => "signalling",
        ev::MS_MOVED => "moved",
        _ => return None,
    })
}

fn cmd_determinism(args: impl Iterator<Item = String>) -> ExitCode {
    let args: Vec<String> = args.collect();
    let ticks = flag_u64(args.iter().cloned(), "--ticks").unwrap_or(50) as u32;
    let seed = flag_u64(args.iter().cloned(), "--seed").unwrap_or(1);
    let a = run_hash(seed, ticks);
    let b = run_hash(seed, ticks);
    if a != b {
        eprintln!("diverged seed={seed} ticks={ticks} {a:016x} vs {b:016x}");
        return ExitCode::from(1);
    }
    println!("ok seed={seed} ticks={ticks} hash={a:016x}");
    ExitCode::SUCCESS
}

fn run_hash(seed: u64, ticks: u32) -> u64 {
    let mut tank = Tank::preset(seed);
    let input = TankInput::default();
    for _ in 0..ticks {
        tank.tick(&input);
    }
    tank.pixel_hash()
}

fn flag_u64(args: impl Iterator<Item = String>, name: &str) -> Option<u64> {
    let args: Vec<String> = args.collect();
    flag_str(&args, name).and_then(|s| s.parse().ok())
}

fn flag_str(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
