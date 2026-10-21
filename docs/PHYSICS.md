# GeoGenesis physics

This document derives the models the engine implements, states their units and
assumptions, and lists the references behind them. It is meant to make the
simulation auditable: every process can be traced to an equation here.

Conventions: metres (m), years (yr), kilograms (kg), seconds (s) only inside
formulas that need them; rates are m/yr unless stated. `z` is surface elevation,
`sea` the sea-level datum, `A` drainage area (m²), `S` channel slope (m/m),
`dx` cell size (m).

---

## 1. Plate kinematics and tectonic forcing

### 1.1 Kinematic plate model

The engine uses a **kinematic** plate model (as used to drive landscape
evolution models from plate reconstructions, e.g. GPlates → Badlands), not a
mantle-convection solver. `N` plates are Voronoi cells around seed points that
move with constant velocities `v_i` (m/yr). Each plate carries a crustal
thickness `T_i` and density `ρ_i`: continental plates start with the configured
mean crustal thickness and a lower density, oceanic plates with ~7 km of dense
basalt.

Boundary classification uses the relative velocity projected on the boundary
normal `n̂` (unit vector from the nearest plate centre to the second nearest):

```
v_rel = v_1 − v_2
c  = v_rel · n̂        (positive = convergence, m/yr)
s  = |v_rel − c n̂|    (transform shear, m/yr)
```

with `|c| > threshold · v_mean` separating convergent/divergent from
transform. The eight D8 directions are used for neighbourhood tests.

### 1.2 Vertical tectonic rate

The instantaneous vertical rate `ż` (m/yr) is a smooth function of distance `d`
to the boundary:

```
convergent:
  continental–continental : +k · min(|c|/v_mean, 2) · 0.25 · exp(−(d/w)²)
  continental–oceanic     : +k · min(|c|/v_mean, 2) · 0.20 · exp(−(d/w)²)   (arc)
  oceanic–oceanic         : −k · min(|c|/v_mean, 2) · 0.30 · exp(−(d/w)²)   (trench)
divergent:
  ridge uplift            : +0.08 k · exp(−(d/w_ridge)²)
transform:
  ~0 (small vertical offset via the initial relief only)
```

where `k = uplift_mm_yr × 10⁻³` is the parameterised scale, `w` the orogen
width, and `w_ridge ≈ 0.35 w`. Rates are deliberately a few mm/yr: the
parameter is a rate scale, not a multiplier of the plate penetration speed.
The hotspot swell term is

```
ż_hot = (hotspot_strength_m / 2e6 yr) · exp(−2 (r/r_hot)²)
```

so a default 1200 m strength builds a ~1 km swell over about 2 Myr.

**Hotspot trails.** The map frame is the mantle frame for plate boundaries, but
plume centres are advected at the negative of the local plate velocity, which
reproduces the relative motion that builds volcanic chains in map coordinates.

**Limitations.** The model does not solve force balance; plate boundaries are
Voronoi bisectors (with a noise warp parameter for irregularity); crust is not
advected as a material field through moving boundaries. These are conscious
trade-offs for a browser-scale engine; the vertical rates and geometry are
calibrated to be physically plausible rather than dynamically exact.

### 1.3 Isostasy (Airy)

Airy equilibrium with crustal thickness `T` and density `ρ_c` over a mantle of
density `ρ_m`:

```
z_eq = sea + (1 − ρ_c/ρ_m)(T − T_ref)
```

For continental crust the buoyancy factor `1 − ρ_c/ρ_m ≈ 0.16`: each kilometre
of extra crust supports ~160 m of elevation. In the ocean branch the water
column is included, so the effective factor is
`(ρ_m − ρ_o)/(ρ_m − ρ_w) ≈ 0.13`, with `ρ_w = 1030 kg/m³`.

The initial crust is chosen so that Airy equilibrium reproduces the generated
relief exactly (`anchor_crust`). Isostasy then relaxes toward the target with
time constant `τ` (`isostasy_relaxation_kyr`):

```
z ← z + (z_eq − z)(1 − exp(−dt/τ))
```

so it responds only to *changes*: tectonic thickening, erosional unloading,
sediment and ice loads. Removing `h` metres of rock thins the crust by `h`; the
equilibrium falls by only `0.16 h`, so most of the removed thickness is
recovered by rebound — the standard erosional unloading response.

**Flexure** is approximated by smoothing the load with a Gaussian whose radius
follows the elastic thickness implied by the `flexure_rigidity` parameter. This
reproduces the first-order effect (broad loads are supported over long
wavelengths) without solving the biharmonic plate equation.

