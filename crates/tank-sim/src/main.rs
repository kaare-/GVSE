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
        Some("gates") => cmd_gates(args),
        Some("soak") => cmd_soak(args),
        Some("scenario") => cmd_scenario(args),
        _ => {
            eprintln!(
                "usage:\n  tank-sim tank --ticks N [--seed N] [--csv path] [--log N]\n  tank-sim clock --tick N\n  tank-sim determinism --ticks N --seed N\n  tank-sim trace --ticks N [--seed N] [--every N] [--genome algae|straw|giant|plate16|brooder|sticky|swimmer|saprotroph|predator|stinger|signaller|spiny|egg|chemo|heater|moss|plant|crown|rooting] [--cycle] [--dark] [--verbose] [--litter N] [--lambda-q8 N] [--minerals N]\n  tank-sim giant-vs-body --ticks N --seeds N\n  tank-sim fuzz --n N [--seed N]\n  tank-sim gates --seeds N --ticks N [--threads N] [--csv path]\n  tank-sim soak --ticks N [--seed N]\n  tank-sim scenario terrarium|cold-night|bloom-crash [--seed N] [--ticks N]"
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
        "heater" => Genome::heater(),
        "moss" => Genome::moss(),
        "plant" => Genome::plant(),
        "crown" => Genome::crown(),
        "rooting" => Genome::rooting(),
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
    let log_every = flag_u64(args.iter().cloned(), "--log").unwrap_or(0);
    let mut tank = Tank::preset(seed);
    let input = TankInput::default();
    let mut csv_file = csv.map(|path| BufWriter::new(File::create(path).expect("csv")));
    if log_every > 0 {
        println!("tick,phase,cells,genotypes,species,bodies,litter,glucose,grit,sediment,minerals,fines_g,starved,aged,damaged,shed,ms");
    }
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
        if log_every > 0 && (i + 1) % log_every == 0 {
            let d = tank.death_causes();
            let (_, bodies) = tank.org_counts(2);
            println!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{:.3}",
                tank.tick_count(),
                tank.phase_name(),
                tank.live_cells(),
                tank.genotype_count(),
                tank.species_count(),
                bodies,
                tank.litter_total(),
                tank.glucose_total(),
                tank.grit_total(),
                tank.sediment_total(),
                tank.mineral_total(),
                tank.fines().iter().sum::<f32>() as u64,
                d[0],
                d[1],
                d[2],
                d[3],
                start.elapsed().as_secs_f64() * 1000.0 / (i + 1) as f64
            );
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

/// One seed of the 2.3.9 statistical gates: the unattended preset tank
/// (one algae, day/night on, nobody touching it).
struct GateRun {
    seed: u64,
    firsts: Vec<(u32, u64)>,
    extinct_at: Option<u64>,
    min_cells: usize,
    end_cells: usize,
    end_genotypes: usize,
    /// Shannon diversity (nats) over live genotypes at the end.
    shannon: f64,
    ms_per_tick: f64,
}

fn gate_run(seed: u64, ticks: u64) -> GateRun {
    let mut tank = Tank::preset(seed);
    let input = TankInput::default();
    let mut firsts: Vec<(u32, u64)> = Vec::new();
    let mut extinct_at = None;
    let mut min_cells = usize::MAX;
    let start = Instant::now();
    for i in 0..ticks {
        tank.tick(&input);
        while let Some(ev) = tank.poll_event() {
            if milestone_name(ev.kind).is_some() && !firsts.iter().any(|(k, _)| *k == ev.kind) {
                firsts.push((ev.kind, tank.tick_count()));
            }
        }
        if i % 100 == 99 {
            let n = tank.live_cells();
            min_cells = min_cells.min(n);
            if n == 0 && extinct_at.is_none() {
                extinct_at = Some(tank.tick_count());
            }
        }
    }
    let ms_per_tick = start.elapsed().as_secs_f64() * 1000.0 / ticks.max(1) as f64;
    let counts = tank.genotype_live_counts();
    let total: u32 = counts.iter().sum();
    let shannon = if total == 0 {
        0.0
    } else {
        counts
            .iter()
            .map(|&c| {
                let p = c as f64 / total as f64;
                -p * p.ln()
            })
            .sum()
    };
    GateRun {
        seed,
        firsts,
        extinct_at,
        min_cells: if min_cells == usize::MAX { 0 } else { min_cells },
        end_cells: tank.live_cells(),
        end_genotypes: counts.len(),
        shannon,
        ms_per_tick,
    }
}

/// `evolution-system.md` §13 regression gates over N seeds, in parallel
/// (each seed is deterministic on its own; the report is sorted by seed).
fn cmd_gates(args: impl Iterator<Item = String>) -> ExitCode {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use tank_core::bio::ev;
    let args: Vec<String> = args.collect();
    let seeds = flag_u64(args.iter().cloned(), "--seeds").unwrap_or(32);
    let ticks = flag_u64(args.iter().cloned(), "--ticks").unwrap_or(200_000);
    let threads = flag_u64(args.iter().cloned(), "--threads")
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(1))
        .max(1);
    let csv = flag_str(&args, "--csv");
    let next = AtomicU64::new(1);
    let runs: Mutex<Vec<GateRun>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let seed = next.fetch_add(1, Ordering::SeqCst);
                if seed > seeds {
                    break;
                }
                let run = gate_run(seed, ticks);
                eprintln!(
                    "seed={} cells={} genotypes={} H={:.2} extinct={:?} ms/tick={:.3} | {}",
                    run.seed,
                    run.end_cells,
                    run.end_genotypes,
                    run.shannon,
                    run.extinct_at,
                    run.ms_per_tick,
                    run.firsts
                        .iter()
                        .map(|(k, t)| format!("{}@{t}", milestone_name(*k).unwrap_or("?")))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                runs.lock().unwrap().push(run);
            });
        }
    });
    let mut runs = runs.into_inner().unwrap();
    runs.sort_by_key(|r| r.seed);
    if let Some(path) = csv {
        let mut file = BufWriter::new(File::create(path).expect("csv"));
        writeln!(
            file,
            "seed,end_cells,min_cells,end_genotypes,shannon,extinct_at,ms_per_tick,new_colour,multicellular,heterotroph,moved,fungus,predation,poison,signalling,giant"
        )
        .ok();
        for r in &runs {
            let at = |k: u32| r.firsts.iter().find(|(kk, _)| *kk == k).map(|(_, t)| t.to_string()).unwrap_or_default();
            writeln!(
                file,
                "{},{},{},{},{:.3},{},{:.3},{},{},{},{},{},{},{},{},{}",
                r.seed,
                r.end_cells,
                r.min_cells,
                r.end_genotypes,
                r.shannon,
                r.extinct_at.map(|t| t.to_string()).unwrap_or_default(),
                r.ms_per_tick,
                at(ev::MS_NEW_COLOUR),
                at(ev::MS_MULTICELLULAR),
                at(ev::MS_HETEROTROPH),
                at(ev::MS_MOVED),
                at(ev::MS_FUNGUS),
                at(ev::MS_PREDATION),
                at(ev::MS_POISON),
                at(ev::MS_SIGNALLING),
                at(ev::MS_GIANT),
            )
            .ok();
        }
    }
    let n = runs.len().max(1) as f64;
    let within = |k: u32, by: u64| -> f64 {
        runs.iter()
            .filter(|r| r.firsts.iter().any(|(kk, t)| *kk == k && *t <= by))
            .count() as f64
            / n
    };
    // Pace targets (§13) in ticks at 30 Hz; gates only judge what the run
    // was long enough to see.
    let gates: [(&str, u32, u64, f64); 4] = [
        ("new colour < 3 min", ev::MS_NEW_COLOUR, 5_400, 0.75),
        ("multicellular < 20 min", ev::MS_MULTICELLULAR, 36_000, 0.75),
        ("heterotroph < 45 min", ev::MS_HETEROTROPH, 81_000, 0.75),
        ("something moved < 2 h", ev::MS_MOVED, 216_000, 0.50),
    ];
    let mut all_pass = true;
    for (name, kind, by, need) in gates {
        let judged = ticks >= by;
        let by = by.min(ticks);
        let frac = within(kind, by);
        let pass = !judged || frac >= need;
        all_pass &= pass;
        println!(
            "gate {name:<24} within {by:>7} ticks: {:>5.1}% (need {:.0}%) → {}",
            frac * 100.0,
            need * 100.0,
            if !judged { "n/a (run too short)" } else if pass { "PASS" } else { "FAIL" }
        );
    }
    let alive = runs.iter().filter(|r| r.extinct_at.is_none()).count() as f64 / n;
    let no_ext_pass = alive >= 0.90;
    all_pass &= no_ext_pass;
    println!(
        "gate {:<24} over {ticks:>7} ticks: {:>5.1}% (need 90%) → {}",
        "no extinction",
        alive * 100.0,
        if no_ext_pass { "PASS" } else { "FAIL" }
    );
    let diverse = runs.iter().filter(|r| r.end_genotypes >= 3).count() as f64 / n;
    let div_pass = diverse >= 0.75;
    all_pass &= div_pass;
    let mean_h = runs.iter().map(|r| r.shannon).sum::<f64>() / n;
    println!(
        "gate {:<24} at the end: {:>5.1}% (need 75%; mean Shannon H {mean_h:.2}) → {}",
        "≥ 3 live genotypes",
        diverse * 100.0,
        if div_pass { "PASS" } else { "FAIL" }
    );
    let mean_ms = runs.iter().map(|r| r.ms_per_tick).sum::<f64>() / n;
    println!(
        "gates: {} seeds × {ticks} ticks, mean {mean_ms:.3} ms/tick ({threads} threads) → {}",
        runs.len(),
        if all_pass { "PASS" } else { "FAIL" }
    );
    if all_pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Long soak of one preset tank: no panic, no NaN in the float fields,
