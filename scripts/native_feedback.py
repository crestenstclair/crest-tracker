#!/usr/bin/env python3
"""Measure native Tracker cursor or Drum hit-edit feedback through the real window.

This is an interactive witness, not a synthetic input or physical latency test.
The native application records receipt-to-post-frame acknowledgements. Close its
window after at least 100 changed keyboard actions on the selected surface during playback.
"""
import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import platform
import subprocess
import sys

from develop import ROOT
from workload import command, repository, utc, write_artifact


def read_measurement(path):
    """Recompute the gate from raw samples; a claimed pass is not sufficient."""
    report = json.loads(path.read_text())
    version = report.get("schema_version")
    scope = report.get("scope")
    if ((version, scope) not in ((1, "tracker_native_grid_feedback"), (2, "tracker_native_feedback"))
            or report.get("complete") is not True
            or report.get("failures") != []
            or report.get("capacity_exceeded") is not False):
        raise ValueError("Native feedback capture is incomplete or lost measured actions")
    samples = report.get("samples")
    if not isinstance(samples, list):
        raise ValueError("Native feedback capture has no raw samples")
    surface = report.get("surface", "grid")
    if (surface not in ("grid", "piano", "piano_selection", "piano_edit", "drum", "drum_edit", "timeline")
            or version == 1 and surface != "grid"
            or version == 2 and "surface" not in report):
        raise ValueError("Native feedback has no supported surface")
    sequences = set()
    playback, keyboard = [], []
    for sample in samples:
        sequence, elapsed = sample.get("sequence"), sample.get("receipt_to_ack_ns")
        state, bounds = sample.get("state", {}), sample.get("bounds", [])
        if (type(sequence) is not int or sequence <= 0 or sequence in sequences
                or type(elapsed) is not int or not 0 <= elapsed < 2_000_000_000
                or state.get("view") not in (("expanded", "minimap") if surface == "timeline" else ("song", "chain", "phrase"))
                or type(state.get("playing")) is not bool
                or sample.get("origin") not in ("native_keyboard", "webview")
                or len(bounds) != 4 or not all(type(v) in (int, float) and math.isfinite(v) for v in bounds)
                or bounds[0] < 0 or bounds[1] < 0 or bounds[2] <= 0 or bounds[3] <= 0):
            raise ValueError("Native feedback sample has invalid identity, timing or geometry")
        if version == 2:
            validate_state(state, surface)
        sequences.add(sequence)
        if state["playing"]:
            playback.append(elapsed)
            if sample["origin"] == "native_keyboard":
                keyboard.append(elapsed)
    if version == 2 and (type(report.get("attempts")) is not int
                         or type(report.get("no_changes")) is not int
                         or report["no_changes"] < 0
                         or report["attempts"] != len(samples) + report["no_changes"]
                         or max(sequences, default=0) > report["attempts"]):
        raise ValueError("Native feedback action accounting is incomplete")
    if surface == "timeline":
        for view in ("expanded", "minimap"):
            measured = timeline_playback_latencies(samples, view)
            if len(measured) < 50 or sorted(measured)[math.ceil(len(measured) * .95) - 1] >= 50_000_000:
                raise ValueError("Timeline requires 50 playback actions below 50 ms p95 in each view")
    if len(keyboard) < 100:
        raise ValueError("Need at least 100 changed native keyboard actions during playback")
    for values, count_key, percentile_key in (
        (playback, "playback_samples", "playback_p95_ns"),
        (keyboard, "native_keyboard_playback_samples", "native_keyboard_playback_p95_ns"),
    ):
        p95 = sorted(values)[math.ceil(len(values) * .95) - 1]
        if report.get(count_key) != len(values) or report.get(percentile_key) != p95:
            raise ValueError("Native feedback summary differs from its raw samples")
        if p95 >= 50_000_000:
            raise ValueError("Native feedback p95 did not meet the 50 ms target")
    if report.get("performance_passed") is not True:
        raise ValueError("Native feedback gate did not pass")
    return report


def timeline_playback_latencies(samples, view):
    """Select native-keyboard playback timings for one exact arrangement view."""
    return [sample["receipt_to_ack_ns"] for sample in samples
            if sample["origin"] == "native_keyboard" and sample["state"]["playing"]
            and sample["state"]["view"] == view]


