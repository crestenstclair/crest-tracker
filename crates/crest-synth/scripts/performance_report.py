"""Read Samply artifacts and summarize suite evidence; no synthetic profiling data."""
from collections import Counter, defaultdict
import gzip
import html
import json
import math
from pathlib import Path
import statistics


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def read_matrix(path, profiled):
    report = json.loads(Path(path).read_text())
    if (report.get("schema_version") != 1 or not report.get("rows")
            or report.get("profiled") is not profiled or report.get("debug_assertions") is not False):
        raise ValueError("missing, unoptimized, or incorrectly classified timing evidence")
    names = set()
    for row in report["rows"]:
        if row["name"] in names:
            raise ValueError("duplicate workload identity")
        names.add(row["name"])
        samples = row["durations_ns"]
        if (not samples or len(samples) != row["samples"]
                or any(type(n) is not int or n < 0 for n in samples)):
            raise ValueError("missing or invalid raw timing samples")
        ordered = sorted(samples)
        for percentile in (50, 95, 99):
            measured = ordered[math.ceil(len(samples) * percentile / 100) - 1] / 1000
            if not math.isclose(measured, row[f"p{percentile}_us"], abs_tol=0.001):
                raise ValueError("raw samples disagree with reported percentiles")
        if any(row[key] not in (None, 0) for key in ("allocations", "deallocations")):
            raise ValueError("callback allocation/deallocation contract failed")
    return report


def profile_summary(path):
    """Resolve sampled PCs using Samply's portable symbol sidecar.

    Count active stack records, not CPU percentages. On macOS Samply coalesces
    sleeping samples with large weights; those must not dominate hot functions.
    Firefox Profiler remains the authority for CPU deltas and full call trees.
    """
    path = Path(path)
    with gzip.open(path, "rt") as stream:
        profile = json.load(stream)
    symbols_path = path.with_suffix(".syms.json")
    symbols = json.loads(symbols_path.read_text())
    addresses = {}
    for library in symbols["data"]:
        table = library["symbol_table"]
        addresses[(library["debug_name"], library["debug_id"].replace("-", "").upper())] = {
            address: symbols["string_table"][table[index]["symbol"]]
            for address, index in library["known_addresses"] if index is not None
        }
    totals = Counter()
    self_totals = Counter()
    thread_summaries = []
    project_samples = 0
    for thread in profile["threads"]:
        strings = thread["stringArray"]
        frames = thread["frameTable"]
        funcs = thread["funcTable"]
        resources = thread["resourceTable"]
        frame_names = []
        for index, func in enumerate(frames["func"]):
            name = strings[funcs["name"][func]]
            resource = funcs["resource"][func]
            lib_index = resources["lib"][resource] if resource is not None and resource >= 0 else None
            if lib_index is not None:
                library = profile["libs"][lib_index]
                # Breakpad appends an age nibble; UUID in the sidecar does not.
                key = (library["debugName"], library["breakpadId"][:32].upper())
                name = addresses.get(key, {}).get(frames["address"][index], name)
            frame_names.append(name)
        samples = thread["samples"]
        prefixes = thread["stackTable"]["prefix"]
        if any(prefix is not None and (type(prefix) is not int or not 0 <= prefix < index)
               for index, prefix in enumerate(prefixes)):
            raise ValueError("invalid or cyclic profile stack table")
        stacks = Counter()
        deltas = samples.get("threadCPUDelta")
        for index, stack in enumerate(samples["stack"]):
            if stack is not None and (deltas is None or (deltas[index] or 0) > 0):
                stacks[stack] += 1
        inclusive = Counter()
        own = Counter()
        for stack, count in stacks.items():
            own[frame_names[thread["stackTable"]["frame"][stack]]] += count
            names = set()
            while stack is not None:
                names.add(frame_names[thread["stackTable"]["frame"][stack]])
                stack = prefixes[stack]
            for name in names:
                inclusive[name] += count
            if any("crest_synth::" in name or "braids::" in name for name in names):
                project_samples += count
        totals.update(inclusive)
        self_totals.update(own)
        thread_summaries.append({
            "name": thread["name"], "pid": thread["pid"], "tid": thread["tid"],
            "sample_records": samples["length"], "active_stack_records": sum(stacks.values()),
            "inclusive": inclusive.most_common(15), "self": own.most_common(15),
        })
    active = sum(thread["active_stack_records"] for thread in thread_summaries)
    if active == 0 or project_samples == 0:
        raise ValueError("profile has no active symbolized Crest Synth stacks")
    return {
        "format": "samply-firefox", "interval_ms": profile["meta"]["interval"],
        "units": "active sampled stack records; inclusive counts overlap; not CPU percentages",
        "active_stack_records": active, "project_stack_records": project_samples,
        "threads": thread_summaries, "inclusive": totals.most_common(30),
        "self": self_totals.most_common(30),
        "project_inclusive": [(name, count) for name, count in totals.most_common()
                              if "crest_synth::" in name or "braids::" in name][:30],
    }


