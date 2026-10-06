//! Wind erosion and aeolian deposition.
//!
//! Wind work is threshold-limited: below the saltation threshold (`u* > u*t`)
//! nothing moves regardless of fetch, and above it the transport rate grows
//! with the cube of the friction velocity (Bagnold). Vegetation raises the
//! threshold; a shadow zone behind a dune crest suppresses deposition on the
//! lee side.
//!
//! Like every process here, this one returns a [`MassBalance`] so the driver
//! can prove conservation.

use crate::erosion::{repair_field, MassBalance};
use crate::grid::ScalarField;
use crate::linalg::clamp;

/// Wind process parameters, matching spec §3d ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct WindParams {
    /// 0.0 … 50.0 m/s: surface wind speed.
    pub wind_speed_ms: f64,
    /// 0.0 … 360.0°: bearing the wind comes from, degrees from +x.
    pub wind_dir_deg: f64,
    /// 0.0 … 1.0: erosional efficiency once saltation starts.
    pub erosivity: f64,
    /// 0.0 … 1.0: fraction of suspension load; rest saltates locally.
    pub sand_flux: f64,
    /// 0.0 … 1.0: threshold increase from vegetation cover.
    pub vegetation_shelter: f64,
    /// Whether dunes may form (controls crest avalanching, not just drift).
    pub dunes: bool,
    /// Threshold friction velocity in m/s below which nothing moves.
    pub threshold_ms: f64,
}

impl Default for WindParams {
    fn default() -> Self {
        Self {
            wind_speed_ms: 8.0,
            wind_dir_deg: 270.0,
            erosivity: 0.4,
            sand_flux: 0.3,
            vegetation_shelter: 0.3,
            dunes: true,
            threshold_ms: 5.0,
        }
    }
}

/// Applies wind erosion and deposition for `dt_yr` years.
///
/// Material is picked up where the wind is above threshold and set down where
/// the surface falls into the lee shadow or the wind slows. Deposition that
/// cannot settle on the grid (off the downwind edge) is exported.
pub fn apply(
    surface: &mut ScalarField,
    params: &WindParams,
    vegetation: &ScalarField,
    cell_size_m: f64,
    dt_yr: f64,
) -> MassBalance {
    apply_with_order(surface, params, vegetation, cell_size_m, dt_yr, None)
}

/// Applies one interval, reusing the caller's downhill order when given.
pub fn apply_with_order(
    surface: &mut ScalarField,
    params: &WindParams,
    vegetation: &ScalarField,
    cell_size_m: f64,
    dt_yr: f64,
    order: Option<&[usize]>,
) -> MassBalance {
    let b = *surface.boundary();
    let dt = dt_yr.max(0.0);
    let mut balance = MassBalance::default();
    if dt == 0.0 {
        return balance;
    }
    let m2 = cell_size_m * cell_size_m;

    let wind_rad = params.wind_dir_deg.to_radians();
    let (wx, wy) = (wind_rad.cos(), wind_rad.sin());
    let threshold =
        params.threshold_ms * (1.0 + clamp(params.vegetation_shelter, 0.0, 1.0));
    let excess = (params.wind_speed_ms - threshold).max(0.0);
    if excess <= 0.0 {
        return balance; // below threshold: nothing moves
    }

    // Bagnold transport capacity ∝ u*³, in m/yr per metre of width.
    let capacity_ms = params.erosivity * excess.powi(3) * 1e-4;
    let capacity_myr = capacity_ms * 365.25 * 24.0 * 3600.0;

    let fetch = (8.0 * 1000.0 / cell_size_m.max(1.0)).ceil() as usize;
    let fetch = fetch.clamp(2, 32);

    // Wind transport runs upwind-to-downwind, not downhill; the shared order
    // is still a valid visitation because every cell is visited exactly once
    // and deposition targets are resolved geometrically below.
    let owned;
    let visit: &[usize] = match order {
        Some(o) => o,
        None => {
            owned = crate::hydrology::accumulation_order(surface);
            &owned
        }
    };
    let mut suspended = vec![0.0f64; b.cell_count()];

    for &idx in visit {
        let (x, y) = b.coords(idx);
        // Consume any suspension that arrived here before this visit; the
        // settle-in-place pass below must see only what is still in flight
        // afterwards, or it double-counts (same bug as fluvial's).
        let incoming = suspended[idx].max(0.0);
        suspended[idx] = 0.0;
        if incoming > 0.0 {
            surface.set_index(idx, surface.get_index(idx) + incoming);
            balance.deposited_m3 += incoming * m2;
        }
        let cover = vegetation.get_index(idx).clamp(0.0, 1.0);
        let local_excess = (params.wind_speed_ms * (1.0 - 0.5 * cover) - threshold).max(0.0);
        if local_excess <= 0.0 {
            continue;
        }

        // Lee shadow: upwind higher ground suppresses both pickup and settling.
        let shade = upwind_relief_ratio(surface, x, y, wx, wy, fetch);
        let pick = capacity_myr
            * dt
            * params.erosivity
            * (1.0 - shade)
            * (1.0 - 0.6 * cover);
        let z = surface.get_index(idx);
        let take = pick.min(z.max(0.0) * 0.1 + 0.01).max(0.0);
        if take <= 0.0 {
            continue;
        }
        surface.set_index(idx, z - take);
        balance.removed_m3 += take * m2;

        // A fraction suspends and travels downwind; the rest saltates and
        // settles within a few cells.
        let suspended_share = clamp(params.sand_flux, 0.0, 1.0);
        let travel = take * suspended_share;
        let local = take - travel;

        // Saltating sand settles just downwind.
        let (dx, dy) = (
            (wx * 2.0).round() as i64,
            (wy * 2.0).round() as i64,
        );
        let nidx = b.index_wrapped(x as i64 + dx, y as i64 + dy);
        surface.set_index(nidx, surface.get_index(nidx) + local);
        balance.deposited_m3 += local * m2;
        suspended[nidx] += travel;

        // Shadowed cells trap what passes over them. Clamp to the travel
        // actually sent: trap is computed from the same `travel` already added
        // to suspended[nidx], so an unclamped trap double-books it (once in
        // suspension, once as deposit) and the ledger over-counts deposition.
        if shade > 0.5 {
            let trap = (travel * shade * 0.8).min(travel).max(0.0);
            surface.set_index(idx, surface.get_index(idx) + trap);
            balance.deposited_m3 += trap * m2;
            suspended[nidx] -= trap;
        }
    }

    // Anything still suspended settles where it is: the transport pass ran
    // out of downwind cells, but the mass is still on the grid. Counting it
    // as export would break the ledger the same way the fluvial stranded
    // load did.
    for (i, &s) in suspended.iter().enumerate() {
        if s > 0.0 {
            surface.set_index(i, surface.get_index(i) + s);
            balance.deposited_m3 += s * m2;
        }
    }

    surface.sanitize_non_finite(0.0);
    repair_field(surface);
    balance
}

