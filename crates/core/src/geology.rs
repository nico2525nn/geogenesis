//! Rocks, stratigraphy and erodibility (spec §4).
//!
//! Each cell belongs to a province (derived from the plate mosaic) with a
//! stratigraphic column. The surface lithology is not a decoration: it feeds
//! erodibility and solubility back into the surface processes, which is what
//! produces differential erosion — soft sedimentary basins are stripped faster
//! than crystalline shields, and an erosional unconformity lets a resistant
//! layer peek through.
//!
//! # Structure and exposure
//!
//! A column is a stack of layers from youngest (top) to oldest (bottom). Each
//! layer has a thickness, a dip and a strike. The structural dip tilts every
//! layer boundary: in the dip direction a boundary lies deeper below the
//! erosion surface, up-dip it lies shallower (possibly reaching the surface as
//! an inlier). The exposed layer at a cell therefore depends on both the
//! cumulative erosion and the cell's horizontal displacement from the province
//! centre projected on the dip direction:
//!
//! `boundary_depth = Σ thickness + clamp(along_dip · tan(dip), -0.6Σ, +0.6Σ)`.
//!
//! The clamp keeps the structure physically ordered (a boundary can never rise
//! above the one above it) even for steep dips on a 1000 km province. The same
//! rule gives the boundary elevations reported by
//! [`Stratigraphy::layer_boundary_elevation_m`].
//!
//! Rock templates follow the spec table (14 lithologies with hardness,
//! permeability, density, solubility and weathering rate). Provenance and
//! colours are included so the renderer can paint a geological map.
//!
//! # User-defined rocks and strata
//!
//! A [`RockSet`] can extend the built-in catalogue with user-defined rocks
//! (SPEC §4b) and a [`StrataSpec`] can replace the generated columns with an
//! explicit layer stack (SPEC §4c). Rock ids stay dense `u8` indices into the
//! set: built-ins keep their catalogue ids, customs are appended, and the
//! exposed-unit, erodibility, solubility and density fields are resolved
//! through the set in the same pass as before. Use [`Geology::build_with_rocks`]
//! or [`Geology::build_custom`] to create such a world and
//! [`Geology::apply_strata_spec`] to install a stack later.

use crate::error::{GeoError, GeoResult};
use crate::grid::{Field, Grid};
use crate::math::rng::{derive_seed, Rng};
use crate::math::Vec2;
use crate::params::Params;
use crate::plates::PlateMosaic;
use crate::rocks::{RockSet, RockUnit, StrataSpec, MAX_FAULT_OFFSET_M};
use crate::world::World;

/// Domain tag for stratigraphy.
const DOMAIN_STRATA: u64 = 0x7374_7261_7461_0001;

/// Maximum fraction of a cumulative depth by which structural dip may tilt a
/// layer boundary. Beyond this the simple tilted-layer model is no longer
/// physical (a boundary could cross the one above it).
const MAX_DIP_SHIFT_FRACTION: f64 = 0.6;

/// Origin class of a rock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RockOrigin {
    /// Crystallised from melt.
    Igneous,
    /// Deposited grain by grain.
    Sedimentary,
    /// Recrystallised under heat and pressure.
    Metamorphic,
}

/// A rock template from spec §4a.
#[derive(Clone, Copy, Debug)]
pub struct Rock {
    /// Stable key.
    pub key: &'static str,
    /// English name.
    pub name: &'static str,
    /// Japanese name.
    pub name_ja: &'static str,
    /// Mohs hardness.
    pub mohs: f64,
    /// Density, kg/m³.
    pub density_kg_m3: f64,
    /// Decimal logarithm of permeability in m².
    pub permeability_log_m2: f64,
    /// Chemical solubility, 0..1.
    pub solubility: f64,
    /// Physical weathering rate, m/yr.
    pub weathering_rate_m_yr: f64,
    /// Relative detachability (1 = average); higher erodes faster.
    pub erodibility: f64,
    /// Origin class.
    pub origin: RockOrigin,
    /// Display colour `[r, g, b]`.
    pub colour: [u8; 3],
}

/// The rock catalogue (spec §4a), in schema order.
pub const ROCKS: [Rock; 14] = [
    Rock {
        key: "granite",
        name: "Granite",
        name_ja: "花崗岩",
        mohs: 6.5,
        density_kg_m3: 2650.0,
        permeability_log_m2: -17.0,
        solubility: 0.03,
        weathering_rate_m_yr: 2e-5,
        erodibility: 0.55,
        origin: RockOrigin::Igneous,
        colour: [196, 176, 166],
    },
    Rock {
        key: "basalt",
        name: "Basalt",
        name_ja: "玄武岩",
        mohs: 6.0,
        density_kg_m3: 3000.0,
        permeability_log_m2: -15.0,
        solubility: 0.06,
        weathering_rate_m_yr: 3e-5,
        erodibility: 0.62,
        origin: RockOrigin::Igneous,
        colour: [90, 92, 100],
    },
    Rock {
        key: "limestone",
        name: "Limestone",
        name_ja: "石灰岩",
        mohs: 3.0,
        density_kg_m3: 2600.0,
        permeability_log_m2: -14.0,
        solubility: 0.85,
        weathering_rate_m_yr: 8e-5,
        erodibility: 0.95,
        origin: RockOrigin::Sedimentary,
        colour: [219, 213, 188],
    },
    Rock {
        key: "sandstone",
        name: "Sandstone",
        name_ja: "砂岩",
        mohs: 3.5,
        density_kg_m3: 2400.0,
        permeability_log_m2: -13.0,
        solubility: 0.18,
        weathering_rate_m_yr: 1.2e-4,
        erodibility: 1.05,
        origin: RockOrigin::Sedimentary,
        colour: [204, 165, 114],
    },
    Rock {
        key: "shale",
        name: "Shale",
        name_ja: "頁岩",
        mohs: 2.5,
        density_kg_m3: 2500.0,
        permeability_log_m2: -18.0,
        solubility: 0.12,
        weathering_rate_m_yr: 2e-4,
        erodibility: 1.35,
        origin: RockOrigin::Sedimentary,
        colour: [120, 120, 128],
    },
    Rock {
        key: "gneiss",
        name: "Gneiss",
        name_ja: "片麻岩",
        mohs: 7.0,
        density_kg_m3: 2750.0,
        permeability_log_m2: -18.0,
        solubility: 0.04,
        weathering_rate_m_yr: 1e-5,
        erodibility: 0.5,
        origin: RockOrigin::Metamorphic,
        colour: [170, 160, 150],
    },
    Rock {
        key: "schist",
        name: "Schist",
        name_ja: "片岩",
        mohs: 5.0,
        density_kg_m3: 2800.0,
        permeability_log_m2: -17.0,
        solubility: 0.07,
        weathering_rate_m_yr: 4e-5,
        erodibility: 0.85,
        origin: RockOrigin::Metamorphic,
        colour: [130, 140, 145],
    },
    Rock {
        key: "marble",
        name: "Marble",
        name_ja: "大理石",
        mohs: 4.0,
        density_kg_m3: 2700.0,
        permeability_log_m2: -16.0,
        solubility: 0.9,
        weathering_rate_m_yr: 6e-5,
        erodibility: 0.9,
        origin: RockOrigin::Metamorphic,
        colour: [230, 226, 220],
    },
    Rock {
        key: "slate",
        name: "Slate",
        name_ja: "粘板岩",
        mohs: 4.5,
        density_kg_m3: 2750.0,
        permeability_log_m2: -19.0,
        solubility: 0.05,
        weathering_rate_m_yr: 5e-5,
        erodibility: 0.8,
        origin: RockOrigin::Metamorphic,
        colour: [80, 88, 100],
    },
    Rock {
        key: "conglomerate",
        name: "Conglomerate",
        name_ja: "礫岩",
        mohs: 4.0,
        density_kg_m3: 2450.0,
        permeability_log_m2: -13.5,
        solubility: 0.15,
        weathering_rate_m_yr: 1.5e-4,
        erodibility: 1.1,
        origin: RockOrigin::Sedimentary,
        colour: [176, 150, 128],
    },
    Rock {
        key: "tuff",
        name: "Tuff",
        name_ja: "凝灰岩",
        mohs: 3.0,
        density_kg_m3: 2000.0,
        permeability_log_m2: -12.5,
        solubility: 0.3,
        weathering_rate_m_yr: 3e-4,
        erodibility: 1.4,
        origin: RockOrigin::Igneous,
        colour: [205, 190, 168],
    },
    Rock {
        key: "dolomite",
        name: "Dolomite",
        name_ja: "ドロマイト",
        mohs: 3.8,
        density_kg_m3: 2850.0,
        permeability_log_m2: -14.5,
        solubility: 0.6,
        weathering_rate_m_yr: 7e-5,
        erodibility: 0.88,
        origin: RockOrigin::Sedimentary,
        colour: [200, 195, 180],
    },
    Rock {
        key: "mudstone",
        name: "Mudstone",
        name_ja: "泥岩",
        mohs: 2.2,
        density_kg_m3: 2400.0,
        permeability_log_m2: -19.0,
        solubility: 0.1,
        weathering_rate_m_yr: 2.5e-4,
        erodibility: 1.45,
        origin: RockOrigin::Sedimentary,
        colour: [110, 105, 98],
    },
    Rock {
        key: "chert",
        name: "Chert",
        name_ja: "チャート",
        mohs: 7.0,
        density_kg_m3: 2600.0,
        permeability_log_m2: -20.0,
        solubility: 0.02,
        weathering_rate_m_yr: 5e-6,
        erodibility: 0.45,
        origin: RockOrigin::Sedimentary,
        colour: [160, 150, 140],
    },
];

