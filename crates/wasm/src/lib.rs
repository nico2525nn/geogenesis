//! WebAssembly C-ABI bridge for the GeoGenesis engine.
//!
//! Deliberately dependency-free: no wasm-bindgen, no js-sys. The JS side
//! instantiates the module, copies parameters into linear memory, calls these
//! functions and reads results back. Every buffer handed out is owned by the
//! engine and freed with [`gg_free`] using the *byte* length reported by the
//! `out_len` slot.
//!
//! # Contract
//!
//! * All fallible calls return an [`i32`] status; details are available through
//!   [`gg_last_error_ptr`].
//! * Flat parameter encoding: a `Float64Array` whose order matches
//!   [`gg_schema_ptr`] (one row per parameter). Enums are indices.
//! * Field buffers are `f32`, row-major. Categorical fields (biome, rock, river)
//!   are widened to `f32`; the JS side converts back when needed.
//! * The simulation is global (`Mutex<Option<Simulation>>`) because the module
//!   is instantiated once per Worker.
//!
//! This is the only crate in the workspace with `unsafe`: raw slice/pointer
//! construction across the C ABI is inherently unsafe, and every occurrence is
//! confined to this file and justified by the caller contract above.

use std::sync::Mutex;

use geogenesis_core::climate::Biome;
use geogenesis_core::export;
use geogenesis_core::params::{push_json_escaped, push_json_number, Params};
use geogenesis_core::pipeline;
use geogenesis_core::sim::{FieldKind, Simulation};
use geogenesis_core::stats;

/// Status codes shared with the JS loader.
#[derive(Debug)]
#[repr(i32)]
pub enum Status {
    Ok = 0,
    InvalidParams = 1,
    BuildFailed = 2,
    NoSimulation = 3,
    BadArgument = 4,
    Internal = 5,
}

/// Last error message for the JS side.
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());
/// The live simulation.
static SIMULATION: Mutex<Option<Box<Simulation>>> = Mutex::new(None);

/// Result of the most recent low-resolution parameter preview.
struct PreviewWorld {
    info_json: String,
    elevation: Vec<f32>,
}

/// Low-resolution preview of the current parameter set, used while the user
/// drags sliders before committing to a full generation. Kept separate from
/// the live simulation so previewing never disturbs the generated world.
static PREVIEW: Mutex<Option<PreviewWorld>> = Mutex::new(None);

fn set_error(message: impl Into<String>) {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = message.into();
    }
}

fn status(code: Status) -> i32 {
    code as i32
}

/// Copies bytes into a fresh engine-owned buffer. Free with [`gg_free`].
///
/// The buffer is first shrunk to a boxed slice so its allocation length equals
/// its logical length; `gg_free` reconstructs it with `Vec::from_raw_parts` and
/// requires that capacity/length match exactly.
fn into_buffer(bytes: Vec<u8>, out_len: *mut usize) -> *mut u8 {
    let mut boxed = bytes.into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    let len = boxed.len();
    std::mem::forget(boxed);
    if !out_len.is_null() {
        // Safety: the caller passed a valid out-pointer per the ABI contract.
        unsafe {
            *out_len = len;
        }
    }
    ptr
}

fn string_buffer(text: String, out_len: *mut usize) -> *mut u8 {
    into_buffer(text.into_bytes(), out_len)
}

fn f32_buffer(values: Vec<f32>, out_len: *mut usize) -> *mut u8 {
    // Same capacity == length requirement as `into_buffer`.
    let mut boxed = values.into_boxed_slice();
    let ptr = boxed.as_mut_ptr().cast::<u8>();
    let len = boxed.len() * std::mem::size_of::<f32>();
    std::mem::forget(boxed);
    if !out_len.is_null() {
        // Safety: valid out-pointer per contract.
        unsafe {
            *out_len = len;
        }
    }
    ptr
}

