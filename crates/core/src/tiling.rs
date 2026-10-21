//! Production tiled (streaming) large-grid generation for 4096²-scale worlds.
//!
//! Single-shot [`crate::pipeline::generate_initial`] needs about fifteen
//! full-resolution fields at once: 3.1 GB and 37.5 s at 4096². SPEC §13 asks
//! for **< 30 s and < 1 GB**, so this module generates the same initial relief
//! from a small coarse anchor plus independent per-tile passes, streaming the
//! result to statistics, a coarse preview map and raster files. No
//! full-resolution [`World`](crate::world::World) is ever allocated.
//!
//! # What is evaluated where
//!
//! * **Anchor pass (global, ~1/8 resolution).** The plate mosaic is built on a
//!   coarse grid ([`PlateMosaic::build`]) with the same seed, so its Voronoi
//!   geometry, boundary rates, hotspot trails and plate types are the same
//!   physical fields as the single-shot mosaic, just sampled coarsely. From it
//!   the anchor keeps the quantities that are physically smooth: boundary
//!   distance, relative convergence, plate continuity, orogen/trench rates
//!   (blurred to the fine-scale smoothing width, expressed in coarse cells so
//!   the physical width is preserved), hotspot uplift and the datum-shift
//!   safety correction. The fractal *craton noise* is sampled on the anchor
//!   lattice only to estimate its global rank CDF and median; the actual mask
//!   is evaluated at full resolution per tile.
//! * **Tile pass (local, streaming).** Each tile evaluates the real pipeline
//!   formula on a core region with a halo:
//!   craton noise at global fine coordinates → rank CDF → plate bias →
//!   Gaussian blur → smoothstep threshold → crust thickness → Gaussian blur →
//!   Airy isostasy (with the water-load ocean branch) → fractal relief with the
//!   same integer-frequency, periodically tiled noise as single-shot (median
//!   removed) → folds → boundary faults → orogen/trench integration with the
//!   fine-resolution `collide` factor and caps → hotspot load → landform stamps
//!   → envelope clamp and the datum-shift safety net.
//!
//! The halo exists because two of those steps are local convolutions (craton
//! rank σ = 2.5 cells, crust σ = 3.5 cells) and because the streamed hillshade
//! needs one-cell gradients. Every halo cell is evaluated from *global*
//! coordinates, so adjacent tiles compute identical values for shared cells and
//! seams are exact, not merely close (away from the outer halo margin where a
//! padding boundary rule may bite; the halo is sized so the core is unaffected).
//!
//! # Determinism and parallelism
//!
//! Tiles only read immutable anchor data and their own buffers, so they map
//! directly onto a worker pool (`std::thread::scope`, one tile per worker per
//! wave). Statistics, preview accumulation and streamed export are folded by
//! the driver in plan order after each wave, which makes the result independent
//! of scheduling *and* of the worker count: every elevation is a pure function
//! of the seed, parameters and global cell coordinates, and the accumulators
//! see tiles in the same order regardless of how many threads produced them.
//!
//! # Limits
//!
//! * Rates that the coarse anchor under-samples (narrow trenches, ridge crests
//!   and arcs) are smoother than in single-shot when the physical extent is
//!   large compared with the feature widths. Features wide compared with two
//!   anchor cells are reproduced faithfully.
//! * The craton rank CDF and the relief median come from anchor samples, so a
//!   tiled world differs from single-shot at the margins of the continent
//!   threshold (documented tolerances in the tests).
//! * Landform stamps are placed from the same RNG stream as single-shot, but
//!   the boundary/continent classification they use is the anchor approximation,
//!   so the exact stamp set can differ slightly.
//! * The generated world is a *generation* output: the simulation time loop
//!   still runs on single-shot grids.

use crate::error::{GeoError, GeoResult};
use crate::export::tiled::{self, ExportKinds, ExportRequest, ExportSummary, TiledExportWriter};
use crate::grid::{Field, FieldScalar, MIN_GRID_DIM};
use crate::math::noise::{FractalKind, NoiseField, NoiseKind, NoiseParams};
use crate::math::rng::{derive_seed, hash2, unit_from_hash, Rng};
use crate::math::{clamp, smoothstep};
use crate::params::Params;
use crate::pipeline::{tile_plan, TileRect};
use crate::plates::{BoundaryType, PlateMosaic};
use crate::spatial;
use crate::stats::{ElevationStats, TileAccumulator};
use std::path::Path;
use std::time::Instant;

/// Domain tag of the initial-relief RNG stream. Must match
/// `pipeline::DOMAIN_RELIEF` so folds and stamps are drawn from the same stream
/// as single-shot.
const RELIEF_DOMAIN: u64 = 0x7265_6C69_6566_0001;
/// Reference oceanic crustal thickness, metres (matches `pipeline`).
const OCEANIC_CRUST_M: f64 = 7000.0;
/// Density of seawater, kg/m³ (matches the Airy water-load correction).
const WATER_DENSITY: f64 = 1030.0;
/// Base cycles of the craton noise across the world (matches `pipeline`).
const CRATON_CYCLES: f64 = 3.5;
/// Bins in the craton-noise rank CDF table.
const CDF_BINS: usize = 4096;
/// Bins in the streamed elevation histogram.
const HISTOGRAM_BINS: usize = 512;
/// Smallest halo that keeps every core cell exact. The two chained blurs have
/// radii 3·2.5 = 8 (craton rank) and 3·3.5 = 11 (crust) cells, so a core cell
/// depends on raw values up to 19 cells away; one cell of slack.
const MIN_HALO: u32 = 20;
/// Memory the worker pool may use for tile buffers; larger machines get more
/// workers, but the pool is capped so peak RSS stays well below the 1 GB goal.
const TILE_MEMORY_BUDGET_BYTES: u64 = 512 << 20;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Options controlling a tiled large-world generation pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LargeWorldOptions {
    /// Core tile edge, cells (the edge tiles may be smaller). Default 512.
    pub tile_size: u32,
    /// Halo cells generated on every side of a tile. Default 32; values below
    /// [`MIN_HALO`] are raised by [`Self::normalized`] because the local
    /// smoothing would otherwise not match across seams.
    pub halo: u32,
    /// Preview downsample factor (fine cells per preview cell). Default 8.
    pub preview_factor: u32,
    /// Worker threads; `0` picks `available_parallelism` capped by the tile
    /// memory budget. The result is independent of this value.
    pub workers: u32,
    /// Optional streamed raster export written during the same tile pass.
    pub export: Option<ExportRequest>,
}

impl Default for LargeWorldOptions {
    fn default() -> Self {
        Self {
            tile_size: 512,
            halo: 32,
            preview_factor: 8,
            workers: 0,
            export: None,
        }
    }
}

impl LargeWorldOptions {
    /// Clamps the options into the range the generator supports.
    #[must_use]
    pub fn normalized(&self) -> Self {
        Self {
            tile_size: self.tile_size.clamp(64, 1024),
            halo: self.halo.clamp(MIN_HALO, 256),
            preview_factor: self.preview_factor.clamp(2, 64),
            workers: self.workers,
            export: self.export.clone(),
        }
    }
}

/// Wall-clock breakdown of a tiled generation pass, seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LargeWorldTiming {
    /// Coarse anchor pass (plate mosaic, CDFs, averaged rate fields).
    pub anchor_s: f64,
    /// Per-tile evaluation, halo smoothing and statistics/preview folding.
    pub tiles_s: f64,
    /// Time spent writing streamed rasters (inline during the tile pass plus
    /// closing the files afterwards).
    pub export_s: f64,
    /// Whole call, including validation and the anchor pass.
    pub total_s: f64,
}

/// Result of a tiled large-world generation.
///
/// The full-resolution elevation is *not* kept: it is streamed into
/// [`Self::stats`], the coarse [`Self::preview`] map and, when requested, the
/// raster files. This struct is small (the preview map is `width/factor` cells
/// on a side) and can be cloned cheaply enough for the WASM boundary.
#[derive(Clone, Debug)]
pub struct LargeWorldResult {
    /// Fine grid width, cells.
    pub width: u32,
    /// Fine grid height, cells.
    pub height: u32,
    /// Sea-level datum, metres.
    pub sea_level_m: f64,
    /// Fine cell edge, km.
    pub cell_km: f64,
    /// Streamed global elevation statistics (min/max/mean/σ/land count).
    pub stats: TileAccumulator,
    /// Elevation histogram over `[histogram_min_m, histogram_max_m]`.
    pub histogram: Vec<u32>,
    /// Lower histogram bound, metres.
    pub histogram_min_m: f64,
    /// Upper histogram bound, metres.
    pub histogram_max_m: f64,
    /// Summed elevations per preview cell (divide by `preview_counts`).
    pub preview: Vec<f64>,
    /// Fine cells folded into each preview cell.
    pub preview_counts: Vec<u32>,
    /// Preview width, cells.
    pub preview_width: u32,
    /// Preview height, cells.
    pub preview_height: u32,
    /// Timing breakdown.
    pub timing: LargeWorldTiming,
    /// Effective options (normalised).
    pub options: LargeWorldOptions,
    /// Validated, normalised parameters the world was generated from; kept so
    /// [`Self::write_exports`] can stream exports without a second call site.
    pub params: Params,
    /// Files written by the inline export pass, when one was requested.
    pub exports: Option<ExportSummary>,
}

