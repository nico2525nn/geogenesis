# GeoGenesis

A procedural terrain generation engine that simulates millions of years of geological processes to produce realistic, diverse landscapes.

## Architecture

- **Core Engine**: Rust (WASM-compatible) — noise generation, plate tectonics, hydrological/glacial/wind erosion, climate simulation
- **Web UI**: TypeScript + React + Three.js — 3D viewport, real-time parameter adjustment, terrain analysis

## Quick Start

```bash
# Rust core
cargo build --release

# Web frontend
cd web
npm install
npm run dev
```

## Project Structure

```
geogenesis/
├── crates/
│   ├── core/       # Terrain generation engine
│   ├── wasm-api/   # WASM bindings
│   ├── cli/        # CLI tool
│   └── bench/      # Benchmarks
├── web/            # React + Three.js frontend
└── docs/           # Algorithm reference
```

## License

MIT
