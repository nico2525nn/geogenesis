//! Generation pipeline: Phase 0 → initial relief (spec §2).
//!
//! Runs once per world, before the time loop:
//!
//! 1. Plate mosaic from the seed.
//! 2. Tectonic uplift field (metres over the span).
//! 3. Fractal base relief, scaled to the vertical amplitude.
//! 4. Fold belt undulation along the anisotropic axis.
//! 5. Fault offsets at plate boundaries.
//! 6. Named landform stamps placed by the boundary map.
//!
//! The output elevation is broadcast into `[-vertical_m, +vertical_m]` only
//! after every contribution is applied, so no single term can dominate by
//! accident of units.

use crate::features::{FeatureKind, StampParams};
use crate::geology::{default_stratigraphy, Stratigraphy};
use crate::grid::ScalarField;
use crate::noise::{NoiseField, NoiseParams};
use crate::params::Params;
use crate::plates::{build_mosaic, PlateMosaic};
use crate::rng::{domain, Rng};
use crate::uplift::{compute_uplift, UpliftField};
use crate::world::World;

/// Everything the pipeline produces that later stages need.
#[derive(Debug, Clone)]
pub struct PipelineOutput {
    pub world: World,
    pub mosaic: PlateMosaic,
    pub uplift: UpliftField,
    pub strata: Vec<Stratigraphy>,
}

/// Runs Phases 0–2 and returns the initial world.
pub fn generate_initial(params: &Params) -> Result<PipelineOutput, crate::GeoError> {
    let mut p = params.clone();
    p.validate()?;
    let seed = p.world.effective_seed();
    let mut world = World::flat(&p.world)?;
    let b = p.world.boundary();
    let extent_m = p.world.spatial_km * 1000.0;

    // 1. Plates.
    let mosaic = build_mosaic(b, &p.plates, seed, extent_m);

    // 2. Uplift (metres over the span).
    let uplift = compute_uplift(&mosaic, b, &p.uplift, seed);

    // 3. Fractal base relief in [-1, 1]. Octaves above 4 add detail below the
    // grid spacing while costing one full extra evaluation per cell each;
    // clamp there so a UI-driven octaves=16 cannot silently 4x the cost.
    let mut base_noise = p.noise.clone();
    base_noise.octaves = base_noise.octaves.min(4);
    let noise_field = NoiseField::new(base_noise, seed);
    let cycles = uplift_cycles(&p.noise, extent_m);
    let mut relief = noise_field.fill(b, cycles);

    // 4. Fold belts: sinusoidal undulation along the anisotropic axis.
    apply_folds(&mut relief, &p, seed);

    // 5. Fault offsets at plate boundaries.
    apply_faults(&mut relief, &mosaic, &p);

    // 6. Combine: base + tectonic relief, then named stamps.
    relief.zip_with(&uplift.total, |n, u| {
        n * p.world.vertical_m * 0.45 + u * 0.55
    });
    apply_stamps(&mut relief, &mosaic, &p, seed);

    // Broadcast into the vertical range without clipping the shape.
    relief.rescale(-p.world.vertical_m, p.world.vertical_m);
    relief.sanitize_non_finite(p.world.sea_level_m);

    world.elevation_m = relief;
    world.sea_level_m = p.world.sea_level_m;

    // One stratigraphic column per plate (provinces follow tectonics).
    let strata = (0..mosaic.len().max(1))
        .map(|i| {
            default_stratigraphy(
                crate::rng::derive_seed(seed, domain::STRATA).wrapping_add(i as u64),
                6,
            )
        })
        .collect();

    Ok(PipelineOutput {
        world,
        mosaic,
        uplift,
        strata,
    })
}

/// Noise cycles across the domain for the base relief.
fn uplift_cycles(noise: &NoiseParams, extent_m: f64) -> f64 {
    let _ = extent_m;
    // 24–64 cycles across the domain gives features from ranges down to hills
    // at 256²; finer detail comes from the octaves, not the base frequency.
    // Octaves above 4 add sub-cell detail the grid cannot resolve while
    // doubling the per-cell cost per octave — the pipeline clamps there.
    32.0 * (1.0 + noise.persistence * 0.5)
}

/// Fold-belt undulation along the anisotropic axis.
fn apply_folds(relief: &mut ScalarField, params: &Params, seed: u64) {
    let b = *relief.boundary();
    let (w, h) = (b.width(), b.height());
    if params.fold_amplitude_m <= 0.0 || w == 0 {
        return;
    }
    let wavelength_cells =
        (params.fold_wavelength_km * 1000.0 / params.world.cell_size_m()).max(4.0);
    let angle = params.noise.anisotropy.direction;
    let (ca, sa) = (angle.cos(), angle.sin());
    let mut rng = Rng::new(seed, domain::FEATURES);
    let phase = rng.next_f64() * std::f64::consts::TAU;
    let amp = params.fold_amplitude_m / params.world.vertical_m.max(1.0);

    for y in 0..h {
        for x in 0..w {
            let u = (x as f64 * ca + y as f64 * sa) / wavelength_cells;
            let fold = (u * std::f64::consts::TAU + phase).sin() * amp * 0.5;
            let i = b.index(x, y);
            relief.as_mut_slice()[i] += fold;
        }
    }
}

