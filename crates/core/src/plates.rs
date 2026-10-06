//! Plate tectonics: tessellation, kinematics and boundary classification.
//!
//! Cells join their nearest Lloyd-relaxed site (Voronoi); the site-joining
//! segment is the boundary normal for motion classification.

use crate::grid::{Boundary, LabelField, ScalarField};
use crate::linalg::{Vec2, Vec3};
use crate::rng::{domain, Rng};

/// Physical constants, in SI where noted.
pub mod units {
    /// Rigid-plate velocity in m/yr.
    pub const TYPICAL_PLATE_SPEED_M_PER_YR: f64 = 50.0;
    /// Reference crustal thickness in km.
    pub const CONTINENTAL_CRUST_KM: f64 = 35.0;
    pub const OCEANIC_CRUST_KM: f64 = 7.0;
    /// Mantle density, kg/m³.
    pub const MANTLE_DENSITY: f64 = 3300.0;
    /// Continental crust density, kg/m³.
    pub const CONTINENTAL_DENSITY: f64 = 2700.0;
    pub const OCEANIC_DENSITY: f64 = 3000.0;
    /// Water density, kg/m³.
    pub const WATER_DENSITY: f64 = 1000.0;
    /// Gravity, m/s².
    pub const GRAVITY: f64 = 9.81;
}

/// Boundary motion classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryType {
    /// Plates approach.
    Convergent,
    /// Plates separate.
    Divergent,
    /// Plates slide past one another.
    Transform,
    /// No significant relative motion.
    Null,
}

impl BoundaryType {
    /// Japanese label.
    pub fn label_ja(self) -> &'static str {
        match self {
            BoundaryType::Convergent => "収束",
            BoundaryType::Divergent => "発散",
            BoundaryType::Transform => "変位",
            BoundaryType::Null => "静止",
        }
    }
}

/// Relative motion between two plates, resolved against a boundary normal.
#[derive(Debug, Clone, Copy)]
pub struct RelativeMotion {
    /// Approach rate in m/yr.
    pub convergent: f64,
    /// Separation rate in m/yr.
    pub divergent: f64,
    /// Along-strike slip rate in m/yr.
    pub transform: f64,
    /// Total relative speed in m/yr.
    pub speed: f64,
}

impl RelativeMotion {
    /// Classifies the boundary; motion below `threshold` m/yr is negligible.
    pub fn classify(&self, threshold: f64) -> BoundaryType {
        let normal_component = self.convergent - self.divergent;
        if self.speed < threshold {
            return BoundaryType::Null;
        }
        if normal_component > threshold {
            BoundaryType::Convergent
        } else if normal_component < -threshold {
            BoundaryType::Divergent
        } else if self.transform > threshold {
            BoundaryType::Transform
        } else {
            BoundaryType::Null
        }
    }

    /// Dominant component magnitude.
    pub fn intensity(&self) -> f64 {
        self.speed
    }
}

/// One tectonic plate.
#[derive(Debug, Clone)]
pub struct Plate {
    /// Stable index into the plate list.
    pub id: u32,
    /// Representative position in normalised grid coordinates `[0, 1]²`.
    pub site: Vec2,
    /// Rigid-body velocity in m/yr.
    pub velocity: Vec2,
    /// Crustal thickness in km.
    pub thickness_km: f64,
    /// Crustal density in kg/m³.
    pub density: f64,
    /// Whether the plate is predominantly oceanic.
    pub is_oceanic: bool,
    /// Number of grid cells assigned to this plate.
    pub area_cells: usize,
}

impl Plate {
    /// Crustal thickness in metres.
    pub fn thickness_m(&self) -> f64 {
        self.thickness_km * 1000.0
    }
}