impl LargeWorldResult {
    /// Mean elevation of every preview cell, metres (sea level for empty cells).
    #[must_use]
    pub fn preview_means(&self) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.preview.len());
        for (index, sum) in self.preview.iter().enumerate() {
            let count = self.preview_counts.get(index).copied().unwrap_or(0);
            if count == 0 {
                out.push(self.sea_level_m);
            } else {
                out.push(sum / f64::from(count));
            }
        }
        out
    }

    /// Hypsometric RGB rendering of the coarse preview (same palette as the CLI
    /// preview), ready for `encode_png_rgb`.
    #[must_use]
    pub fn preview_rgb(&self) -> Vec<u8> {
        tiled::preview_rgb(
            &self.preview,
            &self.preview_counts,
            self.preview_width,
            self.preview_height,
            self.sea_level_m,
        )
    }

    /// Land fraction of the generated world.
    #[must_use]
    pub fn land_fraction(&self) -> f64 {
        self.stats.land_fraction()
    }

    /// Elevation statistics in the same shape as the single-shot report.
    ///
    /// The streaming pass keeps min/max/mean/σ and land cells exactly; the
    /// quantiles (median, p10, p90) are interpolated from the 512-bin
    /// [`Self::histogram`], so they are accurate to one bin width (a few metres
    /// for the default envelope).
    #[must_use]
    pub fn elevation_stats(&self) -> ElevationStats {
        let cells = self.stats.count as usize;
        if cells == 0 {
            return ElevationStats {
                cells,
                ..ElevationStats::default()
            };
        }
        ElevationStats {
            min_m: self.stats.min_m,
            max_m: self.stats.max_m,
            mean_m: self.stats.mean_m(),
            std_m: self.stats.std_dev_m(),
            median_m: self.histogram_quantile(0.50),
            p10_m: self.histogram_quantile(0.10),
            p90_m: self.histogram_quantile(0.90),
            cells,
        }
    }

    /// Approximate quantile from the streamed histogram.
    #[must_use]
    pub fn histogram_quantile(&self, q: f64) -> f64 {
        let total: u64 = self.histogram.iter().map(|&n| u64::from(n)).sum();
        if total == 0 {
            return self.sea_level_m;
        }
        let target = (q.clamp(0.0, 1.0) * total as f64).round() as u64;
        let span = (self.histogram_max_m - self.histogram_min_m).max(1e-9);
        let bin_width = span / self.histogram.len().max(1) as f64;
        let mut seen = 0u64;
        for (index, &count) in self.histogram.iter().enumerate() {
            let count = u64::from(count);
            if seen + count >= target {
                let inside = if count == 0 {
                    0.0
                } else {
                    target.saturating_sub(seen) as f64 / count as f64
                };
                return self.histogram_min_m + (index as f64 + inside) * bin_width;
            }
            seen += count;
        }
        self.histogram_max_m
    }

    /// Streams every raster export for this world to `dir`.
    ///
    /// This re-runs the tile pass (still bounded memory, still deterministic):
    /// the result does not retain the 16.7M elevations, and regenerating them is
    /// cheap compared with the IO. The pass uses the parameters and options the
    /// world was generated with, plus `kinds` for the format selection.
    pub fn write_exports(
        &self,
        dir: impl AsRef<Path>,
        kinds: ExportKinds,
    ) -> GeoResult<ExportSummary> {
        let mut options = self.options.clone();
        options.export = Some(ExportRequest::with_kinds(dir.as_ref(), kinds));
        let result = generate_large_world_with(&self.params, self.width, self.height, &options)?;
        Ok(result.exports.unwrap_or_default())
    }
}

/// Generates a large world with the default tiling options.
///
/// See [`generate_large_world_with`] for the full control surface.
pub fn generate_large_world(
    params: &Params,
    width: u32,
    height: u32,
    tile_size: u32,
    halo: u32,
    preview_factor: u32,
    workers: u32,
) -> GeoResult<LargeWorldResult> {
    let options = LargeWorldOptions {
        tile_size,
        halo,
        preview_factor,
        workers,
        export: None,
    };
    generate_large_world_with(params, width, height, &options)
}

/// Generates a large world, streaming elevations through the tile pass.
///
/// `width`/`height` set the fine grid resolution; the physical extent always
/// comes from `params.world` (so requesting 4096² for a 384² @ 10 km parameter
/// set keeps the 3840 km × 3840 km world and shrinks the cell to 0.94 km).
/// Every elevation is a pure function of the seed, parameters and global cell
/// coordinates, so repeated calls and different worker counts give the same
/// world.
pub fn generate_large_world_with(
    params: &Params,
    width: u32,
    height: u32,
    options: &LargeWorldOptions,
) -> GeoResult<LargeWorldResult> {
    let total_start = Instant::now();
    let mut p = params.clone();
    // Tiled generation is the memory-safe route for grids above the single-shot
    // 2048² guard; that guard exists to stop a full-resolution `World` from being
    // allocated, which never happens here. Everything else is still validated.
    p.allow_large_grid = true;
    p.validate()?;
    p.normalize();
    p.validate()?;
    let options = options.normalized();
    let geom = FineGeometry::new(&p, width, height)?;

    let anchor_start = Instant::now();
    let anchor = AnchorWorld::build(&p, &geom)?;
    let anchor_s = anchor_start.elapsed().as_secs_f64();

    let plan = tile_plan(width, height, options.tile_size, options.halo);
    if plan.is_empty() {
        return Err(GeoError::InvalidGrid(
            "tiled generation plan is empty".to_string(),
        ));
    }
    let generator = TileGenerator::new(&geom, &anchor, options.halo);
    let workers = resolve_workers(options.workers, plan.len(), worker_estimate_bytes(&options));
    let preview_factor = options.preview_factor;
    let preview_width = width.div_ceil(preview_factor);
    let preview_height = height.div_ceil(preview_factor);
    let preview_len = preview_width as usize * preview_height as usize;

    let mut stats = TileAccumulator::new();
    let mut preview = vec![0.0f64; preview_len];
    let mut preview_counts = vec![0u32; preview_len];
    let hist_min = geom.limit_low;
    let hist_max = geom.limit_high;
    let mut histogram = vec![0u32; HISTOGRAM_BINS];
    let mut core = vec![0.0f64; options.tile_size as usize * options.tile_size as usize];

    let mut writer = match &options.export {
        Some(request) => Some(TiledExportWriter::create(
            &request.dir,
            width,
            height,
            geom.sea,
            geom.cell_m_x,
            geom.cell_m_y,
            geom.wraps_x,
            geom.wraps_y,
            request.kinds,
        )?),
        None => None,
    };
    let mut export_s = 0.0;

    let tile_start = Instant::now();
    for wave in plan.chunks(workers) {
        // ---- Evaluate this wave of tiles concurrently, one per worker.
        let buffers = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(wave.len());
            for tile in wave {
                let generator = &generator;
                handles.push(scope.spawn(move || generator.generate(*tile)));
            }
            let mut results = Vec::with_capacity(handles.len());
            for handle in handles {
                let buffer = handle
                    .join()
                    .map_err(|_| GeoError::Numerical("tiled worker panicked".to_string()))?;
                results.push(buffer);
            }
            Ok::<_, GeoError>(results)
        })?;

        // ---- Fold the wave into statistics, preview and exports in plan order.
        for (tile, buffer) in wave.iter().zip(buffers) {
            let tile_w = tile.width as usize;
            let tile_h = tile.height as usize;
            let halo = options.halo as usize;
            for ty in 0..tile_h {
                let start = (ty + halo) * buffer.stride + halo;
                let row = &buffer.data[start..start + tile_w];
                core[ty * tile_w..(ty + 1) * tile_w].copy_from_slice(row);
            }
            let core = &core[..tile_w * tile_h];
            stats.add_tile(core, geom.sea);
            crate::pipeline::accumulate_tile_preview(
                preview_factor,
                preview_width,
                tile,
                core,
                &mut preview,
                &mut preview_counts,
            );
            fold_histogram(core, hist_min, hist_max, &mut histogram);
            if let Some(writer) = writer.as_mut() {
                let export_start = Instant::now();
                writer.write_tile(
                    tile.x,
                    tile.y,
                    tile.width,
                    tile.height,
                    &buffer.data,
                    buffer.stride,
                    options.halo,
                )?;
                export_s += export_start.elapsed().as_secs_f64();
            }
        }
    }
    let tiles_s = tile_start.elapsed().as_secs_f64() - export_s;

    // ---- Close the streamed outputs.
    let exports = if let (Some(writer), Some(request)) = (writer, &options.export) {
        let export_start = Instant::now();
        let mut summary = writer.finish(stats.min_m, stats.max_m)?;
        let preview_bytes = tiled::write_preview_png(
            &request.dir,
            &preview,
            &preview_counts,
            preview_width,
            preview_height,
            geom.sea,
        )?;
        summary.files.push("preview.png".to_string());
        summary.bytes += preview_bytes;
        export_s += export_start.elapsed().as_secs_f64();
        Some(summary)
    } else {
        None
    };
    let total_s = total_start.elapsed().as_secs_f64();
    let timing = LargeWorldTiming {
        anchor_s,
        tiles_s,
        export_s,
        total_s,
    };

    Ok(LargeWorldResult {
        width,
        height,
        sea_level_m: geom.sea,
        cell_km: geom.cell_km,
        stats,
        histogram,
        histogram_min_m: hist_min,
        histogram_max_m: hist_max,
        preview,
        preview_counts,
        preview_width,
        preview_height,
        timing,
        options,
        params: p,
        exports,
    })
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Fine-grid geometry derived from the parameter set and the requested
/// resolution. Kept separate from [`crate::grid::Grid`] because tiled
/// generation never allocates one for the full-resolution world.
#[derive(Clone, Copy, Debug)]
struct FineGeometry {
    width: u32,
    height: u32,
    cell_m_x: f64,
    cell_m_y: f64,
    cell_km: f64,
    sea: f64,
    vertical: f64,
    limit_low: f64,
    limit_high: f64,
    wraps_x: bool,
    wraps_y: bool,
    seed: u64,
}

impl FineGeometry {
    fn new(params: &Params, width: u32, height: u32) -> GeoResult<Self> {
        if width < MIN_GRID_DIM || height < MIN_GRID_DIM {
            return Err(GeoError::InvalidGrid(format!(
                "tiled grid {width}x{height} is below the minimum of {MIN_GRID_DIM} cells per side"
            )));
        }
        if width > crate::grid::MAX_GRID_DIM || height > crate::grid::MAX_GRID_DIM {
            return Err(GeoError::InvalidGrid(format!(
                "tiled grid {width}x{height} exceeds the maximum of {} cells per side",
                crate::grid::MAX_GRID_DIM
            )));
        }
        let extent_x_m = params.world.extent_x_km() * 1000.0;
        let extent_y_m = params.world.extent_y_km() * 1000.0;
        if !(extent_x_m.is_finite() && extent_y_m.is_finite()) || extent_x_m <= 0.0 {
            return Err(GeoError::InvalidGrid(
                "world extent must be positive and finite".to_string(),
            ));
        }
        let cell_m_x = extent_x_m / f64::from(width);
        let cell_m_y = extent_y_m / f64::from(height);
        Ok(Self {
            width,
            height,
            cell_m_x,
            cell_m_y,
            cell_km: cell_m_x / 1000.0,
            sea: params.world.sea_level_m,
            vertical: params.world.vertical_m.max(100.0),
            limit_low: -params.world.vertical_m * 1.8,
            limit_high: params.world.vertical_m * 1.2,
            wraps_x: params.world.boundary.wraps_x(),
            wraps_y: params.world.boundary.wraps_y(),
            seed: params.world.effective_seed(),
        })
    }

