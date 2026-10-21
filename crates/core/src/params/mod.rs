//! The complete engine parameter set.
//!
//! [`Params`] aggregates one [`ParamBlock`] per subsystem. Each block owns its
//! [`ParamSpec`] table, so the schema order, the flat numeric encoding used by
//! the WASM bridge and the defaults can never drift apart: the block's
//! `values_into`/`set_values` pair is the only translation, and every block is
//! tested to round-trip.
//!
//! This module also exposes the schema as JSON (`gg_schema` in the WASM ABI)
//! and the preset table, which is how the Web UI builds itself.

pub mod climate;
pub mod presets;
pub mod processes;
pub mod relief;
pub mod spec;
pub mod surface;
pub mod tectonics;
pub mod world;

use crate::error::{GeoError, GeoResult};
use crate::math::rng::Rng;

pub use climate::{ClimateParams, EcologyParams};
pub use presets::{Preset, PRESETS};
pub use processes::{AeolianParams, ChemicalParams, CoastalParams, GlacialParams};
pub use relief::ReliefParams;
pub use spec::{ParamGroup, ParamKind, ParamScale, ParamSpec};
pub use surface::{FluvialParams, HillslopeParams, HydrologyParams};
pub use tectonics::{GeologyParams, TectonicParams};
pub use world::{format_duration, TimeParams, WorldParams};

/// A group of parameters owned by one subsystem.
///
/// Implementing this trait gives a block range validation, normalisation and
/// randomisation for free; a block only overrides those when it needs
/// cross-field consistency (e.g. time step versus duration).
pub trait ParamBlock: Clone + core::fmt::Debug + Default + Sized {
    /// Schema rows for this block, in the same order as [`Self::values_into`].
    fn specs() -> &'static [ParamSpec];

    /// Appends this block's values in schema order.
    fn values_into(&self, out: &mut Vec<f64>);

    /// Reads values written by [`Self::values_into`]; missing entries keep
    /// their current value.
    fn set_values(&mut self, values: &[f64]);

    /// Sets one key; returns `false` when the key does not belong to the block.
    fn set_key(&mut self, key: &str, value: f64) -> bool;

    /// Reads one key as `f64`.
    fn get_key(&self, key: &str) -> Option<f64>;

    /// Clamps every value into its declared range and rounds integers.
    fn normalize(&mut self) {
        let specs = Self::specs();
        let mut values = Vec::with_capacity(specs.len());
        self.values_into(&mut values);
        if values.len() != specs.len() {
            debug_assert!(
                false,
                "{}: values_into produced {} values for {} specs",
                core::any::type_name::<Self>(),
                values.len(),
                specs.len()
            );
            return;
        }
        for (value, spec) in values.iter_mut().zip(specs) {
            *value = spec.sanitize(*value);
        }
        self.set_values(&values);
    }

    /// Validates every value against its spec; blocks override to add
    /// cross-field checks.
    fn validate(&self) -> GeoResult<()> {
        let specs = Self::specs();
        let mut values = Vec::with_capacity(specs.len());
        self.values_into(&mut values);
        if values.len() != specs.len() {
            return Err(GeoError::InvalidConfig(format!(
                "{}: value/spec count mismatch",
                core::any::type_name::<Self>()
            )));
        }
        for (value, spec) in values.iter().zip(specs) {
            spec.validate_value(*value)?;
        }
        Ok(())
    }

    /// Randomises every value from its spec.
    ///
    /// Blocks override this to keep coupled choices physical (for example a
    /// short step with a long duration).
    fn randomize(&mut self, rng: &mut Rng) {
        let specs = Self::specs();
        let mut values = Vec::with_capacity(specs.len());
        self.values_into(&mut values);
        for (value, spec) in values.iter_mut().zip(specs) {
            *value = spec.random_value(rng);
        }
        self.set_values(&values);
    }
}

