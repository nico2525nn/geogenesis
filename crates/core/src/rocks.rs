//! User-defined rocks and stratigraphy configuration (SPEC §4b/§4c).
//!
//! The built-in catalogue in [`crate::geology::ROCKS`] is fixed at compile
//! time. This module adds the user layer on top of it:
//!
//! * [`CustomRock`] — a fully specified rock (name, colour, hardness,
//!   permeability, friction angle, cohesion, density, solubility, weathering
//!   rate and origin) validated against the SPEC §4a ranges;
//! * [`RockSet`] — the 14 built-in rocks followed by any custom rocks. Rock ids
//!   are dense `u8` indices into the set, so [`crate::geology::Geology`] keeps
//!   exposing `rock_id` exactly as before; built-in ids are unchanged
//!   (`0..14`), custom rocks are appended (`14..`);
//! * [`StrataSpec`] — a user-defined stratigraphic stack (1–100 layers) with
//!   thickness, rock reference, dip, strike, fault offset and unconformity per
//!   layer, convertible into a [`crate::geology::Stratigraphy`];
//! * dependency-free JSON load/save for both, built on a small self-contained
//!   parser (the crate has no serialisation dependencies).
//!
//! # JSON formats
//!
//! A custom rock is a JSON object:
//!
//! ```json
//! {
//!   "name": "Siltite",
//!   "colour": [184, 168, 140],
//!   "mohs": 2.4,
//!   "permeability_m2": 1e-14,
//!   "friction_angle_deg": 30.0,
//!   "cohesion_mpa": 2.0,
//!   "density_kg_m3": 2350.0,
//!   "solubility": 0.2,
//!   "weathering_rate_m_yr": 0.0002,
//!   "origin": "sedimentary"
//! }
//! ```
//!
//! A rock set is `{"version":1,"custom_rocks":[...]}` (a bare array of rock
//! objects is also accepted); a strata spec is
//! `{"version":1,"layers":[...]}` (bare array accepted as well). Each layer is
//! an object such as
//!
//! ```json
//! {"thickness_m":120.0,"rock":"granite","dip_deg":8.0,"strike_deg":0.0,
//!  "fault_offset_m":0.0,"unconformity":false}
//! ```
//!
//! where `rock` may be a built-in key (`"granite"`), a custom rock name
//! (`"Siltite"`), or an explicit numeric rock id. Dip, strike, fault offset and
//! unconformity are optional and default to zero/false. Serialisation writes
//! the shortest round-tripping decimal representation, so a `to_json` /
//! `from_json` cycle preserves every value bit-exactly.

use crate::error::{GeoError, GeoResult};
use crate::geology::{Rock, RockOrigin, Stratigraphy, Stratum, ROCKS};

/// Number of built-in rocks in [`ROCKS`].
pub const BUILTIN_ROCK_COUNT: usize = ROCKS.len();

/// Maximum number of rock units a [`RockSet`] can hold (rock ids are `u8`).
pub const MAX_ROCK_COUNT: usize = u8::MAX as usize + 1;

/// Minimum layer count of a [`StrataSpec`] (SPEC §4c).
pub const MIN_STRATA_LAYERS: usize = 1;
/// Maximum layer count of a [`StrataSpec`] (SPEC §4c).
pub const MAX_STRATA_LAYERS: usize = 100;
/// Minimum layer thickness, metres (SPEC §4c).
pub const MIN_LAYER_THICKNESS_M: f64 = 0.1;
/// Maximum layer thickness, metres (SPEC §4c).
pub const MAX_LAYER_THICKNESS_M: f64 = 100_000.0;
/// Maximum absolute fault displacement, metres (SPEC §4c).
pub const MAX_FAULT_OFFSET_M: f64 = 10_000.0;

/// Accepted hardness range (SPEC §4a).
const MOHS_RANGE: (f64, f64) = (1.0, 10.0);
/// Accepted permeability range, m². The lower bound matches the tightest
/// built-in catalogue entry; SPEC's 1e-12–1e-5 window is the UI slider range.
const PERMEABILITY_RANGE_M2: (f64, f64) = (1e-20, 1e-5);
/// Accepted friction-angle range, degrees.
const FRICTION_RANGE_DEG: (f64, f64) = (20.0, 60.0);
/// Accepted cohesion range, MPa.
const COHESION_RANGE_MPA: (f64, f64) = (0.0, 20.0);
/// Accepted density range, kg/m³.
const DENSITY_RANGE_KG_M3: (f64, f64) = (1_500.0, 3_500.0);
/// Accepted weathering-rate range, m/yr.
const WEATHERING_RANGE_M_YR: (f64, f64) = (1e-6, 1e-2);

// ---------------------------------------------------------------------------
// Minimal dependency-free JSON
// ---------------------------------------------------------------------------

/// A tiny JSON tree used by the rock/strata (de)serialisers.
///
/// The crate deliberately has no serialisation dependency, so this module
/// carries the smallest parser that is still correct for the formats above:
/// full string-escape support (including surrogate pairs), strict number
/// grammar, a nesting-depth limit and duplicate-key rejection.
mod json {
    use crate::error::{GeoError, GeoResult};

    /// Maximum nesting depth, so a hostile document cannot overflow the stack.
    const MAX_DEPTH: usize = 32;

    /// A parsed JSON value.
    #[derive(Clone, Debug, PartialEq)]
    pub(super) enum Value {
        Null,
        Bool(bool),
        Number(f64),
        String(String),
        Array(Vec<Value>),
        Object(Vec<(String, Value)>),
    }

    impl Value {
        /// Member of an object by key.
        pub(super) fn get(&self, key: &str) -> Option<&Self> {
            match self {
                Self::Object(entries) => entries
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value),
                _ => None,
            }
        }

        /// String payload.
        pub(super) fn as_str(&self) -> Option<&str> {
            match self {
                Self::String(text) => Some(text),
                _ => None,
            }
        }

        /// Numeric payload.
        pub(super) fn as_f64(&self) -> Option<f64> {
            match self {
                Self::Number(value) => Some(*value),
                _ => None,
            }
        }

        /// Boolean payload.
        pub(super) fn as_bool(&self) -> Option<bool> {
            match self {
                Self::Bool(value) => Some(*value),
                _ => None,
            }
        }

