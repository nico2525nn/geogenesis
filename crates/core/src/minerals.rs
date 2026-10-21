//! Mineral and resource deposits (spec §2b / Phase 3 #17).
//!
//! The mineral system is a *prospectivity model*, not an ore-genesis solver.
//! It scores the current world for fourteen illustrative deposit types, keeps
//! the strongest one per cell, and turns the result into a deposit map, an
//! inventory (tonnage/value) and JSON for the UI and CLI.
//!
//! # What drives a deposit
//!
//! Every mineral has a small weighted rule over normalised per-cell
//! indicators. All indicators are in `0..=1` and the weights of one rule sum to
//! (at most) one, so a rule score is itself bounded:
//!
//! | indicator | source |
//! |---|---|
//! | `tectonic` | `World::tectonic_rate_m_yr`, scaled by the 5 mm/yr physical ceiling |
//! | `relief`, `flat` | local surface gradient (steep terrain vs. plains) |
//! | `exhumation` | cumulative erosion over half the configured relief envelope |
//! | `cratonic` | thick crust × quiet tectonics × little exhumation |
//! | `hard` | inverse of the exposed rock erodibility |
//! | `igneous` / `metamorphic` / `sedimentary` | exposed [`RockOrigin`] |
//! | `volcanic`, `felsic`, `carbonate`, `organic`, `reservoir`, `chert` | exposed rock key |
//! | `permeability`, `solubility` | rock properties (reservoirs/aquifers, chemical sediments) |
//! | `silicate` | non-carbonate host (bauxite laterite source) |
//! | `sediment` / `basin` | loose sediment and sub-sea-level shelf |
//! | `coastal` / `shallow` / `deep` | distance to ocean, shelf depth, abyssal depth |
//! | `wet` / `arid` / `hot` / `tropical` / `warm` / `vegetation` | [`Climate`] |
//! | `river` / `discharge` / `lake` | [`Hydro`] channels, accumulation, discharge and lakes |
//! | `ice` | snow cover suppresses surface deposits |
//!
//! The rules themselves (each documented on [`Mineral::affinity`]):
//!
//! * **porphyry Cu–Au** — active arcs: tectonic rate + felsic/igneous host +
//!   relief and exhumation; land preferred;
//! * **orogenic gold** — active metamorphic belts: tectonic rate + metamorphic
//!   host + exhumation + relief;
//! * **sedex Zn–Pb** — quiet sedimentary basins: sediment + shale/carbonate +
//!   flat ground + mild tectonic activity;
//! * **BIF iron** — cratonic chert/crystalline basement, stable and unexhumed;
//! * **coal** — wet, vegetated, flat sedimentary lowlands (swamps and deltas);
//! * **oil & gas** — organic-rich source rock, thick sediment, porous reservoir
//!   on land or shelf, little exhumation;
//! * **evaporite (salt/potash)** — arid coastal sabkhas, playas and shallow
//!   shelf, hot and flat, soluble host;
//! * **uranium** — felsic basement or sandstone aquifer in an old craton,
//!   moderately active margin;
//! * **kimberlite diamonds** — deep, stable, thick craton interior far from
//!   the coast (rare, high threshold);
//! * **bauxite** — tropical, wet, flat, vegetated silicate lowlands (laterite);
//! * **REE pegmatite** — felsic/metamorphic basement in orogenic belts;
//! * **graphite** — metamorphic cratonic basement with tectonic fabric;
//! * **placer tin** — active river channels and lowland sediment fed by
//!   felsic basement;
//! * **geothermal** — active volcanic/tectonic belts (a resource, not a solid
//!   deposit; tonnage is an energy-equivalent).
//!
//! Determinism comes from three seeded stateless hashes: a shared smooth
//! "metallogenic province" prospectivity field (two-octave value noise), a
//! per-mineral per-cell speckle, and the inventory grade factor. No RNG state
//! is carried between calls, so [`generate`] on identical inputs is
//! bit-identical.
//!
//! # Map semantics
//!
//! [`MineralMap::dominant`] is the index into [`MINERALS`] of the strongest
//! mineral at the cell, or [`NO_MINERAL`] (`255`) when no mineral reaches the
//! visibility threshold. [`MineralMap::richness`] is that strongest score in
//! `0..=1` and is zero for cells with no dominant mineral.
//! [`MineralMap::deposit_mask`] is `1` exactly where the dominant mineral
//! crosses its economic [`Mineral::deposit_threshold`] (i.e. an economic
//! deposit), and `0` otherwise. Ids are `u8` so the maps stay cheap to ship to
//! the Web UI.
//!
//! The tonnage and value estimates are *illustrative*, deliberately rounded
//! endowments per km² of a fully rich cell; they are useful for comparing
//! worlds and for gameplay, not for resource assessment.

use crate::climate::{Biome, Climate};
use crate::geology::{Geology, RockOrigin};
use crate::grid::{Field, Grid};
use crate::hydrology::Hydro;
use crate::math::rng::{derive_seed, hash2, hash3, unit_from_hash};
use crate::math::{clamp, lerp, smoothstep};
use crate::params::{push_json_escaped, push_json_number, Params};
use crate::world::World;

/// Domain tag for the mineral random streams.
const DOMAIN_MINERALS: u64 = 0x6D69_6E65_7261_6C73;
/// Domain tag for the shared metallogenic-province noise.
const DOMAIN_PROVINCE: u64 = 0x7072_6F76_696E_6365;

/// Sentinel dominant id for "no mineral at this cell".
pub const NO_MINERAL: u8 = 255;

/// Below this strongest-mineral score a cell is reported as [`NO_MINERAL`].
const MIN_DOMINANT_RICHNESS: f64 = 0.055;

/// Depth below sea level, metres, beyond which a cell counts as abyssal.
const ABYSSAL_DEPTH_M: f64 = 600.0;

/// Deposit category, used for symbol grouping in the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MineralCategory {
    /// Magmatic / orthomagmatic.
    #[default]
    Magmatic,
    /// Hydrothermal.
    Hydrothermal,
    /// Sedimentary.
    Sedimentary,
    /// Metamorphic.
    Metamorphic,
    /// Placer.
    Placer,
    /// Evaporite.
    Evaporite,
    /// Energy resource.
    Energy,
}

impl MineralCategory {
    /// All categories in display order.
    pub const ALL: [Self; 7] = [
        Self::Magmatic,
        Self::Hydrothermal,
        Self::Sedimentary,
        Self::Metamorphic,
        Self::Placer,
        Self::Evaporite,
        Self::Energy,
    ];

    /// Stable key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Magmatic => "magmatic",
            Self::Hydrothermal => "hydrothermal",
            Self::Sedimentary => "sedimentary",
            Self::Metamorphic => "metamorphic",
            Self::Placer => "placer",
            Self::Evaporite => "evaporite",
            Self::Energy => "energy",
        }
    }

    /// English label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Magmatic => "Magmatic",
            Self::Hydrothermal => "Hydrothermal",
            Self::Sedimentary => "Sedimentary",
            Self::Metamorphic => "Metamorphic",
            Self::Placer => "Placer",
            Self::Evaporite => "Evaporite",
            Self::Energy => "Energy",
        }
    }

    /// Japanese label.
    #[must_use]
    pub const fn label_ja(self) -> &'static str {
        match self {
            Self::Magmatic => "火成",
            Self::Hydrothermal => "熱水",
            Self::Sedimentary => "堆積",
            Self::Metamorphic => "変成",
            Self::Placer => "漂砂",
            Self::Evaporite => "蒸発岩",
            Self::Energy => "エネルギー",
        }
    }
}

/// One mineral / resource template.
#[derive(Clone, Copy, Debug)]
pub struct Mineral {
    /// Stable key.
    pub key: &'static str,
    /// English label.
    pub label: &'static str,
    /// Japanese label.
    pub label_ja: &'static str,
    /// Short chemical or resource symbol.
    pub symbol: &'static str,
    /// Deposit category.
    pub category: MineralCategory,
    /// Display colour `[r, g, b]`.
    pub colour: [u8; 3],
    /// Illustrative value, USD per tonne of commodity-equivalent.
    pub unit_value_usd_per_t: f64,
    /// Illustrative endowment of a fully rich cell, tonnes per km².
    pub endowment_t_per_km2: f64,
    /// Economic threshold on the richness scale (`0..=1`).
    pub deposit_threshold: f64,
    /// One-line formation affinity shown in the UI.
    pub affinity: &'static str,
}

