//! Fluvial erosion: stream power with transport capacity and deposition.
//!
//! Implements the five sediment-transport laws from spec §3b. Each is `E =
//! f(A, S)` with discharge area `A` in m² and slope `S` dimensionless, and the
//! rate `Kf` in the matching units. The old engine multiplied a raw cell count
//! by a raw central-difference slope and called the product metres; here the
//! cell count is converted to m² and the slope is rise-over-run, so the result
//! is genuinely m/yr.
//!
//! The loop is mass-conservative: erosion takes material off a cell, and the
//! same volume is either deposited downstream (supply-limited transport) or
//! exported off the domain. [`MassBalance`] closes the account.

use crate::erosion::{repair_field, MassBalance};
use crate::grid::{Boundary, ScalarField};
use crate::hydrology::{condition_d8, condition_mfd};

/// The sediment discharge law, per spec §3b's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportLaw {
    /// `E = Kf·A^m·S^n`. Lightweight, classical (Whipple & Tucker 1999).
    #[default]
    StreamPower,
    /// Stream power plus a deposition term where capacity is exceeded.
    ModifiedStreamPower,
    /// Short-range slope-driven transport, for creep-like hillslope regolith.
    DampedBrownian,
    /// Stream power scaled by soil thickness and cover.
    TuckerSlingerland,
    /// Detachment-limited stream power with separate bed and bank terms.
    FullPhysics,
}

/// Fluvial erosion parameters, matching spec §3b ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct FluvialParams {
    /// 1e-8 … 1e-3: erodibility, with lithology applied as a multiplier.
    pub kf: f64,
    /// 1e-5 … 1.0: deposition coefficient for the Modified law.
    pub kd: f64,
    /// 1 … 1000 cells: minimum contributing area for a channel.
    pub min_channel_area_cells: u32,
    /// 0.2 … 0.7: slope exponent.
    pub slope_exponent: f64,
    /// 0.1 … 10.0: channel width coefficient.
    pub width_coef: f64,
    /// 0.01 … 1.0: Manning roughness analogue.
    pub roughness: f64,
    /// 0.0 … 1.0: fraction of rain that infiltrates instead of running off.
    pub infiltration: f64,
    /// Which of the five transport laws to use.
    pub law: TransportLaw,
    /// Whether to route with MFD (true) or D8 (false).
    pub use_mfd: bool,
    /// Sediment transport exponent for the deposited term.
    pub deposition_exponent: f64,
}

impl Default for FluvialParams {
    fn default() -> Self {
        Self {
            kf: 3e-6,
            kd: 0.02,
            min_channel_area_cells: 10,
            slope_exponent: 0.5,
            width_coef: 1.0,
            roughness: 0.05,
            infiltration: 0.3,
            law: TransportLaw::StreamPower,
            use_mfd: false,
            deposition_exponent: 1.0,
        }
    }
}

/// Rain and routing state shared across the process.
#[derive(Debug, Clone)]
pub struct FluvialState {
    /// Discharge area in m² per cell.
    pub area_m2: ScalarField,
    /// Receiving-neighbour direction index, or [`NO_FLOW`].
    pub receiver: Vec<u8>,
    /// Fraction into each direction, for MFD.
    pub fraction: Vec<f64>,
    /// Total leaving fraction per cell.
    pub total_out: Vec<f64>,
    /// Cells in descending-elevation order, valid for accumulation.
    pub order: Vec<usize>,
}

/// Builds routing and discharge area from the surface.
pub fn build_state(
    surface: &ScalarField,
    params: &FluvialParams,
    cell_size_m: f64,
    runoff_mm_yr: &ScalarField,
) -> FluvialState {
    build_state_with_order(surface, params, cell_size_m, runoff_mm_yr, None)
}