/// The full parameter set of a world.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Params {
    /// Opt-in escape hatch for large-grid experiments (CLI only): allows grids
    /// above the 2048² single-shot memory guard. Not part of the schema.
    pub allow_large_grid: bool,
    /// Grid, extent and datum.
    pub world: WorldParams,
    /// Duration, step and history.
    pub time: TimeParams,
    /// Base relief noise and landform stamps.
    pub relief: ReliefParams,
    /// Plates, uplift, isostasy and hotspots.
    pub tectonics: TectonicParams,
    /// Rocks and strata.
    pub geology: GeologyParams,
    /// Temperature, water, wind and climate cycles.
    pub climate: ClimateParams,
    /// Vegetation and biomes.
    pub ecology: EcologyParams,
    /// Runoff and routing.
    pub hydrology: HydrologyParams,
    /// River incision and deposition.
    pub fluvial: FluvialParams,
    /// Hillslope processes.
    pub hillslope: HillslopeParams,
    /// Ice.
    pub glacial: GlacialParams,
    /// Wind.
    pub aeolian: AeolianParams,
    /// Coastlines.
    pub coastal: CoastalParams,
    /// Chemical weathering.
    pub chemical: ChemicalParams,
}

impl Params {
    /// A short, valid default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Schema rows for every block, in encoding order.
    #[must_use]
    pub fn schema() -> Vec<ParamSpec> {
        let mut specs = Vec::with_capacity(96);
        specs.extend_from_slice(WorldParams::specs());
        specs.extend_from_slice(TimeParams::specs());
        specs.extend_from_slice(ReliefParams::specs());
        specs.extend_from_slice(TectonicParams::specs());
        specs.extend_from_slice(GeologyParams::specs());
        specs.extend_from_slice(ClimateParams::specs());
        specs.extend_from_slice(EcologyParams::specs());
        specs.extend_from_slice(HydrologyParams::specs());
        specs.extend_from_slice(FluvialParams::specs());
        specs.extend_from_slice(HillslopeParams::specs());
        specs.extend_from_slice(GlacialParams::specs());
        specs.extend_from_slice(AeolianParams::specs());
        specs.extend_from_slice(CoastalParams::specs());
        specs.extend_from_slice(ChemicalParams::specs());
        debug_assert!(
            {
                let mut keys: Vec<&str> = specs.iter().map(|s| s.key).collect();
                keys.sort_unstable();
                let n = keys.len();
                keys.dedup();
                keys.len() == n
            },
            "duplicate parameter keys in schema"
        );
        specs
    }

    /// Flat value encoding, in [`Self::schema`] order.
    #[must_use]
    pub fn values(&self) -> Vec<f64> {
        let mut out = Vec::with_capacity(96);
        self.world.values_into(&mut out);
        self.time.values_into(&mut out);
        self.relief.values_into(&mut out);
        self.tectonics.values_into(&mut out);
        self.geology.values_into(&mut out);
        self.climate.values_into(&mut out);
        self.ecology.values_into(&mut out);
        self.hydrology.values_into(&mut out);
        self.fluvial.values_into(&mut out);
        self.hillslope.values_into(&mut out);
        self.glacial.values_into(&mut out);
        self.aeolian.values_into(&mut out);
        self.coastal.values_into(&mut out);
        self.chemical.values_into(&mut out);
        out
    }

    /// Reads a flat value encoding produced by [`Self::values`].
    ///
    /// The input must have exactly as many entries as the schema; extra values
    /// are rejected so a stale frontend cannot silently mis-assign parameters.
    pub fn set_values(&mut self, values: &[f64]) -> GeoResult<()> {
        let mut offset = 0usize;
        macro_rules! block {
            ($field:ident, $ty:ty) => {{
                let n = <$ty as ParamBlock>::specs().len();
                let slice = values.get(offset..offset + n).ok_or_else(|| {
                    GeoError::InvalidConfig(format!(
                        "expected {} parameters, got {}",
                        Self::schema().len(),
                        values.len()
                    ))
                })?;
                self.$field.set_values(slice);
                offset += n;
            }};
        }
        block!(world, WorldParams);
        block!(time, TimeParams);
        block!(relief, ReliefParams);
        block!(tectonics, TectonicParams);
        block!(geology, GeologyParams);
        block!(climate, ClimateParams);
        block!(ecology, EcologyParams);
        block!(hydrology, HydrologyParams);
        block!(fluvial, FluvialParams);
        block!(hillslope, HillslopeParams);
        block!(glacial, GlacialParams);
        block!(aeolian, AeolianParams);
        block!(coastal, CoastalParams);
        block!(chemical, ChemicalParams);
        if offset != values.len() {
            return Err(GeoError::InvalidConfig(format!(
                "expected {} parameters, got {}",
                offset,
                values.len()
            )));
        }
        Ok(())
    }