    /// Global cell index on one axis: wraps on periodic axes, clamps on open
    /// ones (matching the single-shot grid's boundary handling).
    #[inline]
    fn map_x(&self, unwrapped: isize) -> u32 {
        if self.wraps_x {
            unwrapped.rem_euclid(self.width as isize) as u32
        } else {
            unwrapped.clamp(0, self.width as isize - 1) as u32
        }
    }

    /// Global cell index on one axis: wraps on periodic axes, clamps on open
    /// ones.
    #[inline]
    fn map_y(&self, unwrapped: isize) -> u32 {
        if self.wraps_y {
            unwrapped.rem_euclid(self.height as isize) as u32
        } else {
            unwrapped.clamp(0, self.height as isize - 1) as u32
        }
    }
}

// ---------------------------------------------------------------------------
// Anchor pass
// ---------------------------------------------------------------------------

/// Everything the per-tile pass needs from the global, low-resolution pass.
#[derive(Clone, Debug)]
struct AnchorWorld {
    anchor_width: u32,
    anchor_height: u32,
    // Craton noise and its global rank CDF.
    craton_noise: NoiseField,
    craton_cycles_x: f64,
    craton_cycles_y: f64,
    craton_periods: (u32, u32),
    craton_scale: (f64, f64),
    craton_min: f64,
    craton_max: f64,
    rank_cdf: Vec<f64>,
    // Base relief noise and its global median.
    relief_noise: NoiseField,
    relief_cycles_x: f64,
    relief_cycles_y: f64,
    relief_periods: (u32, u32),
    relief_scale: (f64, f64),
    relief_median: f64,
    // Smooth globally-averaged fields, stored as f32.
    plate_continental: Field<f32>,
    boundary_distance_m: Field<f32>,
    convergence_m_yr: Field<f32>,
    orogen_rate_m_yr: Field<f32>,
    trench_rate_m_yr: Field<f32>,
    hotspot_m: Field<f32>,
    // Global scalar controls.
    threshold: f64,
    continental_crust_m: f64,
    reference_thickness_m: f64,
    cont_factor: f64,
    ocean_factor: f64,
    datum_shift_m: f64,
    orogen_span_yr: f64,
    orogen_cap_m: f64,
    influence_m: f64,
    mean_speed_m_yr: f64,
    convergence_threshold: f64,
    // Fold / fault parameters (global constants plus a seeded phase).
    fold_amplitude_m: f64,
    fold_wavelength_m: f64,
    fold_sin_cos: (f64, f64),
    fold_phase: f64,
    fold_weight: f64,
    fold_belt_scale_m: f64,
    fault_offset_m: f64,
    fault_falloff_m: f64,
    // Landform stamps placed from the fine-resolution RNG stream.
    stamps: Vec<StampFeature>,
}

impl AnchorWorld {
    fn build(params: &Params, geom: &FineGeometry) -> GeoResult<Self> {
        let seed = geom.seed;
        let tectonic = &params.tectonics;

        // ---- Coarse plate mosaic at ~1/8 of the fine resolution.
        let anchor_width = (geom.width / 8).clamp(64, 1024).min(geom.width.max(1));
        let anchor_height = (((f64::from(anchor_width) * f64::from(geom.height))
            / f64::from(geom.width))
        .round() as u32)
            .max(MIN_GRID_DIM);
        // Keep the physical extent of the fine world: the anchor cell is
        // `scale_x` times the fine cell.
        let scale_x = f64::from(geom.width) / f64::from(anchor_width);
        let mut anchor_params = params.clone();
        anchor_params.world.width = anchor_width;
        anchor_params.world.height = anchor_height;
        anchor_params.world.cell_km = geom.cell_m_x / 1000.0 * scale_x;
        anchor_params.normalize();
        let anchor_grid = anchor_params.world.grid()?;
        let mosaic = PlateMosaic::build(&anchor_params, anchor_grid, seed);

        // ---- Craton noise: same field as single-shot, sampled on the anchor
        // lattice to estimate its rank CDF.
        let craton_noise = NoiseField::new(
            NoiseParams {
                kind: NoiseKind::Value,
                fractal: FractalKind::Fbm,
                octaves: 5,
                lacunarity: 2.0,
                persistence: 0.55,
                anisotropy: 0.0,
                direction_deg: 0.0,
            },
            derive_seed(seed, 0xC2A7_0F1E),
        );
        let craton_cycles_x = CRATON_CYCLES;
        let craton_cycles_y = CRATON_CYCLES * f64::from(geom.height) / f64::from(geom.width);
        let craton_periods = (
            if geom.wraps_x {
                craton_cycles_x.round().max(1.0) as u32
            } else {
                0
            },
            if geom.wraps_y {
                craton_cycles_y.round().max(1.0) as u32
            } else {
                0
            },
        );
        let craton_scale = axis_scales(0.0, 0.0);
        let craton_field = craton_noise.fill(&anchor_grid, craton_cycles_x);
        let (craton_min, craton_max, rank_cdf) = rank_cdf_table(&craton_field);

        // ---- Base relief noise: same field as single-shot, anchor sample for
        // the median that single-shot subtracts.
        let relief_noise = params.relief.noise_field(derive_seed(seed, RELIEF_DOMAIN));
        let relief_cycles_x = clamp(geom.cell_m_x * f64::from(geom.width) / 800_000.0, 3.0, 9.0);
        let relief_cycles_y = relief_cycles_x * f64::from(geom.height) / f64::from(geom.width);
        let relief_periods = (
            if geom.wraps_x {
                relief_cycles_x.round().max(1.0) as u32
            } else {
                0
            },
            if geom.wraps_y {
                relief_cycles_y.round().max(1.0) as u32
            } else {
                0
            },
        );
        let relief_scale = axis_scales(params.relief.anisotropy, params.relief.anisotropy_deg);
        let relief_field = relief_noise.fill(&anchor_grid, relief_cycles_x);
        let relief_median = relief_field.quantile(0.5);

        // ---- Plate type and boundary fields (smooth, physically wide).
        let plate_continental = Field::<f32>::from_fn(anchor_grid, |x, y| {
            if mosaic.plate_at(x, y).continental {
                1.0
            } else {
                0.0
            }
        });
        let boundary_distance_m = Field::<f32>::from_fn(anchor_grid, |x, y| {
            let value = mosaic.boundary_distance_m.get(x, y);
            if value.is_finite() && value < 1.0e30 {
                value
            } else {
                1.0e30
            }
        });
        let convergence_m_yr = mosaic.convergence_m_yr.clone();

        // ---- Orogen/trench rates: split the mosaic's signed rate, then blur at
        // the fine-resolution smoothing width expressed in anchor cells. The
        // mosaic already applies its own (physical) boundary width, so the
        // physical belt width is preserved.
        let sigma_fine = (tectonic.orogeny_width_km * 1000.0 / geom.cell_m_x * 0.6).max(1.0);
        let sigma_anchor = sigma_fine / scale_x.max(1.0);
        let mut orogen = Field::<f64>::zeros(anchor_grid);
        let mut trench = Field::<f64>::zeros(anchor_grid);
        for i in 0..anchor_grid.cell_count() {
            let rate = f64::from(mosaic.boundary_uplift_m_yr.get_index(i));
            if rate >= 0.0 {
                orogen.set_index(i, rate);
            } else {
                trench.set_index(i, -rate);
            }
        }
        spatial::gaussian_blur(&mut orogen, sigma_anchor);
        spatial::gaussian_blur(&mut trench, sigma_anchor * 0.5);
        let orogen_rate_m_yr = orogen.map_to(|v| v as f32);
        let trench_rate_m_yr = trench.map_to(|v| v as f32);

        // ---- Hotspot load (pipeline integrates the raw rate over 1.5 Myr, then
        // smooths at the hotspot radius).
        let hotspot_sigma_fine =
            (tectonic.hotspot_radius_km * 1000.0 / geom.cell_m_x * 0.35).max(1.0);
        let mut hotspot = mosaic.hotspot_uplift_m_yr.map_to(|v| v * 1.5e6);
        spatial::gaussian_blur(&mut hotspot, hotspot_sigma_fine / scale_x.max(1.0));
        let hotspot_m = hotspot.map_to(|v| v as f32);

        // ---- Isostasy constants (identical to the single-shot pipeline).
        let continental_crust_m = (tectonic.plate_thickness_km * 1000.0).max(20_000.0);
        let rho_m = tectonic.mantle_density_kg_m3;
        let rho_c = tectonic.density_continental_kg_m3;
        let rho_o = tectonic.density_oceanic_kg_m3;
        let cont_factor = 1.0 - rho_c / rho_m;
        let ocean_factor = (rho_m - rho_o) / (rho_m - WATER_DENSITY);
        let reference_thickness_m = continental_crust_m - 400.0 / cont_factor.max(1e-3);
        let continental_fraction = clamp(0.22 + 0.50 * tectonic.continental_fraction, 0.16, 0.72);
        let threshold = 1.0 - continental_fraction;

        // ---- Fold, fault and boundary constants.
        let fold_angle = params.relief.anisotropy_deg.to_radians();
        let fold_sin_cos = fold_angle.sin_cos();
        let fold_phase = Rng::new(derive_seed(seed, RELIEF_DOMAIN ^ 0xF0_1D))
            .range_f64(0.0, std::f64::consts::TAU);
        let fold_amplitude_m = params.relief.fold_amplitude_m;
        let fold_wavelength_m = params.relief.fold_wavelength_km * 1000.0;
        let fold_weight = 0.3 + 0.7 * params.relief.anisotropy;
        let fold_belt_scale_m = (tectonic.orogeny_width_km * 500.0).max(1.0);
        let fault_offset_m = params.relief.fault_offset_m;
        let fault_falloff_m = (tectonic.orogeny_width_km * 1000.0 * 0.5).max(geom.cell_m_x);
        let influence_m = (tectonic.boundary_influence_km() * 1000.0).max(2.0 * geom.cell_m_x);
        let mean_speed_m_yr = tectonic.plate_speed_m_yr().max(1e-4);
        let convergence_threshold = tectonic.convergence_threshold;
        let orogen_span_yr = 1_200_000.0;
        let orogen_cap_m = geom.vertical * 0.30;

        let mut anchor = Self {
            anchor_width,
            anchor_height,
            craton_noise,
            craton_cycles_x,
            craton_cycles_y,
            craton_periods,
            craton_scale,
            craton_min,
            craton_max,
            rank_cdf,
            relief_noise,
            relief_cycles_x,
            relief_cycles_y,
            relief_periods,
            relief_scale,
            relief_median,
            plate_continental,
            boundary_distance_m,
            convergence_m_yr,
            orogen_rate_m_yr,
            trench_rate_m_yr,
            hotspot_m,
            threshold,
            continental_crust_m,
            reference_thickness_m,
            cont_factor,
            ocean_factor,
            datum_shift_m: 0.0,
            orogen_span_yr,
            orogen_cap_m,
            influence_m,
            mean_speed_m_yr,
            convergence_threshold,
            fold_amplitude_m,
            fold_wavelength_m,
            fold_sin_cos,
            fold_phase,
            fold_weight,
            fold_belt_scale_m,
            fault_offset_m,
            fault_falloff_m,
            stamps: Vec::new(),
        };

        // ---- Datum safety correction, estimated globally at anchor resolution.
        let datum_shift = anchor.estimate_datum_shift(geom, &craton_field, &relief_field);
        anchor.datum_shift_m = datum_shift;

        // ---- Landform stamps, placed with the fine-resolution RNG stream.
        anchor.stamps = anchor.build_stamps(params, geom);

        Ok(anchor)
    }

