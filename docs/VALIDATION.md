# Scientific validation

Independent, black-box validation of the GeoGenesis engine. The harness lives in
[`crates/core/tests/validation.rs`](../crates/core/tests/validation.rs) and only
uses the public API (`Simulation`, `pipeline`, `climate`, `hydrology`,
`erosion`, `stats`, `export`). It checks physical and statistical *invariants*
that must hold for any scientifically plausible run — conservation, stability,
analytic laws, output-container correctness — rather than golden outputs, so it
stays meaningful while the engine evolves.

Nothing in the engine is special-cased for the harness: every check is the same
thing a user or a downstream tool would do with the released API.

## Running

```bash
# Full validation harness (release) + CLI benchmark + printed report.
./scripts/validate.sh

# Equivalent harness invocation without the CLI benchmark
cargo test --release -p geogenesis-core --test validation -- --nocapture --test-threads=1

# Cheap checks only, in a debug build (what CI runs on every push)
cargo test -p geogenesis-core --test validation
```

The heavy checks (the 2 Myr × 12-run stability sweep and the performance smoke)
are skipped in debug builds so `cargo test --workspace` stays fast in CI. Set
`GEOGENESIS_VALIDATION_HEAVY=1` to force them in a debug build (slow). Running
the whole release harness through `scripts/validate.sh` takes well under a
minute on a laptop-class CPU (≈11–15 s measured for the harness, plus the CLI
benchmark build; timings grow when other processes share the machine).

## What is checked

| group | invariant | test | tolerance / note |
| --- | --- | --- | --- |
| long-run stability | elevation, ice, soil, sediment, erosion, deposition, crust finite | `preset_long_run_table` | zero non-finite cells |
| long-run stability | no repair-clamp pinning, relief inside an envelope | `preset_long_run_table` | \|z\| < 60 km; \|z\| < max(3×vertical scale, 15 km); relief < 25 km |
| long-run stability | plausible land fraction | `preset_long_run_table` | 0.05 – 0.95 of cells above sea level |
| conservation | cumulative process ledger closes | `preset_long_run_table` | \|erosion − deposition − export\| / total < 1e-6 |
| conservation | one coupled `apply_all` step closes | `coupled_apply_all_closes_the_ledger` | `MassBalance::is_conservative(1e-3 m³)` |
| stability | 2 Myr at 100 steps (20 kyr) across 9 presets and 3 extra seeds | `preset_long_run_table` | 12 runs, 128², `adaptive_dt` off |
| determinism | same parameters + steps replay bit-identically; seed changes the world | `deterministic_replay` | exact slice equality |
| climate | lapse rate: 200 m → 3 400 m cools the surface | `climate_invariants` | 8 °C < ΔT < 40 °C |
| climate | orographic rain shadow: windward flank wetter than lee | `climate_invariants` | windward > 1.15 × lee |
| climate | land-mean precipitation tracks the parameter after rescaling | `climate_invariants`, `preset_long_run_table` | ±5 % (±10 % at 128² for long runs) |
| climate | ocean cells never carry land biomes; land cells never Ocean | `climate_invariants`, `preset_long_run_table` | exact |
| ecology | biome classes coherent with temperature/precipitation (rainforest warm+wet, ice cold, desert dry, boreal cool, tundra cold, alpine high) | `climate_invariants` | broad envelope |
| ecology | land biome fractions sum to 1 | `climate_invariants` | ±1e-6 |
| hydrology | receivers are downhill on the depression-filled surface | `hydrology_invariants` | ≤ 1e-6 m uphill |
| hydrology | receiver graph is acyclic; every cell reaches an outlet | `hydrology_invariants` | path ≤ cell count |
| hydrology | lake depth equals fill above the raw surface; lakes sit above sea level | `hydrology_invariants` | 5 cm / 1e-4 relative |
| hydrology | closed basins are filled to the spill level | `hydrology_identifies_lakes_in_a_closed_basin` | basin fills to its 1 500 m rim |
| hydrology | discharge conservation: Σ local runoff = Σ outlet discharge | `hydrology_invariants` | relative < 1e-9 |
| hydrology | drainage area = accumulated cell area (planar grid) | `hydrology_invariants` | relative < 1e-9 |
| hydrology | channel mask only above the threshold, values 0/1/2, monotone in the threshold | `hydrology_invariants` | non-increasing counts |
| erosion | detachment-limited stream-power steady state: S ∝ A^(−m/n) for to (m,n) = (0.5,1), (0.4,1), (0.7,1), (0.5,2) | `detachment_limited_slope_area_exponent` | fitted exponent within 0.01 of −m/n, profile drift < 1e-3 m over 200 steps |
| erosion | linear hillslope diffusion decay matches the discrete analytic dispersion relation | `hillslope_diffusion_decay_matches_analytic_solution` | decay within 2e-3 absolute |
| erosion | glacial ice only on land above the snowline (cold high world) | `glacial_ice_forms_only_above_the_snowline` | no ice on ocean/warm cells; ≤ `max_ice_thickness_m` |
| erosion | aeolian/coastal/chemical no-op cases leave the surface and ledger untouched | `process_no_op_cases` | zero ledger, identical elevation |
| erosion | active processes close their own ledgers and change the surface | `active_processes_close_their_ledgers` | `is_conservative(1e-6)` |
| statistics | min ≤ mean ≤ max, p10 ≤ median ≤ p90, non-negative std/slope | `statistics_invariants` | ordering |
| statistics | hypsometric curve monotone, 21 points, integral in a plausible range | `statistics_invariants` | 0.05 – 0.95 |
| statistics | land area and biome fractions are internally consistent | `statistics_invariants` | ±1e-6 |
| exports | coloured maps are valid 8-bit RGB PNGs (all 6 map modes) | `export_format_invariants` | PNG signature/IHDR/chunks |
| exports | 16-bit heightmap is a valid 16-bit grayscale PNG | `export_format_invariants` | bit depth 16, colour type 0 |
| exports | normal map is a valid 8-bit RGB PNG | `export_format_invariants` | bit depth 8, colour type 2 |
| exports | OBJ vertex/triangle counts and index ranges | `export_format_invariants` | exact counts |
| exports | glTF 2.0 JSON parses structurally and the embedded base64 buffer decodes to the declared byte length; accessor counts match the mesh | `export_format_invariants` | exact |
| exports | XYZ point count and numeric fields | `export_format_invariants` | exact |
| exports | `export::write_all` writes every documented output and the on-disk PNGs are valid | `export_format_invariants` | 10 files, non-empty |
| performance | 256² generation and per-step cost smoke test | `performance_smoke` | generation < 10 s, step < 1 s (very generous; tight targets in `BENCHMARKS.md`) |

