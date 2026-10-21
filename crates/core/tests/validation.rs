//! Independent scientific-validation harness for GeoGenesis.
//!
//! This integration test treats the engine as a black box: it only uses the
//! public API (`Simulation`, `pipeline`, `climate`, `hydrology`, `erosion`,
//! `stats`, `export`) and checks physical/statistical invariants instead of
//! re-stating unit-level implementation details. The goal is to catch
//! regressions that unit tests inside individual modules cannot see, such as a
//! process that silently stops conserving mass when coupled to the others, a
//! climate field that drifts off its parameter, or an exporter that emits a
//! malformed container.
//!
//! # Running
//!
//! ```bash
//! # full harness in release mode (recommended; also prints the measured table)
//! cargo test --release -p geogenesis-core --test validation -- --nocapture --test-threads=1
//!
//! # or through the helper script, which also runs the CLI benchmark:
//! ./scripts/validate.sh
//! ```
//!
//! Cheap checks (statistics, exports, hydrology invariants, no-op cases,
//! climate/biome coherence, deterministic replay) also run in a debug build, so
//! CI covers them on every push. The expensive checks (the 2 Myr × 9-preset
//! stability sweep and the performance smoke) are skipped in debug builds to
//! keep `cargo test --workspace` fast; set `GEOGENESIS_VALIDATION_HEAVY=1` to
//! force them in a debug build.
//!
//! The assertions are deliberately loose where numerical noise, platform
//! differences or parameter extremes can move a number. They encode the
//! *envelope* a scientifically plausible run must stay in, not a golden value.

use std::f64::consts::TAU;
use std::time::Instant;

use geogenesis_core::climate::{self, Biome, Climate, ClimateInputs};
use geogenesis_core::erosion::{self, fluvial, glacial, hillslope, MassBalance};
use geogenesis_core::export::{self, MapColour};
use geogenesis_core::geology::Geology;
use geogenesis_core::grid::{BoundaryKind, Field, Grid};
use geogenesis_core::hydrology::{self, Hydro};
use geogenesis_core::params::Params;
use geogenesis_core::pipeline;
use geogenesis_core::plates::PlateMosaic;
use geogenesis_core::sim::Simulation;
use geogenesis_core::stats;
use geogenesis_core::world::World;

/// Environment variable that forces the heavy checks in a debug build.
const HEAVY_ENV: &str = "GEOGENESIS_VALIDATION_HEAVY";

/// Presets exercised by the long-run stability sweep.
const PRESETS: [&str; 9] = [
    "earthlike",
    "young_orogeny",
    "archipelago",
    "arid_plateau",
    "icehouse",
    "ancient_peneplain",
    "supercontinent",
    "volcanic",
    "riverworld",
];

/// Heavy checks take tens of seconds and are release-only by default.
fn heavy_enabled(name: &str) -> bool {
    if cfg!(debug_assertions) && std::env::var_os(HEAVY_ENV).is_none() {
        println!(
            "[validation] SKIP heavy check `{name}`: debug build (run in release or set {HEAVY_ENV}=1)"
        );
        return false;
    }
    true
}

/// Builds a parameter set for one simulation run.
fn run_params(preset: &str, grid: u32, seed: u64, steps: u32, step_yr: f64) -> Params {
    let mut p = Params::default();
    p.apply_preset(preset)
        .unwrap_or_else(|e| panic!("preset `{preset}` should apply: {e}"));
    p.world.width = grid;
    p.world.height = grid;
    p.world.seed = seed;
    p.time.total_yr = step_yr * f64::from(steps);
    p.time.step_yr = step_yr;
    p.time.max_iters = steps;
    p.time.adaptive_dt = false;
    p.time.history_frames = 4;
    p.normalize();
    p.validate()
        .unwrap_or_else(|e| panic!("preset `{preset}` should validate: {e}"));
    p
}

/// Builds a tiny parameter set for direct process tests.
fn direct_params(width: u32, height: u32, cell_km: f64) -> Params {
    let mut p = Params::default();
    p.world.width = width;
    p.world.height = height;
    p.world.cell_km = cell_km;
    p.normalize();
    p
}

fn climate_of(world: &World, params: &Params) -> Climate {
    climate::compute(&ClimateInputs {
        world,
        params,
        previous: None,
        dt_yr: 0.0,
    })
}

/// Builds the geology needed by the erosion modules, forcing uniform rock.
fn uniform_geology(params: &Params, world: &World) -> Geology {
    let seed = params.world.effective_seed();
    let mosaic = PlateMosaic::build(params, world.grid, seed);
    let mut geology = Geology::build(params, world, &mosaic, seed);
    geology.erodibility.fill_value(1.0);
    geology.solubility.fill_value(0.5);
    geology
}

// ---------------------------------------------------------------------------
// 1. Long-run stability and conservation across presets and seeds
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct RunRow {
    preset: &'static str,
    seed: u64,
    grid: u32,
    steps: u32,
    gen_ms: f64,
    run_ms: f64,
    land_pct: f64,
    relief_m: f64,
    max_m: f64,
    mean_temp_c: f64,
    mean_precip_mm: f64,
    biomes: usize,
    ice_pct: f64,
    closure_rel: f64,
}

