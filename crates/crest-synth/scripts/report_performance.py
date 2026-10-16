#!/usr/bin/env python3
"""Rank callback/control bottlenecks and compare matching workload identities."""
import argparse
import json
from pathlib import Path
import sys

from performance_report import compare, metrics


def load(path):
    report = json.loads(Path(path).read_text())
    if report.get("schema_version") != 1 or not report.get("rows"):
        raise ValueError(f"{path}: unsupported or empty performance report")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", nargs="?", default="target/performance/latest-suite.json")
    parser.add_argument("--baseline", help="Compare p99 against matching cases in an earlier report")
    args = parser.parse_args()
    report = json.loads(Path(args.report).read_text())
    if "run" in report:
        report = json.loads(Path(report["run"]).read_text())
    if report.get("schema_version") == 2:
        print(f"{report['status'].upper()}: {report['id']}; {len(report['cases'])} cases")
        for case in report["cases"]:
            summary = case.get("profile", {}).get("summary", {})
            print(f"{case['status'].upper()} {case['id']}: {len(case['timings'])} timing trials; "
                  f"{summary.get('project_stack_records', 0)} symbolized project stack records")
        print(f"{len(metrics(report))} comparable process and operation metrics")
        if args.baseline:
            comparison = compare(report, json.loads(Path(args.baseline).read_text()))
            print(f"{len(comparison['regressions'])} regressions; {comparison['method']}")
            for row in comparison['regressions']:
                print(f"REGRESSION {row['metric']}: {row['baseline']:.3f} -> {row['current']:.3f}")
            if comparison["regressions"]:
                return 1
        return 0 if report["status"] == "passed" else 1
    report = load(args.report)
    baseline = load(args.baseline) if args.baseline else None
    prior = {row["name"]: row for row in baseline["rows"]} if baseline else {}
    if baseline:
        for field in ("sample_rate", "arch", "os", "cpu", "rustc", "debug_assertions"):
            if baseline.get(field) != report.get(field):
                print(f"Comparison differs in {field}: {baseline.get(field)} -> {report.get(field)}")
    rows = report["rows"]
    failed = [row for row in rows if not row["passed"]]
    print(f"{len(rows)} workloads; {len(failed)} failed budgets; debug_assertions={report['debug_assertions']}")
    print(report["timing_scope"])
    print("Memory null means not measured (control/worker code may allocate).")
    print("\nFailing workloads first, then highest p99 / budget (at least 15 rows):")
    for row in sorted(rows, key=lambda row: (not row["passed"], row["p99_us"] / row["p99_budget_us"]), reverse=True)[:max(15, len(failed))]:
        comparison = ""
        if row["name"] in prior and row["p99_us"] > 0:
            comparison = f"; baseline/current={prior[row['name']]['p99_us'] / row['p99_us']:.2f}x"
        print(f"{'PASS' if row['passed'] else 'FAIL'} {row['name']}: p99={row['p99_us']:.1f} us; budget={row['p99_budget_us']:.1f} us; max={row['max_us']:.1f} us; misses={row['deadline_misses']}/{row['samples']}{comparison}")


if __name__ == "__main__":
    sys.exit(main())
