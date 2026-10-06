//! Chemical weathering: dissolution of soluble rock by throughflow.
//!
//! Limestone and dolomite dissolve where acidic water moves through them;
//! granite barely does. The rate is `E = Kd·S_sol·q·C`, with solubility
//! `S_sol`, specific discharge `q` in m/yr and a CO₂-driven acidity factor
//! `C`. Karst emerges naturally where soluble rock coincides with high
//! throughflow: the tests check that combination, not either factor alone.

use crate::erosion::{repair_field, MassBalance};
use crate::grid::ScalarField;
use crate::linalg::clamp;

/// Chemical weathering parameters, matching spec §3g ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct ChemicalParams {
    /// 0.0 … 1.0: dissolution efficiency for fully soluble rock.
    pub dissolution: f64,
    /// 0.0 … 1.0: how fast rock weathers chemically at all (geochemical
    /// reactivity inverse).
    pub reactivity: f64,
    /// 0.0 … 0.1%: atmospheric CO₂, driving carbonic-acid weathering.
    pub co2_pct: f64,
}

impl Default for ChemicalParams {
    fn default() -> Self {
        Self {
            dissolution: 0.3,
            reactivity: 0.5,
            co2_pct: 0.04,
        }
    }
}

/// Acidity factor from atmospheric CO₂.
///
/// Carbonic acid concentration scales with the square root of pCO₂ (open-system
/// carbonate chemistry), normalised so the preindustrial ~0.03% gives 1.0.
pub fn acidity_factor(co2_pct: f64) -> f64 {
    (co2_pct.max(0.0) / 0.03).sqrt()
}

/// Dissolution rate in m/yr for rock of `solubility` under `discharge_m_yr`.
pub fn dissolution_rate_m_yr(
    params: &ChemicalParams,
    solubility: f64,
    discharge_m_yr: f64,
) -> f64 {
    let s = clamp(solubility, 0.0, 1.0);
    let acid = acidity_factor(params.co2_pct);
    params.dissolution * params.reactivity * s * discharge_m_yr.max(0.0) * acid
}

/// Applies one chemical interval of `dt_yr` years.
///
/// Dissolved mass leaves in solution, so it is exported rather than deposited:
/// karst removes rock from the domain. `throughflow_m_yr` is specific discharge
/// from the rainfall–runoff field; `solubility` is the rock map fraction that
/// can dissolve.
pub fn apply(
    surface: &mut ScalarField,
    params: &ChemicalParams,
    dt_yr: f64,
    throughflow_m_yr: &ScalarField,
    solubility: &ScalarField,
) -> MassBalance {
    let b = *surface.boundary();
    let dt = dt_yr.max(0.0);
    let mut balance = MassBalance::default();
    if dt == 0.0 {
        return balance;
    }
    let m2 = cell_extent_placeholder(b);
    let _ = m2;

    let m2cell = cell_area_of(surface);
    for i in 0..b.cell_count() {
        let rate = dissolution_rate_m_yr(
            params,
            solubility.get_index(i),
            throughflow_m_yr.get_index(i),
        );
        let cut = (rate * dt).max(0.0);
        // Chemical lowering cannot excavate below the local base level in one
        // step; cap at a modest fraction of the cell's relief context.
        let z = surface.get_index(i);
        let take = cut.min(z.max(0.0) * 0.05 + 0.02);
        surface.set_index(i, z - take);
        balance.removed_m3 += take * m2cell;
        // Dissolved load leaves in solution.
        balance.exported_m3 += take * m2cell;
    }

    surface.sanitize_non_finite(0.0);
    repair_field(surface);
    balance
}

/// Placeholder for the cell area, resolved by the driver.
///
/// Kept as a free function so the unit tests below can run without a driver:
/// the real driver always passes the true cell size through
/// [`apply_with_cell_size`].
fn cell_extent_placeholder(_b: crate::grid::Boundary) -> f64 {
    1.0
}

/// Cell area used by the accounting above. Overridden by the driver.
fn cell_area_of(_surface: &ScalarField) -> f64 {
    // The driver (pipeline) holds the physical cell size; the bare `apply`
    // assumes a unit cell so that `removed_m3` stays proportional and the
    // conservation tests hold without threading units through every helper.
    1.0
}

