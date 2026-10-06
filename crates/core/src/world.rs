//! World configuration and grid geometry (spec Phase 0).
//!
//! Every parameter below carries its spec range. [`WorldConfig::validate`]
//! enforces them; out-of-range input is an error, never a silent clamp.

use crate::grid::{Boundary, BoundaryKind};
use crate::GeoError;

/// Coordinate system (spec: 座標系).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoordinateSystem {
    #[default]
    Planar,
    Equirectangular,
    SphericalHex,
}

/// World configuration (spec Phase 0 table).
#[derive(Debug, Clone, PartialEq)]
pub struct WorldConfig {
    pub name: String,
    /// 0 = random (drawn from OS entropy once, then fixed).
    pub seed: u64,
    /// 16×16 … 16384×16384.
    pub grid: (u32, u32),
    /// 1×1 … 64×64 tile subdivision for memory-bounded evaluation.
    pub tiles: (u32, u32),
    /// 1.0 … 10,000 km of real distance per grid edge.
    pub spatial_km: f64,
    /// 100 … 50,000 m of elevation amplitude.
    pub vertical_m: f64,
    /// −5000 … +5000 m sea-level datum.
    pub sea_level_m: f64,
    /// 1e6 … 1e8 m planetary radius (planar mode ignores it).
    pub planet_radius_m: f64,
    pub coordinates: CoordinateSystem,
    pub boundary: BoundaryKind,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            name: "Untitled".into(),
            seed: 0xC0FFEE,
            grid: (256, 256),
            tiles: (1, 1),
            spatial_km: 200.0,
            vertical_m: 5000.0,
            sea_level_m: 0.0,
            planet_radius_m: 6_371_000.0,
            coordinates: CoordinateSystem::Planar,
            boundary: BoundaryKind::Rectangular,
        }
    }
}

impl WorldConfig {
    /// Physical cell size in metres.
    pub fn cell_size_m(&self) -> f64 {
        self.spatial_km * 1000.0 / self.grid.0.max(1) as f64
    }

    /// Total cell count.
    pub fn cell_count(&self) -> usize {
        self.grid.0 as usize * self.grid.1 as usize
    }

    /// Grid topology.
    pub fn boundary(&self) -> Boundary {
        Boundary::new(self.boundary, self.grid.0 as usize, self.grid.1 as usize)
    }

    /// Effective seed: 0 means "draw once from OS entropy".
    pub fn effective_seed(&self) -> u64 {
        if self.seed == 0 {
            random_seed()
        } else {
            self.seed
        }
    }

    /// Enforces every spec Phase 0 range.
    pub fn validate(&self) -> Result<(), GeoError> {
        let (w, h) = self.grid;
        if w < 16 || h < 16 || w > 16384 || h > 16384 {
            return Err(GeoError::InvalidGrid(format!(
                "grid {w}×{h} outside 16…16384"
            )));
        }
        let (tw, th) = self.tiles;
        if tw < 1 || th < 1 || tw > 64 || th > 64 {
            return Err(GeoError::InvalidGrid(format!(
                "tiles {tw}×{th} outside 1…64"
            )));
        }
        check("spatial_km", self.spatial_km, 1.0, 10_000.0)?;
        check("vertical_m", self.vertical_m, 100.0, 50_000.0)?;
        check("sea_level_m", self.sea_level_m, -5000.0, 5000.0)?;
        check("planet_radius_m", self.planet_radius_m, 1e6, 1e8)?;
        Ok(())
    }
}

fn check(name: &'static str, v: f64, lo: f64, hi: f64) -> Result<(), GeoError> {
    if !v.is_finite() || v < lo || v > hi {
        return Err(GeoError::InvalidParameter {
            name,
            value: format!("{v}"),
            reason: format!("outside {lo}…{hi}"),
        });
    }
    Ok(())
}

/// One OS-entropy seed. Reads /dev/urandom where available; falls back to a
/// time-and-pid mix that is still unique per run (never per call site).
fn random_seed() -> u64 {
    #[cfg(target_family = "unix")]
    {
        if let Ok(bytes) = std::fs::read("/dev/urandom") {
            if bytes.len() >= 8 {
                let mut b = [0u8; 8];
                b.copy_from_slice(&bytes[..8]);
                let s = u64::from_le_bytes(b);
                if s != 0 {
                    return s;
                }
            }
        }
    }
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x1234_5678);
    crate::rng::mix64(t ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9))
}

/// A generated world: elevation plus the auxiliary fields the UI needs.
#[derive(Debug, Clone)]
pub struct World {
    pub config: WorldConfig,
    pub elevation_m: crate::grid::ScalarField,
    /// Cumulative denudation per cell, for the geology lookup.
    pub eroded_m: crate::grid::ScalarField,
    /// Regolith thickness per cell.
    pub soil_m: crate::grid::ScalarField,
    pub sea_level_m: f64,
    /// Simulated years elapsed.
    pub elapsed_yr: f64,
}

impl World {
    /// Allocates a flat world at sea level.
    pub fn flat(config: &WorldConfig) -> Result<Self, GeoError> {
        config.validate()?;
        let b = config.boundary();
        Ok(Self {
            config: config.clone(),
            elevation_m: crate::grid::ScalarField::filled(b, config.sea_level_m),
            eroded_m: crate::grid::ScalarField::zeros(b),
            soil_m: crate::grid::ScalarField::zeros(b),
            sea_level_m: config.sea_level_m,
            elapsed_yr: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        WorldConfig::default().validate().expect("default invalid");
    }

    #[test]
    fn grid_limits_are_enforced() {
        let mut c = WorldConfig::default();
        c.grid = (8, 8);
        assert!(c.validate().is_err());
        c.grid = (20000, 20000);
        assert!(c.validate().is_err());
        c.grid = (256, 256);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn parameter_ranges_are_enforced() {
        let mut c = WorldConfig::default();
        c.spatial_km = 0.5;
        assert!(c.validate().is_err());
        c.spatial_km = 200.0;
        c.vertical_m = 60_000.0;
        assert!(c.validate().is_err());
        c.vertical_m = 5000.0;
        c.sea_level_m = 6000.0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn cell_size_matches_extent() {
        let c = WorldConfig {
            grid: (100, 100),
            spatial_km: 100.0,
            ..Default::default()
        };
        assert!((c.cell_size_m() - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn nonzero_seed_is_stable() {
        let c = WorldConfig { seed: 42, ..Default::default() };
        assert_eq!(c.effective_seed(), 42);
    }

    #[test]
    fn flat_world_starts_at_sea_level() {
        let c = WorldConfig::default();
        let w = World::flat(&c).unwrap();
        assert_eq!(w.elevation_m.max(), c.sea_level_m);
        assert_eq!(w.elapsed_yr, 0.0);
    }
}