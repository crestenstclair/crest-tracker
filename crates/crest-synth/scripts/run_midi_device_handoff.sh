#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ! -t 0 ]]; then
  echo "CREST_MIDI_HANDOFF incomplete: an interactive terminal is required" >&2
  exit 2
fi

case "$(uname -s)" in
  Darwin|Linux) ;;
  MINGW*|MSYS*|CYGWIN*) ;;
  *)
    echo "CREST_MIDI_HANDOFF incomplete: unsupported host $(uname -s)" >&2
    exit 2
    ;;
esac

echo "Building the production binary and probing the real MIDI host..."
cargo test adapter::midir_input_device::tests::real_host_seam_reports_zero_or_more_ports_or_one_typed_initialization_failure --lib -- --exact --nocapture
cargo build --bin crest-synth

echo ""
echo "Physical handoff checklist:"
echo "  1. Attach at least one MIDI input and start with audio at a safe level."
echo "  2. Shift+Start opens Settings / MIDI Devices; verify truthful identity/facts."
echo "  3. Connect, play supported messages, and verify Receiving plus audible output."
echo "  4. Switch inputs; verify the old device stops and no note remains stuck."
echo "  5. Unplug/replug; verify Unavailable then exact-identity recovery."
echo "  6. Disconnect/Retry, then Shift+Down; verify exact focus/context return."
echo "  7. Close the window and verify clean process exit."

child_pid=""
cleanup() {
  if [[ -n "${child_pid}" ]] && kill -0 "${child_pid}" 2>/dev/null; then
    kill -INT "${child_pid}" 2>/dev/null || true
    wait "${child_pid}" 2>/dev/null || true
  fi
}
trap cleanup INT TERM EXIT

target_directory="$(scripts/cargo_target_dir.sh)"
"$target_directory/debug/crest-synth" &
child_pid=$!
set +e
wait "${child_pid}"
status=$?
set -e
if [[ ${status} -ne 0 ]]; then
  child_pid=""
  echo "CREST_MIDI_HANDOFF failed: application exited with status ${status}" >&2
  exit "${status}"
fi
child_pid=""
trap - INT TERM EXIT

printf "Did every checklist item pass with the attached physical device? [y/N] "
read -r verified
case "${verified}" in
  y|Y|yes|YES)
    echo "CREST_MIDI_HANDOFF complete: operator confirmed every physical-path item"
    ;;
  *)
    echo "CREST_MIDI_HANDOFF incomplete: operator did not confirm every item" >&2
    exit 2
    ;;
esac
