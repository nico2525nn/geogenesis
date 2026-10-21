//! Spatial operators shared by subsystems: exact Euclidean distance
//! transforms (for continentality, coastal fetch and hotspot footprints),
//! separable Gaussian smoothing (flexural loads, climate fields) and
//! low-pass helpers.

use crate::grid::{Field, FieldScalar, Grid};

/// Exact squared Euclidean distance transform of a 1D line.
///
/// Uses the linear-time algorithm of Felzenszwalb & Huttenlocher (2012).
/// `f` holds squared distance to the nearest seed (0 at seeds, infinity
/// elsewhere); `out` receives the transformed values.
fn edt_1d(f: &[f64], out: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        loop {
            let vk = v[k];
            let s = ((f[q] + (q * q) as f64) - (f[vk] + (vk * vk) as f64))
                / (2.0 * (q as f64 - vk as f64));
            if s <= z[k] {
                if k == 0 {
                    break;
                }
                k -= 1;
            } else {
                break;
            }
        }
        let vk = v[k];
        let s =
            ((f[q] + (q * q) as f64) - (f[vk] + (vk * vk) as f64)) / (2.0 * (q as f64 - vk as f64));
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    let mut k = 0usize;
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let d = q as f64 - v[k] as f64;
        out[q] = d * d + f[v[k]];
    }
}

/// Wrapped 1D distance transform: transforms a line that repeats with period
/// `n`, by doubling it and taking the nearer copy.
fn edt_1d_wrapped(f: &[f64], out: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut doubled = Vec::with_capacity(2 * n);
    doubled.extend_from_slice(f);
    doubled.extend_from_slice(f);
    let mut transformed = vec![0.0f64; 2 * n];
    edt_1d(&doubled, &mut transformed);
    for i in 0..n {
        out[i] = transformed[i].min(transformed[i + n]);
    }
}

/// Exact Euclidean distance transform of a binary mask.
///
/// Returns squared distances in cell units (`f64`) as a field. `wrapped`
/// selects periodic handling per axis, matching the grid boundary.
#[must_use]
pub fn squared_distance_field(mask: &[bool], grid: Grid) -> Field<f64> {
    let w = grid.width();
    let h = grid.height();
    let wrap_x = grid.boundary().wraps_x();
    let wrap_y = grid.boundary().wraps_y();
    // A finite sentinel is essential: the parabola-intersection step computes
    // `inf - inf`, which poisons the transform when an entire line has no
    // seed. The sentinel is larger than any squared distance in the domain, so
    // real seeds always win.
    let sentinel = 4.0 * ((w * w + h * h) as f64) + 1.0;
    let mut data: Vec<f64> = mask
        .iter()
        .map(|&m| if m { 0.0 } else { sentinel })
        .collect();
    // Rows.
    let mut line_in = vec![0.0f64; w];
    let mut line_out = vec![0.0f64; w];
    for y in 0..h {
        line_in.copy_from_slice(&data[y * w..(y + 1) * w]);
        if wrap_x {
            edt_1d_wrapped(&line_in, &mut line_out);
        } else {
            edt_1d(&line_in, &mut line_out);
        }
        data[y * w..(y + 1) * w].copy_from_slice(&line_out);
    }
    // Columns.
    let mut col_in = vec![0.0f64; h];
    let mut col_out = vec![0.0f64; h];
    for x in 0..w {
        for y in 0..h {
            col_in[y] = data[y * w + x];
        }
        if wrap_y {
            edt_1d_wrapped(&col_in, &mut col_out);
        } else {
            edt_1d(&col_in, &mut col_out);
        }
        for y in 0..h {
            data[y * w + x] = col_out[y];
        }
    }
    let mut out = Field::<f64>::zeros(grid);
    out.as_mut_slice().copy_from_slice(&data);
    out
}

/// Distance in metres to the nearest `true` cell of `mask`.
#[must_use]
pub fn distance_m(mask: &[bool], grid: Grid) -> Field<f32> {
    let squared = squared_distance_field(mask, grid);
    let cell_m = grid.cell_size_m();
    let mut out = Field::<f32>::zeros(grid);
    for (dst, &sq) in out.as_mut_slice().iter_mut().zip(squared.as_slice()) {
        *dst = (sq.sqrt() * cell_m) as f32;
    }
    out
}

