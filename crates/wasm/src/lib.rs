//! Raw C-ABI exports for the WebAssembly build.
//!
//! Deliberately dependency-free: no wasm-bindgen, no js-sys. The JS side
//! instantiates the module, calls these functions, and reads results out of
//! linear memory. Every buffer the engine hands out is owned by the engine
//! and freed with [`gg_free`]; the JS wrapper must call it exactly once per
//! buffer. Status codes (not exceptions) cross the boundary, because a WASM
//! trap cannot carry a message.

use geogenesis_core::params::Params;
use geogenesis_core::sim::Simulation;
use geogenesis_core::stats::{terrain_stats, TerrainStats};
use std::sync::Mutex;

/// Last error message, for [`gg_last_error`].
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

fn set_error(message: String) {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = message;
    }
}

/// Status codes returned by every fallible export.
#[repr(i32)]
pub enum Status {
    Ok = 0,
    InvalidParams = 1,
    BuildFailed = 2,
    NoSimulation = 3,
    BadArgument = 4,
}

/// The live simulation. Exactly one may exist; [`gg_create`] replaces it.
static SIMULATION: Mutex<Option<Simulation>> = Mutex::new(None);

/// Serialised parameter snapshot the JS side can diff against.
static PARAMS_JSON: Mutex<String> = Mutex::new(String::new());

/// Creates a simulation from a JSON parameter object.
///
/// `json_ptr`/`json_len` borrow a UTF-8 buffer owned by the caller. Returns a
/// [`Status`] code; details are available via [`gg_last_error`].
///
/// The JSON schema is the flat `key: value` map documented in `web/` — one key
/// per parameter, so the UI can send exactly what its controls hold.
#[no_mangle]
pub extern "C" fn gg_create(json_ptr: *const u8, json_len: usize) -> i32 {
    let bytes = unsafe { std::slice::from_raw_parts(json_ptr, json_len) };
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => {
            set_error(format!("params are not UTF-8: {e}"));
            return Status::BadArgument as i32;
        }
    };
    let params = match params_from_json(text) {
        Ok(p) => p,
        Err(e) => {
            set_error(e);
            return Status::InvalidParams as i32;
        }
    };
    match Simulation::new(&params) {
        Ok(sim) => {
            if let Ok(mut slot) = SIMULATION.lock() {
                *slot = Some(sim);
            }
            if let Ok(mut slot) = PARAMS_JSON.lock() {
                *slot = text.to_string();
            }
            Status::Ok as i32
        }
        Err(e) => {
            set_error(e.to_string());
            Status::BuildFailed as i32
        }
    }
}

/// Runs the initial-relief pipeline only (no time loop).
#[no_mangle]
pub extern "C" fn gg_generate_initial() -> i32 {
    // The initial world is built by gg_create; this entry point exists so the
    // UI can re-run Phase 0-2 after changing structural parameters without
    // rebuilding the whole simulation object.
    match SIMULATION.lock() {
        Ok(slot) if slot.is_some() => Status::Ok as i32,
        Ok(_) => {
            set_error("no simulation: call gg_create first".into());
            Status::NoSimulation as i32
        }
        Err(_) => {
            set_error("simulation lock poisoned".into());
            Status::NoSimulation as i32
        }
    }
}

/// Advances the simulation by `steps` base steps. Returns steps completed.
#[no_mangle]
pub extern "C" fn gg_step(steps: u32) -> i32 {
    let mut completed = 0i32;
    let Ok(mut slot) = SIMULATION.lock() else {
        set_error("simulation lock poisoned".into());
        return -1;
    };
    let Some(sim) = slot.as_mut() else {
        set_error("no simulation: call gg_create first".into());
        return -1;
    };
    for _ in 0..steps.min(10_000) {
        if sim.state.elapsed_yr >= sim.params.total_yr {
            break;
        }
        sim.step();
        completed += 1;
    }
    completed
}