/// Indices of the softer sedimentary rocks, used when building columns.
const SEDIMENTARY: [usize; 7] = [2, 3, 4, 9, 10, 12, 13];

/// Indices of hard crystalline basement rocks.
const CRYSTALLINE: [usize; 7] = [0, 1, 5, 6, 7, 8, 13];

/// Indices of the hardest shield-forming rocks (granite, gneiss, schist,
/// slate, chert, basalt).
const SHIELD_ROCKS: [usize; 6] = [0, 1, 5, 6, 8, 13];

/// One layer in a stratigraphic column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stratum {
    /// Dense rock id: index into the owning [`RockSet`] (built-in catalogue
    /// ids `0..14` keep their old meaning, custom rocks follow).
    pub rock: u8,
    /// Layer thickness, metres.
    pub thickness_m: f64,
    /// Regional dip, degrees.
    pub dip_deg: f64,
    /// Regional strike, degrees.
    pub strike_deg: f64,
    /// Vertical offset of a fault cutting this layer's base, metres; positive
    /// moves the base deeper. Defaults to zero for generated columns.
    pub fault_offset_m: f64,
    /// True when the base of this layer is an erosional unconformity; the
    /// overlying layer (this layer) is weakened where it is exposed.
    pub unconformity: bool,
    /// Approximate age, years (older is deeper).
    pub age_yr: f64,
}

/// A stratigraphic column for one province.
#[derive(Clone, Debug, PartialEq)]
pub struct Stratigraphy {
    /// Layers from youngest (index 0) to oldest.
    pub layers: Vec<Stratum>,
}

impl Stratigraphy {
    /// Total thickness, metres.
    #[must_use]
    pub fn total_thickness_m(&self) -> f64 {
        self.layers.iter().map(|layer| layer.thickness_m).sum()
    }

    /// Total structural offset of a layer boundary below its untilted depth:
    /// the dip shift plus the fault displacement, with the dip shift clamped so
    /// the structure stays ordered.
    #[must_use]
    pub fn boundary_offset_m(&self, layer: usize, along_dip_m: f64) -> Option<f64> {
        let stratum = self.layers.get(layer)?;
        let cumulative: f64 = self.layers[..layer].iter().map(|l| l.thickness_m).sum();
        let dip_shift = along_dip_m * stratum.dip_deg.to_radians().tan();
        let max_shift = MAX_DIP_SHIFT_FRACTION * cumulative;
        let fault_shift = stratum
            .fault_offset_m
            .clamp(-MAX_FAULT_OFFSET_M, MAX_FAULT_OFFSET_M);
        Some(dip_shift.clamp(-max_shift, max_shift) + fault_shift)
    }

    /// Depth of the top boundary of `layer` below the (untilted) erosion
    /// surface after a horizontal displacement of `along_dip_m` in the dip
    /// direction. `None` when the index is out of range.
    ///
    /// The structural shift is clamped to [`MAX_DIP_SHIFT_FRACTION`] of the
    /// cumulative depth so layer order is always preserved.
    #[must_use]
    pub fn boundary_depth_m(&self, layer: usize, along_dip_m: f64) -> Option<f64> {
        if layer >= self.layers.len() {
            return None;
        }
        if layer == 0 {
            return Some(0.0);
        }
        // Walk from the top so a strong fault/dip shift can never push one
        // boundary above the previous one: the same ordering rule the exposure
        // walk uses.
        let mut previous = 0.0f64;
        let mut cumulative = 0.0f64;
        for index in 1..=layer {
            cumulative += self.layers[index - 1].thickness_m;
            let offset = self.boundary_offset_m(index, along_dip_m)?;
            previous = (cumulative + offset).max(previous + 1e-9);
        }
        Some(previous)
    }

