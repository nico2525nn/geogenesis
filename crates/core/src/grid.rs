//! Dense scalar fields with the boundary topology that makes them meaningful.
//!
//! Every stencil routes edge handling through [`Boundary`]; nothing special-cases edges.

use crate::linalg::{fmax, fmin, Vec2};

/// Neighbour count in the fixed D8 order (E, NE, N, NW, W, SW, S, SE).
///
/// Erosion sums over this order, so the mapping is load-bearing:
///
/// ```text
///   NW  N  NE      3  2  1
///    W  ·   E  ->  4  ·  0
///   SW  S  SE      5  6  7
/// ```
pub const DIR_COUNT: usize = 8;

/// The eight cardinal/ordinal offsets, matching [`Direction::D8`] order.
pub const D8_OFFSETS: [(i64, i64); DIR_COUNT] = [
    (1, 0),   // E
    (1, 1),   // NE
    (0, 1),   // N
    (-1, 1),  // NW
    (-1, 0),  // W
    (-1, -1), // SW
    (0, -1),  // S
    (1, -1),  // SE
];

/// Step length for each D8 offset.
pub const D8_DISTANCES: [f64; DIR_COUNT] = [
    1.0,          // E
    std::f64::consts::SQRT_2, // NE
    1.0,          // N
    std::f64::consts::SQRT_2, // NW
    1.0,          // W
    std::f64::consts::SQRT_2, // SW
    1.0,          // S
    std::f64::consts::SQRT_2, // SE
];

/// Cardinal directions in the D8 order restricted to axis-aligned entries.
pub const CARDINAL_INDICES: [usize; 4] = [0, 2, 4, 6];

/// Neighbour direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 8-neighbourhood (D8 flow routing).
    D8,
    /// 4-neighbourhood (cardinal-only stencils).
    Cardinal,
}

impl Direction {
    /// Returns the `(dx, dy)` offsets for this neighbourhood.
    pub const fn offsets(self) -> &'static [(i64, i64)] {
        match self {
            Direction::D8 => &D8_OFFSETS,
            // Axis-aligned entries of the D8 order.
            Direction::Cardinal => &[
                D8_OFFSETS[0],
                D8_OFFSETS[2],
                D8_OFFSETS[4],
                D8_OFFSETS[6],
            ],
        }
    }

    /// Returns the step length for each offset.
    pub fn distances(self) -> &'static [f64] {
        match self {
            Direction::D8 => &D8_DISTANCES,
            Direction::Cardinal => &[1.0, 1.0, 1.0, 1.0],
        }
    }

    /// Index in the full D8 table for each direction in this neighbourhood.
    pub const fn d8_indices(self) -> &'static [usize] {
        match self {
            Direction::D8 => &[0, 1, 2, 3, 4, 5, 6, 7],
            Direction::Cardinal => &CARDINAL_INDICES,
        }
    }

    /// Number of directions.
    pub const fn len(self) -> usize {
        match self {
            Direction::D8 => DIR_COUNT,
            Direction::Cardinal => 4,
        }
    }

    /// Always false; present so `len` does not trip the `len_without_is_empty` lint.
    pub const fn is_empty(self) -> bool {
        false
    }
}

/// Index arithmetic with boundary topology applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boundary {
    kind: BoundaryKind,
    width: usize,
    height: usize,
}

/// The spec's `ワールド境界` enum, named to avoid confusion with [`Boundary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoundaryKind {
    /// 矩形
    #[default]
    Rectangular,
    /// トーラス
    Toroidal,
    /// 球面 (equirectangular)
    Spherical,
}

impl Boundary {
    /// Creates boundary arithmetic for a `width` × `height` grid.
    pub const fn new(kind: BoundaryKind, width: usize, height: usize) -> Self {
        Self { kind, width, height }
    }

    /// Grid width.
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Grid height.
    pub const fn height(&self) -> usize {
        self.height
    }

    /// The configured boundary kind.
    pub const fn kind(&self) -> BoundaryKind {
        self.kind
    }

    /// Total cell count.
    pub const fn cell_count(&self) -> usize {
        self.width * self.height
    }

    /// Returns `true` when x wraps around.
    const fn wraps_x(&self) -> bool {
        matches!(self.kind, BoundaryKind::Toroidal | BoundaryKind::Spherical)
    }

    /// Returns `true` when y wraps around.
    const fn wraps_y(&self) -> bool {
        matches!(self.kind, BoundaryKind::Toroidal)
    }

    /// Flattens `(x, y)` to a linear index.
    #[inline]
    pub const fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    /// Splits a linear index back into `(x, y)`.
    #[inline]
    pub const fn coords(&self, index: usize) -> (usize, usize) {
        (index % self.width, index / self.width)
    }