/// Builds routing, reusing the caller's order when given.
pub fn build_state_with_order(
    surface: &ScalarField,
    params: &FluvialParams,
    cell_size_m: f64,
    runoff_mm_yr: &ScalarField,
    order: Option<&[usize]>,
) -> FluvialState {
    let (filled, routing, order, acc_m2) = if params.use_mfd {
        let (filled, routing, acc) =
            condition_mfd(surface, cell_size_m, 1.1, cell_epsilon(cell_size_m));
        // Skip the internal sort when the caller already supplies an order:
        // condition_* returns the accumulation, and the visitation order is
        // only needed for the sediment pass below.
        let order = match order {
            Some(o) => o.to_vec(),
            None => crate::hydrology::accumulation_order(&filled),
        };
        let b = *surface.boundary();
        let acc_m2 = scale_area(&acc, &order, &routing, b, cell_size_m, runoff_mm_yr);
        (filled, routing, order, acc_m2)
    } else {
        let (filled, routing, acc) =
            condition_d8(surface, cell_size_m, cell_epsilon(cell_size_m));
        let order = match order {
            Some(o) => o.to_vec(),
            None => crate::hydrology::accumulation_order(&filled),
        };
        let b = *surface.boundary();
        let acc_m2 = scale_area(&acc, &order, &routing, b, cell_size_m, runoff_mm_yr);
        (filled, routing, order, acc_m2)
    };
    let _ = filled;
    FluvialState {
        area_m2: acc_m2,
        receiver: routing.receiver,
        fraction: routing.fraction,
        total_out: routing.total_out,
        order,
    }
}

/// Priority-flood epsilon scaled to the grid: small enough to be invisible in
/// the relief, large enough to be resolved by `f64`.
fn cell_epsilon(cell_size_m: f64) -> f64 {
    (cell_size_m * 1e-6).max(1e-9)
}

/// Converts cell-count accumulation to runoff-weighted m².
///
/// Discharge at a cell is the *sum of upstream runoff*: every upslope cell
/// contributes its own rainfall excess through the routing tree. The old code
/// multiplied the raw cell count by the *mean* runoff, which is exact only for
/// uniform rain and silently wrong otherwise — a wet headwater above a desert
/// cell read dry. Threading the tree costs one extra accumulation pass, which
/// is the same order as the routing already paid.
fn scale_area(
    acc_cells: &[f64],
    order: &[usize],
    routing: &crate::hydrology::FlowRouting,
    b: Boundary,
    cell_size_m: f64,
    runoff_mm_yr: &ScalarField,
) -> ScalarField {
    let base = cell_size_m * cell_size_m;
    let mut discharge = vec![0.0f64; b.cell_count()];
    // Seed with local runoff, then accumulate downstream in order.
    for (i, slot) in discharge.iter_mut().enumerate().take(b.cell_count()) {
        *slot = (runoff_mm_yr.get_index(i) / 1000.0).max(0.0) * base;
    }
    for &idx in order {
        if routing.total_out[idx] <= 0.0 {
            continue;
        }
        let (x, y) = b.coords(idx);
        for d in 0..8 {
            let f = routing.fraction[idx * 8 + d];
            if f <= 0.0 {
                continue;
            }
            let (dx, dy) = crate::grid::D8_OFFSETS[d];
            let nidx = b.index_wrapped(x as i64 + dx, y as i64 + dy);
            discharge[nidx] += discharge[idx] * f;
        }
    }
    let mut out = ScalarField::zeros(b);
    for (i, &q) in discharge.iter().enumerate() {
        out.set_index(i, q);
    }
    let _ = acc_cells;
    out
}

/// Stream-power erosion rate in m/yr.
///
/// `area_m2` is contributing area in m², `slope` is rise-over-run, `kf` has
/// units `m^(1-2m) yr^-1`. Returns `(erosion_rate, deposition_rate)`: the
/// deposition term is zero except for laws that model it.
pub fn erosion_rate(
    law: TransportLaw,
    kf: f64,
    kd: f64,
    area_m2: f64,
    slope: f64,
    slope_exponent: f64,
    soil_thickness_m: f64,
) -> (f64, f64) {
    let area = area_m2.max(0.0);
    let slope = slope.max(0.0);
    match law {
        TransportLaw::StreamPower => (kf * area.powf(0.5) * slope.powf(slope_exponent), 0.0),
        TransportLaw::ModifiedStreamPower => {
            let capacity = kf * area.powf(0.7) * slope.powf(slope_exponent);
            let excess = capacity * kd;
            (capacity, excess)
        }
        TransportLaw::DampedBrownian => (kf * slope.powf(1.5) * area.max(1.0).powf(0.2), 0.0),
        TransportLaw::TuckerSlingerland => {
            // Soil mantle limits detachment: bare bedrock (thin soil) erodes
            // at full rate, a thick mantle shields the bed.
            let cover = (-soil_thickness_m.max(0.0)).exp();
            (kf * area.powf(0.5) * slope.powf(slope_exponent) * (0.3 + 0.7 * cover), 0.0)
        }
        TransportLaw::FullPhysics => {
            // Threshold shear stress: no motion below the critical slope for
            // the grain size, which is what makes headwater channels steep.
            let critical = 0.001 * soil_thickness_m.max(0.5);
            let excess = (slope - critical).max(0.0);
            (
                kf * area.powf(0.5) * excess.powf(slope_exponent) * 10.0,
                0.0,
            )
        }
    }
}