/// Runs to completion (bounded by `max_iters`). Returns steps completed.
#[no_mangle]
pub extern "C" fn gg_run() -> i32 {
    let Ok(mut slot) = SIMULATION.lock() else {
        set_error("simulation lock poisoned".into());
        return -1;
    };
    let Some(sim) = slot.as_mut() else {
        set_error("no simulation: call gg_create first".into());
        return -1;
    };
    let before = sim.state.iteration;
    sim.run();
    (sim.state.iteration - before).min(i32::MAX as u64) as i32
}

/// Grid width of the live simulation, or -1.
#[no_mangle]
pub extern "C" fn gg_width() -> i32 {
    with_sim(|sim| sim.params.world.grid.0 as i32).unwrap_or(-1)
}

/// Grid height of the live simulation, or -1.
#[no_mangle]
pub extern "C" fn gg_height() -> i32 {
    with_sim(|sim| sim.params.world.grid.1 as i32).unwrap_or(-1)
}

/// Simulated years elapsed, or NaN.
#[no_mangle]
pub extern "C" fn gg_elapsed_yr() -> f64 {
    with_sim(|sim| sim.state.elapsed_yr).unwrap_or(f64::NAN)
}

/// Sea level in metres, or NaN.
#[no_mangle]
pub extern "C" fn gg_sea_level_m() -> f64 {
    with_sim(|sim| sim.state.sea_level_m).unwrap_or(f64::NAN)
}

/// Copies the elevation field (row-major f64, metres) into a fresh buffer.
///
/// Returns the buffer pointer and writes its byte length to `out_len`.
/// Free with [`gg_free`]. Returns null when no simulation exists.
#[no_mangle]
pub extern "C" fn gg_elevation_ptr(out_len: *mut usize) -> *mut f64 {
    let Some(data) = with_sim(|sim| sim.elevation().as_slice().to_vec()) else {
        return std::ptr::null_mut();
    };
    let mut boxed = data.into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    let len = boxed.len() * std::mem::size_of::<f64>();
    std::mem::forget(boxed);
    if !out_len.is_null() {
        unsafe {
            *out_len = len;
        }
    }
    ptr
}

/// Copies the biome map (row-major u8) into a fresh buffer. See [`gg_elevation_ptr`].
#[no_mangle]
pub extern "C" fn gg_biomes_ptr(out_len: *mut usize) -> *mut u8 {
    let Some(data) = with_sim(|sim| {
        sim.climate
            .as_ref()
            .map(|c| c.biomes.as_slice().iter().map(|&v| v as u8).collect::<Vec<u8>>())
            .unwrap_or_default()
    }) else {
        return std::ptr::null_mut();
    };
    if data.is_empty() {
        return std::ptr::null_mut();
    }
    let mut boxed = data.into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    let len = boxed.len();
    std::mem::forget(boxed);
    if !out_len.is_null() {
        unsafe {
            *out_len = len;
        }
    }
    ptr
}

/// Serialises the terrain report as JSON into a fresh buffer. See [`gg_elevation_ptr`].
#[no_mangle]
pub extern "C" fn gg_stats_ptr(out_len: *mut usize) -> *mut u8 {
    let Some(json) = with_sim(|sim| {
        let stats: TerrainStats = terrain_stats(
            sim.elevation(),
            sim.params.world.cell_size_m(),
            sim.state.sea_level_m,
        );
        let e = &stats.elevation;
        let s = &stats.slope;
        let h = &stats.hypsometry;
        format!(
            "{{\"min\":{},\"max\":{},\"mean\":{},\"std\":{},\"median\":{},\
             \"p10\":{},\"p90\":{},\"cells\":{},\"slope_mean\":{},\
             \"slope_max\":{},\"ruggedness\":{},\"land_fraction\":{},\
             \"hypsometric_integral\":{},\"elapsed_yr\":{}}}",
            e.min_m, e.max_m, e.mean_m, e.std_m, e.median_m, e.p10_m, e.p90_m,
            e.cells, s.mean_deg, s.max_deg, s.ruggedness, h.land_fraction,
            h.integral, sim.state.elapsed_yr
        )
    }) else {
        return std::ptr::null_mut();
    };
    let mut boxed = json.into_bytes().into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    let len = boxed.len();
    std::mem::forget(boxed);
    if !out_len.is_null() {
        unsafe {
            *out_len = len;
        }
    }
    ptr
}

