//! Runnable benchmark and demo for production tiled large-grid generation.
//!
//! Generates a 4096² world with [`geogenesis_core::tiling`] (coarse global
//! anchor + per-tile real relief), streams the terrain/heightmap exports and
//! reports wall time and peak resident set size (VmHWM from `/proc/self/status`
//! on Linux) so the SPEC §13 budget (< 30 s, < 1 GB at 4096²) can be checked
//! from a single command.
//!
//! ```text
//! cargo run --release -p geogenesis-core --example large_grid -- --grid 4096 --out out/large
//! cargo run --release -p geogenesis-core --example large_grid -- \
//!     --grid 1024 --out out/large1024 --workers 8
//! ```
//!
//! Outputs in `--out`:
//!
//! * `terrain.png` — streamed hypsometric tint + hillshade
//! * `height16.png` — 16-bit heightmap normalised to the world range
//! * `elevation.f32` — raw little-endian elevation raster
//! * `preview.png` — box-filtered coarse preview (world / `--preview-factor`)
//! * `normals.png` — only with `--normals`
//!
//! The physical extent comes from the parameter set, so the default invocation
//! (384×384 cells @ 10 km → 3840 km) generates that same 3840 km world at
//! 4096² (0.94 km cells). Pass `--cell-km` to keep the parameter grid's cell
//! size instead: `--cell-km 10` produces a 40,960 km world, matching the
//! single-shot `generate --grid 4096` baseline.
//!
//! The process exits with status 1 when the SPEC budget is exceeded, so the
//! example doubles as an acceptance check.

use geogenesis_core::export;
use geogenesis_core::export::tiled::{ExportKinds, ExportRequest};
use geogenesis_core::params::Params;
use geogenesis_core::tiling::{self, LargeWorldOptions};
use std::path::PathBuf;
use std::time::Instant;

/// Parsed command line.
struct Args {
    grid: u32,
    out: Option<PathBuf>,
    seed: u64,
    cell_km: Option<f64>,
    preset: Option<String>,
    tile_size: u32,
    halo: u32,
    preview_factor: u32,
    workers: u32,
    normals: bool,
    no_export: bool,
    compare_single_shot: bool,
    quiet: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            grid: 4096,
            out: Some(PathBuf::from("out/large")),
            seed: 1337,
            cell_km: None,
            preset: None,
            tile_size: 512,
            halo: 32,
            preview_factor: 8,
            workers: 0,
            normals: false,
            no_export: false,
            compare_single_shot: false,
            quiet: false,
        }
    }
}

const HELP: &str = "\
Tiled large-grid generation benchmark (GeoGenesis)

USAGE:
    cargo run --release -p geogenesis-core --example large_grid -- [OPTIONS]

OPTIONS:
    --grid <n>            Square fine grid size (default 4096)
    --out <dir>           Output directory (default out/large; omit to skip export)
    --seed <n>            World seed (default 1337)
    --cell-km <f>         Keep this cell size instead of preserving the parameter extent
    --preset <key>        Apply a named parameter preset (e.g. earthlike)
    --tile-size <n>       Core tile edge in cells (default 512)
    --halo <n>            Tile halo in cells (default 32)
    --preview-factor <n>  Fine cells per preview cell (default 8)
    --workers <n>         Worker threads (default: auto, capped by memory budget)
    --normals             Also stream normals.png
    --no-export           Generate only (statistics + preview), write no files
    --compare-single-shot Also run generate_initial at this size (<= 2048²) and
                          print the statistics difference for quality evidence
    --quiet               Only print the final summary line
    -h, --help            Show this help
";

