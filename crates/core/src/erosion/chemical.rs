//! Chemical weathering and dissolution (spec §3g).
//!
//! Carbonate and evaporite rocks dissolve along fractures and bedding planes.
//! The rate follows a kinetic law
//!
//! `R = k · solubility · f(T) · (Q/Q₀)^m · k_karst`
//!
//! where:
//!
//! * `f(T)` is an Arrhenius temperature factor (warmer water dissolves faster),
//! * `Q/Q₀` is the relative water flux and `m` the runoff exponent,
//! * `k_karst` captures the karst feedback: flat, well-drained soluble rock
//!   develops a roughened, pitted surface because dissolution focuses on
//!   fractures and dolines instead of a smooth plain.
//!
//! Warm, wet limestone regions therefore develop karst and lose elevation
//! faster than crystalline shields, exactly the contrast the spec asks for.
//! Dissolved material leaves the surface as export, so the process ledger
//! closes exactly by construction.

use crate::climate::Climate;
use crate::erosion::MassBalance;
use crate::geology::Geology;
use crate::hydrology::Hydro;
use crate::math::clamp;
use crate::math::rng::{hash2, unit_from_hash};
use crate::params::Params;
use crate::world::World;

/// Universal gas constant, J/(mol·K).
const GAS_CONSTANT: f64 = 8.314;
/// Reference temperature for the Arrhenius factor, K (15 °C).
const REFERENCE_TEMPERATURE_K: f64 = 288.15;
/// Reference dissolution rate of soluble rock, m/yr.
const REFERENCE_DISSOLUTION_M_YR: f64 = 1.2e-4;
/// Domain tag for deterministic karst pits.
const DOMAIN_KARST: u64 = 0x6B61_7273_7400_0001;

