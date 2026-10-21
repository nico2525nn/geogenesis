//! Plate kinematics and boundary forces (spec §1).
//!
//! The tectonic model is kinematic: plates are Voronoi cells around moving
//! seed points, each with a velocity, crustal thickness and density. The
//! mosaic is rebuilt whenever plates move, which gives moving boundaries,
//! continental collisions, ridges and trenches without solving a full Stokes
//! problem. Vertical rates are derived from the relative motion projected on
//! the boundary normal. Mantle plumes are fixed in the absolute frame and only
//! drift slowly, so a moving plate records a decaying volcanic trail in the
//! hotspot rate field (the kinematic equivalent of the Hawaiian chain).
//!
//! # Physics of the vertical rates
//!
//! Every rate is in metres per year and scaled by the user's mean uplift rate
//! (`uplift_mm_yr`), so the numbers stay in the 0.1–5 mm/yr range that real
//! orogens, trenches and ridges show. Profiles are smooth Gaussians whose
//! widths come from the corresponding parameters (orogen width, trench width,
//! volcanic-arc offset/width, rift width):
//!
//! * **continental collision** — a broad, symmetric orogen belt thickening
//!   both plates;
//! * **oceanic subduction** — a narrow trench on the subducting oceanic plate,
//!   with the volcanic arc on the overriding plate offset inland by
//!   `arc_offset_km`, and a small outer rise further seaward;
//! * **divergence** — a mid-ocean ridge crest (oceanic) or an axial rift
//!   graben flanked by rift shoulders (continental), plus broad ridge flanks;
//! * **transform** — almost no vertical motion (a few percent of the scale at
//!   most).
//!
//! This is the standard "kinematic plate model" approximation used for
//! landscape evolution (e.g. GPlates reconstructions driving Badlands); it is
//! not a mantle-convection solver, and the module documentation says so.
//!
//! Plate boundaries are domain-warped: the Voronoi query point is displaced by
//! a smooth noise vector field so that bisector edges become irregular, curved
//! boundary segments instead of perfectly straight lines. The warp is sampled
//! on a coarse lattice and bilinearly interpolated, which keeps the rebuild
//! cost at `O(cells · plates)` with a small constant.

use crate::grid::{Field, Grid};
use crate::math::noise::{jittered_points, FractalKind, NoiseField, NoiseKind, NoiseParams};
use crate::math::rng::{derive_seed, hash2, unit_from_hash, Rng};
use crate::math::{smoothstep, Vec2};
use crate::params::Params;

/// Domain tag for the plate random stream.
const DOMAIN_PLATES: u64 = 0x706C_6174_6573_0001;
/// Domain tag for hotspot placement.
const DOMAIN_HOTSPOTS: u64 = 0x686F_7473_706F_7473;
/// Domain tag for the boundary domain-warp noise.
const DOMAIN_WARP: u64 = 0x7761_7270_0000_0001;
/// Domain tag for the per-hotspot chain phase.
const DOMAIN_CHAIN: u64 = 0x6368_6169_6E00_0001;

/// Classification of a cell relative to a plate boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryType {
    /// Plate interior.
    Interior = 0,
    /// Plates move towards each other.
    Convergent = 1,
    /// Plates move apart.
    Divergent = 2,
    /// Plates slide past each other.
    Transform = 3,
}

impl BoundaryType {
    /// Converts from the stored `u8`.
    #[must_use]
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Convergent,
            2 => Self::Divergent,
            3 => Self::Transform,
            _ => Self::Interior,
        }
    }

    /// Stored representation.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// True for any boundary type.
    #[must_use]
    pub const fn is_boundary(self) -> bool {
        !matches!(self, Self::Interior)
    }
}

/// One lithospheric plate.
#[derive(Clone, Debug)]
pub struct Plate {
    /// Stable id (index in [`PlateMosaic::plates`]).
    pub id: u16,
    /// Seed point in cell coordinates; the plate owns the cells nearest to it.
    pub centre_cells: Vec2,
    /// Velocity in m/yr.
    pub velocity_m_yr: Vec2,
    /// Crustal thickness in metres.
    pub thickness_m: f64,
    /// Crustal density, kg/m³.
    pub density_kg_m3: f64,
    /// True when the plate carries continental crust.
    pub continental: bool,
    /// Rotation of the plate (rad/yr) so transform boundaries have shear.
    pub angular_velocity: f64,
}

impl Plate {
    /// Mean crustal elevation implied by Airy isostasy relative to the datum,
    /// metres (before any dynamic topography).
    #[must_use]
    pub fn airy_elevation_m(&self, sea_level_m: f64, mantle_density: f64) -> f64 {
        let ratio = (self.density_kg_m3 / mantle_density).clamp(0.1, 0.99);
        sea_level_m + (1.0 - ratio) * (self.thickness_m - 35_000.0) * 0.5
    }
}

/// Aggregate plate-scale statistics (diagnostics, timeline labels, tests).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlateStatistics {
    /// Number of plates.
    pub plate_count: usize,
    /// Share of plates carrying continental crust.
    pub continental_plate_fraction: f64,
    /// Share of the grid covered by continental plates.
    pub continental_area_fraction: f64,
    /// Mean plate speed, m/yr.
    pub mean_speed_m_yr: f64,
    /// Standard deviation of the plate speeds, m/yr.
    pub std_speed_m_yr: f64,
    /// Slowest plate, m/yr.
    pub min_speed_m_yr: f64,
    /// Fastest plate, m/yr.
    pub max_speed_m_yr: f64,
    /// Share of cells classified as boundary (not plate interior).
    pub boundary_cell_fraction: f64,
    /// Total boundary length, km.
    pub boundary_length_km: f64,
    /// Convergent boundary length, km.
    pub convergent_length_km: f64,
    /// Divergent boundary length, km.
    pub divergent_length_km: f64,
    /// Transform boundary length, km.
    pub transform_length_km: f64,
}

/// Plate boundaries, vertical rates and hotspot domes on the grid.
#[derive(Clone, Debug)]
pub struct PlateMosaic {
    /// Geometry.
    pub grid: Grid,
    /// Nearest plate id per cell.
    pub plate_id: Field<u16>,
    /// Boundary classification per cell.
    pub boundary_type: Field<u8>,
    /// Distance to the nearest boundary, metres.
    pub boundary_distance_m: Field<f32>,
    /// Signed relative convergence rate, m/yr (positive converging).
    pub convergence_m_yr: Field<f32>,
    /// Transform shear rate, m/yr.
    pub shear_m_yr: Field<f32>,
    /// Vertical rate from boundary forces, m/yr.
    pub boundary_uplift_m_yr: Field<f32>,
    /// Vertical rate from hotspot swells and trails, m/yr.
    pub hotspot_uplift_m_yr: Field<f32>,
    /// Plate table.
    pub plates: Vec<Plate>,
    /// Hotspot (mantle plume) centres in cell coordinates, fixed frame.
    pub hotspots: Vec<Vec2>,
    /// Absolute drift velocity of each plume, m/yr.
    pub hotspot_velocity_m_yr: Vec<Vec2>,
    /// Number of rebuilds so far (diagnostic).
    pub epoch: u64,
    /// Seed the mosaic was built with; reused for every rebuild so the
    /// boundary warp is a fixed, smoothly evolving field.
    pub seed: u64,
}