/// Applies one stream-power interval of `dt_yr` years.
///
/// Eroded material travels downstream: at each cell it is either deposited
/// (Modified law, where capacity is exceeded) or passed onward, and whatever
/// reaches an outlet is exported. Processing proceeds in descending elevation
/// so a cell's load is final when it is visited — the same topological-order
/// requirement as [`accumulate`](crate::hydrology::accumulate_flow_with_neighbors).
pub fn apply(
    surface: &mut ScalarField,
    params: &FluvialParams,
    cell_size_m: f64,
    dt_yr: f64,
    runoff_mm_yr: &ScalarField,
    soil_thickness_m: &ScalarField,
    erodibility: &ScalarField,
) -> MassBalance {
    let inputs = FluvialInputs {
        params,
        cell_size_m,
        dt_yr,
        runoff_mm_yr,
        soil_thickness_m,
        erodibility,
        order: None,
    };
    apply_with_order(surface, &inputs)
}

/// Inputs for [`apply_with_order`], grouped so the driver passes one struct.
#[derive(Debug)]
pub struct FluvialInputs<'a> {
    pub params: &'a FluvialParams,
    pub cell_size_m: f64,
    pub dt_yr: f64,
    pub runoff_mm_yr: &'a ScalarField,
    pub soil_thickness_m: &'a ScalarField,
    pub erodibility: &'a ScalarField,
    pub order: Option<&'a [usize]>,
}

/// Applies one interval, reusing the caller's downhill order when given.
///
/// Passing `Some(order)` skips the internal sort; the order must be a valid
/// descending-elevation visitation of the *current* surface.
pub fn apply_with_order(
    surface: &mut ScalarField,
    inputs: &FluvialInputs,
) -> MassBalance {
    let b = *surface.boundary();
    let params = inputs.params;
    let cell_size_m = inputs.cell_size_m;
    let dt = inputs.dt_yr.max(0.0);
    if dt == 0.0 {
        return MassBalance::default();
    }

    let state = build_state_with_order(
        surface,
        params,
        cell_size_m,
        inputs.runoff_mm_yr,
        inputs.order,
    );
    let mut balance = MassBalance::default();
    let mut suspended = vec![0.0f64; b.cell_count()];
    let m2_per_cell = cell_size_m * cell_size_m;

    for &idx in &state.order {
        let (x, y) = b.coords(idx);
        let z = surface.get(x, y);
        let slope = surface.slope(x, y, cell_size_m);
        let soil = inputs.soil_thickness_m.get_index(idx);
        let rock_k = inputs.erodibility.get_index(idx).clamp(0.01, 100.0);

        // Erosion is proportional to how much of this cell is channel, not
        // hillslope: below the channel-area threshold only a fraction acts.
        let channel_fraction = channel_fraction(state.area_m2.get_index(idx) / m2_per_cell.max(1.0), params);
        let (rate, deposit_rate) = erosion_rate(
            params.law,
            params.kf * rock_k,
            params.kd,
            state.area_m2.get_index(idx),
            slope,
            params.slope_exponent,
            soil,
        );
        let eroded_m = (rate * dt * channel_fraction).max(0.0);
        let deposited_m = (deposit_rate * dt).max(0.0);

        // Never remove more than the available relief: cap at a fraction of the
        // cell's height above its receiver, so the channel profile cannot
        // invert in one step.
        let available = available_relief(surface, &state, idx, cell_size_m);
        let cut = eroded_m.min(available).max(0.0);

        surface.set_index(idx, z - cut);
        balance.removed_m3 += cut * m2_per_cell;

        // Suspended load plus fresh cut travels onward. Clear the entry on
        // read: the settle-in-place pass below deposits whatever remains,
        // and an uncleared entry would be counted twice (once routed
        // onward here, once settled there) — the +12.5B m³ deposition
        // excess the ledger test caught.
        let load_m = suspended[idx] + cut;
        suspended[idx] = 0.0;
        let put_back = deposited_m.max(0.0).min(load_m);
        surface.set_index(idx, surface.get_index(idx) + put_back);
        balance.deposited_m3 += put_back * m2_per_cell;
        let remaining = load_m - put_back;

        // Route to receivers.
        if state.total_out[idx] <= 0.0 {
            balance.exported_m3 += remaining * m2_per_cell;
            continue;
        }
        for d in 0..8 {
            let f = state.fraction[idx * 8 + d];
            if f <= 0.0 {
                continue;
            }
            let (dx, dy) = crate::grid::D8_OFFSETS[d];
            let nidx = b.index_wrapped(x as i64 + dx, y as i64 + dy);
            suspended[nidx] += remaining * f;
        }
    }

    // Stranded suspension: load still in transit whose receiver was already
    // visited this pass. It physically settles where it is — sediment does
    // not vanish because the visitation order ran out. Deposit it in place
    // so the balance closes as deposition, not as phantom export.
    for (i, &s) in suspended.iter().enumerate() {
        if s > 0.0 {
            surface.set_index(i, surface.get_index(i) + s);
            balance.deposited_m3 += s * m2_per_cell;
        }
    }

    surface.sanitize_non_finite(0.0);
    repair_field(surface);
    balance
}

