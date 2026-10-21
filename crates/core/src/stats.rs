//! Terrain statistics, hypsometry, profiles and histograms.
//!
//! Statistics are computed from the same fields the renderer and exports see,
//! so a number shown in the UI always matches the visible world. Everything is
//! O(N log N) at most (the quantiles sort), which is negligible next to a
//! simulation step.

use crate::climate::Climate;
use crate::geology::Geology;
use crate::grid::Grid;
use crate::hydrology::Hydro;
use crate::world::World;

/// Elevation distribution summary, metres.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElevationStats {
    pub min_m: f64,
    pub max_m: f64,
    pub mean_m: f64,
    pub std_m: f64,
    pub median_m: f64,
    pub p10_m: f64,
    pub p90_m: f64,
    pub cells: usize,
}

impl ElevationStats {
    /// Total relief, metres.
    #[must_use]
    pub fn relief_m(&self) -> f64 {
        self.max_m - self.min_m
    }
}

/// Slope distribution summary.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlopeStats {
    pub mean_deg: f64,
    pub max_deg: f64,
    pub p90_deg: f64,
    pub mean_gradient: f64,
}

/// Land hypsometry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HypsometryStats {
    /// Land fraction of the whole grid.
    pub land_fraction: f64,
    /// Ocean fraction.
    pub ocean_fraction: f64,
    /// Mean land elevation above sea level, metres.
    pub mean_land_m: f64,
    /// Hypsometric integral (0..1).
    pub integral: f64,
    /// Curve: `(normalised elevation 0..1, fraction of land area above it)`.
    pub curve: Vec<(f64, f64)>,
}

/// One biome's share of the grid.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeShare {
    pub key: &'static str,
    pub label: &'static str,
    pub label_ja: &'static str,
    pub fraction: f64,
}

/// Full terrain report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainStats {
    pub grid_width: usize,
    pub grid_height: usize,
    pub cell_km: f64,
    pub elapsed_yr: f64,
    pub sea_level_m: f64,
    pub elevation: ElevationStats,
    pub slope: SlopeStats,
    pub hypsometry: HypsometryStats,
    pub biomes: Vec<BiomeShare>,
    pub mean_temp_c: f64,
    pub mean_precip_mm: f64,
    pub drainage_density_km_per_km2: f64,
    pub channel_cells: usize,
    pub lake_volume_km3: f64,
    pub land_area_km2: f64,
    pub max_relief_m: f64,
    pub mean_erosion_m: f64,
    pub mean_sediment_m: f64,
    pub mean_ice_m: f64,
}

/// Physical cell-area weights for spherical grids, `None` for planar grids.
///
/// The weight vector is row-major like the fields, so it can be zipped with
/// them directly. Planar grids deliberately return `None`: the statistics then
/// take the historical unweighted code path and stay bit-for-bit identical.
fn area_weights(grid: Grid) -> Option<Vec<f64>> {
    if !grid.has_spherical_areas() {
        return None;
    }
    let mut weights = Vec::with_capacity(grid.cell_count());
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            weights.push(grid.cell_area_m2_at(x, y));
        }
    }
    Some(weights)
}

/// Elevation at the weighted quantile `q` of `(value, area)` pairs sorted by
/// value. The returned value is the first order statistic whose cumulative
/// area reaches the target.
fn weighted_quantile(sorted: &[(f64, f64)], q: f64, total_area: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let target = q.clamp(0.0, 1.0) * total_area;
    let mut cumulative = 0.0;
    for &(value, area) in sorted {
        cumulative += area;
        if cumulative >= target {
            return value;
        }
    }
    sorted.last().map_or(0.0, |pair| pair.0)
}

