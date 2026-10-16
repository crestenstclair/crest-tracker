#!/usr/bin/env bash
# All ordinary tests plus the native witnesses that are opt-in elsewhere.
set -euo pipefail
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-2}"
export CREST_REQUIRE_KEY_WITNESS=1
export CREST_WEBVIEW_TESTS=1
# Each selector narrows the native harness. Keep inherited selections out of
# the complete gate and scope each journey explicitly below.
soundfont_evidence="${CREST_SOUNDFONT_EVIDENCE_DIR:-/tmp/crest-linux-soundfont-evidence}"
unset CREST_SOUNDFONT_EVIDENCE_DIR
unset CREST_WEBVIEW_PAGE_NAVIGATION_WITNESS CREST_WEBVIEW_OPTION_WITNESS \
  CREST_WEBVIEW_EMPTY_PATCH_WITNESS CREST_WEBVIEW_CONTROLLER_WITNESS \
  CREST_WEBVIEW_DETAIL_WITNESS CREST_WEBVIEW_SAMPLE_WITNESS \
  CREST_WEBVIEW_PERFORMANCE_WITNESS
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo test --locked --test alsa_signal_recovery -- --ignored
cargo test --locked --doc
cargo clippy --locked --all-targets -- -D warnings
python3 -m unittest discover -s scripts -p 'test_*.py' -v
python3 scripts/check_native_audio.py
CREST_WEBVIEW_PAGE_NAVIGATION_WITNESS=1 cargo test --locked --test webview_projection_shell
CREST_WEBVIEW_OPTION_WITNESS=1 cargo test --locked --test webview_projection_shell
CREST_WEBVIEW_EMPTY_PATCH_WITNESS=1 cargo test --locked --test webview_projection_shell
CREST_WEBVIEW_CONTROLLER_WITNESS=1 cargo test --locked --test webview_projection_shell
CREST_WEBVIEW_DETAIL_WITNESS=1 CREST_WEBVIEW_SAMPLE_WITNESS=1 cargo test --locked --test webview_projection_shell
CREST_SOUNDFONT_EVIDENCE_DIR="$soundfont_evidence" cargo test --locked --test soundfont_file_loading
CREST_SOUNDFONT_EVIDENCE_DIR="$soundfont_evidence" CREST_WEBVIEW_DETAIL_WITNESS=1 cargo test --locked --test webview_projection_shell
