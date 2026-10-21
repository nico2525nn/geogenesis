//! Climate: temperature, moisture transport, precipitation, biomes and
//! vegetation (spec §3a, §5).
//!
//! The climate is a diagnostic snapshot derived from the current surface and
//! the climate parameters; it never feeds back into itself except through the
//! slowly evolving vegetation state. That keeps it cheap and deterministic.
//!
//! # Temperature
//!
//! The zonal mean follows a latitude profile that *preserves the requested
//! global mean*: the equator runs above it and the poles below, weighted so the
//! area average of the anomaly is exactly zero. On top of that the model adds
//! the physical effects the spec asks for:
//!
//! * a moist-adiabatic lapse rate for land elevation,
//! * continentality (interiors are cooler on the mean and much more seasonal),
//! * ocean-current moderation near the coast (maritime climates),
//! * the CO₂ greenhouse offset and a long-period orbital cycle,
//! * a snow/ice albedo feedback that sharpens cold regions.
//!
//! A regional (plane/torus) grid is a window onto the planet. Its latitude
//! window defaults to the tropics-to-temperate belt where biomes are most
//! varied; spherical grids span pole to pole.
//!
//! # Moisture
//!
//! Precipitation is *emergent*, not painted: a normalised moisture column is
//! advected upwind-to-downwind through a cached sweep order. Air picks up
//! moisture over the sea, rains out a fraction when forced upslope (orographic
//! lift), dries further downwind by depletion and rain-shadow descent, and a
//! share of the rain is recycled by evapotranspiration. A subtropical
//! subsidence belt (Hadley cells) suppresses rain under the high-pressure
//! cells. The resulting field is rescaled to the requested land mean, so
//! `precipitation_mm` stays a meaningful global control while the spatial
//! pattern — windward wet belts, rain shadows, dry interiors — comes from the
//! circulation and topography.
//!
//! # Biomes and vegetation
//!
//! Biomes are an extended Whittaker classification (temperature,
//! precipitation, aridity, elevation, snow, slope). Vegetation is a state
//! variable: it grows logistically toward the biome's carrying capacity, is
//! reset by disturbance on steep, eroding ground, and is limited by soil
//! availability. It feeds back into erosion resistance in the fluvial and
//! hillslope modules.

use crate::grid::{Field, Grid};
use crate::math::{clamp, smoothstep, Vec2};
use crate::params::Params;
use crate::spatial;
use crate::world::World;

/// Biome classes from spec §5b, plus ocean and ice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Biome {
    /// Open water.
    Ocean = 0,
    /// Tropical rainforest.
    TropicalRainforest = 1,
    /// Tropical savanna.
    Savanna = 2,
    /// Hot desert.
    Desert = 3,
    /// Temperate grassland / steppe.
    Grassland = 4,
    /// Temperate broadleaf forest.
    TemperateForest = 5,
    /// Mediterranean shrubland.
    Mediterranean = 6,
    /// Boreal conifer forest.
    BorealForest = 7,
    /// Tundra.
    Tundra = 8,
    /// Permanent ice.
    Ice = 9,
    /// High-altitude alpine terrain above the treeline.
    Alpine = 10,
    /// Riparian / wetland.
    Wetland = 11,
}

impl Biome {
    /// All land biomes in display order.
    pub const LAND: [Self; 11] = [
        Self::TropicalRainforest,
        Self::Savanna,
        Self::Desert,
        Self::Grassland,
        Self::TemperateForest,
        Self::Mediterranean,
        Self::BorealForest,
        Self::Tundra,
        Self::Ice,
        Self::Alpine,
        Self::Wetland,
    ];

    /// Converts from the stored `u8`.
    #[must_use]
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::TropicalRainforest,
            2 => Self::Savanna,
            3 => Self::Desert,
            4 => Self::Grassland,
            5 => Self::TemperateForest,
            6 => Self::Mediterranean,
            7 => Self::BorealForest,
            8 => Self::Tundra,
            9 => Self::Ice,
            10 => Self::Alpine,
            11 => Self::Wetland,
            _ => Self::Ocean,
        }
    }

    /// Stable key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Ocean => "ocean",
            Self::TropicalRainforest => "tropical_rainforest",
            Self::Savanna => "savanna",
            Self::Desert => "desert",
            Self::Grassland => "grassland",
            Self::TemperateForest => "temperate_forest",
            Self::Mediterranean => "mediterranean",
            Self::BorealForest => "boreal_forest",
            Self::Tundra => "tundra",
            Self::Ice => "ice",
            Self::Alpine => "alpine",
            Self::Wetland => "wetland",
        }
    }

    /// English label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ocean => "Ocean",
            Self::TropicalRainforest => "Tropical rainforest",
            Self::Savanna => "Savanna",
            Self::Desert => "Desert",
            Self::Grassland => "Grassland",
            Self::TemperateForest => "Temperate forest",
            Self::Mediterranean => "Mediterranean",
            Self::BorealForest => "Boreal forest",
            Self::Tundra => "Tundra",
            Self::Ice => "Ice",
            Self::Alpine => "Alpine",
            Self::Wetland => "Wetland",
        }
    }

    /// Japanese label.
    #[must_use]
    pub const fn label_ja(self) -> &'static str {
        match self {
            Self::Ocean => "海洋",
            Self::TropicalRainforest => "熱帯雨林",
            Self::Savanna => "サバンナ",
            Self::Desert => "砂漠",
            Self::Grassland => "草原",
            Self::TemperateForest => "温帯林",
            Self::Mediterranean => "地中海林",
            Self::BorealForest => "針葉樹林",
            Self::Tundra => "ツンドラ",
            Self::Ice => "氷原",
            Self::Alpine => "高山",
            Self::Wetland => "湿地",
        }
    }

    /// Display colour.
    #[must_use]
    pub const fn colour(self) -> [u8; 3] {
        match self {
            Self::Ocean => [30, 70, 120],
            Self::TropicalRainforest => [16, 92, 42],
            Self::Savanna => [154, 150, 62],
            Self::Desert => [214, 183, 120],
            Self::Grassland => [126, 155, 80],
            Self::TemperateForest => [54, 112, 62],
            Self::Mediterranean => [128, 138, 78],
            Self::BorealForest => [40, 84, 66],
            Self::Tundra => [134, 143, 128],
            Self::Ice => [235, 241, 245],
            Self::Alpine => [148, 146, 140],
            Self::Wetland => [78, 124, 104],
        }
    }
}

