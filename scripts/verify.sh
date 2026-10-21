#!/usr/bin/env bash
# End-to-end verification for GeoGenesis:
#   Rust tests + clippy + fmt, WASM build + ABI smoke test, Web typecheck/build,
#   CLI benchmark and a headless browser screenshot of the real UI.
#
# Usage: ./scripts/verify.sh [--quick]
#   --quick  skip the browser screenshot and the 512² benchmark
set -euo pipefail
cd "$(dirname "$0")/.."
quick=0
if [[ "${1:-}" == "--quick" ]]; then quick=1; fi

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy (workspace, warnings are errors)"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test --workspace"
cargo test --workspace

echo "==> build WebAssembly"
./scripts/build-wasm.sh

echo "==> WASM ABI smoke test"
node scripts/smoke-wasm.mjs

echo "==> web typecheck + production build"
cd web
./node_modules/.bin/tsc --noEmit
./node_modules/.bin/vite build
cd ..

echo "==> CLI benchmark"
cargo build -p geogenesis-cli --release
./target/release/geogenesis bench --grid 256 --steps 10
if [[ $quick -eq 0 ]]; then
  ./target/release/geogenesis bench --grid 512 --steps 5
fi

echo "==> tiled 4096² generation (production streaming pipeline)"
rm -rf /tmp/geogenesis-tiled-4096
./target/release/geogenesis tiles --grid 4096 --tile-size 512 --halo 32 --workers 8 --run --out /tmp/geogenesis-tiled-4096 | tail -6
test -f /tmp/geogenesis-tiled-4096/terrain.png
test -f /tmp/geogenesis-tiled-4096/height16.png
test -f /tmp/geogenesis-tiled-4096/elevation.f32
test -f /tmp/geogenesis-tiled-4096/preview.png

echo "==> CLI integration smoke (custom rocks/strata, minerals, EXR, NetCDF)"
rm -rf /tmp/geogenesis-cli-smoke
./target/release/geogenesis generate --grid 96 --rocks examples/custom-rocks.json --strata examples/strata.json --out /tmp/geogenesis-cli-smoke >/dev/null
for file in terrain.png height16.png normals.png profile.png terrain.exr fields.nc terrain.gltf minerals.png minerals.json stats.json params.json; do
  test -f "/tmp/geogenesis-cli-smoke/$file" || { echo "missing $file"; exit 1; }
done
echo "   ok: all documented export files written"

if [[ $quick -eq 0 ]]; then
  echo "==> independent validation harness (release)"
  cargo test --release -p geogenesis-core --test validation

  echo "==> headless UI screenshot"
  if curl -sf http://localhost:4173/ >/dev/null 2>&1; then
    cp web/public/engine.wasm web/dist/engine.wasm
    node scripts/ui-shot.mjs http://localhost:4173/ /tmp/geogenesis-verify.png 10000
  else
    echo "   (preview server on :4173 not running; skipping screenshot)"
    echo "   start one with: cd web && ./node_modules/.bin/vite preview --port 4173"
  fi
fi

echo
echo "VERIFICATION COMPLETE"
