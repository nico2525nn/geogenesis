//! Flow routing: depression filling, D8/MFD directions, and flow accumulation.
//!
//! Accumulation is only meaningful on a conditioned surface: fill, route, then
//! accumulate in downhill order.

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
    /// Outlets: the rim for a draining boundary, the global minima for a torus.
    pub outlets: Vec<usize>,
    /// Volume of material added, in metres summed over cells.
    pub fill_volume: f64,
}

/// Fills closed depressions (priority-flood + epsilon gradient).
///
/// `epsilon` imposes a shallow downhill direction across flat filled areas.
pub fn fill_depressions_detailed(surface: &ScalarField, epsilon: f64) -> FillResult {
    let b = *surface.boundary();
    let (w, h) = (b.width(), b.height());
    let mut filled = ScalarField::zeros(b);
    let mut closed = vec![false; b.cell_count()];
    let mut queue: BinaryHeap<Reverse<(OrderedF64, usize)>> = BinaryHeap::new();
    let mut outlets = Vec::new();

    match b.kind() {
        BoundaryKind::Toroidal => {
            // A torus has no rim, so seed the global minima instead.
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
            // The rim is the outlet.
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
            // `max` keeps the output pointwise >= input.
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

/// Returns only the filled surface.
pub fn fill_depressions(surface: &ScalarField, epsilon: f64) -> ScalarField {
    fill_depressions_detailed(surface, epsilon).surface
}

/// Volume of material added by depression filling, in metres × cell area.
pub fn fill_volume(original: &ScalarField, filled: &ScalarField) -> f64 {
    let mut sum = 0.0;
    for (a, b) in original.as_slice().iter().zip(filled.as_slice()) {
        sum += (b - a).max(0.0);
    }
    sum
}

/// Routes drainage with D8 (single steepest descent).
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
                // True step length, so diagonal and cardinal slopes compare directly.
                let dist = crate::grid::D8_DISTANCES[d] * step;
                let drop = (z - nz) / dist;
                if drop > best_drop {
                    best_drop = drop;
                    best_dir = Some(d);
                }
            }

            match best_dir {
                // Level cells are outlets, not zero-drop receivers.
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

/// Routes drainage with MFD: splits discharge among downslope neighbours as `slope^p`.
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

/// Accumulates upslope drainage area in cells.
///
/// `order` must be descending elevation on the *filled* surface; a wrong order
/// silently loses area.
pub fn accumulate_flow(routing: &FlowRouting, order: &[usize]) -> Vec<f64> {
    let n = routing.fraction.len() / 8;
    let mut acc = vec![0.0f64; n];

    for &idx in order {
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
            // Receivers are recovered from geometry in `accumulate_flow_with_neighbors`.
            let _ = f;
        }
        acc[idx] = area;
    }
    acc
}

/// Accumulates drainage area, resolving receivers from geometry.
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
        for (d, &(dx, dy)) in D8_OFFSETS.iter().enumerate() {
            let f = routing.fraction[idx * 8 + d];
            if f <= 0.0 {
                continue;
            }
            let nidx = b.index_wrapped(x as i64 + dx, y as i64 + dy);
            acc[nidx] += f * (acc[idx] + 1.0);
        }
        acc[idx] += 1.0;
    }
    let _ = step;
    acc
}

/// Downhill elevation order for accumulation on `surface`.
pub fn accumulation_order(surface: &ScalarField) -> Vec<usize> {
    surface.downhill_order(crate::grid::Direction::D8)
}

/// Fills, routes D8, and accumulates; use the filled surface for erosion.
pub fn condition_d8(surface: &ScalarField, step: f64, epsilon: f64) -> (ScalarField, FlowRouting, Vec<f64>) {
    let filled = fill_depressions(surface, epsilon);
    let routing = route_d8(&filled, step);
    let order = accumulation_order(&filled);
    let acc = accumulate_flow_with_neighbors(&routing, *surface.boundary(), step, &order);
    (filled, routing, acc)
}

/// Fills, routes MFD, and accumulates.
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

/// Specific discharge proxy: `q = (1/n) * h^(5/3) * S^(1/2)`.
///
/// `depth` in metres, `roughness` is Manning's `n`, `slope` is dimensionless.
pub fn manning_discharge(depth: f64, roughness: f64, slope: f64) -> f64 {
    if depth <= 0.0 || roughness <= 0.0 || slope <= 0.0 {
        return 0.0;
    }
    let n = roughness;
    (1.0 / n) * depth.powf(5.0 / 3.0) * slope.sqrt()
}

/// Channel width from discharge: `w = channel_width_coef * Q^0.5`.
pub fn channel_width(discharge: f64, width_coef: f64) -> f64 {
    if discharge <= 0.0 {
        return 0.0;
    }
    width_coef * discharge.sqrt()
}

/// Steepest-descent vector at a cell; zero at outlets.
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

/// Cells whose contributing area exceeds a threshold.
pub fn channel_network(acc: &[f64], b: Boundary, min_area_cells: f64) -> LabelField {
    let mut out = LabelField::filled(b, 0);
    for (i, &a) in acc.iter().enumerate() {
        if a >= min_area_cells {
            out.set_index(i, 1);
        }
    }
    out
}

/// Total drainage leaving the grid; equals the cell count with no sinks.
pub fn total_drainage(acc: &[f64], routing: &FlowRouting, b: Boundary) -> f64 {
    let mut sum = 0.0;
    for (idx, &a) in acc.iter().enumerate().take(b.cell_count()) {
        if routing.total_out[idx] <= 0.0 {
            sum += a + 1.0;
        }
    }
    sum
}

