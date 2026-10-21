//! Grid geometry and dense scalar fields.
//!
//! A [`Grid`] is the immutable geometry of the world: size in cells, cell size
//! in metres, and boundary topology (open rectangle, torus, or sphere in
//! equirectangular layout). A [`Field`] is a dense row-major array tied to a
//! grid; all simulation state that varies in space lives in fields.
//!
//! Values are kept in whatever precision the process needs: elevation, crust
//! thickness and discharge use `f64`; climate, sediment, soil and display
//! layers use `f32`; categorical layers use `u8`/`u16`. The generic wrapper
//! keeps the index arithmetic and statistics in one place.
//!
//! # Indexing
//!
//! Fields are row-major, `index = y * width + x`. Hot loops iterate `y` then
//! `x`; accessors are bounds-checked in debug builds and branch-free in
//! release.

use crate::error::{GeoError, GeoResult};
use crate::math::clamp;

/// Smallest supported grid width or height (spec §2 Phase 0).
pub const MIN_GRID_DIM: u32 = 16;
/// Largest supported grid width or height (spec §2 Phase 0).
pub const MAX_GRID_DIM: u32 = 16_384;

/// The eight D8 neighbour offsets, ordered North-West to South-East.
pub const D8: [(isize, isize); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// The opposite of each [`D8`] direction.
pub const OPPOSITE_D8: [usize; 8] = [7, 6, 5, 4, 3, 2, 1, 0];

/// Distance multiplier for a D8 step: `1` for orthogonal, `sqrt(2)` diagonal.
#[inline]
#[must_use]
pub fn d8_distance_factor(dir: usize) -> f64 {
    if dir >= OPPOSITE_D8.len() {
        return 1.0;
    }
    if D8[dir].0 != 0 && D8[dir].1 != 0 {
        std::f64::consts::SQRT_2
    } else {
        1.0
    }
}

/// How the edges of the grid connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoundaryKind {
    /// Open rectangle: flow and diffusion see an edge and stop.
    Rectangular,
    /// Periodic in both axes (a flat torus).
    Toroidal,
    /// Periodic in longitude, closed at the poles (equirectangular sphere).
    Spherical,
}

impl BoundaryKind {
    /// Stable key used in configuration and JSON.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Rectangular => "rectangular",
            Self::Toroidal => "toroidal",
            Self::Spherical => "spherical",
        }
    }

    /// Human label for the UI.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rectangular => "Rectangular",
            Self::Toroidal => "Toroidal (wrap both axes)",
            Self::Spherical => "Spherical (wrap longitude)",
        }
    }

    /// Parses a key; unknown values are an error for callers that validate.
    #[must_use]
    pub fn parse(key: &str) -> Option<Self> {
        match key {
            "rectangular" => Some(Self::Rectangular),
            "toroidal" => Some(Self::Toroidal),
            "spherical" => Some(Self::Spherical),
            _ => None,
        }
    }

    /// Whether the first axis wraps.
    #[inline]
    #[must_use]
    pub const fn wraps_x(self) -> bool {
        matches!(self, Self::Toroidal | Self::Spherical)
    }

    /// Whether the second axis wraps.
    #[inline]
    #[must_use]
    pub const fn wraps_y(self) -> bool {
        matches!(self, Self::Toroidal)
    }
}

/// Immutable geometry of the simulation domain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    width: u32,
    height: u32,
    cell_size_m: f64,
    boundary: BoundaryKind,
    /// Planet radius for spherical area corrections, metres.
    ///
    /// `None` on planar grids and on spherical grids that were built without a
    /// radius; in that case every cell keeps the planar area `cell_size_m²`,
    /// which is the historical behaviour. [`Grid::cell_area_m2_at`] and
    /// [`Grid::total_area_m2`] switch to true spherical areas as soon as a
    /// radius is present.
    planet_radius_m: Option<f64>,
}

impl Grid {
    /// Builds a grid, validating the spec bounds.
    pub fn new(
        width: u32,
        height: u32,
        cell_size_m: f64,
        boundary: BoundaryKind,
    ) -> GeoResult<Self> {
        if !(MIN_GRID_DIM..=MAX_GRID_DIM).contains(&width) {
            return Err(GeoError::InvalidGrid(format!(
                "width {width} outside {MIN_GRID_DIM}..={MAX_GRID_DIM}"
            )));
        }
        if !(MIN_GRID_DIM..=MAX_GRID_DIM).contains(&height) {
            return Err(GeoError::InvalidGrid(format!(
                "height {height} outside {MIN_GRID_DIM}..={MAX_GRID_DIM}"
            )));
        }
        if !cell_size_m.is_finite() || cell_size_m <= 0.0 {
            return Err(GeoError::InvalidGrid(format!(
                "cell size {cell_size_m} m must be positive and finite"
            )));
        }
        Ok(Self {
            width,
            height,
            cell_size_m,
            boundary,
            planet_radius_m: None,
        })
    }

    /// Builds a grid without validating, for tests and internal reconstruction.
    #[must_use]
    pub const fn unchecked(
        width: u32,
        height: u32,
        cell_size_m: f64,
        boundary: BoundaryKind,
    ) -> Self {
        Self {
            width,
            height,
            cell_size_m,
            boundary,
            planet_radius_m: None,
        }
    }