        /// Array payload.
        pub(super) fn as_array(&self) -> Option<&[Self]> {
            match self {
                Self::Array(items) => Some(items),
                _ => None,
            }
        }
    }

    /// Error at a byte offset inside the document.
    fn error(offset: usize, reason: &str) -> GeoError {
        GeoError::Io(format!("invalid JSON at byte {offset}: {reason}"))
    }

    /// Parses a complete JSON document (surrounding whitespace allowed).
    pub(super) fn parse(text: &str) -> GeoResult<Value> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            position: 0,
        };
        parser.skip_whitespace();
        let value = parser.parse_value(0)?;
        parser.skip_whitespace();
        if parser.position != parser.bytes.len() {
            return Err(error(parser.position, "trailing characters"));
        }
        Ok(value)
    }

    struct Parser<'a> {
        bytes: &'a [u8],
        position: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> Option<u8> {
            self.bytes.get(self.position).copied()
        }

        fn next_byte(&mut self) -> Option<u8> {
            let byte = self.peek()?;
            self.position += 1;
            Some(byte)
        }

        fn expect(&mut self, expected: u8) -> GeoResult<()> {
            match self.next_byte() {
                Some(actual) if actual == expected => Ok(()),
                _ => Err(error(
                    self.position.saturating_sub(1),
                    "unexpected character",
                )),
            }
        }

        fn skip_whitespace(&mut self) {
            while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                self.position += 1;
            }
        }

        fn parse_value(&mut self, depth: usize) -> GeoResult<Value> {
            if depth > MAX_DEPTH {
                return Err(error(self.position, "nesting too deep"));
            }
            self.skip_whitespace();
            match self.peek() {
                Some(b'{') => self.parse_object(depth),
                Some(b'[') => self.parse_array(depth),
                Some(b'"') => self.parse_string().map(Value::String),
                Some(b't') => self.parse_literal("true", Value::Bool(true)),
                Some(b'f') => self.parse_literal("false", Value::Bool(false)),
                Some(b'n') => self.parse_literal("null", Value::Null),
                Some(b'-' | b'0'..=b'9') => self.parse_number(),
                _ => Err(error(self.position, "expected a value")),
            }
        }

        fn parse_literal(&mut self, literal: &str, value: Value) -> GeoResult<Value> {
            let end = self.position + literal.len();
            if end > self.bytes.len() || &self.bytes[self.position..end] != literal.as_bytes() {
                return Err(error(self.position, "invalid literal"));
            }
            self.position = end;
            Ok(value)
        }

        fn parse_object(&mut self, depth: usize) -> GeoResult<Value> {
            self.expect(b'{')?;
            let mut entries: Vec<(String, Value)> = Vec::new();
            self.skip_whitespace();
            if self.peek() == Some(b'}') {
                self.position += 1;
                return Ok(Value::Object(entries));
            }
            loop {
                self.skip_whitespace();
                let key = self.parse_string()?;
                if entries.iter().any(|(existing, _)| *existing == key) {
                    return Err(error(self.position, "duplicate object key"));
                }
                self.skip_whitespace();
                self.expect(b':')?;
                let value = self.parse_value(depth + 1)?;
                entries.push((key, value));
                self.skip_whitespace();
                match self.next_byte() {
                    Some(b',') => continue,
                    Some(b'}') => return Ok(Value::Object(entries)),
                    _ => {
                        return Err(error(
                            self.position.saturating_sub(1),
                            "expected `,` or `}`",
                        ))
                    }
                }
            }
        }

        fn parse_array(&mut self, depth: usize) -> GeoResult<Value> {
            self.expect(b'[')?;
            let mut items = Vec::new();
            self.skip_whitespace();
            if self.peek() == Some(b']') {
                self.position += 1;
                return Ok(Value::Array(items));
            }
            loop {
                items.push(self.parse_value(depth + 1)?);
                self.skip_whitespace();
                match self.next_byte() {
                    Some(b',') => continue,
                    Some(b']') => return Ok(Value::Array(items)),
                    _ => {
                        return Err(error(
                            self.position.saturating_sub(1),
                            "expected `,` or `]`",
                        ))
                    }
                }
            }
        }

        fn parse_string(&mut self) -> GeoResult<String> {
            self.expect(b'"')?;
            let mut out = String::new();
            loop {
                let Some(byte) = self.next_byte() else {
                    return Err(error(self.position, "unterminated string"));
                };
                match byte {
                    b'"' => return Ok(out),
                    b'\\' => self.parse_escape(&mut out)?,
                    0x00..=0x1F => return Err(error(self.position, "control character in string")),
                    0x20..=0x7F => out.push(char::from(byte)),
                    _ => {
                        // Multi-byte UTF-8: the lead byte gives the width.
                        let start = self.position - 1;
                        let width = match byte {
                            0xC2..=0xDF => 2,
                            0xE0..=0xEF => 3,
                            0xF0..=0xF4 => 4,
                            _ => return Err(error(start, "invalid UTF-8 in string")),
                        };
                        let end = start + width;
                        if end > self.bytes.len() {
                            return Err(error(start, "truncated UTF-8 sequence"));
                        }
                        let text = core::str::from_utf8(&self.bytes[start..end])
                            .map_err(|_| error(start, "invalid UTF-8 in string"))?;
                        out.push_str(text);
                        self.position = end;
                    }
                }
            }
        }

        fn parse_escape(&mut self, out: &mut String) -> GeoResult<()> {
            let Some(escape) = self.next_byte() else {
                return Err(error(self.position, "unterminated escape"));
            };
            let decoded = match escape {
                b'"' => '"',
                b'\\' => '\\',
                b'/' => '/',
                b'b' => '\u{8}',
                b'f' => '\u{c}',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'u' => return self.parse_unicode_escape(out),
                _ => return Err(error(self.position.saturating_sub(1), "invalid escape")),
            };
            out.push(decoded);
            Ok(())
        }

        fn parse_unicode_escape(&mut self, out: &mut String) -> GeoResult<()> {
            let first = self.parse_hex4()?;
            let code = if (0xD800..=0xDBFF).contains(&first) {
                // High surrogate: a low surrogate must follow as `\uXXXX`.
                self.expect(b'\\')?;
                self.expect(b'u')?;
                let second = self.parse_hex4()?;
                if !(0xDC00..=0xDFFF).contains(&second) {
                    return Err(error(self.position, "invalid surrogate pair"));
                }
                0x1_0000 + ((first - 0xD800) << 10) + (second - 0xDC00)
            } else if (0xDC00..=0xDFFF).contains(&first) {
                return Err(error(self.position, "lone low surrogate"));
            } else {
                first
            };
            let character = char::from_u32(code)
                .ok_or_else(|| error(self.position, "invalid Unicode escape"))?;
            out.push(character);
            Ok(())
        }

        fn parse_hex4(&mut self) -> GeoResult<u32> {
            let mut value = 0u32;
            for _ in 0..4 {
                let Some(byte) = self.next_byte() else {
                    return Err(error(self.position, "truncated Unicode escape"));
                };
                let digit = match byte {
                    b'0'..=b'9' => u32::from(byte - b'0'),
                    b'a'..=b'f' => u32::from(byte - b'a') + 10,
                    b'A'..=b'F' => u32::from(byte - b'A') + 10,
                    _ => return Err(error(self.position.saturating_sub(1), "invalid hex digit")),
                };
                value = value * 16 + digit;
            }
            Ok(value)
        }

        fn parse_number(&mut self) -> GeoResult<Value> {
            let start = self.position;
            if self.peek() == Some(b'-') {
                self.position += 1;
            }
            match self.peek() {
                Some(b'0') => self.position += 1,
                Some(b'1'..=b'9') => {
                    while matches!(self.peek(), Some(b'0'..=b'9')) {
                        self.position += 1;
                    }
                }
                _ => return Err(error(self.position, "invalid number")),
            }
            if self.peek() == Some(b'.') {
                self.position += 1;
                if !matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(error(self.position, "invalid fraction"));
                }
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            if matches!(self.peek(), Some(b'e' | b'E')) {
                self.position += 1;
                if matches!(self.peek(), Some(b'+' | b'-')) {
                    self.position += 1;
                }
                if !matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(error(self.position, "invalid exponent"));
                }
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            let token = core::str::from_utf8(&self.bytes[start..self.position])
                .map_err(|_| error(start, "invalid number"))?;
            let value: f64 = token
                .parse()
                .map_err(|_| error(start, "number out of range"))?;
            if !value.is_finite() {
                return Err(error(start, "number out of range"));
            }
            Ok(Value::Number(value))
        }
    }

    /// Appends `text` as a JSON string literal.
    pub(super) fn write_string(out: &mut String, text: &str) {
        out.push('"');
        crate::params::push_json_escaped(out, text);
        out.push('"');
    }

    /// Appends a finite `f64`; Rust's shortest round-trip formatting keeps the
    /// value exact through a parse/serialise cycle.
    pub(super) fn write_number(out: &mut String, value: f64) -> GeoResult<()> {
        if !value.is_finite() {
            return Err(GeoError::InvalidConfig(
                "cannot serialise a non-finite number".into(),
            ));
        }
        out.push_str(&value.to_string());
        Ok(())
    }

    /// Appends a newline and `indent` two-space levels (pretty output).
    pub(super) fn newline(out: &mut String, indent: usize) {
        out.push('\n');
        for _ in 0..indent {
            out.push_str("  ");
        }
    }
}

// ---------------------------------------------------------------------------
// Origin and derived physical properties
// ---------------------------------------------------------------------------

/// JSON key of a rock origin.
#[must_use]
pub const fn origin_key(origin: RockOrigin) -> &'static str {
    match origin {
        RockOrigin::Igneous => "igneous",
        RockOrigin::Sedimentary => "sedimentary",
        RockOrigin::Metamorphic => "metamorphic",
    }
}

/// Parses an origin key (`igneous`, `sedimentary`, `metamorphic`).
///
/// # Errors
///
/// Unknown origin names are rejected.
pub fn parse_origin(key: &str) -> GeoResult<RockOrigin> {
    match key.trim().to_ascii_lowercase().as_str() {
        "igneous" => Ok(RockOrigin::Igneous),
        "sedimentary" => Ok(RockOrigin::Sedimentary),
        "metamorphic" => Ok(RockOrigin::Metamorphic),
        other => Err(GeoError::InvalidConfig(format!(
            "unknown rock origin `{other}`; expected igneous, sedimentary or metamorphic"
        ))),
    }
}

