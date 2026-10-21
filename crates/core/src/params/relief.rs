//! Relief generation parameters: fractal noise and structural stamps
//! (spec §2a–2b).

use crate::math::noise::{FractalKind, NoiseField, NoiseKind, NoiseParams};
use crate::math::rng::Rng;
use crate::params::spec::{ParamGroup, ParamSpec};

use super::ParamBlock;

/// Base relief noise and structural deformation (spec §2a).
#[derive(Clone, Debug, PartialEq)]
pub struct ReliefParams {
    /// Noise kind index into [`NoiseKind::ALL`].
    pub noise_kind: f64,
    /// Fractal kind index into [`FractalKind::ALL`].
    pub fractal_kind: f64,
    /// Octave count.
    pub octaves: u32,
    /// Frequency multiplier between octaves.
    pub lacunarity: f64,
    /// Amplitude multiplier between octaves.
    pub persistence: f64,
    /// Directional fabric strength.
    pub anisotropy: f64,
    /// Fabric direction, degrees from east.
    pub anisotropy_deg: f64,
    /// Vertical offset across boundary faults, m.
    pub fault_offset_m: f64,
    /// Wavelength of fold belts, km.
    pub fold_wavelength_km: f64,
    /// Amplitude of fold belts, m.
    pub fold_amplitude_m: f64,
    /// Density of named landform stamps (0 = none, 2 = crowded).
    pub feature_density: f64,
}

impl Default for ReliefParams {
    fn default() -> Self {
        Self {
            noise_kind: 0.0,
            fractal_kind: 0.0,
            octaves: 6,
            lacunarity: 2.0,
            persistence: 0.5,
            anisotropy: 0.2,
            anisotropy_deg: 30.0,
            fault_offset_m: 300.0,
            fold_wavelength_km: 80.0,
            fold_amplitude_m: 500.0,
            feature_density: 0.9,
        }
    }
}

impl ReliefParams {
    /// Selected noise kind.
    #[must_use]
    pub fn noise_kind(&self) -> NoiseKind {
        NoiseKind::ALL[self.noise_kind.round().clamp(0.0, 4.0) as usize]
    }

    /// Selected fractal kind.
    #[must_use]
    pub fn fractal_kind(&self) -> FractalKind {
        FractalKind::ALL[self.fractal_kind.round().clamp(0.0, 2.0) as usize]
    }

    /// Builds the noise configuration for the relief generator.
    #[must_use]
    pub fn noise_params(&self) -> NoiseParams {
        NoiseParams {
            kind: self.noise_kind(),
            fractal: self.fractal_kind(),
            octaves: self.octaves,
            lacunarity: self.lacunarity,
            persistence: self.persistence,
            anisotropy: self.anisotropy,
            direction_deg: self.anisotropy_deg,
        }
    }

    /// A seeded noise field configured by these parameters.
    #[must_use]
    pub fn noise_field(&self, seed: u64) -> NoiseField {
        NoiseField::new(self.noise_params(), seed)
    }
}