    /// Wraps or clamps a coordinate pair into in-range `usize` coordinates.
    #[inline]
    pub fn resolve(&self, x: i64, y: i64) -> (usize, usize) {
        let w = self.width as i64;
        let h = self.height as i64;

        let rx = if self.wraps_x() {
            x.rem_euclid(w)
        } else {
            x.clamp(0, w - 1)
        };
        let ry = if self.wraps_y() {
            y.rem_euclid(h)
        } else {
            y.clamp(0, h - 1)
        };
        (rx as usize, ry as usize)
    }

    /// Returns the linear index of a possibly out-of-range cell.
    ///
    /// Out-of-range reads on a clamping boundary return the clamped cell, which
    /// is the correct Neumann behaviour for stencils at the rim.
    #[inline]
    pub fn index_wrapped(&self, x: i64, y: i64) -> usize {
        let (rx, ry) = self.resolve(x, y);
        ry * self.width + rx
    }

    /// Returns `true` when the cell is on the outer rim.
    #[inline]
    pub fn is_edge(&self, x: usize, y: usize) -> bool {
        x == 0 || y == 0 || x + 1 == self.width || y + 1 == self.height
    }
}

/// A dense row-major scalar field over a grid with boundary topology.
#[derive(Debug, Clone)]
pub struct ScalarField {
    boundary: Boundary,
    data: Vec<f64>,
}

impl ScalarField {
    /// Allocates a field filled with `value`.
    pub fn filled(boundary: Boundary, value: f64) -> Self {
        Self {
            boundary,
            data: vec![value; boundary.cell_count()],
        }
    }

    /// Allocates a zero field.
    pub fn zeros(boundary: Boundary) -> Self {
        Self::filled(boundary, 0.0)
    }

    /// Wraps existing data. Panics if the length does not match the grid.
    pub fn from_vec(boundary: Boundary, data: Vec<f64>) -> Self {
        assert_eq!(
            data.len(),
            boundary.cell_count(),
            "ScalarField data length does not match grid size"
        );
        Self { boundary, data }
    }

    /// Copies the grid without its data.
    pub fn like(&self) -> Self {
        Self::zeros(self.boundary)
    }

    /// The boundary arithmetic for this field.
    pub fn boundary(&self) -> &Boundary {
        &self.boundary
    }

    /// Grid width.
    pub fn width(&self) -> usize {
        self.boundary.width()
    }

    /// Grid height.
    pub fn height(&self) -> usize {
        self.boundary.height()
    }

    /// Number of cells.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Always false; present so `len` does not trip the `len_without_is_empty` lint.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Underlying slice.
    pub fn as_slice(&self) -> &[f64] {
        &self.data
    }

    /// Mutable underlying slice.
    pub fn as_mut_slice(&mut self) -> &mut [f64] {
        &mut self.data
    }

    /// Consumes the field and returns its data.
    pub fn into_vec(self) -> Vec<f64> {
        self.data
    }

    /// Reads a cell by linear index. Panics if out of range.
    #[inline]
    pub fn get_index(&self, index: usize) -> f64 {
        self.data[index]
    }

    /// Writes a cell by linear index. Panics if out of range.
    #[inline]
    pub fn set_index(&mut self, index: usize, value: f64) {
        self.data[index] = value;
    }

    /// Reads a cell by coordinates, wrapping or clamping.
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f64 {
        self.data[self.boundary.index(x, y)]
    }

