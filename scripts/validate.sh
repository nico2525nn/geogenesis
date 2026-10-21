#!/usr/bin/env bash
# Independent scientific validation for GeoGenesis.
#
# Runs the black-box validation harness in release mode (long-run stability and
# conservation across presets/seeds, climate/hydrology/erosion laws, statistics
# and export-format invariants) and then the CLI performance benchmark. The
# harness prints its measured run table with --nocapture; copy that table into
# docs/VALIDATION.md when refreshing the report.
#
# Usage: ./scripts/validate.sh
#
# Environment:
#   CARGO            cargo binary to use (default: cargo from PATH)
#   VALIDATION_ARGS  extra arguments passed to the test binary (optional)
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

cargo_bin="${CARGO:-cargo}"
# One test thread at a time keeps the measured timings meaningful and avoids
# competition between the long-run sweep and the performance smoke test.
test_args=(--nocapture --test-threads=1)
if [[ -n "${VALIDATION_ARGS:-}" ]]; then
  # shellcheck disable=SC2206  # deliberate word splitting of the caller's flags
  extra=(${VALIDATION_ARGS})
  test_args+=("${extra[@]}")
fi

echo "======================================================================"
echo " GeoGenesis scientific validation"
echo "----------------------------------------------------------------------"
echo " harness: cargo test --release -p geogenesis-core --test validation"
echo " bench:   geogenesis bench --grid 256 --steps 10"
echo "======================================================================"
echo

echo "==> validation harness (release)"
"$cargo_bin" test --release -p geogenesis-core --test validation -- "${test_args[@]}"

echo
echo "==> CLI performance benchmark (256^2, 10 steps)"
"$cargo_bin" build -p geogenesis-cli --release
./target/release/geogenesis bench --grid 256 --steps 10

echo
echo "======================================================================"
echo " VALIDATION COMPLETE"
echo " Full report (checks, measured table, limitations): docs/VALIDATION.md"
echo " Re-run with GEOGENESIS_VALIDATION_HEAVY=1 in a debug build to exercise"
echo " the long-run sweep without --release (slower)."
echo "======================================================================"
