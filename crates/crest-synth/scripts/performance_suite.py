#!/usr/bin/env python3
"""Run existing production workloads with timing gates and separate Samply profiles."""
import argparse
from datetime import datetime, timezone
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
import time

from performance_report import compare, profile_summary, read_matrix, render_report, write_json

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "scripts/performance_suite.json"
OUTPUT = ROOT / "target/performance"
PROFILER = OUTPUT / "tools/bin/samply"


def select_cases(manifest, groups, patterns):
    cases = manifest["cases"]
    for group in groups:
        if group not in {case["group"] for case in cases}:
            raise ValueError(f"unknown group: {group}")
    pool = [case for case in cases if not groups or case["group"] in groups]
    for pattern in patterns:
        if not any(fnmatch.fnmatchcase(case["id"], pattern) for case in pool):
            raise ValueError(f"case selection matched nothing: {pattern}")
    selected = [case for case in pool if not patterns or any(fnmatch.fnmatchcase(case["id"], p) for p in patterns)]
    if not selected:
        raise ValueError("empty suite selection")
    return selected


def capture(command):
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(f"command failed: {command}: {result.stderr[-500:]}")
    return result.stdout.strip()


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def source_fingerprint():
    digest = hashlib.sha256()
    paths = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=ROOT)
    for name in sorted(set(paths.split(b"\0")) - {b""}):
        path = ROOT / os.fsdecode(name)
        digest.update(name + b"\0")
        digest.update(path.read_bytes() if path.is_file() else b"<absent>")
    return digest.hexdigest()


def metadata(manifest, args):
    cpu = capture(["sysctl", "-n", "machdep.cpu.brand_string"]) if sys.platform == "darwin" else platform.processor()
    workspace = Path(json.loads(capture(["cargo", "metadata", "--format-version", "1", "--no-deps"]))["workspace_root"])
    return {
        "host": {"os": platform.system(), "release": platform.release(), "arch": platform.machine(),
                 "cpu": cpu, "logical_cpus": os.cpu_count()},
        "rustc": capture(["rustc", "--version", "--verbose"]), "cargo": capture(["cargo", "--version"]),
        "python": platform.python_version(), "revision": capture(["git", "rev-parse", "HEAD"]),
        "dirty": bool(capture(["git", "status", "--porcelain"])), "source_sha256": source_fingerprint(),
        "manifest_sha256": sha256(MANIFEST), "lock_sha256": sha256(workspace / "Cargo.lock"),
        "build": {"profile": "profiling", "cargo_toml_sha256": sha256(ROOT / "Cargo.toml"),
                  "workspace_cargo_toml_sha256": sha256(workspace / "Cargo.toml"),
                  "environment": {k: v for k, v in os.environ.items()
                                  if k in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CFLAGS", "CXXFLAGS", "CC", "CXX")
                                  or k.startswith("CARGO_PROFILE_")}},
        "profiler_spec": manifest["profiler"], "repetitions": args.repetitions,
        "profiling_required": not args.timings_only,
        "invocation": sys.argv,
        "concurrent_load_note": getattr(args, "concurrent_load_note", "not controlled or measured"),
        "soak": {"seconds": getattr(args, "soak_seconds", None),
                 "voices_per_patch": getattr(args, "soak_voices_per_patch", None)},
    }


def execute(command, directory, env, timeout):
    """Capture one owned process group and per-child wait4 resource accounting."""
    directory.mkdir(parents=True, exist_ok=True)
    start = time.monotonic()
    timed_out = False
    with (directory / "stdout.log").open("wb") as stdout, (directory / "stderr.log").open("wb") as stderr:
        process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=stdout, stderr=stderr, start_new_session=True)
        try:
            while True:
                pid, status, usage = os.wait4(process.pid, os.WNOHANG)
                if pid:
                    break
                if time.monotonic() - start > timeout:
                    timed_out = True
                    os.killpg(process.pid, signal.SIGKILL)
                    _, status, usage = os.wait4(process.pid, 0)
                    break
                time.sleep(0.02)
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            _, status, _ = os.wait4(process.pid, 0)
            process.returncode = os.waitstatus_to_exitcode(status)
            raise
        process.returncode = os.waitstatus_to_exitcode(status)
    return {
        "exit_code": process.returncode, "timed_out": timed_out,
        "resources": {"elapsed_seconds": time.monotonic() - start,
                      "user_seconds": usage.ru_utime, "system_seconds": usage.ru_stime,
                      "peak_rss_bytes": usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024),
                      "minor_faults": usage.ru_minflt, "major_faults": usage.ru_majflt,
                      "voluntary_context_switches": usage.ru_nvcsw,
                      "involuntary_context_switches": usage.ru_nivcsw},
    }