    /// Layer exposed at a cell after `eroded_m` of material has been removed
    /// and the erosion surface has moved `along_dip_m` metres down-dip from the
    /// province centre (negative up-dip). Equivalent to [`Self::layer_at_depth`]
    /// when `along_dip_m == 0`.
    #[must_use]
    pub fn layer_at_depth_tilted(&self, eroded_m: f64, along_dip_m: f64) -> usize {
        let eroded = eroded_m.max(0.0);
        let mut exposed = 0usize;
        let mut previous = 0.0f64;
        let mut cumulative = 0.0f64;
        for (i, layer) in self.layers.iter().enumerate().skip(1) {
            cumulative += self.layers[i - 1].thickness_m;
            let max_shift = MAX_DIP_SHIFT_FRACTION * cumulative;
            let dip_shift =
                (along_dip_m * layer.dip_deg.to_radians().tan()).clamp(-max_shift, max_shift);
            let fault_shift = layer
                .fault_offset_m
                .clamp(-MAX_FAULT_OFFSET_M, MAX_FAULT_OFFSET_M);
            let boundary = (cumulative + dip_shift + fault_shift).max(previous + 1e-9);
            if eroded >= boundary {
                exposed = i;
                previous = boundary;
            } else {
                break;
            }
        }
        exposed
    }

    /// Layer exposed after `eroded_m` of material has been removed (no
    /// structural tilt).
    #[must_use]
    pub fn layer_at_depth(&self, eroded_m: f64) -> usize {
        self.layer_at_depth_tilted(eroded_m, 0.0)
    }

    /// Elevation of the top boundary of `layer` beneath the current surface.
    ///
    /// `surface_m` is the present surface height and `eroded_m` the cumulative
    /// erosion, so `surface_m + eroded_m` reconstructs the reference surface
    /// the column was deposited against. `None` for an out-of-range index.
    #[must_use]
    pub fn layer_boundary_elevation_m(
        &self,
        layer: usize,
        surface_m: f64,
        eroded_m: f64,
        along_dip_m: f64,
    ) -> Option<f64> {
        let depth = self.boundary_depth_m(layer, along_dip_m)?;
        Some(surface_m + eroded_m.max(0.0) - depth)
    }
}

/// Geological state of the world: columns plus derived surface maps.
#[derive(Clone, Debug)]
pub struct Geology {
    /// One column per province.
    pub strata: Vec<Stratigraphy>,
    /// Province index per cell (usually the plate id).
    pub province: Field<u16>,
    /// Exposed rock index per cell.
    pub rock_id: Field<u8>,
    /// Relative erodibility multiplier per cell (1 = reference).
    pub erodibility: Field<f32>,
    /// Chemical solubility per cell (0..1).
    pub solubility: Field<f32>,
    /// Bulk density of the exposed rock, kg/m³.
    pub density_kg_m3: Field<f32>,
    /// Rock catalogue in effect: the 14 built-ins plus any user-defined rocks.
    pub rocks: RockSet,
    /// Horizontal reference point of each province in cell coordinates
    /// (centroid of its cells), used to compute the structural dip.
    pub province_origin: Vec<Vec2>,
}

impl Geology {
    /// Builds the initial stratigraphy from the plate provinces using the
    /// built-in rock catalogue.
    #[must_use]
    pub fn build(params: &Params, world: &World, mosaic: &PlateMosaic, seed: u64) -> Self {
        Self::build_with_rocks(params, world, mosaic, seed, &RockSet::builtin())
    }

    /// Builds the initial stratigraphy from the plate provinces using `rocks`
    /// as the available catalogue.
    ///
    /// Generated columns draw their lithologies from the whole set, so
    /// user-defined rocks can appear as exposed units; pass a [`StrataSpec`] to
    /// [`Self::build_custom`] to pin an explicit stack instead.
    #[must_use]
    pub fn build_with_rocks(
        params: &Params,
        world: &World,
        mosaic: &PlateMosaic,
        seed: u64,
        rocks: &RockSet,
    ) -> Self {
        let grid = world.grid;
        let provinces = mosaic.plates.len().max(1);
        let g = &params.geology;
        let pools = RockPools::new(rocks);
        let mut rng = Rng::new(derive_seed(seed, DOMAIN_STRATA));
        let columns: Vec<Stratigraphy> = (0..provinces)
            .map(|p| build_column(&mut rng, g, params.world.vertical_m, p, rocks, &pools))
            .collect();

        let mut province = Field::<u16>::zeros(grid);
        for (i, &pid) in mosaic.plate_id.as_slice().iter().enumerate() {
            province.set_index(i, pid.min(provinces as u16 - 1));
        }
        // Province centroids: reference points for the structural dip.
        let mut origins = vec![Vec2::zero(); provinces];
        let mut counts = vec![0usize; provinces];
        for (i, &pid) in province.as_slice().iter().enumerate() {
            let (x, y) = grid.coords(i);
            let p = (pid as usize).min(provinces - 1);
            origins[p].x += x as f64 + 0.5;
            origins[p].y += y as f64 + 0.5;
            counts[p] += 1;
        }
        for (p, origin) in origins.iter_mut().enumerate() {
            if counts[p] > 0 {
                *origin = Vec2::new(origin.x / counts[p] as f64, origin.y / counts[p] as f64);
            } else {
                origin.x = mosaic.plates[p].centre_cells.x;
                origin.y = mosaic.plates[p].centre_cells.y;
            }
        }

        let mut geology = Self {
            strata: columns,
            province,
            rock_id: Field::zeros(grid),
            erodibility: Field::filled(grid, 1.0),
            solubility: Field::zeros(grid),
            density_kg_m3: Field::filled(grid, 2_500.0),
            rocks: rocks.clone(),
            province_origin: origins,
        };
        geology.update_surface(world, params);
        geology
    }

    /// Builds a world with user-defined rocks and, optionally, an explicit
    /// stratigraphic stack (SPEC §4b/§4c).
    ///
    /// # Errors
    ///
    /// Propagates [`StrataSpec`] validation errors and out-of-range rock
    /// references.
    pub fn build_custom(
        params: &Params,
        world: &World,
        mosaic: &PlateMosaic,
        seed: u64,
        rocks: &RockSet,
        strata: Option<&StrataSpec>,
    ) -> GeoResult<Self> {
        let mut geology = Self::build_with_rocks(params, world, mosaic, seed, rocks);
        if let Some(spec) = strata {
            geology.apply_strata_spec(spec, world, params)?;
        }
        Ok(geology)
    }

    /// Replaces every province column with the layers of `spec` and refreshes
    /// the exposed-rock fields.
    ///
    /// # Errors
    ///
    /// Rejects specs that do not resolve against the current [`RockSet`].
    pub fn apply_strata_spec(
        &mut self,
        spec: &StrataSpec,
        world: &World,
        params: &Params,
    ) -> GeoResult<()> {
        let column = spec.to_stratigraphy(&self.rocks)?;
        for stored in &mut self.strata {
            *stored = column.clone();
        }
        self.update_surface(world, params);
        Ok(())
    }

    /// Swaps in a different rock set (e.g. after loading user-defined rocks).
    ///
    /// Existing columns must only reference ids that exist in the new set; call
    /// [`Self::apply_strata_spec`] or rebuild the world to change lithologies.
    ///
    /// # Errors
    ///
    /// Rejects a set that is smaller than a rock id already referenced by a
    /// column.
    pub fn set_rock_set(&mut self, rocks: RockSet) -> GeoResult<()> {
        let limit = rocks.len();
        for column in &self.strata {
            for layer in &column.layers {
                if (layer.rock as usize) >= limit {
                    return Err(GeoError::InvalidConfig(format!(
                        "stratum references rock id {} but the new rock set has only {limit} rocks",
                        layer.rock
                    )));
                }
            }
        }
        self.rocks = rocks;
        Ok(())
    }

