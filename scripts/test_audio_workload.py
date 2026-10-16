"""Reject missing or misleading integrated-audio evidence."""
import json
from pathlib import Path
import tempfile
import unittest

from audio_workload import read_measurement


class AudioEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.addCleanup(self.root.cleanup)
        self.path = Path(self.root.name) / "measurement.json"
        self.measurement = {
            "scope": "integrated_tracker_headless_audio", "audio_seconds": 40,
            "performance_passed": False, "workload": "sustained",
            "evidence": {"complete": True, "final_voices": 0, "peak_voices": 64,
                         "routing_failures": 0, "voice_refusals": 0, "nonfinite_samples": 0,
                         "allocations": 0, "deallocations": 0, "audible_tracks": [True] * 16,
                         "sustained_64_blocks": 7400, "scheduled_late_events": 0,
                         "scheduled_max_lateness_frames": 0, "scheduled_queue_high_water": 97}}

    def read(self, strict=False):
        self.path.write_text(json.dumps(self.measurement))
        return read_measurement(self.path, 40, strict)

    def test_missing_malformed_and_absent_evidence_fail(self):
        with self.assertRaises(OSError):
            read_measurement(self.path, 40, False)
        self.path.write_text("{")
        with self.assertRaises(ValueError):
            read_measurement(self.path, 40, False)
        for value in [None, [], {}, {"evidence": None}]:
            self.measurement = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.read()

    def test_completed_safety_run_cannot_pass_strict_deadlines(self):
        self.assertEqual(self.read(), self.measurement)
        with self.assertRaises(ValueError):
            self.read(strict=True)
        self.measurement["performance_passed"] = True
        self.assertEqual(self.read(strict=True), self.measurement)

    def test_incomplete_wrong_duration_and_failed_cleanup_reject(self):
        self.measurement["audio_seconds"] = 3
        with self.assertRaises(ValueError):
            self.read()
        self.measurement["audio_seconds"] = 40
        for key, value in [("complete", False), ("final_voices", 1), ("peak_voices", 60),
                           ("allocations", 1), ("audible_tracks", [True] * 15 + [False]),
                           ("sustained_64_blocks", 10)]:
            previous = self.measurement["evidence"][key]
            self.measurement["evidence"][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.read()
            self.measurement["evidence"][key] = previous

    def test_mixed_run_requires_live_edits_monitor_cancellation_and_dense_attacks(self):
        self.measurement["workload"] = "mixed"
        self.measurement["audio_seconds"] = 45
        self.measurement["evidence"].update(peak_voices=96, live_edits=2,
                                             monitor_cancellations=3, dense_attacks=1400)
        self.path.write_text(json.dumps(self.measurement))
        self.assertEqual(read_measurement(self.path, 45, False, "mixed"), self.measurement)
        for key in ["live_edits", "monitor_cancellations", "dense_attacks"]:
            with self.subTest(key=key):
                value = self.measurement["evidence"].pop(key)
                self.path.write_text(json.dumps(self.measurement))
                with self.assertRaises(ValueError):
                    read_measurement(self.path, 45, False, "mixed")
                self.measurement["evidence"][key] = value
