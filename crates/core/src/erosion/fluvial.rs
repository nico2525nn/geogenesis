//! Fluvial incision and sediment transport (spec §3b).
//!
//! Three transport laws share one framework:
//!
//! * **Detachment-limited stream power** (`fluvial_law = 0`):
//!   `E = K A^m S^n`, solved *implicitly* so geological time steps stay stable.
//! * **Transport-limited** (`= 1`): the channel erodes up to (and deposits
//!   above) its sediment transport capacity `Qc = Kt A^m S^n`; all sediment is
//!   routed downstream and deposited where capacity drops.
//! * **SPACE erosion–deposition** (`= 2`): detachment is limited by the
//!   available capacity (`E = K A^m S^n (1 - Qs/Qc)`) while deposition settles
//!   at a velocity `V` (`D = V Qs/Qc`), the standard SPACE formulation.
//!
//! # Discretisation
//!
//! All volumes below are per step and per cell; `A` is the D8 drainage area,
//! `A_c` the physical cell area (`Grid::cell_area_m2_at`, `∝ cos φ` on a
//! sphere), `dx` the distance to the receiver, `z_r` the receiver elevation,
//! `dt` the step and `Kt_eff` the effective transport coefficient (rock
//! hardness, vegetation and the incision threshold folded in). The capacity
//! volume is evaluated at the *unknown* new elevation `z`,
//!
//! ```text
//! C(z) = Kt_eff A^m dt A_c ((z - z_r)/dx)^n          (C = 0 when z ≤ z_r).
//! ```
//!
//! * **Detachment-limited** solves `z = z0 - K A^m dt A_c ((z - z_r)/dx)^n`
//!   with a bracketed Newton iteration (Braun & Willett, 2013). For `n = 1`
//!   that reduces to the exact linear solution; for other `n` the iteration
//!   converges to the discrete steady state `S = (U/K)^(1/n) A^(-m/n)`.
//! * **Depositional laws** (transport-limited and SPACE) are solved with an
//!   upstream-to-downstream sweep that carries the sediment volume `V_in`
//!   arriving from the donors, and *implicitly* per cell:
//!
//!   - supply deficit (`V_in < C(z0)`): erosion. With `λ = 1` for the
//!     transport-limited law and `λ = K/Kt` for SPACE, the cell solves
//!     `z = z0 - λ (C(z) - V_in) / A_c`. `F(z)` is strictly increasing with
//!     `F(z_r) < 0 < F(z0)`, so the bracketed iteration is unconditionally
//!     stable and a cell can never be incised below its receiver; the implicit
//!     root replaces the old explicit relief fraction cap.
//!   - supply excess (`V_in ≥ C(z0)`): deposition. The outgoing load is the
//!     *exact* exponential relaxation of the excess toward capacity,
//!     `φ = 1 - exp(-kd dt)` (no `clamp`, stable for any `dt`),
//!     `V_out = (1-φ) V_in + φ C(z)`, i.e. the cell solves
//!     `z = z0 + φ (V_in - C(z)) / A_c`. `G(z)` is monotone and bracketed by
//!     `z0` and `z0 + φ V_in/A_c`, so the root always exists and the implicit
//!     step cannot overshoot the available sediment.
//!
//!   Mass is conserved exactly by construction:
//!   `V_out = V_in + V_eroded - V_deposited`.
//!
//! Lakes trap sediment up to their accommodation, deltas trap a share at the
//! coastline and a fraction of channel-bed deposition is spread onto the
//! adjacent floodplain; everything not deposited is exported. The returned
//! [`MassBalance`] therefore closes to floating-point precision, and since
//! every solve is a deterministic bracketed iteration over a fixed cell order
//! the laws stay bit-reproducible.

use crate::climate::Climate;
use crate::erosion::{with_scratch, MassBalance};
use crate::geology::Geology;
use crate::grid::{Field, Grid};
use crate::hydrology::Hydro;
use crate::params::{FluvialParams, Params};
use crate::world::World;

/// Seconds per Julian year, for converting annual discharge to m³/s.
const SECONDS_PER_YEAR: f64 = 3.155_760e7;
/// Share of the incoming load a coastline traps as a delta; the rest is
/// exported to the ocean sink.
const OCEAN_DEPOSIT_SHARE: f64 = 0.3;
/// Share of channel-bed deposition parked on a low floodplain bank.
const FLOODPLAIN_SHARE: f64 = 0.5;

/// Applies fluvial incision and sediment transport for one time step.
pub fn apply(
    world: &mut World,
    params: &Params,
    hydro: &Hydro,
    climate: &Climate,
    geology: &Geology,
    dt_yr: f64,
) -> MassBalance {
    let f = &params.fluvial;
    if dt_yr <= 0.0 || f.kf <= 0.0 || f.kf.is_nan() {
        return MassBalance::default();
    }
    let law = f.law.round();
    let mut balance = if law < 0.5 {
        detachment_limited(world, params, hydro, climate, geology, dt_yr)
    } else {
        depositional(world, params, hydro, climate, geology, dt_yr, law >= 1.5)
    };
    if f.bank_erosion > 0.0 {
        bank_erosion(world, params, hydro, dt_yr, &mut balance);
    }
    balance
}

/// D8 receiver geometry of a channel cell: `(receiver, path length, slope)`.
#[inline]
fn receiver_slope(
    grid: Grid,
    hydro: &Hydro,
    world: &World,
    index: usize,
) -> Option<(usize, f64, f64)> {
    let receiver = hydro.receiver_of(index)?;
    let dir = hydro.flow_dir.get_index(index) as usize;
    let dx = if dir < 8 {
        grid.step_distance_m(dir)
    } else {
        grid.cell_size_m()
    };
    let z0 = world.elevation_m.get_index(index);
    let z_bed = world.elevation_m.get_index(receiver);
    Some((receiver, dx, (z0 - z_bed) / dx))
}

/// Erodibility at a cell after rock hardness, vegetation protection and the
/// incision threshold have been applied. `None` when the threshold suppresses
/// incision entirely.
fn effective_erodibility(
    f: &FluvialParams,
    root_strength: f64,
    geology: &Geology,
    climate: &Climate,
    index: usize,
    area_m2: f64,
    slope: f64,
) -> Option<f64> {
    let rock = f64::from(geology.erodibility.get_index(index)).max(0.05);
    let veg = f64::from(climate.vegetation.get_index(index)).clamp(0.0, 1.0);
    let protection = 1.0 - 0.75 * root_strength.clamp(0.0, 1.0) * veg;
    let mut coefficient = f.kf * rock * protection.max(0.0);
    if f.incision_threshold > 0.0 {
        let power = coefficient * area_m2.powf(f.area_exponent) * slope.powf(f.slope_exponent);
        if !(power > f.incision_threshold) {
            return None;
        }
        coefficient *= 1.0 - f.incision_threshold / power;
    }
    Some(coefficient)
}