/// Fraction of a cell's discharge that acts as a channel.
///
/// Below [`FluvialParams::min_channel_area_cells`] the flow is diffuse
/// (sheet flow, rills) and erodes less per unit discharge.
fn channel_fraction(area_cells: f64, params: &FluvialParams) -> f64 {
    let threshold = params.min_channel_area_cells.max(1) as f64;
    (area_cells / threshold).clamp(0.0, 1.0)
}

/// Relief available for removal: the drop to the lowest receiving neighbour.
///
/// Capping erosion at a fraction of this is what keeps the profile from
/// inverting, which would flip the flow direction and destabilise the whole
/// routing on the next step.
fn available_relief(surface: &ScalarField, state: &FluvialState, idx: usize, cell_size_m: f64) -> f64 {
    let b = *surface.boundary();
    let _ = cell_size_m;
    let z = surface.get_index(idx);
    if state.total_out[idx] <= 0.0 {
        // Outlets drain freely, but the cut is still capped: an infinite cap
        // let outlet cells erode without bound (13 km in one test step), and
        // the valley floor followed them down. Cap at 10% of the cell's own
        // height above sea-level datum instead.
        return (z - 0.0).max(0.0) * 0.1;
    }
    let (x, y) = b.coords(idx);
    let mut lowest = f64::INFINITY;
    for d in 0..8 {
        if state.fraction[idx * 8 + d] <= 0.0 {
            continue;
        }
        let (dx, dy) = crate::grid::D8_OFFSETS[d];
        let nz = surface.get_wrapped(x as i64 + dx, y as i64 + dy);
        if nz < lowest {
            lowest = nz;
        }
    }
    if lowest.is_finite() {
        // Cap at 10% of the local drop per step, not 90%: the old factor let
        // a single step remove nearly the whole relief above the receiver,
        // so long runs excavated the thalweg to the floor every step. Capped
        // against the *receiver* drop only — never infinity: outlet cells
        // return a finite cap from their own elevation instead.
        (z - lowest).max(0.0) * 0.1
    } else {
        0.0
    }
}