/// Generates a low-resolution preview of `values` at roughly `target_width`
/// cells and stores it for [`gg_preview_ptr`] / [`gg_preview_info_ptr`].
///
/// The preview runs the real generation pipeline on a coarser grid, so it
/// shows the same continents, biomes and relief as the final world. It does
/// not touch the live simulation.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn gg_preview(values_ptr: *const f64, count: usize, target_width: u32) -> i32 {
    if values_ptr.is_null() || count == 0 {
        set_error("parameter array is empty");
        return status(Status::BadArgument);
    }
    // Safety: JS allocated `count * 8` bytes with `gg_alloc` and filled them.
    let values = unsafe { std::slice::from_raw_parts(values_ptr, count) };
    let params = match Params::from_values(values) {
        Ok(p) => p,
        Err(e) => {
            set_error(e.to_string());
            return status(Status::InvalidParams);
        }
    };
    let width = target_width.clamp(32, 512);
    let out = match pipeline::generate_preview(&params, width) {
        Ok(out) => out,
        Err(e) => {
            set_error(e.to_string());
            return status(Status::BuildFailed);
        }
    };
    let grid = out.world.grid;
    let mut info = String::from("{\"width\":");
    push_json_number(&mut info, grid.width() as f64);
    info.push_str(",\"height\":");
    push_json_number(&mut info, grid.height() as f64);
    info.push_str(",\"seaLevel\":");
    push_json_number(&mut info, out.world.sea_level_m);
    info.push_str(",\"time\":0}");
    let elevation = out.world.elevation_f32();
    if let Ok(mut slot) = PREVIEW.lock() {
        *slot = Some(PreviewWorld {
            info_json: info,
            elevation,
        });
    } else {
        set_error("preview lock poisoned");
        return status(Status::Internal);
    }
    status(Status::Ok)
}

/// Metadata of the most recent preview: `{width,height,seaLevel,time}` JSON.
#[no_mangle]
pub extern "C" fn gg_preview_info_ptr(out_len: *mut usize) -> *mut u8 {
    let text = match PREVIEW.lock() {
        Ok(slot) => slot
            .as_ref()
            .map_or_else(|| "null".to_string(), |preview| preview.info_json.clone()),
        Err(_) => "null".to_string(),
    };
    string_buffer(text, out_len)
}

