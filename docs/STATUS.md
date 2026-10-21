# GeoGenesis status

Living status against `SPEC.md` and the project objective. Updated as work
lands; see the linked documents for evidence.

## Objective checklist

| area | status | evidence |
| --- | --- | --- |
| Plate tectonics, crust, volcanism | done | `plates.rs`, `uplift.rs`, 30+ tests, `PHYSICS.md §1` |
| Erosion, deposition, sediment | done | `erosion/*`, ledger closure tests, `PHYSICS.md §5–7` |
| Climate and water cycle | done | `climate/mod.rs`, `hydrology/mod.rs`, `PHYSICS.md §3–4` |
| Geology / stratigraphy | done | `geology.rs`, dip/strike exposure tests |
| User-defined rocks + strata | done | `rocks.rs`, JSON round-trip, `--rocks`/`--strata`, `examples/` |
| Mineral/resource deposits | done | `minerals.rs` (14 minerals), `minerals.png/json`, CLI + UI wiring |
| Spherical per-cell areas | done | exact latitude-band areas threaded through hydrology/stats/climate/ledger |
| Fully implicit depositional laws | done | upstream implicit Newton/exponential settling; analytic steady states |
| Plugins (user-defined processes) | done | `plugin.rs`, `--plugin impacts`, `crates/plugin-example` |
| 4096² tiled generation | done | 3.4–3.8 s / 123–209 MB, bit-exact seams, worker invariant |
| Time evolution, history, seeking | done | `sim/mod.rs`; exact-year seek test; UI timeline |
| 3D terrain view | done | GPU renderer, screenshots in `README.md` |
| Full parameter editing | done | 151 schema-driven parameters, presets, search |
| Auto-generation / proposals | done | `Params::random`, `geogenesis explore` seed scoring |
| Timeline play / seek | done | worker chunked run/seek, UI transport |
| Cross-section, statistics | done | profile canvas + `stats.rs`, profile PNG export |
| Exports | mostly done | PNG (8/16-bit), normal maps, cross-section, f32, OBJ, glTF, XYZ, JSON |
| Tests / benchmarks / docs | done | 321 core + 14 independent validation + 3 plugin + 5 WASM ABI tests; `BENCHMARKS.md`, `VALIDATION.md`, `verify.sh` |
| 512² / 1024² generation targets | met | 420 ms / 1.82 s (`BENCHMARKS.md`) |
| WASM < 2 MB gzip | met | 216 KB gzip (602 KB raw) |
| UI parameter preview < 16 ms | met | 7.6 ms generation (96²), `bench` |
| 4096² < 30 s / < 1 GB | **met** | production tiled generation: 3.4–3.8 s / 123–209 MB (`generate --tiled`, `tiles --run`); 512² 0.19 s / 14 MB, 1024² 0.33 s / 39 MB |
| EXR / NetCDF export | done | dependency-free EXR (FLOAT/HALF) + NetCDF-3 classic writers; `write_all` emits `terrain.exr` + `fields.nc`; external validation with OpenEXR/ffmpeg/SciPy |
| Container/plugin extensibility | done | compile-time `ProcessPlugin` API, built-in impacts demo, out-of-tree example crate (`docs/PLUGINS.md`) |
| Custom rocks / strata (core + CLI + JSON) | done | `rocks.rs`, `--rocks/--strata`, `examples/*.json` |
| Mineral deposits (core + CLI export) | done | `minerals.rs`, `minerals.png`/`.json`, 14 minerals, inventory |
| Web UI editor for custom rocks/minerals overlay | pending | follow-up UI work |
| Real-GPU 60 fps measurement | **met** | 59.9 fps idle / 59.8 playback at 384² (294k tris), 59.0/57.6 at 512² on AMD Radeon iGPU via ANGLE (hardware-GL headless); see `docs/screenshots/hardware-fps.md` |

_Last full verification (`./scripts/verify.sh`, final): fmt/clippy clean,
**321 core + 14 validation + 3 plugin + 5 WASM tests**, WASM ABI smoke, web build, 256²/512² benchmarks,
96² preview 7.3 ms, 4096² tiled generation 3.4–3.8 s, headless UI screenshot —
all green. This is the state of `HEAD`._

## Verification commands

```bash
cargo test --workspace           # 321 core + 14 validation + 3 plugin + 5 WASM
./scripts/smoke-wasm.mjs         # C ABI contract
./scripts/verify.sh              # fmt, clippy, tests, wasm, web build, bench, UI screenshot
./target/release/geogenesis bench --grid 256 --steps 10
./target/release/geogenesis tiles --grid 4096 --run --out out/tiled4096
make tiles                      # same, with plan, memory budget and preview PNG
```

## Next work items

Detailed plan and acceptance criteria: [ROADMAP.md](ROADMAP.md).

1. Production tiled pipeline: real plate mosaic, fold/fault/smoothing and
   climate evaluation per tile, streamed PNG/OBJ/statistics output, then
   (done: 3.4–3.8 s / 123–209 MB).
2. EXR and NetCDF writers.
3. Fully implicit depositional laws; spherical per-cell area correction.
4. Custom rock definitions, mineral deposits and a stratigraphy UI.
5. Real-GPU performance measurement and tuning.