fn check_long_run(
    preset: &'static str,
    seed: u64,
    grid: u32,
    failures: &mut Vec<String>,
) -> RunRow {
    let steps = 100u32; // 2 Myr at 20 kyr per step
    let step_yr = 20_000.0;
    let params = run_params(preset, grid, seed, steps, step_yr);

    let started = Instant::now();
    let sim = match Simulation::new(&params) {
        Ok(sim) => sim,
        Err(e) => {
            failures.push(format!("{preset}/seed{seed}: construction failed: {e}"));
            return RunRow {
                preset,
                seed,
                grid,
                steps,
                gen_ms: started.elapsed().as_secs_f64() * 1e3,
                run_ms: 0.0,
                land_pct: 0.0,
                relief_m: 0.0,
                max_m: 0.0,
                mean_temp_c: 0.0,
                mean_precip_mm: 0.0,
                biomes: 0,
                ice_pct: 0.0,
                closure_rel: f64::INFINITY,
            };
        }
    };
    let gen_ms = started.elapsed().as_secs_f64() * 1e3;
    let started = Instant::now();
    let mut sim = sim;
    let ran = sim.step_many(steps);
    let run_ms = started.elapsed().as_secs_f64() * 1e3;

    let context = format!("{preset}/seed{seed}/{grid}x{grid}");
    let mut fail = |message: String| failures.push(format!("{context}: {message}"));

    if ran != steps {
        fail(format!("only {ran} of {steps} steps ran"));
    }
    if sim.state.iteration != u64::from(steps) {
        fail(format!("iteration {} != {steps}", sim.state.iteration));
    }
    let wanted_yr = f64::from(steps) * step_yr;
    if (sim.state.elapsed_yr - wanted_yr).abs() > 1.0 {
        fail(format!(
            "elapsed {:.3} yr != requested {wanted_yr:.0} yr",
            sim.state.elapsed_yr
        ));
    }

    // ---- Numerical health ------------------------------------------------
    let world = &sim.world;
    let elevation = world.elevation_m.as_slice();
    if world.elevation_m.non_finite_count() > 0 {
        fail(format!(
            "{} non-finite elevation cells",
            world.elevation_m.non_finite_count()
        ));
    }
    for (name, count) in [
        ("ice", world.ice_m.non_finite_count()),
        ("soil", world.soil_m.non_finite_count()),
        ("sediment", world.sediment_m.non_finite_count()),
        ("erosion", world.erosion_m.non_finite_count()),
        ("deposition", world.deposition_m.non_finite_count()),
        ("crust", world.crust_m.non_finite_count()),
    ] {
        if count > 0 {
            fail(format!("{count} non-finite cells in {name}"));
        }
    }
    // The repair pass clamps to +/-60 km; touching that bound means a process
    // diverged instead of being physically rebalanced.
    let max_abs = elevation.iter().fold(0.0f64, |acc, z| acc.max(z.abs()));
    if max_abs >= 59_999.0 {
        fail(format!(
            "elevation reached the repair clamp ({max_abs:.0} m)"
        ));
    }
    let (min_z, max_z) = world.relief_range_m();
    let relief = max_z - min_z;
    let envelope = (params.world.vertical_m * 3.0).max(15_000.0);
    if max_z > envelope || min_z < -envelope {
        fail(format!(
            "elevation {min_z:.0}..{max_z:.0} m outside envelope +/-{envelope:.0} m"
        ));
    }
    if relief > 25_000.0 {
        fail(format!("relief {relief:.0} m exceeds 25 km"));
    }

    // ---- Land / sea ------------------------------------------------------
    let land = world.land_fraction();
    if !(0.05..=0.95).contains(&land) {
        fail(format!("land fraction {land:.3} outside 0.05..=0.95"));
    }

    // ---- Mass ledger -----------------------------------------------------
    let ledger = sim.state.ledger;
    let total = ledger.total_volume_m3();
    let closure = ledger.closure_error_m3();
    let closure_rel = if total > 0.0 {
        closure.abs() / total
    } else {
        closure.abs()
    };
    if !closure.is_finite() {
        fail(format!("non-finite ledger closure {closure}"));
    }
    if closure_rel > 1e-6 {
        fail(format!(
            "ledger closure relative {closure_rel:.3e} > 1e-6 (closure {closure:.3} m3 of {total:.3e} m3)"
        ));
    }
    for (name, value) in [
        ("erosion", ledger.erosion_m3),
        ("deposition", ledger.deposition_m3),
        ("export", ledger.export_m3),
        ("storage", ledger.storage_m3),
    ] {
        if !value.is_finite() || value < 0.0 {
            fail(format!(
                "ledger {name} volume {value} is negative/non-finite"
            ));
        }
    }
    if ledger.erosion_m3 <= 0.0 {
        fail("no erosion recorded over the whole run".to_string());
    }

    // ---- Climate snapshot ------------------------------------------------
    // The driver refreshes climate on its own schedule, so recompute it here:
    // the coherence checks below must describe the same surface the run ended
    // on (a stale snapshot can contain cells that crossed sea level since).
    if sim.climate().is_none() {
        fail("no climate snapshot after the run".to_string());
    }
    let climate = climate_of(world, &params);
    if climate.temperature_c.non_finite_count() > 0
        || climate.precipitation_mm.non_finite_count() > 0
        || climate.vegetation.non_finite_count() > 0
    {
        fail("non-finite climate fields".to_string());
    }
    if climate.precipitation_mm.as_slice().iter().any(|p| *p < 0.0) {
        fail("negative precipitation".to_string());
    }
    if climate
        .temperature_c
        .as_slice()
        .iter()
        .any(|t| !(-90.0..=70.0).contains(t))
    {
        fail("temperature outside -90..=70 C".to_string());
    }
    let mean_temp = climate.mean_land_temperature();
    let mean_precip = climate.mean_land_precipitation();
    if !mean_temp.is_finite() || !(-60.0..=50.0).contains(&mean_temp) {
        fail(format!(
            "implausible mean land temperature {mean_temp:.1} C"
        ));
    }
    let requested_precip = params.climate.precipitation_mm;
    if (mean_precip - requested_precip).abs() > requested_precip * 0.10 + 10.0 {
        fail(format!(
            "mean land precipitation {mean_precip:.0} mm/yr drifted from parameter {requested_precip:.0} mm/yr"
        ));
    }

    // Ocean/land biome coherence.
    for i in 0..world.cell_count() {
        let ocean = elevation[i] <= world.sea_level_m;
        let biome = climate.biome.get_index(i);
        if ocean && biome != Biome::Ocean as u8 {
            fail(format!("ocean cell {i} assigned land biome {biome}"));
            break;
        }
        if !ocean && biome == Biome::Ocean as u8 {
            fail(format!("land cell {i} assigned Ocean biome"));
            break;
        }
    }
    let land_fractions = climate.biome_fractions();
    let biome_count = land_fractions.iter().filter(|(_, f)| *f >= 0.01).count();
    if biome_count < 2 {
        fail(format!(
            "degenerate biome mix ({biome_count} classes >= 1% of land)"
        ));
    }
    let biome_sum: f64 = land_fractions.iter().map(|(_, f)| f).sum();
    if (biome_sum - 1.0).abs() > 1e-6 {
        fail(format!("land biome fractions sum to {biome_sum:.6}, not 1"));
    }

    // Statistical sanity of the coupled state.
    let report = sim.stats();
    if !(report.elevation.min_m <= report.elevation.mean_m
        && report.elevation.mean_m <= report.elevation.max_m)
    {
        fail("elevation min/mean/max are not ordered".to_string());
    }
    if !(0.0..=1.0).contains(&report.hypsometry.integral) {
        fail(format!(
            "hypsometric integral {} outside 0..1",
            report.hypsometry.integral
        ));
    }
    if sim.state.repairs > 0 {
        fail(format!("{} repair operations reported", sim.state.repairs));
    }
    if sim.history.is_empty() {
        fail("no history frames captured".to_string());
    }

    run_row(
        preset,
        seed,
        grid,
        steps,
        gen_ms,
        run_ms,
        world,
        Some((&climate, biome_count)),
        closure_rel,
    )
}

fn run_row(
    preset: &'static str,
    seed: u64,
    grid: u32,
    steps: u32,
    gen_ms: f64,
    run_ms: f64,
    world: &World,
    climate: Option<(&Climate, usize)>,
    closure_rel: f64,
) -> RunRow {
    let (min_z, max_z) = world.relief_range_m();
    let (mean_temp_c, mean_precip_mm, biomes) = climate.map_or((0.0, 0.0, 0), |(c, n)| {
        (c.mean_land_temperature(), c.mean_land_precipitation(), n)
    });
    let ice_pct = world.ice_m.fraction_where(|v| v > 0.0) * 100.0;
    RunRow {
        preset,
        seed,
        grid,
        steps,
        gen_ms,
        run_ms,
        land_pct: world.land_fraction() * 100.0,
        relief_m: max_z - min_z,
        max_m: max_z,
        mean_temp_c,
        mean_precip_mm,
        biomes,
        ice_pct,
        closure_rel,
    }
}

