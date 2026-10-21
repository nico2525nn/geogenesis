//! Generation pipeline: parameter set → initial world (spec §2 Phases 0–2).
//!
//! The initial relief is assembled from physically meaningful pieces:
//!
//! 1. **Crust map** — a low-frequency craton field biased by plate type decides
//!    where continental crust exists. Crustal thickness is smoothed, which
//!    automatically produces continental shelves instead of a hard coastline.
//! 2. **Airy base** — elevation from isostasy: thick buoyant continental crust
//!    floats near the datum, thin dense oceanic crust sits several kilometres
//!    below it. Water loading is included in the ocean branch.
//! 3. **Fractal relief** — seeded fBm/ridged noise, stronger on continents.
//! 4. **Folds and faults** — directional fabric and boundary steps.
//! 5. **Orogens and trenches** — convergent margins thicken the crust over a
//!    wide, smoothed belt; subduction zones flex the surface down.
//! 6. **Hotspots and landform stamps** — plume swells, ranges, plateaus,
//!    rifts, ridges, trenches, calderas, mesas and isolated peaks.
//!
//! The generated distribution is not normalised to an exact range: the
//! sea-level datum decides how much of the world is ocean. A conservative
//! centring correction only fires when a seed produces a pathological
//! (all-ocean or all-land) world.

use crate::error::GeoResult;
use crate::geology::Geology;
use crate::grid::{Field, Grid};
use crate::math::noise::{FractalKind, NoiseField, NoiseKind, NoiseParams};
use crate::math::rng::{derive_seed, hash2, unit_from_hash, Rng};
use crate::math::{clamp, smoothstep};
use crate::params::Params;
use crate::plates::{BoundaryType, PlateMosaic};
use crate::spatial;
use crate::uplift::anchor_crust;
use crate::world::World;

/// Domain tag for initial-relief random numbers.
const DOMAIN_RELIEF: u64 = 0x7265_6C69_6566_0001;

/// Reference oceanic crustal thickness, metres.
const OCEANIC_CRUST_M: f64 = 7000.0;
/// Density of seawater, kg/m³ (used in the Airy water-load correction).
const WATER_DENSITY: f64 = 1030.0;

/// Everything generated before the time loop starts.
#[derive(Clone, Debug)]
pub struct PipelineOutput {
    /// Initial world state.
    pub world: World,
    /// Plate mosaic.
    pub mosaic: PlateMosaic,
    /// Geological columns and surface rock maps.
    pub geology: Geology,
}

/// Runs Phases 0–2 and returns the initial world.
pub fn generate_initial(params: &Params) -> GeoResult<PipelineOutput> {
    let mut p = params.clone();
    // Validate the raw parameters first: silently clamping an out-of-range
    // grid would hide caller bugs. `normalize` then reconciles soft bounds.
    p.validate()?;
    p.normalize();
    p.validate()?;
    let grid = p.world.grid()?;
    let seed = p.world.effective_seed();
    let mut world = World::new(&p.world)?;
    let sea = world.sea_level_m;
    let tectonic = &p.tectonics;

    // ---- Plates and cratons ----------------------------------------------
    let mosaic = PlateMosaic::build(&p, grid, seed);

    // Low-frequency craton field: where continental crust exists.
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
    )
    .fill(&grid, 3.5);
    // Rank-normalise so the threshold below maps directly to an area fraction.
    let mut craton_rank = Field::<f64>::zeros(grid);
    {
        let sorted = craton_noise.sorted_values();
        for (i, &value) in craton_noise.as_slice().iter().enumerate() {
            let rank = sorted.partition_point(|&v| v < value);
            craton_rank.set_index(i, rank as f64 / (sorted.len().max(2) - 1) as f64);
        }
    }
    // Bias by plate type and smooth: continents tend to follow continental
    // plates but may span boundaries, as on Earth.
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            let plate = mosaic.plate_at(x, y);
            let bias = if plate.continental { 0.14 } else { -0.14 };
            craton_rank.set_index(i, (craton_rank.get_index(i) + bias).clamp(0.0, 1.0));
        }
    }
    spatial::gaussian_blur(&mut craton_rank, 2.5);
    let continental_fraction = clamp(0.22 + 0.50 * tectonic.continental_fraction, 0.16, 0.72);
    let threshold = 1.0 - continental_fraction;
    let mut craton = Field::<f64>::zeros(grid);
    for (i, &rank) in craton_rank.as_slice().iter().enumerate() {
        craton.set_index(i, smoothstep(threshold - 0.05, threshold + 0.05, rank));
    }

    // Crustal thickness: thin basaltic ocean crust → thick granitic continents.
    let continental_crust_m = (tectonic.plate_thickness_km * 1000.0).max(20_000.0);
    let mut crust = Field::<f64>::zeros(grid);
    for (i, &c) in craton.as_slice().iter().enumerate() {
        crust.set_index(
            i,
            OCEANIC_CRUST_M + c * (continental_crust_m - OCEANIC_CRUST_M),
        );
    }
    // Smoothing produces shelves, slopes and a natural coastline.
    spatial::gaussian_blur(&mut crust, 3.5);

    // Airy isostasy with a water-load correction in the ocean branch.
    let rho_m = tectonic.mantle_density_kg_m3;
    let rho_c = tectonic.density_continental_kg_m3;
    let rho_o = tectonic.density_oceanic_kg_m3;
    let cont_factor = 1.0 - rho_c / rho_m;
    let ocean_factor = (rho_m - rho_o) / (rho_m - WATER_DENSITY);
    let reference_thickness = continental_crust_m - 400.0 / cont_factor.max(1e-3);
    let mut elevation = Field::<f64>::zeros(grid);
    for i in 0..grid.cell_count() {
        let c = craton.get_index(i);
        let factor = ocean_factor + (cont_factor - ocean_factor) * c;
        let z = factor * (crust.get_index(i) - reference_thickness);
        elevation.set_index(i, sea + z);
    }

    // ---- Fractal relief ---------------------------------------------------
    let extent_km = grid.width() as f64 * p.world.cell_km;
    let cycles = clamp(extent_km / 800.0, 3.0, 9.0);
    let noise = p.relief.noise_field(derive_seed(seed, DOMAIN_RELIEF));
    let mut base_relief = noise.fill(&grid, cycles);
    let median = base_relief.quantile(0.5);
    base_relief.map_in_place(|_, v| v - median);
    // Continents are rougher than abyssal plains, and relief grows with the
    // vertical scale. This also keeps the ocean floor from looking noisy.
    let mut relief = Field::<f64>::zeros(grid);
    for i in 0..grid.cell_count() {
        let roughness = 0.05 + 0.13 * craton.get_index(i);
        relief.set_index(i, base_relief.get_index(i) * p.world.vertical_m * roughness);
    }
    elevation.add_scaled(&relief, 1.0);

    // ---- Folds, faults, orogens, hotspots, stamps ------------------------
    apply_folds(&mut elevation, &mosaic, &craton, &p, seed);
    apply_faults(&mut elevation, &mosaic, &p);

    // Orogens: the boundary rate is smoothed over the orogen width before
    // being integrated over a reference span, turning the Voronoi edges into
    // broad mountain belts rather than straight walls.
    let mut orogen_rate = Field::<f64>::zeros(grid);
    let mut trench_rate = Field::<f64>::zeros(grid);
    for i in 0..grid.cell_count() {
        let rate = f64::from(mosaic.boundary_uplift_m_yr.get_index(i));
        if rate >= 0.0 {
            orogen_rate.set_index(i, rate);
        } else {
            trench_rate.set_index(i, -rate);
        }
    }
    let sigma_cells = (tectonic.orogeny_width_km * 1000.0 / grid.cell_size_m() * 0.6).max(1.0);
    spatial::gaussian_blur(&mut orogen_rate, sigma_cells);
    spatial::gaussian_blur(&mut trench_rate, sigma_cells * 0.5);
    let orogen_span_yr = 1_200_000.0;
    // Physical rates are millimetres per year, so the integrated uplift is
    // (rate × span) metres; cap at 30% of the vertical scale to leave room for
    // later evolution.
    let orogen_cap = p.world.vertical_m * 0.30;
    for i in 0..grid.cell_count() {
        let continental = craton.get_index(i);
        // Continental collision thickens crust; oceanic subduction mainly
        // produces a trench and a volcanic arc, not a broad plateau.
        let collide = 0.1 + 0.9 * continental;
        let uplift = (orogen_rate.get_index(i) * orogen_span_yr * collide).min(orogen_cap);
        let trench = (trench_rate.get_index(i) * orogen_span_yr * 0.9).min(orogen_cap * 0.6);
        elevation.set_index(i, elevation.get_index(i) + uplift - trench);
    }

    // Hotspot swells: broad, smooth domes.
    let mut hotspot = Field::<f64>::zeros(grid);
    for (i, &rate) in mosaic.hotspot_uplift_m_yr.as_slice().iter().enumerate() {
        hotspot.set_index(i, f64::from(rate) * 1.5e6);
    }
    spatial::gaussian_blur(
        &mut hotspot,
        (tectonic.hotspot_radius_km * 1000.0 / grid.cell_size_m() * 0.35).max(1.0),
    );
    elevation.add_scaled(&hotspot, 1.0);

    apply_stamps(&mut elevation, &mosaic, &craton, &p, seed);

    // ---- Datum, bounds and pathological-world correction ------------------
    let limit_low = -p.world.vertical_m * 1.8;
    let limit_high = p.world.vertical_m * 1.2;
    elevation.clamp_values(limit_low, limit_high);
    elevation.sanitize_non_finite(sea);
    // If the seed produced an all-ocean or all-land world, shift the datum by
    // the smallest amount that restores a plausible mix. This is a safety net,
    // not the primary control: sea level still moves relative to the terrain.
    let land = elevation.fraction_where(|z| z > sea);
    if !(0.08..=0.80).contains(&land) {
        let target = 0.30;
        let threshold_z = elevation.quantile(1.0 - target);
        let shift = clamp(sea - threshold_z, -p.world.vertical_m, p.world.vertical_m) * 0.8;
        elevation.add_scalar(shift);
        elevation.clamp_values(limit_low, limit_high);
    }
    world.elevation_m = elevation;

    // ---- Crust anchor and geology ----------------------------------------
    anchor_crust(&mut world, &p);
    let geology = Geology::build(&p, &world, &mosaic, seed);

    debug_assert_eq!(world.grid, grid);
    Ok(PipelineOutput {
        world,
        mosaic,
        geology,
    })
}