/// Erodibility implied by hardness, used for user-defined rocks.
///
/// Calibrated against the built-in catalogue (`erodibility ≈ 2.9 · Mohs^-0.9`)
/// with a small origin factor: sedimentary rocks are more friable, metamorphic
/// rocks slightly more resistant. The result stays inside the range the
/// catalogue spans, so mixing custom and built-in rocks keeps the erodibility
/// contrast physical.
#[must_use]
pub fn derived_erodibility(mohs: f64, origin: RockOrigin) -> f64 {
    let hardness = mohs.clamp(MOHS_RANGE.0, MOHS_RANGE.1);
    let origin_factor = match origin {
        RockOrigin::Igneous => 0.95,
        RockOrigin::Sedimentary => 1.12,
        RockOrigin::Metamorphic => 0.90,
    };
    (2.9 * hardness.powf(-0.9) * origin_factor).clamp(0.10, 5.0)
}

/// Friction angle implied by hardness and origin, degrees.
#[must_use]
pub fn derived_friction_angle_deg(mohs: f64, origin: RockOrigin) -> f64 {
    let offset = match origin {
        RockOrigin::Igneous => 3.0,
        RockOrigin::Sedimentary => 0.0,
        RockOrigin::Metamorphic => 1.5,
    };
    (28.0 + 1.6 * mohs.clamp(MOHS_RANGE.0, MOHS_RANGE.1) + offset)
        .clamp(FRICTION_RANGE_DEG.0, FRICTION_RANGE_DEG.1)
}

/// Cohesion implied by hardness and origin, MPa.
#[must_use]
pub fn derived_cohesion_mpa(mohs: f64, origin: RockOrigin) -> f64 {
    let factor = match origin {
        RockOrigin::Igneous => 1.10,
        RockOrigin::Sedimentary => 0.80,
        RockOrigin::Metamorphic => 1.20,
    };
    let hardness = mohs.clamp(MOHS_RANGE.0, MOHS_RANGE.1) / 10.0;
    (hardness * hardness * 30.0 * factor).clamp(COHESION_RANGE_MPA.0, COHESION_RANGE_MPA.1)
}

// ---------------------------------------------------------------------------
// CustomRock
// ---------------------------------------------------------------------------

/// A user-defined rock (SPEC §4b).
///
/// All properties are validated against the SPEC §4a ranges by
/// [`Self::validate`]; a rock must be registered in a [`RockSet`] before the
/// geology can expose it.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomRock {
    /// Display name; unique within a [`RockSet`] (case-insensitive).
    pub name: String,
    /// Display colour, RGB.
    pub colour: [u8; 3],
    /// Mohs hardness, 1–10.
    pub mohs: f64,
    /// Permeability, m².
    pub permeability_m2: f64,
    /// Internal friction angle, degrees (20–60).
    pub friction_angle_deg: f64,
    /// Cohesion, MPa (0–20).
    pub cohesion_mpa: f64,
    /// Bulk density, kg/m³ (1500–3500).
    pub density_kg_m3: f64,
    /// Chemical solubility, 0–1.
    pub solubility: f64,
    /// Physical weathering rate, m/yr (1e-6–1e-2).
    pub weathering_rate_m_yr: f64,
    /// Petrological origin class.
    pub origin: RockOrigin,
}

impl Default for CustomRock {
    fn default() -> Self {
        Self {
            name: String::new(),
            colour: [128, 128, 128],
            mohs: 5.0,
            permeability_m2: 1e-15,
            friction_angle_deg: 35.0,
            cohesion_mpa: 5.0,
            density_kg_m3: 2_500.0,
            solubility: 0.15,
            weathering_rate_m_yr: 1e-4,
            origin: RockOrigin::Sedimentary,
        }
    }
}

impl CustomRock {
    /// A named rock with the default physical properties.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// The stable key used in serialised references (the trimmed name).
    #[must_use]
    pub fn key(&self) -> &str {
        self.name.trim()
    }

    /// Validates every property against the SPEC §4a ranges.
    ///
    /// # Errors
    ///
    /// Returns [`GeoError::InvalidConfig`] naming the first offending field.
    pub fn validate(&self) -> GeoResult<()> {
        let name = self.key();
        if name.is_empty() {
            return Err(GeoError::InvalidConfig(
                "custom rock name must not be empty".into(),
            ));
        }
        if name.chars().count() > 64 {
            return Err(GeoError::InvalidConfig(
                "custom rock name must be at most 64 characters".into(),
            ));
        }
        if name.chars().any(char::is_control) {
            return Err(GeoError::InvalidConfig(
                "custom rock name must not contain control characters".into(),
            ));
        }
        check_range("mohs", self.mohs, MOHS_RANGE, name)?;
        check_range(
            "permeability_m2",
            self.permeability_m2,
            PERMEABILITY_RANGE_M2,
            name,
        )?;
        check_range(
            "friction_angle_deg",
            self.friction_angle_deg,
            FRICTION_RANGE_DEG,
            name,
        )?;
        check_range("cohesion_mpa", self.cohesion_mpa, COHESION_RANGE_MPA, name)?;
        check_range(
            "density_kg_m3",
            self.density_kg_m3,
            DENSITY_RANGE_KG_M3,
            name,
        )?;
        check_range("solubility", self.solubility, (0.0, 1.0), name)?;
        check_range(
            "weathering_rate_m_yr",
            self.weathering_rate_m_yr,
            WEATHERING_RANGE_M_YR,
            name,
        )?;
        Ok(())
    }

    /// Compact JSON object for this rock.
    ///
    /// # Errors
    ///
    /// Non-finite numeric fields cannot be represented in JSON.
    pub fn to_json(&self) -> GeoResult<String> {
        let mut out = String::with_capacity(320);
        self.write_json(&mut out, 0, false)?;
        Ok(out)
    }

    /// Parses one custom rock object.
    ///
    /// # Errors
    ///
    /// Malformed JSON, missing fields, unknown origin names and out-of-range
    /// values are rejected.
    pub fn from_json(text: &str) -> GeoResult<Self> {
        let value = json::parse(text)?;
        Self::from_value(&value)
    }

    fn from_value(value: &json::Value) -> GeoResult<Self> {
        let name = required_string(value, "name")?;
        let colour = match value.get("colour") {
            Some(item) => parse_colour(item)?,
            None => match value.get("color") {
                Some(item) => parse_colour(item)?,
                None => {
                    return Err(GeoError::InvalidConfig(format!(
                        "custom rock `{name}`: missing `colour`"
                    )))
                }
            },
        };
        let origin = match value.get("origin") {
            Some(item) => {
                let key = item.as_str().ok_or_else(|| {
                    GeoError::InvalidConfig(format!(
                        "custom rock `{name}`: `origin` must be a string"
                    ))
                })?;
                parse_origin(key)?
            }
            None => {
                return Err(GeoError::InvalidConfig(format!(
                    "custom rock `{name}`: missing `origin`"
                )))
            }
        };
        let rock = Self {
            name,
            colour,
            mohs: required_number(value, "mohs")?,
            permeability_m2: required_number(value, "permeability_m2")?,
            friction_angle_deg: required_number(value, "friction_angle_deg")?,
            cohesion_mpa: required_number(value, "cohesion_mpa")?,
            density_kg_m3: required_number(value, "density_kg_m3")?,
            solubility: required_number(value, "solubility")?,
            weathering_rate_m_yr: required_number(value, "weathering_rate_m_yr")?,
            origin,
        };
        rock.validate()?;
        Ok(rock)
    }

    fn write_json(&self, out: &mut String, indent: usize, pretty: bool) -> GeoResult<()> {
        out.push('{');
        let mut first = true;
        let field = |out: &mut String, key: &str, first: &mut bool| {
            if !*first {
                out.push(',');
            }
            if pretty {
                json::newline(out, indent + 1);
            }
            json::write_string(out, key);
            out.push(':');
            if pretty {
                out.push(' ');
            }
            *first = false;
        };
        field(out, "name", &mut first);
        json::write_string(out, &self.name);
        field(out, "colour", &mut first);
        out.push('[');
        for (index, channel) in self.colour.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            json::write_number(out, f64::from(*channel))?;
        }
        out.push(']');
        field(out, "mohs", &mut first);
        json::write_number(out, self.mohs)?;
        field(out, "permeability_m2", &mut first);
        json::write_number(out, self.permeability_m2)?;
        field(out, "friction_angle_deg", &mut first);
        json::write_number(out, self.friction_angle_deg)?;
        field(out, "cohesion_mpa", &mut first);
        json::write_number(out, self.cohesion_mpa)?;
        field(out, "density_kg_m3", &mut first);
        json::write_number(out, self.density_kg_m3)?;
        field(out, "solubility", &mut first);
        json::write_number(out, self.solubility)?;
        field(out, "weathering_rate_m_yr", &mut first);
        json::write_number(out, self.weathering_rate_m_yr)?;
        field(out, "origin", &mut first);
        json::write_string(out, origin_key(self.origin));
        if pretty {
            json::newline(out, indent);
        }
        out.push('}');
        Ok(())
    }

    /// Converts to the dense rock unit stored in a [`RockSet`].
    fn to_unit(&self, id: u8) -> RockUnit {
        RockUnit {
            id,
            key: self.key().to_string(),
            name: self.key().to_string(),
            name_ja: self.key().to_string(),
            colour: self.colour,
            mohs: self.mohs,
            permeability_m2: self.permeability_m2,
            friction_angle_deg: self.friction_angle_deg,
            cohesion_mpa: self.cohesion_mpa,
            density_kg_m3: self.density_kg_m3,
            solubility: self.solubility,
            weathering_rate_m_yr: self.weathering_rate_m_yr,
            origin: self.origin,
            erodibility: derived_erodibility(self.mohs, self.origin),
            builtin: false,
        }
    }
}