/// no water from nowhere; prints the hash every 100k ticks.
fn cmd_soak(args: impl Iterator<Item = String>) -> ExitCode {
    let args: Vec<String> = args.collect();
    let ticks = flag_u64(args.iter().cloned(), "--ticks").unwrap_or(1_000_000);
    let seed = flag_u64(args.iter().cloned(), "--seed").unwrap_or(1);
    let mut tank = Tank::preset(seed);
    let input = TankInput::default();
    let sat0 = tank.sat_sum();
    let start = Instant::now();
    let mut peak_cells = 0usize;
    for i in 0..ticks {
        tank.tick(&input);
        while tank.poll_event().is_some() {}
        peak_cells = peak_cells.max(tank.live_cells());
        if i % 1000 == 999 {
            if tank.fines().iter().any(|f| !f.is_finite()) {
                println!("soak: NaN in fines at tick {}", tank.tick_count());
                return ExitCode::FAILURE;
            }
            if !tank.day_factor_now().is_finite() {
                println!("soak: NaN day factor at tick {}", tank.tick_count());
                return ExitCode::FAILURE;
            }
            if tank.sat_sum() > sat0 {
                println!("soak: water created at tick {}", tank.tick_count());
                return ExitCode::FAILURE;
            }
        }
        if i % 100_000 == 99_999 {
            println!(
                "soak seed={seed} tick={} hash={:016x} cells={} genotypes={} sat={} ms/tick={:.3}",
                tank.tick_count(),
                tank.pixel_hash(),
                tank.live_cells(),
                tank.genotype_count(),
                tank.sat_sum(),
                start.elapsed().as_secs_f64() * 1000.0 / (i + 1) as f64
            );
        }
    }
    println!(
        "soak: {ticks} ticks, peak cells {peak_cells}, end cells {}, water {} → {} → PASS",
        tank.live_cells(),
        sat0,
        tank.sat_sum()
    );
    ExitCode::SUCCESS
}


