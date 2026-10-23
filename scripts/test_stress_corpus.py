import copy
import unittest

from stress_corpus import BUDGETS, failures


class StressGateTests(unittest.TestCase):
    def setUp(self):
        self.scene = dict(complete=True, error=None, final_voices=0, routing_failures=0,
                          voice_refusals=0, nonfinite_samples=0, late_events=0,
                          playback_recoveries=0, clipped_samples=0, audible_tracks=[True])
        timing = dict(count=200, p95_ms=1.0, p99_ms=1.0, max_ms=2.0, over_budget=0)
        self.metrics = {**self.scene, "kind": "summary", "audio_deadline_ms": 5.333,
                        "timings": {stage: {"playing": copy.deepcopy(timing)}
                                    for stage in [*BUDGETS, "audio_callback"]}}

    def test_complete_audible_run_with_headroom_passes(self):
        self.assertEqual(failures(self.scene, self.metrics), [])

    def test_recovered_audio_and_ui_stalls_still_fail(self):
        self.scene["playback_recoveries"] = 1
        self.metrics["timings"]["request"]["playing"]["max_ms"] = 150
        errors = failures(self.scene, self.metrics)
        self.assertTrue(any("playback_recoveries=1" in error for error in errors))
        self.assertTrue(any("request.max_ms" in error for error in errors))

    def test_one_callback_overrun_fails_despite_good_percentiles(self):
        self.metrics["timings"]["audio_callback"]["playing"]["over_budget"] = 1
        self.assertTrue(any("deadline misses=1" in error for error in failures(self.scene, self.metrics)))

    def test_silence_missing_metrics_and_truncated_log_fail(self):
        self.scene["audible_tracks"] = [False]
        self.metrics["kind"] = "sample"
        del self.metrics["timings"]["request"]
        errors = failures(self.scene, self.metrics)
        self.assertTrue(any("no audible" in error for error in errors))
        self.assertTrue(any("final performance summary" in error for error in errors))
        self.assertTrue(any("insufficient playback samples for request" in error for error in errors))

    def test_callback_requires_headroom(self):
        self.metrics["timings"]["audio_callback"]["playing"]["p99_ms"] = 5.0
        self.assertTrue(any("70%" in error for error in failures(self.scene, self.metrics)))


if __name__ == "__main__":
    unittest.main()