/// Computes the full terrain report.
#[must_use]
pub fn compute(world: &World, climate: &Climate, hydro: &Hydro, geology: &Geology) -> TerrainStats {
    let grid = world.grid;
    let sea = world.sea_level_m;
    // Area weights for spherical worlds: means, land fraction, volumes and the
    // hypsometric curve are all weighted by physical cell area, so the small
    // polar cells no longer count as much as equatorial ones.
    let weights = area_weights(grid);
    let total_weight: f64 = weights.as_ref().map_or(0.0, |w| w.iter().sum());
    let mut values: Vec<f64> = world
        .elevation_m
        .as_slice()
        .iter()
        .map(|&z| if z.is_finite() { z } else { sea })
        .collect();
    let cells = values.len();
    let mean = match &weights {
        Some(w) => {
            values
                .iter()
                .zip(w.iter())
                .map(|(v, weight)| v * weight)
                .sum::<f64>()
                / total_weight
        }
        None => values.iter().sum::<f64>() / cells.max(1) as f64,
    };
    let variance = match &weights {
        Some(w) => {
            values
                .iter()
                .zip(w.iter())
                .map(|(v, weight)| (v - mean) * (v - mean) * weight)
                .sum::<f64>()
                / total_weight
        }
        None => values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / cells.max(1) as f64,
    };
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let quantile = |q: f64| -> f64 {
        if values.is_empty() {
            return sea;
        }
        let pos = q.clamp(0.0, 1.0) * (values.len() - 1) as f64;
        let lo = pos.floor() as usize;
        let hi = pos.ceil() as usize;
        let t = pos - lo as f64;
        values[lo] + (values[hi] - values[lo]) * t
    };
    let elevation = ElevationStats {
        min_m: values.first().copied().unwrap_or(sea),
        max_m: values.last().copied().unwrap_or(sea),
        mean_m: mean,
        std_m: variance.sqrt(),
        median_m: quantile(0.5),
        p10_m: quantile(0.10),
        p90_m: quantile(0.90),
        cells,
    };

    // Slopes.
    let mut slope_deg = Vec::with_capacity(cells);
    let mut gradient_sum = 0.0;
    let mut gradient_weight = 0.0;
    let mut slope_deg_sum = 0.0;
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let s = world.elevation_m.slope(x, y);
            let weight = weights.as_ref().map_or(1.0, |w| w[grid.index(x, y)]);
            gradient_sum += s * weight;
            gradient_weight += weight;
            slope_deg_sum += s.atan().to_degrees() * weight;
            slope_deg.push(s.atan().to_degrees());
        }
    }
    slope_deg.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let weight_denominator = if weights.is_some() {
        gradient_weight.max(f64::MIN_POSITIVE)
    } else {
        cells.max(1) as f64
    };
    let slope = SlopeStats {
        mean_deg: if slope_deg.is_empty() {
            0.0
        } else if weights.is_some() {
            slope_deg_sum / weight_denominator
        } else {
            slope_deg.iter().sum::<f64>() / slope_deg.len() as f64
        },
        max_deg: slope_deg.last().copied().unwrap_or(0.0),
        p90_deg: *slope_deg
            .get((slope_deg.len() as f64 * 0.9) as usize)
            .unwrap_or(&0.0),
        mean_gradient: gradient_sum / weight_denominator,
    };

    // Hypsometry over land only.
    let hypsometry = if let Some(w) = &weights {
        let total_area = total_weight;
        // Sanitised elevations in row-major order, so `w` stays aligned.
        let mut land: Vec<(f64, f64)> = world
            .elevation_m
            .as_slice()
            .iter()
            .zip(w.iter())
            .filter_map(|(&z, &area)| {
                let z = if z.is_finite() { z } else { sea };
                (z > sea).then_some((z, area))
            })
            .collect();
        let land_area: f64 = land.iter().map(|(_, area)| *area).sum();
        let land_fraction = if total_area > 0.0 {
            land_area / total_area
        } else {
            0.0
        };
        land.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
        let max_land = land.last().map_or(sea + 1.0, |pair| pair.0);
        let mean_land = if land_area > 0.0 {
            land.iter().map(|(z, area)| z * area).sum::<f64>() / land_area
        } else {
            0.0
        };
        let span = (max_land - sea).max(1.0);
        let mut curve = Vec::with_capacity(21);
        let mut integral = 0.0;
        let mut previous_area = 1.0f64;
        for k in 0..=20 {
            let q = k as f64 / 20.0;
            let height = if land.is_empty() {
                sea
            } else {
                weighted_quantile(&land, 1.0 - q, land_area)
            };
            let above = if land.is_empty() || land_area <= 0.0 {
                0.0
            } else {
                land.iter()
                    .filter(|(z, _)| *z >= height)
                    .map(|(_, area)| *area)
                    .sum::<f64>()
                    / land_area
            };
            let norm_height = ((height - sea) / span).clamp(0.0, 1.0);
            curve.push((norm_height, above));
            if k > 0 {
                integral += 0.5 * (above + previous_area) * (1.0 / 20.0);
            }
            previous_area = above;
        }
        HypsometryStats {
            land_fraction,
            ocean_fraction: 1.0 - land_fraction,
            mean_land_m: mean_land - sea,
            integral: integral.clamp(0.0, 1.0),
            curve,
        }
    } else {
        let land: Vec<f64> = values.iter().copied().filter(|&z| z > sea).collect();
        let land_fraction = land.len() as f64 / cells.max(1) as f64;
        let max_land = land.last().copied().unwrap_or(sea + 1.0);
        let mean_land = if land.is_empty() {
            0.0
        } else {
            land.iter().sum::<f64>() / land.len() as f64
        };
        let span = (max_land - sea).max(1.0);
        let mut curve = Vec::with_capacity(21);
        let mut integral = 0.0;
        let mut previous_area = 1.0f64;
        for k in 0..=20 {
            let q = k as f64 / 20.0;
            // Area fraction above a height. land is sorted ascending; the
            // height at quantile (1-q) has q of the land above it.
            let height = if land.is_empty() {
                sea
            } else {
                let idx = ((1.0 - q) * (land.len() - 1) as f64) as usize;
                land[idx.min(land.len() - 1)]
            };
            let above =
                land.iter().filter(|&&z| z >= height).count() as f64 / land.len().max(1) as f64;
            let norm_height = ((height - sea) / span).clamp(0.0, 1.0);
            curve.push((norm_height, above));
            if k > 0 {
                integral += 0.5 * (above + previous_area) * (1.0 / 20.0);
            }
            previous_area = above;
        }
        HypsometryStats {
            land_fraction,
            ocean_fraction: 1.0 - land_fraction,
            mean_land_m: mean_land - sea,
            integral: integral.clamp(0.0, 1.0),
            curve,
        }
    };

    let biomes = climate
        .biome_fractions_of_grid()
        .into_iter()
        .map(|(b, fraction)| BiomeShare {
            key: b.key(),
            label: b.label(),
            label_ja: b.label_ja(),
            fraction,
        })
        .collect();
    let land_area_km2 = if let Some(w) = &weights {
        world
            .elevation_m
            .as_slice()
            .iter()
            .zip(w.iter())
            .filter(|(z, _)| z.is_finite() && **z > sea)
            .map(|(_, weight)| *weight)
            .sum::<f64>()
            / 1e6
    } else {
        hypsometry.land_fraction * cells as f64 * (grid.cell_size_m() / 1000.0).powi(2)
    };
    let weighted_field_mean = |field: &crate::grid::Field<f32>| -> f64 {
        match &weights {
            Some(w) => {
                let mut sum = 0.0;
                let mut total = 0.0;
                for (i, &v) in field.as_slice().iter().enumerate() {
                    sum += f64::from(v) * w[i];
                    total += w[i];
                }
                if total > 0.0 {
                    sum / total
                } else {
                    0.0
                }
            }
            None => field.mean().unwrap_or(0.0),
        }
    };
    let mean_erosion = weighted_field_mean(&world.erosion_m);
    let mean_sediment = weighted_field_mean(&world.sediment_m);
    let mean_ice = weighted_field_mean(&world.ice_m);
    let _ = geology;

    TerrainStats {
        grid_width: grid.width(),
        grid_height: grid.height(),
        cell_km: grid.cell_size_m() / 1000.0,
        elapsed_yr: world.elapsed_yr,
        sea_level_m: sea,
        elevation,
        slope,
        hypsometry,
        biomes,
        mean_temp_c: climate.mean_land_temperature(),
        mean_precip_mm: climate.mean_land_precipitation(),
        drainage_density_km_per_km2: hydro.drainage_density_km_per_km2(),
        channel_cells: hydro.channel_cells(),
        lake_volume_km3: hydro.lake_volume_m3() / 1e9,
        land_area_km2,
        max_relief_m: values.last().copied().unwrap_or(sea)
            - values.first().copied().unwrap_or(sea),
        mean_erosion_m: mean_erosion,
        mean_sediment_m: mean_sediment,
        mean_ice_m: mean_ice,
    }
}
/// One sample along a profile.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProfileSample {
    /// Distance from the profile start, metres.
    pub distance_m: f64,
    /// Surface elevation, metres.
    pub elevation_m: f64,
    /// Water depth above the surface if submerged, otherwise 0.
    pub water_depth_m: f64,
    /// Exposed rock index.
    pub rock: u8,
    /// Biome index.
    pub biome: u8,
}