/// The mineral catalogue, in schema order.
pub const MINERALS: [Mineral; 14] = [
    Mineral {
        key: "porphyry_cu_au",
        label: "Porphyry Cu-Au",
        label_ja: "斑岩銅金",
        symbol: "Cu-Au",
        category: MineralCategory::Magmatic,
        colour: [184, 115, 51],
        unit_value_usd_per_t: 9_000.0,
        endowment_t_per_km2: 6_000.0,
        deposit_threshold: 0.52,
        affinity: "Active convergent margin: felsic/igneous host, uplift and exhumation.",
    },
    Mineral {
        key: "orogenic_gold",
        label: "Orogenic gold",
        label_ja: "造山型金",
        symbol: "Au",
        category: MineralCategory::Hydrothermal,
        colour: [212, 175, 55],
        unit_value_usd_per_t: 60_000_000.0,
        endowment_t_per_km2: 25.0,
        deposit_threshold: 0.56,
        affinity: "Metamorphic belts with active tectonics and strong exhumation.",
    },
    Mineral {
        key: "sedex_zn_pb",
        label: "Sedex Zn-Pb",
        label_ja: "層準型亜鉛鉛",
        symbol: "Zn-Pb",
        category: MineralCategory::Hydrothermal,
        colour: [110, 140, 150],
        unit_value_usd_per_t: 2_500.0,
        endowment_t_per_km2: 9_000.0,
        deposit_threshold: 0.5,
        affinity: "Quiet sedimentary basins: shale/carbonate host, sediment and mild extension.",
    },
    Mineral {
        key: "bif_iron",
        label: "Banded iron (BIF)",
        label_ja: "縞状鉄鉱層",
        symbol: "Fe",
        category: MineralCategory::Sedimentary,
        colour: [140, 70, 50],
        unit_value_usd_per_t: 110.0,
        endowment_t_per_km2: 150_000.0,
        deposit_threshold: 0.47,
        affinity: "Stable cratonic chert/crystalline basement, little exhumation.",
    },
    Mineral {
        key: "coal",
        label: "Coal",
        label_ja: "石炭",
        symbol: "C",
        category: MineralCategory::Energy,
        colour: [45, 42, 40],
        unit_value_usd_per_t: 55.0,
        endowment_t_per_km2: 300_000.0,
        deposit_threshold: 0.45,
        affinity: "Wet, vegetated, flat sedimentary lowlands and deltas.",
    },
    Mineral {
        key: "oil_gas",
        label: "Oil & gas",
        label_ja: "石油・天然ガス",
        symbol: "CH",
        category: MineralCategory::Energy,
        colour: [86, 74, 54],
        unit_value_usd_per_t: 480.0,
        endowment_t_per_km2: 120_000.0,
        deposit_threshold: 0.46,
        affinity:
            "Organic-rich source rock, thick sediment and porous reservoir, little exhumation.",
    },
    Mineral {
        key: "evaporite",
        label: "Evaporite salt/potash",
        label_ja: "蒸発岩（岩塩・カリ）",
        symbol: "NaCl",
        category: MineralCategory::Evaporite,
        colour: [225, 222, 205],
        unit_value_usd_per_t: 90.0,
        endowment_t_per_km2: 400_000.0,
        deposit_threshold: 0.46,
        affinity: "Arid coastal sabkhas, playas and shallow shelf basins.",
    },
    Mineral {
        key: "uranium",
        label: "Uranium",
        label_ja: "ウラン",
        symbol: "U",
        category: MineralCategory::Hydrothermal,
        colour: [120, 190, 80],
        unit_value_usd_per_t: 100_000.0,
        endowment_t_per_km2: 250.0,
        deposit_threshold: 0.50,
        affinity: "Felsic cratonic basement or sandstone aquifer near an active margin.",
    },
    Mineral {
        key: "kimberlite_diamonds",
        label: "Kimberlite diamonds",
        label_ja: "キンバーライト・ダイヤモンド",
        symbol: "Di",
        category: MineralCategory::Magmatic,
        colour: [200, 230, 240],
        unit_value_usd_per_t: 5_000_000.0,
        endowment_t_per_km2: 8.0,
        deposit_threshold: 0.62,
        affinity: "Deep, stable craton interior far from the coast (rare).",
    },
    Mineral {
        key: "bauxite",
        label: "Bauxite",
        label_ja: "ボーキサイト",
        symbol: "Al",
        category: MineralCategory::Sedimentary,
        colour: [190, 120, 90],
        unit_value_usd_per_t: 55.0,
        endowment_t_per_km2: 120_000.0,
        deposit_threshold: 0.46,
        affinity: "Tropical, wet, flat, vegetated silicate lowlands (laterite).",
    },
    Mineral {
        key: "ree_pegmatite",
        label: "Rare-earth pegmatite",
        label_ja: "希土類ペグマタイト",
        symbol: "REE",
        category: MineralCategory::Magmatic,
        colour: [160, 90, 180],
        unit_value_usd_per_t: 35_000.0,
        endowment_t_per_km2: 2_500.0,
        deposit_threshold: 0.55,
        affinity: "Felsic/metamorphic basement in an orogenic belt.",
    },
    Mineral {
        key: "graphite",
        label: "Graphite",
        label_ja: "石墨",
        symbol: "Cg",
        category: MineralCategory::Metamorphic,
        colour: [70, 72, 78],
        unit_value_usd_per_t: 900.0,
        endowment_t_per_km2: 4_000.0,
        deposit_threshold: 0.50,
        affinity: "Metamorphic cratonic basement with tectonic fabric.",
    },
    Mineral {
        key: "placer_tin",
        label: "Placer tin",
        label_ja: "漂砂スズ",
        symbol: "Sn",
        category: MineralCategory::Placer,
        colour: [150, 160, 170],
        unit_value_usd_per_t: 26_000.0,
        endowment_t_per_km2: 120.0,
        deposit_threshold: 0.55,
        affinity: "Active river channels and lowland sediment fed by felsic basement.",
    },
    Mineral {
        key: "geothermal",
        label: "Geothermal field",
        label_ja: "地熱",
        symbol: "Geo",
        category: MineralCategory::Energy,
        colour: [220, 90, 60],
        unit_value_usd_per_t: 40.0,
        endowment_t_per_km2: 30_000.0,
        deposit_threshold: 0.40,
        affinity: "Active volcanic/tectonic belts (energy-equivalent tonnage).",
    },
];

/// Index of porphyry Cu–Au in [`MINERALS`].
pub const PORPHYRY_CU_AU: u8 = 0;
/// Index of orogenic gold in [`MINERALS`].
pub const OROGENIC_GOLD: u8 = 1;
/// Index of coal in [`MINERALS`].
pub const COAL: u8 = 4;
/// Index of oil & gas in [`MINERALS`].
pub const OIL_GAS: u8 = 5;
/// Index of the evaporite commodity in [`MINERALS`].
pub const EVAPORITE: u8 = 6;
/// Index of geothermal in [`MINERALS`].
pub const GEOTHERMAL: u8 = 13;

/// Looks up a catalogue entry by index.
#[must_use]
pub fn mineral(index: u8) -> Option<&'static Mineral> {
    MINERALS.get(index as usize)
}

/// Looks up a catalogue index by stable key.
#[must_use]
pub fn mineral_index(key: &str) -> Option<u8> {
    MINERALS.iter().position(|m| m.key == key).map(|i| i as u8)
}

/// Display colour for a dominant-mineral id; [`NO_MINERAL`] is neutral grey.
#[must_use]
pub fn dominant_colour_rgb(index: u8) -> [u8; 3] {
    match MINERALS.get(index as usize) {
        Some(m) => m.colour,
        None => [110, 110, 110],
    }
}