    /// Recomputes exposed rock, erodibility, solubility and density after
    /// erosion.
    pub fn update_surface(&mut self, world: &World, params: &Params) {
        let grid = world.grid;
        let scale = params.geology.erodibility_scale.max(0.01);
        let cell_m = grid.cell_size_m();
        let reactivity = params.chemical.reactivity;
        let provinces = self.strata.len().max(1);
        // Per-province, per-layer structural gradients (d(depth)/dx, /dy).
        // Precomputed once so the hot loop only does two multiplies per layer.
        let tilts: Vec<Vec<(f64, f64)>> = self
            .strata
            .iter()
            .map(|column| {
                column
                    .layers
                    .iter()
                    .map(|layer| {
                        // Dip direction is perpendicular to strike.
                        let azimuth = (layer.strike_deg + 90.0).to_radians();
                        let slope = layer.dip_deg.to_radians().tan();
                        (azimuth.cos() * slope, azimuth.sin() * slope)
                    })
                    .collect()
            })
            .collect();

        let province = &self.province;
        let origins = &self.province_origin;
        let strata = &self.strata;
        let rocks = &self.rocks;
        let rock_id = &mut self.rock_id;
        let erodibility = &mut self.erodibility;
        let solubility = &mut self.solubility;
        let density = &mut self.density_kg_m3;

        for i in 0..grid.cell_count() {
            let (x, y) = grid.coords(i);
            let p = (province.get_index(i) as usize).min(provinces - 1);
            let column = &strata[p];
            let origin = origins[p];
            let dx = (x as f64 + 0.5 - origin.x) * cell_m;
            let dy = (y as f64 + 0.5 - origin.y) * cell_m;
            let eroded = f64::from(world.erosion_m.get_index(i)).max(0.0);

            // Walk the column using the tilted boundary depths.
            let mut exposed = 0usize;
            let mut previous = 0.0f64;
            let mut cumulative = 0.0f64;
            for layer_index in 1..column.layers.len() {
                cumulative += column.layers[layer_index - 1].thickness_m;
                let layer = &column.layers[layer_index];
                let (gx, gy) = tilts[p][layer_index];
                let max_shift = MAX_DIP_SHIFT_FRACTION * cumulative;
                let dip_shift = (dx * gx + dy * gy).clamp(-max_shift, max_shift);
                let fault_shift = layer
                    .fault_offset_m
                    .clamp(-MAX_FAULT_OFFSET_M, MAX_FAULT_OFFSET_M);
                let boundary = (cumulative + dip_shift + fault_shift).max(previous + 1e-9);
                if eroded >= boundary {
                    exposed = layer_index;
                    previous = boundary;
                } else {
                    break;
                }
            }
            let stratum = &column.layers[exposed];
            let rock = rocks.get_or_default(stratum.rock);
            rock_id.set_index(i, rock.id);
            // Erodibility combines lithology, the global scale and the
            // protective effect of a sediment/soil cover (covered bedrock
            // erodes as loose material, which is handled by the process
            // modules; here we expose bedrock resistance). Weathered
            // unconformity surfaces and their overlying basal layers are
            // weakened, which is what makes old erosion surfaces visible as
            // topographic steps.
            let mut value = rock.erodibility / scale;
            if stratum.unconformity {
                value *= 1.18;
            }
            // A little extra resistance for the hard crystalline shields keeps
            // the contrast between shields and sedimentary basins visible.
            if matches!(rock.origin, RockOrigin::Igneous | RockOrigin::Metamorphic) {
                value *= 0.94;
            }
            erodibility.set_index(i, value.clamp(0.05, 10.0) as f32);
            solubility.set_index(i, (rock.solubility * reactivity).clamp(0.0, 1.0) as f32);
            #[allow(clippy::cast_possible_truncation)]
            density.set_index(i, rock.density_kg_m3 as f32);
        }
    }

    /// Colour of the exposed rock at a cell.
    #[must_use]
    pub fn rock_colour_at(&self, x: usize, y: usize) -> [u8; 3] {
        self.rock_at(x, y).colour
    }

    /// The exposed rock unit at a cell.
    #[must_use]
    pub fn rock_at(&self, x: usize, y: usize) -> &RockUnit {
        self.rocks.get_or_default(self.rock_id.get(x, y))
    }
}

/// Lithology pools used when generating columns: the built-in soft/crystalline
/// sets plus any user-defined rocks, classified by origin and hardness.
struct RockPools {
    /// Soft sedimentary candidates (built-in [`SEDIMENTARY`] plus custom
    /// sedimentary rocks).
    sedimentary: Vec<u8>,
    /// Crystalline basement candidates.
    crystalline: Vec<u8>,
    /// Hard shield candidates (Mohs >= 6).
    shield: Vec<u8>,
}

impl RockPools {
    fn new(rocks: &RockSet) -> Self {
        let mut pools = Self {
            sedimentary: SEDIMENTARY.iter().map(|&index| index as u8).collect(),
            crystalline: CRYSTALLINE.iter().map(|&index| index as u8).collect(),
            shield: SHIELD_ROCKS.iter().map(|&index| index as u8).collect(),
        };
        for unit in rocks.units().iter().skip(crate::rocks::BUILTIN_ROCK_COUNT) {
            if unit.origin == RockOrigin::Sedimentary {
                pools.sedimentary.push(unit.id);
            } else {
                pools.crystalline.push(unit.id);
            }
            if unit.mohs >= 6.0 {
                pools.shield.push(unit.id);
            }
        }
        pools
    }
}

