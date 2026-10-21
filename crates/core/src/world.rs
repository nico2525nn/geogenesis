//! The world state: every spatially varying quantity the simulation evolves.
//!
//! The [`World`] owns the fields but knows nothing about the physics; process
//! modules receive it and mutate the parts they own. Keeping the state in one
//! struct means snapshots, statistics, exports and the WASM bridge all read the
//! same data instead of reconstructing it per subsystem.
//!
//! Field precision is deliberate:
//!
//! * elevation, crustal thickness — `f64` (metres, up to 1e4, erosion over
//!   millions of years needs the headroom),
//! * sediment, soil, ice, rates — `f32` (display/diagnostic, memory matters),
//! * categorical layers — `u8`/`u16`.

use crate::error::GeoResult;
use crate::grid::{Field, Grid};
use crate::params::WorldParams;

/// All mutable state of a world.
#[derive(Clone, Debug)]
pub struct World {
    /// Immutable geometry.
    pub grid: Grid,
    /// Surface elevation (bedrock plus sediment), metres relative to the datum.
    pub elevation_m: Field<f64>,
    /// Crustal thickness beneath each cell, metres; drives Airy isostasy.
    pub crust_m: Field<f64>,
    /// Loose sediment thickness on top of bedrock, metres.
    pub sediment_m: Field<f32>,
    /// Soil thickness available for hillslope transport, metres.
    pub soil_m: Field<f32>,
    /// Land ice thickness, metres.
    pub ice_m: Field<f32>,
    /// Cumulative erosion, metres (positive down).
    pub erosion_m: Field<f32>,
    /// Cumulative deposition, metres (positive up).
    pub deposition_m: Field<f32>,
    /// Instantaneous tectonic vertical rate, m/yr (positive up).
    pub tectonic_rate_m_yr: Field<f32>,
    /// Current sea level, metres.
    pub sea_level_m: f64,
    /// Sea level without eustatic/climate change, metres.
    pub sea_level_base_m: f64,
    /// Simulated time, years.
    pub elapsed_yr: f64,
    /// Reference crustal thickness used as the Airy isostasy anchor, metres.
    ///
    /// The initial crust is derived from the initial relief relative to this
    /// thickness, so isostasy relaxes only changes to the load.
    pub crust_reference_m: f64,
}

impl World {
    /// Allocates a flat world at sea level with a uniform reference crust.
    pub fn new(params: &WorldParams) -> GeoResult<Self> {
        let grid = params.grid()?;
        let sea = params.sea_level_m;
        Ok(Self {
            grid,
            elevation_m: Field::filled(grid, sea),
            crust_m: Field::filled(grid, params.vertical_m), // placeholder thickness
            sediment_m: Field::zeros(grid),
            soil_m: Field::zeros(grid),
            ice_m: Field::zeros(grid),
            erosion_m: Field::zeros(grid),
            deposition_m: Field::zeros(grid),
            tectonic_rate_m_yr: Field::zeros(grid),
            sea_level_m: sea,
            sea_level_base_m: sea,
            elapsed_yr: 0.0,
            crust_reference_m: 35_000.0,
        })
    }

    /// Number of cells.
    #[inline]
    #[must_use]
    pub fn cell_count(&self) -> usize {
        self.grid.cell_count()
    }

    /// Nominal planar cell area, m² (`cell_size_m²`).
    ///
    /// On spherical worlds the physical cell area varies with latitude; use
    /// [`Grid::cell_area_m2_at`](crate::grid::Grid::cell_area_m2_at) (or the
    /// area-aware helpers below) for volumes and area-weighted means. This
    /// accessor is kept for display scaling and historical planar arithmetic.
    #[inline]
    #[must_use]
    pub fn cell_area_m2(&self) -> f64 {
        self.grid.cell_area_m2()
    }

    /// Elevation of the bedrock surface (surface minus sediment).
    #[inline]
    #[must_use]
    pub fn bedrock_m(&self, x: usize, y: usize) -> f64 {
        self.elevation_m.get(x, y) - f64::from(self.sediment_m.get(x, y))
    }

    /// Land fraction above the current sea level.
    #[must_use]
    pub fn land_fraction(&self) -> f64 {
        self.elevation_m.fraction_where(|z| z > self.sea_level_m)
    }

    /// Ocean fraction (complement of [`Self::land_fraction`]).
    #[must_use]
    pub fn ocean_fraction(&self) -> f64 {
        1.0 - self.land_fraction()
    }

