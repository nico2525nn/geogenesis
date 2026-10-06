//! `geogenesis`: batch terrain generation and inspection.
//!
//! Dependency-free argument parsing (no clap): `geogenesis <command> [flags]`.
//! Flags are `--key value` pairs; `generate` also accepts `--set key=value`
//! overrides on top of a JSON parameter file.

use geogenesis_core::export::{format_named, write_export, ExportFormat};
use geogenesis_core::params::Params;
use geogenesis_core::sim::Simulation;
use geogenesis_core::stats::terrain_stats;
use std::path::PathBuf;

fn main() {
    let code = run(std::env::args().skip(1).collect());
    std::process::exit(code);
}

fn run(args: Vec<String>) -> i32 {
    if args.is_empty() {
        print_help();
        return 2;
    }
    match args[0].as_str() {
        "generate" => cmd_generate(&args[1..]),
        "info" => cmd_info(&args[1..]),
        "validate" => cmd_validate(&args[1..]),
        "help" | "--help" | "-h" => {
            print_help();
            0
        }
        other => {
            eprintln!("unknown command `{other}`");
            print_help();
            2
        }
    }
}

fn print_help() {
    eprintln!(
        "geogenesis — procedural terrain generation\n\
         \n\
         USAGE:\n  \
           genesis generate [--params FILE] [--set key=value]... [--format obj|xyz|raw|png] [--out FILE]\n  \
           genesis info [--params FILE] [--set key=value]...\n  \
           genesis validate [--params FILE] [--set key=value]...\n\
         \n\
         PARAM FILE: flat JSON object, e.g. {{\"seed\": 42, \"width\": 256}}.\n  \
         Unknown keys are ignored; out-of-range values are clamped."
    );
}

/// Parses `--key value` and `--set key=value` flags.
#[allow(clippy::type_complexity)]
fn parse_flags(args: &[String]) -> (Option<PathBuf>, Vec<(String, String)>, Option<String>, Option<PathBuf>) {
    let mut params_file = None;
    let mut sets = Vec::new();
    let mut format = None;
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--params" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    params_file = Some(PathBuf::from(v));
                }
            }
            "--set" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    if let Some((k, val)) = v.split_once('=') {
                        sets.push((k.to_string(), val.to_string()));
                    }
                }
            }
            "--format" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    format = Some(v.clone());
                }
            }
            "--out" | "-o" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    out = Some(PathBuf::from(v));
                }
            }
            _ => {}
        }
        i += 1;
    }
    (params_file, sets, format, out)
}

/// Loads params from an optional JSON file plus `--set` overrides.
fn load_params(file: Option<&PathBuf>, sets: &[(String, String)]) -> Result<Params, String> {
    let mut json = String::from("{}");
    if let Some(path) = file {
        json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    }
    // Merge --set overrides as JSON fragments.
    let mut merged = json.trim().trim_start_matches('{').trim_end_matches('}').to_string();
    for (k, v) in sets {
        if !merged.trim().is_empty() {
            merged.push(',');
        }
        merged.push_str(&format!("\"{k}\":{v}"));
    }
    let merged = format!("{{{merged}}}");
    params_from_cli_json(&merged)
}

/// Minimal JSON-to-Params mapping shared with the WASM layer's schema.
fn params_from_cli_json(text: &str) -> Result<Params, String> {
    // Reuse the WASM crate's parser contract by shelling the same keys:
    // keep one canonical key list by delegating to a shared builder.
    let mut p = Params::default();
    apply_json_map(&mut p, text)?;
    p.clamp_to_spec();
    let mut v = p.clone();
    v.validate().map_err(|e| e.to_string())?;
    Ok(p)
}