    /// Attaches a planet radius (metres) for spherical area corrections.
    ///
    /// The radius is only *used* when the boundary is
    /// [`BoundaryKind::Spherical`]; on planar grids it is stored but ignored.
    /// Returns an error when the radius is not a positive finite number.
    pub fn with_planet_radius(self, radius_m: f64) -> GeoResult<Self> {
        if !radius_m.is_finite() || radius_m <= 0.0 {
            return Err(GeoError::InvalidGrid(format!(
                "planet radius {radius_m} m must be positive and finite"
            )));
        }
        Ok(Self {
            planet_radius_m: Some(radius_m),
            ..self
        })
    }

    /// Planet radius in metres, when one was attached.
    #[inline]
    #[must_use]
    pub const fn planet_radius_m(self) -> Option<f64> {
        self.planet_radius_m
    }

    /// Whether per-cell areas vary because this is a spherical world with a
    /// known radius.
    ///
    /// When this is `false`, [`Self::cell_area_m2_at`] equals
    /// [`Self::cell_area_m2`] for every cell.
    #[inline]
    #[must_use]
    pub const fn has_spherical_areas(self) -> bool {
        self.planet_radius_m.is_some() && matches!(self.boundary, BoundaryKind::Spherical)
    }

    /// Width in cells.
    #[inline]
    #[must_use]
    pub const fn width(self) -> usize {
        self.width as usize
    }

    /// Height in cells.
    #[inline]
    #[must_use]
    pub const fn height(self) -> usize {
        self.height as usize
    }

