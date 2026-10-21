//! Hydrology: depression filling, flow routing, drainage accumulation, lakes
//! and discharge (spec §3a–3b).
//!
//! Routing is a priority flood (Barnes et al., 2014) over a *monotone radix
//! heap*. The flood visits cells in non-decreasing spill elevation, which is
//! exactly the monotone-key access pattern a radix heap is built for, so it
//! replaces the `BinaryHeap` of the classic implementation with an amortised
//! O(1) bucket queue. Closed basins are raised to their spill level in the same
//! pass, so the filled surface drains everywhere and water ponding in the filled
//! part becomes a lake.
//!
//! Flow directions are a true D8 steepest descent on the filled surface, with
//! the flood order used only to cross flat lake surfaces (where no neighbour is
//! lower). That keeps receivers acyclic, gives sharp channels and a
//! well-defined stream-power slope, and still terminates in a lake's outlet.
//!
//! A multiple-flow-direction option (`hydrology.routing`) partitions flow over
//! all downhill neighbours instead of one; it is used for accumulation and
//! discharge, while the primary D8 receiver still defines the channel path.

use crate::climate::Climate;
use crate::grid::{BoundaryKind, Field, Grid};
use crate::params::Params;
use crate::world::World;

/// Result of routing water across the current surface.
#[derive(Clone, Debug)]
pub struct Hydro {
    /// Grid geometry.
    pub grid: Grid,
    /// Depression-filled surface, metres.
    pub filled_m: Field<f64>,
    /// Downstream cell index per cell, `-1` for outlets/ocean.
    pub receiver: Vec<i32>,
    /// D8 direction (0..7) toward the receiver; `255` when none.
    pub flow_dir: Field<u8>,
    /// Upstream catchment area in cells.
    pub accumulation_cells: Field<f64>,
    /// Upstream catchment area in m².
    pub drainage_area_m2: Field<f64>,
    /// Water discharge, m³/yr.
    pub discharge_m3_yr: Field<f64>,
    /// Standing water depth above the land surface, metres.
    pub lake_depth_m: Field<f32>,
    /// Channel mask: 0 hillslope, 1 channel, 2 major river.
    pub river_mask: Field<u8>,
    /// Cells in upstream-to-downstream order (topological).
    pub order_upstream: Vec<u32>,
    /// Cells in downstream-to-upstream order.
    pub order_downstream: Vec<u32>,
    /// Total surface runoff used for routing, mm/yr (mean over the grid).
    pub mean_runoff_mm: f64,
    /// Multiple-flow-direction partition, present only in MFD mode.
    pub mfd: Option<MfdRouting>,
}

/// Multiple-flow-direction partition: for every cell, up to eight downhill
/// directions and the fraction of that cell's flow sent along each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MfdRouting {
    /// Per-cell D8 direction slots (`255` = unused).
    pub dirs: Vec<[u8; 8]>,
    /// Per-cell flow fractions; the used slots sum to one.
    pub weights: Vec<[f32; 8]>,
}

impl MfdRouting {
    /// Allocates an empty partition for `cells` cells.
    fn new(cells: usize) -> Self {
        Self {
            dirs: vec![[255u8; 8]; cells],
            weights: vec![[0.0f32; 8]; cells],
        }
    }

    /// Downhill directions and flow fractions for one cell.
    pub fn downstream(&self, index: usize) -> impl Iterator<Item = (usize, f64)> + '_ {
        let dirs = self.dirs.get(index).copied().unwrap_or([255u8; 8]);
        let weights = self.weights.get(index).copied().unwrap_or([0.0f32; 8]);
        dirs.into_iter()
            .zip(weights)
            .filter(|(dir, weight)| *dir < 8 && *weight > 0.0)
            .map(|(dir, weight)| (dir as usize, f64::from(weight)))
    }

    /// Number of cells covered by the partition.
    #[must_use]
    pub fn len(&self) -> usize {
        self.dirs.len()
    }

    /// True when no cell has a partition.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }
}

impl Hydro {
    /// An empty hydro state for construction before the first routing pass.
    #[must_use]
    pub fn empty(grid: Grid) -> Self {
        let n = grid.cell_count();
        Self {
            grid,
            filled_m: Field::zeros(grid),
            receiver: vec![-1; n],
            flow_dir: Field::filled(grid, 255),
            accumulation_cells: Field::zeros(grid),
            drainage_area_m2: Field::zeros(grid),
            discharge_m3_yr: Field::zeros(grid),
            lake_depth_m: Field::zeros(grid),
            river_mask: Field::zeros(grid),
            order_upstream: Vec::new(),
            order_downstream: Vec::new(),
            mean_runoff_mm: 0.0,
            mfd: None,
        }
    }

    /// Receivers as an `Option`-friendly view.
    #[must_use]
    pub fn receiver_of(&self, index: usize) -> Option<usize> {
        let r = self.receiver.get(index).copied().unwrap_or(-1);
        if r < 0 {
            None
        } else {
            Some(r as usize)
        }
    }

    /// MFD partition, when this routing pass used multiple flow directions.
    #[must_use]
    pub fn mfd_routing(&self) -> Option<&MfdRouting> {
        self.mfd.as_ref()
    }

    /// True when the cell drains through more than one downstream direction.
    #[must_use]
    pub fn uses_mfd(&self) -> bool {
        self.mfd.is_some()
    }

    /// Lake volume, m³.
    ///
    /// Uses the physical area of each cell, so on a spherical grid polar lakes
    /// no longer count as large as equatorial ones of the same depth.
    #[must_use]
    pub fn lake_volume_m3(&self) -> f64 {
        let grid = self.grid;
        let mut volume = 0.0;
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let depth = f64::from(self.lake_depth_m.get(x, y)).max(0.0);
                if depth > 0.0 {
                    volume += depth * grid.cell_area_m2_at(x, y);
                }
            }
        }
        volume
    }

    /// Number of channel cells.
    #[must_use]
    pub fn channel_cells(&self) -> usize {
        self.river_mask
            .as_slice()
            .iter()
            .filter(|&&m| m > 0)
            .count()
    }

    /// Drainage density in km per km² (channel length / basin area).
    #[must_use]
    pub fn drainage_density_km_per_km2(&self) -> f64 {
        let grid = self.grid;
        let total_km2 = if grid.has_spherical_areas() {
            grid.total_area_m2() / 1e6
        } else {
            grid.cell_count() as f64 * (grid.cell_area_m2() / 1e6)
        };
        if total_km2 <= 0.0 {
            return 0.0;
        }
        let channel_km = self.channel_cells() as f64 * grid.cell_size_m() / 1000.0;
        channel_km / total_km2
    }

    /// Longest flow path in cells from any channel head to an outlet.
    #[must_use]
    pub fn longest_flow_path_cells(&self) -> usize {
        let mut lengths = vec![0u32; self.grid.cell_count()];
        for &u in &self.order_upstream {
            let next = lengths[u as usize] + 1;
            if let Some(r) = self.receiver_of(u as usize) {
                lengths[r] = lengths[r].max(next);
            }
        }
        lengths.iter().copied().max().unwrap_or(0) as usize
    }
}

