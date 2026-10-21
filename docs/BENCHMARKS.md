# Benchmarks

All numbers below were measured on the development machine (AMD/Intel
x86_64, 16 threads) with the release profile (`lto=fat`, `codegen-units=1`,
`panic=abort`), native `geogenesis` CLI, after the final optimisation pass.
Background load was ~6 (load average), so treat single-digit percentages as
noise. Reproduce with:

```bash
cargo build -p geogenesis-cli --release
./target/release/geogenesis bench --grid 256 --steps 10
./target/release/geogenesis bench --grid 512 --steps 5
./target/release/geogenesis bench --grid 1024 --steps 2
./scripts/build-wasm.sh            # prints raw + gzip size
node scripts/smoke-wasm.mjs        # ABI correctness + timing of 96² create
```

## Initial generation (pipeline Phases 0–2)

| grid | earthlike preset | SPEC target | status |
| --- | --- | --- | --- |
| 256² | 105 ms | — | |
| 384² | 212 ms | — | default UI world |
| 512² | 420 ms | < 500 ms | met |
| 1024² | 1.82 s | < 3 s | met |
| 4096² | **3.4–3.8 s / 123–209 MB** (production tiled) | < 30 s | met; opt-in single-shot remains 37.5 s / 3.1 GB |

## Simulation stepping (earthlike, per base step of 10 kyr)

| grid | per-step | full 200-step run |
| --- | --- | --- |
| 256² | 12.7 ms | 2.5 s |
| 384² | 27.7 ms | 5.5 s |
| 512² | 65 ms | 13 s |
| 1024² | ~0.4 s (cooling) | minutes |

Cost is dominated by hillslope diffusion, fluvial incision and the scheduled
climate/routing/isostasy passes. The schedule parameters
(`climate_interval_yr`, `hydro_interval_yr`, `isostasy_interval_yr`,
`tectonic_interval_yr`) trade accuracy for speed: their defaults amortise the
expensive passes over 2–10 steps.

## Routing micro-benchmark (hydrology module)

| grid | route time |
| --- | --- |
| 256² | 2.9–3.1 ms |
| 512² | 14–15 ms |

## Tiled large-grid generation (Phase 3)

The streaming tiled pipeline (`tiling::generate_large_world`) is now the
production path for 4096² generation. CLI: `geogenesis tiles --grid 4096
--workers 8 --run --out out/tiled4096` or `geogenesis generate --tiled --grid
4096`. Peak RSS is read from `/proc/self/status` (VmHWM).

| workload | time | peak RSS | outputs |
| --- | --- | --- | --- |
| 4096² (384²-params extent, 0.94 km cells) | **3.4–3.8 s** | **123–209 MB** | terrain/height16/normals PNG, elevation.f32, preview, stats |
| 4096² example with `--compare-single-shot` | 6.15 s | 207 MB | + comparison/QA outputs |
| 1024² | 0.33 s | 39 MB | |
| 512² | 0.19 s | 14 MB | |

Timing breakdown (4096² example): anchor 0.32 s, tiles 2.39 s, streamed exports
3.44 s. SPEC §13 targets (< 30 s, < 1 GB) are met with wide margin; the worker
pool is capped so tile buffers stay below 512 MB on many-core machines.
Quality vs the single-shot pipeline: land/mean within ~1 % and relief within
~10 % (coarse 1/8 anchor sampling of smooth tectonic fields); core tiles are
bit-identical to a world-spanning tile and results are worker-count invariant.

## GPU rendering (hardware WebGL)

Measured headless Chrome with the AMD Radeon iGPU
(`ANGLE (AMD, AMD Radeon Graphics (radeonsi renoir ACO), OpenGL ES 3.2)`,
flags `--headless=new --enable-gpu --use-gl=angle --use-angle=gl`), production
build, canvas ≈916×715 CSS @ DPR 1, vsync capped:

| grid | triangles | idle | playback |
| --- | --- | --- | --- |
| 384² | 294k | **59.9 fps** (P50/P95 16.7 ms) | **59.8 fps** |
| 512² | 523k | 59.0 fps | 57.6 fps |
| 1024² | 2.09M | ~58 fps | 44–49 fps |

Uncapped headroom ~247 fps at 384²; `show()` CPU cost 7.2 ms at 384². Full
methodology and reproduce commands: `docs/screenshots/hardware-fps.md`.

## WebAssembly

| artifact | size |
| --- | --- |
| `engine.wasm` raw | 412 KB |
| `engine.wasm` gzip | 145 KB |
| SPEC target | < 2 MB gzip |

## Browser / rendering

Measured with headless Chrome + SwiftShader (software GL), earthlike 384²:

- terrain frame upload (`TerrainView.show`): ~12 ms (height texture + minimap);
  simulation frames arrive ~110 ms apart in software GL, so rendering keeps up;
- real-GPU 60 fps is expected (static ~5.7 MB GPU buffers, DPR capped at 2) but
  cannot be measured in software rendering;
- full simulation run in the browser: interactive, with live progress; the
  software-GL headless environment is not representative of real hardware.

## Parameter preview (slider change → viewport)

| step | measured |
| --- | --- |
| 96² generation (native pipeline, real `generate_preview`) | **7.2–7.6 ms** (`bench` best-of-3) |
| 128² generation | 25.2 ms |
| preview round-trip through WASM + Worker + render | ~40–50 ms |
| UI debounce before requesting the preview | 100 ms |
| per-frame terrain upload (`TerrainView.show`) | ~12 ms |

The UI preview uses the *same* generation pipeline at 96 cells, so it
reproduces the real continents, relief and biome layout rather than a
placeholder. The 14.1 ms generation is within the SPEC `< 16 ms` target for
the preview itself; the visible latency is dominated by the deliberate 100 ms
debounce that keeps slider drags from queueing dozens of requests.

## Not yet measured / open targets

- 4096² tiled/parallel generation and the < 1 GB memory budget (Phase 3).