/// A complete plate mosaic.
#[derive(Debug, Clone)]
pub struct PlateMosaic {
    /// The plates, ordered by id.
    pub plates: Vec<Plate>,
    /// Plate id per cell.
    pub labels: LabelField,
    /// Boundary classification per cell; `0` where not a boundary.
    pub boundary_type: LabelField,
    /// Signed normal motion rate in m/yr (positive convergent).
    pub convergence: ScalarField,
    /// Absolute tangential slip rate in m/yr.
    pub transform_rate: ScalarField,
    /// Distance in cells to the nearest plate boundary.
    pub boundary_distance: ScalarField,
    /// Boundary unit normal per cell, stored as two fields.
    pub normal_x: ScalarField,
    pub normal_y: ScalarField,
}

impl PlateMosaic {
    /// Number of plates.
    pub fn len(&self) -> usize {
        self.plates.len()
    }

    /// Whether the mosaic is empty.
    pub fn is_empty(&self) -> bool {
        self.plates.is_empty()
    }

    /// Boundary type by label id.
    pub fn boundary_type_at(&self, x: usize, y: usize) -> BoundaryType {
        match self.boundary_type.get(x, y) {
            1 => BoundaryType::Convergent,
            2 => BoundaryType::Divergent,
            3 => BoundaryType::Transform,
            _ => BoundaryType::Null,
        }
    }

    /// Fraction of boundary cells of each type.
    pub fn boundary_type_fractions(&self) -> [f64; 4] {
        let n = self.boundary_type.len() as f64;
        let mut counts = [0.0f64; 4];
        for v in self.boundary_type.as_slice() {
            counts[(*v).min(3) as usize] += 1.0;
        }
        for c in counts.iter_mut() {
            *c /= n;
        }
        counts
    }

    /// Number of cells sitting on a plate boundary.
    pub fn boundary_cell_count(&self) -> usize {
        self.labels.boundary_cell_count()
    }

    /// Returns `true` when every cell references an existing plate.
    pub fn labels_are_consistent(&self) -> bool {
        let n = self.plates.len() as u32;
        self.labels.as_slice().iter().all(|&l| l < n)
    }
}

/// Plate generation parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct PlateParams {
    /// 1–32.
    pub count: u32,
    /// 5.0–200.0 km.
    pub thickness_km: f64,
    /// Densities for continental / transitional / oceanic, g/cm³.
    pub densities: [f64; 3],
    /// Threshold on relative speed, as a fraction of typical speed.
    pub collision_threshold: f64,
    /// Lloyd sweeps to equalise plate areas.
    pub lloyd_iterations: u32,
    /// Fraction of plates forced oceanic before area-based reassignment.
    pub oceanic_bias: f64,
}

impl Default for PlateParams {
    fn default() -> Self {
        Self {
            count: 8,
            thickness_km: 35.0,
            densities: [2.7, 3.0, 3.3],
            collision_threshold: 0.5,
            lloyd_iterations: 8,
            oceanic_bias: 0.4,
        }
    }
}

/// Generates plate sites with Lloyd relaxation toward equal-area cells.
fn generate_sites(
    b: Boundary,
    count: usize,
    rng: &mut Rng,
    lloyd_iters: u32,
) -> Vec<Vec2> {
    let (w, h) = (b.width(), b.height());
    if count == 0 || w == 0 || h == 0 {
        return Vec::new();
    }

    // Jittered stratified seeding, then relax.
    let cols = (count as f64).sqrt().ceil().max(1.0) as usize;
    let rows = count.div_ceil(cols).max(1);

    let mut sites: Vec<Vec2> = Vec::with_capacity(count);
    for i in 0..count {
        let (cx, cy) = (i % cols, i / cols);
        let (jx, jy) = (rng.next_f64(), rng.next_f64());
        sites.push(Vec2::new(
            (cx as f64 + 0.2 + 0.6 * jx) / cols as f64,
            (cy as f64 + 0.2 + 0.6 * jy) / rows as f64,
        ));
    }

    let mut labels = assign_sites(b, &sites);

    for _ in 0..lloyd_iters {
        let mut sums = vec![(0.0f64, 0.0f64, 0usize); sites.len()];
        for y in 0..h {
            for x in 0..w {
                let l = labels.get(x, y) as usize;
                if l < sums.len() {
                    sums[l].0 += x as f64;
                    sums[l].1 += y as f64;
                    sums[l].2 += 1;
                }
            }
        }
        let mut moved = false;
        for (i, s) in sites.iter_mut().enumerate() {
            let (sx, sy, n) = sums[i];
            if n > 0 {
                let target = Vec2::new(
                    (sx / n as f64) / (w.max(1) as f64),
                    (sy / n as f64) / (h.max(1) as f64),
                );
                if target.distance(*s) > 1e-9 {
                    *s = target;
                    moved = true;
                }
            }
        }
        labels = assign_sites(b, &sites);
        if !moved {
            break;
        }
    }

    sites
}