/// Number of low bits of a packed priority key reserved for the cell index.
///
/// The largest supported grid is 16384² = 2²⁸ cells, so 28 bits cover every
/// index exactly. The remaining 36 bits keep the elevation ordering; the
/// resulting ~1e-7 m quantisation is far below any physical elevation scale and
/// only ever reorders cells whose spill levels are equal to within that
/// tolerance.
const INDEX_BITS: u32 = 28;
const INDEX_MASK: u64 = (1u64 << INDEX_BITS) - 1;

/// Maps a finite elevation to an order-preserving unsigned integer key.
#[inline]
fn elevation_key(z: f64) -> u64 {
    let bits = z.to_bits();
    if bits & (1 << 63) == 0 {
        // Non-negative floats order as their bit patterns; flip the sign bit so
        // they sort above every negative value.
        bits | (1 << 63)
    } else {
        // Negative floats order opposite to their bit patterns.
        !bits
    }
}

/// Packs a cell index and its (quantised) elevation key into one `u64`, so the
/// priority queue only moves 8-byte words.
#[inline]
fn pack_cell(index: usize, z: f64) -> u64 {
    (elevation_key(z) & !INDEX_MASK) | index as u64
}

/// Computes an order-preserving dense rank for every cell: rank 0 is the
/// lowest elevation.
///
/// The ranks are the index of each cell in elevation order, produced by a
/// stable 3-pass 12-bit LSD radix sort of the packed elevation keys. A dense
/// rank lets the flood use direct-addressed buckets instead of a comparison
/// heap, which is the difference between an O(n log n) and an O(n) fill.
fn elevation_ranks(elevation: &[f64]) -> Vec<u32> {
    let n = elevation.len();
    let mut packed: Vec<u64> = (0..n).map(|i| pack_cell(i, elevation[i])).collect();
    let mut scratch: Vec<u64> = vec![0; n];
    let mut counts = [0u32; 1 << 12];
    // The key occupies bits 28..64 of the packed word (see `pack_cell`).
    for shift in [28u32, 40, 52] {
        counts.fill(0);
        for &value in &packed {
            counts[((value >> shift) & 0x0FFF) as usize] += 1;
        }
        let mut sum = 0u32;
        for count in &mut counts {
            let taken = *count;
            *count = sum;
            sum += taken;
        }
        for &value in &packed {
            let digit = ((value >> shift) & 0x0FFF) as usize;
            scratch[counts[digit] as usize] = value;
            counts[digit] += 1;
        }
        std::mem::swap(&mut packed, &mut scratch);
    }
    let mut ranks = vec![0u32; n];
    for (rank, &value) in packed.iter().enumerate() {
        ranks[(value & INDEX_MASK) as usize] = rank as u32;
    }
    ranks
}

/// A direct-addressed queue over dense cell ranks: push a cell with its
/// elevation rank, pop the lowest rank first. Insert and pop are both O(1)
/// through a three-level bitmask, which is exactly what the monotone-key
/// priority flood needs.
struct RankQueue {
    /// Per-rank head of the cell list (`u32::MAX` when empty).
    head: Vec<u32>,
    /// Per-cell next pointer inside its rank list.
    next: Vec<u32>,
    /// Bit masks: level 0 marks occupied ranks, each higher level marks
    /// non-empty words of the level below; the top level is a single word.
    levels: Vec<Vec<u64>>,
    len: usize,
}

impl RankQueue {
    fn new(cells: usize) -> Self {
        let mut sizes = vec![(cells + 63) / 64];
        loop {
            let last = *sizes.last().expect("non-empty");
            let next = (last + 63) / 64;
            sizes.push(next);
            if next == 1 {
                break;
            }
        }
        let levels = sizes.into_iter().map(|words| vec![0u64; words]).collect();
        Self {
            head: vec![u32::MAX; cells],
            next: vec![u32::MAX; cells],
            levels,
            len: 0,
        }
    }

    #[inline]
    fn insert(&mut self, rank: usize, cell: u32) {
        self.next[cell as usize] = self.head[rank];
        self.head[rank] = cell;
        let mut index = rank;
        for level in 0..self.levels.len() {
            let word = index >> 6;
            let bit = index & 63;
            let was = self.levels[level][word];
            self.levels[level][word] = was | (1u64 << bit);
            if was != 0 || level + 1 == self.levels.len() {
                break;
            }
            index = word;
        }
        self.len += 1;
    }

    /// Lowest occupied rank.
    #[inline]
    fn lowest_rank(&self) -> Option<usize> {
        let mut index = 0usize;
        for level in (0..self.levels.len()).rev() {
            let word = self.levels[level][index];
            if word == 0 {
                return None;
            }
            index = (index << 6) | word.trailing_zeros() as usize;
        }
        Some(index)
    }

    fn pop(&mut self) -> Option<u32> {
        let rank = self.lowest_rank()?;
        let cell = self.head[rank];
        debug_assert!(cell != u32::MAX);
        self.head[rank] = self.next[cell as usize];
        let mut index = rank;
        for level in 0..self.levels.len() {
            let word = index >> 6;
            let bit = index & 63;
            let word_value = self.levels[level][word] & !(1u64 << bit);
            self.levels[level][word] = word_value;
            if word_value != 0 || level + 1 == self.levels.len() {
                break;
            }
            index = word;
        }
        self.len -= 1;
        Some(cell)
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Distance multiplier of the eight D8 directions (true for the diagonal ones).
const D8_DIAGONAL: [bool; 8] = [true, false, true, false, false, true, false, true];

/// Marks the outlets: ocean cells and open-boundary edge cells.
fn outlet_mask(grid: Grid, elevation: &[f64], sea: f64) -> Vec<bool> {
    let w = grid.width();
    let h = grid.height();
    let rectangular = matches!(grid.boundary(), BoundaryKind::Rectangular);
    let mut seeded = vec![false; grid.cell_count()];
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let i = row + x;
            let edge = rectangular && (x == 0 || y == 0 || x + 1 == w || y + 1 == h);
            seeded[i] = edge || elevation[i] <= sea;
        }
    }
    seeded
}

