#!/usr/bin/env python3
"""Measure and audit the pure Tracker scheduler against the sibling Synth checkout.

Artifacts include raw timing samples, exact command arguments, source identity,
and machine/toolchain metadata. This does not run native audio or measure device
latency. Requires Python 3.11+, Cargo, and existing dependencies; installs nothing.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import socket
import subprocess
import sys
import time

from develop import local_cargo

ROOT = Path(__file__).resolve().parents[1]


def utc():
    return datetime.now(timezone.utc).isoformat()


def command(arguments, directory=ROOT):
    return subprocess.run(arguments, cwd=directory, check=True, capture_output=True).stdout


def repository(directory):
    """Fingerprint tracked changes and all nonignored untracked file contents."""
    head = command(["git", "rev-parse", "HEAD"], directory).decode().strip()
    status = command(["git", "status", "--porcelain=v1", "-z", "--untracked-files=all"], directory)
    difference = command(["git", "diff", "--binary", "HEAD", "--"], directory)
    paths = command(["git", "ls-files", "--others", "--exclude-standard", "-z"], directory)
    digest = hashlib.sha256()
    for part in (head.encode(), status, difference):
        digest.update(len(part).to_bytes(8, "little"))
        digest.update(part)
    untracked = 0
    for encoded in sorted(filter(None, paths.split(b"\0"))):
        path = directory / os.fsdecode(encoded)
        digest.update(len(encoded).to_bytes(8, "little"))
        digest.update(encoded)
        if path.is_symlink():
            content = os.fsencode(os.readlink(path))
            digest.update(b"symlink\0" + len(content).to_bytes(8, "little") + content)
        else:
            digest.update(b"file\0" + path.stat().st_size.to_bytes(8, "little"))
            with path.open("rb") as source:
                for block in iter(lambda: source.read(1024 * 1024), b""):
                    digest.update(block)
        untracked += 1
    return {"path": str(directory), "head": head, "dirty": bool(status),
            "dirty_fingerprint_sha256": digest.hexdigest(), "untracked_files_hashed": untracked,
            "fingerprint_method": "HEAD + porcelain-v1 -z + binary diff HEAD + sorted nonignored untracked paths/contents",
            "status_porcelain": status.decode(errors="replace").split("\0")[:-1]}


def write_artifact(path, artifact):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + f".{os.getpid()}.tmp")
    try:
        temporary.write_text(json.dumps(artifact, indent=2) + "\n")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cycles", type=int, default=10_000)
    parser.add_argument("--synth-path", type=Path, default=ROOT.parent / "crest-synth")
    parser.add_argument("--output", type=Path)
    options = parser.parse_args(argv)
    if not 1 <= options.cycles <= 4_294_967_295:
        parser.error("--cycles must be in 1..4294967295 (the document repeat representation)")
    synth = options.synth_path.expanduser().resolve()
    if not (synth / "Cargo.toml").is_file():
        parser.error(f"Synth checkout is missing: {synth}")
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    output = (options.output or ROOT / "target" / "workload" / f"scheduler-{timestamp}.json").expanduser().resolve()
    arguments = local_cargo(synth, "run", "--release", "-p", "crest-tracker", "--example", "scheduler_workload", "--", "--cycles", str(options.cycles))
    artifact = {
        "schema_version": 1, "started_utc": utc(), "scope": "pure_scheduler",
        "invocation": {"wrapper_argv": [sys.executable, str(Path(__file__).resolve()), *(argv if argv is not None else sys.argv[1:])],
                       "cargo_argv": arguments, "cwd": str(ROOT), "output": str(output)},
        "host": {"hostname": socket.gethostname(), "platform": platform.platform(), "machine": platform.machine(),
                 "processor": platform.processor(), "logical_cpus": os.cpu_count(), "python": platform.python_version()},
        "toolchain": {"rustc": command(["rustc", "-Vv"]).decode().strip(), "cargo": command(["cargo", "-V"]).decode().strip()},
        "build_environment": {key: value for key, value in os.environ.items()
                              if key in {"RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR"} or key.startswith("CARGO_PROFILE_RELEASE_")},
        "repositories_before": {"tracker": repository(ROOT), "synth": repository(synth)},
        "audio_device": None, "sample_rate": None, "audio_block_frames": None,
    }
    started = time.monotonic_ns()
    result = subprocess.run(arguments, cwd=ROOT, capture_output=True, text=True)
    artifact.update({"finished_utc": utc(), "cargo_wall_ns_including_build": time.monotonic_ns() - started,
                     "exit_code": result.returncode, "cargo_stderr": result.stderr,
                     "repositories_after": {"tracker": repository(ROOT), "synth": repository(synth)}})
    artifact["source_changed_during_run"] = artifact["repositories_before"] != artifact["repositories_after"]
    try:
        artifact["measurement"] = json.loads(result.stdout) if result.returncode == 0 else None
        artifact["status"] = artifact["measurement"]["status"] if result.returncode == 0 else "failed"
    except (json.JSONDecodeError, KeyError, TypeError) as error:
        artifact.update({"status": "failed", "parse_error": str(error), "cargo_stdout": result.stdout})
    write_artifact(output, artifact)
    print(f"Scheduler workload {artifact['status']}: {output}")
    if artifact["source_changed_during_run"]:
        print("Sources changed during run; artifact records both fingerprints.", file=sys.stderr)
    if artifact["status"] != "passed":
        print(result.stderr[-4000:], file=sys.stderr)
        return result.returncode or 1
    measurement = artifact["measurement"]
    print(f"Compared {measurement['comparison']['events_compared']:,} complete events; "
          f"peak {measurement['audit']['simultaneous_voices_high_water']} voices; exact drift zero.")
    for run in measurement["runs"]:
        timing = run["window_timing_ns"]
        print(f"{run['name']}: p50/p95/p99/max window ns "
              f"{timing['p50']}/{timing['p95']}/{timing['p99']}/{timing['max']}; "
              f"digest {run['trace_digest_fnv1a64']}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"workload: {error}", file=sys.stderr)
        sys.exit(1)