/// Streaming accumulator for tile-based statistics (large-grid generation).
///
/// Tiled generation never holds the whole elevation field in memory, so global
/// statistics are folded tile by tile. Only min/max/sum/sum-of-squares and a
/// land-cell count are kept, enough for mean, standard deviation, relief and
/// land fraction.
#[derive(Clone, Copy, Debug, Default)]
pub struct TileAccumulator {
    pub count: u64,
    pub min_m: f64,
    pub max_m: f64,
    pub sum_m: f64,
    pub sum_sq_m: f64,
    pub land_cells: u64,
}

impl TileAccumulator {
    #[must_use]
    pub fn new() -> Self {
        Self {
            count: 0,
            min_m: f64::INFINITY,
            max_m: f64::NEG_INFINITY,
            ..Self::default()
        }
    }

    /// Folds one tile of elevations into the accumulator.
    pub fn add_tile(&mut self, elevation: &[f64], sea_level_m: f64) {
        for &z in elevation {
            if !z.is_finite() {
                continue;
            }
            self.count += 1;
            self.min_m = self.min_m.min(z);
            self.max_m = self.max_m.max(z);
            self.sum_m += z;
            self.sum_sq_m += z * z;
            if z > sea_level_m {
                self.land_cells += 1;
            }
        }
    }