/// Marks the frontier outlets (outlets adjacent to a non-outlet cell).
///
/// Interior ocean cells are already drained at their own elevation and never
/// need to be pushed, so only the coastline enters the flood. The scan is a
/// tight direct-index loop: generic neighbour lookups dominated this pass when
/// the ocean was large.
fn mark_frontier_outlets(grid: Grid, seeded: &[bool], frontier: &mut [bool]) -> usize {
    let w = grid.width();
    let h = grid.height();
    let wraps_x = grid.boundary().wraps_x();
    let wraps_y = grid.boundary().wraps_y();
    let w_isize = w as isize;
    let h_isize = h as isize;
    let mut count = 0usize;
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let i = row + x;
            if !seeded[i] {
                continue;
            }
            let mut is_frontier = false;
            'neighbours: for dy in -1isize..=1 {
                let ny = y as isize + dy;
                if !(0..h_isize).contains(&ny) && !wraps_y {
                    continue;
                }
                let ny = ny.rem_euclid(h_isize) as usize;
                let nrow = ny * w;
                for dx in -1isize..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x as isize + dx;
                    if !(0..w_isize).contains(&nx) && !wraps_x {
                        continue;
                    }
                    let nx = nx.rem_euclid(w_isize) as usize;
                    if !seeded[nrow + nx] {
                        is_frontier = true;
                        break 'neighbours;
                    }
                }
            }
            if is_frontier {
                frontier[i] = true;
                count += 1;
            }
        }
    }
    count
}

/// Priority-flood depression fill (Barnes et al., 2014) over a direct-addressed
/// rank queue, with two further optimisations that dominate on large grids:
///
/// * **seed pruning** — outlet cells are all marked drained up front; only
///   those on the coastline (adjacent to a non-outlet) ever enter the queue,
///   so an ocean basin does not swamp the frontier;
/// * a **pit queue** — a cell whose raw elevation is already below the current
///   spill level is filled and visited immediately through a LIFO stack,
///   because all such cells share the same spill and need no ordering.
///
/// Fills `receiver` with the flow direction the flood implies (used as the
/// fallback for flat lake surfaces) and returns the filled surface plus the
/// downstream-first visit order.
fn priority_flood(
    grid: Grid,
    elevation: &[f64],
    seeded: &[bool],
    frontier: &[bool],
    ranks: &[u32],
    receiver: &mut [i32],
) -> (Vec<f64>, Vec<u32>) {
    let n = grid.cell_count();
    let w = grid.width();
    let h = grid.height();
    let w_isize = w as isize;
    let offsets: [isize; 8] = [
        -w_isize - 1,
        -w_isize,
        -w_isize + 1,
        -1,
        1,
        w_isize - 1,
        w_isize,
        w_isize + 1,
    ];
    // Multiply-by-reciprocal division by the row width, used once per pop.
    let row_magic = ((1u64 << 48) + w as u64 - 1) / w as u64;

    let mut filled = elevation.to_vec();
    let mut visited = seeded.to_vec();
    let mut queue = RankQueue::new(n);
    let mut stack: Vec<u32> = Vec::new();
    let mut visit_order: Vec<u32> = Vec::with_capacity(n);
    for i in 0..n {
        if frontier[i] {
            queue.insert(ranks[i] as usize, i as u32);
        }
    }

    while !stack.is_empty() || !queue.is_empty() {
        let c_u = match stack.pop() {
            Some(c) => c,
            None => match queue.pop() {
                Some(c) => c,
                None => break,
            },
        };
        let c = c_u as usize;
        visit_order.push(c_u);
        let spill = filled[c];
        let y = ((c as u64 * row_magic) >> 48) as usize;
        let x = c - y * w;
        let mut discover = |ni: usize, queue: &mut RankQueue, stack: &mut Vec<u32>| {
            if visited[ni] {
                return;
            }
            visited[ni] = true;
            receiver[ni] = c as i32;
            let z = elevation[ni];
            if z <= spill {
                filled[ni] = spill;
                stack.push(ni as u32);
            } else {
                filled[ni] = z;
                queue.insert(ranks[ni] as usize, ni as u32);
            }
        };
        if x > 0 && y > 0 && x + 1 < w && y + 1 < h {
            let base = c as isize;
            for offset in &offsets {
                discover((base + offset) as usize, &mut queue, &mut stack);
            }
        } else {
            for dir in 0..8 {
                let Some((nx, ny)) = grid.neighbor(x, y, dir) else {
                    continue;
                };
                discover(ny * w + nx, &mut queue, &mut stack);
            }
        }
    }

    // Cells the flood never reached are outlet cells already at their own
    // elevation; keep the output total if a pathological surface slips through.
    for i in 0..n {
        if !visited[i] {
            filled[i] = elevation[i];
        }
    }
    (filled, visit_order)
}

/// Direction index from a cell to one of its neighbours (`255` for none).
#[inline]
fn direction_to(grid: Grid, index: usize, target: usize) -> u8 {
    let w = grid.width();
    let (x, y) = grid.coords(index);
    let (rx, ry) = grid.coords(target);
    let mut dx = rx as isize - x as isize;
    let mut dy = ry as isize - y as isize;
    if grid.boundary().wraps_x() && dx.abs() > 1 {
        dx = if dx > 1 {
            dx - w as isize
        } else {
            dx + w as isize
        };
    }
    if grid.boundary().wraps_y() && dy.abs() > 1 {
        dy = if dy > 1 {
            dy - grid.height() as isize
        } else {
            dy + grid.height() as isize
        };
    }
    match (dx, dy) {
        (-1, -1) => 0,
        (0, -1) => 1,
        (1, -1) => 2,
        (-1, 0) => 3,
        (1, 0) => 4,
        (-1, 1) => 5,
        (0, 1) => 6,
        (1, 1) => 7,
        _ => 255,
    }
}

