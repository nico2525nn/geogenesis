//! GeoGenesis core: a geological terrain genesis engine.
//!
//! The engine turns a seed and a parameter set into a world, by running the
//! processes in the order a geologist would:
//!
//! 1. **Tectonics** — plate boundaries, relative motion, uplift and subsidence.
//! 2. **Structure** — the initial relief, plus named landform stamps.
//! 3. **Climate** — temperature and precipitation fields.
//! 4. **Weathering and erosion** — runoff, rivers, hillslopes, wind, ice,
//!    waves and dissolution, applied in the schedule from spec §6b.
//! 5. **Report** — statistics, profiles, geological maps and exports.
//!
//! Every module is deterministic given [`world::WorldConfig::seed`]. There is no
//! global state, no wall-clock input, and no thread-count-dependent output, so
//! a world can be regenerated exactly from its seed and parameters.

#![doc(html_root_url = "https://docs.rs/geogenesis-core/0.1.0")]

pub mod grid;
pub mod hydrology;
pub mod linalg;
pub mod noise;
pub mod rng;

/// Result type used throughout the engine.
pub type GeoResult<T> = core::result::Result<T, GeoError>;

/// Errors the engine can report.
#[derive(Debug)]
pub enum GeoError {
    /// A parameter was outside the range declared in the spec.
    InvalidParameter {
        name: &'static str,
        value: String,
        reason: String,
    },
    /// Grid dimensions were zero, or outside spec §Phase 0 limits.
    InvalidGrid(String),
    /// An export target could not be written.
    Io(String),
    /// A simulation produced non-finite values that could not be repaired.
    Numerical(String),
}

impl core::fmt::Display for GeoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GeoError::InvalidParameter {
                name,
                value,
                reason,
            } => write!(f, "parameter `{name}` = {value} is invalid: {reason}"),
            GeoError::InvalidGrid(m) => write!(f, "invalid grid: {m}"),
            GeoError::Io(m) => write!(f, "io error: {m}"),
            GeoError::Numerical(m) => write!(f, "numerical error: {m}"),
        }
    }
}

impl std::error::Error for GeoError {}

impl From<std::io::Error> for GeoError {
    fn from(e: std::io::Error) -> Self {
        GeoError::Io(e.to_string())
    }
}