fn main() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;

    let mut params = Params::default();
    if let Some(preset) = &args.preset {
        params
            .apply_preset(preset)
            .map_err(|error| format!("preset `{preset}`: {error}"))?;
    }
    params.world.seed = args.seed;
    // The example requests `grid` cells; the physical extent follows the
    // parameters. `--cell-km` switches to single-shot CLI semantics (grid cells
    // of exactly that size → extent = grid × cell_km); without it the parameter
    // grid's extent is preserved and the tiled pass shrinks the fine cells.
    if let Some(cell_km) = args.cell_km {
        params.world.width = args.grid;
        params.world.height = args.grid;
        params.world.cell_km = cell_km;
    }
    params.normalize();

    let extent_x_km = params.world.extent_x_km();
    let extent_y_km = params.world.extent_y_km();
    let cell_km = extent_x_km / f64::from(args.grid);
    if !args.quiet {
        println!(
            "world:    {}x{} @ {:.4} km/cell ({:.0}x{:.0} km), seed {}",
            args.grid, args.grid, cell_km, extent_x_km, extent_y_km, args.seed
        );
        println!(
            "tiling:   {}px tiles, {}px halo, preview 1/{}, workers {}",
            args.tile_size,
            args.halo,
            args.preview_factor,
            if args.workers == 0 {
                "auto".to_string()
            } else {
                args.workers.to_string()
            }
        );
    }

    let kinds = if args.normals {
        ExportKinds::ALL
    } else {
        ExportKinds::default()
    };
    let export = match (&args.out, args.no_export) {
        (Some(dir), false) => {
            if !args.quiet {
                println!("output:   {}", dir.display());
            }
            Some(ExportRequest::with_kinds(dir, kinds))
        }
        _ => None,
    };
    let options = LargeWorldOptions {
        tile_size: args.tile_size,
        halo: args.halo,
        preview_factor: args.preview_factor,
        workers: args.workers,
        export,
    };

    let start = Instant::now();
    let result = tiling::generate_large_world_with(&params, args.grid, args.grid, &options)
        .map_err(|error| format!("tiled generation failed: {error}"))?;
    let wall = start.elapsed();
    let peak_rss = peak_rss_bytes();

    // The coarse preview is small; save it separately when no export ran.
    if let Some(dir) = &args.out {
        if !args.no_export {
            // Export already wrote preview.png; nothing else to do.
        } else {
            let rgb = result.preview_rgb();
            let png = export::encode_png_rgb(result.preview_width, result.preview_height, &rgb);
            export::write_bytes(&dir.join("preview.png"), &png)
                .map_err(|error| format!("cannot write preview: {error}"))?;
        }
    }

    print_report(&args, &result, wall, peak_rss)?;
    if args.compare_single_shot {
        compare_with_single_shot(&params, &args, &result)?;
    }

    // Acceptance gate (SPEC §13). Keep the timeout generous for slow CI VMs,
    // but the reported numbers are the measured ones.
    let mut failures = Vec::new();
    if args.grid >= 4096 {
        if wall.as_secs_f64() > 30.0 {
            failures.push(format!("wall time {:.2} s > 30 s", wall.as_secs_f64()));
        }
        if let Some(rss) = peak_rss {
            if rss > 1_000_000_000 {
                failures.push(format!("peak RSS {:.1} MB > 1 GB", rss as f64 / 1e6));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("SPEC §13 budget missed: {}", failures.join("; ")))
    }
}

fn print_report(
    args: &Args,
    result: &tiling::LargeWorldResult,
    wall: std::time::Duration,
    peak_rss: Option<u64>,
) -> Result<(), String> {
    let stats = result.elevation_stats();
    let timing = result.timing;
    let exports = result.exports.as_ref();
    println!();
    println!(
        "generation: {:.2} s wall (anchor {:.2} s, tiles {:.2} s, export {:.2} s)",
        wall.as_secs_f64(),
        timing.anchor_s,
        timing.tiles_s,
        timing.export_s
    );
    match peak_rss {
        Some(bytes) => println!("peak RSS:   {:.1} MB (VmHWM)", bytes as f64 / 1e6),
        None => println!("peak RSS:   unavailable (not Linux /proc)"),
    }
    println!(
        "stats:      land {:.1}%, relief {:.0} m, mean {:.0} m, min {:.0} m, max {:.0} m, \
         median {:.0} m",
        result.land_fraction() * 100.0,
        stats.relief_m(),
        stats.mean_m,
        stats.min_m,
        stats.max_m,
        stats.median_m
    );
    println!(
        "preview:    {}x{} (factor {})",
        result.preview_width, result.preview_height, args.preview_factor
    );
    if let Some(summary) = exports {
        println!(
            "export:     {} files, {:.1} MB",
            summary.files.len(),
            summary.bytes as f64 / 1e6
        );
        for file in &summary.files {
            println!("            - {file}");
        }
    }
    // Machine-readable final line (always printed so scripts can parse it).
    println!(
        "{{\"grid\":{},\"seconds\":{:.4},\"peakRssBytes\":{},\"anchorSeconds\":{:.4},\
         \"tileSeconds\":{:.4},\"exportSeconds\":{:.4},\"landFraction\":{:.6},\
         \"reliefM\":{:.1},\"meanM\":{:.1},\"minM\":{:.1},\"maxM\":{:.1},\
         \"medianM\":{:.1},\"workers\":{}}}",
        args.grid,
        wall.as_secs_f64(),
        peak_rss.unwrap_or(0),
        timing.anchor_s,
        timing.tiles_s,
        timing.export_s,
        result.land_fraction(),
        stats.relief_m(),
        stats.mean_m,
        stats.min_m,
        stats.max_m,
        stats.median_m,
        args.workers
    );
    if !args.quiet && args.grid >= 4096 {
        let time_ok = wall.as_secs_f64() < 30.0;
        let memory_ok = peak_rss.is_none_or(|bytes| bytes < 1_000_000_000);
        println!(
            "SPEC §13:   {} (< 30 s)  {} (< 1 GB at 4096²)",
            if time_ok { "PASS" } else { "FAIL" },
            if memory_ok { "PASS" } else { "FAIL" }
        );
    }
    Ok(())
}