/// Elevation (`f32`, row-major) of the most recent preview. Free with
/// [`gg_free`] using the reported byte length.
#[no_mangle]
pub extern "C" fn gg_preview_ptr(out_len: *mut usize) -> *mut u8 {
    let values = match PREVIEW.lock() {
        Ok(slot) => slot
            .as_ref()
            .map(|preview| preview.elevation.clone())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    f32_buffer(values, out_len)
}

/// Export formats for [`gg_export_ptr`].
///
/// `0` heightmap PNG (16-bit), `1` normal-map PNG, `2` self-contained glTF,
/// `3` cross-section PNG, `4` OpenEXR float heightmap, `5` NetCDF-3 classic
/// gridded fields (elevation + available climate/hydrology). The buffer is
/// engine-owned: copy it out and free it with [`gg_free`] using the reported
/// byte length.
#[no_mangle]
pub extern "C" fn gg_export_ptr(kind: u32, out_len: *mut usize) -> *mut u8 {
    let Ok(slot) = SIMULATION.lock() else {
        set_error("simulation lock poisoned");
        return into_buffer(Vec::new(), out_len);
    };
    let Some(sim) = slot.as_ref() else {
        set_error("no simulation");
        return into_buffer(Vec::new(), out_len);
    };
    let bytes = match kind {
        0 => export::height_png16(&sim.world),
        1 => export::normal_map_png(&sim.world),
        2 => export::gltf_string(&sim.world, &sim.params, 512).into_bytes(),
        3 => {
            let Some(climate) = sim.climate() else {
                set_error("no climate snapshot for profile export");
                return into_buffer(Vec::new(), out_len);
            };
            let grid = sim.world.grid;
            let samples = stats::profile(
                &sim.world,
                climate,
                sim.geology(),
                f64::from(grid.width() as u32) * 0.15,
                f64::from(grid.height() as u32) * 0.5,
                f64::from(grid.width() as u32) * 0.85,
                f64::from(grid.height() as u32) * 0.5,
                768,
            );
            export::profile_png(&sim.world, &samples)
        }
        4 => {
            let grid = sim.world.grid;
            let elevation = sim.world.elevation_f32();
            match export::exr::encode_exr(
                grid.width() as u32,
                grid.height() as u32,
                &[export::exr::ExrChannel {
                    name: "Y",
                    sample_type: export::exr::ExrSampleType::Float,
                    values: &elevation,
                }],
            ) {
                Ok(bytes) => bytes,
                Err(error) => {
                    set_error(error.to_string());
                    return into_buffer(Vec::new(), out_len);
                }
            }
        }
        5 => {
            let grid = sim.world.grid;
            let width = grid.width() as u32;
            let height = grid.height() as u32;
            let elevation = sim.world.elevation_f32();
            let mut values: Vec<(&str, Option<&str>, Vec<f32>)> =
                vec![("elevation_m", Some("m"), elevation)];
            if let Some(climate) = sim.climate() {
                values.push((
                    "precipitation_mm",
                    Some("mm/yr"),
                    climate.precipitation_mm.as_slice().to_vec(),
                ));
                values.push((
                    "temperature_c",
                    Some("degC"),
                    climate.temperature_c.as_slice().to_vec(),
                ));
            }
            let hydro = sim.hydro();
            values.push((
                "discharge_m3_yr",
                Some("m^3/yr"),
                hydro
                    .discharge_m3_yr
                    .as_slice()
                    .iter()
                    .map(|v| *v as f32)
                    .collect(),
            ));
            let fields: Vec<export::netcdf::NetCdfField<'_>> = values
                .iter()
                .map(|(name, units, samples)| export::netcdf::NetCdfField {
                    name,
                    units: *units,
                    values: samples,
                })
                .collect();
            match export::netcdf::encode_netcdf_fields(width, height, &fields) {
                Ok(bytes) => bytes,
                Err(error) => {
                    set_error(error.to_string());
                    return into_buffer(Vec::new(), out_len);
                }
            }
        }
        _ => {
            set_error(format!("unknown export kind {kind}"));
            return into_buffer(Vec::new(), out_len);
        }
    };
    into_buffer(bytes, out_len)
}

/// Engine version string.
#[no_mangle]
pub extern "C" fn gg_version_ptr(out_len: *mut usize) -> *mut u8 {
    string_buffer(geogenesis_core::VERSION.to_string(), out_len)
}

/// Parameter schema as JSON (see `Params::schema_json`).
#[no_mangle]
pub extern "C" fn gg_schema_ptr(out_len: *mut usize) -> *mut u8 {
    string_buffer(Params::schema_json(), out_len)
}

/// Presets as JSON (key, bilingual labels, description, overrides).
#[no_mangle]
pub extern "C" fn gg_presets_ptr(out_len: *mut usize) -> *mut u8 {
    string_buffer(Params::presets_json(), out_len)
}

/// Allocates `size` bytes owned by the engine. Used by JS to marshal the
/// parameter array into linear memory. Free with [`gg_free`].
#[no_mangle]
pub extern "C" fn gg_alloc(size: usize) -> *mut u8 {
    if size == 0 {
        return std::ptr::null_mut();
    }
    let mut buffer = vec![0u8; size];
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Frees a buffer returned by any `gg_*_ptr` function or by [`gg_alloc`].
///
/// `len` is the **byte** length reported by the producing call.
// Safety: the JS caller contract guarantees the pointer/length pair comes from
// an engine allocation and is freed exactly once. The ABI cannot express that
// in the Rust type system, so the check is documented here instead.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn gg_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    // Safety: the caller contract guarantees this pointer/length pair came
    // from one of the engine's allocations and is freed exactly once.
    unsafe {
        let _ = Vec::from_raw_parts(ptr, len, len);
    }
}

/// Copies the last error message into a fresh buffer.
#[no_mangle]
pub extern "C" fn gg_last_error_ptr(out_len: *mut usize) -> *mut u8 {
    let text = LAST_ERROR.lock().map(|s| s.clone()).unwrap_or_default();
    string_buffer(text, out_len)
}

/// Creates (or replaces) the simulation from a flat parameter array.
///
/// Returns [`Status::Ok`] or an error code. The parameter array is read, not
/// retained.
// Safety: the JS caller allocates `count * 8` bytes with `gg_alloc` and fills
// them before calling; documented in the module header.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn gg_create(values_ptr: *const f64, count: usize) -> i32 {
    if values_ptr.is_null() || count == 0 {
        set_error("parameter array is empty");
        return status(Status::BadArgument);
    }
    // Safety: JS allocated `count * 8` bytes with `gg_alloc` and filled them.
    let values = unsafe { std::slice::from_raw_parts(values_ptr, count) };
    let params = match Params::from_values(values) {
        Ok(p) => p,
        Err(e) => {
            set_error(e.to_string());
            return status(Status::InvalidParams);
        }
    };
    match Simulation::new(&params) {
        Ok(sim) => {
            if let Ok(mut slot) = SIMULATION.lock() {
                *slot = Some(Box::new(sim));
            } else {
                set_error("simulation lock poisoned");
                return status(Status::Internal);
            }
            status(Status::Ok)
        }
        Err(e) => {
            set_error(e.to_string());
            status(Status::BuildFailed)
        }
    }
}