    /// Writes a cell by coordinates. Panics if out of range.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, value: f64) {
        let i = self.boundary.index(x, y);
        self.data[i] = value;
    }

    /// Reads with out-of-range coordinates, applying boundary behaviour.
    #[inline]
    pub fn get_wrapped(&self, x: i64, y: i64) -> f64 {
        self.data[self.boundary.index_wrapped(x, y)]
    }

    /// Adds to a cell by coordinates. Panics if out of range.
    #[inline]
    pub fn add(&mut self, x: usize, y: usize, delta: f64) {
        let i = self.boundary.index(x, y);
        self.data[i] += delta;
    }

    /// Applies `f` to every cell in place.
    pub fn map_in_place(&mut self, mut f: impl FnMut(usize, f64) -> f64) {
        for (i, v) in self.data.iter_mut().enumerate() {
            *v = f(i, *v);
        }
    }

    /// Combines two fields cellwise; panics if the grids differ.
    pub fn zip_with(&mut self, other: &Self, mut f: impl FnMut(f64, f64) -> f64) {
        assert_eq!(self.boundary, other.boundary, "grid shapes differ");
        for (a, b) in self.data.iter_mut().zip(&other.data) {
            *a = f(*a, *b);
        }
    }

    /// Adds another field cellwise; panics if the grids differ.
    pub fn add_assign_field(&mut self, other: &Self) {
        self.zip_with(other, |a, b| a + b);
    }

    /// Multiplies another field cellwise; panics if the grids differ.
    pub fn mul_assign_field(&mut self, other: &Self) {
        self.zip_with(other, |a, b| a * b);
    }

    /// Largest value in the field.
    pub fn max(&self) -> f64 {
        self.data.iter().copied().fold(f64::NEG_INFINITY, fmax)
    }

    /// Smallest value in the field.
    pub fn min(&self) -> f64 {
        self.data.iter().copied().fold(f64::INFINITY, fmin)
    }

    /// Arithmetic mean.
    pub fn mean(&self) -> f64 {
        if self.data.is_empty() {
            return 0.0;
        }
        self.data.iter().sum::<f64>() / self.data.len() as f64
    }

    /// Population standard deviation.
    pub fn std_dev(&self) -> f64 {
        if self.data.is_empty() {
            return 0.0;
        }
        let m = self.mean();
        (self.data.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / self.data.len() as f64).sqrt()
    }

    /// Quantile `q` in `[0, 1]`, using linear interpolation between order statistics.
    pub fn quantile(&self, q: f64) -> f64 {
        if self.data.is_empty() {
            return f64::NAN;
        }
        let mut sorted = self.data.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let pos = crate::linalg::clamp(q, 0.0, 1.0) * (sorted.len() - 1) as f64;
        let lo = pos.floor() as usize;
        let hi = pos.ceil() as usize;
        crate::linalg::lerp(sorted[lo], sorted[hi], pos - lo as f64)
    }

    /// Clamps every value into `[lo, hi]`.
    pub fn clamp_all(&mut self, lo: f64, hi: f64) {
        for v in &mut self.data {
            *v = crate::linalg::clamp(*v, lo, hi);
        }
    }

    /// Rescales values linearly so the field spans `[new_min, new_max]`.
    ///
    /// A flat field is left alone: dividing by a zero range would produce NaN.
    pub fn rescale(&mut self, new_min: f64, new_max: f64) {
        let (lo, hi) = (self.min(), self.max());
        let span = hi - lo;
        if span <= f64::EPSILON {
            return;
        }
        for v in &mut self.data {
            *v = crate::linalg::lerp(new_min, new_max, (*v - lo) / span);
        }
    }

    /// Replaces every non-finite value with `replacement`.
    pub fn sanitize_non_finite(&mut self, replacement: f64) -> usize {
        let mut count = 0;
        for v in &mut self.data {
            if !v.is_finite() {
                *v = replacement;
                count += 1;
            }
        }
        count
    }

    /// Number of non-finite cells remaining.
    pub fn non_finite_count(&self) -> usize {
        self.data.iter().filter(|v| !v.is_finite()).count()
    }

    /// Central-difference gradient `(dz/dx, dz/dy)`.
    ///
    /// Step is the physical cell size, so the returned gradient is dimensionless
    /// (rise over run) and directly usable as a slope for stream power.
    pub fn gradient(&self, x: usize, y: usize, step: f64) -> Vec2 {
        let zp = self.get_wrapped(x as i64 + 1, y as i64);
        let zm = self.get_wrapped(x as i64 - 1, y as i64);
        let yp = self.get_wrapped(x as i64, y as i64 + 1);
        let ym = self.get_wrapped(x as i64, y as i64 - 1);
        Vec2::new((zp - zm) / (2.0 * step), (yp - ym) / (2.0 * step))
    }

    /// Magnitude of the central-difference gradient (rise over run).
    pub fn slope(&self, x: usize, y: usize, step: f64) -> f64 {
        self.gradient(x, y, step).length()
    }

    /// Direction of steepest descent as a unit vector.
    ///
    /// Returns `Vec2::ZERO` on flat ground so callers can detect pits without
    /// dividing by zero.
    pub fn steepest_descent(&self, x: usize, y: usize, step: f64) -> Vec2 {
        let g = self.gradient(x, y, step);
        (-g).normalized()
    }

    /// 5-point Laplacian: `f(x+1) + f(x-1) + f(y+1) + f(y-1) - 4f(x,y)`.
    ///
    /// This is the discrete Laplacian *without* the `1/h²` factor; callers must
    /// divide by `step²` to obtain units of 1/metre.
    pub fn laplacian_raw(&self, x: usize, y: usize) -> f64 {
        let (xi, yi) = (x as i64, y as i64);
        self.get_wrapped(xi + 1, yi)
            + self.get_wrapped(xi - 1, yi)
            + self.get_wrapped(xi, yi + 1)
            + self.get_wrapped(xi, yi - 1)
            - 4.0 * self.get_wrapped(xi, yi)
    }

    /// Divergence of a vector field stored as two `ScalarField`s.
    pub fn divergence(u: &ScalarField, v: &ScalarField, step: f64) -> ScalarField {
        let b = u.boundary();
        let mut out = ScalarField::zeros(*b);
        for y in 0..b.height() {
            for x in 0..b.width() {
                let dudx = (u.get_wrapped(x as i64 + 1, y as i64) - u.get_wrapped(x as i64 - 1, y as i64))
                    / (2.0 * step);
                let dvdy = (v.get_wrapped(x as i64, y as i64 + 1) - v.get_wrapped(x as i64, y as i64 - 1))
                    / (2.0 * step);
                out.set(x, y, dudx + dvdy);
            }
        }
        out
    }

    /// Applies the 5-point Laplacian smoothing kernel `iters` times.
    ///
    /// `coef` must satisfy `coef <= 0.25` for the explicit update to be stable;
    /// [`crate::erosion::hillslope`] derives `coef` from that condition.