impl ParamBlock for ReliefParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 11] = [
            ParamSpec::enumerated(
                "noise_kind",
                "Noise type",
                "ノイズ種類",
                ParamGroup::Relief,
                &["Perlin", "Simplex", "Value", "Wavelet", "Gaussian spikes"],
                0.0,
                "Base fractal primitive for the relief.",
            ),
            ParamSpec::enumerated(
                "fractal_kind",
                "Fractal style",
                "フラクタル",
                ParamGroup::Relief,
                &["fBm", "Ridged", "Billow"],
                0.0,
                "How octaves combine: smooth fBm, sharp ridges or puffy billows.",
            ),
            ParamSpec::int(
                "octaves",
                "Octaves",
                "オクターブ",
                ParamGroup::Relief,
                "",
                1.0,
                16.0,
                6.0,
                "Fractal detail levels.",
            ),
            ParamSpec::float(
                "lacunarity",
                "Lacunarity",
                "ラクナリティ",
                ParamGroup::Relief,
                "×",
                1.0,
                4.0,
                2.0,
                "Frequency multiplier between octaves.",
            ),
            ParamSpec::float(
                "persistence",
                "Persistence",
                "永続度",
                ParamGroup::Relief,
                "",
                0.05,
                1.0,
                0.5,
                "Amplitude decay between octaves.",
            ),
            ParamSpec::float(
                "anisotropy",
                "Anisotropy",
                "異方性",
                ParamGroup::Relief,
                "",
                0.0,
                1.0,
                0.2,
                "Directional fabric strength: 0 isotropic, 1 strongly linear.",
            ),
            ParamSpec::float(
                "anisotropy_deg",
                "Fabric direction",
                "異方性方向",
                ParamGroup::Relief,
                "°",
                0.0,
                360.0,
                30.0,
                "Trend of the anisotropic fabric.",
            ),
            ParamSpec::float(
                "fault_offset_m",
                "Fault offset",
                "断層変位",
                ParamGroup::Relief,
                "m",
                0.0,
                1000.0,
                300.0,
                "Vertical displacement across boundary faults.",
            ),
            ParamSpec::log(
                "fold_wavelength_km",
                "Fold wavelength",
                "褶曲波長",
                ParamGroup::Relief,
                "km",
                1.0,
                500.0,
                80.0,
                "Spacing of fold-belt ridges.",
            ),
            ParamSpec::float(
                "fold_amplitude_m",
                "Fold amplitude",
                "褶曲振幅",
                ParamGroup::Relief,
                "m",
                0.0,
                5000.0,
                500.0,
                "Height of fold-belt undulations.",
            ),
            ParamSpec::float(
                "feature_density",
                "Landform density",
                "地形特徴密度",
                ParamGroup::Relief,
                "×",
                0.0,
                2.0,
                0.9,
                "How many named landforms (ranges, rifts, peaks…) are stamped.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            self.noise_kind,
            self.fractal_kind,
            f64::from(self.octaves),
            self.lacunarity,
            self.persistence,
            self.anisotropy,
            self.anisotropy_deg,
            self.fault_offset_m,
            self.fold_wavelength_km,
            self.fold_amplitude_m,
            self.feature_density,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.noise_kind = g(0, 0.0).round().clamp(0.0, 4.0);
        self.fractal_kind = g(1, 0.0).round().clamp(0.0, 2.0);
        self.octaves = g(2, 6.0).round().clamp(1.0, 16.0) as u32;
        self.lacunarity = g(3, 2.0);
        self.persistence = g(4, 0.5);
        self.anisotropy = g(5, 0.3);
        self.anisotropy_deg = g(6, 30.0);
        self.fault_offset_m = g(7, 300.0);
        self.fold_wavelength_km = g(8, 80.0);
        self.fold_amplitude_m = g(9, 500.0);
        self.feature_density = g(10, 0.9);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "noise_kind" => self.noise_kind = value.round().clamp(0.0, 4.0),
            "fractal_kind" => self.fractal_kind = value.round().clamp(0.0, 2.0),
            "octaves" => self.octaves = value.round().clamp(1.0, 16.0) as u32,
            "lacunarity" => self.lacunarity = value,
            "persistence" => self.persistence = value,
            "anisotropy" => self.anisotropy = value,
            "anisotropy_deg" => self.anisotropy_deg = value,
            "fault_offset_m" => self.fault_offset_m = value,
            "fold_wavelength_km" => self.fold_wavelength_km = value,
            "fold_amplitude_m" => self.fold_amplitude_m = value,
            "feature_density" => self.feature_density = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "noise_kind" => self.noise_kind,
            "fractal_kind" => self.fractal_kind,
            "octaves" => f64::from(self.octaves),
            "lacunarity" => self.lacunarity,
            "persistence" => self.persistence,
            "anisotropy" => self.anisotropy,
            "anisotropy_deg" => self.anisotropy_deg,
            "fault_offset_m" => self.fault_offset_m,
            "fold_wavelength_km" => self.fold_wavelength_km,
            "fold_amplitude_m" => self.fold_amplitude_m,
            "feature_density" => self.feature_density,
            _ => return None,
        })
    }

    fn normalize(&mut self) {
        self.noise_kind = self.noise_kind.round().clamp(0.0, 4.0);
        self.fractal_kind = self.fractal_kind.round().clamp(0.0, 2.0);
        self.octaves = self.octaves.clamp(1, 16);
        // The annual cost of octaves is linear in cells; cap the pair so a UI
        // slider cannot silently turn generation into a multi-second stall.
        if self.octaves > 8 && self.persistence > 0.6 {
            self.persistence = 0.6;
        }
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.noise_kind = rng.range_u32(0, 3) as f64;
        self.fractal_kind = rng.range_u32(0, 3) as f64;
        self.octaves = rng.range_u32(4, 8);
        self.lacunarity = rng.range_f64(1.8, 2.6);
        self.persistence = rng.range_f64(0.42, 0.62);
        self.anisotropy = rng.range_f64(0.0, 0.6);
        self.anisotropy_deg = rng.range_f64(0.0, 360.0);
        self.fault_offset_m = rng.range_f64(50.0, 600.0);
        self.fold_wavelength_km = rng.range_f64(40.0, 180.0);
        self.fold_amplitude_m = rng.range_f64(100.0, 1200.0);
        self.feature_density = rng.range_f64(0.4, 1.4);
    }
}