/// Creates a simulation with optional user-defined geology JSON documents.
///
/// `rocks_ptr/rocks_len` and `strata_ptr/strata_len` are UTF-8 JSON buffers
/// (allocated by the caller with `gg_alloc`); pass a null pointer and zero
/// length for "use the built-in defaults". Rocks use the `RockSet` JSON format
/// (`{"version":1,"custom_rocks":[...]}`), strata the `StrataSpec` format
/// (`{"version":1,"layers":[...]}`); see `examples/` in the repository.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn gg_create_with_geology(
    values_ptr: *const f64,
    count: usize,
    rocks_ptr: *const u8,
    rocks_len: usize,
    strata_ptr: *const u8,
    strata_len: usize,
) -> i32 {
    if values_ptr.is_null() || count == 0 {
        set_error("parameter array is empty");
        return status(Status::BadArgument);
    }
    let values = unsafe { std::slice::from_raw_parts(values_ptr, count) };
    let params = match Params::from_values(values) {
        Ok(p) => p,
        Err(e) => {
            set_error(e.to_string());
            return status(Status::InvalidParams);
        }
    };
    let rocks_json = match read_optional_utf8(rocks_ptr, rocks_len) {
        Ok(text) => text,
        Err(message) => {
            set_error(message);
            return status(Status::BadArgument);
        }
    };
    let strata_json = match read_optional_utf8(strata_ptr, strata_len) {
        Ok(text) => text,
        Err(message) => {
            set_error(message);
            return status(Status::BadArgument);
        }
    };
    let mut sim = match Simulation::new(&params) {
        Ok(sim) => sim,
        Err(e) => {
            set_error(e.to_string());
            return status(Status::BuildFailed);
        }
    };
    let strata_opt = if strata_json.trim().is_empty() {
        None
    } else {
        Some(strata_json.as_str())
    };
    if let Err(e) = sim.set_geology_from_json(&rocks_json, strata_opt) {
        set_error(e.to_string());
        return status(Status::InvalidParams);
    }
    if let Ok(mut slot) = SIMULATION.lock() {
        *slot = Some(Box::new(sim));
    } else {
        set_error("simulation lock poisoned");
        return status(Status::Internal);
    }
    status(Status::Ok)
}

fn read_optional_utf8(ptr: *const u8, len: usize) -> Result<String, String> {
    if ptr.is_null() || len == 0 {
        return Ok(String::new());
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes)
        .map(ToOwned::to_owned)
        .map_err(|e| format!("geology JSON is not UTF-8: {e}"))
}

/// JSON describing the user-defined geology config of the current simulation
/// (`{"rocks": ..., "strata": ...}`) or `null` when the world uses defaults.
#[no_mangle]
pub extern "C" fn gg_geology_json_ptr(out_len: *mut usize) -> *mut u8 {
    let Ok(slot) = SIMULATION.lock() else {
        set_error("simulation lock poisoned");
        return into_buffer(Vec::new(), out_len);
    };
    let Some(sim) = slot.as_ref() else {
        set_error("no simulation");
        return into_buffer(Vec::new(), out_len);
    };
    match sim.custom_geology_json() {
        Some(json) => string_buffer(json, out_len),
        None => string_buffer("null".to_string(), out_len),
    }
}

/// Advances by `steps` base steps; returns the number completed.
#[no_mangle]
pub extern "C" fn gg_step(steps: u32) -> i32 {
    let Ok(mut slot) = SIMULATION.lock() else {
        set_error("simulation lock poisoned");
        return -1;
    };
    let Some(sim) = slot.as_mut() else {
        set_error("no simulation");
        return -1;
    };
    let completed = sim.step_many(steps.min(100_000));
    completed.min(i32::MAX as u32) as i32
}

