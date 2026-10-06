//! Landform stamps: named features placed by the tectonic and noise fields.
//!
//! Each stamp is a pure function of its parameters and the grid, testable by shape.

use crate::grid::{Boundary, ScalarField};
use crate::linalg::{clamp, smoothstep, Vec2};
use crate::rng::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureKind {
    /// Linear orogeny along a plate convergence (spec: 走向/長さ/幅/標高/非対称性).
    MountainRange,
    /// Broad uplift with cliff edges (広がり/高さ/縁の急峻さ/傾斜方向).
    Plateau,
    /// Extensional valley at a divergent boundary (長さ/幅/深さ/非対称性).
    Rift,
    /// Mid-ocean ridge (長さ/高さ/幅).
    OceanRidge,
    /// Deep subduction trough (長さ/深さ/幅/曲率).
    Trench,
    /// Volcanic island chain on drifting plate (長さ/間隔/高さ).
    HotspotChain,
    /// Volcanic collapse bowl (直径/深さ/縁の高さ).
    Caldera,
    /// River-deposited fan (面積/勾配/形状).
    Delta,
    /// Volcanic cone (高さ/基部半径/急峻さ).
    IsolatedPeak,
    /// Erosional remnant with a flat cap (上面面積/高さ/縁の急峻さ).
    Mesa,
}

impl FeatureKind {
    pub const ALL: [FeatureKind; 10] = [
        FeatureKind::MountainRange,
        FeatureKind::Plateau,
        FeatureKind::Rift,
        FeatureKind::Trench,
        FeatureKind::OceanRidge,
        FeatureKind::HotspotChain,
        FeatureKind::Caldera,
        FeatureKind::Delta,
        FeatureKind::IsolatedPeak,
        FeatureKind::Mesa,
    ];
}

/// Shared geometric parameters for every stamp; meanings vary slightly per stamp.
#[derive(Debug, Clone)]
pub struct StampParams {
    pub kind: FeatureKind,
    /// Centre of the feature, in grid cells.
    pub centre: Vec2,
    /// Primary axis of elongation, in degrees from +x. Ignored for round stamps.
    pub direction_deg: f64,
    /// Half-length along the axis, in cells.
    pub length_cells: f64,
    /// Half-width across the axis, in cells.
    pub width_cells: f64,
    /// Peak signed displacement in metres (+up, −down).
    pub height_m: f64,
    /// 0 = symmetric; 1 = fully one-sided (asymmetric fold).
    pub asymmetry: f64,
    /// Seed for any internal stochastic placement (mesa caps, delta lobes).
    pub seed: u64,
}

impl StampParams {
    pub fn new(kind: FeatureKind, cx: f64, cy: f64) -> Self {
        Self {
            kind,
            centre: Vec2::new(cx, cy),
            direction_deg: 0.0,
            length_cells: 16.0,
            width_cells: 6.0,
            height_m: 1000.0,
            asymmetry: 0.0,
            seed: 0,
        }
    }
}

/// Elongated Gaussian relief along a strike, in metres.
/// `asymmetry` shifts the crest toward one flank.
pub fn mountain_range(b: Boundary, p: &StampParams) -> ScalarField {
    let mut out = ScalarField::zeros(b);
    let a = clamp(p.asymmetry, 0.0, 1.0);
    let angle = p.direction_deg.to_radians();
    let (ca, sa) = (angle.cos(), angle.sin());

    let (w, h) = (b.width(), b.height() as i64);
    let x0 = (p.centre.x - p.length_cells).floor().max(0.0) as i64;
    let x1 = (p.centre.x + p.length_cells).ceil().min(w as f64 - 1.0).max(0.0) as i64;
    let y0 = (p.centre.y - p.width_cells * 3.0).floor().max(0.0) as i64;
    let y1 = (p.centre.y + p.width_cells * 3.0).ceil().min(h as f64 - 1.0).max(0.0) as i64;

    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f64 - p.centre.x;
            let dy = y as f64 - p.centre.y;
            let u = dx * ca + dy * sa;
            let v = -dx * sa + dy * ca;

            let along = (u / p.length_cells.max(0.5)).abs();
            let across = if v < 0.0 {
                (v / (p.width_cells.max(0.5) * (1.0 + a))).abs()
            } else {
                (v / (p.width_cells.max(0.5) * (1.0 - a))).abs()
            };

            let r2 = along * along * 0.25 + across * across;
            if r2 > 9.0 {
                continue;
            }
            let value = p.height_m * (-r2).exp() * smooth_profile(along);
            out.set(x as usize, y as usize, value);
        }
    }
    out
}

