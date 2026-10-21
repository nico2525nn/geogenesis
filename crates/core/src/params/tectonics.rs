//! Tectonic and geological parameter blocks.

use crate::math::rng::Rng;
use crate::params::spec::{ParamGroup, ParamSpec};

use super::ParamBlock;

/// Plate kinematics, boundary forces, isostasy and hotspots (spec §1).
#[derive(Clone, Debug, PartialEq)]
pub struct TectonicParams {
    /// Number of initial plates.
    pub plate_count: u32,
    /// Mean plate speed, cm/yr.
    pub plate_motion_cm_yr: f64,
    /// Mean crust/lithosphere thickness, km.
    pub plate_thickness_km: f64,
    /// Fraction of plates whose crust is continental.
    pub continental_fraction: f64,
    /// Density of continental crust, kg/m³.
    pub density_continental_kg_m3: f64,
    /// Density of oceanic crust, kg/m³.
    pub density_oceanic_kg_m3: f64,
    /// Density of the mantle, kg/m³.
    pub mantle_density_kg_m3: f64,
    /// Relative convergence above which a boundary is considered collisional.
    pub convergence_threshold: f64,
    /// Mean tectonic vertical velocity, mm/yr. Negative subsides.
    pub uplift_mm_yr: f64,
    /// Width of the orogenic belt at convergent boundaries, km.
    pub orogeny_width_km: f64,
    /// Flexural rigidity of the lithosphere, N·m.
    pub flexure_rigidity: f64,
    /// Whether Airy isostasy relaxes topography towards crustal equilibrium.
    pub isostasy: bool,
    /// Isostatic relaxation time, kyr.
    pub isostasy_relaxation_kyr: f64,
    /// Number of mantle hotspots.
    pub hotspot_count: u32,
    /// Peak hotspot dome height, m.
    pub hotspot_strength_m: f64,
    /// Hotspot dome radius, km.
    pub hotspot_radius_km: f64,
    /// Boundary irregularity (0 = straight Voronoi edges, 1 = very wiggly).
    pub boundary_wiggle: f64,
    /// Width of the subduction trench on the oceanic plate, km.
    pub trench_width_km: f64,
    /// Distance from the subduction boundary to the volcanic-arc axis, km.
    pub arc_offset_km: f64,
    /// Width of the volcanic-arc uplift belt, km.
    pub arc_width_km: f64,
    /// Width of the axial rift / mid-ocean-ridge valley and its flanks, km.
    pub rift_width_km: f64,
    /// Age scale of a hotspot trail, Myr (how long the swell takes to decay).
    pub hotspot_trail_myr: f64,
    /// Absolute drift speed of a mantle plume, cm/yr.
    pub hotspot_drift_cm_yr: f64,
}

impl Default for TectonicParams {
    fn default() -> Self {
        Self {
            plate_count: 12,
            plate_motion_cm_yr: 6.0,
            plate_thickness_km: 35.0,
            continental_fraction: 0.45,
            density_continental_kg_m3: 2800.0,
            density_oceanic_kg_m3: 3050.0,
            mantle_density_kg_m3: 3350.0,
            convergence_threshold: 0.5,
            uplift_mm_yr: 2.0,
            orogeny_width_km: 120.0,
            flexure_rigidity: 1e23,
            isostasy: true,
            isostasy_relaxation_kyr: 20.0,
            hotspot_count: 5,
            hotspot_strength_m: 1200.0,
            hotspot_radius_km: 150.0,
            boundary_wiggle: 0.45,
            trench_width_km: 40.0,
            arc_offset_km: 120.0,
            arc_width_km: 70.0,
            rift_width_km: 60.0,
            hotspot_trail_myr: 8.0,
            hotspot_drift_cm_yr: 0.5,
        }
    }
}

impl TectonicParams {
    /// Continental crust density in kg/m³.
    #[inline]
    #[must_use]
    pub fn rho_continental(&self) -> f64 {
        self.density_continental_kg_m3
    }

    /// Oceanic crust density in kg/m³.
    #[inline]
    #[must_use]
    pub fn rho_oceanic(&self) -> f64 {
        self.density_oceanic_kg_m3
    }