impl PlateMosaic {
    /// Initialises plates at random positions and builds the first mosaic.
    #[must_use]
    pub fn build(params: &Params, grid: Grid, seed: u64) -> Self {
        let mut rng = Rng::new(derive_seed(seed, DOMAIN_PLATES));
        let tectonic = &params.tectonics;
        let count = tectonic.plate_count.max(1) as usize;
        let points = jittered_points(&grid, count, rng.next_u64());
        let speed_m_yr = tectonic.plate_speed_m_yr().max(0.002);
        let plates: Vec<Plate> = points
            .iter()
            .enumerate()
            .map(|(i, &(x, y, w))| {
                let continental = (i as f64) < (count as f64) * tectonic.continental_fraction
                    || w < tectonic.continental_fraction;
                // Oceanic crust is thin and dense; continental crust is thick
                // and buoyant. Thickness is jittered around the mean.
                let (thickness_m, density) = if continental {
                    (
                        tectonic.plate_thickness_km * 1000.0 * (0.75 + 0.5 * w),
                        tectonic.density_continental_kg_m3,
                    )
                } else {
                    ((7.0 + 4.0 * w) * 1000.0, tectonic.density_oceanic_kg_m3)
                };
                let angle = rng.range_f64(0.0, std::f64::consts::TAU);
                let speed = speed_m_yr * (0.35 + 1.3 * rng.next_f64());
                Plate {
                    id: i as u16,
                    centre_cells: Vec2::new(x, y),
                    velocity_m_yr: Vec2::from_angle(angle) * speed,
                    thickness_m,
                    density_kg_m3: density,
                    continental,
                    angular_velocity: rng.normal_with(0.0, 1e-8),
                }
            })
            .collect();

        let hotspot_count = tectonic.hotspot_count as usize;
        let mut h_rng = Rng::new(derive_seed(seed, DOMAIN_HOTSPOTS));
        let hotspots: Vec<Vec2> = jittered_points(&grid, hotspot_count, h_rng.next_u64())
            .into_iter()
            .map(|(x, y, _)| Vec2::new(x, y))
            .collect();
        // Mantle plumes drift slowly in the absolute frame; this makes chains
        // curvier than a perfectly fixed plume would.
        let drift = tectonic.hotspot_drift_m_yr();
        let hotspot_velocity_m_yr: Vec<Vec2> = (0..hotspot_count)
            .map(|_| {
                Vec2::from_angle(h_rng.range_f64(0.0, std::f64::consts::TAU))
                    * (drift * h_rng.range_f64(0.4, 1.6))
            })
            .collect();

        let mut mosaic = Self {
            grid,
            plate_id: Field::zeros(grid),
            boundary_type: Field::zeros(grid),
            boundary_distance_m: Field::filled(grid, f32::MAX),
            convergence_m_yr: Field::zeros(grid),
            shear_m_yr: Field::zeros(grid),
            boundary_uplift_m_yr: Field::zeros(grid),
            hotspot_uplift_m_yr: Field::zeros(grid),
            plates,
            hotspots,
            hotspot_velocity_m_yr,
            epoch: 0,
            seed,
        };
        mosaic.rebuild(params, seed);
        mosaic
    }

    /// Moves plates and plumes by `dt_yr`, then rebuilds the mosaic fields.
    ///
    /// Plates move in the map frame; plumes are fixed in the mantle frame and
    /// only drift slowly, which is what makes a moving plate record a volcanic
    /// trail in [`Self::hotspot_uplift_m_yr`].
    pub fn advance(&mut self, params: &Params, dt_yr: f64) {
        if dt_yr <= 0.0 {
            return;
        }
        let grid = self.grid;
        let (w, h) = (grid.width() as f64, grid.height() as f64);
        let cell_m = grid.cell_size_m();
        for plate in &mut self.plates {
            let delta = plate.velocity_m_yr * dt_yr / cell_m;
            plate.centre_cells.x = (plate.centre_cells.x + delta.x).rem_euclid(w);
            plate.centre_cells.y = (plate.centre_cells.y + delta.y).rem_euclid(h);
        }
        for (hotspot, velocity) in self.hotspots.iter_mut().zip(&self.hotspot_velocity_m_yr) {
            let delta = *velocity * dt_yr / cell_m;
            hotspot.x = (hotspot.x + delta.x).rem_euclid(w);
            hotspot.y = (hotspot.y + delta.y).rem_euclid(h);
        }
        self.epoch = self.epoch.wrapping_add(1);
        let seed = self.seed;
        self.rebuild(params, seed);
    }

    /// Nearest plate to a point in cell coordinates.
    #[must_use]
    pub fn nearest_plate(&self, x: f64, y: f64) -> Option<&Plate> {
        let (w, h) = (self.grid.width() as f64, self.grid.height() as f64);
        nearest_index(&self.plate_centres(), x, y, w, h).map(|i| &self.plates[i])
    }

    /// Plate centres as a flat vector (avoided in hot loops; kept small).
    fn plate_centres(&self) -> Vec<Vec2> {
        self.plates.iter().map(|p| p.centre_cells).collect()
    }

    /// Boundary classification at a cell.
    #[must_use]
    pub fn boundary_type_at(&self, x: usize, y: usize) -> BoundaryType {
        BoundaryType::from_u8(self.boundary_type.get(x, y))
    }

    /// Plate owning a cell.
    #[must_use]
    pub fn plate_at(&self, x: usize, y: usize) -> &Plate {
        &self.plates[self.plate_id.get(x, y) as usize]
    }

    /// Fraction of the grid covered by continental plates.
    #[must_use]
    pub fn continental_fraction(&self) -> f64 {
        let mut n = 0usize;
        for &pid in self.plate_id.as_slice() {
            if self.plates[pid as usize].continental {
                n += 1;
            }
        }
        n as f64 / self.grid.cell_count() as f64
    }

