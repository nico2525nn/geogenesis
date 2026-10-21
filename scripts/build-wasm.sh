#!/usr/bin/env bash
# Builds the engine to WebAssembly and installs it where Vite serves it.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release --target wasm32-unknown-unknown -p geogenesis-wasm
mkdir -p web/public
cp target/wasm32-unknown-unknown/release/geogenesis_wasm.wasm web/public/engine.wasm

raw=$(stat -c %s web/public/engine.wasm)
gz=$(gzip -9 -c web/public/engine.wasm | wc -c)
echo "engine.wasm: ${raw} bytes raw, ${gz} bytes gzip (target < 2 MB gzip)"
