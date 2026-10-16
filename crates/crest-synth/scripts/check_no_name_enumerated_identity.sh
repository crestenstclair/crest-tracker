#!/usr/bin/env bash
# no_name_enumerated_identity — static guard for the open-closed invariant.
#
# Product invariant:
#   no type in Synth, Mixer, RealTime, or Control may enumerate a variant,
#   field, or descriptor entry named after a specific effect or bus; and the
#   reducer, resolver, projector, renderer, rack, or live orchestration may
#   not branch on a concrete capability identity to define fields. Effects,
#   slots, sends, returns, and capability detail are descriptor-driven. The
#   single exception is MasterGainDb: master gain is genuinely global, a
#   property of the master stage rather than of any effect.
#
# Why this exists: the retired closed enumerated design
# (MixerTrackParameter::ReverbSend, GlobalEffectsProcessor,
# GlobalParameter::ReverbRoomSize, ...) shipped before the current canonical
# registry boundary. DESIGN.md records the durable invariant, and this script
# enforces it mechanically so a regression fails the build.
#
# What it checks: the four bound context trees (src/synth, src/mixer,
# src/real_time, src/control) must not contain any of the retired identifiers
# this mission removed, in identifier position. The forbidden set is seeded
# from plan.md § Bulk Edit Classification / occurrence_map.yaml:
#   1. MixerTrackParameter::ReverbSend / ::DelaySend and their fields
#   2. the GlobalParameter reverb/delay fields (ReverbRoomSize, ReverbDamping,
#      ReverbReturn, DelayMilliseconds, DelayFeedback, DelayReturn)
#   3. GlobalEffectsProcessor and its named process inputs (reverb_input,
#      delay_input)
#   4. the GlobalReverbDelay adapter pair
#
# Deliberately out of scope (precision over reach — this is one invariant,
# not a naming linter):
#   - src/adapter/*: an adapter implementing a capability is supposed to own
#     and declare its concrete identity.
#   - code below a file's first `#[cfg(test)]`: conformance fixtures must name
#     concrete registry entries in order to prove generic production logic.
#   - comments and string literals: doc comments legitimately narrate the
#     retired design, registry capability ids ("effect.reverb") are the open
#     registry working as designed, and retained observation/telemetry labels
#     are classified logs_telemetry: do_not_change in occurrence_map.yaml.
#   - composite identifiers such as reverb_input_rms (retained transitional
#     telemetry accessors over the generic bus_input_rms arrays) and
#     InvalidMaxDelayMilliseconds: matching is exact-identifier with word
#     boundaries, so only the retired names themselves fire.
#   - MasterGainDb / master_gain_db: the single declared exception; it never
#     matches the seeded set, and the self-test asserts it stays allowed.
#
# Modes:
#   (no args)          scan the four context trees; exit 0 and print
#                      "CREST_STATIC_VALIDATION no_name_enumerated_identity passed"
#                      when clean, exit 1 with file:line:identifier otherwise
#   --scan-root <dir>  scan an arbitrary tree (used by
#                      tests/no_name_enumeration_guard.rs fixtures)
#   --self-test        prove the failure path: a seeded reintroduction must
#                      fail and the documented exceptions must pass
#
# Exit codes — a missing tool, a found candidate, and a clean tree are three
# different answers and must never collapse into one:
#   0  clean (or self-test satisfied)
#   1  name-enumerated identifiers found
#   2  usage error, or the scanner itself failed while running
#   3  a required external tool is absent — the invariant was NOT checked
set -u
set -o pipefail

# Locate the repository root with parameter expansion rather than dirname, so
# the tool gate below is the first external dependency this script has: a
# pre-gate call to a missing tool would fail with a message about that tool
# instead of the gate's own report.
script_dir="${BASH_SOURCE[0]%/*}"
[[ "$script_dir" == "${BASH_SOURCE[0]}" ]] && script_dir='.'
cd "$script_dir/.." || exit 2