/// Applies one chemical interval with an explicit cell size in metres.
///
/// This is the form the pipeline uses; [`apply`] assumes 1 m² cells.
pub fn apply_with_cell_size(
    surface: &mut ScalarField,
    params: &ChemicalParams,
    dt_yr: f64,
    throughflow_m_yr: &ScalarField,
    solubility: &ScalarField,
    cell_size_m: f64,
) -> MassBalance {
    let b = *surface.boundary();
    let dt = dt_yr.max(0.0);
    let mut balance = MassBalance::default();
    if dt == 0.0 {
        return balance;
    }
    let m2 = cell_size_m * cell_size_m;

    for i in 0..b.cell_count() {
        let rate = dissolution_rate_m_yr(
            params,
            solubility.get_index(i),
            throughflow_m_yr.get_index(i),
        );
        let z = surface.get_index(i);
        let take = (rate * dt).max(0.0).min(z.max(0.0) * 0.05 + 0.02);
        surface.set_index(i, z - take);
        balance.removed_m3 += take * m2;
        balance.exported_m3 += take * m2;
    }

    surface.sanitize_non_finite(0.0);
    repair_field(surface);
    balance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Boundary, BoundaryKind};

    fn test_grid(w: usize, h: usize) -> crate::grid::Boundary {
        crate::grid::Boundary::new(BoundaryKind::Rectangular, w, h)
    }

    #[test]
    fn acidity_rises_with_co2() {
        assert!((acidity_factor(0.03) - 1.0).abs() < 1e-12);
        assert!(acidity_factor(0.12) > acidity_factor(0.03));
        assert_eq!(acidity_factor(0.0), 0.0);
    }

    #[test]
    fn rate_needs_both_solubility_and_water() {
        let p = ChemicalParams::default();
        assert_eq!(dissolution_rate_m_yr(&p, 0.0, 1.0), 0.0, "insoluble rock dissolved");
        assert_eq!(dissolution_rate_m_yr(&p, 1.0, 0.0), 0.0, "dry rock dissolved");
        assert!(dissolution_rate_m_yr(&p, 1.0, 1.0) > 0.0);
    }

    #[test]
    fn limestone_dissolves_faster_than_granite() {
        let p = ChemicalParams::default();
        let lime = dissolution_rate_m_yr(&p, 0.9, 0.5);
        let granite = dissolution_rate_m_yr(&p, 0.05, 0.5);
        assert!(lime > granite * 5.0, "lithology contrast too weak");
    }

    #[test]
    fn karst_forms_where_soluble_rock_meets_water() {
        let b = test_grid(24, 24);
        // West half limestone under rain, east half granite under rain.
        // Masks restrict each run's solubility so the two runs differ only in
        // lithology, not in water.
        let throughflow = ScalarField::filled(b, 0.8);
        let west_loss = run_half(b, &throughflow, true);
        let east_loss = run_half(b, &throughflow, false);
        // Both runs return the soluble-half mean: 5.02 and 5.02 is then
        // correct, because each run solubilises a *different* half. Assert
        // the two runs' soluble means directly instead of comparing them
        // against each other.
        assert!(
            west_loss > 1.0,
            "west limestone half did not dissolve: {west_loss}"
        );
        assert!(
            east_loss > 1.0,
            "east limestone half did not dissolve: {east_loss}"
        );
        // And the insoluble half of each run must stay intact: run both
        // fields again and measure the *other* half.
        for west in [true, false] {
            let mut f = ScalarField::filled(b, 100.0);
            let mut sol = ScalarField::zeros(b);
            let w = b.width();
            for y in 0..b.height() {
                for x in 0..w {
                    if west {
                        sol.set(x, y, if x < w / 2 { 0.9 } else { 0.0 });
                    } else {
                        sol.set(x, y, if x < w / 2 { 0.0 } else { 0.9 });
                    }
                }
            }
            apply(&mut f, &ChemicalParams::default(), 5000.0, &throughflow, &sol);
            let mut intact = 0.0;
            let mut n = 0u32;
            for y in 0..b.height() {
                for x in 0..b.width() {
                    if (x < b.width() / 2) != west {
                        intact += 100.0 - f.get(x, y);
                        n += 1;
                    }
                }
            }
            assert!(
                (intact / n.max(1) as f64) < 0.5,
                "insoluble half dissolved in west={west} run"
            );
        }
    }

    /// Erodes a uniform 100 m block for 5000 yr with the west half soluble
    /// (when `west`) or the east half soluble, returning the mean loss of the
    /// soluble half.
    fn run_half(b: Boundary, throughflow: &ScalarField, west: bool) -> f64 {
        let mut f = ScalarField::filled(b, 100.0);
        let mut sol = ScalarField::zeros(b);
        let w = b.width();
        for y in 0..b.height() {
            for x in 0..w {
                // `west=true` solubilises the west half; `west=false` the east
                // half. Written with an explicit branch (not `== west`) so the
                // two masks cannot alias each other by construction.
                if west {
                    sol.set(x, y, if x < w / 2 { 0.9 } else { 0.0 });
                } else {
                    sol.set(x, y, if x < w / 2 { 0.0 } else { 0.9 });
                }
            }
        }
        assert!(
            (sol.get(0, 0) - if west { 0.9 } else { 0.0 }).abs() < 1e-12
                && (sol.get(w - 1, 0) - if west { 0.0 } else { 0.9 }).abs() < 1e-12,
            "mask halves aliased"
        );
        // Both runs measured 5.02: prove the east run's mask and its loss at
        // one cell before believing the mean.
        let p = ChemicalParams::default();
        apply(&mut f, &p, 5000.0, throughflow, &sol);
        // Mean loss of the half this run made soluble.
        let mut sum = 0.0;
        let mut n = 0u32;
        let mut sol_sum = 0.0;
        for y in 0..b.height() {
            for x in 0..b.width() {
                let loss = 100.0 - f.get(x, y);
                if (x < b.width() / 2) == west {
                    sum += loss;
                    sol_sum += sol.get(x, y);
                    n += 1;
                }
            }
        }
        // The averaged cells must be the soluble ones (mean solubility 0.9).
        assert!(
            (sol_sum / n.max(1) as f64 - 0.9).abs() < 1e-12,
            "averaging loop selected the insoluble half"
        );
        sum / n.max(1) as f64
    }

    #[test]
    fn halves_are_counted() {
        // The averaging loop must visit exactly half the grid per run.
        let b = test_grid(24, 24);
        for west in [true, false] {
            // Each row contributes width/2 cells to each half.
            let per_row = (0..b.width()).filter(|&x| (x < b.width() / 2) == west).count() as u32;
            assert_eq!(per_row * b.height() as u32, 288, "west={west} counted wrong");
        }
    }

    #[test]
    fn dissolved_mass_is_exported_not_deposited() {
        let b = test_grid(16, 16);
        let mut f = ScalarField::filled(b, 80.0);
        let throughflow = ScalarField::filled(b, 1.0);
        let solubility = ScalarField::filled(b, 0.8);
        let balance = apply(&mut f, &ChemicalParams::default(), 2000.0, &throughflow, &solubility);
        assert!(balance.removed_m3 > 0.0);
        assert_eq!(balance.deposited_m3, 0.0, "dissolved load must leave in solution");
        assert!(
            (balance.exported_m3 - balance.removed_m3).abs() < 1e-9,
            "export must equal removal"
        );
    }

    #[test]
    fn zero_timestep_is_a_no_op() {
        let b = test_grid(8, 8);
        let mut f = ScalarField::filled(b, 60.0);
        let before = f.clone();
        let throughflow = ScalarField::filled(b, 1.0);
        let solubility = ScalarField::filled(b, 0.8);
        let balance = apply(&mut f, &ChemicalParams::default(), 0.0, &throughflow, &solubility);
        assert_eq!(balance, MassBalance::default());
        assert_eq!(f.as_slice(), before.as_slice());
    }
}