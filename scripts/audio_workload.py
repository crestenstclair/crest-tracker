#!/usr/bin/env python3
"""Exercise Tracker's production host and Synth DSP without opening physical devices."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

from develop import ROOT, local_cargo
from workload import command, repository, utc, write_artifact


def read_measurement(path, seconds, strict, workload="sustained"):
    """Accept only a completed, safe workload with the requested duration."""
    measurement = json.loads(path.read_text())
    if not isinstance(measurement, dict):
        raise ValueError("Audio measurement must be an object")
    evidence = measurement.get("evidence")
    if not isinstance(evidence, dict):
        raise ValueError("Audio measurement has no evidence")
    expected = {"complete": True, "final_voices": 0,
                "routing_failures": 0, "voice_refusals": 0, "nonfinite_samples": 0,
                "allocations": 0, "deallocations": 0, "audible_tracks": [True] * 16,
                "scheduled_late_events": 0, "scheduled_max_lateness_frames": 0}
    if any(evidence.get(key) != value for key, value in expected.items()):
        raise ValueError("Audio workload did not complete its safety checks")
    if (measurement.get("scope") != "integrated_tracker_headless_audio"
            or measurement.get("audio_seconds") != seconds
            or not isinstance(evidence.get("sustained_64_blocks"), int)
            or measurement.get("workload") != workload
            or not isinstance(evidence.get("scheduled_queue_high_water"), int)
            or evidence["scheduled_queue_high_water"] <= 0):
        raise ValueError("Audio measurement does not cover the requested workload")
    if workload == "sustained":
        if evidence.get("peak_voices") != 64 or evidence["sustained_64_blocks"] <= seconds * 150:
            raise ValueError("Sustained workload did not maintain 64 voices")
    elif workload == "mixed":
        for key, minimum in [("peak_voices", 65), ("sustained_64_blocks", 1),
                             ("live_edits", 2), ("monitor_cancellations", 2), ("dense_attacks", 1000)]:
            if not isinstance(evidence.get(key), int) or evidence[key] < minimum:
                raise ValueError(f"Mixed workload lacks required coverage: {key}")
    else:
        raise ValueError("Unknown workload")
    if strict and measurement.get("performance_passed") is not True:
        raise ValueError("Strict callback deadline gate failed")
    return measurement


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workload", choices=["sustained", "mixed"], default="sustained")
    parser.add_argument("--seconds", type=int, default=45,
                        help="rendered audio duration, not wall-clock sleep (1..3600)")
    parser.add_argument("--strict", action="store_true",
                        help="also require zero callback deadline misses and p99 below 70%%")
    parser.add_argument("--synth-path", type=Path, default=ROOT.parent / "crest-synth")
    parser.add_argument("--output", type=Path)
    options = parser.parse_args(argv)
    if not 1 <= options.seconds <= 3600:
        parser.error("--seconds must be in 1..3600")
    if options.workload == "mixed" and options.seconds < 45:
        parser.error("Mixed workload requires at least 45 rendered seconds")
    synth = options.synth_path.expanduser().resolve()
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    output = (options.output or ROOT / "target/workload" / f"audio-{stamp}.json").expanduser().resolve()
    # A reused artifact path must never pick up a previous run's measurement.
    measurement_path = output.with_name(f"{output.stem}.{stamp}.measurement.json")
    output.parent.mkdir(parents=True, exist_ok=True)
    arguments = local_cargo(synth, "test", "--release", "-p", "crest-tracker-app",
                            ("sixteen_tracks_keep_sixty_four" if options.workload == "sustained"
                             else "live_edits_dense_retriggers_and_monitor_cancel"), "--", "--ignored", "--nocapture", "--test-threads=1")
    environment = dict(os.environ, CREST_AUDIO_SECONDS=str(options.seconds),
                       CREST_AUDIO_REPORT=str(measurement_path))
    environment.pop("CREST_AUDIO_STRICT", None)
    if options.strict:
        environment["CREST_AUDIO_STRICT"] = "1"
    artifact = {"schema_version": 1, "started_utc": utc(), "scope": "integrated_tracker_headless_audio",
                "command": arguments, "cwd": str(ROOT), "strict": options.strict, "workload": options.workload,
                "measurement_path": str(measurement_path),
                "host": {"platform": platform.platform(), "machine": platform.machine(), "logical_cpus": os.cpu_count()},
                "toolchain": command(["rustc", "-Vv"]).decode().strip(),
                "repositories_before": {"tracker": repository(ROOT), "synth": repository(synth)}}
    started = time.monotonic_ns()
    result = subprocess.run(arguments, cwd=ROOT, env=environment, capture_output=True, text=True)
    artifact.update({"finished_utc": utc(), "wall_ns_including_build": time.monotonic_ns() - started,
                     "exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr,
                     "repositories_after": {"tracker": repository(ROOT), "synth": repository(synth)}})
    artifact["source_changed_during_run"] = artifact["repositories_before"] != artifact["repositories_after"]
    try:
        artifact["measurement"] = read_measurement(measurement_path, options.seconds, False, options.workload)
        if options.strict and artifact["measurement"].get("performance_passed") is not True:
            artifact["measurement_error"] = "Strict callback deadline gate failed"
    except (OSError, ValueError) as error:
        artifact.update({"measurement": None, "measurement_error": str(error)})
    artifact["status"] = "passed" if (result.returncode == 0
        and artifact["measurement"] is not None and "measurement_error" not in artifact
        and not artifact["source_changed_during_run"]) else "failed"
    write_artifact(output, artifact)
    print(f"Integrated audio {artifact['status']}: {output}")
    print(result.stdout[-3000:])
    if result.returncode:
        print(result.stderr[-3000:], file=sys.stderr)
    if artifact["source_changed_during_run"]:
        print("Source changed during the run; repeat against stable sources.", file=sys.stderr)
    if "measurement_error" in artifact:
        print(artifact["measurement_error"], file=sys.stderr)
    return 0 if artifact["status"] == "passed" else result.returncode or 1


if __name__ == "__main__":
    sys.exit(main())