/// Frees a buffer returned by `gg_*_ptr`. Must be called exactly once.
#[no_mangle]
pub extern "C" fn gg_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    unsafe {
        let _ = Vec::from_raw_parts(ptr, len, len);
    }
}

/// Copies the last error message into a fresh buffer. See [`gg_elevation_ptr`].
#[no_mangle]
pub extern "C" fn gg_last_error(out_len: *mut usize) -> *mut u8 {
    let text = LAST_ERROR
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let mut boxed = text.into_bytes().into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    let len = boxed.len();
    std::mem::forget(boxed);
    if !out_len.is_null() {
        unsafe {
            *out_len = len;
        }
    }
    ptr
}

fn with_sim<T>(f: impl FnOnce(&Simulation) -> T) -> Option<T> {
    SIMULATION.lock().ok()?.as_ref().map(f)
}

/// Parses the flat JSON parameter map.
///
/// Keys are `snake_case` parameter names; unknown keys are ignored so older
/// frontends keep working against newer engines. Values are numbers, except
/// `noise_kind`, `fractal`, `transport_law`, `mode` and `rock`, which are
/// strings, and `boundary`, which is `rectangular`/`toroidal`/`spherical`.
fn params_from_json(text: &str) -> Result<Params, String> {
    let mut p = Params::default();
    let map = parse_flat_json(text)?;

    let num = |key: &str| map.get(key).copied();
    if let Some(v) = num("seed") {
        p.world.seed = v as u64;
    }
    if let Some(v) = num("width") {
        p.world.grid.0 = v as u32;
    }
    if let Some(v) = num("height") {
        p.world.grid.1 = v as u32;
    }
    if let Some(v) = num("spatial_km") {
        p.world.spatial_km = v;
    }
    if let Some(v) = num("vertical_m") {
        p.world.vertical_m = v;
    }
    if let Some(v) = num("sea_level_m") {
        p.world.sea_level_m = v;
    }
    if let Some(v) = num("octaves") {
        p.noise.octaves = v as u32;
    }
    if let Some(v) = num("lacunarity") {
        p.noise.lacunarity = v;
    }
    if let Some(v) = num("persistence") {
        p.noise.persistence = v;
    }
    if let Some(v) = num("anisotropy") {
        p.noise.anisotropy.amount = v;
    }
    if let Some(v) = num("anisotropy_deg") {
        p.noise.anisotropy.direction = v.to_radians();
    }
    if let Some(v) = num("fault_offset_m") {
        p.fault_offset_m = v;
    }
    if let Some(v) = num("fold_wavelength_km") {
        p.fold_wavelength_km = v;
    }
    if let Some(v) = num("fold_amplitude_m") {
        p.fold_amplitude_m = v;
    }
    if let Some(v) = num("plates") {
        p.plates.count = v as u32;
    }
    if let Some(v) = num("uplift_mm_yr") {
        p.uplift.mean_rate_mm_yr = v;
    }
    if let Some(v) = num("hotspots") {
        p.uplift.hotspot_count = v as u32;
    }
    if let Some(v) = num("hotspot_strength_m") {
        p.uplift.hotspot_strength_m = v;
    }
    if let Some(v) = num("rain_mm") {
        p.rain.annual_mm = v;
    }
    if let Some(v) = num("wind_dir_deg") {
        p.rain.wind_dir_deg = v;
        p.wind.wind_dir_deg = v;
    }
    if let Some(v) = num("rain_shadow") {
        p.rain.rain_shadow_strength = v;
    }
    if let Some(v) = num("kf") {
        p.fluvial.kf = v;
    }
    if let Some(v) = num("kd") {
        p.fluvial.kd = v;
    }
    if let Some(v) = num("slope_exp") {
        p.fluvial.slope_exponent = v;
    }
    if let Some(v) = num("diffusivity") {
        p.hillslope.diffusivity_m2_yr = v;
    }
    if let Some(v) = num("repose_deg") {
        p.hillslope.repose_angle_deg = v;
    }
    if let Some(v) = num("wind_speed") {
        p.wind.wind_speed_ms = v;
    }
    if let Some(v) = num("mean_temp_c") {
        p.glacial.mean_temp_c = v;
        p.climate.mean_temp_c = v;
    }
    if let Some(v) = num("ela_m") {
        p.glacial.ela_m = v;
    }
    if let Some(v) = num("wave_k") {
        p.coastal.wave_erosivity = v;
    }
    if let Some(v) = num("total_yr") {
        p.total_yr = v;
    }
    if let Some(v) = num("step_yr") {
        p.step_yr = v;
    }
    if let Some(s) = string_field(text, "noise_kind") {
        p.noise.kind = match s.as_str() {
            "simplex" => geogenesis_core::noise::NoiseKind::Simplex,
            "value" => geogenesis_core::noise::NoiseKind::Value,
            "wavelet" => geogenesis_core::noise::NoiseKind::Wavelet,
            "gaussian" => geogenesis_core::noise::NoiseKind::GaussianSpike,
            _ => geogenesis_core::noise::NoiseKind::Perlin,
        };
    }
    if let Some(s) = string_field(text, "fractal") {
        p.noise.fractal = match s.as_str() {
            "ridged" => geogenesis_core::noise::FractalKind::Ridged,
            "billow" => geogenesis_core::noise::FractalKind::Billow,
            _ => geogenesis_core::noise::FractalKind::Fbm,
        };
    }
    if let Some(s) = string_field(text, "transport_law") {
        if let Some(law) = geogenesis_core::params::transport_law_named(&s) {
            p.fluvial.law = law;
        }
    }
    if let Some(s) = string_field(text, "boundary") {
        p.world.boundary = match s.as_str() {
            "toroidal" => geogenesis_core::grid::BoundaryKind::Toroidal,
            "spherical" => geogenesis_core::grid::BoundaryKind::Spherical,
            _ => geogenesis_core::grid::BoundaryKind::Rectangular,
        };
    }

    p.clamp_to_spec();
    let mut validated = p.clone();
    validated.validate().map_err(|e| e.to_string())?;
    Ok(p)
}