/// Applies dissolution over `dt_yr`.
pub fn apply(
    world: &mut World,
    params: &Params,
    climate: &Climate,
    hydro: &Hydro,
    geology: &Geology,
    dt_yr: f64,
) -> MassBalance {
    if dt_yr <= 0.0 {
        return MassBalance::default();
    }
    let c = &params.chemical;
    let mut balance = MassBalance::default();
    if c.dissolution_coefficient <= 0.0 || c.reactivity <= 0.0 {
        return balance;
    }
    let grid = world.grid;
    let planar_area = grid.cell_area_m2();
    let spherical_areas = grid.has_spherical_areas();
    let sea = world.sea_level_m;
    let reference_flux_m_yr = 1.0; // ~ the basin-mean water flux (m/yr)
    let seed = params.world.effective_seed();
    let activation = c.activation_energy_kj_mol.max(0.0) * 1000.0;
    let exponent = c.runoff_exponent.max(0.05);
    let temperature_weight = c.temperature_sensitivity.clamp(0.0, 1.0);

    let mut erosion = vec![0.0f64; grid.cell_count()];
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            let z = world.elevation_m.get_index(i);
            if z <= sea {
                continue;
            }
            let lake = f64::from(hydro.lake_depth_m.get_index(i));
            if lake > 0.0 {
                continue; // standing water: deposition, not dissolution
            }
            let solubility =
                clamp(f64::from(geology.solubility.get_index(i)), 0.0, 1.0) * c.reactivity;
            if solubility <= 1e-6 {
                continue;
            }
            // Arrhenius temperature kinetics.
            let temp_k =
                (f64::from(climate.temperature_c.get_index(i)) + 273.15).clamp(250.0, 340.0);
            let arrhenius = if activation > 0.0 {
                (-activation / GAS_CONSTANT * (1.0 / temp_k - 1.0 / REFERENCE_TEMPERATURE_K)).exp()
            } else {
                1.0
            };
            let temp_factor = clamp(1.0 + temperature_weight * (arrhenius - 1.0), 0.1, 8.0);
            // Absolute water flux: routed discharge plus direct runoff, in
            // metres of water per year. Dissolution increases with the amount
            // of water passing through, not its ratio to the basin mean.
            // Water depth equivalent: discharge divided by the physical cell
            // area (constant on planar grids, latitude-band on spherical ones).
            let discharge_depth =
                hydro.discharge_m3_yr.get_index(i).max(0.0) / grid.cell_area_m2_at_index(i);
            let direct = f64::from(climate.precipitation_mm.get_index(i)) / 1000.0 * 0.4;
            let flux = discharge_depth + direct;
            let runoff_factor = clamp((flux / reference_flux_m_yr).powf(exponent), 0.02, 8.0);
            // Karst feedback: flat, soluble ground drains through fractures and
            // dissolves into a pitted surface. The deterministic per-cell hash
            // roughens the rate without breaking reproducibility or mass
            // conservation.
            let (gx, gy) = world.elevation_m.gradient(x, y);
            let slope = gx.hypot(gy);
            let flatness = 1.0 - clamp(slope / 0.25, 0.0, 1.0);
            let pit = 2.0 * unit_from_hash(hash2(x as u64, y as u64, seed ^ DOMAIN_KARST)) - 1.0;
            let karst = 1.0 + c.karst_roughness * (0.45 * flatness + 0.55 * pit);
            let rate = REFERENCE_DISSOLUTION_M_YR
                * c.dissolution_coefficient
                * solubility
                * temp_factor
                * runoff_factor
                * karst.max(0.05);
            let depth = (rate * dt_yr).min(2.0).min(z - sea);
            if depth > 0.0 {
                erosion[i] = depth;
            }
        }
    }

    let mut erosion_volume = 0.0f64;
    for i in 0..grid.cell_count() {
        let depth = erosion[i];
        if depth <= 0.0 {
            continue;
        }
        world
            .elevation_m
            .set_index(i, world.elevation_m.get_index(i) - depth);
        // Dissolution consumes a little soil cover as the surface settles.
        world.soil_m.set_index(
            i,
            (f64::from(world.soil_m.get_index(i)) - depth * 0.05).max(0.0) as f32,
        );
        let cumulative = f64::from(world.erosion_m.get_index(i)) + depth;
        world.erosion_m.set_index(i, cumulative as f32);
        erosion_volume += if spherical_areas {
            depth * grid.cell_area_m2_at_index(i)
        } else {
            depth
        };
    }
    // Per-cell physical areas on spherical grids; historical arithmetic on
    // planar grids.
    balance.erosion_m3 = if spherical_areas {
        erosion_volume
    } else {
        erosion_volume * planar_area
    };
    // Everything dissolved is carried away in groundwater/rivers.
    balance.export_m3 = balance.erosion_m3;
    balance
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

    fn setup() -> (World, Params, Climate, Hydro, Geology) {
        let mut p = Params::default();
        p.world.width = 16;
        p.world.height = 16;
        p.world.cell_km = 5.0;
        p.chemical.dissolution_coefficient = 1.0;
        let grid = p.world.grid().unwrap();
        let mut world = World::new(&p.world).unwrap();
        world.elevation_m.fill_value(400.0);
        let mosaic = PlateMosaic::build(&p, grid, 4);
        let mut geology = Geology::build(&p, &world, &mosaic, 4);
        geology.solubility.fill_value(1.0);
        let climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 1000.0,
        });
        let hydro = route(&world, &p, Some(&climate));
        (world, p, climate, hydro, geology)
    }

    fn apply_dt(
        world: &mut World,
        p: &Params,
        climate: &Climate,
        hydro: &Hydro,
        geology: &Geology,
        dt: f64,
    ) -> MassBalance {
        super::apply(world, p, climate, hydro, geology, dt)
    }

    #[test]
    fn soluble_rock_dissolves() {
        let (mut world, p, climate, hydro, geology) = setup();
        let before = world.elevation_m.get(8, 8);
        let balance = apply_dt(&mut world, &p, &climate, &hydro, &geology, 100_000.0);
        assert!(balance.erosion_m3 > 0.0);
        assert!(world.elevation_m.get(8, 8) < before);
        assert!(balance.is_conservative(1e-9));
        assert_eq!(balance.export_m3, balance.erosion_m3);
    }

    #[test]
    fn insoluble_rock_survives() {
        let (mut world, p, climate, hydro, mut geology) = setup();
        geology.solubility.fill_value(0.0);
        let before = world.elevation_m.get(8, 8);
        let balance = apply_dt(&mut world, &p, &climate, &hydro, &geology, 1e5);
        assert_eq!(balance.erosion_m3, 0.0);
        assert_eq!(world.elevation_m.get(8, 8), before);
    }

    #[test]
    fn warm_water_dissolves_faster() {
        let (mut cold_world, p, mut cold_climate, hydro, geology) = setup();
        for t in cold_climate.temperature_c.as_mut_slice() {
            *t = 0.0;
        }
        let cold = apply_dt(&mut cold_world, &p, &cold_climate, &hydro, &geology, 5e3);

        let (mut warm_world, p2, mut warm_climate, hydro2, geology2) = setup();
        for t in warm_climate.temperature_c.as_mut_slice() {
            *t = 30.0;
        }
        let warm = apply_dt(&mut warm_world, &p2, &warm_climate, &hydro2, &geology2, 5e3);
        assert!(
            warm.erosion_m3 > cold.erosion_m3 * 1.5,
            "warm {} vs cold {}",
            warm.erosion_m3,
            cold.erosion_m3
        );
    }

    #[test]
    fn wetter_climate_dissolves_faster() {
        let build = |precip: f64| -> (World, Params, Climate, Hydro, Geology) {
            let mut p = Params::default();
            p.world.width = 16;
            p.world.height = 16;
            p.world.cell_km = 5.0;
            p.chemical.dissolution_coefficient = 1.0;
            p.climate.precipitation_mm = precip;
            let grid = p.world.grid().unwrap();
            let mut world = World::new(&p.world).unwrap();
            world.elevation_m.fill_value(400.0);
            let mosaic = PlateMosaic::build(&p, grid, 4);
            let mut geology = Geology::build(&p, &world, &mosaic, 4);
            geology.solubility.fill_value(1.0);
            let climate = compute(&ClimateInputs {
                world: &world,
                params: &p,
                previous: None,
                dt_yr: 1000.0,
            });
            let hydro = route(&world, &p, Some(&climate));
            (world, p, climate, hydro, geology)
        };
        let (mut dry_world, dry_p, dry_climate, dry_hydro, dry_geology) = build(200.0);
        let dry = apply_dt(
            &mut dry_world,
            &dry_p,
            &dry_climate,
            &dry_hydro,
            &dry_geology,
            5e3,
        );
        let (mut wet_world, wet_p, wet_climate, wet_hydro, wet_geology) = build(3000.0);
        let wet = apply_dt(
            &mut wet_world,
            &wet_p,
            &wet_climate,
            &wet_hydro,
            &wet_geology,
            5e3,
        );
        assert!(
            wet.erosion_m3 > dry.erosion_m3,
            "wet {} vs dry {}",
            wet.erosion_m3,
            dry.erosion_m3
        );
    }

    #[test]
    fn karst_roughness_pits_the_surface() {
        let (mut smooth_world, mut p, climate, hydro, geology) = setup();
        p.chemical.karst_roughness = 0.0;
        apply_dt(&mut smooth_world, &p, &climate, &hydro, &geology, 1e3);

        let (mut rough_world, mut p2, climate2, hydro2, geology2) = setup();
        p2.chemical.karst_roughness = 1.0;
        apply_dt(&mut rough_world, &p2, &climate2, &hydro2, &geology2, 1e3);

        let variance = |world: &World| -> f64 {
            let mean = world.erosion_m.mean().unwrap_or(0.0);
            world
                .erosion_m
                .as_slice()
                .iter()
                .map(|v| {
                    let d = f64::from(*v) - mean;
                    d * d
                })
                .sum::<f64>()
                / world.erosion_m.len().max(1) as f64
        };
        assert!(
            variance(&rough_world) > variance(&smooth_world) * 1.05,
            "karst pits should roughen the erosion field"
        );
    }

    #[test]
    fn chemical_is_deterministic() {
        let (mut a, p, ca, ha, ga) = setup();
        let (mut b, _, cb, hb, gb) = setup();
        let ba = apply_dt(&mut a, &p, &ca, &ha, &ga, 1e5);
        let bb = apply_dt(&mut b, &p, &cb, &hb, &gb, 1e5);
        assert_eq!(a.elevation_m.as_slice(), b.elevation_m.as_slice());
        assert_eq!(ba, bb);
    }
}