# Tool gate. Every external program this script invokes is verified before
# anything is scanned. An absent scanner cannot report "no candidates": that
# would turn a machine without ripgrep into a permanent pass for an invariant
# that exists precisely because prose controls failed here once already.
readonly REQUIRED_TOOLS=(rg perl sort mktemp mkdir rm cat)
require_tools() {
  local tool missing=()
  for tool in "${REQUIRED_TOOLS[@]}"; do
    command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
  done
  if [[ "${#missing[@]}" -ne 0 ]]; then
    for tool in "${missing[@]}"; do
      printf 'check_no_name_enumerated_identity: required tool not found: %s\n' \
        "$tool" >&2
    done
    printf 'check_no_name_enumerated_identity: the no-name-enumerated-identity invariant was NOT checked; install the tool(s) named above and re-run. A missing tool is a failure, never a pass.\n' >&2
    exit 3
  fi
}
require_tools

pascal='ReverbSend|DelaySend|GlobalEffectsProcessor|GlobalReverbDelay'
pascal+='|ReverbRoomSize|ReverbDamping|ReverbReturn'
pascal+='|DelayMilliseconds|DelayFeedback|DelayReturn|ReverbInput|DelayInput'
snake='reverb_send|delay_send|global_effects_processor|global_reverb_delay'
snake+='|reverb_room_size|reverb_damping|reverb_return'
snake+='|delay_milliseconds|delay_feedback|delay_return|reverb_input|delay_input'
camel='reverbSend|delaySend|reverbRoomSize|reverbDamping|reverbReturn'
camel+='|delayMilliseconds|delayFeedback|delayReturn'
FORBIDDEN="\\b(${pascal}|${snake}|${camel})\\b"
# Any concrete registry constant follows this spelling convention. Generic
# production policy receives `CapabilityId` values from descriptors and must
# therefore never need one of these adapter-owned constants.
CAPABILITY_POLICY_FORBIDDEN='\b[A-Z][A-Z0-9_]*_CAPABILITY_ID\b'

# Remove string literals first (so "// not a comment" inside a string cannot
# hide code), then line comments. Line-granular: good enough for this tree,
# and false negatives on exotic multi-line literals are acceptable because
# identifiers, not strings, are the guarded surface.
strip_noncode() {
  perl -pe 's/r?"(?:\\.|[^"\\])*"/""/g; s{//.*$}{}'
}

# scan <dir>... — prints one line per violation.
# Returns 0 clean, 1 violations found, 2 the scanner itself failed. rg exits 1
# for "no matches" and >=2 for a real error; conflating those is how a broken
# scan would read as a clean tree, so the two are separated here.
scan() {
  local violations=0 candidate file rest line content stripped ident
  local candidates rg_status
  candidates="$(rg -n --no-heading --sort path -g '*.rs' -e "$FORBIDDEN" "$@")"
  rg_status=$?
  case "$rg_status" in
    0) ;;
    1) return 0 ;;
    *)
      printf 'check_no_name_enumerated_identity: rg failed (exit %s) while scanning %s\n' \
        "$rg_status" "$*" >&2
      return 2
      ;;
  esac
  while IFS= read -r candidate; do
    [[ -n "$candidate" ]] || continue
    file="${candidate%%:*}"
    rest="${candidate#*:}"
    line="${rest%%:*}"
    content="${rest#*:}"
    if ! stripped="$(printf '%s\n' "$content" | strip_noncode)"; then
      printf 'check_no_name_enumerated_identity: perl failed while stripping %s:%s\n' \
        "$file" "$line" >&2
      return 2
    fi
    if printf '%s\n' "$stripped" | rg -q -e "$FORBIDDEN"; then
      while IFS= read -r ident; do
        printf '%s:%s: name-enumerated identifier %s\n' "$file" "$line" "$ident"
      done < <(printf '%s\n' "$stripped" | rg -o -e "$FORBIDDEN" | sort -u)
      violations=1
    fi
  done <<<"$candidates"
  return "$violations"
}

