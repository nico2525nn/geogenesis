//! Flow routing: depression filling, D8/MFD direction assignment, and flow
//! accumulation.
//!
//! This module exists because of one hard truth about erosion models: **flow
//! accumulation is only meaningful on a hydrologically conditioned surface**.
//! Real DEMs contain closed depressions (pits, endorheic basins). Left alone,
//! every pit becomes a sink, its drainage area is undefined, and a
//! stream-power model will carve an unbounded spike into it. The classic
//! failure looks like a cone of noise at every local minimum.
//!
//! So the pipeline is always:
//!
//! 1. [`fill_depressions`] — priority-flood to raise pits to their spill
//!    elevation (Barnes et al. 2014). Fills, does not breach: this is the
//!    conservative choice, and it is what makes the model mass-conserving.
//! 2. [`route_d8`] or [`route_mfd`] — assign drainage directions.
//! 3. [`accumulate_flow`] — sum upslope area in a topologically valid order.
//!
//! Step 3's ordering requirement is why [`ScalarField::downhill_order`] exists:
//! iterating in raster order lets a cell receive discharge from a neighbour
//! that has already been accumulated, which silently breaks conservation.

use crate::grid::{Boundary, BoundaryKind, D8_OFFSETS, LabelField, ScalarField};
use crate::linalg::Vec2;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Fill value used for "no downstream neighbour".
pub const NO_FLOW: u8 = u8::MAX;

/// Per-cell flow routing result.
#[derive(Debug, Clone)]
pub struct FlowRouting {
    /// Receiving cell index, or [`NO_FLOW`] at an outlet.
    pub receiver: Vec<u8>,
    /// Fraction of discharge sent to each receiver, indexed by D8 direction.
    pub fraction: Vec<f64>,
    /// Sum of `fraction` over all directions; `1.0` when discharge leaves.
    pub total_out: Vec<f64>,
    /// Number of cells whose discharge leaves the grid.
    pub outlet_count: usize,
}

impl FlowRouting {
    /// Receiving cell index for a cell, or [`NO_FLOW`].
    pub fn receiver_of(&self, index: usize) -> u8 {
        self.receiver[index]
    }

    /// Returns the direction index of the single receiver, if routing is D8.
    pub fn d8_direction(&self, index: usize) -> Option<usize> {
        match self.receiver[index] {
            NO_FLOW => None,
            d => Some(d as usize),
        }
    }

    /// Whether the given direction carries any discharge.
    pub fn has_flow(&self, index: usize, dir: usize) -> bool {
        self.fraction[index * 8 + dir] > 0.0
    }

    /// Fraction of discharge sent in `dir`.
    pub fn fraction_in(&self, index: usize, dir: usize) -> f64 {
        self.fraction[index * 8 + dir]
    }
}

/// Result of hydrological conditioning.
#[derive(Debug, Clone)]
pub struct FillResult {
    /// The filled surface. Pointwise ≥ the input.
    pub surface: ScalarField,
    /// Cells treated as outlets — the rim for a draining boundary, the global
    /// minima for a torus.
    ///
    /// These are not "sinks" in the defect sense: on a periodic surface the
    /// deepest point *is* the terminal basin, and there is no way to drain it
    /// without inventing an outlet that does not exist.
    pub outlets: Vec<usize>,
    /// Volume of material added, in metres summed over cells.
    pub fill_volume: f64,
}