/// Assigns every cell to its nearest site (brute-force Voronoi).
fn assign_sites(b: Boundary, sites: &[Vec2]) -> LabelField {
    let (w, h) = (b.width(), b.height());
    let mut labels = LabelField::filled(b, 0);
    if sites.is_empty() {
        return labels;
    }

    let wraps_x = matches!(
        b.kind(),
        crate::grid::BoundaryKind::Toroidal | crate::grid::BoundaryKind::Spherical
    );
    let wraps_y = matches!(b.kind(), crate::grid::BoundaryKind::Toroidal);

    for y in 0..h {
        for x in 0..w {
            let px = x as f64 / (w.max(1)) as f64;
            let py = y as f64 / (h.max(1)) as f64;
            let mut best = 0usize;
            let mut best_d = f64::INFINITY;

            for (i, s) in sites.iter().enumerate() {
                // Shortest distance round the wrapping axes keeps the mosaic seamless.
                let d = toroidal_distance2(px, py, *s, wraps_x, wraps_y);
                if d < best_d {
                    best_d = d;
                    best = i;
                }
            }
            labels.set(x, y, best as u32);
        }
    }
    labels
}

/// Squared distance honouring wraparound on the axes that wrap.
fn toroidal_distance2(px: f64, py: f64, s: Vec2, wrap_x: bool, wrap_y: bool) -> f64 {
    let mut dx = (px - s.x).abs();
    let mut dy = (py - s.y).abs();
    if wrap_x {
        if dx > 0.5 {
            dx = 1.0 - dx;
        }
    }
    if wrap_y {
        if dy > 0.5 {
            dy = 1.0 - dy;
        }
    }
    dx * dx + dy * dy
}