    /// Builds parameters from a flat encoding, normalising and validating.
    pub fn from_values(values: &[f64]) -> GeoResult<Self> {
        let mut params = Self::default();
        params.set_values(values)?;
        params.normalize();
        params.validate()?;
        Ok(params)
    }

    /// Looks up a spec by key.
    #[must_use]
    pub fn spec(key: &str) -> Option<ParamSpec> {
        Self::schema().into_iter().find(|s| s.key == key)
    }

    /// Sets one parameter by key, then re-normalises. Unknown keys are errors.
    pub fn set_key(&mut self, key: &str, value: f64) -> GeoResult<()> {
        let spec = Self::spec(key)
            .ok_or_else(|| GeoError::NotFound(format!("unknown parameter `{key}`")))?;
        if !self.set_key_raw(key, spec.sanitize(value)) {
            return Err(GeoError::NotFound(format!("unhandled parameter `{key}`")));
        }
        self.normalize();
        Ok(())
    }

    /// Sets one parameter without normalising; returns false for unknown keys.
    fn set_key_raw(&mut self, key: &str, value: f64) -> bool {
        if self.world.set_key(key, value) {
            return true;
        }
        if self.time.set_key(key, value) {
            return true;
        }
        if self.relief.set_key(key, value) {
            return true;
        }
        if self.tectonics.set_key(key, value) {
            return true;
        }
        if self.geology.set_key(key, value) {
            return true;
        }
        if self.climate.set_key(key, value) {
            return true;
        }
        if self.ecology.set_key(key, value) {
            return true;
        }
        if self.hydrology.set_key(key, value) {
            return true;
        }
        if self.fluvial.set_key(key, value) {
            return true;
        }
        if self.hillslope.set_key(key, value) {
            return true;
        }
        if self.glacial.set_key(key, value) {
            return true;
        }
        if self.aeolian.set_key(key, value) {
            return true;
        }
        if self.coastal.set_key(key, value) {
            return true;
        }
        self.chemical.set_key(key, value)
    }

    /// Reads one parameter by key.
    #[must_use]
    pub fn get_key(&self, key: &str) -> Option<f64> {
        self.world
            .get_key(key)
            .or_else(|| self.time.get_key(key))
            .or_else(|| self.relief.get_key(key))
            .or_else(|| self.tectonics.get_key(key))
            .or_else(|| self.geology.get_key(key))
            .or_else(|| self.climate.get_key(key))
            .or_else(|| self.ecology.get_key(key))
            .or_else(|| self.hydrology.get_key(key))
            .or_else(|| self.fluvial.get_key(key))
            .or_else(|| self.hillslope.get_key(key))
            .or_else(|| self.glacial.get_key(key))
            .or_else(|| self.aeolian.get_key(key))
            .or_else(|| self.coastal.get_key(key))
            .or_else(|| self.chemical.get_key(key))
    }

    /// Applies a list of preset overrides, sanitising values and normalising
    /// once at the end (order-independent).
    pub fn apply_overrides(&mut self, overrides: &[(&str, f64)]) -> GeoResult<()> {
        for (key, value) in overrides {
            let spec = Self::spec(key)
                .ok_or_else(|| GeoError::NotFound(format!("preset key `{key}` is unknown")))?;
            if !self.set_key_raw(key, spec.sanitize(*value)) {
                return Err(GeoError::InvalidConfig(format!(
                    "preset key `{key}` is not handled"
                )));
            }
        }
        self.normalize();
        Ok(())
    }

