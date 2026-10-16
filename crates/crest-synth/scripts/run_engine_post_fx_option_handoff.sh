#!/bin/sh
set -eu

if [ "$(uname -s)" != "Darwin" ]; then
  echo "CREST_OPTION_HANDOFF_BLOCKED: production physical handoff requires macOS/WKWebView" >&2
  exit 2
fi

if [ ! -t 0 ] || [ ! -t 1 ]; then
  echo "CREST_OPTION_HANDOFF_BLOCKED: run this checklist from an interactive terminal" >&2
  exit 2
fi

scripts/check_build_cache_size.sh --guard

evidence_dir="target/handoff-evidence"
mkdir -p "$evidence_dir"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
evidence_log="${CREST_OPTION_HANDOFF_LOG:-$evidence_dir/engine-post-fx-options-$stamp.log}"
app_log="$evidence_dir/engine-post-fx-options-app-$stamp.log"
maximum_seconds="${CREST_OPTION_HANDOFF_TIMEOUT_SECONDS:-600}"

case "$maximum_seconds" in
  *[!0-9]* | "")
    echo "CREST_OPTION_HANDOFF_BLOCKED: timeout must be a positive integer" >&2
    exit 2
    ;;
esac
if [ "$maximum_seconds" -le 0 ]; then
  echo "CREST_OPTION_HANDOFF_BLOCKED: timeout must be positive" >&2
  exit 2
fi

{
  echo "CREST_OPTION_HANDOFF started=$stamp"
  echo "host=$(uname -m)-$(uname -s)"
  echo "timeoutSeconds=$maximum_seconds"
  echo "applicationLog=$app_log"
  echo "requiredDevices=physical-keyboard,physical-controller"
} >"$evidence_log"

echo "Engine/Post FX option physical handoff"
echo "Evidence: $evidence_log"
echo "Application transcript: $app_log"
echo
echo "Use the production app with a physical keyboard and a connected controller."
echo "Exercise every checklist item below, then close the app window. The run is"
echo "bounded to ${maximum_seconds}s and only an exit status of 0 is accepted."
echo
echo "  1. Engine: Edit+Up opens ENGINE TYPE OPTIONS at the current row."
echo "  2. Up/Down moves one non-wrapping focus; CURRENT remains independent."
echo "  3. Edit on CURRENT closes unchanged; Edit on another row shows a request."
echo "  4. Shift+Down closes unchanged and returns to the exact Engine origin."
echo "  5. Repeat entry/return for every occupied Post FX slot, including duplicates."
echo "  6. Repeat entry/return for an Empty Post FX slot and choose a fill option."
echo "  7. Resize Wide→Standard→Compact→Wide: identity, focus, order, and Utility persist."
echo "  8. Repeat Shift press/release at least four times; no repeat action or crash occurs."
echo "  9. Repeat entry, navigation, choose, and close with the physical controller."
echo

cargo run --bin crest-synth >"$app_log" 2>&1 &
app_pid=$!
(
  sleep "$maximum_seconds"
  if kill -0 "$app_pid" 2>/dev/null; then
    echo "CREST_OPTION_HANDOFF timeout=true" >>"$evidence_log"
    kill -TERM "$app_pid" 2>/dev/null || true
  fi
) &
watchdog_pid=$!

set +e
wait "$app_pid"
app_status=$?
set -e
kill "$watchdog_pid" 2>/dev/null || true
wait "$watchdog_pid" 2>/dev/null || true
echo "applicationExit=$app_status" >>"$evidence_log"

if [ "$app_status" -ne 0 ]; then
  echo "CREST_OPTION_HANDOFF_FAILED: production app exited $app_status; see $app_log" >&2
  exit 1
fi

record_confirmation() {
  item="$1"
  prompt="$2"
  printf "%s [y/N] " "$prompt"
  IFS= read -r answer
  case "$answer" in
    y | Y | yes | YES)
      echo "$item=pass" >>"$evidence_log"
      ;;
    *)
      echo "$item=fail" >>"$evidence_log"
      return 1
      ;;
  esac
}

failed=0
record_confirmation "keyboardEntry" "Keyboard Edit+Up entry and current focus passed?" || failed=1
record_confirmation "keyboardNavigation" "Keyboard non-wrapping Up/Down and independent CURRENT passed?" || failed=1
record_confirmation "chooseLifecycle" "Current no-op and changed request/lifecycle passed?" || failed=1
record_confirmation "unchangedClose" "Shift+Down closed unchanged?" || failed=1
record_confirmation "exactEngineReturn" "Engine returned to its exact stable origin?" || failed=1
record_confirmation "occupiedSlotReturn" "Every occupied/duplicate slot returned exactly?" || failed=1
record_confirmation "emptySlotReturn" "Empty-slot entry, fill, and exact return passed?" || failed=1
record_confirmation "resizeInvariant" "Wide/Standard/Compact resize preserved semantic identity and Utility?" || failed=1
record_confirmation "repeatedShift" "Four repeated physical Shift gestures produced no repeat/crash?" || failed=1
record_confirmation "controllerGrammar" "Physical controller entry/navigation/choose/close passed?" || failed=1

completed="$(date -u +%Y%m%dT%H%M%SZ)"
echo "completed=$completed" >>"$evidence_log"
if [ "$failed" -ne 0 ]; then
  echo "result=failed" >>"$evidence_log"
  echo "CREST_OPTION_HANDOFF_FAILED: one or more physical observations were not confirmed" >&2
  exit 1
fi

echo "result=passed" >>"$evidence_log"
echo "CREST_OPTION_HANDOFF_PASSED evidence=$evidence_log application=$app_log"