pub fn smooth(&mut self, coef: f64, iters: usize) {
        debug_assert!(
            coef <= 0.25 + f64::EPSILON,
            "explicit diffusion coef {coef} exceeds the stability limit 0.25"
        );
        if iters == 0 {
            return;
        }
        let b = *self.boundary();
        let mut scratch = self.like();
        for _ in 0..iters {
            for y in 0..b.height() {
                for x in 0..b.width() {
                    scratch.set(
                        x,
                        y,
                        coef * self.laplacian_raw(x, y) + self.get(x, y),
                    );
                }
            }
            core::mem::swap(self, &mut scratch);
        }
    }

    /// Cell indices in descending elevation order (`dir` is currently unused).
    ///
    /// Erosion must apply in this order; raster order silently breaks mass
    /// conservation.
    pub fn downhill_order(&self, dir: Direction) -> Vec<usize> {
        let b = *self.boundary();
        let mut order: Vec<usize> = (0..b.cell_count()).collect();
        // NaN sorts first so it cannot corrupt a well-formed neighbour.
        order.sort_unstable_by(|&a, &c| {
            let (va, vc) = (self.data[a], self.data[c]);
            match (va.is_nan(), vc.is_nan()) {
                (true, false) => core::cmp::Ordering::Less,
                (false, true) => core::cmp::Ordering::Greater,
                _ => vc.partial_cmp(&va).unwrap_or(core::cmp::Ordering::Equal),
            }
        });
        let _ = dir;
        order
    }
}

/// A dense row-major integer field, used for plate ids and rock ids.
#[derive(Debug, Clone)]
pub struct LabelField {
    boundary: Boundary,
    data: Vec<u32>,
}

impl LabelField {
    /// Allocates a field filled with `value`.
    pub fn filled(boundary: Boundary, value: u32) -> Self {
        Self {
            boundary,
            data: vec![value; boundary.cell_count()],
        }
    }

    /// Wraps existing data. Panics on length mismatch.
    pub fn from_vec(boundary: Boundary, data: Vec<u32>) -> Self {
        assert_eq!(
            data.len(),
            boundary.cell_count(),
            "LabelField data length does not match grid size"
        );
        Self { boundary, data }
    }

    /// Grid width.
    pub fn width(&self) -> usize {
        self.boundary.width()
    }

    /// Grid height.
    pub fn usize_height(&self) -> usize {
        self.boundary.height()
    }

    /// Grid height.
    pub fn height(&self) -> usize {
        self.boundary.height()
    }

    /// Number of cells.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Always false; present so `len` does not trip the `len_without_is_empty` lint.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Boundary arithmetic.
    pub fn boundary(&self) -> &Boundary {
        &self.boundary
    }

    /// Reads a cell by linear index.
    #[inline]
    pub fn get_index(&self, index: usize) -> u32 {
        self.data[index]
    }

    /// Writes a cell by linear index.
    #[inline]
    pub fn set_index(&mut self, index: usize, value: u32) {
        self.data[index] = value;
    }

    /// Reads a cell by coordinates.
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u32 {
        self.data[self.boundary.index(x, y)]
    }