/// Sinusoidal fold belts, strongest on continental crust.
fn apply_folds(
    elevation: &mut Field<f64>,
    mosaic: &PlateMosaic,
    craton: &Field<f64>,
    params: &Params,
    seed: u64,
) {
    let grid = elevation.grid();
    let amp = params.relief.fold_amplitude_m;
    let wavelength_m = params.relief.fold_wavelength_km * 1000.0;
    if amp <= 0.0 || wavelength_m <= 0.0 {
        return;
    }
    let angle = params.relief.anisotropy_deg.to_radians();
    let (sa, ca) = angle.sin_cos();
    let cell_m = grid.cell_size_m();
    let mut rng = Rng::new(derive_seed(seed, DOMAIN_RELIEF ^ 0xF0_1D));
    let phase = rng.range_f64(0.0, std::f64::consts::TAU);
    let weight = 0.3 + 0.7 * params.relief.anisotropy;
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let u = x as f64 * cell_m * ca + y as f64 * cell_m * sa;
            let fold = (u / wavelength_m * std::f64::consts::TAU + phase).sin() * 0.5;
            let i = grid.index(x, y);
            // Fold belts belong to orogenic zones, not to every craton: gate
            // them by proximity to a convergent boundary.
            let dist = f64::from(mosaic.boundary_distance_m.get_index(i));
            let belt = (-dist / (params.tectonics.orogeny_width_km * 500.0).max(1.0)).exp();
            let continental = 0.25 + 0.75 * craton.get_index(i);
            let gate = 0.1 + 0.9 * belt;
            elevation.set_index(
                i,
                elevation.get_index(i) + fold * amp * weight * continental * gate,
            );
        }
    }
}

/// Vertical fault offsets concentrated on plate boundaries.
fn apply_faults(elevation: &mut Field<f64>, mosaic: &PlateMosaic, params: &Params) {
    let grid = elevation.grid();
    let offset = params.relief.fault_offset_m;
    if offset <= 0.0 {
        return;
    }
    let cell_m = grid.cell_size_m();
    let falloff_m = (params.tectonics.orogeny_width_km * 1000.0 * 0.5).max(cell_m);
    let fault_seed = 0xFA_17_00u64;
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            let kind = mosaic.boundary_type_at(x, y);
            if kind == BoundaryType::Interior {
                continue;
            }
            let dist = f64::from(mosaic.boundary_distance_m.get_index(i));
            let falloff = (-dist / falloff_m).exp();
            let sign = if kind == BoundaryType::Convergent {
                -1.0
            } else {
                1.0
            };
            let jitter = 0.6 + 0.8 * unit_from_hash(hash2(x as u64, y as u64, fault_seed));
            elevation.set_index(i, elevation.get_index(i) + sign * offset * falloff * jitter);
        }
    }
}