/// Solves the implicit stream-power step
///
/// `z - z0 + C (z - z_bed)^n = 0`, with `C = K A^m dt / dx^n`,
///
/// for `z` in `[z_bed, z0]` using a Newton iteration safeguarded by bisection.
/// The residual function is monotonically increasing, so a bracket always
/// exists and the iteration is unconditionally convergent.
#[must_use]
pub fn implicit_stream_power(z0: f64, z_bed: f64, coefficient: f64, dx: f64, n: f64) -> f64 {
    let span = z0 - z_bed;
    if !(span > 0.0) || !(coefficient > 0.0) || !(dx > 0.0) || !coefficient.is_finite() {
        return z0;
    }
    if !(n > 0.0) || !n.is_finite() {
        return z0;
    }
    let c = coefficient / dx.powf(n);
    if !c.is_finite() || c <= 0.0 {
        return z_bed;
    }
    if (n - 1.0).abs() < 1e-12 {
        // Exact linear solution; this is the common n = 1 fast path.
        return (z0 + c * z_bed) / (1.0 + c);
    }
    let residual = |z: f64| z - z0 + c * (z - z_bed).max(0.0).powf(n);
    let mut lo = z_bed;
    let mut hi = z0;
    let mut z = 0.5 * (lo + hi);
    let tolerance = 1e-13 * (span.abs() + 1.0);
    for _ in 0..60 {
        let f = residual(z);
        if f.abs() <= tolerance {
            return z;
        }
        if f < 0.0 {
            lo = z;
        } else {
            hi = z;
        }
        let d = (z - z_bed).max(0.0);
        let derivative = 1.0 + n * c * d.powf(n - 1.0);
        let mut next = if derivative.is_finite() && derivative > 0.0 {
            z - f / derivative
        } else {
            f64::NAN
        };
        if !(next > lo && next < hi) {
            next = 0.5 * (lo + hi);
        }
        z = next;
    }
    z
}

/// Volume balance of the implicit erosion–deposition solve of one channel cell.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ChannelStep {
    /// Sediment volume eroded from the cell during the step, m³.
    erosion_m3: f64,
    /// Sediment volume deposited on the cell during the step, m³.
    deposition_m3: f64,
    /// Sediment volume passed to the D8 receiver, m³.
    outgoing_m3: f64,
}

/// Transport capacity volume of a cell at elevation `z`:
/// `κ · max(z − z_r, 0)^n`, with `κ` already carrying `Kt_eff A^m dt A_c / dx^n`.
#[inline]
fn capacity_volume(kappa: f64, z: f64, z_receiver: f64, n: f64) -> f64 {
    let relief = z - z_receiver;
    if !(relief > 0.0) || !(kappa > 0.0) || !kappa.is_finite() {
        return 0.0;
    }
    let value = kappa * relief.powf(n);
    if value.is_finite() {
        value
    } else {
        f64::MAX
    }
}

/// Safeguarded Newton iteration for a monotonically increasing root on
/// `[lo, hi]` with `f(lo) ≤ 0 ≤ f(hi)`.
///
/// Newton converges in a handful of steps on the smooth stream-power residual;
/// whenever a Newton step would leave the bracket it falls back to bisection,
/// so the iteration is unconditionally convergent (as in
/// [`implicit_stream_power`]).
#[inline]
fn bracketed_root(
    mut lo: f64,
    mut hi: f64,
    residual: impl Fn(f64) -> f64,
    derivative: impl Fn(f64) -> f64,
    tolerance: f64,
) -> f64 {
    let mut z = 0.5 * (lo + hi);
    for _ in 0..80 {
        let value = residual(z);
        if value.abs() <= tolerance {
            return z;
        }
        if value < 0.0 {
            lo = z;
        } else {
            hi = z;
        }
        if hi - lo <= f64::EPSILON * hi.abs().max(1.0) {
            return z;
        }
        let slope = derivative(z);
        let newton = if slope.is_finite() && slope > 0.0 {
            z - value / slope
        } else {
            f64::NAN
        };
        z = if newton > lo && newton < hi {
            newton
        } else {
            0.5 * (lo + hi)
        };
    }
    z
}

/// Solves the implicit erosion–deposition step of one channel cell.
///
/// This is the depositional-law analogue of [`implicit_stream_power`]; the
/// derivation of the two branches and the mass-conservation identity are in
/// the module documentation. Volumes are m³ per step, `kappa` is the capacity
/// prefactor `Kt_eff A^m dt A_c / dx^n`, `detachment_ratio` is `λ` (1 for the
/// transport-limited law, `K/Kt` for SPACE) and `settle_fraction` is
/// `φ = 1 − exp(−kd dt)`. The result always satisfies
/// `outgoing = incoming + erosion - deposition` and `outgoing ≥ 0`.
#[must_use]
fn solve_channel_step(
    z0: f64,
    z_receiver: f64,
    dx: f64,
    cell_area_m2: f64,
    incoming_m3: f64,
    kappa: f64,
    slope_exponent: f64,
    detachment_ratio: f64,
    settle_fraction: f64,
) -> ChannelStep {
    let incoming = incoming_m3.max(0.0);
    if !(cell_area_m2 > 0.0) || !(dx > 0.0) || !z0.is_finite() || !z_receiver.is_finite() {
        return ChannelStep {
            outgoing_m3: incoming,
            ..ChannelStep::default()
        };
    }
    let n = slope_exponent;
    let capacity0 = capacity_volume(kappa, z0, z_receiver, n);
    let span = (z0 - z_receiver).abs().max(1.0);
    let tolerance = 1e-13 * span;

    if incoming < capacity0 {
        // ---- Supply deficit: detachment, limited by transport (SPACE) ------
        let ratio = detachment_ratio.clamp(0.0, 1.0);
        if ratio <= 0.0 {
            return ChannelStep {
                outgoing_m3: incoming,
                ..ChannelStep::default()
            };
        }
        let residual = |z: f64| {
            z - z0 + ratio * (capacity_volume(kappa, z, z_receiver, n) - incoming) / cell_area_m2
        };
        let derivative = |z: f64| {
            let relief = (z - z_receiver).max(0.0);
            let dc = if relief > 0.0 && kappa > 0.0 {
                kappa * n * relief.powf(n - 1.0)
            } else {
                0.0
            };
            1.0 + ratio * dc / cell_area_m2
        };
        // F(z_receiver) < 0 and F(z0) > 0; if the receiver is above the cell the
        // branch cannot trigger (capacity at zero relief is zero).
        let lo = z_receiver.min(z0);
        let hi = z0;
        let z_new = bracketed_root(lo, hi, residual, derivative, tolerance);
        let z_new = z_new.clamp(lo, hi);
        let eroded = ((z0 - z_new) * cell_area_m2).max(0.0);
        return ChannelStep {
            erosion_m3: eroded,
            outgoing_m3: incoming + eroded,
            ..ChannelStep::default()
        };
    }

    // ---- Supply excess: exponential settling toward capacity ---------------
    let phi = settle_fraction.clamp(0.0, 1.0);
    if phi <= 0.0 {
        return ChannelStep {
            outgoing_m3: incoming,
            ..ChannelStep::default()
        };
    }
    let residual = |z: f64| {
        z - z0 - phi * (incoming - capacity_volume(kappa, z, z_receiver, n)) / cell_area_m2
    };
    let derivative = |z: f64| {
        let relief = (z - z_receiver).max(0.0);
        let dc = if relief > 0.0 && kappa > 0.0 {
            kappa * n * relief.powf(n - 1.0)
        } else {
            0.0
        };
        1.0 + phi * dc / cell_area_m2
    };
    // G(z0) ≤ 0 and G(z0 + φ V_in/A_c) = φ C(·)/A_c ≥ 0, so a root exists.
    let hi = z0 + phi * incoming / cell_area_m2;
    let z_new = bracketed_root(z0, hi, residual, derivative, tolerance).clamp(z0, hi);
    let deposited = ((z_new - z0) * cell_area_m2).clamp(0.0, incoming);
    ChannelStep {
        deposition_m3: deposited,
        outgoing_m3: incoming - deposited,
        ..ChannelStep::default()
    }
}

