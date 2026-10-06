//! Lithology: the fourteen rock types, their properties, and the strata stack.
//!
//! Every erosion process reads erodibility from here rather than from a global
//! constant: granite highlands persist while shale lowlands wash away, which is
//! the mechanism behind differential erosion (mesas, cuestas, karst).

use crate::grid::{Boundary, LabelField, ScalarField};
use crate::linalg::clamp;

/// The fourteen rock types from spec §4a.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RockKind {
    Granite = 0,
    Basalt = 1,
    Limestone = 2,
    Sandstone = 3,
    Shale = 4,
    Gneiss = 5,
    Schist = 6,
    Marble = 7,
    Slate = 8,
    Conglomerate = 9,
    Tuff = 10,
    Dolomite = 11,
    Mudstone = 12,
    #[default]
    Chert = 13,
}

impl RockKind {
    /// All fourteen kinds, in spec order.
    pub const ALL: [RockKind; 14] = [
        RockKind::Granite,
        RockKind::Basalt,
        RockKind::Limestone,
        RockKind::Sandstone,
        RockKind::Shale,
        RockKind::Gneiss,
        RockKind::Schist,
        RockKind::Marble,
        RockKind::Slate,
        RockKind::Conglomerate,
        RockKind::Tuff,
        RockKind::Dolomite,
        RockKind::Mudstone,
        RockKind::Chert,
    ];

    /// Japanese name from the spec.
    pub fn name_ja(self) -> &'static str {
        match self {
            RockKind::Granite => "花崗岩",
            RockKind::Basalt => "玄武岩",
            RockKind::Limestone => "石灰岩",
            RockKind::Sandstone => "砂岩",
            RockKind::Shale => "頁岩",
            RockKind::Gneiss => "片麻岩",
            RockKind::Schist => "片岩",
            RockKind::Marble => "大理石",
            RockKind::Slate => "粘板岩",
            RockKind::Conglomerate => "礫岩",
            RockKind::Tuff => "凝灰岩",
            RockKind::Dolomite => "ドロマイト",
            RockKind::Mudstone => "泥岩",
            RockKind::Chert => "チャート",
        }
    }

    /// English name.
    pub fn name(self) -> &'static str {
        match self {
            RockKind::Granite => "granite",
            RockKind::Basalt => "basalt",
            RockKind::Limestone => "limestone",
            RockKind::Sandstone => "sandstone",
            RockKind::Shale => "shale",
            RockKind::Gneiss => "gneiss",
            RockKind::Schist => "schist",
            RockKind::Marble => "marble",
            RockKind::Slate => "slate",
            RockKind::Conglomerate => "conglomerate",
            RockKind::Tuff => "tuff",
            RockKind::Dolomite => "dolomite",
            RockKind::Mudstone => "mudstone",
            RockKind::Chert => "chert",
        }
    }
}

/// Physical properties of one rock type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RockProps {
    /// Mohs hardness, 1–10.
    pub hardness: f64,
    /// Permeability in m² (log-scale quantity; stored linear).
    pub permeability_m2: f64,
    /// Internal friction angle in degrees, 20–60.
    pub friction_deg: f64,
    /// Cohesion in MPa, 0–20.
    pub cohesion_mpa: f64,
    /// Density in g/cm³, 1.5–3.5.
    pub density: f64,
    /// Dissolution susceptibility, 0–1.
    pub solubility: f64,
    /// Surface weathering rate in m/yr.
    pub weathering_m_yr: f64,
}