    /// Aggregate plate statistics: speed distribution, boundary lengths and
    /// continental coverage. O(cells) and deterministic.
    #[must_use]
    pub fn statistics(&self) -> PlateStatistics {
        let plates = &self.plates;
        let n = plates.len().max(1) as f64;
        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        let mut min_speed = f64::INFINITY;
        let mut max_speed = 0.0f64;
        let mut continental_plates = 0usize;
        for plate in plates {
            let speed = plate.velocity_m_yr.length();
            sum += speed;
            sum_sq += speed * speed;
            min_speed = min_speed.min(speed);
            max_speed = max_speed.max(speed);
            if plate.continental {
                continental_plates += 1;
            }
        }
        if !min_speed.is_finite() {
            min_speed = 0.0;
        }
        let mean = sum / n;
        let variance = (sum_sq / n - mean * mean).max(0.0);
        let continental_cells = self
            .plate_id
            .as_slice()
            .iter()
            .filter(|&&pid| plates[pid as usize].continental)
            .count();

        let grid = self.grid;
        let cell_m = grid.cell_size_m();
        let mut boundary_cells = 0usize;
        let mut lengths = [0.0f64; 4];
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let i = grid.index(x, y);
                let pid = self.plate_id.get_index(i);
                let kind = BoundaryType::from_u8(self.boundary_type.get_index(i));
                if kind.is_boundary() {
                    boundary_cells += 1;
                }
                // Count each cell edge once: east and south neighbours.
                for dir in [4usize, 6] {
                    let Some((nx, ny)) = grid.neighbor(x, y, dir) else {
                        continue;
                    };
                    let j = grid.index(nx, ny);
                    if self.plate_id.get_index(j) == pid {
                        continue;
                    }
                    let other = BoundaryType::from_u8(self.boundary_type.get_index(j));
                    let edge_kind = if kind.is_boundary() { kind } else { other };
                    lengths[edge_kind.as_u8() as usize] += cell_m;
                }
            }
        }
        let cells = grid.cell_count() as f64;
        PlateStatistics {
            plate_count: plates.len(),
            continental_plate_fraction: continental_plates as f64 / n,
            continental_area_fraction: continental_cells as f64 / cells,
            mean_speed_m_yr: mean,
            std_speed_m_yr: variance.sqrt(),
            min_speed_m_yr: min_speed,
            max_speed_m_yr: max_speed,
            boundary_cell_fraction: boundary_cells as f64 / cells,
            boundary_length_km: lengths[1..].iter().sum::<f64>() / 1000.0,
            convergent_length_km: lengths[1] / 1000.0,
            divergent_length_km: lengths[2] / 1000.0,
            transform_length_km: lengths[3] / 1000.0,
        }
    }

    /// Rebuilds every derived field from the current plate and plume
    /// positions. Called on construction and after each [`Self::advance`].
    ///
    /// Cost is `O(cells · plates + cells · hotspots)`: the nearest/second
    /// nearest plate scan, one noise sample pair per cell for the boundary
    /// warp, and a bounded hotspot trail loop.
    pub(crate) fn rebuild(&mut self, params: &Params, seed: u64) {
        let grid = self.grid;
        let tectonic = &params.tectonics;
        let (w, h) = (grid.width() as f64, grid.height() as f64);
        let cell_m = grid.cell_size_m();
        let wrap_x = grid.boundary().wraps_x();
        let wrap_y = grid.boundary().wraps_y();
        let mean_speed = tectonic.plate_speed_m_yr().max(1e-4);
        let conv_threshold = tectonic.convergence_threshold;
        let v_scale = tectonic.vertical_rate_scale_m_yr();
        let max_rate = tectonic.max_vertical_rate_m_yr();
        let n_plates = self.plates.len();

        // Widths in cells of every boundary process.
        let km_to_cells = 1000.0 / cell_m;
        let orogen_sigma = (tectonic.orogeny_width_km * km_to_cells * 1.2).max(1.0);
        let trench_sigma = (tectonic.trench_width_km * km_to_cells * 0.5).max(0.75);
        let arc_offset = (tectonic.arc_offset_km * km_to_cells).max(0.0);
        let arc_sigma = (tectonic.arc_width_km * km_to_cells * 0.6).max(1.0);
        let rift_width = (tectonic.rift_width_km * km_to_cells).max(1.0);
        let rift_axis_sigma = (rift_width * 0.25).max(0.5);
        let rift_flank_offset = (rift_width * 0.9).max(1.0);
        let rift_flank_sigma = (rift_width * 0.55).max(0.6);
        let ridge_crest_sigma = (rift_width * 0.35).max(0.5);
        let influence_cells = (tectonic.boundary_influence_km() * km_to_cells).max(2.0);

        // ---- Domain warp: displace the query point so the Voronoi bisectors
        // become irregular, curved boundary segments. Amplitude scales with
        // the mean plate spacing and is clamped so the warp stays invertible.
        let mean_patch = (grid.cell_count() as f64 / n_plates.max(1) as f64).sqrt();
        let warp_amp = (tectonic.boundary_wiggle * mean_patch * 0.30).clamp(0.0, 0.032 * w.min(h));
        let warp = NoiseField::new(
            NoiseParams {
                kind: NoiseKind::Value,
                fractal: FractalKind::Fbm,
                octaves: 2,
                lacunarity: 2.0,
                persistence: 0.4,
                anisotropy: 0.0,
                direction_deg: 0.0,
            },
            derive_seed(seed, DOMAIN_WARP),
        );
        let warp_cycles_x = 3u32;
        let warp_cycles_y = ((3.0 * h / w).round() as u32).max(1);
        let warp_period_x = if wrap_x { warp_cycles_x } else { 0 };
        let warp_period_y = if wrap_y { warp_cycles_y } else { 0 };
        // The warp field is smooth by construction, so evaluate it on a coarse
        // lattice (about one sample per four cells) and interpolate: this cuts
        // the noise cost by ~an order of magnitude with no visible difference.
        let warp_stride = 4.0;
        let warp_cw = ((w / warp_stride).ceil() as usize).max(2);
        let warp_ch = ((h / warp_stride).ceil() as usize).max(2);
        let warp_eff_x = w / warp_cw as f64;
        let warp_eff_y = h / warp_ch as f64;
        let mut warp_x = vec![0.0f64; warp_cw * warp_ch];
        let mut warp_y = vec![0.0f64; warp_cw * warp_ch];
        if warp_amp > 0.0 {
            for cy in 0..warp_ch {
                let yc = cy as f64 * warp_eff_y;
                let v = yc / h * warp_cycles_y as f64;
                for cx in 0..warp_cw {
                    let xc = cx as f64 * warp_eff_x;
                    let u = xc / w * warp_cycles_x as f64;
                    warp_x[cy * warp_cw + cx] =
                        warp.sample_tiled_xy(u, v, warp_period_x, warp_period_y) * warp_amp;
                    warp_y[cy * warp_cw + cx] =
                        warp.sample_tiled_xy(u + 17.31, v + 9.77, warp_period_x, warp_period_y)
                            * warp_amp;
                }
            }
        }

        // ---- Hotspot trail parameters.
        let hotspot_peak = tectonic.hotspot_peak_rate_m_yr();
        let hotspot_sigma = (tectonic.hotspot_radius_km * km_to_cells * 0.55).max(0.75);
        let trail_tau = tectonic.hotspot_trail_yr();
        let node_wavelength = (hotspot_sigma * 2.2).max(1.0);
        let chain_phase: Vec<f64> = (0..self.hotspots.len())
            .map(|i| unit_from_hash(hash2(i as u64, seed, DOMAIN_CHAIN)) * std::f64::consts::TAU)
            .collect();
        let mut vhat = vec![Vec2::zero(); n_plates];
        let mut speeds = vec![0.0f64; n_plates];
        for (i, plate) in self.plates.iter().enumerate() {
            speeds[i] = plate.velocity_m_yr.length();
            vhat[i] = if speeds[i] > 1e-9 {
                plate.velocity_m_yr / speeds[i]
            } else {
                Vec2::zero()
            };
        }
        let cut_off_radius = hotspot_sigma * 3.5;
        let max_trail_age = trail_tau * 5.0;

        // Smooth taper to zero across the outer three cells of the boundary
        // belt: the classified boundary belt must not end in a rate step.
        let taper_cells = 3.0f64;
        let mut taper = Field::<f32>::filled(grid, 0.0);

        let boundary_type = &mut self.boundary_type;
        let boundary_distance = &mut self.boundary_distance_m;
        let convergence = &mut self.convergence_m_yr;
        let shear = &mut self.shear_m_yr;
        let boundary_uplift = &mut self.boundary_uplift_m_yr;
        let hotspot_uplift = &mut self.hotspot_uplift_m_yr;
        let plate_id = &mut self.plate_id;
        let plates = &self.plates;
        let hotspots = &self.hotspots;

        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let xf0 = x as f64 + 0.5;
                let yf0 = y as f64 + 0.5;
                let (wx, wy) = if warp_amp > 0.0 {
                    // Bilinear lookup in the coarse warp lattice.
                    let fx = (xf0 / warp_eff_x).min(warp_cw as f64 - 1e-9).max(0.0);
                    let fy = (yf0 / warp_eff_y).min(warp_ch as f64 - 1e-9).max(0.0);
                    let ix = fx.floor() as usize;
                    let iy = fy.floor() as usize;
                    let tx = fx - ix as f64;
                    let ty = fy - iy as f64;
                    let ix1 = if wrap_x {
                        (ix + 1) % warp_cw
                    } else {
                        (ix + 1).min(warp_cw - 1)
                    };
                    let iy1 = if wrap_y {
                        (iy + 1) % warp_ch
                    } else {
                        (iy + 1).min(warp_ch - 1)
                    };
                    let row0 = iy * warp_cw;
                    let row1 = iy1 * warp_cw;
                    let lerp = |a: f64, b: f64, c: f64, d: f64| {
                        let top = a + (b - a) * tx;
                        let bottom = c + (d - c) * tx;
                        top + (bottom - top) * ty
                    };
                    (
                        lerp(
                            warp_x[row0 + ix],
                            warp_x[row0 + ix1],
                            warp_x[row1 + ix],
                            warp_x[row1 + ix1],
                        ),
                        lerp(
                            warp_y[row0 + ix],
                            warp_y[row0 + ix1],
                            warp_y[row1 + ix],
                            warp_y[row1 + ix1],
                        ),
                    )
                } else {
                    (0.0, 0.0)
                };
                let xf = xf0 + wx;
                let yf = yf0 + wy;
                let mut best = f64::INFINITY;
                let mut best_id = 0usize;
                let mut second = f64::INFINITY;
                let mut second_id = 0usize;
                for (i, plate) in plates.iter().enumerate() {
                    let dx = axis_delta(plate.centre_cells.x - xf, w, wrap_x);
                    let dy = axis_delta(plate.centre_cells.y - yf, h, wrap_y);
                    let d = dx * dx + dy * dy;
                    if d < best {
                        second = best;
                        second_id = best_id;
                        best = d;
                        best_id = i;
                    } else if d < second {
                        second = d;
                        second_id = i;
                    }
                }
                let idx = grid.index(x, y);
                plate_id.set_index(idx, best_id as u16);

                // Hotspot swells and trails: a plume is fixed in the mantle
                // frame, so plate material downstream of it passed over the
                // plume in the past and keeps a decaying swell. Age increases
                // downstream along the plate velocity, which makes the uplift
                // field an elongated volcanic chain aligned with plate motion.
                let mut hot = 0.0;
                if hotspot_peak > 0.0 && !hotspots.is_empty() {
                    let vel = vhat[best_id];
                    let speed = speeds[best_id];
                    for (hi, centre) in hotspots.iter().enumerate() {
                        let dx = axis_delta(xf - centre.x, w, wrap_x) * cell_m;
                        let dy = axis_delta(yf - centre.y, h, wrap_y) * cell_m;
                        let radius2 = dx * dx + dy * dy;
                        if radius2 > cut_off_radius * cut_off_radius * cell_m * cell_m {
                            continue;
                        }
                        let along = dx * vel.x + dy * vel.y;
                        let cross_x = dx - vel.x * along;
                        let cross_y = dy - vel.y * along;
                        let cross = (cross_x * cross_x + cross_y * cross_y).sqrt();
                        let sigma_m = hotspot_sigma * cell_m;
                        let value = if along <= 0.0 {
                            // Ahead of the plume: only the plume head dome.
                            hotspot_peak * (-0.5 * radius2 / (sigma_m * sigma_m)).exp()
                        } else {
                            // Behind the plume: a swell that ages and decays
                            // downstream, with nodal volcanic centres.
                            let age = if speed > 1e-9 {
                                along / speed
                            } else {
                                max_trail_age
                            };
                            if age > max_trail_age || cross > cut_off_radius * cell_m {
                                continue;
                            }
                            let decay = (-age / trail_tau).exp();
                            let cross_prof = (-0.5 * cross * cross / (sigma_m * sigma_m)).exp();
                            let ramp = (along / (0.5 * node_wavelength * cell_m)).min(1.0);
                            let node_phase = along / (node_wavelength * cell_m)
                                * std::f64::consts::TAU
                                + chain_phase[hi];
                            let node = 1.0 - 0.45 * ramp * (1.0 - node_phase.cos());
                            hotspot_peak * decay * cross_prof * node
                        };
                        hot += value;
                    }
                    if hot > hotspot_peak * 2.0 {
                        hot = hotspot_peak * 2.0;
                    }
                }
                hotspot_uplift.set_index(idx, hot as f32);

                if n_plates < 2 || !second.is_finite() {
                    boundary_type.set_index(idx, BoundaryType::Interior.as_u8());
                    boundary_distance.set_index(idx, f32::MAX);
                    boundary_uplift.set_index(idx, 0.0);
                    continue;
                }
                // The boundary lies halfway between the two nearest seeds in
                // the warped metric.
                let gap = (second.sqrt() - best.sqrt()).max(0.0);
                let dist_cells = gap * 0.5;
                boundary_distance.set_index(idx, (dist_cells * cell_m) as f32);

                let p1 = &plates[best_id];
                let p2 = &plates[second_id];
                let dx = axis_delta(p2.centre_cells.x - p1.centre_cells.x, w, wrap_x);
                let dy = axis_delta(p2.centre_cells.y - p1.centre_cells.y, h, wrap_y);
                let normal = Vec2::new(dx, dy).normalized();
                let dv = p1.velocity_m_yr - p2.velocity_m_yr;
                let conv = dv.dot(normal); // > 0: p1 moves towards p2
                let rate_ratio = conv.abs() / mean_speed;
                let strength = smoothstep(0.0, 1.5, rate_ratio);
                let kind = if rate_ratio > conv_threshold {
                    if conv > 0.0 {
                        BoundaryType::Convergent
                    } else {
                        BoundaryType::Divergent
                    }
                } else {
                    BoundaryType::Transform
                };
                convergence.set_index(idx, conv as f32);
                let shear_rate = (dv - normal * conv).length();
                shear.set_index(idx, shear_rate as f32);

                let taper_value = ((influence_cells - dist_cells) / taper_cells).clamp(0.0, 1.0);
                taper.set_index(
                    idx,
                    (taper_value * taper_value * (3.0 - 2.0 * taper_value)) as f32,
                );
                if !kind.is_boundary() || dist_cells > influence_cells {
                    boundary_type.set_index(idx, BoundaryType::Interior.as_u8());
                    boundary_uplift.set_index(idx, 0.0);
                    continue;
                }
                boundary_type.set_index(idx, kind.as_u8());
                let mut rate = match kind {
                    BoundaryType::Convergent => convergent_rate(
                        p1,
                        p2,
                        dist_cells,
                        strength,
                        v_scale,
                        orogen_sigma,
                        trench_sigma,
                        arc_offset,
                        arc_sigma,
                    ),
                    BoundaryType::Divergent => divergent_rate(
                        p1,
                        p2,
                        dist_cells,
                        strength,
                        v_scale,
                        rift_axis_sigma,
                        rift_flank_offset,
                        rift_flank_sigma,
                        ridge_crest_sigma,
                    ),
                    BoundaryType::Transform => {
                        // Transpression/transtension along a transform fault
                        // produces only minor vertical motion, but not none: a
                        // small ridge or trough follows the shear fabric.
                        let shear_ratio = (shear_rate / mean_speed).clamp(0.0, 2.0);
                        conv.signum()
                            * v_scale
                            * 0.04
                            * shear_ratio
                            * gaussian(dist_cells, orogen_sigma * 0.8)
                    }
                    BoundaryType::Interior => 0.0,
                };
                if !rate.is_finite() {
                    rate = 0.0;
                }
                let tapered = rate.clamp(-max_rate, max_rate) * f64::from(taper_value as f32);
                boundary_uplift.set_index(idx, tapered as f32);
            }
        }

        // Smooth the rate field so triple junctions, polarity switches and the
        // boundary/influence edge do not produce single-cell steps, then apply
        // the belt taper again so plate interiors carry exactly zero force.
        crate::spatial::gaussian_blur(boundary_uplift, 1.5);
        for i in 0..grid.cell_count() {
            let factor = f64::from(taper.get_index(i));
            let rate = f64::from(boundary_uplift.get_index(i)) * factor;
            boundary_uplift.set_index(i, rate as f32);
        }
    }
}

