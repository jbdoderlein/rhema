#!/usr/bin/env bash
# The extraction contract for the claw product.
#
# The product (the daemon and the two integrations that feed it, plus
# the web and android trees) is leaving for its own repository; the
# framework and the resident tier stay here. While both still live in
# this tree, this checks the one thing that voids the move: a framework
# crate growing an edge into the product. It also names every framework
# crate the product still needs, and marks the ones the crates.io
# closure does not carry yet. Those are the publish work the split
# needs, since across repositories a path dependency no longer exists.
#
#   scripts/check-claw-boundary.sh            # report
#   scripts/check-claw-boundary.sh --strict   # ... and fail on an
#                                             # unpublished crossing
#
# Exit status is 1 on a back-edge, and with --strict on a crossing edge
# the closure does not carry.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

STRICT=0
for arg in "$@"; do
  case "$arg" in
    --strict) STRICT=1 ;;
    *) echo "unknown argument: $arg (supported: --strict)" >&2; exit 2 ;;
  esac
done

# Crates that move to the claw repository. The client tools are one
# workspace and move whole: the contract crate alone would orphan the
# two crates that path-depend on it. The web and android trees carry no
# manifests and are not listed.
PRODUCT=(
  rhema-claw
  rhema-forge-integration
  rhema-calendar-integration
  rhema-claw-tools-contract
  rhema-tools-common
  notify
)

declare -A IS_PRODUCT IS_PUBLISHED MANIFEST

for crate in "${PRODUCT[@]}"; do IS_PRODUCT[$crate]=1; done

# The crates.io closure, hand-ordered in publish-crates.sh.
while read -r crate; do
  [ -n "$crate" ] && IS_PUBLISHED[$crate]=1
done < <(sed -n '/^CRATES=(/,/^)/p' scripts/publish-crates.sh |
  sed -n 's/^  \([a-z0-9-]*\)$/\1/p')

for manifest in crates/*/Cargo.toml integrations/*/Cargo.toml tools/*/Cargo.toml; do
  crate=$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$manifest" | head -1)
  MANIFEST[$crate]=$manifest
done

# In-tree dependencies, one name per line: every `path = ` line in a
# dependencies table, dev-dependencies excluded (a published package
# never resolves them).
edges() {
  awk '
    /^\[/ { section = $0; next }
    section ~ /dev-dependencies/ { next }
    section ~ /dependencies\]$/ {
      if (match($0, /^[A-Za-z0-9_-]+/)) { name = substr($0, 1, RLENGTH) }
      if ($0 ~ /path *=/) print name
    }
  ' "$1" | sort -u
}

bad=0
crossing=()
unpublished=()

for crate in "${!MANIFEST[@]}"; do
  for dep in $(edges "${MANIFEST[$crate]}"); do
    [ -n "${MANIFEST[$dep]:-}" ] || continue
    if [ -z "${IS_PRODUCT[$crate]:-}" ] && [ -n "${IS_PRODUCT[$dep]:-}" ]; then
      echo "back-edge: $crate -> $dep (${MANIFEST[$crate]})" >&2
      bad=1
    elif [ -n "${IS_PRODUCT[$crate]:-}" ] && [ -z "${IS_PRODUCT[$dep]:-}" ]; then
      crossing+=("$dep")
      if [ -z "${IS_PUBLISHED[$dep]:-}" ]; then
        unpublished+=("$dep")
      fi
    fi
  done
done

if [ "$bad" -ne 0 ]; then
  echo "the claw boundary is broken; see the edges above" >&2
  exit 1
fi

if [ "${#crossing[@]}" -eq 0 ]; then
  echo "the product depends on no framework crate"
  exit 0
fi

echo "the product's framework dependencies:"
for dep in $(printf '%s\n' "${crossing[@]}" | sort -u); do
  printf '  %s\n' "$dep"
done

if [ "${#unpublished[@]}" -eq 0 ]; then
  echo "all of them are in the crates.io closure"
  exit 0
fi

echo
echo "not in the crates.io closure, so unreachable from another repository:"
for dep in $(printf '%s\n' "${unpublished[@]}" | sort -u); do
  echo "  $dep (${MANIFEST[$dep]})"
done

if [ "$STRICT" -eq 1 ]; then
  echo "the product cannot be built outside this tree yet" >&2
  exit 1
fi
