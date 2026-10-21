# GeoGenesis developer tasks.
SHELL := /bin/bash
CARGO ?= cargo
PRESET ?= earthlike
SEED ?= 42
GRID ?= 384
TILES_GRID ?= 4096
TILES_WORKERS ?= 8

.PHONY: help wasm web dev test smoke bench cli generate sweep tiles validate clean fmt clippy check verify

help: ## List available targets
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-12s %s\n", $$1, $$2}'

wasm: ## Build the WebAssembly engine into web/public/engine.wasm
	./scripts/build-wasm.sh

web: wasm ## Production build of the Web UI into web/dist
	cd web && ./node_modules/.bin/tsc --noEmit && ./node_modules/.bin/vite build

dev: wasm ## Start the Vite dev server on http://localhost:4000
	cd web && ./node_modules/.bin/vite

test: wasm ## Rust tests plus the headless WASM ABI smoke test
	$(CARGO) test --workspace
	node scripts/smoke-wasm.mjs

smoke: wasm ## Only the headless WASM ABI smoke test
	node scripts/smoke-wasm.mjs

validate: ## Independent scientific-validation harness + CLI benchmark report
	./scripts/validate.sh

bench: ## Build the CLI and benchmark generation + stepping
	$(CARGO) build -p geogenesis-cli --release
	./target/release/geogenesis bench --grid 256 --steps 20
	./target/release/geogenesis bench --grid 512 --steps 10

cli: ## Build the native CLI
	$(CARGO) build -p geogenesis-cli --release

generate: cli ## Generate a world with the CLI (PRESET/SEED/GRID variables)
	./target/release/geogenesis generate --preset $(PRESET) --seed $(SEED) --grid $(GRID) --out out/$(PRESET)-$(SEED)

sweep: cli ## Run a seed sweep (STEPS = number of seeds)
	./target/release/geogenesis sweep --seed $(SEED) --steps 8 --grid 128

tiles: cli ## Report the large-grid tile plan and run the tiled prototype (TILES_GRID=4096)
	./target/release/geogenesis tiles --grid $(TILES_GRID) --tile-size 512 --halo 32 --workers $(TILES_WORKERS) --run --preview-factor 8 --out out/tiled-$(TILES_GRID)

clean: ## Remove build artefacts
	$(CARGO) clean
	rm -rf web/dist

fmt: ## Format Rust sources
	$(CARGO) fmt --all

clippy: ## Lint Rust sources
	$(CARGO) clippy --workspace --all-targets -- -D warnings

verify: ## Full verification: fmt, clippy, tests, WASM smoke, web build, bench, UI screenshot
	./scripts/verify.sh

check: ## Format check, clippy and web typecheck
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	cd web && ./node_modules/.bin/tsc --noEmit