# Scans only production portions of Rust files for concrete capability policy.
# The first cfg(test) module and everything below it are fixture code, not the
# shipped reducer/projection/render path. Keeping this second scan separate
# preserves exact line numbers and the older guard's string/comment behavior.
scan_capability_policy() {
  local violations=0 file production candidates rg_status rest line content stripped ident
  local files
  files="$(rg --files -g '*.rs' "$@")"
  rg_status=$?
  case "$rg_status" in
    0) ;;
    1) return 0 ;;
    *)
      printf 'check_no_name_enumerated_identity: rg failed (exit %s) while listing %s\n' \
        "$rg_status" "$*" >&2
      return 2
      ;;
  esac
  while IFS= read -r file; do
    [[ -n "$file" ]] || continue
    if ! production="$(perl -0pe 's/\n#\[cfg\(test\)\][\s\S]*\z//' "$file")"; then
      printf 'check_no_name_enumerated_identity: perl failed while trimming test code from %s\n' \
        "$file" >&2
      return 2
    fi
    candidates="$(printf '%s\n' "$production" | rg -n -e "$CAPABILITY_POLICY_FORBIDDEN")"
    rg_status=$?
    case "$rg_status" in
      0) ;;
      1) continue ;;
      *)
        printf 'check_no_name_enumerated_identity: rg failed (exit %s) while scanning %s\n' \
          "$rg_status" "$file" >&2
        return 2
        ;;
    esac
    while IFS= read -r rest; do
      [[ -n "$rest" ]] || continue
      line="${rest%%:*}"
      content="${rest#*:}"
      if ! stripped="$(printf '%s\n' "$content" | strip_noncode)"; then
        printf 'check_no_name_enumerated_identity: perl failed while stripping %s:%s\n' \
          "$file" "$line" >&2
        return 2
      fi
      if printf '%s\n' "$stripped" | rg -q -e "$CAPABILITY_POLICY_FORBIDDEN"; then
        while IFS= read -r ident; do
          printf '%s:%s: concrete capability policy identifier %s\n' "$file" "$line" "$ident"
        done < <(printf '%s\n' "$stripped" | rg -o -e "$CAPABILITY_POLICY_FORBIDDEN" | sort -u)
        violations=1
      fi
    done <<<"$candidates"
  done <<<"$files"
  return "$violations"
}

fail_with_guidance() {
  cat <<'EOF'

NO-NAME-ENUMERATION GUARD FAILED
The identifiers above reintroduce a name-enumerated effect or routing
identity into a bound context. The invariant "no name-enumerated effect or
routing identity" requires effects, slots, sends, and
returns to be addressed by index into descriptor-driven arrays, so adding a
registry entry never changes a Synth, Mixer, RealTime, or Control type.
Register concrete effects in src/adapter/ instead. MasterGainDb is the
single declared exception. If you believe a flagged identifier is legitimate,
amend the product invariant in DESIGN.md first — do not weaken this script.
EOF
  exit 1
}

if [[ "${1:-}" == "--self-test" ]]; then
  fixture_root="$(mktemp -d "${TMPDIR:-/tmp}/crest-name-enum-guard.XXXXXX")"
  trap 'rm -rf "$fixture_root"' EXIT

  mkdir -p "$fixture_root/violation" "$fixture_root/allowed"
  cat >"$fixture_root/violation/reintroduced.rs" <<'EOF'
pub enum MixerTrackParameter {
    Level,
    ReverbSend,
}
pub struct GlobalReverbDelay {
    delay_feedback: f32,
}
pub fn process(reverb_input: &[f32]) {}
pub const SAMPLE_CAPABILITY_ID: &str = "instrument.sample";
EOF
  cat >"$fixture_root/allowed/exceptions.rs" <<'EOF'