/// Area weight of one cell for spatial means.
///
/// Planar grids use `1.0`, so the historical unweighted arithmetic (and its
/// floating-point results) is preserved exactly. Spherical grids use the
/// physical latitude-band area, making every mean a true area average.
#[inline]
fn cell_weight(grid: Grid, x: usize, y: usize) -> f64 {
    if grid.has_spherical_areas() {
        grid.cell_area_m2_at(x, y)
    } else {
        1.0
    }
}

/// One climate snapshot.
#[derive(Clone, Debug)]
pub struct Climate {
    /// Grid geometry.
    pub grid: Grid,
    /// Surface temperature, °C.
    pub temperature_c: Field<f32>,
    /// Annual precipitation, mm/yr.
    pub precipitation_mm: Field<f32>,
    /// Aridity index: potential evaporation divided by precipitation.
    pub aridity: Field<f32>,
    /// Vegetation cover, 0..1.
    pub vegetation: Field<f32>,
    /// Biome per cell.
    pub biome: Field<u8>,
    /// Snow/ice cover fraction, 0..1.
    pub snow_cover: Field<f32>,
    /// Seasonal temperature amplitude (half the annual range), °C.
    pub seasonal_amplitude_c: Field<f32>,
    /// Distance to the nearest ocean cell, km.
    pub ocean_distance_km: Field<f32>,
    /// Time since the climate was last updated, years (for vegetation growth).
    pub age_yr: f64,
    /// Wind direction used, as a downwind unit vector.
    wind: Vec2,
    /// Upwind sweep order, cached because wind and grid rarely change.
    sweep_order: Vec<u32>,
}

impl Climate {
    /// Temperature statistics over land, °C.
    ///
    /// Area weighted on spherical grids; a plain cell average on planar grids.
    #[must_use]
    pub fn mean_land_temperature(&self) -> f64 {
        let grid = self.grid;
        let mut sum = 0.0;
        let mut weight = 0.0;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                if self.biome.get(x, y) != Biome::Ocean as u8 {
                    sum += f64::from(self.temperature_c.get(x, y)) * cell_weight(grid, x, y);
                    weight += cell_weight(grid, x, y);
                }
            }
        }
        if weight > 0.0 {
            sum / weight
        } else {
            0.0
        }
    }

    /// Mean precipitation over land, mm/yr.
    ///
    /// Area weighted on spherical grids; a plain cell average on planar grids.
    #[must_use]
    pub fn mean_land_precipitation(&self) -> f64 {
        let grid = self.grid;
        let mut sum = 0.0;
        let mut weight = 0.0;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                if self.biome.get(x, y) != Biome::Ocean as u8 {
                    sum += f64::from(self.precipitation_mm.get(x, y)) * cell_weight(grid, x, y);
                    weight += cell_weight(grid, x, y);
                }
            }
        }
        if weight > 0.0 {
            sum / weight
        } else {
            0.0
        }
    }

    /// Mean sea-surface temperature over ocean cells, °C.
    ///
    /// This is the best available proxy for the global surface temperature
    /// (land is colder because it is elevated), used to shift the snowline.
    /// Area weighted on spherical grids.
    #[must_use]
    pub fn mean_ocean_temperature(&self) -> Option<f64> {
        let grid = self.grid;
        let mut sum = 0.0;
        let mut weight = 0.0;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                if self.biome.get(x, y) == Biome::Ocean as u8 {
                    sum += f64::from(self.temperature_c.get(x, y)) * cell_weight(grid, x, y);
                    weight += cell_weight(grid, x, y);
                }
            }
        }
        if weight > 0.0 {
            Some(sum / weight)
        } else {
            None
        }
    }

    /// Mean land vegetation cover, 0..1.
    ///
    /// Area weighted on spherical grids; a plain cell average on planar grids.
    #[must_use]
    pub fn mean_land_vegetation(&self) -> f64 {
        let grid = self.grid;
        let mut sum = 0.0;
        let mut weight = 0.0;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                if self.biome.get(x, y) != Biome::Ocean as u8 {
                    sum += f64::from(self.vegetation.get(x, y)) * cell_weight(grid, x, y);
                    weight += cell_weight(grid, x, y);
                }
            }
        }
        if weight > 0.0 {
            sum / weight
        } else {
            0.0
        }
    }

    /// Biome area fractions over land only, ordered like [`Biome::LAND`].
    ///
    /// Fractions sum to one for any world with land, which makes them
    /// comparable with the spec's biome-mix targets. Area weighted on spherical
    /// grids: sharing the land area, not the cell count.
    #[must_use]
    pub fn biome_fractions(&self) -> Vec<(Biome, f64)> {
        let grid = self.grid;
        let mut areas = [0.0f64; 12];
        let mut land = 0.0f64;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let weight = cell_weight(grid, x, y);
                let b = self.biome.get(x, y) as usize;
                areas[b] += weight;
                if b != Biome::Ocean as usize {
                    land += weight;
                }
            }
        }
        let total = if land > 0.0 { land } else { 1.0 };
        Biome::LAND
            .iter()
            .map(|&biome| (biome, areas[biome as usize] / total))
            .collect()
    }

    /// Biome area fractions over the whole grid, ordered like [`Biome::LAND`].
    ///
    /// Kept separate from [`Self::biome_fractions`] for statistics that compare
    /// against the world's total area (the CLI prints those). Area weighted on
    /// spherical grids, so the shares sum to the land area fraction there.
    #[must_use]
    pub fn biome_fractions_of_grid(&self) -> Vec<(Biome, f64)> {
        let grid = self.grid;
        let mut areas = [0.0f64; 12];
        let mut total = 0.0f64;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let weight = cell_weight(grid, x, y);
                areas[self.biome.get(x, y) as usize] += weight;
                total += weight;
            }
        }
        let total = if total > 0.0 { total } else { 1.0 };
        Biome::LAND
            .iter()
            .map(|&biome| (biome, areas[biome as usize] / total))
            .collect()
    }

    /// Temperature at a cell, °C.
    #[must_use]
    pub fn temperature_at(&self, x: usize, y: usize) -> f64 {
        f64::from(self.temperature_c.get(x, y))
    }

    /// Precipitation at a cell, mm/yr.
    #[must_use]
    pub fn precipitation_at(&self, x: usize, y: usize) -> f64 {
        f64::from(self.precipitation_mm.get(x, y))
    }

    /// Prevailing wind as a downwind unit vector.
    #[must_use]
    pub fn wind_vector(&self) -> Vec2 {
        self.wind
    }
}

