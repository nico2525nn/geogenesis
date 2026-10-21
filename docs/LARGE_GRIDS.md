# Large grids (Phase 3): measured baseline and tiling plan

SPEC §13 asks for 4096² generation in < 30 s and < 1 GB. This document records
the measured single-shot baseline and the design for the tiled path.

## Measured single-shot baseline

`generate` (no simulation steps) on the development machine, release build:

| grid | generation | peak RSS |
| --- | --- | --- |
| 1024² | 1.78 s | 238 MB |
| 2048² | 8.44 s | 775 MB |
| 4096² (opt-in `--allow-large-grid`) | **37.5 s** | **3.1 GB** |

Single-shot 4096² was measured with the opt-in `--allow-large-grid` flag
(there is a default 2048² guard so an accidental UI/CLI request cannot exhaust
memory): it completes in 37.5 s but needs 3.1 GB, i.e. it is close to the time
target and 3× over the memory target. **Memory is the binding constraint**, which
is what the tiled path below solves. The cost is the number of full-size fields: elevation/crust
(`f64`), ~10 `f32` surface fields, plate rate fields, climate outputs, hydrology
routing arrays (`u32` receivers/order, `f64` area/discharge) and geology.

## Tiled design

1. **Global low-resolution anchor pass** (1/8 resolution, e.g. 512²): craton
   field, continental mask ordering, plate mosaic, climate zone coefficients,
   sea-level datum. These are the only globally coupled quantities; keeping
   them at low resolution costs ~15 MB.
2. **Tile passes with halo**: each tile (e.g. 512² with a 32-cell halo) evaluates
   local operations by *sampling* the global anchor fields bilinearly:
   plate Voronoi/velocity (point queries), fractal noise (integer frequency
   ratios make sampling position-independent), folds/faults/stamps. Smoothing
   and the Euclidean distance transform run on the halo so seams match.
3. **Streamed outputs**: tiles are consumed immediately for PNG rows, OBJ/glTF
   chunks, statistics accumulators and 16-bit heightmaps; only the accumulator
   fields (min/max/histogram, optional downsampled map for the UI) stay in
   memory. No full-size `World` is ever allocated.
4. **Simulation**: the time loop stays at the 1024² cap; 4096² is a
   generation/export mode (a `--grid` high-resolution still image). Interactive
   evolution at >1024² needs the tiled hydration/erosion scheme as a follow-up.
5. **Parallelism**: tiles are independent given the anchor pass, so they map
   directly onto a worker pool (`std::thread::scope`); determinism is preserved
   because each tile only reads immutable anchors plus its own buffers.

Memory estimate for 4096²: anchor pass ~15 MB + 4 concurrent 512² tiles with
halo (~30 MB each with scratch) + streamed accumulators (< 100 MB) → substantially
below the 1 GB budget.

## Status: production implementation and measurements

`crates/core/src/tiling.rs` implements the full streaming pipeline: a coarse
1/8-resolution anchor pass (plate mosaic, craton rank CDF, orogen/trench/hotspot
rate fields blurred at fine physical widths, datum safety net, landform stamps)
and per-tile evaluation of the real relief phases (craton noise at global
coordinates → rank CDF → plate bias → halo Gaussian blur → smoothstep mask →
crust → Airy isostasy → integer-frequency fractal relief → folds/faults/
orogen/trench/hotspot/stamps → envelope and datum clamp). `export/tiled.rs`
streams PNG (8/16-bit) and raw `f32` rasters through the same pass, so no
full-resolution buffer is ever allocated.

Commands:

```sh
# main command: terrain-only tiled generation (time evolution stays <=2048²)
geogenesis generate --tiled --grid 4096 --out out/tiled-world

# tiled command: plan/memory budget, optional generation + exports
geogenesis tiles --grid 4096 --workers 8 --run --out out/tiled4096
```

Measured on the development machine (16 cores, release build, `VmHWM`):

| workload | time | peak RSS |
| --- | --- | --- |
| 4096² via `tiles`/`generate --tiled` (default extent, ~0.94 km cells) | **3.4–3.8 s** | **123–209 MB** |
| 4096² example with single-shot comparison | 6.15 s | 207 MB |
| 1024² | 0.33 s | 39 MB |
| 512² | 0.19 s | 14 MB |

Timing breakdown at 4096²: anchor 0.32 s, tiles 2.39 s, streamed exports
3.44 s. SPEC §13 targets (< 30 s, < 1 GB) are met with a wide margin on both
axes, including on many-core machines (worker count is capped so tile buffers
stay below 512 MB).

### Quality and determinism

- **Seams are bit-exact**: every core cell of all tiles of a 256² world is
  bit-identical to a single tile covering the whole world (unit test), thanks
  to halo padding and pure global-coordinate evaluation.
- **Worker-count invariant**: 1 vs 4 workers give bit-identical preview,
  histogram and statistics (folding always happens in plan order).
- **Agreement with single-shot**: land fraction within 0.009 and mean elevation
  within ~5 % of the vertical envelope at 512²/1024²; the base construction is
  essentially exact when tectonic terms are disabled. Remaining differences
  come from the 1/8 anchor sampling of smooth tectonic fields: features
  narrower than ~2 anchor cells are smoother, and trench minima are typically
  ~200–400 m shallower than single-shot at high resolution.
- Deterministic, finite, no unsafe, no new dependencies.

### Exports for large grids

`tiles --run --out DIR` writes `terrain.png`, `height16.png`, `normals.png`,
`elevation.f32`, `preview.png` and `stats.json` (streamed; largest buffer is
one tile-row band). Mesh formats (OBJ/glTF) are intentionally not streamed for
4096² — use the rasters/EXR/NetCDF at that size and OBJ/glTF for ≤2048² worlds.

### Remaining non-goals

- Interactive time evolution above the single-shot 2048² guard; the streaming
  path is a generation/export mode.
- OBJ/glTF chunk writers for tiled worlds (see above).
- Higher-resolution anchors for sharper trench detail (optional refinement).