    /// Rank of a craton-noise value in the global distribution (0..1).
    #[inline]
    fn rank_of(&self, value: f64) -> f64 {
        let span = self.craton_max - self.craton_min;
        if !(span > 0.0) {
            return 0.5;
        }
        let t = ((value - self.craton_min) / span).clamp(0.0, 1.0);
        let x = t * CDF_BINS as f64;
        let bin = (x as usize).min(CDF_BINS - 1);
        let frac = x - bin as f64;
        let low = self.rank_cdf[bin];
        let high = self.rank_cdf[bin + 1];
        (low + (high - low) * frac).clamp(0.0, 1.0)
    }

    /// Anchor-grid sample coordinates of a fine cell, as `(x, y)` in anchor
    /// cell units (cell-centre aligned).
    #[inline]
    fn anchor_coords(&self, geom: &FineGeometry, gx: u32, gy: u32) -> (f64, f64) {
        let x = (f64::from(gx) + 0.5) / f64::from(geom.width) * f64::from(self.anchor_width) - 0.5;
        let y =
            (f64::from(gy) + 0.5) / f64::from(geom.height) * f64::from(self.anchor_height) - 0.5;
        (x, y)
    }

    /// Bilinearly samples one anchor field at a fine cell.
    fn sample_at<T: FieldScalar>(
        &self,
        field: &Field<T>,
        geom: &FineGeometry,
        gx: u32,
        gy: u32,
    ) -> f64 {
        let (x, y) = self.anchor_coords(geom, gx, gy);
        let ax = BilinearAxis::build(&[x], self.anchor_width as usize, geom.wraps_x);
        let ay = BilinearAxis::build(&[y], self.anchor_height as usize, geom.wraps_y);
        bilinear_sample(field, &ax, &ay, 0, 0)
    }

    /// Continental crust fraction at a fine cell (rank CDF + plate bias,
    /// thresholded but not blurred).
    fn continent_at(&self, geom: &FineGeometry, gx: u32, gy: u32) -> f64 {
        let u = (f64::from(gx) + 0.5) / f64::from(geom.width)
            * self.craton_cycles_x
            * self.craton_scale.0;
        let v = (f64::from(gy) + 0.5) / f64::from(geom.height)
            * self.craton_cycles_y
            * self.craton_scale.1;
        let value =
            self.craton_noise
                .sample_tiled_xy(u, v, self.craton_periods.0, self.craton_periods.1);
        let rank = self.rank_of(value);
        let plate = self.sample_at(&self.plate_continental, geom, gx, gy);
        let bias = (2.0 * plate - 1.0) * 0.14;
        let rank = (rank + bias).clamp(0.0, 1.0);
        smoothstep(self.threshold - 0.05, self.threshold + 0.05, rank)
    }

    /// Boundary classification at a fine cell, from the smooth anchor fields.
    fn boundary_type_at(&self, geom: &FineGeometry, gx: u32, gy: u32) -> BoundaryType {
        let distance = self.sample_at(&self.boundary_distance_m, geom, gx, gy);
        if distance > self.influence_m {
            return BoundaryType::Interior;
        }
        let convergence = self.sample_at(&self.convergence_m_yr, geom, gx, gy);
        let ratio = convergence.abs() / self.mean_speed_m_yr;
        if ratio > self.convergence_threshold {
            if convergence > 0.0 {
                BoundaryType::Convergent
            } else {
                BoundaryType::Divergent
            }
        } else {
            BoundaryType::Transform
        }
    }

    /// Estimated global datum shift, matching the single-shot pathological-world
    /// guard. Zero for a healthy world.
    fn estimate_datum_shift(
        &self,
        geom: &FineGeometry,
        craton_field: &Field<f64>,
        relief_field: &Field<f64>,
    ) -> f64 {
        let grid = craton_field.grid();
        let count = grid.cell_count();
        let mut estimate = Field::<f64>::zeros(grid);
        let mut land = 0usize;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let i = grid.index(x, y);
                let rank = self.rank_of(craton_field.get_index(i));
                let plate = self.plate_continental.get_index(i);
                let bias = (2.0 * f64::from(plate) - 1.0) * 0.14;
                let rank = (rank + bias).clamp(0.0, 1.0);
                let continent = smoothstep(self.threshold - 0.05, self.threshold + 0.05, rank);
                let crust =
                    OCEANIC_CRUST_M + continent * (self.continental_crust_m - OCEANIC_CRUST_M);
                let factor = self.ocean_factor + (self.cont_factor - self.ocean_factor) * continent;
                let mut z = geom.sea + factor * (crust - self.reference_thickness_m);
                let roughness = 0.05 + 0.13 * continent;
                z += (relief_field.get_index(i) - self.relief_median) * geom.vertical * roughness;
                let collide = 0.1 + 0.9 * continent;
                z +=
                    (f64::from(self.orogen_rate_m_yr.get_index(i)) * self.orogen_span_yr * collide)
                        .min(self.orogen_cap_m);
                z -= (f64::from(self.trench_rate_m_yr.get_index(i)) * self.orogen_span_yr * 0.9)
                    .min(self.orogen_cap_m * 0.6);
                z += f64::from(self.hotspot_m.get_index(i));
                let z = if z.is_finite() {
                    z.clamp(geom.limit_low, geom.limit_high)
                } else {
                    geom.sea
                };
                estimate.set_index(i, z);
                if z > geom.sea {
                    land += 1;
                }
            }
        }
        let land_fraction = land as f64 / count.max(1) as f64;
        if (0.08..=0.80).contains(&land_fraction) {
            return 0.0;
        }
        // Mirror `generate_initial`: shift the datum by the smallest amount that
        // restores a plausible land/ocean mix (target 30% land).
        let threshold_z = estimate.quantile(0.70);
        clamp(geom.sea - threshold_z, -geom.vertical, geom.vertical) * 0.8
    }

    /// Places the landform stamps from the fine-resolution RNG stream.
    fn build_stamps(&self, params: &Params, geom: &FineGeometry) -> Vec<StampFeature> {
        let count = (params.relief.feature_density * 16.0)
            .round()
            .clamp(0.0, 40.0) as usize;
        if count == 0 {
            return Vec::new();
        }
        let mut rng = Rng::new(derive_seed(geom.seed, RELIEF_DOMAIN ^ 0x51A9));
        let mut stamps = Vec::with_capacity(count);
        for _ in 0..count * 5 {
            if stamps.len() >= count {
                break;
            }
            let x = rng.range_f64(0.0, f64::from(geom.width));
            let y = rng.range_f64(0.0, f64::from(geom.height));
            let xi = (x as u32) % geom.width;
            let yi = (y as u32) % geom.height;
            let boundary = self.boundary_type_at(geom, xi, yi);
            // Interior stamps only make sense on land.
            let continental = self.continent_at(geom, xi, yi);
            if boundary == BoundaryType::Interior && continental < 0.4 {
                continue;
            }
            let Some(kind) = pick_landform(boundary, &mut rng) else {
                continue;
            };
            let length_cells = rng.range_f64(8.0, 34.0);
            let width_cells = rng.range_f64(2.5, 7.5);
            let height = match kind {
                Landform::Range => rng.range_f64(0.10, 0.28) * geom.vertical,
                Landform::Trench => -rng.range_f64(0.10, 0.22) * geom.vertical,
                Landform::Rift => -rng.range_f64(0.05, 0.14) * geom.vertical,
                Landform::OceanRidge => rng.range_f64(0.05, 0.14) * geom.vertical,
                Landform::Plateau => rng.range_f64(0.08, 0.22) * geom.vertical,
                Landform::Mesa => rng.range_f64(0.05, 0.14) * geom.vertical,
                Landform::Caldera => -rng.range_f64(0.03, 0.08) * geom.vertical,
                Landform::Peak => rng.range_f64(0.06, 0.18) * geom.vertical,
            };
            let direction_rad = rng.range_f64(0.0, std::f64::consts::TAU);
            let radius = (length_cells * 0.5 + width_cells).ceil() as isize;
            stamps.push(StampFeature {
                cx: x,
                cy: y,
                length_cells,
                width_cells,
                height_m: height,
                direction_rad,
                kind,
                radius,
            });
        }
        stamps
    }
}