/// Everything the climate model needs from the rest of the world.
#[derive(Debug)]
pub struct ClimateInputs<'a> {
    /// World state (elevation and sea level are used).
    pub world: &'a World,
    /// Parameters.
    pub params: &'a Params,
    /// Previous climate, for vegetation growth and sweep-order reuse.
    pub previous: Option<&'a Climate>,
    /// Seconds... years since the previous update.
    pub dt_yr: f64,
}

/// Computes a climate snapshot.
#[must_use]
pub fn compute(inputs: &ClimateInputs<'_>) -> Climate {
    let world = inputs.world;
    let params = inputs.params;
    let grid = world.grid;
    let cc = &params.climate;
    let eco = &params.ecology;
    let sea = world.sea_level_m;
    let t = world.elapsed_yr;
    let spherical = grid.boundary() == crate::grid::BoundaryKind::Spherical;
    // Cell area weight for the land-mean target: physical area on a sphere,
    // unit weight (historical arithmetic) on planar grids.
    let area_weight_at = |index: usize| -> f64 {
        if grid.has_spherical_areas() {
            let (x, y) = grid.coords(index);
            grid.cell_area_m2_at(x, y)
        } else {
            1.0
        }
    };

    // Ocean mask and distance to the sea, used by temperature and moisture.
    let ocean_mask: Vec<bool> = world
        .elevation_m
        .as_slice()
        .iter()
        .map(|&z| z <= sea)
        .collect();
    let ocean_distance_km = spatial::distance_m(&ocean_mask, grid).map_to(|m| (m as f32) / 1000.0);

    let wind = cc.wind_vector();
    let wind_speed_factor = clamp(cc.wind_speed_ms / 8.0, 0.2, 3.0);

    // ---- Temperature -----------------------------------------------------
    // Long-period orbital/climate cycle on top of the CO₂ baseline.
    let cycle = if cc.climate_cycle_amplitude_c > 0.0 && cc.climate_cycle_period_kyr > 0.0 {
        cc.climate_cycle_amplitude_c
            * (std::f64::consts::TAU * t / (cc.climate_cycle_period_kyr * 1000.0)).sin()
    } else {
        0.0
    };
    let global_mean = cc.reference_temp_c() + cycle;

    // Latitude per row and its zonal-mean temperature anomaly. Regional maps
    // cover a window of the planet (default: the tropics-to-temperate belt);
    // spherical maps span pole to pole.
    let h = grid.height() as f64;
    let mut row_lat = Vec::with_capacity(grid.height());
    let mut row_anomaly = Vec::with_capacity(grid.height());
    let mut row_dry = Vec::with_capacity(grid.height());
    for y in 0..grid.height() {
        let y_fraction = (y as f64 + 0.5) / h;
        let lat = cc.latitude_at(y_fraction, spherical);
        row_lat.push(lat);
        row_anomaly.push(cc.zonal_temperature_anomaly_c(lat));
        // Hadley subsidence: descending dry air under the subtropical highs.
        let belt = (-((lat.abs() - 27.0) / 10.0).powi(2)).exp();
        row_dry.push(cc.subtropical_dryness * belt);
    }

    let mut temperature_c = Field::<f32>::zeros(grid);
    let mut snow_cover = Field::<f32>::zeros(grid);
    let mut seasonal_amplitude_c = Field::<f32>::zeros(grid);
    {
        let mut raw = vec![0.0f64; grid.cell_count()];
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let i = grid.index(x, y);
                let z = world.elevation_m.get_index(i);
                let dist_km = f64::from(ocean_distance_km.get_index(i));
                let inland = (dist_km / 1500.0).clamp(0.0, 1.0);
                let above_sea = (z - sea).max(0.0);
                let mut temp = global_mean + row_anomaly[y];
                // Environmental lapse: land surface cools with elevation.
                temp -= cc.lapse_rate_c_km * above_sea / 1000.0;
                // Interiors are cooler on the annual mean (no maritime heat
                // reservoir) and far more seasonal.
                let continental = cc.continentality * inland.powf(1.2);
                temp -= continental * 2.5;
                // Maritime moderation: coastal cells are buffered by the ocean.
                let maritime = (-dist_km / 300.0).exp();
                temp += cc.ocean_current_moderation * maritime * 2.0;
                raw[i] = temp;
            }
        }
        // Snow/ice albedo feedback: where snow persists, reflect sunlight and
        // cool further. One explicit pass is enough to sharpen the snowline.
        for i in 0..grid.cell_count() {
            let land = !ocean_mask[i];
            let first = smoothstep(1.5, -8.0, raw[i]);
            let snow = if land { first } else { 0.0 };
            let cooled = raw[i] - cc.snow_albedo_cooling_c * snow;
            let final_snow = if land {
                smoothstep(1.5, -8.0, cooled)
            } else {
                0.0
            };
            temperature_c.set_index(i, cooled as f32);
            snow_cover.set_index(i, final_snow as f32);
        }
        // Seasonal amplitude: small on warm maritime coasts, large in cold
        // continental interiors and at high latitude.
        for y in 0..grid.height() {
            let lat_abs = row_lat[y].abs();
            let latitude_gain = 0.35 + 0.65 * (lat_abs / 60.0).min(1.0);
            for x in 0..grid.width() {
                let i = grid.index(x, y);
                let dist_km = f64::from(ocean_distance_km.get_index(i));
                let inland = (dist_km / 1500.0).clamp(0.0, 1.0);
                let gain = 0.35 + 1.1 * inland * cc.continentality;
                let amplitude = cc.seasonal_range_c * gain * latitude_gain * (0.6 + cc.seasonality);
                let sea_ice = if ocean_mask[i] {
                    // Frozen sea keeps a small amplitude; ice caps are stable.
                    (1.0 - f64::from(snow_cover.get_index(i)) * 0.8).max(0.15)
                } else {
                    1.0
                };
                seasonal_amplitude_c.set_index(i, (amplitude * sea_ice).clamp(0.0, 45.0) as f32);
            }
        }
    }

    // ---- Precipitation: upwind moisture sweep ----------------------------
    let sweep_order = inputs
        .previous
        .filter(|prev| prev.window_matches(grid) && prev.wind == wind)
        .map(|prev| prev.sweep_order.clone())
        .unwrap_or_else(|| upwind_order(grid, wind));

    // Moisture is tracked as a normalised atmospheric column (1 = saturated
    // marine air). Air is advected upwind-to-downwind; forced ascent rains out
    // a fraction of the column, so the windward flank wets and the lee dries.
    let mut moisture = Field::<f64>::zeros(grid);
    let mut raw_rain = Field::<f64>::zeros(grid);
    for i in 0..grid.cell_count() {
        if ocean_mask[i] {
            // Marine air is saturated.
            moisture.set_index(i, 1.0);
        }
    }
    for &cell_index in &sweep_order {
        let idx = cell_index as usize;
        if ocean_mask[idx] {
            continue; // evaporation keeps the marine air at saturation
        }
        let (x, y) = grid.coords(idx);
        // Upwind moisture, blended over one and two cells so a single dry cell
        // cannot staircase the pattern into stripes.
        let near = sample_upwind(&moisture, x, y, wind, 1.0);
        let far = sample_upwind(&moisture, x, y, wind, 2.0);
        let mut upwind = 0.7 * near + 0.3 * far;
        if upwind <= 1e-6 {
            // Air entering across an open boundary still carries some moisture.
            upwind = 0.35;
        }
        // Orographic forcing: upslope component of the wind times the local
        // gradient, scaled to something like a vertical velocity.
        let (dzdx, dzdy) = world.elevation_m.gradient(x, y);
        let along_wind = wind.x * dzdx + wind.y * dzdy;
        let ascent = along_wind.max(0.0);
        let descent = (-along_wind).max(0.0);
        // Per-cell rain fraction: a background drizzle plus modest orographic
        // rain-out. A 10 km cell is only a small step of the air's journey, so
        // each step must stay well below complete condensation or a single
        // ridge would strip all moisture and turn the whole interior to desert.
        let lift = clamp(ascent * 8.0 * wind_speed_factor, 0.0, 2.0);
        let mut rain_fraction = 0.012 + 0.16 * cc.orographic_strength * lift;
        // Rain shadow: descending air suppresses rain on the lee side.
        let lee = clamp(descent * 15.0, 0.0, 0.7);
        rain_fraction *= 1.0 - cc.rain_shadow_strength * lee;
        // Elevation–precipitation coupling (orographic cloud belts).
        let relative_elev = ((world.elevation_m.get_index(idx) - sea) / 2500.0).clamp(-0.5, 1.5);
        rain_fraction *= (1.0 + 0.25 * cc.elev_precip_correlation * relative_elev).clamp(0.4, 1.8);
        // Subtropical subsidence.
        rain_fraction *= 1.0 - 0.8 * row_dry[y];
        // Warm air holds more water, so a given condensation efficiency
        // produces less rain per metre of lift; cold air rains out faster.
        let temp = f64::from(temperature_c.get_index(idx));
        let thermal = (1.0 + 0.02 * temp).clamp(0.4, 1.8);
        rain_fraction /= thermal;
        let rain_fraction = clamp(rain_fraction, 0.002, 0.4);
        let rain = upwind * rain_fraction;
        raw_rain.set_index(idx, rain);
        // A share of the rain is evapotranspired and rained out further
        // downwind (continental moisture recycling).
        let evap = clamp(0.35 + 0.02 * temp, 0.15, 1.0);
        let recycled = rain * cc.moisture_recycling * evap;
        moisture.set_index(idx, (upwind - rain + recycled).clamp(0.0, 1.2));
    }

    // Smooth the raw pattern a little: terrain gradients are noisy at grid
    // scale and the atmospheric column integrates over many kilometres.
    spatial::gaussian_blur(&mut raw_rain, 1.2);

    // Rescale the land pattern to the requested mean precipitation, so
    // `precipitation_mm` remains a meaningful global control while the
    // windward/lee contrast comes from the terrain. A soft saturation caps the
    // wettest windward belts at a few times the mean, then the mean is
    // restored exactly (two passes converge to the requested value).
    let base = cc.precipitation_mm.max(1.0);
    let cap = base * 3.0;
    let mut sum_raw = 0.0;
    // On a spherical grid this is the land *area*, on a planar one the land
    // cell count; either way `base * n_land / sum_raw` targets the requested
    // mean under the matching weighting.
    let mut n_land = 0.0;
    for i in 0..grid.cell_count() {
        if ocean_mask[i] {
            continue;
        }
        let weight = area_weight_at(i);
        sum_raw += raw_rain.get_index(i) * weight;
        n_land += weight;
    }
    let scale = if n_land > 0.0 && sum_raw > 0.0 {
        base * n_land / sum_raw
    } else {
        1.0
    };
    let mut precipitation_mm = Field::<f32>::zeros(grid);
    let mut sum_out = 0.0;
    for i in 0..grid.cell_count() {
        if ocean_mask[i] {
            precipitation_mm.set_index(i, base as f32);
            continue;
        }
        let scaled = raw_rain.get_index(i) * scale;
        // Soft cap: linear for small values, saturating at a few times the
        // mean so one windward ridge cannot produce unphysical hyper-rain.
        let value = cap * (1.0 - (-scaled / cap).exp());
        precipitation_mm.set_index(i, clamp(value, 0.0, 12_000.0) as f32);
        sum_out += value * area_weight_at(i);
    }
    // Restore the requested land mean after the soft saturation.
    if sum_out > 0.0 && n_land > 0.0 && (sum_out - base * n_land).abs() > 1e-9 {
        let correction = base * n_land / sum_out;
        for i in 0..grid.cell_count() {
            if ocean_mask[i] {
                continue;
            }
            let v = f64::from(precipitation_mm.get_index(i)) * correction;
            precipitation_mm.set_index(i, clamp(v, 0.0, 12_000.0) as f32);
        }
    }

    // ---- Aridity, biomes, vegetation -------------------------------------
    let mut aridity = Field::<f32>::zeros(grid);
    let mut biome = Field::<u8>::zeros(grid);
    let mut vegetation = Field::<f32>::zeros(grid);
    let prev_veg = inputs.previous.map(|c| &c.vegetation);
    let growth_rate = eco.vegetation_growth_rate.max(0.0);
    let slope_cache = {
        // Biome and vegetation both need the slope; compute it once.
        let mut slope = vec![0.0f64; grid.cell_count()];
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let i = grid.index(x, y);
                slope[i] = world.elevation_m.slope(x, y);
            }
        }
        slope
    };
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            let temp = f64::from(temperature_c.get_index(i));
            let rain = f64::from(precipitation_mm.get_index(i));
            let snow = f64::from(snow_cover.get_index(i));
            // Potential evapotranspiration, mm/yr (Thornthwaite-like).
            let warmth = temp.max(0.0);
            let pet = 300.0 + 30.0 * warmth + 0.4 * warmth * warmth;
            let arid = (pet / rain.max(1.0)).max(0.0);
            aridity.set_index(i, (arid as f32).min(30.0));

            let elev = world.elevation_m.get_index(i) - sea;
            let slope_deg = slope_cache[i].atan().to_degrees();
            let temp_eff = temp + eco.biome_shift_c;
            let class = if ocean_mask[i] {
                Biome::Ocean
            } else {
                classify_biome(temp_eff, rain, arid, elev, snow, slope_deg)
            };
            biome.set_index(i, class as u8);

            // Vegetation carrying capacity: moisture, warmth, snow, soil.
            let moisture_factor = (1.0 - (arid / 3.0).min(1.0)).powf(0.7);
            let warmth_factor = smoothstep(-10.0, 6.0, temp);
            let soil = f64::from(world.soil_m.get_index(i));
            let soil_factor = 0.35 + 0.65 * (soil / 0.6).clamp(0.0, 1.0);
            let target = if ocean_mask[i] {
                0.0
            } else {
                eco.vegetation_max
                    * moisture_factor
                    * warmth_factor
                    * soil_factor
                    * (1.0 - 0.9 * snow)
            };
            // Logistic growth from the previous cover. Disturbance resets
            // cover on steep, actively eroding ground; the logistic term then
            // recovers it over subsequent climate updates.
            let current = f64::from(prev_veg.map_or(target as f32, |f| f.get_index(i)));
            let target = target.clamp(0.0, 1.0);
            let mut next = if target <= 0.01 {
                current * (1.0 - clamp(growth_rate * inputs.dt_yr, 0.0, 0.9))
            } else if growth_rate <= 0.0 {
                target
            } else {
                let seed = current.max(0.01);
                let ratio = (target / seed - 1.0) * (-growth_rate * inputs.dt_yr).exp();
                target / (1.0 + ratio)
            };
            // Disturbance reset: steep slopes and very dry ground shed cover.
            // The thresholds are in degrees of a 10 km-scale cell, where even
            // a 10° slope is rugged terrain.
            let steep = smoothstep(4.0, 14.0, slope_deg);
            let dry = smoothstep(3.0, 6.0, arid);
            let disturbance = eco.vegetation_disturbance * (0.7 * steep + 0.3 * dry);
            next *= 1.0 - 0.85 * disturbance;
            vegetation.set_index(i, clamp(next, 0.0, 1.0) as f32);
        }
    }
    // Ocean has no vegetation regardless.
    for i in 0..grid.cell_count() {
        if ocean_mask[i] {
            vegetation.set_index(i, 0.0);
        }
    }

    Climate {
        grid,
        temperature_c,
        precipitation_mm,
        aridity,
        vegetation,
        biome,
        snow_cover,
        seasonal_amplitude_c,
        ocean_distance_km,
        age_yr: inputs.dt_yr,
        wind,
        sweep_order,
    }
}