def observations(directory):
    records = []
    texts = []
    for name in ("stdout.log", "stderr.log"):
        text = (directory / name).read_text(errors="replace")
        texts.append(text)
        for line in text.splitlines():
            match = re.search(r"\b(CREST_[A-Z0-9_]+)\s+(.*)", line)
            if match:
                marker, payload = match.groups()
                try:
                    payload = json.loads(payload)
                except json.JSONDecodeError:
                    pass
                records.append({"marker": marker, "payload": payload, "stream": name})
    write_json(directory / "observations.json", records)
    return "\n".join(texts), {record["marker"] for record in records}


def validate_execution(case, result, directory, profiled):
    errors = []
    if result["timed_out"]:
        errors.append("workload watchdog expired; owned process group terminated")
    if result["exit_code"] != 0:
        errors.append(f"workload exited {result['exit_code']}")
    text, markers = observations(directory)
    if case["kind"] == "test" and not case.get("custom_harness"):
        summaries = re.findall(r"test result: (\w+)\. (\d+) passed; (\d+) failed", text)
        expected = case["profile_iterations"] if profiled else 1
        if len(summaries) != expected or any(status != "ok" or int(passed) == 0 or int(failed) for status, passed, failed in summaries):
            errors.append(f"expected {expected} nonempty passing libtest result(s); found {summaries}")
    for marker in case.get("required_markers", []):
        if marker not in markers:
            errors.append(f"missing completion evidence: {marker}")
    for artifact in case.get("required_artifacts", []):
        try:
            if not json.loads((directory / artifact).read_text()):
                raise ValueError("empty artifact")
        except (OSError, ValueError) as error:
            errors.append(f"invalid {artifact}: {error}")
    if case.get("matrix"):
        try:
            result["matrix"] = read_matrix(directory / "matrix.json", profiled)
            if not profiled:
                errors += [f"timing budget failed: {row['name']}" for row in result["matrix"]["rows"] if not row["passed"]]
            workload = result["matrix"].get("concurrent_workload") or {}
            strict = workload.get("strict_tracker_target")
            if strict is not None and not strict.get("passed", False):
                errors.append("strict Tracker callback execution target failed (70% p99, zero deadline misses, zero Rust allocations/deallocations)")
        except (OSError, ValueError, KeyError, TypeError) as error:
            errors.append(f"invalid matrix evidence: {error}")
    return errors


def build(cases, directory):
    command = ["cargo", "test", "--profile", "profiling", "--no-run", "--message-format=json"]
    for target in sorted({case["target"] for case in cases if case["kind"] == "test"}):
        command += ["--test", target]
    # Integration-test compilation also builds the real binary. Explicitly build
    # bins when the selection contains only standalone scenes.
    if not any(case["kind"] == "test" for case in cases):
        command = ["cargo", "build", "--profile", "profiling", "--bins", "--message-format=json"]
    print("Building optimized, symbolized workload executables...", flush=True)
    with (directory / "build.stdout.jsonl").open("w") as stdout, (directory / "build.stderr.log").open("w") as stderr:
        result = subprocess.run(command, cwd=ROOT, stdout=stdout, stderr=stderr)
    write_json(directory / "build-command.json", command)
    if result.returncode:
        raise RuntimeError("profiling build failed; see build.stderr.log")
    executables = {}
    for line in (directory / "build.stdout.jsonl").read_text().splitlines():
        artifact = json.loads(line)
        if artifact.get("reason") == "compiler-artifact" and artifact.get("executable"):
            target = artifact["target"]
            if "test" in target["kind"]:
                executables[("test", target["name"])] = artifact["executable"]
            elif "bin" in target["kind"] and not artifact["profile"]["test"]:
                executables[("bin", target["name"])] = artifact["executable"]
    for case in cases:
        if (case["kind"], case["target"]) not in executables:
            raise RuntimeError(f"Cargo did not produce executable for {case['id']}")
    return executables


def interactive_blocker():
    if sys.platform == "darwin":
        import plistlib
        result = subprocess.run(["ioreg", "-a", "-l", "-d", "1"], capture_output=True, timeout=10)
        if result.returncode:
            return "cannot inspect interactive session"
        roots = plistlib.loads(result.stdout)
        roots = roots if isinstance(roots, list) else [roots]
        users = [user for root in roots for user in root.get("IOConsoleUsers", [])]
        if not users:
            return "no logged-in console session"
        if any(user.get("CGSSessionScreenIsLocked") for user in users):
            return "macOS display is locked; native paint and physical scenes require an unlocked interactive session"
    elif not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")):
        return "no interactive display session"
    return None