def metrics(run):
    values = defaultdict(list)
    for case in run["cases"]:
        for trial in case["timings"]:
            if "resources" not in trial or trial.get("timed_out"):
                continue
            for field in ("elapsed_seconds", "user_seconds", "system_seconds", "peak_rss_bytes"):
                values[f"{case['id']}/{field}"].append(trial["resources"][field])
            for row in trial.get("matrix", {}).get("rows", []):
                values[f"{case['id']}/{row['name']}/p99_us"].append(row["p99_us"])
    return {name: {"median": statistics.median(samples), "min": min(samples),
                   "max": max(samples), "trials": len(samples)} for name, samples in values.items()}


def compare(current, baseline, percent=20):
    for field in ("host", "rustc", "build", "manifest_sha256", "lock_sha256", "repetitions"):
        if current["metadata"][field] != baseline["metadata"][field]:
            raise ValueError(f"incompatible baseline: {field}")
    if baseline["status"] != "passed":
        raise ValueError("baseline is not a passing run")
    now, before = metrics(current), metrics(baseline)
    if set(now) != set(before):
        raise ValueError("incompatible baseline: workload or metric coverage differs")
    differences = []
    for name, measurement in now.items():
        old, new = before[name]["median"], measurement["median"]
        floor = 50 if name.endswith("/p99_us") else (1024 * 1024 if name.endswith("/peak_rss_bytes") else 0.020)
        differences.append({"metric": name, "baseline": old, "current": new,
                            "change_percent": (new / old - 1) * 100 if old else None,
                            "regression": new > old * (1 + percent / 100) and new - old > floor})
    return {"threshold_percent": percent, "absolute_floors": "50 us p99; 20 ms process/CPU; 1 MiB RSS",
            "method": "median across independent unprofiled trials; screening gate, not significance test",
            "differences": differences, "regressions": [row for row in differences if row["regression"]]}