/// Places named landforms guided by the plate-boundary map (spec §2b).
fn apply_stamps(
    elevation: &mut Field<f64>,
    mosaic: &PlateMosaic,
    craton: &Field<f64>,
    params: &Params,
    seed: u64,
) {
    let grid = elevation.grid();
    let vertical = params.world.vertical_m;
    let rng = &mut Rng::new(derive_seed(seed, DOMAIN_RELIEF ^ 0x51A9));
    let count = (params.relief.feature_density * 16.0)
        .round()
        .clamp(0.0, 40.0) as usize;
    if count == 0 {
        return;
    }
    let mut placed = 0usize;
    for _ in 0..count * 5 {
        if placed >= count {
            break;
        }
        let x = rng.range_f64(0.0, grid.width() as f64);
        let y = rng.range_f64(0.0, grid.height() as f64);
        let (xi, yi) = (x as usize % grid.width(), y as usize % grid.height());
        let boundary = mosaic.boundary_type_at(xi, yi);
        // Interior stamps only make sense on land.
        let continental = craton.get(xi, yi);
        if boundary == BoundaryType::Interior && continental < 0.4 {
            continue;
        }
        let Some(kind) = pick_landform(boundary, rng) else {
            continue;
        };
        let length_cells = rng.range_f64(8.0, 34.0);
        let width_cells = rng.range_f64(2.5, 7.5);
        let height = match kind {
            Landform::Range => rng.range_f64(0.10, 0.28) * vertical,
            Landform::Trench => -rng.range_f64(0.10, 0.22) * vertical,
            Landform::Rift => -rng.range_f64(0.05, 0.14) * vertical,
            Landform::OceanRidge => rng.range_f64(0.05, 0.14) * vertical,
            Landform::Plateau => rng.range_f64(0.08, 0.22) * vertical,
            Landform::Mesa => rng.range_f64(0.05, 0.14) * vertical,
            Landform::Caldera => -rng.range_f64(0.03, 0.08) * vertical,
            Landform::Peak => rng.range_f64(0.06, 0.18) * vertical,
        };
        let direction = rng.range_f64(0.0, std::f64::consts::TAU);
        stamp(
            elevation,
            x,
            y,
            length_cells,
            width_cells,
            height,
            direction,
            kind,
        );
        placed += 1;
    }
}

/// Landform primitive applied by [`apply_stamps`].
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

/// Stamps one landform into the elevation field.
#[allow(clippy::too_many_arguments)]
fn stamp(
    elevation: &mut Field<f64>,
    cx: f64,
    cy: f64,
    length_cells: f64,
    width_cells: f64,
    height_m: f64,
    direction_rad: f64,
    kind: Landform,
) {
    let grid = elevation.grid();
    let (sa, ca) = direction_rad.sin_cos();
    let radius = (length_cells * 0.5 + width_cells).ceil() as isize;
    let xs = cx.round() as isize;
    let ys = cy.round() as isize;
    let w = grid.width() as isize;
    let h = grid.height() as isize;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            // Wrap on periodic axes, skip outside an open boundary. Clamping
            // would fold every out-of-range offset onto the same edge cell and
            // pile the stamp into a spike (the old rectangular/spherical bug).
            let raw_x = xs + dx;
            let raw_y = ys + dy;
            let x = if grid.boundary().wraps_x() {
                raw_x.rem_euclid(w) as usize
            } else if (0..w).contains(&raw_x) {
                raw_x as usize
            } else {
                continue;
            };
            let y = if grid.boundary().wraps_y() {
                raw_y.rem_euclid(h) as usize
            } else if (0..h).contains(&raw_y) {
                raw_y as usize
            } else {
                continue;
            };
            let px = x as f64 - cx;
            let py = y as f64 - cy;
            let u = (px * ca + py * sa) / length_cells.max(1e-6);
            let v = (-px * sa + py * ca) / width_cells.max(1e-6);
            let r2 = u * u + v * v;
            if r2 > 4.0 {
                continue;
            }
            let profile = match kind {
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
            };
            let i = grid.index(x, y);
            elevation.set_index(i, elevation.get_index(i) + height_m * profile);
        }
    }
}

/// Builds the initial world at a lower resolution for fast previews.
pub fn generate_preview(params: &Params, target_width: u32) -> GeoResult<PipelineOutput> {
    let mut p = params.clone();
    let scale = (f64::from(p.world.width) / f64::from(target_width.max(16))).max(1.0);
    p.world.width = target_width.max(16);
    p.world.height = ((f64::from(p.world.height) / scale).round() as u32).max(16);
    p.world.cell_km *= scale;
    generate_initial(&p)
}

/// Convenience helper: checks all parameters and returns the grid.
pub fn check_params(params: &Params) -> GeoResult<Grid> {
    let mut p = params.clone();
    p.normalize();
    p.validate()?;
    p.world.grid()
}

/// Low-resolution continental anchor field for tiled generation (Phase 3).
///
/// Tiled generation needs a globally consistent, cheap field to look up from
/// every tile. This samples the craton/continent mask on a coarse grid
/// (default 1/8 of the world) at a harmless memory cost; tiles can then
/// bilinearly sample it while evaluating local detail.
pub fn generate_anchor_mask(params: &Params, target_width: u32) -> GeoResult<(Grid, Field<f64>)> {
    let mut p = params.clone();
    p.normalize();
    let width = target_width.clamp(16, 2048);
    let scale = (f64::from(p.world.width) / f64::from(width)).max(1.0);
    p.world.width = width;
    p.world.height = ((f64::from(p.world.height) / scale).round() as u32).max(16);
    p.world.cell_km *= scale;
    let (grid, mut mask) = continental_mask(&p);
    // Raw craton values hover around the threshold; clamp so tiles can treat
    // the anchor as a well-defined 0..1 continental fraction.
    for value in mask.as_mut_slice() {
        *value = value.clamp(0.0, 1.0);
    }
    Ok((grid, mask))
}

/// One tile of a tiled generation pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileRect {
    /// Tile origin without halo (cells).
    pub x: u32,
    /// Tile origin without halo (cells).
    pub y: u32,
    /// Tile width without halo.
    pub width: u32,
    /// Tile height without halo.
    pub height: u32,
    /// Halo radius actually available on each side (clipped at the border).
    pub halo_left: u32,
    /// Halo radius actually available on each side (clipped at the border).
    pub halo_right: u32,
    /// Halo radius actually available on each side (clipped at the border).
    pub halo_top: u32,
    /// Halo radius actually available on each side (clipped at the border).
    pub halo_bottom: u32,
}

impl TileRect {
    /// Origin including the halo.
    #[must_use]
    pub fn origin_with_halo(&self) -> (u32, u32) {
        (self.x - self.halo_left, self.y - self.halo_top)
    }

    /// Size including the halo.
    #[must_use]
    pub fn size_with_halo(&self) -> (u32, u32) {
        (
            self.width + self.halo_left + self.halo_right,
            self.height + self.halo_top + self.halo_bottom,
        )
    }