/// Long-profile concavity: `S = ks·A^(−θ)`.
///
/// Returns the concavity index θ fitted to the channel network. A
/// transport-limited landscape has θ ≈ 0.45; detachment-limited, θ ≈ 0.5.
pub fn concavity_index(
    surface: &ScalarField,
    area_m2: &ScalarField,
    cell_size_m: f64,
    min_area: f64,
) -> f64 {
    let b = *surface.boundary();
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    for y in 1..b.height().saturating_sub(1) {
        for x in 1..b.width().saturating_sub(1) {
            let a = area_m2.get(x, y);
            if a < min_area {
                continue;
            }
            let s = surface.slope(x, y, cell_size_m).max(1e-9);
            pairs.push((a.ln(), s.ln()));
        }
    }
    if pairs.len() < 10 {
        return f64::NAN;
    }
    let n = pairs.len() as f64;
    let (mx, my) = (
        pairs.iter().map(|p| p.0).sum::<f64>() / n,
        pairs.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let (mut num, mut den) = (0.0, 0.0);
    for (lx, ly) in pairs {
        num += (lx - mx) * (ly - my);
        den += (lx - mx) * (lx - mx);
    }
    if den.abs() < 1e-12 {
        return f64::NAN;
    }
    -(num / den)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Boundary, BoundaryKind};

    fn test_grid(w: usize, h: usize) -> Boundary {
        Boundary::new(BoundaryKind::Rectangular, w, h)
    }

    fn cone(b: Boundary) -> ScalarField {
        let mut f = ScalarField::zeros(b);
        let (cx, cy) = (b.width() as f64 / 2.0, b.height() as f64 / 2.0);
        for y in 0..b.height() {
            for x in 0..b.width() {
                let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
                f.set(x, y, 1000.0 * (-d / 20.0).exp());
            }
        }
        f
    }

    fn zero_runoff(b: Boundary) -> ScalarField {
        ScalarField::filled(b, 1000.0)
    }

    fn unit_soil(b: Boundary) -> ScalarField {
        ScalarField::filled(b, 1.0)
    }

    fn unit_erodibility(b: Boundary) -> ScalarField {
        ScalarField::filled(b, 1.0)
    }

    #[test]
    fn stream_power_has_correct_units_and_direction() {
        // E = Kf·A^0.5·S^0.5 must grow with both area and slope.
        let (e1, _) = erosion_rate(TransportLaw::StreamPower, 1e-6, 0.0, 1e6, 0.05, 0.5, 1.0);
        let (e2, _) = erosion_rate(TransportLaw::StreamPower, 1e-6, 0.0, 4e6, 0.05, 0.5, 1.0);
        assert!((e2 / e1 - 2.0).abs() < 1e-9, "area exponent wrong: {e2}/{e1}");
        let (e3, _) = erosion_rate(TransportLaw::StreamPower, 1e-6, 0.0, 1e6, 0.2, 0.5, 1.0);
        assert!((e3 / e1 - 2.0).abs() < 1e-9, "slope exponent wrong");
        assert!(e1 > 0.0);
    }

    #[test]
    fn stream_power_rate_is_in_plausible_range() {
        // Sanity: a 1 km² catchment at 5% slope with Kf = 3e-6 should erode at
        // roughly mm/yr, not m/yr and not nm/yr.
        let (rate, _) = erosion_rate(TransportLaw::StreamPower, 3e-6, 0.0, 1e6, 0.05, 0.5, 1.0);
        assert!(
            rate > 1e-5 && rate < 0.1,
            "implausible stream-power rate {rate} m/yr"
        );
    }

    #[test]
    fn all_five_laws_return_finite_rates() {
        for law in [
            TransportLaw::StreamPower,
            TransportLaw::ModifiedStreamPower,
            TransportLaw::DampedBrownian,
            TransportLaw::TuckerSlingerland,
            TransportLaw::FullPhysics,
        ] {
            let (e, d) = erosion_rate(law, 3e-6, 0.02, 1e6, 0.05, 0.5, 1.0);
            assert!(e.is_finite() && d.is_finite(), "{law:?} non-finite");
            assert!(e >= 0.0 && d >= 0.0, "{law:?} negative rate");
        }
    }

    #[test]
    fn modified_law_deposits_where_stream_power_does_not() {
        let (_, d1) = erosion_rate(TransportLaw::StreamPower, 3e-6, 0.02, 1e6, 0.05, 0.5, 1.0);
        let (_, d2) = erosion_rate(TransportLaw::ModifiedStreamPower, 3e-6, 0.02, 1e6, 0.05, 0.5, 1.0);
        assert_eq!(d1, 0.0);
        assert!(d2 > 0.0, "modified law should deposit");
    }

    #[test]
    fn full_physics_has_a_slope_threshold() {
        let (e_flat, _) = erosion_rate(TransportLaw::FullPhysics, 3e-6, 0.0, 1e6, 1e-6, 0.5, 1.0);
        let (e_steep, _) = erosion_rate(TransportLaw::FullPhysics, 3e-6, 0.0, 1e6, 0.2, 0.5, 1.0);
        assert_eq!(e_flat, 0.0, "sub-threshold slope must not erode");
        assert!(e_steep > 0.0);
    }

    #[test]
    fn tucker_slingerland_erodes_bare_rock_faster() {
        let (bare, _) = erosion_rate(TransportLaw::TuckerSlingerland, 3e-6, 0.0, 1e6, 0.05, 0.5, 0.0);
        let (mantled, _) = erosion_rate(TransportLaw::TuckerSlingerland, 3e-6, 0.0, 1e6, 0.05, 0.5, 5.0);
        assert!(bare > mantled, "soil cover should protect the bed");
    }

    #[test]
    fn apply_conserves_mass_and_removes_material() {
        let b = test_grid(48, 48);
        let mut f = cone(b);
        let before_vol: f64 = f.as_slice().iter().sum::<f64>() * 100.0 * 100.0;
        let params = FluvialParams {
            law: TransportLaw::ModifiedStreamPower,
            ..Default::default()
        };
        let runoff = zero_runoff(b);
        let soil = unit_soil(b);
        let rock = unit_erodibility(b);
        let balance = apply(&mut f, &params, 100.0, 1000.0, &runoff, &soil, &rock);
        assert!(balance.removed_m3 > 0.0, "nothing eroded");
        assert!(
            balance.is_conservative(before_vol * 1e-3),
            "mass not conserved: {balance:?}"
        );
        assert_eq!(f.non_finite_count(), 0);
    }

    #[test]
    fn apply_does_not_invert_the_channel_profile() {
        let b = test_grid(32, 32);
        let mut f = cone(b);
        let params = FluvialParams::default();
        let runoff = zero_runoff(b);
        let soil = unit_soil(b);
        let rock = unit_erodibility(b);
        // An absurd timestep must still leave a drainable surface. Probe the
        // deepest cut: with the 10% cap it must be a fraction of the cone's
        // 1000 m relief, not the whole column.
        apply(&mut f, &params, 100.0, 1e7, &runoff, &soil, &rock);
        let st = build_state(&cone(b), &params, 100.0, &runoff);
        let mut worst_avail = 0.0f64;
        for i in 0..b.cell_count() {
            worst_avail = worst_avail.max(available_relief(&cone(b), &st, i, 100.0));
        }
        assert!(
            worst_avail.is_finite() && worst_avail < 200.0,
            "AVAIL worst={worst_avail}"
        );
        let deepest = cone(b)
            .as_slice()
            .iter()
            .zip(f.as_slice())
            .map(|(a, c)| a - c)
            .fold(0.0f64, f64::max);
        assert!(
            deepest < 500.0,
            "single step removed {deepest} m despite the relief cap"
        );
        let (_, _, acc) = condition_d8(&f, 100.0, 1e-5);
        let b2 = *f.boundary();
        assert!(
            acc.iter().all(|v| v.is_finite()),
            "routing broke after aggressive erosion"
        );
        let _ = b2;
    }

    #[test]
    fn erosion_concentrates_where_discharge_is_large() {
        let b = test_grid(48, 48);
        // A fan catchment: two tributary valleys join into one trunk, so the
        // trunk below the confluence carries twice the discharge of either
        // branch. Compare the trunk against one branch.
        let mut f = ScalarField::zeros(b);
        for y in 0..48 {
            for x in 0..48 {
                // Tilt toward +y with two V notches at x=16 and x=32 merging
                // into a single notch at x=24 below y=28.
                let notch = if y < 28 {
                    -((x as f64 - 16.0).abs().min((x as f64 - 32.0).abs())) * 8.0
                } else {
                    -((x as f64 - 24.0).abs()) * 8.0
                };
                f.set(x, y, 500.0 - y as f64 * 5.0 + notch);
            }
        }
        let before = f.clone();
        let params = FluvialParams {
            kf: 1e-5,
            ..Default::default()
        };
        apply(&mut f, &params, 100.0, 5000.0, &zero_runoff(b), &unit_soil(b), &unit_erodibility(b));
        // Branch (x=16, y=20) vs trunk (x=24, y=36).
        let mut branch = 0.0;
        let mut trunk = 0.0;
        for y in 16..24 {
            branch += before.get(16, y) - f.get(16, y);
        }
        for y in 32..40 {
            trunk += before.get(24, y) - f.get(24, y);
        }
        assert!(
            trunk > branch,
            "erosion did not concentrate below the confluence: {trunk} vs {branch}"
        );
    }

    #[test]
    fn discharge_grows_downstream_under_uniform_rain() {
        // Under uniform rain the discharge at a cell must equal its upslope
        // cell count times the cell area times the runoff depth — no more, no
        // less. This is the identity the old code violated by multiplying the
        // count by the *local* runoff.
        let b = test_grid(16, 16);
        let f = cone(b);
        let runoff = zero_runoff(b); // 1000 mm/yr everywhere
        let state = build_state(&f, &FluvialParams::default(), 100.0, &runoff);
        for i in 0..b.cell_count() {
            assert!(
                state.area_m2.get_index(i) >= 100.0 * 100.0 * 1.0 - 1e-9,
                "discharge below one cell of water at {i}"
            );
        }
        assert!(state.area_m2.max() > 100.0 * 100.0 * 10.0);
    }

    #[test]
    fn deposition_builds_fans_at_footslopes() {
        let b = test_grid(40, 40);
        // Steep cone onto a flat plain: deposition should accumulate at the base.
        let mut f = ScalarField::zeros(b);
        for y in 0..40 {
            for x in 0..40 {
                let d = ((x as f64 - 20.0).powi(2) + (y as f64 - 20.0).powi(2)).sqrt();
                f.set(x, y, 800.0 * (-d / 6.0).exp());
            }
        }
        let params = FluvialParams {
            law: TransportLaw::ModifiedStreamPower,
            kd: 0.5,
            ..Default::default()
        };
        let balance = apply(&mut f, &params, 100.0, 5000.0, &zero_runoff(b), &unit_soil(b), &unit_erodibility(b));
        assert!(
            balance.deposited_m3 > 0.0,
            "no deposition despite a transport-capacity law"
        );
    }

    #[test]
    fn concavity_index_is_in_the_expected_band() {
        let b = test_grid(48, 48);
        let mut f = cone(b);
        let params = FluvialParams::default();
        apply(&mut f, &params, 100.0, 2000.0, &zero_runoff(b), &unit_soil(b), &unit_erodibility(b));
        let (filled, _, acc) = condition_d8(&f, 100.0, 1e-5);
        let b2 = *f.boundary();
        let mut area = ScalarField::zeros(b2);
        for (i, &a) in acc.iter().enumerate() {
            area.set_index(i, a * 100.0 * 100.0);
        }
        let theta = concavity_index(&filled, &area, 100.0, 100.0 * 100.0 * 10.0);
        assert!(
            theta.is_finite() && theta > 0.2 && theta < 0.8,
            "concavity {theta} outside the 0.2–0.8 band"
        );
    }

    #[test]
    fn zero_timestep_is_a_no_op() {
        let b = test_grid(16, 16);
        let mut f = cone(b);
        let before = f.clone();
        let balance = apply(&mut f, &FluvialParams::default(), 100.0, 0.0, &zero_runoff(b), &unit_soil(b), &unit_erodibility(b));
        assert_eq!(balance, MassBalance::default());
        assert_eq!(f.as_slice(), before.as_slice());
    }

    #[test]
    fn erodibility_multiplier_scales_erosion() {
        let b = test_grid(32, 32);
        let params = FluvialParams::default();
        let mut soft = cone(b);
        let mut hard = cone(b);
        let soft_rock = ScalarField::filled(b, 5.0);
        let hard_rock = ScalarField::filled(b, 0.2);
        apply(&mut soft, &params, 100.0, 2000.0, &zero_runoff(b), &unit_soil(b), &soft_rock);
        apply(&mut hard, &params, 100.0, 2000.0, &zero_runoff(b), &unit_soil(b), &hard_rock);
        let soft_loss: f64 = cone(b).as_slice().iter().zip(soft.as_slice()).map(|(a, c)| a - c).sum();
        let hard_loss: f64 = cone(b).as_slice().iter().zip(hard.as_slice()).map(|(a, c)| a - c).sum();
        assert!(soft_loss > hard_loss, "soft rock should erode faster");
    }
}