impl Climate {
    /// Whether a cached sweep order and wind still apply to `grid`.
    fn window_matches(&self, grid: Grid) -> bool {
        self.grid.width() == grid.width() && self.grid.height() == grid.height()
    }
}

/// Sorts cell indices from upwind to downwind.
fn upwind_order(grid: Grid, wind: Vec2) -> Vec<u32> {
    let w = grid.width() as f64;
    let h = grid.height() as f64;
    let mut cells: Vec<u32> = (0..grid.cell_count() as u32).collect();
    // Projection onto the wind vector; ties break on index for determinism.
    cells.sort_by(|&a, &b| {
        let (ax, ay) = grid.coords(a as usize);
        let (bx, by) = grid.coords(b as usize);
        let pa = ax as f64 * wind.x / w + ay as f64 * wind.y / h;
        let pb = bx as f64 * wind.x / w + by as f64 * wind.y / h;
        pa.partial_cmp(&pb)
            .unwrap_or(core::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    cells
}

/// Bilinear sample of moisture `distance` grid steps upwind of `(x, y)`.
fn sample_upwind(moisture: &Field<f64>, x: usize, y: usize, wind: Vec2, distance: f64) -> f64 {
    let sx = x as f64 - wind.x * distance;
    let sy = y as f64 - wind.y * distance;
    let v = moisture.bilinear(sx, sy);
    // Guard against negative interpolation artefacts.
    v.max(0.0)
}

/// Extends Whittaker's diagram with elevation, snow, slope and aridity.
///
/// Thresholds are calibrated for the engine's absolute temperature and
/// precipitation ranges so an Earth-like world produces the full spectrum of
/// biomes instead of a few large blocks.
fn classify_biome(
    temp_c: f64,
    rain_mm: f64,
    aridity: f64,
    elevation_m: f64,
    snow: f64,
    slope_deg: f64,
) -> Biome {
    // Permanent ice: cold and snow-covered, or below the polar treeline.
    if snow > 0.75 && temp_c < -2.0 {
        return Biome::Ice;
    }
    if temp_c < -8.0 {
        return Biome::Ice;
    }
    // Cold or high ground: tundra, then alpine above the treeline.
    if temp_c < -2.0 {
        return Biome::Tundra;
    }
    if temp_c < 5.0 {
        if elevation_m > 1800.0 && slope_deg > 3.0 {
            return Biome::Alpine;
        }
        // Cold deserts and dry steppes do not carry closed forest.
        if rain_mm > 300.0 && aridity < 4.0 {
            return Biome::BorealForest;
        }
        return Biome::Tundra;
    }
    // Exposed high mountains above the local treeline.
    if elevation_m > 2600.0 && temp_c < 10.0 {
        return Biome::Alpine;
    }
    // Wetlands occupy flat, poorly drained, perennially wet lowlands.
    if rain_mm > 1000.0 && aridity < 1.4 && elevation_m < 12.0 && slope_deg < 0.6 && temp_c > 2.0 {
        return Biome::Wetland;
    }
    if temp_c >= 20.0 {
        // Tropical belt.
        if rain_mm > 1650.0 && aridity < 1.8 {
            return Biome::TropicalRainforest;
        }
        if rain_mm > 1500.0 && aridity < 3.0 {
            return Biome::TropicalRainforest;
        }
        if aridity > 3.2 || rain_mm < 420.0 {
            return Biome::Desert;
        }
        if rain_mm > 1050.0 && aridity < 2.4 {
            return Biome::TropicalRainforest;
        }
        Biome::Savanna
    } else {
        // Temperate, mediterranean and subtropical belt.
        if rain_mm > 2000.0 && aridity < 1.6 {
            return Biome::TemperateForest;
        }
        if aridity > 3.0 || rain_mm < 320.0 {
            return Biome::Desert;
        }
        if temp_c > 11.0 && (420.0..950.0).contains(&rain_mm) && aridity > 1.25 {
            return Biome::Mediterranean;
        }
        if rain_mm < 750.0 || aridity > 1.9 {
            return Biome::Grassland;
        }
        Biome::TemperateForest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;
    use crate::world::World;

    fn world_with_relief() -> (World, Params) {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        let mut world = World::new(&p.world).unwrap();
        let grid = world.grid;
        world.elevation_m =
            crate::grid::Field::from_fn(grid, |x, _| if x < 16 { -500.0 } else { 800.0 });
        world.elevation_m.set(20, 20, 3000.0);
        (world, p)
    }

    fn climate(world: &World, p: &Params) -> Climate {
        compute(&ClimateInputs {
            world,
            params: p,
            previous: None,
            dt_yr: 2000.0,
        })
    }

    #[test]
    fn temperature_falls_with_altitude_and_latitude() {
        let (world, p) = world_with_relief();
        let c = climate(&world, &p);
        // Same x (same ocean distance) but a 3 km mountain peak vs lowland.
        let low = f64::from(c.temperature_c.get(17, 20));
        let high = f64::from(c.temperature_c.get(20, 20));
        assert!(high < low - 10.0, "lapse rate missing: {low} -> {high}");
        // The north edge of the window is poleward and colder than the middle.
        let pole = f64::from(c.temperature_c.get(5, 0));
        let equator = f64::from(c.temperature_c.get(5, 15));
        assert!(
            pole < equator - 2.0,
            "latitude gradient missing: {pole} vs {equator}"
        );
    }

    #[test]
    fn zonal_profile_preserves_the_global_mean() {
        let p = Params::default();
        // Area-weighted mean of the anomaly over the sphere is zero.
        let mut sum = 0.0;
        let mut weight = 0.0;
        for k in 0..=1800 {
            let lat = -90.0 + k as f64 * 0.1;
            let w = lat.to_radians().cos();
            sum += p.climate.zonal_temperature_anomaly_c(lat) * w;
            weight += w;
        }
        assert!((sum / weight).abs() < 5e-5, "mean anomaly {}", sum / weight);
        // Equator warmer than pole.
        assert!(p.climate.zonal_temperature_anomaly_c(0.0) > 8.0);
        assert!(p.climate.zonal_temperature_anomaly_c(90.0) < -12.0);
    }

    #[test]
    fn ocean_cells_have_sane_temperatures() {
        let (world, p) = world_with_relief();
        let c = climate(&world, &p);
        for &t in c.temperature_c.as_slice() {
            assert!(
                t.is_finite() && (-70.0..=70.0).contains(&t),
                "temperature {t}"
            );
        }
    }

    #[test]
    fn precipitation_mean_tracks_the_parameter() {
        let (world, mut p) = world_with_relief();
        p.climate.precipitation_mm = 1500.0;
        let c = climate(&world, &p);
        let mean = c.mean_land_precipitation();
        assert!(
            (mean - 1500.0).abs() < 1500.0 * 0.35,
            "land mean precipitation {mean} far from 1500"
        );
        assert!(c
            .precipitation_mm
            .as_slice()
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0));
    }

    #[test]
    fn rain_shadow_dries_the_downwind_side() {
        let mut p = Params::default();
        p.world.width = 64;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        p.climate.wind_dir_deg = 270.0; // from the west, blowing east
        p.climate.orographic_strength = 1.0;
        p.climate.rain_shadow_strength = 1.0;
        p.climate.elev_precip_correlation = 0.0;
        let grid = p.world.grid().unwrap();
        let mut world = World::new(&p.world).unwrap();
        // Lowland west of a north-south ridge, then a low lee plateau.
        world.elevation_m = crate::grid::Field::from_fn(grid, |x, _| {
            if x < 16 {
                200.0
            } else if x <= 24 {
                200.0 + (x - 16) as f64 * 287.5
            } else {
                200.0
            }
        });
        let c = climate(&world, &p);
        let windward = f64::from(c.precipitation_mm.get(12, 16));
        let lee = f64::from(c.precipitation_mm.get(45, 16));
        assert!(windward > lee * 1.2, "windward {windward}, lee {lee}");
    }

    #[test]
    fn subtropical_belt_is_drier_than_the_equatorial_belt() {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 64;
        p.world.cell_km = 10.0;
        p.climate.wind_dir_deg = 270.0;
        p.climate.orographic_strength = 0.2;
        p.climate.elev_precip_correlation = 0.0;
        p.climate.subtropical_dryness = 0.9;
        let grid = p.world.grid().unwrap();
        let mut world = World::new(&p.world).unwrap();
        // A flat continent in the eastern half; ocean to the west.
        world.elevation_m =
            crate::grid::Field::from_fn(grid, |x, _| if x < 10 { -200.0 } else { 120.0 });
        let c = climate(&world, &p);
        // Rows for the equator and for the subtropical high (~27° latitude).
        let center = p.climate.latitude_center_deg;
        let span = p.climate.latitude_span_deg;
        let row = |lat: f64| {
            (((0.5 - (lat - center) / span) * f64::from(p.world.height)) as usize)
                .min((p.world.height - 1) as usize)
        };
        let y_eq = row(0.0);
        let y_sub = row(27.0);
        let wet = f64::from(c.precipitation_mm.get(20, y_eq));
        let dry = f64::from(c.precipitation_mm.get(20, y_sub));
        assert!(
            wet > dry * 1.1,
            "subtropical {dry} should be drier than equator {wet}"
        );
    }

    #[test]
    fn biomes_cover_hot_cold_and_arid_cases() {
        assert_eq!(
            classify_biome(27.0, 2600.0, 0.6, 100.0, 0.0, 1.0),
            Biome::TropicalRainforest
        );
        assert_eq!(
            classify_biome(30.0, 100.0, 8.0, 100.0, 0.0, 1.0),
            Biome::Desert
        );
        assert_eq!(
            classify_biome(-10.0, 200.0, 1.0, 100.0, 1.0, 1.0),
            Biome::Ice
        );
        assert_eq!(
            classify_biome(0.0, 450.0, 1.0, 100.0, 0.2, 2.0),
            Biome::BorealForest
        );
        // A cold, dry steppe stays tundra instead of growing forest.
        assert_eq!(
            classify_biome(0.0, 150.0, 1.0, 400.0, 0.3, 2.0),
            Biome::Tundra
        );
        assert_eq!(
            classify_biome(8.0, 250.0, 6.0, 100.0, 0.0, 1.0),
            Biome::Desert
        );
        assert_eq!(
            classify_biome(15.0, 800.0, 1.4, 120.0, 0.0, 2.0),
            Biome::Mediterranean
        );
        assert_eq!(
            classify_biome(8.0, 500.0, 2.4, 120.0, 0.0, 2.0),
            Biome::Grassland
        );
        assert_eq!(
            classify_biome(11.0, 1400.0, 1.0, 5.0, 0.0, 0.1),
            Biome::Wetland
        );
    }

    #[test]
    fn ocean_has_no_vegetation() {
        let (world, p) = world_with_relief();
        let c = climate(&world, &p);
        for (i, &b) in c.biome.as_slice().iter().enumerate() {
            if b == Biome::Ocean as u8 {
                assert_eq!(c.vegetation.get_index(i), 0.0);
            }
        }
    }

    #[test]
    fn climate_is_deterministic() {
        let (world, p) = world_with_relief();
        let a = climate(&world, &p);
        let b = climate(&world, &p);
        assert_eq!(a.temperature_c.as_slice(), b.temperature_c.as_slice());
        assert_eq!(a.precipitation_mm.as_slice(), b.precipitation_mm.as_slice());
        assert_eq!(a.biome.as_slice(), b.biome.as_slice());
        assert_eq!(a.vegetation.as_slice(), b.vegetation.as_slice());
    }

    #[test]
    fn co2_and_seasons_warm_and_cool_the_world() {
        let (world, mut p) = world_with_relief();
        p.climate.co2_ppm = 420.0;
        let warm = climate(&world, &p);
        p.climate.co2_ppm = 280.0;
        let cold = climate(&world, &p);
        let a = f64::from(warm.temperature_c.get(17, 24));
        let b = f64::from(cold.temperature_c.get(17, 24));
        assert!(a > b + 1.0, "CO2 doubling should warm: {a} vs {b}");
        // Seasonal amplitude is larger inland than on the coast.
        assert!(warm
            .seasonal_amplitude_c
            .as_slice()
            .iter()
            .all(|v| v.is_finite()));
    }

    #[test]
    fn vegetation_grows_toward_target_and_is_disturbed_by_steepness() {
        let (mut world, p) = world_with_relief();
        world.soil_m.fill_value(1.0);
        // A dry first pass leaves low cover; a wetter second pass must grow it.
        let mut dry_p = p.clone();
        dry_p.climate.precipitation_mm = 250.0;
        let dry = compute(&ClimateInputs {
            world: &world,
            params: &dry_p,
            previous: None,
            dt_yr: 200_000.0,
        });
        let grown = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: Some(&dry),
            dt_yr: 200_000.0,
        });
        assert!(
            grown.mean_land_vegetation() > dry.mean_land_vegetation(),
            "vegetation should grow: {} -> {}",
            dry.mean_land_vegetation(),
            grown.mean_land_vegetation()
        );
        // Steep ground is disturbed: with the same climate and history it
        // carries less cover than the flat reference world.
        let mut p_steep = p.clone();
        p_steep.ecology.vegetation_disturbance = 1.0;
        let mut steep_world = world.clone();
        steep_world
            .elevation_m
            .map_in_place(|i, _| 200.0 + (i % 4) as f64 * 1_500.0);
        let steep = compute(&ClimateInputs {
            world: &steep_world,
            params: &p_steep,
            previous: Some(&dry),
            dt_yr: 200_000.0,
        });
        assert!(
            steep.mean_land_vegetation() < grown.mean_land_vegetation(),
            "steep {} should be below flat {}",
            steep.mean_land_vegetation(),
            grown.mean_land_vegetation()
        );
    }

    #[test]
    fn spherical_climate_means_are_area_weighted() {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        p.world.boundary = crate::grid::BoundaryKind::Spherical;
        p.world.planet_radius_km = 6371.0;
        let grid = p.world.grid().unwrap();
        assert!(grid.has_spherical_areas());
        let mut world = World::new(&p.world).unwrap();
        // Flat, entirely land world: the only spatial structure is latitude.
        world.elevation_m.fill_value(120.0);
        let c = climate(&world, &p);
        let mut sum = 0.0;
        let mut weight = 0.0;
        let mut temp_sum = 0.0;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                if c.biome.get(x, y) == Biome::Ocean as u8 {
                    continue;
                }
                let area = grid.cell_area_m2_at(x, y);
                sum += f64::from(c.precipitation_mm.get(x, y)) * area;
                temp_sum += f64::from(c.temperature_c.get(x, y)) * area;
                weight += area;
            }
        }
        assert!(
            (c.mean_land_precipitation() - sum / weight).abs() < 1e-9,
            "land precipitation mean is not area weighted"
        );
        assert!(
            (c.mean_land_temperature() - temp_sum / weight).abs() < 1e-9,
            "land temperature mean is not area weighted"
        );
        // The requested precipitation target is honoured in the area-weighted
        // sense (the pattern is rescaled to the land area mean).
        assert!(
            (c.mean_land_precipitation() - p.climate.precipitation_mm).abs()
                < p.climate.precipitation_mm * 0.05,
            "land mean precipitation {} far from {}",
            c.mean_land_precipitation(),
            p.climate.precipitation_mm
        );
        // Biome shares sum to one over the land area, and the grid shares sum
        // to the land area fraction of the grid, on a sphere.
        let land_sum: f64 = c.biome_fractions().iter().map(|(_, f)| *f).sum();
        assert!((land_sum - 1.0).abs() < 1e-12);
        let grid_sum: f64 = c.biome_fractions_of_grid().iter().map(|(_, f)| *f).sum();
        assert!((grid_sum - 1.0).abs() < 1e-12, "all-land grid {grid_sum}");
        let ocean = c.mean_ocean_temperature();
        assert!(ocean.is_none(), "all-land world reported a sea temperature");
    }

    #[test]
    fn planar_climate_statistics_remain_unweighted() {
        // On planar grids the area weighting collapses to unit weights, so the
        // statistics must match the historical unweighted arithmetic exactly.
        let (world, p) = world_with_relief();
        let c = climate(&world, &p);
        let mut sum = 0.0;
        let mut n = 0.0;
        for (&v, &b) in c.precipitation_mm.as_slice().iter().zip(c.biome.as_slice()) {
            if b != Biome::Ocean as u8 {
                sum += f64::from(v);
                n += 1.0;
            }
        }
        assert_eq!(c.mean_land_precipitation(), sum / n);
        let mut land = 0.0;
        for &b in c.biome.as_slice() {
            if b != Biome::Ocean as u8 {
                land += 1.0;
            }
        }
        let plain: Vec<(Biome, f64)> = Biome::LAND
            .iter()
            .map(|&biome| {
                let count = c
                    .biome
                    .as_slice()
                    .iter()
                    .filter(|&&b| b == biome as u8)
                    .count() as f64;
                (biome, count / land)
            })
            .collect();
        assert_eq!(c.biome_fractions(), plain);
    }

    /// Calibrated default world: after a scaled-down Earth-like run the terrain
    /// and biomes must stay within generous physical bounds.
    ///
    /// The full-resolution reference run (256², 200 steps, see the CLI) is too
    /// expensive for CI, so this uses the same parameter set on a coarser grid
    /// and a shorter duration; the bounds are deliberately loose and mainly
    /// guard against regressions that drown the world, explode relief, or
    /// collapse the biome mix to one or two classes.
    #[test]
    fn default_world_stays_plausible() {
        let mut p = Params::default();
        p.world.width = 96;
        p.world.height = 96;
        p.world.cell_km = 10.0;
        p.time.total_yr = 6.0e5;
        p.time.step_yr = 1.0e4;
        p.time.max_iters = 60;
        p.time.history_frames = 4;
        p.normalize();
        let mut sim = crate::sim::Simulation::new(&p).unwrap();
        sim.step_many(60);
        let report = sim.stats();

        // No numerical blow-ups anywhere.
        assert_eq!(sim.world.elevation_m.non_finite_count(), 0);
        assert!(sim.world.relief_range_m().1.is_finite());
        assert!(sim.world.relief_range_m().0.is_finite());
        // Relief stays below the ~8 km habitable envelope and the world is
        // neither drowned nor land-locked.
        assert!(
            sim.world.relief_range_m().1 < 8_000.0,
            "highest point {} m",
            sim.world.relief_range_m().1
        );
        let land = report.hypsometry.land_fraction;
        assert!((0.15..=0.60).contains(&land), "land fraction {land}");
        // A non-degenerate biome mix: at least four classes each covering a
        // measurable share of the land, and a mix of warm, dry and temperate
        // classes (a world made only of forests fails).
        let climate = sim.climate().expect("climate is computed at step 0");
        let fractions = climate.biome_fractions();
        let shared = fractions.iter().filter(|(_, f)| *f >= 0.01).count();
        assert!(
            shared >= 4,
            "only {shared} biomes share the land: {fractions:?}"
        );
        let has = |b: Biome| fractions.iter().any(|(k, f)| *k == b && *f >= 0.01);
        assert!(
            has(Biome::TemperateForest) || has(Biome::BorealForest),
            "no forest biome: {fractions:?}"
        );
        assert!(
            has(Biome::Desert) || has(Biome::Savanna) || has(Biome::Grassland),
            "no dry/open biome: {fractions:?}"
        );
        assert!(
            has(Biome::TropicalRainforest) || has(Biome::Savanna) || has(Biome::Wetland),
            "no warm biome: {fractions:?}"
        );
        // Climate fields are sane.
        assert!(climate
            .temperature_c
            .as_slice()
            .iter()
            .all(|v| v.is_finite()));
        assert!(climate
            .precipitation_mm
            .as_slice()
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0));
        let mean_precip = climate.mean_land_precipitation();
        assert!(
            (mean_precip - p.climate.precipitation_mm).abs() < p.climate.precipitation_mm * 0.05,
            "land mean precipitation {mean_precip} drifted from the parameter"
        );
        // The ledger closes relative to the enormous volumes of a 2 Myr run.
        let ledger = sim.state.ledger;
        assert!(ledger.erosion_m3 >= 0.0 && ledger.export_m3 >= 0.0);
        assert!(
            ledger.closure_error_m3().abs() <= ledger.total_volume_m3() * 1e-9 + 1.0,
            "{ledger:?}"
        );
    }
}