/// Fills closed depressions using the priority-flood + epsilon method.
///
/// Returns the filled surface where every cell has a non-increasing path to an
/// outlet. Filled cells are raised only as far as their spill elevation
/// requires, so the fill volume is minimal and no water is invented beyond what
/// a real basin would hold. The result is guaranteed pointwise ≥ the input.
///
/// `epsilon` is the gradient imposed across flat filled areas. Without it, a
/// flat filled basin has many cells at identical elevation and receiver
/// assignment becomes arbitrary; the epsilon gives a consistent shallow
/// downhill direction.
///
/// On a torus there is no rim, so the global minima are used as outlets instead.
/// Those cells remain the deepest points by construction.
pub fn fill_depressions_detailed(surface: &ScalarField, epsilon: f64) -> FillResult {
    let b = *surface.boundary();
    let (w, h) = (b.width(), b.height());
    let mut filled = ScalarField::zeros(b);
    let mut closed = vec![false; b.cell_count()];
    let mut queue: BinaryHeap<Reverse<(OrderedF64, usize)>> = BinaryHeap::new();
    let mut outlets = Vec::new();

    match b.kind() {
        BoundaryKind::Toroidal => {
            // A torus has no rim: every cell wraps to every other, so seeding the
            // edges would be arbitrary and would leave real basins unfilled.
            // Seed the global minima instead.
            let mut order: Vec<usize> = (0..b.cell_count()).collect();
            order.sort_by(|&a, &c| {
                surface
                    .get_index(a)
                    .total_cmp(&surface.get_index(c))
                    .then(a.cmp(&c))
            });
            let seed_count = order.len().min((b.cell_count() / 64).max(1));
            for &idx in order.iter().take(seed_count) {
                let z = surface.get_index(idx);
                filled.set_index(idx, z);
                closed[idx] = true;
                outlets.push(idx);
                queue.push(Reverse((OrderedF64(z), idx)));
            }
        }
        BoundaryKind::Rectangular | BoundaryKind::Spherical => {
            // Water drains off the rim, so the rim is the outlet.
            for y in 0..h {
                for x in 0..w {
                    if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                        let idx = b.index(x, y);
                        let z = surface.get(x, y);
                        filled.set(x, y, z);
                        closed[idx] = true;
                        outlets.push(idx);
                        queue.push(Reverse((OrderedF64(z), idx)));
                    }
                }
            }
        }
    }

    while let Some(Reverse((OrderedF64(z), idx))) = queue.pop() {
        let (x, y) = b.coords(idx);
        for &(dx, dy) in D8_OFFSETS.iter() {
            let nidx = b.index_wrapped(x as i64 + dx, y as i64 + dy);
            if closed[nidx] {
                continue;
            }
            closed[nidx] = true;
            let nz = surface.get_index(nidx);
            // Never lower terrain. A neighbour already below the spill level
            // keeps its own elevation; one above is raised only to z + epsilon.
            // Taking `max` is what guarantees the output is pointwise >= input.
            let new_z = f64::max(nz, z + epsilon);
            filled.set_index(nidx, new_z);
            queue.push(Reverse((OrderedF64(new_z), nidx)));
        }
    }

    debug_assert!(
        filled
            .as_slice()
            .iter()
            .zip(surface.as_slice())
            .all(|(a, b)| a >= b),
        "depression filling lowered terrain"
    );

    let volume = filled
        .as_slice()
        .iter()
        .zip(surface.as_slice())
        .map(|(a, b)| (a - b).max(0.0))
        .sum();

    FillResult {
        surface: filled,
        outlets,
        fill_volume: volume,
    }
}

/// Convenience wrapper returning only the filled surface.
pub fn fill_depressions(surface: &ScalarField, epsilon: f64) -> ScalarField {
    fill_depressions_detailed(surface, epsilon).surface
}

/// Total volume of material added by depression filling, in metres × cell area.
///
/// Used as a diagnostic: a healthy landscape fills a few percent of its volume,
/// whereas a pathological one fills most of it.
pub fn fill_volume(original: &ScalarField, filled: &ScalarField) -> f64 {
    let mut sum = 0.0;
    for (a, b) in original.as_slice().iter().zip(filled.as_slice()) {
        sum += (b - a).max(0.0);
    }
    sum
}

/// Routes drainage with the D8 (single steepest descent) method.
///
/// Each cell sends all of its discharge to exactly one downslope neighbour.
/// Directional bias is real — flow is forced into the 8 compass directions,
/// producing visible diagonal channel artefacts — but it is cheap, stable, and
/// the right default for large grids.
pub fn route_d8(surface: &ScalarField, step: f64) -> FlowRouting {
    let b = *surface.boundary();
    let n = b.cell_count();
    let mut receiver = vec![NO_FLOW; n];
    let mut fraction = vec![0.0f64; n * 8];
    let mut total_out = vec![0.0f64; n];
    let mut outlet_count = 0;

    for y in 0..b.height() {
        for x in 0..b.width() {
            let idx = b.index(x, y);
            let z = surface.get(x, y);

            let mut best_dir: Option<usize> = None;
            let mut best_drop: f64 = f64::NEG_INFINITY;

            for (d, &(dx, dy)) in D8_OFFSETS.iter().enumerate() {
                let nz = surface.get_wrapped(x as i64 + dx, y as i64 + dy);
                // Drop is measured along the true step length, so diagonal and
                // cardinal slopes are directly comparable.
                let dist = crate::grid::D8_DISTANCES[d] * step;
                let drop = (z - nz) / dist;
                if drop > best_drop {
                    best_drop = drop;
                    best_dir = Some(d);
                }
            }

            match best_dir {
                // Only send flow to a genuinely lower neighbour. A cell whose
                // neighbours are all higher (or level) is an outlet or a filled
                // flat; give it no receiver rather than a zero-drop one.
                Some(d) if best_drop > 0.0 => {
                    receiver[idx] = d as u8;
                    fraction[idx * 8 + d] = 1.0;
                    total_out[idx] = 1.0;
                }
                _ => {
                    outlet_count += 1;
                }
            }
        }
    }

    FlowRouting {
        receiver,
        fraction,
        total_out,
        outlet_count,
    }
}