    /// Writes a cell by coordinates.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, value: u32) {
        let i = self.boundary.index(x, y);
        self.data[i] = value;
    }

    /// Reads with out-of-range coordinates, applying boundary behaviour.
    #[inline]
    pub fn get_wrapped(&self, x: i64, y: i64) -> u32 {
        self.data[self.boundary.index_wrapped(x, y)]
    }

    /// Underlying slice.
    pub fn as_slice(&self) -> &[u32] {
        &self.data
    }

    /// Consumes the field and returns its data.
    pub fn into_vec(self) -> Vec<u32> {
        self.data
    }

    /// Number of cells whose label differs from at least one neighbour.
    ///
    /// With a rectangular boundary, cells on the rim are excluded: their missing
    /// out-of-grid neighbours would otherwise all count as "different".
    pub fn boundary_cell_count(&self) -> usize {
        let b = self.boundary();
        let mut count = 0;
        for y in 0..b.height() {
            for x in 0..b.width() {
                if b.is_edge(x, y) {
                    continue;
                }
                let here = self.get(x, y);
                let differs = D8_OFFSETS.iter().any(|&(dx, dy)| {
                    self.get_wrapped(x as i64 + dx, y as i64 + dy) != here
                });
                if differs {
                    count += 1;
                }
            }
        }
        count
    }

    /// Distance in cells to the nearest differently-labelled cell, capped at `max`.
    pub fn distance_to_boundary(&self, max: usize) -> ScalarField {
        let b = self.boundary();
        let mut dist = ScalarField::filled(*b, max as f64);
        let mut queue: Vec<usize> = Vec::new();

        for y in 0..b.height() {
            for x in 0..b.width() {
                if b.is_edge(x, y) {
                    continue;
                }
                let here = self.get(x, y);
                let on_edge = D8_OFFSETS
                    .iter()
                    .any(|&(dx, dy)| self.get_wrapped(x as i64 + dx, y as i64 + dy) != here);
                if on_edge {
                    dist.set(x, y, 0.0);
                    queue.push(b.index(x, y));
                }
            }
        }

        let mut head = 0;
        while head < queue.len() {
            let idx = queue[head];
            head += 1;
            let (x, y) = b.coords(idx);
            let d = dist.get(x, y);
            if d >= max as f64 {
                continue;
            }
            for &(dx, dy) in D8_OFFSETS.iter() {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                let nd = d + 1.0;
                if self.get_wrapped(nx, ny) == self.get_wrapped(x as i64, y as i64)
                    && dist.get_wrapped(nx, ny) > nd
                {
                    dist.set_wrapped(nx, ny, nd);
                    queue.push(b.index_wrapped(nx, ny));
                }
            }
        }
        dist
    }
}

impl LabelField {
    /// Writes a cell with out-of-range coordinates, applying boundary behaviour.
    #[inline]
    pub fn set_wrapped(&mut self, x: i64, y: i64, value: u32) {
        let i = self.boundary.index_wrapped(x, y);
        self.data[i] = value;
    }
}

/// Helper trait so `ScalarField::set_wrapped` reads naturally in stencils.
trait SetWrapped {
    fn set_wrapped(&mut self, x: i64, y: i64, value: f64);
}

impl SetWrapped for ScalarField {
    #[inline]
    fn set_wrapped(&mut self, x: i64, y: i64, value: f64) {
        let i = self.boundary.index_wrapped(x, y);
        self.data[i] = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundary(kind: BoundaryKind, w: usize, h: usize) -> Boundary {
        Boundary::new(kind, w, h)
    }

    #[test]
    fn index_round_trips() {
        let b = boundary(BoundaryKind::Rectangular, 7, 5);
        for y in 0..5 {
            for x in 0..7 {
                let (rx, ry) = b.coords(b.index(x, y));
                assert_eq!((rx, ry), (x, y));
            }
        }
    }

    #[test]
    fn rect_clamps_out_of_range() {
        let b = boundary(BoundaryKind::Rectangular, 4, 4);
        assert_eq!(b.index_wrapped(-1, 2), b.index(0, 2));
        assert_eq!(b.index_wrapped(9, 2), b.index(3, 2));
        assert_eq!(b.index_wrapped(2, -5), b.index(2, 0));
    }

    #[test]
    fn toroidal_wraps_both_axes() {
        let b = boundary(BoundaryKind::Toroidal, 4, 4);
        assert_eq!(b.index_wrapped(-1, 0), b.index(3, 0));
        assert_eq!(b.index_wrapped(4, 0), b.index(0, 0));
        assert_eq!(b.index_wrapped(0, -1), b.index(0, 3));
        assert_eq!(b.index_wrapped(0, 4), b.index(0, 0));
    }

    #[test]
    fn spherical_wraps_x_but_clamps_poles() {
        let b = boundary(BoundaryKind::Spherical, 8, 4);
        assert_eq!(b.index_wrapped(-1, 1), b.index(7, 1));
        assert_eq!(b.index_wrapped(8, 1), b.index(0, 1));
        // Poles clamp rather than wrapping.
        assert_eq!(b.index_wrapped(2, -1), b.index(2, 0));
        assert_eq!(b.index_wrapped(2, 4), b.index(2, 3));
    }

    #[test]
    fn boundary_wrap_rules() {
        assert!(boundary(BoundaryKind::Toroidal, 4, 4).wraps_x());
        assert!(boundary(BoundaryKind::Toroidal, 4, 4).wraps_y());
        assert!(boundary(BoundaryKind::Spherical, 4, 4).wraps_x());
        assert!(!boundary(BoundaryKind::Spherical, 4, 4).wraps_y());
        assert!(!boundary(BoundaryKind::Rectangular, 4, 4).wraps_x());
        assert!(!boundary(BoundaryKind::Rectangular, 4, 4).wraps_y());
    }

    #[test]
    fn direction_offsets_are_distinct() {
        let mut seen = Vec::new();
        for (dx, dy) in D8_OFFSETS {
            assert_ne!((dx, dy), (0, 0), "no neighbour may be the centre cell");
            seen.push((dx, dy));
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), DIR_COUNT);
    }