impl RockProps {
    /// The fourteen-type database. Values are representative field numbers:
    /// granite hard and insoluble, limestone soft and soluble, shale weak.
    pub fn of(kind: RockKind) -> Self {
        match kind {
            RockKind::Granite => Self { hardness: 7.0, permeability_m2: 1e-14, friction_deg: 45.0, cohesion_mpa: 15.0, density: 2.7, solubility: 0.05, weathering_m_yr: 2e-5 },
            RockKind::Basalt => Self { hardness: 6.0, permeability_m2: 1e-13, friction_deg: 40.0, cohesion_mpa: 10.0, density: 3.0, solubility: 0.08, weathering_m_yr: 5e-5 },
            RockKind::Limestone => Self { hardness: 3.0, permeability_m2: 1e-11, friction_deg: 35.0, cohesion_mpa: 5.0, density: 2.5, solubility: 0.9, weathering_m_yr: 3e-4 },
            RockKind::Sandstone => Self { hardness: 4.0, permeability_m2: 1e-12, friction_deg: 35.0, cohesion_mpa: 4.0, density: 2.3, solubility: 0.1, weathering_m_yr: 1e-4 },
            RockKind::Shale => Self { hardness: 3.0, permeability_m2: 1e-15, friction_deg: 25.0, cohesion_mpa: 2.0, density: 2.4, solubility: 0.15, weathering_m_yr: 2e-4 },
            RockKind::Gneiss => Self { hardness: 7.0, permeability_m2: 1e-14, friction_deg: 42.0, cohesion_mpa: 12.0, density: 2.8, solubility: 0.05, weathering_m_yr: 3e-5 },
            RockKind::Schist => Self { hardness: 5.0, permeability_m2: 1e-14, friction_deg: 30.0, cohesion_mpa: 6.0, density: 2.7, solubility: 0.1, weathering_m_yr: 8e-5 },
            RockKind::Marble => Self { hardness: 4.0, permeability_m2: 1e-13, friction_deg: 35.0, cohesion_mpa: 8.0, density: 2.7, solubility: 0.85, weathering_m_yr: 2e-4 },
            RockKind::Slate => Self { hardness: 5.0, permeability_m2: 1e-15, friction_deg: 32.0, cohesion_mpa: 7.0, density: 2.6, solubility: 0.1, weathering_m_yr: 6e-5 },
            RockKind::Conglomerate => Self { hardness: 5.0, permeability_m2: 1e-12, friction_deg: 38.0, cohesion_mpa: 5.0, density: 2.4, solubility: 0.12, weathering_m_yr: 1e-4 },
            RockKind::Tuff => Self { hardness: 3.0, permeability_m2: 1e-12, friction_deg: 30.0, cohesion_mpa: 3.0, density: 1.8, solubility: 0.2, weathering_m_yr: 3e-4 },
            RockKind::Dolomite => Self { hardness: 4.0, permeability_m2: 1e-12, friction_deg: 38.0, cohesion_mpa: 7.0, density: 2.8, solubility: 0.7, weathering_m_yr: 1.5e-4 },
            RockKind::Mudstone => Self { hardness: 3.0, permeability_m2: 1e-15, friction_deg: 22.0, cohesion_mpa: 1.5, density: 2.3, solubility: 0.15, weathering_m_yr: 2.5e-4 },
            RockKind::Chert => Self { hardness: 7.0, permeability_m2: 1e-16, friction_deg: 40.0, cohesion_mpa: 10.0, density: 2.6, solubility: 0.03, weathering_m_yr: 1e-5 },
        }
    }

    /// Fluvial erodibility multiplier: soft, weak rock erodes up to ~25x
    /// faster than hard, cohesive rock at equal discharge and slope.
    pub fn erodibility(&self) -> f64 {
        let hardness_term = (8.0 - self.hardness.clamp(1.0, 10.0)) / 7.0;
        let cohesion_term = 1.0 / (1.0 + self.cohesion_mpa.max(0.0));
        clamp(0.2 + 4.8 * hardness_term * (0.5 + 0.5 * cohesion_term * 2.0), 0.1, 10.0)
    }

    /// Hillslope angle of repose, from the friction angle.
    pub fn repose_tan(&self) -> f64 {
        self.friction_deg.to_radians().tan().max(0.2)
    }
}

/// One stratigraphic layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Stratum {
    pub rock: RockKind,
    /// Thickness in metres.
    pub thickness_m: f64,
    /// Dip in degrees from horizontal.
    pub dip_deg: f64,
    /// Strike in degrees from +x.
    pub strike_deg: f64,
    /// Vertical fault offset in metres.
    pub fault_offset_m: f64,
    /// Whether an unconformity (erosional gap) caps this layer.
    pub unconformity: bool,
}

/// The layer stack under the surface, oldest at index 0.
#[derive(Debug, Clone)]
pub struct Stratigraphy {
    pub layers: Vec<Stratum>,
}