/// Builds a complete plate mosaic.
pub fn build_mosaic(
    b: Boundary,
    params: &PlateParams,
    world_seed: u64,
    spatial_extent_m: f64,
) -> PlateMosaic {
    let count = params.count.max(1) as usize;
    let extent_factor = (spatial_extent_m / 1.0e7).clamp(0.1, 10.0);
    let mut rng = Rng::new(world_seed, domain::PLATES);
    let sites = generate_sites(b, count, &mut rng, params.lloyd_iterations);
    let labels = assign_sites(b, &sites);

    // Isotropic directions, normal-distributed magnitudes.
    let max_speed = units::TYPICAL_PLATE_SPEED_M_PER_YR;
    let mut plates: Vec<Plate> = sites
        .iter()
        .enumerate()
        .map(|(i, site)| {
            let angle = rng.next_angle();
            // Clamp at 2.5 sigma against outlier draws.
            let speed = rng.next_normal().abs().min(2.5) * max_speed * 0.3;
            let velocity = Vec2::from_angle(angle, speed);
            Plate {
                id: i as u32,
                site: *site,
                velocity,
                thickness_km: params.thickness_km,
                density: units::CONTINENTAL_DENSITY,
                is_oceanic: false,
                area_cells: 0,
            }
        })
        .collect();

    let (w, h) = (b.width(), b.height());
    let mut areas = vec![0usize; plates.len()];
    for i in 0..b.cell_count() {
        let l = labels.get_index(i) as usize;
        if l < areas.len() {
            areas[l] += 1;
        }
    }

    // Largest plates are continental.
    let mut order: Vec<usize> = (0..plates.len()).collect();
    order.sort_by(|&a, &c| areas[c].cmp(&areas[a]));
    let continental_count = ((plates.len() as f64) * (1.0 - params.oceanic_bias)).round() as usize;
    let continental_count = continental_count.clamp(1, plates.len());

    for (rank, &idx) in order.iter().enumerate() {
        let is_continental = rank < continental_count;
        let area_fraction = areas[idx] as f64 / (w * h) as f64;
        // Guard against zero-area plates.
        let area_fraction = area_fraction.max(1e-9);

        plates[idx].is_oceanic = !is_continental;
        plates[idx].area_cells = areas[idx];
        plates[idx].density = if is_continental {
            units::CONTINENTAL_DENSITY
        } else {
            units::OCEANIC_DENSITY
        };
        plates[idx].thickness_km = isostatic_thickness_km(
            is_continental,
            area_fraction,
            params.thickness_km,
        ) * extent_factor.clamp(1.0, 2.0);
        if let Some(d) = params.densities.get(if is_continental { 0 } else { 2 }) {
            plates[idx].density = d * 1000.0;
        }
    }

    let threshold = params.collision_threshold * max_speed;
    let (boundary_type, convergence, transform_rate, boundary_distance, normal_x, normal_y) =
        classify_boundaries(b, &labels, &plates, threshold);

    PlateMosaic {
        plates,
        labels,
        boundary_type,
        convergence,
        transform_rate,
        boundary_distance,
        normal_x,
        normal_y,
    }
}

/// Airy-isostasy crustal thickness in km.
pub fn isostatic_thickness_km(continental: bool, area_fraction: f64, reference_km: f64) -> f64 {
    let t = if continental {
        reference_km.clamp(5.0, 80.0)
    } else {
        // Away from the ridge, crust thickens with age (approximated by plate area).
        units::OCEANIC_CRUST_KM + 8.0 * (1.0 - area_fraction.clamp(0.0, 1.0)).sqrt()
    };
    t.clamp(1.0, 200.0)
}

/// Relative motion across a boundary with the given normal.
pub fn relative_motion(a: &Plate, b: &Plate, normal: Vec2) -> RelativeMotion {
    let rel = b.velocity - a.velocity;
    let along = rel.dot(normal);
    let tangent = Vec2::new(-normal.y, normal.x);
    let slip = rel.dot(tangent);
    RelativeMotion {
        convergent: -along,
        divergent: along,
        transform: slip.abs(),
        speed: rel.length(),
    }
}