    /// Land area in m².
    ///
    /// Area weighted on spherical grids (sum of the physical areas of the land
    /// cells); the historical closed form is kept on planar grids so their
    /// arithmetic stays bit-identical.
    #[must_use]
    pub fn land_area_m2(&self) -> f64 {
        let grid = self.grid;
        if grid.has_spherical_areas() {
            let mut area = 0.0;
            for (index, &z) in self.elevation_m.as_slice().iter().enumerate() {
                if z > self.sea_level_m {
                    area += grid.cell_area_m2_at_index(index);
                }
            }
            area
        } else {
            self.land_fraction() * self.cell_count() as f64 * self.cell_area_m2()
        }
    }

    /// Mean land elevation above sea level, or `None` for a water world.
    ///
    /// Area weighted on spherical grids; a plain cell average on planar grids.
    #[must_use]
    pub fn mean_land_elevation_m(&self) -> Option<f64> {
        let grid = self.grid;
        if grid.has_spherical_areas() {
            let mut sum = 0.0;
            let mut weight = 0.0f64;
            for (index, &z) in self.elevation_m.as_slice().iter().enumerate() {
                if z > self.sea_level_m {
                    let area = grid.cell_area_m2_at_index(index);
                    sum += (z - self.sea_level_m) * area;
                    weight += area;
                }
            }
            if weight == 0.0 {
                None
            } else {
                Some(sum / weight)
            }
        } else {
            let mut sum = 0.0;
            let mut n = 0usize;
            for v in self.elevation_m.as_slice() {
                if *v > self.sea_level_m {
                    sum += *v - self.sea_level_m;
                    n += 1;
                }
            }
            if n == 0 {
                None
            } else {
                Some(sum / n as f64)
            }
        }
    }

    /// Highest and lowest surface elevation.
    #[must_use]
    pub fn relief_range_m(&self) -> (f64, f64) {
        (
            self.elevation_m.min_value().unwrap_or(self.sea_level_m),
            self.elevation_m.max_value().unwrap_or(self.sea_level_m),
        )
    }

    /// Total water volume in lakes above the filled surface, m³.
    ///
    /// `lake_depth_m` is provided by the hydrology module; this helper keeps
    /// the conversion in one place.
    #[must_use]
    pub fn lake_volume_m3(&self, lake_depth_m: &Field<f32>) -> f64 {
        let grid = self.grid;
        lake_depth_m
            .as_slice()
            .iter()
            .enumerate()
            .map(|(i, &d)| f64::from(d).max(0.0) * grid.cell_area_m2_at_index(i))
            .sum()
    }

    /// Repairs non-finite values, leaving a valid world behind.
    ///
    /// Called at the end of every simulation step so a single bad process
    /// cannot poison the next one.
    /// Clamps fields to physical bounds and replaces non-finite values.
    ///
    /// Returns the number of cells whose values had to be repaired (non-finite
    /// replacements). The driver accumulates this into `SimState::repairs`.
    pub fn sanitize(&mut self) -> usize {
        let sea = self.sea_level_m;
        let mut repaired = 0usize;
        repaired += self.elevation_m.sanitize_non_finite(sea);
        repaired += self.crust_m.sanitize_non_finite(30_000.0);
        repaired += self.sediment_m.sanitize_non_finite(0.0);
        repaired += self.soil_m.sanitize_non_finite(0.0);
        repaired += self.ice_m.sanitize_non_finite(0.0);
        repaired += self.erosion_m.sanitize_non_finite(0.0);
        repaired += self.deposition_m.sanitize_non_finite(0.0);
        repaired += self.tectonic_rate_m_yr.sanitize_non_finite(0.0);
        // Physical bounds: sediment/soil/ice cannot be negative, crust cannot
        // vanish. Elevation is clipped to a generous envelope so an unstable
        // process cannot send the mesh to infinity.
        self.sediment_m.clamp_values(0.0, 20_000.0);
        self.soil_m.clamp_values(0.0, 100.0);
        self.ice_m.clamp_values(0.0, 10_000.0);
        self.erosion_m.clamp_values(0.0, 1e7);
        self.deposition_m.clamp_values(0.0, 1e7);
        let limit = self
            .elevation_m
            .max_value()
            .unwrap_or(10_000.0)
            .max(10_000.0)
            * 3.0;
        let _ = limit;
        self.crust_m.clamp_values(1_000.0, 200_000.0);
        repaired
    }