    /// Isostatic elevation scale for continental crust, m per km of thickness.
    ///
    /// Airy equilibrium: `z = (1 - rho_c/rho_m) * T_c`, so thicker crust
    /// floats higher. The factor is in metres of elevation per kilometre of
    /// crust.
    #[inline]
    #[must_use]
    pub fn airy_m_per_km(&self) -> f64 {
        let ratio = self.density_continental_kg_m3 / self.mantle_density_kg_m3;
        (1.0 - ratio) * 1000.0
    }

    /// Plate speed in metres per year.
    #[inline]
    #[must_use]
    pub fn plate_speed_m_yr(&self) -> f64 {
        self.plate_motion_cm_yr * 0.01
    }

    /// Isostatic relaxation time in years.
    #[inline]
    #[must_use]
    pub fn relaxation_yr(&self) -> f64 {
        self.isostasy_relaxation_kyr * 1000.0
    }

    /// Young's modulus assumed for the elastic lithosphere, Pa.
    pub const YOUNG_MODULUS_PA: f64 = 7.0e10;
    /// Poisson ratio assumed for the elastic lithosphere.
    pub const POISSON_RATIO: f64 = 0.25;

    /// Effective elastic thickness implied by [`Self::flexure_rigidity`], m.
    ///
    /// A thin elastic plate has rigidity `D = E·Te³ / (12·(1-ν²))`, so
    /// `Te = (12·D·(1-ν²)/E)^(1/3)`. The default 1e23 N·m corresponds to
    /// roughly 25 km of elastic lithosphere, a typical cratonic value.
    #[must_use]
    pub fn elastic_thickness_m(&self) -> f64 {
        let d = self.flexure_rigidity.max(1e15);
        let e = Self::YOUNG_MODULUS_PA;
        let nu = Self::POISSON_RATIO;
        (12.0 * d * (1.0 - nu * nu) / e).max(0.0).cbrt()
    }

    /// Flexural parameter `α = (D / (Δρ·g))^(1/4)`, metres.
    ///
    /// This is the characteristic bending length of the plate: loads narrower
    /// than a few `α` are supported regionally by the lithosphere instead of
    /// being locally compensated by Airy isostasy.
    #[must_use]
    pub fn flexure_length_m(&self) -> f64 {
        let drho = (self.mantle_density_kg_m3 - self.density_continental_kg_m3).max(50.0);
        let gravity = 9.81;
        (self.flexure_rigidity.max(1e15) / (drho * gravity)).powf(0.25)
    }

    /// Gaussian smoothing length (std. dev., metres) of the flexural load
    /// response. About one flexural parameter; this filters loads narrower
    /// than the flexural wavelength so they are regionally supported.
    #[must_use]
    pub fn flexure_smoothing_m(&self) -> f64 {
        self.flexure_length_m().clamp(2e3, 5e5)
    }

    /// Gaussian smoothing length of the flexural response in grid cells.
    #[must_use]
    pub fn flexure_smoothing_cells(&self, cell_m: f64) -> f64 {
        (self.flexure_smoothing_m() / cell_m.max(1.0)).max(0.0)
    }

    /// Scale used for vertical boundary rates, m/yr (the magnitude of the
    /// user-facing mean uplift rate).
    #[must_use]
    pub fn vertical_rate_scale_m_yr(&self) -> f64 {
        (self.uplift_mm_yr * 1e-3).clamp(0.0, 5e-3)
    }

    /// Hard ceiling on any boundary vertical rate, m/yr.
    ///
    /// Real Earth orogens rise at a few mm/yr; the ceiling keeps a slider
    /// mistake (or a fast plate pair) from producing kilometre-per-step
    /// motion.
    #[must_use]
    pub fn max_vertical_rate_m_yr(&self) -> f64 {
        5e-3
    }

    /// Peak hotspot uplift rate implied by the plume strength, m/yr.
    #[must_use]
    pub fn hotspot_peak_rate_m_yr(&self) -> f64 {
        (self.hotspot_strength_m / 2.0e6).max(0.0)
    }