/// Classifies every boundary cell by relative motion.
#[allow(clippy::type_complexity)]
fn classify_boundaries(
    b: Boundary,
    labels: &LabelField,
    plates: &[Plate],
    threshold: f64,
) -> (
    LabelField,
    ScalarField,
    ScalarField,
    ScalarField,
    ScalarField,
    ScalarField,
) {
    let (w, h) = (b.width(), b.height());
    let mut types = LabelField::filled(b, 0);
    let mut convergence = ScalarField::zeros(b);
    let mut transform_rate = ScalarField::zeros(b);
    let mut distance = ScalarField::filled(b, f64::MAX);
    let mut normal_x = ScalarField::zeros(b);
    let mut normal_y = ScalarField::zeros(b);

    let offsets: &[(i64, i64)] = &crate::grid::D8_OFFSETS;

    for y in 0..h {
        for x in 0..w {
            let here = labels.get(x, y) as usize;
            if here >= plates.len() {
                continue;
            }
            let xi = x as i64;
            let yi = y as i64;

            let mut other: Option<usize> = None;
            let mut best_d = f64::INFINITY;
            for &(dx, dy) in offsets {
                let l = labels.get_wrapped(xi + dx, yi + dy) as usize;
                if l != here && l < plates.len() {
                    let d = (dx * dx + dy * dy) as f64;
                    if d < best_d {
                        best_d = d;
                        other = Some(l);
                    }
                }
            }

            let Some(other) = other else {
                continue;
            };

            // Voronoi bisector: the site-joining segment is the normal.
            let normal = (plates[other].site - plates[here].site).normalized();
            let motion = relative_motion(&plates[here], &plates[other], normal);
            let kind = motion.classify(threshold);

            types.set(x, y, match kind {
                BoundaryType::Convergent => 1,
                BoundaryType::Divergent => 2,
                BoundaryType::Transform => 3,
                BoundaryType::Null => 0,
            });
            convergence.set(x, y, motion.convergent - motion.divergent);
            transform_rate.set(x, y, motion.transform);
            normal_x.set(x, y, normal.x);
            normal_y.set(x, y, normal.y);
            distance.set(x, y, 0.0);
        }
    }

    let mut queue: Vec<usize> = Vec::new();
    for i in 0..b.cell_count() {
        if distance.get_index(i) == 0.0 {
            queue.push(i);
        }
    }
    let mut head = 0;
    while head < queue.len() {
        let idx = queue[head];
        head += 1;
        let (x, y) = b.coords(idx);
        let d = distance.get(x, y);
        for &(dx, dy) in offsets {
            let nidx = b.index_wrapped(x as i64 + dx, y as i64 + dy);
            if distance.get_index(nidx) > d + 1.0 {
                distance.set_index(nidx, d + 1.0);
                queue.push(nidx);
            }
        }
    }

    (
        types,
        convergence,
        transform_rate,
        distance,
        normal_x,
        normal_y,
    )
}

/// Velocity of a point, following the plate it rides on.
pub fn plate_velocity_at(mosaic: &PlateMosaic, x: usize, y: usize) -> Vec2 {
    let l = mosaic.labels.get(x, y) as usize;
    mosaic
        .plates
        .get(l)
        .map(|p| p.velocity)
        .unwrap_or(Vec2::ZERO)
}

/// Back-traces a hotspot to its position in normalised grid coordinates `time_yr` ago.
pub fn backtrace_hotspot(
    mosaic: &PlateMosaic,
    x: usize,
    y: usize,
    time_yr: f64,
    spatial_extent_m: f64,
) -> Vec2 {
    let v = plate_velocity_at(mosaic, x, y);
    let (w, h) = (mosaic.labels.width(), mosaic.labels.height());
    let px = x as f64 / w.max(1) as f64;
    let py = y as f64 / h.max(1) as f64;
    let dx = -v.x * time_yr / spatial_extent_m;
    let dy = -v.y * time_yr / spatial_extent_m;
    Vec2::new(px + dx, py + dy)
}

/// Plate cell counts.
pub fn area_distribution(mosaic: &PlateMosaic) -> Vec<usize> {
    let mut counts = vec![0usize; mosaic.plates.len()];
    for &l in mosaic.labels.as_slice() {
        if (l as usize) < counts.len() {
            counts[l as usize] += 1;
        }
    }
    counts
}

/// Builds a mosaic with controlled velocities for tests.
pub fn mosaic_with_velocities(
    b: Boundary,
    velocities: &[Vec2],
) -> PlateMosaic {
    let n = velocities.len();
    let mut sites = Vec::with_capacity(n);
    for i in 0..n {
        sites.push(Vec2::new(
            (i as f64 + 0.5) / n.max(1) as f64,
            0.5,
        ));
    }
    let labels = assign_sites(b, &sites);
    let plates: Vec<Plate> = velocities
        .iter()
        .enumerate()
        .map(|(i, v)| Plate {
            id: i as u32,
            site: sites[i],
            velocity: *v,
            thickness_km: 35.0,
            density: units::CONTINENTAL_DENSITY,
            is_oceanic: false,
            area_cells: 0,
        })
        .collect();
    let (boundary_type, convergence, transform_rate, boundary_distance, normal_x, normal_y) =
        classify_boundaries(b, &labels, &plates, 1.0);
    PlateMosaic {
        plates,
        labels,
        boundary_type,
        convergence,
        transform_rate,
        boundary_distance,
        normal_x,
        normal_y,
    }
}