---

## 2. Initial relief

1. **Craton field.** A low-frequency fBm value-noise field is rank-normalised
   to `[0, 1]` (so a threshold maps directly to an area fraction), biased by
   plate type (`±0.14`), smoothed, and thresholded with a smoothstep to a
   continental mask `c ∈ [0, 1]` with target area
   `0.22 + 0.50 · continental_fraction`.
2. **Crust.** `T = 7 km + c · (plate_thickness − 7 km)`, then smoothed over a
   few cells. Smoothing produces continental shelves, slopes and a natural
   coastline without special-casing the coast.
3. **Airy base.** Section 1.3 with the water-load branch, referenced so a
   default continental column sits ~400 m above the datum.
4. **Fractal relief.** Seeded fractal noise (Perlin/simplex/value/wavelet/
   Gaussian spikes; fBm/ridged/billow) with integer frequency ratios for exact
   tiling on toroidal grids. Amplitude `vertical_m × (0.05 + 0.13 c)` gives
   rough continents and smoother abyssal plains.
5. **Folds.** `amp · sin(2π u/λ + φ)` along the fabric axis, gated by boundary
   proximity so fold belts stay in orogenic zones.
6. **Faults.** `±fault_offset` with exponential fall-off from the boundary and
   deterministic per-cell jitter.
7. **Orogens and trenches.** Boundary vertical rates are averaged over the
   orogen width, integrated over a 1.2 Myr reference span, capped at 30% of the
   vertical scale, and applied with a continental weighting.
8. **Hotspots.** Gaussian swells from the plume field.
9. **Landform stamps.** Ranges, plateaus, rifts, ocean ridges, trenches,
   calderas, mesas and isolated peaks placed according to boundary type.
10. **Safety corrections.** Clamp to the vertical envelope, repair non-finite
    values, and if a seed is pathological (all land/all ocean) shift the datum
    by the minimum amount needed to restore a plausible mix.

---

## 3. Climate

### 3.1 Temperature

The baseline is a zonal profile about the global mean (`mean_temp_c`):
equatorial latitudes are above the mean and high latitudes below, with the
profile integrated to preserve the parameter mean. Regional (non-spherical)
maps use a configurable latitude window (`latitude_center_deg`,
`latitude_span_deg`; default 22°N centre, 62° span, i.e. tropics to temperate),
so a 4,000 km map does not unrealistically span pole to pole.

```
T = T_profile(lat) − Γ·max(z − sea, 0)/1000               (lapse rate Γ ≈ 6.5 °C/km)
  − continentality · f(distance to ocean)                 (interior cooling)
  + ocean_moderation · exp(−d_ocean/250 km)               (currents)
  + ΔT_CO2,   ΔT_CO2 = S · log2(CO₂/280)                  (S ≈ 3 °C per doubling)
  + A_cycle · sin(2π t / P_cycle)                         (orbital cycles)
  − snow_albedo_cooling_c · snow_fraction                 (ice–albedo feedback)
```

The per-cell seasonal amplitude (continentality, latitude, snow) is also
produced for the glacial model.

### 3.2 Moisture and precipitation

Air is advected in the upwind direction (a cached sweep order). The column
moisture `q ∈ [0, 1]` is saturated over ocean cells (`q = 1`). Over land:

```
lift        = max(0, ŵ · ∇z)                     (upslope in the wind direction)
rain_frac   = clamp((q0 + q1 · lift·orographic) · f_elev, f_min, 0.9)
rain        = q_in · rain_frac
q_out       = q_in − rain + recycling · rain
```

plus a subtropical dry belt (Hadley circulation), land–sea distance effects,
light smoothing, and a soft-capped rescaling of the land pattern so the mean
land precipitation equals `precipitation_mm`. Rain shadows are therefore
emergent: windward slopes rain out, lee sides stay dry.

### 3.3 Biomes and vegetation

Biome classification extends Whittaker's temperature–precipitation diagram with
aridity (potential evapotranspiration over precipitation), elevation (alpine)
and snow/ice. Classes: tropical rainforest, savanna, desert, grassland,
temperate forest, Mediterranean, boreal forest, tundra, ice, alpine, wetland,
ocean. Vegetation cover is a state variable grown logistically toward a
carrying capacity set by moisture, warmth and soil, with a disturbance reset
on steep, dry, actively eroding cells. Vegetation reduces erosion
(stream-power protection, aeolian shielding) and raises soil cohesion.