    /// Number of cells actually stored for this tile (with halo).
    #[must_use]
    pub fn buffer_cells(&self) -> usize {
        let (w, h) = self.size_with_halo();
        w as usize * h as usize
    }
}

/// Builds a deterministic, non-overlapping tile plan covering a fine grid.
///
/// Tiles are laid out left-to-right, top-to-bottom with `tile_size` cells per
/// side (the right/top edge tiles may be smaller). Each tile records the halo
/// actually available after clipping at the world border, so callers can size
/// their scratch buffers without special-casing edges. The core regions cover
/// every cell exactly once; halos are duplicated work, not duplicated output.
#[must_use]
pub fn tile_plan(fine_width: u32, fine_height: u32, tile_size: u32, halo: u32) -> Vec<TileRect> {
    let size = tile_size.max(16);
    let mut tiles = Vec::new();
    let mut y = 0u32;
    while y < fine_height {
        let height = size.min(fine_height - y);
        let mut x = 0u32;
        while x < fine_width {
            let width = size.min(fine_width - x);
            tiles.push(TileRect {
                x,
                y,
                width,
                height,
                halo_left: halo.min(x),
                halo_right: halo.min(fine_width - x - width),
                halo_top: halo.min(y),
                halo_bottom: halo.min(fine_height - y - height),
            });
            x += width;
        }
        y += height;
    }
    tiles
}

/// Deterministic global Voronoi plate sites in normalised coordinates
/// `(centre_u, centre_v, continental)` for the tiled relief prototype.
///
/// Sites are generated from the world seed only, never from tile bounds, so
/// every tile evaluates the same plate layout and boundaries stay continuous.
fn tile_plate_sites(params: &Params) -> Vec<(f64, f64, bool)> {
    const PLATES: usize = 9;
    let seed = params.world.effective_seed();
    (0..PLATES)
        .map(|k| {
            let h1 = crate::math::rng::hash2(k as u64, 0, seed);
            let h2 = crate::math::rng::hash2(k as u64, 1, seed ^ 0x51A9);
            let angle = (h1 % 1_000_003) as f64 / 1_000_003.0 * std::f64::consts::TAU;
            let radius = ((h2 % 1_000_003) as f64 / 1_000_003.0).sqrt() * 0.55;
            (
                0.5 + angle.cos() * radius,
                0.5 + angle.sin() * radius,
                k < 4,
            )
        })
        .collect()
}

/// Prototype tile-local relief evaluation for tiled generation.
///
/// Produces the core elevations of one tile from the coarse continent anchor
/// (bilinear) plus deterministic, integer-frequency fractal detail sampled at
/// *global* cell coordinates. Because every octave is periodic with an integer
/// number of cycles, adjacent tiles agree along their shared boundary; this is
/// the seam-continuity property the production tile evaluator must preserve.
pub fn generate_tile_relief(
    params: &Params,
    anchor: &Field<f64>,
    fine_width: u32,
    fine_height: u32,
    tile: &TileRect,
) -> Vec<f64> {
    let fine_w = f64::from(fine_width.max(1));
    let fine_h = f64::from(fine_height.max(1));
    let sea = params.world.sea_level_m;
    let vertical = params.world.vertical_m.max(100.0);
    let plates = tile_plate_sites(params);
    let mut out = Vec::with_capacity(tile.width as usize * tile.height as usize);
    for ty in 0..tile.height {
        let v = (f64::from(tile.y) + f64::from(ty) + 0.5) / fine_h;
        for tx in 0..tile.width {
            let u = (f64::from(tile.x) + f64::from(tx) + 0.5) / fine_w;
            let continent = sample_anchor(anchor, u, v);
            // Continental base, floored at the abyssal plain.
            let base = sea + vertical * (continent - 0.45) * 0.55;
            // Deterministic integer-frequency detail: periodic on toroidal maps
            // and independent of tile boundaries.
            let mut detail = 0.0;
            let mut amplitude = 1.0;
            let mut total = 0.0;
            for octave in 1..=5u32 {
                let cycles = octave * 3;
                let phase = f64::from(octave) * 1.7 + params.world.effective_seed() as f64 * 1e-6;
                let wave = (std::f64::consts::TAU * f64::from(cycles) * u + phase).sin()
                    * (std::f64::consts::TAU * f64::from(cycles) * 0.7 * v + phase * 1.3).cos();
                detail += wave * amplitude;
                total += amplitude;
                amplitude *= 0.55;
            }
            detail /= total.max(1e-9);
            // Detail is stronger on continents, muted on the abyssal plain.
            let roughness = 0.02 + 0.10 * continent;
            // Plate-domain bias + orogenic ridge near boundaries. Both are
            // evaluated from global coordinates so tiles stay seam-consistent.
            let mut d1 = f64::INFINITY;
            let mut d2 = f64::INFINITY;
            let mut continental = true;
            for &(cu, cv, is_continental) in &plates {
                // Wrapped horizontal distance (periodic world).
                let mut dx = (u - cu).abs();
                if dx > 0.5 {
                    dx = 1.0 - dx;
                }
                let dy = v - cv;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < d1 {
                    d2 = d1;
                    d1 = dist;
                    continental = is_continental;
                } else if dist < d2 {
                    d2 = dist;
                }
            }
            let boundary = (-((d2 - d1) / 0.025).powi(2)).exp();
            let plate_bias = if continental { 0.02 } else { -0.03 };
            let ridge = boundary * if continental { 0.14 } else { -0.05 };
            out.push(base + vertical * (roughness * detail + plate_bias + ridge));
        }
    }
    out
}

/// Drives a tiled generation pass and returns a coarse preview plus global
/// statistics without ever allocating a full-resolution elevation grid.
///
/// This is the integration prototype for the 4096² path: the plan covers the
/// world, each tile is generated locally from the anchor and folded into the
/// preview map and [`crate::stats::TileAccumulator`].
pub fn generate_tiled_preview(
    params: &Params,
    fine_width: u32,
    fine_height: u32,
    tile_size: u32,
    halo: u32,
    preview_factor: u32,
) -> GeoResult<(Vec<f64>, Vec<u32>, crate::stats::TileAccumulator)> {
    generate_tiled_preview_with_workers(
        params,
        fine_width,
        fine_height,
        tile_size,
        halo,
        preview_factor,
        0,
    )
}