/// Craton-noise rank CDF: `rank_cdf[i]` is the fraction of anchor samples below
/// the lower edge of histogram bin `i`; the last entry is 1.
fn rank_cdf_table(field: &Field<f64>) -> (f64, f64, Vec<f64>) {
    let sorted = field.sorted_values();
    let min = sorted.first().copied().unwrap_or(0.0);
    let max = sorted.last().copied().unwrap_or(1.0);
    let span = max - min;
    let mut counts = vec![0u32; CDF_BINS];
    if span > 0.0 {
        for &value in &sorted {
            let t = ((value - min) / span).clamp(0.0, 1.0);
            let bin = ((t * CDF_BINS as f64) as usize).min(CDF_BINS - 1);
            counts[bin] += 1;
        }
    }
    let total = sorted.len().max(1) as f64;
    let mut cdf = vec![0.0f64; CDF_BINS + 1];
    let mut accumulated = 0.0f64;
    for (bin, &count) in counts.iter().enumerate() {
        cdf[bin] = accumulated / total;
        accumulated += f64::from(count);
    }
    cdf[CDF_BINS] = accumulated / total;
    (min, max, cdf)
}

/// Coordinate scales implementing noise anisotropy, matching
/// [`NoiseField`]'s internal `axis_scales` exactly so tiles sample the same
/// field as single-shot.
fn axis_scales(anisotropy: f64, direction_deg: f64) -> (f64, f64) {
    if anisotropy <= 0.0 {
        return (1.0, 1.0);
    }
    let theta = direction_deg.to_radians();
    let along = 1.0 - 0.85 * anisotropy;
    let (sin, cos) = theta.sin_cos();
    let sx = 1.0 + (along - 1.0) * cos * cos;
    let sy = 1.0 + (along - 1.0) * sin * sin;
    (sx.max(0.08), sy.max(0.08))
}

/// One axis of bilinear sample indices, precomputed for a whole tile row or
/// column.
#[derive(Clone, Debug, Default)]
struct BilinearAxis {
    low: Vec<usize>,
    high: Vec<usize>,
    frac: Vec<f64>,
}

impl BilinearAxis {
    /// Builds the index/weight table for coordinates given in cell units.
    fn build(coords: &[f64], len: usize, wraps: bool) -> Self {
        let len_f = len as f64;
        let mut low = Vec::with_capacity(coords.len());
        let mut high = Vec::with_capacity(coords.len());
        let mut frac = Vec::with_capacity(coords.len());
        for &coordinate in coords {
            let x = if wraps {
                coordinate.rem_euclid(len_f)
            } else {
                coordinate.clamp(0.0, (len_f - 1.0).max(0.0))
            };
            let base = x.floor();
            let i0 = (base as usize).min(len - 1);
            let i1 = if wraps || i0 + 1 < len {
                (i0 + 1) % len
            } else {
                i0
            };
            low.push(i0);
            high.push(i1);
            frac.push(x - base);
        }
        Self { low, high, frac }
    }
}

/// Bilinear lookup through precomputed axis tables.
#[inline]
fn bilinear_sample<T: FieldScalar>(
    field: &Field<T>,
    ax: &BilinearAxis,
    ay: &BilinearAxis,
    bx: usize,
    by: usize,
) -> f64 {
    let stride = field.grid().width();
    let x0 = ax.low[bx];
    let x1 = ax.high[bx];
    let y0 = ay.low[by];
    let y1 = ay.high[by];
    let row0 = y0 * stride;
    let row1 = y1 * stride;
    let tx = ax.frac[bx];
    let top = field.get_index(row0 + x0).to_f64()
        + (field.get_index(row0 + x1).to_f64() - field.get_index(row0 + x0).to_f64()) * tx;
    let bottom = field.get_index(row1 + x0).to_f64()
        + (field.get_index(row1 + x1).to_f64() - field.get_index(row1 + x0).to_f64()) * tx;
    top + (bottom - top) * ay.frac[by]
}

// ---------------------------------------------------------------------------
// Landform stamps
// ---------------------------------------------------------------------------

/// Landform primitive placed by the single-shot stamp pass (the same set, so
/// tiled worlds contain the same kinds of named features).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Landform {
    Range,
    Plateau,
    Rift,
    OceanRidge,
    Trench,
    Caldera,
    Mesa,
    Peak,
}

/// One placed landform, in fine cell coordinates.
#[derive(Clone, Copy, Debug)]
struct StampFeature {
    cx: f64,
    cy: f64,
    length_cells: f64,
    width_cells: f64,
    height_m: f64,
    direction_rad: f64,
    kind: Landform,
    radius: isize,
}

/// Random landform choice for a boundary type; mirrors the single-shot
/// `pick_landform`, including the RNG draw for `Interior` returning `None`.
fn pick_landform(boundary: BoundaryType, rng: &mut Rng) -> Option<Landform> {
    let r = rng.next_f64();
    match boundary {
        BoundaryType::Convergent => {
            if r < 0.55 {
                Some(Landform::Range)
            } else if r < 0.8 {
                Some(Landform::Trench)
            } else {
                Some(Landform::Plateau)
            }
        }
        BoundaryType::Divergent => {
            if r < 0.5 {
                Some(Landform::Rift)
            } else {
                Some(Landform::OceanRidge)
            }
        }
        BoundaryType::Transform => {
            if r < 0.6 {
                Some(Landform::Rift)
            } else {
                Some(Landform::Peak)
            }
        }
        BoundaryType::Interior => {
            if r < 0.22 {
                Some(Landform::Plateau)
            } else if r < 0.42 {
                Some(Landform::Mesa)
            } else if r < 0.62 {
                Some(Landform::Peak)
            } else if r < 0.72 {
                Some(Landform::Caldera)
            } else {
                None
            }
        }
    }
}

/// Landform profile value, matching the single-shot stamp profile.
fn stamp_profile(kind: Landform, u: f64, v: f64, r2: f64) -> f64 {
    match kind {
        Landform::Range => (-(u * u * 1.5 + v * v * 1.5)).exp(),
        Landform::Plateau => 1.0 / (1.0 + r2.powi(3)),
        Landform::Rift => -(-(v * v * 2.0)).exp() * (-(u * u * 0.3)).exp(),
        Landform::OceanRidge => (-(v * v * 2.5)).exp() * (-(u * u * 0.2)).exp(),
        Landform::Trench => -(-(v * v * 6.0)).exp() * (-(u * u * 0.4)).exp(),
        Landform::Caldera => {
            let ring = (r2.sqrt() - 0.7).abs();
            (-ring * ring * 8.0).exp() * 0.8 - 2.0 * (-r2 * 6.0).exp()
        }
        Landform::Mesa => {
            if r2 < 1.4 {
                0.9
            } else {
                (-(r2 - 1.4) * 3.0).exp()
            }
        }
        Landform::Peak => (-r2 * 3.0).exp(),
    }
}

// ---------------------------------------------------------------------------
// Tile pass
// ---------------------------------------------------------------------------

/// One tile's generated elevations, including the halo.
#[derive(Clone, Debug)]
struct TileBuffer {
    data: Vec<f64>,
    stride: usize,
}

/// Evaluates tiles from the anchor data; immutable, so it can be shared by all
/// worker threads.
#[derive(Clone, Copy, Debug)]
struct TileGenerator<'a> {
    geom: &'a FineGeometry,
    anchor: &'a AnchorWorld,
    halo: u32,
}

impl<'a> TileGenerator<'a> {
    fn new(geom: &'a FineGeometry, anchor: &'a AnchorWorld, halo: u32) -> Self {
        Self { geom, anchor, halo }
    }