/// Dominant-mineral coverage of the map as `(mineral index, fraction)` pairs.
///
/// Only minerals that dominate at least one cell are reported, sorted by
/// descending fraction (ties by ascending index).
#[must_use]
pub fn coverage_fractions(map: &MineralMap) -> Vec<(u8, f64)> {
    let mut counts = [0u32; 256];
    let total = map.dominant.len().max(1) as f64;
    for &id in map.dominant.as_slice() {
        counts[id as usize] += 1;
    }
    let mut out: Vec<(u8, f64)> = (0..MINERALS.len())
        .filter_map(|i| {
            let count = counts[i];
            if count == 0 {
                None
            } else {
                Some((i as u8, f64::from(count) / total))
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(core::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    out
}

/// Mineral maps produced by [`generate`].
#[derive(Clone, Debug, PartialEq)]
pub struct MineralMap {
    /// Dominant mineral index per cell, or [`NO_MINERAL`].
    pub dominant: Field<u8>,
    /// Richness of the dominant mineral, `0..=1` (0 for [`NO_MINERAL`]).
    pub richness: Field<f32>,
    /// Economic deposit mask: `1` where the dominant mineral is economic.
    pub deposit_mask: Field<u8>,
}

/// One mineral's share of the inventory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MineralEntry {
    /// Index into [`MINERALS`].
    pub index: u8,
    /// Stable key.
    pub key: &'static str,
    /// English label.
    pub label: &'static str,
    /// Japanese label.
    pub label_ja: &'static str,
    /// Chemical/resource symbol.
    pub symbol: &'static str,
    /// Deposit category.
    pub category: MineralCategory,
    /// Display colour.
    pub colour: [u8; 3],
    /// Number of economic deposit cells.
    pub cells: usize,
    /// Deposit cells as a fraction of the whole grid.
    pub coverage: f64,
    /// Economic deposit area, km².
    pub area_km2: f64,
    /// Mean richness over the deposit cells.
    pub mean_richness: f64,
    /// Illustrative endowment at full richness, t/km².
    pub endowment_t_per_km2: f64,
    /// Estimated tonnage, tonnes (illustrative).
    pub tonnage_t: f64,
    /// Illustrative unit value, USD/t.
    pub unit_value_usd_per_t: f64,
    /// Estimated value, USD (illustrative).
    pub value_usd: f64,
}

/// Resource inventory over the whole map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MineralInventory {
    /// Grid width in cells.
    pub grid_width: usize,
    /// Grid height in cells.
    pub grid_height: usize,
    /// Area of one cell, km².
    pub cell_area_km2: f64,
    /// Number of economic deposit cells.
    pub deposit_cells: usize,
    /// Economic deposit area, km².
    pub deposit_area_km2: f64,
    /// Mean richness over all cells (`0..=1`).
    pub mean_richness: f64,
    /// Sum of all entry values, USD (illustrative).
    pub total_value_usd: f64,
    /// One entry per catalogue mineral, in catalogue order.
    pub entries: Vec<MineralEntry>,
}

/// Normalised per-cell indicators used by the formation rules.
#[derive(Clone, Copy, Debug, Default)]
struct Site {
    x: usize,
    y: usize,
    land: f64,
    coastal: f64,
    shallow: f64,
    deep: f64,
    flat: f64,
    relief: f64,
    tectonic: f64,
    exhumation: f64,
    sediment: f64,
    cratonic: f64,
    hard: f64,
    igneous: f64,
    volcanic: f64,
    metamorphic: f64,
    sedimentary: f64,
    felsic: f64,
    organic: f64,
    reservoir: f64,
    chert: f64,
    silicate: f64,
    permeability: f64,
    solubility: f64,
    discharge: f64,
    lowland: f64,
    wet: f64,
    arid: f64,
    hot: f64,
    tropical: f64,
    warm: f64,
    vegetation: f64,
    wetland: f64,
    river: f64,
    lake: f64,
    ice: f64,
    province: f64,
}

impl Site {
    /// `1` on land or a shallow shelf, `0` in the abyssal ocean.
    #[inline]
    fn basin(self) -> f64 {
        self.land.max(self.shallow)
    }
}

/// Shared generator state for [`generate`] and [`mineral_potential_map`].
struct MineralGenerator<'a> {
    grid: Grid,
    world: &'a World,
    params: &'a Params,
    climate: &'a Climate,
    hydro: &'a Hydro,
    geology: &'a Geology,
    seed: u64,
    /// Number of cells with valid data in every input field.
    cells: usize,
}

impl<'a> MineralGenerator<'a> {
    fn new(
        world: &'a World,
        params: &'a Params,
        climate: &'a Climate,
        hydro: &'a Hydro,
        geology: &'a Geology,
    ) -> Self {
        let grid = world.grid;
        let cells = world
            .cell_count()
            .min(climate.temperature_c.len())
            .min(hydro.drainage_area_m2.len())
            .min(geology.rock_id.len());
        let seed = derive_seed(params.world.effective_seed(), DOMAIN_MINERALS);
        Self {
            grid,
            world,
            params,
            climate,
            hydro,
            geology,
            seed,
            cells,
        }
    }

    /// Two-octave, optionally wrapping value noise in `-1..=1`.
    fn province_noise(&self, x: f64, y: f64, scale: f64, seed: u64) -> f64 {
        let grid = self.grid;
        let period_x = if grid.boundary().wraps_x() {
            Some((grid.width() as f64 / scale).round().max(1.0) as i64)
        } else {
            None
        };
        let period_y = if grid.boundary().wraps_y() {
            Some((grid.height() as f64 / scale).round().max(1.0) as i64)
        } else {
            None
        };
        let u = x / scale;
        let v = y / scale;
        let x0 = u.floor();
        let y0 = v.floor();
        let tx = smoothstep(0.0, 1.0, u - x0);
        let ty = smoothstep(0.0, 1.0, v - y0);
        let sample = |ix: i64, iy: i64| {
            let hx = match period_x {
                Some(p) => ix.rem_euclid(p.max(1)),
                None => ix,
            };
            let hy = match period_y {
                Some(p) => iy.rem_euclid(p.max(1)),
                None => iy,
            };
            let h = hash2(hx as u64, hy as u64, seed);
            unit_from_hash(h) * 2.0 - 1.0
        };
        let ix = x0 as i64;
        let iy = y0 as i64;
        let top = lerp(sample(ix, iy), sample(ix + 1, iy), tx);
        let bottom = lerp(sample(ix, iy + 1), sample(ix + 1, iy + 1), tx);
        lerp(top, bottom, ty)
    }

    /// Smooth metallogenic-province multiplier in about `0.3..=1.0`.
    fn prospectivity(&self, x: f64, y: f64) -> f64 {
        let coarse = self.province_noise(x, y, 26.0, self.seed ^ DOMAIN_PROVINCE);
        let fine = self.province_noise(x, y, 9.0, self.seed ^ DOMAIN_PROVINCE.rotate_left(17));
        let n = 0.65 * coarse + 0.35 * fine;
        0.65 + 0.35 * n
    }

    /// Computes every indicator for one cell.
    fn site(&self, index: usize) -> Site {
        let grid = self.grid;
        let (x, y) = grid.coords(index);
        let world = self.world;
        let sea = world.sea_level_m;
        let z = world.elevation_m.get_index(index);
        let vertical = self.params.world.vertical_m.max(100.0);
        let land = if z > sea { 1.0 } else { 0.0 };
        let above = (z - sea).max(0.0);
        let depth = (sea - z).max(0.0);
        let shallow = if land > 0.5 {
            0.0
        } else {
            (1.0 - depth / 250.0).clamp(0.0, 1.0)
        };
        let ocean_distance = f64::from(self.climate.ocean_distance_km.get_index(index));
        let coastal = if land > 0.5 {
            (1.0 - ocean_distance / 80.0).clamp(0.0, 1.0)
        } else {
            shallow
        };
        let deep = if land < 0.5 && depth > ABYSSAL_DEPTH_M {
            1.0
        } else {
            0.0
        };
        let slope = world.elevation_m.slope(x, y);
        let flat = (1.0 - slope / 0.10).clamp(0.0, 1.0);
        let relief = (slope / 0.15).clamp(0.0, 1.0);
        let tectonic =
            (f64::from(world.tectonic_rate_m_yr.get_index(index)).abs() / 5.0e-3).clamp(0.0, 1.0);
        let exhumation =
            (f64::from(world.erosion_m.get_index(index)) / (vertical * 0.5)).clamp(0.0, 1.0);
        let sediment = (f64::from(world.sediment_m.get_index(index)) / 300.0).clamp(0.0, 1.0);
        let crust = ((world.crust_m.get_index(index) - 25_000.0) / 20_000.0).clamp(0.0, 1.0);
        let hard = clamp(
            1.0 - (f64::from(self.geology.erodibility.get_index(index)) - 0.4) / 1.1,
            0.0,
            1.0,
        );
        // A craton is thick, hard, quiet and little exhumed crust, not merely
        // any continental cell.
        let cratonic = crust * (0.5 + 0.5 * hard) * (1.0 - tectonic) * (1.0 - 0.6 * exhumation);

        // Resolve through the world's rock set so user-defined rocks from
        // `crate::rocks` participate in the same rules as the built-ins.
        let rock = self
            .geology
            .rocks
            .get_or_default(self.geology.rock_id.get_index(index));
        let rock_key = rock.key.as_str();
        let igneous = if rock.origin == RockOrigin::Igneous {
            1.0
        } else {
            0.0
        };
        let metamorphic = if rock.origin == RockOrigin::Metamorphic {
            1.0
        } else {
            0.0
        };
        let sedimentary = if rock.origin == RockOrigin::Sedimentary {
            1.0
        } else {
            0.0
        };
        let volcanic = if matches!(rock_key, "basalt" | "tuff") {
            1.0
        } else {
            0.0
        };
        let felsic = match rock_key {
            "granite" => 1.0,
            "gneiss" => 0.9,
            "schist" => 0.6,
            "chert" => 0.3,
            _ => 0.0,
        };
        let carbonate = if matches!(rock_key, "limestone" | "dolomite" | "marble") {
            1.0
        } else {
            0.0
        };
        let organic = if matches!(rock_key, "shale" | "mudstone") {
            1.0
        } else {
            0.0
        };
        let reservoir = if matches!(rock_key, "sandstone" | "conglomerate") {
            1.0
        } else {
            0.0
        };
        let chert = if rock_key == "chert" { 1.0 } else { 0.0 };
        let log_permeability = rock.permeability_m2.max(1e-30).log10();
        let permeability = clamp((log_permeability + 20.0) / 7.5, 0.0, 1.0);
        let solubility = rock.solubility.clamp(0.0, 1.0);

        let rain = f64::from(self.climate.precipitation_mm.get_index(index));
        let aridity = f64::from(self.climate.aridity.get_index(index));
        let temperature = f64::from(self.climate.temperature_c.get_index(index));
        let vegetation = f64::from(self.climate.vegetation.get_index(index)).clamp(0.0, 1.0);
        let snow = f64::from(self.climate.snow_cover.get_index(index)).clamp(0.0, 1.0);
        let wetland = if self.climate.biome.get_index(index) == Biome::Wetland as u8 {
            1.0
        } else {
            0.0
        };

        let domain_cells = grid.cell_count().max(1) as f64;
        let accumulation = self.hydro.accumulation_cells.get_index(index) / domain_cells;
        let discharge = (self.hydro.discharge_m3_yr.get_index(index) / 1.0e9).clamp(0.0, 1.0);
        let flow_strength = (accumulation * 4.0).clamp(0.0, 1.0).max(discharge);
        let river = if self.hydro.river_mask.get_index(index) > 0 {
            0.55 + 0.45 * flow_strength
        } else {
            0.0
        };
        let lake = (f64::from(self.hydro.lake_depth_m.get_index(index)) / 3.0).clamp(0.0, 1.0);

        Site {
            x,
            y,
            land,
            coastal,
            shallow,
            deep,
            flat,
            relief,
            tectonic,
            exhumation,
            sediment,
            cratonic,
            hard,
            igneous,
            volcanic,
            metamorphic,
            sedimentary,
            felsic,
            organic,
            reservoir,
            chert,
            silicate: 1.0 - carbonate,
            permeability,
            solubility,
            discharge,
            lowland: (1.0 - above / (vertical * 0.35)).clamp(0.0, 1.0),
            wet: (rain / 2500.0).clamp(0.0, 1.0),
            arid: ((aridity - 2.0) / 4.0).clamp(0.0, 1.0),
            hot: ((temperature - 10.0) / 18.0).clamp(0.0, 1.0),
            tropical: ((temperature - 20.0) / 8.0).clamp(0.0, 1.0),
            warm: ((temperature - 2.0) / 22.0).clamp(0.0, 1.0),
            vegetation,
            wetland,
            river,
            lake,
            ice: snow,
            province: self.prospectivity(x as f64, y as f64),
        }
    }

    /// Rule score of one mineral for one cell, in `0..=1`.
    fn rule_score(mineral: usize, s: &Site) -> f64 {
        match mineral {
            // Active arcs: tectonic rate + igneous/felsic host + relief.
            0 => {
                (0.38 * s.tectonic
                    + 0.20 * s.igneous
                    + 0.14 * s.relief
                    + 0.10 * s.exhumation
                    + 0.10 * s.volcanic
                    + 0.08 * s.coastal)
                    * (0.25 + 0.75 * s.land)
            }
            // Metamorphic belts with active uplift and exhumation.
            1 => {
                0.50 * s.tectonic
                    + 0.20 * s.metamorphic
                    + 0.16 * s.exhumation
                    + 0.08 * s.relief
                    + 0.06 * s.hard
            }
            // Quiet sedimentary basins with organic/carbonate host.
            2 => {
                (0.30 * s.sedimentary
                    + 0.20 * s.organic
                    + 0.20 * s.flat
                    + 0.16 * s.sediment
                    + 0.14 * s.tectonic)
                    * (0.25 + 0.75 * s.basin())
            }
            // Cratonic chert/crystalline basement, stable and unexhumed.
            3 => {
                0.9 * (0.34 * s.chert + 0.28 * s.cratonic + 0.20 * s.hard + 0.18 * s.metamorphic)
                    * (0.30 + 0.70 * s.land)
            }
            // Wet, vegetated, flat sedimentary lowlands.
            4 => {
                1.65 * (0.28 * s.wet
                    + 0.24 * s.vegetation
                    + 0.16 * s.flat
                    + 0.12 * s.sedimentary
                    + 0.10 * s.wetland
                    + 0.10 * s.lake)
                    * s.land
                    * (0.5 + 0.5 * s.wet)
                    * (0.35 + 0.65 * s.warm)
            }
            // Organic source + thick sediment + porous reservoir.
            5 => {
                1.2 * (0.26 * s.organic
                    + 0.22 * s.sediment
                    + 0.18 * s.flat
                    + 0.14 * s.reservoir
                    + 0.10 * (1.0 - s.exhumation)
                    + 0.10 * s.permeability)
                    * s.basin()
            }
            // Arid sabkhas, playas and shallow shelf basins.
            6 => {
                0.9 * (0.28 * s.arid
                    + 0.16 * s.coastal
                    + 0.14 * s.flat
                    + 0.10 * s.sediment
                    + 0.10 * s.solubility
                    + 0.10 * s.lake
                    + 0.12 * s.hot)
                    * (0.15 + 0.85 * s.arid)
                    * s.basin()
            }
            // Felsic cratonic basement / sandstone aquifer.
            7 => {
                0.85 * (0.32 * s.felsic
                    + 0.24 * s.cratonic
                    + 0.20 * s.reservoir
                    + 0.12 * s.permeability
                    + 0.12 * s.tectonic)
                    * (0.30 + 0.70 * s.land)
            }
            // Deep, stable craton interior far from the coast (rare).
            8 => {
                0.45 * (0.44 * s.cratonic
                    + 0.24 * s.hard
                    + 0.18 * (1.0 - s.tectonic)
                    + 0.14 * (1.0 - s.coastal))
                    * (0.30 + 0.70 * s.land)
            }
            // Tropical laterite.
            9 => {
                1.1 * (0.30 * s.tropical
                    + 0.26 * s.wet
                    + 0.18 * s.flat
                    + 0.14 * s.vegetation
                    + 0.12 * s.silicate)
                    * (0.2 + 0.8 * s.tropical)
                    * s.land
            }
            // Felsic/metamorphic basement in orogenic belts.
            10 => {
                0.75 * (0.34 * s.felsic
                    + 0.26 * s.metamorphic
                    + 0.22 * s.cratonic
                    + 0.18 * s.tectonic)
                    * (0.30 + 0.70 * s.land)
            }
            // Metamorphic cratonic basement with tectonic fabric.
            11 => {
                0.65 * (0.34 * s.metamorphic
                    + 0.26 * s.cratonic
                    + 0.22 * s.tectonic
                    + 0.18 * s.hard)
                    * (0.30 + 0.70 * s.land)
            }
            // Placer: active rivers and lowland sediment from felsic sources.
            12 => {
                1.25 * (0.30 * s.river
                    + 0.12 * s.discharge
                    + 0.18 * s.sediment
                    + 0.14 * s.felsic
                    + 0.14 * s.lowland
                    + 0.12 * s.flat)
                    * s.land
            }
            // Geothermal: active volcanic/tectonic belts (energy resource).
            _ => {
                (0.50 * s.tectonic + 0.20 * s.volcanic + 0.15 * s.relief + 0.15 * s.igneous)
                    * (0.40 + 0.60 * s.basin())
            }
        }
    }

    /// Final richness of one mineral at one cell, in `0..=1`.
    fn richness(&self, site: &Site, mineral: usize) -> f64 {
        let score = Self::rule_score(mineral, site);
        let speck = 0.6
            + 0.8
                * unit_from_hash(hash3(
                    site.x as u64,
                    site.y as u64,
                    mineral as u64,
                    self.seed,
                ));
        let mut value = score * site.province * speck;
        if site.deep > 0.5 {
            // Abyssal plains bury most deposits; keep only rare ridge settings.
            value *= 0.45;
        }
        value *= 1.0 - 0.7 * site.ice;
        clamp(value, 0.0, 1.0)
    }
}

/// Builds the mineral map for the current world state.
///
/// All five inputs must describe the same grid; the function tolerates shorter
/// fields by treating the missing cells as barren rather than panicking. It is
/// deterministic: identical inputs (including `params.world.seed`) produce
/// identical maps.
#[must_use]
pub fn generate(
    world: &World,
    params: &Params,
    climate: &Climate,
    hydro: &Hydro,
    geology: &Geology,
) -> MineralMap {
    let generator = MineralGenerator::new(world, params, climate, hydro, geology);
    let grid = world.grid;
    let mut dominant = Field::filled(grid, NO_MINERAL);
    let mut richness = Field::zeros(grid);
    let mut deposit_mask = Field::zeros(grid);
    for index in 0..generator.cells {
        let site = generator.site(index);
        let mut best = NO_MINERAL;
        let mut best_score = 0.0f64;
        for m in 0..MINERALS.len() {
            let value = generator.richness(&site, m);
            if value > best_score {
                best_score = value;
                best = m as u8;
            }
        }
        if best != NO_MINERAL && best_score >= MIN_DOMINANT_RICHNESS {
            dominant.set_index(index, best);
            richness.set_index(index, best_score as f32);
            if best_score >= MINERALS[best as usize].deposit_threshold {
                deposit_mask.set_index(index, 1);
            }
        }
    }
    MineralMap {
        dominant,
        richness,
        deposit_mask,
    }
}

/// Richness field of one mineral, for a per-mineral "prospectivity" map layer.
///
/// Cells outside the valid input range are zero. The returned values use the
/// same seeded speckle as [`generate`], so the mineral that maximises this
/// field at a cell is that cell's [`MineralMap::dominant`] entry whenever it
/// wins the full comparison.
#[must_use]
pub fn mineral_potential_map(
    index: u8,
    world: &World,
    params: &Params,
    climate: &Climate,
    hydro: &Hydro,
    geology: &Geology,
) -> Field<f32> {
    let generator = MineralGenerator::new(world, params, climate, hydro, geology);
    let grid = world.grid;
    let mineral = index as usize;
    if mineral >= MINERALS.len() {
        return Field::zeros(grid);
    }
    Field::from_fn(grid, |x, y| {
        let i = grid.index(x, y);
        if i < generator.cells {
            generator.richness(&generator.site(i), mineral) as f32
        } else {
            0.0
        }
    })
}

/// Aggregates the map into tonnage/value estimates.
///
/// `params` supplies the world seed for the deterministic (illustrative) grade
/// factor of each commodity. The grid should be the map's grid; a mismatched
/// grid uses the overlapping cells only.
#[must_use]
pub fn inventory(map: &MineralMap, grid: Grid, params: &Params) -> MineralInventory {
    let cells = map.dominant.len().min(grid.cell_count());
    let total_cells = cells.max(1) as f64;
    let cell_area_km2 = grid.cell_area_m2() / 1_000_000.0;
    // Deposits are weighted by the physical cell area (spherical worlds have
    // latitude-dependent areas); `cell_area_km2` stays the nominal planar area
    // reported in the summary for backward compatibility.
    let mut deposit_area_km2 = 0.0f64;
    let seed = derive_seed(params.world.effective_seed(), DOMAIN_MINERALS);
    let mut entries: Vec<MineralEntry> = MINERALS
        .iter()
        .enumerate()
        .map(|(i, m)| MineralEntry {
            index: i as u8,
            key: m.key,
            label: m.label,
            label_ja: m.label_ja,
            symbol: m.symbol,
            category: m.category,
            colour: m.colour,
            cells: 0,
            coverage: 0.0,
            area_km2: 0.0,
            mean_richness: 0.0,
            endowment_t_per_km2: m.endowment_t_per_km2,
            tonnage_t: 0.0,
            unit_value_usd_per_t: m.unit_value_usd_per_t,
            value_usd: 0.0,
        })
        .collect();
    let mut richness_sum = 0.0f64;
    let mut deposit_cells = 0usize;
    for i in 0..cells {
        let id = map.dominant.get_index(i);
        richness_sum += f64::from(map.richness.get_index(i));
        if id == NO_MINERAL || map.deposit_mask.get_index(i) == 0 {
            continue;
        }
        let entry = &mut entries[id as usize];
        let area_km2 = grid.cell_area_m2_at_index(i) / 1_000_000.0;
        entry.cells += 1;
        entry.area_km2 += area_km2;
        entry.mean_richness += f64::from(map.richness.get_index(i));
        deposit_area_km2 += area_km2;
        deposit_cells += 1;
    }
    let mut total_value = 0.0f64;
    for entry in &mut entries {
        if entry.cells == 0 {
            continue;
        }
        let count = entry.cells as f64;
        entry.coverage = count / total_cells;
        entry.mean_richness /= count;
        // Deterministic, illustrative grade variation around the mean.
        let grade = 0.85 + 0.30 * unit_from_hash(hash2(entry.index as u64, 0, seed));
        entry.tonnage_t = entry.area_km2 * entry.endowment_t_per_km2 * entry.mean_richness * grade;
        entry.value_usd = entry.tonnage_t * entry.unit_value_usd_per_t;
        total_value += entry.value_usd;
    }
    MineralInventory {
        grid_width: grid.width(),
        grid_height: grid.height(),
        cell_area_km2,
        deposit_cells,
        deposit_area_km2,
        mean_richness: richness_sum / total_cells,
        total_value_usd: total_value,
        entries,
    }
}

/// Serialises the catalogue, map coverage and inventory as JSON.
///
/// This is the payload the Web UI and CLI use for the resource panel; it is a
/// single object with `minerals`, `map` and `inventory` members.
#[must_use]
pub fn mineral_json(map: &MineralMap, inventory: &MineralInventory) -> String {
    let mut out = String::with_capacity(16_000);
    out.push_str("{\"minerals\":[");
    for (i, m) in MINERALS.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"key\":\"");
        push_json_escaped(&mut out, m.key);
        out.push_str("\",\"label\":\"");
        push_json_escaped(&mut out, m.label);
        out.push_str("\",\"label_ja\":\"");
        push_json_escaped(&mut out, m.label_ja);
        out.push_str("\",\"symbol\":\"");
        push_json_escaped(&mut out, m.symbol);
        out.push_str("\",\"category\":\"");
        push_json_escaped(&mut out, m.category.key());
        out.push_str("\",\"colour\":[");
        out.push_str(&m.colour[0].to_string());
        out.push(',');
        out.push_str(&m.colour[1].to_string());
        out.push(',');
        out.push_str(&m.colour[2].to_string());
        out.push_str("],\"unit_value_usd_per_t\":");
        push_json_number(&mut out, m.unit_value_usd_per_t);
        out.push_str(",\"endowment_t_per_km2\":");
        push_json_number(&mut out, m.endowment_t_per_km2);
        out.push_str(",\"deposit_threshold\":");
        push_json_number(&mut out, m.deposit_threshold);
        out.push_str(",\"affinity\":\"");
        push_json_escaped(&mut out, m.affinity);
        out.push_str("\"}");
    }
    out.push_str("],\"map\":{\"width\":");
    out.push_str(&map.dominant.width().to_string());
    out.push_str(",\"height\":");
    out.push_str(&map.dominant.height().to_string());
    out.push_str(",\"cells\":");
    out.push_str(&map.dominant.len().to_string());
    out.push_str(",\"deposit_cells\":");
    out.push_str(&inventory.deposit_cells.to_string());
    out.push_str(",\"deposit_area_km2\":");
    push_json_number(&mut out, inventory.deposit_area_km2);
    out.push_str(",\"mean_richness\":");
    push_json_number(&mut out, inventory.mean_richness);
    out.push_str(",\"dominant\":[");
    for (i, (id, fraction)) in coverage_fractions(map).iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"index\":");
        out.push_str(&id.to_string());
        out.push_str(",\"key\":\"");
        push_json_escaped(&mut out, MINERALS[*id as usize].key);
        out.push_str("\",\"fraction\":");
        push_json_number(&mut out, *fraction);
        out.push('}');
    }
    out.push_str("]},\"inventory\":{\"grid_width\":");
    out.push_str(&inventory.grid_width.to_string());
    out.push_str(",\"grid_height\":");
    out.push_str(&inventory.grid_height.to_string());
    out.push_str(",\"cell_area_km2\":");
    push_json_number(&mut out, inventory.cell_area_km2);
    out.push_str(",\"deposit_cells\":");
    out.push_str(&inventory.deposit_cells.to_string());
    out.push_str(",\"deposit_area_km2\":");
    push_json_number(&mut out, inventory.deposit_area_km2);
    out.push_str(",\"mean_richness\":");
    push_json_number(&mut out, inventory.mean_richness);
    out.push_str(",\"total_value_usd\":");
    push_json_number(&mut out, inventory.total_value_usd);
    out.push_str(",\"entries\":[");
    for (i, entry) in inventory.entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"index\":");
        out.push_str(&entry.index.to_string());
        out.push_str(",\"key\":\"");
        push_json_escaped(&mut out, entry.key);
        out.push_str("\",\"cells\":");
        out.push_str(&entry.cells.to_string());
        out.push_str(",\"coverage\":");
        push_json_number(&mut out, entry.coverage);
        out.push_str(",\"area_km2\":");
        push_json_number(&mut out, entry.area_km2);
        out.push_str(",\"mean_richness\":");
        push_json_number(&mut out, entry.mean_richness);
        out.push_str(",\"tonnage_t\":");
        push_json_number(&mut out, entry.tonnage_t);
        out.push_str(",\"unit_value_usd_per_t\":");
        push_json_number(&mut out, entry.unit_value_usd_per_t);
        out.push_str(",\"value_usd\":");
        push_json_number(&mut out, entry.value_usd);
        out.push('}');
    }
    out.push_str("]}}");
    out
}

