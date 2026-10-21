//! GeoGenesis command-line interface.
//!
//! Subcommands:
//!
//! * `generate` — build a world, optionally run the simulation, export maps,
//!   meshes, raw fields and a statistics report.
//! * `bench`    — measure generation and per-step cost against the spec's
//!   performance targets.
//! * `schema`   — print the parameter schema as JSON (shared with the UI).
//! * `presets`  — list presets.
//! * `sweep`    — run a parameter sweep over seeds and summarise the results.

use std::path::PathBuf;
use std::time::Instant;

use geogenesis_core::export;
use geogenesis_core::export::tiled as tiled_export;
use geogenesis_core::minerals;
use geogenesis_core::params::{format_duration, Params, PRESETS};
use geogenesis_core::pipeline;
use geogenesis_core::sim::Simulation;
use geogenesis_core::stats;
use geogenesis_core::tiling;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("generate") => cmd_generate(&args[1..]),
        Some("bench") => cmd_bench(&args[1..]),
        Some("schema") => {
            println!("{}", Params::schema_json());
            Ok(())
        }
        Some("presets") => {
            for preset in PRESETS {
                println!(
                    "{:<20} {:<26} {}",
                    preset.key, preset.label, preset.description
                );
            }
            Ok(())
        }
        Some("sweep") => cmd_sweep(&args[1..]),
        Some("explore") => cmd_explore(&args[1..]),
        Some("tiles") => cmd_tiles(&args[1..]),
        _ => {
            print_help();
            Ok(())
        }
    };
    if let Err(message) = code {
        eprintln!("error: {message}");
        std::process::exit(2);
    }
}

fn print_help() {
    println!("GeoGenesis {}", geogenesis_core::VERSION);
    println!();
    println!("usage: geogenesis <command> [options]");
    println!();
    println!("commands:");
    println!("  generate  Build a world and optionally run and export it");
    println!("  bench     Benchmark generation and stepping");
    println!("  sweep     Run a seed sweep and print terrain metrics");
    println!("  explore   Score many seeds and propose the most interesting worlds");
    println!("  tiles     Print the tiling plan and memory budget for a large grid");
    println!("  schema    Print the JSON parameter schema");
    println!("  presets   List built-in presets");
    println!();
    println!("common options:");
    println!("  --preset <key>     start from a preset (default: earthlike)");
    println!("  --seed <u64>       world seed");
    println!("  --grid <n>         square grid size in cells");
    println!("  --set key=value    override any schema parameter (repeatable)");
    println!("  --steps <n>        run n base steps after generation");
    println!("  --run              run the full configured duration");
    println!("  --out <dir>        export directory (default: out/<preset>-<seed>)");
}

