//! Parameter metadata: one [`ParamSpec`] describes every tunable value of the
//! engine — key, bilingual labels, unit, range, default, scale and UI kind.
//!
//! The schema is the single source of truth for four consumers:
//!
//! * the Rust default configuration and its validation,
//! * the CLI (`--key value` flags, presets, JSON dump),
//! * the WASM ABI (`gg_schema` returns this table as JSON),
//! * the Web UI, which builds its whole control panel from that JSON.
//!
//! Adding a parameter therefore means adding one spec in the owning block and
//! one field in that block's struct — no UI or serialisation work.

use crate::math::rng::Rng;

/// UI grouping of parameters (also drives the Web UI sections).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamGroup {
    /// Grid, extent, datum and seed.
    World,
    /// Duration, step size and history settings.
    Time,
    /// Base relief noise and landform stamps.
    Relief,
    /// Plates, motion, orogeny, isostasy and hotspots.
    Tectonics,
    /// Rocks, strata and erodibility.
    Geology,
    /// Temperature, precipitation, wind and long-term climate.
    Climate,
    /// Vegetation and biomes.
    Ecology,
    /// Runoff and flow routing.
    Hydrology,
    /// River incision and sediment transport.
    Fluvial,
    /// Hillslope diffusion, soil and landslides.
    Hillslope,
    /// Ice sheets and glacial erosion.
    Cryosphere,
    /// Wind erosion and dunes.
    Aeolian,
    /// Waves, tides and coastlines.
    Coastal,
    /// Dissolution and chemical weathering.
    Chemistry,
}

impl ParamGroup {
    /// All groups in display order.
    pub const ALL: [Self; 14] = [
        Self::World,
        Self::Time,
        Self::Relief,
        Self::Tectonics,
        Self::Geology,
        Self::Climate,
        Self::Ecology,
        Self::Hydrology,
        Self::Fluvial,
        Self::Hillslope,
        Self::Cryosphere,
        Self::Aeolian,
        Self::Coastal,
        Self::Chemistry,
    ];

    /// Stable key used in JSON.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::Time => "time",
            Self::Relief => "relief",
            Self::Tectonics => "tectonics",
            Self::Geology => "geology",
            Self::Climate => "climate",
            Self::Ecology => "ecology",
            Self::Hydrology => "hydrology",
            Self::Fluvial => "fluvial",
            Self::Hillslope => "hillslope",
            Self::Cryosphere => "cryosphere",
            Self::Aeolian => "aeolian",
            Self::Coastal => "coastal",
            Self::Chemistry => "chemistry",
        }
    }

    /// English label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::World => "World",
            Self::Time => "Time",
            Self::Relief => "Relief",
            Self::Tectonics => "Tectonics",
            Self::Geology => "Geology",
            Self::Climate => "Climate",
            Self::Ecology => "Ecology",
            Self::Hydrology => "Hydrology",
            Self::Fluvial => "Rivers",
            Self::Hillslope => "Hillslopes",
            Self::Cryosphere => "Ice",
            Self::Aeolian => "Wind",
            Self::Coastal => "Coast",
            Self::Chemistry => "Chemistry",
        }
    }

    /// Japanese label.
    #[must_use]
    pub const fn label_ja(self) -> &'static str {
        match self {
            Self::World => "ワールド",
            Self::Time => "時間",
            Self::Relief => "地形",
            Self::Tectonics => "テクトニクス",
            Self::Geology => "地質",
            Self::Climate => "気候",
            Self::Ecology => "生態系",
            Self::Hydrology => "水循環",
            Self::Fluvial => "河川",
            Self::Hillslope => "斜面",
            Self::Cryosphere => "氷河",
            Self::Aeolian => "風成",
            Self::Coastal => "海岸",
            Self::Chemistry => "化学風化",
        }
    }
}

/// How a numeric parameter is distributed for sliders and randomisation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamScale {
    /// Linear from `min` to `max`.
    Linear,
    /// Logarithmic; `min` must be positive. Used for rate coefficients that
    /// span many orders of magnitude.
    Log,
}

/// The kind of control a parameter needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// Continuous floating-point value.
    Float,
    /// Integer value, rounded on read.
    Int,
    /// One of a fixed list of named choices, stored as its index.
    Enum(&'static [&'static str]),
}