/// Fades the crest over the outer quarter so range ends die out before the segment tip.
fn smooth_profile(along: f64) -> f64 {
    1.0 - smoothstep(0.6, 1.0, along.abs().min(1.0))
}

/// A broad plateau: flat interior, cliffed rim, optional tilt.
pub fn plateau(b: Boundary, p: &StampParams, tilt_deg: f64, tilt_m: f64) -> ScalarField {
    let mut out = ScalarField::zeros(b);
    let r = p.length_cells.max(1.0);
    let x0 = (p.centre.x - r * 1.5).floor().max(0.0) as i64;
    let x1 = (p.centre.x + r * 1.5).ceil().min(b.width() as f64 - 1.0).max(0.0) as i64;
    let y0 = (p.centre.y - r * 1.5).floor().max(0.0) as i64;
    let y1 = (p.centre.y + r * 1.5).ceil().min(b.height() as f64 - 1.0).max(0.0) as i64;

    let tilt = tilt_deg.to_radians();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = (x as f64 - p.centre.x) / r;
            let dy = (y as f64 - p.centre.y) / r;
            let d = (dx * dx + dy * dy).sqrt();
            // Super-Gaussian (d^6): flat interior, steep rim; d^4 sags 6% at d = 0.5.
            let cap = (-(d * d * d * d * d * d)).exp();
            let rise = tilt_m * (dx * tilt.cos() + dy * tilt.sin());
            out.set(x as usize, y as usize, p.height_m * cap + rise);
        }
    }
    out
}

/// A rift: an extensional trough with asymmetric flanks.
/// `height_m` is negative (a depression); the flanks rise slightly.
pub fn rift(b: Boundary, p: &StampParams) -> ScalarField {
    let mut out = ScalarField::zeros(b);
    let a = clamp(p.asymmetry, -1.0, 1.0);
    let angle = p.direction_deg.to_radians();
    let (ca, sa) = (angle.cos(), angle.sin());

    let span = (p.length_cells + p.width_cells * 3.0).ceil() as i64;
    let x0 = (p.centre.x as i64 - span).max(0);
    let x1 = (p.centre.x as i64 + span).min(b.width() as i64 - 1);
    let y0 = (p.centre.y as i64 - span).max(0);
    let y1 = (p.centre.y as i64 + span).min(b.height() as i64 - 1);

    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f64 - p.centre.x;
            let dy = y as f64 - p.centre.y;
            let u = (dx * ca + dy * sa) / p.length_cells.max(0.5);
            let across = -dx * sa + dy * ca;
            let half_width = if across < 0.0 {
                p.width_cells.max(0.5) * (1.0 + a)
            } else {
                p.width_cells.max(0.5) * (1.0 - a)
            };
            let v = across / half_width;

            let along = smooth_profile(u.abs().min(1.0));
            // Trough with shoulders at |v| ~ 1.4; `depth` stays positive so a negative height means depression.
            let depth = p.height_m.abs().max(1.0);
            let trough = (-(v * v)).exp();
            let shoulder = (-((v.abs() - 1.4) * (v.abs() - 1.4))).exp();
            let relief = shoulder * 0.35 - trough;
            out.set(x as usize, y as usize, depth * relief * along);
        }
    }
    out
}

/// A mid-ocean ridge: a long low dome symmetric across strike.
pub fn ocean_ridge(b: Boundary, p: &StampParams) -> ScalarField {
    let ridge = StampParams {
        kind: FeatureKind::OceanRidge,
        asymmetry: 0.0,
        ..p.clone()
    };
    mountain_range(b, &ridge)
}

