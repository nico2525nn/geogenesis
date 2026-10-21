//! GeoGenesis core: a scientific terrain-genesis and landscape-evolution
//! engine.
//!
//! The engine turns a seed and a parameter set into a world, then evolves it
//! over geological time by applying the processes a geologist would:
//!
//! 1. **Tectonics** — moving plates, boundary forces, isostasy and hotspots.
//! 2. **Relief** — fractal base topography, folds, faults and landform stamps.
//! 3. **Climate** — temperature, moisture transport, precipitation and biomes.
//! 4. **Hydrology** — depression filling, flow routing, discharge and lakes.
//! 5. **Surface processes** — rivers, hillslopes, soil, ice, wind, waves and
//!    chemical dissolution, each on its own schedule.
//! 6. **Report** — statistics, profiles, maps, history frames and exports.
//!
//! Every module is deterministic given [`params::WorldParams::seed`]. There is
//! no global state, no wall-clock input, and no thread-count-dependent output,
//! so a world can be regenerated exactly from its seed and parameters.
//!
//! Units are SI throughout the simulation internals: metres, years, kilograms,
//! kelvin for absolute temperature (Celsius for the API), metres per year for
//! rates. UI-facing parameters use the conventional units named in their
//! [`params::ParamSpec`] (km, mm/yr, °C) and convert at the boundary.

#![doc(html_root_url = "https://docs.rs/geogenesis-core/2.0.0")]

pub mod climate;
pub mod erosion;
pub mod error;
pub mod export;
pub mod geology;
pub mod grid;
pub mod hydrology;
pub mod math;
pub mod minerals;
pub mod params;
pub mod pipeline;
pub mod plates;
pub mod plugin;
pub mod rocks;
pub mod sim;
pub mod spatial;
pub mod stats;
pub mod tiling;
pub mod uplift;
pub mod world;

pub use error::{GeoError, GeoResult};
pub use grid::{BoundaryKind, Field, FieldScalar, Grid};
pub use params::Params;
pub use sim::Simulation;

/// Engine version string, also reported through the WASM ABI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