/// Complete metadata for one tunable parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    /// Stable snake_case key; used in JSON, CLI and presets.
    pub key: &'static str,
    /// English label.
    pub label: &'static str,
    /// Japanese label.
    pub label_ja: &'static str,
    /// UI group.
    pub group: ParamGroup,
    /// Unit symbol (`"m/yr"`, `"mm/yr"`, `"°C"`, `""` for dimensionless).
    pub unit: &'static str,
    /// Lower bound.
    pub min: f64,
    /// Upper bound.
    pub max: f64,
    /// Default value.
    pub default: f64,
    /// Slider scale.
    pub scale: ParamScale,
    /// Value kind.
    pub kind: ParamKind,
    /// One-line guidance shown as a tooltip.
    pub help: &'static str,
}

impl ParamSpec {
    /// A linear floating-point parameter.
    #[must_use]
    pub const fn float(
        key: &'static str,
        label: &'static str,
        label_ja: &'static str,
        group: ParamGroup,
        unit: &'static str,
        min: f64,
        max: f64,
        default: f64,
        help: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            label_ja,
            group,
            unit,
            min,
            max,
            default,
            scale: ParamScale::Linear,
            kind: ParamKind::Float,
            help,
        }
    }

    /// A logarithmic floating-point parameter (`min > 0`).
    #[must_use]
    pub const fn log(
        key: &'static str,
        label: &'static str,
        label_ja: &'static str,
        group: ParamGroup,
        unit: &'static str,
        min: f64,
        max: f64,
        default: f64,
        help: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            label_ja,
            group,
            unit,
            min,
            max,
            default,
            scale: ParamScale::Log,
            kind: ParamKind::Float,
            help,
        }
    }

    /// An integer parameter.
    #[must_use]
    pub const fn int(
        key: &'static str,
        label: &'static str,
        label_ja: &'static str,
        group: ParamGroup,
        unit: &'static str,
        min: f64,
        max: f64,
        default: f64,
        help: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            label_ja,
            group,
            unit,
            min,
            max,
            default,
            scale: ParamScale::Linear,
            kind: ParamKind::Int,
            help,
        }
    }

    /// A named-choice parameter; `default` is the choice index.
    #[must_use]
    pub const fn enumerated(
        key: &'static str,
        label: &'static str,
        label_ja: &'static str,
        group: ParamGroup,
        options: &'static [&'static str],
        default: f64,
        help: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            label_ja,
            group,
            unit: "",
            min: 0.0,
            max: (options.len() - 1) as f64,
            default,
            scale: ParamScale::Linear,
            kind: ParamKind::Enum(options),
            help,
        }
    }

    /// A boolean parameter, stored as `0` (off) or `1` (on).
    #[must_use]
    pub const fn boolean(
        key: &'static str,
        label: &'static str,
        label_ja: &'static str,
        group: ParamGroup,
        default_on: bool,
        help: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            label_ja,
            group,
            unit: "",
            min: 0.0,
            max: 1.0,
            default: if default_on { 1.0 } else { 0.0 },
            scale: ParamScale::Linear,
            kind: ParamKind::Enum(&["Off", "On"]),
            help,
        }
    }

    /// True when the parameter takes whole numbers.
    #[must_use]
    pub const fn is_integer(&self) -> bool {
        matches!(self.kind, ParamKind::Int | ParamKind::Enum(_))
    }

    /// Clamps a raw value into the declared range and rounds integers/enums.
    #[must_use]
    pub fn sanitize(&self, value: f64) -> f64 {
        let v = if value.is_finite() {
            value
        } else {
            self.default
        };
        let v = v.clamp(self.min, self.max);
        if self.is_integer() {
            v.round()
        } else {
            v
        }
    }

    /// Validates that a value is inside the declared range.
    pub fn validate_value(&self, value: f64) -> Result<(), crate::error::GeoError> {
        if !value.is_finite() {
            return Err(crate::error::GeoError::param(
                self.key,
                value,
                "must be finite",
            ));
        }
        if value < self.min || value > self.max {
            return Err(crate::error::GeoError::param(
                self.key,
                value,
                format!("must be in {}..={}", self.min, self.max),
            ));
        }
        Ok(())
    }

    /// Samples a value for this parameter, biased to physically plausible
    /// regions: logarithmic specs are sampled log-uniformly, enums uniformly,
    /// and integers with the neighbouring integers equally likely.
    #[must_use]
    pub fn random_value(&self, rng: &mut Rng) -> f64 {
        match self.kind {
            ParamKind::Enum(options) => rng.below(options.len()) as f64,
            ParamKind::Int => {
                let lo = self.min.ceil();
                let hi = self.max.floor();
                if hi <= lo {
                    return lo;
                }
                (lo + (rng.next_f64() * (hi - lo + 1.0)).floor()).clamp(lo, hi)
            }
            ParamKind::Float => {
                let v = match self.scale {
                    ParamScale::Linear => rng.range_f64(self.min, self.max),
                    ParamScale::Log => {
                        let (lo, hi) = (self.min.max(1e-300).ln(), self.max.ln());
                        (rng.range_f64(lo, hi)).exp()
                    }
                };
                self.sanitize(v)
            }
        }
    }

    /// Clamps a value into the declared range without rounding, or errors.
    #[must_use]
    pub fn clamped(&self, value: f64) -> f64 {
        self.sanitize(value)
    }

    /// Renders the value in the spec's natural notation (for logs, exponent).
    #[must_use]
    pub fn display_value(&self, value: f64) -> String {
        match self.kind {
            ParamKind::Enum(options) => options
                .get(value.round().max(0.0) as usize)
                .copied()
                .unwrap_or("?")
                .to_string(),
            ParamKind::Int => format!("{:.0}", value.round()),
            ParamKind::Float => {
                if self.scale == ParamScale::Log {
                    if value != 0.0 && (value.abs() < 1e-2 || value.abs() >= 1e5) {
                        format!("{value:.2e}")
                    } else {
                        format!("{value:.4}")
                    }
                } else {
                    let span = self.max - self.min;
                    if span <= 4.0 {
                        format!("{value:.2}")
                    } else if span <= 100.0 {
                        format!("{value:.1}")
                    } else {
                        format!("{value:.0}")
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: ParamSpec = ParamSpec::float(
        "kf",
        "Erodibility",
        "侵食係数",
        ParamGroup::Fluvial,
        "m/yr",
        1e-8,
        1e-2,
        1e-4,
        "Stream-power erodibility",
    );

    #[test]
    fn keys_and_groups_are_stable() {
        assert_eq!(ParamGroup::ALL.len(), 14);
        assert_eq!(ParamGroup::Fluvial.key(), "fluvial");
        assert_eq!(ParamGroup::Fluvial.label_ja(), "河川");
        assert_eq!(SPEC.key, "kf");
    }

    #[test]
    fn sanitize_clamps_and_rounds() {
        assert_eq!(SPEC.sanitize(1e-20), 1e-8);
        assert_eq!(SPEC.sanitize(1e3), 1e-2);
        assert_eq!(SPEC.sanitize(f64::NAN), SPEC.default);
        let int = ParamSpec::int("n", "N", "N", ParamGroup::Tectonics, "", 1.0, 10.0, 5.0, "");
        assert_eq!(int.sanitize(4.6), 5.0);
        assert_eq!(int.sanitize(100.0), 10.0);
    }

    #[test]
    fn enum_and_boolean_kinds() {
        let e = ParamSpec::enumerated(
            "mode",
            "Mode",
            "モード",
            ParamGroup::Time,
            &["A", "B", "C"],
            1.0,
            "",
        );
        assert_eq!(e.max, 2.0);
        let b = ParamSpec::boolean("iso", "Iso", "アイソ", ParamGroup::Tectonics, true, "");
        assert_eq!(b.default, 1.0);
        assert_eq!(b.kind, ParamKind::Enum(&["Off", "On"]));
    }

    #[test]
    fn random_values_stay_in_range() {
        let mut rng = Rng::new(3);
        for _ in 0..1000 {
            let v = SPEC.random_value(&mut rng);
            assert!((SPEC.min..=SPEC.max).contains(&v));
        }
    }
}