    /// Merges another partial result (for parallel tile workers).
    pub fn merge(&mut self, other: &Self) {
        if other.count == 0 {
            return;
        }
        self.count += other.count;
        self.min_m = self.min_m.min(other.min_m);
        self.max_m = self.max_m.max(other.max_m);
        self.sum_m += other.sum_m;
        self.sum_sq_m += other.sum_sq_m;
        self.land_cells += other.land_cells;
    }

    #[must_use]
    pub fn mean_m(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum_m / self.count as f64
        }
    }

    #[must_use]
    pub fn relief_m(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.max_m - self.min_m
        }
    }

    #[must_use]
    pub fn std_dev_m(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let mean = self.mean_m();
        (self.sum_sq_m / self.count as f64 - mean * mean)
            .max(0.0)
            .sqrt()
    }

    #[must_use]
    pub fn land_fraction(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.land_cells as f64 / self.count as f64
        }
    }
}

/// Samples a cross-section between two cell coordinates.
#[must_use]
pub fn profile(
    world: &World,
    climate: &Climate,
    geology: &Geology,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    samples: usize,
) -> Vec<ProfileSample> {
    let grid = world.grid;
    let n = samples.max(2);
    let dx = x1 - x0;
    let dy = y1 - y0;
    let length_m = (dx * dx + dy * dy).sqrt() * grid.cell_size_m();
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let t = k as f64 / (n - 1) as f64;
        let x = x0 + dx * t;
        let y = y0 + dy * t;
        let z = world.elevation_m.bilinear(x, y);
        let water = (world.sea_level_m - z).max(0.0);
        let xi = grid.clamp_x(x.round() as isize);
        let yi = grid.clamp_y(y.round() as isize);
        out.push(ProfileSample {
            distance_m: t * length_m,
            elevation_m: z,
            water_depth_m: water,
            rock: geology.rock_id.get(xi, yi),
            biome: climate.biome.get(xi, yi),
        });
    }
    out
}