#[derive(Default)]
struct Options {
    preset: Option<String>,
    seed: Option<u64>,
    grid: Option<u32>,
    steps: Option<u32>,
    run: bool,
    out: Option<PathBuf>,
    overrides: Vec<(String, f64)>,
    allow_large_grid: bool,
    plugins: Vec<String>,
    rocks_file: Option<PathBuf>,
    strata_file: Option<PathBuf>,
    tiled: bool,
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let value = |i: usize| -> Result<&String, String> {
            args.get(i + 1)
                .ok_or_else(|| format!("`{arg}` needs a value"))
        };
        match arg {
            "--preset" => {
                options.preset = Some(value(i)?.clone());
                i += 2;
            }
            "--seed" => {
                options.seed = Some(
                    value(i)?
                        .parse()
                        .map_err(|_| "seed must be an integer".to_string())?,
                );
                i += 2;
            }
            "--grid" => {
                options.grid = Some(
                    value(i)?
                        .parse()
                        .map_err(|_| "grid must be an integer".to_string())?,
                );
                i += 2;
            }
            "--steps" => {
                options.steps = Some(
                    value(i)?
                        .parse()
                        .map_err(|_| "steps must be an integer".to_string())?,
                );
                i += 2;
            }
            "--run" => {
                options.run = true;
                i += 1;
            }
            "--allow-large-grid" => {
                options.allow_large_grid = true;
                i += 1;
            }
            "--plugin" => {
                options.plugins.push(value(i)?.clone());
                i += 2;
            }
            "--tiled" => {
                options.tiled = true;
                i += 1;
            }
            "--rocks" => {
                options.rocks_file = Some(PathBuf::from(value(i)?));
                i += 2;
            }
            "--strata" => {
                options.strata_file = Some(PathBuf::from(value(i)?));
                i += 2;
            }
            "--out" => {
                options.out = Some(PathBuf::from(value(i)?));
                i += 2;
            }
            "--set" => {
                let pair = value(i)?;
                let (key, raw) = pair
                    .split_once('=')
                    .ok_or_else(|| format!("--set expects key=value, got `{pair}`"))?;
                let number: f64 = raw
                    .parse()
                    .map_err(|_| format!("`{raw}` is not a number"))?;
                options.overrides.push((key.to_string(), number));
                i += 2;
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(options)
}

fn build_params(options: &Options) -> Result<Params, String> {
    let mut params = Params::default();
    params
        .apply_preset(options.preset.as_deref().unwrap_or("earthlike"))
        .map_err(|e| e.to_string())?;
    if let Some(grid) = options.grid {
        params
            .set_key("width", f64::from(grid))
            .and_then(|()| params.set_key("height", f64::from(grid)))
            .map_err(|e| e.to_string())?;
    }
    if let Some(seed) = options.seed {
        params
            .set_key("seed", seed as f64)
            .map_err(|e| e.to_string())?;
    }
    for (key, value) in &options.overrides {
        params
            .set_key(key, *value)
            .map_err(|e| format!("--set {key}: {e}"))?;
    }
    params.allow_large_grid = options.allow_large_grid || options.tiled;
    params.normalize();
    params.validate().map_err(|e| e.to_string())?;
    Ok(params)
}

/// Builds a simulation honouring `--plugin` selections.
fn build_simulation(params: &Params, options: &Options) -> Result<Simulation, String> {
    if options.plugins.is_empty() {
        return Simulation::new(params).map_err(|e| e.to_string());
    }
    let mut plugins: Vec<Box<dyn geogenesis_core::plugin::ProcessPlugin>> = Vec::new();
    for name in &options.plugins {
        let plugin = geogenesis_core::plugin::builtin(name, params.world.effective_seed())
            .ok_or_else(|| format!("unknown plugin `{name}` (available: impacts)"))?;
        plugins.push(plugin);
    }
    Simulation::with_plugins(params, plugins).map_err(|e| e.to_string())
}

/// Peak resident set size of this process, bytes (Linux `/proc` only).
fn peak_rss_bytes() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

fn tiled_stats_json(
    params: &Params,
    width: u32,
    height: u32,
    preset: &str,
    result: &tiling::LargeWorldResult,
    out: Option<&std::path::Path>,
) -> String {
    let elevation = result.elevation_stats();
    let timing = result.timing;
    let mut files = String::from("[");
    if let Some(summary) = &result.exports {
        for (i, name) in summary.files.iter().enumerate() {
            if i > 0 {
                files.push(',');
            }
            files.push('"');
            for ch in name.chars() {
                if ch == '"' || ch == '\\' {
                    files.push('\\');
                }
                files.push(ch);
            }
            files.push('"');
        }
    }
    files.push(']');
    let out_json = match out {
        Some(path) => format!("\"{}\"", path.display().to_string().replace('\\', "\\\\")),
        None => "null".to_string(),
    };
    format!(
        concat!(
            "{{\"tiled\":true,\"width\":{width},\"height\":{height},\"cell_km\":{cell_km},",
            "\"sea_level_m\":{sea},\"land_fraction\":{land},",
            "\"elevation\":{{\"min_m\":{emin},\"max_m\":{emax},\"mean_m\":{emean},\"std_m\":{estd},",
            "\"median_m\":{emed},\"p10_m\":{ep10},\"p90_m\":{ep90},\"relief_m\":{erelief},\"cells\":{cells}}},",
            "\"timing\":{{\"anchor_s\":{anchor},\"tiles_s\":{tiles},\"export_s\":{export},\"total_s\":{total}}},",
            "\"peak_rss_mb\":{rss},\"exports\":{files},\"out\":{out_json},\"preset\":\"{preset}\",\"seed\":{seed}}}"
        ),
        width = width,
        height = height,
        cell_km = params.world.cell_km,
        sea = result.sea_level_m,
        land = result.land_fraction(),
        emin = elevation.min_m,
        emax = elevation.max_m,
        emean = elevation.mean_m,
        estd = elevation.std_m,
        emed = elevation.median_m,
        ep10 = elevation.p10_m,
        ep90 = elevation.p90_m,
        erelief = elevation.relief_m(),
        cells = elevation.cells,
        anchor = timing.anchor_s,
        tiles = timing.tiles_s,
        export = timing.export_s,
        total = timing.total_s,
        rss = peak_rss_bytes().map_or(0.0, |b| b as f64 / (1024.0 * 1024.0)),
        files = files,
        out_json = out_json,
        preset = preset,
        seed = params.world.effective_seed(),
    )
}

fn run_tiled_generate(options: &Options, params: &Params) -> Result<(), String> {
    if options.run || options.steps.unwrap_or(0) > 0 {
        return Err(
            "--tiled generates terrain only; time evolution needs a grid of 2048² or less"
                .to_string(),
        );
    }
    if options.rocks_file.is_some() || options.strata_file.is_some() {
        return Err("--tiled does not support --rocks/--strata yet".to_string());
    }
    let preset = options.preset.as_deref().unwrap_or("earthlike");
    let out = options.out.clone().unwrap_or_else(|| {
        PathBuf::from(format!(
            "out/{preset}-{}-tiled",
            params.world.effective_seed()
        ))
    });
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let width = params.world.width;
    let height = params.world.height;
    println!(
        "tiled world: {width}x{height} @ {} km ({:.0}x{:.0} km), seed {}",
        params.world.cell_km,
        params.world.extent_x_km(),
        params.world.extent_y_km(),
        params.world.effective_seed()
    );
    let world_options = tiling::LargeWorldOptions {
        tile_size: 512,
        halo: 32,
        preview_factor: 8,
        workers: 0,
        export: Some(tiled_export::ExportRequest::with_kinds(
            &out,
            tiled_export::ExportKinds::ALL,
        )),
    };
    let start = Instant::now();
    let result = tiling::generate_large_world_with(params, width, height, &world_options)
        .map_err(|e| e.to_string())?;
    // Coarse UI preview next to the rasters.
    export::write_png(
        &out.join("preview.png"),
        result.preview_width,
        result.preview_height,
        &result.preview_rgb(),
    )
    .map_err(|e| e.to_string())?;
    let stats_json = tiled_stats_json(params, width, height, preset, &result, Some(&out));
    export::write_bytes(&out.join("stats.json"), stats_json.as_bytes())
        .map_err(|e| e.to_string())?;
    let params_json = format!(
        "{{\"values\":[{}],\"preset\":\"{preset}\",\"seed\":{}}}",
        params
            .values()
            .iter()
            .map(|v| format!("{v}"))
            .collect::<Vec<_>>()
            .join(","),
        params.world.effective_seed()
    );
    export::write_bytes(&out.join("params.json"), params_json.as_bytes())
        .map_err(|e| e.to_string())?;
    let elevation = result.elevation_stats();
    println!(
        "generated in {:.2} s (anchor {:.2} s, tiles {:.2} s, export {:.2} s, peak RSS {:.0} MB)",
        result.timing.total_s.max(start.elapsed().as_secs_f64()),
        result.timing.anchor_s,
        result.timing.tiles_s,
        result.timing.export_s,
        peak_rss_bytes().map_or(0.0, |b| b as f64 / (1024.0 * 1024.0))
    );
    println!(
        "elevation: {:.0}..{:.0} m (mean {:.0}, relief {:.0}), land {:.1}%",
        elevation.min_m,
        elevation.max_m,
        elevation.mean_m,
        elevation.relief_m(),
        result.land_fraction() * 100.0
    );
    println!("exported: {}", out.display());
    if let Some(summary) = &result.exports {
        for name in &summary.files {
            println!("  {name}");
        }
    }
    println!("  stats.json");
    println!("  params.json");
    Ok(())
}

fn cmd_generate(args: &[String]) -> Result<(), String> {
    let options = parse_options(args)?;
    let params = build_params(&options)?;
    if options.tiled {
        return run_tiled_generate(&options, &params);
    }
    let height = if options.grid.is_some() {
        params.world.height
    } else {
        params.world.width
    };
    println!(
        "world: {}x{} @ {} km ({}x{} km), seed {}",
        params.world.width,
        height,
        params.world.cell_km,
        params.world.extent_x_km().round(),
        params.world.extent_y_km().round(),
        params.world.seed
    );
    println!(
        "time: {} in {} steps of {}",
        format_duration(params.time.total_yr),
        params.planned_steps(),
        format_duration(params.time.step_yr)
    );

    let start = Instant::now();
    let mut sim = build_simulation(&params, &options)?;
    if options.rocks_file.is_some() || options.strata_file.is_some() {
        let rocks = match options.rocks_file.as_deref() {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                geogenesis_core::rocks::RockSet::from_json(&text)
                    .map_err(|e| format!("invalid rock set {}: {e}", path.display()))?
            }
            None => geogenesis_core::rocks::RockSet::builtin(),
        };
        let strata = match options.strata_file.as_deref() {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                Some(
                    geogenesis_core::rocks::StrataSpec::from_json(&text)
                        .map_err(|e| format!("invalid strata {}: {e}", path.display()))?,
                )
            }
            None => None,
        };
        sim.set_geology_custom(&rocks, strata.as_ref())
            .map_err(|e| e.to_string())?;
        println!(
            "geology: {} rock units ({} custom){}",
            rocks.len(),
            rocks.custom_count(),
            if strata.is_some() {
                ", custom strata stack"
            } else {
                ""
            }
        );
    }
    let generation = start.elapsed();
    println!(
        "generated in {:.1} ms (land {:.1}%)",
        generation.as_secs_f64() * 1000.0,
        sim.world.land_fraction() * 100.0
    );

