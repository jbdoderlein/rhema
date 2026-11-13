#!/usr/bin/env bash
# Publish the library closure to crates.io, in dependency order.
#
#   .gitea/scripts/publish-crates.sh --check    # offline: the list is complete
#                                               # and topologically ordered
#   .gitea/scripts/publish-crates.sh 0.2.3      # publish whatever is missing
#
# CRATES is hand-ordered because the release also skips crates that never
# reach crates.io (rhema-code waits for its crossterm patches to land
# upstream), so the list, not the workspace, says what ships. --check runs
# in CI on every push: a crate that joins the dependency chain without
# joining the list fails there, not halfway through a tag release.
#
# CARGO_REGISTRY_TOKEN must be allowed to publish every crate in the
# list. A token restricted to specific crates answers 403 for the first
# name it does not carry, and a brand-new name is the usual culprit:
# tokens cannot be edited, so a crate joining the list needs a new one.
set -euo pipefail

CRATES=(
  rhema-core
  rhema-config
  rhema-provider
  rhema-agent-tools
  rhema-sandbox
  rhema-sandbox-local
  rhema-sandbox-http
  rhema-mcp
  rhema-rsd
  rhema-memory
  rhema-identity
  rhema-autonomous-agent
  rhema-transcript
  rhema-coding-agent
  rhema-cloud-agent
  rhema-provider-anthropic
  rhema-provider-chatgpt
  rhema-provider-openai-compatible
  rhema-provider-opencode
  rhema-provider-copilot
  rhema-provider-google
  rhema-provider-llama-cpp
  rhema-provider-openrouter
  rhema-provider-registry
  rhema-acp
)

index_of() {
  local i
  for i in "${!CRATES[@]}"; do
    if [ "${CRATES[$i]}" = "$1" ]; then
      echo "$i"
      return 0
    fi
  done
  return 1
}

# The crate's in-tree dependency lines, as "<name> <versioned|path-only>".
# Only `path = ` entries are in-tree; the dev sections are skipped because
# a published package never resolves them.
manifest_edges() {
  awk '
    /^\[/ { section = $0; next }
    section ~ /dependencies\]$/ && section !~ /dev-dependencies/ && /path *=/ {
      name = $0
      sub(/[[:space:]]*=.*/, "", name)
      print name, (/version *=/ ? "versioned" : "path-only")
    }
  ' "$1"
}

check() {
  local bad=0 crate manifest at dep dep_at kind
  for crate in "${CRATES[@]}"; do
    manifest="crates/$crate/Cargo.toml"
    if [ ! -f "$manifest" ]; then
      echo "publish list names $crate, which has no $manifest" >&2
      bad=1
      continue
    fi
    if grep -qE '^publish *= *false' "$manifest"; then
      echo "publish list names $crate, which sets publish = false" >&2
      bad=1
      continue
    fi
    at=$(index_of "$crate")
    while read -r dep kind; do
      if ! dep_at=$(index_of "$dep"); then
        echo "$crate depends on $dep, which the publish list does not carry" >&2
        bad=1
      elif [ "$dep_at" -ge "$at" ]; then
        echo "$crate is listed before $dep; move $dep earlier" >&2
        bad=1
      fi
      if [ "$kind" = path-only ]; then
        echo "$crate depends on $dep by path only; crates.io needs a version" >&2
        bad=1
      fi
    done < <(manifest_edges "$manifest")
  done
  return "$bad"
}

# Idempotent and rate-limit aware. crates.io caps NEW crates per rolling
# window, and a first release publishes a dozen at once, so: skip any
# crate whose exact version is already on the registry, tolerate the
# version landing between check and publish, and let a rate-limited crate
# fail the run without blocking later ones. Re-running resumes where the
# last run stopped.
publish() {
  local version="${1#v}" crate ok=yes out
  check || {
    echo "the publish list is incomplete; nothing was uploaded" >&2
    return 1
  }
  for crate in "${CRATES[@]}"; do
    if curl -fsSL -A "rhema-release (git.vhaudiquet.fr/vhaudiquet/rhema)" \
      "https://crates.io/api/v1/crates/$crate/$version" >/dev/null 2>&1; then
      echo "$crate v$version is already on crates.io, skipping"
      continue
    fi
    echo "publishing $crate v$version ..."
    if out=$(cargo publish -p "$crate" 2>&1); then
      echo "$out"
    elif echo "$out" | grep -qi "already exists\|already uploaded"; then
      echo "$crate v$version landed between check and publish, counting as done"
    else
      printf '%s\n' "$out" >&2
      ok=no
    fi
    sleep 5
  done
  [ "$ok" = yes ] || {
    echo "some crates did not publish; re-run this job to resume" >&2
    return 1
  }
}

case "${1:---check}" in
  --check)
    check
    ;;
  --help | -h)
    sed -n '2,9p' "$0"
    ;;
  *)
    publish "$1"
    ;;
esac
