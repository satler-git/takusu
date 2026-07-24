#!/usr/bin/env bash
# Cut a release: bump versions everywhere, commit, tag, and push.
#
# Usage:
#   ./scripts/release.sh              # auto: v0.YYYYMMDD.n (next n for today)
#   ./scripts/release.sh 1.0.0        # explicit: v1.0.0
#   ./scripts/release.sh 1.0.0 --no-push   # do everything except push
#
# Files updated:
#   Cargo.toml              (workspace.package.version)
#   Cargo.lock              (workspace member version entries, via cargo check)
#   mobile/app.json         (expo.version)
#   mobile/package.json     (version)
#
# The git tag (with "v" prefix) is what triggers .github/workflows/release.yaml.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

NO_PUSH=0
EXPLICIT=""
for arg in "$@"; do
  case "$arg" in
    --no-push) NO_PUSH=1 ;;
    -h|--help)
      sed -n '2,/^$/p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) EXPLICIT="$arg" ;;
  esac
done

# ── Determine the new version ──────────────────────────────────────────────
if [ -n "$EXPLICIT" ]; then
  # Strip a leading "v" if the user typed one.
  EXPLICIT="${EXPLICIT#v}"
  VERSION="$EXPLICIT"
else
  TODAY="$(date +%Y%m%d)"
  # Find the highest n used today (v0.YYYYMMDD.n) and increment.
  # Use jj tag list (not git tag) so the view matches what jj tag set will
  # see — git and jj can diverge after partial fetches.
  LAST_N=$(jj tag list "v0.${TODAY}.*" 2>/dev/null \
            | sed 's/:.*//' \
            | sed "s/^v0\.${TODAY}\.//" \
            | grep -E '^[0-9]+$' \
            | sort -n \
            | tail -1 \
            || true)
  NEXT_N=$(( ${LAST_N:-0} + 1 ))
  VERSION="0.${TODAY}.${NEXT_N}"
fi

TAG="v${VERSION}"

echo "── Release: ${TAG} ──"
echo ""

# ── Sanity: refuse if tag already exists ───────────────────────────────────
# Use jj tag list for consistency with the version computation above and
# jj tag set below.
if jj tag list "$TAG" 2>/dev/null | grep -q .; then
  echo "Error: tag ${TAG} already exists" >&2
  exit 1
fi

# ── Show what will change (dry run, no edits yet) ───────────────────────────
echo "Files that will be updated to ${VERSION}:"
echo "  Cargo.toml              (workspace.package.version)"
echo "  Cargo.lock              (workspace member version entries)"
echo "  mobile/app.json         (expo.version)"
echo "  mobile/package.json     (version)"
echo ""
echo "This will:"
echo "  1. Create a new change with these version bumps"
echo "  2. Create git tag ${TAG}"
if [ "$NO_PUSH" -eq 1 ]; then
  echo "  3. (skip push — --no-push)"
else
  echo "  3. Push the tag to origin (triggers release workflow)"
fi
echo ""
read -r -p "Proceed? [y/N] " ans
case "$ans" in
  y|Y|yes) ;;
  *) echo "Aborted."; exit 1 ;;
esac

# ── Apply version bumps, describe, tag ──────────────────────────────────────
# If the current change is empty (no description, no edits), reuse it instead
# of creating a redundant empty change on top.
IS_EMPTY=$(jj log -r @ --no-graph --no-pager -T 'if(empty && !description, "yes", "no")')
if [ "$IS_EMPTY" != "yes" ]; then
  jj new
fi

perl -0pi -e \
  's/(\[workspace\.package\]\nversion = ")[^"]*(")/${1}'"${VERSION}"'${2}/' \
  Cargo.toml
perl -0pi -e \
  's/("version":\s*")[^"]*(")/${1}'"${VERSION}"'${2}/' \
  mobile/app.json
perl -0pi -e \
  's/("version":\s*")[^"]*(")/${1}'"${VERSION}"'${2}/' \
  mobile/package.json

# Regenerate Cargo.lock so workspace member version entries match the
# bumped workspace.package.version. The Nix CI build uses --locked, so a
# stale Cargo.lock fails the release. `cargo check` only updates the lock
# minimally (just the changed workspace member versions) without bumping
# transitive dependencies.
cargo check --workspace --quiet

jj describe -m "release ${TAG}"

# Move the main bookmark to the release commit so that main carries the
# version bump. Without this, main would keep the pre-release version
# forever (the bump commit would only live on the tag object).
jj bookmark set main -r @ --allow-backwards

# Tag the current working-copy commit (@) — jj tag set manages tags in jj.
jj tag set "$TAG"

echo ""
echo "Created tag ${TAG} on @ (main bookmark moved to @)"

if [ "$NO_PUSH" -eq 0 ]; then
  echo "Pushing main and tag to origin..."
  jj git push --bookmark main
  git push origin "$TAG"
  echo "Pushed. The release workflow should start shortly:"
  echo "  https://github.com/takusu-dev/takusu/actions/workflows/release.yaml"
else
  echo "(--no-push: tag and bookmark created locally only)"
fi
