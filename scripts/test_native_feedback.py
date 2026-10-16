"""Native feedback gates require actual correlated raw samples."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

from native_feedback import read_measurement


class NativeFeedbackTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "measurement.json"
        self.report = {
            "schema_version": 1, "scope": "tracker_native_grid_feedback", "complete": True,
            "failures": [], "capacity_exceeded": False, "performance_passed": True,
            "playback_samples": 100, "playback_p95_ns": 25_000_000,
            "native_keyboard_playback_samples": 100, "native_keyboard_playback_p95_ns": 25_000_000,
            "samples": [{"sequence": n + 1, "receipt_to_ack_ns": 25_000_000,
                         "origin": "native_keyboard", "state": {"view": "song", "playing": True},
                         "bounds": [10, 20, 100, 24]} for n in range(100)],
        }

    def read(self, report):
        self.path.write_text(json.dumps(report))
        return read_measurement(self.path)

    def test_complete_raw_evidence_passes(self):
        self.assertTrue(self.read(self.report)["performance_passed"])

    def test_legacy_schema_cannot_certify_surfaces_requiring_exact_focus(self):
        for surface in ("piano", "piano_selection", "piano_edit", "drum", "drum_edit", "timeline"):
            report = copy.deepcopy(self.report)
            report["surface"] = surface
            for index, sample in enumerate(report["samples"]):
                sample["state"]["view"] = ("expanded" if index < 50 else "minimap") if surface == "timeline" else "phrase"
            with self.subTest(surface=surface), self.assertRaisesRegex(ValueError, "surface"):
                self.read(report)
        self.report["surface"] = "grid"
        self.assertTrue(self.read(self.report)["performance_passed"])

    def test_piano_requires_exact_cursor_and_independent_surface_evidence(self):
        report = copy.deepcopy(self.report)
        report.update(schema_version=2, scope="tracker_native_feedback", surface="piano",
                      attempts=100, no_changes=0)
        for sample in report["samples"]:
            sample["state"].update(view="phrase", revision=4, focus={
                "kind": "piano", "phrase": 9007199254740993, "pitch": 67,
                "at": {"numerator": sample["sequence"], "denominator": 4}})
        self.assertTrue(self.read(report)["performance_passed"])
        for change in ("surface", "pitch", "fraction", "identity", "accounting"):
            broken = copy.deepcopy(report)
            focus = broken["samples"][0]["state"]["focus"]
            if change == "surface":
                broken["surface"] = "grid"
            elif change == "pitch":
                focus["pitch"] = 128
            elif change == "fraction":
                focus["at"]["denominator"] = 0
            elif change == "identity":
                focus["phrase"] = 2**64
            else:
                broken["attempts"] = 101
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.read(broken)

    def test_timeline_requires_both_views_exact_occurrences_and_independent_latency(self):
        report = copy.deepcopy(self.report)
        report.update(schema_version=2, scope="tracker_native_feedback", surface="timeline",
                      attempts=100, no_changes=0)
        for index, sample in enumerate(report["samples"]):
            expanded = index < 50
            sample["state"].update(view="expanded" if expanded else "minimap", revision=4, focus={
                "kind": "timeline", "phrase": 104,
                "occurrence": {"track": 1, "chain_use": 9007199254740993, "phrase_use": 18},
                "start": {"numerator": 0, "denominator": 1},
                "row": index if expanded else None,
                "at": {"numerator": index, "denominator": 4} if expanded else None,
                "cells": ["C4", "17", "80%", "—", "—", "—"] if expanded else []})
        self.assertTrue(self.read(report)["performance_passed"])
        for field, value in (("phrase", 0), ("row", None), ("at", None), ("cells", ["C4"]),
                             ("start", {"numerator": 0, "denominator": 0}),
                             ("occurrence", {"track": 1, "chain_use": 2**64, "phrase_use": 18})):
            broken = copy.deepcopy(report)
            broken["samples"][0]["state"]["focus"][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.read(broken)
        missing_view = copy.deepcopy(report)
        for sample in missing_view["samples"][50:]:
            sample["state"] = copy.deepcopy(missing_view["samples"][0]["state"])
        with self.assertRaisesRegex(ValueError, "each view"):
            self.read(missing_view)
        slow_view = copy.deepcopy(report)
        for sample in slow_view["samples"][:3]:
            sample["receipt_to_ack_ns"] = 51_000_000
        with self.assertRaisesRegex(ValueError, "each view"):
            self.read(slow_view)

    def test_drum_requires_exact_step_pitch_and_output_identity(self):
        report = copy.deepcopy(self.report)
        report.update(schema_version=2, scope="tracker_native_feedback", surface="drum",
                      attempts=100, no_changes=0)
        for sample in report["samples"]:
            sample["state"].update(view="phrase", revision=4, focus={
                "kind": "drum", "phrase": 104, "step": sample["sequence"], "lane": 1, "pitch": 36,
                "target": {"kind": "instrument", "id": 9007199254740993, "channel": 2}})
        self.assertTrue(self.read(report)["performance_passed"])
        for field, value in (("step", -1), ("lane", -1), ("pitch", 128), ("phrase", 0),
                             ("kind", "grid"), ("target", {"kind": "instrument", "id": 1, "channel": 0}),
                             ("target", {"kind": "instrument", "id": 2**64, "channel": 1})):
            broken = copy.deepcopy(report)
            broken["samples"][0]["state"]["focus"][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                self.read(broken)

    def test_piano_selection_requires_exact_selected_identity_on_its_own_surface(self):
        report = copy.deepcopy(self.report)
        report.update(schema_version=2, scope="tracker_native_feedback", surface="piano_selection",
                      attempts=100, no_changes=0)
        for sample in report["samples"]:
            sample["state"].update(view="phrase", revision=4, focus={
                "kind": "piano_selection", "phrase": 104, "pitch": 60,
                "at": {"numerator": 0, "denominator": 1},
                "selected": 9007199254740993 + sample["sequence"] % 2})
        self.assertTrue(self.read(report)["performance_passed"])
        for invalid in (0, -1, 2**64, True, "1000"):
            broken = copy.deepcopy(report)
            broken["samples"][0]["state"]["focus"]["selected"] = invalid
            with self.subTest(selected=invalid), self.assertRaises(ValueError):
                self.read(broken)
        report["surface"] = "piano"
        with self.assertRaises(ValueError):
            self.read(report)

    def test_missing_stopped_or_webview_only_samples_fail(self):
        for change in ("missing", "stopped", "webview"):
            report = copy.deepcopy(self.report)
            if change == "missing":
                report["samples"].pop()
            else:
                for sample in report["samples"]:
                    if change == "stopped":
                        sample["state"]["playing"] = False
                    else:
                        sample["origin"] = "webview"
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.read(report)

    def test_piano_edit_requires_all_exact_selected_note_values(self):
        report = copy.deepcopy(self.report)
        report.update(schema_version=2, scope="tracker_native_feedback", surface="piano_edit",
                      attempts=100, no_changes=0)
        note = {"id": 9007199254740993, "at": {"numerator": 1, "denominator": 3},
                "duration": {"numerator": 1, "denominator": 4}, "pitch": 60,
                "velocity": 88, "release_velocity": 45}
        for sample in report["samples"]:
            changed = dict(note, velocity=88 + sample["sequence"] % 2)
            sample["state"].update(view="phrase", revision=sample["sequence"], focus={
                "kind": "piano_edit", "phrase": 104, "notes": [changed, dict(note, id=1001, pitch=64)]})
        self.assertTrue(self.read(report)["performance_passed"])
        for field, value in (("id", 0), ("id", 2**64), ("pitch", 128), ("velocity", 0),
                             ("at", None), ("duration", "1/4"),
                             ("release_velocity", True), ("duration", {"numerator": 0, "denominator": 1}),
                             ("at", {"numerator": 0, "denominator": 0})):
            broken = copy.deepcopy(report)
            broken["samples"][0]["state"]["focus"]["notes"][1][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                self.read(broken)
        for notes in ([], [note, note]):
            broken = copy.deepcopy(report)
            broken["samples"][0]["state"]["focus"]["notes"] = notes
            with self.subTest(notes=notes), self.assertRaises(ValueError):
                self.read(broken)

    def test_drum_edit_requires_its_own_exact_hit_evidence(self):
        report = copy.deepcopy(self.report)
        report.update(schema_version=2, scope="tracker_native_feedback", surface="drum_edit",
                      attempts=100, no_changes=0)
        for sample in report["samples"]:
            sample["state"].update(view="phrase", revision=sample["sequence"], focus={
                "kind": "drum_edit", "phrase": 104, "step": 64, "lane": 1, "pitch": 36,
                "target": {"kind": "instrument", "id": 18, "channel": 2},
                "hits": [9007199254740993] if sample["sequence"] % 2 else []})
        self.assertTrue(self.read(report)["performance_passed"])
        for invalid in (None, [0], [2**64], [True], [17,17]):
            broken = copy.deepcopy(report)
            broken["samples"][0]["state"]["focus"]["hits"] = invalid
            with self.subTest(hits=invalid), self.assertRaises(ValueError):
                self.read(broken)
        report["surface"] = "drum"
        with self.assertRaises(ValueError):
            self.read(report)

    def test_false_summary_drops_and_invalid_geometry_fail(self):
        for change in ("summary", "drop", "geometry", "duplicate", "slow", "incomplete"):
            report = copy.deepcopy(self.report)
            if change == "summary":
                report["native_keyboard_playback_p95_ns"] = 1
            elif change == "drop":
                report["failures"] = ["Action was superseded"]
            elif change == "geometry":
                report["samples"][0]["bounds"][2] = 0
            elif change == "duplicate":
                report["samples"][1]["sequence"] = 1
            elif change == "slow":
                for sample in report["samples"]:
                    sample["receipt_to_ack_ns"] = 60_000_000
                report["playback_p95_ns"] = report["native_keyboard_playback_p95_ns"] = 60_000_000
            else:
                report["complete"] = False
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.read(report)


if __name__ == "__main__":
    unittest.main()