/// Builds one province column.
///
/// A province is either a crystalline shield (hard basement at the surface,
/// almost no sedimentary cover) or a sedimentary basin (soft shelf sequence
/// over a crystalline basement), controlled by `shield_fraction`. Sedimentary
/// layers become less likely with depth, so the bottom of every column is
/// crystalline basement. Candidate lithologies come from `rocks`, so custom
/// rocks can appear as generated units.
fn build_column(
    rng: &mut Rng,
    g: &crate::params::GeologyParams,
    vertical_m: f64,
    province: usize,
    rocks: &RockSet,
    pools: &RockPools,
) -> Stratigraphy {
    let layer_count = g.strata_layers.max(1) as usize;
    let mean_thickness = (vertical_m * 2.0 / layer_count as f64) * g.strata_thickness_scale;
    let shield = rng.chance(g.shield_fraction.clamp(0.0, 1.0));
    let mut layers = Vec::with_capacity(layer_count);
    let mut age = 0.0;
    for i in 0..layer_count {
        let depth_fraction = if layer_count > 1 {
            i as f64 / (layer_count - 1) as f64
        } else {
            0.0
        };
        let basement = i + 1 == layer_count;
        // Sedimentary cover thins downwards; shields barely have any.
        let sedimentary_chance = if shield {
            0.0
        } else {
            (g.sedimentary_fraction * (1.1 - 0.7 * depth_fraction)).clamp(0.0, 0.95)
        };
        let use_sedimentary = !basement && rng.chance(sedimentary_chance);
        let rock = if use_sedimentary {
            pools.sedimentary[rng.below(pools.sedimentary.len())]
        } else if shield {
            pools.shield[rng.below(pools.shield.len())]
        } else {
            choose_crystalline(
                rng,
                g.rock_diversity,
                province,
                i,
                rocks,
                &pools.crystalline,
            )
        };
        let thickness = (mean_thickness
            * (0.4 + 1.2 * rng.next_f64())
            * if use_sedimentary { 1.25 } else { 0.85 })
        .max(5.0);
        age += thickness / 1000.0; // ~1 m/kyr accumulation, order-of-magnitude
        let dip = (g.dip_deg + rng.normal_with(0.0, g.dip_variation_deg.max(0.0))).clamp(0.0, 60.0);
        layers.push(Stratum {
            rock,
            thickness_m: thickness,
            dip_deg: dip,
            strike_deg: (g.strike_deg + rng.normal_with(0.0, 10.0)).rem_euclid(360.0),
            fault_offset_m: 0.0,
            unconformity: i > 0 && rng.chance(g.unconformity_chance),
            age_yr: age * 1000.0,
        });
    }
    Stratigraphy { layers }
}

/// Picks a crystalline basement rock; diversity lets provinces differ.
///
/// The province/ layer hash gives each province a personality. Built-in worlds
/// keep the historic `min(13)` mapping; once custom rocks exist the hash spans
/// the whole set so user-defined lithologies can be drawn too.
fn choose_crystalline(
    rng: &mut Rng,
    diversity: f64,
    province: usize,
    layer: usize,
    rocks: &RockSet,
    crystalline: &[u8],
) -> u8 {
    let hash = (province as u64).wrapping_mul(2_654_435_761) + (layer as u64).wrapping_mul(40_503);
    let builtins = crate::rocks::BUILTIN_ROCK_COUNT;
    #[allow(clippy::cast_possible_truncation)]
    let candidate = if rocks.len() > builtins {
        (hash % rocks.len() as u64) as u8
    } else {
        // Historic mapping: built-in-only worlds keep the exact same
        // province/layer personality as before custom rocks existed.
        (hash % builtins as u64) as u8
    };
    if rng.chance(diversity)
        && !matches!(
            rocks.get_or_default(candidate).origin,
            RockOrigin::Sedimentary
        )
    {
        return candidate;
    }
    #[allow(clippy::cast_possible_truncation)]
    let fallback = rng.below(crystalline.len()) as u8;
    let position = fallback as usize;
    crystalline[position]
}

/// Number of provinces the geology can address (`u16` field bound).
#[must_use]
pub fn province_capacity() -> usize {
    65_535
}