/// Detachment-limited stream power: every channel cell incises toward the
/// implicit equilibrium and the eroded volume is exported to base level.
fn detachment_limited(
    world: &mut World,
    params: &Params,
    hydro: &Hydro,
    climate: &Climate,
    geology: &Geology,
    dt_yr: f64,
) -> MassBalance {
    let grid = world.grid;
    let planar_area = grid.cell_area_m2();
    let spherical_areas = grid.has_spherical_areas();
    // Physical cell area: constant on planar grids (bit-identical to the
    // historical arithmetic), the latitude band area on spherical grids.
    let cell_area_at = |index: usize| -> f64 {
        if spherical_areas {
            let (x, y) = grid.coords(index);
            grid.cell_area_m2_at(x, y)
        } else {
            planar_area
        }
    };
    let sea = world.sea_level_m;
    let f = &params.fluvial;
    let m = f.area_exponent;
    let n = f.slope_exponent.clamp(0.05, 4.0);
    let root = params.ecology.root_strength;
    let max_incision = f.max_incision_m.max(0.0);
    let mut balance = MassBalance::default();

    // Downstream-to-upstream so the receiver's new elevation is already known
    // when the implicit step is solved: the result is truly implicit.
    for &u in &hydro.order_downstream {
        let i = u as usize;
        let z0 = world.elevation_m.get_index(i);
        if z0 <= sea || hydro.lake_depth_m.get_index(i) > 0.0 || hydro.river_mask.get_index(i) == 0
        {
            continue;
        }
        let Some((receiver, dx, slope)) = receiver_slope(grid, hydro, world, i) else {
            continue;
        };
        if !(slope > 0.0) {
            continue;
        }
        let cell_area = cell_area_at(i);
        let area = hydro.drainage_area_m2.get_index(i).max(cell_area);
        let Some(coefficient) = effective_erodibility(f, root, geology, climate, i, area, slope)
        else {
            continue;
        };
        let z_bed = world.elevation_m.get_index(receiver);
        let z_new = implicit_stream_power(z0, z_bed, coefficient * area.powf(m) * dt_yr, dx, n);
        let incision = (z0 - z_new).clamp(0.0, max_incision);
        if incision <= 0.0 {
            continue;
        }
        world.elevation_m.set_index(i, z0 - incision);
        world.erosion_m.set_index(
            i,
            (f64::from(world.erosion_m.get_index(i)) + incision) as f32,
        );
        let volume = incision * cell_area;
        balance.erosion_m3 += volume;
        balance.export_m3 += volume;
    }
    balance
}

/// Transport-limited (`space = false`) or SPACE (`space = true`) sediment
/// routing with an implicit, mass-conserving upstream sweep and lake, delta and
/// floodplain deposition.
///
/// The sweep is the standard upstream-to-downstream topological order: when a
/// cell is visited every donor has already added its load to `flux`, so `V_in`
/// is exact. Each channel cell then solves its local erosion–deposition
/// equation implicitly (see [`solve_channel_step`] and the module docs), which
/// removes the explicit stability caps of the old single-pass formulation: the
/// solution is bounded by physical quantities (receiver elevation and available
/// sediment) instead of by a `dt`-dependent clamp.
fn depositional(
    world: &mut World,
    params: &Params,
    hydro: &Hydro,
    climate: &Climate,
    geology: &Geology,
    dt_yr: f64,
    space: bool,
) -> MassBalance {
    let grid = world.grid;
    let cells = grid.cell_count();
    let planar_area = grid.cell_area_m2();
    let spherical_areas = grid.has_spherical_areas();
    // Physical cell area: constant on planar grids (bit-identical to the old
    // arithmetic), latitude-band area on spherical grids.
    let cell_area_at = |index: usize| -> f64 {
        if spherical_areas {
            let (x, y) = grid.coords(index);
            grid.cell_area_m2_at(x, y)
        } else {
            planar_area
        }
    };
    let sea = world.sea_level_m;
    let f = &params.fluvial;
    let m = f.area_exponent;
    let slope_exponent = f.slope_exponent.clamp(0.05, 4.0);
    let root = params.ecology.root_strength;
    let transport_efficiency = f.transport_efficiency.clamp(0.0, 1.0);
    // Transport capacity is Kt = K / efficiency; the transport-limited law
    // fills capacity at once (λ = 1), SPACE detaches at the K/Kt ratio.
    let capacity_coefficient = f.kf / transport_efficiency.max(0.05);
    let detachment_ratio = if space { transport_efficiency } else { 1.0 };
    // Exact exponential relaxation of an excess load toward capacity over the
    // step: φ = 1 - exp(-kd dt). No `clamp` is needed; φ ∈ [0, 1) for any dt.
    let settle_rate = f.kd.max(0.0) * dt_yr / 1000.0;
    let settle_fraction = -(-settle_rate).exp_m1();
    let mut balance = MassBalance::default();

    with_scratch(cells, 3, |buffers| {
        let (erosion_buf, rest) = buffers.split_at_mut(1);
        let (deposition_buf, flux_buf) = rest.split_at_mut(1);
        let erosion = &mut erosion_buf[0];
        let deposition = &mut deposition_buf[0];
        let flux = &mut flux_buf[0];

        // Upstream-to-downstream: every donor is processed before its receiver,
        // so the incoming load is complete when a cell is handled.
        for &u in &hydro.order_upstream {
            let i = u as usize;
            let area_i = cell_area_at(i);
            let z0 = world.elevation_m.get_index(i);

            // ---- Sinks: ocean deltas and lakes ---------------------------
            if z0 <= sea {
                let incoming = flux[i];
                if incoming > 0.0 {
                    let accommodation = (sea - z0).max(0.0) * area_i;
                    let deposit = (incoming * OCEAN_DEPOSIT_SHARE).min(accommodation);
                    if deposit > 0.0 {
                        deposition[i] += deposit / area_i;
                        balance.deposition_m3 += deposit;
                    }
                    balance.export_m3 += incoming - deposit;
                }
                continue;
            }
            let lake_depth = hydro.lake_depth_m.get_index(i);
            if lake_depth > 0.0 {
                let incoming = flux[i];
                if incoming > 0.0 {
                    let accommodation = f64::from(lake_depth) * area_i;
                    let deposit = incoming.min(accommodation);
                    if deposit > 0.0 {
                        deposition[i] += deposit / area_i;
                        balance.deposition_m3 += deposit;
                    }
                    let leftover = incoming - deposit;
                    if leftover > 0.0 {
                        if let Some(receiver) = hydro.receiver_of(i) {
                            flux[receiver] += leftover;
                        } else {
                            balance.export_m3 += leftover;
                        }
                    }
                }
                continue;
            }

            // ---- Unchannelled cells pass the load through -----------------
            if hydro.river_mask.get_index(i) == 0 {
                let incoming = flux[i];
                if incoming > 0.0 {
                    if let Some(receiver) = hydro.receiver_of(i) {
                        flux[receiver] += incoming;
                    } else {
                        balance.export_m3 += incoming;
                    }
                }
                continue;
            }

            let Some((receiver, dx, _raw_slope)) = receiver_slope(grid, hydro, world, i) else {
                // Outlet: no downstream capacity; the arriving load is exported.
                let incoming = flux[i];
                if incoming > 0.0 {
                    balance.export_m3 += incoming;
                }
                continue;
            };
            let z_receiver = world.elevation_m.get_index(receiver);
            let slope = ((z0 - z_receiver) / dx).max(0.0);
            let area = hydro.drainage_area_m2.get_index(i).max(area_i);
            let Some(coefficient) =
                effective_erodibility(f, root, geology, climate, i, area, slope)
            else {
                // Threshold suppresses detachment; the load still passes.
                let incoming = flux[i];
                if incoming > 0.0 {
                    flux[receiver] += incoming;
                }
                continue;
            };
            // Rock hardness, vegetation and the incision threshold scale the
            // whole capacity coefficient (`coefficient / f.kf`).
            let capacity_scale = (coefficient / f.kf).clamp(0.0, 1.0);
            let kt_eff = capacity_coefficient * capacity_scale;
            // κ such that C(z) = κ (z - z_r)^n is the capacity volume in m³.
            let kappa = kt_eff * area.powf(m) * dt_yr * area_i / dx.powf(slope_exponent);

            let incoming = flux[i];
            let step = solve_channel_step(
                z0,
                z_receiver,
                dx,
                area_i,
                incoming,
                kappa,
                slope_exponent,
                detachment_ratio,
                settle_fraction,
            );
            if step.erosion_m3 > 0.0 {
                erosion[i] += step.erosion_m3 / area_i;
                balance.erosion_m3 += step.erosion_m3;
            }
            if step.deposition_m3 > 0.0 {
                // Park part of the deposited load on a low floodplain bank.
                let mut remaining = step.deposition_m3;
                if let Some(target) = floodplain_target(grid, hydro, world, i) {
                    let target_area = cell_area_at(target);
                    let share = step.deposition_m3 * FLOODPLAIN_SHARE;
                    if share > 0.0 && target_area > 0.0 {
                        deposition[target] += share / target_area;
                        remaining -= share;
                    }
                }
                deposition[i] += remaining / area_i;
                balance.deposition_m3 += step.deposition_m3;
            }
            if step.outgoing_m3 > 0.0 {
                flux[receiver] += step.outgoing_m3;
            }
        }

        // Apply the buffered surface change in one sequential pass.
        for i in 0..cells {
            let erosion_m = erosion[i];
            let deposition_m = deposition[i];
            if erosion_m == 0.0 && deposition_m == 0.0 {
                continue;
            }
            let z = world.elevation_m.get_index(i);
            world.elevation_m.set_index(i, z - erosion_m + deposition_m);
            if erosion_m > 0.0 {
                world.erosion_m.set_index(
                    i,
                    (f64::from(world.erosion_m.get_index(i)) + erosion_m) as f32,
                );
            }
            if deposition_m > 0.0 {
                world.deposition_m.set_index(
                    i,
                    (f64::from(world.deposition_m.get_index(i)) + deposition_m) as f32,
                );
            }
        }
    });
    let _ = space;
    balance
}