/// Runs to the configured end, returning the number of steps taken.
#[no_mangle]
pub extern "C" fn gg_run() -> i32 {
    let Ok(mut slot) = SIMULATION.lock() else {
        set_error("simulation lock poisoned");
        return -1;
    };
    let Some(sim) = slot.as_mut() else {
        set_error("no simulation");
        return -1;
    };
    let before = sim.state.iteration;
    sim.run();
    (sim.state.iteration - before).min(i32::MAX as u64) as i32
}

/// Rebuilds the simulation from the current parameters, dropping history.
#[no_mangle]
pub extern "C" fn gg_reset() -> i32 {
    let Ok(mut slot) = SIMULATION.lock() else {
        set_error("simulation lock poisoned");
        return status(Status::Internal);
    };
    let Some(sim) = slot.as_mut() else {
        set_error("no simulation");
        return status(Status::NoSimulation);
    };
    match sim.reset() {
        Ok(()) => status(Status::Ok),
        Err(e) => {
            set_error(e.to_string());
            status(Status::BuildFailed)
        }
    }
}

/// Rebuilds from the initial conditions while preserving timeline frames.
///
/// This is the primitive behind backward scrubbing: frames captured so far
/// describe the same deterministic run, so they remain valid after the state
/// is rebuilt and replayed.
#[no_mangle]
pub extern "C" fn gg_rewind() -> i32 {
    let Ok(mut slot) = SIMULATION.lock() else {
        set_error("simulation lock poisoned");
        return status(Status::Internal);
    };
    let Some(sim) = slot.as_mut() else {
        set_error("no simulation");
        return status(Status::NoSimulation);
    };
    match sim.reset_preserving_history() {
        Ok(()) => status(Status::Ok),
        Err(e) => {
            set_error(e.to_string());
            status(Status::BuildFailed)
        }
    }
}

/// Seeks to an absolute simulation time (years), rebuilding if needed.
#[no_mangle]
pub extern "C" fn gg_seek(year: f64) -> i32 {
    let Ok(mut slot) = SIMULATION.lock() else {
        set_error("simulation lock poisoned");
        return -1;
    };
    let Some(sim) = slot.as_mut() else {
        set_error("no simulation");
        return -1;
    };
    if !year.is_finite() {
        set_error("seek target is not finite");
        return -1;
    }
    match sim.seek(year) {
        Ok(steps) => steps.min(i32::MAX as u64) as i32,
        Err(e) => {
            set_error(e.to_string());
            -1
        }
    }
}

/// JSON summary of the simulation state, timeline metadata and ledger.
#[no_mangle]
pub extern "C" fn gg_state_ptr(out_len: *mut usize) -> *mut u8 {
    let Ok(slot) = SIMULATION.lock() else {
        return string_buffer("{\"error\":\"lock poisoned\"}".into(), out_len);
    };
    let Some(sim) = slot.as_ref() else {
        return string_buffer("{\"error\":\"no simulation\"}".into(), out_len);
    };
    string_buffer(state_json(sim), out_len)
}