    /// Age scale of a hotspot trail, years.
    #[must_use]
    pub fn hotspot_trail_yr(&self) -> f64 {
        (self.hotspot_trail_myr * 1e6).max(1.0e4)
    }

    /// Absolute plume drift speed, m/yr.
    #[must_use]
    pub fn hotspot_drift_m_yr(&self) -> f64 {
        self.hotspot_drift_cm_yr * 0.01
    }

    /// Width over which boundary classification applies, km.
    ///
    /// Beyond this distance from the nearest plate boundary a cell is plate
    /// interior: no boundary forces, and landform stamps/faults treat it as
    /// intraplate. It grows with the orogen width so every boundary process
    /// fits inside the classified belt.
    #[must_use]
    pub fn boundary_influence_km(&self) -> f64 {
        let arc_reach = self.arc_offset_km + 2.0 * self.arc_width_km;
        (1.5 * self.orogeny_width_km)
            .max(arc_reach)
            .max(2.2 * self.rift_width_km)
            .max(2.0 * self.trench_width_km)
            .clamp(30.0, 3000.0)
    }
}

impl ParamBlock for TectonicParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 23] = [
            ParamSpec::int(
                "plate_count",
                "Plate count",
                "プレート数",
                ParamGroup::Tectonics,
                "",
                2.0,
                32.0,
                12.0,
                "Number of initial lithospheric plates.",
            ),
            ParamSpec::float(
                "plate_motion_cm_yr",
                "Plate speed",
                "プレート速度",
                ParamGroup::Tectonics,
                "cm/yr",
                0.0,
                20.0,
                6.0,
                "Mean relative plate velocity; drives orogeny and rifting.",
            ),
            ParamSpec::float(
                "plate_thickness_km",
                "Crust thickness",
                "地殻厚",
                ParamGroup::Tectonics,
                "km",
                5.0,
                200.0,
                35.0,
                "Mean initial crustal thickness.",
            ),
            ParamSpec::float(
                "continental_fraction",
                "Continental fraction",
                "大陸割合",
                ParamGroup::Tectonics,
                "",
                0.0,
                1.0,
                0.45,
                "Share of plates built from buoyant continental crust.",
            ),
            ParamSpec::float(
                "density_continental_kg_m3",
                "Continental density",
                "大陸地殻密度",
                ParamGroup::Tectonics,
                "kg/m³",
                2600.0,
                2950.0,
                2800.0,
                "Higher density floats the crust lower.",
            ),
            ParamSpec::float(
                "density_oceanic_kg_m3",
                "Oceanic density",
                "海洋地殻密度",
                ParamGroup::Tectonics,
                "kg/m³",
                2960.0,
                3300.0,
                3050.0,
                "Density of basaltic ocean crust.",
            ),
            ParamSpec::float(
                "mantle_density_kg_m3",
                "Mantle density",
                "マントル密度",
                ParamGroup::Tectonics,
                "kg/m³",
                3320.0,
                3500.0,
                3350.0,
                "Reference mantle density for isostatic equilibrium.",
            ),
            ParamSpec::float(
                "convergence_threshold",
                "Collision threshold",
                "衝突閾値",
                ParamGroup::Tectonics,
                "",
                0.3,
                0.8,
                0.5,
                "Relative convergence above which collision dominates.",
            ),
            ParamSpec::float(
                "uplift_mm_yr",
                "Tectonic uplift",
                "隆起速度",
                ParamGroup::Tectonics,
                "mm/yr",
                -10.0,
                50.0,
                2.0,
                "Mean vertical tectonic velocity (negative subsides).",
            ),
            ParamSpec::log(
                "orogeny_width_km",
                "Orogen width",
                "造山帯幅",
                ParamGroup::Tectonics,
                "km",
                10.0,
                500.0,
                120.0,
                "Width of the uplift belt along convergent boundaries.",
            ),
            ParamSpec::log(
                "flexure_rigidity",
                "Flexural rigidity",
                "曲げ剛性",
                ParamGroup::Tectonics,
                "N·m",
                1e20,
                1e25,
                1e23,
                "Elastic stiffness of the lithosphere; smooths tectonic loads.",
            ),
            ParamSpec::boolean(
                "isostasy",
                "Isostasy",
                "アイソスタシー",
                ParamGroup::Tectonics,
                true,
                "Relax topography towards crustal buoyancy equilibrium.",
            ),
            ParamSpec::log(
                "isostasy_relaxation_kyr",
                "Isostatic relaxation",
                "アイソスタシー緩和",
                ParamGroup::Tectonics,
                "kyr",
                1.0,
                500.0,
                20.0,
                "Time constant for isostatic compensation.",
            ),
            ParamSpec::int(
                "hotspot_count",
                "Hotspots",
                "ホットスポット",
                ParamGroup::Tectonics,
                "",
                0.0,
                50.0,
                5.0,
                "Number of mantle plumes heating the base of the plates.",
            ),
            ParamSpec::float(
                "hotspot_strength_m",
                "Hotspot strength",
                "ホットスポット強度",
                ParamGroup::Tectonics,
                "m",
                0.0,
                5000.0,
                1200.0,
                "Peak plume-supported uplift.",
            ),
            ParamSpec::log(
                "hotspot_radius_km",
                "Hotspot radius",
                "ホットスポット半径",
                ParamGroup::Tectonics,
                "km",
                20.0,
                1000.0,
                150.0,
                "Surface footprint radius of a plume.",
            ),
            ParamSpec::float(
                "boundary_wiggle",
                "Boundary irregularity",
                "境界の不規則さ",
                ParamGroup::Tectonics,
                "",
                0.0,
                1.0,
                0.45,
                "Amount of noise added to otherwise straight plate boundaries.",
            ),
            ParamSpec::log(
                "trench_width_km",
                "Trench width",
                "海溝幅",
                ParamGroup::Tectonics,
                "km",
                5.0,
                200.0,
                40.0,
                "Width of the down-flexed trench on the subducting oceanic plate.",
            ),
            ParamSpec::float(
                "arc_offset_km",
                "Arc offset",
                "火山弧距離",
                ParamGroup::Tectonics,
                "km",
                0.0,
                500.0,
                120.0,
                "Distance from a subduction boundary to the volcanic-arc axis.",
            ),
            ParamSpec::log(
                "arc_width_km",
                "Arc width",
                "火山弧幅",
                ParamGroup::Tectonics,
                "km",
                10.0,
                300.0,
                70.0,
                "Width of the volcanic-arc uplift belt on the overriding plate.",
            ),
            ParamSpec::log(
                "rift_width_km",
                "Rift width",
                "リフト幅",
                ParamGroup::Tectonics,
                "km",
                10.0,
                300.0,
                60.0,
                "Half-width of the axial rift / mid-ocean-ridge and its flanks.",
            ),
            ParamSpec::log(
                "hotspot_trail_myr",
                "Hotspot trail age",
                "ホットスポット痕跡",
                ParamGroup::Tectonics,
                "Myr",
                0.1,
                100.0,
                8.0,
                "Age scale of the swell trail left behind a moving plate.",
            ),
            ParamSpec::float(
                "hotspot_drift_cm_yr",
                "Plume drift",
                "プルーム移動",
                ParamGroup::Tectonics,
                "cm/yr",
                0.0,
                10.0,
                0.5,
                "Absolute drift speed of mantle plumes (fixed frame).",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            f64::from(self.plate_count),
            self.plate_motion_cm_yr,
            self.plate_thickness_km,
            self.continental_fraction,
            self.density_continental_kg_m3,
            self.density_oceanic_kg_m3,
            self.mantle_density_kg_m3,
            self.convergence_threshold,
            self.uplift_mm_yr,
            self.orogeny_width_km,
            self.flexure_rigidity,
            f64::from(u8::from(self.isostasy)),
            self.isostasy_relaxation_kyr,
            f64::from(self.hotspot_count),
            self.hotspot_strength_m,
            self.hotspot_radius_km,
            self.boundary_wiggle,
            self.trench_width_km,
            self.arc_offset_km,
            self.arc_width_km,
            self.rift_width_km,
            self.hotspot_trail_myr,
            self.hotspot_drift_cm_yr,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.plate_count = g(0, 12.0).round().max(1.0) as u32;
        self.plate_motion_cm_yr = g(1, 6.0);
        self.plate_thickness_km = g(2, 35.0);
        self.continental_fraction = g(3, 0.45);
        self.density_continental_kg_m3 = g(4, 2800.0);
        self.density_oceanic_kg_m3 = g(5, 3050.0);
        self.mantle_density_kg_m3 = g(6, 3350.0);
        self.convergence_threshold = g(7, 0.5);
        self.uplift_mm_yr = g(8, 6.0);
        self.orogeny_width_km = g(9, 120.0);
        self.flexure_rigidity = g(10, 1e23);
        self.isostasy = g(11, 1.0) >= 0.5;
        self.isostasy_relaxation_kyr = g(12, 20.0);
        self.hotspot_count = g(13, 5.0).round().max(0.0) as u32;
        self.hotspot_strength_m = g(14, 1200.0);
        self.hotspot_radius_km = g(15, 150.0);
        self.boundary_wiggle = g(16, 0.45);
        self.trench_width_km = g(17, 40.0);
        self.arc_offset_km = g(18, 120.0);
        self.arc_width_km = g(19, 70.0);
        self.rift_width_km = g(20, 60.0);
        self.hotspot_trail_myr = g(21, 8.0);
        self.hotspot_drift_cm_yr = g(22, 0.5);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "plate_count" => self.plate_count = value.round().max(1.0) as u32,
            "plate_motion_cm_yr" => self.plate_motion_cm_yr = value,
            "plate_thickness_km" => self.plate_thickness_km = value,
            "continental_fraction" => self.continental_fraction = value,
            "density_continental_kg_m3" => self.density_continental_kg_m3 = value,
            "density_oceanic_kg_m3" => self.density_oceanic_kg_m3 = value,
            "mantle_density_kg_m3" => self.mantle_density_kg_m3 = value,
            "convergence_threshold" => self.convergence_threshold = value,
            "uplift_mm_yr" => self.uplift_mm_yr = value,
            "orogeny_width_km" => self.orogeny_width_km = value,
            "flexure_rigidity" => self.flexure_rigidity = value,
            "isostasy" => self.isostasy = value >= 0.5,
            "isostasy_relaxation_kyr" => self.isostasy_relaxation_kyr = value,
            "hotspot_count" => self.hotspot_count = value.round().max(0.0) as u32,
            "hotspot_strength_m" => self.hotspot_strength_m = value,
            "hotspot_radius_km" => self.hotspot_radius_km = value,
            "boundary_wiggle" => self.boundary_wiggle = value,
            "trench_width_km" => self.trench_width_km = value,
            "arc_offset_km" => self.arc_offset_km = value,
            "arc_width_km" => self.arc_width_km = value,
            "rift_width_km" => self.rift_width_km = value,
            "hotspot_trail_myr" => self.hotspot_trail_myr = value,
            "hotspot_drift_cm_yr" => self.hotspot_drift_cm_yr = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "plate_count" => f64::from(self.plate_count),
            "plate_motion_cm_yr" => self.plate_motion_cm_yr,
            "plate_thickness_km" => self.plate_thickness_km,
            "continental_fraction" => self.continental_fraction,
            "density_continental_kg_m3" => self.density_continental_kg_m3,
            "density_oceanic_kg_m3" => self.density_oceanic_kg_m3,
            "mantle_density_kg_m3" => self.mantle_density_kg_m3,
            "convergence_threshold" => self.convergence_threshold,
            "uplift_mm_yr" => self.uplift_mm_yr,
            "orogeny_width_km" => self.orogeny_width_km,
            "flexure_rigidity" => self.flexure_rigidity,
            "isostasy" => f64::from(u8::from(self.isostasy)),
            "isostasy_relaxation_kyr" => self.isostasy_relaxation_kyr,
            "hotspot_count" => f64::from(self.hotspot_count),
            "hotspot_strength_m" => self.hotspot_strength_m,
            "hotspot_radius_km" => self.hotspot_radius_km,
            "boundary_wiggle" => self.boundary_wiggle,
            "trench_width_km" => self.trench_width_km,
            "arc_offset_km" => self.arc_offset_km,
            "arc_width_km" => self.arc_width_km,
            "rift_width_km" => self.rift_width_km,
            "hotspot_trail_myr" => self.hotspot_trail_myr,
            "hotspot_drift_cm_yr" => self.hotspot_drift_cm_yr,
            _ => return None,
        })
    }

    fn validate(&self) -> crate::error::GeoResult<()> {
        // Ensure the mantle is denser than either crust type; otherwise Airy
        // isostasy would make continental crust sink below oceanic crust.
        if self.mantle_density_kg_m3
            <= self
                .density_continental_kg_m3
                .max(self.density_oceanic_kg_m3)
        {
            return Err(crate::error::GeoError::InvalidConfig(
                "mantle density must exceed crustal densities for stable isostasy".into(),
            ));
        }
        if self.density_oceanic_kg_m3 <= self.density_continental_kg_m3 {
            return Err(crate::error::GeoError::InvalidConfig(
                "oceanic crust must be denser than continental crust".into(),
            ));
        }
        Ok(())
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.plate_count = rng.range_u32(5, 20);
        self.plate_motion_cm_yr = rng.range_f64(2.0, 14.0);
        self.plate_thickness_km = rng.range_f64(20.0, 60.0);
        self.continental_fraction = rng.range_f64(0.25, 0.7);
        self.uplift_mm_yr = rng.range_f64(2.0, 15.0);
        self.orogeny_width_km = rng.range_f64(40.0, 260.0);
        self.hotspot_count = rng.range_u32(0, 12);
        self.hotspot_strength_m = rng.range_f64(300.0, 2500.0);
        self.hotspot_radius_km = rng.range_f64(60.0, 300.0);
        self.boundary_wiggle = rng.range_f64(0.2, 0.75);
        self.trench_width_km = rng.range_f64(15.0, 90.0);
        self.arc_offset_km = rng.range_f64(50.0, 250.0);
        self.arc_width_km = rng.range_f64(30.0, 140.0);
        self.rift_width_km = rng.range_f64(25.0, 120.0);
        self.hotspot_trail_myr = rng.range_f64(2.0, 25.0);
        self.hotspot_drift_cm_yr = rng.range_f64(0.0, 2.5);
        self.isostasy = true;
    }
}

