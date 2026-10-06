//! Erosion and deposition processes.
//!
//! Seven subprocesses from spec §3, each dimensionally consistent:
//!
//! * Rates are m/yr, durations are years, areas are m², so every product that
//!   lands on the elevation field is in metres.
//! * Each process returns a [`MassBalance`] so the driver can prove that
//!   material removed equals material deposited plus material exported. The
//!   old engine had no accounting and silently created and destroyed rock.
//! * Every `apply` clamps its own timestep to the stability limit and sanitises
//!   non-finite values before handing the field back.

pub mod chemical;
pub mod coastal;
pub mod fluvial;
pub mod glacial;
pub mod hillslope;
pub mod rainfall;
pub mod wind;

use crate::grid::ScalarField;

/// Mass removed, moved and exported by one process application.
///
/// `removed` is the volume taken off the surface, `deposited` the volume put
/// back on it elsewhere, and `exported` the volume that left the domain. For a
/// closed domain with no outlets, `removed == deposited` up to rounding; for an
/// open one, `removed == deposited + exported`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MassBalance {
    /// Volume removed from the surface, in m³.
    pub removed_m3: f64,
    /// Volume redeposited on the surface, in m³.
    pub deposited_m3: f64,
    /// Volume that left the domain, in m³.
    pub exported_m3: f64,
}

impl MassBalance {
    /// Net volume change imposed on the domain (negative = net loss).
    pub fn net_m3(&self) -> f64 {
        self.deposited_m3 - self.removed_m3
    }

    /// Residual after closing the balance. Zero for a conservative process.
    pub fn closure_error_m3(&self) -> f64 {
        self.removed_m3 - self.deposited_m3 - self.exported_m3
    }

    /// Whether the process conserved mass within `tol` m³.
    pub fn is_conservative(&self, tol_m3: f64) -> bool {
        self.removed_m3.is_finite()
            && self.deposited_m3.is_finite()
            && self.exported_m3.is_finite()
            && self.closure_error_m3().abs() <= tol_m3
    }

    /// Adds another balance into this one.
    pub fn add_assign(&mut self, other: MassBalance) {
        self.removed_m3 += other.removed_m3;
        self.deposited_m3 += other.deposited_m3;
        self.exported_m3 += other.exported_m3;
    }
}

/// Stability-limited timestep for explicit diffusion.
///
/// The 5-point Laplacian is stable only when `coef = K·dt/dx² ≤ 0.25`. Returns
/// the largest safe `dt` for the given diffusivity and grid spacing.
pub fn diffusion_timestep_yr(diffusivity_m2_yr: f64, cell_size_m: f64) -> f64 {
    if diffusivity_m2_yr <= 0.0 || cell_size_m <= 0.0 {
        return f64::INFINITY;
    }
    0.25 * cell_size_m * cell_size_m / diffusivity_m2_yr
}

/// Clamps a requested timestep to the stability limit.
pub fn clamp_timestep(requested_yr: f64, diffusivity_m2_yr: f64, cell_size_m: f64) -> f64 {
    let limit = diffusion_timestep_yr(diffusivity_m2_yr, cell_size_m);
    if limit.is_finite() {
        requested_yr.min(limit)
    } else {
        requested_yr
    }
}

/// Repairs a field after a process: replaces non-finite cells with the local
/// neighbourhood mean rather than zero, which preserves the relief instead of
/// punching flat spots into it.
pub fn repair_field(field: &mut ScalarField) -> usize {
    let b = *field.boundary();
    let mut bad: Vec<usize> = Vec::new();
    for i in 0..b.cell_count() {
        if !field.get_index(i).is_finite() {
            bad.push(i);
        }
    }
    for &idx in &bad {
        let (x, y) = b.coords(idx);
        let mut sum = 0.0;
        let mut n = 0u32;
        for &(dx, dy) in crate::grid::D8_OFFSETS.iter() {
            let v = field.get_wrapped(x as i64 + dx, y as i64 + dy);
            if v.is_finite() {
                sum += v;
                n += 1;
            }
        }
        field.set_index(idx, if n > 0 { sum / n as f64 } else { 0.0 });
    }
    bad.len()
}