fn state_json(sim: &Simulation) -> String {
    let mut out = String::with_capacity(2048);
    out.push_str("{\"elapsed_yr\":");
    push_json_number(&mut out, sim.state.elapsed_yr);
    out.push_str(",\"total_yr\":");
    push_json_number(&mut out, sim.params.time.total_yr);
    out.push_str(",\"iteration\":");
    out.push_str(&sim.state.iteration.to_string());
    out.push_str(",\"step_yr\":");
    push_json_number(&mut out, sim.state.step_yr);
    out.push_str(",\"progress\":");
    push_json_number(&mut out, sim.progress());
    out.push_str(",\"sea_level_m\":");
    push_json_number(&mut out, sim.sea_level());
    out.push_str(",\"finished\":");
    out.push_str(if sim.state.finished { "true" } else { "false" });
    out.push_str(",\"grid\":{\"width\":");
    out.push_str(&sim.params.world.width.to_string());
    out.push_str(",\"height\":");
    out.push_str(&sim.params.world.height.to_string());
    out.push_str(",\"cell_km\":");
    push_json_number(&mut out, sim.params.world.cell_km);
    out.push_str(",\"extent_km\":");
    push_json_number(&mut out, sim.params.world.extent_x_km());
    out.push('}');
    out.push_str(",\"ledger\":{\"erosion_m3\":");
    push_json_number(&mut out, sim.state.ledger.erosion_m3);
    out.push_str(",\"deposition_m3\":");
    push_json_number(&mut out, sim.state.ledger.deposition_m3);
    out.push_str(",\"export_m3\":");
    push_json_number(&mut out, sim.state.ledger.export_m3);
    out.push_str(",\"storage_m3\":");
    push_json_number(&mut out, sim.state.ledger.storage_m3);
    out.push_str(",\"closure_error_m3\":");
    push_json_number(&mut out, sim.state.ledger.closure_error_m3());
    out.push('}');
    out.push_str(",\"history\":{\"count\":");
    out.push_str(&sim.history.len().to_string());
    out.push_str(",\"memory_bytes\":");
    out.push_str(&sim.history.memory_bytes().to_string());
    out.push_str(",\"frames\":[");
    for (i, frame) in sim.history.frames.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"time\":");
        push_json_number(&mut out, frame.elapsed_yr);
        out.push_str(",\"sea\":");
        push_json_number(&mut out, frame.sea_level_m);
        out.push_str(",\"min\":");
        push_json_number(&mut out, frame.summary.min_m);
        out.push_str(",\"max\":");
        push_json_number(&mut out, frame.summary.max_m);
        out.push_str(",\"mean\":");
        push_json_number(&mut out, frame.summary.mean_m);
        out.push_str(",\"land\":");
        push_json_number(&mut out, frame.summary.land_fraction);
        out.push('}');
    }
    out.push_str("]}");
    out.push('}');
    out
}

/// Terrain statistics as JSON (see `stats::to_json`).
#[no_mangle]
pub extern "C" fn gg_stats_ptr(out_len: *mut usize) -> *mut u8 {
    let Ok(slot) = SIMULATION.lock() else {
        return string_buffer("{}".into(), out_len);
    };
    let Some(sim) = slot.as_ref() else {
        return string_buffer("{}".into(), out_len);
    };
    string_buffer(stats::to_json(&sim.stats()), out_len)
}

/// Copies a display field as `f32` (row-major). Kind ids match
/// [`FieldKind`]; id `8` is slope in degrees, computed on demand.
#[no_mangle]
pub extern "C" fn gg_field_ptr(kind: u8, out_len: *mut usize) -> *mut u8 {
    let Ok(mut slot) = SIMULATION.lock() else {
        return std::ptr::null_mut();
    };
    let Some(sim) = slot.as_mut() else {
        return std::ptr::null_mut();
    };
    // Mineral fields are derived lazily because the prospectivity model is
    // much more expensive than the state fields.
    if kind >= 17 {
        sim.ensure_minerals();
    }
    let values: Vec<f32> = if kind == 8 {
        sim.slope_deg().as_slice().to_vec()
    } else {
        let Some(field_kind) = FieldKind::from_u8(kind) else {
            set_error(format!("unknown field kind {kind}"));
            return std::ptr::null_mut();
        };
        match sim.field(field_kind) {
            Some(view) => view.to_f32(),
            None => {
                set_error(format!("field {kind} is not available"));
                return std::ptr::null_mut();
            }
        }
    };
    f32_buffer(values, out_len)
}

/// JSON report with the mineral catalogue, deposit summary and inventory for
/// the current world (lazily computed; may take ~tens of ms at 384²).
#[no_mangle]
pub extern "C" fn gg_minerals_json_ptr(out_len: *mut usize) -> *mut u8 {
    let Ok(mut slot) = SIMULATION.lock() else {
        set_error("simulation lock poisoned");
        return into_buffer(Vec::new(), out_len);
    };
    let Some(sim) = slot.as_mut() else {
        set_error("no simulation");
        return into_buffer(Vec::new(), out_len);
    };
    match sim.minerals_report() {
        Some(json) => string_buffer(json, out_len),
        None => {
            set_error("minerals are not available yet");
            into_buffer(Vec::new(), out_len)
        }
    }
}