/// Rocks, strata and erodibility (spec §4).
#[derive(Clone, Debug, PartialEq)]
pub struct GeologyParams {
    /// Number of sedimentary/volcanic layers per province.
    pub strata_layers: u32,
    /// Multiplier on generated layer thicknesses.
    pub strata_thickness_scale: f64,
    /// Regional dip of the strata, degrees.
    pub dip_deg: f64,
    /// Regional strike of the strata, degrees from east.
    pub strike_deg: f64,
    /// Probability that a layer boundary is an unconformity.
    pub unconformity_chance: f64,
    /// 0 = every province shares one rock, 1 = maximum variety.
    pub rock_diversity: f64,
    /// Multiplier on erosion resistance (higher = more resistant world).
    pub erodibility_scale: f64,
    /// Share of layers that are sedimentary (softer, porous).
    pub sedimentary_fraction: f64,
    /// Share of provinces whose surface is a hard crystalline shield.
    pub shield_fraction: f64,
    /// Spread of the per-layer dip around the regional dip, degrees.
    pub dip_variation_deg: f64,
}

impl Default for GeologyParams {
    fn default() -> Self {
        Self {
            strata_layers: 7,
            strata_thickness_scale: 0.16,
            dip_deg: 8.0,
            strike_deg: 0.0,
            unconformity_chance: 0.08,
            rock_diversity: 0.6,
            erodibility_scale: 1.0,
            sedimentary_fraction: 0.4,
            shield_fraction: 0.35,
            dip_variation_deg: 4.0,
        }
    }
}