/// Peak vertical rate at a convergent boundary, m/yr.
///
/// Continental collision thickens both plates over a broad belt. Where one
/// plate is oceanic, the oceanic plate subducts: the subducting side gets a
/// narrow trench and a small outer rise, the overriding side a volcanic arc
/// offset inland (`arc_offset`) plus a gentle back-arc downwarp.
#[allow(clippy::too_many_arguments)]
fn convergent_rate(
    p1: &Plate,
    p2: &Plate,
    dist: f64,
    strength: f64,
    v_scale: f64,
    orogen_sigma: f64,
    trench_sigma: f64,
    arc_offset: f64,
    arc_sigma: f64,
) -> f64 {
    if p1.continental && p2.continental {
        // Collision: broad symmetric orogenic belt.
        return v_scale * (0.35 + 0.55 * strength) * gaussian(dist, orogen_sigma);
    }
    // Subduction polarity: an oceanic plate subducts under a continental one;
    // between two oceanic plates the thinner (younger, weaker) plate subducts.
    let p1_subducts = if p1.continental != p2.continental {
        !p1.continental
    } else if (p1.thickness_m - p2.thickness_m).abs() > 1.0 {
        p1.thickness_m < p2.thickness_m
    } else {
        p1.id < p2.id
    };
    if p1_subducts {
        // Trench on the oceanic side, close to the boundary, plus the outer
        // rise that flexes up a few trench widths seaward.
        let trench = -v_scale * (0.40 + 0.40 * strength) * gaussian(dist, trench_sigma);
        let outer_rise = v_scale * 0.08 * gaussian(dist - 4.0 * trench_sigma, 2.0 * trench_sigma);
        trench + outer_rise
    } else {
        // Volcanic arc on the overriding plate.
        let island_arc = if p1.continental { 1.0 } else { 0.7 };
        let arc = v_scale
            * (0.25 + 0.35 * strength)
            * island_arc
            * gaussian(dist - arc_offset, arc_sigma);
        // Back-arc downwarp beyond the arc on continental overriding plates.
        let back_arc = if p1.continental {
            -v_scale * 0.10 * gaussian(dist - (arc_offset + 2.5 * arc_sigma), 1.8 * arc_sigma)
        } else {
            0.0
        };
        arc + back_arc
    }
}