/// Copies a timeline frame's surface as `f32`.
#[no_mangle]
pub extern "C" fn gg_history_frame_ptr(index: u32, out_len: *mut usize) -> *mut u8 {
    let Ok(slot) = SIMULATION.lock() else {
        return std::ptr::null_mut();
    };
    let Some(sim) = slot.as_ref() else {
        return std::ptr::null_mut();
    };
    let Some(frame) = sim.history.frame(index as usize) else {
        set_error(format!("history frame {index} does not exist"));
        return std::ptr::null_mut();
    };
    f32_buffer(frame.elevation_m.clone(), out_len)
}

/// Cross-section as JSON: distance/elevation/water/rock/biome samples.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn gg_profile_ptr(
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    samples: u32,
    out_len: *mut usize,
) -> *mut u8 {
    let Ok(slot) = SIMULATION.lock() else {
        return string_buffer("{}".into(), out_len);
    };
    let Some(sim) = slot.as_ref() else {
        return string_buffer("{}".into(), out_len);
    };
    let climate = match sim.climate() {
        Some(c) => c,
        None => return string_buffer("{\"samples\":[]}".into(), out_len),
    };
    let n = samples.clamp(2, 4096) as usize;
    let samples = stats::profile(&sim.world, climate, &sim.geology, x0, y0, x1, y1, n);
    let mut out = String::with_capacity(samples.len() * 48);
    out.push_str("{\"samples\":[");
    for (i, sample) in samples.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        push_json_number(&mut out, sample.distance_m);
        out.push(',');
        push_json_number(&mut out, sample.elevation_m);
        out.push(',');
        push_json_number(&mut out, sample.water_depth_m);
        out.push(',');
        out.push_str(&sample.rock.to_string());
        out.push(',');
        out.push_str(&sample.biome.to_string());
        out.push(']');
    }
    out.push_str("],\"sampled\":");
    out.push_str(&samples.len().to_string());
    out.push('}');
    string_buffer(out, out_len)
}

/// Exports the current terrain as an OBJ mesh (text).
#[no_mangle]
pub extern "C" fn gg_obj_ptr(out_len: *mut usize) -> *mut u8 {
    let Ok(slot) = SIMULATION.lock() else {
        return std::ptr::null_mut();
    };
    let Some(sim) = slot.as_ref() else {
        return std::ptr::null_mut();
    };
    let text = export::obj_string(&sim.world, &sim.params, 512);
    string_buffer(text, out_len)
}

/// Exports the current terrain as an XYZ point cloud (text).
#[no_mangle]
pub extern "C" fn gg_xyz_ptr(step: u32, out_len: *mut usize) -> *mut u8 {
    let Ok(slot) = SIMULATION.lock() else {
        return std::ptr::null_mut();
    };
    let Some(sim) = slot.as_ref() else {
        return std::ptr::null_mut();
    };
    let text = export::xyz_string(&sim.world, &sim.params, step.max(1) as usize);
    string_buffer(text, out_len)
}

/// Biome catalogue as JSON (labels, colours, ordering).
#[no_mangle]
pub extern "C" fn gg_biomes_ptr(out_len: *mut usize) -> *mut u8 {
    let mut out = String::with_capacity(2048);
    out.push_str("{\"biomes\":[");
    out.push_str(
        "{\"id\":0,\"key\":\"ocean\",\"label\":\"Ocean\",\"label_ja\":\"海洋\",\"colour\":",
    );
    push_colour(&mut out, Biome::Ocean.colour());
    out.push('}');
    for biome in Biome::LAND {
        out.push(',');
        out.push_str("{\"id\":");
        out.push_str(&(biome as u8).to_string());
        out.push_str(",\"key\":\"");
        push_json_escaped(&mut out, biome.key());
        out.push_str("\",\"label\":\"");
        push_json_escaped(&mut out, biome.label());
        out.push_str("\",\"label_ja\":\"");
        push_json_escaped(&mut out, biome.label_ja());
        out.push_str("\",\"colour\":");
        push_colour(&mut out, biome.colour());
        out.push('}');
    }
    out.push_str("]}");
    string_buffer(out, out_len)
}