---

## 4. Hydrology

### 4.1 Depression filling

Closed depressions are raised to their spill level with the priority-flood
algorithm (Barnes et al., 2014): a min-heap seeded from ocean cells (or the
global minimum on a closed world) visits cells in increasing elevation and
assigns `filled = max(z, spill)`. The difference `filled − z` is standing water;
the flood parent defines the outlet direction through flats and lakes, which
avoids the classic D8 dead ends.

**Spherical grids.** On `Spherical` boundaries the grid carries the planet
radius and each cell has its exact latitude-band area
`A = 2R²Δλ·sin(Δφ/2)·cos(φ_c)`, which sums to `4πR²`. Hydrology accumulation
and discharge, statistics, climate means and all erosion volume conversions use
`cell_area_m2_at(x, y)`, so mass conservation holds across latitude bands;
planar worlds take the historical constant-area path bit-for-bit.

### 4.2 Routing and accumulation

Receivers come from the flood parent (D8); a Kahn topological sort over the
receiver forest gives an upstream–downstream order. Catchment area, discharge
`Q = Σ A_cell · runoff` and the channel mask (area threshold, top decile =
major rivers) are accumulated in that order. The multiple-flow-direction option
partitions flow among downslope neighbours by a power law of slope.

### 4.3 Runoff

`runoff = precip · (1 − infiltration) · (1 − evapotranspiration) + baseflow`,
with baseflow a fraction of infiltration. A relative infiltration/evaporation
cap prevents a dry-by-construction configuration.

---

## 5. Fluvial incision and deposition

### 5.1 Detachment-limited stream power

```
E = K_f · A^m · S^n            (m/yr)
```

Discretised with `S = (z_i − z_r)/dx`. Explicit time stepping would require
`dt < dx/(K A^m S^{n−1})`, which is orders of magnitude below geological steps,
so the engine solves the **implicit** form (Braun & Willett, 2013). For `n = 1`
the solution is exact and O(N):

```
z_i = (z_i⁰ + C z_r) / (1 + C),      C = K A^m dt / dx
```

processed downstream-to-upstream so `z_r` is already final. For `n ≠ 1`, the
coefficient is frozen at the current slope, `C = K A^m S^{n−1} dt/dx`, and a
short fixed-point iteration converges quickly for the supported exponents
(0.5–2). The scheme is unconditionally stable and cannot invert a channel below
its receiver, since `z_i` is a convex combination of `z_i⁰` and `z_r`.

`K` is modulated by rock erodibility (`geology::erodibility`) and vegetation
protection: `K_eff = K_f · f_rock · (1 − 0.75 · root_strength · vegetation)`.

### 5.2 Depositional laws (implicit, mass-conserving)

For transport-limited and SPACE-style laws the engine sweeps upstream carrying
the incoming sediment volume `V_in`. The transport capacity `C(z) = Kt·A^m·dt·
A_c·((z−z_r)/dx)^n` is evaluated at the *unknown* surface `z`, and each cell is
solved implicitly with a bracketed Newton iteration:

```
deficit  : z = z₀ − λ(C(z) − V_in)/A_c        (λ = 1 transport-limited, K/Kt SPACE)
excess   : z = z₀ + φ(V_in − C(z))/A_c        (φ = 1 − e^{−k_d dt}, exact settling)
```

so the law is stable at geological time steps without the former explicit caps.
The ledger closes by construction (`V_out = V_in + V_eroded − V_deposited`), and
the transport-limited steady state recovers the analytic
`S^n = (U/Kt)·A_c^{−m}·a^{1−m}` (fitted exponents 0.4999/0.3333/0.3000 for the
validated parameter sets).

### 5.3 Laws

- **Detachment-limited stream power** — incision only; eroded volume is
  exported to the base level (ocean/lake).
- **Transport-limited / SPACE-style** — a sediment flux is carried downstream,
  deposited where transport capacity drops (lakes, floodplains, low-gradient
  reaches), and the remainder is exported. The ledger records erosion,
  deposition and export separately and closes within tolerance.

### 5.3 Channel initiation

A cell is a channel when its catchment exceeds `channel_init_cells`; hillslope
cells are handled by diffusion. Bank erosion and floodplain deposition use the
discharge and channel width derived from `Q` and Manning's equation.

---

## 6. Hillslope processes

### 6.1 Creep / diffusion

```
∂z/∂t = ∇·(D ∇z)
```