def render_report(run, directory):
    directory = Path(directory)
    write_json(directory / "run.json", run)
    build_profile = run["metadata"].get("build", {}).get("profile", "unspecified")
    lines = [f"# Crest Synth performance suite — {run['status']}", "",
             f"Run: `{run['id']}`. Build profile: `{build_profile}`. "
             f"Profiler: {run['metadata'].get('profiler', 'unavailable')}.", "",
             "Timing trials run without the profiler. Profiles are separate diagnostic executions. "
             "Process metrics include startup, assertions and evidence serialization. "
             "Callback/control metrics surround the production operations. "
             "Rust allocation counters do not intercept C/C++ allocation.", "",
             "[Machine-readable run](run.json) · [Build output](build.stderr.log)", ""]
    if run.get("errors"):
        lines += ["Errors: " + "; ".join(run["errors"]), ""]
    lines += ["| Workload | Status | Timing trials | Median process time | Peak RSS | Profile |",
              "| --- | --- | ---: | ---: | ---: | --- |"]
    for case in run["cases"]:
        successful = [trial for trial in case["timings"] if trial["status"] == "passed"]
        elapsed = f"{statistics.median(t['resources']['elapsed_seconds'] for t in successful):.3f} s" if successful else "—"
        rss = f"{max(t['resources']['peak_rss_bytes'] for t in successful) / 1048576:.1f} MiB" if successful else "—"
        profile = case.get("profile")
        link = f"[Samply]({profile['directory']}/profile.json.gz) · [symbols]({profile['directory']}/profile.json.syms.json)" if profile else "not collected"
        lines.append(f"| [{case['id']}]({case['id']}/) | {case['status']} | {len(successful)} | {elapsed} | {rss} | {link} |")
    lines += ["", "Open a profile with `python3 scripts/performance_suite.py open <profile.json.gz>`. "
              "Keep its adjacent `.syms.json` file. Firefox Profiler provides thread timelines, "
              "call trees, flame graphs and source navigation where local debug files remain available.", ""]
    comparison = run.get("comparison")
    if comparison:
        lines += [f"## Baseline comparison: {len(comparison['regressions'])} regressions", "",
                  comparison["method"] + ". Floors: " + comparison["absolute_floors"] + ".", ""]
        for row in comparison["regressions"]:
            lines.append(f"- `{row['metric']}`: {row['baseline']:.3f} → {row['current']:.3f} ({row['change_percent']:+.1f}%).")
    timing_metrics = {name: value for name, value in metrics(run).items() if name.endswith("/p99_us")}
    if timing_metrics:
        lines += ["", "## Highest operation p99", "", "Median p99 across trials; range shows trial variation.", "",
                  "| Operation | Median p99 | Range |", "| --- | ---: | ---: |"]
        for name, value in sorted(timing_metrics.items(), key=lambda item: item[1]["median"], reverse=True)[:25]:
            lines.append(f"| `{name}` | {value['median']:.1f} us | {value['min']:.1f}–{value['max']:.1f} us |")
    for case in run["cases"]:
        lines += ["", f"## {case['id']}", ""]
        for trial in case["timings"] + ([case["profile"]] if case.get("profile") else []):
            where = trial["directory"]
            lines.append(f"- [{where}]({where}/execution.json): {trial['status']}. "
                         f"[stdout]({where}/stdout.log) · [stderr]({where}/stderr.log) · "
                         f"[observations]({where}/observations.json). " + "; ".join(trial.get("errors", [])))
            if trial.get("matrix"):
                lines.append(f"  [Raw operation timings and budgets]({where}/matrix.json)")
                if not trial["profiled"]:
                    for row in trial["matrix"]["rows"]:
                        if not row["passed"]:
                            lines.append(f"  FAILED `{row['name']}`: p99 {row['p99_us']:.1f} us / "
                                         f"budget {row['p99_budget_us']:.1f} us; "
                                         f"{row['deadline_misses']}/{row['samples']} deadline misses.")
        summary = case.get("profile", {}).get("summary")
        if summary:
            lines += ["", f"{summary['active_stack_records']} active stack records; {summary['units']}.", "",
                      "| Sampled project function | Inclusive records |", "| --- | ---: |"]
            for name, count in summary["project_inclusive"][:8]:
                lines.append(f"| `{name}` | {count} |")
    markdown = "\n".join(lines) + "\n"
    (directory / "report.md").write_text(markdown)
    # A dependency-free HTML artifact: preserve Markdown text and make its links clickable.
    import re
    escaped = html.escape(markdown)
    linked = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", r'<a href="\2">\1</a>', escaped)
    (directory / "report.html").write_text(
        '<!doctype html><meta charset="utf-8"><title>Crest Synth performance suite</title>'
        '<style>body{margin:2rem;font:14px ui-monospace,monospace;background:#111;color:#eee}'
        'pre{white-space:pre-wrap;overflow-wrap:anywhere}a{color:#8cc8ff}</style><pre>' + linked + '</pre>')