## Measured release run

Environment: AMD Ryzen 7 7730U (x86_64), Linux, `rustc 1.98.1`, `--release`,
measured on the integration checkpoint at commit `933c7df`. The engine is
deterministic, so the physical columns reproduce exactly on a re-run; absolute
timings vary with machine load (other CI workstreams were compiling in
parallel while these were taken, which is why the per-run times are larger
than the numbers in `BENCHMARKS.md`).

Command:

```bash
cargo test --release -p geogenesis-core --test validation -- --nocapture --test-threads=1
```

The long-run sweep uses 128² grids, 2 Myr per run at 20 kyr per step
(`adaptive_dt` disabled so every run is exactly 100 steps), nine presets plus
three additional seeds.

| preset | seed | grid | steps | gen ms | run ms | land % | relief m | max elev m | mean T °C | mean P mm/yr | biomes ≥1 % | ice % | ledger closure |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| earthlike | 7 | 128 | 100 | 63 | 1 020 | 42.44 | 8 400 | 4 135 | 9.55 | 1 200 | 10 | 7.40 | 2.2e-16 |
| young_orogeny | 7 | 128 | 100 | 63 | 1 977 | 68.19 | 12 046 | 7 744 | −1.02 | 1 500 | 10 | 31.43 | 0.0e0 |
| archipelago | 7 | 128 | 100 | 116 | 844 | 37.12 | 9 587 | 6 161 | 16.51 | 2 400 | 9 | 1.61 | 2.7e-16 |
| arid_plateau | 7 | 128 | 100 | 33 | 847 | 70.59 | 8 110 | 4 527 | 17.02 | 140 | 4 | 0.00 | 2.7e-16 |
| icehouse | 7 | 128 | 100 | 55 | 1 230 | 57.15 | 9 608 | 5 479 | −14.04 | 650 | 6 | 41.44 | 3.8e-17 |
| ancient_peneplain | 7 | 128 | 100 | 30 | 738 | 48.42 | 6 638 | 2 809 | 12.21 | 900 | 8 | 0.00 | 6.7e-16 |
| supercontinent | 7 | 128 | 100 | 78 | 1 315 | 82.12 | 8 689 | 4 948 | 7.38 | 850 | 10 | 16.44 | 5.1e-17 |
| volcanic | 7 | 128 | 100 | 85 | 833 | 47.50 | 11 877 | 7 273 | 10.05 | 1 600 | 10 | 9.80 | 0.0e0 |
| riverworld | 7 | 128 | 100 | 43 | 1 101 | 65.56 | 8 004 | 4 393 | 13.46 | 3 200 | 7 | 6.05 | 5.2e-17 |
| earthlike | 11 | 128 | 100 | 52 | 995 | 44.40 | 8 238 | 3 707 | 15.60 | 1 200 | 9 | 1.94 | 8.5e-17 |
| icehouse | 11 | 128 | 100 | 55 | 1 180 | 61.97 | 9 492 | 5 042 | −9.43 | 650 | 7 | 26.27 | 3.2e-16 |
| arid_plateau | 11 | 128 | 100 | 50 | 1 069 | 66.65 | 9 159 | 4 606 | 19.44 | 140 | 4 | 0.00 | 2.6e-16 |

Totals: 1 200 simulated steps in 14.0 s of wall clock (11.7 ms/step average,
including generation and all statistics work). Every run stayed inside the
relief envelope (highest point 7 744 m), had a non-degenerate biome mix
(4–10 classes ≥ 1 % of land), preserved finite fields and closed its mass ledger
to approximately machine precision. The closure column is
`|erosion − deposition − export| / total volume` on the cumulative driver
ledger, so 0.0 means exact floating-point cancellation on that platform.