def run_trial(case, executable, run_directory, label, profiled, manifest):
    directory = run_directory / case["id"] / label
    directory.mkdir(parents=True)
    # Prevent shell leftovers from silently changing the workload or disabling gates.
    env = {key: value for key, value in os.environ.items() if not key.startswith("CREST_")}
    overrides = {**case.get("env", {}), "CREST_PERFORMANCE_EVIDENCE_DIR": str(directory),
                 "CREST_PERFORMANCE_REPORT": str(directory / "matrix.json"),
                 "CREST_PERFORMANCE_PROFILE": "1" if profiled else "0"}
    env.update(overrides)
    command = [executable, *case["args"]]
    if profiled:
        command = [str(PROFILER), "record", "--save-only", "--unstable-presymbolicate",
                   "--rate", str(manifest["profiler"]["rate_hz"]), "--iteration-count", str(case["profile_iterations"]),
                   "--profile-name", case["id"], "--output", str(directory / "profile.json.gz"), "--", *command]
    result = {"directory": str(directory.relative_to(run_directory)), "command": command,
              "environment_overrides": overrides, "profiled": profiled, "errors": []}
    write_json(directory / "execution.json", result)
    try:
        result.update(execute(command, directory, env, case["timeout_seconds"] * (case["profile_iterations"] if profiled else 1)))
        result["errors"] += validate_execution(case, result, directory, profiled)
        if profiled:
            result["resource_scope"] = "profiler process and its children; excluded from timing comparisons"
            result["summary"] = profile_summary(directory / "profile.json.gz")
            write_json(directory / "profile-summary.json", result["summary"])
    except (OSError, ValueError, KeyError, TypeError, IndexError) as error:
        result["errors"].append(str(error))
    result["status"] = "failed" if result["errors"] else "passed"
    write_json(directory / "execution.json", result)
    return result