/// The retired `ReverbSend` and `GlobalEffectsProcessor` may be narrated
/// in comments; history is not identity.
pub enum GlobalParameter {
    MasterGainDb,
}
pub struct GlobalParameters {
    master_gain_db: f32,
}
pub enum EffectError {
    InvalidMaxDelayMilliseconds,
}
pub const fn reverb_input_rms() -> f32 {
    0.0
}
pub const CAPABILITY: &str = "effect.reverb";
pub const LABEL: &str = "PATCH Lead\n> reverbSend=0.4\nGLOBAL";
EOF

  violation_output="$(scan "$fixture_root/violation")"
  case "$?" in
    0)
      echo 'self-test: seeded reintroduction was NOT detected' >&2
      exit 1
      ;;
    1) ;;
    *)
      echo 'self-test: the scanner failed while reading the seeded fixture' >&2
      exit 2
      ;;
  esac
  for expected in \
    'reintroduced.rs:3: name-enumerated identifier ReverbSend' \
    'reintroduced.rs:5: name-enumerated identifier GlobalReverbDelay' \
    'reintroduced.rs:6: name-enumerated identifier delay_feedback' \
    'reintroduced.rs:8: name-enumerated identifier reverb_input'; do
    if ! printf '%s\n' "$violation_output" | rg -Fq "$expected"; then
      echo "self-test: expected violation '$expected' was not reported" >&2
      printf '%s\n' "$violation_output" >&2
      exit 1
    fi
  done
  capability_output="$(scan_capability_policy "$fixture_root/violation")"
  case "$?" in
    1) ;;
    0)
      echo 'self-test: seeded concrete capability policy was NOT detected' >&2
      exit 1
      ;;
    *) exit 2 ;;
  esac
  if ! printf '%s\n' "$capability_output" | rg -Fq \
    'reintroduced.rs:9: concrete capability policy identifier SAMPLE_CAPABILITY_ID'; then
    echo 'self-test: concrete capability policy report was incomplete' >&2
    printf '%s\n' "$capability_output" >&2
    exit 1
  fi

  allowed_output="$(scan "$fixture_root/allowed")"
  case "$?" in
    0) ;;
    1)
      echo 'self-test: documented exceptions were incorrectly flagged' >&2
      printf '%s\n' "$allowed_output" >&2
      exit 1
      ;;
    *)
      echo 'self-test: the scanner failed while reading the exception fixture' >&2
      exit 2
      ;;
  esac

  echo 'CREST_STATIC_VALIDATION no_name_enumerated_identity self-test passed'
  exit 0
fi

# report <dir>... — scan and turn the three scan outcomes into the three
# declared exit codes; a scanner failure never prints the pass marker.
report() {
  local violations=0
  scan "$@"
  case "$?" in
    0) ;;
    1) violations=1 ;;
    *) exit 2 ;;
  esac
  scan_capability_policy "$@"
  case "$?" in
    0) ;;
    1) violations=1 ;;
    *) exit 2 ;;
  esac
  [[ "$violations" -eq 0 ]] || fail_with_guidance
  echo 'CREST_STATIC_VALIDATION no_name_enumerated_identity passed'
  exit 0
}

# The legacy effect/bus vocabulary binds the four domain roots. Phase 7's
# concrete-capability policy additionally binds the renderer and cumulative
# live orchestration, without retroactively applying retired telemetry names
# to those evidence modules.
report_production_scopes() {
  local violations=0
  scan src/synth src/mixer src/real_time src/control
  case "$?" in
    0) ;;
    1) violations=1 ;;
    *) exit 2 ;;
  esac
  scan_capability_policy \
    src/synth \
    src/mixer \
    src/real_time \
    src/control \
    src/testing/live_demo_scene.rs \
    src/testing/live_demo_runner.rs \
    src/testing/live_demo_report.rs \
    src/shell/component_vocabulary.rs \
    src/shell/standalone_application.rs
  case "$?" in
    0) ;;
    1) violations=1 ;;
    *) exit 2 ;;
  esac
  [[ "$violations" -eq 0 ]] || fail_with_guidance
  echo 'CREST_STATIC_VALIDATION no_name_enumerated_identity passed'
  exit 0
}

if [[ "${1:-}" == "--scan-root" ]]; then
  if [[ "$#" -ne 2 || ! -d "$2" ]]; then
    echo 'usage: check_no_name_enumerated_identity.sh --scan-root <dir>' >&2
    exit 2
  fi
  report "$2"
fi

if [[ "$#" -ne 0 ]]; then
  echo 'usage: check_no_name_enumerated_identity.sh [--self-test | --scan-root <dir>]' >&2
  exit 2
fi

report_production_scopes