impl ParamBlock for GeologyParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 10] = [
            ParamSpec::int(
                "strata_layers",
                "Strata layers",
                "層数",
                ParamGroup::Geology,
                "",
                1.0,
                100.0,
                7.0,
                "Layers per stratigraphic column.",
            ),
            ParamSpec::log(
                "strata_thickness_scale",
                "Layer thickness",
                "層厚スケール",
                ParamGroup::Geology,
                "×",
                0.1,
                10.0,
                0.16,
                "Scales the generated layer thicknesses.",
            ),
            ParamSpec::float(
                "dip_deg",
                "Dip",
                "傾斜",
                ParamGroup::Geology,
                "°",
                0.0,
                60.0,
                8.0,
                "Regional dip of the strata.",
            ),
            ParamSpec::float(
                "strike_deg",
                "Strike",
                "走向",
                ParamGroup::Geology,
                "°",
                0.0,
                360.0,
                0.0,
                "Regional strike direction.",
            ),
            ParamSpec::float(
                "unconformity_chance",
                "Unconformity chance",
                "不整合率",
                ParamGroup::Geology,
                "",
                0.0,
                0.5,
                0.08,
                "Probability that a layer boundary is an erosional gap.",
            ),
            ParamSpec::float(
                "rock_diversity",
                "Rock diversity",
                "岩石多様性",
                ParamGroup::Geology,
                "",
                0.0,
                1.0,
                0.6,
                "How much rock type varies between provinces.",
            ),
            ParamSpec::log(
                "erodibility_scale",
                "Erodibility scale",
                "侵食性スケール",
                ParamGroup::Geology,
                "×",
                0.1,
                10.0,
                1.0,
                "Global multiplier on rock resistance.",
            ),
            ParamSpec::float(
                "sedimentary_fraction",
                "Sedimentary share",
                "堆積岩割合",
                ParamGroup::Geology,
                "",
                0.0,
                1.0,
                0.4,
                "Share of soft sedimentary layers in each column.",
            ),
            ParamSpec::float(
                "shield_fraction",
                "Shield fraction",
                "盾状地割合",
                ParamGroup::Geology,
                "",
                0.0,
                1.0,
                0.35,
                "Share of provinces with a hard crystalline shield at the surface.",
            ),
            ParamSpec::float(
                "dip_variation_deg",
                "Dip variation",
                "傾斜変動",
                ParamGroup::Geology,
                "°",
                0.0,
                30.0,
                4.0,
                "Per-layer scatter of the regional dip.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            f64::from(self.strata_layers),
            self.strata_thickness_scale,
            self.dip_deg,
            self.strike_deg,
            self.unconformity_chance,
            self.rock_diversity,
            self.erodibility_scale,
            self.sedimentary_fraction,
            self.shield_fraction,
            self.dip_variation_deg,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.strata_layers = g(0, 7.0).round().clamp(1.0, 100.0) as u32;
        self.strata_thickness_scale = g(1, 1.0);
        self.dip_deg = g(2, 8.0);
        self.strike_deg = g(3, 0.0);
        self.unconformity_chance = g(4, 0.08);
        self.rock_diversity = g(5, 0.6);
        self.erodibility_scale = g(6, 1.0);
        self.sedimentary_fraction = g(7, 0.4);
        self.shield_fraction = g(8, 0.35);
        self.dip_variation_deg = g(9, 4.0);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "strata_layers" => self.strata_layers = value.round().clamp(1.0, 100.0) as u32,
            "strata_thickness_scale" => self.strata_thickness_scale = value,
            "dip_deg" => self.dip_deg = value,
            "strike_deg" => self.strike_deg = value,
            "unconformity_chance" => self.unconformity_chance = value,
            "rock_diversity" => self.rock_diversity = value,
            "erodibility_scale" => self.erodibility_scale = value,
            "sedimentary_fraction" => self.sedimentary_fraction = value,
            "shield_fraction" => self.shield_fraction = value,
            "dip_variation_deg" => self.dip_variation_deg = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "strata_layers" => f64::from(self.strata_layers),
            "strata_thickness_scale" => self.strata_thickness_scale,
            "dip_deg" => self.dip_deg,
            "strike_deg" => self.strike_deg,
            "unconformity_chance" => self.unconformity_chance,
            "rock_diversity" => self.rock_diversity,
            "erodibility_scale" => self.erodibility_scale,
            "sedimentary_fraction" => self.sedimentary_fraction,
            "shield_fraction" => self.shield_fraction,
            "dip_variation_deg" => self.dip_variation_deg,
            _ => return None,
        })
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.strata_layers = rng.range_u32(3, 14);
        self.strata_thickness_scale = rng.range_f64(0.4, 2.5);
        self.dip_deg = rng.range_f64(0.0, 25.0);
        self.strike_deg = rng.range_f64(0.0, 360.0);
        self.rock_diversity = rng.range_f64(0.2, 0.9);
        self.erodibility_scale = rng.range_f64(0.5, 2.0);
        self.sedimentary_fraction = rng.range_f64(0.2, 0.7);
        self.shield_fraction = rng.range_f64(0.1, 0.6);
        self.dip_variation_deg = rng.range_f64(1.0, 12.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flexure_parameters_are_physical() {
        let p = TectonicParams::default();
        let te_km = p.elastic_thickness_m() / 1000.0;
        assert!(
            (10.0..=40.0).contains(&te_km),
            "default elastic thickness {te_km} km is not lithospheric"
        );
        let alpha_km = p.flexure_length_m() / 1000.0;
        assert!(
            (20.0..=120.0).contains(&alpha_km),
            "default flexural parameter {alpha_km} km out of range"
        );
        // Stiffer plates bend over longer distances, monotonically.
        let mut stiffer = p.clone();
        stiffer.flexure_rigidity *= 10.0;
        assert!(stiffer.elastic_thickness_m() > p.elastic_thickness_m());
        assert!(stiffer.flexure_length_m() > p.flexure_length_m());
        // And everything stays finite and positive across the schema range.
        for rigidity in [1e20, 1e21, 1e22, 1e23, 1e24, 1e25] {
            let mut q = p.clone();
            q.flexure_rigidity = rigidity;
            assert!(q.elastic_thickness_m().is_finite() && q.elastic_thickness_m() > 0.0);
            assert!(q.flexure_length_m().is_finite() && q.flexure_length_m() > 0.0);
            assert!(q.flexure_smoothing_cells(10_000.0).is_finite());
        }
    }

    #[test]
    fn vertical_rate_scale_respects_the_physical_ceiling() {
        let mut p = TectonicParams::default();
        p.uplift_mm_yr = 2.0;
        assert!((p.vertical_rate_scale_m_yr() - 2e-3).abs() < 1e-12);
        // A slider value of 50 mm/yr is clamped to the 5 mm/yr physical cap.
        p.uplift_mm_yr = 50.0;
        assert!(p.vertical_rate_scale_m_yr() <= p.max_vertical_rate_m_yr() + 1e-12);
        assert!(p.max_vertical_rate_m_yr() <= 5e-3 + 1e-12);
        // Negative mean uplift does not create a global subsidence field.
        p.uplift_mm_yr = -5.0;
        assert_eq!(p.vertical_rate_scale_m_yr(), 0.0);
    }

    #[test]
    fn boundary_influence_covers_every_boundary_process() {
        let p = TectonicParams::default();
        let width = p.boundary_influence_km();
        assert!(width >= 1.5 * p.orogeny_width_km);
        assert!(width >= p.arc_offset_km + 2.0 * p.arc_width_km);
        assert!(width >= 2.2 * p.rift_width_km);
    }

    #[test]
    fn hotspot_helpers_scale_with_the_strength() {
        let mut p = TectonicParams::default();
        p.hotspot_strength_m = 2000.0;
        assert!((p.hotspot_peak_rate_m_yr() - 1e-3).abs() < 1e-15);
        p.hotspot_trail_myr = 5.0;
        assert!((p.hotspot_trail_yr() - 5e6).abs() < 1e-6);
        p.hotspot_drift_cm_yr = 2.0;
        assert!((p.hotspot_drift_m_yr() - 0.02).abs() < 1e-12);
    }
}
