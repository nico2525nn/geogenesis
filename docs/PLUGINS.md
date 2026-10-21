# Process plugins

GeoGenesis can run **user-defined processes** alongside the built-in erosion,
tectonics and climate chain. This is a compile-time Rust extension point: the
engine is dependency-free and deterministic, and plugins are ordinary Rust
crates that implement a small trait. That keeps the WebAssembly build, browser
UI and CLI free of dynamic loading, while still letting users extend the
simulation without changing engine code.

## API

```rust
use geogenesis_core::erosion::MassBalance;
use geogenesis_core::plugin::{ProcessContext, ProcessPlugin};

#[derive(Debug)]
struct MyProcess { /* configuration */ }

impl ProcessPlugin for MyProcess {
    fn name(&self) -> &'static str { "my-process" }
    fn description(&self) -> &'static str { "one-line description" }

    fn apply(&self, ctx: &mut ProcessContext<'_>) -> MassBalance {
        // ctx.world        &mut World        elevation, crust, sediment, ...
        // ctx.params       &Params
        // ctx.climate      Option<&Climate>
        // ctx.hydro        &Hydro            flow routing / discharge
        // ctx.geology      &Geology          strata, erodibility
        // ctx.dt_yr        f64               this step in years
        // ctx.elapsed_yr   f64               simulated time at step start
        // ctx.grid         Grid
        MassBalance::default()
    }
}
```

Register plugins when constructing the simulation:

```rust
use geogenesis_core::params::Params;
use geogenesis_core::sim::Simulation;

let params = Params::default();
let mut sim = Simulation::with_plugins(
    &params,
    vec![Box::new(MyProcess { /* ... */ })],
)?;
sim.run();
```

Or add one to a running simulation with `sim.add_plugin(Box::new(...))` and
inspect them with `sim.plugins()`.

## Semantics and requirements

- **Once per step**, after the built-in process chain. A plugin can schedule its
  own interval with `ctx.elapsed_yr` and `ctx.dt_yr` (see the impact plugin).
- **Determinism is mandatory.** A plugin may only use its immutable
  configuration, the context, and deterministic hashes of the seed (`crate::math::rng`).
  Never use clocks, entropy, thread scheduling or unordered iteration.
- **Mass balance is the contract.** Return the volume you moved (`erosion_m3`),
  the volume redeposited (`deposition_m3`), and the volume that left the domain
  (`export_m3`). The driver folds this into the global ledger, which is checked
  each step; returning inconsistent volumes shows up as a closure error.
- **Keep the isostatic feedback working** by updating `world.erosion_m` /
  `world.deposition_m` for the cells you move (the engine thins the crust from
  the cumulative erosion field).
- **Reset and seek preserve plugins.** `reset`, `reset_preserving_history` and
  backward seeks rebuild the world from the parameters and keep the plugin list,
  so deterministic plugins reproduce the same terrain.
- **Performance**: plugins run on the simulation thread once per step. Keep them
  O(cells) per step and avoid allocations; use the existing scratch-buffer
  helpers where useful.
- **Memory** at 4096² matters: do not allocate full-grid scratch inside `apply`.

## Built-in demo: meteor impacts

`crate::plugin::ImpactPlugin` is a state-free deterministic example that fires
rare impacts, excavates a parabolic crater and redeposits part of the ejecta on
the rim (exact ledger). Enable it from the CLI:

```bash
geogenesis generate --grid 384 --plugin impacts --run
geogenesis bench --grid 256 --plugin impacts --steps 20
```

The schedule is `(k + hash(seed, k)) * mean_interval_yr` with a default mean
interval of 4 Myr and 12 km craters. It is a demonstration, not a calibrated
impact flux model.

## Out-of-tree example crate

`crates/plugin-example` is a separate crate (also a workspace member for CI) that
implements a conservative pairwise thermal diffusion process. It shows the full
lifecycle: mutation, cumulative erosion/deposition bookkeeping, exact mass
balance, determinism and tests. Copy it as a starting point for your own crate:

```toml
[dependencies]
geogenesis-core = { path = "../GeoGenesis/crates/core" }
```

## Limitations

- Plugins are a **native/CLI** feature. The WebAssembly module is fixed at build
  time, so the browser UI does not load third-party plugins. Running the page
  server-side or building a custom WASM module with your plugin is the intended
  path.
- No per-plugin scheduling API yet: use `ctx.elapsed_yr` to implement intervals
  (the built-in impact plugin does this).
- A plugin cannot add new schema parameters automatically; expose configuration
  in your crate and pass it when constructing the plugin (or load it from your
  own file in a wrapper binary).