fn required_string(value: &json::Value, key: &str) -> GeoResult<String> {
    match value.get(key) {
        Some(item) => item
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| GeoError::InvalidConfig(format!("field `{key}` must be a string"))),
        None => Err(GeoError::InvalidConfig(format!("missing field `{key}`"))),
    }
}

fn required_number(value: &json::Value, key: &str) -> GeoResult<f64> {
    match value.get(key) {
        Some(item) => item
            .as_f64()
            .ok_or_else(|| GeoError::InvalidConfig(format!("field `{key}` must be a number"))),
        None => Err(GeoError::InvalidConfig(format!("missing field `{key}`"))),
    }
}

fn optional_number(value: &json::Value, key: &str) -> GeoResult<Option<f64>> {
    match value.get(key) {
        Some(item) => item
            .as_f64()
            .map(Some)
            .ok_or_else(|| GeoError::InvalidConfig(format!("field `{key}` must be a number"))),
        None => Ok(None),
    }
}

fn parse_colour(value: &json::Value) -> GeoResult<[u8; 3]> {
    let items = value
        .as_array()
        .ok_or_else(|| GeoError::InvalidConfig("`colour` must be an array of 3 numbers".into()))?;
    if items.len() != 3 {
        return Err(GeoError::InvalidConfig(format!(
            "`colour` needs 3 channels, got {}",
            items.len()
        )));
    }
    let mut colour = [0u8; 3];
    for (slot, item) in colour.iter_mut().zip(items) {
        let channel = item
            .as_f64()
            .ok_or_else(|| GeoError::InvalidConfig("`colour` channels must be numbers".into()))?;
        if !(0.0..=255.0).contains(&channel) {
            return Err(GeoError::InvalidConfig(
                "`colour` channels must be in 0..=255".into(),
            ));
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            *slot = channel.round() as u8;
        }
    }
    Ok(colour)
}