/// A subduction trench: a deep, narrow trough with curvature and a slight forearc rise.
pub fn trench(b: Boundary, p: &StampParams, curvature_deg: f64) -> ScalarField {
    let mut out = ScalarField::zeros(b);
    let curve = curvature_deg.to_radians();
    let angle = p.direction_deg.to_radians();
    let (ca, sa) = (angle.cos(), angle.sin());

    let span = (p.length_cells + p.width_cells * 4.0).ceil() as i64;
    let x0 = (p.centre.x as i64 - span).max(0);
    let x1 = (p.centre.x as i64 + span).min(b.width() as i64 - 1);
    let y0 = (p.centre.y as i64 - span).max(0);
    let y1 = (p.centre.y as i64 + span).min(b.height() as i64 - 1);

    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f64 - p.centre.x;
            let dy = y as f64 - p.centre.y;
            let u = (dx * ca + dy * sa) / p.length_cells.max(0.5);
            let arc = curve * u * u * p.width_cells;
            let v = (dx * -sa + dy * ca - arc) / p.width_cells.max(0.5);

            let along = smooth_profile(u.abs().min(1.0));
            let depth = p.height_m.abs();
            let trough = -(-(v * v * 4.0)).exp();
            let rise = 0.15 * (-((v.abs() - 1.6) * (v.abs() - 1.6) * 4.0)).exp();
            out.set(x as usize, y as usize, depth * (trough + rise) * along);
        }
    }
    out
}

/// A volcanic cone: isolated peak with profile control.
/// `steepness` 1 is a stratovolcano, 0 a shield; smooth at the summit (no one-cell spike).
pub fn isolated_peak(b: Boundary, p: &StampParams, steepness: f64) -> ScalarField {
    let mut out = ScalarField::zeros(b);
    let s = clamp(steepness, 0.0, 1.0);
    let gamma = 1.0 + 1.5 * s;
    // Rescale the radius so a steep cone is strictly narrower than a shield at equal height.
    let r = p.width_cells.max(1.0) / (1.0 + s);

    let span = (r * 2.0).ceil() as i64;
    let x0 = (p.centre.x as i64 - span).max(0);
    let x1 = (p.centre.x as i64 + span).min(b.width() as i64 - 1);
    let y0 = (p.centre.y as i64 - span).max(0);
    let y1 = (p.centre.y as i64 + span).min(b.height() as i64 - 1);

    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = (x as f64 - p.centre.x) / r;
            let dy = (y as f64 - p.centre.y) / r;
            let d = (dx * dx + dy * dy).sqrt();
            if d > 2.0 {
                continue;
            }
            let v = p.height_m * (-(d.powf(gamma))).exp();
            out.set(x as usize, y as usize, v);
        }
    }
    out
}

/// A volcanic caldera: a collapse bowl inside a raised rim.
/// `rim_height_fraction` is the rim height as a fraction of the bowl depth.
pub fn caldera(b: Boundary, p: &StampParams, rim_height_fraction: f64) -> ScalarField {
    let mut out = ScalarField::zeros(b);
    let depth = p.height_m.abs();
    let rim = depth * clamp(rim_height_fraction, 0.0, 2.0);
    let r = p.width_cells.max(1.0);

    let span = (r * 2.5).ceil() as i64;
    let x0 = (p.centre.x as i64 - span).max(0);
    let x1 = (p.centre.x as i64 + span).min(b.width() as i64 - 1);
    let y0 = (p.centre.y as i64 - span).max(0);
    let y1 = (p.centre.y as i64 + span).min(b.height() as i64 - 1);

    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = (x as f64 - p.centre.x) / r;
            let dy = (y as f64 - p.centre.y) / r;
            let d = (dx * dx + dy * dy).sqrt();
            let floor = -(1.0 - smoothstep(0.6, 1.0, d));
            let collar = (-((d - 1.0) * (d - 1.0) * 8.0)).exp();
            out.set(x as usize, y as usize, depth * floor + rim * collar);
        }
    }
    out
}