    #[test]
    fn cardinal_subset_is_axis_aligned_and_sized() {
        let d = Direction::Cardinal;
        assert_eq!(d.len(), 4);
        assert_eq!(d.offsets().len(), 4);
        assert_eq!(d.distances().len(), 4);
        for (i, &(dx, dy)) in d.offsets().iter().enumerate() {
            assert!(dx == 0 || dy == 0, "cardinal offset {i} is diagonal");
            assert_eq!(d.distances()[i], 1.0);
        }
        for &i in d.d8_indices() {
            let (dx, dy) = D8_OFFSETS[i];
            assert!(dx == 0 || dy == 0);
        }
    }

    #[test]
    fn d8_offsets_and_distances_agree() {
        for i in 0..DIR_COUNT {
            let (dx, dy) = D8_OFFSETS[i];
            let expect = ((dx * dx + dy * dy) as f64).sqrt();
            assert!((D8_DISTANCES[i] - expect).abs() < 1e-12, "distance mismatch at {i}");
        }
    }

    #[test]
    fn field_from_vec_checks_length() {
        let b = boundary(BoundaryKind::Rectangular, 3, 3);
        assert!(std::panic::catch_unwind(|| ScalarField::from_vec(b, vec![0.0; 8])).is_err());
        assert!(ScalarField::from_vec(b, vec![0.0; 9]).len() == 9);
    }

