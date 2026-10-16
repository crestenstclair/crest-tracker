#!/usr/bin/env bash
# Resolve Cargo's actual output directory, including workspace/config overrides.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo metadata --format-version 1 --no-deps | \
  python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