/// Computes the D8 steepest-descent receiver of every non-outlet cell on the
/// filled surface. The flood receiver is kept for flat lake surfaces, where no
/// neighbour is lower, and outlet cells never have a receiver.
fn compute_receivers(
    grid: Grid,
    reference: &[f64],
    seeded: &[bool],
    receiver: &mut [i32],
) -> Vec<u8> {
    let n = grid.cell_count();
    let w = grid.width();
    let h = grid.height();
    // Comparing `drop / distance` is the same as comparing `drop` scaled by the
    // inverse distance, which avoids a division per candidate neighbour.
    let mut flow_dir = vec![255u8; n];
    let w_isize = w as isize;
    let offsets: [isize; 8] = [
        -w_isize - 1,
        -w_isize,
        -w_isize + 1,
        -1,
        1,
        w_isize - 1,
        w_isize,
        w_isize + 1,
    ];

    for y in 0..h {
        let interior_row = y > 0 && y + 1 < h;
        let row = y * w;
        for x in 0..w {
            let i = row + x;
            if seeded[i] {
                receiver[i] = -1;
                continue;
            }
            let here = reference[i];
            let mut best_slope = 0.0f64;
            let mut best_dir = 255u8;
            if interior_row && x > 0 && x + 1 < w {
                let base = i as isize;
                for (dir, offset) in offsets.iter().enumerate() {
                    let j = (base + offset) as usize;
                    let there = reference[j];
                    if there < here {
                        let score = if D8_DIAGONAL[dir] {
                            (here - there) * std::f64::consts::FRAC_1_SQRT_2
                        } else {
                            here - there
                        };
                        if score > best_slope {
                            best_slope = score;
                            best_dir = dir as u8;
                        }
                    }
                }
            } else {
                for dir in 0..8 {
                    let Some((nx, ny)) = grid.neighbor(x, y, dir) else {
                        continue;
                    };
                    let j = ny * w + nx;
                    let there = reference[j];
                    if there < here {
                        let score = if D8_DIAGONAL[dir] {
                            (here - there) * std::f64::consts::FRAC_1_SQRT_2
                        } else {
                            here - there
                        };
                        if score > best_slope {
                            best_slope = score;
                            best_dir = dir as u8;
                        }
                    }
                }
            }
            if best_dir < 8 {
                flow_dir[i] = best_dir;
                receiver[i] = grid
                    .neighbor_index(i, best_dir as usize)
                    .map_or(-1, |j| j as i32);
            } else if receiver[i] >= 0 {
                // Flat surface: follow the direction the flood crossed it.
                flow_dir[i] = direction_to(grid, i, receiver[i] as usize);
            }
        }
    }
    flow_dir
}

/// Builds the MFD partition over downhill neighbours of `reference`.
fn build_mfd(
    grid: Grid,
    reference: &[f64],
    seeded: &[bool],
    receiver: &[i32],
    exponent: f64,
) -> MfdRouting {
    let n = grid.cell_count();
    let mut mfd = MfdRouting::new(n);
    let p = exponent.clamp(0.1, 8.0);
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            if seeded[i] {
                // Outlets do not route water onward.
                continue;
            }
            let here = reference[i];
            let mut slopes = [0.0f64; 8];
            let mut dirs = [255u8; 8];
            let mut slots = 0usize;
            let mut total = 0.0f64;
            for dir in 0..8 {
                let Some((nx, ny)) = grid.neighbor(x, y, dir) else {
                    continue;
                };
                let there = reference[ny * grid.width() + nx];
                if there < here {
                    let slope = (here - there) / grid.step_distance_m(dir);
                    let weight = slope.powf(p);
                    if weight > 0.0 {
                        slopes[slots] = weight;
                        dirs[slots] = dir as u8;
                        slots += 1;
                        total += weight;
                    }
                }
            }
            if slots == 0 {
                // Flat cell: route the whole flow along the flood direction.
                if receiver[i] >= 0 {
                    let dir = direction_to(grid, i, receiver[i] as usize);
                    if dir < 8 {
                        mfd.dirs[i][0] = dir;
                        mfd.weights[i][0] = 1.0;
                    }
                }
                continue;
            }
            for slot in 0..slots {
                mfd.dirs[i][slot] = dirs[slot];
                mfd.weights[i][slot] = (slopes[slot] / total) as f32;
            }
        }
    }
    mfd
}

/// Computes the topological order of the receiver forest (upstream first).
fn topological_order(receiver: &[i32], n: usize) -> Vec<u32> {
    let mut donor_count = vec![0u8; n];
    for &r in receiver {
        if r >= 0 {
            donor_count[r as usize] += 1;
        }
    }
    let mut queue: Vec<u32> = (0..n as u32)
        .filter(|&i| donor_count[i as usize] == 0)
        .collect();
    let mut order = Vec::with_capacity(n);
    let mut head = 0usize;
    while head < queue.len() {
        let u = queue[head];
        head += 1;
        order.push(u);
        let r = receiver[u as usize];
        if r >= 0 {
            let ru = r as usize;
            donor_count[ru] -= 1;
            if donor_count[ru] == 0 {
                queue.push(r as u32);
            }
        }
    }
    order
}

/// Local runoff for one cell, mm/yr.
#[inline]
fn local_runoff(precipitation_mm: f64, infiltration: f64, evap: f64, baseflow: f64) -> f64 {
    let direct = precipitation_mm * (1.0 - infiltration) * (1.0 - evap);
    let groundwater = precipitation_mm * infiltration * baseflow * 0.25;
    (direct + groundwater).max(0.0)
}