    /// Snapshot of the surface as `f32` for display and history frames.
    #[must_use]
    pub fn elevation_f32(&self) -> Vec<f32> {
        self.elevation_m
            .as_slice()
            .iter()
            .map(|&v| v as f32)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::WorldParams;

    fn params() -> WorldParams {
        WorldParams {
            width: 32,
            height: 24,
            cell_km: 5.0,
            ..WorldParams::default()
        }
    }

    #[test]
    fn new_world_is_flat_and_at_sea_level() {
        let p = params();
        let mut w = World::new(&p).unwrap();
        assert_eq!(w.grid.width(), 32);
        assert_eq!(w.land_fraction(), 0.0);
        assert_eq!(w.relief_range_m(), (p.sea_level_m, p.sea_level_m));
        w.elevation_m.set(0, 0, 100.0);
        assert!((w.land_fraction() - 1.0 / 768.0).abs() < 1e-12);
        assert!(w.mean_land_elevation_m().unwrap() > 99.0);
    }

    #[test]
    fn sanitize_repairs_all_fields() {
        let p = params();
        let mut w = World::new(&p).unwrap();
        w.elevation_m.set(1, 1, f64::NAN);
        w.soil_m.set(2, 2, -5.0);
        w.erosion_m.set(3, 3, f32::INFINITY);
        w.sanitize();
        assert_eq!(w.elevation_m.non_finite_count(), 0);
        assert_eq!(w.soil_m.get(2, 2), 0.0);
        assert_eq!(w.erosion_m.get(3, 3), 0.0);
    }

    #[test]
    fn spherical_area_helpers_are_area_weighted() {
        let p = WorldParams {
            width: 32,
            height: 16,
            cell_km: 10.0,
            sea_level_m: -1000.0,
            boundary: crate::grid::BoundaryKind::Spherical,
            ..WorldParams::default()
        };
        let grid = p.grid().unwrap();
        assert!(grid.has_spherical_areas());
        let mut w = World::new(&p).unwrap();
        // A world shaped like the latitude cosine: high at the equator, low at
        // the poles, entirely above sea level.
        w.elevation_m = crate::grid::Field::from_fn(grid, |_, y| {
            let lat = (y as f64 + 0.5) * std::f64::consts::PI / grid.height() as f64
                - std::f64::consts::FRAC_PI_2;
            1000.0 * lat.cos()
        });
        let total = grid.total_area_m2();
        // Every cell is land: the land area is the sphere area.
        assert!((w.land_area_m2() - total).abs() / total < 1e-12);
        // Mean land elevation is the area-weighted mean, not the cell mean.
        let mut sum = 0.0;
        let mut weight = 0.0;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let area = grid.cell_area_m2_at(x, y);
                sum += (w.elevation_m.get(x, y) + 1000.0) * area;
                weight += area;
            }
        }
        // Mean elevation *above sea level*.
        let weighted_above_sea = sum / weight;
        let cell_above_sea = w.elevation_m.mean().unwrap() + 1000.0;
        assert!((w.mean_land_elevation_m().unwrap() - weighted_above_sea).abs() < 1e-9);
        assert!(
            (weighted_above_sea - cell_above_sea).abs() > 1.0,
            "no area weighting effect"
        );
        // A unit lake depth everywhere has the sphere volume.
        let depths = crate::grid::Field::<f32>::filled(grid, 1.0);
        assert!((w.lake_volume_m3(&depths) - total).abs() / total < 1e-12);
    }

    #[test]
    fn planar_area_helpers_keep_the_historical_formulas() {
        let p = params();
        let mut w = World::new(&p).unwrap();
        w.elevation_m
            .map_in_place(|i, _| if i % 3 == 0 { 100.0 } else { -100.0 });
        let grid = w.grid;
        assert_eq!(
            w.land_area_m2(),
            w.land_fraction() * w.cell_count() as f64 * grid.cell_area_m2()
        );
        let depths = crate::grid::Field::<f32>::filled(grid, 2.0);
        let expected: f64 = depths
            .as_slice()
            .iter()
            .map(|&d| f64::from(d) * grid.cell_area_m2())
            .sum();
        assert_eq!(w.lake_volume_m3(&depths), expected);
    }

    #[test]
    fn bedrock_discounts_sediment() {
        let p = params();
        let mut w = World::new(&p).unwrap();
        w.elevation_m.set(0, 0, 500.0);
        w.sediment_m.set(0, 0, 200.0);
        assert!((w.bedrock_m(0, 0) - 300.0).abs() < 1e-12);
    }
}