/// Picks a low floodplain cell beside a channel: an unchannelled land cell at
/// most 1.5 m above the channel bed. Deterministic neighbour order.
fn floodplain_target(grid: Grid, hydro: &Hydro, world: &World, channel: usize) -> Option<usize> {
    let sea = world.sea_level_m;
    let z_channel = world.elevation_m.get_index(channel);
    for dir in [1usize, 3, 4, 6] {
        let Some(other) = grid.neighbor_index(channel, dir) else {
            continue;
        };
        if hydro.river_mask.get_index(other) != 0 || hydro.lake_depth_m.get_index(other) > 0.0 {
            continue;
        }
        let z = world.elevation_m.get_index(other);
        if z > sea && z > z_channel && (z - z_channel) < 1.5 {
            return Some(other);
        }
    }
    None
}

/// Hydraulic geometry of a channel: width from discharge (Leopold–Maddock
/// style, `w ∝ Q^0.5`), modulated by the channel-width parameter.
#[must_use]
fn channel_width_m(f: &FluvialParams, discharge_m3_yr: f64) -> f64 {
    let q = (discharge_m3_yr / SECONDS_PER_YEAR).max(1e-6);
    (f.channel_width_coeff.max(0.1) * 20.0 * q.sqrt()).clamp(1.0, 5000.0)
}

/// Cheap stream-power proxy for bank undercutting: `sqrt(Q S / w)`, scaled by
/// Manning roughness (rougher beds do less lateral work). Uses one `sqrt`
/// instead of the transcendental functions a full hydraulic geometry would
/// need, which matters because this runs on every channel cell every step.
#[must_use]
fn bank_stream_power(f: &FluvialParams, discharge_m3_yr: f64, width_m: f64, slope: f64) -> f64 {
    let manning = f.manning_roughness.clamp(0.005, 0.5);
    let roughness = (0.05 / manning).sqrt().clamp(0.3, 4.0);
    let power = discharge_m3_yr * slope.max(0.0) / width_m.max(1.0);
    if power <= 0.0 {
        return 0.0;
    }
    power.sqrt() * roughness
}

/// Lateral channel migration: channels undercut their banks and the failed
/// material is partly redeposited on the channel bed (lateral accretion) and
/// partly exported downstream. Mass-conserving by construction.
fn bank_erosion(
    world: &mut World,
    params: &Params,
    hydro: &Hydro,
    dt_yr: f64,
    balance: &mut MassBalance,
) {
    let grid = world.grid;
    let planar_area = grid.cell_area_m2();
    let spherical_areas = grid.has_spherical_areas();
    let cell_area_at = |index: usize| -> f64 {
        if spherical_areas {
            let (x, y) = grid.coords(index);
            grid.cell_area_m2_at(x, y)
        } else {
            planar_area
        }
    };
    let sea = world.sea_level_m;
    let f = &params.fluvial;
    let year_factor = (dt_yr / 1000.0).clamp(0.0, 50.0);

    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            if hydro.river_mask.get_index(i) == 0 || hydro.lake_depth_m.get_index(i) > 0.0 {
                continue;
            }
            let z_channel = world.elevation_m.get_index(i);
            if z_channel <= sea {
                continue;
            }
            let Some((_receiver, _dx, slope)) = receiver_slope(grid, hydro, world, i) else {
                continue;
            };
            let discharge = hydro.discharge_m3_yr.get_index(i);
            let width = channel_width_m(f, discharge);
            let stream_power = bank_stream_power(f, discharge, width, slope.max(0.0));
            let depth =
                (f.bank_erosion * 0.05 * year_factor * (1.0 + 0.05 * stream_power.min(20.0)))
                    .clamp(0.0, 1.0);
            if depth <= 0.0 {
                continue;
            }
            for dir in [1usize, 3, 4, 6] {
                let Some(j) = grid.neighbor_index(i, dir) else {
                    continue;
                };
                if hydro.river_mask.get_index(j) != 0 || hydro.lake_depth_m.get_index(j) > 0.0 {
                    continue;
                }
                let z_bank = world.elevation_m.get_index(j);
                if z_bank <= sea {
                    continue;
                }
                let drop = z_bank - z_channel;
                if !(drop > 0.0) || drop > 500.0 {
                    continue;
                }
                let eroded = depth.min(drop * 0.5);
                if eroded <= 1e-9 {
                    continue;
                }
                world.elevation_m.set_index(j, z_bank - eroded);
                world
                    .erosion_m
                    .set_index(j, (f64::from(world.erosion_m.get_index(j)) + eroded) as f32);
                let volume = eroded * cell_area_at(j);
                balance.erosion_m3 += volume;
                let deposited = volume * 0.6;
                let channel_area = cell_area_at(i);
                let channel_now = world.elevation_m.get_index(i);
                world
                    .elevation_m
                    .set_index(i, channel_now + deposited / channel_area);
                world.deposition_m.set_index(
                    i,
                    (f64::from(world.deposition_m.get_index(i)) + deposited / channel_area) as f32,
                );
                balance.deposition_m3 += deposited;
                balance.export_m3 += volume - deposited;
            }
        }
    }
}