    /// Number of cells.
    #[inline]
    #[must_use]
    pub const fn cell_count(self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    /// Cell edge length in metres.
    #[inline]
    #[must_use]
    pub const fn cell_size_m(self) -> f64 {
        self.cell_size_m
    }

    /// Planar cell area in square metres: `cell_size_m²`.
    ///
    /// This is the *nominal* cell area. On a spherical grid the physical area
    /// varies with latitude; use [`Self::cell_area_m2_at`] (or
    /// [`Self::total_area_m2`]) for volume and area-weighted quantities. The
    /// planar value stays available for display scaling, extents and the
    /// historical, non-spherical code paths.
    #[inline]
    #[must_use]
    pub fn cell_area_m2(self) -> f64 {
        self.cell_size_m * self.cell_size_m
    }

    /// Physical area of cell `(x, y)` in square metres.
    ///
    /// * [`BoundaryKind::Rectangular`] / [`BoundaryKind::Toroidal`], or a
    ///   spherical grid without a radius: `cell_size_m²` (unchanged).
    /// * [`BoundaryKind::Spherical`] with a radius `R`: the exact area of the
    ///   latitude band the cell covers in an equirectangular layout,
    ///
    ///   `A(y) = R² Δλ (sin φ_hi - sin φ_lo)`
    ///
    ///   with `Δλ = 2π / width`, `φ_lo = y π / height - π/2` and
    ///   `φ_hi = (y + 1) π / height - π/2`. Because
    ///   `sin φ_hi - sin φ_lo = 2 sin(Δφ/2) cos φ_c` for the cell-centre
    ///   latitude `φ_c = (y + 0.5) π/height - π/2`, the area is *exactly*
    ///   proportional to `cos(latitude)`: equatorial cells are the largest and
    ///   polar cells the smallest, with `A(y) / A(y_ref) = cos φ_c / cos φ_ref`.
    ///   Summed over the grid the areas add up to `4πR²` (see
    ///   [`Self::total_area_m2`]).
    ///
    /// `x` does not enter the equirectangular area, but it is part of the
    /// signature so callers can address a cell directly and so a future
    /// equal-area layout can use it.
    #[inline]
    #[must_use]
    pub fn cell_area_m2_at(self, x: usize, y: usize) -> f64 {
        debug_assert!(x < self.width());
        debug_assert!(y < self.height());
        let Some(radius) = self
            .planet_radius_m
            .filter(|_| matches!(self.boundary, BoundaryKind::Spherical))
        else {
            return self.cell_area_m2();
        };
        // Exact latitude-band area. `sin(a + b) - sin(a - b) = 2 sin b cos a`
        // is used instead of subtracting two sines so the result stays exactly
        // proportional to cos(cell-centre latitude) at any grid height.
        let height = f64::from(self.height);
        let half_band = 0.5 * std::f64::consts::PI / height;
        let centre_lat =
            (y as f64 + 0.5) * std::f64::consts::PI / height - std::f64::consts::FRAC_PI_2;
        let dsin = 2.0 * half_band.sin() * centre_lat.cos();
        radius * radius * std::f64::consts::TAU / f64::from(self.width) * dsin
    }

    /// Physical area of the cell at a row-major index.
    ///
    /// Convenience wrapper around [`Self::cell_area_m2_at`] for hot loops that
    /// already carry an index; it is `cell_size_m²` on planar grids and the
    /// latitude-band area on spherical grids.
    #[inline]
    #[must_use]
    pub fn cell_area_m2_at_index(self, index: usize) -> f64 {
        if self.has_spherical_areas() {
            let (x, y) = self.coords(index);
            self.cell_area_m2_at(x, y)
        } else {
            self.cell_area_m2()
        }
    }

    /// Total physical area of the domain in square metres.
    ///
    /// * rectangular/toroidal (or spherical without a radius):
    ///   `width * height * cell_size_m²`,
    /// * spherical with a radius `R`: `4πR²`, whether the grid is coarse or
    ///   fine — the per-cell band areas are defined so that they sum to
    ///   exactly the sphere area.
    #[must_use]
    pub fn total_area_m2(self) -> f64 {
        if self.has_spherical_areas() {
            let radius = self
                .planet_radius_m
                .expect("checked by has_spherical_areas");
            4.0 * std::f64::consts::PI * radius * radius
        } else {
            self.cell_count() as f64 * self.cell_area_m2()
        }
    }

    /// Domain width in metres.
    #[inline]
    #[must_use]
    pub fn extent_x_m(self) -> f64 {
        f64::from(self.width) * self.cell_size_m
    }

    /// Domain height in metres.
    #[inline]
    #[must_use]
    pub fn extent_y_m(self) -> f64 {
        f64::from(self.height) * self.cell_size_m
    }

    /// Boundary topology.
    #[inline]
    #[must_use]
    pub const fn boundary(self) -> BoundaryKind {
        self.boundary
    }

    /// Row-major index of cell `(x, y)`; coordinates must be in range.
    #[inline]
    #[must_use]
    pub fn index(self, x: usize, y: usize) -> usize {
        debug_assert!(x < self.width());
        debug_assert!(y < self.height());
        y * self.width() + x
    }

    /// Coordinates of a row-major index.
    #[inline]
    #[must_use]
    pub fn coords(self, index: usize) -> (usize, usize) {
        debug_assert!(index < self.cell_count());
        (index % self.width(), index / self.width())
    }

    /// Wraps/clamps an x coordinate into range according to the boundary.
    #[inline]
    #[must_use]
    pub fn clamp_x(self, x: isize) -> usize {
        let w = self.width() as isize;
        if self.boundary.wraps_x() {
            x.rem_euclid(w) as usize
        } else {
            x.clamp(0, w - 1) as usize
        }
    }

    /// Wraps/clamps a y coordinate into range according to the boundary.
    #[inline]
    #[must_use]
    pub fn clamp_y(self, y: isize) -> usize {
        let h = self.height() as isize;
        if self.boundary.wraps_y() {
            y.rem_euclid(h) as usize
        } else {
            y.clamp(0, h - 1) as usize
        }
    }

    /// Neighbour of `(x, y)` in direction `dir` (see [`D8`]).
    ///
    /// Returns `None` when the neighbour falls outside an open boundary.
    #[inline]
    #[must_use]
    pub fn neighbor(self, x: usize, y: usize, dir: usize) -> Option<(usize, usize)> {
        let (dx, dy) = D8[dir];
        let nx = x as isize + dx;
        let ny = y as isize + dy;
        let inside_x = self.boundary.wraps_x() || (0..self.width() as isize).contains(&nx);
        let inside_y = self.boundary.wraps_y() || (0..self.height() as isize).contains(&ny);
        if inside_x && inside_y {
            Some((
                if self.boundary.wraps_x() {
                    nx.rem_euclid(self.width() as isize) as usize
                } else {
                    nx as usize
                },
                if self.boundary.wraps_y() {
                    ny.rem_euclid(self.height() as isize) as usize
                } else {
                    ny as usize
                },
            ))
        } else {
            None
        }
    }

    /// Neighbour index of `index` in direction `dir`.
    #[inline]
    #[must_use]
    pub fn neighbor_index(self, index: usize, dir: usize) -> Option<usize> {
        let (x, y) = self.coords(index);
        self.neighbor(x, y, dir).map(|(nx, ny)| self.index(nx, ny))
    }

    /// Ground distance in metres of a D8 step.
    #[inline]
    #[must_use]
    pub fn step_distance_m(self, dir: usize) -> f64 {
        self.cell_size_m * d8_distance_factor(dir)
    }

    /// Applies `f(x, y)` to every cell in row-major order.
    #[inline]
    pub fn for_each_cell(self, mut f: impl FnMut(usize, usize, usize)) {
        for y in 0..self.height() {
            for x in 0..self.width() {
                f(self.index(x, y), x, y);
            }
        }
    }

    /// Checks that a claimed size matches this grid.
    #[must_use]
    pub fn matches(self, width: usize, height: usize) -> bool {
        self.width() == width && self.height() == height
    }
}

impl Default for Grid {
    fn default() -> Self {
        Self::unchecked(256, 256, 1000.0, BoundaryKind::Toroidal)
    }
}

/// Scalar element types a [`Field`] can hold.
///
/// Implemented for the concrete numeric types the engine uses; the trait keeps
/// statistics, quantiles and conversions generic without external crates.
pub trait FieldScalar: Copy + Default + PartialOrd + core::fmt::Debug {
    /// Additive identity.
    const ZERO: Self;
    /// Converts from `f64`, rounding/saturating as appropriate.
    #[must_use]
    fn from_f64(v: f64) -> Self;
    /// Widens to `f64` for statistics.
    #[must_use]
    fn to_f64(self) -> f64;
}

impl FieldScalar for f64 {
    const ZERO: Self = 0.0;
    #[inline]
    fn from_f64(v: f64) -> Self {
        v
    }
    #[inline]
    fn to_f64(self) -> f64 {
        self
    }
}

impl FieldScalar for f32 {
    const ZERO: Self = 0.0;
    #[inline]
    fn from_f64(v: f64) -> Self {
        v as Self
    }
    #[inline]
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}

impl FieldScalar for u8 {
    const ZERO: Self = 0;
    #[inline]
    fn from_f64(v: f64) -> Self {
        if v.is_nan() {
            return 0;
        }
        v.round().clamp(0.0, 255.0) as Self
    }
    #[inline]
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}

impl FieldScalar for u16 {
    const ZERO: Self = 0;
    #[inline]
    fn from_f64(v: f64) -> Self {
        if v.is_nan() {
            return 0;
        }
        v.round().clamp(0.0, 65_535.0) as Self
    }
    #[inline]
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}

impl FieldScalar for i32 {
    const ZERO: Self = 0;
    #[inline]
    fn from_f64(v: f64) -> Self {
        if v.is_nan() {
            return 0;
        }
        v.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as Self
    }
    #[inline]
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}

/// A dense row-major scalar field tied to a [`Grid`].
#[derive(Clone, Debug, PartialEq)]
pub struct Field<T: FieldScalar> {
    grid: Grid,
    data: Vec<T>,
}

impl<T: FieldScalar> Field<T> {
    /// Allocates a field initialised to `T::ZERO`.
    #[must_use]
    pub fn zeros(grid: Grid) -> Self {
        Self {
            data: vec![T::ZERO; grid.cell_count()],
            grid,
        }
    }

