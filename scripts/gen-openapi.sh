#!/usr/bin/env bash
# Regenerate the OpenAPI spec and TypeScript types from the Rust routers.
#
# Usage: ./scripts/gen-openapi.sh
#
# This script:
# 1. Runs the `takusu-local` `generate-openapi` binary
# 2. Runs the `takusu-agent` `generate-openapi` binary
# 3. Merges the two specs with `merge-openapi.mjs`
# 4. Runs `openapi-typescript` to produce src/types.gen.ts
#
# The generated files are committed to the repo so that CI can verify
# they are up to date.

set -euo pipefail

cd "$(dirname "$0")/.."

SPEC="ts/takusu-client/openapi/openapi.json"
TYPES="ts/takusu-client/src/types.gen.ts"

LOCAL_SPEC=$(mktemp)
AGENT_SPEC=$(mktemp)
trap 'rm -f "$LOCAL_SPEC" "$AGENT_SPEC"' EXIT

echo "==> Generating local OpenAPI spec"
cargo run -p takusu-local --bin generate-openapi -- -o "$LOCAL_SPEC"

echo "==> Generating agent OpenAPI spec"
cargo run -p takusu-agent --bin generate-openapi --no-default-features --features openapi,audio-device -- -o "$AGENT_SPEC"

echo "==> Merging specs into $SPEC"
node ts/takusu-client/scripts/merge-openapi.mjs "$LOCAL_SPEC" "$AGENT_SPEC" "$SPEC"

echo "==> Generating TypeScript types ($TYPES)"
cd ts/takusu-client
npm run gen:types
cd ../..

echo "==> Done"