/// Distance in metres to the nearest cell satisfying `predicate`, using a
/// per-cell mask built from the elevation field.
#[must_use]
pub fn distance_to<T: FieldScalar>(
    field: &Field<T>,
    mut predicate: impl FnMut(f64) -> bool,
) -> Field<f32> {
    let mask: Vec<bool> = field
        .as_slice()
        .iter()
        .map(|v| predicate(v.to_f64()))
        .collect();
    distance_m(&mask, field.grid())
}

/// Separable Gaussian blur in place; `sigma_cells` is measured in cells.
///
/// Boundary handling follows the grid topology: wrapped axes wrap, open axes
/// replicate the edge cell. The kernel is truncated at three sigma, which keeps
/// the cost bounded for large flexural smoothing radii.
pub fn gaussian_blur<T: FieldScalar>(field: &mut Field<T>, sigma_cells: f64) {
    if !sigma_cells.is_finite() || sigma_cells <= 0.05 {
        return;
    }
    let grid = field.grid();
    let (w, h) = (grid.width(), grid.height());
    let max_radius = w.min(h) as f64 * 0.5;
    let sigma = sigma_cells.min(max_radius.max(1.0));
    let radius = (sigma * 3.0).ceil() as usize;
    if radius == 0 {
        return;
    }
    let mut kernel = Vec::with_capacity(radius + 1);
    let denom = 2.0 * sigma * sigma;
    let mut sum = 0.0;
    for i in 0..=radius {
        let weight = (-((i * i) as f64) / denom).exp();
        kernel.push(weight);
        sum += if i == 0 { weight } else { 2.0 * weight };
    }
    for weight in &mut kernel {
        *weight /= sum;
    }

    // Horizontal pass into a temporary buffer.
    let mut temp = vec![0.0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = field.get(x, y).to_f64() * kernel[0];
            for k in 1..=radius {
                let xl = clamp_index(x as isize - k as isize, w, grid.boundary().wraps_x());
                let xr = clamp_index(x as isize + k as isize, w, grid.boundary().wraps_x());
                acc += (field.get(xl, y).to_f64() + field.get(xr, y).to_f64()) * kernel[k];
            }
            temp[y * w + x] = acc;
        }
    }
    // Vertical pass back into the field.
    for y in 0..h {
        for x in 0..w {
            let mut acc = temp[y * w + x] * kernel[0];
            for k in 1..=radius {
                let yu = clamp_index(y as isize - k as isize, h, grid.boundary().wraps_y());
                let yd = clamp_index(y as isize + k as isize, h, grid.boundary().wraps_y());
                acc += (temp[yu * w + x] + temp[yd * w + x]) * kernel[k];
            }
            field.as_mut_slice()[y * w + x] = T::from_f64(acc);
        }
    }
}

#[inline]
fn clamp_index(v: isize, n: usize, wrapped: bool) -> usize {
    if wrapped {
        v.rem_euclid(n as isize) as usize
    } else {
        v.clamp(0, n as isize - 1) as usize
    }
}

/// Applies a broad smoothing kernel whose radius is given in kilometres.
pub fn blur_km<T: FieldScalar>(field: &mut Field<T>, sigma_km: f64) {
    let sigma_cells = sigma_km * 1000.0 / field.grid().cell_size_m();
    gaussian_blur(field, sigma_cells);
}