    let mut stepping = std::time::Duration::ZERO;
    if options.run {
        let t = Instant::now();
        sim.run();
        stepping = t.elapsed();
    } else if let Some(steps) = options.steps {
        let t = Instant::now();
        sim.step_many(steps);
        stepping = t.elapsed();
    }
    if stepping > std::time::Duration::ZERO {
        println!(
            "advanced to {} ({} iterations) in {:.1} ms ({:.2} ms/step)",
            format_duration(sim.state.elapsed_yr),
            sim.state.iteration,
            stepping.as_secs_f64() * 1000.0,
            stepping.as_secs_f64() * 1000.0 / sim.state.iteration.max(1) as f64
        );
    }

    let report = sim.stats();
    println!(
        "elevation: {:.0}..{:.0} m (mean {:.0}, relief {:.0}), slope mean {:.1}°, land {:.1}%",
        report.elevation.min_m,
        report.elevation.max_m,
        report.elevation.mean_m,
        report.elevation.relief_m(),
        report.slope.mean_deg,
        report.hypsometry.land_fraction * 100.0
    );
    println!(
        "climate: {:.1} °C, {:.0} mm/yr; rivers: {} channel cells, drainage {:.3} km/km²",
        report.mean_temp_c,
        report.mean_precip_mm,
        report.channel_cells,
        report.drainage_density_km_per_km2
    );
    println!(
        "ledger: eroded {:.3e} m³, deposited {:.3e} m³, exported {:.3e} m³, closure error {:.3e}",
        sim.state.ledger.erosion_m3,
        sim.state.ledger.deposition_m3,
        sim.state.ledger.export_m3,
        sim.state.ledger.closure_error_m3()
    );