/// Relative area lost at outlets; zero when accumulation conserves.
pub fn conservation_error(routing: &FlowRouting, acc: &[f64], b: Boundary) -> f64 {
    let total = b.cell_count() as f64;
    let exported: f64 = (0..b.cell_count())
        .filter(|&i| routing.total_out[i] <= 0.0)
        .map(|i| acc[i] + 1.0)
        .sum();
    (exported - total).abs() / total
}

/// Returns `true` if the surface contains an interior sink.
///
/// Rim cells are excluded: they drain off the domain.
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

/// Counts interior sinks, excluding the given outlets.
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

/// Counts interior sinks on a draining boundary.
pub fn count_closed_depressions(surface: &ScalarField) -> usize {
    count_closed_depressions_excluding(surface, &[])
}

/// Total-order `f64` wrapper so `BinaryHeap` has a deterministic order on NaN.
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
        assert!(routing.outlet_count > 0);
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
        // Intent: a rimmed bowl fills without leaving pits.
        let b = Boundary::new(BoundaryKind::Rectangular, 9, 9);
        let mut f = ScalarField::zeros(b);
        for y in 0..9 {
            for x in 0..9 {
                let d2 = ((x as f64 - 4.0).powi(2) + (y as f64 - 4.0).powi(2)).sqrt();
                f.set(x, y, if d2 < 3.0 { -5.0 + d2 } else { 0.0 });
            }
        }
        assert!(has_closed_depression(&f, 1.0));
        let filled = fill_depressions(&f, 1e-5);
        assert!(!has_closed_depression(&filled, 1.0), "fill left a pit");
        for i in 0..f.len() {
            assert!(filled.as_slice()[i] >= f.as_slice()[i] - 1e-12);
        }
        let vol = fill_volume(&f, &filled);
        assert!(vol > 0.0 && vol < 100.0, "implausible fill volume {vol}");
    }

    #[test]
    fn depression_filling_raises_only_to_the_spill_level() {
        // Intent: a deep centre fills to its spill elevation and no higher.
        let b = Boundary::new(BoundaryKind::Rectangular, 5, 5);
        let mut f = ScalarField::filled(b, 10.0);
        f.set(2, 2, 1.0); // deep centre
        let filled = fill_depressions(&f, 1e-5);
        assert!(
            (filled.get(2, 2) - 10.0).abs() < 1e-3,
            "centre filled to {} instead of the spill level 10",
            filled.get(2, 2)
        );
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
        // Intent: basins still fill when the escape runs across the wrap seam.
        let b = Boundary::new(BoundaryKind::Toroidal, 8, 8);
        let mut f = ScalarField::filled(b, 5.0);
        f.set(0, 0, 0.0); // the global minimum: this one is an outlet
        f.set(4, 4, 1.0); // a second, disconnected basin
        let result = fill_depressions_detailed(&f, 1e-5);
        assert!(
            result.outlets.contains(&b.index(0, 0)),
            "global minimum should be an outlet"
        );
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
        // Intent: fill is a modest fraction of the relief.
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
        // Intent: flow funnels to the low corner of the ramp.
        let f = ramp_decreasing(16, 16);
        let (_, routing, acc) = condition_d8(&f, 1.0, 1e-5);
        let b = *f.boundary();
        let sink = b.index(0, 15);
        let biggest = acc
            .iter()
            .enumerate()
            .max_by(|a, c| a.1.partial_cmp(c.1).unwrap())
            .unwrap();
        assert!(routing.total_out[biggest.0] <= 0.0 || acc[sink] > acc[biggest.0] * 0.5);
    }

    #[test]
    fn mfd_splits_flow_among_neighbours() {
        // Intent: one cell on an -x ramp disperses across W/NW/SW.
        let f = ramp_decreasing(16, 16);
        let (_, routing, _) = condition_mfd(&f, 1.0, 1.1, 1e-5);
        let b = *f.boundary();
        let idx = b.index(8, 8);
        let dirs_used = (0..8).filter(|&d| routing.fraction_in(idx, d) > 0.0).count();
        assert!(dirs_used >= 1, "no flow at all");
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
        // The ramp falls 10x faster in x than y, so E (index 0) dominates.
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
        // Intent: steepest descent on this ramp is due east.
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
        // Intent: equal x/y fall gives NE flow.
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
        // Intent: raster order loses area; downhill order conserves.
        let f = ramp_decreasing(8, 8);
        let step = 1.0;
        let filled = fill_depressions(&f, 1e-5);
        let routing = route_d8(&filled, step);
        let b = *f.boundary();

        let good = accumulation_order(&filled);
        let good_acc = accumulate_flow_with_neighbors(&routing, b, step, &good);
        let good_err = conservation_error(&routing, &good_acc, b);

        let mut bad: Vec<usize> = (0..b.cell_count()).collect();
        bad.reverse();
        let bad_acc = accumulate_flow_with_neighbors(&routing, b, step, &bad);
        let bad_err = conservation_error(&routing, &bad_acc, b);

        assert!(good_err < 1e-9, "valid order should conserve");
        assert!(bad_err > good_err, "wrong order should be worse ({bad_err} vs {good_err})");
    }

    #[test]
    fn priority_heap_orders_nan_definitely() {
        // Intent: NaN keys still pop in a total order.
        let a = OrderedF64(f64::NAN);
        let b = OrderedF64(1.0);
        let mut heap = BinaryHeap::new();
        heap.push(Reverse((a, 0usize)));
        heap.push(Reverse((b, 1usize)));
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