def run_suite(args, manifest):
    if not hasattr(os, "wait4"):
        raise RuntimeError("Samply suite resource accounting requires macOS or Linux (os.wait4)")
    groups = args.group or ([] if args.case else ["headless"])
    cases = select_cases(manifest, groups, args.case)
    if getattr(args, "soak_seconds", None) is not None:
        if args.soak_seconds <= 0 or not 1 <= args.soak_voices_per_patch <= 92:
            raise ValueError("soak requires positive seconds and 1..92 voices per Patch (fixture MIDI pitches 36..127)")
        if not args.timings_only or args.repetitions != 1 or [case["id"] for case in cases] != ["stress.concurrent"]:
            raise ValueError("soak requires --case stress.concurrent --repetitions 1 --timings-only")
        case = dict(cases[0])
        case["env"] = {**case.get("env", {}), "CREST_PERFORMANCE_SOAK_SECONDS": str(args.soak_seconds),
                       "CREST_PERFORMANCE_SOAK_VOICES": str(args.soak_voices_per_patch)}
        case["timeout_seconds"] = args.soak_seconds + 120
        cases = [case]
    identifier = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    directory = OUTPUT / "runs" / identifier
    directory.mkdir(parents=True)
    run = {"schema_version": 2, "id": identifier, "status": "running", "errors": [],
           "metadata": metadata(manifest, args), "cases": []}
    write_json(directory / "workloads.json", manifest)
    write_json(directory / "run.json", run)
    try:
        if not args.timings_only:
            if not PROFILER.is_file():
                raise RuntimeError("Samply missing: run make performance-tools")
            version = capture([str(PROFILER), "--version"])
            if version.split()[-1] != manifest["profiler"]["version"]:
                raise RuntimeError(f"unexpected profiler version: {version}; run make performance-tools")
            run["metadata"]["profiler"] = version
        executables = build(cases, directory)
        run["metadata"]["executables"] = {f"{kind}:{name}": {"path": path, "sha256": sha256(path)}
                                                for (kind, name), path in executables.items()}
        for case in cases:
            result = {"id": case["id"], "group": case["group"], "status": "running", "timings": []}
            run["cases"].append(result)
            blocker = interactive_blocker() if case["group"] != "headless" else None
            if blocker:
                result.update(status="blocked", errors=[blocker])
                run["errors"].append(f"{case['id']}: {blocker}")
                print(f"BLOCKED {case['id']}: {blocker}", flush=True)
                continue
            executable = executables[(case["kind"], case["target"])]
            for trial in range(case.get("repetitions", args.repetitions)):
                print(f"TIMING {case['id']} {trial + 1}/{case.get('repetitions', args.repetitions)}", flush=True)
                result["timings"].append(run_trial(case, executable, directory, f"timing-{trial + 1}", False, manifest))
                render_report(run, directory)
            if not args.timings_only:
                print(f"PROFILE {case['id']}", flush=True)
                result["profile"] = run_trial(case, executable, directory, "profile", True, manifest)
            trials = result["timings"] + ([result["profile"]] if result.get("profile") else [])
            result["status"] = "failed" if any(trial["status"] != "passed" for trial in trials) else "passed"
            print(f"{result['status'].upper()} {case['id']}", flush=True)
            render_report(run, directory)
        run["status"] = "failed" if run["errors"] or any(case["status"] != "passed" for case in run["cases"]) else "passed"
        if args.baseline:
            baseline = json.loads(Path(args.baseline).read_text())
            run["comparison"] = compare(run, baseline, args.regression_percent)
            run["comparison"]["baseline_path"] = str(Path(args.baseline).resolve())
            if run["comparison"]["regressions"]:
                run["status"] = "failed"
        if args.timings_only and run["status"] == "passed":
            run["status"] = "timings-only"
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        run["status"] = "failed"
        run["errors"].append(str(error) or "interrupted")
    finally:
        try:
            run["metadata"]["source_after"] = {"revision": capture(["git", "rev-parse", "HEAD"]),
                                                "dirty": bool(capture(["git", "status", "--porcelain"])),
                                                "source_sha256": source_fingerprint()}
            run["metadata"]["source_changed_during_run"] = any(
                run["metadata"][key] != run["metadata"]["source_after"][key]
                for key in ("revision", "dirty", "source_sha256"))
        except (OSError, RuntimeError, KeyError) as error:
            run["errors"].append(f"could not fingerprint sources after run: {error}")
            run["status"] = "failed"
        render_report(run, directory)
        write_json(OUTPUT / "latest-suite.json", {"run": str(directory / "run.json"), "report": str(directory / "report.html")})
        print(f"{run['status'].upper()}: {directory / 'report.html'}", flush=True)
    return 0 if run["status"] in ("passed", "timings-only") else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("list", help="List workloads and existing source targets")
    commands.add_parser("install-profiler", help="Install the pinned Samply locally under target/")
    opening = commands.add_parser("open", help="Open a retained profile in Firefox Profiler")
    opening.add_argument("profile")
    run = commands.add_parser("run", help="Run headless suite by default; retain every result, including failures")
    run.add_argument("--group", action="append", default=[], choices=["headless", "native", "physical"])
    run.add_argument("--case", action="append", default=[], help="Case identity or glob; may repeat")
    run.add_argument("--repetitions", type=int, default=3)
    run.add_argument("--timings-only", action="store_true", help="Explicitly omit profiling; result is not a complete suite pass")
    run.add_argument("--baseline", help="Prior compatible run.json; regressions fail the run")
    run.add_argument("--regression-percent", type=float, default=20)
    run.add_argument("--soak-seconds", type=int, help="Continuous unprofiled stress.concurrent duration; never repeats/restarts the fixture")
    run.add_argument("--soak-voices-per-patch", type=int, default=4, help="Explicit held-note request per each of 16 Synth Patches during soak; actual active counts are recorded")
    run.add_argument("--concurrent-load-note", default="not controlled or measured", help="Describe other machine activity; timings are not isolated-host certification")
    args = parser.parse_args()
    manifest = json.loads(MANIFEST.read_text())
    if args.command == "list":
        for case in manifest["cases"]:
            print(f"{case['id']:26} {case['group']:9} {case['kind']}:{case['target']} {' '.join(case['args'])}")
        return 0
    if args.command == "install-profiler":
        return subprocess.call(["cargo", "install", "--locked", "--version", manifest["profiler"]["version"],
                                "--root", str(OUTPUT / "tools"), "samply"], cwd=ROOT)
    if args.command == "open":
        return subprocess.call([str(PROFILER), "load", str(Path(args.profile).resolve())], cwd=ROOT)
    if args.repetitions < 1 or not 0 < args.regression_percent < 1000:
        parser.error("repetitions must be positive; regression percent must be between 0 and 1000")
    try:
        return run_suite(args, manifest)
    except (ValueError, OSError, RuntimeError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    sys.exit(main())