fn check_range(field: &str, value: f64, range: (f64, f64), owner: &str) -> GeoResult<()> {
    let (min, max) = range;
    if !value.is_finite() || !(min..=max).contains(&value) {
        return Err(GeoError::InvalidConfig(format!(
            "{owner}: `{field}` = {value} outside {min}..={max}"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// RockUnit / RockSet
// ---------------------------------------------------------------------------

/// A resolved rock unit: a built-in catalogue entry plus the physical
/// properties the processes consume.
///
/// Built-in units mirror [`crate::geology::Rock`] (with the derived friction
/// angle, cohesion and linear permeability filled in); custom units come from a
/// validated [`CustomRock`]. `id` is the dense index stored in
/// [`crate::geology::Geology::rock_id`].
#[derive(Clone, Debug, PartialEq)]
pub struct RockUnit {
    /// Dense rock id (`0..`); built-ins keep their catalogue index.
    pub id: u8,
    /// Stable key: the catalogue key for built-ins, the trimmed name for
    /// custom rocks.
    pub key: String,
    /// Display name.
    pub name: String,
    /// Japanese display name (falls back to the Latin name for custom rocks).
    pub name_ja: String,
    /// Display colour, RGB.
    pub colour: [u8; 3],
    /// Mohs hardness, 1–10.
    pub mohs: f64,
    /// Permeability, m².
    pub permeability_m2: f64,
    /// Internal friction angle, degrees.
    pub friction_angle_deg: f64,
    /// Cohesion, MPa.
    pub cohesion_mpa: f64,
    /// Bulk density, kg/m³.
    pub density_kg_m3: f64,
    /// Chemical solubility, 0–1.
    pub solubility: f64,
    /// Physical weathering rate, m/yr.
    pub weathering_rate_m_yr: f64,
    /// Petrological origin.
    pub origin: RockOrigin,
    /// Relative detachability (1 = reference); the erosion modules consume it.
    pub erodibility: f64,
    /// True for one of the 14 catalogue rocks.
    pub builtin: bool,
}

impl From<&Rock> for RockUnit {
    fn from(rock: &Rock) -> Self {
        Self {
            id: 0,
            key: rock.key.to_string(),
            name: rock.name.to_string(),
            name_ja: rock.name_ja.to_string(),
            colour: rock.colour,
            mohs: rock.mohs,
            permeability_m2: 10f64.powf(rock.permeability_log_m2),
            friction_angle_deg: derived_friction_angle_deg(rock.mohs, rock.origin),
            cohesion_mpa: derived_cohesion_mpa(rock.mohs, rock.origin),
            density_kg_m3: rock.density_kg_m3,
            solubility: rock.solubility,
            weathering_rate_m_yr: rock.weathering_rate_m_yr,
            origin: rock.origin,
            erodibility: rock.erodibility,
            builtin: true,
        }
    }
}

/// The rock catalogue available to a world: the 14 built-ins followed by any
/// user-defined rocks (SPEC §4b).
///
/// Rock ids are dense `u8` indices into the set, so
/// [`crate::geology::Geology::rock_id`] keeps working unchanged; registering a
/// custom rock only appends a new id and never renumbers built-ins. That keeps
/// worlds interchangeable and the display ABI stable.
#[derive(Clone, Debug, PartialEq)]
pub struct RockSet {
    units: Vec<RockUnit>,
}

impl Default for RockSet {
    fn default() -> Self {
        Self::builtin()
    }
}

impl RockSet {
    /// The 14 built-in rocks, without customs.
    #[must_use]
    pub fn builtin() -> Self {
        let units = ROCKS
            .iter()
            .enumerate()
            .map(|(index, rock)| {
                let mut unit = RockUnit::from(rock);
                unit.id = index as u8;
                unit
            })
            .collect();
        Self { units }
    }

    /// Number of rock units (built-ins plus customs).
    #[must_use]
    pub fn len(&self) -> usize {
        self.units.len()
    }

    /// True when the set has no units (never true for a valid set).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// All units in id order.
    #[must_use]
    pub fn units(&self) -> &[RockUnit] {
        &self.units
    }

    /// Unit by dense id.
    #[must_use]
    pub fn get(&self, id: u8) -> Option<&RockUnit> {
        self.units.get(id as usize)
    }

    /// Unit by dense id, falling back to the first built-in for out-of-range
    /// ids (display and export paths must never panic).
    #[must_use]
    pub fn get_or_default(&self, id: u8) -> &RockUnit {
        self.get(id).unwrap_or(&self.units[0])
    }

    /// Number of user-defined rocks.
    #[must_use]
    pub fn custom_count(&self) -> usize {
        self.units.len().saturating_sub(BUILTIN_ROCK_COUNT)
    }

    /// Ids of the user-defined rocks, in registration order.
    #[must_use]
    pub fn custom_ids(&self) -> Vec<u8> {
        (BUILTIN_ROCK_COUNT..self.units.len())
            .map(|id| id as u8)
            .collect()
    }

    /// Resolves a built-in key or custom name to its dense id.
    #[must_use]
    pub fn id_of_key(&self, key: &str) -> Option<u8> {
        self.units
            .iter()
            .find(|unit| unit.key == key)
            .map(|unit| unit.id)
    }

    /// Validates and appends a custom rock, returning its new id.
    ///
    /// # Errors
    ///
    /// Rejects invalid properties, duplicate names (case-insensitive, including
    /// clashes with built-in names) and a full set (`u8` id space exhausted).
    pub fn push_custom(&mut self, rock: CustomRock) -> GeoResult<u8> {
        rock.validate()?;
        if self.units.len() >= MAX_ROCK_COUNT {
            return Err(GeoError::InvalidConfig(format!(
                "rock set is full ({MAX_ROCK_COUNT} units)"
            )));
        }
        let name = rock.key().to_ascii_lowercase();
        if let Some(existing) = self
            .units
            .iter()
            .find(|unit| unit.name.to_ascii_lowercase() == name)
        {
            return Err(GeoError::InvalidConfig(format!(
                "duplicate rock name `{}` (already used by `{}`)",
                rock.key(),
                existing.key
            )));
        }
        #[allow(clippy::cast_possible_truncation)]
        let id = self.units.len() as u8;
        self.units.push(rock.to_unit(id));
        Ok(id)
    }

    /// Appends every rock from an iterator, failing on the first invalid or
    /// duplicate entry.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::push_custom`] errors.
    pub fn extend_custom(&mut self, rocks: impl IntoIterator<Item = CustomRock>) -> GeoResult<()> {
        for rock in rocks {
            self.push_custom(rock)?;
        }
        Ok(())
    }

    /// Serialises the user-defined rocks as compact JSON.
    ///
    /// # Errors
    ///
    /// Only if a non-finite value slipped past validation.
    pub fn to_json(&self) -> GeoResult<String> {
        self.write_json(false)
    }

    /// Serialises the user-defined rocks as indented JSON.
    ///
    /// # Errors
    ///
    /// Only if a non-finite value slipped past validation.
    pub fn to_json_pretty(&self) -> GeoResult<String> {
        self.write_json(true)
    }

    fn write_json(&self, pretty: bool) -> GeoResult<String> {
        let mut out = String::with_capacity(64 + 280 * self.custom_count());
        out.push('{');
        if pretty {
            json::newline(&mut out, 1);
        }
        json::write_string(&mut out, "version");
        out.push(':');
        if pretty {
            out.push(' ');
        }
        out.push('1');
        out.push(',');
        if pretty {
            json::newline(&mut out, 1);
        }
        json::write_string(&mut out, "custom_rocks");
        out.push(':');
        if pretty {
            out.push(' ');
        }
        out.push('[');
        let customs = &self.units[BUILTIN_ROCK_COUNT..];
        for (index, unit) in customs.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            if pretty {
                json::newline(&mut out, 2);
            }
            let rock = unit_to_custom(unit);
            rock.write_json(&mut out, 2, pretty)?;
        }
        if pretty && !customs.is_empty() {
            json::newline(&mut out, 1);
        }
        out.push(']');
        if pretty {
            json::newline(&mut out, 0);
        }
        out.push('}');
        Ok(out)
    }

    /// Parses a rock set, starting from the built-in catalogue and appending the
    /// serialised custom rocks in order.
    ///
    /// Accepts `{"version":1,"custom_rocks":[...]}` or a bare array of custom
    /// rock objects.
    ///
    /// # Errors
    ///
    /// Malformed JSON, invalid rock definitions and duplicate names are
    /// rejected.
    pub fn from_json(text: &str) -> GeoResult<Self> {
        let value = json::parse(text)?;
        let entries = match &value {
            json::Value::Array(items) => items,
            json::Value::Object(_) => {
                if let Some(version) = value.get("version").and_then(json::Value::as_f64) {
                    if !(0.5..=1.5).contains(&version) {
                        return Err(GeoError::InvalidConfig(format!(
                            "unsupported rock-set JSON version {version}"
                        )));
                    }
                }
                value
                    .get("custom_rocks")
                    .or_else(|| value.get("rocks"))
                    .and_then(json::Value::as_array)
                    .ok_or_else(|| {
                        GeoError::InvalidConfig("rock-set JSON needs a `custom_rocks` array".into())
                    })?
            }
            _ => {
                return Err(GeoError::InvalidConfig(
                    "rock-set JSON must be an object or an array".into(),
                ))
            }
        };
        let mut set = Self::builtin();
        for entry in entries {
            // A full-catalogue document may list built-ins by reference
            // (`{"builtin":"granite"}`); those are already in the set, so
            // validate the reference and skip it.
            if let Some(item) = entry.get("builtin") {
                let key = item.as_str().ok_or_else(|| {
                    GeoError::InvalidConfig("`builtin` must be a rock key string".into())
                })?;
                if !set
                    .units()
                    .iter()
                    .any(|unit| unit.builtin && unit.key == key)
                {
                    return Err(GeoError::NotFound(format!(
                        "built-in rock `{key}` does not exist"
                    )));
                }
                continue;
            }
            let rock = CustomRock::from_value(entry)?;
            set.push_custom(rock)?;
        }
        Ok(set)
    }
}

/// Converts a custom unit back into a [`CustomRock`] for serialisation.
fn unit_to_custom(unit: &RockUnit) -> CustomRock {
    CustomRock {
        name: unit.name.clone(),
        colour: unit.colour,
        mohs: unit.mohs,
        permeability_m2: unit.permeability_m2,
        friction_angle_deg: unit.friction_angle_deg,
        cohesion_mpa: unit.cohesion_mpa,
        density_kg_m3: unit.density_kg_m3,
        solubility: unit.solubility,
        weathering_rate_m_yr: unit.weathering_rate_m_yr,
        origin: unit.origin,
    }
}

// ---------------------------------------------------------------------------
// StrataSpec
// ---------------------------------------------------------------------------

/// Reference to a rock in a [`StrataSpec`] layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RockSelector {
    /// Built-in catalogue key, e.g. `granite`.
    Builtin(String),
    /// User-defined rock name.
    Custom(String),
    /// Explicit dense rock id in the owning [`RockSet`].
    Id(u8),
}

impl RockSelector {
    /// Resolves the selector against a rock set.
    ///
    /// # Errors
    ///
    /// Unknown keys/names or an out-of-range id are reported.
    pub fn resolve(&self, rocks: &RockSet) -> GeoResult<u8> {
        match self {
            Self::Builtin(key) => rocks
                .units()
                .iter()
                .find(|unit| unit.builtin && unit.key == *key)
                .map(|unit| unit.id)
                .ok_or_else(|| GeoError::NotFound(format!("built-in rock `{key}` does not exist"))),
            Self::Custom(name) => rocks
                .units()
                .iter()
                .find(|unit| !unit.builtin && unit.name == *name)
                .map(|unit| unit.id)
                .ok_or_else(|| GeoError::NotFound(format!("custom rock `{name}` is not defined"))),
            Self::Id(id) => {
                if (*id as usize) < rocks.len() {
                    Ok(*id)
                } else {
                    Err(GeoError::InvalidConfig(format!(
                        "rock id {id} is outside the {} rocks in the set",
                        rocks.len()
                    )))
                }
            }
        }
    }

    /// True when the selector carries no rock name/id.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Builtin(name) | Self::Custom(name) => name.trim().is_empty(),
            Self::Id(_) => false,
        }
    }
}

/// One layer of a user-defined stratigraphic stack (SPEC §4c).
#[derive(Clone, Debug, PartialEq)]
pub struct StrataLayer {
    /// Layer thickness, metres (0.1–100 000).
    pub thickness_m: f64,
    /// Rock of this layer (built-in or custom).
    pub rock: RockSelector,
    /// Dip of the layer, degrees (0–90).
    pub dip_deg: f64,
    /// Strike of the layer, degrees from east (0–360).
    pub strike_deg: f64,
    /// Vertical offset of a fault cutting this layer's base, metres
    /// (-10 000–+10 000); positive moves the base deeper.
    pub fault_offset_m: f64,
    /// True when the base of the layer is an erosional unconformity; the
    /// overlying layer is then weakened, as in the generated columns.
    pub unconformity: bool,
}

impl Default for StrataLayer {
    fn default() -> Self {
        Self {
            thickness_m: 100.0,
            rock: RockSelector::Builtin("sandstone".into()),
            dip_deg: 0.0,
            strike_deg: 0.0,
            fault_offset_m: 0.0,
            unconformity: false,
        }
    }
}

impl StrataLayer {
    /// A layer of `thickness_m` metres of a built-in rock key.
    #[must_use]
    pub fn builtin(key: impl Into<String>, thickness_m: f64) -> Self {
        Self {
            thickness_m,
            rock: RockSelector::Builtin(key.into()),
            ..Self::default()
        }
    }

    /// A layer of `thickness_m` metres of a custom rock name.
    #[must_use]
    pub fn custom(name: impl Into<String>, thickness_m: f64) -> Self {
        Self {
            thickness_m,
            rock: RockSelector::Custom(name.into()),
            ..Self::default()
        }
    }

    /// Validates the numeric ranges and the rock selector shape.
    ///
    /// # Errors
    ///
    /// Out-of-range thickness/dip/strike/fault offset or an empty rock selector.
    pub fn validate(&self) -> GeoResult<()> {
        check_range(
            "thickness_m",
            self.thickness_m,
            (MIN_LAYER_THICKNESS_M, MAX_LAYER_THICKNESS_M),
            "strata layer",
        )?;
        check_range("dip_deg", self.dip_deg, (0.0, 90.0), "strata layer")?;
        check_range("strike_deg", self.strike_deg, (0.0, 360.0), "strata layer")?;
        check_range(
            "fault_offset_m",
            self.fault_offset_m,
            (-MAX_FAULT_OFFSET_M, MAX_FAULT_OFFSET_M),
            "strata layer",
        )?;
        if self.rock.is_empty() {
            return Err(GeoError::InvalidConfig(
                "strata layer `rock` must name a rock".into(),
            ));
        }
        Ok(())
    }

    fn write_json(&self, out: &mut String, indent: usize, pretty: bool) -> GeoResult<()> {
        out.push('{');
        let mut first = true;
        let field = |out: &mut String, key: &str, first: &mut bool| {
            if !*first {
                out.push(',');
            }
            if pretty {
                json::newline(out, indent + 1);
            }
            json::write_string(out, key);
            out.push(':');
            if pretty {
                out.push(' ');
            }
            *first = false;
        };
        field(out, "thickness_m", &mut first);
        json::write_number(out, self.thickness_m)?;
        field(out, "rock", &mut first);
        match &self.rock {
            RockSelector::Builtin(key) | RockSelector::Custom(key) => {
                json::write_string(out, key);
            }
            RockSelector::Id(id) => json::write_number(out, f64::from(*id))?,
        }
        field(out, "dip_deg", &mut first);
        json::write_number(out, self.dip_deg)?;
        field(out, "strike_deg", &mut first);
        json::write_number(out, self.strike_deg)?;
        field(out, "fault_offset_m", &mut first);
        json::write_number(out, self.fault_offset_m)?;
        field(out, "unconformity", &mut first);
        out.push_str(if self.unconformity { "true" } else { "false" });
        if pretty {
            json::newline(out, indent);
        }
        out.push('}');
        Ok(())
    }

    fn from_value(value: &json::Value) -> GeoResult<Self> {
        let rock = match value.get("rock") {
            Some(json::Value::String(name)) => {
                // A plain string may name a built-in key or a custom rock; the
                // built-in interpretation wins, mirroring `RockSet` lookups.
                if ROCKS.iter().any(|rock| rock.key == name.as_str()) {
                    RockSelector::Builtin(name.clone())
                } else {
                    RockSelector::Custom(name.clone())
                }
            }
            Some(json::Value::Number(id)) => {
                let id = *id;
                if !(0.0..=255.0).contains(&id) || id.fract() != 0.0 {
                    return Err(GeoError::InvalidConfig(
                        "strata layer `rock` id must be an integer in 0..=255".into(),
                    ));
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    RockSelector::Id(id as u8)
                }
            }
            _ => {
                return Err(GeoError::InvalidConfig(
                    "strata layer needs a `rock` string or integer id".into(),
                ))
            }
        };
        let layer = Self {
            thickness_m: required_number(value, "thickness_m")?,
            rock,
            dip_deg: optional_number(value, "dip_deg")?.unwrap_or(0.0),
            strike_deg: optional_number(value, "strike_deg")?.unwrap_or(0.0),
            fault_offset_m: optional_number(value, "fault_offset_m")?.unwrap_or(0.0),
            unconformity: match value.get("unconformity") {
                Some(item) => item.as_bool().ok_or_else(|| {
                    GeoError::InvalidConfig("`unconformity` must be a boolean".into())
                })?,
                None => false,
            },
        };
        layer.validate()?;
        Ok(layer)
    }
}

/// A user-defined stratigraphic stack (SPEC §4c): 1–100 layers, ordered from
/// the youngest at the surface to the oldest at depth.
///
/// Convert it into a [`Stratigraphy`] with [`Self::to_stratigraphy`] (which
/// resolves the rock references and validates everything), then install it on a
/// [`crate::geology::Geology`] with
/// [`crate::geology::Geology::apply_strata_spec`].
#[derive(Clone, Debug, PartialEq)]
pub struct StrataSpec {
    /// Layers, youngest (index 0, at the surface) to oldest.
    pub layers: Vec<StrataLayer>,
}

impl Default for StrataSpec {
    fn default() -> Self {
        Self {
            layers: vec![StrataLayer::default()],
        }
    }
}

impl StrataSpec {
    /// Builds and structurally validates a spec.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::validate`].
    pub fn new(layers: Vec<StrataLayer>) -> GeoResult<Self> {
        let spec = Self { layers };
        spec.validate()?;
        Ok(spec)
    }

    /// Number of layers.
    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// Total thickness, metres.
    #[must_use]
    pub fn total_thickness_m(&self) -> f64 {
        self.layers.iter().map(|layer| layer.thickness_m).sum()
    }

    /// Validates the layer count and per-layer ranges (rock references are not
    /// resolved here; see [`Self::validate_with`]).
    ///
    /// # Errors
    ///
    /// Empty stacks, more than 100 layers and out-of-range layers are rejected.
    pub fn validate(&self) -> GeoResult<()> {
        if !(MIN_STRATA_LAYERS..=MAX_STRATA_LAYERS).contains(&self.layers.len()) {
            return Err(GeoError::InvalidConfig(format!(
                "strata spec needs {MIN_STRATA_LAYERS}..={MAX_STRATA_LAYERS} layers, got {}",
                self.layers.len()
            )));
        }
        for (index, layer) in self.layers.iter().enumerate() {
            layer.validate().map_err(|error| {
                GeoError::InvalidConfig(format!("strata layer {index}: {error}"))
            })?;
        }
        Ok(())
    }

    /// Validates the structure and resolves every rock reference against
    /// `rocks`.
    ///
    /// # Errors
    ///
    /// Structural errors plus unknown built-in keys, undefined custom names and
    /// out-of-range ids.
    pub fn validate_with(&self, rocks: &RockSet) -> GeoResult<()> {
        self.validate()?;
        for (index, layer) in self.layers.iter().enumerate() {
            layer.rock.resolve(rocks).map_err(|error| {
                GeoError::InvalidConfig(format!("strata layer {index}: {error}"))
            })?;
        }
        Ok(())
    }

    /// Converts the spec into a [`Stratigraphy`] using `rocks` for the ids.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::validate_with`].
    pub fn to_stratigraphy(&self, rocks: &RockSet) -> GeoResult<Stratigraphy> {
        self.validate_with(rocks)?;
        let mut layers = Vec::with_capacity(self.layers.len());
        let mut age_kyr = 0.0f64;
        for layer in &self.layers {
            let rock = layer.rock.resolve(rocks)?;
            age_kyr += layer.thickness_m / 1000.0;
            layers.push(Stratum {
                rock,
                thickness_m: layer.thickness_m,
                dip_deg: layer.dip_deg,
                strike_deg: layer.strike_deg.rem_euclid(360.0),
                fault_offset_m: layer.fault_offset_m,
                unconformity: layer.unconformity,
                age_yr: age_kyr * 1000.0,
            });
        }
        Ok(Stratigraphy { layers })
    }

    /// Compact JSON for the spec.
    ///
    /// # Errors
    ///
    /// Only if a non-finite value slipped past validation.
    pub fn to_json(&self) -> GeoResult<String> {
        self.write_json(false)
    }

    /// Indented JSON for the spec.
    ///
    /// # Errors
    ///
    /// Only if a non-finite value slipped past validation.
    pub fn to_json_pretty(&self) -> GeoResult<String> {
        self.write_json(true)
    }

    fn write_json(&self, pretty: bool) -> GeoResult<String> {
        let mut out = String::with_capacity(96 + 150 * self.layers.len());
        out.push('{');
        if pretty {
            json::newline(&mut out, 1);
        }
        json::write_string(&mut out, "version");
        out.push(':');
        if pretty {
            out.push(' ');
        }
        out.push('1');
        out.push(',');
        if pretty {
            json::newline(&mut out, 1);
        }
        json::write_string(&mut out, "layers");
        out.push(':');
        if pretty {
            out.push(' ');
        }
        out.push('[');
        for (index, layer) in self.layers.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            if pretty {
                json::newline(&mut out, 2);
            }
            layer.write_json(&mut out, 2, pretty)?;
        }
        if pretty && !self.layers.is_empty() {
            json::newline(&mut out, 1);
        }
        out.push(']');
        if pretty {
            json::newline(&mut out, 0);
        }
        out.push('}');
        Ok(out)
    }

    /// Parses a strata spec.
    ///
    /// Accepts `{"version":1,"layers":[...]}` or a bare array of layer objects.
    /// Rock references are checked structurally; call [`Self::validate_with`]
    /// (or [`Self::to_stratigraphy`]) to resolve them against a [`RockSet`].
    ///
    /// # Errors
    ///
    /// Malformed JSON, layer-count violations and out-of-range layers are
    /// rejected.
    pub fn from_json(text: &str) -> GeoResult<Self> {
        let value = json::parse(text)?;
        let entries = match &value {
            json::Value::Array(items) => items.clone(),
            json::Value::Object(_) => {
                if let Some(version) = value.get("version").and_then(json::Value::as_f64) {
                    if !(0.5..=1.5).contains(&version) {
                        return Err(GeoError::InvalidConfig(format!(
                            "unsupported strata JSON version {version}"
                        )));
                    }
                }
                value
                    .get("layers")
                    .and_then(json::Value::as_array)
                    .ok_or_else(|| {
                        GeoError::InvalidConfig("strata JSON needs a `layers` array".into())
                    })?
                    .to_vec()
            }
            _ => {
                return Err(GeoError::InvalidConfig(
                    "strata JSON must be an object or an array".into(),
                ))
            }
        };
        let layers = entries
            .iter()
            .map(StrataLayer::from_value)
            .collect::<GeoResult<Vec<_>>>()?;
        let spec = Self { layers };
        spec.validate()?;
        Ok(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn soft_rock() -> CustomRock {
        CustomRock {
            name: "Siltite".into(),
            colour: [184, 168, 140],
            mohs: 2.0,
            permeability_m2: 1e-14,
            friction_angle_deg: 28.0,
            cohesion_mpa: 1.5,
            density_kg_m3: 2_350.0,
            solubility: 0.2,
            weathering_rate_m_yr: 2e-4,
            origin: RockOrigin::Sedimentary,
        }
    }

    fn hard_rock() -> CustomRock {
        CustomRock {
            name: "Quartzite".into(),
            colour: [210, 205, 195],
            mohs: 8.0,
            permeability_m2: 1e-18,
            friction_angle_deg: 45.0,
            cohesion_mpa: 16.0,
            density_kg_m3: 2_650.0,
            solubility: 0.01,
            weathering_rate_m_yr: 2e-6,
            origin: RockOrigin::Metamorphic,
        }
    }

    fn set_with_customs() -> RockSet {
        let mut rocks = RockSet::builtin();
        rocks.push_custom(soft_rock()).unwrap();
        rocks.push_custom(hard_rock()).unwrap();
        rocks
    }

    fn spec_with(rocks: &RockSet) -> StrataSpec {
        assert_eq!(rocks.custom_count(), 2);
        StrataSpec::new(vec![
            StrataLayer {
                thickness_m: 120.0,
                rock: RockSelector::Builtin("sandstone".into()),
                dip_deg: 10.0,
                strike_deg: 30.0,
                fault_offset_m: 0.0,
                unconformity: false,
            },
            StrataLayer {
                thickness_m: 80.5,
                rock: RockSelector::Custom("Siltite".into()),
                dip_deg: 12.5,
                strike_deg: 30.0,
                fault_offset_m: 250.0,
                unconformity: true,
            },
            StrataLayer {
                thickness_m: 400.0,
                rock: RockSelector::Id(15),
                dip_deg: 5.0,
                strike_deg: 30.0,
                fault_offset_m: -100.0,
                unconformity: false,
            },
        ])
        .unwrap_or_else(|error| panic!("valid spec rejected: {error}"))
    }

    #[test]
    fn builtin_set_matches_the_catalogue() {
        let rocks = RockSet::builtin();
        assert_eq!(rocks.len(), BUILTIN_ROCK_COUNT);
        assert_eq!(rocks.custom_count(), 0);
        assert!(rocks.custom_ids().is_empty());
        for (index, unit) in rocks.units().iter().enumerate() {
            assert_eq!(unit.id as usize, index);
            assert!(unit.builtin);
            assert_eq!(unit.key, ROCKS[index].key);
            assert!((unit.erodibility - ROCKS[index].erodibility).abs() < 1e-12);
            assert!((unit.density_kg_m3 - ROCKS[index].density_kg_m3).abs() < 1e-9);
            assert!((unit.solubility - ROCKS[index].solubility).abs() < 1e-12);
            assert!(
                (FRICTION_RANGE_DEG.0..=FRICTION_RANGE_DEG.1).contains(&unit.friction_angle_deg)
            );
            assert!((COHESION_RANGE_MPA.0..=COHESION_RANGE_MPA.1).contains(&unit.cohesion_mpa));
            assert!(unit.permeability_m2.is_finite() && unit.permeability_m2 > 0.0);
        }
        assert_eq!(rocks.id_of_key("granite"), Some(0));
        assert_eq!(rocks.id_of_key("chert"), Some(13));
        assert_eq!(rocks.id_of_key("nope"), None);
        assert_eq!(rocks.get_or_default(250).id, 0);
    }

    #[test]
    fn custom_rocks_get_appended_ids_and_derived_properties() {
        let rocks = set_with_customs();
        assert_eq!(rocks.len(), BUILTIN_ROCK_COUNT + 2);
        assert_eq!(rocks.custom_count(), 2);
        assert_eq!(rocks.custom_ids(), vec![14, 15]);
        let soft = rocks.get(14).unwrap();
        let hard = rocks.get(15).unwrap();
        assert!(!soft.builtin && !hard.builtin);
        assert_eq!(soft.name, "Siltite");
        assert_eq!(soft.origin, RockOrigin::Sedimentary);
        assert!(
            derived_erodibility(2.0, RockOrigin::Sedimentary)
                > 1.3 * derived_erodibility(8.0, RockOrigin::Metamorphic)
        );
        assert!(soft.erodibility > 1.3 * hard.erodibility);
        assert!((soft.permeability_m2 - 1e-14).abs() < 1e-18);
        assert_eq!(rocks.id_of_key("Siltite"), Some(14));
        assert_eq!(rocks.id_of_key("Quartzite"), Some(15));
    }

    #[test]
    fn invalid_custom_rocks_are_rejected() {
        let mut rocks = RockSet::builtin();
        let mut mutate: Vec<fn(&mut CustomRock)> = vec![
            |rock| rock.name = "  ".into(),
            |rock| rock.mohs = 0.5,
            |rock| rock.mohs = 11.0,
            |rock| rock.permeability_m2 = 1e-3,
            |rock| rock.friction_angle_deg = 10.0,
            |rock| rock.cohesion_mpa = 25.0,
            |rock| rock.density_kg_m3 = 1_000.0,
            |rock| rock.solubility = 1.5,
            |rock| rock.weathering_rate_m_yr = 0.0,
        ];
        for apply in &mut mutate {
            let mut rock = soft_rock();
            apply(&mut rock);
            assert!(rock.validate().is_err(), "accepted invalid rock {rock:?}");
            assert!(rocks.push_custom(rock).is_err());
        }
        assert!(rocks.push_custom(soft_rock()).is_ok());
        assert!(rocks.push_custom(soft_rock()).is_err());
        let mut granite_clash = soft_rock();
        granite_clash.name = "Granite".into();
        assert!(rocks.push_custom(granite_clash).is_err());
        // Duplicate detection is case-insensitive.
        let mut lower = soft_rock();
        lower.name = "siltite".into();
        assert!(rocks.push_custom(lower).is_err());
    }

    #[test]
    fn custom_rock_json_round_trips_exactly() {
        let rocks = set_with_customs();
        let json = rocks.to_json().unwrap();
        let parsed = RockSet::from_json(&json).unwrap();
        assert_eq!(parsed, rocks);
        let pretty = rocks.to_json_pretty().unwrap();
        assert!(pretty.contains('\n'));
        assert_eq!(RockSet::from_json(&pretty).unwrap(), rocks);
        // A bare array of custom rocks is accepted too.
        let rock = soft_rock();
        let single = rock.to_json().unwrap();
        let array = format!("[{single}]");
        let parsed_single = RockSet::from_json(&array).unwrap();
        assert_eq!(parsed_single.custom_count(), 1);
        assert_eq!(parsed_single.get(14).unwrap().name, "Siltite");
        assert_eq!(CustomRock::from_json(&single).unwrap(), rock);
        // An empty list of custom rocks is a valid set (the default catalogue).
        assert_eq!(RockSet::from_json("[]").unwrap(), RockSet::builtin());
        // A full-catalogue document may list built-ins by reference.
        let full = format!("{{\"version\":1,\"rocks\":[{{\"builtin\":\"granite\"}},{single}]}}");
        let parsed_full = RockSet::from_json(&full).unwrap();
        assert_eq!(parsed_full.custom_count(), 1);
        assert!(
            RockSet::from_json("{\"version\":1,\"rocks\":[{\"builtin\":\"unobtainium\"}]}")
                .is_err()
        );
        // The default set serialises an empty custom list and round-trips.
        let builtin = RockSet::builtin();
        let builtin_json = builtin.to_json().unwrap();
        assert!(builtin_json.contains("\"custom_rocks\":[]"));
        assert_eq!(RockSet::from_json(&builtin_json).unwrap(), builtin);
    }

    #[test]
    fn malformed_rock_json_is_rejected() {
        let bad_documents = [
            "",
            "{",
            "{}",
            "{\"version\":1}",
            "{\"version\":9,\"custom_rocks\":[]}",
            "{\"version\":1,\"custom_rocks\":[{}]}",
            "{\"version\":1,\"custom_rocks\":[{\"name\":\"X\"}]}",
            "{\"version\":1,\"custom_rocks\":[]} trailing",
            "{\"version\":1,\"custom_rocks\":[{\"name\":\"X\",\"colour\":[1,2]}]}",
        ];
        for document in bad_documents {
            assert!(
                RockSet::from_json(document).is_err(),
                "accepted malformed document `{document}`"
            );
        }
        let rock = soft_rock();
        let base = rock.to_json().unwrap();
        let unknown_origin = base.replace("\"sedimentary\"", "\"volcanic\"");
        assert!(CustomRock::from_json(&unknown_origin).is_err());
        let bad_hardness = base.replace("\"mohs\":2", "\"mohs\":12");
        assert!(CustomRock::from_json(&bad_hardness).is_err());
        // Escaped and non-BMP names survive the parser (surrogate pairs).
        let mut escaped = soft_rock();
        escaped.name = "Silt \"n\" rock".into();
        let text = escaped.to_json().unwrap();
        assert_eq!(CustomRock::from_json(&text).unwrap().name, escaped.name);
        escaped.name = "Rock 🪨".into();
        let text = escaped.to_json().unwrap();
        assert_eq!(CustomRock::from_json(&text).unwrap().name, escaped.name);
        // A duplicate JSON key is a hard error, not last-wins.
        let duplicate = base.replacen('{', "{\"name\":\"Other\",", 1);
        assert!(CustomRock::from_json(&duplicate).is_err());
    }

    #[test]
    fn strata_spec_json_round_trips_and_converts() {
        let rocks = set_with_customs();
        let spec = spec_with(&rocks);
        assert_eq!(spec.layer_count(), 3);
        assert!((spec.total_thickness_m() - 600.5).abs() < 1e-9);
        spec.validate_with(&rocks).unwrap();

        let json = spec.to_json().unwrap();
        let parsed = StrataSpec::from_json(&json).unwrap();
        assert_eq!(parsed, spec);
        assert_eq!(
            StrataSpec::from_json(&spec.to_json_pretty().unwrap()).unwrap(),
            spec
        );
        // Numeric rock id survives as an id selector.
        assert_eq!(parsed.layers[2].rock, RockSelector::Id(15));

        let column = spec.to_stratigraphy(&rocks).unwrap();
        assert_eq!(column.layers.len(), 3);
        assert_eq!(column.layers[0].rock, 3); // sandstone
        assert_eq!(column.layers[1].rock, 14); // Siltite
        assert_eq!(column.layers[2].rock, 15); // Quartzite
        assert!((column.layers[1].fault_offset_m - 250.0).abs() < 1e-9);
        assert!(column.layers[1].unconformity);
        assert!(column.layers[0].age_yr < column.layers[1].age_yr);
        assert!(column.layers[1].age_yr < column.layers[2].age_yr);
        for layer in &column.layers {
            assert!((0.0..360.0).contains(&layer.strike_deg));
        }
        // A bare array of layers is accepted as well.
        let array = json
            .trim_start_matches("{\"version\":1,\"layers\":")
            .trim_end_matches('}')
            .to_string();
        assert_eq!(StrataSpec::from_json(&array).unwrap(), spec);
    }

    #[test]
    fn invalid_strata_specs_are_rejected() {
        assert!(StrataSpec::new(vec![]).is_err());
        assert!(StrataSpec::new(vec![StrataLayer::default(); MAX_STRATA_LAYERS + 1]).is_err());
        assert!(StrataSpec::new(vec![StrataLayer::default(); MAX_STRATA_LAYERS]).is_ok());
        let mut layer = StrataLayer::default();
        layer.thickness_m = 0.01;
        assert!(StrataSpec::new(vec![layer.clone()]).is_err());
        layer.thickness_m = 1e6;
        assert!(StrataSpec::new(vec![layer.clone()]).is_err());
        layer.thickness_m = 10.0;
        layer.dip_deg = 95.0;
        assert!(StrataSpec::new(vec![layer.clone()]).is_err());
        layer.dip_deg = 45.0;
        layer.fault_offset_m = 20_000.0;
        assert!(StrataSpec::new(vec![layer.clone()]).is_err());
        layer.fault_offset_m = 0.0;
        layer.strike_deg = 400.0;
        assert!(StrataSpec::new(vec![layer.clone()]).is_err());
        layer.strike_deg = 0.0;
        layer.rock = RockSelector::Builtin("  ".into());
        assert!(StrataSpec::new(vec![layer]).is_err());

        let rocks = set_with_customs();
        let unknown = StrataSpec::new(vec![StrataLayer::builtin("unobtainium", 10.0)]).unwrap();
        assert!(unknown.validate_with(&rocks).is_err());
        assert!(unknown.to_stratigraphy(&rocks).is_err());
        let missing_custom = StrataSpec::new(vec![StrataLayer::custom("Missing", 10.0)]).unwrap();
        assert!(missing_custom.validate_with(&rocks).is_err());
        let bad_id = StrataSpec::new(vec![StrataLayer {
            rock: RockSelector::Id(200),
            ..StrataLayer::default()
        }])
        .unwrap();
        assert!(bad_id.validate_with(&rocks).is_err());

        for document in [
            "",
            "{}",
            "{\"version\":1}",
            "{\"version\":1,\"layers\":[]}",
            "{\"version\":1,\"layers\":[{\"rock\":\"granite\"}]}",
            "{\"version\":1,\"layers\":[{\"thickness_m\":10,\"rock\":\"granite\",\"dip_deg\":120}]}",
            "{\"version\":2,\"layers\":[{\"thickness_m\":10,\"rock\":\"granite\"}]}",
        ] {
            assert!(
                StrataSpec::from_json(document).is_err(),
                "accepted malformed strata document `{document}`"
            );
        }
    }

    #[test]
    fn strata_layer_order_survives_extreme_dip_and_fault_offsets() {
        // Steep dip plus maximum fault offsets: whichever layer is exposed, the
        // boundaries must stay strictly ordered because generation clamps the
        // structural shifts.
        let rocks = RockSet::builtin();
        let mut layers = Vec::new();
        for index in 0..12 {
            layers.push(StrataLayer {
                thickness_m: 400.0,
                rock: RockSelector::Builtin("sandstone".into()),
                dip_deg: 85.0,
                strike_deg: (index as f64 * 30.0) % 360.0,
                fault_offset_m: if index % 2 == 0 { 9_000.0 } else { -9_000.0 },
                unconformity: index % 3 == 0,
            });
        }
        let spec = StrataSpec::new(layers).unwrap();
        let column = spec.to_stratigraphy(&rocks).unwrap();
        for along_dip in [-2.0e6, -50_000.0, 0.0, 50_000.0, 2.0e6] {
            let mut previous = column.boundary_depth_m(0, along_dip).unwrap();
            for layer in 1..column.layers.len() {
                let depth = column.boundary_depth_m(layer, along_dip).unwrap();
                assert!(
                    depth > previous,
                    "boundary {layer} not below the previous one ({depth} vs {previous})"
                );
                previous = depth;
            }
            let mut previous_index = 0;
            for eroded in [0.0, 100.0, 1_000.0, 5_000.0, 50_000.0] {
                let exposed = column.layer_at_depth_tilted(eroded, along_dip);
                assert!(
                    exposed >= previous_index,
                    "erosion moved the exposure upward: {exposed} < {previous_index}"
                );
                previous_index = exposed;
            }
        }
    }
}
