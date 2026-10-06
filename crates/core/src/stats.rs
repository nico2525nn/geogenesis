//! Terrain statistics: elevation, slope and hypsometry summaries.
//!
//! The UI's analysis panel and the CLI's report both read from here, so every
//! number is defined once.

use crate::grid::ScalarField;

/// Elevation summary in metres.
#[derive(Debug, Clone, PartialEq)]
pub struct ElevationStats {
    pub min_m: f64,
    pub max_m: f64,
    pub mean_m: f64,
    pub std_m: f64,
    pub median_m: f64,
    pub p10_m: f64,
    pub p90_m: f64,
    pub cells: u64,
}

/// Slope summary in degrees.
#[derive(Debug, Clone, PartialEq)]
pub struct SlopeStats {
    pub mean_deg: f64,
    pub max_deg: f64,
    pub ruggedness: f64,
}

/// Hypsometric summary: land fraction above sea level and the curve itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Hypsometry {
    /// Fraction of cells above sea level.
    pub land_fraction: f64,
    /// 16-bin elevation histogram, normalised to sum to 1.
    pub bins: [f64; 16],
    /// Hypsometric integral: mean normalised elevation.
    pub integral: f64,
}

/// Full terrain report.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainStats {
    pub elevation: ElevationStats,
    pub slope: SlopeStats,
    pub hypsometry: Hypsometry,
}

/// Computes elevation statistics.
pub fn elevation_stats(field: &ScalarField) -> ElevationStats {
    ElevationStats {
        min_m: field.min(),
        max_m: field.max(),
        mean_m: field.mean(),
        std_m: field.std_dev(),
        median_m: field.quantile(0.5),
        p10_m: field.quantile(0.1),
        p90_m: field.quantile(0.9),
        cells: field.len() as u64,
    }
}

/// Computes slope statistics in degrees.
pub fn slope_stats(field: &ScalarField, cell_size_m: f64) -> SlopeStats {
    let b = *field.boundary();
    let mut sum = 0.0f64;
    let mut max = 0.0f64;
    let mut n = 0u64;
    for y in 0..b.height() {
        for x in 0..b.width() {
            let s = field.slope(x, y, cell_size_m.max(1e-9));
            if !s.is_finite() {
                continue;
            }
            let deg = s.atan().to_degrees();
            sum += deg;
            max = max.max(deg);
            n += 1;
        }
    }
    let mean = if n > 0 { sum / n as f64 } else { 0.0 };
    // Ruggedness (Riley TRI analogue): mean absolute neighbour difference.
    let mut tri = 0.0;
    let mut tri_n = 0u64;
    for y in 1..b.height().saturating_sub(1) {
        for x in 1..b.width().saturating_sub(1) {
            let z = field.get(x, y);
            let mut local = 0.0;
            for &(dx, dy) in crate::grid::D8_OFFSETS.iter() {
                local += (field.get_wrapped(x as i64 + dx, y as i64 + dy) - z).abs();
            }
            tri += local / 8.0;
            tri_n += 1;
        }
    }
    SlopeStats {
        mean_deg: mean,
        max_deg: max,
        ruggedness: if tri_n > 0 { tri / tri_n as f64 } else { 0.0 },
    }
}

/// Computes hypsometry against `sea_level_m`.
pub fn hypsometry(field: &ScalarField, sea_level_m: f64) -> Hypsometry {
    let (lo, hi) = (field.min(), field.max());
    let span = (hi - lo).max(1e-9);
    let mut bins = [0.0f64; 16];
    let mut land = 0u64;
    for &v in field.as_slice() {
        if v > sea_level_m {
            land += 1;
        }
        let mut b = ((v - lo) / span * 16.0).floor() as usize;
        b = b.min(15);
        bins[b] += 1.0;
    }
    let n = field.len().max(1) as f64;
    for b in bins.iter_mut() {
        *b /= n;
    }
    // Hypsometric integral: area under the normalised curve.
    let integral = bins
        .iter()
        .enumerate()
        .map(|(i, h)| (i as f64 + 0.5) / 16.0 * h)
        .sum();
    Hypsometry {
        land_fraction: land as f64 / n,
        bins,
        integral,
    }
}

/// Full report for a field.
pub fn terrain_stats(field: &ScalarField, cell_size_m: f64, sea_level_m: f64) -> TerrainStats {
    TerrainStats {
        elevation: elevation_stats(field),
        slope: slope_stats(field, cell_size_m),
        hypsometry: hypsometry(field, sea_level_m),
    }
}

/// Cross-section profile along a row, in metres.
pub fn profile_row(field: &ScalarField, y: usize, samples: usize) -> Vec<f64> {
    let b = *field.boundary();
    let y = y.min(b.height().saturating_sub(1));
    (0..samples.max(1))
        .map(|i| {
            let x = (i as f64 / samples.max(1) as f64 * b.width() as f64) as usize;
            field.get(x.min(b.width().saturating_sub(1)), y)
        })
        .collect()
}

/// Cross-section profile along a column, in metres.
pub fn profile_column(field: &ScalarField, x: usize, samples: usize) -> Vec<f64> {
    let b = *field.boundary();
    let x = x.min(b.width().saturating_sub(1));
    (0..samples.max(1))
        .map(|i| {
            let y = (i as f64 / samples.max(1) as f64 * b.height() as f64) as usize;
            field.get(x, y.min(b.height().saturating_sub(1)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Boundary, BoundaryKind};

    fn test_grid() -> Boundary {
        Boundary::new(BoundaryKind::Rectangular, 24, 24)
    }

    #[test]
    fn stats_match_field() {
        let b = test_grid();
        let mut f = ScalarField::zeros(b);
        for i in 0..b.cell_count() {
            f.set_index(i, i as f64);
        }
        let s = elevation_stats(&f);
        assert_eq!(s.min_m, 0.0);
        assert_eq!(s.max_m, (b.cell_count() - 1) as f64);
        assert_eq!(s.cells, b.cell_count() as u64);
        assert!(s.p10_m < s.median_m && s.median_m < s.p90_m);
    }

    #[test]
    fn flat_field_has_zero_slope() {
        let b = test_grid();
        let f = ScalarField::filled(b, 100.0);
        let s = slope_stats(&f, 50.0);
        assert_eq!(s.mean_deg, 0.0);
        assert_eq!(s.max_deg, 0.0);
    }

    #[test]
    fn hypsometry_bins_sum_to_one() {
        let b = test_grid();
        let mut f = ScalarField::zeros(b);
        for i in 0..b.cell_count() {
            f.set_index(i, (i % 100) as f64);
        }
        let h = hypsometry(&f, 50.0);
        assert!((h.bins.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!((0.0..=1.0).contains(&h.land_fraction));
        assert!((0.0..=1.0).contains(&h.integral));
    }

    #[test]
    fn profiles_sample_the_right_axis() {
        let b = test_grid();
        let mut f = ScalarField::zeros(b);
        for y in 0..24 {
            for x in 0..24 {
                f.set(x, y, x as f64 * 10.0 + y as f64);
            }
        }
        let row = profile_row(&f, 5, 24);
        assert_eq!(row.len(), 24);
        assert!((row[0] - 5.0).abs() < 1e-9);
        assert!((row[23] - 235.0).abs() < 1e-9);
        let col = profile_column(&f, 5, 24);
        assert!((col[0] - 50.0).abs() < 1e-9);
    }
}