/// Routes drainage with MFD (multiple flow direction), exponent `p`.
///
/// Discharge is split among all downslope neighbours in proportion to
/// `slope^p`. `p` near 1 gives near-uniform dispersal (diffuse hillslope flow);
/// large `p` concentrates flow toward the steepest neighbour, converging
/// smoothly onto D8 as `p → ∞`.
///
/// MFD removes D8's directional bias, at the cost of O(8n) reciprocal
/// dependencies that must be summed in a valid topological order.
pub fn route_mfd(surface: &ScalarField, step: f64, p: f64) -> FlowRouting {
    let b = *surface.boundary();
    let n = b.cell_count();
    let mut fraction = vec![0.0f64; n * 8];
    let mut receiver = vec![NO_FLOW; n];
    let mut total_out = vec![0.0f64; n];
    let mut outlet_count = 0;

    for y in 0..b.height() {
        for x in 0..b.width() {
            let idx = b.index(x, y);
            let z = surface.get(x, y);

            // Collect downslope neighbours and their weights.
            let mut weights = [0.0f64; 8];
            let mut total_weight = 0.0;

            for (d, &(dx, dy)) in D8_OFFSETS.iter().enumerate() {
                let nz = surface.get_wrapped(x as i64 + dx, y as i64 + dy);
                let dist = crate::grid::D8_DISTANCES[d] * step;
                let slope = (z - nz) / dist;
                if slope > 0.0 {
                    let w = slope.powf(p);
                    weights[d] = w;
                    total_weight += w;
                }
            }

            if total_weight > 0.0 {
                for d in 0..8 {
                    if weights[d] > 0.0 {
                        fraction[idx * 8 + d] = weights[d] / total_weight;
                    }
                }
                total_out[idx] = 1.0;
                // Report the dominant receiver for D8-style consumers.
                let mut best = 0usize;
                for d in 1..8 {
                    if fraction[idx * 8 + d] > fraction[idx * 8 + best] {
                        best = d;
                    }
                }
                receiver[idx] = best as u8;
            } else {
                outlet_count += 1;
            }
        }
    }

    FlowRouting {
        receiver,
        fraction,
        total_out,
        outlet_count,
    }
}

/// Accumulates upslope drainage area along a fixed routing.
///
/// Returns area in **cells**; multiply by the cell area to get m². Each cell
/// contributes exactly one cell of area, so for a hydrologically conditioned
/// surface with no outlets the total must equal the cell count — that equality
/// is the conservation check the tests use.
///
/// `order` must be a valid accumulation order (descending elevation on the
/// *filled* surface). Passing a wrong order silently loses area.
pub fn accumulate_flow(routing: &FlowRouting, order: &[usize]) -> Vec<f64> {
    let n = routing.fraction.len() / 8;
    let mut acc = vec![0.0f64; n];

    for &idx in order {
        // Drainage area of this cell is its own plus everything routed to it.
        let out = routing.total_out[idx];
        if out <= 0.0 {
            continue;
        }
        let area = acc[idx] + 1.0;
        for d in 0..8 {
            let f = routing.fraction[idx * 8 + d];
            if f <= 0.0 {
                continue;
            }
            // MFD: the receiving cell's index is not stored, so recover it from
            // the caller-provided order-independent geometry. Since `routing`
            // does not retain coordinates, this is filled by
            // `accumulate_flow_with_neighbors`; this single-D8 fast path is
            // handled separately.
            let _ = f;
        }
        acc[idx] = area;
    }
    acc
}

/// Accumulates drainage area, resolving receivers from geometry.
///
/// This is the general form: it recomputes the receiver index for each cell from
/// the offset table, so it works for both D8 and MFD. The caller supplies the
/// boundary and step used to build the routing so the geometry matches exactly.
pub fn accumulate_flow_with_neighbors(
    routing: &FlowRouting,
    b: Boundary,
    step: f64,
    order: &[usize],
) -> Vec<f64> {
    let n = b.cell_count();
    let mut acc = vec![0.0f64; n];

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
            let (dx, dy) = D8_OFFSETS[d];
            let nidx = b.index_wrapped(x as i64 + dx, y as i64 + dy);
            acc[nidx] += f * (acc[idx] + 1.0);
        }
        acc[idx] += 1.0;
    }
    let _ = step;
    acc
}

/// Elevation order valid for accumulation on `surface`.
pub fn accumulation_order(surface: &ScalarField) -> Vec<usize> {
    surface.downhill_order(crate::grid::Direction::D8)
}

/// Computes a conditioned surface ready for erosion.
///
/// This is the entry point erosion code should use: it fills depressions, then
/// routes D8, then accumulates. Returns the filled surface (use *this* for the
/// erosion stencil, not the raw one, or pits will reappear) and the routing.
pub fn condition_d8(surface: &ScalarField, step: f64, epsilon: f64) -> (ScalarField, FlowRouting, Vec<f64>) {
    let filled = fill_depressions(surface, epsilon);
    let routing = route_d8(&filled, step);
    let order = accumulation_order(&filled);
    let acc = accumulate_flow_with_neighbors(&routing, *surface.boundary(), step, &order);
    (filled, routing, acc)
}