/// Ratio of upwind relief to local fetch, 0 (exposed) to ~1 (deep shadow).
fn upwind_relief_ratio(
    surface: &ScalarField,
    x: usize,
    y: usize,
    wx: f64,
    wy: f64,
    fetch: usize,
) -> f64 {
    let z = surface.get(x, y);
    let mut max_slope = 0.0;
    for i in 1..=fetch {
        let px = (x as f64 - wx * i as f64).round() as i64;
        let py = (y as f64 - wy * i as f64).round() as i64;
        let rise = surface.get_wrapped(px, py) - z;
        // Angle of the upwind skyline; anything above ~20° casts a shadow.
        let slope = rise / i as f64;
        if slope > max_slope {
            max_slope = slope;
        }
    }
    (max_slope / 0.35).clamp(0.0, 0.9)
}

/// Dune migration rate in m/yr for a dune of `height_m`.
///
/// Bagnold: migration is inversely proportional to height — small dunes race,
/// large ones crawl.
pub fn dune_migration_m_yr(height_m: f64, capacity_myr: f64) -> f64 {
    if height_m <= 0.0 {
        return 0.0;
    }
    capacity_myr / height_m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Boundary, BoundaryKind};

    fn test_grid(w: usize, h: usize) -> Boundary {
        Boundary::new(BoundaryKind::Rectangular, w, h)
    }

    fn bare(b: Boundary) -> ScalarField {
        ScalarField::zeros(b)
    }

    #[test]
    fn below_threshold_nothing_moves() {
        let b = test_grid(16, 16);
        let mut f = ScalarField::filled(b, 50.0);
        let before = f.clone();
        let params = WindParams {
            wind_speed_ms: 1.0, // well below the 5 m/s default threshold
            ..Default::default()
        };
        let balance = apply(&mut f, &params, &bare(b), 100.0, 1000.0);
        assert_eq!(balance, MassBalance::default());
        assert_eq!(f.as_slice(), before.as_slice());
    }

    #[test]
    fn windward_erodes_and_mass_is_accounted() {
        let b = test_grid(32, 8);
        // A dune-symmetric bump: wind from -x erodes the stoss side.
        let mut f = ScalarField::zeros(b);
        for y in 0..8 {
            for x in 0..32 {
                let d = (x as f64 - 16.0) / 6.0;
                f.set(x, y, 20.0 * (-(d * d)).exp());
            }
        }
        let before: f64 = f.as_slice().iter().sum();
        let params = WindParams {
            wind_speed_ms: 15.0,
            wind_dir_deg: 0.0,
            ..Default::default()
        };
        let balance = apply(&mut f, &params, &bare(b), 100.0, 100.0);
        assert!(balance.removed_m3 > 0.0, "no wind erosion");
        assert!(
            balance.is_conservative(before * 100.0 * 100.0 * 1e-3),
            "wind mass not conserved: {balance:?}"
        );
        assert_eq!(f.non_finite_count(), 0);
    }

    #[test]
    fn vegetation_suppresses_erosion() {
        let b = test_grid(24, 24);
        let params = WindParams {
            wind_speed_ms: 12.0,
            ..Default::default()
        };
        let mut bare_f = ScalarField::filled(b, 30.0);
        let mut veg_f = ScalarField::filled(b, 30.0);
        let bare_veg = bare(b);
        let dense_veg = ScalarField::filled(b, 1.0);
        let b1 = apply(&mut bare_f, &params, &bare_veg, 100.0, 500.0);
        let b2 = apply(&mut veg_f, &params, &dense_veg, 100.0, 500.0);
        assert!(b1.removed_m3 > b2.removed_m3, "vegetation had no effect");
    }

    #[test]
    fn dune_migration_is_inverse_to_height() {
        let fast = dune_migration_m_yr(2.0, 10.0);
        let slow = dune_migration_m_yr(20.0, 10.0);
        assert!(fast > slow, "small dunes should migrate faster");
        assert_eq!(dune_migration_m_yr(0.0, 10.0), 0.0);
    }

    #[test]
    fn zero_timestep_is_a_no_op() {
        let b = test_grid(12, 12);
        let mut f = ScalarField::filled(b, 40.0);
        let before = f.clone();
        let balance = apply(&mut f, &WindParams::default(), &bare(b), 100.0, 0.0);
        assert_eq!(balance, MassBalance::default());
        assert_eq!(f.as_slice(), before.as_slice());
    }
}