# GeoGenesis architecture

This document describes how the engine, the WASM bridge and the Web UI fit
together, and where to add code. It complements the requirement document
`SPEC.md`.

## 1. Design principles

1. **Determinism is a contract.** A world is a pure function of `Params` and
   the number of steps. No clocks, no OS entropy, no thread-count-dependent
   ordering. Randomness comes from `math::rng` seeded only by
   `WorldParams::seed`; each subsystem derives its own stream with
   `derive_seed(seed, TAG)`.
2. **Physical units at every boundary.** Internals are metres, years, kg/m³,
   m/yr; UI parameters use the conventional units named by their `ParamSpec`
   (km, mm/yr, °C) and are converted where the process consumes them.
3. **Conserve mass where the physics says so.** Processes return a
   `MassBalance`; the driver accumulates a ledger and checks closure.
4. **Stable at geological time steps.** Implicit or CFL-substepped solvers
   instead of tiny explicit steps.
5. **One source of truth for parameters.** A `ParamSpec` table per subsystem
   drives defaults, validation, the CLI, the JSON schema and the UI.
6. **Zero dependencies in the core and the WASM bridge.** Small binary,
   reproducible builds, no supply-chain surface.

## 2. Workspace

```
Cargo.toml                 Rust workspace
crates/core/               geogenesis-core (no dependencies)
  src/
    lib.rs                 module map + re-exports
    error.rs               GeoError / GeoResult
    math/                  rng, noise, Vec2, scalar helpers
    grid.rs                Grid geometry + generic dense Field<T>
    spatial.rs             Euclidean distance transform, Gaussian blur,
                           perimeter, downsampling
    params/                ParamSpec, ParamBlock, Params aggregate, presets,
                           per-subsystem parameter blocks
    world.rs               World state (elevation, crust, sediment, soil, ice,
                           erosion, tectonic rate, sea level)
    rocks.rs               user-defined rock catalogue and strata stack (JSON)
    minerals.rs            14-mineral prospectivity map + inventory
    plates.rs              kinematic plates, boundaries, hotspot swells
    uplift.rs              tectonic vertical motion, Airy isostasy, flexure
    geology.rs             14-rock catalogue, strata, erodibility, solubility
    climate/mod.rs         temperature, moisture sweep, biomes, vegetation
    hydrology/mod.rs       priority-flood fill, D8 routing, accumulation,
                           discharge, lakes, river mask
    erosion/
      mod.rs               MassBalance, CFL helpers, repair, process chain
      fluvial.rs           implicit stream-power incision + deposition
      hillslope.rs         diffusion, soil production, landsliding
      glacial.rs           ice mass balance, flow, erosion, U-valleys
      aeolian.rs           saltation / loess / dust
      coastal.rs           wave fetch, retreat, deltas, longshore drift
      chemical.rs          dissolution kinetics
    pipeline.rs            parameter set → initial world (Phases 0–2)
    tiling.rs              streaming tiled generation for 4096² worlds
    plugin.rs              user-defined process plugin API + impact demo
    sim/mod.rs             time loop, schedule, adaptive dt, history, seeking
    stats.rs               terrain statistics, hypsometry, profiles, JSON
    export/mod.rs          raw f32, PNG (8/16-bit), normal maps, cross-section
                           figures, OBJ, glTF, XYZ, EXR, NetCDF, write_all
    export/exr.rs          dependency-free OpenEXR writer (FLOAT/HALF)
    export/netcdf.rs       dependency-free NetCDF-3 classic writer
    export/tiled.rs        streaming PNG/raw writers for tiled generation
crates/wasm/               geogenesis-wasm: raw C ABI for wasm32
crates/cli/                geogenesis: generate / bench / sweep / schema
web/                       Vite + TypeScript + three.js UI
scripts/                   build-wasm.sh, smoke-wasm.mjs, ui-shot.mjs
docs/                      this file, PHYSICS.md
```

## 3. Data model

### Grid and Field

`grid::Grid` is immutable geometry: width/height in cells, cell size in metres
and a boundary topology (`Rectangular`, `Toroidal`, `Spherical`). All edge
behaviour (neighbour lookup, wrapping, clamping, gradients) is centralised
there.

`grid::Field<T>` is a row-major dense array bound to a grid. `T` implements
`FieldScalar` for `f64`, `f32`, `u8`, `u16`, `i32`. Precision is chosen per
quantity: elevation/crust/discharge are `f64`; climate, sediment, soil, ice,
rates and display overlays are `f32`; categorical layers are `u8`/`u16`.

### Params and the schema

Each subsystem owns a parameter struct implementing `params::ParamBlock`:

```rust
fn specs() -> &'static [ParamSpec];
fn values_into(&self, out: &mut Vec<f64>);
fn set_values(&mut self, values: &[f64]);
fn set_key(&mut self, key: &str, value: f64) -> bool;
fn get_key(&self, key: &str) -> Option<f64>;
// optional overrides: normalize(), validate(), randomize()
```