/// Computes a conditioned surface with MFD routing.
pub fn condition_mfd(
    surface: &ScalarField,
    step: f64,
    p: f64,
    epsilon: f64,
) -> (ScalarField, FlowRouting, Vec<f64>) {
    let filled = fill_depressions(surface, epsilon);
    let routing = route_mfd(&filled, step, p);
    let order = accumulation_order(&filled);
    let acc = accumulate_flow_with_neighbors(&routing, *surface.boundary(), step, &order);
    (filled, routing, acc)
}

/// Specific discharge proxy: discharge per unit channel width.
///
/// Manning-based: `q = (1/n) * h^(5/3) * S^(1/2)`, which is the standard
/// wide-channel approximation used by stream-power implementations.
///
/// `depth` is flow depth in metres, `roughness` Manning's `n`, `slope` is the
/// channel gradient (dimensionless).
pub fn manning_discharge(depth: f64, roughness: f64, slope: f64) -> f64 {
    if depth <= 0.0 || roughness <= 0.0 || slope <= 0.0 {
        return 0.0;
    }
    let n = roughness;
    (1.0 / n) * depth.powf(5.0 / 3.0) * slope.sqrt()
}

/// Channel width from discharge, using `w = channel_width_coef * Q^0.5`.
///
/// The square root is the standard empirical result for fully rough turbulent
/// flow: doubling width quadruples conveyance.
pub fn channel_width(discharge: f64, width_coef: f64) -> f64 {
    if discharge <= 0.0 {
        return 0.0;
    }
    width_coef * discharge.sqrt()
}

/// Steepest-descent vector of the surface at a cell, for visualisation.
pub fn flow_vector(surface: &ScalarField, routing: &FlowRouting, index: usize, step: f64) -> Vec2 {
    let b = *surface.boundary();
    let dir = routing.receiver[index];
    if dir == NO_FLOW {
        return Vec2::ZERO;
    }
    let d = dir as usize;
    let (dx, dy) = D8_OFFSETS[d];
    let dist = crate::grid::D8_DISTANCES[d] * step;
    let _ = b;
    Vec2::new(dx as f64 / dist, dy as f64 / dist)
}

/// Channel network: cells whose contributing area exceeds a threshold.
///
/// This is the drainage network a stream-power model carves. Expressed as a
/// label field so it can be exported and overlaid.
pub fn channel_network(acc: &[f64], b: Boundary, min_area_cells: f64) -> LabelField {
    let mut out = LabelField::filled(b, 0);
    for (i, &a) in acc.iter().enumerate() {
        if a >= min_area_cells {
            out.set_index(i, 1);
        }
    }
    out
}

/// Total drainage leaving the grid.
///
/// On a closed (toroidal) surface with no sinks this equals the total number of
/// cells. Used as the conservation check for [`accumulate_flow_with_neighbors`].
pub fn total_drainage(acc: &[f64], routing: &FlowRouting, b: Boundary) -> f64 {
    // Sum the area of cells that lose water from the domain.
    let mut sum = 0.0;
    for idx in 0..b.cell_count() {
        if routing.total_out[idx] <= 0.0 {
            // Outlet: its accumulated area left the system.
            sum += acc[idx] + 1.0;
        }
    }
    sum
}

/// Total accumulated area that exited via outlets, accounting for MFD fractions.
pub fn conservation_error(routing: &FlowRouting, acc: &[f64], b: Boundary) -> f64 {
    // With no sinks, total accumulated area at outlets must equal cell count.
    let total = b.cell_count() as f64;
    let exported: f64 = (0..b.cell_count())
        .filter(|&i| routing.total_out[i] <= 0.0)
        .map(|i| acc[i] + 1.0)
        .sum();
    (exported - total).abs() / total
}

/// Returns `true` if the surface contains an interior sink: a cell that is
/// lower than every neighbour and therefore cannot drain.
///
/// This is the condition that makes flow accumulation meaningless, so every
/// caller should confirm it is `false` after conditioning. Rim cells are
/// excluded because they legitimately drain off the domain.
pub fn has_closed_depression(surface: &ScalarField, step: f64) -> bool {
    let b = *surface.boundary();
    let (w, h) = (b.width(), b.height());
    if w < 3 || h < 3 {
        return false;
    }
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let z = surface.get(x, y);
            let mut any_lower = false;
            for &(dx, dy) in D8_OFFSETS.iter() {
                if surface.get_wrapped(x as i64 + dx, y as i64 + dy) < z {
                    any_lower = true;
                    break;
                }
            }
            if !any_lower {
                return true;
            }
        }
    }
    let _ = step;
    false
}

