#!/usr/bin/env bash
# Measure Rust workspace coverage with cargo-llvm-cov (nextest as the runner).
#
# Usage:
#   ./scripts/coverage.sh              # print text summary to stdout
#   ./scripts/coverage.sh lcov         # write lcov.info + html/ under target/coverage
#   ./scripts/coverage.sh html         # write html/ under target/coverage
#   ./scripts/coverage.sh open         # write html/ and open it in a browser
#
# Run inside `nix develop .#rust` (or any shell with cargo-llvm-cov and
# cargo-nextest available). Tests run under nextest for faster, parallel
# execution. takusu-worker is a wasm cdylib whose ignored integration tests
# require wrangler, so it is excluded from coverage.
set -euo pipefail

FORMAT="${1:-text}"
OUT_DIR="target/coverage"
EXCLUDES=(--exclude takusu-worker)
# cargo-llvm-cov --html --output-dir X writes the index to X/html/index.html,
# so point --output-dir at OUT_DIR to get OUT_DIR/html/index.html.
HTML_INDEX="$OUT_DIR/html/index.html"

case "$FORMAT" in
  text)
    cargo llvm-cov nextest --workspace "${EXCLUDES[@]}" --all-features
    ;;
  lcov)
    # Run the instrumented test suite once, then generate both lcov and html
    # reports from the same profdata via `cargo llvm-cov report`.
    mkdir -p "$OUT_DIR"
    cargo llvm-cov nextest --workspace "${EXCLUDES[@]}" --all-features --no-report
    cargo llvm-cov report --lcov --output-path "$OUT_DIR/lcov.info"
    cargo llvm-cov report --html --output-dir "$OUT_DIR"
    echo "lcov report: $OUT_DIR/lcov.info"
    echo "html report: $HTML_INDEX"
    ;;
  html)
    mkdir -p "$OUT_DIR"
    cargo llvm-cov nextest --workspace "${EXCLUDES[@]}" --all-features --no-report
    cargo llvm-cov report --html --output-dir "$OUT_DIR"
    echo "html report: $HTML_INDEX"
    ;;
  open)
    mkdir -p "$OUT_DIR"
    cargo llvm-cov nextest --workspace "${EXCLUDES[@]}" --all-features --no-report
    cargo llvm-cov report --html --output-dir "$OUT_DIR"
    echo "html report: $HTML_INDEX"
    if command -v xdg-open >/dev/null 2>&1; then
      xdg-open "$HTML_INDEX"
    elif command -v open >/dev/null 2>&1; then
      open "$HTML_INDEX"
    else
      echo "no opener found; open $HTML_INDEX manually"
    fi
    ;;
  *)
    echo "unknown format: $FORMAT" >&2
    echo "usage: $0 [text|lcov|html|open]" >&2
    exit 2
    ;;
esac