`params::Params` aggregates the blocks. Schema order and the flat `f64`
encoding are the same order, so `Params::values()` / `Params::from_values()`
round-trip exactly (tested). `Params::schema_json()` and
`Params::presets_json()` are served through the WASM ABI; the UI builds its
entire parameter panel from them. Presets are data-only override lists
(`params/presets.rs`).

### World

`World` owns every spatially varying quantity: surface elevation, crustal
thickness, sediment, soil, ice, cumulative erosion/deposition, current tectonic
rate, sea level and its base datum, elapsed time and the Airy anchor
(`crust_reference_m`). Processes receive `&mut World` and mutate the parts they
own.

### Simulation

`sim::Simulation` owns `Params`, `World`, `PlateMosaic`, `Geology`, the latest
`Climate` and `Hydro`, `SimState` (elapsed time, iteration, current step,
ledger, sea level, finished flag), and `History` (timeline frames).

## 4. Generation pipeline (`pipeline.rs`)

1. Build the plate mosaic (Voronoi plates with velocities and crustal
   properties).
2. Generate a rank-normalised low-frequency *craton field*, biased by plate
   type and smoothed; map it to crustal thickness (7 km oceanic → plate
   thickness continental) and smooth again (shelves/slopes).
3. Airy isostasy, including the water-load correction in the ocean branch,
   gives the base elevation.
4. Add fractal relief (stronger on continents), folds (gated to orogenic
   belts), fault offsets on boundaries, smoothed orogen/trench belts (rates
   integrated over a reference span), hotspot swells and landform stamps.
5. Clamp, repair, apply a conservative safety correction only for pathological
   all-ocean/all-land seeds.
6. Anchor crustal thickness to the final relief (`uplift::anchor_crust`) so
   isostasy relaxes *changes* instead of flattening the generated landscape.
7. Build the stratigraphic columns and exposed-rock maps (`Geology::build`).

The initial climate and hydrology are computed by `Simulation::new`.

## 5. Time loop and scheduling

`Simulation::step`:

1. Update sea level (eustatic cycle from the climate period and amplitude, plus
   the configured trend).
2. Advance plates and rebuild the mosaic every ~100 kyr (rebuild is
   `O(cells × plates)`).
3. Apply tectonic vertical motion + isostatic relaxation (`uplift::apply`).
4. Refresh climate every ~40 kyr and hydrology every ~20 kyr (both are
   expensive; the landscape changes slowly between updates).
5. Apply the process chain for this step:
   fluvial incision → hillslope diffusion/landslides/soil → glacial (own
   interval) → aeolian (own interval) → coastal → chemical.
6. Thin the crust by the eroded depth (isostatic rebound feedback), recompute
   the exposed strata/erodibility, repair non-finite states, update counters
   and the mass ledger.
7. Adaptive step: halve `dt` when a step's ledger closure error exceeds the
   tolerance, recover slowly otherwise.
8. Capture a timeline frame when the next frame time is crossed.

Processes are invoked every base step unless their own interval (in years)
says otherwise; those receive `dt × interval_steps`. The end of the run is
`total_yr` or `max_iters`.

## 6. History and arbitrary-time seeking

Each frame stores the surface as `f32`, sea level, time and a small summary
(min/max/mean/land fraction). The frame budget is derived from a 64 MiB cap and
`history_frames`.

`Simulation::seek(year)`:

- forward: step until elapsed ≥ target, finishing with a sub-step of exactly
  the remaining interval so the state lands on the requested year;
- backward: rebuild the deterministic world from the parameters *while keeping
  the existing frames* (`reset_preserving_history`), then replay forward.

The engine's determinism makes replayed states exactly equal to a straight
run (tested). The Web Worker implements the same logic in chunks and emits
progress messages, so the UI stays responsive and can show the nearest cached
frame immediately while the exact replay runs.

## 6b. Tiled generation (4096²)

`tiling.rs` implements the Phase 3 large-grid path: a coarse 1/8-resolution
anchor pass (plate mosaic, craton rank CDF, smoothed tectonic rates, landform
stamps) followed by halo-tiled evaluation of the real relief phases. Tiles are
independent given the anchor, so they run on a `std::thread::scope` pool; folding
into statistics/preview/exports happens in plan order, which makes the result
bit-identical for any worker count. Exports are streamed (`export/tiled.rs`), so
no full-resolution buffer is ever allocated. CLI: `geogenesis generate --tiled
--grid 4096` or `geogenesis tiles --run --out DIR`; the WASM/UI path intentionally
caps interactive worlds at 2048². Measured 4096²: 3.4–3.8 s / 123–209 MB.

## 6c. Process plugins

`plugin.rs` exposes a `ProcessPlugin` trait (`apply(&self, ctx: &mut
ProcessContext) -> MassBalance`) and `Simulation::with_plugins`. Plugins run
once per step after the built-in chain; their volumes join the ledger; resets
and backward seeks preserve the plugin list. A deterministic meteor-impact demo
(`--plugin impacts`) and an out-of-tree example crate (`crates/plugin-example`)
show the extension path. Details: [PLUGINS.md](PLUGINS.md).