/// Channel long profile along a downstream path, useful for statistics and
/// validation tests (slope-area analysis).
#[must_use]
pub fn long_profile(elevation: &Field<f64>, hydro: &Hydro, start: usize) -> Vec<(f64, f64, f64)> {
    let mut out = Vec::new();
    let mut distance = 0.0;
    let mut current = Some(start);
    let grid = elevation.grid();
    for _ in 0..grid.cell_count() {
        let Some(i) = current else { break };
        let area = hydro.drainage_area_m2.get_index(i);
        out.push((distance, elevation.get_index(i), area));
        match hydro.receiver_of(i) {
            Some(r) => {
                let dir = hydro.flow_dir.get_index(i) as usize;
                distance += if dir < 8 {
                    grid.step_distance_m(dir)
                } else {
                    grid.cell_size_m()
                };
                current = Some(r);
            }
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::{compute, ClimateInputs};
    use crate::geology::Geology;
    use crate::hydrology::route;
    use crate::params::Params;
    use crate::plates::PlateMosaic;
    use crate::world::World;

    fn setup() -> (World, Params, Hydro, Climate, Geology) {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        p.fluvial.kf = 1e-4;
        let grid = p.world.grid().unwrap();
        let mut world = World::new(&p.world).unwrap();
        // Tilted ramp towards the east.
        world.elevation_m.map_in_place(|i, _| {
            let x = i % 32;
            800.0 - x as f64 * 20.0
        });
        let mosaic = PlateMosaic::build(&p, grid, 1);
        let geology = Geology::build(&p, &world, &mosaic, 1);
        let climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 1000.0,
        });
        let hydro = route(&world, &p, Some(&climate));
        (world, p, hydro, climate, geology)
    }

    #[test]
    fn every_law_erodes_and_closes_the_ledger() {
        for law in [0.0, 1.0, 2.0] {
            let (mut world, mut p, hydro, climate, geology) = setup();
            p.fluvial.law = law;
            let before = world.elevation_m.clone();
            let balance = apply(&mut world, &p, &hydro, &climate, &geology, 10_000.0);
            assert!(balance.erosion_m3 > 0.0, "law {law} eroded nothing");
            assert!(balance.total_volume_m3() > 0.0, "law {law} has no flux");
            let scale = balance.erosion_m3.max(1.0);
            assert!(
                balance.closure_error_m3().abs() / scale < 1e-6,
                "law {law} ledger: {balance:?}"
            );
            let lowered = before
                .as_slice()
                .iter()
                .zip(world.elevation_m.as_slice())
                .filter(|(a, b)| **b < **a - 1e-9)
                .count();
            assert!(lowered > 0, "law {law} lowered nothing");
        }
    }

    #[test]
    fn implicit_solver_matches_the_discrete_equation() {
        for n in [0.5, 0.75, 1.0, 1.5, 2.0] {
            for (z0, z_bed, coefficient, dx) in [
                (1000.0, 800.0, 2.0, 10_000.0),
                (500.0, 100.0, 1e6, 10_000.0),
                (50.0, 49.0, 1e-3, 500.0),
            ] {
                let z = implicit_stream_power(z0, z_bed, coefficient, dx, n);
                assert!(z >= z_bed && z <= z0, "n={n} z={z}");
                let c = coefficient / dx.powf(n);
                let residual = z - z0 + c * (z - z_bed).powf(n);
                assert!(
                    residual.abs() < 1e-7 * (z0 - z_bed).max(1.0),
                    "n={n} residual {residual}"
                );
            }
        }
        // n = 1 has the closed-form solution.
        let z = implicit_stream_power(1000.0, 500.0, 3.0, 100.0, 1.0);
        let c = 3.0 / 100.0;
        assert!((z - (1000.0 + c * 500.0) / (1.0 + c)).abs() < 1e-12);
    }

    /// Builds a one-cell-per-row channel on a raster with rock uplift and a
    /// pinned base level, runs it at the analytical steady state and returns
    /// the measured `(log A, log S)` samples of the interior channel.
    fn steady_state_slope_area(n: f64, m: f64) -> Vec<(f64, f64)> {
        let mut p = Params::default();
        p.world.width = 64;
        p.world.height = 16;
        p.world.cell_km = 10.0;
        p.world.boundary = crate::grid::BoundaryKind::Rectangular;
        p.world.sea_level_m = -1000.0;
        p.hydrology.channel_init_cells = 1;
        p.fluvial.kf = 1e-5;
        p.fluvial.area_exponent = m;
        p.fluvial.slope_exponent = n;
        p.fluvial.incision_threshold = 0.0;
        p.fluvial.max_incision_m = 5000.0;
        p.fluvial.bank_erosion = 0.0;
        p.ecology.root_strength = 0.0;
        let grid = p.world.grid().unwrap();
        let mut world = World::new(&p.world).unwrap();
        let cell_area = grid.cell_area_m2();
        let uplift = 1e-4f64; // m/yr
        let dt = 1000.0;
        let width = grid.width();
        let base_level = 0.0;
        // Analytical steady-state profile, built downstream-to-upstream. The
        // open west boundary is a high wall so every interior cell drains east.
        let mut z = vec![0.0; grid.cell_count()];
        for y in 0..grid.height() {
            z[grid.index(width - 1, y)] = base_level;
            for x in (1..width - 1).rev() {
                let area = x as f64 * cell_area;
                let slope = (uplift / (p.fluvial.kf * area.powf(m))).powf(1.0 / n);
                let downstream = z[grid.index(x + 1, y)];
                z[grid.index(x, y)] = downstream + slope * grid.cell_size_m();
            }
            z[grid.index(0, y)] = z[grid.index(1, y)] + 500.0;
        }
        for (i, &value) in z.iter().enumerate() {
            world.elevation_m.set_index(i, value);
        }
        let mosaic = PlateMosaic::build(&p, grid, 3);
        let mut geology = Geology::build(&p, &world, &mosaic, 3);
        geology
            .erodibility
            .as_mut_slice()
            .iter_mut()
            .for_each(|v| *v = 1.0);
        let mut climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 0.0,
        });
        climate
            .vegetation
            .as_mut_slice()
            .iter_mut()
            .for_each(|v| *v = 0.0);
        let mut hydro = route(&world, &p, Some(&climate));
        for _ in 0..40 {
            // Rock uplift, then fluvial incision, then pin base level.
            world
                .elevation_m
                .map_in_place(|_, value| value + uplift * dt);
            apply(&mut world, &p, &hydro, &climate, &geology, dt);
            for y in 0..grid.height() {
                world
                    .elevation_m
                    .set_index(grid.index(width - 1, y), base_level);
            }
            hydro = route(&world, &p, Some(&climate));
        }
        let mut samples = Vec::new();
        for y in [4usize, 8, 12] {
            for x in 2..width - 2 {
                let i = grid.index(x, y);
                let area = hydro.drainage_area_m2.get_index(i);
                let z0 = world.elevation_m.get_index(i);
                let zr = world.elevation_m.get_index(grid.index(x + 1, y));
                let slope = (z0 - zr) / grid.cell_size_m();
                if slope > 0.0 && area > 0.0 {
                    samples.push((area.ln(), slope.ln()));
                }
            }
        }
        samples
    }

    #[test]
    fn slope_area_steady_state_recovers_the_m_over_n_exponent() {
        for (n, m) in [(1.0f64, 0.5f64), (1.5, 0.5), (2.0, 0.4)] {
            let samples = steady_state_slope_area(n, m);
            assert!(samples.len() > 20, "not enough channel samples");
            // Least-squares fit log S = a + b log A.
            let count = samples.len() as f64;
            let mean_x = samples.iter().map(|(x, _)| *x).sum::<f64>() / count;
            let mean_y = samples.iter().map(|(_, y)| *y).sum::<f64>() / count;
            let mut sxx = 0.0;
            let mut sxy = 0.0;
            for (x, y) in &samples {
                sxx += (x - mean_x) * (x - mean_x);
                sxy += (x - mean_x) * (y - mean_y);
            }
            let exponent = sxy / sxx;
            let expected = -m / n;
            assert!(
                (exponent - expected).abs() < 0.02,
                "n={n} m={m}: fitted {exponent}, expected {expected}"
            );
            // And the intercept must match (U/K)^(1/n) within a few percent.
            let intercept = mean_y - exponent * mean_x;
            let want = (1e-4f64 / 1e-5).powf(1.0 / n).ln();
            assert!(
                (intercept - want).abs() < 0.15,
                "n={n} m={m}: intercept {intercept} vs {want}"
            );
        }
    }

    #[test]
    fn slope_area_steady_state_is_a_fixed_point() {
        // Starting at the analytical steady state, the coupled uplift + fluvial
        // step must not move the channel appreciably.
        let (mut world, p, hydro, climate, geology) = {
            let mut p = Params::default();
            p.world.width = 48;
            p.world.height = 16;
            p.world.cell_km = 10.0;
            p.world.boundary = crate::grid::BoundaryKind::Rectangular;
            p.world.sea_level_m = -1000.0;
            p.hydrology.channel_init_cells = 1;
            p.fluvial.kf = 1e-4;
            p.fluvial.area_exponent = 0.4;
            p.fluvial.slope_exponent = 1.5;
            p.fluvial.bank_erosion = 0.0;
            p.ecology.root_strength = 0.0;
            let grid = p.world.grid().unwrap();
            let mut world = World::new(&p.world).unwrap();
            let cell_area = grid.cell_area_m2();
            let uplift = 1e-4;
            let base_level = 0.0;
            for y in 0..grid.height() {
                world
                    .elevation_m
                    .set_index(grid.index(grid.width() - 1, y), base_level);
                for x in (1..grid.width() - 1).rev() {
                    let area = x as f64 * cell_area;
                    let slope = (uplift / (p.fluvial.kf * area.powf(0.4))).powf(1.0 / 1.5);
                    let downstream = world.elevation_m.get_index(grid.index(x + 1, y));
                    world
                        .elevation_m
                        .set_index(grid.index(x, y), downstream + slope * grid.cell_size_m());
                }
                world.elevation_m.set_index(
                    grid.index(0, y),
                    world.elevation_m.get_index(grid.index(1, y)) + 500.0,
                );
            }
            let mosaic = PlateMosaic::build(&p, grid, 4);
            let mut geology = Geology::build(&p, &world, &mosaic, 4);
            geology
                .erodibility
                .as_mut_slice()
                .iter_mut()
                .for_each(|v| *v = 1.0);
            let mut climate = compute(&ClimateInputs {
                world: &world,
                params: &p,
                previous: None,
                dt_yr: 0.0,
            });
            climate
                .vegetation
                .as_mut_slice()
                .iter_mut()
                .for_each(|v| *v = 0.0);
            let hydro = route(&world, &p, Some(&climate));
            (world, p, hydro, climate, geology)
        };
        let dt = 1000.0;
        let uplift = 1e-4;
        let before = world.elevation_m.clone();
        world.elevation_m.map_in_place(|_, z| z + uplift * dt);
        apply(&mut world, &p, &hydro, &climate, &geology, dt);
        let mut worst = 0.0f64;
        for i in 0..world.cell_count() {
            let delta = (world.elevation_m.get_index(i) - before.get_index(i)).abs();
            worst = worst.max(delta);
        }
        assert!(worst < 1.5, "steady state drifted by {worst} m");
    }

    #[test]
    fn depositional_laws_deposit_sediment_in_a_lake() {
        // Ring wall around a bowl: the bowl is a lake in the hydrology.
        for law in [1.0, 2.0] {
            let mut p = Params::default();
            p.world.width = 32;
            p.world.height = 32;
            p.world.cell_km = 10.0;
            p.fluvial.law = law;
            p.fluvial.kf = 1e-3;
            let grid = p.world.grid().unwrap();
            let mut world = World::new(&p.world).unwrap();
            world.elevation_m.map_in_place(|i, _| {
                let (x, y) = grid.coords(i);
                let dx = x as f64 - 16.0;
                let dy = y as f64 - 16.0;
                let d = (dx * dx + dy * dy).sqrt();
                if d < 4.0 {
                    200.0
                } else if d < 7.0 {
                    900.0
                } else {
                    -300.0
                }
            });
            let mosaic = PlateMosaic::build(&p, grid, 5);
            let mut geology = Geology::build(&p, &world, &mosaic, 5);
            geology
                .erodibility
                .as_mut_slice()
                .iter_mut()
                .for_each(|v| *v = 1.0);
            let mut climate = compute(&ClimateInputs {
                world: &world,
                params: &p,
                previous: None,
                dt_yr: 0.0,
            });
            climate
                .vegetation
                .as_mut_slice()
                .iter_mut()
                .for_each(|v| *v = 0.0);
            let hydro = route(&world, &p, Some(&climate));
            assert!(hydro.lake_volume_m3() > 0.0, "test world has no lake");
            let lake_floor = grid.index(16, 16);
            let before = world.elevation_m.get_index(lake_floor);
            let balance = apply(&mut world, &p, &hydro, &climate, &geology, 10_000.0);
            assert!(
                balance.closure_error_m3().abs() / balance.erosion_m3.max(1.0) < 1e-6,
                "lake law {law}: {balance:?}"
            );
            let after = world.elevation_m.get_index(lake_floor);
            assert!(
                after > before - 1e-9,
                "lake floor was not aggraded: {before} -> {after}"
            );
        }
    }

    #[test]
    fn implicit_solver_is_stable_for_huge_steps_for_every_law() {
        for law in [0.0, 1.0, 2.0] {
            let (mut world, mut p, hydro, climate, geology) = setup();
            p.fluvial.law = law;
            let _ = apply(&mut world, &p, &hydro, &climate, &geology, 1e7);
            assert_eq!(
                world.elevation_m.non_finite_count(),
                0,
                "law {law} produced non-finite elevations"
            );
        }
        // Detachment-limited incision cannot invert a channel below its
        // receiver, even with a geological time step.
        let (mut world, p, hydro, climate, geology) = setup();
        apply(&mut world, &p, &hydro, &climate, &geology, 1e7);
        for i in 0..world.grid.cell_count() {
            if let Some(r) = hydro.receiver_of(i) {
                assert!(world.elevation_m.get_index(i) >= world.elevation_m.get_index(r) - 1e-6);
            }
        }
    }

    #[test]
    fn each_law_is_deterministic() {
        for law in [0.0, 1.0, 2.0] {
            let make = || {
                let (mut world, mut p, hydro, climate, geology) = setup();
                p.fluvial.law = law;
                apply(&mut world, &p, &hydro, &climate, &geology, 5000.0);
                world.elevation_m.as_slice().to_vec()
            };
            assert_eq!(make(), make(), "law {law} is not deterministic");
        }
    }

    #[test]
    fn bank_erosion_cuts_banks_and_conserves_mass() {
        let (mut world, mut p, hydro, climate, geology) = setup();
        p.fluvial.law = 0.0;
        p.fluvial.bank_erosion = 0.8;
        let before = world.elevation_m.clone();
        let balance = apply(&mut world, &p, &hydro, &climate, &geology, 20_000.0);
        assert!(balance.erosion_m3 > 0.0);
        assert!(
            balance.closure_error_m3().abs() / balance.erosion_m3.max(1.0) < 1e-6,
            "bank erosion ledger: {balance:?}"
        );
        let channel_bank_eroded = (0..world.cell_count()).any(|i| {
            hydro.river_mask.get_index(i) == 0
                && world.elevation_m.get_index(i) < before.get_index(i) - 1e-9
        });
        assert!(channel_bank_eroded, "no hillslope cell was undercut");
    }

    /// Builds a one-cell-per-row channel on a raster with rock uplift and a
    /// pinned base level, initialised on the analytical transport-limited
    /// steady state `S = (U/Kt)^(1/n) A_c^(-m/n) a^((1-m)/n)` where
    /// `a = A / A_cell` is the drainage area in cell-equivalents. Runs the law
    /// and returns the measured `(ln a, ln S)` samples of the interior channel.
    fn transport_limited_slope_area(n: f64, m: f64) -> Vec<(f64, f64)> {
        let mut p = Params::default();
        p.world.width = 64;
        p.world.height = 16;
        p.world.cell_km = 10.0;
        p.world.boundary = crate::grid::BoundaryKind::Rectangular;
        p.world.sea_level_m = -1000.0;
        p.hydrology.channel_init_cells = 1;
        p.fluvial.law = 1.0;
        p.fluvial.kf = 1e-5;
        p.fluvial.kd = 1e-5; // negligible settling: pure capacity routing
        p.fluvial.area_exponent = m;
        p.fluvial.slope_exponent = n;
        p.fluvial.incision_threshold = 0.0;
        p.fluvial.bank_erosion = 0.0;
        p.ecology.root_strength = 0.0;
        let grid = p.world.grid().unwrap();
        let mut world = World::new(&p.world).unwrap();
        let cell_area = grid.cell_area_m2();
        let uplift = 1e-4f64; // m/yr
        let dt = 1000.0;
        let width = grid.width();
        let base_level = 0.0;
        let kt = p.fluvial.kf / p.fluvial.transport_efficiency.max(0.05);
        // Analytical profile, built downstream-to-upstream. Cell x drains
        // cells 0..=x, so its area in cell-equivalents is a = x + 1.
        for y in 0..grid.height() {
            world
                .elevation_m
                .set_index(grid.index(width - 1, y), base_level);
            for x in (1..width - 1).rev() {
                let a = x as f64; // row cell 0 is an outlet; cell x drains 1..=x
                let slope = ((uplift / kt) * cell_area.powf(-m) * a.powf(1.0 - m)).powf(1.0 / n);
                let downstream = world.elevation_m.get_index(grid.index(x + 1, y));
                world
                    .elevation_m
                    .set_index(grid.index(x, y), downstream + slope * grid.cell_size_m());
            }
            world.elevation_m.set_index(
                grid.index(0, y),
                world.elevation_m.get_index(grid.index(1, y)) + 500.0,
            );
        }
        let mosaic = PlateMosaic::build(&p, grid, 3);
        let mut geology = Geology::build(&p, &world, &mosaic, 3);
        geology
            .erodibility
            .as_mut_slice()
            .iter_mut()
            .for_each(|v| *v = 1.0);
        let mut climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 0.0,
        });
        climate
            .vegetation
            .as_mut_slice()
            .iter_mut()
            .for_each(|v| *v = 0.0);
        let mut hydro = route(&world, &p, Some(&climate));
        for _ in 0..30 {
            world
                .elevation_m
                .map_in_place(|_, value| value + uplift * dt);
            apply(&mut world, &p, &hydro, &climate, &geology, dt);
            for y in 0..grid.height() {
                world
                    .elevation_m
                    .set_index(grid.index(width - 1, y), base_level);
            }
            hydro = route(&world, &p, Some(&climate));
        }
        let mut samples = Vec::new();
        for y in [4usize, 8, 12] {
            for x in 2..width - 2 {
                let i = grid.index(x, y);
                let area = hydro.drainage_area_m2.get_index(i);
                let z0 = world.elevation_m.get_index(i);
                let zr = world.elevation_m.get_index(grid.index(x + 1, y));
                let slope = (z0 - zr) / grid.cell_size_m();
                if slope > 0.0 && area > 0.0 {
                    samples.push(((area / cell_area).ln(), slope.ln()));
                }
            }
        }
        samples
    }

    #[test]
    fn transport_limited_steady_state_matches_the_analytic_slope_area_law() {
        for (n, m) in [(1.0f64, 0.5f64), (1.5, 0.5), (2.0, 0.4)] {
            let samples = transport_limited_slope_area(n, m);
            assert!(samples.len() > 20, "not enough channel samples");
            let count = samples.len() as f64;
            let mean_x = samples.iter().map(|(x, _)| *x).sum::<f64>() / count;
            let mean_y = samples.iter().map(|(_, y)| *y).sum::<f64>() / count;
            let mut sxx = 0.0;
            let mut sxy = 0.0;
            for (x, y) in &samples {
                sxx += (x - mean_x) * (x - mean_x);
                sxy += (x - mean_x) * (y - mean_y);
            }
            let exponent = sxy / sxx;
            let expected = (1.0 - m) / n;
            assert!(
                (exponent - expected).abs() < 0.02,
                "n={n} m={m}: fitted {exponent}, expected {expected}"
            );
            let intercept = mean_y - exponent * mean_x;
            // Kt = K / transport_efficiency, A_c = (10 km)².
            let kt = 1e-5 / Params::default().fluvial.transport_efficiency.max(0.05);
            let cell_area: f64 = 1.0e8;
            // ln S = (1/n) ln(U/Kt) - (m/n) ln A_c + ((1-m)/n) ln a.
            let want = (1e-4f64 / kt).ln() / n - (m / n) * cell_area.ln();
            println!(
                "transport-limited n={n} m={m}: exponent {exponent:.4} (want {expected:.4}), \
                 intercept {intercept:.4} (want {want:.4})"
            );
            assert!(
                (intercept - want).abs() < 0.15,
                "n={n} m={m}: intercept {intercept} vs {want}"
            );
        }
    }

    #[test]
    fn depositional_step_solver_conserves_volume_and_stays_bracketed() {
        let cell_area = 1.0e8;
        let dx = 10_000.0;
        let z_receiver = 200.0;
        // Capacity prefactor for a modest channel.
        let kappa = 5.0e6;
        let n = 1.0;
        // -- Detachment branch: incoming well below capacity at z0.
        let z0 = 300.0;
        let capacity0 = capacity_volume(kappa, z0, z_receiver, n);
        let incoming = 0.1 * capacity0;
        let step = solve_channel_step(z0, z_receiver, dx, cell_area, incoming, kappa, n, 1.0, 0.5);
        assert!(step.deposition_m3 == 0.0);
        assert!(step.erosion_m3 > 0.0);
        assert!(step.outgoing_m3 - (incoming + step.erosion_m3) < 1e-9 * step.outgoing_m3.max(1.0));
        let z_new = z0 - step.erosion_m3 / cell_area;
        assert!(z_new >= z_receiver && z_new < z0, "z_new {z_new}");
        // At the solution the outgoing load equals the capacity (λ = 1).
        let capacity_new = capacity_volume(kappa, z_new, z_receiver, n);
        assert!((step.outgoing_m3 - capacity_new).abs() <= 1e-6 * capacity_new.max(1.0));
        // -- Deposition branch: incoming well above capacity.
        let incoming = 4.0 * capacity0;
        let step = solve_channel_step(z0, z_receiver, dx, cell_area, incoming, kappa, n, 1.0, 0.5);
        assert!(step.erosion_m3 == 0.0);
        assert!(step.deposition_m3 > 0.0 && step.deposition_m3 <= incoming);
        assert!((step.outgoing_m3 + step.deposition_m3 - incoming).abs() <= 1e-9 * incoming);
        assert!(step.outgoing_m3 >= 0.0);
        // φ = 1/2: outgoing = (incoming + capacity(z_new)) / 2.
        let z_new = z0 + step.deposition_m3 / cell_area;
        let capacity_new = capacity_volume(kappa, z_new, z_receiver, n);
        let want = 0.5 * (incoming + capacity_new);
        assert!((step.outgoing_m3 - want).abs() <= 1e-6 * incoming);
        // An enormous step still deposits at most the incoming load and never
        // produces a negative outgoing flux.
        let step = solve_channel_step(z0, z_receiver, dx, cell_area, 1e18, kappa, n, 1.0, 1.0);
        assert!((0.0..=1e18).contains(&step.deposition_m3));
        assert!(step.outgoing_m3 >= 0.0);
        assert!(step.outgoing_m3 + step.deposition_m3 <= 1e18 * (1.0 + 1e-12));
    }

    #[test]
    fn depositional_ledger_matches_the_surface_change_on_spherical_worlds() {
        for law in [1.0, 2.0] {
            let mut p = Params::default();
            p.world.width = 32;
            p.world.height = 24;
            p.world.cell_km = 10.0;
            p.world.boundary = crate::grid::BoundaryKind::Spherical;
            p.world.planet_radius_km = 6371.0;
            p.world.sea_level_m = -1000.0;
            p.hydrology.channel_init_cells = 1;
            p.fluvial.law = law;
            p.fluvial.kf = 1e-3;
            p.ecology.root_strength = 0.0;
            let grid = p.world.grid().unwrap();
            assert!(grid.has_spherical_areas());
            let mut world = World::new(&p.world).unwrap();
            world.elevation_m.map_in_place(|i, _| {
                let (x, y) = grid.coords(i);
                let lat = (y as f64 + 0.5) * std::f64::consts::PI / grid.height() as f64
                    - std::f64::consts::FRAC_PI_2;
                // Relief that differs strongly between equator and poles.
                400.0 + 600.0 * lat.cos() - x as f64 * 8.0
            });
            let mosaic = PlateMosaic::build(&p, grid, 7);
            let mut geology = Geology::build(&p, &world, &mosaic, 7);
            geology
                .erodibility
                .as_mut_slice()
                .iter_mut()
                .for_each(|v| *v = 1.0);
            let mut climate = compute(&ClimateInputs {
                world: &world,
                params: &p,
                previous: None,
                dt_yr: 0.0,
            });
            climate
                .vegetation
                .as_mut_slice()
                .iter_mut()
                .for_each(|v| *v = 0.0);
            let hydro = route(&world, &p, Some(&climate));
            let before: Vec<f64> = world.elevation_m.as_slice().to_vec();
            let balance = apply(&mut world, &p, &hydro, &climate, &geology, 10_000.0);
            // Relative ledger closure.
            let scale = balance.erosion_m3.max(1.0);
            assert!(
                balance.closure_error_m3().abs() / scale < 1e-6,
                "law {law}: ledger {balance:?}"
            );
            // The actual surface-volume change, measured with the physical cell
            // areas, must equal deposition - erosion: this is what proves the
            // ledger used per-cell areas consistently.
            let mut delta_volume = 0.0;
            for y in 0..grid.height() {
                for x in 0..grid.width() {
                    let i = grid.index(x, y);
                    delta_volume +=
                        (world.elevation_m.get_index(i) - before[i]) * grid.cell_area_m2_at(x, y);
                }
            }
            let net = balance.deposition_m3 - balance.erosion_m3;
            let scale = net.abs().max(balance.erosion_m3).max(1.0);
            assert!(
                (delta_volume - net).abs() / scale < 1e-9,
                "law {law}: surface volume {delta_volume} vs ledger net {net}"
            );
            assert!(balance.erosion_m3 > 0.0 || balance.deposition_m3 > 0.0);
        }
    }

    #[test]
    fn long_profile_terminates() {
        let (world, _p, hydro, _, _) = setup();
        let start = world.grid.index(0, 0);
        let profile = long_profile(&world.elevation_m, &hydro, start);
        assert!(!profile.is_empty());
        assert!(profile.len() <= world.grid.cell_count());
    }
}