discretised conservatively on cell edges with a pair exchange (the same volume
leaves one cell and enters the other). The diffusivity grows with slope
(nonlinear Roering-style creep, Roering et al., 1999) and is reduced by
vegetation roots. Explicit updates are sub-stepped so that
`D dt/dx² ≤ ~0.2`, with a hard substep cap; a per-pair flux limiter keeps the
scheme positivity-preserving even for absurd time steps.

### 6.2 Soil

Soil depth grows with a production rate limited by the remaining capacity
(`soil_max`), modulated by vegetation; it controls creep depth, aeolian supply
and cohesion. Soil is stripped on steep or submerged cells.

### 6.3 Landsliding

Where the local slope exceeds the critical angle (reduced by root cohesion),
excess material is transferred to the steepest downslope neighbour, with the
runout distance parameter controlling the fraction moved per event. Transfers
are pairwise and exactly mass-conserving.

---

## 7. Glacial processes

**Mass balance.** The snowline (ELA) is derived from the local temperature
relative to a reference, with `ela_sensitivity_m_per_c`. Accumulation is above
the snowline and scaled by snowfall; ablation uses a positive-degree-day
formulation based on the seasonal amplitude. Ice thickness relaxes toward the
local balance with a ~4 kyr time constant and is capped.

**Flow.** Ice flows down the surface gradient with a shallow-ice (SIA)
diffusion, `q ∝ −h^{n+2}|∇s|^{n−1}∇s` (n = 3), integrated with CFL sub-steps:
ice transport is conservative pairwise.

**Erosion.** Sliding velocity scales with `h²|∇s|` (Glen flow) and erosion is
proportional to sliding, capped by the available ice and a per-step limit.
Eroded material is exported and tracked in the ledger.

**U-valleys.** A mass-conserving lateral pass moves material from steep ice
flanks toward the valley axis and redeposits till downslope; ridge crests are
protected. The previous (non-conservative) neighbourhood blend was removed, and
the conservation test is part of the suite.

---

## 8. Aeolian processes

Threshold shear velocity `u* = u√C_d` (drag coefficient parameter). Above the
threshold, material moves as saltation, short-range loess or suspended dust,
limited by vegetation cover and sediment supply. Saltation deposits preferentially
in lee/low cells (dune formation switch), loess travels a few cells, and dust is
exported. The ledger closes exactly by construction (residual assigned to export).

---

## 9. Coastal processes

Wave energy at the shore is estimated from the upwind wave fetch (a march over
water up to the fetch reference distance), wind speed and tidal range. Cliff
retreat is proportional to wave power, exposure and sea-level position, and
inversely related to rock resistance. Eroded sediment builds beaches and deltas
in shallow water, weighted by river discharge; longshore drift redistributes
sediment along the coast at ±90° to the wind. Sea-level change itself is driven
by the simulation (eustatic cycle + trend), not by this module.

---

## 10. Chemical weathering

Dissolution follows Arrhenius-like kinetics:

```
R = k_d · solubility · exp(−E_a/R_g (1/T − 1/T_ref)) · f(Q) · f(karst)
```

with `f(Q)` a power law in water flux, temperature sensitivity blended by the
`sensitivity` parameter, and a karst feedback that concentrates dissolution on
flat, soluble ground with deterministic pitting. Dissolved load is exported and
tracked in the ledger.

---

## 10b. Mineral and resource deposits

`minerals.rs` is a **prospectivity model** (not ore-genesis): each of 14
deposit types (porphyry Cu–Au, orogenic gold, sedex Zn–Pb, BIF iron, coal, oil
& gas, evaporite salt/potash, uranium, diamonds, bauxite, REE pegmatite,
graphite, placer tin, geothermal) scores each cell from normalised indicators:

- tectonic activity (`|ż|` vs the 5 mm/yr physical ceiling) and relief/exhumation,
- exposed rock origin/key/permeability/solubility and erodibility (custom rocks included),
- sediment thickness, coastal/shelf/abyssal setting,
- climate (wet, arid, warm, tropical, vegetation, ice) and hydrology (river
  channels, accumulation, discharge, lakes).

The weighted rule is multiplied by a seeded two-octave metallogenic-province
noise and a deterministic per-mineral hash speckle, then the strongest mineral
is kept per cell with a 0–1 richness and an economic threshold mask. An
inventory converts deposit area × illustrative endowment × mean richness into
tonnage and value. Endowments and unit values are illustrative; the model is
deterministic and intended for comparing worlds and driving the UI/CLI, not
for resource assessment.