### Analytic and cross-check diagnostics

Separate tests print their own diagnostics; from the same run:

```
slope-area:  m=0.5 n=1: fitted -0.5000 (expected -0.5000)
             m=0.4 n=1: fitted -0.4000 (expected -0.4000)
             m=0.7 n=1: fitted -0.7000 (expected -0.7000)
             m=0.5 n=2: fitted -0.2500 (expected -0.2500)
diffusion:   measured decay 0.846400 (analytic 0.846400, dt = 1e6 yr)
hydrology:   9 216 cells, 5 978 outlets, 130 channel cells (14 major),
             80 lake cells, mean runoff 489.0 mm/yr
coupled step: eroded 2.431e12 m³, deposited 1.512e11 m³,
              exported 2.279e12 m³, closure 4.9e-4 m³
performance: 256² generation 224 ms, step 40.6 ms (0.62 µs/cell/step)
             (18–41 ms/step across runs, depending on machine load)
```

### CLI benchmark (same machine, same release build)

`scripts/validate.sh` also runs the official benchmark:

```
grid   generate ms   step ms   steps/s   land %
256²      ~100–190    13–25     41–75     39.4
preview: 96² generation ~8–12 ms (spec target < 16 ms)
```

(Timings fluctuate by up to ±70 % with system load — the ranges above cover
several runs while other workstreams were compiling on the same machine. See
[`BENCHMARKS.md`](BENCHMARKS.md) for the authoritative measured targets.)

## Known limitations

* **Invariants, not ground truth.** The harness proves the engine is internally
  consistent and numerically stable; it does not compare against real DEMs,
  climate reanalysis or a reference landscape-evolution code. Calibration
  against real-world data is out of scope here.
* **Coarse grids for speed.** The 2 Myr sweep runs at 128² over ~12 runs. The
  physics is resolution-independent, but exact biome shares and relief differ
  from the 384²/4096² worlds; the bounds are set generously for that reason.
* **Qualitative climate checks.** The moisture model is a stylised upwind
  column model, so the tests check directions and broad magnitudes (lapse
  rate, rain-shadow sign and strength, land-mean precipitation) rather than
  exact GCM values.
* **Planar hydrology arithmetic.** The exact discharge/drainage-area
  conservation identities are checked on planar (rectangular/toroidal) grids;
  spherical area weighting has separate unit tests but is not re-validated
  end-to-end here.
* **Structural export checks.** PNG, OBJ, glTF and XYZ checks validate container
  structure, dimensions, counts and buffer length, not pixel-for-pixel content
  or mesh geometry against a reference.
* **Performance bounds are smoke bounds.** The harness fails only on
  catastrophic slowdowns (10 s generation / 1 s per step at 256²). Tight
  performance targets are enforced by the CLI benchmark and documented
  separately.
* **Debug builds skip heavy checks.** CI runs the cheap subset in debug; the
  full release harness is run by `scripts/validate.sh`. Set
  `GEOGENESIS_VALIDATION_HEAVY=1` to force heavy checks in debug (slow).
* **Single machine determinism.** Replay is bit-identical on the same
  build/platform (f64, no threads in the engine); cross-platform bitwise
  equality is not claimed.
* **Scope.** Tiled 4096² generation, the WASM ABI/browser layer, the plugin
  API, custom rocks/minerals and EXR/NetCDF writers are covered by their own
  tests and scripts, not by this harness.

## Findings and observations

No invariant violations were found by the harness in the validated working
tree. Two non-blocking observations were reported to the maintainers:

1. **`Simulation::stats()` can briefly disagree with itself.** The driver
   recomputes climate on a schedule (every ~40 kyr) but `stats()` reads the
   stored snapshot, so calling it right after terrain moves can report biome
   fractions from an older surface while hypsometry uses the current one. At
   96² after 10 steps the gap is small (Σbiomes − land_fraction ≈ −5e-4) and it
   disappears at the next climate refresh; the harness recomputes climate from
   the final surface before checking coherence. If exact consistency is wanted,
   callers should recompute climate (or `stats()` should do it for them).
   Reproduction:

   ```bash
   ./target/release/geogenesis generate --preset earthlike --seed 5 --grid 96 \
       --steps 10 --out /tmp/gg-stats-probe
   # then compare hypsometry.land_fraction with sum(biomes[].fraction) in
   # /tmp/gg-stats-probe/stats.json
   ```

2. **`SimState.repairs` is dead diagnostic state.** It is initialised to zero
   and never incremented, so "no repair explosion" cannot be observed through
   it. The harness instead asserts that no elevation reaches the ±60 km repair
   clamp and that no field contains non-finite values (which is the actual
   invariant). Either wire the counter into `world.sanitize()`/`erosion::repair`
   or remove the field.

Further failures found while running the harness should be reported with the
failing test name plus its printed context; each assertion message includes the
preset, seed and grid so a run can be reproduced directly.