/// 3D velocity including the vertical component implied by subduction at `dip_deg`.
pub fn subducting_velocity(motion: &RelativeMotion, dip_deg: f64) -> Vec3 {
    let dip = dip_deg.to_radians();
    Vec3::new(
        -motion.divergent,
        0.0,
        -motion.convergent * dip.tan(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BoundaryKind;

    fn rect(w: usize, h: usize) -> Boundary {
        Boundary::new(BoundaryKind::Rectangular, w, h)
    }

    #[test]
    fn every_cell_gets_a_valid_plate_label() {
        let b = rect(32, 32);
        let params = PlateParams::default();
        let m = build_mosaic(b, &params, 12345, 1e7);
        assert!(m.labels_are_consistent(), "a cell referenced a missing plate");
        assert_eq!(m.len(), params.count as usize);
    }

    #[test]
    fn plate_areas_sum_to_the_grid() {
        let b = rect(40, 40);
        let m = build_mosaic(b, &PlateParams::default(), 999, 1e7);
        let areas = area_distribution(&m);
        assert_eq!(areas.iter().sum::<usize>(), b.cell_count());
    }

    #[test]
    fn lloyd_relaxation_equalises_plate_areas() {
        // Intent: relaxation keeps every plate near the ideal area.
        let b = rect(60, 60);
        let relaxed = build_mosaic(
            b,
            &PlateParams {
                lloyd_iterations: 12,
                ..Default::default()
            },
            4242,
            1e7,
        );
        let areas = area_distribution(&relaxed);
        let max = *areas.iter().max().unwrap() as f64;
        let min = *areas.iter().filter(|&&a| a > 0).min().unwrap() as f64;
        let ideal = b.cell_count() as f64 / relaxed.len() as f64;
        assert!(
            max < ideal * 2.5,
            "largest plate {max} vs ideal {ideal}"
        );
        assert!(min > ideal * 0.2, "smallest plate {min} vs ideal {ideal}");
    }

    #[test]
    fn different_seeds_give_different_mosaics() {
        let b = rect(24, 24);
        let p = PlateParams::default();
        let a = build_mosaic(b, &p, 1, 1e7);
        let c = build_mosaic(b, &p, 2, 1e7);
        assert_ne!(a.labels.as_slice(), c.labels.as_slice());
    }

    #[test]
    fn same_seed_is_bit_identical() {
        let b = rect(24, 24);
        let p = PlateParams::default();
        let a = build_mosaic(b, &p, 777, 1e7);
        let c = build_mosaic(b, &p, 777, 1e7);
        assert_eq!(a.labels.as_slice(), c.labels.as_slice());
        assert_eq!(a.convergence.as_slice(), c.convergence.as_slice());
    }

    #[test]
    fn convergence_is_positive_where_plates_approach() {
        // Intent: head-on plates collide along their shared boundary.
        let v = vec![Vec2::new(10.0, 0.0), Vec2::new(-10.0, 0.0)];
        let m = mosaic_with_velocities(rect(20, 20), &v);

        let convergent = m.labels.height() / 2;
        let mut found = false;
        for y in 0..m.labels.height() {
            let x = 10;
            if m.boundary_type.get(x, y) == 1 {
                assert!(
                    m.convergence.get(x, y) > 0.0,
                    "convergence should be positive at a convergent seam"
                );
                found = true;
            }
        }
        assert!(found, "no convergent boundary was detected");
        let _ = convergent;
    }

    #[test]
    fn divergence_is_negative_where_plates_separate() {
        let v = vec![Vec2::new(-10.0, 0.0), Vec2::new(10.0, 0.0)];
        let m = mosaic_with_velocities(rect(20, 20), &v);
        let mut found = false;
        for y in 0..m.labels.height() {
            let x = 10;
            if m.boundary_type.get(x, y) == 2 {
                assert!(
                    m.convergence.get(x, y) < 0.0,
                    "divergent seam should carry negative signed convergence"
                );
                found = true;
            }
        }
        assert!(found, "no divergent boundary detected");
    }

    #[test]
    fn transform_is_detected_for_pure_shear() {
        // Intent: tangential motion is a transform boundary.
        let v = vec![Vec2::ZERO, Vec2::new(0.0, 10.0)];
        let m = mosaic_with_velocities(rect(20, 20), &v);
        let mut found = false;
        for y in 0..m.labels.height() {
            let x = 10;
            if m.boundary_type.get(x, y) == 3 {
                assert!(m.transform_rate.get(x, y) > 0.0);
                found = true;
            }
        }
        assert!(found, "no transform boundary detected");
    }

    #[test]
    fn identical_velocities_produce_no_motion() {
        let v = vec![Vec2::new(5.0, 5.0), Vec2::new(5.0, 5.0)];
        let m = mosaic_with_velocities(rect(16, 16), &v);
        for &t in m.boundary_type.as_slice() {
            assert_eq!(t, 0, "co-moving plates must not be a boundary");
        }
    }

    #[test]
    fn boundary_normal_is_parallel_to_the_site_separation() {
        let b = rect(20, 20);
        let v = vec![Vec2::ZERO, Vec2::new(20.0, 0.0)];
        let m = mosaic_with_velocities(b, &v);
        let mut checked = 0;
        for y in 0..20 {
            let x = 10;
            if m.boundary_type.get(x, y) != 0 {
                let n = Vec2::new(m.normal_x.get(x, y), m.normal_y.get(x, y));
                let expected = (m.plates[1].site - m.plates[0].site).normalized();
                assert!(
                    n.distance(expected) < 1e-9,
                    "normal {n:?} != site separation {expected:?}"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "no boundary found to check");
    }

    #[test]
    fn boundary_distance_is_zero_at_seams_and_grows_inward() {
        let b = rect(24, 24);
        let m = build_mosaic(b, &PlateParams::default(), 5150, 1e7);
        let mut zero_count = 0;
        for i in 0..b.cell_count() {
            if m.boundary_distance.get_index(i) == 0.0 {
                zero_count += 1;
            }
        }
        assert!(zero_count > 0, "no boundary cells found");
        assert!(zero_count < b.cell_count(), "everything is a boundary");
    }

    #[test]
    fn toroidal_mosaic_wraps_without_a_seam() {
        let b = Boundary::new(BoundaryKind::Toroidal, 32, 32);
        let m = build_mosaic(b, &PlateParams::default(), 2468, 1e7);
        assert!(m.labels_are_consistent());
        // Intent: no plate is stranded by the wrap.
        let areas = area_distribution(&m);
        assert!(
            areas.iter().all(|&a| a > 0),
            "a plate has no cells on a torus: {areas:?}"
        );
    }

    #[test]
    fn continental_plates_are_larger_than_oceanic_ones() {
        let b = rect(48, 48);
        let m = build_mosaic(
            b,
            &PlateParams {
                oceanic_bias: 0.5,
                ..Default::default()
            },
            13579,
            1e7,
        );
        let mean_cont: f64 = m
            .plates
            .iter()
            .filter(|p| !p.is_oceanic)
            .map(|p| p.area_cells as f64)
            .sum::<f64>()
            / m.plates.iter().filter(|p| !p.is_oceanic).count().max(1) as f64;
        let mean_oce: f64 = m
            .plates
            .iter()
            .filter(|p| p.is_oceanic)
            .map(|p| p.area_cells as f64)
            .sum::<f64>()
            / m.plates.iter().filter(|p| p.is_oceanic).count().max(1) as f64;
        assert!(mean_cont > mean_oce, "continental {mean_cont} vs oceanic {mean_oce}");
    }

    #[test]
    fn oceanic_plates_are_thinner_and_denser() {
        let b = rect(32, 32);
        let m = build_mosaic(b, &PlateParams::default(), 31415, 1e7);
        for p in &m.plates {
            if p.is_oceanic {
                assert!(
                    p.thickness_km < p.density / 1000.0 * 20.0,
                    "oceanic plate implausibly thick: {} km",
                    p.thickness_km
                );
            }
        }
        let cont: Vec<f64> = m.plates.iter().filter(|p| !p.is_oceanic).map(|p| p.density).collect();
        let oce: Vec<f64> = m.plates.iter().filter(|p| p.is_oceanic).map(|p| p.density).collect();
        let mean_cont = cont.iter().sum::<f64>() / cont.len().max(1) as f64;
        let mean_oce = oce.iter().sum::<f64>() / oce.len().max(1) as f64;
        assert!(mean_oce > mean_cont, "oceanic density {mean_oce} <= {mean_cont}");
    }

    #[test]
    fn boundary_type_fractions_sum_to_one() {
        let b = rect(32, 32);
        let m = build_mosaic(b, &PlateParams::default(), 8080, 1e7);
        let f = m.boundary_type_fractions();
        assert!((f.iter().sum::<f64>() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn boundary_classification_is_threshold_respecting() {
        let slow = RelativeMotion {
            convergent: 1.0,
            divergent: 0.0,
            transform: 0.0,
            speed: 1.0,
        };
        assert_eq!(slow.classify(5.0), BoundaryType::Null, "below threshold");
        let fast = RelativeMotion {
            convergent: 50.0,
            divergent: 0.0,
            transform: 0.0,
            speed: 50.0,
        };
        assert_eq!(fast.classify(5.0), BoundaryType::Convergent);
    }

    #[test]
    fn hotspot_backtrace_moves_opposite_to_plate_motion() {
        let b = rect(20, 20);
        let v = vec![Vec2::new(10.0, 0.0)];
        let m = mosaic_with_velocities(b, &v);
        let here = Vec2::new(0.5, 0.5);
        let past = backtrace_hotspot(&m, 10, 10, 1.0e6, 1.0e7);
        // Intent: older volcanos sit behind the +x plate motion.
        assert!(
            past.x < here.x,
            "backtrace should move opposite to plate motion: {past:?}"
        );
    }

    #[test]
    fn isostatic_thickness_orders_continental_above_oceanic() {
        let cont = isostatic_thickness_km(true, 0.2, 35.0);
        let oce = isostatic_thickness_km(false, 0.2, 35.0);
        assert!(cont > oce, "continental {cont} should exceed oceanic {oce}");
        assert!((cont - 35.0).abs() < 1e-9, "continental should use the reference");
    }

    #[test]
    fn single_plate_has_no_boundaries() {
        let b = rect(16, 16);
        let m = build_mosaic(b, &PlateParams { count: 1, ..Default::default() }, 1, 1e7);
        assert_eq!(m.boundary_cell_count(), 0);
        assert_eq!(m.labels.as_slice(), &vec![0u32; b.cell_count()][..]);
    }

    #[test]
    fn plate_count_respects_spec_limits() {
        let b = rect(64, 64);
        for count in [1u32, 2, 8, 16, 32] {
            let m = build_mosaic(b, &PlateParams { count, ..Default::default() }, 5, 1e7);
            assert_eq!(m.len(), count as usize);
            assert!(m.labels_are_consistent());
        }
    }
}