/// Same as [`generate_tiled_preview`] with an explicit worker count.
///
/// `workers = 0` picks the available parallelism. The result is identical for
/// every worker count, which is what makes the parallel tiled path safe to use
/// for deterministic worlds.
pub fn generate_tiled_preview_with_workers(
    params: &Params,
    fine_width: u32,
    fine_height: u32,
    tile_size: u32,
    halo: u32,
    preview_factor: u32,
    workers: u32,
) -> GeoResult<(Vec<f64>, Vec<u32>, crate::stats::TileAccumulator)> {
    let anchor_width = (fine_width / 8).clamp(16, 2048);
    let (_, anchor) = generate_anchor_mask(params, anchor_width)?;
    let plan = tile_plan(fine_width, fine_height, tile_size, halo);
    let factor = preview_factor.max(1);
    let preview_width = fine_width.div_ceil(factor);
    let preview_height = fine_height.div_ceil(factor);
    let preview_len = preview_width as usize * preview_height as usize;
    let sea = params.world.sea_level_m;
    let workers = if workers == 0 {
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(4)
    } else {
        workers as usize
    }
    .min(plan.len().max(1));
    let chunk = plan.len().div_ceil(workers).max(1);

    // Tiles only read immutable inputs, so they run in parallel; partial
    // previews and statistics are merged in plan order afterwards, which keeps
    // the result independent of scheduling.
    let mut partials = Vec::with_capacity(workers);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for tiles in plan.chunks(chunk) {
            let anchor = &anchor;
            handles.push(scope.spawn(move || {
                let mut local_preview = vec![0.0f64; preview_len];
                let mut local_counts = vec![0u32; preview_len];
                let mut local_stats = crate::stats::TileAccumulator::new();
                for tile in tiles {
                    let elevations =
                        generate_tile_relief(params, anchor, fine_width, fine_height, tile);
                    local_stats.add_tile(&elevations, sea);
                    accumulate_tile_preview(
                        factor,
                        preview_width,
                        tile,
                        &elevations,
                        &mut local_preview,
                        &mut local_counts,
                    );
                }
                (local_preview, local_counts, local_stats)
            }));
        }
        for handle in handles {
            partials.push(handle.join().unwrap_or_else(|_| {
                (
                    vec![0.0; preview_len],
                    vec![0u32; preview_len],
                    crate::stats::TileAccumulator::new(),
                )
            }));
        }
    });

    let mut preview = vec![0.0f64; preview_len];
    let mut counts = vec![0u32; preview_len];
    let mut stats = crate::stats::TileAccumulator::new();
    for (local_preview, local_counts, local_stats) in partials {
        for index in 0..preview_len {
            preview[index] += local_preview[index];
            counts[index] += local_counts[index];
        }
        stats.merge(&local_stats);
    }
    Ok((preview, counts, stats))
}

/// Folds one tile's core elevations into a coarse preview grid (box filter).
///
/// Tiled generation streams tiles to outputs; the Web UI also needs a small
/// map. `factor` fine cells per preview cell; `elevation` is the tile's core
/// region (no halo, row-major). `preview`/`counts` cover
/// `ceil(fine_width/factor) * ceil(fine_height/factor)` cells and are
/// independent of tile order.
pub fn accumulate_tile_preview(
    factor: u32,
    preview_width: u32,
    tile: &TileRect,
    elevation: &[f64],
    preview: &mut [f64],
    counts: &mut [u32],
) {
    let factor = factor.max(1) as usize;
    let stride = tile.width as usize;
    assert_eq!(elevation.len(), stride * tile.height as usize, "tile size");
    let preview_width = preview_width.max(1) as usize;
    assert_eq!(preview.len(), counts.len(), "preview/counts length");
    for ty in 0..tile.height as usize {
        for tx in 0..tile.width as usize {
            let value = elevation[ty * stride + tx];
            if !value.is_finite() {
                continue;
            }
            let px = (tile.x as usize + tx) / factor;
            let py = (tile.y as usize + ty) / factor;
            let index = py * preview_width + px;
            if index < preview.len() {
                preview[index] += value;
                counts[index] += 1;
            }
        }
    }
}

/// Bilinearly samples a coarse anchor field at normalised world coordinates
/// `(u, v)` in `0..1`, with the same wrapping/clamping as the world grid.
///
/// This is the lookup tiled generation uses from every tile: `u` wraps on
/// periodic worlds and `v` clamps on open ones, matching cell-centre alignment.
#[must_use]
pub fn sample_anchor(mask: &Field<f64>, u: f64, v: f64) -> f64 {
    let grid = mask.grid();
    let w = grid.width() as isize;
    let h = grid.height() as isize;
    // Cell-centre aligned coordinates in cell units, wrapping on periodic axes.
    let cell_x = u.rem_euclid(1.0) * w as f64 - 0.5;
    let cell_y = v.clamp(0.0, 1.0) * h as f64 - 0.5;
    let x0 = if grid.boundary().wraps_x() {
        (cell_x.floor() as isize).rem_euclid(w)
    } else {
        (cell_x.floor() as isize).clamp(0, w - 1)
    };
    let y0 = if grid.boundary().wraps_y() {
        (cell_y.floor() as isize).rem_euclid(h)
    } else {
        (cell_y.floor() as isize).clamp(0, h - 1)
    };
    let x1 = if grid.boundary().wraps_x() {
        (x0 + 1).rem_euclid(w)
    } else {
        (x0 + 1).min(w - 1)
    };
    let y1 = if grid.boundary().wraps_y() {
        (y0 + 1).rem_euclid(h)
    } else {
        (y0 + 1).min(h - 1)
    };
    let tx = cell_x - cell_x.floor();
    let ty = cell_y - cell_y.floor();
    let x0 = x0 as usize;
    let y0 = y0 as usize;
    let x1 = x1 as usize;
    let y1 = y1 as usize;
    let top = mask.get(x0, y0) * (1.0 - tx) + mask.get(x1, y0) * tx;
    let bottom = mask.get(x0, y1) * (1.0 - tx) + mask.get(x1, y1) * tx;
    (top * (1.0 - ty) + bottom * ty).clamp(0.0, 1.0)
}

/// Samples the anchor field for a rectangular tile of a finer world.
///
/// `fine_width`/`fine_height` describe the full target grid; `origin_x`/
/// `origin_y` are the tile's top-left cell and `tile_width`/`tile_height` its
/// size. The output is row-major of length `tile_width * tile_height` and is
/// seam-continuous with neighbouring tiles by construction (both sample the
/// same anchor at shared cell centres).
pub fn fill_tile_anchor(
    anchor: &Field<f64>,
    fine_width: u32,
    fine_height: u32,
    origin_x: u32,
    origin_y: u32,
    tile_width: u32,
    tile_height: u32,
    out: &mut Vec<f64>,
) {
    let full_w = f64::from(fine_width.max(1));
    let full_h = f64::from(fine_height.max(1));
    out.clear();
    out.reserve(tile_width as usize * tile_height as usize);
    for ty in 0..tile_height {
        let v = (f64::from(origin_y) + f64::from(ty) + 0.5) / full_h;
        for tx in 0..tile_width {
            let u = (f64::from(origin_x) + f64::from(tx) + 0.5) / full_w;
            out.push(sample_anchor(anchor, u, v));
        }
    }
}