/// Routes water and builds all hydrologic fields.
#[must_use]
pub fn route(world: &World, params: &Params, climate: Option<&Climate>) -> Hydro {
    let grid = world.grid;
    let n = grid.cell_count();
    let sea = world.sea_level_m;
    let fill_depressions = params.hydrology.depression_fill;

    // Sanitised elevation view: routing must never propagate a NaN.
    let sanitized: Vec<f64>;
    let elevation: &[f64] = if world.elevation_m.non_finite_count() == 0 {
        world.elevation_m.as_slice()
    } else {
        sanitized = world
            .elevation_m
            .as_slice()
            .iter()
            .map(|&z| if z.is_finite() { z } else { sea })
            .collect();
        &sanitized
    };

    // Outlets. A world with no ocean and a closed boundary seeds its deepest
    // cell as the single outlet so water still has somewhere to go.
    let mut seeded = outlet_mask(grid, elevation, sea);
    let mut has_seed = seeded.iter().any(|&s| s);
    if !has_seed {
        let mut best = 0usize;
        let mut best_z = f64::INFINITY;
        for (i, &z) in elevation.iter().enumerate() {
            if z < best_z {
                best_z = z;
                best = i;
            }
        }
        seeded[best] = true;
        has_seed = true;
    }
    let _ = has_seed;

    // ---- Depression fill and receivers ------------------------------------
    let mut receiver = vec![-1i32; n];
    let (filled_vec, visit_order) = if fill_depressions {
        let ranks = elevation_ranks(elevation);
        let mut frontier = vec![false; n];
        mark_frontier_outlets(grid, &seeded, &mut frontier);
        priority_flood(grid, elevation, &seeded, &frontier, &ranks, &mut receiver)
    } else {
        (elevation.to_vec(), Vec::new())
    };
    let flow_dir_vec = compute_receivers(grid, &filled_vec, &seeded, &mut receiver);

    // ---- Topological order -------------------------------------------------
    // The flood visits lower spill elevations first and every receiver was
    // visited before the cell it receives (strictly lower on slopes, the
    // discoverer on flats), so the reversed visit order plus the (never
    // visited) outlets is already a valid upstream-first order — no Kahn pass.
    let mut order_upstream: Vec<u32> = if fill_depressions {
        // Outlets are never popped by the flood (they are all drained up
        // front), so take the reversed visit order of the flooded cells and
        // append the outlets, which are always downstream terminals.
        let mut order: Vec<u32> = Vec::with_capacity(n);
        order.extend(
            visit_order
                .iter()
                .rev()
                .filter(|&&i| !seeded[i as usize])
                .copied(),
        );
        order.extend((0..n as u32).filter(|&i| seeded[i as usize]));
        order
    } else {
        topological_order(&receiver, n)
    };
    if order_upstream.len() != n {
        // Rebuild from the receiver graph if the fast path was inconsistent.
        order_upstream = topological_order(&receiver, n);
        if order_upstream.len() != n {
            // Keep the run alive: append the stragglers in index order.
            let mut seen = vec![false; n];
            for &i in &order_upstream {
                seen[i as usize] = true;
            }
            for i in 0..n as u32 {
                if !seen[i as usize] {
                    order_upstream.push(i);
                }
            }
        }
    }
    let order_downstream: Vec<u32> = order_upstream.iter().rev().copied().collect();

    // ---- Runoff, accumulation and discharge --------------------------------
    let hyd = &params.hydrology;
    let infiltration = hyd.infiltration.clamp(0.0, 1.0);
    let evap = hyd.evapotranspiration_factor.clamp(0.0, 0.95);
    let baseflow = hyd.groundwater_baseflow.clamp(0.0, 1.0);
    let cell_area = grid.cell_area_m2();
    let spherical_areas = grid.has_spherical_areas();
    // Physical area of one cell. Constant on planar grids (the historical,
    // bit-identical arithmetic); `∝ cos(latitude)` on spherical grids.
    let cell_area_at = |index: usize| -> f64 {
        if spherical_areas {
            let (x, y) = grid.coords(index);
            grid.cell_area_m2_at(x, y)
        } else {
            cell_area
        }
    };
    let channel_threshold = f64::from(hyd.channel_init_cells.max(1));
    let default_precip = params.climate.precipitation_mm;
    let mut runoff = vec![0.0f32; n];
    let mut runoff_sum = 0.0f64;
    for i in 0..n {
        let precip = climate.map_or(default_precip, |c| {
            f64::from(c.precipitation_mm.get_index(i))
        });
        let value = local_runoff(precip, infiltration, evap, baseflow);
        runoff[i] = value as f32;
        runoff_sum += value;
    }
    // Mean runoff is reported over the grid; on a sphere it must be area
    // weighted, otherwise polar rows (tiny area) count as much as equatorial
    // ones and the reported mean stops matching the water actually routed.
    let mean_runoff_mm = if spherical_areas {
        let mut weighted = 0.0;
        for (i, &value) in runoff.iter().enumerate() {
            weighted += f64::from(value) * cell_area_at(i);
        }
        let total = grid.total_area_m2();
        if total > 0.0 {
            weighted / total
        } else {
            0.0
        }
    } else {
        runoff_sum / n as f64
    };

    // Accumulation (cells) and discharge (m³/yr) interleaved so the
    // random-access pass touches one array instead of two.
    let mut flow = vec![[0.0f64; 2]; n];
    let mfd = if hyd.uses_mfd() {
        Some(build_mfd(
            grid,
            &filled_vec,
            &seeded,
            &receiver,
            hyd.mfd_exponent,
        ))
    } else {
        None
    };
    if let Some(mfd) = &mfd {
        for &u in &order_upstream {
            let ui = u as usize;
            let incoming = flow[ui];
            let discharge = incoming[1] + cell_area_at(ui) * f64::from(runoff[ui]) / 1000.0;
            let accumulation = incoming[0] + 1.0;
            flow[ui] = [accumulation, discharge];
            let dirs = mfd.dirs[ui];
            let weights = mfd.weights[ui];
            for slot in 0..8 {
                let dir = dirs[slot];
                if dir >= 8 {
                    continue;
                }
                let Some(j) = grid.neighbor_index(ui, dir as usize) else {
                    continue;
                };
                let fraction = f64::from(weights[slot]);
                if fraction > 0.0 {
                    flow[j][0] += accumulation * fraction;
                    flow[j][1] += discharge * fraction;
                }
            }
        }
    } else {
        for &u in &order_upstream {
            let ui = u as usize;
            let incoming = flow[ui];
            let discharge = incoming[1] + cell_area_at(ui) * f64::from(runoff[ui]) / 1000.0;
            let accumulation = incoming[0] + 1.0;
            flow[ui] = [accumulation, discharge];
            let r = receiver[ui];
            if r >= 0 {
                let ri = r as usize;
                flow[ri][0] += accumulation;
                flow[ri][1] += discharge;
            }
        }
    }

    finish(
        grid,
        filled_vec,
        elevation,
        receiver,
        flow_dir_vec,
        flow,
        order_upstream,
        order_downstream,
        mean_runoff_mm,
        channel_threshold,
        mfd,
    )
}

