//! Climate: temperature and precipitation fields, and the biome map.
//!
//! Temperature follows elevation (lapse rate), latitude and continentality;
//! precipitation is computed by [`rainfall`](crate::erosion::rainfall) and
//! consumed here. The biome per cell is the extended Whittaker classification
//! over `(temperature, precipitation)` with elevation and soil modifiers from
//! spec §5b.

use crate::erosion::rainfall::{precipitation, RainfallParams};
use crate::grid::{Boundary, LabelField, ScalarField};
use crate::linalg::clamp;

/// Climate parameters, matching spec §5a ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct ClimateParams {
    /// −90.0 … 90.0°: latitude of the domain centre (spherical mode).
    pub latitude_deg: f64,
    /// −50.0 … 50.0 °C: sea-level mean annual temperature.
    pub mean_temp_c: f64,
    /// 0.0 … 30.0 °C: diurnal range.
    pub diurnal_range_c: f64,
    /// 0.0 … 50.0 °C: seasonal range.
    pub annual_range_c: f64,
    /// 0.0 … 1.0: maritime moderation of extremes.
    pub ocean_moderation: f64,
    /// 0.0 … 1.0: inland amplification of extremes.
    pub continentality: f64,
}

impl Default for ClimateParams {
    fn default() -> Self {
        Self {
            latitude_deg: 35.0,
            mean_temp_c: 15.0,
            diurnal_range_c: 10.0,
            annual_range_c: 22.0,
            ocean_moderation: 0.4,
            continentality: 0.4,
        }
    }
}

/// Vegetation parameters, matching spec §5b ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct VegetationParams {
    /// 0.0 … 1.0: baseline cover fraction.
    pub density: f64,
    /// 0.1 … 50.0 m: rooting depth.
    pub root_depth_m: f64,
    /// 0.0 … 1.0: soil-binding strength.
    pub root_strength: f64,
    /// 0.0 … 1.0 m: litter-layer thickness.
    pub litter_m: f64,
    /// 0.0 … 10.0 kg/m²/yr: productivity.
    pub productivity: f64,
    /// 0.0 … 1.0: succession speed toward the climatic climax.
    pub succession: f64,
}

impl Default for VegetationParams {
    fn default() -> Self {
        Self {
            density: 0.6,
            root_depth_m: 2.0,
            root_strength: 0.5,
            litter_m: 0.05,
            productivity: 1.2,
            succession: 0.2,
        }
    }
}

/// The nine biome types from spec §5b.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Biome {
    TropicalRainforest = 0,
    TemperateForest = 1,
    BorealForest = 2,
    Grassland = 3,
    Desert = 4,
    Tundra = 5,
    #[default]
    IceCap = 6,
    Mediterranean = 7,
    Savanna = 8,
}

impl Biome {
    /// All nine biomes.
    pub const ALL: [Biome; 9] = [
        Biome::TropicalRainforest,
        Biome::TemperateForest,
        Biome::BorealForest,
        Biome::Grassland,
        Biome::Desert,
        Biome::Tundra,
        Biome::IceCap,
        Biome::Mediterranean,
        Biome::Savanna,
    ];
}

/// Annual mean temperature field in °C.
pub fn temperature_field(
    elevation: &ScalarField,
    params: &ClimateParams,
    lapse_c_km: f64,
    cell_size_m: f64,
) -> ScalarField {
    let b = *elevation.boundary();
    let (w, h) = (b.width(), b.height());
    let mut out = ScalarField::zeros(b);
    let lat0 = params.latitude_deg.clamp(-90.0, 90.0);
    let _ = cell_size_m;

    for y in 0..h {
        for x in 0..w {
            let z = elevation.get(x, y);
            // Latitude from the row: top row is lat0 + half-span, bottom minus.
            let lat = lat0 + (0.5 - y as f64 / h.max(1) as f64) * 60.0;
            let lat = lat.clamp(-90.0, 90.0);
            // Solar geometry: cosine falloff from the equator.
            let solar = lat.to_radians().cos().max(0.0);
            let base = params.mean_temp_c - 28.0 * (1.0 - solar);
            // Continentality sharpens the latitudinal gradient.
            let continental = 1.0 + params.continentality * 0.4 * (1.0 - solar);
            let t = base * continental - lapse_c_km * z / 1000.0;
            out.set(x, y, t);
        }
    }
    out.sanitize_non_finite(params.mean_temp_c);
    out
}

/// Seasonal temperature amplitude in °C at a cell.
pub fn seasonal_amplitude_c(params: &ClimateParams, latitude_deg: f64) -> f64 {
    let lat_factor = (latitude_deg.abs() / 90.0).clamp(0.0, 1.0);
    let raw = params.annual_range_c * (0.3 + 0.7 * lat_factor);
    raw * (1.0 - params.ocean_moderation * 0.6)
}