#[test]
fn preset_long_run_table() {
    if !heavy_enabled("preset_long_run_table") {
        return;
    }
    // Nine presets, one seed each, plus a second seed for three representative
    // worlds: 12 independent 2 Myr runs at 128².
    let mut configs: Vec<(&'static str, u64)> =
        PRESETS.iter().map(|preset| (*preset, 7u64)).collect();
    configs.push(("earthlike", 11));
    configs.push(("icehouse", 11));
    configs.push(("arid_plateau", 11));

    let mut failures = Vec::new();
    let mut rows = Vec::new();
    let started = Instant::now();
    for (preset, seed) in configs {
        let row = check_long_run(preset, seed, 128, &mut failures);
        println!(
            "  run {:<18} seed {:<3} grid {:<4} steps {:<3} gen {:>6.0} ms  run {:>7.0} ms",
            row.preset, row.seed, row.grid, row.steps, row.gen_ms, row.run_ms
        );
        rows.push(row);
    }
    let total_s = started.elapsed().as_secs_f64();

    println!();
    println!("=== GeoGenesis long-run validation (2 Myr per run, release) ===");
    println!(
        "{:<18} {:>5} {:>5} {:>5} {:>8} {:>8} {:>7} {:>9} {:>8} {:>8} {:>8} {:>7} {:>9} {:>7}",
        "preset",
        "seed",
        "grid",
        "steps",
        "gen ms",
        "run ms",
        "land %",
        "relief m",
        "max m",
        "mean T C",
        "mean P",
        "biomes",
        "ice %",
        "closure"
    );
    for row in &rows {
        println!(
            "{:<18} {:>5} {:>5} {:>5} {:>8.0} {:>8.0} {:>7.2} {:>9.0} {:>8.0} {:>8.2} {:>8.0} {:>7} {:>9.2} {:>9.1e}",
            row.preset,
            row.seed,
            row.grid,
            row.steps,
            row.gen_ms,
            row.run_ms,
            row.land_pct,
            row.relief_m,
            row.max_m,
            row.mean_temp_c,
            row.mean_precip_mm,
            row.biomes,
            row.ice_pct,
            row.closure_rel
        );
    }
    let total_steps: u32 = rows.iter().map(|r| r.steps).sum();
    println!(
        "  {total_steps} simulated steps in {total_s:.1} s ({:.1} ms/step average, all runs included)",
        total_s * 1e3 / f64::from(total_steps.max(1))
    );
    assert!(
        failures.is_empty(),
        "long-run validation failures ({}):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// 2. Deterministic replay
// ---------------------------------------------------------------------------

#[test]
fn deterministic_replay() {
    let params = run_params("earthlike", 64, 1234, 20, 20_000.0);
    let mut first = Simulation::new(&params).expect("first simulation");
    let initial_a = first.elevation().as_slice().to_vec();
    first.step_many(20);
    let final_a = first.elevation().as_slice().to_vec();
    let frames = first.history.len();

    let mut second = Simulation::new(&params).expect("second simulation");
    let initial_b = second.elevation().as_slice().to_vec();
    second.step_many(20);
    let final_b = second.elevation().as_slice().to_vec();

    assert_eq!(
        initial_a, initial_b,
        "same parameters must generate the identical initial world"
    );
    assert_eq!(
        final_a, final_b,
        "same parameters and steps must replay bit-identically"
    );
    assert_eq!(frames, second.history.len());
    assert_eq!(first.state.ledger, second.state.ledger);

    // A different seed must produce a different initial world.
    let mut other_params = params.clone();
    other_params.world.seed = 4321;
    let other = Simulation::new(&other_params).expect("other seed");
    assert_ne!(
        initial_a,
        other.elevation().as_slice(),
        "changing the seed must change the generated world"
    );
}

// ---------------------------------------------------------------------------
// 3. Climate and ecology invariants
// ---------------------------------------------------------------------------

/// A synthetic world with ocean to the west, a north-south ridge, a flat lee
/// plain and one isolated 3.4 km peak, used to exercise the climate model.
fn climate_fixture() -> (World, Params) {
    let mut p = direct_params(64, 64, 10.0);
    let grid = p.world.grid().expect("fixture grid");
    let mut world = World::new(&p.world).expect("fixture world");
    world.elevation_m = Field::from_fn(grid, |x, _| {
        if x < 8 {
            -300.0 // open sea to the west: the prevailing westerlies pick up moisture
        } else if (20..=27).contains(&x) {
            200.0 + (x - 20) as f64 * 380.0 // windward ramp to a 2.9 km ridge
        } else {
            200.0 // lee plain
        }
    });
    // An isolated peak above the lee plain, for the lapse-rate check.
    world.elevation_m.set(44, 32, 3_400.0);
    // Strong orographic / rain-shadow settings so the signal is unambiguous.
    p.climate.wind_dir_deg = 270.0; // from the west, blowing east
    p.climate.orographic_strength = 1.0;
    p.climate.rain_shadow_strength = 1.0;
    p.climate.elev_precip_correlation = 0.0;
    p.climate.subtropical_dryness = 0.0;
    p.climate.continentality = 0.3;
    (world, p)
}

#[test]
fn climate_invariants() {
    let (mut world, p) = climate_fixture();
    let grid = world.grid;
    world.soil_m.fill_value(1.0);
    let climate = climate_of(&world, &p);
    let sea = world.sea_level_m;

    // -- Lapse rate: altitude cools, everything else being equal -----------
    let low = f64::from(climate.temperature_c.get(14, 32));
    let high = f64::from(climate.temperature_c.get(44, 32));
    let drop = low - high;
    assert!(
        drop > 8.0,
        "lapse rate too weak: 200 m -> 3400 m cools by only {drop:.1} C"
    );
    assert!(
        drop < 40.0,
        "lapse rate unphysically strong: {drop:.1} C over 3.2 km"
    );
    for i in 0..grid.cell_count() {
        let t = f64::from(climate.temperature_c.get_index(i));
        assert!(
            t.is_finite() && (-80.0..=70.0).contains(&t),
            "temperature {t}"
        );
        let precip = f64::from(climate.precipitation_mm.get_index(i));
        assert!(precip.is_finite() && (0.0..=12_000.0).contains(&precip));
        let veg = f64::from(climate.vegetation.get_index(i));
        assert!(veg.is_finite() && (0.0..=1.0).contains(&veg));
        let seasonal = f64::from(climate.seasonal_amplitude_c.get_index(i));
        assert!(
            seasonal.is_finite() && (0.0..=45.0).contains(&seasonal),
            "seasonal amplitude {seasonal}"
        );
    }

    // -- Rain shadow: the windward flank is wetter than the lee plain ------
    let windward: f64 = (9..=19)
        .map(|x| f64::from(climate.precipitation_mm.get(x, 32)))
        .sum::<f64>()
        / 11.0;
    let lee: f64 = (34..=60)
        .map(|x| f64::from(climate.precipitation_mm.get(x, 32)))
        .sum::<f64>()
        / 27.0;
    assert!(
        windward > lee * 1.15,
        "rain shadow missing: windward {windward:.0} mm/yr vs lee {lee:.0} mm/yr"
    );

    // -- Land-mean precipitation tracks the parameter ----------------------
    let mean = climate.mean_land_precipitation();
    let requested = p.climate.precipitation_mm;
    assert!(
        (mean - requested).abs() < requested * 0.05 + 5.0,
        "land mean precipitation {mean:.0} mm/yr far from requested {requested:.0} mm/yr"
    );

    // -- Ocean/land biome coherence ---------------------------------------
    let mut ocean_cells = 0usize;
    for i in 0..grid.cell_count() {
        let biome = climate.biome.get_index(i);
        let ocean = world.elevation_m.get_index(i) <= sea;
        if ocean {
            ocean_cells += 1;
            assert_eq!(
                biome,
                Biome::Ocean as u8,
                "ocean cell {i} has land biome {biome}"
            );
        } else {
            assert_ne!(biome, Biome::Ocean as u8, "land cell {i} marked as ocean");
        }
    }
    assert!(ocean_cells > 0, "fixture should contain ocean");

    // -- Biome/temperature/precipitation coherence -------------------------
    for i in 0..grid.cell_count() {
        let biome = Biome::from_u8(climate.biome.get_index(i));
        if biome == Biome::Ocean {
            continue;
        }
        let temp = f64::from(climate.temperature_c.get_index(i));
        let rain = f64::from(climate.precipitation_mm.get_index(i));
        match biome {
            Biome::TropicalRainforest => assert!(
                temp > 14.0 && rain > 700.0,
                "{biome:?} at {temp:.1} C / {rain:.0} mm is too cold/dry"
            ),
            Biome::Desert => assert!(
                rain < 1_500.0,
                "{biome:?} with {rain:.0} mm/yr is not a desert"
            ),
            Biome::Ice => assert!(
                temp < 6.0,
                "{biome:?} at {temp:.1} C is not cold enough for permanent ice"
            ),
            Biome::Wetland => assert!(rain > 700.0, "{biome:?} with {rain:.0} mm/yr is too dry"),
            Biome::BorealForest => assert!(
                temp < 10.0,
                "{biome:?} at {temp:.1} C is too warm for the boreal zone"
            ),
            Biome::Tundra => assert!(temp < 6.0, "{biome:?} at {temp:.1} C is too warm"),
            Biome::TemperateForest
            | Biome::Grassland
            | Biome::Savanna
            | Biome::Mediterranean
            | Biome::Alpine => {}
            Biome::Ocean => unreachable!("ocean handled above"),
        }
        // Alpine/high-altitude classes must sit on high ground.
        if biome == Biome::Alpine {
            assert!(
                world.elevation_m.get_index(i) - sea > 800.0,
                "alpine biome at {:.0} m above sea",
                world.elevation_m.get_index(i) - sea
            );
        }
    }

    // -- Biome fractions ---------------------------------------------------
    let fractions = climate.biome_fractions();
    let sum: f64 = fractions.iter().map(|(_, f)| f).sum();
    assert!(
        (sum - 1.0).abs() < 1e-6,
        "land biome fractions sum to {sum}, not 1"
    );
    assert!(fractions.iter().all(|(_, f)| *f >= 0.0 && *f <= 1.0));
    assert!(
        fractions.iter().any(|(_, f)| *f > 0.0),
        "no land biome assigned"
    );
}

// ---------------------------------------------------------------------------
// 4. Hydrology invariants
// ---------------------------------------------------------------------------

/// Follows the receiver chain from `start`; panics if it cycles or is longer
/// than the grid.
fn drain_path(hydro: &Hydro, start: usize) {
    let mut current = start;
    for _ in 0..=hydro.grid.cell_count() {
        match hydro.receiver_of(current) {
            None => return,
            Some(next) => {
                assert_ne!(next, current, "self-receiver at cell {current}");
                current = next;
            }
        }
    }
    panic!("receiver chain from cell {start} did not terminate (cycle?)");
}

#[test]
fn hydrology_invariants() {
    let params = run_params("earthlike", 96, 21, 5, 20_000.0);
    let mut sim = Simulation::new(&params).expect("simulation");
    sim.step_many(5);
    let world = &sim.world;
    let climate = sim.climate().expect("climate");
    let hydro = hydrology::route(world, &params, Some(climate));
    let grid = world.grid;
    let n = grid.cell_count();
    let cell_area = grid.cell_area_m2();
    let sea = world.sea_level_m;

    // -- Filled surface and receivers -------------------------------------
    for i in 0..n {
        let filled = hydro.filled_m.get_index(i);
        let raw = world.elevation_m.get_index(i);
        assert!(
            filled >= raw - 1e-6,
            "cell {i}: filled {filled:.3} below raw elevation {raw:.3}"
        );
        assert!(filled.is_finite(), "cell {i}: non-finite filled elevation");
        if let Some(receiver) = hydro.receiver_of(i) {
            let target = hydro.filled_m.get_index(receiver);
            assert!(
                target <= filled + 1e-6,
                "cell {i}: receiver {receiver} is uphill in the filled surface ({target:.3} > {filled:.3})"
            );
        }
    }

    // -- Acyclic receiver graph, every cell reaches an outlet ---------------
    for i in 0..n {
        drain_path(&hydro, i);
    }
    let outlets = (0..n).filter(|&i| hydro.receiver_of(i).is_none()).count();
    assert!(outlets > 0, "no outlets in a world with ocean");

    // -- Lakes: lake depth is exactly the fill above the raw surface -------
    let mut lake_cells = 0usize;
    for i in 0..n {
        let depth = f64::from(hydro.lake_depth_m.get_index(i));
        let fill = hydro.filled_m.get_index(i) - world.elevation_m.get_index(i);
        assert!(depth >= 0.0 && depth.is_finite());
        if depth > 0.0 {
            lake_cells += 1;
            assert!(
                (depth - fill).abs() <= 0.05 + fill.abs() * 1e-4,
                "cell {i}: lake depth {depth} does not match fill {fill}"
            );
            assert!(
                world.elevation_m.get_index(i) > sea - 1e-6,
                "cell {i}: lake below sea level"
            );
        } else {
            assert!(
                fill <= 0.02 + 1e-6,
                "cell {i}: filled {fill:.4} m above surface but not a lake"
            );
        }
    }

    // -- Discharge conservation -------------------------------------------
    let mut donor_discharge = vec![0.0f64; n];
    let mut donor_cells = vec![0.0f64; n];
    for i in 0..n {
        if let Some(receiver) = hydro.receiver_of(i) {
            donor_discharge[receiver] += hydro.discharge_m3_yr.get_index(i);
            donor_cells[receiver] += hydro.accumulation_cells.get_index(i);
        }
    }
    let mut total_local = 0.0f64;
    for i in 0..n {
        let discharge = hydro.discharge_m3_yr.get_index(i);
        assert!(discharge.is_finite() && discharge >= 0.0);
        let local = discharge - donor_discharge[i];
        assert!(
            local >= -1e-9 * discharge.abs().max(1.0),
            "cell {i}: local runoff {local} is negative"
        );
        total_local += local;
        let accumulation = hydro.accumulation_cells.get_index(i);
        let local_cells = accumulation - donor_cells[i];
        assert!(
            local_cells >= -1e-9,
            "cell {i}: accumulation {accumulation} smaller than its donors"
        );
    }
    let expected_water = cell_area * n as f64 * hydro.mean_runoff_mm / 1000.0;
    assert!(expected_water > 0.0, "no runoff in the world");
    let relative = (total_local - expected_water).abs() / expected_water;
    assert!(
        relative < 1e-9,
        "discharge not conserved: measured {total_local:.6e} m3/yr, expected {expected_water:.6e} m3/yr (rel {relative:.2e})"
    );

    // -- Drainage area is the accumulated cell area on a planar grid ------
    for i in 0..n {
        let area = hydro.drainage_area_m2.get_index(i);
        let expected = hydro.accumulation_cells.get_index(i) * cell_area;
        assert!(
            (area - expected).abs() <= expected.max(1.0) * 1e-9,
            "cell {i}: drainage area {area} != accumulation*cell area {expected}"
        );
    }

    // -- Channel mask ------------------------------------------------------
    let threshold = f64::from(params.hydrology.channel_init_cells.max(1));
    let mut channels = 0usize;
    let mut major = 0usize;
    for i in 0..n {
        let mask = hydro.river_mask.get_index(i);
        assert!(mask <= 2, "river mask value {mask} out of range");
        if mask > 0 {
            channels += 1;
            assert!(
                hydro.accumulation_cells.get_index(i) >= threshold,
                "cell {i}: channel mask set below the channel threshold"
            );
            if mask == 2 {
                major += 1;
            }
        }
    }
    assert!(channels > 0, "no channels identified");
    assert!(major <= channels);
    println!(
        "  hydrology: {} cells, {} outlets, {} channel cells ({major} major), {} lake cells, mean runoff {:.1} mm/yr",
        n, outlets, channels, lake_cells, hydro.mean_runoff_mm
    );

    // -- Channel mask monotonic with the initiation threshold -------------
    let mut previous = usize::MAX;
    for threshold in [2u32, 10, 40, 200] {
        let mut changed = params.clone();
        changed.hydrology.channel_init_cells = threshold;
        let routed = hydrology::route(world, &changed, Some(climate));
        let count = routed.channel_cells();
        assert!(
            count <= previous,
            "raising channel_init_cells to {threshold} increased channel cells ({count} > {previous})"
        );
        previous = count;
    }
}

#[test]
fn hydrology_identifies_lakes_in_a_closed_basin() {
    let p = direct_params(48, 48, 10.0);
    let grid = p.world.grid().expect("grid");
    let mut world = World::new(&p.world).expect("world");
    // Ocean margin, a ring wall and a low interior basin: the basin must fill
    // to the spill level and be reported as a lake.
    world.elevation_m = Field::from_fn(grid, |x, y| {
        let dx = x as f64 - 24.0;
        let dy = y as f64 - 24.0;
        let distance = dx.hypot(dy);
        if distance < 4.0 {
            600.0
        } else if distance < 8.0 {
            1_500.0
        } else {
            -300.0
        }
    });
    let hydro = hydrology::route(&world, &p, None);
    let center = grid.index(24, 24);
    let depth = hydro.lake_depth_m.get_index(center);
    assert!(depth > 0.0, "basin centre is not a lake: depth {depth}");
    assert!(
        hydro.filled_m.get_index(center) > world.elevation_m.get_index(center),
        "filled surface did not rise into the basin"
    );
    // The spill level must be the rim, not the basin floor.
    assert!(
        (hydro.filled_m.get_index(center) - 1_500.0).abs() < 1.0,
        "lake level {:.0} m is not the 1500 m rim",
        hydro.filled_m.get_index(center)
    );
    // The wall itself is not submerged.
    let wall = grid.index(24, 17);
    assert_eq!(hydro.lake_depth_m.get_index(wall), 0.0);
    // Every lake cell drains out of the basin eventually.
    for i in 0..grid.cell_count() {
        if hydro.lake_depth_m.get_index(i) > 0.0 {
            drain_path(&hydro, i);
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Erosion laws
// ---------------------------------------------------------------------------

/// Builds an east-flowing chain (one per row) with a synthetic routing that is
/// valid by construction. Used to test the stream-power law independently of
/// the real routing implementation.
fn synthetic_chain(
    cols: usize,
    rows: usize,
    cell_m: f64,
    params: &Params,
) -> (World, Hydro, Geology, Climate) {
    let grid =
        Grid::new(cols as u32, rows as u32, cell_m, BoundaryKind::Rectangular).expect("chain grid");
    let mut p = params.clone();
    p.world.width = cols as u32;
    p.world.height = rows as u32;
    p.world.cell_km = cell_m / 1000.0;
    let mut world = World::new(&p.world).expect("chain world");
    let cell_area = grid.cell_area_m2();
    let n = grid.cell_count();

    let mut receiver = vec![-1i32; n];
    let mut flow_dir = Field::<u8>::filled(grid, 255);
    let mut accumulation = Field::<f64>::zeros(grid);
    let mut drainage = Field::<f64>::zeros(grid);
    let river_mask = Field::<u8>::filled(grid, 1);
    let mut order_downstream = Vec::with_capacity(n);
    for y in 0..rows {
        for x in (0..cols).rev() {
            let i = grid.index(x, y);
            order_downstream.push(i as u32);
            if x + 1 < cols {
                receiver[i] = (i + 1) as i32;
                flow_dir.set_index(i, 4); // D8 index 4 = east
            }
            accumulation.set_index(i, (x + 1) as f64);
            drainage.set_index(i, (x + 1) as f64 * cell_area);
        }
    }
    let order_upstream: Vec<u32> = (0..n as u32).collect();
    // A gentle initial eastward slope; the uplift loop starts the transient.
    world.elevation_m = Field::from_fn(grid, |x, _| 500.0 - x as f64 * 0.5);
    let hydro = Hydro {
        grid,
        filled_m: world.elevation_m.clone(),
        receiver,
        flow_dir,
        accumulation_cells: accumulation,
        drainage_area_m2: drainage,
        discharge_m3_yr: Field::<f64>::zeros(grid),
        lake_depth_m: Field::<f32>::zeros(grid),
        river_mask,
        order_upstream,
        order_downstream,
        mean_runoff_mm: 0.0,
        mfd: None,
    };
    let geology = uniform_geology(&p, &world);
    let climate = climate_of(&world, &p);
    (world, hydro, geology, climate)
}

/// Measures the steady-state slope-area exponent of the detachment-limited
/// stream-power law: `S = (U/K)^(1/n) A^(-m/n)`.
///
/// The chain starts from the analytic steady-state profile, so the test does
/// not have to wait out a geological transient. Two hundred uplift + incision
/// steps must keep that profile stationary (`E = U`); the fitted exponent is
/// then read off the profile. A wrong implicit solver or a wrong erodibility
/// chain would make the profile drift and the exponent deviate.
fn measure_slope_area_exponent(m: f64, n: f64) -> f64 {
    const COLS: usize = 64;
    const ROWS: usize = 16;
    const CELL_M: f64 = 1_000.0;
    const UPLIFT_M_YR: f64 = 1.0e-4;
    const DT_YR: f64 = 1_000.0;
    const ITERATIONS: usize = 200;
    const K: f64 = 1.0e-5;

    let mut params = direct_params(COLS as u32, ROWS as u32, CELL_M / 1000.0);
    params.fluvial.law = 0.0;
    params.fluvial.kf = K;
    params.fluvial.area_exponent = m;
    params.fluvial.slope_exponent = n;
    params.fluvial.bank_erosion = 0.0;
    params.fluvial.incision_threshold = 0.0;
    params.fluvial.max_incision_m = 2_000.0;
    params.ecology.root_strength = 0.0;
    params.normalize();

    let (mut world, hydro, geology, climate) = synthetic_chain(COLS, ROWS, CELL_M, &params);
    let grid = world.grid;
    let cell_area = grid.cell_area_m2();
    // Analytic steady state: S(x) = (U/K)^(1/n) A(x)^(-m/n).
    let base = 500.0;
    let outlet = COLS - 1;
    for y in 0..ROWS {
        world.elevation_m.set(outlet, y, base);
        for x in (0..outlet).rev() {
            let area = (x + 1) as f64 * cell_area;
            let slope = (UPLIFT_M_YR / K).powf(1.0 / n) * area.powf(-m / n);
            let next = world.elevation_m.get(x + 1, y);
            world.elevation_m.set(x, y, next + slope * CELL_M);
        }
    }
    let analytic = world.elevation_m.as_slice().to_vec();
    for _ in 0..ITERATIONS {
        // Uniform rock uplift with a fixed base level at the eastern outlet.
        for y in 0..ROWS {
            for x in 0..COLS - 1 {
                let i = grid.index(x, y);
                world
                    .elevation_m
                    .set_index(i, world.elevation_m.get_index(i) + UPLIFT_M_YR * DT_YR);
            }
        }
        let _ = fluvial::apply(&mut world, &params, &hydro, &climate, &geology, DT_YR);
    }
    // The steady state must not drift: each uplift increment is removed by the
    // implicit incision step.
    let mut drift = 0.0f64;
    for (actual, expected) in world.elevation_m.as_slice().iter().zip(&analytic) {
        drift = drift.max((actual - expected).abs());
    }
    assert!(
        drift < 1.0e-3,
        "m={m} n={n}: steady-state profile drifted by {drift:.3e} m over {ITERATIONS} steps"
    );
    // Fit log(S) = a + b log(A) over interior cells (the last column is the
    // fixed base level).
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xx = 0.0;
    let mut sum_xy = 0.0;
    let mut count = 0.0f64;
    for x in 2..COLS - 2 {
        let z0 = world.elevation_m.get(x, 1);
        let z1 = world.elevation_m.get(x + 1, 1);
        let slope = (z0 - z1) / CELL_M;
        assert!(
            slope > 0.0,
            "m={m} n={n}: non-positive slope {slope} at x={x}"
        );
        let area = (x + 1) as f64 * cell_area;
        let lx = area.ln();
        let ly = slope.ln();
        sum_x += lx;
        sum_y += ly;
        sum_xx += lx * lx;
        sum_xy += lx * ly;
        count += 1.0;
    }
    let denominator = count * sum_xx - sum_x * sum_x;
    assert!(denominator.abs() > 1e-12, "degenerate regression");
    (count * sum_xy - sum_x * sum_y) / denominator
}

#[test]
fn detachment_limited_slope_area_exponent() {
    for (m, n) in [(0.5, 1.0), (0.4, 1.0), (0.7, 1.0), (0.5, 2.0)] {
        let expected = -m / n;
        let fitted = measure_slope_area_exponent(m, n);
        println!("  slope-area: m={m} n={n}: fitted exponent {fitted:.4}, expected {expected:.4}");
        // The chain starts at the analytic steady state and its area grows
        // linearly over more than an order of magnitude, so the fit should be
        // tight; 1% absolute tolerance covers the endpoint rounding.
        assert!(
            (fitted - expected).abs() < 0.01,
            "slope-area exponent for m={m}, n={n}: fitted {fitted:.4} vs expected {expected:.4}"
        );
    }
}

#[test]
fn hillslope_diffusion_decay_matches_analytic_solution() {
    let mut params = direct_params(32, 32, 1.0);
    params.world.boundary = BoundaryKind::Toroidal;
    params.hillslope.diffusivity = 0.02;
    params.hillslope.nonlinearity = 0.0;
    params.hillslope.critical_slope_deg = 89.0;
    params.hillslope.soil_production = 0.0;
    params.ecology.root_strength = 0.0;
    let grid = params.world.grid().expect("grid");
    let mut world = World::new(&params.world).expect("world");
    let dx = grid.cell_size_m();
    let wavelength = 4.0 * dx;
    let k = TAU / wavelength;
    world.elevation_m = Field::from_fn(grid, |x, _| 500.0 + 50.0 * (k * x as f64 * dx).sin());
    let climate = climate_of(&world, &params);
    let geology = uniform_geology(&params, &world);

    // Discrete dispersion relation of the engine's nine-point stencil for a
    // mode varying along x: per application the amplitude is multiplied by
    // `1 - D dt 4(1 - cos(k dx)) / dx^2`.
    let dt = 1.0e6;
    let amplitude_before = world.elevation_m.get(1, 3) - world.elevation_m.get(0, 3);
    let dispersion = 4.0 * (1.0 - (k * dx).cos()) / (dx * dx);
    let expected_factor = 1.0 - params.hillslope.diffusivity * dt * dispersion;
    assert!(
        expected_factor > 0.0 && expected_factor < 1.0,
        "test setup: unstable or non-decaying factor {expected_factor}"
    );
    for _ in 0..2 {
        hillslope::apply(&mut world, &params, &climate, &geology, dt);
    }
    let amplitude_after = world.elevation_m.get(1, 3) - world.elevation_m.get(0, 3);
    let measured = amplitude_after / amplitude_before;
    let expected = expected_factor * expected_factor;
    println!("  diffusion: measured decay {measured:.6}, analytic {expected:.6} (dt {dt:.1e} yr)");
    assert!(
        (measured - expected).abs() < 2.0e-3,
        "diffusion decay {measured:.6} != analytic {expected:.6}"
    );
    // The mean is preserved by pure diffusion.
    let mean_after = world.elevation_m.mean().expect("mean");
    assert!(
        (mean_after - 500.0).abs() < 1.0e-6,
        "diffusion changed the mean: {mean_after}"
    );
}

#[test]
fn glacial_ice_forms_only_above_the_snowline() {
    let mut params = direct_params(48, 48, 5.0);
    params.climate.mean_temp_c = -18.0;
    params.glacial.ela_m = 2_500.0;
    params.normalize();
    let grid = params.world.grid().expect("grid");
    let mut world = World::new(&params.world).expect("world");
    world.elevation_m.map_in_place(|i, _| {
        let (x, y) = grid.coords(i);
        let dx = x as f64 - 24.0;
        let dy = y as f64 - 24.0;
        4_000.0 - dx.hypot(dy) * 250.0
    });
    let climate = climate_of(&world, &params);
    let _ = glacial::apply(&mut world, &params, &climate, 20_000.0);

    let max_ice = world.ice_m.max_value().expect("ice field");
    assert!(max_ice > 0.0, "no ice formed in a cold, high world");
    assert!(
        max_ice <= params.glacial.max_ice_thickness_m + 1e-6,
        "ice thickness {max_ice} exceeds max_ice_thickness_m {}",
        params.glacial.max_ice_thickness_m
    );
    let sea = world.sea_level_m;
    let mut ice_cells = 0usize;
    for i in 0..grid.cell_count() {
        let ice = f64::from(world.ice_m.get_index(i));
        if ice <= 0.0 {
            continue;
        }
        ice_cells += 1;
        // Ice cannot sit on the ocean.
        assert!(
            world.elevation_m.get_index(i) > sea,
            "ice on a submarine cell {i}"
        );
        // ... nor where the annual mean is warm: melt/sublimation dominates.
        let temp = f64::from(climate.temperature_c.get_index(i));
        assert!(temp < 6.0, "ice at a cell with annual mean {temp:.1} C");
    }
    assert!(ice_cells > 0);
    // The warm lowlands stay ice-free.
    let lowland = grid.index(24, 2);
    assert_eq!(
        world.ice_m.get_index(lowland),
        0.0,
        "ice formed at the low, warm edge of the world"
    );
}

// ---------------------------------------------------------------------------
// 6. Filter / no-op behaviour of individual processes
// ---------------------------------------------------------------------------

fn no_op_fixture() -> (World, Params, Climate, Hydro, Geology) {
    let p = direct_params(32, 32, 5.0);
    let grid = p.world.grid().expect("grid");
    let mut world = World::new(&p.world).expect("world");
    world.elevation_m = Field::from_fn(grid, |x, _| {
        if x < 8 {
            -200.0
        } else {
            300.0 + (x - 8) as f64 * 10.0
        }
    });
    for i in 0..world.cell_count() {
        world.soil_m.set_index(i, 1.0);
        world.sediment_m.set_index(i, 0.5);
    }
    let climate = climate_of(&world, &p);
    let hydro = hydrology::route(&world, &p, Some(&climate));
    let geology = uniform_geology(&p, &world);
    (world, p, climate, hydro, geology)
}

/// Runs one process on a copy of the fixture and asserts it was a no-op.
fn assert_no_op(
    label: &str,
    mutate: impl FnOnce(&mut World, &Params, &Climate, &Hydro, &Geology) -> MassBalance,
) {
    let (mut world, params, climate, hydro, geology) = no_op_fixture();
    let before = world.elevation_m.as_slice().to_vec();
    let balance = mutate(&mut world, &params, &climate, &hydro, &geology);
    assert_eq!(
        world.elevation_m.as_slice(),
        before,
        "{label}: elevation changed"
    );
    assert_eq!(balance.erosion_m3, 0.0, "{label}: erosion recorded");
    assert_eq!(balance.deposition_m3, 0.0, "{label}: deposition recorded");
    assert_eq!(balance.export_m3, 0.0, "{label}: export recorded");
    assert!(
        balance.is_conservative(1e-9),
        "{label}: ledger does not close: {balance:?}"
    );
}

#[test]
fn process_no_op_cases() {
    // Aeolian: wind below the threshold shear velocity moves no sand.
    assert_no_op("aeolian below threshold", |world, p, climate, _, _| {
        let mut calm = p.clone();
        calm.aeolian.threshold_wind_ms = 30.0;
        erosion::aeolian::apply(world, &calm, climate, 10_000.0)
    });
    // Aeolian: transport disabled by its strength parameter.
    assert_no_op("aeolian strength 0", |world, p, climate, _, _| {
        let mut disabled = p.clone();
        disabled.aeolian.erosion_strength = 0.0;
        erosion::aeolian::apply(world, &disabled, climate, 10_000.0)
    });
    // Coastal: zero wave erosivity.
    assert_no_op("coastal wave erosivity 0", |world, p, climate, _, _| {
        let mut disabled = p.clone();
        disabled.coastal.wave_erosivity = 0.0;
        erosion::coastal::apply(world, &disabled, climate, 10_000.0)
    });
    // Chemical: zero dissolution coefficient.
    assert_no_op(
        "chemical dissolution 0",
        |world, p, climate, hydro, geology| {
            let mut disabled = p.clone();
            disabled.chemical.dissolution_coefficient = 0.0;
            erosion::chemical::apply(world, &disabled, climate, hydro, geology, 10_000.0)
        },
    );
}

#[test]
fn active_processes_close_their_ledgers() {
    let (mut world, mut params, climate, hydro, geology) = no_op_fixture();
    params.aeolian.erosion_strength = 0.6;
    params.climate.wind_speed_ms = 16.0;
    params.coastal.wave_erosivity = 0.6;
    params.chemical.dissolution_coefficient = 1.0;
    params.glacial.ela_m = 0.0;
    params.glacial.max_ice_thickness_m = 200.0;
    let before = world.elevation_m.as_slice().to_vec();
    let dt = 10_000.0f64;
    let mut total = MassBalance::default();
    for (name, balance) in [
        (
            "aeolian",
            erosion::aeolian::apply(&mut world, &params, &climate, dt),
        ),
        (
            "coastal",
            erosion::coastal::apply(&mut world, &params, &climate, dt),
        ),
        (
            "chemical",
            erosion::chemical::apply(&mut world, &params, &climate, &hydro, &geology, dt),
        ),
        (
            "glacial",
            erosion::glacial::apply(&mut world, &params, &climate, dt),
        ),
    ] {
        assert!(
            balance.is_conservative(1e-6),
            "{name}: ledger does not close: {balance:?}"
        );
        assert!(
            balance.erosion_m3 >= 0.0 && balance.deposition_m3 >= 0.0 && balance.export_m3 >= 0.0,
            "{name}: negative ledger volume: {balance:?}"
        );
        total.add_assign(&balance);
        println!(
            "  {name}: eroded {:.3e} m3, deposited {:.3e} m3, exported {:.3e} m3",
            balance.erosion_m3, balance.deposition_m3, balance.export_m3
        );
    }
    assert!(total.erosion_m3 > 0.0, "no process moved any material");
    let changed = world
        .elevation_m
        .as_slice()
        .iter()
        .zip(&before)
        .filter(|(a, b)| (*a - *b).abs() > 1e-9)
        .count();
    assert!(changed > 0, "no elevation changed despite recorded erosion");
    assert_eq!(world.elevation_m.non_finite_count(), 0);
}

// ---------------------------------------------------------------------------
// 7. Statistics invariants
// ---------------------------------------------------------------------------

#[test]
fn statistics_invariants() {
    let params = run_params("earthlike", 96, 5, 10, 20_000.0);
    let mut sim = Simulation::new(&params).expect("simulation");
    sim.step_many(10);
    let world = &sim.world;
    // Recompute the diagnostic fields from the current surface so the report
    // and the fields it is compared against describe the same snapshot.
    let climate = climate_of(world, &params);
    let hydro = hydrology::route(world, &params, Some(&climate));
    let report = stats::compute(world, &climate, &hydro, sim.geology());
    let grid = world.grid;
    let cells = grid.cell_count();

    assert_eq!(report.grid_width, grid.width());
    assert_eq!(report.grid_height, grid.height());
    assert!((report.cell_km - grid.cell_size_m() / 1000.0).abs() < 1e-12);

    // Ordering / quantiles.
    let e = &report.elevation;
    assert!(e.min_m <= e.mean_m && e.mean_m <= e.max_m);
    assert!(e.std_m >= 0.0 && e.std_m.is_finite());
    assert!(e.p10_m <= e.median_m && e.median_m <= e.p90_m);
    assert!(e.min_m <= e.p10_m && e.p90_m <= e.max_m);
    assert_eq!(e.cells, cells);
    assert!((report.max_relief_m - (e.max_m - e.min_m)).abs() < 1e-9);
    assert!(report.slope.mean_deg <= report.slope.max_deg);
    assert!(report.slope.p90_deg <= report.slope.max_deg + 1e-9);
    assert!(report.slope.mean_deg >= 0.0);

    // Hypsometry.
    let h = &report.hypsometry;
    assert!((h.land_fraction - world.land_fraction()).abs() < 1e-12);
    assert!((h.ocean_fraction - (1.0 - h.land_fraction)).abs() < 1e-12);
    assert!((0.0..=1.0).contains(&h.integral));
    // The discrete 21-bin curve is not an exact unit integral, but a real
    // hypsometric integral sits well inside the unit interval.
    assert!(
        (0.05..=0.95).contains(&h.integral),
        "hypsometric integral {} outside a plausible 0.05..0.95",
        h.integral
    );
    // The curve runs from the summit (height 1, almost no area above) down to
    // the lowest land (height 0, all land above), so height decreases while the
    // area-above fraction increases.
    assert_eq!(h.curve.len(), 21);
    for pair in h.curve.windows(2) {
        assert!(
            pair[1].1 >= pair[0].1 - 1e-9,
            "hypsometric curve is not monotone: {:?} -> {:?}",
            pair[0],
            pair[1]
        );
        assert!(
            pair[1].0 <= pair[0].0 + 1e-9,
            "hypsometric curve height is not monotone: {:?} -> {:?}",
            pair[0],
            pair[1]
        );
        assert!((0.0..=1.0).contains(&pair[0].0));
        assert!((0.0..=1.0).contains(&pair[0].1));
    }
    assert!(
        h.curve[0].1 <= 0.2,
        "hypsometric curve does not start near zero area above the summit: {:?}",
        h.curve[0]
    );
    assert!(
        h.curve[20].1 >= 0.999,
        "hypsometric curve does not end with all land above the base: {:?}",
        h.curve[20]
    );

    // Area accounting.
    let expected_land_km2 = h.land_fraction * cells as f64 * (grid.cell_size_m() / 1000.0).powi(2);
    assert!(
        (report.land_area_km2 - expected_land_km2).abs() <= expected_land_km2 * 1e-12,
        "land area {} != {}",
        report.land_area_km2,
        expected_land_km2
    );

    // Biome shares are fractions of the whole grid and sum to the land area.
    let biome_sum: f64 = report.biomes.iter().map(|b| b.fraction).sum();
    assert!(
        (biome_sum - h.land_fraction).abs() < 1e-6,
        "biome fractions sum to {biome_sum}, expected land fraction {}",
        h.land_fraction
    );
    assert!(report
        .biomes
        .iter()
        .all(|b| (0.0..=1.0).contains(&b.fraction)));
    assert!(report.biomes.iter().all(|b| !b.key.is_empty()));

    // Climate/hydrology consistency with the report.
    assert!((report.mean_temp_c - climate.mean_land_temperature()).abs() < 1e-9);
    assert!((report.mean_precip_mm - climate.mean_land_precipitation()).abs() < 1e-9);
    assert_eq!(report.channel_cells, hydro.channel_cells());
    assert_eq!(report.mean_erosion_m, world.erosion_m.mean().unwrap_or(0.0));
    assert_eq!(
        report.mean_sediment_m,
        world.sediment_m.mean().unwrap_or(0.0)
    );
    assert!(report.mean_ice_m >= 0.0 && report.mean_ice_m.is_finite());
    assert!(report.lake_volume_km3 >= 0.0 && report.lake_volume_km3.is_finite());
    assert!((report.elapsed_yr - sim.state.elapsed_yr).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// 8. Export format invariants
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct PngHeader {
    width: u32,
    height: u32,
    bit_depth: u8,
    colour_type: u8,
}

fn be_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Parses and validates a PNG: signature, IHDR fields and chunk structure.
fn parse_png(bytes: &[u8], label: &str) -> PngHeader {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    assert!(
        bytes.len() > 33,
        "{label}: PNG too short ({} bytes)",
        bytes.len()
    );
    assert_eq!(&bytes[..8], &SIGNATURE, "{label}: bad PNG signature");
    assert_eq!(&bytes[12..16], b"IHDR", "{label}: first chunk is not IHDR");
    let header_len = be_u32(bytes, 8) as usize;
    assert_eq!(header_len, 13, "{label}: IHDR length {header_len}");
    let width = be_u32(bytes, 16);
    let height = be_u32(bytes, 20);
    let bit_depth = bytes[24];
    let colour_type = bytes[25];
    assert_eq!(bytes[26], 0, "{label}: compression method");
    assert_eq!(bytes[27], 0, "{label}: filter method");
    assert_eq!(bytes[28], 0, "{label}: interlace method");
    assert!(width > 0 && height > 0, "{label}: empty image");
    assert!(
        matches!(bit_depth, 8 | 16),
        "{label}: unexpected bit depth {bit_depth}"
    );

    // Walk the chunks to make sure the container is well-formed.
    let mut offset = 8usize;
    let mut saw_idat = false;
    let mut saw_iend = false;
    while offset + 12 <= bytes.len() {
        let length = be_u32(bytes, offset) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        assert!(
            offset + 12 + length <= bytes.len(),
            "{label}: truncated chunk at {offset}"
        );
        match kind {
            b"IDAT" => saw_idat = true,
            b"IEND" => {
                saw_iend = true;
                break;
            }
            _ => {}
        }
        offset += 12 + length;
    }
    assert!(saw_idat, "{label}: no IDAT chunk");
    assert!(saw_iend, "{label}: missing IEND chunk");
    PngHeader {
        width,
        height,
        bit_depth,
        colour_type,
    }
}

/// Mesh extraction geometry used by the OBJ and glTF exporters.
fn extraction_geometry(width: usize, height: usize, max_side: usize) -> (usize, usize) {
    let step = (width.max(height) / max_side.max(2) + 1).max(1);
    (width.div_ceil(step), height.div_ceil(step))
}

fn check_obj(text: &str, width: usize, height: usize, max_side: usize) {
    let (columns, rows) = extraction_geometry(width, height, max_side);
    let vertex_count = columns * rows;
    let mut vertices = 0usize;
    let mut faces = 0usize;
    for line in text.lines() {
        if line.starts_with("v ") {
            vertices += 1;
        } else if line.starts_with("f ") {
            faces += 1;
            for token in line.split_whitespace().skip(1) {
                let index: usize = token
                    .split('/')
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .expect("OBJ index is an integer");
                assert!(
                    index >= 1 && index <= vertex_count,
                    "OBJ face index {index} outside 1..={vertex_count}"
                );
            }
        }
    }
    assert_eq!(vertices, vertex_count, "OBJ vertex count");
    assert_eq!(faces, (columns - 1) * (rows - 1) * 2, "OBJ triangle count");
}

fn check_gltf(text: &str, width: usize, height: usize, max_side: usize) {
    let (columns, rows) = extraction_geometry(width, height, max_side);
    let vertex_count = columns * rows;
    let index_count = (columns - 1) * (rows - 1) * 6;
    assert!(
        text.starts_with('{') && text.ends_with('}'),
        "glTF JSON braces"
    );
    assert!(
        text.contains("\"asset\":{\"version\":\"2.0\""),
        "glTF asset version"
    );
    assert!(text.contains("\"POSITION\":0"), "glTF POSITION attribute");
    assert!(
        text.contains("data:application/octet-stream;base64,"),
        "glTF embedded buffer"
    );
    let declared: usize = text
        .split("\"buffers\":[{\"byteLength\":")
        .nth(1)
        .expect("glTF buffer byteLength")
        .split(',')
        .next()
        .expect("byteLength number")
        .parse()
        .expect("byteLength parses");
    let expected_bytes = vertex_count * 3 * 3 * 4 + index_count * 4;
    assert_eq!(
        declared, expected_bytes,
        "glTF buffer byteLength {declared} != positions+normals+colours+indices {expected_bytes}"
    );
    let encoded = text
        .split("base64,")
        .nth(1)
        .expect("glTF base64 payload")
        .split('"')
        .next()
        .expect("base64 payload end");
    assert_eq!(encoded.len() % 4, 0, "base64 payload is not padded to 4");
    let padding = encoded.chars().rev().take_while(|c| *c == '=').count();
    let decoded = encoded.len() / 4 * 3 - padding;
    assert_eq!(
        decoded, declared,
        "glTF embedded buffer decodes to {decoded} bytes but declares {declared}"
    );
    // Accessor counts must agree with the mesh geometry.
    let counts: Vec<usize> = text
        .split("\"count\":")
        .skip(1)
        .map(|chunk| {
            chunk
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .expect("accessor count")
                .parse()
                .expect("accessor count parses")
        })
        .collect();
    assert_eq!(counts.len(), 4, "glTF accessor count: {counts:?}");
    assert_eq!(counts[0], vertex_count, "glTF position accessor count");
    assert_eq!(counts[1], vertex_count, "glTF normal accessor count");
    assert_eq!(counts[2], vertex_count, "glTF colour accessor count");
    assert_eq!(counts[3], index_count, "glTF index accessor count");
    assert_eq!(
        text.matches("\"bufferView\":").count(),
        4,
        "glTF bufferViews"
    );
}

#[test]
fn export_format_invariants() {
    let params = run_params("earthlike", 48, 3, 1, 20_000.0);
    let output = pipeline::generate_initial(&params).expect("initial world");
    let world = &output.world;
    let climate = climate_of(world, &params);
    let hydro = hydrology::route(world, &params, Some(&climate));
    let grid = world.grid;
    let width = grid.width();
    let height = grid.height();

    // ---- Coloured raster maps -------------------------------------------
    for colour in [
        MapColour::Terrain,
        MapColour::Elevation,
        MapColour::Biomes,
        MapColour::Slope,
        MapColour::Rock,
        MapColour::Rivers,
    ] {
        let rgb = export::colour_map(
            world,
            Some(&climate),
            Some(&output.geology),
            Some(&hydro),
            colour,
        );
        assert_eq!(rgb.len(), width * height * 3, "{colour:?}: RGB buffer size");
        let png = export::encode_png_rgb(width as u32, height as u32, &rgb);
        let header = parse_png(&png, &format!("{colour:?} map"));
        assert_eq!(header.width as usize, width);
        assert_eq!(header.height as usize, height);
        assert_eq!(header.bit_depth, 8);
        assert_eq!(header.colour_type, 2);
    }
    let raw = export::elevation_raw_f32(world);
    assert_eq!(raw.len(), width * height * 4, "raw f32 raster size");

    // ---- 16-bit height map ----------------------------------------------
    let height16 = export::height_png16(world);
    let header = parse_png(&height16, "height16.png");
    assert_eq!(header.width as usize, width);
    assert_eq!(header.height as usize, height);
    assert_eq!(header.bit_depth, 16, "height PNG must be 16-bit");
    assert_eq!(header.colour_type, 0, "height PNG must be grayscale");

    // ---- Normal map ------------------------------------------------------
    let normal = export::normal_map_png(world);
    let header = parse_png(&normal, "normals.png");
    assert_eq!(header.width as usize, width);
    assert_eq!(header.height as usize, height);
    assert_eq!(header.bit_depth, 8);
    assert_eq!(header.colour_type, 2, "normal map must be RGB");

    // ---- OBJ -------------------------------------------------------------
    let max_side = 16;
    let obj = export::obj_string(world, &params, max_side);
    check_obj(&obj, width, height, max_side);

    // ---- glTF ------------------------------------------------------------
    let gltf = export::gltf_string(world, &params, max_side);
    check_gltf(&gltf, width, height, max_side);

    // ---- XYZ -------------------------------------------------------------
    let step = 2;
    let xyz = export::xyz_string(world, &params, step);
    let expected_points = width.div_ceil(step) * height.div_ceil(step);
    assert_eq!(xyz.lines().count(), expected_points, "XYZ point count");
    for line in xyz.lines().take(5) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 3);
        for field in fields {
            field.parse::<f64>().expect("XYZ component parses");
        }
    }

    // ---- write_all: the files the CLI and UI rely on --------------------
    let dir = std::env::temp_dir().join(format!(
        "geogenesis-validation-export-{}",
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean previous export dir");
    }
    let written = export::write_all(
        &dir,
        world,
        &params,
        Some(&climate),
        Some(&output.geology),
        Some(&hydro),
    )
    .expect("write_all");
    for name in [
        "terrain.png",
        "biomes.png",
        "rivers.png",
        "elevation.f32",
        "height16.png",
        "normals.png",
        "profile.png",
        "terrain.obj",
        "terrain.gltf",
        "terrain.xyz",
    ] {
        let path = dir.join(name);
        assert!(path.is_file(), "write_all did not write {name}");
        let size = std::fs::metadata(&path).expect("metadata").len();
        assert!(size > 0, "write_all wrote empty {name}");
    }
    assert!(written.contains(&"terrain.png".to_string()));
    let on_disk = std::fs::read(dir.join("height16.png")).expect("read height16");
    let header = parse_png(&on_disk, "height16.png on disk");
    assert_eq!(header.bit_depth, 16);
    let on_disk = std::fs::read(dir.join("terrain.png")).expect("read terrain");
    parse_png(&on_disk, "terrain.png on disk");
    std::fs::remove_dir_all(&dir).expect("clean export dir");
}

// ---------------------------------------------------------------------------
// 9. Performance smoke (release only; generous, warn-style bounds)
// ---------------------------------------------------------------------------

#[test]
fn performance_smoke() {
    if !heavy_enabled("performance_smoke") {
        return;
    }
    let params = run_params("earthlike", 256, 42, 5, 10_000.0);
    let started = Instant::now();
    let mut sim = Simulation::new(&params).expect("256^2 simulation");
    let generation_ms = started.elapsed().as_secs_f64() * 1e3;
    let steps = 5u32;
    let started = Instant::now();
    sim.step_many(steps);
    let step_ms = started.elapsed().as_secs_f64() * 1e3 / f64::from(steps);
    let cells = 256.0 * 256.0;
    println!(
        "  performance: 256^2 generation {generation_ms:.0} ms (target < 1000), step {step_ms:.1} ms ({:.2} us/cell/step)",
        step_ms * 1e3 / cells
    );
    // Generous smoke bounds: catch catastrophic slowdowns without turning the
    // harness into a flaky benchmark. Tight targets live in `geogenesis bench`.
    assert!(
        generation_ms < 10_000.0,
        "256^2 generation took {generation_ms:.0} ms (smoke bound 10 s)"
    );
    assert!(
        step_ms < 1_000.0,
        "256^2 step took {step_ms:.1} ms (smoke bound 1 s)"
    );
}

// ---------------------------------------------------------------------------
// 10. Cross-process conservation of one coupled step
// ---------------------------------------------------------------------------

#[test]
fn coupled_apply_all_closes_the_ledger() {
    let params = run_params("earthlike", 64, 9, 1, 20_000.0);
    let output = pipeline::generate_initial(&params).expect("initial world");
    let mut world = output.world;
    let climate = climate_of(&world, &params);
    let hydro = hydrology::route(&world, &params, Some(&climate));
    let mut geology = output.geology;
    geology.update_surface(&world, &params);
    let before = world.elevation_m.as_slice().to_vec();
    let balance = erosion::apply_all(&mut world, &params, 20_000.0, &hydro, &climate, &geology);
    assert!(
        balance.is_conservative(1e-3),
        "coupled process chain ledger does not close: {balance:?}"
    );
    assert!(balance.erosion_m3 > 0.0, "coupled chain eroded nothing");
    assert_eq!(world.elevation_m.non_finite_count(), 0);
    let changed = world
        .elevation_m
        .as_slice()
        .iter()
        .zip(&before)
        .filter(|(a, b)| (*a - *b).abs() > 1e-9)
        .count();
    assert!(changed > 0, "coupled chain changed no elevation");
    println!(
        "  coupled step: eroded {:.3e} m3, deposited {:.3e} m3, exported {:.3e} m3, closure {:.3e}",
        balance.erosion_m3,
        balance.deposition_m3,
        balance.export_m3,
        balance.closure_error_m3()
    );
}
