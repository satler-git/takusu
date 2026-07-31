#!/usr/bin/env bash
# Regenerate the OpenAPI spec and TypeScript types from the Rust router.
#
# Usage: ./scripts/gen-openapi.sh
#
# This script:
# 1. Runs the `generate-openapi` binary to produce openapi.json
# 2. Runs `openapi-typescript` to produce src/types.gen.ts
#
# The generated files are committed to the repo so that CI can verify
# they are up to date.

set -euo pipefail

cd "$(dirname "$0")/.."

SPEC="ts/takusu-client/openapi/openapi.json"
TYPES="ts/takusu-client/src/types.gen.ts"

echo "==> Generating OpenAPI spec ($SPEC)"
cargo run -p takusu-local --bin generate-openapi -- -o "$SPEC"

echo "==> Generating TypeScript types ($TYPES)"
cd ts/takusu-client
npm run gen:types
cd ../..

echo "==> Done"