/// Renders the mineral map as an RGB PNG: economic deposits in their mineral
/// colour (brightness scaled by richness), sub-economic occurrences dimmed, and
/// barren cells as a muted terrain tint (ocean/land) for context.
#[must_use]
pub fn map_png(map: &MineralMap, world: &crate::world::World) -> Vec<u8> {
    let grid = world.grid;
    let cells = grid.cell_count().min(map.dominant.len());
    let mut rgb = vec![0u8; cells * 3];
    let sea = world.sea_level_m;
    for i in 0..cells {
        let (x, y) = grid.coords(i);
        let z = world.elevation_m.get(x, y);
        let base = if z <= sea {
            [22.0, 48.0, 78.0]
        } else {
            let t = ((z - sea) / 4000.0).clamp(0.0, 1.0);
            [58.0 + 60.0 * t, 66.0 + 60.0 * t, 44.0 + 48.0 * t]
        };
        let id = map.dominant.get_index(i);
        let richness = f64::from(map.richness.get_index(i));
        let colour = if id == NO_MINERAL {
            base
        } else {
            let c = MINERALS[id as usize].colour;
            let gain = if map.deposit_mask.get_index(i) != 0 {
                0.65 + 0.35 * richness
            } else {
                0.22 + 0.18 * richness
            };
            [
                (base[0] * (1.0 - gain) + f64::from(c[0]) * gain).clamp(0.0, 255.0),
                (base[1] * (1.0 - gain) + f64::from(c[1]) * gain).clamp(0.0, 255.0),
                (base[2] * (1.0 - gain) + f64::from(c[2]) * gain).clamp(0.0, 255.0),
            ]
        };
        rgb[i * 3] = colour[0].round() as u8;
        rgb[i * 3 + 1] = colour[1].round() as u8;
        rgb[i * 3 + 2] = colour[2].round() as u8;
    }
    crate::export::encode_png_rgb(grid.width() as u32, grid.height() as u32, &rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::{self, ClimateInputs};
    use crate::hydrology;
    use crate::pipeline;
    use crate::plates::PlateMosaic;

    /// Builds world + climate + hydro + geology for a parameter set.
    fn build(p: &Params) -> (World, Climate, Hydro, Geology) {
        let out = pipeline::generate_initial(p).expect("world generation");
        let climate = climate::compute(&ClimateInputs {
            world: &out.world,
            params: p,
            previous: None,
            dt_yr: 1000.0,
        });
        let hydro = hydrology::route(&out.world, p, Some(&climate));
        (out.world, climate, hydro, out.geology)
    }

    fn small_params(seed: u64) -> Params {
        let mut p = Params::default();
        p.world.width = 48;
        p.world.height = 48;
        p.world.cell_km = 10.0;
        p.world.seed = seed;
        p.time.total_yr = 50_000.0;
        p.time.max_iters = 10;
        p.normalize();
        p
    }

    fn preset_params(key: &str) -> Params {
        let mut p = Params::default();
        p.apply_preset(key).expect("preset");
        p.world.width = 64;
        p.world.height = 64;
        p.normalize();
        p
    }

    #[test]
    fn catalogue_is_complete_and_well_formed() {
        assert!(MINERALS.len() >= 10, "catalogue is too small");
        let mut keys: Vec<&str> = MINERALS.iter().map(|m| m.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), MINERALS.len(), "duplicate mineral keys");
        let mut seen = [false; 7];
        for m in &MINERALS {
            assert!(!m.label.is_empty() && !m.label_ja.is_empty());
            assert!(!m.symbol.is_empty() && !m.affinity.is_empty());
            assert!(m.unit_value_usd_per_t > 0.0 && m.unit_value_usd_per_t.is_finite());
            assert!(m.endowment_t_per_km2 > 0.0 && m.endowment_t_per_km2.is_finite());
            assert!(
                (0.0..=1.0).contains(&m.deposit_threshold),
                "{} threshold out of range",
                m.key
            );
            let colour = m.colour;
            assert!(colour.iter().any(|&c| c > 0));
            seen[m.category as usize] = true;
        }
        // Every category must be represented.
        assert!(seen.iter().all(|&x| x), "catalogue misses a category");
        // Index helpers resolve both ways.
        for (i, m) in MINERALS.iter().enumerate() {
            assert_eq!(mineral_index(m.key), Some(i as u8));
            assert_eq!(mineral(i as u8).map(|x| x.key), Some(m.key));
            assert_eq!(dominant_colour_rgb(i as u8), m.colour);
        }
        assert!(mineral(MINERALS.len() as u8).is_none());
        assert_eq!(dominant_colour_rgb(NO_MINERAL), [110, 110, 110]);
        assert_eq!(mineral_index("not_a_mineral"), None);
    }

    #[test]
    fn generation_is_deterministic_and_seed_sensitive() {
        let p = small_params(7);
        let (world, climate, hydro, geology) = build(&p);
        let a = generate(&world, &p, &climate, &hydro, &geology);
        let b = generate(&world, &p, &climate, &hydro, &geology);
        assert_eq!(a, b, "identical inputs must produce identical maps");
        assert_eq!(
            a.richness.as_slice(),
            b.richness.as_slice(),
            "richness must be bit-identical"
        );
        // A different world seed changes the stochastic terms.
        let mut q = p.clone();
        q.world.seed = 8;
        let (world2, climate2, hydro2, geology2) = build(&q);
        let c = generate(&world2, &q, &climate2, &hydro2, &geology2);
        assert_ne!(a.richness.as_slice(), c.richness.as_slice());
    }

    #[test]
    fn richness_is_bounded_and_ids_are_valid() {
        let p = small_params(11);
        let (world, climate, hydro, geology) = build(&p);
        let map = generate(&world, &p, &climate, &hydro, &geology);
        assert_eq!(map.dominant.len(), world.cell_count());
        assert_eq!(map.richness.len(), world.cell_count());
        assert_eq!(map.deposit_mask.len(), world.cell_count());
        for i in 0..world.cell_count() {
            let id = map.dominant.get_index(i);
            let value = map.richness.get_index(i);
            let mask = map.deposit_mask.get_index(i);
            assert!(value.is_finite(), "richness is not finite");
            assert!(
                (0.0..=1.0).contains(&value),
                "richness {value} out of range"
            );
            assert!(id == NO_MINERAL || (id as usize) < MINERALS.len());
            assert!(mask <= 1);
            if id == NO_MINERAL {
                assert_eq!(value, 0.0);
                assert_eq!(mask, 0);
            } else {
                assert!(value >= MIN_DOMINANT_RICHNESS as f32);
                let economic = value >= MINERALS[id as usize].deposit_threshold as f32;
                assert_eq!(mask == 1, economic, "deposit mask disagrees with threshold");
            }
        }
        // A living world should expose a mix of minerals and some barren cells.
        let coverage = coverage_fractions(&map);
        assert!(coverage.len() >= 2, "only one mineral dominates the map");
        let sum: f64 = coverage.iter().map(|(_, f)| f).sum();
        assert!(sum > 0.0 && sum <= 1.0 + 1e-9);
        let barren = map
            .dominant
            .as_slice()
            .iter()
            .filter(|&&id| id == NO_MINERAL)
            .count();
        assert!(barren > 0, "every cell has a dominant mineral");
    }

    #[test]
    fn coverage_fractions_are_sorted_and_consistent() {
        let p = small_params(3);
        let (world, climate, hydro, geology) = build(&p);
        let map = generate(&world, &p, &climate, &hydro, &geology);
        let coverage = coverage_fractions(&map);
        let mut ids: Vec<u8> = coverage.iter().map(|(id, _)| *id).collect();
        let unique = {
            let mut v = ids.clone();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        assert_eq!(ids.len(), unique, "coverage contains duplicate ids");
        ids.dedup();
        let total_cells = map.dominant.len() as f64;
        for &(id, fraction) in &coverage {
            assert!((id as usize) < MINERALS.len());
            let count = map.dominant.as_slice().iter().filter(|&&d| d == id).count();
            assert!((fraction - count as f64 / total_cells).abs() < 1e-12);
            assert!((0.0..=1.0).contains(&fraction));
        }
        for pair in coverage.windows(2) {
            assert!(pair[0].1 >= pair[1].1, "coverage is not sorted");
        }
    }

    /// Synthetic map with a known arrangement for exact inventory arithmetic.
    fn synthetic_map() -> (MineralMap, Grid, Params) {
        let grid = Grid::unchecked(4, 4, 2_000.0, crate::grid::BoundaryKind::Rectangular);
        let mut dominant = Field::filled(grid, NO_MINERAL);
        let mut richness = Field::zeros(grid);
        let mut mask = Field::zeros(grid);
        // Two copper deposits (richness 0.9, 0.7) and one coal deposit (0.5).
        for (cell, id, value) in [
            (0usize, PORPHYRY_CU_AU, 0.9f32),
            (1, PORPHYRY_CU_AU, 0.7),
            (2, COAL, 0.5),
        ] {
            dominant.set_index(cell, id);
            richness.set_index(cell, value);
            mask.set_index(cell, 1);
        }
        dominant.set_index(3, 3);
        richness.set_index(3, 0.2);
        let mut p = Params::default();
        p.world.seed = 42;
        (
            MineralMap {
                dominant,
                richness,
                deposit_mask: mask,
            },
            grid,
            p,
        )
    }

    #[test]
    fn inventory_math_matches_the_contract() {
        let (map, grid, params) = synthetic_map();
        let inv = inventory(&map, grid, &params);
        assert_eq!(inv.grid_width, 4);
        assert_eq!(inv.grid_height, 4);
        assert!((inv.cell_area_km2 - 4.0).abs() < 1e-12);
        assert_eq!(inv.deposit_cells, 3);
        assert!((inv.deposit_area_km2 - 12.0).abs() < 1e-9);
        assert_eq!(inv.entries.len(), MINERALS.len());
        let total: f64 = inv.entries.iter().map(|e| e.value_usd).sum();
        assert!((inv.total_value_usd - total).abs() <= total.abs() * 1e-12 + 1e-9);
        // Copper entry: two cells, mean richness 0.8, area 8 km².
        let copper = &inv.entries[PORPHYRY_CU_AU as usize];
        assert_eq!(copper.cells, 2);
        assert!((copper.area_km2 - 8.0).abs() < 1e-9);
        assert!((copper.mean_richness - 0.8).abs() < 1e-6);
        assert!((copper.coverage - 2.0 / 16.0).abs() < 1e-12);
        assert!((copper.value_usd - copper.tonnage_t * copper.unit_value_usd_per_t).abs() < 1e-6);
        let expected =
            copper.area_km2 * MINERALS[PORPHYRY_CU_AU as usize].endowment_t_per_km2 * 0.8;
        let grade = copper.tonnage_t / expected.max(1e-12);
        assert!(
            (0.85..=1.15).contains(&grade),
            "grade factor {grade} out of range"
        );
        // Coal entry: one cell.
        let coal = &inv.entries[COAL as usize];
        assert_eq!(coal.cells, 1);
        assert!((coal.mean_richness - 0.5).abs() < 1e-6);
        // The non-deposit dominant cell contributes no tonnage.
        let graphite = &inv.entries[11];
        assert_eq!(graphite.cells, 0);
        assert_eq!(graphite.tonnage_t, 0.0);
        // Mean richness covers every cell: (0.9 + 0.7 + 0.5 + 0.2) / 16.
        assert!((inv.mean_richness - 2.3 / 16.0).abs() < 1e-5);
        assert!(inv.total_value_usd.is_finite() && inv.total_value_usd > 0.0);
        // Overlapping a smaller grid uses only the overlapping cells.
        let small = Grid::unchecked(2, 2, 2_000.0, crate::grid::BoundaryKind::Rectangular);
        let clipped = inventory(&map, small, &params);
        assert_eq!(clipped.deposit_cells, 3);
    }

    #[test]
    fn tectonic_activity_concentrates_gold_and_copper() {
        let mut p = small_params(5);
        p.world.width = 64;
        p.world.height = 64;
        p.normalize();
        let (mut world, climate, hydro, geology) = build(&p);
        // Half an active orogen, half a quiet craton; only this field changes.
        world
            .tectonic_rate_m_yr
            .map_in_place(|i, _| if i % 64 < 32 { 5.0e-3 } else { 0.0 });
        let active_gold =
            mineral_potential_map(OROGENIC_GOLD, &world, &p, &climate, &hydro, &geology);
        let active_copper =
            mineral_potential_map(PORPHYRY_CU_AU, &world, &p, &climate, &hydro, &geology);
        let mut flat = world.clone();
        flat.tectonic_rate_m_yr.fill_value(0.0);
        let quiet_gold =
            mineral_potential_map(OROGENIC_GOLD, &flat, &p, &climate, &hydro, &geology);
        let mean = |field: &Field<f32>, x0: usize, x1: usize| {
            let mut sum = 0.0;
            for y in 0..64 {
                for x in x0..x1 {
                    sum += f64::from(field.get(x, y));
                }
            }
            sum / (64.0 * (x1 - x0) as f64)
        };
        // Orogenic gold responds strongly to tectonic activity (0.42 weight).
        let quiet_half = mean(&quiet_gold, 0, 64);
        let active_half = mean(&active_gold, 0, 32);
        assert!(
            active_half > quiet_half + 0.02,
            "tectonic activity does not raise gold potential: {quiet_half} -> {active_half}"
        );
        assert!(
            mean(&active_gold, 0, 32) > mean(&active_gold, 32, 64),
            "active half is not richer in gold"
        );
        // Copper's tectonic weight is smaller but still visible.
        assert!(
            mean(&active_copper, 0, 32) > mean(&active_copper, 32, 64),
            "copper potential did not respond to the active margin"
        );
    }

    #[test]
    fn climate_controls_coal_and_evaporites() {
        let mut p = small_params(9);
        p.world.width = 64;
        p.world.height = 64;
        p.normalize();
        let (world, mut climate, hydro, geology) = build(&p);
        let coal =
            |climate: &Climate| mineral_potential_map(COAL, &world, &p, climate, &hydro, &geology);
        let evap = |climate: &Climate| {
            mineral_potential_map(EVAPORITE, &world, &p, climate, &hydro, &geology)
        };
        let mean = |field: &Field<f32>| {
            field.as_slice().iter().map(|&v| f64::from(v)).sum::<f64>() / field.len().max(1) as f64
        };
        let baseline_coal = mean(&coal(&climate));
        let baseline_evap = mean(&evap(&climate));
        // Wet world: high rainfall, low aridity, lush vegetation.
        climate.precipitation_mm.fill_value(3200.0);
        climate.aridity.fill_value(0.5);
        climate.vegetation.fill_value(0.9);
        let wet_coal = mean(&coal(&climate));
        let wet_evap = mean(&evap(&climate));
        assert!(
            wet_coal > baseline_coal,
            "rain and vegetation do not raise coal potential"
        );
        assert!(
            wet_coal > wet_evap,
            "wet world is richer in evaporites than coal"
        );
        // Dry world: low rainfall, high aridity.
        climate.precipitation_mm.fill_value(120.0);
        climate.aridity.fill_value(9.0);
        climate.vegetation.fill_value(0.0);
        let dry_evap = mean(&evap(&climate));
        let dry_coal = mean(&coal(&climate));
        assert!(
            dry_evap > baseline_evap,
            "aridity does not raise evaporite potential: {baseline_evap} -> {dry_evap}"
        );
        assert!(
            dry_evap > dry_coal,
            "dry world is richer in coal than evaporites"
        );
    }

    #[test]
    fn placers_follow_river_channels_and_discharge() {
        let mut p = small_params(17);
        p.world.width = 64;
        p.world.height = 64;
        p.normalize();
        let (mut world, climate, mut hydro, geology) = build(&p);
        // A major river corridor on the western half only.
        hydro.river_mask.fill_value(0);
        hydro.accumulation_cells.fill_value(1.0);
        hydro.discharge_m3_yr.fill_value(1.0e6);
        for y in 0..64 {
            for x in 0..32 {
                let i = world.grid.index(x, y);
                hydro.river_mask.set_index(i, 2);
                hydro.accumulation_cells.set_index(i, 400.0);
                hydro.discharge_m3_yr.set_index(i, 4.0e9);
            }
        }
        let placer = mineral_potential_map(12, &world, &p, &climate, &hydro, &geology);
        let mean = |x0: usize, x1: usize| {
            let mut sum = 0.0;
            for y in 0..64 {
                for x in x0..x1 {
                    sum += f64::from(placer.get(x, y));
                }
            }
            sum / (64.0 * (x1 - x0) as f64)
        };
        assert!(
            mean(0, 32) > mean(32, 64) * 1.3,
            "placers do not follow the river corridor: {} vs {}",
            mean(0, 32),
            mean(32, 64)
        );
        // Sediment supply also raises the placer potential on the same river.
        let before = mean(0, 32);
        world.sediment_m.fill_value(250.0);
        let richer = mineral_potential_map(12, &world, &p, &climate, &hydro, &geology);
        let after = {
            let mut sum = 0.0;
            for y in 0..64 {
                for x in 0..32 {
                    sum += f64::from(richer.get(x, y));
                }
            }
            sum / (64.0 * 32.0)
        };
        assert!(after > before, "sediment does not raise placer potential");
    }

    #[test]
    fn basins_and_tropics_raise_oil_and_bauxite() {
        let mut p = small_params(19);
        p.world.width = 64;
        p.world.height = 64;
        p.normalize();
        let (world, mut climate, hydro, geology) = build(&p);
        let mean = |field: &Field<f32>| {
            field.as_slice().iter().map(|&v| f64::from(v)).sum::<f64>() / field.len().max(1) as f64
        };
        // Thick sediment + organic host raises oil & gas potential.
        let mut basin = world.clone();
        basin.sediment_m.fill_value(280.0);
        let oil_plain = mean(&mineral_potential_map(
            OIL_GAS, &world, &p, &climate, &hydro, &geology,
        ));
        let oil_basin = mean(&mineral_potential_map(
            OIL_GAS, &basin, &p, &climate, &hydro, &geology,
        ));
        assert!(
            oil_basin > oil_plain,
            "sediment thickness does not raise oil potential: {oil_plain} -> {oil_basin}"
        );
        // Hot, wet tropics raise bauxite (laterite) potential.
        let cool_bauxite = mean(&mineral_potential_map(
            9, &world, &p, &climate, &hydro, &geology,
        ));
        climate.temperature_c.fill_value(27.0);
        climate.precipitation_mm.fill_value(3000.0);
        climate.aridity.fill_value(0.5);
        climate.vegetation.fill_value(0.9);
        let tropical_bauxite = mean(&mineral_potential_map(
            9, &world, &p, &climate, &hydro, &geology,
        ));
        assert!(
            tropical_bauxite > cool_bauxite * 1.2,
            "tropics do not raise bauxite potential: {cool_bauxite} -> {tropical_bauxite}"
        );
    }

    #[test]
    fn cratonic_deposits_prefer_thick_quiet_crust() {
        let mut p = small_params(29);
        p.world.width = 64;
        p.world.height = 64;
        p.normalize();
        let (world, climate, hydro, geology) = build(&p);
        let mean = |field: &Field<f32>| {
            field.as_slice().iter().map(|&v| f64::from(v)).sum::<f64>() / field.len().max(1) as f64
        };
        // A thick, quiet craton: BIF iron and diamonds should be richer than on
        // thin, active crust (the `cratonic` indicator).
        let mut craton = world.clone();
        craton.tectonic_rate_m_yr.fill_value(0.0);
        craton.crust_m.fill_value(45_000.0);
        let iron_craton = mean(&mineral_potential_map(
            3, &craton, &p, &climate, &hydro, &geology,
        ));
        let diamonds_craton = mean(&mineral_potential_map(
            8, &craton, &p, &climate, &hydro, &geology,
        ));
        let mut mobile = world.clone();
        mobile.tectonic_rate_m_yr.fill_value(5.0e-3);
        mobile.crust_m.fill_value(18_000.0);
        let iron_mobile = mean(&mineral_potential_map(
            3, &mobile, &p, &climate, &hydro, &geology,
        ));
        let diamonds_mobile = mean(&mineral_potential_map(
            8, &mobile, &p, &climate, &hydro, &geology,
        ));
        assert!(
            iron_craton > iron_mobile * 1.2,
            "BIF iron does not prefer cratons: {iron_craton} vs {iron_mobile}"
        );
        assert!(
            diamonds_craton > diamonds_mobile * 1.5,
            "diamonds do not prefer cratons: {diamonds_craton} vs {diamonds_mobile}"
        );
    }

    #[test]
    fn potential_map_agrees_with_the_dominant_field() {
        let p = small_params(23);
        let (world, climate, hydro, geology) = build(&p);
        let map = generate(&world, &p, &climate, &hydro, &geology);
        let mut checked = 0usize;
        for id in 0..MINERALS.len() as u8 {
            let potential = mineral_potential_map(id, &world, &p, &climate, &hydro, &geology);
            assert_eq!(potential.len(), map.dominant.len());
            for i in 0..map.dominant.len() {
                if map.dominant.get_index(i) == id {
                    let value = potential.get_index(i);
                    assert!(
                        (value - map.richness.get_index(i)).abs() <= 1e-6,
                        "potential field disagrees with dominant richness"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "no dominant cells to check");
    }

    #[test]
    fn preset_distributions_are_plausible() {
        // Wet river world: coal should dominate evaporites.
        let p = preset_params("riverworld");
        let (world, climate, hydro, geology) = build(&p);
        let map = generate(&world, &p, &climate, &hydro, &geology);
        let counts = |id: u8| map.dominant.as_slice().iter().filter(|&&d| d == id).count();
        let coal = counts(COAL);
        let evaporite = counts(EVAPORITE);
        assert!(coal > 0, "no coal in the river world");
        assert!(
            coal > evaporite,
            "river world: coal {coal} should beat evaporites {evaporite}"
        );
        assert!(map.deposit_mask.as_slice().contains(&1));

        // Arid plateau: evaporites should dominate coal.
        let p = preset_params("arid_plateau");
        let (world, climate, hydro, geology) = build(&p);
        let map = generate(&world, &p, &climate, &hydro, &geology);
        let counts = |id: u8| map.dominant.as_slice().iter().filter(|&&d| d == id).count();
        let coal = counts(COAL);
        let evaporite = counts(EVAPORITE);
        assert!(evaporite > 0, "no evaporites in the arid plateau");
        assert!(
            evaporite > coal,
            "arid plateau: evaporites {evaporite} should beat coal {coal}"
        );

        // Earth-like world: a varied, finite mineral map.
        let p = preset_params("earthlike");
        let (world, climate, hydro, geology) = build(&p);
        let map = generate(&world, &p, &climate, &hydro, &geology);
        let coverage = coverage_fractions(&map);
        assert!(
            coverage.len() >= 3,
            "earth-like world has too little variety"
        );
        for &v in map.richness.as_slice() {
            assert!(v.is_finite() && (0.0..=1.0).contains(&v));
        }
        let inv = inventory(&map, world.grid, &p);
        assert!(inv.deposit_cells > 0, "no economic deposits");
        assert!(
            inv.deposit_cells < world.cell_count(),
            "every cell is a deposit"
        );
        assert!(inv.total_value_usd.is_finite() && inv.total_value_usd > 0.0);
    }

    #[test]
    fn degenerate_worlds_do_not_panic() {
        // A flat, fully oceanic world: no land, no relief, no deposits.
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        p.world.seed = 1;
        p.normalize();
        let world = World::new(&p.world).expect("flat world");
        let grid = world.grid;
        let mosaic = PlateMosaic::build(&p, grid, 1);
        let geology = Geology::build(&p, &world, &mosaic, 1);
        let climate = climate::compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 1000.0,
        });
        let hydro = hydrology::route(&world, &p, Some(&climate));
        let map = generate(&world, &p, &climate, &hydro, &geology);
        assert_eq!(map.dominant.len(), grid.cell_count());
        for &id in map.dominant.as_slice() {
            assert!(id == NO_MINERAL || (id as usize) < MINERALS.len());
        }
        for &v in map.richness.as_slice() {
            assert!(v.is_finite());
        }
        let inv = inventory(&map, grid, &p);
        assert_eq!(inv.entries.len(), MINERALS.len());
        assert!(inv.total_value_usd.is_finite());
        for entry in &inv.entries {
            assert!(entry.tonnage_t.is_finite() && entry.value_usd.is_finite());
        }
        // A deep abyssal world is essentially barren: the deep-ocean penalty
        // keeps deposits rare even where the geology looks prospective.
        let mut deep = world.clone();
        deep.elevation_m.fill_value(-4_000.0);
        let deep_climate = climate::compute(&ClimateInputs {
            world: &deep,
            params: &p,
            previous: None,
            dt_yr: 1000.0,
        });
        let deep_hydro = hydrology::route(&deep, &p, Some(&deep_climate));
        let deep_map = generate(&deep, &p, &deep_climate, &deep_hydro, &geology);
        let deep_inv = inventory(&deep_map, grid, &p);
        for &v in deep_map.richness.as_slice() {
            assert!(v.is_finite() && (0.0..=1.0).contains(&v));
        }
        assert!(
            deep_inv.deposit_cells * 100 < grid.cell_count(),
            "abyssal world has {} deposits",
            deep_inv.deposit_cells
        );
        // Empty maps and mismatched grids must not panic either.
        let empty = MineralMap {
            dominant: Field::filled(grid, NO_MINERAL),
            richness: Field::zeros(grid),
            deposit_mask: Field::zeros(grid),
        };
        assert!(coverage_fractions(&empty).is_empty());
        let mismatched = Grid::unchecked(17, 19, 500.0, crate::grid::BoundaryKind::Toroidal);
        let inv = inventory(&empty, mismatched, &p);
        assert!(inv.total_value_usd.is_finite());
        assert!(
            mineral_potential_map(NO_MINERAL, &world, &p, &climate, &hydro, &geology)
                .as_slice()
                .iter()
                .all(|&v| v == 0.0)
        );
    }

    #[test]
    fn json_is_well_formed() {
        let p = small_params(2);
        let (world, climate, hydro, geology) = build(&p);
        let map = generate(&world, &p, &climate, &hydro, &geology);
        let inv = inventory(&map, world.grid, &p);
        let json = mineral_json(&map, &inv);
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(json.contains("\"minerals\":["));
        assert!(json.contains("\"inventory\":{"));
        for m in &MINERALS {
            assert!(json.contains(m.key), "JSON misses mineral {}", m.key);
        }
        assert!(json.contains("\"total_value_usd\":"));
        // Balanced braces/brackets outside of strings.
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
            assert!(depth >= 0, "unbalanced JSON");
        }
        assert_eq!(depth, 0);
        assert!(!in_string, "unterminated JSON string");
    }
}