    /// Generates one tile (core plus halo).
    fn generate(&self, tile: TileRect) -> TileBuffer {
        let geom = self.geom;
        let anchor = self.anchor;
        let halo = self.halo as usize;
        let stride = tile.width as usize + 2 * halo;
        let rows = tile.height as usize + 2 * halo;
        let cells = stride * rows;
        let x_start = tile.x as isize - halo as isize;
        let y_start = tile.y as isize - halo as isize;

        // Per-column / per-row global coordinates and derived sample positions.
        let mut global_x = Vec::with_capacity(stride);
        let mut global_y = Vec::with_capacity(rows);
        let mut anchor_x = Vec::with_capacity(stride);
        let mut anchor_y = Vec::with_capacity(rows);
        let mut craton_u = Vec::with_capacity(stride);
        let mut craton_v = Vec::with_capacity(rows);
        let mut relief_u = Vec::with_capacity(stride);
        let mut relief_v = Vec::with_capacity(rows);
        let mut fold_x = Vec::with_capacity(stride);
        let mut fold_y = Vec::with_capacity(rows);
        let (fold_sin, fold_cos) = anchor.fold_sin_cos;
        let width_f = f64::from(geom.width);
        let height_f = f64::from(geom.height);
        for bx in 0..stride {
            let gx = geom.map_x(x_start + bx as isize);
            global_x.push(gx);
            anchor_x.push((f64::from(gx) + 0.5) / width_f * f64::from(anchor.anchor_width) - 0.5);
            craton_u.push(
                (f64::from(gx) + 0.5) / width_f * anchor.craton_cycles_x * anchor.craton_scale.0,
            );
            relief_u.push(
                (f64::from(gx) + 0.5) / width_f * anchor.relief_cycles_x * anchor.relief_scale.0,
            );
            fold_x.push(f64::from(gx) * geom.cell_m_x * fold_cos);
        }
        for by in 0..rows {
            let gy = geom.map_y(y_start + by as isize);
            global_y.push(gy);
            anchor_y.push((f64::from(gy) + 0.5) / height_f * f64::from(anchor.anchor_height) - 0.5);
            craton_v.push(
                (f64::from(gy) + 0.5) / height_f * anchor.craton_cycles_y * anchor.craton_scale.1,
            );
            relief_v.push(
                (f64::from(gy) + 0.5) / height_f * anchor.relief_cycles_y * anchor.relief_scale.1,
            );
            fold_y.push(f64::from(gy) * geom.cell_m_x * fold_sin);
        }
        let anchor_ax = BilinearAxis::build(&anchor_x, anchor.anchor_width as usize, geom.wraps_x);
        let anchor_ay = BilinearAxis::build(&anchor_y, anchor.anchor_height as usize, geom.wraps_y);

        // ---- Pass 1: craton rank (noise + plate bias).
        let mut rank = vec![0.0f64; cells];
        for by in 0..rows {
            let v = craton_v[by];
            let row = by * stride;
            for bx in 0..stride {
                let value = anchor.craton_noise.sample_tiled_xy(
                    craton_u[bx],
                    v,
                    anchor.craton_periods.0,
                    anchor.craton_periods.1,
                );
                let plate =
                    bilinear_sample(&anchor.plate_continental, &anchor_ax, &anchor_ay, bx, by);
                let bias = (2.0 * plate - 1.0) * 0.14;
                rank[row + bx] = (anchor.rank_of(value) + bias).clamp(0.0, 1.0);
            }
        }

        // ---- Local smoothing of the rank, then the sharp continental mask.
        let mut work = Vec::new();
        spatial::gaussian_blur_slice(&mut rank, &mut work, stride, rows, 2.5);

        // ---- Pass 2: continent → crust thickness (stored in `elev` for the
        // final pass, crust in `rank` so the blend can be blurred in place).
        let mut elev = vec![0.0f64; cells];
        let range = (anchor.continental_crust_m - OCEANIC_CRUST_M).max(1.0);
        let low = anchor.threshold - 0.05;
        let high = anchor.threshold + 0.05;
        for i in 0..cells {
            let continent = smoothstep(low, high, rank[i]);
            elev[i] = continent;
            rank[i] = OCEANIC_CRUST_M + continent * range;
        }
        spatial::gaussian_blur_slice(&mut rank, &mut work, stride, rows, 3.5);
        drop(work);

        // ---- Landform stamps as a sparse additive field.
        let mut stamp_delta = vec![0.0f64; cells];
        self.apply_stamps(&mut stamp_delta, stride, rows, x_start, y_start);

        // ---- Pass 3: Airy base + relief + folds/faults/orogen/hotspot/stamps.
        let vertical = geom.vertical;
        let mut elevation = vec![0.0f64; cells];
        let fold_on = anchor.fold_amplitude_m > 0.0 && anchor.fold_wavelength_m > 0.0;
        let fault_on = anchor.fault_offset_m > 0.0;
        let fault_seed = 0xFA_17_00u64;
        for by in 0..rows {
            let gy = global_y[by] as u64;
            let fold_row = fold_y[by];
            let row = by * stride;
            for bx in 0..stride {
                let i = row + bx;
                let gx = global_x[bx] as u64;
                let continent = elev[i];
                let crust = rank[i];
                let factor =
                    anchor.ocean_factor + (anchor.cont_factor - anchor.ocean_factor) * continent;
                let mut z = geom.sea + factor * (crust - anchor.reference_thickness_m);
                // Fractal relief: same field, integer frequencies, global
                // coordinates, and the median single-shot subtracts.
                let noise = anchor.relief_noise.sample_tiled_xy(
                    relief_u[bx],
                    relief_v[by],
                    anchor.relief_periods.0,
                    anchor.relief_periods.1,
                );
                let roughness = 0.05 + 0.13 * continent;
                z += (noise - anchor.relief_median) * vertical * roughness;
                // Orogenic belt distance, used by folds and faults.
                let distance =
                    bilinear_sample(&anchor.boundary_distance_m, &anchor_ax, &anchor_ay, bx, by);
                if fold_on {
                    let u = fold_x[bx] + fold_row;
                    let fold = (u / anchor.fold_wavelength_m * std::f64::consts::TAU
                        + anchor.fold_phase)
                        .sin()
                        * 0.5;
                    let belt = (-distance / anchor.fold_belt_scale_m).exp();
                    let continental = 0.25 + 0.75 * continent;
                    let gate = 0.1 + 0.9 * belt;
                    z += fold * anchor.fold_amplitude_m * anchor.fold_weight * continental * gate;
                }
                if fault_on && distance <= anchor.influence_m {
                    let convergence =
                        bilinear_sample(&anchor.convergence_m_yr, &anchor_ax, &anchor_ay, bx, by);
                    let sign = if convergence > 0.0 { -1.0 } else { 1.0 };
                    let falloff = (-distance / anchor.fault_falloff_m).exp();
                    let jitter = 0.6 + 0.8 * unit_from_hash(hash2(gx, gy, fault_seed));
                    z += sign * anchor.fault_offset_m * falloff * jitter;
                }
                // Orogen / trench integrated over the reference span, with the
                // fine-resolution collision weight and caps.
                let collide = 0.1 + 0.9 * continent;
                let orogen =
                    bilinear_sample(&anchor.orogen_rate_m_yr, &anchor_ax, &anchor_ay, bx, by)
                        * anchor.orogen_span_yr
                        * collide;
                let trench =
                    bilinear_sample(&anchor.trench_rate_m_yr, &anchor_ax, &anchor_ay, bx, by)
                        * anchor.orogen_span_yr
                        * 0.9;
                z += orogen.min(anchor.orogen_cap_m);
                z -= trench.min(anchor.orogen_cap_m * 0.6);
                // Hotspot swells and trails.
                z += bilinear_sample(&anchor.hotspot_m, &anchor_ax, &anchor_ay, bx, by);
                // Named landforms.
                z += stamp_delta[i];
                // Envelope, sanitise, and the datum safety net (only non-zero
                // for pathological seeds).
                let mut z = if z.is_finite() { z } else { geom.sea };
                z = z.clamp(geom.limit_low, geom.limit_high);
                z += anchor.datum_shift_m;
                z = z.clamp(geom.limit_low, geom.limit_high);
                elevation[i] = z;
            }
        }
        TileBuffer {
            data: elevation,
            stride,
        }
    }

    /// Adds the sparse landform stamps that overlap this padded tile.
    fn apply_stamps(
        &self,
        delta: &mut [f64],
        stride: usize,
        rows: usize,
        x_start: isize,
        y_start: isize,
    ) {
        let geom = self.geom;
        for stamp in &self.anchor.stamps {
            let (sin, cos) = stamp.direction_rad.sin_cos();
            let sx = stamp.cx.round() as isize;
            let sy = stamp.cy.round() as isize;
            let radius = stamp.radius;
            for dy in -radius..=radius {
                let raw_y = sy + dy;
                let gy = if geom.wraps_y {
                    raw_y.rem_euclid(geom.height as isize)
                } else if (0..geom.height as isize).contains(&raw_y) {
                    raw_y
                } else {
                    continue;
                };
                let py = gy as f64 - stamp.cy;
                let mut visit_row = |by: usize| {
                    for dx in -radius..=radius {
                        let raw_x = sx + dx;
                        let gx = if geom.wraps_x {
                            raw_x.rem_euclid(geom.width as isize)
                        } else if (0..geom.width as isize).contains(&raw_x) {
                            raw_x
                        } else {
                            continue;
                        };
                        let px = gx as f64 - stamp.cx;
                        let u = (px * cos + py * sin) / stamp.length_cells.max(1e-6);
                        let v = (-px * sin + py * cos) / stamp.width_cells.max(1e-6);
                        let r2 = u * u + v * v;
                        if r2 > 4.0 {
                            continue;
                        }
                        let profile = stamp_profile(stamp.kind, u, v, r2) * stamp.height_m;
                        for_each_axis_position(
                            gx,
                            x_start,
                            stride,
                            geom.width as usize,
                            geom.wraps_x,
                            |bx| {
                                delta[by * stride + bx] += profile;
                            },
                        );
                    }
                };
                for_each_axis_position(
                    gy,
                    y_start,
                    rows,
                    geom.height as usize,
                    geom.wraps_y,
                    &mut visit_row,
                );
            }
        }
    }
}

/// Calls `visit` with every buffer position (0..span) on one axis whose global
/// cell index equals `global`, for a buffer starting at `start` (unwrapped) and
/// covering `span` cells of a periodic or open axis of length `len`.
///
/// On periodic axes the same global cell can appear more than once in a buffer
/// (halo wider than the world), hence the loop.
#[inline]
fn for_each_axis_position(
    global: isize,
    start: isize,
    span: usize,
    len: usize,
    wraps: bool,
    mut visit: impl FnMut(usize),
) {
    if wraps {
        let period = len as isize;
        // Positions u = global + k * period with start <= u < start + span.
        let k_min = ceil_div(start - global, period);
        let k_max = floor_div(start + span as isize - 1 - global, period);
        for k in k_min..=k_max {
            let position = global + k * period - start;
            if position >= 0 && (position as usize) < span {
                visit(position as usize);
            }
        }
    } else if global >= 0 && global < len as isize {
        let position = global - start;
        if position >= 0 && (position as usize) < span {
            visit(position as usize);
        }
    }
}

#[inline]
fn ceil_div(a: isize, b: isize) -> isize {
    -((-a).div_euclid(b))
}

#[inline]
fn floor_div(a: isize, b: isize) -> isize {
    a.div_euclid(b)
}

// ---------------------------------------------------------------------------
// Driver helpers
// ---------------------------------------------------------------------------

/// Rough peak memory of one concurrent tile evaluation (four padded `f64`
/// buffers and their scratch). Used to cap the worker pool so peak RSS stays
/// inside the SPEC budget even on many-core machines.
fn worker_estimate_bytes(options: &LargeWorldOptions) -> u64 {
    let side = u64::from(options.tile_size + 2 * options.halo);
    side * side * 8 * 5
}

/// Resolves the worker count: `0` means "as many as the machine offers", capped
/// so the concurrent tile buffers stay inside [`TILE_MEMORY_BUDGET_BYTES`].
fn resolve_workers(requested: u32, plan_len: usize, per_worker_bytes: u64) -> usize {
    let available = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4);
    let memory_cap = (TILE_MEMORY_BUDGET_BYTES / per_worker_bytes.max(1)).max(1) as usize;
    let requested = if requested == 0 {
        available
    } else {
        requested as usize
    };
    requested.min(memory_cap).min(plan_len).max(1)
}