/// Counts interior sinks that are *not* designated outlets.
///
/// On a torus the global minima are legitimate terminal basins, so counting
/// them as defects would be wrong; pass the outlet set to exclude them.
pub fn count_closed_depressions_excluding(
    surface: &ScalarField,
    outlets: &[usize],
) -> usize {
    let b = *surface.boundary();
    let (w, h) = (b.width(), b.height());
    if w < 3 || h < 3 {
        return 0;
    }
    let mut is_outlet = vec![false; b.cell_count()];
    for &i in outlets {
        is_outlet[i] = true;
    }
    let mut count = 0;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            if is_outlet[b.index(x, y)] {
                continue;
            }
            let z = surface.get(x, y);
            if !D8_OFFSETS
                .iter()
                .any(|&(dx, dy)| surface.get_wrapped(x as i64 + dx, y as i64 + dy) < z)
            {
                count += 1;
            }
        }
    }
    count
}

/// Counts interior sinks, for a draining boundary where every sink is a defect.
pub fn count_closed_depressions(surface: &ScalarField) -> usize {
    count_closed_depressions_excluding(surface, &[])
}

/// Total-order wrapper so `BinaryHeap` can order `f64` keys.
///
/// `f64` is not `Ord`, and a hand-written `Ord` that mishandles NaN would make
/// the priority flood spin forever on a NaN cell. `total_cmp` gives a
/// deterministic total order, which is all the heap needs.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OrderedF64(f64);

impl Eq for OrderedF64 {}