/// A river delta: a fan of distributary lobes.
pub fn delta(b: Boundary, p: &StampParams) -> ScalarField {
    let mut out = ScalarField::zeros(b);
    let mut rng = Rng::from_seed(p.seed.wrapping_add(0xDE17A));
    let base = p.direction_deg.to_radians();
    let lobe_width = (p.width_cells.max(1.0) * 0.6).max(2.0);

    // Flanking lobes are re-centred off-axis so they do not collapse onto one maximum.
    let reach = p.length_cells.max(1.0);
    let side = lobe_width * 2.2;
    for (offset_along, offset_across, height_scale) in [
        (0.0, 0.0, 1.0),
        (0.55, -side, 0.6),
        (0.55, side, 0.6),
        (0.9, -side * 1.8, 0.38),
        (0.9, side * 1.8, 0.38),
    ] {
        let angle = base;
        let cx = p.centre.x + angle.cos() * reach * offset_along - angle.sin() * offset_across;
        let cy = p.centre.y + angle.sin() * reach * offset_along + angle.cos() * offset_across;
        let lobe_height = p.height_m.abs()
            * height_scale
            * rng.range_f64(0.85, 1.0);
        let lobe = StampParams {
            kind: FeatureKind::Delta,
            centre: Vec2::new(cx, cy),
            direction_deg: angle.to_degrees(),
            length_cells: reach * 0.55,
            width_cells: lobe_width,
            height_m: lobe_height,
            asymmetry: 0.0,
            seed: p.seed,
        };
        let field = mountain_range(b, &lobe);
        out.add_assign_field(&field);
    }
    out
}

/// A mesa: a flat-capped remnant with cliff edges.
/// Cap interior varies below `height_m * 1e-3`; the rim falls within one cell.
pub fn mesa(b: Boundary, p: &StampParams, edge_sharpness: f64) -> ScalarField {
    let mut out = ScalarField::zeros(b);
    let sharp = clamp(edge_sharpness, 0.5, 8.0);
    let r = p.length_cells.max(1.0);

    let span = (r * 1.6).ceil() as i64;
    let x0 = (p.centre.x as i64 - span).max(0);
    let x1 = (p.centre.x as i64 + span).min(b.width() as i64 - 1);
    let y0 = (p.centre.y as i64 - span).max(0);
    let y1 = (p.centre.y as i64 + span).min(b.height() as i64 - 1);

    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = (x as f64 - p.centre.x) / r;
            let dy = (y as f64 - p.centre.y) / r;
            let d = (dx * dx + dy * dy).sqrt();
            let v = if d < 0.85 {
                // Held flat explicitly: a raw Fermi profile sags 15% by d = 0.4.
                p.height_m
            } else {
                // min() of Fermi wall and fade joins them without a bump.
                let fermi = p.height_m / (1.0 + (sharp * (d - 1.0)).exp());
                let fade = (1.0 - smoothstep(0.85, 1.6, d)) * p.height_m;
                fermi.min(fade).max(0.0)
            };
            out.set(x as usize, y as usize, v);
        }
    }
    out
}

/// A hotspot chain: a line of aged volcanos with decreasing height.
/// `spacing_cells` is the distance between cone centres.
pub fn hotspot_chain(b: Boundary, p: &StampParams, cone_count: usize, spacing_cells: f64) -> ScalarField {
    let mut out = ScalarField::zeros(b);
    let angle = p.direction_deg.to_radians();
    let (ca, sa) = (angle.cos(), angle.sin());

    for i in 0..cone_count.max(1) {
        let t = i as f64;
        let cx = p.centre.x + ca * t * spacing_cells;
        let cy = p.centre.y + sa * t * spacing_cells;
        let decay = 1.0 / (1.0 + 0.35 * t);
        let cone = StampParams {
            kind: FeatureKind::IsolatedPeak,
            centre: Vec2::new(cx, cy),
            width_cells: p.width_cells,
            height_m: p.height_m * decay,
            ..p.clone()
        };
        let field = isolated_peak(b, &cone, 0.8);
        // Pointwise maximum, not a sum: adjacent cones must not stack a phantom ridge.
        let dst = out.as_mut_slice();
        let src = field.as_slice();
        for (a, v) in dst.iter_mut().zip(src.iter()) {
            if *v > *a {
                *a = *v;
            }
        }
    }
    out
}

