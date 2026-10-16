#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target_dir="$("$script_dir/cargo_target_dir.sh")"
warn_gib="${CREST_TARGET_WARN_GIB:-24}"
hard_gib="${CREST_TARGET_HARD_GIB:-32}"
mode="${1:---guard}"

if [[ ! "$warn_gib" =~ ^[0-9]+$ ]] || [[ ! "$hard_gib" =~ ^[0-9]+$ ]]; then
  echo "CREST_BUILD_CACHE thresholds must be whole GiB values" >&2
  exit 2
fi
if (( warn_gib < 1 || hard_gib <= warn_gib )); then
  echo "CREST_BUILD_CACHE requires 0 < warning < hard limit" >&2
  exit 2
fi
if [[ "$mode" != "--guard" && "$mode" != "--status" ]]; then
  echo "usage: check_build_cache_size.sh [--guard|--status]" >&2
  exit 2
fi

size_kib=0
if [[ -d "$target_dir" ]]; then
  size_kib="$(du -sk "$target_dir" | awk '{print $1}')"
fi

warn_kib=$((warn_gib * 1024 * 1024))
hard_kib=$((hard_gib * 1024 * 1024))
size_gib="$(awk -v kib="$size_kib" 'BEGIN { printf "%.1f", kib / 1048576 }')"

if [[ "$mode" == "--status" ]]; then
  printf 'CREST_BUILD_CACHE %s GiB used · warning %s GiB · hard stop %s GiB\n' \
    "$size_gib" "$warn_gib" "$hard_gib"
fi

if (( size_kib >= hard_kib )); then
  cat >&2 <<EOF
CREST_BUILD_CACHE refusing to build: target/ uses ${size_gib} GiB (hard stop ${hard_gib} GiB).
Preview cleanup: cargo clean --dry-run --profile dev
Reclaim debug/test artifacts: make cache-prune
For a deliberate one-command override, set CREST_TARGET_HARD_GIB above the current size.
EOF
  exit 1
fi

if (( size_kib >= warn_kib )); then
  cat >&2 <<EOF
CREST_BUILD_CACHE warning: target/ uses ${size_gib} GiB (warning ${warn_gib} GiB; hard stop ${hard_gib} GiB).
Run 'make cache-prune-preview' to inspect the reclaimable debug/test artifacts.
EOF
fi
