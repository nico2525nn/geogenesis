# GeoGenesis — Specification

## Overview

GeoGenesis is a procedural terrain generation engine that simulates millions of years of geological processes. The core engine is written in Rust with WebAssembly output, and the UI is a web application using TypeScript with React and Three.js.

## Architecture

```
+-------------------+  +------------------+  +-------------------+
|  3D Viewport      |  |  Parameter Panel |  |  Analysis Views   |
|  (Three.js)       |  |  (all params)    |  |  (profile, stats) |
+---------+---------+  +--------+---------+  +---------+---------+
          |                      |                      |
          +----------+-----------+----------------------+
                     | WASM / Worker
          +----------+----------------------------------+
          |           Rust Core Engine (WASM)            |
          |  +-----------+ +----------+ +-------------+  |
          |  | Terrain   | | Erosion  | | Climate/    |  |
          |  | Generator  | | Simulator| | Biome       |  |
          |  +-----------+ +----------+ +-------------+  |
          +---------------------------------------------+
```

## Pipeline

1. **World Config** — Grid size, seed, coordinate system
2. **Tectonic Base** — Plate generation, uplift map, hotspots
3. **Initial Heightmap** — Fractal noise + tectonic deformation
4. **Erosion Loop** — Fluvial → Hillslope → Wind → Glacial → Coastal → Chemical
5. **Climate** — Temperature, precipitation, biome classification
6. **Export** — OBJ, XYZ, raw binary

## Crate Map

| Crate | Purpose |
|-------|---------|
| `core` | Engine: noise, tectonics, erosion, climate, geology, simulation loop |
| `wasm-api` | `wasm-bindgen` bindings for browser |
| `cli` | CLI tool for batch generation |
| `bench` | Performance benchmarks |

## Build

```
cargo build --release        # Rust core
wasm-pack build crates/wasm-api  # WASM
cd web && npm install && npm run dev  # Frontend
```

## Quality Targets

| Metric | Target |
|--------|--------|
| WASM binary size | < 2MB (gzip) |
| 512² grid generation | < 500ms |
| 1024² grid generation | < 3s |
| Memory (4096²) | < 1GB |
| Parameter → preview | < 16ms |
| WebGL 100k triangles | 60fps |