fn apply_json_map(p: &mut Params, text: &str) -> Result<(), String> {
    let get = |key: &str| extract_number(text, key);
    if let Some(v) = get("seed") {
        p.world.seed = v as u64;
    }
    if let Some(v) = get("width") {
        p.world.grid.0 = v as u32;
    }
    if let Some(v) = get("height") {
        p.world.grid.1 = v as u32;
    }
    if let Some(v) = get("plates") {
        p.plates.count = v as u32;
    }
    if let Some(v) = get("hotspots") {
        p.uplift.hotspot_count = v as u32;
    }
    if let Some(v) = get("octaves") {
        p.noise.octaves = v as u32;
    }
    // Numeric fields, applied through a table so the key list stays auditable.
    // Each entry is (json key, setter); the closure form avoids borrowing two
    // disjoint fields of `p` through one table of &mut.
    let mut set = |key: &str, v: f64| {
        match key {
            "spatial_km" => p.world.spatial_km = v,
            "vertical_m" => p.world.vertical_m = v,
            "sea_level_m" => p.world.sea_level_m = v,
            "lacunarity" => p.noise.lacunarity = v,
            "persistence" => p.noise.persistence = v,
            "anisotropy" => p.noise.anisotropy.amount = v,
            "fault_offset_m" => p.fault_offset_m = v,
            "fold_wavelength_km" => p.fold_wavelength_km = v,
            "fold_amplitude_m" => p.fold_amplitude_m = v,
            "thickness_km" => p.plates.thickness_km = v,
            "uplift_noise" => p.uplift.noise_frequency = v,
            "uplift_mm_yr" => p.uplift.mean_rate_mm_yr = v,
            "hotspot_strength_m" => p.uplift.hotspot_strength_m = v,
            "rain_mm" => p.rain.annual_mm = v,
            "heterogeneity" => p.rain.heterogeneity = v,
            "wind_dir_deg" => {
                p.rain.wind_dir_deg = v;
                p.wind.wind_dir_deg = v;
            }
            "rain_shadow" => p.rain.rain_shadow_strength = v,
            "seasonality" => p.rain.seasonality = v,
            "kf" => p.fluvial.kf = v,
            "kd" => p.fluvial.kd = v,
            "slope_exp" => p.fluvial.slope_exponent = v,
            "infiltration" => p.fluvial.infiltration = v,
            "diffusivity" => p.hillslope.diffusivity_m2_yr = v,
            "repose_deg" => p.hillslope.repose_angle_deg = v,
            "soil_prod" => p.hillslope.soil_production_m_yr = v,
            "max_soil" => p.hillslope.max_soil_m = v,
            "wind_speed" => p.wind.wind_speed_ms = v,
            "mean_temp_c" => {
                p.glacial.mean_temp_c = v;
                p.climate.mean_temp_c = v;
            }
            "ela_m" => p.glacial.ela_m = v,
            "wave_k" => p.coastal.wave_erosivity = v,
            "total_yr" => p.total_yr = v,
            "step_yr" => p.step_yr = v,
            _ => {}
        }
    };
    for key in [
        "spatial_km", "vertical_m", "sea_level_m", "lacunarity", "persistence",
        "anisotropy", "fault_offset_m", "fold_wavelength_km", "fold_amplitude_m",
        "thickness_km", "uplift_noise", "uplift_mm_yr", "hotspot_strength_m",
        "rain_mm", "heterogeneity", "wind_dir_deg", "rain_shadow", "seasonality",
        "kf", "kd", "slope_exp", "infiltration", "diffusivity", "repose_deg",
        "soil_prod", "max_soil", "wind_speed", "mean_temp_c", "ela_m", "wave_k",
        "total_yr", "step_yr",
    ] {
        if let Some(v) = get(key) {
            set(key, v);
        }
    }
    Ok(())
}

fn extract_number(text: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\"");
    let pos = text.find(&needle)?;
    let rest = &text[pos + needle.len()..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let mut end = 0;
    for (i, c) in after.char_indices() {
        if c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E') {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    after[..end].parse::<f64>().ok()
}

fn cmd_generate(args: &[String]) -> i32 {
    let (file, sets, format, out) = parse_flags(args);
    let params = match load_params(file.as_ref(), &sets) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("invalid params: {e}");
            return 1;
        }
    };
    let format = format
        .as_deref()
        .and_then(format_named)
        .unwrap_or(ExportFormat::Obj);
    let out = out.unwrap_or_else(|| PathBuf::from(default_name(format)));

    let mut sim = match Simulation::new(&params) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("build failed: {e}");
            return 1;
        }
    };
    sim.run();
    if let Err(e) = write_export(&out, format, sim.elevation(), params.world.cell_size_m()) {
        eprintln!("export failed: {e}");
        return 1;
    }
    let stats = terrain_stats(
        sim.elevation(),
        params.world.cell_size_m(),
        sim.state.sea_level_m,
    );
    println!(
        "wrote {} ({}x{}, {:.0}–{:.0} m, land {:.0}%)",
        out.display(),
        params.world.grid.0,
        params.world.grid.1,
        stats.elevation.min_m,
        stats.elevation.max_m,
        stats.hypsometry.land_fraction * 100.0
    );
    0
}

fn default_name(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::RawF64 => "terrain.bin",
        ExportFormat::XyzAscii => "terrain.xyz",
        ExportFormat::Obj => "terrain.obj",
        ExportFormat::Png16 => "terrain.png",
    }
}

fn cmd_info(args: &[String]) -> i32 {
    let (file, sets, _, _) = parse_flags(args);
    let params = match load_params(file.as_ref(), &sets) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("invalid params: {e}");
            return 1;
        }
    };
    let mut sim = match Simulation::new(&params) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("build failed: {e}");
            return 1;
        }
    };
    sim.run();
    let stats = terrain_stats(
        sim.elevation(),
        params.world.cell_size_m(),
        sim.state.sea_level_m,
    );
    let e = &stats.elevation;
    let s = &stats.slope;
    println!(
        "elevation min {:.1} max {:.1} mean {:.1} std {:.1} (m)\n\
         slope mean {:.1} max {:.1} deg, ruggedness {:.2} m\n\
         land {:.1}%, hypsometric integral {:.3}, elapsed {:.0} yr",
        e.min_m,
        e.max_m,
        e.mean_m,
        e.std_m,
        s.mean_deg,
        s.max_deg,
        s.ruggedness,
        stats.hypsometry.land_fraction * 100.0,
        stats.hypsometry.integral,
        sim.state.elapsed_yr
    );
    0
}

fn cmd_validate(args: &[String]) -> i32 {
    let (file, sets, _, _) = parse_flags(args);
    match load_params(file.as_ref(), &sets) {
        Ok(_) => {
            println!("params valid");
            0
        }
        Err(e) => {
            eprintln!("invalid: {e}");
            1
        }
    }
}
