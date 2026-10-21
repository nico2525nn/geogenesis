# Contributing to GeoGenesis

Thanks for helping improve GeoGenesis! This project is a deterministic
geological simulation engine plus a browser UI, so contributions should keep
two things sacred: **determinism** and **mass conservation**.

## Getting set up

```bash
rustup target add wasm32-unknown-unknown
./scripts/build-wasm.sh
cd web && npm ci && cd ..
make dev                  # http://localhost:4000
make test                 # Rust tests + WASM ABI smoke test
make verify               # everything: fmt, clippy, tests, wasm, web, bench, UI
```

Requirements: Rust 1.82+, Node 20+, and (optionally) `make`.

## Ground rules

- **No new dependencies in `crates/core` and `crates/wasm`.** The engine ships
  as a small, dependency-free WebAssembly module; adding a crate there needs a
  very strong reason and a size/performance justification.
- **No `unsafe`** outside `crates/wasm/src/lib.rs`, where it is confined to the
  C-ABI boundary and documented per block.
- **Determinism is a contract.** A world is a pure function of `Params` and the
  step count. Use `crate::math::rng` with domain tags; no clocks, OS entropy or
  thread-count-dependent ordering.
- **Physics changes need a mass ledger.** Processes return a `MassBalance`;
  the driver checks closure every step. Add/extend tests that assert closure
  and stability at absurd time steps.
- **Schema changes propagate automatically** to the CLI, the WASM ABI and the
  Web UI. Add a `ParamSpec` row, value plumbing and preset coverage.
- Keep `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings`
  clean. Tests must stay fast (the full suite is ~15 s debug).

## Where things live

| path | contents |
| --- | --- |
| `crates/core/src/pipeline.rs` | generation pipeline (plates → relief → climate) |
| `crates/core/src/sim/mod.rs` | time loop, schedule, history, seeking, plugins |
| `crates/core/src/erosion/` | fluvial, hillslope, glacial, aeolian, coastal, chemical |
| `crates/core/src/params/` | schema and preset data (single source of truth) |
| `crates/wasm/src/lib.rs` | C ABI (buffer protocol documented in the header) |
| `web/src/engine/` | WASM loader + Worker protocol |
| `web/src/view/` | three.js renderer and minimap |
| `web/src/ui/` | schema-driven panels, timeline, statistics, export |
| `docs/` | architecture, physics, benchmarks, plugins, roadmap, validation |

Useful docs: [ARCHITECTURE](docs/ARCHITECTURE.md), [PHYSICS](docs/PHYSICS.md),
[PLUGINS](docs/PLUGINS.md), [ROADMAP](docs/ROADMAP.md).

## Pull-request checklist

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `.scripts/build-wasm.sh` + `node scripts/smoke-wasm.mjs` (if the ABI or
      core behaviour changed)
- [ ] `cd web && npm run check && npm run build` (if the UI changed)
- [ ] New visual/behaviour evidence (screenshots for UI changes)
- [ ] Benchmarks reported if the change affects performance-sensitive paths
- [ ] Docs/roadmap updated if scope changes

## Reporting bugs

Please include: exact command (CLI) or preset/seed/grid/parameters (UI), the
engine version (`geogenesis --version`), expected vs actual behaviour, and a
minimal reproduction. For terrain-quality issues, a screenshot and the exported
`params.json` / `stats.json` help enormously.