    /// Applies a named preset from [`PRESETS`].
    pub fn apply_preset(&mut self, key: &str) -> GeoResult<()> {
        let preset = presets::find(key)
            .ok_or_else(|| GeoError::NotFound(format!("preset `{key}` is unknown")))?;
        self.apply_overrides(preset.overrides)
    }

    /// A fully random but internally coherent parameter set.
    #[must_use]
    pub fn random(seed: u64) -> Self {
        let mut rng = Rng::new(crate::math::rng::mix64(seed | 1));
        let mut p = Self::default();
        p.world.randomize(&mut rng);
        p.time.randomize(&mut rng);
        p.relief.randomize(&mut rng);
        p.tectonics.randomize(&mut rng);
        p.geology.randomize(&mut rng);
        p.climate.randomize(&mut rng);
        p.ecology.randomize(&mut rng);
        p.hydrology.randomize(&mut rng);
        p.fluvial.randomize(&mut rng);
        p.hillslope.randomize(&mut rng);
        p.glacial.randomize(&mut rng);
        p.aeolian.randomize(&mut rng);
        p.coastal.randomize(&mut rng);
        p.chemical.randomize(&mut rng);
        p.world.seed = seed;
        p.normalize();
        p
    }

    /// Clamps every block, then repairs cross-block consistency.
    pub fn normalize(&mut self) {
        self.world.normalize();
        self.time.normalize();
        self.relief.normalize();
        self.tectonics.normalize();
        self.geology.normalize();
        self.climate.normalize();
        self.ecology.normalize();
        self.hydrology.normalize();
        self.fluvial.normalize();
        self.hillslope.normalize();
        self.glacial.normalize();
        self.aeolian.normalize();
        self.coastal.normalize();
        self.chemical.normalize();

        // The datum must sit inside the relief envelope.
        let limit = self.world.vertical_m * 1.5;
        self.world.sea_level_m = self.world.sea_level_m.clamp(-limit, limit);
        // The ice model needs a plausible snowline band.
        self.glacial.ela_m = self.glacial.ela_m.clamp(0.0, 8000.0);
        // History cannot have more frames than steps.
        let steps = self.time.steps_needed();
        if u64::from(self.time.history_frames) > steps + 1 {
            self.time.history_frames = (steps + 1).clamp(2, 128) as u32;
        }
        // Keep the erosion law's coefficient meaningful for the chosen law.
        if self.fluvial.law.round() < 0.5 {
            // Detachment-limited: deposition must stay effectively off in the
            // solver, but keep the knob meaningful for switching back.
        }
    }

    /// Full validation: per-block ranges plus cross-block consistency.
    pub fn validate(&self) -> GeoResult<()> {
        self.world.validate()?;
        self.time.validate()?;
        self.relief.validate()?;
        self.tectonics.validate()?;
        self.geology.validate()?;
        self.climate.validate()?;
        self.ecology.validate()?;
        self.hydrology.validate()?;
        self.fluvial.validate()?;
        self.hillslope.validate()?;
        self.glacial.validate()?;
        self.aeolian.validate()?;
        self.coastal.validate()?;
        self.chemical.validate()?;

        if self.world.vertical_m < 100.0 {
            return Err(GeoError::param(
                "vertical_m",
                self.world.vertical_m,
                "must be at least 100 m",
            ));
        }
        if self.time.step_yr > self.time.total_yr {
            return Err(GeoError::InvalidConfig("step_yr exceeds total_yr".into()));
        }
        // A grid larger than the memory budget is rejected up front instead of
        // failing midway through allocation.
        let cells = self.world.width as u64 * u64::from(self.world.height);
        if !self.allow_large_grid && cells > 2048 * 2048 {
            return Err(GeoError::InvalidGrid(format!(
                "grid {}x{} exceeds the 2048x2048 limit",
                self.world.width, self.world.height
            )));
        }
        Ok(())
    }

    /// Iterations that will actually run.
    #[must_use]
    pub fn planned_steps(&self) -> u64 {
        self.time.steps_needed().min(u64::from(self.time.max_iters))
    }

