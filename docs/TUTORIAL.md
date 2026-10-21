# GeoGenesis tutorial

A hands-on introduction: from a clean checkout to a world you can explore,
evolve, slice and export.

## 1. Setup

Requirements: Rust stable (1.82+), Node 20+, the `wasm32-unknown-unknown`
target, and `make` (optional).

```bash
rustup target add wasm32-unknown-unknown
cd web && npm ci && cd ..
./scripts/build-wasm.sh     # compiles the engine to web/public/engine.wasm
./scripts/verify.sh         # optional: full test + benchmark + UI check
```

## 2. Run the Web UI

```bash
make dev        # development server on http://localhost:4000
# or a production build
make web        # writes web/dist
cd web && npm run preview -- --port 4173
```

The app boots the engine as WebAssembly inside a Worker, generates the default
Earth-like world and renders it as an interactive 3D terrain.

### Generate a world

1. Pick a **preset** (left panel): `Earth-like`, `Archipelago`, `Icehouse`,
   `Volcanic`, `Riverworld`, … Each preset is a set of parameter overrides; the
   description under the select explains the archetype.
2. Tweak parameters. The list is generated from the engine schema — every
   parameter has units, a range, help text, per-row reset and a "modified"
   marker. While you edit, the viewport shows a low-resolution preview of the
   same pipeline; the full world is rebuilt only when you press **Generate**.
3. Use the seed field or the dice to explore different worlds with identical
   parameters. **Randomize** creates a coherent random parameter set.

### Explore the terrain

- **View** dropdown: 20 overlays — terrain shading, biomes, temperature,
  precipitation, vegetation, snow, slope, rock, erodibility, sediment, soil,
  ice, erosion rate, rivers, lakes, tectonic uplift, aridity and the three
  mineral modes (dominant mineral, prospectivity richness, economic deposits).
- **Relief − / +**: vertical exaggeration (10–120).
- **Minimap**: drag the gold line to draw a cross-section anywhere; click/drag
  the handles or the line itself. The cross-section panel shows terrain, sea
  level, water and the biome strip.
- **Legend** (top-right of the viewport): ramp or categorical palette for the
  current mode.
- **Statistics** (right panel): terrain distribution, land/ocean ratio, slope
  and hydrology, climate, mass-balance ledger, biome composition, hypsometric
  curve and a **Minerals** card with deposit area, estimated value and the top
  deposit types. Values update live during playback.
- **FPS** is shown in the HUD (green ≥55, amber 30–54, red <30).
- **Suggest** proposes a deterministic candidate world using the
  low-resolution preview pipeline (apply it to the parameter panel, then
  Generate).
- **Geology…** loads user-defined rock and strata JSON files (see
  `examples/custom-rocks.json` and `examples/strata.json`) and applies them on
  the next Generate.

### Time

The timeline controls geological time:

- **Run** evolves the world to the configured duration (chunked in the Worker,
  with progress).
- **Play/pause** streams frames at your chosen speed; the slider scrubs
  through captured history frames; dragging requests an exact seek.
- **Jump** accepts `500k`, `2.5Myr`, `1.2e6`, etc. Seeks are exact: the engine
  replays deterministically and lands on the requested year.
- Keyboard: `Space` play/pause, `←/→` step, `Shift+←/→` jump, `Home/End`
  start/end, `G` generate, `R` randomize, `/` focus the parameter filter.

### Project save/load

Save project writes a JSON file with the resolved parameters, preset/seed,
overlay mode and current simulated time. Load applies the parameters, generates
the world (including custom geology when present) and seeks back to the saved
time. Use it to archive a world or share a reproducible setup.

### Export

The **Export** menu saves: viewport PNG, 16-bit heightmap PNG, normal map,
cross-section PNG, OBJ mesh, self-contained glTF, XYZ point cloud, OpenEXR
heightmap, NetCDF gridded fields, raw heights (`elevation.f32`), project JSON,
parameters JSON and statistics JSON. The CLI writes the same set (plus the
mineral map/inventory and biome/river maps).

