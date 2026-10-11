#!/usr/bin/env bash
# =============================================================================
# tidy-target.sh — report on and prune Cargo build artifacts in target/.
#
# Why this exists: Cargo garbage-collects old *sessions* inside an incremental
# unit directory, but it never removes the unit directory itself when the
# compilation fingerprint changes. Every feature-flag, RUSTFLAGS, or dependency
# shift mints a fresh `<crate>-<hash>` dir and orphans the previous one. Across
# a 92-crate workspace that compounds fast — this repo reached 745 debug unit
# dirs (26 variants of build_script_build alone) totalling 42 GB, of which only
# ~4.6 GB belonged to the live build config.
#
# `prune` keeps the $KEEP most recent unit dirs per crate per profile and drops
# the rest, so the cache serving the current config survives untouched and the
# next build is still incremental. Nothing here is unrecoverable: rustc rebuilds
# any pruned unit on demand.
#
# Usage:  scripts/tidy-target.sh report
#         scripts/tidy-target.sh prune [KEEP=1] [DRY=1]
#         scripts/tidy-target.sh prune-all           # drop every incremental/
# =============================================================================
set -euo pipefail

MODE="${1:-report}"
TARGET="${CARGO_TARGET_DIR:-target}"
KEEP="${KEEP:-1}"
DRY="${DRY:-0}"

gb() { awk -v k="${1:-0}" 'BEGIN {printf "%.2f GB", k/1048576}'; }

# Free space on the volume holding target/, in KiB. Used to report what a prune
# ACTUALLY returned: rustc hardlinks incremental .rcgu.o objects into deps/, so
# `du` of a unit dir counts bytes that survive deletion via their other link.
# On this repo that gap was real — 52.47 GB of unit dirs returned 47.44 GB.
avail_k() { df -k "$TARGET" | tail -1 | awk '{print $4}'; }

# Sum `du -sk` over a NUL-delimited path list on stdin. Chunked so we never
# blow ARG_MAX on the ~1000 unit dirs this repo accumulates.
sum_k() { xargs -0 -n40 du -sk 2>/dev/null | awk -F'\t' '{s+=$1} END {print s+0}'; }

require_target() {
  [ -d "$TARGET" ] || { echo "no $TARGET/ — nothing to do"; exit 0; }
}

# Refuse to touch anything while a build holds the profile lock. Deleting
# incremental state out from under a live rustc corrupts the build.
assert_no_build() {
  local lock held=""
  for lock in "$TARGET"/*/.cargo-lock; do
    [ -e "$lock" ] || continue
    if lsof -- "$lock" >/dev/null 2>&1; then held="$held $lock"; fi
  done
  if [ -n "$held" ]; then
    echo "REFUSING: a cargo build holds:$held" >&2
    echo "Wait for it to finish, then re-run." >&2
    exit 1
  fi
}

# Emit "KEEP<TAB>path" / "PRUNE<TAB>path" for every incremental unit dir,
# newest-first within each crate name. Unit dirs are `<crate_name>-<hash>`;
# rustc always passes a valid Rust identifier as --crate-name, so the only
# hyphen is the hash separator and stripping the last -<alnum> run is exact.
classify() {
  local inc="$1" u
  for u in "$inc"/*/; do
    [ -d "$u" ] || continue
    u="${u%/}"
    printf '%s\t%s\n' "$(stat -f '%m' "$u")" "$u"
  done | sort -rn | awk -F'\t' -v keep="$KEEP" '
    { name = $2; sub(/.*\//, "", name); sub(/-[0-9a-z]+$/, "", name)
      print (++n[name] <= keep ? "KEEP\t" : "PRUNE\t") $2 }'
}

report() {
  require_target
  echo "== $TARGET/ =="
  du -x -d 2 -k "$TARGET" 2>/dev/null | sort -rn | \
    awk -F'\t' '$1>102400 {printf "  %8.2f GB  %s\n", $1/1048576, $2}' | head -12

  local inc profile plan keep_k prune_k total_k=0
  for inc in "$TARGET"/*/incremental; do
    [ -d "$inc" ] || continue
    profile="$(basename "$(dirname "$inc")")"
    plan="$(classify "$inc")"
    # `|| true` is load-bearing: under `set -o pipefail` a no-match grep fails the
    # whole pipeline, which would abort the report on an already-tidy tree.
    keep_k=$( { grep '^KEEP'  <<<"$plan" || true; } | cut -f2 | tr '\n' '\0' | sum_k)
    prune_k=$( { grep '^PRUNE' <<<"$plan" || true; } | cut -f2 | tr '\n' '\0' | sum_k)
    total_k=$((total_k + prune_k))
    echo
    echo "== $profile/incremental (KEEP=$KEEP newest per crate) =="
    printf '  live   %10s  %4d unit dirs\n' "$(gb "$keep_k")"  "$(grep -c '^KEEP'  <<<"$plan" || true)"
    printf '  stale  %10s  %4d unit dirs\n' "$(gb "$prune_k")" "$(grep -c '^PRUNE' <<<"$plan" || true)"
  done
  echo
  echo "make tidy would drop $(gb "$total_k") of stale units with no rebuild cost."
  echo "(apparent size — hardlinks shared with deps/ mean real disk freed is somewhat less)"
}

prune() {
  require_target
  assert_no_build
  local inc profile plan prune_k n before_k after_k did=0
  before_k=$(avail_k)
  for inc in "$TARGET"/*/incremental; do
    [ -d "$inc" ] || continue
    profile="$(basename "$(dirname "$inc")")"
    plan="$(classify "$inc" | grep '^PRUNE' || true)"
    n=$(grep -c . <<<"$plan" || true)
    [ "$n" -gt 0 ] || { echo "$profile/incremental: already tidy"; continue; }
    prune_k=$(cut -f2 <<<"$plan" | tr '\n' '\0' | sum_k)
    if [ "$DRY" = "1" ]; then
      echo "$profile/incremental: would drop $n stale unit dirs ($(gb "$prune_k") apparent)"
    else
      cut -f2 <<<"$plan" | tr '\n' '\0' | xargs -0 -n40 rm -rf
      echo "$profile/incremental: dropped $n stale unit dirs ($(gb "$prune_k") apparent)"
      did=1
    fi
  done
  [ "$DRY" = "1" ] && { echo "(dry run — nothing deleted)"; return; }
  [ "$did" = "1" ] || { echo "nothing to do"; return; }
  after_k=$(avail_k)
  # df can drift under concurrent writes elsewhere on the volume; never report
  # a negative reclaim.
  echo "freed $(gb $(( after_k > before_k ? after_k - before_k : 0 ))) of real disk space"
}

prune_all() {
  require_target
  assert_no_build
  local inc freed_k=0 k
  for inc in "$TARGET"/*/incremental; do
    [ -d "$inc" ] || continue
    k=$(du -sk "$inc" 2>/dev/null | cut -f1)
    rm -rf "$inc"
    echo "removed $inc ($(gb "$k"))"
    freed_k=$((freed_k + k))
  done
  echo "freed $(gb "$freed_k") — next build recompiles workspace crates once"
}

case "$MODE" in
  report)    report ;;
  prune)     prune ;;
  prune-all) prune_all ;;
  *) echo "usage: $0 {report|prune|prune-all}" >&2; exit 2 ;;
esac