def validate_state(state, surface):
    """Version 2 carries the concrete focus identity, separate from playback state."""
    maximum = 2**64 - 1
    integer = lambda value, low=0, high=maximum: type(value) is int and low <= value <= high
    focus = state.get("focus", {})
    if not integer(state.get("revision")) or focus.get("kind") != surface:
        raise ValueError("Native feedback surface or revision did not match")
    if surface == "grid":
        if not (integer(focus.get("row")) and integer(focus.get("column"))
                and isinstance(focus.get("label"), str) and focus["label"]):
            raise ValueError("Native grid feedback has invalid focus")
    elif surface in ("piano", "piano_selection"):
        at = focus.get("at", {})
        if not (state.get("view") == "phrase" and integer(focus.get("phrase"), 1)
                and integer(focus.get("pitch"), 0, 127)
                and integer(at.get("numerator")) and integer(at.get("denominator"), 1)):
            raise ValueError("Native Piano feedback has invalid exact cursor identity")
        if surface == "piano_selection" and ("selected" not in focus
                or focus["selected"] is not None and not integer(focus["selected"], 1)):
            raise ValueError("Native Piano selection feedback has invalid event identity")
    elif surface == "timeline":
        occurrence, start = focus.get("occurrence", {}), focus.get("start", {})
        if not (state.get("view") in ("expanded", "minimap") and integer(focus.get("phrase"), 1)
                and isinstance(occurrence, dict) and isinstance(start, dict)
                and all(integer(occurrence.get(key), 1) for key in ("track", "chain_use", "phrase_use"))
                and integer(start.get("numerator")) and integer(start.get("denominator"), 1)):
            raise ValueError("Native timeline feedback has invalid occurrence identity")
        if state["view"] == "expanded":
            at, cells = focus.get("at", {}), focus.get("cells")
            if not (integer(focus.get("row"), 0, 2**32 - 1) and isinstance(at, dict)
                    and integer(at.get("numerator")) and integer(at.get("denominator"), 1)
                    and isinstance(cells, list) and len(cells) == 6
                    and all(isinstance(cell, str) and cell for cell in cells)):
                raise ValueError("Native Expanded feedback has invalid exact row or cells")
        elif focus.get("row", False) is not None or focus.get("at", False) is not None or focus.get("cells") != []:
            raise ValueError("Native Minimap feedback must identify an occurrence without source cells")
    elif surface == "piano_edit":
        notes = focus.get("notes")
        if not (state.get("view") == "phrase" and integer(focus.get("phrase"), 1)
                and isinstance(notes, list) and notes):
            raise ValueError("Native Piano edit feedback has no selected notes")
        identities = set()
        for note in notes:
            if not isinstance(note, dict):
                raise ValueError("Native Piano edit feedback has invalid note data")
            at, duration = note.get("at", {}), note.get("duration", {})
            if not (integer(note.get("id"), 1) and note["id"] not in identities
                    and integer(note.get("pitch"), 0, 127) and integer(note.get("velocity"), 1, 127)
                    and integer(note.get("release_velocity"), 0, 127)
                    and isinstance(at, dict) and isinstance(duration, dict)
                    and integer(at.get("numerator")) and integer(at.get("denominator"), 1)
                    and integer(duration.get("numerator"), 1) and integer(duration.get("denominator"), 1)):
                raise ValueError("Native Piano edit feedback has invalid exact note identity or values")
            identities.add(note["id"])
    else:
        target = focus.get("target", {})
        valid_target = (target.get("kind") == "instrument" and integer(target.get("id"), 1)
                        or target.get("kind") == "channel" and (target.get("endpoint") is None
                            or isinstance(target.get("endpoint"), str)))
        if not (state.get("view") == "phrase" and integer(focus.get("phrase"), 1)
                and integer(focus.get("step"), 0, 2**32 - 1) and integer(focus.get("lane"))
                and integer(focus.get("pitch"), 0, 127) and integer(target.get("channel"), 1, 16)
                and valid_target):
            raise ValueError("Native Drum feedback has invalid step or lane identity")
        if surface == "drum_edit":
            hits = focus.get("hits")
            if (not isinstance(hits, list) or not all(integer(hit, 1) for hit in hits)
                    or len(hits) != len(set(hits))):
                raise ValueError("Native Drum edit feedback has invalid exact hit identities")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--synth-path", type=Path, default=ROOT.parent / "crest-synth")
    parser.add_argument("--verify-only", type=Path, help="validate an existing raw measurement")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--surface", choices=("grid", "piano", "piano_selection", "piano_edit", "drum", "drum_edit", "timeline"), default="grid",
                        help="require 100 native keyboard playback samples on this surface")
    parser.add_argument("--timeout", type=int, default=900, help="maximum interactive seconds")
    options = parser.parse_args(argv)
    if options.verify_only:
        try:
            measurement = read_measurement(options.verify_only)
        except (OSError, ValueError, TypeError) as error:
            print(str(error), file=sys.stderr)
            return 1
        print(f"Native {measurement.get('surface', 'grid')} feedback passed: {measurement['native_keyboard_playback_samples']} keyboard playback samples, p95 {measurement['native_keyboard_playback_p95_ns']/1e6:.3f} ms")
        return 0
    if options.timeout < 1:
        parser.error("--timeout must be positive")
    synth = options.synth_path.expanduser().resolve()
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    output = (options.output or ROOT / "target/workload" / f"feedback-{stamp}.json").resolve()
    measurement_path = output.with_name(f"{output.stem}.{stamp}.measurement.json")
    output.parent.mkdir(parents=True, exist_ok=True)
    # Use the real bundle identity/resources so native UI drivers can select the
    # same application users run. A bare Mach-O has no Launch Services identity.
    build = [sys.executable, str(ROOT / "scripts/bundle_macos.py"),
             "--synth-path", str(synth), "--release"]
    subprocess.run(build, cwd=ROOT, check=True)
    arguments = [str(ROOT / "target/release/Crest Tracker.app/Contents/MacOS/crest-tracker-app")]
    artifact = {"schema_version": 2, "scope": "tracker_native_feedback", "surface": options.surface, "started_utc": utc(),
                "command": arguments, "build_command": build, "measurement_path": str(measurement_path),
                "host": platform.platform(), "toolchain": command(["rustc", "-Vv"]).decode(),
                "repositories_before": {"tracker": repository(ROOT), "synth": repository(synth)}}
    description = {"grid": "navigate Song/Chain/step Phrase grids with W/A/S/D",
                   "piano": "navigate the Piano Roll time/pitch cursor with W/A/S/D",
                   "piano_selection": "cycle overlapping Piano notes with Return without editing the source",
                   "piano_edit": "edit selected visible Piano notes, or undo/redo a note edit with [ / ]",
                   "drum": "navigate the Drum step/lane cursor with W/A/S/D",
                   "drum_edit": "toggle Drum hits with Return, clear with E, or undo/redo with [ / ]",
                   "timeline": "navigate Expanded rows and Minimap occurrences with W/A/S/D (at least 50 playback actions in each view)"}[options.surface]
    print(f"Load a Song, start playback, {description} at least 100 times. Wait for visible feedback between actions. Close the window to finish.", flush=True)
    environment = dict(os.environ, CREST_TRACKER_FEEDBACK_REPORT=str(measurement_path),
                       CREST_TRACKER_FEEDBACK_SURFACE=options.surface)
    try:
        completed = subprocess.run(arguments, cwd=ROOT, env=environment, timeout=options.timeout,
                                   capture_output=True, text=True)
        artifact.update(exit_code=completed.returncode, stdout=completed.stdout, stderr=completed.stderr)
    except subprocess.TimeoutExpired:
        artifact.update(exit_code=1, measurement_error="Native interactive witness timed out")
    artifact.update(finished_utc=utc(), repositories_after={"tracker": repository(ROOT), "synth": repository(synth)})
    artifact["source_changed_during_run"] = artifact["repositories_before"] != artifact["repositories_after"]
    try:
        artifact["measurement"] = read_measurement(measurement_path)
        if artifact["measurement"].get("surface", "grid") != options.surface:
            raise ValueError("Captured surface differs from the requested witness")
    except (OSError, ValueError, TypeError) as error:
        artifact["measurement_error"] = str(error)
    artifact["status"] = "passed" if (artifact["exit_code"] == 0 and "measurement_error" not in artifact
                                      and not artifact["source_changed_during_run"]) else "failed"
    write_artifact(output, artifact)
    print(f"Native feedback {artifact['status']}: {output}")
    if "measurement_error" in artifact:
        print(artifact["measurement_error"], file=sys.stderr)
    return 0 if artifact["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