## 3. Use the CLI

```bash
cargo build -p geogenesis-cli --release
./target/release/geogenesis --help

# A quick world with every export
./target/release/geogenesis generate --preset earthlike --seed 42 --grid 384

# Evolve it 2 Myr in 10 kyr steps
./target/release/geogenesis generate --grid 384 --run

# Custom overrides (any schema key)
./target/release/geogenesis generate --set precipitation_mm=2400 --set kf=9e-5

# Performance
./target/release/geogenesis bench --grid 256 --steps 10
./target/release/geogenesis bench --grid 512 --steps 5

# Seed discovery: score many seeds and propose interesting worlds
./target/release/geogenesis explore --preset earthlike --seed 1 --steps 24 --grid 96

# Parameter/seed sweeps
./target/release/geogenesis sweep --seed 1 --steps 8 --grid 128

# User-defined rocks + strata stack (JSON files in examples/)
./target/release/geogenesis generate --grid 384 \
  --rocks examples/custom-rocks.json --strata examples/strata.json

# Custom process plugins (built-in demo: deterministic meteor impacts)
./target/release/geogenesis generate --grid 384 --plugin impacts --run

# Tiled 4096² generation (3.4–3.8 s, <1 GB; streamed rasters)
./target/release/geogenesis generate --tiled --grid 4096

# Tiled large-grid generation (plans + memory budget + streamed exports)
./target/release/geogenesis tiles --grid 4096 --workers 8 --run --out out/tiled4096
# or through the main command (terrain only; time evolution stays <=2048²)
./target/release/geogenesis generate --tiled --grid 4096

# Independent scientific validation (release harness + CLI benchmark report)
./scripts/validate.sh
```

Generated files land in `out/<preset>-<seed>/` (or `--out`).

## 4. Understand the outputs

| file | meaning |
| --- | --- |
| `terrain.png` | hypsometric + hillshade map |
| `biomes.png`, `rivers.png` | categorical overlays |
| `height16.png` | 16-bit heightmap (GIS/terrain import) |
| `normals.png` | normal map derived from terrain gradients |
| `profile.png` | cross-section figure with sea level and water |
| `elevation.f32` | raw little-endian float32 heights, row-major |
| `terrain.obj`, `terrain.gltf` | 3D meshes (glTF is self-contained) |
| `terrain.exr` | OpenEXR float heightmap |
| `fields.nc` | NetCDF-3 fields (elevation + climate/hydrology) |
| `minerals.png`, `minerals.json` | mineral map + inventory |
| `terrain.xyz` | point cloud |
| `stats.json`, `params.json` | reproducible world description |

## 5. Extend the engine

See [ARCHITECTURE.md §9](ARCHITECTURE.md) for the full guide. The short version:

- **Add a parameter**: add a field and a `ParamSpec` row to a subsystem's
  `params/*.rs` block and extend its value plumbing. The CLI, WASM schema and
  the Web UI panel pick it up automatically.
- **Add a process**: create a module under `crates/core/src/erosion/`, write
  `apply(...) -> MassBalance`, schedule it from `sim::step`, expose a display
  field through `FieldKind`, and add conservation tests.
- **Add a preset**: one entry in `params/presets.rs`.

## 6. Troubleshooting

- **`engine.wasm` missing / 404**: run `./scripts/build-wasm.sh` (it copies the
  module to `web/public/engine.wasm`).
- **Blank viewport**: update the browser (WebGL2 required); check the console.
- **Slow generation**: start with 256²; 512² takes ~0.4 s native, 1024² ~1.8 s.
  In the browser, WASM is slower; the UI runs generation in a Worker and shows
  progress.
- **`--grid 4096` rejected**: the single-shot path is memory-guarded; use the
  tiled path (`geogenesis tiles`) or opt in explicitly with
  `--allow-large-grid` on a machine with enough RAM.
- **CI**: `.github/workflows/ci.yml` runs fmt, clippy, tests, WASM smoke test
  and the Web build; the demo is deployed to GitHub Pages from `main`.