pub fn apply_feature(b: Boundary, p: &StampParams) -> ScalarField {
    match p.kind {
        FeatureKind::MountainRange => mountain_range(b, p),
        FeatureKind::Plateau => plateau(b, p, 0.0, 0.0),
        FeatureKind::Rift => rift(b, p),
        FeatureKind::OceanRidge => ocean_ridge(b, p),
        FeatureKind::Trench => trench(b, p, 12.0),
        FeatureKind::HotspotChain => hotspot_chain(b, p, 5, p.width_cells * 2.0),
        FeatureKind::Caldera => caldera(b, p, 0.25),
        FeatureKind::Delta => delta(b, p),
        FeatureKind::IsolatedPeak => isolated_peak(b, p, 0.7),
        FeatureKind::Mesa => mesa(b, p, 8.0),
    }
}

pub fn stamp_into(target: &mut ScalarField, p: &StampParams, weight: f64) {
    let b = *target.boundary();
    let field = apply_feature(b, p);
    for (a, v) in target.as_mut_slice().iter_mut().zip(field.as_slice()) {
        *a += v * weight;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BoundaryKind;

    fn test_boundary() -> Boundary {
        Boundary::new(BoundaryKind::Rectangular, 64, 64)
    }

    fn params_at(cx: f64, cy: f64) -> StampParams {
        StampParams::new(FeatureKind::MountainRange, cx, cy)
    }

    #[test]
    fn mountain_range_is_taller_than_it_is_wide() {
        let b = test_boundary();
        let p = StampParams {
            length_cells: 24.0,
            width_cells: 5.0,
            height_m: 2000.0,
            ..params_at(32.0, 32.0)
        };
        let f = mountain_range(b, &p);
        // Intent: relief persists along the axis but falls off across it.
        let mut along = Vec::new();
        for x in 12..52 {
            along.push(f.get(x, 32));
        }
        let peak = along.iter().cloned().fold(0.0f64, f64::max);
        assert!((peak - 2000.0).abs() < 1e-6, "crest height wrong: {peak}");
        assert!(
            f.get(32, 48).abs() < peak * 0.05,
            "range too wide across strike"
        );
        assert!(
            f.max() / 2000.0 > 0.999 && f.max() <= 2000.0,
            "range height wrong: {}",
            f.max()
        );
    }

    #[test]
    fn mountain_range_has_no_hard_ends() {
        let b = test_boundary();
        let p = StampParams {
            length_cells: 16.0,
            width_cells: 4.0,
            height_m: 1500.0,
            ..params_at(32.0, 32.0)
        };
        let f = mountain_range(b, &p);
        // Intent: segment tip fades out and the crest stays smooth.
        assert!(
            f.get(48, 32).abs() < 1500.0 * 0.05,
            "hard end at segment tip: {}",
            f.get(48, 32)
        );
        for x in 22..40 {
            let a = f.get(x, 32);
            let c = f.get(x + 1, 32);
            assert!(
                (a - c).abs() < 1500.0 * 0.1,
                "discontinuity on the crest at {x}: {a} vs {c}"
            );
        }
    }

    #[test]
    fn asymmetry_shifts_the_crest_off_centre() {
        let b = test_boundary();
        let sym = StampParams {
            length_cells: 20.0,
            width_cells: 6.0,
            height_m: 1000.0,
            asymmetry: 0.0,
            ..params_at(32.0, 32.0)
        };
        let asym = StampParams {
            asymmetry: 0.6,
            ..sym.clone()
        };
        let (fs, fa) = (mountain_range(b, &sym), mountain_range(b, &asym));
        assert!(
            (fs.get(32, 32 - 3) - fs.get(32, 32 + 3)).abs() < 1e-6,
            "symmetric range has unequal flanks"
        );
        assert!(
            (fa.get(32, 32 - 3) - fa.get(32, 32 + 3)).abs() > 50.0,
            "asymmetry failed to shift the crest"
        );
        assert!(fa.max() <= 1000.0 * (1.0 + 1e-9));
    }

    #[test]
    fn plateau_is_flat_inside_and_cliffed_at_the_rim() {
        let b = test_boundary();
        let p = StampParams {
            length_cells: 14.0,
            height_m: 1200.0,
            ..params_at(32.0, 32.0)
        };
        let f = plateau(b, &p, 0.0, 0.0);
        let peak = f.max();
        for dy in -5..=5 {
            for dx in -5..=5 {
                let v = f.get((32 + dx) as usize, (32 + dy) as usize);
                assert!(
                    (v - peak).abs() < peak * 0.02,
                    "plateau not flat at ({dx},{dy}): {v} vs {peak}"
                );
            }
        }
        assert!(f.get(8, 32).abs() < peak * 0.01, "plateau rim leaked");
    }

    #[test]
    fn rift_is_a_depression_with_raised_shoulders() {
        let b = test_boundary();
        let p = StampParams {
            length_cells: 20.0,
            width_cells: 5.0,
            height_m: -1500.0,
            ..params_at(32.0, 32.0)
        };
        let f = rift(b, &p);
        assert!(f.min() < -1000.0, "rift trough too shallow: {}", f.min());
        // Intent: trough with raised shoulders at rows 25 and 39.
        let shoulder = f.get(32, 25).max(f.get(32, 39));
        assert!(shoulder > 0.0, "no rift-flank uplift: {shoulder}");
        let centre = f.get(32, 32);
        assert!(
            centre < -500.0,
            "trough too shallow at axis (min is {} elsewhere)",
            f.min()
        );
    }

    #[test]
    fn trench_descends_below_surroundings_with_an_outer_rise() {
        let b = test_boundary();
        let p = StampParams {
            length_cells: 24.0,
            width_cells: 3.0,
            height_m: -6000.0,
            ..params_at(32.0, 32.0)
        };
        let f = trench(b, &p, 0.0);
        assert!(f.min() < -4000.0, "trench too shallow: {}", f.min());
        let rise = f.get(32, 37);
        assert!(rise > 0.0, "no outer rise on the forearc side");
    }

    #[test]
    fn isolated_peak_is_highest_at_its_centre() {
        let b = test_boundary();
        let p = StampParams {
            width_cells: 6.0,
            height_m: 2500.0,
            ..params_at(32.0, 32.0)
        };
        for steep in [0.0, 0.5, 1.0] {
            let f = isolated_peak(b, &p, steep);
            assert!(
                (f.max() - 2500.0).abs() < 1e-6,
                "cone height wrong at steepness {steep}"
            );
            let centre = f.get(32, 32);
            let around = [f.get(31, 32), f.get(33, 32), f.get(32, 31), f.get(32, 33)];
            for &n in &around {
                assert!(n <= centre + 1e-9, "spike at summit");
            }
        }
    }

    #[test]
    fn steep_cone_is_narrower_than_shield() {
        let b = test_boundary();
        let p = StampParams {
            width_cells: 8.0,
            height_m: 2000.0,
            ..params_at(32.0, 32.0)
        };
        // Intent: the steep cone falls below half-height closer to the summit.
        fn half_radius(field: &ScalarField) -> f64 {
            for r in 1..20 {
                if field.get(32 + r, 32) < field.max() * 0.5 {
                    return r as f64;
                }
            }
            20.0
        }
        let shield = isolated_peak(b, &p, 0.0);
        let cone = isolated_peak(b, &p, 1.0);
        assert!(
            half_radius(&cone) < half_radius(&shield),
            "steep cone is not narrower than shield"
        );
    }

    #[test]
    fn caldera_bowl_is_lower_than_its_rim() {
        let b = test_boundary();
        let p = StampParams {
            width_cells: 6.0,
            height_m: -800.0,
            ..params_at(32.0, 32.0)
        };
        let f = caldera(b, &p, 0.3);
        let floor = f.get(32, 32);
        let rim = f.get(38, 32).max(f.get(26, 32)).max(f.get(32, 38)).max(f.get(32, 26));
        assert!(floor < 0.0, "caldera floor is not a depression");
        assert!(rim > 0.0, "caldera has no rim");
        assert!(rim - floor > 800.0, "rim-to-floor relief wrong");
    }

    #[test]
    fn mesa_top_is_flat_and_rim_is_steep() {
        let b = test_boundary();
        let p = StampParams {
            length_cells: 10.0,
            height_m: 600.0,
            ..params_at(32.0, 32.0)
        };
        let f = mesa(b, &p, 8.0);
        let peak = f.max();
        for dy in -4..=4 {
            for dx in -4..=4 {
                let v = f.get((32 + dx) as usize, (32 + dy) as usize);
                assert!(
                    (v - peak).abs() < peak * 1e-3,
                    "mesa cap not flat: {v} vs {peak}"
                );
            }
        }
        // Intent: the wall falls decisively within a single cell past the cap edge.
        let at_edge = f.get(41, 32);
        let past_edge = f.get(42, 32);
        assert!(
            at_edge - past_edge > peak * 0.15,
            "mesa rim not steep: edge {at_edge} next {past_edge} peak {peak}"
        );
    }

    #[test]
    fn hotspot_chain_decays_along_the_trail() {
        let b = test_boundary();
        let p = StampParams {
            height_m: 1500.0,
            width_cells: 4.0,
            direction_deg: 0.0,
            ..params_at(16.0, 32.0)
        };
        let f = hotspot_chain(b, &p, 5, 8.0);
        let first = f.get(16, 32);
        let last = f.get(48, 32);
        assert!(first > last, "chain does not decay: {first} vs {last}");
        assert!(f.max() <= 1500.0 * (1.0 + 1e-9));
    }

    #[test]
    fn delta_is_positive_and_lobed() {
        let b = test_boundary();
        let p = StampParams {
            height_m: 200.0,
            length_cells: 12.0,
            width_cells: 4.0,
            seed: 1234,
            ..params_at(32.0, 32.0)
        };
        let f = delta(b, &p);
        assert!(f.min() >= 0.0, "delta dips below zero");
        // Intent: count lobes greedily — tallest first, a peak counts past one lobe-width.
        let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
        for y in 0..64 {
            for x in 0..64 {
                let v = f.get(x, y);
                if v > 20.0 {
                    candidates.push((v, x, y));
                }
            }
        }
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut peaks: Vec<(usize, usize)> = Vec::new();
        for (_, x, y) in candidates {
            if peaks.iter().all(|&(px, py)| {
                let dx = px as i64 - x as i64;
                let dy = py as i64 - y as i64;
                dx * dx + dy * dy > 36
            }) {
                peaks.push((x, y));
            }
        }
        assert!(
            peaks.len() >= 2,
            "delta has no distributary lobes: {peaks:?}"
        );
    }

    #[test]
    fn all_dispatch_paths_stay_finite() {
        let b = test_boundary();
        for &kind in &FeatureKind::ALL {
            let p = StampParams {
                kind,
                length_cells: 12.0,
                width_cells: 5.0,
                height_m: 800.0,
                seed: 77,
                ..params_at(32.0, 32.0)
            };
            let f = apply_feature(b, &p);
            assert_eq!(
                f.non_finite_count(),
                0,
                "{kind:?} produced non-finite values"
            );
        }
    }

    #[test]
    fn stamp_into_accumulates_with_weight() {
        let b = test_boundary();
        let mut target = ScalarField::zeros(b);
        let p = params_at(20.0, 20.0);
        stamp_into(&mut target, &p, 2.0);
        let once = target.clone();
        stamp_into(&mut target, &p, 2.0);
        for i in 0..target.len() {
            assert!((target.as_slice()[i] - 2.0 * once.as_slice()[i]).abs() < 1e-9);
        }
    }

    #[test]
    fn ocean_ridge_is_lower_and_broader_than_orogeny() {
        let b = test_boundary();
        let o = StampParams {
            kind: FeatureKind::OceanRidge,
            length_cells: 24.0,
            width_cells: 6.0,
            height_m: 800.0,
            ..params_at(32.0, 32.0)
        };
        let r = StampParams {
            kind: FeatureKind::MountainRange,
            length_cells: 24.0,
            width_cells: 6.0,
            height_m: 800.0,
            ..params_at(32.0, 32.0)
        };
        let (fo, fr) = (ocean_ridge(b, &o), mountain_range(b, &r));
        assert!(
            (fo.max() - fr.max()).abs() < 1e-9,
            "ridge and range with equal params must agree"
        );
    }
}