/// Vertical fault offsets concentrated at plate boundaries.
fn apply_faults(relief: &mut ScalarField, mosaic: &PlateMosaic, params: &Params) {
    if params.fault_offset_m <= 0.0 {
        return;
    }
    let b = *relief.boundary();
    let scale = params.fault_offset_m / params.world.vertical_m.max(1.0);
    for y in 0..b.height() {
        for x in 0..b.width() {
            if mosaic.boundary_type.get(x, y) == 0 {
                continue;
            }
            // Offset falls off with distance from the boundary.
            let d = mosaic.boundary_distance.get(x, y);
            let falloff = (-d / 3.0).exp();
            let side = if mosaic.convergence.get(x, y) >= 0.0 { 1.0 } else { -1.0 };
            let i = b.index(x, y);
            relief.as_mut_slice()[i] += side * scale * falloff * 0.5;
        }
    }
}

/// Places named landform stamps guided by the boundary map.
fn apply_stamps(
    relief: &mut ScalarField,
    mosaic: &PlateMosaic,
    params: &Params,
    seed: u64,
) {
    let b = *relief.boundary();
    let mut rng = Rng::new(seed, domain::FEATURES);
    let v = params.world.vertical_m;

    // Convergent boundaries get ranges and trenches; divergent ones get rifts
    // and ridges; mover-over cells get isolated peaks and mesas.
    let mut placed = 0usize;
    for _ in 0..64 {
        let x = rng.range_f64(0.0, b.width() as f64);
        let y = rng.range_f64(0.0, b.height() as f64);
        let (xi, yi) = (x as usize % b.width(), y as usize % b.height());
        let kind = mosaic.boundary_type_at(xi, yi);
        let stamp_kind = match kind {
            crate::plates::BoundaryType::Convergent => {
                if rng.next_f64() < 0.6 {
                    FeatureKind::MountainRange
                } else {
                    FeatureKind::Trench
                }
            }
            crate::plates::BoundaryType::Divergent => {
                if rng.next_f64() < 0.5 {
                    FeatureKind::Rift
                } else {
                    FeatureKind::OceanRidge
                }
            }
            crate::plates::BoundaryType::Transform => FeatureKind::Rift,
            crate::plates::BoundaryType::Null => {
                let r = rng.next_f64();
                if r < 0.3 {
                    FeatureKind::IsolatedPeak
                } else if r < 0.5 {
                    FeatureKind::Mesa
                } else if r < 0.65 {
                    FeatureKind::Plateau
                } else if r < 0.8 {
                    FeatureKind::Caldera
                } else {
                    continue;
                }
            }
        };
        let size = rng.range_f64(4.0, 18.0);
        let height = match stamp_kind {
            FeatureKind::MountainRange => rng.range_f64(0.2, 0.6) * v,
            FeatureKind::Trench => -rng.range_f64(0.3, 0.8) * v,
            FeatureKind::Rift => -rng.range_f64(0.1, 0.4) * v,
            FeatureKind::OceanRidge => rng.range_f64(0.05, 0.2) * v,
            FeatureKind::IsolatedPeak => rng.range_f64(0.1, 0.4) * v,
            FeatureKind::Mesa => rng.range_f64(0.05, 0.2) * v,
            FeatureKind::Plateau => rng.range_f64(0.1, 0.3) * v,
            FeatureKind::Caldera => -rng.range_f64(0.05, 0.15) * v,
            _ => rng.range_f64(0.05, 0.2) * v,
        };
        let stamp = StampParams {
            kind: stamp_kind,
            centre: crate::linalg::Vec2::new(x, y),
            direction_deg: rng.range_f64(0.0, 360.0),
            length_cells: size * rng.range_f64(1.0, 2.5),
            width_cells: size * 0.5,
            height_m: height,
            asymmetry: rng.range_f64(0.0, 0.5),
            seed: rng.next_u64(),
        };
        crate::features::stamp_into(relief, &stamp, 0.5);
        placed += 1;
        if placed >= 24 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;

    #[test]
    fn initial_world_spans_the_vertical_range() {
        let p = Params::default();
        let out = generate_initial(&p).expect("generation failed");
        let f = &out.world.elevation_m;
        assert!((f.min() + p.world.vertical_m).abs() < 1e-6);
        assert!((f.max() - p.world.vertical_m).abs() < 1e-6);
        assert_eq!(f.non_finite_count(), 0);
    }

    #[test]
    fn initial_world_is_reproducible() {
        let p = Params::default();
        let a = generate_initial(&p).unwrap();
        let c = generate_initial(&p).unwrap();
        assert_eq!(a.world.elevation_m.as_slice(), c.world.elevation_m.as_slice());
    }

    #[test]
    fn different_seeds_give_different_worlds() {
        let mut p = Params::default();
        p.world.seed = 1;
        let mut q = Params::default();
        q.world.seed = 2;
        let a = generate_initial(&p).unwrap();
        let c = generate_initial(&q).unwrap();
        assert_ne!(
            a.world.elevation_m.as_slice(),
            c.world.elevation_m.as_slice()
        );
    }

    #[test]
    fn strata_columns_follow_plate_count() {
        let p = Params::default();
        let out = generate_initial(&p).unwrap();
        assert_eq!(out.strata.len(), out.mosaic.len().max(1));
    }

    #[test]
    fn invalid_params_are_rejected() {
        let mut p = Params::default();
        p.world.grid = (4, 4);
        assert!(generate_initial(&p).is_err());
    }
}