    /// Schema as JSON, consumed by the Web UI.
    #[must_use]
    pub fn schema_json() -> String {
        let mut out = String::with_capacity(24_000);
        out.push_str("{\"version\":\"");
        out.push_str(env!("CARGO_PKG_VERSION"));
        out.push_str("\",\"groups\":[");
        for (i, group) in ParamGroup::ALL.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"key\":\"");
            out.push_str(group.key());
            out.push_str("\",\"label\":\"");
            push_json_escaped(&mut out, group.label());
            out.push_str("\",\"label_ja\":\"");
            push_json_escaped(&mut out, group.label_ja());
            out.push_str("\"}");
        }
        out.push_str("],\"params\":[");
        for (i, spec) in Self::schema().iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"key\":\"");
            push_json_escaped(&mut out, spec.key);
            out.push_str("\",\"label\":\"");
            push_json_escaped(&mut out, spec.label);
            out.push_str("\",\"label_ja\":\"");
            push_json_escaped(&mut out, spec.label_ja);
            out.push_str("\",\"group\":\"");
            out.push_str(spec.group.key());
            out.push_str("\",\"unit\":\"");
            push_json_escaped(&mut out, spec.unit);
            out.push_str("\",\"min\":");
            push_json_number(&mut out, spec.min);
            out.push_str(",\"max\":");
            push_json_number(&mut out, spec.max);
            out.push_str(",\"default\":");
            push_json_number(&mut out, spec.default);
            out.push_str(",\"scale\":\"");
            out.push_str(match spec.scale {
                ParamScale::Linear => "linear",
                ParamScale::Log => "log",
            });
            out.push_str("\",\"kind\":\"");
            out.push_str(match spec.kind {
                ParamKind::Float => "float",
                ParamKind::Int => "int",
                ParamKind::Enum(_) => "enum",
            });
            out.push('"');
            if let ParamKind::Enum(options) = spec.kind {
                out.push_str(",\"options\":[");
                for (j, option) in options.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    push_json_escaped(&mut out, option);
                    out.push('"');
                }
                out.push(']');
            }
            out.push_str(",\"help\":\"");
            push_json_escaped(&mut out, spec.help);
            out.push_str("\"}");
        }
        out.push_str("]}");
        out
    }

    /// Presets as JSON, consumed by the Web UI.
    #[must_use]
    pub fn presets_json() -> String {
        let mut out = String::with_capacity(6000);
        out.push_str("{\"presets\":[");
        for (i, preset) in PRESETS.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"key\":\"");
            push_json_escaped(&mut out, preset.key);
            out.push_str("\",\"label\":\"");
            push_json_escaped(&mut out, preset.label);
            out.push_str("\",\"label_ja\":\"");
            push_json_escaped(&mut out, preset.label_ja);
            out.push_str("\",\"description\":\"");
            push_json_escaped(&mut out, preset.description);
            out.push_str("\",\"overrides\":{");
            for (j, (key, value)) in preset.overrides.iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                out.push('"');
                push_json_escaped(&mut out, key);
                out.push_str("\":");
                push_json_number(&mut out, *value);
            }
            out.push_str("}}");
        }
        out.push_str("]}");
        out
    }
}

/// Appends a JSON-escaped string (without surrounding quotes).
/// Appends a JSON-escaped string (without surrounding quotes) to `out`.
pub fn push_json_escaped(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}

/// Appends a finite JSON number; non-finite values become `null`.
/// Appends a finite JSON number to `out`; non-finite values become `null`.
pub fn push_json_number(out: &mut String, value: f64) {
    if value.is_finite() {
        if value == value.trunc() && value.abs() < 1e15 {
            out.push_str(&format!("{}", value as i64));
        } else {
            out.push_str(&format!("{value:.10e}"));
        }
    } else {
        out.push_str("null");
    }
}

