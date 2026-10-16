//! No-name-enumeration guard — the open-closed property as a build gate.
//!
//! Product invariant:
//! no type in Synth, Mixer, RealTime, or Control may enumerate a variant,
//! field, or descriptor entry named after a specific effect or bus. Effects,
//! slots, sends, and returns are addressed by index into descriptor-driven
//! arrays, so adding a registry entry must require no change to any of these
//! types. The single declared exception is `MasterGainDb`: master gain is
//! genuinely global — a property of the master stage, not of any effect.
//!
//! Why this is a mechanical gate rather than prose: the closed enumerated
//! design (`MixerTrackParameter::ReverbSend`, `GlobalEffectsProcessor`,
//! `GlobalParameter::ReverbRoomSize`, ...) shipped even though `DESIGN.md`
//! declared the three-slot / eight-return expansion in advance. A design
//! document is a demonstrably insufficient control here, so a failed check —
//! not a reviewer's attention — is what stops the next closed shortcut.
//!
//! The scan itself lives in `scripts/check_no_name_enumerated_identity.sh`.
//! It matches the
//! retired identifiers exactly, in identifier position only, over the four
//! context trees. Deliberately excluded, with reasons:
//! - `src/adapter/*`: an adapter implementing reverb is supposed to say
//!   "reverb"; the invariant binds the four domain contexts, not the
//!   adapters that register concrete entries.
//! - comments and string literals: doc comments narrate the retired design
//!   as history; registry capability ids (`"effect.reverb"`) are the open
//!   registry working as designed; retained observation/telemetry labels are
//!   `logs_telemetry: do_not_change` in `occurrence_map.yaml`.
//! - composite identifiers (`reverb_input_rms`, the transitional telemetry
//!   accessors over generic `bus_input_rms` arrays;
//!   `InvalidMaxDelayMilliseconds`): word-boundary matching keeps them out.
//!
//! If this test fails on your change, do not weaken the script or this test:
//! address the effect or bus by index/registry entry instead, or — if you
//! believe the identifier is genuinely legitimate — amend the invariant in
//! `DESIGN.md` first.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PASS_MARKER: &str = "CREST_STATIC_VALIDATION no_name_enumerated_identity passed";
const SELF_TEST_MARKER: &str =
    "CREST_STATIC_VALIDATION no_name_enumerated_identity self-test passed";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run_guard(args: &[&str]) -> Output {
    Command::new("bash")
        .arg(repo_root().join("scripts/check_no_name_enumerated_identity.sh"))
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("the declared guard script must be runnable")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The script's declared exit code when a required external tool is absent.
/// It is deliberately neither `0` (clean) nor `VIOLATIONS_FOUND_EXIT`: an
/// unrun scan is a third answer, not a verdict about the tree.
const MISSING_TOOL_EXIT: i32 = 3;
/// The script's declared exit code when the scan ran and found identifiers.
const VIOLATIONS_FOUND_EXIT: i32 = 1;

/// Every external program the guard script invokes. Kept in step with the
/// script's own `REQUIRED_TOOLS`; a tool-less fixture is built by omitting
/// one entry from this list.
const GUARD_TOOLS: [&str; 7] = ["rg", "perl", "sort", "mktemp", "mkdir", "rm", "cat"];

fn tool_path(tool: &str) -> PathBuf {
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .unwrap_or_else(|error| panic!("resolving {tool} must be possible: {error}"));
    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    assert!(
        output.status.success() && !resolved.is_empty(),
        "the host must provide {tool} for the guard's tool-gate fixtures"
    );
    PathBuf::from(resolved)
}

/// Builds a `PATH` directory holding every guard tool except `withheld`, so
/// the gate is exercised against one specific absence rather than an empty
/// environment.
fn path_without(fixture_name: &str, withheld: &str) -> PathBuf {
    assert!(
        GUARD_TOOLS.contains(&withheld),
        "{withheld} is not one of the guard's declared tools"
    );
    let dir = fixture_dir(fixture_name);
    for tool in GUARD_TOOLS.iter().filter(|tool| **tool != withheld) {
        std::os::unix::fs::symlink(tool_path(tool), dir.join(tool))
            .expect("fixture tool link must be creatable");
    }
    dir
}

/// Runs the guard with an explicit `PATH`. `bash` is invoked by absolute path
/// so the interpreter itself stays resolvable while the script's own tools
/// are withheld.
fn run_guard_with_path(path: &Path) -> Output {
    Command::new(tool_path("bash"))
        .arg(repo_root().join("scripts/check_no_name_enumerated_identity.sh"))
        .current_dir(repo_root())
        .env("PATH", path)
        .output()
        .expect("the declared guard script must be runnable")
}

fn fixture_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("stale fixture directory must be removable");
    }
    fs::create_dir_all(&dir).expect("fixture directory must be creatable");
    dir
}