/// Assembles the [`Hydro`] result, deriving drainage area, the river mask
/// (with the largest tenth of channels promoted to major rivers) and lake
/// depths in cache-friendly sequential passes.
#[allow(clippy::too_many_arguments)]
fn finish(
    grid: Grid,
    filled_vec: Vec<f64>,
    elevation: &[f64],
    receiver: Vec<i32>,
    flow_dir_vec: Vec<u8>,
    flow: Vec<[f64; 2]>,
    order_upstream: Vec<u32>,
    order_downstream: Vec<u32>,
    mean_runoff_mm: f64,
    channel_threshold: f64,
    mfd: Option<MfdRouting>,
) -> Hydro {
    let n = grid.cell_count();
    let cell_area = grid.cell_area_m2();
    let spherical_areas = grid.has_spherical_areas();
    // Drainage area is the sum of the upstream *physical* cell areas. On a
    // sphere that is not `accumulation_cells * cell_area` (the cells have
    // different areas), so it is folded with one more upstream sweep over the
    // receiver/MFD graph. Planar grids keep the historical closed form, which
    // also keeps their arithmetic bit-identical.
    let mut physical_area: Vec<f64> = Vec::new();
    if spherical_areas {
        physical_area.resize(n, 0.0);
        let cell_area_at = |index: usize| {
            let (x, y) = grid.coords(index);
            grid.cell_area_m2_at(x, y)
        };
        if let Some(mfd) = &mfd {
            for &u in &order_upstream {
                let i = u as usize;
                physical_area[i] += cell_area_at(i);
                let amount = physical_area[i];
                for (dir, fraction) in mfd.downstream(i) {
                    if let Some(j) = grid.neighbor_index(i, dir) {
                        physical_area[j] += amount * fraction;
                    }
                }
            }
        } else {
            for &u in &order_upstream {
                let i = u as usize;
                physical_area[i] += cell_area_at(i);
                let r = receiver[i];
                if r >= 0 {
                    physical_area[r as usize] += physical_area[i];
                }
            }
        }
    }
    let mut accumulation = Vec::with_capacity(n);
    let mut discharge = Vec::with_capacity(n);
    let mut area = Vec::with_capacity(n);
    let mut river_mask = Vec::with_capacity(n);
    let mut channel_areas = Vec::new();
    for (i, cell) in flow.iter().enumerate() {
        let acc = cell[0];
        accumulation.push(acc);
        discharge.push(cell[1]);
        area.push(if spherical_areas {
            physical_area[i]
        } else {
            acc * cell_area
        });
        if acc >= channel_threshold {
            river_mask.push(1u8);
            channel_areas.push(acc);
        } else {
            river_mask.push(0u8);
        }
    }
    if !channel_areas.is_empty() {
        channel_areas.sort_unstable_by(|a, b| b.total_cmp(a));
        let major_threshold = channel_areas[channel_areas.len() / 10];
        for (i, mask) in river_mask.iter_mut().enumerate() {
            if *mask > 0 && accumulation[i] >= major_threshold {
                *mask = 2;
            }
        }
    }

    let lake_depth: Vec<f32> = (0..n)
        .map(|i| {
            let depth = filled_vec[i] - elevation[i];
            if depth > 0.02 {
                depth as f32
            } else {
                0.0
            }
        })
        .collect();

    Hydro {
        grid,
        filled_m: Field::from_vec(grid, filled_vec).expect("filled field matches the grid"),
        receiver,
        flow_dir: Field::from_vec(grid, flow_dir_vec).expect("flow directions match the grid"),
        accumulation_cells: Field::from_vec(grid, accumulation)
            .expect("accumulation matches the grid"),
        drainage_area_m2: Field::from_vec(grid, area).expect("drainage area matches the grid"),
        discharge_m3_yr: Field::from_vec(grid, discharge).expect("discharge matches the grid"),
        lake_depth_m: Field::from_vec(grid, lake_depth).expect("lake depth matches the grid"),
        river_mask: Field::from_vec(grid, river_mask).expect("river mask matches the grid"),
        order_upstream,
        order_downstream,
        mean_runoff_mm,
        mfd,
    }
}