/// Folds one tile into the elevation histogram.
fn fold_histogram(values: &[f64], min: f64, max: f64, bins: &mut [u32]) {
    if bins.is_empty() {
        return;
    }
    let span = (max - min).max(1e-9);
    let last = bins.len() - 1;
    for &value in values {
        if !value.is_finite() {
            continue;
        }
        let t = ((value - min) / span).clamp(0.0, 1.0);
        let bin = ((t * bins.len() as f64) as usize).min(last);
        bins[bin] += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BoundaryKind;
    use crate::pipeline::generate_initial;

    fn small_params(width: u32, height: u32, seed: u64) -> Params {
        let mut p = Params::default();
        p.world.width = width;
        p.world.height = height;
        p.world.cell_km = 10.0;
        p.world.seed = seed;
        p.time.total_yr = 50_000.0;
        p.time.step_yr = 10_000.0;
        p
    }

    fn options(width: u32) -> LargeWorldOptions {
        LargeWorldOptions {
            tile_size: if width >= 128 { 128 } else { 64 },
            halo: 32,
            preview_factor: 4,
            workers: 1,
            export: None,
        }
    }

    fn run(p: &Params, width: u32, height: u32, options: &LargeWorldOptions) -> LargeWorldResult {
        generate_large_world_with(p, width, height, options).expect("tiled generation")
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("geogenesis_tiling_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn options_are_normalised_into_safe_ranges() {
        let raw = LargeWorldOptions {
            tile_size: 3,
            halo: 0,
            preview_factor: 0,
            workers: 0,
            export: None,
        };
        let options = raw.normalized();
        assert_eq!(options.tile_size, 64);
        assert_eq!(options.halo, MIN_HALO);
        assert_eq!(options.preview_factor, 2);
        let huge = LargeWorldOptions {
            tile_size: 99_999,
            halo: 99_999,
            preview_factor: 99_999,
            workers: 0,
            export: None,
        }
        .normalized();
        assert_eq!(huge.tile_size, 1024);
        assert_eq!(huge.halo, 256);
        assert_eq!(huge.preview_factor, 64);
    }

    #[test]
    fn geometry_maps_halos_inside_the_world() {
        let mut p = small_params(64, 48, 5);
        p.world.boundary = BoundaryKind::Rectangular;
        let geom = FineGeometry::new(&p, 64, 48).unwrap();
        for unwrapped in [-100isize, -1, 0, 63, 64, 1000] {
            let gx = geom.map_x(unwrapped);
            assert!((gx as usize) < 64, "x {unwrapped} -> {gx}");
            let gy = geom.map_y(unwrapped);
            assert!((gy as usize) < 48, "y {unwrapped} -> {gy}");
        }
        // Periodic axes wrap to the opposite edge.
        p.world.boundary = BoundaryKind::Toroidal;
        let geom = FineGeometry::new(&p, 64, 48).unwrap();
        assert_eq!(geom.map_x(-1), 63);
        assert_eq!(geom.map_x(64), 0);
        assert_eq!(geom.map_y(-1), 47);
        // Tiny and huge grids are rejected cleanly.
        assert!(FineGeometry::new(&p, 4, 4).is_err());
    }

    #[test]
    fn buffer_positions_respect_periodicity_and_bounds() {
        let mut seen = Vec::new();
        // Buffer over a periodic world: the same global cell can appear twice
        // when the halo wraps a whole period.
        for_each_axis_position(3, -2, 10, 5, true, |position| seen.push(position));
        assert_eq!(seen, vec![0, 5]);
        seen.clear();
        for_each_axis_position(3, 0, 10, 5, true, |position| seen.push(position));
        assert_eq!(seen, vec![3, 8]);
        // Open axis: out-of-range cells are skipped and in-range ones mapped.
        seen.clear();
        for_each_axis_position(3, -4, 12, 8, false, |position| seen.push(position));
        assert_eq!(seen, vec![7]);
        seen.clear();
        for_each_axis_position(-1, -4, 12, 8, false, |position| seen.push(position));
        assert!(seen.is_empty());
    }

    #[test]
    fn tile_cores_match_a_world_spanning_tile_exactly() {
        // Every tile core cell must equal the value a single tile covering the
        // whole world produces for that cell. This is the strongest seam
        // contract: the per-tile pass is a pure function of global coordinates,
        // and the halo is wide enough that the padding boundary cannot reach the
        // core. It also checks halos are consistent with neighbours inside the
        // halo's safe margin.
        let mut p = small_params(256, 256, 7);
        p.world.boundary = BoundaryKind::Toroidal;
        let geom = FineGeometry::new(&p, 256, 256).unwrap();
        let anchor = AnchorWorld::build(&p, &geom).unwrap();
        let halo = 32u32;
        let generator = TileGenerator::new(&geom, &anchor, halo);
        let plan = tile_plan(256, 256, 128, halo);
        let whole = tile_plan(256, 256, 256, halo);
        assert_eq!(whole.len(), 1);
        let reference = generator.generate(whole[0]);
        let h = halo as usize;
        let mut checked = 0usize;
        for tile in &plan {
            let buffer = generator.generate(*tile);
            let stride = buffer.stride;
            for ty in 0..tile.height as usize {
                for tx in 0..tile.width as usize {
                    let gx = tile.x as usize + tx;
                    let gy = tile.y as usize + ty;
                    let got = buffer.data[(ty + h) * stride + h + tx];
                    let want = reference.data[(gy + h) * reference.stride + h + gx];
                    assert_eq!(got, want, "tile ({}, {}) cell ({gx}, {gy})", tile.x, tile.y);
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 256 * 256);
        // The step across a tile seam between neighbouring cells stays small
        // compared with the vertical envelope (no visible tile edge).
        let left = plan.iter().find(|t| t.x == 0 && t.y == 0).copied().unwrap();
        let right = plan
            .iter()
            .find(|t| t.x == 128 && t.y == 0)
            .copied()
            .unwrap();
        let left_buf = generator.generate(left);
        let right_buf = generator.generate(right);
        for row in 0..left.height as usize {
            let a = left_buf.data[(row + h) * left_buf.stride + h + 127];
            let b = right_buf.data[(row + h) * right_buf.stride + h];
            assert!(
                (a - b).abs() < p.world.vertical_m * 0.25,
                "seam step {a} vs {b}"
            );
        }
    }

    #[test]
    fn generation_is_deterministic_and_worker_invariant() {
        let p = small_params(128, 128, 21);
        let one_thread = options(128);
        let many_threads = LargeWorldOptions {
            workers: 4,
            ..options(128)
        };
        let a = run(&p, 128, 128, &one_thread);
        let b = run(&p, 128, 128, &many_threads);
        // Every elevation is a pure function of global coordinates and the
        // accumulators are folded in plan order, so the results are identical
        // down to the bit.
        assert_eq!(a.preview, b.preview);
        assert_eq!(a.preview_counts, b.preview_counts);
        assert_eq!(a.histogram, b.histogram);
        assert_eq!(a.stats.count, b.stats.count);
        assert_eq!(a.stats.min_m.to_bits(), b.stats.min_m.to_bits());
        assert_eq!(a.stats.max_m.to_bits(), b.stats.max_m.to_bits());
        assert_eq!(a.stats.sum_m.to_bits(), b.stats.sum_m.to_bits());
        assert_eq!(a.stats.land_cells, b.stats.land_cells);
        // Repeated calls are identical too.
        let c = run(&p, 128, 128, &many_threads);
        assert_eq!(a.preview, c.preview);
        assert_eq!(a.stats.sum_m.to_bits(), c.stats.sum_m.to_bits());
    }

    #[test]
    fn preview_folds_every_cell_exactly_once() {
        let p = small_params(128, 128, 3);
        let result = run(&p, 128, 128, &options(128));
        assert_eq!(result.preview_width, 32);
        assert_eq!(result.preview_height, 32);
        assert_eq!(result.stats.count, 128 * 128);
        let total: u64 = result.preview_counts.iter().map(|&n| u64::from(n)).sum();
        assert_eq!(total, 128 * 128);
        assert!(result.preview_counts.iter().all(|&n| n == 16));
        assert!(result.preview_means().iter().all(|v| v.is_finite()));
        // Preview means track the true per-cell values: their average equals
        // the global mean.
        let preview_mean = result.preview_means().iter().sum::<f64>() / 1024.0;
        assert!((preview_mean - result.stats.mean_m()).abs() < 1e-9);
        // Elevation statistics are self-consistent.
        let stats = result.elevation_stats();
        assert_eq!(stats.cells, 128 * 128);
        assert!(stats.max_m > stats.min_m);
        assert!(stats.median_m >= stats.min_m && stats.median_m <= stats.max_m);
        assert!(stats.p10_m <= stats.median_m && stats.median_m <= stats.p90_m);
    }

    #[test]
    fn tiled_statistics_match_single_shot_on_a_small_grid() {
        // Same parameters and seed, generated by the single-shot pipeline and by
        // the tiled path. The tiled world samples the smooth tectonic fields from
        // a 1/8-resolution anchor, so the statistics agree within documented
        // tolerances rather than exactly (measured spread across seeds at 256²
        // and 512²: |Δland| <= 0.01, |Δmean| <= 0.01·vertical, |Δrelief| < 10%).
        for seed in [42u64, 1337] {
            let p = small_params(256, 256, seed);
            let single = generate_initial(&p).unwrap();
            let tiled = run(&p, 256, 256, &options(256));
            let single_land = single.world.land_fraction();
            let tiled_land = tiled.land_fraction();
            assert!(
                (single_land - tiled_land).abs() < 0.05,
                "seed {seed}: land fraction differs: single {single_land:.3}, tiled {tiled_land:.3}"
            );
            let (single_min, single_max) = single.world.relief_range_m();
            let single_relief = single_max - single_min;
            assert!(
                (single_max - tiled.stats.max_m).abs() < p.world.vertical_m * 0.20,
                "seed {seed}: max elevation differs: {} vs {}",
                single_max,
                tiled.stats.max_m
            );
            assert!(
                (single_min - tiled.stats.min_m).abs() < p.world.vertical_m * 0.20,
                "seed {seed}: min elevation differs: {} vs {}",
                single_min,
                tiled.stats.min_m
            );
            let single_mean = single.world.elevation_m.mean().unwrap();
            assert!(
                (single_mean - tiled.stats.mean_m()).abs() < p.world.vertical_m * 0.03,
                "seed {seed}: mean elevation differs: {single_mean} vs {}",
                tiled.stats.mean_m()
            );
            let single_std = single.world.elevation_m.std_dev().unwrap();
            assert!(
                (single_std - tiled.stats.std_dev_m()).abs() < p.world.vertical_m * 0.05,
                "seed {seed}: elevation σ differs: {single_std} vs {}",
                tiled.stats.std_dev_m()
            );
            assert!(
                (single_relief - tiled.stats.relief_m()).abs() < single_relief * 0.15,
                "seed {seed}: relief differs: {single_relief} vs {}",
                tiled.stats.relief_m()
            );
            // Terrain exists: a healthy relief range and both land and water.
            assert!((0.05..0.95).contains(&tiled_land));
            assert!(tiled.stats.relief_m() > p.world.vertical_m * 0.5);
            assert_eq!(tiled.stats.count, 256 * 256);
        }
    }

    #[test]
    fn anchor_tectonic_rates_match_a_fine_mosaic() {
        // Regression for the anchor geometry: the coarse pass must preserve the
        // physical extent, so its blurred orogen/trench rates have the same
        // amplitude as the single-shot fine mosaic (a 16x-too-large anchor cell
        // used to attenuate them by more than an order of magnitude).
        let p = small_params(256, 256, 42);
        let geom = FineGeometry::new(&p, 256, 256).unwrap();
        let anchor = AnchorWorld::build(&p, &geom).unwrap();
        let grid = p.world.grid().unwrap();
        let mosaic = PlateMosaic::build(&p, grid, p.world.effective_seed());
        let sigma = (p.tectonics.orogeny_width_km * 1000.0 / grid.cell_size_m() * 0.6).max(1.0);
        let mut orogen = Field::<f64>::zeros(grid);
        let mut trench = Field::<f64>::zeros(grid);
        for i in 0..grid.cell_count() {
            let rate = f64::from(mosaic.boundary_uplift_m_yr.get_index(i));
            if rate >= 0.0 {
                orogen.set_index(i, rate);
            } else {
                trench.set_index(i, -rate);
            }
        }
        spatial::gaussian_blur(&mut orogen, sigma);
        spatial::gaussian_blur(&mut trench, sigma * 0.5);
        let mut sums = [0.0f64; 8];
        for gy in 0..256u32 {
            for gx in 0..256u32 {
                let i = grid.index(gx as usize, gy as usize);
                let t_or = anchor.sample_at(&anchor.orogen_rate_m_yr, &geom, gx, gy);
                let t_tr = anchor.sample_at(&anchor.trench_rate_m_yr, &geom, gx, gy);
                sums[0] += orogen.get_index(i);
                sums[1] += t_or;
                sums[2] = sums[2].max(orogen.get_index(i));
                sums[3] = sums[3].max(t_or);
                sums[4] += trench.get_index(i);
                sums[5] += t_tr;
                sums[6] = sums[6].max(trench.get_index(i));
                sums[7] = sums[7].max(t_tr);
            }
        }
        let n = 256.0 * 256.0;
        // Means within 2x and peaks within 30% of the fine-mosaic values.
        assert!(
            sums[1] / n > 0.5 * sums[0] / n && sums[1] / n < 2.0 * sums[0] / n,
            "orogen mean {} vs fine {}",
            sums[1] / n,
            sums[0] / n
        );
        assert!(
            sums[3] > 0.7 * sums[2],
            "orogen peak {} vs fine {}",
            sums[3],
            sums[2]
        );
        assert!(
            sums[5] / n > 0.5 * sums[4] / n && sums[5] / n < 2.0 * sums[4] / n,
            "trench mean {} vs fine {}",
            sums[5] / n,
            sums[4] / n
        );
        assert!(
            sums[7] > 0.5 * sums[6],
            "trench peak {} vs fine {}",
            sums[7],
            sums[6]
        );
    }

    #[test]
    fn all_cells_are_finite_and_inside_the_envelope() {
        for seed in [1u64, 7, 99, 2024] {
            let p = small_params(128, 128, seed);
            let result = run(&p, 128, 128, &options(128));
            let stats = &result.stats;
            assert_eq!(stats.count, 128 * 128);
            assert!(stats.min_m.is_finite() && stats.max_m.is_finite());
            assert!(
                stats.min_m >= -p.world.vertical_m * 1.8 - 1e-6,
                "seed {seed}: min {} below envelope",
                stats.min_m
            );
            assert!(
                stats.max_m <= p.world.vertical_m * 1.2 + 1e-6,
                "seed {seed}: max {} above envelope",
                stats.max_m
            );
            assert_eq!(result.histogram.iter().sum::<u32>(), 128 * 128);
        }
    }

    #[test]
    fn pathological_parameters_are_guarded_not_poisoned() {
        // A world far above the sea and a world far below it both hit the
        // datum safety net; neither may produce NaN or a degenerate field.
        for sea in [-4000.0, 4000.0] {
            let mut p = small_params(96, 96, 11);
            p.world.sea_level_m = sea;
            let result = run(&p, 96, 96, &options(96));
            let land = result.land_fraction();
            assert!(land.is_finite(), "land fraction {land}");
            assert!(
                land > 0.01 && land < 0.99,
                "sea {sea}: guard failed, land fraction {land}"
            );
            assert!(result.stats.relief_m() > 0.0);
        }
        // Very small vertical scale still produces finite, bounded terrain.
        let mut p = small_params(96, 96, 4);
        p.world.vertical_m = 300.0;
        let result = run(&p, 96, 96, &options(96));
        assert!(result.stats.min_m.is_finite() && result.stats.max_m.is_finite());
        assert_eq!(result.stats.count, 96 * 96);
    }

    #[test]
    fn exports_stream_to_a_valid_directory() {
        let dir = temp_dir("exports");
        let p = small_params(64, 64, 17);
        let mut opts = options(64);
        opts.export = Some(ExportRequest::with_kinds(&dir, ExportKinds::ALL));
        let result = run(&p, 64, 64, &opts);
        let summary = result.exports.expect("export summary");
        assert_eq!(
            summary.files,
            vec![
                "terrain.png".to_string(),
                "normals.png".to_string(),
                "elevation.f32".to_string(),
                "height16.png".to_string(),
                "preview.png".to_string()
            ]
        );
        // Raw raster matches the streamed statistics exactly.
        let raw = std::fs::read(dir.join("elevation.f32")).unwrap();
        assert_eq!(raw.len(), 64 * 64 * 4);
        let values: Vec<f64> = raw
            .chunks_exact(4)
            .map(|bytes| f64::from(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])))
            .collect();
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!((min - result.stats.min_m).abs() < 0.01);
        assert!((max - result.stats.max_m).abs() < 0.01);
        // PNG headers: size, bit depth and colour type.
        for (name, width, height, depth, colour) in [
            ("terrain.png", 64u32, 64u32, 8u8, 2u8),
            ("height16.png", 64, 64, 16, 0),
            ("normals.png", 64, 64, 8, 2),
            ("preview.png", 16, 16, 8, 2),
        ] {
            let bytes = std::fs::read(dir.join(name)).unwrap();
            assert_eq!(
                &bytes[..8],
                &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
                "{name} signature"
            );
            assert_eq!(
                u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
                width,
                "{name} width"
            );
            assert_eq!(
                u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
                height,
                "{name} height"
            );
            assert_eq!(bytes[24], depth, "{name} bit depth");
            assert_eq!(bytes[25], colour, "{name} colour type");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_exports_reruns_the_pass_with_the_same_world() {
        let dir = temp_dir("write_exports");
        let p = small_params(64, 64, 23);
        let result = run(&p, 64, 64, &options(64));
        let summary = result
            .write_exports(&dir, ExportKinds::default())
            .expect("streamed exports");
        assert_eq!(
            summary.files,
            vec![
                "terrain.png".to_string(),
                "elevation.f32".to_string(),
                "height16.png".to_string(),
                "preview.png".to_string()
            ]
        );
        let raw = std::fs::read(dir.join("elevation.f32")).unwrap();
        assert_eq!(raw.len(), 64 * 64 * 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rectangular_worlds_stream_without_edge_spikes() {
        // Open boundaries put clamped padding in the halo; the result must stay
        // finite and inside the relief envelope.
        for boundary in [BoundaryKind::Rectangular, BoundaryKind::Spherical] {
            let mut p = small_params(96, 80, 7);
            p.world.boundary = boundary;
            let result = run(&p, 96, 80, &options(96));
            assert_eq!(result.stats.count, 96 * 80);
            assert!(result.stats.min_m.is_finite());
            assert!(result.stats.max_m < p.world.vertical_m * 1.25);
            let land = result.land_fraction();
            assert!((0.02..0.98).contains(&land), "{boundary:?}: land {land}");
        }
    }

    #[test]
    fn unknown_large_grid_requests_fail_with_parameter_errors() {
        let p = small_params(64, 64, 1);
        assert!(generate_large_world_with(&p, 8, 8, &options(64)).is_err());
    }

    #[test]
    fn rank_cdf_is_monotone_and_bounded() {
        let p = small_params(64, 64, 5);
        let geom = FineGeometry::new(&p, 64, 64).unwrap();
        let anchor = AnchorWorld::build(&p, &geom).unwrap();
        assert_eq!(anchor.rank_cdf.len(), CDF_BINS + 1);
        assert!((anchor.rank_cdf[0] - 0.0).abs() < 1e-12);
        assert!((anchor.rank_cdf[CDF_BINS] - 1.0).abs() < 1e-12);
        for pair in anchor.rank_cdf.windows(2) {
            assert!(pair[0] <= pair[1] + 1e-12);
        }
        assert!(anchor.rank_of(anchor.craton_min) < 0.1);
        assert!(anchor.rank_of(anchor.craton_max) > 0.9);
    }
}