    /// Allocates a field filled with `value`.
    #[must_use]
    pub fn filled(grid: Grid, value: T) -> Self {
        Self {
            data: vec![value; grid.cell_count()],
            grid,
        }
    }

    /// Builds a field from a function of cell coordinates.
    #[must_use]
    pub fn from_fn(grid: Grid, mut f: impl FnMut(usize, usize) -> T) -> Self {
        let mut data = Vec::with_capacity(grid.cell_count());
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                data.push(f(x, y));
            }
        }
        Self { grid, data }
    }

    /// Wraps an existing buffer, validating its length.
    pub fn from_vec(grid: Grid, data: Vec<T>) -> GeoResult<Self> {
        if data.len() != grid.cell_count() {
            return Err(GeoError::InvalidGrid(format!(
                "field length {} does not match grid {}x{}",
                data.len(),
                grid.width(),
                grid.height()
            )));
        }
        Ok(Self { grid, data })
    }

    /// Geometry of the field.
    #[inline]
    #[must_use]
    pub const fn grid(&self) -> Grid {
        self.grid
    }

    /// Width in cells.
    #[inline]
    #[must_use]
    pub fn width(&self) -> usize {
        self.grid.width()
    }

    /// Height in cells.
    #[inline]
    #[must_use]
    pub fn height(&self) -> usize {
        self.grid.height()
    }

    /// Number of cells.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the field has no cells (never true for a valid grid).
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Raw row-major slice.
    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.data
    }

    /// Mutable raw row-major slice.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data
    }

    /// Consumes the field, returning the raw buffer.
    #[inline]
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        self.data
    }

    /// Iterates the values in row-major order.
    #[inline]
    pub fn iter(&self) -> core::slice::Iter<'_, T> {
        self.data.iter()
    }

    /// Iterates mutably in row-major order.
    #[inline]
    pub fn iter_mut(&mut self) -> core::slice::IterMut<'_, T> {
        self.data.iter_mut()
    }

    /// Value at `(x, y)`. Panics out of range (debug and release).
    #[inline]
    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> T {
        self.data[self.grid.index(x, y)]
    }

    /// Writes `(x, y)`.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, value: T) {
        let i = self.grid.index(x, y);
        self.data[i] = value;
    }

    /// Value at a row-major index.
    #[inline]
    #[must_use]
    pub fn get_index(&self, index: usize) -> T {
        self.data[index]
    }

    /// Writes a row-major index.
    #[inline]
    pub fn set_index(&mut self, index: usize, value: T) {
        self.data[index] = value;
    }

    /// Fills every cell with `value`.
    pub fn fill_value(&mut self, value: T) {
        self.data.fill(value);
    }

    /// Replaces every cell by `f(index, value)`.
    pub fn map_in_place(&mut self, mut f: impl FnMut(usize, T) -> T) {
        for (i, v) in self.data.iter_mut().enumerate() {
            *v = f(i, *v);
        }
    }

    /// Element-wise combination with another field: `self[i] = f(i, self[i], other[i])`.
    ///
    /// Panics if the grids differ.
    pub fn zip_with(&mut self, other: &Self, mut f: impl FnMut(usize, T, T) -> T) {
        assert_eq!(self.grid, other.grid, "zip_with grids differ");
        for (i, (a, b)) in self.data.iter_mut().zip(other.data.iter()).enumerate() {
            *a = f(i, *a, *b);
        }
    }

    /// Adds `other * factor` to every cell.
    pub fn add_scaled(&mut self, other: &Self, factor: f64) {
        assert_eq!(self.grid, other.grid, "add_scaled grids differ");
        for (a, b) in self.data.iter_mut().zip(other.data.iter()) {
            *a = T::from_f64(a.to_f64() + b.to_f64() * factor);
        }
    }

    /// Adds a constant to every cell.
    pub fn add_scalar(&mut self, value: f64) {
        for v in &mut self.data {
            *v = T::from_f64(v.to_f64() + value);
        }
    }

    /// Multiplies every cell by `value`.
    pub fn mul_scalar(&mut self, value: f64) {
        for v in &mut self.data {
            *v = T::from_f64(v.to_f64() * value);
        }
    }

    /// Clamps every cell into `[lo, hi]`.
    pub fn clamp_values(&mut self, lo: f64, hi: f64) {
        for v in &mut self.data {
            *v = T::from_f64(clamp(v.to_f64(), lo, hi));
        }
    }

    /// Minimum value, or `None` for an empty field.
    #[must_use]
    pub fn min_value(&self) -> Option<f64> {
        let mut m = f64::INFINITY;
        for v in &self.data {
            let x = v.to_f64();
            if x < m {
                m = x;
            }
        }
        if m.is_finite() {
            Some(m)
        } else {
            None
        }
    }

    /// Maximum value, or `None` for an empty field.
    #[must_use]
    pub fn max_value(&self) -> Option<f64> {
        let mut m = f64::NEG_INFINITY;
        for v in &self.data {
            let x = v.to_f64();
            if x > m {
                m = x;
            }
        }
        if m.is_finite() {
            Some(m)
        } else {
            None
        }
    }

    /// Arithmetic mean, or `None` for an empty field.
    #[must_use]
    pub fn mean(&self) -> Option<f64> {
        if self.data.is_empty() {
            return None;
        }
        let mut sum = 0.0;
        for v in &self.data {
            sum += v.to_f64();
        }
        Some(sum / self.data.len() as f64)
    }

    /// Sum of all values.
    #[must_use]
    pub fn sum(&self) -> f64 {
        self.data.iter().map(|v| v.to_f64()).sum()
    }

    /// Population standard deviation.
    #[must_use]
    pub fn std_dev(&self) -> Option<f64> {
        let mean = self.mean()?;
        let mut acc = 0.0;
        for v in &self.data {
            let d = v.to_f64() - mean;
            acc += d * d;
        }
        Some((acc / self.data.len() as f64).sqrt())
    }

    /// Copy of the values sorted ascending.
    #[must_use]
    pub fn sorted_values(&self) -> Vec<f64> {
        let mut values: Vec<f64> = self.data.iter().map(|v| v.to_f64()).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        values
    }

    /// Quantile from an already sorted slice (`q` in `[0, 1]`), linear
    /// interpolation between order statistics.
    #[must_use]
    pub fn quantile_sorted(sorted: &[f64], q: f64) -> f64 {
        if sorted.is_empty() {
            return f64::NAN;
        }
        if sorted.len() == 1 {
            return sorted[0];
        }
        let pos = clamp(q, 0.0, 1.0) * (sorted.len() - 1) as f64;
        let lo = pos.floor() as usize;
        let hi = pos.ceil() as usize;
        let t = pos - lo as f64;
        sorted[lo] + (sorted[hi] - sorted[lo]) * t
    }

    /// Quantile `q` in `[0, 1]` (allocates and sorts; use sparingly).
    #[must_use]
    pub fn quantile(&self, q: f64) -> f64 {
        Self::quantile_sorted(&self.sorted_values(), q)
    }

    /// Median value.
    #[must_use]
    pub fn median(&self) -> f64 {
        self.quantile(0.5)
    }

    /// Linearly rescales the field so its current min/max map to
    /// `[new_min, new_max]`. A constant field becomes the midpoint.
    pub fn rescale(&mut self, new_min: f64, new_max: f64) {
        let (Some(lo), Some(hi)) = (self.min_value(), self.max_value()) else {
            return;
        };
        let span = hi - lo;
        if span < 1e-12 {
            self.fill_value(T::from_f64(0.5 * (new_min + new_max)));
            return;
        }
        let scale = (new_max - new_min) / span;
        for v in &mut self.data {
            *v = T::from_f64(new_min + (v.to_f64() - lo) * scale);
        }
    }

    /// Number of non-finite cells (`f32`/`f64` only; integral types are 0).
    #[must_use]
    pub fn non_finite_count(&self) -> usize {
        self.data.iter().filter(|v| !v.to_f64().is_finite()).count()
    }

    /// Replaces non-finite cells with `fallback`; returns how many changed.
    pub fn sanitize_non_finite(&mut self, fallback: f64) -> usize {
        let mut fixed = 0;
        for v in &mut self.data {
            if !v.to_f64().is_finite() {
                *v = T::from_f64(fallback);
                fixed += 1;
            }
        }
        fixed
    }

    /// Counts cells with value strictly greater than `threshold`.
    #[must_use]
    pub fn count_above(&self, threshold: f64) -> usize {
        self.data.iter().filter(|v| v.to_f64() > threshold).count()
    }

    /// Fraction of cells satisfying `predicate`.
    #[must_use]
    pub fn fraction_where(&self, mut predicate: impl FnMut(f64) -> bool) -> f64 {
        if self.data.is_empty() {
            return 0.0;
        }
        let n = self.data.iter().filter(|v| predicate(v.to_f64())).count();
        n as f64 / self.data.len() as f64
    }

    /// Converts to another scalar type.
    #[must_use]
    pub fn map_to<U: FieldScalar>(&self, mut f: impl FnMut(f64) -> U) -> Field<U> {
        Field {
            grid: self.grid,
            data: self.data.iter().map(|v| f(v.to_f64())).collect(),
        }
    }

    /// Bilinear sample at fractional cell coordinates `(x, y)`.
    ///
    /// Coordinates are wrapped or clamped according to the grid boundary, so
    /// the result is always well defined; `x`/`y` may be outside
    /// `[0, width-1]`.
    #[must_use]
    pub fn bilinear(&self, x: f64, y: f64) -> f64 {
        let g = self.grid;
        let w = g.width() as f64;
        let h = g.height() as f64;
        let max_x = w - 1.0;
        let max_y = h - 1.0;
        let x = if g.boundary().wraps_x() {
            x.rem_euclid(w)
        } else {
            clamp(x, 0.0, max_x)
        };
        let y = if g.boundary().wraps_y() {
            y.rem_euclid(h)
        } else {
            clamp(y, 0.0, max_y)
        };
        let x0f = x.floor();
        let y0f = y.floor();
        let fx = x - x0f;
        let fy = y - y0f;
        let x0 = (x0f as usize).min(g.width() - 1);
        let y0 = (y0f as usize).min(g.height() - 1);
        let x1 = if g.boundary().wraps_x() || x0 + 1 < g.width() {
            (x0 + 1) % g.width()
        } else {
            x0
        };
        let y1 = if g.boundary().wraps_y() || y0 + 1 < g.height() {
            (y0 + 1) % g.height()
        } else {
            y0
        };
        let z00 = self.get(x0, y0).to_f64();
        let z10 = self.get(x1, y0).to_f64();
        let z01 = self.get(x0, y1).to_f64();
        let z11 = self.get(x1, y1).to_f64();
        let a = z00 + (z10 - z00) * fx;
        let b = z01 + (z11 - z01) * fx;
        a + (b - a) * fy
    }

    /// Height gradient in metres per metre at `(x, y)`.
    ///
    /// Central differences where both neighbours exist, one-sided at an open
    /// edge; the result is always finite.
    #[must_use]
    pub fn gradient(&self, x: usize, y: usize) -> (f64, f64) {
        let g = self.grid;
        let w = g.width() as isize;
        let h = g.height() as isize;
        let i = g.index(x, y);
        let z = self.data[i].to_f64();
        let (x0, x1) = (
            if g.boundary().wraps_x() || x > 0 {
                Some(g.clamp_x(x as isize - 1))
            } else {
                None
            },
            if g.boundary().wraps_x() || (x as isize + 1) < w {
                Some(g.clamp_x(x as isize + 1))
            } else {
                None
            },
        );
        let (y0, y1) = (
            if g.boundary().wraps_y() || y > 0 {
                Some(g.clamp_y(y as isize - 1))
            } else {
                None
            },
            if g.boundary().wraps_y() || (y as isize + 1) < h {
                Some(g.clamp_y(y as isize + 1))
            } else {
                None
            },
        );
        let dzdx = match (x0, x1) {
            (Some(a), Some(b)) => {
                let count = if g.boundary().wraps_x() {
                    let steps = (x as isize - a as isize).rem_euclid(w)
                        + (b as isize - x as isize).rem_euclid(w);
                    steps.max(1) as f64
                } else {
                    (b as f64 - a as f64).abs().max(1.0)
                };
                (self.get(b, y).to_f64() - self.get(a, y).to_f64()) / (count * g.cell_size_m())
            }
            (Some(a), None) => (z - self.get(a, y).to_f64()) / g.cell_size_m(),
            (None, Some(b)) => (self.get(b, y).to_f64() - z) / g.cell_size_m(),
            (None, None) => 0.0,
        };
        let dzdy = match (y0, y1) {
            (Some(a), Some(b)) => {
                let count = (b as f64 - a as f64).abs().max(1.0);
                (self.get(x, b).to_f64() - self.get(x, a).to_f64()) / (count * g.cell_size_m())
            }
            (Some(a), None) => (z - self.get(x, a).to_f64()) / g.cell_size_m(),
            (None, Some(b)) => (self.get(x, b).to_f64() - z) / g.cell_size_m(),
            (None, None) => 0.0,
        };
        (dzdx, dzdy)
    }

    /// Slope in metres per metre.
    #[must_use]
    pub fn slope(&self, x: usize, y: usize) -> f64 {
        let (gx, gy) = self.gradient(x, y);
        gx.hypot(gy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Grid {
        Grid::new(16, 16, 1000.0, BoundaryKind::Toroidal).unwrap()
    }

    #[test]
    fn grid_index_roundtrip_and_extent() {
        let g = grid();
        for (x, y) in [(0, 0), (15, 0), (3, 5), (15, 15)] {
            assert_eq!(g.coords(g.index(x, y)), (x, y));
        }
        assert_eq!(g.cell_count(), 256);
        assert_eq!(g.extent_x_m(), 16_000.0);
        assert_eq!(g.extent_y_m(), 16_000.0);
        assert_eq!(g.cell_area_m2(), 1.0e6);
    }

    #[test]
    fn toroidal_neighbours_wrap() {
        let g = grid();
        assert_eq!(g.neighbor(0, 0, 0), Some((15, 15)));
        assert_eq!(g.neighbor(15, 15, 7), Some((0, 0)));
        assert_eq!(g.clamp_x(-1), 15);
        assert_eq!(g.clamp_y(-2), 14);
    }

    #[test]
    fn open_boundary_has_no_outside_neighbours() {
        let g = Grid::new(16, 16, 1000.0, BoundaryKind::Rectangular).unwrap();
        assert_eq!(g.neighbor(0, 0, 0), None);
        assert_eq!(g.neighbor(0, 0, 4), Some((1, 0)));
        assert_eq!(g.neighbor(15, 15, 7), None);
        assert_eq!(g.clamp_x(-1), 0);
        assert_eq!(g.clamp_x(99), 15);
    }

    #[test]
    fn grid_rejects_bad_dimensions() {
        assert!(Grid::new(8, 8, 1000.0, BoundaryKind::Rectangular).is_err());
        assert!(Grid::new(17, 16, 1000.0, BoundaryKind::Rectangular).is_ok());
        assert!(Grid::new(16, 16, 0.0, BoundaryKind::Rectangular).is_err());
        assert!(Grid::new(16, 16, f64::NAN, BoundaryKind::Rectangular).is_err());
    }

    #[test]
    fn field_statistics() {
        let g = grid();
        let f = Field::<f64>::from_fn(g, |x, y| (x + y) as f64);
        assert_eq!(f.min_value(), Some(0.0));
        assert_eq!(f.max_value(), Some(30.0));
        assert_eq!(f.mean(), Some(15.0));
        assert!((f.quantile(0.0) - 0.0).abs() < 1e-12);
        assert!((f.quantile(1.0) - 30.0).abs() < 1e-12);
        assert!((f.median() - 15.0).abs() < 1e-12);
    }

    #[test]
    fn rescale_maps_extremes() {
        let g = grid();
        let mut f = Field::<f64>::from_fn(g, |x, _| x as f64);
        f.rescale(-10.0, 10.0);
        assert!((f.min_value().unwrap() - -10.0).abs() < 1e-12);
        assert!((f.max_value().unwrap() - 10.0).abs() < 1e-12);
    }

    #[test]
    fn constant_field_rescale_is_the_midpoint() {
        let g = grid();
        let mut f = Field::<f64>::filled(g, 3.0);
        f.rescale(-2.0, 2.0);
        assert!(f.as_slice().iter().all(|&v| v == 0.0));
    }

    #[test]
    fn bilinear_of_linear_field_is_exact() {
        let g = Grid::new(16, 16, 1.0, BoundaryKind::Rectangular).unwrap();
        let f = Field::<f64>::from_fn(g, |x, y| 2.0 * x as f64 + 3.0 * y as f64);
        for (x, y) in [(0.0, 0.0), (3.5, 2.25), (14.0, 15.0)] {
            let want = 2.0 * x + 3.0 * y;
            assert!((f.bilinear(x, y) - want).abs() < 1e-9, "({x},{y})");
        }
    }

    #[test]
    fn toroidal_bilinear_wraps() {
        let g = Grid::new(16, 16, 1.0, BoundaryKind::Toroidal).unwrap();
        let f = Field::<f64>::from_fn(g, |x, _| if x == 0 { 10.0 } else { 0.0 });
        // Sampling left of x=0 must wrap to the same values as inside the
        // right edge: continuity across the seam is what matters.
        let v = f.bilinear(-0.25, 3.0);
        let w = f.bilinear(15.75, 3.0);
        assert!((v - w).abs() < 1e-12, "{v} vs {w}");
        assert!((0.0..=10.0).contains(&v), "got {v}");
    }

    #[test]
    fn gradient_of_a_plane_is_exact() {
        let g = Grid::new(16, 16, 25.0, BoundaryKind::Rectangular).unwrap();
        let f = Field::<f64>::from_fn(g, |x, y| (x as f64 * 25.0) * 0.1 + (y as f64 * 25.0) * 0.05);
        let (dzdx, dzdy) = f.gradient(8, 8);
        assert!((dzdx - 0.1).abs() < 1e-9);
        assert!((dzdy - 0.05).abs() < 1e-9);
        assert!((f.slope(8, 8) - (0.1f64 * 0.1 + 0.05 * 0.05).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn sanitize_repairs_non_finite() {
        let g = grid();
        let mut f = Field::<f64>::filled(g, 1.0);
        f.set(0, 0, f64::NAN);
        f.set(1, 0, f64::INFINITY);
        assert_eq!(f.non_finite_count(), 2);
        assert_eq!(f.sanitize_non_finite(7.0), 2);
        assert_eq!(f.non_finite_count(), 0);
        assert_eq!(f.get(0, 0), 7.0);
    }

    #[test]
    fn integer_field_rounds_and_saturates() {
        let g = grid();
        let f = Field::<u8>::from_fn(g, |x, _| if x == 0 { 255u8 } else { 2u8 });
        assert_eq!(f.get(0, 0), 255);
        assert_eq!(f.get(1, 0), 2);
        // Conversion from f64 rounds and saturates.
        assert_eq!(u8::from_f64(300.0), 255);
        assert_eq!(u8::from_f64(1.6), 2);
    }

    #[test]
    fn zip_and_add_scaled() {
        let g = grid();
        let mut a = Field::<f64>::filled(g, 1.0);
        let b = Field::<f64>::filled(g, 2.0);
        a.zip_with(&b, |_, x, y| x + y);
        assert_eq!(a.as_slice()[0], 3.0);
        a.add_scaled(&b, -0.5);
        assert_eq!(a.as_slice()[0], 2.0);
    }

    #[test]
    fn planar_grids_keep_constant_cell_area() {
        for boundary in [BoundaryKind::Rectangular, BoundaryKind::Toroidal] {
            let g = Grid::new(16, 16, 2500.0, boundary).unwrap();
            let area = 2500.0 * 2500.0;
            assert_eq!(g.cell_area_m2(), area);
            assert!(!g.has_spherical_areas());
            for y in 0..g.height() {
                for x in 0..g.width() {
                    assert_eq!(g.cell_area_m2_at(x, y), area);
                }
            }
            assert_eq!(g.total_area_m2(), g.cell_count() as f64 * area);
        }
        // A spherical boundary without a radius keeps the planar behaviour.
        let g = Grid::new(16, 16, 1000.0, BoundaryKind::Spherical).unwrap();
        assert!(!g.has_spherical_areas());
        assert_eq!(g.cell_area_m2_at(0, 0), 1.0e6);
        assert_eq!(g.total_area_m2(), 256.0 * 1.0e6);
    }

    #[test]
    fn spherical_cell_areas_follow_cosine_latitude() {
        let radius_m = 6_371_000.0;
        let g = Grid::new(64, 32, 1000.0, BoundaryKind::Spherical)
            .unwrap()
            .with_planet_radius(radius_m)
            .unwrap();
        assert!(g.has_spherical_areas());
        assert_eq!(g.planet_radius_m(), Some(radius_m));

        let band = |y: usize| g.cell_area_m2_at(0, y);
        // Equatorial cells are much larger than polar ones.
        let equator = band(g.height() / 2);
        let pole = band(0);
        let mut total = 0.0;
        for y in 0..g.height() {
            for x in 0..g.width() {
                total += g.cell_area_m2_at(x, y);
            }
        }
        println!(
            "spherical 64x32 R=6371 km: equator cell {equator:.6e} m², pole cell {pole:.6e} m², \
             ratio {:.4}, summed area {total:.6e} m², 4πR² {:.6e} m²",
            equator / pole,
            4.0 * std::f64::consts::PI * radius_m * radius_m
        );
        assert!(equator > pole * 10.0, "equator {equator} vs pole {pole}");
        // The area is exactly proportional to cos(cell-centre latitude).
        let h = g.height() as f64;
        let cos_at = |y: usize| {
            (((y as f64 + 0.5) * std::f64::consts::PI / h) - std::f64::consts::FRAC_PI_2).cos()
        };
        for y in 0..g.height() {
            for y_ref in [0usize, g.height() / 3, g.height() - 1] {
                let ratio = band(y) / band(y_ref);
                let want = cos_at(y) / cos_at(y_ref);
                assert!(
                    (ratio - want).abs() <= 1e-12 * want.abs(),
                    "row {y}: ratio {ratio} != cos ratio {want}"
                );
            }
        }
        // No column dependence in an equirectangular layout.
        for x in [0usize, 1, g.width() / 2, g.width() - 1] {
            assert_eq!(g.cell_area_m2_at(x, 7), band(7));
        }
        assert!(g.cell_area_m2_at(0, 0) > 0.0);
    }

    #[test]
    fn spherical_cell_areas_sum_to_the_sphere_area() {
        let radius_m = 6_371_000.0;
        for (width, height) in [(16u32, 16u32), (64, 32), (31, 16), (128, 65)] {
            let g = Grid::new(width, height, 1000.0, BoundaryKind::Spherical)
                .unwrap()
                .with_planet_radius(radius_m)
                .unwrap();
            let sphere = 4.0 * std::f64::consts::PI * radius_m * radius_m;
            let mut sum = 0.0;
            for y in 0..g.height() {
                for x in 0..g.width() {
                    sum += g.cell_area_m2_at(x, y);
                }
            }
            assert!(
                (sum - sphere).abs() / sphere < 1e-12,
                "{width}x{height}: summed {sum} vs 4πR² {sphere}"
            );
            assert!((g.total_area_m2() - sphere).abs() / sphere < 1e-15);
        }
    }

    #[test]
    fn planet_radius_must_be_positive_and_finite() {
        let g = Grid::new(16, 16, 1000.0, BoundaryKind::Spherical).unwrap();
        assert!(g.with_planet_radius(0.0).is_err());
        assert!(g.with_planet_radius(-1.0).is_err());
        assert!(g.with_planet_radius(f64::NAN).is_err());
        assert!(g.with_planet_radius(f64::INFINITY).is_err());
        assert!(g.with_planet_radius(1.0).is_ok());
    }
}
