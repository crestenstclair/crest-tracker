"""Controlled negatives for the performance suite, independent of host speed."""
import copy
import gzip
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from performance_report import compare, profile_summary, read_matrix, write_json
from performance_suite import execute, interactive_blocker, run_suite, select_cases, validate_execution


class SuiteTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.case = {"id": "controlled", "kind": "test", "profile_iterations": 1}

    def process(self, code, timeout=5):
        return execute([sys.executable, "-c", code], self.directory, os.environ.copy(), timeout)

    def test_failed_workload_is_retained_and_rejected(self):
        result = self.process("print('CREST_NEGATIVE {\"retained\": true}'); raise SystemExit(7)")
        errors = validate_execution(self.case, result, self.directory, False)
        self.assertIn("workload exited 7", errors)
        evidence = json.loads((self.directory / "observations.json").read_text())
        self.assertEqual(evidence[0]["payload"], {"retained": True})

    def test_complete_runner_fails_retains_artifacts_and_continues_other_cases(self):
        from types import SimpleNamespace
        manifest = {"profiler": {}, "cases": [
            {"id": "negative", "group": "headless", "kind": "test", "target": "python",
             "args": ["-c", "raise SystemExit(7)"], "timeout_seconds": 5},
            {"id": "positive", "group": "headless", "kind": "test", "target": "python",
             "args": ["-c", "print('test result: ok. 1 passed; 0 failed')"], "timeout_seconds": 5},
        ]}
        args = SimpleNamespace(group=[], case=[], repetitions=1, timings_only=True, baseline=None)
        with patch("performance_suite.OUTPUT", self.directory), patch("performance_suite.metadata", return_value={}), \
                patch("performance_suite.build", return_value={("test", "python"): sys.executable}):
            self.assertEqual(run_suite(args, manifest), 1)
        pointer = json.loads((self.directory / "latest-suite.json").read_text())
        report = json.loads(Path(pointer["run"]).read_text())
        self.assertEqual(report["status"], "failed")
        self.assertEqual([case["status"] for case in report["cases"]], ["failed", "passed"])
        self.assertTrue(Path(pointer["report"]).is_file())

    def test_wrong_libtest_filter_fails_even_with_zero_exit(self):
        result = self.process("print('test result: ok. 0 passed; 0 failed; 12 filtered out')")
        self.assertTrue(validate_execution(self.case, result, self.directory, False))

    def test_profiler_exit_zero_cannot_hide_failed_child_test(self):
        result = self.process("print('test result: FAILED. 0 passed; 1 failed')")
        self.assertTrue(validate_execution(self.case, result, self.directory, True))

    def test_all_profile_iterations_must_run(self):
        self.case["profile_iterations"] = 2
        result = self.process("print('test result: ok. 1 passed; 0 failed')")
        self.assertTrue(validate_execution(self.case, result, self.directory, True))

    def test_nonempty_success_and_resources(self):
        result = self.process("print('test result: ok. 4 passed; 0 failed')")
        self.assertEqual(validate_execution(self.case, result, self.directory, False), [])
        self.assertGreater(result["resources"]["peak_rss_bytes"], 0)
        self.assertGreater(result["resources"]["elapsed_seconds"], 0)

    def test_timeout_kills_owned_descendant_and_keeps_logs(self):
        marker = self.directory / "escaped.txt"
        child = f"import time; from pathlib import Path; time.sleep(0.5); Path({str(marker)!r}).write_text('escaped')"
        code = f"import subprocess,sys,time; subprocess.Popen([sys.executable,'-c',{child!r}]); print('started',flush=True); time.sleep(30)"
        result = self.process(code, timeout=0.15)
        self.assertTrue(result["timed_out"])
        self.assertLess(result["resources"]["elapsed_seconds"], 2)
        import time
        time.sleep(0.6)
        self.assertFalse(marker.exists())
        self.assertIn("started", (self.directory / "stdout.log").read_text())

    def test_custom_harness_requires_completion_evidence(self):
        self.case.update(custom_harness=True, required_markers=["CREST_FINISHED"])
        result = self.process("print('transport ready')")
        self.assertEqual(validate_execution(self.case, result, self.directory, False), ["missing completion evidence: CREST_FINISHED"])

    def test_required_scene_artifact_cannot_be_missing_or_empty(self):
        self.case.update(custom_harness=True, required_artifacts=["scene.json"])
        result = self.process("pass")
        self.assertTrue(validate_execution(self.case, result, self.directory, False))
        (self.directory / "scene.json").write_text("{}")
        self.assertTrue(validate_execution(self.case, result, self.directory, False))

    def test_selection_typos_fail_and_globs_reuse_existing_cases(self):
        manifest = {"cases": [{"id": "stress.graph", "group": "headless"}, {"id": "native.paint", "group": "native"}]}
        self.assertEqual(select_cases(manifest, [], ["stress.*"]), manifest["cases"][:1])
        for groups, patterns in [(["typo"], []), ([], ["typo"]), (["headless"], ["native.*"])]:
            with self.assertRaises(ValueError):
                select_cases(manifest, groups, patterns)

    def matrix(self):
        return {"schema_version": 1, "profiled": False, "debug_assertions": False, "rows": [{
            "name": "callback", "samples": 2, "durations_ns": [1000, 2000],
            "p50_us": 1, "p95_us": 2, "p99_us": 2, "passed": True,
            "allocations": 0, "deallocations": 0,
        }]}

    def test_raw_timings_must_agree_with_summary(self):
        path = self.directory / "matrix.json"
        matrix = self.matrix()
        write_json(path, matrix)
        self.assertEqual(read_matrix(path, False), matrix)
        matrix["rows"][0]["p99_us"] = 999
        write_json(path, matrix)
        with self.assertRaisesRegex(ValueError, "disagree"):
            read_matrix(path, False)

    def test_profiler_timings_cannot_count_as_budget_proof(self):
        path = self.directory / "matrix.json"
        matrix = self.matrix()
        matrix["profiled"] = True
        matrix["rows"][0]["passed"] = False
        write_json(path, matrix)
        self.assertEqual(read_matrix(path, True), matrix)
        with self.assertRaises(ValueError):
            read_matrix(path, False)
        matrix["rows"][0]["allocations"] = 1
        write_json(path, matrix)
        with self.assertRaisesRegex(ValueError, "allocation"):
            read_matrix(path, True)

    def test_failed_timing_rows_stay_in_aggregate_evidence(self):
        self.case["matrix"] = True
        matrix = self.matrix()
        matrix["rows"][0]["passed"] = False
        write_json(self.directory / "matrix.json", matrix)
        result = self.process("print('test result: ok. 1 passed; 0 failed')")
        self.assertIn("timing budget failed: callback", validate_execution(self.case, result, self.directory, False))
        self.assertEqual(result["matrix"], matrix)

    def profile(self):
        path = self.directory / "profile.json.gz"
        profile = {"meta": {"interval": 1}, "libs": [{"debugName": "fixture", "breakpadId": "A" * 32 + "0"}],
                   "threads": [{"name": "audio", "pid": 1, "tid": 2, "stringArray": ["0x10", "sleep"],
                                "frameTable": {"func": [0, 1], "address": [16, 32]},
                                "funcTable": {"name": [0, 1], "resource": [0, 0]},
                                "resourceTable": {"lib": [0]},
                                "stackTable": {"frame": [0, 1], "prefix": [None, None]},
                                "samples": {"stack": [0, 1], "threadCPUDelta": [1000, 0],
                                            "length": 2, "weight": [1, 100000]}}]}
        with gzip.open(path, "wt") as stream:
            json.dump(profile, stream)
        write_json(path.with_suffix(".syms.json"), {
            "string_table": ["crest_synth::real_time::AudioRenderer::render"],
            "data": [{"debug_name": "fixture", "debug_id": "A" * 32,
                      "symbol_table": [{"symbol": 0}], "known_addresses": [[16, 0]]}],
        })
        return path

    def test_real_symbol_lookup_excludes_coalesced_sleep_weights(self):
        summary = profile_summary(self.profile())
        self.assertEqual(summary["project_stack_records"], 1)
        self.assertEqual(summary["active_stack_records"], 1)
        self.assertEqual(summary["self"], [("crest_synth::real_time::AudioRenderer::render", 1)])

    def test_missing_corrupt_or_unmatched_symbols_fail(self):
        path = self.profile()
        sidecar = path.with_suffix(".syms.json")
        sidecar.unlink()
        with self.assertRaises(OSError):
            profile_summary(path)
        sidecar.write_text("{")
        with self.assertRaises(ValueError):
            profile_summary(path)
        write_json(sidecar, {"string_table": [], "data": []})
        with self.assertRaisesRegex(ValueError, "symbolized"):
            profile_summary(path)
        path.write_bytes(b"not a gzip profile")
        with self.assertRaises(OSError):
            profile_summary(path)

    def test_cyclic_profile_stack_is_rejected(self):
        path = self.profile()
        with gzip.open(path, "rt") as stream:
            profile = json.load(stream)
        profile["threads"][0]["stackTable"]["prefix"][0] = 0
        with gzip.open(path, "wt") as stream:
            json.dump(profile, stream)
        with self.assertRaisesRegex(ValueError, "cyclic"):
            profile_summary(path)

    def test_locked_display_is_blocked_for_both_ioreg_root_formats(self):
        import plistlib
        from types import SimpleNamespace
        for root in ({"IOConsoleUsers": [{"CGSSessionScreenIsLocked": True}]},
                     [{"IOConsoleUsers": [{"CGSSessionScreenIsLocked": True}]}]):
            result = SimpleNamespace(returncode=0, stdout=plistlib.dumps(root))
            with patch("performance_suite.sys.platform", "darwin"), patch("performance_suite.subprocess.run", return_value=result):
                self.assertIn("locked", interactive_blocker())
    def run_fixture(self):
        metadata = {field: "same" for field in ("host", "rustc", "build", "manifest_sha256", "lock_sha256", "repetitions")}
        trial = {"status": "passed", "resources": {"elapsed_seconds": 1, "user_seconds": 0.5,
                                                    "system_seconds": 0.1, "peak_rss_bytes": 10000000},
                 "matrix": self.matrix()}
        return {"status": "passed", "metadata": metadata, "cases": [{"id": "stress", "timings": [trial]}]}

    def test_regression_threshold_noise_floor_and_coverage(self):
        baseline = self.run_fixture()
        current = copy.deepcopy(baseline)
        current["cases"][0]["timings"][0]["resources"]["elapsed_seconds"] = 1.5
        result = compare(current, baseline)
        self.assertEqual([row["metric"] for row in result["regressions"]], ["stress/elapsed_seconds"])
        current["cases"][0]["timings"][0]["matrix"]["rows"][0]["p99_us"] = 3
        self.assertEqual(len(compare(current, baseline)["regressions"]), 1)
        current["cases"] = []
        with self.assertRaisesRegex(ValueError, "coverage"):
            compare(current, baseline)

    def test_incompatible_or_failed_baseline_cannot_pass(self):
        baseline = self.run_fixture()
        current = copy.deepcopy(baseline)
        baseline["metadata"]["host"] = "different CPU"
        with self.assertRaisesRegex(ValueError, "host"):
            compare(current, baseline)
        baseline = self.run_fixture()
        baseline["status"] = "failed"
        with self.assertRaisesRegex(ValueError, "passing"):
            compare(current, baseline)


if __name__ == "__main__":
    unittest.main()