/// Peak vertical rate at a divergent boundary, m/yr.
///
/// Oceanic divergence forms a mid-ocean ridge crest with broad flanks.
/// Continental divergence forms an axial rift graben flanked by uplifted rift
/// shoulders, the classic rift-valley profile.
#[allow(clippy::too_many_arguments)]
fn divergent_rate(
    p1: &Plate,
    p2: &Plate,
    dist: f64,
    strength: f64,
    v_scale: f64,
    rift_axis_sigma: f64,
    rift_flank_offset: f64,
    rift_flank_sigma: f64,
    ridge_crest_sigma: f64,
) -> f64 {
    if p1.continental && p2.continental {
        let graben = -v_scale * (0.18 + 0.15 * strength) * gaussian(dist, rift_axis_sigma);
        let shoulders = v_scale * 0.15 * gaussian(dist - rift_flank_offset, rift_flank_sigma);
        graben + shoulders
    } else {
        let crest = v_scale * (0.15 + 0.10 * strength) * gaussian(dist, ridge_crest_sigma);
        let flanks =
            v_scale * 0.08 * gaussian(dist - 2.5 * ridge_crest_sigma, 2.5 * ridge_crest_sigma);
        crest + flanks
    }
}

/// Smooth unit Gaussian profile.
#[inline]
fn gaussian(distance: f64, sigma: f64) -> f64 {
    let t = distance / sigma.max(1e-6);
    (-0.5 * t * t).exp()
}

/// Wrapped (or open) coordinate difference on one axis.
///
/// Hot-loop version: the inputs are always within about one period of each
/// other, so a single comparison replaces the expensive `rem_euclid` used by
/// [`wrapped_delta`].
#[inline]
fn axis_delta(delta: f64, period: f64, wraps: bool) -> f64 {
    if !wraps {
        return delta;
    }
    let half = period * 0.5;
    if delta > half {
        delta - period
    } else if delta < -half {
        delta + period
    } else {
        delta
    }
}