/// Extended Whittaker classification with elevation and soil modifiers.
///
/// Returns the biome for `(temp_c, rain_mm)` following the classic diagram:
/// hot+wet → rainforest, hot+dry → desert, cold → tundra/ice, with temperate
/// and boreal bands between. Elevation pushes alpine cells toward tundra, thin
/// soil toward desert/steppe.
pub fn classify_biome(
    temp_c: f64,
    rain_mm: f64,
    elevation_m: f64,
    soil_m: f64,
) -> Biome {
    if !temp_c.is_finite() || !rain_mm.is_finite() {
        return Biome::IceCap;
    }
    // Elevation override: above ~4000 m nothing but ice and rock survives.
    if elevation_m > 4500.0 || temp_c < -18.0 {
        return Biome::IceCap;
    }
    if elevation_m > 3200.0 || temp_c < -8.0 {
        return Biome::Tundra;
    }
    // Thin soil on dry ground cannot hold forest.
    let dry_soil = soil_m < 0.2 && rain_mm < 600.0;

    if temp_c >= 22.0 {
        if rain_mm >= 1750.0 {
            return Biome::TropicalRainforest;
        }
        if rain_mm >= 750.0 {
            return Biome::Savanna;
        }
        return Biome::Desert;
    }
    if temp_c >= 12.0 {
        if rain_mm < 350.0 || dry_soil && rain_mm < 500.0 {
            return Biome::Desert;
        }
        if rain_mm < 700.0 {
            // Dry-summer subtropics read as Mediterranean.
            return Biome::Mediterranean;
        }
        if rain_mm >= 900.0 {
            return Biome::TemperateForest;
        }
        return Biome::Grassland;
    }
    if temp_c >= 2.0 {
        if rain_mm < 300.0 {
            return Biome::Desert;
        }
        if rain_mm >= 600.0 {
            return Biome::BorealForest;
        }
        return Biome::Grassland;
    }
    // 2 °C … −8 °C: boreal forest where wet, tundra/steppe where dry.
    if rain_mm >= 400.0 {
        Biome::BorealForest
    } else {
        Biome::Tundra
    }
}

/// Full climate state: temperature, precipitation, vegetation cover, biomes.
#[derive(Debug, Clone)]
pub struct Climate {
    pub temperature_c: ScalarField,
    pub precipitation_mm: ScalarField,
    pub vegetation: ScalarField,
    pub biomes: LabelField,
}

/// Computes climate from elevation, soils and parameters.
pub fn compute_climate(
    elevation: &ScalarField,
    soil_m: &ScalarField,
    climate: &ClimateParams,
    vegetation: &VegetationParams,
    rain: &RainfallParams,
    seed: u64,
    cell_size_m: f64,
    lapse_c_km: f64,
) -> Climate {
    let b = *elevation.boundary();
    let temperature_c = temperature_field(elevation, climate, lapse_c_km, cell_size_m);
    let precipitation_mm = precipitation(elevation, rain, seed, cell_size_m);

    let mut veg = ScalarField::zeros(b);
    let mut biomes = LabelField::filled(b, Biome::IceCap as u32);
    for i in 0..b.cell_count() {
        let t = temperature_c.get_index(i);
        let r = precipitation_mm.get_index(i);
        let (x, y) = b.coords(i);
        let biome = classify_biome(t, r, elevation.get(x, y), soil_m.get_index(i));
        biomes.set_index(i, biome as u32);
        // Cover follows the biome and the baseline density, with thin soil
        // limiting establishment.
        let climax = biome_cover(biome);
        let soil_limit = clamp(soil_m.get_index(i) / 0.5, 0.15, 1.0);
        veg.set_index(i, clamp(vegetation.density * climax * soil_limit, 0.0, 1.0));
    }

    Climate {
        temperature_c,
        precipitation_mm,
        vegetation: veg,
        biomes,
    }
}

/// Climax vegetation cover per biome.
pub fn biome_cover(biome: Biome) -> f64 {
    match biome {
        Biome::TropicalRainforest => 1.0,
        Biome::TemperateForest => 0.95,
        Biome::BorealForest => 0.85,
        Biome::Grassland => 0.6,
        Biome::Savanna => 0.55,
        Biome::Mediterranean => 0.6,
        Biome::Desert => 0.08,
        Biome::Tundra => 0.3,
        Biome::IceCap => 0.0,
    }
}

