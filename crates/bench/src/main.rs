//! Benchmarks against the spec §13 quality targets.
//!
//! Reports generation time for 512² and 1024² grids and one full erosion step,
//! with PASS/FAIL against the targets. Run with `--release`: debug timings
//! are meaningless for the thresholds below.

use geogenesis_core::params::Params;
use geogenesis_core::pipeline::generate_initial;
use geogenesis_core::plates::{build_mosaic, PlateParams};
use geogenesis_core::sim::Simulation;
use geogenesis_core::uplift::{compute_uplift, UpliftParams};
use std::time::Instant;

/// (label, grid, generation budget ms, step budget ms).
const TARGETS: [(&str, u32, u128, u128); 2] = [
    ("512", 512, 500, 2000),
    ("1024", 1024, 3000, 8000),
];

fn main() {
    let mut failures = 0;
    let phased = std::env::var("GG_PHASES").is_ok();
    for (label, size, gen_budget_ms, step_budget_ms) in TARGETS {
        let mut p = Params::default();
        p.world.grid = (size, size);
        p.world.seed = 0xBEAC;
        p.total_yr = 10_000.0;
        p.max_iters = 100;

        let t0 = Instant::now();
        let out = match generate_initial(&p) {
            Ok(o) => o,
            Err(e) => {
                println!("{label}: BUILD FAILED: {e}");
                failures += 1;
                continue;
            }
        };
        let gen_ms = t0.elapsed().as_millis();

        if phased {
            let b = p.world.boundary();
            let extent = p.world.spatial_km * 1000.0;
            let seed = p.world.effective_seed();
            let t = Instant::now();
            let mosaic = build_mosaic(b, &PlateParams::default(), seed, extent);
            let mosaic_ms = t.elapsed().as_millis();
            let t = Instant::now();
            let _ = compute_uplift(&mosaic, b, &UpliftParams::default(), seed);
            let uplift_ms = t.elapsed().as_millis();
            println!("  phases: mosaic {mosaic_ms} ms | uplift {uplift_ms} ms");
        }

        let t1 = Instant::now();
        let mut sim = match Simulation::new(&p) {
            Ok(s) => s,
            Err(e) => {
                println!("{label}: SIM BUILD FAILED: {e}");
                failures += 1;
                continue;
            }
        };
        let sim_ms = t1.elapsed().as_millis();

        let t2 = Instant::now();
        sim.step();
        let step_ms = t2.elapsed().as_millis();
        let _ = out;

        let gen_ok = gen_ms <= gen_budget_ms;
        let step_ok = step_ms <= step_budget_ms;
        if !gen_ok || !step_ok {
            failures += 1;
        }
        println!(
            "{label}²: pipeline {gen_ms} ms (budget {gen_budget_ms}, {}) | \
             sim-build {sim_ms} ms | \
             one step {step_ms} ms (budget {step_budget_ms}, {}) | \
             cells {} | range {:.0}…{:.0} m",
            if gen_ok { "PASS" } else { "FAIL" },
            if step_ok { "PASS" } else { "FAIL" },
            sim.elevation().len(),
            sim.elevation().min(),
            sim.elevation().max(),
        );
    }
    if failures > 0 {
        std::process::exit(1);
    }
}