/// A deterministic seed from a human-readable string (FNV-1a + mix).
#[must_use]
pub fn seed_from_str(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    crate::math::rng::mix64(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params_are_valid() {
        let p = Params::default();
        p.validate().expect("defaults must validate");
        assert!(p.planned_steps() > 0);
    }

    #[test]
    fn schema_and_values_have_matching_length() {
        let p = Params::default();
        assert_eq!(Params::schema().len(), p.values().len());
        assert!(Params::schema().len() > 80, "schema is suspiciously small");
    }

    #[test]
    fn schema_keys_are_unique() {
        let specs = Params::schema();
        let mut keys: Vec<&str> = specs.iter().map(|s| s.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), specs.len());
    }

    #[test]
    fn values_roundtrip() {
        let p = Params::default();
        let v = p.values();
        let mut q = Params::default();
        q.set_values(&v).unwrap();
        assert_eq!(p, q);
        let r = Params::from_values(&v).unwrap();
        assert_eq!(p, r);
    }

    #[test]
    fn malformed_value_vectors_are_rejected() {
        let p = Params::default();
        let mut v = p.values();
        v.pop();
        assert!(Params::from_values(&v).is_err());
        v.push(1.0);
        v.push(1.0);
        assert!(Params::from_values(&v).is_err());
    }

    #[test]
    fn set_and_get_key_roundtrip() {
        let mut p = Params::default();
        for spec in Params::schema() {
            // Pick a value inside range, distinct from the default.
            let target = if spec.max > spec.min {
                spec.min + (spec.max - spec.min) * 0.37
            } else {
                spec.default
            };
            p.set_key(spec.key, target).unwrap();
            let got = p.get_key(spec.key).unwrap();
            assert!(
                (got - target).abs() <= (spec.max - spec.min).abs() * 1e-9 + 1e-12
                    || spec.is_integer(),
                "{}: set {} got {}",
                spec.key,
                target,
                got
            );
        }
    }

    #[test]
    fn unknown_keys_are_errors() {
        let mut p = Params::default();
        assert!(p.set_key("not_a_param", 1.0).is_err());
        assert_eq!(p.get_key("not_a_param"), None);
    }

    #[test]
    fn schema_json_is_structurally_sane() {
        let json = Params::schema_json();
        assert!(json.starts_with('{'));
        assert!(json.ends_with('}'));
        assert!(json.contains("\"params\":["));
        assert!(json.contains("\"key\":\"kf\""));
        assert!(json.contains("\"label_ja\":\"侵食係数 Kf\""));
        // Balanced braces and brackets is a cheap structural check.
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escaped = false;
        for ch in json.chars() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0);
        }
        assert_eq!(depth, 0);
        assert!(!in_string);
    }

    #[test]
    fn presets_json_mentions_every_preset() {
        let json = Params::presets_json();
        for preset in PRESETS {
            assert!(json.contains(preset.key), "missing {}", preset.key);
        }
    }

    #[test]
    fn random_params_validate_across_many_seeds() {
        for seed in 0..64u64 {
            let p = Params::random(seed);
            p.validate().unwrap_or_else(|e| panic!("seed {seed}: {e}"));
            assert_eq!(p.world.seed, seed);
            assert!(p.planned_steps() >= 1);
            assert!(p.time.steps_needed() <= u64::from(p.time.max_iters));
        }
    }

    #[test]
    fn every_preset_applies_and_validates() {
        for preset in PRESETS {
            let mut p = Params::default();
            p.apply_preset(preset.key)
                .unwrap_or_else(|e| panic!("preset {}: {e}", preset.key));
            p.validate()
                .unwrap_or_else(|e| panic!("preset {}: {e}", preset.key));
        }
    }

    #[test]
    fn seed_from_str_is_stable() {
        assert_eq!(seed_from_str("geogenesis"), seed_from_str("geogenesis"));
        assert_ne!(seed_from_str("a"), seed_from_str("b"));
    }

    #[test]
    fn large_grid_requires_explicit_opt_in() {
        let mut params = Params::default();
        params.world.width = 4096;
        params.world.height = 4096;
        params.normalize();
        // Safe default: the single-shot path is memory-guarded.
        assert!(params.validate().is_err());
        params.allow_large_grid = true;
        assert!(params.validate().is_ok());
        // The guard is independent of the opt-in flag for sane sizes.
        params.world.width = 384;
        params.world.height = 384;
        assert!(params.validate().is_ok());
    }
}
