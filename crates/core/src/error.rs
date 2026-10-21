//! Engine error type.
//!
//! Errors carry enough context to be shown verbatim in a UI: the parameter
//! name, the offending value and a one-line reason. The WASM boundary reports
//! them through `gg_last_error`.

use core::fmt;

/// Everything the engine can fail at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeoError {
    /// A parameter value is outside the range declared by its [`crate::params::ParamSpec`].
    InvalidParameter {
        /// Parameter key, as declared in the schema.
        key: &'static str,
        /// The offending value, rendered by the caller.
        value: String,
        /// One-line human-readable reason.
        reason: String,
    },
    /// Cross-parameter consistency failed (e.g. `step_yr > total_yr`).
    InvalidConfig(String),
    /// A grid dimension or extent is outside the supported range.
    InvalidGrid(String),
    /// A numerical routine could not produce a finite, bounded result.
    Numerical(String),
    /// A file or serialisation operation failed.
    Io(String),
    /// A requested preset, field or history frame does not exist.
    NotFound(String),
}

impl GeoError {
    /// Helper for building a parameter error from any displayable value.
    pub fn param(key: &'static str, value: impl fmt::Display, reason: impl Into<String>) -> Self {
        Self::InvalidParameter {
            key,
            value: value.to_string(),
            reason: reason.into(),
        }
    }

    /// Stable machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidParameter { .. } => "invalid_parameter",
            Self::InvalidConfig(_) => "invalid_config",
            Self::InvalidGrid(_) => "invalid_grid",
            Self::Numerical(_) => "numerical",
            Self::Io(_) => "io",
            Self::NotFound(_) => "not_found",
        }
    }
}

impl fmt::Display for GeoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidParameter { key, value, reason } => {
                write!(f, "parameter `{key}` = {value}: {reason}")
            }
            Self::InvalidConfig(m) => write!(f, "invalid configuration: {m}"),
            Self::InvalidGrid(m) => write!(f, "invalid grid: {m}"),
            Self::Numerical(m) => write!(f, "numerical error: {m}"),
            Self::Io(m) => write!(f, "io error: {m}"),
            Self::NotFound(m) => write!(f, "not found: {m}"),
        }
    }
}

impl std::error::Error for GeoError {}

impl From<std::io::Error> for GeoError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Result alias used throughout the engine.
pub type GeoResult<T> = Result<T, GeoError>;