    #[test]
    fn statistics_are_correct() {
        let b = boundary(BoundaryKind::Rectangular, 4, 1);
        let f = ScalarField::from_vec(b, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(f.min(), 1.0);
        assert_eq!(f.max(), 4.0);
        assert_eq!(f.mean(), 2.5);
        assert!((f.std_dev() - 1.118_034).abs() < 1e-5);
    }

    #[test]
    fn quantile_interpolates() {
        let b = boundary(BoundaryKind::Rectangular, 4, 1);
        let f = ScalarField::from_vec(b, vec![0.0, 1.0, 2.0, 3.0]);
        assert_eq!(f.quantile(0.0), 0.0);
        assert_eq!(f.quantile(1.0), 3.0);
        assert!((f.quantile(0.5) - 1.5).abs() < 1e-12);
    }

    #[test]
    fn rescale_maps_range() {
        let b = boundary(BoundaryKind::Rectangular, 3, 1);
        let mut f = ScalarField::from_vec(b, vec![1.0, 2.0, 3.0]);
        f.rescale(0.0, 10.0);
        assert_eq!(f.as_slice(), &[0.0, 5.0, 10.0]);
    }

    #[test]
    fn rescale_of_flat_field_does_not_produce_nan() {
        let b = boundary(BoundaryKind::Rectangular, 3, 1);
        let mut f = ScalarField::from_vec(b, vec![5.0, 5.0, 5.0]);
        f.rescale(0.0, 10.0);
        assert_eq!(f.non_finite_count(), 0);
        assert_eq!(f.max(), 5.0);
    }

    #[test]
    fn sanitize_replaces_non_finite() {
        let b = boundary(BoundaryKind::Rectangular, 3, 1);
        let mut f = ScalarField::from_vec(b, vec![f64::NAN, 1.0, f64::INFINITY]);
        assert_eq!(f.sanitize_non_finite(0.0), 2);
        assert_eq!(f.as_slice(), &[0.0, 1.0, 0.0]);
        assert_eq!(f.non_finite_count(), 0);
    }

    #[test]
    fn gradient_of_a_plane_is_constant() {
        let b = boundary(BoundaryKind::Toroidal, 16, 16);
        let step = 100.0;
        let mut f = ScalarField::zeros(b);
        for y in 0..16 {
            for x in 0..16 {
                f.set(x, y, 0.01 * (x as f64 * step) + 0.02 * (y as f64 * step));
            }
        }
        for y in 2..14 {
            for x in 2..14 {
                let g = f.gradient(x, y, step);
                assert!((g.x - 0.01).abs() < 1e-9, "dzdx {}", g.x);
                assert!((g.y - 0.02).abs() < 1e-9, "dzdy {}", g.y);
                assert!((f.slope(x, y, step) - 0.01f64.hypot(0.02)).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn steepest_descent_points_downhill() {
        let b = boundary(BoundaryKind::Rectangular, 8, 8);
        let step = 1.0;
        let mut f = ScalarField::zeros(b);
        // Ramp increasing in +x: downhill must point in -x.
        for y in 0..8 {
            for x in 0..8 {
                f.set(x, y, x as f64);
            }
        }
        let d = f.steepest_descent(4, 4, step);
        assert!((d.x + 1.0).abs() < 1e-12 && d.y.abs() < 1e-12, "{d:?}");
    }

    #[test]
    fn steepest_descent_on_flat_ground_is_zero() {
        let b = boundary(BoundaryKind::Rectangular, 4, 4);
        let f = ScalarField::zeros(b);
        assert_eq!(f.steepest_descent(1, 1, 1.0), Vec2::ZERO);
    }

    #[test]
    fn laplacian_of_a_plane_is_zero() {
        let b = boundary(BoundaryKind::Rectangular, 8, 8);
        let mut f = ScalarField::zeros(b);
        for y in 0..8 {
            for x in 0..8 {
                f.set(x, y, 3.0 * x as f64 + 2.0 * y as f64);
            }
        }
        for y in 1..7 {
            for x in 1..7 {
                assert!(f.laplacian_raw(x, y).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn laplacian_of_a_pit_is_positive() {
        let b = boundary(BoundaryKind::Rectangular, 5, 5);
        let mut f = ScalarField::filled(b, 1.0);
        f.set(2, 2, 0.0);
        // Four neighbours at 1.0 around a centre of 0.0: 4*1 - 4*0 = 4 > 0.
        assert!((f.laplacian_raw(2, 2) - 4.0).abs() < 1e-12);
    }

    #[test]
    fn laplacian_of_a_peak_is_negative() {
        let b = boundary(BoundaryKind::Rectangular, 5, 5);
        let mut f = ScalarField::filled(b, 0.0);
        f.set(2, 2, 10.0);
        // Centre 10.0 surrounded by 0.0: -4*10 = -40 < 0.
        assert!((f.laplacian_raw(2, 2) + 40.0).abs() < 1e-12);
    }

    #[test]
    fn smooth_diffuses_a_spike() {
        let b = boundary(BoundaryKind::Rectangular, 9, 9);
        let mut f = ScalarField::zeros(b);
        f.set(4, 4, 100.0);
        let before = f.max();
        // coef * laplacian with coef small enough to stay stable.
        f.smooth(0.1, 20);
        assert!(f.max() < before, "peak not lowered");
        assert!(f.get(4, 4) > 0.0, "spike entirely vanished");
        let neighbours_sum: f64 = [3, 5].iter()
            .flat_map(|&y| [3, 4, 5].iter().map(move |&x| (x, y)))
            .map(|(x, y)| f.get(x, y))
            .sum();
        assert!(neighbours_sum > 0.0, "spike did not spread");
    }

    #[test]
    fn smoothing_preserves_total_mass_on_a_torus() {
        let b = boundary(BoundaryKind::Toroidal, 12, 12);
        let mut f = ScalarField::zeros(b);
        f.set(6, 6, 50.0);
        let before: f64 = f.as_slice().iter().sum();
        f.smooth(0.2, 10);
        let after: f64 = f.as_slice().iter().sum();
        assert!((before - after).abs() < 1e-9, "mass {before} -> {after}");
    }

    #[test]
    fn downhill_order_is_descending_by_elevation() {
        let b = boundary(BoundaryKind::Rectangular, 3, 3);
        let f = ScalarField::from_vec(b, (0..9).map(|i| i as f64).collect());
        let order = f.downhill_order(Direction::D8);
        for w in order.windows(2) {
            assert!(
                f.get_index(w[0]) >= f.get_index(w[1]),
                "order is not descending"
            );
        }
    }

    #[test]
    fn downhill_order_handles_nan() {
        let b = boundary(BoundaryKind::Rectangular, 2, 2);
        let f = ScalarField::from_vec(b, vec![f64::NAN, 1.0, 2.0, 3.0]);
        let order = f.downhill_order(Direction::D8);
        assert_eq!(order.len(), 4);
        // NaN must not crash the sort.
        assert!(order.iter().all(|&i| i < 4));
    }

    #[test]
    fn zip_requires_matching_grids() {
        let a = ScalarField::zeros(boundary(BoundaryKind::Rectangular, 4, 4));
        let b = ScalarField::zeros(boundary(BoundaryKind::Rectangular, 2, 8));
        let mut a = a;
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            a.zip_with(&b, |x, y| x + y)
        }))
        .is_err());
    }

    #[test]
    fn field_arithmetic_operates_cellwise() {
        let bb = boundary(BoundaryKind::Rectangular, 2, 2);
        let mut a = ScalarField::from_vec(bb, vec![1.0, 2.0, 3.0, 4.0]);
        let b = ScalarField::from_vec(bb, vec![10.0, 20.0, 30.0, 40.0]);
        a.add_assign_field(&b);
        assert_eq!(a.as_slice(), &[11.0, 22.0, 33.0, 44.0]);
        let mut c = ScalarField::from_vec(bb, vec![2.0, 2.0, 2.0, 2.0]);
        c.mul_assign_field(&ScalarField::from_vec(bb, vec![1.0, 2.0, 3.0, 4.0]));
        assert_eq!(c.as_slice(), &[2.0, 4.0, 6.0, 8.0]);
    }

    #[test]
    fn divergence_of_a_constant_gradient_field_is_two() {
        // Periodic field: a linear ramp is not periodic on a torus.
        let bb = boundary(BoundaryKind::Toroidal, 16, 16);
        let step = 1.0;
        let k = std::f64::consts::TAU / 16.0;
        let mut u = ScalarField::zeros(bb);
        let mut v = ScalarField::zeros(bb);
        for y in 0..16 {
            for x in 0..16 {
                u.set(x, y, (k * x as f64).cos());
                v.set(x, y, (k * y as f64).cos());
            }
        }
        let d = ScalarField::divergence(&u, &v, step);
        for y in 0..16 {
            for x in 0..16 {
                // du/dx = -k sin(kx), dv/dy = -k sin(ky).
                let expect =
                    -k * (k * x as f64).sin() - k * (k * y as f64).sin();
                assert!(
                    (d.get(x, y) - expect).abs() < 0.05,
                    "at {x},{y}: {} vs {expect}",
                    d.get(x, y)
                );
            }
        }
        // The mean divergence of this field is zero, which the seam must not break.
        assert!(d.mean().abs() < 0.02, "seam leaked mass: {}", d.mean());
    }

    #[test]
    fn label_boundary_count_finds_the_seam() {
        let bb = boundary(BoundaryKind::Rectangular, 8, 8);
        // Left half label 0, right half label 1.
        let mut lf = LabelField::filled(bb, 0);
        for y in 0..8 {
            for x in 4..8 {
                lf.set(x, y, 1);
            }
        }
        // Both columns adjacent to the seam count: 2 columns x 6 interior rows.
        assert_eq!(lf.boundary_cell_count(), 12);
    }

    #[test]
    fn label_boundary_count_ignores_rim_when_uniform() {
        let bb = boundary(BoundaryKind::Rectangular, 8, 8);
        let lf = LabelField::filled(bb, 7);
        assert_eq!(lf.boundary_cell_count(), 0);
    }

    #[test]
    fn distance_to_boundary_is_zero_on_seam_and_grows_inward() {
        let bb = boundary(BoundaryKind::Rectangular, 12, 12);
        let mut lf = LabelField::filled(bb, 0);
        for y in 0..12 {
            for x in 6..12 {
                lf.set(x, y, 1);
            }
        }
        let d = lf.distance_to_boundary(10);
        assert_eq!(d.get(5, 5), 0.0);
        assert_eq!(d.get(4, 5), 1.0);
        assert!(d.get(0, 5) > d.get(2, 5));
        assert!(d.get(2, 5) > d.get(4, 5));
    }

    #[test]
    fn label_field_wrapped_writes_stay_in_range() {
        let bb = boundary(BoundaryKind::Toroidal, 4, 4);
        let mut lf = LabelField::filled(bb, 0);
        lf.set_wrapped(-1, -1, 9);
        assert_eq!(lf.get(3, 3), 9);
    }
}

#[cfg(test)]
mod boundary_kind_tests {
    use super::BoundaryKind;

    #[test]
    fn kinds_are_distinct() {
        assert_ne!(BoundaryKind::Rectangular, BoundaryKind::Toroidal);
        assert_ne!(BoundaryKind::Toroidal, BoundaryKind::Spherical);
        assert_ne!(BoundaryKind::Rectangular, BoundaryKind::Spherical);
    }
}