/// Index of the nearest centre to `(x, y)` on a wrapped grid.
fn nearest_index(centres: &[Vec2], x: f64, y: f64, w: f64, h: f64) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, c) in centres.iter().enumerate() {
        let dx = wrapped_delta(c.x - x, w);
        let dy = wrapped_delta(c.y - y, h);
        let d = dx * dx + dy * dy;
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// Wrapped coordinate difference on a periodic axis.
#[inline]
fn wrapped_delta(delta: f64, period: f64) -> f64 {
    let mut d = delta.rem_euclid(period);
    if d > period * 0.5 {
        d -= period;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BoundaryKind;
    use crate::params::Params;

    fn params() -> Params {
        let mut p = Params::default();
        p.world.width = 64;
        p.world.height = 64;
        p.world.cell_km = 10.0;
        p
    }

    fn grid_of(p: &Params) -> Grid {
        p.world.grid().unwrap()
    }

    /// Replaces the random plate table by an explicit, hand-built layout so a
    /// test can control the boundary geometry and polarity.
    fn controlled_mosaic(p: &Params, plates: Vec<Plate>, hotspots: Vec<Vec2>) -> PlateMosaic {
        let grid = grid_of(p);
        let mut m = PlateMosaic::build(p, grid, 42);
        m.plates = plates;
        m.hotspots = hotspots;
        m.hotspot_velocity_m_yr = vec![Vec2::zero(); m.hotspots.len()];
        m.rebuild(p, 42);
        m
    }

    fn plate(id: u16, x: f64, y: f64, vx: f64, continental: bool) -> Plate {
        Plate {
            id,
            centre_cells: Vec2::new(x, y),
            velocity_m_yr: Vec2::new(vx, 0.0),
            thickness_m: if continental { 35_000.0 } else { 8_000.0 },
            density_kg_m3: if continental { 2800.0 } else { 3050.0 },
            continental,
            angular_velocity: 0.0,
        }
    }

    /// Mean hotspot rate on a horizontal band of the grid.
    fn band_mean(field: &Field<f32>, y: usize, x0: usize, x1: usize) -> f64 {
        let mut sum = 0.0;
        let mut n = 0usize;
        for x in x0..x1 {
            sum += f64::from(field.get(x, y));
            n += 1;
        }
        sum / n.max(1) as f64
    }

    #[test]
    fn mosaic_covers_every_cell() {
        let p = params();
        let grid = grid_of(&p);
        let m = PlateMosaic::build(&p, grid, p.world.effective_seed());
        assert_eq!(m.plate_id.len(), grid.cell_count());
        for &pid in m.plate_id.as_slice() {
            assert!((pid as usize) < m.plates.len());
        }
        assert!(m.boundary_type.as_slice().iter().all(|&t| t <= 3));
    }

    #[test]
    fn boundaries_exist_and_are_classified() {
        let p = params();
        let grid = grid_of(&p);
        let m = PlateMosaic::build(&p, grid, 42);
        let counts = [
            m.boundary_type
                .as_slice()
                .iter()
                .filter(|&&t| t == 1)
                .count(),
            m.boundary_type
                .as_slice()
                .iter()
                .filter(|&&t| t == 2)
                .count(),
            m.boundary_type
                .as_slice()
                .iter()
                .filter(|&&t| t == 3)
                .count(),
        ];
        assert!(counts.iter().sum::<usize>() > 0, "no boundaries generated");
        assert!(m
            .boundary_distance_m
            .as_slice()
            .iter()
            .all(|v| v.is_finite() || *v == f32::MAX));
    }

    #[test]
    fn interiors_are_quiet_and_only_boundaries_move_the_surface() {
        let mut p = params();
        p.world.width = 256;
        p.world.height = 256;
        let grid = grid_of(&p);
        let m = PlateMosaic::build(&p, grid, 7);
        let mut interior = 0usize;
        let influence = p.tectonics.boundary_influence_km() * 1000.0;
        for i in 0..grid.cell_count() {
            match BoundaryType::from_u8(m.boundary_type.get_index(i)) {
                BoundaryType::Interior => {
                    interior += 1;
                    assert_eq!(m.boundary_uplift_m_yr.get_index(i), 0.0);
                    assert!(
                        f64::from(m.boundary_distance_m.get_index(i)) > influence,
                        "interior cell is inside the boundary belt"
                    );
                }
                _ => {
                    assert!(
                        f64::from(m.boundary_distance_m.get_index(i)) <= influence + 1.0,
                        "boundary cell is outside the boundary belt"
                    );
                }
            }
        }
        let fraction = interior as f64 / grid.cell_count() as f64;
        assert!(
            (0.001..=0.95).contains(&fraction),
            "interior fraction {fraction} is implausible"
        );
    }

    #[test]
    fn hotspot_domes_raise_the_rate() {
        let mut p = params();
        p.tectonics.hotspot_count = 4;
        p.tectonics.hotspot_strength_m = 2000.0;
        let grid = grid_of(&p);
        let m = PlateMosaic::build(&p, grid, 7);
        let max = m.hotspot_uplift_m_yr.max_value().unwrap();
        assert!(max > 0.0);
        assert!(max <= 2.0 * p.tectonics.hotspot_peak_rate_m_yr() + 1e-12);
    }

    #[test]
    fn hotspot_trails_follow_the_plate_motion() {
        let mut p = params();
        p.world.width = 160;
        p.world.height = 64;
        p.world.boundary = BoundaryKind::Rectangular;
        p.tectonics.plate_count = 1;
        p.tectonics.hotspot_count = 0;
        p.tectonics.hotspot_strength_m = 3000.0;
        p.tectonics.hotspot_trail_myr = 8.0;
        // One eastward-moving plate with a plume at the centre.
        let m = controlled_mosaic(
            &p,
            vec![plate(0, 64.0, 32.0, 0.06, true)],
            vec![Vec2::new(64.0, 32.0)],
        );
        let y = 32;
        let near_down = band_mean(&m.hotspot_uplift_m_yr, y, 66, 78);
        let far_down = band_mean(&m.hotspot_uplift_m_yr, y, 100, 130);
        let upwind = band_mean(&m.hotspot_uplift_m_yr, y, 20, 50);
        assert!(
            near_down > 5.0 * upwind,
            "trail does not extend downstream: near {near_down}, upwind {upwind}"
        );
        assert!(
            far_down < near_down * 0.75,
            "trail does not decay with age: near {near_down}, far {far_down}"
        );
        assert!(
            m.hotspot_uplift_m_yr.max_value().unwrap()
                <= 2.0 * p.tectonics.hotspot_peak_rate_m_yr() + 1e-12
        );

        // The plume sits in the mantle frame: after a million years the plate
        // has moved but the plume has barely drifted.
        let mut advanced = m.clone();
        advanced.advance(&p, 1.0e6);
        let dx = advanced.hotspots[0].x - 64.0;
        assert!(dx.abs() < 2.0, "plume drifted {dx} cells in 1 Myr");
        assert!(
            advanced.plates[0].centre_cells.x - 64.0 > 3.0,
            "plate did not move"
        );
    }

    #[test]
    fn continental_collision_builds_a_broad_symmetric_orogen() {
        let mut p = params();
        p.world.width = 128;
        p.world.height = 64;
        p.world.boundary = BoundaryKind::Rectangular;
        p.tectonics.plate_count = 2;
        p.tectonics.hotspot_count = 0;
        let m = controlled_mosaic(
            &p,
            vec![
                plate(0, 32.0, 32.0, 0.03, true),
                plate(1, 96.0, 32.0, -0.03, true),
            ],
            vec![],
        );
        let y = 32;
        let boundary_rate = f64::from(m.boundary_uplift_m_yr.get(64, y));
        assert!(boundary_rate > 0.0, "collision boundary does not rise");
        // The belt must be broad: still clearly rising one orogen width away.
        let one_width = (p.tectonics.orogeny_width_km / p.world.cell_km).round() as usize;
        let shoulder = f64::from(m.boundary_uplift_m_yr.get(64 - one_width, y));
        assert!(
            shoulder > 0.15 * boundary_rate,
            "orogen is too narrow: boundary {boundary_rate}, shoulder {shoulder}"
        );
        // And no deep trench on a purely continental collision.
        let min = m.boundary_uplift_m_yr.min_value().unwrap();
        assert!(
            min > -0.25 * boundary_rate,
            "collision created a trench: {min}"
        );
    }

    #[test]
    fn subduction_creates_a_trench_and_an_offset_volcanic_arc() {
        let mut p = params();
        p.world.width = 128;
        p.world.height = 64;
        p.world.boundary = BoundaryKind::Rectangular;
        p.tectonics.plate_count = 2;
        p.tectonics.hotspot_count = 0;
        p.tectonics.uplift_mm_yr = 2.0;
        p.tectonics.arc_offset_km = 120.0;
        p.tectonics.trench_width_km = 40.0;
        // Continental plate in the west moving east; oceanic plate in the east
        // moving west: the oceanic plate subducts eastward, so the trench is
        // on its side and the arc is on the continental side.
        let m = controlled_mosaic(
            &p,
            vec![
                plate(0, 32.0, 32.0, 0.03, true),
                plate(1, 96.0, 32.0, -0.03, false),
            ],
            vec![],
        );
        let y = 32;
        let cell_km = p.world.cell_km;
        let offset_cells = (p.tectonics.arc_offset_km / cell_km) as usize;
        // Trench: strong negative rate on the oceanic side within a few cells.
        let mut trench = 0.0f64;
        let mut trench_distance = 0usize;
        for x in 64..76 {
            let rate = f64::from(m.boundary_uplift_m_yr.get(x, y));
            if rate < trench {
                trench = rate;
                trench_distance = x - 64;
            }
        }
        assert!(trench < -0.5e-3, "no subduction trench: {trench} m/yr");
        assert!(
            trench_distance < offset_cells,
            "trench is not at the boundary"
        );
        // Arc: positive bump on the continental side around the offset.
        let mut arc = 0.0f64;
        let mut arc_x = 0usize;
        for x in 36..64 {
            let rate = f64::from(m.boundary_uplift_m_yr.get(x, y));
            if rate > arc {
                arc = rate;
                arc_x = 64 - x;
            }
        }
        assert!(arc > 0.3e-3, "no volcanic arc: {arc} m/yr");
        assert!(
            (offset_cells.saturating_sub(4)..=offset_cells + 6).contains(&arc_x),
            "arc sits {arc_x} cells inland, expected about {offset_cells}"
        );
        // No arc on the oceanic side (it is the down-going plate), only the
        // much smaller flexural outer rise further seaward.
        assert!(
            f64::from(m.boundary_uplift_m_yr.get(64 + offset_cells, y)) < 0.35 * arc,
            "volcanic arc on the subducting plate"
        );
    }

    #[test]
    fn divergence_makes_a_ridge_or_a_rift_valley() {
        let mut p = params();
        p.world.width = 128;
        p.world.height = 64;
        p.world.boundary = BoundaryKind::Rectangular;
        p.tectonics.plate_count = 2;
        p.tectonics.hotspot_count = 0;
        // Oceanic plate pair moving apart: mid-ocean ridge (positive crest).
        let oceanic = controlled_mosaic(
            &p,
            vec![
                plate(0, 32.0, 32.0, -0.03, false),
                plate(1, 96.0, 32.0, 0.03, false),
            ],
            vec![],
        );
        let ridge = f64::from(oceanic.boundary_uplift_m_yr.get(64, 32));
        assert!(ridge > 0.0, "no mid-ocean ridge: {ridge}");
        assert!(
            f64::from(oceanic.boundary_uplift_m_yr.get(70, 32)) > 0.0,
            "ridge flanks are not rising"
        );
        // Continental plate pair moving apart: axial graben with shoulders.
        let continental = controlled_mosaic(
            &p,
            vec![
                plate(0, 32.0, 32.0, -0.03, true),
                plate(1, 96.0, 32.0, 0.03, true),
            ],
            vec![],
        );
        let axis = f64::from(continental.boundary_uplift_m_yr.get(64, 32));
        assert!(axis < 0.0, "continental rift has no axial graben: {axis}");
        let mut shoulder = 0.0f64;
        for x in 66..84 {
            shoulder = shoulder.max(f64::from(continental.boundary_uplift_m_yr.get(x, 32)));
        }
        assert!(shoulder > 0.0, "continental rift has no uplifted shoulders");
    }

    #[test]
    fn transform_boundaries_have_little_vertical_motion() {
        let mut p = params();
        p.world.width = 128;
        p.world.height = 64;
        p.world.boundary = BoundaryKind::Rectangular;
        p.tectonics.plate_count = 2;
        p.tectonics.hotspot_count = 0;
        // Two plates sliding past each other: the relative motion is parallel
        // to the boundary normal is zero, so the boundary is a transform.
        let m = controlled_mosaic(
            &p,
            vec![
                plate(0, 32.0, 32.0, 0.0, true),
                plate(1, 96.0, 32.0, 0.0, true),
            ],
            vec![],
        );
        // Rebuild used convergent velocities; replace with a pure shear pair.
        let mut shear = m;
        shear.plates[0].velocity_m_yr = Vec2::new(0.0, 0.03);
        shear.plates[1].velocity_m_yr = Vec2::new(0.0, -0.03);
        shear.rebuild(&p, 42);
        let max_rate = shear.boundary_uplift_m_yr.max_value().unwrap();
        let min_rate = shear.boundary_uplift_m_yr.min_value().unwrap();
        let magnitude = max_rate.abs().max(min_rate.abs());
        assert!(
            magnitude > 0.0,
            "transform boundary should still have a little vertical motion"
        );
        assert!(
            magnitude < 0.25e-3,
            "transform boundary moves vertically too fast: {magnitude} m/yr"
        );
        // But the boundary really is classified as transform somewhere.
        assert!(shear
            .boundary_type
            .as_slice()
            .iter()
            .any(|&t| t == BoundaryType::Transform.as_u8()));
    }

    #[test]
    fn rates_are_smooth_and_bounded_by_the_physical_ceiling() {
        let p = params();
        let grid = grid_of(&p);
        let m = PlateMosaic::build(&p, grid, 11);
        let cap = p.tectonics.max_vertical_rate_m_yr() as f32;
        let mut max_jump = 0.0f32;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let rate = m.boundary_uplift_m_yr.get(x, y);
                assert!(rate.is_finite());
                assert!(rate.abs() <= cap + 1e-9, "rate {rate} exceeds {cap}");
                if let Some((nx, ny)) = grid.neighbor(x, y, 4) {
                    let jump = (rate - m.boundary_uplift_m_yr.get(nx, ny)).abs();
                    max_jump = max_jump.max(jump);
                }
            }
        }
        assert!(
            max_jump < 1.0e-3,
            "boundary rate field has an unsmoothed jump of {max_jump} m/yr"
        );
    }

    #[test]
    fn classification_matches_relative_motion() {
        let p = params();
        let grid = grid_of(&p);
        let m = PlateMosaic::build(&p, grid, 5);
        let mean_speed = p.tectonics.plate_speed_m_yr().max(1e-4);
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let i = grid.index(x, y);
                let kind = BoundaryType::from_u8(m.boundary_type.get_index(i));
                let conv = f64::from(m.convergence_m_yr.get_index(i));
                match kind {
                    BoundaryType::Convergent => assert!(conv > 0.0, "convergent cell has {conv}"),
                    BoundaryType::Divergent => assert!(conv < 0.0, "divergent cell has {conv}"),
                    BoundaryType::Transform => {
                        assert!(conv.abs() <= mean_speed * p.tectonics.convergence_threshold + 1e-9)
                    }
                    BoundaryType::Interior => {}
                }
                let shear = f64::from(m.shear_m_yr.get_index(i));
                assert!(shear >= 0.0 && shear.is_finite());
            }
        }
    }

    #[test]
    fn statistics_describe_the_plate_mosaic() {
        let p = params();
        let grid = grid_of(&p);
        let m = PlateMosaic::build(&p, grid, 3);
        let stats = m.statistics();
        assert_eq!(stats.plate_count, p.tectonics.plate_count as usize);
        assert!((0.0..=1.0).contains(&stats.continental_plate_fraction));
        assert!((0.0..=1.0).contains(&stats.continental_area_fraction));
        assert!((stats.continental_area_fraction - m.continental_fraction()).abs() < 1e-12);
        assert!(stats.mean_speed_m_yr > 0.0);
        assert!(stats.std_speed_m_yr >= 0.0);
        assert!(stats.min_speed_m_yr <= stats.mean_speed_m_yr);
        assert!(stats.max_speed_m_yr >= stats.mean_speed_m_yr);
        assert!((0.0..=1.0).contains(&stats.boundary_cell_fraction));
        assert!(stats.boundary_length_km > 0.0);
        let typed =
            stats.convergent_length_km + stats.divergent_length_km + stats.transform_length_km;
        assert!((typed - stats.boundary_length_km).abs() < 1e-6);
        // Deterministic: the same seed and parameters give the same numbers.
        let again = PlateMosaic::build(&p, grid_of(&p), 3).statistics();
        assert_eq!(stats, again);
    }

    #[test]
    fn plate_speed_statistics_follow_the_parameter() {
        let mut slow = params();
        slow.tectonics.plate_motion_cm_yr = 2.0;
        let mut fast = slow.clone();
        fast.tectonics.plate_motion_cm_yr = 8.0;
        let a = PlateMosaic::build(&slow, grid_of(&slow), 9).statistics();
        let b = PlateMosaic::build(&fast, grid_of(&fast), 9).statistics();
        let ratio = b.mean_speed_m_yr / a.mean_speed_m_yr;
        assert!(
            (ratio - 4.0).abs() < 0.2,
            "mean speed does not scale with the parameter: ratio {ratio}"
        );
        assert!(
            a.std_speed_m_yr > 0.0,
            "every plate has exactly the same speed"
        );
    }

    /// Mean absolute displacement of a plate-pair boundary from the straight
    /// Voronoi bisector between the two seed points, in cells.
    ///
    /// Straight Voronoi edges score only the pixel staircase (a fraction of a
    /// cell). Domain warping displaces the boundary away from the bisector, so
    /// the score grows with the warp amplitude.
    fn boundary_warp_cells(m: &PlateMosaic) -> f64 {
        let grid = m.grid;
        let (w, h) = (grid.width() as f64, grid.height() as f64);
        let wrap_x = grid.boundary().wraps_x();
        let wrap_y = grid.boundary().wraps_y();
        let n = m.plates.len();
        let mut total = 0.0;
        let mut count = 0usize;
        for a in 0..n {
            for b in 0..n {
                if a == b {
                    continue;
                }
                let ca = m.plates[a].centre_cells;
                let cb = m.plates[b].centre_cells;
                let dx = axis_delta(cb.x - ca.x, w, wrap_x);
                let dy = axis_delta(cb.y - ca.y, h, wrap_y);
                let len = (dx * dx + dy * dy).sqrt();
                if len < 1e-9 {
                    continue;
                }
                let (nx, ny) = (dx / len, dy / len);
                let mid = Vec2::new(
                    (ca.x + 0.5 * dx).rem_euclid(w),
                    (ca.y + 0.5 * dy).rem_euclid(h),
                );
                let mut pair_cells = 0usize;
                for y in 0..grid.height() {
                    for x in 0..grid.width() {
                        let i = grid.index(x, y);
                        if m.plate_id.get_index(i) != a as u16 {
                            continue;
                        }
                        let adjacent = [4usize, 6, 3, 1].iter().any(|&dir| {
                            grid.neighbor(x, y, dir)
                                .is_some_and(|(nx, ny)| m.plate_id.get(nx, ny) == b as u16)
                        });
                        if !adjacent {
                            continue;
                        }
                        let px = axis_delta(x as f64 + 0.5 - mid.x, w, wrap_x);
                        let py = axis_delta(y as f64 + 0.5 - mid.y, h, wrap_y);
                        total += (px * nx + py * ny).abs();
                        pair_cells += 1;
                    }
                }
                count += pair_cells;
            }
        }
        total / count.max(1) as f64
    }

    #[test]
    fn boundaries_are_not_long_straight_voronoi_edges() {
        let mut straight = params();
        straight.world.width = 128;
        straight.world.height = 128;
        // An open rectangular domain keeps each plate pair's boundary a single
        // convex segment, which makes the straightness metric unambiguous.
        straight.world.boundary = BoundaryKind::Rectangular;
        straight.tectonics.plate_count = 8;
        straight.tectonics.hotspot_count = 0;
        straight.tectonics.boundary_wiggle = 0.0;
        let mut wiggly = straight.clone();
        wiggly.tectonics.boundary_wiggle = 0.7;
        let ms = PlateMosaic::build(&straight, grid_of(&straight), 21);
        let mw = PlateMosaic::build(&wiggly, grid_of(&wiggly), 21);
        let s = boundary_warp_cells(&ms);
        let w = boundary_warp_cells(&mw);
        assert!(
            s < 0.75,
            "unwarped Voronoi boundaries are not straight: {s}"
        );
        assert!(
            w > 2.0 * s && w > 0.5,
            "boundary wiggle did not bend the edges: straight {s}, wiggly {w}"
        );
        // And the two geometries really are different maps.
        let changed = ms
            .plate_id
            .as_slice()
            .iter()
            .zip(mw.plate_id.as_slice())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            changed > ms.grid.cell_count() / 100,
            "wiggle changed only {changed} cells"
        );
    }

    #[test]
    fn plates_move_and_stay_inside_the_grid() {
        let p = params();
        let grid = grid_of(&p);
        let mut m = PlateMosaic::build(&p, grid, 3);
        let before: Vec<Vec2> = m.plates.iter().map(|p| p.centre_cells).collect();
        m.advance(&p, 100_000.0);
        assert!(m
            .plates
            .iter()
            .zip(before.iter())
            .any(|(a, b)| (a.centre_cells - *b).length() > 1e-6));
        for plate in &m.plates {
            assert!((0.0..64.0).contains(&plate.centre_cells.x));
            assert!((0.0..64.0).contains(&plate.centre_cells.y));
        }
        // Advancing changes the rates: boundaries moved.
        assert!(m
            .boundary_uplift_m_yr
            .as_slice()
            .iter()
            .any(|v| v.abs() > 0.0));
    }

    #[test]
    fn rebuild_is_deterministic() {
        let p = params();
        let grid = grid_of(&p);
        let a = PlateMosaic::build(&p, grid, 11);
        let b = PlateMosaic::build(&p, grid_of(&p), 11);
        assert_eq!(a.plate_id.as_slice(), b.plate_id.as_slice());
        assert_eq!(
            a.boundary_uplift_m_yr.as_slice(),
            b.boundary_uplift_m_yr.as_slice()
        );
        assert_eq!(
            a.hotspot_uplift_m_yr.as_slice(),
            b.hotspot_uplift_m_yr.as_slice()
        );
        let mut aa = a.clone();
        let mut bb = b;
        aa.advance(&p, 250_000.0);
        bb.advance(&p, 250_000.0);
        assert_eq!(
            aa.boundary_uplift_m_yr.as_slice(),
            bb.boundary_uplift_m_yr.as_slice()
        );
        assert_eq!(
            aa.hotspot_uplift_m_yr.as_slice(),
            bb.hotspot_uplift_m_yr.as_slice()
        );
    }
}