/// Convenience: grid-level map from plate ids to province indices.
#[must_use]
pub fn province_field(grid: Grid, mosaic: &PlateMosaic) -> Field<u16> {
    let mut province = Field::<u16>::zeros(grid);
    for (i, &pid) in mosaic.plate_id.as_slice().iter().enumerate() {
        province.set_index(i, pid);
    }
    province
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::{self, ClimateInputs};
    use crate::hydrology;
    use crate::plates::PlateMosaic;
    use crate::rocks::{CustomRock, RockSelector, StrataLayer};

    fn setup() -> (Params, World, PlateMosaic) {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.geology.strata_layers = 5;
        let grid = p.world.grid().unwrap();
        let mosaic = PlateMosaic::build(&p, grid, 3);
        let world = World::new(&p.world).unwrap();
        (p, world, mosaic)
    }

    fn stratum(rock: usize, thickness: f64, dip: f64) -> Stratum {
        Stratum {
            rock: rock as u8,
            thickness_m: thickness,
            dip_deg: dip,
            strike_deg: 0.0,
            fault_offset_m: 0.0,
            unconformity: false,
            age_yr: 0.0,
        }
    }

    #[test]
    fn rock_catalogue_is_complete_and_unique() {
        assert_eq!(ROCKS.len(), 14);
        let mut keys: Vec<&str> = ROCKS.iter().map(|r| r.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 14);
        for rock in &ROCKS {
            assert!(rock.erodibility > 0.0);
            assert!(rock.density_kg_m3 > 1500.0);
            assert!((0.0..=1.0).contains(&rock.solubility));
        }
        // Soft sedimentary rocks really are more erodible than shields.
        let soft: f64 = SEDIMENTARY
            .iter()
            .map(|&i| ROCKS[i].erodibility)
            .sum::<f64>()
            / SEDIMENTARY.len() as f64;
        let hard: f64 = CRYSTALLINE
            .iter()
            .map(|&i| ROCKS[i].erodibility)
            .sum::<f64>()
            / CRYSTALLINE.len() as f64;
        assert!(
            soft > 1.4 * hard,
            "sedimentary rocks are not softer than crystalline ones: {soft} vs {hard}"
        );
    }

    #[test]
    fn strata_have_the_requested_layer_count_and_positive_thickness() {
        let (p, world, mosaic) = setup();
        let geo = Geology::build(&p, &world, &mosaic, 9);
        for column in &geo.strata {
            assert_eq!(column.layers.len(), p.geology.strata_layers as usize);
            assert!(column.total_thickness_m() > 0.0);
            // Every column ends in crystalline basement.
            let basement = column.layers.last().unwrap();
            assert!(!matches!(
                ROCKS[basement.rock as usize].origin,
                RockOrigin::Sedimentary
            ));
        }
    }

    #[test]
    fn erosion_exposes_deeper_layers() {
        let (p, world, mosaic) = setup();
        let mut geo = Geology::build(&p, &world, &mosaic, 9);
        let before = geo.rock_id.clone();
        let mut eroded_world = world.clone();
        let total: f64 = geo.strata[0].total_thickness_m();
        eroded_world.erosion_m.fill_value((total * 0.95) as f32);
        geo.update_surface(&eroded_world, &p);
        // Somewhere the exposed rock must have changed.
        let changed = before
            .as_slice()
            .iter()
            .zip(geo.rock_id.as_slice())
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed > 0, "no layer change after deep erosion");
    }

    #[test]
    fn erodibility_and_solubility_are_bounded() {
        let (p, world, mosaic) = setup();
        let geo = Geology::build(&p, &world, &mosaic, 9);
        for &e in geo.erodibility.as_slice() {
            assert!(e.is_finite() && e > 0.0 && e < 10.0);
        }
        for &s in geo.solubility.as_slice() {
            assert!((0.0..=1.0).contains(&s));
        }
    }

    #[test]
    fn layer_at_depth_walks_the_column() {
        let column = Stratigraphy {
            layers: vec![
                stratum(0, 100.0, 0.0),
                stratum(1, 50.0, 0.0),
                stratum(2, 25.0, 0.0),
            ],
        };
        assert_eq!(column.layer_at_depth(0.0), 0);
        assert_eq!(column.layer_at_depth(99.0), 0);
        assert_eq!(column.layer_at_depth(101.0), 1);
        assert_eq!(column.layer_at_depth(150.0), 2);
        assert_eq!(column.layer_at_depth(1e6), 2);
    }

    #[test]
    fn dip_controls_which_layer_is_exposed() {
        let column = Stratigraphy {
            layers: vec![
                stratum(3, 100.0, 10.0),
                stratum(4, 100.0, 10.0),
                stratum(0, 100.0, 10.0),
            ],
        };
        // Same erosion, different structural position: down-dip the boundary
        // is deeper so the younger layer remains; up-dip the older layer is
        // already at the surface.
        let eroded = 130.0;
        let down_dip = column.layer_at_depth_tilted(eroded, 3000.0);
        let flat = column.layer_at_depth_tilted(eroded, 0.0);
        let up_dip = column.layer_at_depth_tilted(eroded, -3000.0);
        assert!(
            up_dip >= flat && flat >= down_dip,
            "dip does not control exposure: up {up_dip}, flat {flat}, down {down_dip}"
        );
        assert!(up_dip > down_dip, "dip contrast is invisible");
    }

    #[test]
    fn layer_boundary_elevations_are_ordered_and_tilt() {
        let column = Stratigraphy {
            layers: vec![
                stratum(3, 200.0, 8.0),
                stratum(4, 150.0, 8.0),
                stratum(0, 120.0, 8.0),
            ],
        };
        let surface = 1200.0;
        let flat: Vec<f64> = (0..3)
            .map(|i| {
                column
                    .layer_boundary_elevation_m(i, surface, 0.0, 0.0)
                    .unwrap()
            })
            .collect();
        assert!((flat[0] - surface).abs() < 1e-9);
        assert!(
            flat[1] < flat[0] && flat[2] < flat[1],
            "boundaries out of order"
        );
        // Down-dip the layers are deeper; up-dip they are shallower.
        let down = column
            .layer_boundary_elevation_m(2, surface, 0.0, 4000.0)
            .unwrap();
        let up = column
            .layer_boundary_elevation_m(2, surface, 0.0, -4000.0)
            .unwrap();
        assert!(
            down < flat[2] && up > flat[2],
            "boundary elevation does not tilt"
        );
        // Reconstructed surface above an erosion surface.
        let eroded = column
            .layer_boundary_elevation_m(1, 1000.0, 100.0, 0.0)
            .unwrap();
        assert!((eroded - 900.0).abs() < 1e-9);
        assert!(column
            .layer_boundary_elevation_m(9, 0.0, 0.0, 0.0)
            .is_none());
    }

    #[test]
    fn shields_and_basins_have_contrasting_lithologies() {
        let (mut p, world, mosaic) = setup();
        p.geology.shield_fraction = 0.0;
        p.geology.rock_diversity = 0.0;
        let basin = Geology::build(&p, &world, &mosaic, 4);
        p.geology.shield_fraction = 1.0;
        let shield = Geology::build(&p, &world, &mosaic, 4);
        // Soft (argillaceous/calcareous) cover, excluding hard chert.
        const SOFT: [usize; 6] = [2, 3, 4, 9, 10, 12];
        let sedimentary_top = |geo: &Geology| {
            geo.strata
                .iter()
                .filter(|column| SOFT.contains(&(column.layers[0].rock as usize)))
                .count()
        };
        assert!(
            sedimentary_top(&basin) > 0,
            "no sedimentary basin provinces"
        );
        assert_eq!(
            sedimentary_top(&shield),
            0,
            "shield provinces have sedimentary tops"
        );
        let mean_erodibility = |geo: &Geology| {
            geo.erodibility
                .as_slice()
                .iter()
                .map(|&v| f64::from(v))
                .sum::<f64>()
                / geo.erodibility.len().max(1) as f64
        };
        let soft = mean_erodibility(&basin);
        let hard = mean_erodibility(&shield);
        assert!(
            soft > 1.3 * hard,
            "shield/basin erodibility contrast too weak: {soft} vs {hard}"
        );
        // And inside a basin world there is a strong cell-to-cell contrast.
        let min = basin.erodibility.min_value().unwrap();
        let max = basin.erodibility.max_value().unwrap();
        assert!(
            max > 1.8 * min,
            "no visible erodibility contrast: {min}..{max}"
        );
    }

    #[test]
    fn differential_erodibility_produces_differential_erosion() {
        // A uniform ramp is routed identically everywhere; soft and hard
        // provinces are interleaved as vertical stripes so both see the same
        // drainage-area distribution. The only difference is the lithology.
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        p.geology.strata_layers = 3;
        // Keep the stream-power coefficient in the regime where incision is
        // proportional to rock erodibility (not saturated by the per-step cap
        // or by the implicit solver), and isolate the lithology term by
        // disabling bank erosion.
        p.fluvial.kf = 5e-6;
        p.fluvial.max_incision_m = 5_000.0;
        p.fluvial.bank_erosion = 0.0;
        p.hydrology.channel_init_cells = 1;
        p.normalize();
        let grid = p.world.grid().unwrap();
        let mosaic = PlateMosaic::build(&p, grid, 2);
        let mut world = World::new(&p.world).unwrap();
        world
            .elevation_m
            .map_in_place(|i, _| 1400.0 - grid.coords(i).0 as f64 * 30.0);
        let mut geology = Geology::build(&p, &world, &mosaic, 2);
        geology.strata = vec![
            Stratigraphy {
                layers: vec![stratum(4, 500.0, 0.0), stratum(0, 500.0, 0.0)],
            },
            Stratigraphy {
                layers: vec![stratum(0, 500.0, 0.0), stratum(0, 500.0, 0.0)],
            },
        ];
        geology.province_origin = vec![Vec2::new(16.0, 16.0), Vec2::new(16.0, 16.0)];
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                geology.province.set(x, y, (x % 2) as u16);
            }
        }
        geology.update_surface(&world, &p);
        let climate = climate::compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 0.0,
        });
        let hydro = hydrology::route(&world, &p, Some(&climate));
        for _ in 0..8 {
            crate::erosion::fluvial::apply(&mut world, &p, &hydro, &climate, &geology, 10_000.0);
        }
        let mut sums = [0.0f64; 2];
        let mut counts = [0usize; 2];
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let p = x % 2;
                sums[p] += f64::from(world.erosion_m.get(x, y));
                counts[p] += 1;
            }
        }
        let soft = sums[0] / counts[0].max(1) as f64;
        let hard = sums[1] / counts[1].max(1) as f64;
        assert!(
            soft > 0.0 && hard > 0.0,
            "erosion did not run: {soft}, {hard}"
        );
        assert!(
            soft > 1.3 * hard,
            "soft and hard lithologies erode equally: soft {soft} m, hard {hard} m"
        );
    }

    #[test]
    fn unconformities_increase_erodibility() {
        let (mut p, world, mosaic) = setup();
        p.geology.strata_layers = 3;
        p.geology.unconformity_chance = 0.0;
        let mut geo = Geology::build(&p, &world, &mosaic, 6);
        let baseline = geo.erodibility.max_value().unwrap();
        let mut geo2 = geo.clone();
        for column in &mut geo2.strata {
            for layer in column.layers.iter_mut().skip(1) {
                layer.unconformity = true;
            }
        }
        // Expose the second layer everywhere so the unconformity flag matters.
        let thickest_top = geo
            .strata
            .iter()
            .map(|column| column.layers[0].thickness_m)
            .fold(0.0f64, f64::max);
        let mut eroded = world.clone();
        eroded.erosion_m.fill_value((thickest_top * 1.2) as f32);
        geo.update_surface(&eroded, &p);
        let without = geo.erodibility.max_value().unwrap();
        geo2.update_surface(&eroded, &p);
        let with = geo2.erodibility.max_value().unwrap();
        assert!(
            with > without,
            "unconformities do not weaken the exposed layer: {without} vs {with}"
        );
        // Sanity: the baseline build had the same fields.
        assert!((baseline - without).abs() < 1.0);
    }

    /// Two user-defined rocks: a very soft siltstone and a hard quartzite.
    fn custom_rocks() -> RockSet {
        let soft = CustomRock {
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
        };
        let hard = CustomRock {
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
        };
        let mut rocks = RockSet::builtin();
        rocks.push_custom(soft).unwrap();
        rocks.push_custom(hard).unwrap();
        rocks
    }

    fn custom_stratum(rock: u8, thickness: f64, dip: f64) -> Stratum {
        Stratum {
            rock,
            thickness_m: thickness,
            dip_deg: dip,
            strike_deg: 0.0,
            fault_offset_m: 0.0,
            unconformity: false,
            age_yr: 0.0,
        }
    }

    fn custom_spec() -> crate::rocks::StrataSpec {
        crate::rocks::StrataSpec::new(vec![
            StrataLayer {
                thickness_m: 150.0,
                rock: RockSelector::Id(14),
                ..StrataLayer::default()
            },
            StrataLayer {
                thickness_m: 400.0,
                rock: RockSelector::Id(15),
                ..StrataLayer::default()
            },
        ])
        .unwrap()
    }

    #[test]
    fn build_custom_with_rocks_is_deterministic() {
        let (p, world, mosaic) = setup();
        let rocks = custom_rocks();
        let spec = custom_spec();
        let a = Geology::build_custom(&p, &world, &mosaic, 11, &rocks, Some(&spec)).unwrap();
        let b = Geology::build_custom(&p, &world, &mosaic, 11, &rocks, Some(&spec)).unwrap();
        assert_eq!(a.rock_id.as_slice(), b.rock_id.as_slice());
        assert_eq!(a.erodibility.as_slice(), b.erodibility.as_slice());
        assert_eq!(a.solubility.as_slice(), b.solubility.as_slice());
        assert_eq!(a.density_kg_m3.as_slice(), b.density_kg_m3.as_slice());
        assert_eq!(a.strata, b.strata);
        // Every province carries the user-defined stack.
        for column in &a.strata {
            assert_eq!(column.layers.len(), 2);
            assert_eq!(column.layers[0].rock, 14);
            assert_eq!(column.layers[1].rock, 15);
        }
        assert!(a.rock_id.iter().all(|&id| id == 14));
    }

    #[test]
    fn generated_geology_is_deterministic() {
        let (p, world, mosaic) = setup();
        let a = Geology::build(&p, &world, &mosaic, 9);
        let b = Geology::build(&p, &world, &mosaic, 9);
        assert_eq!(a.rock_id.as_slice(), b.rock_id.as_slice());
        assert_eq!(a.erodibility.as_slice(), b.erodibility.as_slice());
        assert_eq!(a.solubility.as_slice(), b.solubility.as_slice());
        assert_eq!(a.density_kg_m3.as_slice(), b.density_kg_m3.as_slice());
        assert_eq!(a.strata, b.strata);
    }

    #[test]
    fn json_round_trip_builds_the_same_custom_world() {
        let (p, world, mosaic) = setup();
        let rocks_json = custom_rocks().to_json_pretty().unwrap();
        let spec_json = custom_spec().to_json_pretty().unwrap();
        let rocks = RockSet::from_json(&rocks_json).unwrap();
        let spec = crate::rocks::StrataSpec::from_json(&spec_json).unwrap();
        let first = Geology::build_custom(&p, &world, &mosaic, 4, &rocks, Some(&spec)).unwrap();
        // Round-trip the loaded config again: the world must be identical.
        let rocks2 = RockSet::from_json(&rocks.to_json().unwrap()).unwrap();
        let spec2 = crate::rocks::StrataSpec::from_json(&spec.to_json().unwrap()).unwrap();
        let second = Geology::build_custom(&p, &world, &mosaic, 4, &rocks2, Some(&spec2)).unwrap();
        assert_eq!(first.rock_id.as_slice(), second.rock_id.as_slice());
        assert_eq!(first.erodibility.as_slice(), second.erodibility.as_slice());
        assert_eq!(
            first.density_kg_m3.as_slice(),
            second.density_kg_m3.as_slice()
        );
        assert_eq!(first.strata, second.strata);
    }

    #[test]
    fn apply_strata_spec_replaces_generated_columns() {
        let (p, world, mosaic) = setup();
        let rocks = custom_rocks();
        let mut geology = Geology::build_with_rocks(&p, &world, &mosaic, 3, &rocks);
        geology
            .apply_strata_spec(&custom_spec(), &world, &p)
            .unwrap();
        for column in &geology.strata {
            assert_eq!(column.layers.len(), 2);
            assert_eq!(column.layers[0].rock, 14);
        }
        assert!(geology.rock_id.iter().all(|&id| id == 14));
        // A spec that references a rock the set does not contain is rejected.
        let mut smaller = RockSet::builtin();
        let mut only = CustomRock::new("Only");
        only.mohs = 4.0;
        only.permeability_m2 = 1e-15;
        only.friction_angle_deg = 30.0;
        only.cohesion_mpa = 4.0;
        only.density_kg_m3 = 2_500.0;
        smaller.push_custom(only).unwrap();
        let mut geology2 = Geology::build_with_rocks(&p, &world, &mosaic, 3, &smaller);
        assert!(geology2
            .apply_strata_spec(&custom_spec(), &world, &p)
            .is_err());
        // Shrinking the set below an id a column already uses is rejected.
        let mut geology3 =
            Geology::build_custom(&p, &world, &mosaic, 3, &rocks, Some(&custom_spec())).unwrap();
        assert!(geology3.set_rock_set(smaller).is_err());
    }

    #[test]
    fn custom_soft_rock_erodes_faster_than_hard_rock() {
        // Vertical stripes of the same ramp: identical routing, the only
        // difference is the exposed custom lithology.
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        p.geology.strata_layers = 3;
        p.fluvial.kf = 5e-6;
        p.fluvial.max_incision_m = 5_000.0;
        p.fluvial.bank_erosion = 0.0;
        p.hydrology.channel_init_cells = 1;
        p.normalize();
        let grid = p.world.grid().unwrap();
        let rocks = custom_rocks();
        let mosaic = PlateMosaic::build(&p, grid, 2);
        let mut world = World::new(&p.world).unwrap();
        world
            .elevation_m
            .map_in_place(|i, _| 1400.0 - grid.coords(i).0 as f64 * 30.0);
        let mut geology = Geology::build_with_rocks(&p, &world, &mosaic, 2, &rocks);
        geology.strata = vec![
            Stratigraphy {
                layers: vec![
                    custom_stratum(14, 500.0, 0.0),
                    custom_stratum(15, 500.0, 0.0),
                ],
            },
            Stratigraphy {
                layers: vec![
                    custom_stratum(15, 500.0, 0.0),
                    custom_stratum(15, 500.0, 0.0),
                ],
            },
        ];
        geology.province_origin = vec![Vec2::new(16.0, 16.0), Vec2::new(16.0, 16.0)];
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                geology.province.set(x, y, (x % 2) as u16);
            }
        }
        geology.update_surface(&world, &p);
        let soft_erodibility = f64::from(geology.erodibility.get(0, 0));
        let hard_erodibility = f64::from(geology.erodibility.get(1, 0));
        assert!(
            soft_erodibility > 1.3 * hard_erodibility,
            "custom erodibility contrast missing: {soft_erodibility} vs {hard_erodibility}"
        );
        let climate = climate::compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 0.0,
        });
        let hydro = hydrology::route(&world, &p, Some(&climate));
        for _ in 0..8 {
            let _ = crate::erosion::fluvial::apply(
                &mut world, &p, &hydro, &climate, &geology, 10_000.0,
            );
        }
        let mut sums = [0.0f64; 2];
        let mut counts = [0usize; 2];
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let stripe = x % 2;
                sums[stripe] += f64::from(world.erosion_m.get(x, y));
                counts[stripe] += 1;
            }
        }
        let soft = sums[0] / counts[0].max(1) as f64;
        let hard = sums[1] / counts[1].max(1) as f64;
        assert!(soft > 0.0 && hard > 0.0, "erosion did not run");
        assert!(
            soft > 1.3 * hard,
            "custom soft rock does not erode faster: soft {soft} m, hard {hard} m"
        );
    }

    #[test]
    fn unconformity_weakens_overlying_custom_layer() {
        let (p, world, mosaic) = setup();
        let rocks = custom_rocks();
        let spec = |unconformity: bool| {
            crate::rocks::StrataSpec::new(vec![
                StrataLayer {
                    thickness_m: 50.0,
                    rock: RockSelector::Id(14),
                    ..StrataLayer::default()
                },
                StrataLayer {
                    thickness_m: 400.0,
                    rock: RockSelector::Id(15),
                    unconformity,
                    ..StrataLayer::default()
                },
            ])
            .unwrap()
        };
        // Erode through the top layer so the boundary-bearing layer is exposed.
        let mut eroded = world.clone();
        eroded.erosion_m.fill_value(120.0);
        let mut without =
            Geology::build_custom(&p, &world, &mosaic, 5, &rocks, Some(&spec(false))).unwrap();
        without.update_surface(&eroded, &p);
        let mut with =
            Geology::build_custom(&p, &world, &mosaic, 5, &rocks, Some(&spec(true))).unwrap();
        with.update_surface(&eroded, &p);
        assert!(without.rock_id.iter().all(|&id| id == 15));
        assert!(with.rock_id.iter().all(|&id| id == 15));
        let mean_without = without.erodibility.mean().unwrap();
        let mean_with = with.erodibility.mean().unwrap();
        assert!(
            mean_with > mean_without * 1.1,
            "unconformity does not weaken the overlying layer: {mean_without} vs {mean_with}"
        );
    }

    #[test]
    fn generated_columns_can_expose_custom_rocks() {
        let (mut p, world, mosaic) = setup();
        p.geology.shield_fraction = 0.0;
        p.geology.rock_diversity = 1.0;
        let rocks = custom_rocks();
        let mut exposed_custom = false;
        for seed in 5..12u64 {
            let geology = Geology::build_with_rocks(&p, &world, &mosaic, seed, &rocks);
            for &id in geology.rock_id.as_slice() {
                assert!((id as usize) < rocks.len());
                if id as usize >= crate::rocks::BUILTIN_ROCK_COUNT {
                    exposed_custom = true;
                    assert!(!geology.rocks.get(id).unwrap().builtin);
                }
            }
        }
        assert!(
            exposed_custom,
            "custom rocks never appeared in generated columns"
        );
    }

    #[test]
    fn density_and_colour_follow_the_exposed_rock() {
        let (p, world, mosaic) = setup();
        let geology = Geology::build(&p, &world, &mosaic, 2);
        for i in 0..world.cell_count() {
            let unit = geology.rocks.get(geology.rock_id.get_index(i)).unwrap();
            assert!(
                (f64::from(geology.density_kg_m3.get_index(i)) - unit.density_kg_m3).abs() < 1.0
            );
            assert!(
                (f64::from(geology.solubility.get_index(i))
                    - unit.solubility * p.chemical.reactivity)
                    .abs()
                    < 1e-5
            );
            let (x, y) = world.grid.coords(i);
            assert_eq!(geology.rock_colour_at(x, y), unit.colour);
        }
    }

    #[test]
    fn fault_offsets_shift_boundaries_without_inverting_layers() {
        let rocks = RockSet::builtin();
        let spec = crate::rocks::StrataSpec::new(vec![
            StrataLayer::builtin("sandstone", 100.0),
            StrataLayer {
                thickness_m: 100.0,
                rock: RockSelector::Builtin("limestone".into()),
                fault_offset_m: 150.0,
                ..StrataLayer::default()
            },
            StrataLayer {
                thickness_m: 100.0,
                rock: RockSelector::Builtin("shale".into()),
                fault_offset_m: -50.0,
                ..StrataLayer::default()
            },
        ])
        .unwrap();
        let column = spec.to_stratigraphy(&rocks).unwrap();
        let first = column.boundary_depth_m(1, 0.0).unwrap();
        assert!(
            (first - 250.0).abs() < 1e-9,
            "fault offset not applied: {first}"
        );
        let second = column.boundary_depth_m(2, 0.0).unwrap();
        assert!(second > first, "fault offset inverted the stack");
        // Extreme structural positions still keep the order.
        for along_dip in [-1e6, 0.0, 1e6] {
            let mut previous = 0.0;
            for layer in 1..column.layers.len() {
                let depth = column.boundary_depth_m(layer, along_dip).unwrap();
                assert!(depth > previous, "layer {layer} boundary inverted");
                previous = depth;
            }
        }
    }
}