fn write_fixture(dir: &Path, file: &str, content: &str) {
    fs::write(dir.join(file), content).expect("fixture file must be writable");
}

/// The delivered tree satisfies the invariant: the validation command passes
/// and emits its success marker.
#[test]
fn guard_passes_on_the_delivered_tree() {
    let output = run_guard(&[]);
    let stdout = stdout_of(&output);
    assert!(
        output.status.success(),
        "guard failed on the delivered tree — an earlier change reintroduced \
         a name-enumerated identity:\n{stdout}"
    );
    assert!(
        stdout.contains(PASS_MARKER),
        "guard must emit the declared pass marker, got:\n{stdout}"
    );
}

/// Negative proof: a reintroduced retired variant is detected, and the
/// report names file, line, and identifier so the failure is actionable.
#[test]
fn guard_fails_on_a_reintroduced_variant_with_file_line_and_identifier() {
    let dir = fixture_dir("name_enum_guard_reintroduced_variant");
    write_fixture(
        &dir,
        "mixer_track_parameters.rs",
        "pub enum MixerTrackParameter {\n    Level,\n    ReverbSend,\n}\n",
    );
    let output = run_guard(&["--scan-root", dir.to_str().unwrap()]);
    let stdout = stdout_of(&output);
    assert!(
        !output.status.success(),
        "guard accepted a reintroduced ReverbSend variant:\n{stdout}"
    );
    assert!(
        stdout.contains("mixer_track_parameters.rs:3: name-enumerated identifier ReverbSend"),
        "failure report must name file, line, and identifier, got:\n{stdout}"
    );
}

/// Negative proof across all four retired rename families from
/// `plan.md` § Bulk Edit Classification.
#[test]
fn guard_fails_on_every_retired_identifier_family() {
    let dir = fixture_dir("name_enum_guard_all_families");
    write_fixture(
        &dir,
        "reintroduced.rs",
        concat!(
            "pub struct GlobalEffectsProcessor;\n",
            "pub struct GlobalReverbDelay;\n",
            "pub enum GlobalParameter {\n",
            "    DelayFeedback,\n",
            "}\n",
            "pub fn process(reverb_input: &[f32], delay_send: f32) {}\n",
        ),
    );
    let output = run_guard(&["--scan-root", dir.to_str().unwrap()]);
    let stdout = stdout_of(&output);
    assert!(
        !output.status.success(),
        "guard accepted retired identifiers:\n{stdout}"
    );
    for expected in [
        "reintroduced.rs:1: name-enumerated identifier GlobalEffectsProcessor",
        "reintroduced.rs:2: name-enumerated identifier GlobalReverbDelay",
        "reintroduced.rs:4: name-enumerated identifier DelayFeedback",
        "reintroduced.rs:6: name-enumerated identifier delay_send",
        "reintroduced.rs:6: name-enumerated identifier reverb_input",
    ] {
        assert!(
            stdout.contains(expected),
            "expected report line '{expected}' missing from:\n{stdout}"
        );
    }
}