/// Runs the single-shot pipeline at the same size and prints the differences,
/// as quality evidence for the tiled path (same parameters, same seed).
fn compare_with_single_shot(
    params: &Params,
    args: &Args,
    tiled: &tiling::LargeWorldResult,
) -> Result<(), String> {
    if args.grid > 2048 {
        println!(
            "compare:    skipped (single-shot at {}² would exceed the memory budget)",
            args.grid
        );
        return Ok(());
    }
    let mut p = params.clone();
    // Same physical world and resolution as the tiled pass: keep the parameter
    // extent, set the grid size and derive the cell size (generate_initial's
    // schema allows cells down to 1 km, so comparison is capped at 2048²).
    p.world.width = args.grid;
    p.world.height = args.grid;
    p.world.cell_km = params.world.extent_x_km() / f64::from(args.grid);
    p.allow_large_grid = true;
    let start = Instant::now();
    let single = geogenesis_core::pipeline::generate_initial(&p)
        .map_err(|error| format!("single-shot generation failed: {error}"))?;
    let elapsed = start.elapsed();
    let stats = &single.world;
    let single_land = stats.land_fraction();
    let (single_min, single_max) = stats.relief_range_m();
    let single_mean = stats.elevation_m.mean().unwrap_or(0.0);
    let single_std = stats.elevation_m.std_dev().unwrap_or(0.0);
    let sorted = stats.elevation_m.sorted_values();
    let quantile = |q: f64| -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let position = (q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64).round() as usize;
        sorted[position.min(sorted.len() - 1)]
    };
    let single_p10 = quantile(0.10);
    let single_p50 = quantile(0.50);
    let single_p90 = quantile(0.90);
    let single_p99 = quantile(0.99);
    let tiled_stats = tiled.elevation_stats();
    println!();
    println!(
        "compare:    single-shot {}² in {:.2} s (land {:.1}%, relief {:.0} m, mean {:.0} m)",
        args.grid,
        elapsed.as_secs_f64(),
        single_land * 100.0,
        single_max - single_min,
        single_mean
    );
    println!(
        "            tiled      (land {:.1}%, relief {:.0} m, mean {:.0} m)  ->  \
         Δland {:+.3}, Δrelief {:+.0} m, Δmean {:+.0} m, Δstd {:+.0} m",
        tiled.land_fraction() * 100.0,
        tiled_stats.relief_m(),
        tiled_stats.mean_m,
        tiled.land_fraction() - single_land,
        tiled_stats.relief_m() - (single_max - single_min),
        tiled_stats.mean_m - single_mean,
        tiled_stats.std_m - single_std
    );
    println!(
        "            quantiles  single p10/p50/p90/p99 = {:.0}/{:.0}/{:.0}/{:.0} m, \
         tiled p10/p50/p90 = {:.0}/{:.0}/{:.0} m",
        single_p10,
        single_p50,
        single_p90,
        single_p99,
        tiled_stats.p10_m,
        tiled_stats.median_m,
        tiled_stats.p90_m
    );
    Ok(())
}

fn parse_args(argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args::default();
    let mut argv = argv.peekable();
    while let Some(argument) = argv.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--grid" => args.grid = parse_u32(argv.next(), "--grid")?,
            "--out" => {
                args.out = Some(PathBuf::from(
                    argv.next().ok_or("`--out` needs a directory")?,
                ));
            }
            "--no-out" => args.out = None,
            "--seed" => {
                args.seed = argv
                    .next()
                    .ok_or("`--seed` needs a value")?
                    .parse()
                    .map_err(|_| "`--seed` must be an integer".to_string())?;
            }
            "--cell-km" => {
                args.cell_km = Some(
                    argv.next()
                        .ok_or("`--cell-km` needs a value")?
                        .parse()
                        .map_err(|_| "`--cell-km` must be a number".to_string())?,
                );
            }
            "--preset" => args.preset = Some(argv.next().ok_or("`--preset` needs a key")?),
            "--tile-size" => args.tile_size = parse_u32(argv.next(), "--tile-size")?,
            "--halo" => args.halo = parse_u32(argv.next(), "--halo")?,
            "--preview-factor" => {
                args.preview_factor = parse_u32(argv.next(), "--preview-factor")?;
            }
            "--workers" => args.workers = parse_u32(argv.next(), "--workers")?,
            "--normals" => args.normals = true,
            "--no-export" => args.no_export = true,
            "--compare-single-shot" => args.compare_single_shot = true,
            "--quiet" => args.quiet = true,
            other => return Err(format!("unknown option `{other}` (try --help)")),
        }
    }
    if args.grid < 16 {
        return Err("`--grid` must be at least 16".to_string());
    }
    Ok(args)
}

fn parse_u32(value: Option<String>, flag: &str) -> Result<u32, String> {
    value
        .ok_or_else(|| format!("`{flag}` needs a value"))?
        .parse()
        .map_err(|_| format!("`{flag}` must be an integer"))
}

/// Peak resident set size of this process, Linux only.
fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}