/// Histogram of a slice into `bins` equal-width buckets over `[min, max]`.
#[must_use]
pub fn histogram(values: impl Iterator<Item = f64>, min: f64, max: f64, bins: usize) -> Vec<u32> {
    let bins = bins.max(1);
    let mut out = vec![0u32; bins];
    let span = (max - min).max(1e-9);
    for v in values {
        if !v.is_finite() {
            continue;
        }
        let t = ((v - min) / span).clamp(0.0, 0.999_999);
        out[(t * bins as f64) as usize] += 1;
    }
    out
}

/// Renders a stats report as JSON (used by the WASM ABI and the CLI).
#[must_use]
pub fn to_json(stats: &TerrainStats) -> String {
    use crate::params::push_json_number;
    let mut out = String::with_capacity(4000);
    out.push_str("{\"grid\":{\"width\":");
    out.push_str(&stats.grid_width.to_string());
    out.push_str(",\"height\":");
    out.push_str(&stats.grid_height.to_string());
    out.push_str(",\"cell_km\":");
    push_json_number(&mut out, stats.cell_km);
    out.push_str("},\"elapsed_yr\":");
    push_json_number(&mut out, stats.elapsed_yr);
    out.push_str(",\"sea_level_m\":");
    push_json_number(&mut out, stats.sea_level_m);
    out.push_str(",\"elevation\":{\"min\":");
    push_json_number(&mut out, stats.elevation.min_m);
    out.push_str(",\"max\":");
    push_json_number(&mut out, stats.elevation.max_m);
    out.push_str(",\"mean\":");
    push_json_number(&mut out, stats.elevation.mean_m);
    out.push_str(",\"std\":");
    push_json_number(&mut out, stats.elevation.std_m);
    out.push_str(",\"median\":");
    push_json_number(&mut out, stats.elevation.median_m);
    out.push_str(",\"p10\":");
    push_json_number(&mut out, stats.elevation.p10_m);
    out.push_str(",\"p90\":");
    push_json_number(&mut out, stats.elevation.p90_m);
    out.push_str(",\"cells\":");
    out.push_str(&stats.elevation.cells.to_string());
    out.push_str("},\"slope\":{\"mean_deg\":");
    push_json_number(&mut out, stats.slope.mean_deg);
    out.push_str(",\"max_deg\":");
    push_json_number(&mut out, stats.slope.max_deg);
    out.push_str(",\"p90_deg\":");
    push_json_number(&mut out, stats.slope.p90_deg);
    out.push_str(",\"mean_gradient\":");
    push_json_number(&mut out, stats.slope.mean_gradient);
    out.push_str("},\"hypsometry\":{\"land_fraction\":");
    push_json_number(&mut out, stats.hypsometry.land_fraction);
    out.push_str(",\"ocean_fraction\":");
    push_json_number(&mut out, stats.hypsometry.ocean_fraction);
    out.push_str(",\"mean_land_m\":");
    push_json_number(&mut out, stats.hypsometry.mean_land_m);
    out.push_str(",\"integral\":");
    push_json_number(&mut out, stats.hypsometry.integral);
    out.push_str(",\"curve\":[");
    for (i, (h, a)) in stats.hypsometry.curve.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        push_json_number(&mut out, *h);
        out.push(',');
        push_json_number(&mut out, *a);
        out.push(']');
    }
    out.push_str("]},\"biomes\":[");
    for (i, b) in stats.biomes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"key\":\"");
        out.push_str(b.key);
        out.push_str("\",\"label\":\"");
        out.push_str(b.label);
        out.push_str("\",\"label_ja\":\"");
        out.push_str(b.label_ja);
        out.push_str("\",\"fraction\":");
        push_json_number(&mut out, b.fraction);
        out.push('}');
    }
    out.push_str("],\"climate\":{\"mean_temp_c\":");
    push_json_number(&mut out, stats.mean_temp_c);
    out.push_str(",\"mean_precip_mm\":");
    push_json_number(&mut out, stats.mean_precip_mm);
    out.push_str("},\"hydrology\":{\"drainage_density\":");
    push_json_number(&mut out, stats.drainage_density_km_per_km2);
    out.push_str(",\"channel_cells\":");
    out.push_str(&stats.channel_cells.to_string());
    out.push_str(",\"lake_volume_km3\":");
    push_json_number(&mut out, stats.lake_volume_km3);
    out.push_str("},\"land_area_km2\":");
    push_json_number(&mut out, stats.land_area_km2);
    out.push_str(",\"max_relief_m\":");
    push_json_number(&mut out, stats.max_relief_m);
    out.push_str(",\"mean_erosion_m\":");
    push_json_number(&mut out, stats.mean_erosion_m);
    out.push_str(",\"mean_sediment_m\":");
    push_json_number(&mut out, stats.mean_sediment_m);
    out.push_str(",\"mean_ice_m\":");
    push_json_number(&mut out, stats.mean_ice_m);
    out.push('}');
    out
}