## 11. Couplings and feedbacks

- Erosion → crust thinning → isostatic rebound (the main long-term feedback
  between climate/erosion and tectonics).
- Sediment and ice loads depress the crust in the Airy target.
- Vegetation ↔ erosion (protection), soil ↔ vegetation, climate ↔ glacial ELA.
- Orography → precipitation → erosion → isostasy → orography.
- Uplift belts ↔ fold/fault relief; hotspots add time-transgressive swells.
- Sea level (eustatic) changes coastal erosion, lake/ocean masks and river
  base level.

---

## 12. Numerical methods

| concern | approach |
| --- | --- |
| channel incision at large `dt` | implicit downstream sweep (n = 1 exact, Newton/fixed point for general n) |
| diffusion stability | conservative pair fluxes, CFL sub-stepping, hard substep cap, per-pair flux limiter |
| ice flow stability | SIA diffusion with CFL sub-steps |
| depressions and flats | priority-flood fill + flood-parent routing |
| non-finite values | repair after every process; field sanitisation at the end of a step |
| conservation | per-process `MassBalance`; driver ledger with tolerance ∝ domain volume |
| step adaptation | halve `dt` when a step's ledger closure exceeds tolerance, recover slowly |
| determinism | seeded SplitMix64 streams with domain tags; no wall-clock; stable iteration orders |

---

## 13. Validation

The test suite checks, among others:

- reproducibility from seed; different seeds differ;
- schema/default/values round-trip and preset validity;
- periodicity of noise on toroidal grids (no seams);
- exact Euclidean distance transform against brute force (including wrap);
- priority-flood properties (receivers downhill in the filled surface, lakes
  where expected);
- implicit stream-power stability for absurd `dt`, and channel profiles that
  never invert below the receiver;
- conservation for every erosion/deposition family (erosion ≈ deposition +
  export within tolerance);
- slope-area exponent recovery for stream-power steady states;
- glacial ice only above the snowline, no ice in warm worlds, ledger closure;
- aeolian/coastal/chemical conservation and no-op cases;
- climate invariants (lapse rate, rain shadow, biome classes, ocean cells);
- default-world plausibility over 2 Myr (relief bounds, land fraction, biome
  diversity, no NaN);
- WASM ABI determinism and buffer contracts via a Node smoke test.

Performance is tracked with `geogenesis bench` (generation and per-step
timings by grid size) and the browser UI is verified end-to-end with a
DevTools-protocol screenshot script.

---

## 14. References

- Whipple, K. X. & Tucker, G. E. (1999). Dynamics of the stream-power river
  incision model: implications for height limits of mountain ranges, landscape
  response timescales, and research needs. *JGR Solid Earth*.
- Braun, J. & Willett, S. D. (2013). A very efficient O(n), implicit and
  parallel method to solve the stream power equation governing fluvial
  incision and landscape evolution. *Geomorphology*.
- Barnes, R., Lehman, C. & Mulla, D. (2014). Priority-flood: an optimal
  depression-filling and watershed-labeling algorithm for digital elevation
  models. *Computers & Geosciences*.
- Shobe, C. M., Tucker, G. E. & Barnhart, K. R. (2017). The SPACE 1.0 model: a
  Landlab component for 2-D calculation of sediment transport, bedrock
  erosion, and landscape evolution. *Geoscientific Model Development*.
- Roering, J. J., Kirchner, J. W. & Dietrich, W. E. (1999). Evidence for
  nonlinear, slope-dependent sediment transport and its implications for
  mountain-range evolution. *Water Resources Research*.
- Felzenszwalb, P. F. & Huttenlocher, D. P. (2012). Distance transforms of
  sampled functions. *Theory of Computing*.
- Whittaker, R. H. (1975). *Communities and Ecosystems* (Whittaker diagram).
- Cook, R. L. & DeRose, T. (2005). Wavelet noise. *ACM Transactions on
  Graphics*.
- Perlin, K. (1985). An image synthesizer. *SIGGRAPH*.
- GPlates / Badlands-style kinematic tectonic forcing, for the plate model
  approximation.
- Salles, T. et al. (2018). Badlands: a parallel basin and landscape dynamics
  model. *SoftwareX*.
- Braun, J. & Sambridge, M. (1997). Modelling landscape evolution on
  geological time scales: a new method based on irregular spatial
  discretization (CASCADE). *Basin Research*.