fn push_colour(out: &mut String, colour: [u8; 3]) {
    out.push_str(&format!("[{},{},{}]", colour[0], colour[1], colour[2]));
}

/// Rock catalogue as JSON (used by the geological overlay legend).
#[no_mangle]
pub extern "C" fn gg_rocks_ptr(out_len: *mut usize) -> *mut u8 {
    let mut out = String::with_capacity(4096);
    out.push_str("{\"rocks\":[");
    for (i, rock) in geogenesis_core::geology::ROCKS.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"id\":");
        out.push_str(&i.to_string());
        out.push_str(",\"key\":\"");
        push_json_escaped(&mut out, rock.key);
        out.push_str("\",\"label\":\"");
        push_json_escaped(&mut out, rock.name);
        out.push_str("\",\"label_ja\":\"");
        push_json_escaped(&mut out, rock.name_ja);
        out.push_str("\",\"colour\":");
        push_colour(&mut out, rock.colour);
        out.push('}');
    }
    out.push_str("]}");
    string_buffer(out, out_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Vec<f64> {
        Params::default().values()
    }

    #[test]
    fn create_step_and_read_state() {
        let values = defaults();
        let code = gg_create(values.as_ptr(), values.len());
        assert_eq!(code, 0, "create failed: {}", LAST_ERROR.lock().unwrap());
        assert_eq!(gg_step(2), 2);
        let mut len = 0usize;
        let ptr = gg_state_ptr(&mut len);
        assert!(!ptr.is_null() && len > 0);
        // Safety: the buffer is engine-owned and valid for `len` bytes.
        let json = unsafe { std::slice::from_raw_parts(ptr, len).to_vec() };
        gg_free(ptr, len);
        let text = String::from_utf8(json).unwrap();
        assert!(text.contains("\"iteration\":2"), "{text}");
        assert!(text.contains("\"history\""));
    }

    #[test]
    fn preview_returns_low_resolution_world() {
        let params = Params::default();
        let values = params.values();
        let code = gg_preview(values.as_ptr(), values.len(), 96);
        assert_eq!(code, 0);
        let mut len = 0usize;
        let info_ptr = gg_preview_info_ptr(&mut len);
        assert!(len > 0);
        let info = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(info_ptr, len) })
            .into_owned();
        gg_free(info_ptr, len);
        assert!(info.contains("\"width\":96"), "{info}");
        let mut bytes = 0usize;
        let elevation_ptr = gg_preview_ptr(&mut bytes);
        assert_eq!(bytes, 96 * 96 * 4);
        gg_free(elevation_ptr, bytes);
    }

    #[test]
    fn field_and_stats_buffers_have_expected_sizes() {
        let p = Params::default();
        let values = p.values();
        assert_eq!(gg_create(values.as_ptr(), values.len()), 0);
        let cells = p.world.width as usize * p.world.height as usize;
        let mut len = 0usize;
        let ptr = gg_field_ptr(0, &mut len);
        assert_eq!(len, cells * 4);
        gg_free(ptr, len);
        let stats_ptr = gg_stats_ptr(&mut len);
        assert!(len > 100);
        gg_free(stats_ptr, len);
    }

    #[test]
    fn seek_and_history_frames_work() {
        let values = {
            let mut p = Params::default();
            p.world.width = 32;
            p.world.height = 32;
            p.time.total_yr = 40_000.0;
            p.time.step_yr = 10_000.0;
            p.world.seed = 7;
            p.normalize();
            p.values()
        };
        assert_eq!(gg_create(values.as_ptr(), values.len()), 0);
        gg_step(2);
        let mut len = 0usize;
        let ptr = gg_history_frame_ptr(0, &mut len);
        assert!(!ptr.is_null());
        assert_eq!(len, 32 * 32 * 4);
        gg_free(ptr, len);
        assert!(gg_seek(10_000.0) >= 0);
    }

    #[test]
    fn invalid_parameter_arrays_are_rejected() {
        assert_ne!(gg_create(std::ptr::null(), 0), 0);
        let short = [0.0f64; 3];
        assert_ne!(gg_create(short.as_ptr(), short.len()), 0);
        let mut len = 0usize;
        let ptr = gg_last_error_ptr(&mut len);
        assert!(!ptr.is_null() && len > 0);
        gg_free(ptr, len);
    }
}