    let out = options.out.unwrap_or_else(|| {
        PathBuf::from(format!(
            "out/{}-{}",
            options.preset.as_deref().unwrap_or("earthlike"),
            params.world.seed
        ))
    });
    let mut written = export::write_all(
        &out,
        &sim.world,
        &params,
        sim.climate(),
        Some(sim.geology()),
        Some(sim.hydro()),
    )
    .map_err(|e| e.to_string())?;
    // Mineral prospectivity map + inventory (lazily computed; includes the
    // per-mineral tonnage/value summary for this world).
    if let Some((mineral_map, mineral_inventory)) = sim.minerals_snapshot() {
        export::write_bytes(
            &out.join("minerals.png"),
            &minerals::map_png(&mineral_map, &sim.world),
        )
        .map_err(|e| e.to_string())?;
        let mineral_json = minerals::mineral_json(&mineral_map, &mineral_inventory);
        export::write_bytes(&out.join("minerals.json"), mineral_json.as_bytes())
            .map_err(|e| e.to_string())?;
        written.push("minerals.png".to_string());
        written.push("minerals.json".to_string());
    }
    let stats_json = stats::to_json(&report);
    export::write_bytes(&out.join("stats.json"), stats_json.as_bytes())
        .map_err(|e| e.to_string())?;
    let params_json = format!(
        "{{\"values\":[{}],\"preset\":\"{}\",\"seed\":{}}}",
        params
            .values()
            .iter()
            .map(|v| format!("{v}"))
            .collect::<Vec<_>>()
            .join(","),
        options.preset.as_deref().unwrap_or("earthlike"),
        params.world.seed
    );
    export::write_bytes(&out.join("params.json"), params_json.as_bytes())
        .map_err(|e| e.to_string())?;
    println!("exported: {}", out.display());
    for file in written {
        println!("  {file}");
    }
    println!("  stats.json");
    println!("  params.json");
    Ok(())
}