/// Sort order of cells by elevation, highest first; a cheap fallback used by
/// modules that need a downhill visitation order without routing water.
#[must_use]
pub fn descending_order(elevation: &Field<f64>) -> Vec<u32> {
    let mut order: Vec<u32> = (0..elevation.len() as u32).collect();
    order.sort_unstable_by(|&a, &b| {
        elevation
            .get_index(b as usize)
            .total_cmp(&elevation.get_index(a as usize))
    });
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::noise::{NoiseField, NoiseParams};
    use crate::params::Params;
    use crate::world::World;

    fn setup(width: u32, height: u32) -> (World, Params) {
        let mut p = Params::default();
        p.world.width = width;
        p.world.height = height;
        p.world.cell_km = 10.0;
        let world = World::new(&p.world).unwrap();
        (world, p)
    }

    /// A deterministic fractal surface with ocean margins, used by the
    /// correctness and performance tests.
    fn fractal_world(width: u32, height: u32) -> (World, Params) {
        let (mut world, p) = setup(width, height);
        let noise = NoiseField::new(NoiseParams::default(), 7);
        let mut relief = noise.fill(&world.grid, 3.0);
        relief.rescale(-4000.0, 3000.0);
        let w = world.grid.width();
        let h = world.grid.height();
        world.elevation_m.zip_with(&relief, |i, _old, z| {
            let (x, y) = (i % w, i / w);
            let dx = (x as f64 / w as f64 - 0.5).abs();
            let dy = (y as f64 / h as f64 - 0.5).abs();
            let edge = (dx.max(dy) - 0.3).max(0.0) * 6.0;
            z - edge * 3000.0
        });
        (world, p)
    }

    #[test]
    fn every_cell_drains_or_is_an_outlet() {
        let (mut world, p) = setup(32, 32);
        // Ocean around a ring wall, with a land basin behind the wall: the
        // basin must fill to the spill level as a lake.
        let grid = world.grid;
        world.elevation_m.map_in_place(|i, _| {
            let (x, y) = grid.coords(i);
            let dx = x as f64 - 16.0;
            let dy = y as f64 - 16.0;
            let d = (dx * dx + dy * dy).sqrt();
            if d < 3.0 {
                600.0
            } else if d < 6.0 {
                1500.0
            } else {
                -200.0
            }
        });
        let hydro = route(&world, &p, None);
        assert_eq!(hydro.order_upstream.len(), grid.cell_count());
        // The basin bottom must be a lake.
        let depth = hydro.lake_depth_m.get(16, 16);
        assert!(depth > 0.0, "bowl centre is not a lake: {depth}");
        for i in 0..grid.cell_count() {
            if let Some(r) = hydro.receiver_of(i) {
                assert_ne!(r, i);
                // Receiver is not above the cell in the filled surface.
                assert!(hydro.filled_m.get_index(r) <= hydro.filled_m.get_index(i) + 1e-9);
            }
        }
    }

    #[test]
    fn accumulation_increases_downstream() {
        let (mut world, p) = setup(32, 32);
        let grid = world.grid;
        // Tilted plane draining east.
        world
            .elevation_m
            .map_in_place(|i, _| 1000.0 - (i % grid.width()) as f64 * 10.0);
        let hydro = route(&world, &p, None);
        let outlet = grid.index(grid.width() - 1, 0);
        let acc = hydro.accumulation_cells.get_index(outlet);
        assert!(acc >= 1.0);
        let mut expected = 0.0;
        for _ in 0..grid.height() {
            expected += grid.width() as f64;
        }
        assert!(acc <= expected);
        assert!(hydro.discharge_m3_yr.max_value().unwrap() > 0.0);
    }

    #[test]
    fn river_mask_marks_channels() {
        let (mut world, p) = setup(32, 32);
        let grid = world.grid;
        world
            .elevation_m
            .map_in_place(|i, _| 500.0 - (i % grid.width()) as f64);
        let hydro = route(&world, &p, None);
        assert!(hydro.channel_cells() > 0);
        assert!(hydro.drainage_density_km_per_km2() >= 0.0);
    }

    #[test]
    fn routing_is_deterministic() {
        let (mut world, p) = setup(32, 32);
        world
            .elevation_m
            .map_in_place(|i, _| (i as f64 * 0.37).sin() * 300.0);
        let a = route(&world, &p, None);
        let b = route(&world, &p, None);
        assert_eq!(a.receiver, b.receiver);
        assert_eq!(a.order_upstream, b.order_upstream);
        assert_eq!(
            a.accumulation_cells.as_slice(),
            b.accumulation_cells.as_slice()
        );
    }

    #[test]
    fn longest_path_is_bounded() {
        let (mut world, p) = setup(32, 32);
        let grid = world.grid;
        world
            .elevation_m
            .map_in_place(|i, _| 1000.0 - (i % grid.width()) as f64 * 5.0);
        let hydro = route(&world, &p, None);
        let path = hydro.longest_flow_path_cells();
        assert!(path > 0 && path <= grid.cell_count(), "path={path}");
    }

    #[test]
    fn receivers_form_an_acyclic_forest() {
        let (world, p) = fractal_world(48, 48);
        let hydro = route(&world, &p, None);
        let n = world.cell_count();
        for start in 0..n {
            let mut steps = 0usize;
            let mut cell = start;
            loop {
                match hydro.receiver_of(cell) {
                    None => break,
                    Some(next) => {
                        assert_ne!(next, cell, "self loop at {cell}");
                        cell = next;
                        steps += 1;
                        assert!(steps <= n, "cycle from {start}");
                    }
                }
            }
        }
        // Every cell appears exactly once in the topological order.
        let mut seen = vec![false; n];
        for &u in &hydro.order_upstream {
            assert!(!seen[u as usize]);
            seen[u as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
        // Downstream order is exactly the reversed upstream order.
        assert_eq!(
            hydro.order_downstream,
            hydro
                .order_upstream
                .iter()
                .rev()
                .copied()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn fully_enclosed_world_drains_to_the_global_minimum() {
        // No ocean and a toroidal boundary: the flood must seed the global
        // minimum and every other cell must drain into it.
        let (mut world, p) = setup(32, 32);
        let grid = world.grid;
        let mut min_index = 0usize;
        let mut min_z = f64::INFINITY;
        for i in 0..grid.cell_count() {
            let (x, y) = grid.coords(i);
            let z = 200.0 + (x as f64 * 0.7).sin() * 100.0 + y as f64 * 0.5;
            world.elevation_m.set_index(i, z);
            if z < min_z {
                min_z = z;
                min_index = i;
            }
        }
        let hydro = route(&world, &p, None);
        assert_eq!(hydro.order_upstream.len(), grid.cell_count());
        let acc = hydro.accumulation_cells.get_index(min_index);
        assert!(
            (acc - grid.cell_count() as f64).abs() < 1e-9,
            "outlet catchment {acc} != {}",
            grid.cell_count()
        );
    }

    #[test]
    fn flat_surface_routes_everywhere() {
        let (mut world, p) = setup(32, 32);
        world.elevation_m.fill_value(500.0);
        let hydro = route(&world, &p, None);
        assert_eq!(hydro.order_upstream.len(), world.cell_count());
        // No cell can be above another on a flat surface; every cell is either
        // an outlet (no flood direction) or follows the flat-crossing chain.
        for i in 0..world.cell_count() {
            assert!(hydro.accumulation_cells.get_index(i) >= 1.0);
        }
    }

    #[test]
    fn no_depression_fill_keeps_pits_as_outlets() {
        let (mut world, mut p) = setup(24, 24);
        p.hydrology.depression_fill = false;
        let grid = world.grid;
        // A single pit in an otherwise flat plain.
        world.elevation_m.fill_value(100.0);
        world.elevation_m.set(12, 12, 50.0);
        let hydro = route(&world, &p, None);
        assert_eq!(hydro.lake_depth_m.max_value().unwrap(), 0.0);
        // The pit is the only sink; its eight neighbours drain into it.
        assert_eq!(hydro.receiver_of(grid.index(12, 12)), None);
        assert!(hydro.accumulation_cells.get(12, 12) > 1.0);
    }

    #[test]
    fn mfd_partition_is_downhill_and_conserves_flow() {
        let (world, mut p) = fractal_world(48, 48);
        p.hydrology.routing = 1.0;
        p.hydrology.mfd_exponent = 1.1;
        let hydro = route(&world, &p, None);
        let mfd = hydro.mfd_routing().expect("MFD partition missing");
        assert_eq!(mfd.len(), world.cell_count());
        for i in 0..world.cell_count() {
            let mut sum = 0.0;
            for (dir, fraction) in mfd.downstream(i) {
                assert!((0.0..=1.0).contains(&fraction));
                let j = world.grid.neighbor_index(i, dir).expect("valid direction");
                // Flow only goes to a strictly lower filled cell, or along the
                // flat-crossing chain (equal filled surface).
                assert!(
                    hydro.filled_m.get_index(j) <= hydro.filled_m.get_index(i) + 1e-9,
                    "MFD sent flow uphill at {i}"
                );
                sum += fraction;
            }
            if hydro.receiver_of(i).is_some() {
                assert!((sum - 1.0).abs() < 1e-5, "weights sum to {sum} at {i}");
            }
        }
        // Total catchment reaching the terminal cells must equal the number of
        // cells: every cell contributes one unit of area and every non-terminal
        // cell passes all of its accumulated area downstream.
        let mut total_out = 0.0;
        for i in 0..world.cell_count() {
            if mfd.downstream(i).next().is_none() {
                total_out += hydro.accumulation_cells.get_index(i);
            }
        }
        let n = world.cell_count() as f64;
        assert!(
            (total_out - n).abs() / n < 1e-3,
            "MFD lost flow: {total_out} of {n}"
        );
        // And the D8 receiver still routes the primary channel path.
        assert!((0..world.cell_count()).any(|i| hydro.receiver_of(i).is_some()));
        assert!(hydro.river_mask.iter().any(|&m| m > 0));
    }

    #[test]
    fn mfd_and_d8_agree_on_determinism() {
        let (world, mut p) = fractal_world(32, 32);
        p.hydrology.routing = 1.0;
        let a = route(&world, &p, None);
        let b = route(&world, &p, None);
        assert_eq!(
            a.accumulation_cells.as_slice(),
            b.accumulation_cells.as_slice()
        );
        assert_eq!(a.mfd, b.mfd);
    }

    /// Builds a spherical world on an east-tilted plane with no ocean: every
    /// row drains east on its own, so each row's outlet collects exactly one
    /// row worth of catchment. The rows differ only in latitude, which is
    /// exactly what the per-cell area correction must expose.
    fn spherical_plane() -> (World, Params) {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        p.world.boundary = BoundaryKind::Spherical;
        p.world.planet_radius_km = 6371.0;
        p.world.sea_level_m = -1000.0;
        p.hydrology.depression_fill = false;
        p.hydrology.channel_init_cells = 1;
        let grid = p.world.grid().unwrap();
        assert!(grid.has_spherical_areas());
        let mut world = World::new(&p.world).unwrap();
        world.elevation_m.map_in_place(|i, _| {
            let x = i % grid.width();
            1000.0 - x as f64 * 10.0
        });
        (world, p)
    }

    #[test]
    fn spherical_discharge_uses_per_cell_area() {
        let (world, p) = spherical_plane();
        let grid = world.grid;
        let hydro = route(&world, &p, None);
        let w = grid.width();
        // Every row outlet has the same *cell* count but a different area.
        let row_area = |y: usize| -> f64 { (0..w).map(|x| grid.cell_area_m2_at(x, y)).sum() };
        let equator_y = grid.height() / 2;
        let pole_y = 0;
        let outlet = |y: usize| grid.index(w - 1, y);
        for y in [equator_y, pole_y] {
            assert_eq!(
                hydro.accumulation_cells.get_index(outlet(y)),
                w as f64,
                "row {y} catchment count"
            );
            let drainage = hydro.drainage_area_m2.get_index(outlet(y));
            let want = row_area(y);
            assert!(
                (drainage - want).abs() <= 1e-9 * want,
                "row {y} drainage area {drainage} != sum of cell areas {want}"
            );
        }
        // Same number of contributing cells, but the equatorial row carries
        // proportionally more water because its cells are larger.
        let q_eq = hydro.discharge_m3_yr.get_index(outlet(equator_y));
        let q_pole = hydro.discharge_m3_yr.get_index(outlet(pole_y));
        let ratio = q_eq / q_pole;
        let want = row_area(equator_y) / row_area(pole_y);
        assert!(
            (ratio - want).abs() <= 1e-9 * want.abs(),
            "discharge ratio {ratio} != area ratio {want}"
        );
        assert!(ratio > 5.0, "equatorial discharge barely larger: {ratio}");
        println!(
            "spherical hydrology: row cell count {} both rows, row area ratio {want:.4}, \
             discharge ratio {ratio:.4}, row area {:.6e} m²",
            w,
            row_area(pole_y).max(f64::MIN_POSITIVE)
        );
        // Mean runoff is an area-weighted mean on the sphere; the world is
        // uniform so it must still equal the local runoff value.
        let first = hydro.mean_runoff_mm;
        assert!(first > 0.0);
        let again = route(&world, &p, None);
        assert_eq!(
            hydro.discharge_m3_yr.as_slice(),
            again.discharge_m3_yr.as_slice()
        );
        assert_eq!(
            hydro.drainage_area_m2.as_slice(),
            again.drainage_area_m2.as_slice()
        );
    }

    #[test]
    fn spherical_lake_volume_uses_physical_areas() {
        let (world, p) = spherical_plane();
        let mut hydro = route(&world, &p, None);
        hydro.lake_depth_m.fill_value(1.0);
        let volume = hydro.lake_volume_m3();
        let total = world.grid.total_area_m2();
        assert!(
            (volume - total).abs() / total < 1e-12,
            "lake volume {volume} != total area {total}"
        );
        // And the planar fallback stays exactly the old value.
        let mut planar = Params::default();
        planar.world.width = 16;
        planar.world.height = 16;
        let planar_world = World::new(&planar.world).unwrap();
        let mut planar_hydro = route(&planar_world, &planar, None);
        planar_hydro.lake_depth_m.fill_value(1.0);
        assert!((planar_hydro.lake_volume_m3() - 256.0 * 1.0e8).abs() < 1e-6);
    }

    #[test]
    fn route_scales_to_browser_grids() {
        // Generous caps: this is a regression guard against accidentally
        // reintroducing an O(n log n) comparison heap, not a benchmark.
        // Debug builds are roughly an order of magnitude slower than release.
        let (world, p) = fractal_world(256, 256);
        let mut small = std::time::Duration::MAX;
        let mut hydro = None;
        for _ in 0..5 {
            let start = std::time::Instant::now();
            hydro = Some(route(&world, &p, None));
            small = small.min(start.elapsed());
        }
        let hydro = hydro.expect("routed");
        assert_eq!(hydro.order_upstream.len(), world.cell_count());
        let (world512, p512) = fractal_world(512, 512);
        let mut large = std::time::Duration::MAX;
        let mut hydro512 = None;
        for _ in 0..3 {
            let start = std::time::Instant::now();
            hydro512 = Some(route(&world512, &p512, None));
            large = large.min(start.elapsed());
        }
        assert_eq!(
            hydro512.expect("routed").order_upstream.len(),
            world512.cell_count()
        );
        println!(
            "route (best of repeats): 256² {:?}, 512² {:?} ({:.1} ns/cell)",
            small,
            large,
            large.as_secs_f64() * 1e9 / world512.cell_count() as f64
        );
        let cap_256 = if cfg!(debug_assertions) {
            std::time::Duration::from_millis(1500)
        } else {
            std::time::Duration::from_millis(150)
        };
        let cap_512 = cap_256 * 4;
        assert!(small < cap_256, "256² routing too slow: {small:?}");
        assert!(large < cap_512, "512² routing too slow: {large:?}");
    }
}