/// Fraction of cells in each biome.
pub fn biome_fractions(climate: &Climate) -> [f64; 9] {
    let mut counts = [0.0f64; 9];
    for &v in climate.biomes.as_slice() {
        if (v as usize) < 9 {
            counts[v as usize] += 1.0;
        }
    }
    let n = climate.biomes.len().max(1) as f64;
    for c in counts.iter_mut() {
        *c /= n;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BoundaryKind;

    fn test_grid() -> Boundary {
        Boundary::new(BoundaryKind::Rectangular, 24, 24)
    }

    #[test]
    fn temperature_falls_with_elevation_and_latitude() {
        let b = test_grid();
        let mut z = ScalarField::zeros(b);
        for y in 0..24 {
            for x in 0..24 {
                z.set(x, y, y as f64 * 200.0); // south high, north low
            }
        }
        let t = temperature_field(&z, &ClimateParams::default(), 6.5, 100.0);
        // Higher elevation + higher latitude must both cool.
        assert!(t.get(12, 0) > t.get(12, 23), "latitude gradient inverted");
        let mut flat = ScalarField::zeros(b);
        for y in 0..24 {
            for x in 0..24 {
                flat.set(x, y, x as f64 * 100.0);
            }
        }
        let t2 = temperature_field(&flat, &ClimateParams::default(), 6.5, 100.0);
        assert!(t2.get(0, 12) > t2.get(23, 12), "lapse rate inverted");
    }

    #[test]
    fn seasonal_amplitude_grows_toward_the_poles() {
        let p = ClimateParams::default();
        assert!(seasonal_amplitude_c(&p, 70.0) > seasonal_amplitude_c(&p, 10.0));
        assert!(seasonal_amplitude_c(&p, 0.0) >= 0.0);
    }

    #[test]
    fn ocean_moderation_dampens_seasons() {
        let maritime = ClimateParams { ocean_moderation: 1.0, ..Default::default() };
        let inland = ClimateParams { ocean_moderation: 0.0, ..Default::default() };
        assert!(seasonal_amplitude_c(&maritime, 50.0) < seasonal_amplitude_c(&inland, 50.0));
    }

    #[test]
    fn whittaker_hot_wet_is_rainforest() {
        assert_eq!(classify_biome(26.0, 2500.0, 100.0, 2.0), Biome::TropicalRainforest);
        assert_eq!(classify_biome(28.0, 200.0, 100.0, 2.0), Biome::Desert);
        assert_eq!(classify_biome(26.0, 1200.0, 100.0, 2.0), Biome::Savanna);
    }

    #[test]
    fn whittaker_temperate_band() {
        assert_eq!(classify_biome(15.0, 1200.0, 200.0, 2.0), Biome::TemperateForest);
        assert_eq!(classify_biome(15.0, 500.0, 200.0, 2.0), Biome::Mediterranean);
        assert_eq!(classify_biome(15.0, 200.0, 200.0, 2.0), Biome::Desert);
    }

    #[test]
    fn whittaker_cold_is_boreal_or_tundra_or_ice() {
        assert_eq!(classify_biome(5.0, 800.0, 500.0, 2.0), Biome::BorealForest);
        assert_eq!(classify_biome(-2.0, 500.0, 500.0, 2.0), Biome::BorealForest);
        assert_eq!(classify_biome(-20.0, 300.0, 500.0, 2.0), Biome::IceCap);
        assert_eq!(classify_biome(0.0, 200.0, 500.0, 2.0), Biome::Tundra);
    }

    #[test]
    fn elevation_overrides_to_alpine() {
        assert_eq!(classify_biome(20.0, 2000.0, 5000.0, 2.0), Biome::IceCap);
        assert_eq!(classify_biome(20.0, 2000.0, 3500.0, 2.0), Biome::Tundra);
    }

    #[test]
    fn non_finite_inputs_map_to_ice() {
        assert_eq!(classify_biome(f64::NAN, 1000.0, 0.0, 1.0), Biome::IceCap);
        assert_eq!(classify_biome(15.0, f64::INFINITY, 0.0, 1.0), Biome::IceCap);
    }

    #[test]
    fn climate_produces_all_fields() {
        let b = test_grid();
        let mut z = ScalarField::zeros(b);
        for y in 0..24 {
            for x in 0..24 {
                z.set(x, y, 500.0 + x as f64 * 100.0);
            }
        }
        let soil = ScalarField::filled(b, 2.0);
        let c = compute_climate(
            &z,
            &soil,
            &ClimateParams::default(),
            &VegetationParams::default(),
            &crate::erosion::rainfall::RainfallParams::default(),
            7,
            100.0,
            6.5,
        );
        assert_eq!(c.temperature_c.non_finite_count(), 0);
        assert_eq!(c.precipitation_mm.non_finite_count(), 0);
        assert!(c.vegetation.min() >= 0.0 && c.vegetation.max() <= 1.0);
        let frac = biome_fractions(&c);
        assert!((frac.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(frac.iter().any(|&f| f > 0.0));
    }

    #[test]
    fn desert_has_barely_any_cover() {
        assert!(biome_cover(Biome::Desert) < 0.15);
        assert!(biome_cover(Biome::TropicalRainforest) > 0.95);
    }

    #[test]
    fn vegetation_follows_moisture() {
        let b = test_grid();
        let z = ScalarField::filled(b, 300.0);
        let soil = ScalarField::filled(b, 2.0);
        let wet_rain = crate::erosion::rainfall::RainfallParams {
            annual_mm: 2500.0,
            ..Default::default()
        };
        let dry_rain = crate::erosion::rainfall::RainfallParams {
            annual_mm: 150.0,
            ..Default::default()
        };
        let wet = compute_climate(&z, &soil, &ClimateParams::default(), &VegetationParams::default(), &wet_rain, 1, 100.0, 6.5);
        let dry = compute_climate(&z, &soil, &ClimateParams::default(), &VegetationParams::default(), &dry_rain, 1, 100.0, 6.5);
        assert!(wet.vegetation.mean() > dry.vegetation.mean());
    }
}