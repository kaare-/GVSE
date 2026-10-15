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

use tank_core::{day_factor, is_day, phase_name, Tank, TankInput};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("tank") => cmd_tank(args),
        Some("clock") => cmd_clock(args),
        Some("determinism") => cmd_determinism(args),
        _ => {
            eprintln!(
                "usage:\n  tank-sim tank --ticks N [--seed N] [--csv path]\n  tank-sim clock --tick N\n  tank-sim determinism --ticks N --seed N"
            );
            ExitCode::from(2)
        }
    }
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
    for i in 0..ticks {
        tank.tick(&input);
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
        "seed={seed} tick={} phase={} day_factor={:.4} hash={:016x} ms_per_tick={ms:.3} sat={} sand={}",
        tank.tick_count(),
        tank.phase_name(),
        tank.day_factor_now(),
        tank.pixel_hash(),
        tank.sat_sum(),
        tank.count_material(2)
    );
    ExitCode::SUCCESS
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