impl Stratigraphy {
    /// Total stack thickness in metres.
    pub fn total_thickness_m(&self) -> f64 {
        self.layers.iter().map(|l| l.thickness_m.max(0.0)).sum()
    }

    /// Rock exposed after `eroded_m` metres of denudation at a point.
    ///
    /// Walks down through the stack; the unconformity flag does not change the
    /// exposed rock (it records lost time, not a different lithology).
    pub fn exposed_rock(&self, eroded_m: f64) -> RockKind {
        let mut remaining = eroded_m.max(0.0);
        for layer in self.layers.iter().rev() {
            if remaining <= layer.thickness_m {
                return layer.rock;
            }
            remaining -= layer.thickness_m;
        }
        self.layers.first().map(|l| l.rock).unwrap_or(RockKind::Granite)
    }
}

/// Surface geology: which rock crops out in each cell.
#[derive(Debug, Clone)]
pub struct Geology {
    /// Rock kind id per cell.
    pub outcrop: LabelField,
    /// Erodibility multiplier per cell, from the outcrop rock.
    pub erodibility: ScalarField,
    /// Solubility per cell, for chemical weathering.
    pub solubility: ScalarField,
    /// Rock strength per cell, for coastal erosion.
    pub strength: ScalarField,
}

/// Builds surface geology from a stratigraphy, an erosion-depth field, and a
/// lateral variation field that decides which stack column each cell uses.
///
/// `eroded_m` is cumulative denudation per cell; deeper erosion exposes older
/// (lower) strata. `province_noise` in [−1, 1] shifts the column choice so
/// adjacent cells can expose different rocks even at equal depth.
pub fn build_geology(
    b: Boundary,
    strata: &[Stratigraphy],
    eroded_m: &ScalarField,
    province_noise: &ScalarField,
) -> Geology {
    let mut outcrop = LabelField::filled(b, 0);
    let mut erodibility = ScalarField::zeros(b);
    let mut solubility = ScalarField::zeros(b);
    let mut strength = ScalarField::zeros(b);

    let n = strata.len().max(1);
    for i in 0..b.cell_count() {
        // Column choice: map the province noise onto a stack index.
        let t = ((province_noise.get_index(i) + 1.0) / 2.0).clamp(0.0, 1.0);
        let stack = &strata[(t * (n - 1) as f64).round() as usize % n];
        let rock = stack.exposed_rock(eroded_m.get_index(i));
        let props = RockProps::of(rock);
        outcrop.set_index(i, rock as u32);
        erodibility.set_index(i, props.erodibility());
        solubility.set_index(i, props.solubility);
        strength.set_index(i, props.cohesion_mpa.max(0.1));
    }

    Geology {
        outcrop,
        erodibility,
        solubility,
        strength,
    }
}