/// Separable Gaussian blur of a flat row-major `f64` slice with reusable
/// scratch, for tiled generation (large grids).
///
/// [`gaussian_blur`] operates on a whole [`Field`] and allocates its temporary
/// on every call. Tiled generation instead blurs one tile (core plus halo) at a
/// time, dozens of tiles per pass, so it wants a scratch buffer it can reuse
/// across tiles and no `Grid` to describe the padded buffer.
///
/// Boundary handling clamps (replicates the edge value) on all four sides,
/// regardless of grid topology. That is deliberate: callers pad the tile with a
/// halo at least `ceil(3 * sigma)` cells wide and only trust the interior, so
/// the boundary rule cannot affect the cells that are kept. `scratch` is
/// resized as needed and keeps its allocation between calls.
///
/// The kernel is truncated at three sigma like [`gaussian_blur`], and the
/// summation order matches it, so for a clamped (non-wrapping) grid the two
/// functions produce bit-identical values.
pub fn gaussian_blur_slice(
    values: &mut [f64],
    scratch: &mut Vec<f64>,
    width: usize,
    height: usize,
    sigma_cells: f64,
) {
    assert_eq!(
        values.len(),
        width * height,
        "blur slice must be width * height"
    );
    if width == 0 || height == 0 {
        return;
    }
    if !sigma_cells.is_finite() || sigma_cells <= 0.05 {
        return;
    }
    let max_radius = width.min(height) as f64 * 0.5;
    let sigma = sigma_cells.min(max_radius.max(1.0));
    let radius = (sigma * 3.0).ceil() as usize;
    if radius == 0 {
        return;
    }
    let mut kernel = Vec::with_capacity(radius + 1);
    let denom = 2.0 * sigma * sigma;
    let mut sum = 0.0;
    for i in 0..=radius {
        let weight = (-((i * i) as f64) / denom).exp();
        kernel.push(weight);
        sum += if i == 0 { weight } else { 2.0 * weight };
    }
    for weight in &mut kernel {
        *weight /= sum;
    }

    // Horizontal pass into the scratch buffer.
    scratch.clear();
    scratch.resize(width * height, 0.0);
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let mut acc = values[row + x] * kernel[0];
            for k in 1..=radius {
                let xl = x.saturating_sub(k);
                let xr = (x + k).min(width - 1);
                acc += (values[row + xl] + values[row + xr]) * kernel[k];
            }
            scratch[row + x] = acc;
        }
    }
    // Vertical pass back into the caller's buffer.
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let mut acc = scratch[row + x] * kernel[0];
            for k in 1..=radius {
                let yu = y.saturating_sub(k) * width + x;
                let yd = (y + k).min(height - 1) * width + x;
                acc += (scratch[yu] + scratch[yd]) * kernel[k];
            }
            values[row + x] = acc;
        }
    }
}

/// Circumference-like boundary length of a mask, in cell edges.
///
/// Counts edges between a `true` cell and a `false` cell (including open
/// boundary edges). Useful for coastline length statistics.
#[must_use]
pub fn perimeter_cells(mask: &[bool], grid: Grid) -> usize {
    let mut count = 0;
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            if !mask[grid.index(x, y)] {
                continue;
            }
            let neighbours = [
                grid.neighbor(x, y, 0),
                grid.neighbor(x, y, 2),
                grid.neighbor(x, y, 5),
                grid.neighbor(x, y, 7),
            ];
            for nb in neighbours {
                match nb {
                    Some((nx, ny)) if mask[grid.index(nx, ny)] => {}
                    _ => count += 1,
                }
            }
        }
    }
    count
}