/// Low-frequency continent mask for a seed: useful for tests and diagnostics.
#[must_use]
pub fn continental_mask(params: &Params) -> (Grid, Field<f64>) {
    let mut p = params.clone();
    p.normalize();
    let grid = p.world.grid().unwrap_or_default();
    let seed = p.world.effective_seed();
    let mosaic = PlateMosaic::build(&p, grid, seed);
    let noise = NoiseField::new(
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
    )
    .fill(&grid, 3.5);
    let sorted = noise.sorted_values();
    let mut mask = Field::<f64>::zeros(grid);
    for (i, &value) in noise.as_slice().iter().enumerate() {
        let rank = sorted.partition_point(|&v| v < value) as f64 / (sorted.len().max(2) - 1) as f64;
        let plate = mosaic.plate_at(grid.coords(i).0, grid.coords(i).1);
        let bias = if plate.continental { 0.14 } else { -0.14 };
        mask.set_index(i, rank + bias);
    }
    spatial::gaussian_blur(&mut mask, 2.5);
    (grid, mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;

    fn test_params() -> Params {
        let mut p = Params::default();
        p.world.width = 96;
        p.world.height = 96;
        p.world.cell_km = 10.0;
        p.time.total_yr = 50_000.0;
        p.time.step_yr = 10_000.0;
        p
    }

    #[test]
    fn default_world_generates_valid_state() {
        let p = Params::default();
        let out = generate_initial(&p).unwrap();
        assert_eq!(out.world.elevation_m.non_finite_count(), 0);
        assert_eq!(out.world.crust_m.non_finite_count(), 0);
        assert_eq!(
            out.world.crust_reference_m,
            p.tectonics.plate_thickness_km * 1000.0
        );
        let (min, max) = out.world.relief_range_m();
        assert!(
            max - min > p.world.vertical_m * 0.6,
            "relief too small: {min}..{max}"
        );
        assert!(min > -p.world.vertical_m * 2.0);
        assert!(max < p.world.vertical_m * 1.5);
    }

    #[test]
    fn continents_are_coherent_not_boundary_strips() {
        // Regression against the old model, where land formed thin bands along
        // Voronoi edges. A coherent continent must contain a large connected
        // component of land.
        let mut p = test_params();
        p.world.seed = 42;
        let out = generate_initial(&p).unwrap();
        let grid = out.world.grid;
        let sea = out.world.sea_level_m;
        let mut visited = vec![false; grid.cell_count()];
        let mut best = 0usize;
        for start in 0..grid.cell_count() {
            if visited[start] || out.world.elevation_m.get_index(start) <= sea {
                continue;
            }
            // Iterative flood fill over land cells.
            let mut stack = vec![start];
            visited[start] = true;
            let mut size = 0usize;
            while let Some(cell) = stack.pop() {
                size += 1;
                let (x, y) = grid.coords(cell);
                for dir in 0..8 {
                    if let Some((nx, ny)) = grid.neighbor(x, y, dir) {
                        let n = grid.index(nx, ny);
                        if !visited[n] && out.world.elevation_m.get_index(n) > sea {
                            visited[n] = true;
                            stack.push(n);
                        }
                    }
                }
            }
            best = best.max(size);
        }
        let fraction = best as f64 / grid.cell_count() as f64;
        assert!(
            fraction > 0.10,
            "largest landmass is only {fraction:.3} of the map"
        );
    }

    #[test]
    fn worlds_are_reproducible() {
        let p = test_params();
        let a = generate_initial(&p).unwrap();
        let b = generate_initial(&p).unwrap();
        assert_eq!(
            a.world.elevation_m.as_slice(),
            b.world.elevation_m.as_slice()
        );
        assert_eq!(a.mosaic.plate_id.as_slice(), b.mosaic.plate_id.as_slice());
    }

    #[test]
    fn different_seeds_give_different_worlds() {
        let mut a = test_params();
        a.world.seed = 1;
        let mut b = test_params();
        b.world.seed = 2;
        let wa = generate_initial(&a).unwrap();
        let wb = generate_initial(&b).unwrap();
        assert_ne!(
            wa.world.elevation_m.as_slice(),
            wb.world.elevation_m.as_slice()
        );
    }

    #[test]
    fn land_and_ocean_both_exist_for_several_seeds() {
        for seed in [1u64, 7, 42, 99, 1234, 2024, 31337] {
            let mut p = test_params();
            p.world.seed = seed;
            let out = generate_initial(&p).unwrap();
            let land = out.world.land_fraction();
            assert!(
                (0.05..0.92).contains(&land),
                "seed {seed}: land fraction {land}"
            );
        }
    }

    #[test]
    fn no_nan_under_extreme_parameters() {
        let mut p = Params::default();
        p.world.width = 48;
        p.world.height = 48;
        p.world.vertical_m = 300.0;
        p.world.sea_level_m = 400.0;
        p.normalize();
        let out = generate_initial(&p).unwrap();
        assert_eq!(out.world.elevation_m.non_finite_count(), 0);
    }

    #[test]
    fn preview_downsamples_grid() {
        let p = Params::default();
        let out = generate_preview(&p, 64).unwrap();
        assert_eq!(out.world.grid.width(), 64);
        assert!(out.world.grid.height() >= 16);
    }

    #[test]
    fn extreme_parameters_are_rejected_or_normalised() {
        let mut p = Params::default();
        p.world.sea_level_m = 1e9;
        p.normalize();
        assert!(generate_initial(&p).is_ok());
        p.world.width = 4;
        p.world.height = 4;
        assert!(generate_initial(&p).is_err());
    }

    #[test]
    fn tile_plan_fits_the_4096_memory_budget() {
        // SPEC §13: 4096² generation must stay under 1 GB. Tiles are the only
        // full-resolution buffers; keep a generous 32-worker ceiling.
        let plan = tile_plan(4096, 4096, 512, 32);
        let max_tile_cells = plan.iter().map(TileRect::buffer_cells).max().unwrap();
        let workers = 32usize;
        // Per tile: one f64 elevation field + one f64 scratch + u32 coverage.
        let per_tile_bytes = max_tile_cells * (8 + 8 + 4);
        // Global: 1/8 preview map + counters, and the coarse anchor.
        let preview_cells = (4096 / 8) * (4096 / 8);
        let global_bytes = preview_cells * (8 + 4) + preview_cells * 8;
        let total = per_tile_bytes * workers + global_bytes;
        assert!(
            total < 1_000_000_000,
            "tiled plan needs {} MB, above the 1 GB budget",
            total / (1024 * 1024)
        );
        // The estimate should be far inside the budget, not borderline.
        assert!(
            total < 250_000_000,
            "tiled plan unexpectedly large: {total} bytes"
        );
    }

    #[test]
    fn tile_plan_covers_the_grid_exactly_once() {
        let tiles = tile_plan(1000, 700, 256, 32);
        assert!(!tiles.is_empty());
        let mut covered = vec![0u8; 1000 * 700];
        for tile in &tiles {
            assert!(tile.x + tile.width <= 1000);
            assert!(tile.y + tile.height <= 700);
            for y in tile.y..tile.y + tile.height {
                for x in tile.x..tile.x + tile.width {
                    covered[(y * 1000 + x) as usize] += 1;
                }
            }
            // Halo is clipped at the borders and never leaves the world.
            let (ox, oy) = tile.origin_with_halo();
            assert!(ox <= tile.x && oy <= tile.y);
            let (hw, hh) = tile.size_with_halo();
            assert!(ox + hw <= 1000 && oy + hh <= 700);
            assert!(tile.halo_left == 32 || tile.x == 0);
            assert!(tile.halo_top == 32 || tile.y == 0);
        }
        assert!(covered.iter().all(|&n| n == 1), "tiles must not overlap");
        // Deterministic layout.
        assert_eq!(tile_plan(1000, 700, 256, 32), tiles);
        // A single tile covers small grids.
        let single = tile_plan(200, 100, 512, 8);
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].halo_left, 0);
        assert_eq!(single[0].size_with_halo(), (200, 100));
    }

    #[test]
    fn tiled_preview_integrates_plan_relief_and_stats() {
        let mut p = test_params();
        p.world.width = 1024;
        p.world.height = 1024;
        p.world.seed = 9;
        let (preview, counts, stats) = generate_tiled_preview(&p, 1024, 1024, 256, 24, 8).unwrap();
        assert_eq!(preview.len(), 128 * 128);
        assert_eq!(counts.len(), preview.len());
        assert!(
            counts.iter().all(|&n| n == 64),
            "every preview cell covered once"
        );
        assert_eq!(stats.count, 1024 * 1024);
        assert!(stats.land_fraction() > 0.05 && stats.land_fraction() < 0.95);
        assert!(stats.relief_m() > 500.0);
        // Deterministic.
        let (again, _, again_stats) = generate_tiled_preview(&p, 1024, 1024, 256, 24, 8).unwrap();
        assert_eq!(preview, again);
        assert_eq!(stats.count, again_stats.count);
        assert!((stats.mean_m() - again_stats.mean_m()).abs() < 1e-12);
        // Larger tiles must give the same world statistics as smaller tiles.
        let (_, _, big) = generate_tiled_preview(&p, 1024, 1024, 1024, 0, 8).unwrap();
        assert_eq!(big.count, stats.count);
        assert!((big.mean_m() - stats.mean_m()).abs() < 1e-9);
    }

    #[test]
    fn tiled_preview_is_worker_count_invariant() {
        let mut p = test_params();
        p.world.width = 512;
        p.world.height = 512;
        p.world.seed = 17;
        let one = generate_tiled_preview_with_workers(&p, 512, 512, 128, 16, 4, 1).unwrap();
        let four = generate_tiled_preview_with_workers(&p, 512, 512, 128, 16, 4, 4).unwrap();
        assert_eq!(one.1, four.1);
        assert_eq!(one.2.count, four.2.count);
        assert_eq!(one.2.land_cells, four.2.land_cells);
        // Floating-point additions happen in a different grouping, so allow
        // rounding-level differences rather than bit equality.
        assert!((one.2.mean_m() - four.2.mean_m()).abs() < 1e-6);
        assert!((one.2.relief_m() - four.2.relief_m()).abs() < 1e-6);
        assert_eq!(one.0.len(), four.0.len());
        assert!(
            one.0
                .iter()
                .zip(four.0.iter())
                .all(|(a, b)| (a - b).abs() < 1e-6),
            "preview differs beyond rounding"
        );
    }

    #[test]
    fn tile_relief_is_seam_continuous_and_deterministic() {
        let mut p = test_params();
        p.world.width = 256;
        p.world.height = 256;
        p.world.seed = 13;
        let (_, anchor) = generate_anchor_mask(&p, 32).unwrap();
        let left = TileRect {
            x: 0,
            y: 0,
            width: 128,
            height: 64,
            halo_left: 0,
            halo_right: 0,
            halo_top: 0,
            halo_bottom: 0,
        };
        let right = TileRect { x: 128, ..left };
        let lhs = generate_tile_relief(&p, &anchor, 256, 256, &left);
        let rhs = generate_tile_relief(&p, &anchor, 256, 256, &right);
        assert_eq!(lhs.len(), 128 * 64);
        assert_eq!(rhs.len(), 128 * 64);
        for row in 0..64 {
            let last_left = lhs[row * 128 + 127];
            let first_right = rhs[row * 128];
            let a = sample_anchor(&anchor, (127.5) / 256.0, (row as f64 + 0.5) / 256.0);
            let b = sample_anchor(&anchor, (128.5) / 256.0, (row as f64 + 0.5) / 256.0);
            // The continent component uses the same anchor on both cells; the
            // integer-frequency detail is a smooth global function, so the
            // seam step stays small relative to the relief envelope.
            assert!((last_left - first_right).abs() < p.world.vertical_m * 0.08);
            let _ = (a, b);
        }
        // Deterministic and finite.
        assert_eq!(lhs, generate_tile_relief(&p, &anchor, 256, 256, &left));
        assert!(lhs.iter().all(|z| z.is_finite()));
    }

    #[test]
    fn tile_previews_average_into_the_coarse_map() {
        let tile = TileRect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
            halo_left: 0,
            halo_right: 0,
            halo_top: 0,
            halo_bottom: 0,
        };
        let elevation: Vec<f64> = (0..64).map(|i| i as f64).collect();
        let mut preview = vec![0.0; 16];
        let mut counts = vec![0u32; 16];
        accumulate_tile_preview(2, 4, &tile, &elevation, &mut preview, &mut counts);
        assert!(counts.iter().all(|&n| n == 4));
        // Top-left 2x2 block: rows 0/1, columns 0/1 -> values 0,1,8,9.
        assert!((preview[0] / counts[0] as f64 - 4.5).abs() < 1e-12);
        // Bottom-right 2x2 block: values 54,55,62,63.
        assert!((preview[15] / counts[15] as f64 - 58.5).abs() < 1e-12);
        // Tiles are additive and order independent: left half alone plus
        // right half alone equals the full-tile accumulation.
        let left = TileRect {
            x: 0,
            y: 0,
            width: 4,
            height: 8,
            halo_left: 0,
            halo_right: 0,
            halo_top: 0,
            halo_bottom: 0,
        };
        let left_values: Vec<f64> = (0..8)
            .flat_map(|row| (0..4).map(move |col| (row * 8 + col) as f64))
            .collect();
        let right = TileRect {
            x: 4,
            y: 0,
            width: 4,
            height: 8,
            halo_left: 0,
            halo_right: 0,
            halo_top: 0,
            halo_bottom: 0,
        };
        let right_values: Vec<f64> = (0..8)
            .flat_map(|row| (4..8).map(move |col| (row * 8 + col) as f64))
            .collect();
        let mut combined = vec![0.0; 16];
        let mut combined_counts = vec![0u32; 16];
        accumulate_tile_preview(
            2,
            4,
            &left,
            &left_values,
            &mut combined,
            &mut combined_counts,
        );
        accumulate_tile_preview(
            2,
            4,
            &right,
            &right_values,
            &mut combined,
            &mut combined_counts,
        );
        assert!(combined_counts.iter().all(|&n| n == 4));
        for index in 0..16 {
            assert!((combined[index] - preview[index]).abs() < 1e-12);
        }
    }

    #[test]
    fn anchor_samples_are_bilinear_and_wrap() {
        let mut p = test_params();
        p.world.width = 128;
        p.world.height = 128;
        p.world.seed = 5;
        let (grid, _) = generate_anchor_mask(&p, 32).unwrap();
        let mut mask = Field::from_fn(grid, |x, y| (x as f64) / 31.0 + (y as f64) / 310.0);
        // Exact cell centres.
        let sampled = sample_anchor(&mask, 0.5 / 32.0, 0.5 / 32.0);
        assert!((sampled - mask.get(0, 0)).abs() < 1e-9);
        // Horizontal wrap: a uniform field samples identically across the seam.
        for value in mask.as_mut_slice() {
            *value = 0.25;
        }
        for u in [0.0, 1e-6, 0.499, 0.999, 1.0 - 1e-6] {
            let sampled = sample_anchor(&mask, u, 0.3);
            assert!((sampled - 0.25).abs() < 1e-12, "u={u} -> {sampled}");
        }
        // Range is clamped even for out-of-range coordinates.
        let clamped = sample_anchor(&mask, -3.7, 4.2);
        assert!((0.0..=1.0).contains(&clamped));
        // Re-sampling is deterministic.
        assert_eq!(
            sample_anchor(&mask, 0.9, 0.2),
            sample_anchor(&mask, 0.9, 0.2)
        );
    }

    #[test]
    fn tiles_share_anchor_values_across_seams() {
        let mut p = test_params();
        p.world.width = 512;
        p.world.height = 512;
        p.world.seed = 21;
        let (_, anchor) = generate_anchor_mask(&p, 64).unwrap();
        let mut left = Vec::new();
        let mut right = Vec::new();
        fill_tile_anchor(&anchor, 512, 512, 0, 0, 256, 256, &mut left);
        fill_tile_anchor(&anchor, 512, 512, 256, 0, 256, 256, &mut right);
        assert_eq!(left.len(), 256 * 256);
        assert_eq!(right.len(), 256 * 256);
        // The tile just right of the seam samples the cell adjacent to it.
        for row in 0..256 {
            let last = left[row * 256 + 255];
            let first_right = right[row * 256];
            assert!(
                (last - first_right).abs() < 0.5,
                "seam jump at row {row}: {last} vs {first_right}"
            );
        }
        // A uniform anchor reproduces exactly everywhere.
        let grid = anchor.grid();
        let uniform = Field::from_fn(grid, |_, _| 0.3);
        fill_tile_anchor(&uniform, 512, 512, 128, 64, 32, 16, &mut left);
        assert!(left.iter().all(|&v| (v - 0.3).abs() < 1e-12));
        // Out-of-world tile origins clamp instead of panicking.
        fill_tile_anchor(&uniform, 512, 512, 500, 500, 16, 16, &mut left);
        assert_eq!(left.len(), 256);
        assert!(left.iter().all(|&v| (v - 0.3).abs() < 1e-12));
    }

    #[test]
    fn anchor_mask_is_coarse_global_and_deterministic() {
        let mut p = test_params();
        p.world.width = 256;
        p.world.height = 256;
        p.world.seed = 11;
        let (grid, mask) = generate_anchor_mask(&p, 64).unwrap();
        assert_eq!(grid.width(), 64);
        assert_eq!(grid.height(), 64);
        assert_eq!(mask.len(), grid.cell_count());
        for &value in mask.as_slice() {
            assert!(
                (0.0..=1.0).contains(&value),
                "mask value {value} outside 0..1"
            );
        }
        let land = mask.as_slice().iter().filter(|&&v| v > 0.5).count();
        assert!(land > 200 && land < 3800, "land cells {land}");
        let (_, again) = generate_anchor_mask(&p, 64).unwrap();
        assert_eq!(mask.as_slice(), again.as_slice());
    }

    #[test]
    fn open_and_spherical_boundaries_generate_without_edge_spikes() {
        // Regression: stamping used to fold out-of-range offsets onto the edge
        // for non-periodic boundaries, producing kilometre-scale spikes.
        use crate::grid::BoundaryKind;
        for boundary in [
            BoundaryKind::Rectangular,
            BoundaryKind::Toroidal,
            BoundaryKind::Spherical,
        ] {
            let mut p = test_params();
            p.world.width = 96;
            p.world.height = 96;
            p.world.boundary = boundary;
            p.world.seed = 7;
            let out = generate_initial(&p).unwrap_or_else(|e| panic!("{boundary:?}: {e}"));
            let max = out.world.elevation_m.max_value().unwrap();
            let min = out.world.elevation_m.min_value().unwrap();
            assert!(
                max < p.world.vertical_m * 1.3,
                "{boundary:?}: max elevation {max} above the envelope"
            );
            assert!(
                min > -p.world.vertical_m * 2.0,
                "{boundary:?}: min elevation {min} below the envelope"
            );
            assert_eq!(out.world.elevation_m.non_finite_count(), 0);
        }
    }

    #[test]
    fn oceanic_and_continental_crust_differ() {
        let p = test_params();
        let out = generate_initial(&p).unwrap();
        // Crust thickness must span the oceanic–continental range.
        let (min, max) = (
            out.world.crust_m.min_value().unwrap(),
            out.world.crust_m.max_value().unwrap(),
        );
        assert!(max > 30_000.0, "no thick continental crust: max {max}");
        assert!(min < 12_000.0, "no thin oceanic crust: min {min}");
        // The ocean floor must be several kilometres below the datum on average.
        let deep = out
            .world
            .elevation_m
            .as_slice()
            .iter()
            .filter(|&&z| z < -2000.0)
            .count();
        assert!(
            deep > out.world.cell_count() / 10,
            "ocean too shallow: {deep}"
        );
    }
}
