#!/usr/bin/env python3
"""Run native event-driven edit tours against portable songs; retain every result."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

from workload import ROOT, repository, write_artifact

SCENES = ("mixer", "edit", "navigation")
# Interactive playback budgets, not loading/preparation budgets. Percentiles
# are histogram upper bounds (0.1 ms bins), and max values remain exact.
BUDGETS = {
    "page_render": {"p95_ms": 16.667, "max_ms": 50.0},
    "window_frame": {"p95_ms": 16.667, "max_ms": 50.0},
    "host_tick": {"p95_ms": 8.0, "max_ms": 50.0},
    "request": {"p95_ms": 8.0, "max_ms": 50.0},
    "tick_gap": {"p99_ms": 50.0, "max_ms": 100.0},
}


def failures(scene, metrics):
    errors = []
    if scene.get("complete") is not True or scene.get("error"):
        errors.append(f"scene incomplete: {str(scene.get('error')).split(' · {')[0][:300]}")
    for field in ("final_voices", "routing_failures", "voice_refusals", "nonfinite_samples",
                  "late_events", "playback_recoveries", "clipped_samples"):
        if scene.get(field) != 0:
            errors.append(f"{field}={scene.get(field)}")
    if not any(scene.get("audible_tracks", [])):
        errors.append("no audible playback observed")
    if metrics.get("kind") != "summary":
        errors.append("missing final performance summary")
    for field in ("late_events", "routing_failures", "voice_refusals", "nonfinite_samples", "playback_recoveries"):
        if metrics.get(field) != 0:
            errors.append(f"metrics {field}={metrics.get(field)}")
    timings = metrics.get("timings", {})
    for stage, limits in BUDGETS.items():
        values = timings.get(stage, {}).get("playing", {})
        if values.get("count", 0) < 10:
            errors.append(f"insufficient playback samples for {stage}")
        for field, limit in limits.items():
            if values.get(field, float("inf")) > limit:
                errors.append(f"{stage}.{field}={values.get(field)} exceeds {limit}")
    audio = timings.get("audio_callback", {}).get("playing", {})
    if audio.get("count", 0) < 100:
        errors.append("insufficient callback samples")
    if audio.get("over_budget") != 0:
        errors.append(f"audio callback deadline misses={audio.get('over_budget')}")
    deadline = metrics.get("audio_deadline_ms", 0)
    if deadline <= 0 or audio.get("p99_ms", float("inf")) > deadline * .7:
        errors.append("callback p99 exceeds 70% of device buffer deadline")
    return errors


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path, help="Directory containing .crest-song saves (searched recursively)")
    parser.add_argument("--scene", action="append", choices=SCENES)
    parser.add_argument("--match", default="", help="Case-insensitive substring of song filename")
    parser.add_argument("--limit", type=int, help="Limit songs, preserving sorted order")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--binary", type=Path, help="Use an already-built native application")
    parser.add_argument("--timeout", type=float, default=180, help="Seconds allowed per scene")
    options = parser.parse_args(argv)
    if options.timeout <= 0 or (options.limit is not None and options.limit <= 0):
        parser.error("timeout and limit must be positive")
    songs = sorted(p.resolve() for p in options.corpus.expanduser().rglob("*.crest-song")
                   if options.match.casefold() in p.name.casefold())
    if options.limit:
        songs = songs[:options.limit]
    if not songs:
        parser.error("No matching .crest-song files")
    output = (options.output or ROOT / ".dev/stress" / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")).resolve()
    output.mkdir(parents=True, exist_ok=False)
    source = repository(ROOT)
    if options.binary:
        binary = options.binary.resolve()
    elif sys.platform == "darwin":
        subprocess.run([sys.executable, "scripts/bundle_macos.py", "--release"], cwd=ROOT, check=True)
        binary = ROOT / "target/release/Crest Tracker.app/Contents/MacOS/crest-tracker-app"
    else:
        subprocess.run(["cargo", "build", "--release", "-p", "crest-tracker-app"], cwd=ROOT, check=True)
        binary = ROOT / "target/release/crest-tracker-app"
    report = {"schema": 1, "host": platform.platform(), "source": source,
              "binary": str(binary), "binary_sha256": digest(binary), "budgets_ms": BUDGETS,
              "callback_p99_fraction": .7, "results": [], "complete": False}
    for song_index, song in enumerate(songs):
        before = digest(song)
        for scene in options.scene or SCENES:
            case = output / f"{song_index:03d}-{scene}"
            case.mkdir()
            started = time.monotonic()
            arguments = [str(binary), "--demo-stress", scene, str(song)]
            errors = []
            with (case / "scene.json").open("w") as stdout, (case / "stderr.log").open("w") as stderr:
                try:
                    result = subprocess.run(arguments, cwd=ROOT, env={**os.environ, "CREST_PERF_DIR": str(case)},
                                            stdout=stdout, stderr=stderr, timeout=options.timeout)
                    if result.returncode:
                        errors.append(f"process exit {result.returncode}")
                except subprocess.TimeoutExpired:
                    errors.append(f"timed out after {options.timeout} seconds")
            try:
                scene_report = json.loads((case / "scene.json").read_text())
                logs = list(case.glob("run-*.jsonl"))
                if len(logs) != 1:
                    raise ValueError("expected exactly one performance log")
                records = [json.loads(line) for line in logs[0].read_text().splitlines()]
                if records[0].get("debug_assertions") is not False:
                    errors.append("performance gates require an optimized build without debug assertions")
                metrics = records[-1]
                errors.extend(failures(scene_report, metrics))
            except (OSError, ValueError, IndexError) as error:
                errors.append(f"incomplete evidence: {error}")
                metrics = {}
            if digest(song) != before:
                errors.append("SOURCE SONG CHANGED")
            report["results"].append({"song": str(song), "song_sha256": before, "scene": scene,
                                      "seconds": time.monotonic() - started, "artifacts": str(case),
                                      "command": arguments, "failures": errors, "metrics": metrics})
            write_artifact(output / "report.json", report)
            print(f"{'FAIL' if errors else 'PASS'} {scene}: {song.name}" + (f" · {'; '.join(errors)}" if errors else ""), flush=True)
            if "SOURCE SONG CHANGED" in errors:
                raise RuntimeError("Stopped: source song changed")
    report["complete"] = True
    report["passed"] = all(not result["failures"] for result in report["results"])
    report["source_after"] = repository(ROOT)
    write_artifact(output / "report.json", report)
    print(f"{sum(not r['failures'] for r in report['results'])}/{len(report['results'])} passed · {output / 'report.json'}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