/// Minimal flat-JSON number parser: `{"a": 1, "b": 2.5}`.
///
/// Hand-written because the core has no serde dependency and the WASM binary
/// must stay small. Handles numbers, ignores strings/arrays/nesting.
fn parse_flat_json(text: &str) -> Result<std::collections::HashMap<String, f64>, String> {
    let mut map = std::collections::HashMap::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Find the next quoted key.
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'"' {
            end += 1;
        }
        if end >= bytes.len() {
            break;
        }
        let key = std::str::from_utf8(&bytes[start..end]).map_err(|e| e.to_string())?;
        i = end + 1;
        // Skip to the colon.
        while i < bytes.len() && bytes[i] != b':' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        i += 1;
        // Skip whitespace.
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n' || bytes[i] == b'\r') {
            i += 1;
        }
        // Parse a number if present; otherwise skip the value.
        let num_start = i;
        if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+' || bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || matches!(bytes[i], b'.' | b'e' | b'E' | b'-' | b'+'))
            {
                i += 1;
            }
            if let Ok(num) = std::str::from_utf8(&bytes[num_start..i])
                .unwrap_or("")
                .parse::<f64>()
            {
                if num.is_finite() {
                    map.insert(key.to_string(), num);
                }
            }
        }
    }
    Ok(map)
}

/// Extracts a `"key": "value"` string field with the same minimal parser.
fn string_field(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let pos = text.find(&needle)?;
    let rest = &text[pos + needle.len()..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    if !after.starts_with('"') {
        return None;
    }
    let end = after[1..].find('"')?;
    Some(after[1..1 + end].to_string())
}