/// Grid used to draw a profile, exposed for the UI's minimap.
#[must_use]
pub fn profile_grid(world: &World) -> Grid {
    world.grid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::{compute as compute_climate, ClimateInputs};
    use crate::geology::Geology;
    use crate::hydrology::route;
    use crate::params::Params;
    use crate::plates::PlateMosaic;
    use crate::world::World;

    fn setup() -> (World, Climate, Hydro, Geology) {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        let grid = p.world.grid().unwrap();
        let mut world = World::new(&p.world).unwrap();
        world.elevation_m.map_in_place(|i, _| {
            let x = i % 32;
            -1000.0 + x as f64 * 60.0
        });
        let mosaic = PlateMosaic::build(&p, grid, 1);
        let geology = Geology::build(&p, &world, &mosaic, 1);
        let climate = compute_climate(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 1000.0,
        });
        let hydro = route(&world, &p, Some(&climate));
        (world, climate, hydro, geology)
    }

    #[test]
    fn elevation_and_hypsometry_are_consistent() {
        let (world, climate, hydro, geology) = setup();
        let stats = compute(&world, &climate, &hydro, &geology);
        assert_eq!(stats.elevation.cells, 1024);
        assert!(stats.elevation.max_m > stats.elevation.min_m);
        assert!(stats.hypsometry.land_fraction > 0.0);
        assert!(stats.hypsometry.integral >= 0.0 && stats.hypsometry.integral <= 1.0);
        assert!(!stats.biomes.is_empty());
    }

    #[test]
    fn histogram_counts_every_value() {
        let h = histogram([0.0, 1.0, 2.0, 3.0].into_iter(), 0.0, 4.0, 4);
        assert_eq!(h.iter().sum::<u32>(), 4);
        assert_eq!(h, vec![1, 1, 1, 1]);
    }

    #[test]
    fn profile_spans_the_requested_line() {
        let (world, climate, hydro, geology) = setup();
        let _ = (hydro, geology);
        let geology = Geology::build(
            &Params::default(),
            &world,
            &PlateMosaic::build(&Params::default(), world.grid, 1),
            1,
        );
        let samples = profile(&world, &climate, &geology, 0.0, 0.0, 31.0, 31.0, 40);
        assert_eq!(samples.len(), 40);
        assert!(samples.last().unwrap().distance_m > 0.0);
        assert!(samples.iter().all(|s| s.elevation_m.is_finite()));
    }

    #[test]
    fn stats_json_is_well_formed() {
        let (world, climate, hydro, geology) = setup();
        let stats = compute(&world, &climate, &hydro, &geology);
        let json = to_json(&stats);
        assert!(json.starts_with('{'));
        assert!(json.ends_with('}'));
        let mut depth = 0i32;
        for ch in json.chars() {
            match ch {
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0);
        }
        assert_eq!(depth, 0);
    }

    #[test]
    fn spherical_statistics_are_area_weighted() {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 16;
        p.world.cell_km = 10.0;
        p.world.boundary = crate::grid::BoundaryKind::Spherical;
        p.world.planet_radius_km = 6371.0;
        let grid = p.world.grid().unwrap();
        assert!(grid.has_spherical_areas());
        let mut world = World::new(&p.world).unwrap();
        // Equatorial bulge in elevation, polar lowlands: the cell-count mean and
        // the area-weighted mean differ substantially.
        world.elevation_m =
            crate::grid::Field::from_fn(grid, |_, y| 500.0 - (y as f64 - 7.5).abs() * 120.0);
        let mosaic = PlateMosaic::build(&p, grid, 1);
        let geology = Geology::build(&p, &world, &mosaic, 1);
        let climate = compute_climate(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 1000.0,
        });
        let hydro = route(&world, &p, Some(&climate));
        let stats = compute(&world, &climate, &hydro, &geology);
        assert_eq!(stats.elevation.cells, grid.cell_count());

        let mut total_area = 0.0;
        let mut weighted_sum = 0.0;
        let mut land_area = 0.0;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let area = grid.cell_area_m2_at(x, y);
                let z = world.elevation_m.get(x, y);
                total_area += area;
                weighted_sum += z * area;
                if z > world.sea_level_m {
                    land_area += area;
                }
            }
        }
        assert!((total_area - grid.total_area_m2()).abs() / total_area < 1e-12);
        let weighted_mean = weighted_sum / total_area;
        let naive_mean = world.elevation_m.sum() / grid.cell_count() as f64;
        assert!(
            (stats.elevation.mean_m - weighted_mean).abs() < 1e-9,
            "mean {} vs area-weighted {}",
            stats.elevation.mean_m,
            weighted_mean
        );
        assert!(
            (stats.elevation.mean_m - naive_mean).abs() > 1.0,
            "area weighting had no effect"
        );
        assert!(
            (stats.land_area_km2 - land_area / 1e6).abs() / (land_area / 1e6) < 1e-9,
            "land area {} vs {}",
            stats.land_area_km2,
            land_area / 1e6
        );
        assert!(
            (stats.hypsometry.land_fraction - land_area / total_area).abs() < 1e-12,
            "hypsometry land fraction"
        );
        assert!(
            (stats.hypsometry.integral - 0.0).abs() <= 1.0,
            "hypsometric integral out of range"
        );
        // Volume-type means (thickness fields) are area weighted too.
        world.erosion_m.fill_value(3.0);
        world.ice_m.fill_value(0.5);
        let stats = compute(&world, &climate, &hydro, &geology);
        assert!((stats.mean_erosion_m - 3.0).abs() < 1e-12);
        assert!((stats.mean_ice_m - 0.5).abs() < 1e-12);
    }

    #[test]
    fn tile_accumulator_matches_direct_statistics() {
        let tile_a = [100.0, 200.0, -300.0, 0.0];
        let tile_b = [50.0, 400.0, -100.0, 25.0];
        let mut acc = TileAccumulator::new();
        acc.add_tile(&tile_a, 0.0);
        acc.add_tile(&tile_b, 0.0);
        let all: Vec<f64> = tile_a.iter().chain(tile_b.iter()).copied().collect();
        let mean = all.iter().sum::<f64>() / all.len() as f64;
        assert_eq!(acc.count, 8);
        assert!((acc.mean_m() - mean).abs() < 1e-12);
        assert_eq!(acc.min_m, -300.0);
        assert_eq!(acc.max_m, 400.0);
        assert_eq!(acc.relief_m(), 700.0);
        assert_eq!(acc.land_fraction(), 0.625);
        let variance = all.iter().map(|z| (z - mean) * (z - mean)).sum::<f64>() / 8.0;
        assert!((acc.std_dev_m() - variance.sqrt()).abs() < 1e-9);
        let mut left = TileAccumulator::new();
        left.add_tile(&tile_a, 0.0);
        let mut right = TileAccumulator::new();
        right.add_tile(&tile_b, 0.0);
        left.merge(&right);
        assert!((left.mean_m() - acc.mean_m()).abs() < 1e-12);
        assert_eq!(left.land_fraction(), acc.land_fraction());
    }
}