/// Downsamples a field by integer factor, averaging blocks; returns a plain
/// vector of `width/factor * height/factor` values for display.
#[must_use]
pub fn downsample_mean<T: FieldScalar>(field: &Field<T>, factor: usize) -> Vec<f32> {
    let factor = factor.max(1);
    let grid = field.grid();
    let out_w = grid.width() / factor;
    let out_h = grid.height() / factor;
    let mut out = Vec::with_capacity(out_w * out_h);
    for by in 0..out_h {
        for bx in 0..out_w {
            let mut sum = 0.0;
            let mut n = 0.0f64;
            for y in by * factor..(by + 1) * factor {
                for x in bx * factor..(bx + 1) * factor {
                    sum += field.get(x, y).to_f64();
                    n += 1.0;
                }
            }
            out.push((sum / n.max(1.0)) as f32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BoundaryKind;

    fn grid(kind: BoundaryKind) -> Grid {
        Grid::new(16, 16, 1000.0, kind).unwrap()
    }

    #[test]
    fn distance_transform_of_single_source_matches_bruteforce() {
        let g = grid(BoundaryKind::Rectangular);
        let mut mask = vec![false; g.cell_count()];
        let src = g.index(3, 4);
        mask[src] = true;
        let d = distance_m(&mask, g);
        for y in 0..g.height() {
            for x in 0..g.width() {
                let dx = x as f64 - 3.0;
                let dy = y as f64 - 4.0;
                let want = (dx * dx + dy * dy).sqrt() * 1000.0;
                let got = f64::from(d.get(x, y));
                assert!((got - want).abs() < 1e-3, "({x},{y}): {got} vs {want}");
            }
        }
    }

    #[test]
    fn wrapped_distance_respects_torus() {
        let g = grid(BoundaryKind::Toroidal);
        let mut mask = vec![false; g.cell_count()];
        mask[g.index(0, 0)] = true;
        let d = distance_m(&mask, g);
        // One cell left of the source wraps to x=15, distance 1 cell.
        assert!((f64::from(d.get(15, 0)) - 1000.0).abs() < 1e-3);
        assert!((f64::from(d.get(0, 11)) - 5000.0).abs() < 1e-3);
        assert!((f64::from(d.get(15, 11)) - 1000.0 * 26f64.sqrt()).abs() < 1e-3);
    }

    #[test]
    fn blur_preserves_a_constant_field() {
        let g = grid(BoundaryKind::Toroidal);
        let mut f = Field::<f64>::filled(g, 7.0);
        gaussian_blur(&mut f, 3.0);
        for v in f.as_slice() {
            assert!((v - 7.0).abs() < 1e-9);
        }
    }

    #[test]
    fn blur_spreads_a_delta() {
        let g = grid(BoundaryKind::Toroidal);
        let mut f = Field::<f64>::zeros(g);
        f.set(8, 6, 1.0);
        gaussian_blur(&mut f, 2.0);
        assert!(f.get(8, 6) < 1.0);
        assert!(f.get(9, 6) > 0.0);
        assert!((f.sum() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn perimeter_of_a_single_cell_is_four() {
        let g = grid(BoundaryKind::Rectangular);
        let mut mask = vec![false; g.cell_count()];
        mask[g.index(5, 5)] = true;
        assert_eq!(perimeter_cells(&mask, g), 4);
    }

    #[test]
    fn downsample_averages_blocks() {
        let g = grid(BoundaryKind::Rectangular);
        let f = Field::<f64>::from_fn(g, |x, _| x as f64);
        let small = downsample_mean(&f, 2);
        assert_eq!(small.len(), (g.width() / 2) * (g.height() / 2));
        assert!((small[0] - 0.5) < 1e-6);
    }

    #[test]
    fn slice_blur_matches_field_blur_on_open_boundaries() {
        // `gaussian_blur` clamps on open axes, exactly like the slice version,
        // so the whole field must match bit for bit.
        let g = Grid::new(24, 18, 1000.0, BoundaryKind::Rectangular).unwrap();
        let mut field = Field::<f64>::from_fn(g, |x, y| ((x * 13 + y * 7) % 11) as f64 - 5.0);
        let original = field.as_slice().to_vec();
        gaussian_blur(&mut field, 2.0);
        let mut flat = original;
        let mut scratch = Vec::new();
        gaussian_blur_slice(&mut flat, &mut scratch, 24, 18, 2.0);
        assert_eq!(flat, field.as_slice());
        // The scratch is reusable: blurring a fresh copy with the same scratch
        // gives the identical result.
        let mut again = flat.clone();
        gaussian_blur_slice(&mut again, &mut scratch, 24, 18, 2.0);
        let mut expected = flat.clone();
        let mut fresh = Vec::new();
        gaussian_blur_slice(&mut expected, &mut fresh, 24, 18, 2.0);
        assert_eq!(again, expected);
    }

    #[test]
    fn slice_blur_preserves_constants_and_handles_trivial_sigmas() {
        let (w, h) = (32usize, 20usize);
        let mut values = vec![7.5f64; w * h];
        let mut scratch = Vec::new();
        gaussian_blur_slice(&mut values, &mut scratch, w, h, 3.25);
        for value in &values {
            assert!((value - 7.5).abs() < 1e-9, "constant changed: {value}");
        }
        // Non-positive sigma and empty input are no-ops, not panics.
        let snapshot = values.clone();
        gaussian_blur_slice(&mut values, &mut scratch, w, h, 0.0);
        assert_eq!(values, snapshot);
        let mut empty: Vec<f64> = Vec::new();
        gaussian_blur_slice(&mut empty, &mut scratch, 0, 0, 2.0);
        assert!(empty.is_empty());
    }
}