fn cmd_bench(args: &[String]) -> Result<(), String> {
    let options = parse_options(args)?;
    let grids: Vec<u32> = match options.grid {
        Some(g) => vec![g],
        None => vec![256, 512],
    };
    println!(
        "{:>6} {:>12} {:>12} {:>12} {:>10}",
        "grid", "generate ms", "step ms", "steps/s", "land %"
    );
    for grid in grids {
        let mut params = build_params(&Options {
            grid: Some(grid),
            ..Default::default()
        })?;
        if options.steps.is_none() {
            // Keep benchmark time bounded.
            params.time.max_iters = params.time.max_iters.max(10);
        }
        let start = Instant::now();
        let mut sim = build_simulation(&params, &options)?;
        if options.rocks_file.is_some() || options.strata_file.is_some() {
            let rocks = match options.rocks_file.as_deref() {
                Some(path) => {
                    let text = std::fs::read_to_string(path)
                        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                    geogenesis_core::rocks::RockSet::from_json(&text)
                        .map_err(|e| format!("invalid rock set {}: {e}", path.display()))?
                }
                None => geogenesis_core::rocks::RockSet::builtin(),
            };
            let strata = match options.strata_file.as_deref() {
                Some(path) => {
                    let text = std::fs::read_to_string(path)
                        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                    Some(
                        geogenesis_core::rocks::StrataSpec::from_json(&text)
                            .map_err(|e| format!("invalid strata {}: {e}", path.display()))?,
                    )
                }
                None => None,
            };
            sim.set_geology_custom(&rocks, strata.as_ref())
                .map_err(|e| e.to_string())?;
            println!(
                "geology: {} rock units ({} custom){}",
                rocks.len(),
                rocks.custom_count(),
                if strata.is_some() {
                    ", custom strata stack"
                } else {
                    ""
                }
            );
        }
        let generate_ms = start.elapsed().as_secs_f64() * 1000.0;
        let steps = options.steps.unwrap_or(20).min(sim.params.time.max_iters);
        let t = Instant::now();
        sim.step_many(steps);
        let step_ms = t.elapsed().as_secs_f64() * 1000.0 / f64::from(steps.max(1));
        println!(
            "{:>6} {:>12.1} {:>12.1} {:>12.1} {:>10.1}",
            format!("{grid}²"),
            generate_ms,
            step_ms,
            1000.0 / step_ms.max(1e-9),
            sim.world.land_fraction() * 100.0
        );
    }
    // Parameter-preview latency (the UI rebuilds a 96-cell world while sliders
    // move): report the generation time against the < 16 ms target.
    let params = Params::default();
    let best = (0..3)
        .map(|_| {
            let start = Instant::now();
            let _ = pipeline::generate_preview(&params, 96);
            start.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::INFINITY, f64::min);
    println!();
    println!(
        "preview: 96² generation {best:.1} ms (spec target < 16 ms; UI adds ~45 ms worker/transfer)"
    );
    println!();
    println!("spec targets: 512² generate < 500 ms; 1024² < 3 s; 4096² < 30 s; UI update < 16 ms");
    Ok(())
}

/// Scores a candidate world for the `explore` proposer.
///
/// The score encodes what makes a generated world interesting: a balanced
/// land/ocean split, a relief envelope that neither looks flat nor runaway,
/// an active river network, a temperate-to-warm climate and a diverse biome
/// mix. It is deliberately simple and documented so artists can see why a
/// seed was proposed.
fn world_score(stats: &stats::TerrainStats, vertical_m: f64) -> f64 {
    let land = stats.hypsometry.land_fraction;
    let mut score = 1.0;
    // Land/ocean balance peaks around 38% land.
    score -= (land - 0.38).abs() * 2.2;
    // Relief should use a good part of the vertical envelope but not saturate.
    let relief_ratio = stats.max_relief_m / vertical_m.max(1.0);
    if relief_ratio < 0.35 {
        score -= (0.35 - relief_ratio) * 1.5;
    } else if relief_ratio > 1.15 {
        score -= (relief_ratio - 1.15).min(1.0) * 1.2;
    }
    // River network presence.
    score += (stats.drainage_density_km_per_km2 * 80.0).min(0.6);
    // Climate plausibility.
    let t = stats.mean_temp_c;
    if t < -5.0 || t > 30.0 {
        score -= 0.6;
    }
    // Biome diversity (fraction of land biomes above 1%).
    let diverse = stats.biomes.iter().filter(|b| b.fraction > 0.01).count();
    score += (diverse as f64 * 0.04).min(0.4);
    // Penalise degenerate worlds.
    if land < 0.05 || land > 0.82 {
        score -= 2.0;
    }
    score
}

fn cmd_explore(args: &[String]) -> Result<(), String> {
    let options = parse_options(args)?;
    let count = options.steps.unwrap_or(24).clamp(1, 500);
    let top = 6usize;
    let grid = options.grid.unwrap_or(96);
    println!(
        "exploring {} seeds at {grid}² (preset {})",
        count,
        options.preset.as_deref().unwrap_or("earthlike")
    );
    let mut scored: Vec<(f64, u64, stats::TerrainStats)> = Vec::new();
    for offset in 0..count {
        let seed = options.seed.unwrap_or(1) + u64::from(offset);
        let params = build_params(&Options {
            seed: Some(seed),
            grid: Some(grid),
            preset: options.preset.clone(),
            ..Default::default()
        })?;
        let mut sim = build_simulation(&params, &options)?;
        if options.rocks_file.is_some() || options.strata_file.is_some() {
            let rocks = match options.rocks_file.as_deref() {
                Some(path) => {
                    let text = std::fs::read_to_string(path)
                        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                    geogenesis_core::rocks::RockSet::from_json(&text)
                        .map_err(|e| format!("invalid rock set {}: {e}", path.display()))?
                }
                None => geogenesis_core::rocks::RockSet::builtin(),
            };
            let strata = match options.strata_file.as_deref() {
                Some(path) => {
                    let text = std::fs::read_to_string(path)
                        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                    Some(
                        geogenesis_core::rocks::StrataSpec::from_json(&text)
                            .map_err(|e| format!("invalid strata {}: {e}", path.display()))?,
                    )
                }
                None => None,
            };
            sim.set_geology_custom(&rocks, strata.as_ref())
                .map_err(|e| e.to_string())?;
            println!(
                "geology: {} rock units ({} custom){}",
                rocks.len(),
                rocks.custom_count(),
                if strata.is_some() {
                    ", custom strata stack"
                } else {
                    ""
                }
            );
        }
        // A handful of steps exposes river networks and erosion feedbacks.
        sim.step_many(3);
        let report = sim.stats();
        let score = world_score(&report, params.world.vertical_m);
        scored.push((score, seed, report));
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!(
        "{:>3} {:>8} {:>7} {:>8} {:>8} {:>7} {:>10}",
        "#", "seed", "score", "land %", "relief m", "temp °C", "rivers"
    );
    for (rank, (score, seed, report)) in scored.iter().take(top).enumerate() {
        println!(
            "{:>3} {:>8} {:>7.3} {:>7.1} {:>8.0} {:>7.1} {:>10}",
            rank + 1,
            seed,
            score,
            report.hypsometry.land_fraction * 100.0,
            report.max_relief_m,
            report.mean_temp_c,
            report.channel_cells
        );
    }
    if let Some((_, best_seed, _)) = scored.first() {
        println!();
        println!(
            "best seed: {best_seed} — reproduce with `generate --preset {} --seed {best_seed}`",
            options.preset.as_deref().unwrap_or("earthlike")
        );
    }
    Ok(())
}

fn cmd_tiles(args: &[String]) -> Result<(), String> {
    let mut grid = 4096u32;
    let mut tile_size = 512u32;
    let mut halo = 32u32;
    let mut preview_factor = 8u32;
    let mut workers = 0u32;
    let mut seed = 1337u64;
    let mut cell_km: Option<f64> = None;
    let mut preset: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut run = false;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--run" => {
                run = true;
                i += 1;
                continue;
            }
            "--json" => {
                json = true;
                i += 1;
                continue;
            }
            _ => {}
        }
        let raw = args
            .get(i + 1)
            .ok_or_else(|| format!("`{}` needs a value", args[i]))?;
        match args[i].as_str() {
            "--grid" => {
                grid = raw
                    .parse()
                    .map_err(|_| "grid must be an integer".to_string())?;
            }
            "--tile-size" => {
                tile_size = raw
                    .parse()
                    .map_err(|_| "tile-size must be an integer".to_string())?;
            }
            "--halo" => {
                halo = raw
                    .parse()
                    .map_err(|_| "halo must be an integer".to_string())?;
            }
            "--preview-factor" => {
                preview_factor = raw
                    .parse()
                    .map_err(|_| "preview-factor must be an integer".to_string())?;
            }
            "--workers" => {
                workers = raw
                    .parse()
                    .map_err(|_| "workers must be an integer".to_string())?;
            }
            "--seed" => {
                seed = raw
                    .parse()
                    .map_err(|_| "seed must be an integer".to_string())?;
            }
            "--cell-km" => {
                cell_km = Some(
                    raw.parse()
                        .map_err(|_| "cell-km must be a number".to_string())?,
                );
            }
            "--preset" => preset = Some(raw.clone()),
            "--out" => out = Some(PathBuf::from(raw)),
            other => return Err(format!("unknown tiles option `{other}`")),
        }
        i += 2;
    }
    let mut params = Params::default();
    params.world.seed = seed;
    if let Some(key) = preset.as_deref() {
        params.apply_preset(key).map_err(|e| e.to_string())?;
    }
    if let Some(km) = cell_km {
        params.world.cell_km = km;
    }
    params.normalize();

    let plan = pipeline::tile_plan(grid, grid, tile_size, halo);
    let core_cells = u64::from(grid) * u64::from(grid);
    let buffer_cells: u64 = plan.iter().map(|t| t.buffer_cells() as u64).sum();
    let halo_overhead = (buffer_cells as f64 / core_cells as f64 - 1.0) * 100.0;
    let max_tile_cells = plan.iter().map(|t| t.buffer_cells()).max().unwrap_or(0);
    let auto_workers = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4)
        .min(plan.len().max(1));
    let effective_workers = if workers == 0 {
        auto_workers
    } else {
        (workers as usize).min(plan.len().max(1))
    };
    println!(
        "tile plan: {grid}x{grid} in {} tiles of up to {tile_size} cells (halo {halo})",
        plan.len()
    );
    println!("core cells:        {core_cells}");
    println!("buffer cells:      {buffer_cells} (halo overhead {halo_overhead:.1}%)");
    println!(
        "largest tile:      {max_tile_cells} cells (~{} KB f64, ~{} KB f32)",
        max_tile_cells * 8 / 1024,
        max_tile_cells * 4 / 1024
    );
    println!(
        "concurrent budget: {effective_workers} workers x {} KB (f64 field) = {} MB",
        max_tile_cells * 8 / 1024,
        (max_tile_cells * 8 * effective_workers) / (1024 * 1024)
    );
    if !run {
        if json {
            println!(
                "{{\"tiled\":false,\"grid\":{grid},\"tiles\":{},\"tileSize\":{tile_size},\"halo\":{halo},\"workers\":{effective_workers},\"seed\":{seed},\"haloOverheadPct\":{halo_overhead:.3}}}",
                plan.len()
            );
        }
        println!();
        println!("run with --run to generate the world (add --out <dir> to write exports)");
        return Ok(());
    }

    let world_options = tiling::LargeWorldOptions {
        tile_size,
        halo,
        preview_factor,
        workers,
        export: out.as_ref().map(|dir| {
            tiled_export::ExportRequest::with_kinds(dir, tiled_export::ExportKinds::ALL)
        }),
    };
    let result = tiling::generate_large_world_with(&params, grid, grid, &world_options)
        .map_err(|e| e.to_string())?;
    if let Some(dir) = out.as_ref() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        export::write_png(
            &dir.join("preview.png"),
            result.preview_width,
            result.preview_height,
            &result.preview_rgb(),
        )
        .map_err(|e| e.to_string())?;
        let stats_json = tiled_stats_json(
            &params,
            grid,
            grid,
            preset.as_deref().unwrap_or("defaults"),
            &result,
            Some(dir),
        );
        export::write_bytes(&dir.join("stats.json"), stats_json.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let elevation = result.elevation_stats();
    let rss = peak_rss_bytes().map_or(0.0, |b| b as f64 / (1024.0 * 1024.0));
    if json {
        println!(
            "{{\"tiled\":true,\"grid\":{grid},\"tiles\":{},\"tileSize\":{tile_size},\"halo\":{halo},\"workers\":{effective_workers},\"seed\":{seed},\"seconds\":{:.3},\"anchorSeconds\":{:.3},\"tileSeconds\":{:.3},\"exportSeconds\":{:.3},\"landFraction\":{:.6},\"reliefM\":{:.1},\"meanM\":{:.1},\"peakRssMb\":{:.1},\"preview\":{},\"out\":{}}}",
            plan.len(),
            result.timing.total_s,
            result.timing.anchor_s,
            result.timing.tiles_s,
            result.timing.export_s,
            result.land_fraction(),
            elevation.relief_m(),
            elevation.mean_m,
            rss,
            grid / preview_factor.max(1),
            out.as_ref()
                .map_or_else(|| "null".to_string(), |p| format!("\"{}\"", p.display()))
        );
    } else {
        println!();
        println!(
            "tiled generation: {:.2} s (anchor {:.2} s, tiles {:.2} s, export {:.2} s), peak RSS {:.0} MB",
            result.timing.total_s,
            result.timing.anchor_s,
            result.timing.tiles_s,
            result.timing.export_s,
            rss
        );
        println!(
            "elevation: {:.0}..{:.0} m (mean {:.0}, relief {:.0}), land {:.1}%",
            elevation.min_m,
            elevation.max_m,
            elevation.mean_m,
            elevation.relief_m(),
            result.land_fraction() * 100.0
        );
        if let Some(dir) = out.as_ref() {
            println!("exported: {}", dir.display());
            if let Some(summary) = &result.exports {
                for name in &summary.files {
                    println!("  {name}");
                }
            }
            println!("  stats.json");
        }
    }
    Ok(())
}
fn cmd_sweep(args: &[String]) -> Result<(), String> {
    let options = parse_options(args)?;
    let start_seed = options.seed.unwrap_or(1);
    let count = options.steps.unwrap_or(10).max(1);
    println!("seed,land_fraction,relief_m,slope_deg,drainage_density,mean_temp_c");
    for seed in start_seed..start_seed + u64::from(count) {
        let params = build_params(&Options {
            seed: Some(seed),
            ..Default::default()
        })?;
        let mut sim = build_simulation(&params, &options)?;
        if options.rocks_file.is_some() || options.strata_file.is_some() {
            let rocks = match options.rocks_file.as_deref() {
                Some(path) => {
                    let text = std::fs::read_to_string(path)
                        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                    geogenesis_core::rocks::RockSet::from_json(&text)
                        .map_err(|e| format!("invalid rock set {}: {e}", path.display()))?
                }
                None => geogenesis_core::rocks::RockSet::builtin(),
            };
            let strata = match options.strata_file.as_deref() {
                Some(path) => {
                    let text = std::fs::read_to_string(path)
                        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                    Some(
                        geogenesis_core::rocks::StrataSpec::from_json(&text)
                            .map_err(|e| format!("invalid strata {}: {e}", path.display()))?,
                    )
                }
                None => None,
            };
            sim.set_geology_custom(&rocks, strata.as_ref())
                .map_err(|e| e.to_string())?;
            println!(
                "geology: {} rock units ({} custom){}",
                rocks.len(),
                rocks.custom_count(),
                if strata.is_some() {
                    ", custom strata stack"
                } else {
                    ""
                }
            );
        }
        sim.step_many(5);
        let report = sim.stats();
        println!(
            "{},{:.4},{:.1},{:.2},{:.4},{:.2}",
            seed,
            report.hypsometry.land_fraction,
            report.elevation.relief_m(),
            report.slope.mean_deg,
            report.drainage_density_km_per_km2,
            report.mean_temp_c
        );
    }
    Ok(())
}