/// False-positive boundary: `MasterGainDb` (the single declared exception),
/// comment narration, string-literal registry ids and telemetry labels, and
/// composite identifiers all pass cleanly.
#[test]
fn guard_allows_master_gain_and_the_documented_exempt_surfaces() {
    let dir = fixture_dir("name_enum_guard_exempt_surfaces");
    write_fixture(
        &dir,
        "exceptions.rs",
        concat!(
            "/// The retired `ReverbSend` and `GlobalEffectsProcessor` are\n",
            "/// narrated here as history, which is not identity.\n",
            "pub enum GlobalParameter {\n",
            "    MasterGainDb,\n",
            "}\n",
            "pub struct GlobalParameters {\n",
            "    master_gain_db: f32,\n",
            "}\n",
            "pub enum EffectError {\n",
            "    InvalidMaxDelayMilliseconds,\n",
            "}\n",
            "pub const fn reverb_input_rms() -> f32 {\n",
            "    0.0\n",
            "}\n",
            "pub const CAPABILITY: &str = \"effect.reverb\";\n",
            "pub const LABEL: &str = \"PATCH Lead\\n> reverbSend=0.4\\nGLOBAL\";\n",
        ),
    );
    let output = run_guard(&["--scan-root", dir.to_str().unwrap()]);
    let stdout = stdout_of(&output);
    assert!(
        output.status.success(),
        "guard flagged a documented exception:\n{stdout}"
    );
    assert!(stdout.contains(PASS_MARKER));
}

/// A missing scanner must not read as a clean tree. Without `rg` the guard
/// exits with its own missing-tool code, names the absent tool, and never
/// prints the pass marker — otherwise any
/// machine lacking ripgrep would satisfy the invariant vacuously.
#[test]
fn guard_refuses_to_run_without_ripgrep_rather_than_reporting_a_clean_tree() {
    let path = path_without("name_enum_guard_without_rg", "rg");
    let output = run_guard_with_path(&path);
    let stdout = stdout_of(&output);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(MISSING_TOOL_EXIT),
        "a missing scanner must exit {MISSING_TOOL_EXIT}, distinct from a clean pass (0) and \
         from a real violation ({VIOLATIONS_FOUND_EXIT}):\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("rg"),
        "the failure must name the absent tool, got:\n{stderr}"
    );
    assert!(
        !stdout.contains(PASS_MARKER),
        "an unrun scan must never emit the pass marker, got:\n{stdout}"
    );
    assert!(
        !stderr.contains("perl"),
        "the gate must name only the tool that is actually absent, got:\n{stderr}"
    );
}

/// The same gate covers the comment/string stripper: without `perl` the scan
/// would see every line as empty and report no candidates.
#[test]
fn guard_refuses_to_run_without_perl_rather_than_stripping_every_line_away() {
    let path = path_without("name_enum_guard_without_perl", "perl");
    let output = run_guard_with_path(&path);
    let stdout = stdout_of(&output);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(MISSING_TOOL_EXIT),
        "a missing stripper must exit {MISSING_TOOL_EXIT}:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("perl"),
        "the failure must name the absent tool, got:\n{stderr}"
    );
    assert!(
        !stdout.contains(PASS_MARKER),
        "an unrun scan must never emit the pass marker, got:\n{stdout}"
    );
}

/// The three outcomes stay three: the tool-less exit differs from the exit a
/// genuine reintroduction produces, so a caller can tell "not checked" from
/// "checked and dirty".
#[test]
fn missing_tool_and_found_violation_report_different_exit_codes() {
    let dir = fixture_dir("name_enum_guard_exit_code_separation");
    write_fixture(&dir, "reintroduced.rs", "pub struct GlobalReverbDelay;\n");
    let violation = run_guard(&["--scan-root", dir.to_str().unwrap()]);
    assert_eq!(
        violation.status.code(),
        Some(VIOLATIONS_FOUND_EXIT),
        "a found violation must exit {VIOLATIONS_FOUND_EXIT}, got:\n{}",
        stdout_of(&violation)
    );

    let path = path_without("name_enum_guard_exit_code_separation_bin", "rg");
    let missing_tool = run_guard_with_path(&path);
    assert_ne!(
        missing_tool.status.code(),
        violation.status.code(),
        "an unrun scan must not be reported with the verdict code of a scanned tree"
    );
}

/// The script's own `--self-test` (used at accept time without cargo)
/// observes the failure path and the exception path.
#[test]
fn guard_self_test_observes_its_own_failure_path() {
    let output = run_guard(&["--self-test"]);
    let stdout = stdout_of(&output);
    assert!(
        output.status.success(),
        "guard self-test failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(SELF_TEST_MARKER),
        "self-test must emit its marker, got:\n{stdout}"
    );
}