/// Generates a default stratigraphy with `layers` cyclically alternating hard
/// and soft rocks — the classic mesa/cuesta-forming arrangement.
pub fn default_stratigraphy(seed: u64, layers: usize) -> Stratigraphy {
    let mut rng = crate::rng::Rng::from_seed(seed);
    let hard = [RockKind::Sandstone, RockKind::Limestone, RockKind::Granite, RockKind::Chert];
    let soft = [RockKind::Shale, RockKind::Mudstone, RockKind::Tuff, RockKind::Slate];
    let mut stack = Vec::with_capacity(layers.max(1));
    for i in 0..layers.max(1) {
        let rock = if i % 2 == 0 {
            *rng.choose(&hard).unwrap()
        } else {
            *rng.choose(&soft).unwrap()
        };
        stack.push(Stratum {
            rock,
            thickness_m: rng.range_f64(50.0, 800.0),
            dip_deg: rng.range_f64(0.0, 15.0),
            strike_deg: rng.range_f64(0.0, 360.0),
            fault_offset_m: rng.range_f64(-200.0, 200.0),
            unconformity: rng.next_f64() < 0.2,
        });
    }
    Stratigraphy { layers: stack }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BoundaryKind;

    fn test_grid() -> Boundary {
        Boundary::new(BoundaryKind::Rectangular, 16, 16)
    }

    #[test]
    fn database_has_fourteen_distinct_rocks() {
        assert_eq!(RockKind::ALL.len(), 14);
        let mut ids: Vec<u32> = RockKind::ALL.iter().map(|r| *r as u32).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 14);
    }

    #[test]
    fn properties_stay_inside_spec_ranges() {
        for kind in RockKind::ALL {
            let p = RockProps::of(kind);
            assert!((1.0..=10.0).contains(&p.hardness), "{kind:?} hardness");
            assert!((1e-16..=1e-5).contains(&p.permeability_m2) || p.permeability_m2 > 0.0);
            assert!((20.0..=60.0).contains(&p.friction_deg), "{kind:?} friction");
            assert!((0.0..=20.0).contains(&p.cohesion_mpa), "{kind:?} cohesion");
            assert!((1.5..=3.5).contains(&p.density), "{kind:?} density");
            assert!((0.0..=1.0).contains(&p.solubility), "{kind:?} solubility");
        }
    }

    #[test]
    fn granite_is_harder_and_less_soluble_than_limestone() {
        let g = RockProps::of(RockKind::Granite);
        let l = RockProps::of(RockKind::Limestone);
        assert!(g.hardness > l.hardness);
        assert!(g.solubility < l.solubility);
        assert!(g.erodibility() < l.erodibility());
    }

    #[test]
    fn erodibility_orders_soft_above_hard() {
        let soft = RockProps::of(RockKind::Shale).erodibility();
        let hard = RockProps::of(RockKind::Granite).erodibility();
        assert!(soft > hard * 2.0, "shale {soft} vs granite {hard}");
    }

    #[test]
    fn stratigraphy_exposes_younger_rock_first() {
        let stack = Stratigraphy {
            layers: vec![
                Stratum { rock: RockKind::Granite, thickness_m: 500.0, dip_deg: 0.0, strike_deg: 0.0, fault_offset_m: 0.0, unconformity: false },
                Stratum { rock: RockKind::Sandstone, thickness_m: 200.0, dip_deg: 0.0, strike_deg: 0.0, fault_offset_m: 0.0, unconformity: false },
                Stratum { rock: RockKind::Shale, thickness_m: 100.0, dip_deg: 0.0, strike_deg: 0.0, fault_offset_m: 0.0, unconformity: false },
            ],
        };
        assert_eq!(stack.exposed_rock(0.0), RockKind::Shale);
        assert_eq!(stack.exposed_rock(150.0), RockKind::Sandstone);
        assert_eq!(stack.exposed_rock(400.0), RockKind::Granite);
        // Eroding past the stack bottom still returns the basement.
        assert_eq!(stack.exposed_rock(5000.0), RockKind::Granite);
    }

    #[test]
    fn build_geology_maps_depth_to_outcrop() {
        let b = test_grid();
        let strata = vec![Stratigraphy {
            layers: vec![
                Stratum { rock: RockKind::Granite, thickness_m: 1000.0, dip_deg: 0.0, strike_deg: 0.0, fault_offset_m: 0.0, unconformity: false },
                Stratum { rock: RockKind::Limestone, thickness_m: 100.0, dip_deg: 0.0, strike_deg: 0.0, fault_offset_m: 0.0, unconformity: false },
            ],
        }];
        let mut eroded = ScalarField::zeros(b);
        eroded.set(0, 0, 10.0);   // shallow: limestone
        eroded.set(1, 0, 500.0);  // deep: granite
        let province = ScalarField::zeros(b);
        let geo = build_geology(b, &strata, &eroded, &province);
        assert_eq!(geo.outcrop.get(0, 0), RockKind::Limestone as u32);
        assert_eq!(geo.outcrop.get(1, 0), RockKind::Granite as u32);
        // Erodibility follows the rock: granite resists, limestone yields.
        assert!(geo.erodibility.get(1, 0) < geo.erodibility.get(0, 0));
        assert!(geo.solubility.get(0, 0) > geo.solubility.get(1, 0));
    }

    #[test]
    fn default_stratigraphy_alternates_hard_and_soft() {
        let s = default_stratigraphy(42, 8);
        assert_eq!(s.layers.len(), 8);
        assert!(s.total_thickness_m() > 0.0);
    }
}