## 7. WASM ABI (`crates/wasm/src/lib.rs`)

A single global simulation behind a `Mutex`, no wasm-bindgen. Functions are
`gg_*`; fallible ones return an `i32` status and expose the message through
`gg_last_error_ptr`. Buffers returned by `gg_*_ptr` are engine-owned
`boxed_slice` allocations; the JS side copies them out and calls
`gg_free(ptr, byte_len)` exactly once. Parameters are a flat `Float64Array` in
schema order. The ABI also exposes the schema, presets, biome/rock catalogues,
fields (with slope computed on demand), history frames, cross-sections,
statistics, state JSON and OBJ/XYZ exports.

Low-resolution parameter previews go through `gg_preview`, which runs
`pipeline::generate_preview` into a separate engine-owned buffer
(`gg_preview_ptr` + `gg_preview_info_ptr`) without touching the live
simulation, so dragging a slider can show a real coarse world immediately.

Binary assets are produced by `gg_export_ptr(kind)`: 16-bit heightmap PNG,
normal map, glTF, cross-section PNG, EXR and NetCDF. Mineral results are
available through `gg_minerals_json_ptr` for the UI inventory panel, and
`gg_create_with_geology` accepts user-defined `RockSet`/`StrataSpec` JSON for
custom geology.

The Worker (`web/src/engine/worker.ts`) owns the only engine instance and
serialises requests; `web/src/engine/wasm.ts` implements the typed wrapper and
the buffer-copy discipline.

## 8. Web UI

- `web/src/main.ts` — app shell: boots the worker, builds the panels, wires
  callbacks (generate, step, run, seek, mode overlays, profile line, export),
  playback loop, keyboard shortcuts, localStorage persistence. Parameter edits
  are debounced into a 96-cell preview request while in terrain mode; Generate
  still builds the full-resolution world.
- `web/src/engine/` — loader (`wasm.ts`) and Worker protocol implementation.
- `web/src/types.ts` — shared types and the Worker protocol.
- `web/src/colors.ts` — colour ramps shared by the renderer, minimap and legend.
- `web/src/view/terrain.ts` — three.js renderer: height-mapped mesh, vertex
  colours per overlay mode, water, lighting, orbit camera, minimap and
  cross-section interaction.
- `web/src/ui/` — schema-driven parameter panel, timeline, statistics, legend,
  cross-section panel, export menu and formatting helpers.
- `web/src/styles.css`, `web/index.html` — layout and theme.

## 9. Extension guide

**Add a parameter:** add a field to the subsystem's struct and a matching
`ParamSpec` in its `specs()` table, then extend `values_into`, `set_values`,
`set_key`, `get_key` in the same file. Update the block's `Default` and,
optionally, `randomize`. The UI, CLI and WASM ABI pick it up automatically;
presets can override it by key.

**Add a process:** create a module under `crates/core/src/erosion/` (or a new
subsystem), give it `params` block, a state struct if needed, and
`pub fn apply(...) -> MassBalance`; schedule it from `sim::step`; add it to
`erosion::apply_all` if it belongs to the process chain; expose any display
field through `sim::FieldKind` and the WASM field ids. Add conservation and
stability tests.

**Add a preset:** one `Preset` entry with `(key, value)` overrides in
`params/presets.rs`.

**Add a process as a plugin (without forking):** implement
`plugin::ProcessPlugin` in your own crate, register it with
`Simulation::with_plugins`, and optionally expose a CLI
`--plugin` name by adding to `plugin::builtin`. See
[docs/PLUGINS.md](PLUGINS.md). Plugins run after the built-in chain, return a
`MassBalance`, are preserved across resets/seeks and must be deterministic.

## 10. Verification and tooling

- `cargo test --workspace` — 200+ unit/integration tests: determinism,
  reproducibility across seeds, numerical stability with adversarial
  parameters, mass-balance closure, climate/erosion invariants, schema
  round-trips, export format sanity.
- `scripts/smoke-wasm.mjs` — loads the real WASM module in Node, drives the C
  ABI, checks determinism, buffers, history, seeking, stats, profiles, exports
  and every preset.
- `scripts/ui-shot.mjs` — Chrome DevTools-protocol screenshot/status probe for
  end-to-end UI verification.
- `cargo clippy --workspace` / `cargo fmt --all -- --check`.
- `geogenesis bench` — reports generation and per-step timings by grid size.

## 11. Build and release

- `scripts/build-wasm.sh` builds the release WASM (LTO, `opt-level=3`,
  `panic=abort`, no dependencies) and copies it to `web/public/engine.wasm`.
- `cargo build -p geogenesis-cli --release` produces the native CLI.
- `cd web && npm run build` produces `web/dist` (static, deploy anywhere).
- The release profile uses `lto = "fat"`, `codegen-units = 1`,
  `panic = "abort"` and symbol stripping.