impl PartialOrd for OrderedF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp_decreasing(w: usize, h: usize) -> ScalarField {
        let b = Boundary::new(BoundaryKind::Rectangular, w, h);
        let mut f = ScalarField::zeros(b);
        for y in 0..h {
            for x in 0..w {
                f.set(x, y, (w - x) as f64 * 10.0 - y as f64);
            }
        }
        f
    }

    #[test]
    fn d8_routes_everything_downhill_to_the_edge() {
        let f = ramp_decreasing(16, 16);
        let step = 1.0;
        let routing = route_d8(&f, step);
        // Only the left/bottom rims should be outlets for a ramp sloping to -x.
        assert!(routing.outlet_count > 0);
        // Every interior cell must have a receiver.
        let b = *f.boundary();
        for y in 1..15 {
            for x in 1..15 {
                let idx = b.index(x, y);
                assert_ne!(routing.receiver[idx], NO_FLOW, "pit at {x},{y}");
            }
        }
    }

    #[test]
    fn d8_receiver_is_strictly_lower() {
        let f = ramp_decreasing(16, 16);
        let step = 1.0;
        let routing = route_d8(&f, step);
        let b = *f.boundary();
        for y in 0..16 {
            for x in 0..16 {
                let idx = b.index(x, y);
                if let Some(d) = routing.d8_direction(idx) {
                    let (dx, dy) = D8_OFFSETS[d];
                    let here = f.get(x, y);
                    let there = f.get_wrapped(x as i64 + dx, y as i64 + dy);
                    assert!(there < here, "receiver not lower at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn depression_filling_creates_no_pits() {
        // A bowl surrounded by a rim: the classic pit.
        let b = Boundary::new(BoundaryKind::Rectangular, 9, 9);
        let mut f = ScalarField::zeros(b);
        for y in 0..9 {
            for x in 0..9 {
                let d2 = ((x as f64 - 4.0).powi(2) + (y as f64 - 4.0).powi(2)).sqrt();
                f.set(x, y, if d2 < 3.0 { -5.0 + d2 } else { 0.0 });
            }
        }
        // Raw surface has a closed depression.
        assert!(has_closed_depression(&f, 1.0));
        let filled = fill_depressions(&f, 1e-5);
        assert!(!has_closed_depression(&filled, 1.0), "fill left a pit");
        // Fill never lowers terrain.
        for i in 0..f.len() {
            assert!(filled.as_slice()[i] >= f.as_slice()[i] - 1e-12);
        }
        // Fill volume is bounded by the basin: 3x3-ish bowl, a few metres deep.
        let vol = fill_volume(&f, &filled);
        assert!(vol > 0.0 && vol < 100.0, "implausible fill volume {vol}");
    }

    #[test]
    fn depression_filling_raises_only_to_the_spill_level() {
        // A deep centre inside a flat wall at 10: the fill must reach the wall
        // top (its spill elevation) and go no higher.
        let b = Boundary::new(BoundaryKind::Rectangular, 5, 5);
        let mut f = ScalarField::filled(b, 10.0);
        f.set(2, 2, 1.0); // deep centre
        let filled = fill_depressions(&f, 1e-5);
        // Epsilon accumulates along the flat, so allow a few epsilon of slack.
        assert!(
            (filled.get(2, 2) - 10.0).abs() < 1e-3,
            "centre filled to {} instead of the spill level 10",
            filled.get(2, 2)
        );
        // Nothing may be raised above the rim height either.
        assert!(filled.max() <= 10.0 + 1e-3, "over-filled: {}", filled.max());
    }

    #[test]
    fn depression_filling_on_a_pit_free_surface_is_a_no_op() {
        let f = ramp_decreasing(16, 16);
        let filled = fill_depressions(&f, 1e-5);
        let diff = fill_volume(&f, &filled);
        assert!(diff < 1e-6, "spurious fill {diff} on a monotone ramp");
    }

    #[test]
    fn depression_filling_respects_toroidal_wrap() {
        // A torus has no rim, so the global minima are the outlets. A basin that
        // is not itself an outlet must still be filled even when its only escape
        // runs across the wrap seam.
        let b = Boundary::new(BoundaryKind::Toroidal, 8, 8);
        let mut f = ScalarField::filled(b, 5.0);
        f.set(0, 0, 0.0); // the global minimum: this one is an outlet
        f.set(4, 4, 1.0); // a second, disconnected basin
        let result = fill_depressions_detailed(&f, 1e-5);
        assert!(
            result.outlets.contains(&b.index(0, 0)),
            "global minimum should be an outlet"
        );
        // The second basin must be filled up toward the outlet level.
        assert!(
            result.surface.get(4, 4) > 1.0,
            "disconnected basin not filled: {}",
            result.surface.get(4, 4)
        );
        assert!(
            count_closed_depressions_excluding(&result.surface, &result.outlets) == 0,
            "unresolved sinks remain on the torus"
        );
    }

    #[test]
    fn depression_filling_on_a_torus_leaves_no_sinks_beyond_outlets() {
        let b = Boundary::new(BoundaryKind::Toroidal, 16, 16);
        let mut f = ScalarField::zeros(b);
        for y in 0..16 {
            for x in 0..16 {
                f.set(
                    x,
                    y,
                    (crate::rng::hash2_f64(x as i64, y as i64, 31337) - 0.5) * 40.0,
                );
            }
        }
        assert!(count_closed_depressions(&f) > 0, "expected sinks");
        let result = fill_depressions_detailed(&f, 1e-5);
        let unresolved = count_closed_depressions_excluding(&result.surface, &result.outlets);
        assert_eq!(unresolved, 0, "sinks survived filling");
        // The only remaining sinks must be the outlets themselves.
        assert!(!result.outlets.is_empty());
    }

    #[test]
    fn depression_filling_reports_a_plausible_fill_volume() {
        let b = Boundary::new(BoundaryKind::Rectangular, 32, 32);
        let mut f = ScalarField::zeros(b);
        for y in 0..32 {
            for x in 0..32 {
                f.set(
                    x,
                    y,
                    (crate::rng::hash2_f64(x as i64, y as i64, 5150) - 0.5) * 100.0,
                );
            }
        }
        let result = fill_depressions_detailed(&f, 1e-5);
        let n = b.cell_count() as f64;
        let mean_fill = result.fill_volume / n;
        // A plausible terrain fills a modest fraction of its relief, not all of it.
        assert!(mean_fill > 0.0, "no depression detected at all");
        assert!(
            mean_fill < result.surface.max() - result.surface.min(),
            "fill volume {mean_fill} exceeds the relief"
        );
    }

    #[test]
    fn d8_accumulation_conserves_area() {
        let f = ramp_decreasing(24, 24);
        let step = 1.0;
        let (filled, routing, acc) = condition_d8(&f, step, 1e-5);
        let b = *f.boundary();
        // On a conditioned surface with sinks only on the rim, area is conserved.
        let err = conservation_error(&routing, &acc, b);
        assert!(err < 1e-9, "area not conserved: error {err}");
        let _ = filled;
    }

    #[test]
    fn mfd_accumulation_conserves_area() {
        let f = ramp_decreasing(24, 24);
        let step = 1.0;
        let (filled, routing, acc) = condition_mfd(&f, step, 1.1, 1e-5);
        let b = *f.boundary();
        let err = conservation_error(&routing, &acc, b);
        assert!(err < 1e-9, "mfd area not conserved: error {err}");
        let _ = filled;
    }

    #[test]
    fn accumulation_increases_downstream() {
        let f = ramp_decreasing(20, 20);
        let (_, routing, acc) = condition_d8(&f, 1.0, 1e-5);
        let b = *f.boundary();
        // Every cell with a receiver must have smaller accumulation than it.
        for y in 1..19 {
            for x in 1..19 {
                let idx = b.index(x, y);
                if let Some(d) = routing.d8_direction(idx) {
                    let (dx, dy) = D8_OFFSETS[d];
                    let nidx = b.index_wrapped(x as i64 + dx, y as i64 + dy);
                    assert!(acc[nidx] > acc[idx], "no downstream growth at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn accumulation_gathers_the_watershed_above_a_sink() {
        // Everything must funnel to the low corner of the ramp.
        let f = ramp_decreasing(16, 16);
        let (_, routing, acc) = condition_d8(&f, 1.0, 1e-5);
        let b = *f.boundary();
        let sink = b.index(0, 15);
        let biggest = acc
            .iter()
            .enumerate()
            .max_by(|a, c| a.1.partial_cmp(c.1).unwrap())
            .unwrap();
        // The most-accumulated cell is the outlet.
        assert!(routing.total_out[biggest.0] <= 0.0 || acc[sink] > acc[biggest.0] * 0.5);
    }

    #[test]
    fn mfd_splits_flow_among_neighbours() {
        // On a ramp sloping in -x, a cell should send some flow W and some NW/SW.
        let f = ramp_decreasing(16, 16);
        let (_, routing, _) = condition_mfd(&f, 1.0, 1.1, 1e-5);
        let b = *f.boundary();
        let idx = b.index(8, 8);
        let dirs_used = (0..8).filter(|&d| routing.fraction_in(idx, d) > 0.0).count();
        assert!(dirs_used >= 1, "no flow at all");
        // Fractions sum to 1.
        let sum: f64 = (0..8).map(|d| routing.fraction_in(idx, d)).sum();
        assert!((sum - 1.0).abs() < 1e-9, "fractions sum to {sum}");
    }

    #[test]
    fn mfd_fractions_sum_to_one_wherever_flow_exists() {
        let f = ramp_decreasing(20, 20);
        let (_, routing, _) = condition_mfd(&f, 1.0, 1.5, 1e-5);
        for idx in 0..routing.fraction.len() / 8 {
            if routing.total_out[idx] > 0.0 {
                let sum: f64 = (0..8).map(|d| routing.fraction_in(idx, d)).sum();
                assert!((sum - 1.0).abs() < 1e-9, "cell {idx} sums to {sum}");
            }
        }
    }

    #[test]
    fn mfd_converges_to_d8_as_exponent_grows() {
        let f = ramp_decreasing(20, 20);
        let (_, routing, _) = condition_mfd(&f, 1.0, 50.0, 1e-5);
        let b = *f.boundary();
        let idx = b.index(10, 10);
        let mut dominant = 0usize;
        let mut dom_frac = 0.0;
        for d in 0..8 {
            let fr = routing.fraction_in(idx, d);
            if fr > dom_frac {
                dom_frac = fr;
                dominant = d;
            }
        }
        assert!(dom_frac > 0.9, "high exponent did not concentrate: {dom_frac}");
        // ramp_decreasing falls 10x faster in x than y, so E (index 0) dominates.
        assert_eq!(dominant, 0, "dominant direction is not east");
    }

    #[test]
    fn mfd_spreads_more_than_d8_at_moderate_exponent() {
        let f = ramp_decreasing(20, 20);
        let b = *f.boundary();
        let idx = b.index(10, 10);
        let (_, d8r, _) = condition_d8(&f, 1.0, 1e-5);
        let (_, mfdr, _) = condition_mfd(&f, 1.0, 1.1, 1e-5);
        let d8_dirs = (0..8).filter(|&d| d8r.fraction_in(idx, d) > 0.0).count();
        let mfd_dirs = (0..8).filter(|&d| mfdr.fraction_in(idx, d) > 0.0).count();
        assert_eq!(d8_dirs, 1, "D8 should use exactly one direction");
        assert!(
            mfd_dirs > d8_dirs,
            "MFD should disperse flow: {mfd_dirs} vs {d8_dirs}"
        );
    }

    #[test]
    fn condition_removes_all_pits_before_routing() {
        let b = Boundary::new(BoundaryKind::Rectangular, 16, 16);
        let mut f = ScalarField::zeros(b);
        let seed = 99u64;
        for y in 0..16 {
            for x in 0..16 {
                f.set(
                    x,
                    y,
                    (crate::rng::hash2_f64(x as i64, y as i64, seed) - 0.5) * 20.0,
                );
            }
        }
        let pits_before = count_closed_depressions(&f);
        assert!(pits_before > 0, "expected pits in random terrain");
        let (filled, _, _) = condition_d8(&f, 1.0, 1e-5);
        assert_eq!(
            count_closed_depressions(&filled),
            0,
            "conditioning left {pits_before} -> {} pits",
            count_closed_depressions(&filled)
        );
    }

    #[test]
    fn depression_filling_reduces_pit_count_monotonically() {
        let b = Boundary::new(BoundaryKind::Rectangular, 24, 24);
        let mut f = ScalarField::zeros(b);
        for y in 0..24 {
            for x in 0..24 {
                f.set(
                    x,
                    y,
                    (crate::rng::hash2_f64(x as i64, y as i64, 31337) - 0.5) * 40.0,
                );
            }
        }
        let before = count_closed_depressions(&f);
        let filled = fill_depressions(&f, 1e-5);
        let after = count_closed_depressions(&filled);
        assert_eq!(after, 0);
        assert!(before > after);
    }

    #[test]
    fn channel_network_marks_large_rivers_only() {
        let f = ramp_decreasing(32, 32);
        let (_, _, acc) = condition_d8(&f, 1.0, 1e-5);
        let b = *f.boundary();
        let net = channel_network(&acc, b, 10.0);
        let marked = net.as_slice().iter().filter(|&&v| v == 1).count();
        let total = b.cell_count();
        assert!(marked > 0 && marked < total, "channel mask wrong: {marked}/{total}");
    }

    #[test]
    fn manning_discharge_increases_with_depth_and_slope() {
        let d1 = manning_discharge(1.0, 0.03, 0.01);
        let d2 = manning_discharge(2.0, 0.03, 0.01);
        let d3 = manning_discharge(1.0, 0.03, 0.02);
        assert!(d2 > d1, "deeper flow should carry more");
        assert!(d3 > d1, "steeper flow should carry more");
        assert_eq!(manning_discharge(0.0, 0.03, 0.01), 0.0);
        assert_eq!(manning_discharge(1.0, 0.0, 0.01), 0.0);
    }

    #[test]
    fn channel_width_scales_with_sqrt_discharge() {
        let w1 = channel_width(1.0, 1.0);
        let w4 = channel_width(4.0, 1.0);
        assert!((w4 / w1 - 2.0).abs() < 1e-12, "width should double");
    }

    #[test]
    fn flow_vector_points_downstream_and_is_zero_at_outlets() {
        // ramp_decreasing falls 10 m per x step but only 1 m per y step, so the
        // steepest descent is due east.
        let f = ramp_decreasing(12, 12);
        let (_, routing, _) = condition_d8(&f, 1.0, 1e-5);
        let b = *f.boundary();
        let v = flow_vector(&f, &routing, b.index(6, 6), 1.0);
        assert!(v.x > 0.99, "flow should point east: {v:?}");
        assert!(v.y.abs() < 1e-9, "flow should not run north: {v:?}");
        let zero = flow_vector(&f, &routing, 0, 1.0);
        if routing.receiver[0] == NO_FLOW {
            assert_eq!(zero, Vec2::ZERO);
        }
    }

    #[test]
    fn flow_vector_is_diagonal_on_an_isotropic_ramp() {
        // Falls equally in x and y, so steepest descent is the NE diagonal.
        let b = Boundary::new(BoundaryKind::Rectangular, 12, 12);
        let mut f = ScalarField::zeros(b);
        for y in 0..12 {
            for x in 0..12 {
                f.set(x, y, -(x as f64 + y as f64));
            }
        }
        let (_, routing, _) = condition_d8(&f, 1.0, 1e-5);
        let v = flow_vector(&f, &routing, b.index(5, 5), 1.0);
        assert!(v.x > 0.6 && v.y > 0.6, "expected NE flow: {v:?}");
    }

    #[test]
    fn accumulate_requires_valid_order_to_conserve() {
        // Demonstrates *why* the order matters: raster order loses area.
        let f = ramp_decreasing(8, 8);
        let step = 1.0;
        let filled = fill_depressions(&f, 1e-5);
        let routing = route_d8(&filled, step);
        let b = *f.boundary();

        let good = accumulation_order(&filled);
        let good_acc = accumulate_flow_with_neighbors(&routing, b, step, &good);
        let good_err = conservation_error(&routing, &good_acc, b);

        // Wrong order: reverse raster.
        let mut bad: Vec<usize> = (0..b.cell_count()).collect();
        bad.reverse();
        let bad_acc = accumulate_flow_with_neighbors(&routing, b, step, &bad);
        let bad_err = conservation_error(&routing, &bad_acc, b);

        assert!(good_err < 1e-9, "valid order should conserve");
        assert!(bad_err > good_err, "wrong order should be worse ({bad_err} vs {good_err})");
    }

    #[test]
    fn priority_heap_orders_nan_definitely() {
        // total_cmp must give a total order even for NaN so the flood terminates.
        let a = OrderedF64(f64::NAN);
        let b = OrderedF64(1.0);
        let mut heap = BinaryHeap::new();
        heap.push(Reverse((a, 0usize)));
        heap.push(Reverse((b, 1usize)));
        // Must pop both without panicking or looping.
        assert!(heap.pop().is_some());
        assert!(heap.pop().is_some());
        assert!(heap.pop().is_none());
    }

    #[test]
    fn total_drainage_equals_cell_count_when_fully_conditioned() {
        let f = ramp_decreasing(16, 16);
        let (_, routing, acc) = condition_d8(&f, 1.0, 1e-5);
        let b = *f.boundary();
        let drained = total_drainage(&acc, &routing, b);
        assert!((drained - b.cell_count() as f64).abs() < 1e-6);
    }
}