fn cmd_scenario(args: impl Iterator<Item = String>) -> ExitCode {
    let args: Vec<String> = args.collect();
    let name = args.first().map(|s| s.as_str()).unwrap_or("");
    let seed = flag_u64(args.iter().cloned(), "--seed").unwrap_or(1);
    let ticks = flag_u64(args.iter().cloned(), "--ticks");
    match name {
        "terrarium" => scenario_terrarium(seed, ticks.unwrap_or(3_000)),
        "cold-night" => scenario_cold_night(seed, ticks.unwrap_or(40_000)),
        "bloom-crash" => scenario_bloom_crash(seed, ticks.unwrap_or(12_000)),
        _ => {
            eprintln!("scenario: terrarium | cold-night | bloom-crash");
            ExitCode::from(2)
        }
    }
}

fn scenario_terrarium(seed: u64, ticks: u64) -> ExitCode {
    let mut tank = Tank::try_new(seed, 32, 32).expect("size");
    tank.set_lambda_scale_q8(0);
    for vx in 0..32 {
        for vy in 0..6 {
            let mut c = Cell::sand();
            c.sat = 200;
            tank.place(vx, vy, c);
        }
        for vy in 6..12 {
            tank.place(vx, vy, Cell::water());
        }
    }
    let moss = tank
        .seed_genome_at(48, (31 - 5) * 3 + 1, Genome::moss())
        .expect("moss");
    let plant = tank
        .seed_genome_at(60, (31 - 5) * 3 + 1, Genome::plant())
        .expect("plant");
    for id in [moss, plant] {
        let o = tank.cell(id).unwrap().org;
        tank.feed_org_full(o);
    }
    let input = TankInput {
        always_day: true,
        ..TankInput::default()
    };
    let mut land = false;
    for _ in 0..ticks {
        tank.tick(&input);
        while let Some(e) = tank.poll_event() {
            if e.kind == tank_core::bio::ev::LIFE_ON_LAND
                || e.kind == tank_core::bio::ev::MS_LIFE_ON_LAND
            {
                land = true;
            }
        }
    }
    let alive = tank.live_cells();
    let ok = alive >= 1 && land;
    println!(
        "scenario terrarium seed={seed} ticks={ticks} cells={alive} life_on_land={land} → {}",
        if ok { "PASS" } else { "FAIL" }
    );
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn scenario_cold_night(seed: u64, ticks: u64) -> ExitCode {
    let mut tank = water_tank(seed, 32, 32, 24);
    tank.set_lambda_scale_q8(0);
    // Pen a heater so its halo holds.
    for dx in -1..=1 {
        for dy in -1..=1 {
            if (dx, dy) != (0, 0) {
                tank.place(16 + dx, 12 + dy, Cell::bedrock());
            }
        }
    }
    let heater = tank
        .seed_genome_at(16 * 3 + 1, (31 - 12) * 3 + 1, Genome::heater())
        .expect("heater");
    let algae = tank
        .seed_genome_at(40, 40, Genome::algae())
        .expect("algae");
    for id in [heater, algae] {
        let o = tank.cell(id).unwrap().org;
        tank.feed_org_full(o);
    }
    let input = TankInput {
        always_day: false,
        t_base: 5.0,
        ..TankInput::default()
    };
    let mut warm = false;
    for _ in 0..ticks {
        tank.tick(&input);
        while let Some(e) = tank.poll_event() {
            if e.kind == tank_core::bio::ev::WARM_BLOODED
                || e.kind == tank_core::bio::ev::MS_WARM_BLOODED
            {
                warm = true;
            }
        }
    }
    let alive = tank.live_cells();
    let ok = alive >= 1;
    println!(
        "scenario cold-night seed={seed} ticks={ticks} cells={alive} warm_blooded={warm} t={} → {}",
        tank.t_tank(),
        if ok { "PASS" } else { "FAIL" }
    );
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn scenario_bloom_crash(seed: u64, ticks: u64) -> ExitCode {
    let mut tank = water_tank(seed, 24, 32, 28);
    let w = tank.visible_size().0 as usize * 3;
    for px in 0..w {
        tank.add_fines(px, 200.0);
    }
    tank.tick(&TankInput::default());
    let cloudy0 = tank.cloud_mean();
    let mut cloudy_ev = false;
    for _ in 0..600 {
        tank.tick(&TankInput::default());
        while let Some(e) = tank.poll_event() {
            if e.kind == tank_core::bio::ev::CLOUDY {
                cloudy_ev = true;
            }
        }
    }
    for _ in 0..ticks.saturating_sub(600) {
        tank.tick(&TankInput::default());
    }
    let clear = tank.cloud_mean();
    let ok = cloudy0 < 0.5 && cloudy_ev && clear > 0.9;
    println!(
        "scenario bloom-crash seed={seed} cloud {cloudy0:.3} → {clear:.3} event={cloudy_ev} → {}",
        if ok { "PASS" } else { "FAIL" }
    );
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
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
        ev::MS_NEW_SPECIES => "new_species",
        ev::MS_WARM_BLOODED => "warm_blooded",
        ev::MS_LIFE_ON_LAND => "life_on_land",
        ev::MS_CUTTING_TOOK_ROOT => "cutting_took_root",
        ev::MS_COLOUR_CROWN => "colour_crown",
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
