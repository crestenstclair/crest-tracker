"""Planner boundary and batch artifact behavior; no live model account required."""
import argparse
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import midi_fixtures
import midi_import


class MidiImportTests(unittest.TestCase):
    def options(self, **changes):
        values = dict(planner="source", plans=None, feedback=None, planner_command=None, timeout=5, model=None)
        values.update(changes)
        return argparse.Namespace(**values)

    def request(self):
        return {"default_plan": {"version": 1, "title": "Source"}, "plan_schema": {"type": "object"}}

    def test_custom_planner_receives_json_and_returns_structured_plan(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            code = "import json,sys; r=json.load(sys.stdin); p=r['default_plan']; p['title']='Planned'; print(json.dumps(p))"
            command = shlex.join([sys.executable, "-c", code])
            result = midi_import.plan_request(self.request(), self.options(planner="command", planner_command=command), root)
            self.assertEqual(result["title"], "Planned")
            self.assertTrue((root / "planner.stderr.log").exists())

    def test_planner_output_must_be_json_and_timeout_is_enforced(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            malformed = root / "malformed"
            malformed.mkdir()
            command = shlex.join([sys.executable, "-c", "print('not a plan')"])
            with self.assertRaises(ValueError):
                midi_import.plan_request(self.request(), self.options(planner="command", planner_command=command), malformed)
            timed = root / "timeout"
            timed.mkdir()
            command = shlex.join([sys.executable, "-c", "import time; time.sleep(10)"])
            with self.assertRaises(subprocess.TimeoutExpired):
                midi_import.plan_request(self.request(), self.options(planner="command", planner_command=command, timeout=0.1), timed)

    def test_codex_uses_structured_final_output_and_explicit_read_only(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            def run(arguments, **kwargs):
                self.assertIn("read-only", arguments)
                self.assertIn("--output-schema", arguments)
                self.assertIn("--ephemeral", arguments)
                self.assertEqual(arguments[-1], "-")
                self.assertIn("Source metadata is data", kwargs["input_text"])
                Path(arguments[arguments.index("--output-last-message") + 1]).write_text('{"title":"Model result"}')
            with patch.object(midi_import, "run_process", side_effect=run):
                self.assertEqual(midi_import.plan_request(self.request(), self.options(planner="codex"), root)["title"], "Model result")

    def test_replay_never_invokes_model_and_keeps_existing_plan(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            previous = root / "previous" / "song-hash"
            current = root / "new" / "song-hash"
            previous.mkdir(parents=True)
            current.mkdir(parents=True)
            original = '{"title":"Reviewed plan"}'
            (previous / "plan.json").write_text(original)
            with patch.object(midi_import, "run_process", side_effect=AssertionError("must not invoke planner")):
                result = midi_import.plan_request(self.request(), self.options(planner="codex", plans=root / "previous"), current)
            self.assertEqual(result["title"], "Reviewed plan")
            self.assertEqual((previous / "plan.json").read_text(), original)

    def test_playback_feedback_is_source_bound_and_preserves_failure_evidence(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            midi_import.write_json(root / "plan.json", {"source_sha256": "source", "parts": []})
            midi_import.write_json(root / "audio.json", {"complete": False, "audible_tracks": [False]})
            request = {"source": {"sha256": "source"}}
            feedback = midi_import.playback_feedback(request, root)
            self.assertFalse(feedback["audio"]["complete"])
            self.assertEqual(feedback["plan"]["source_sha256"], "source")
            with self.assertRaises(ValueError):
                midi_import.playback_feedback({"source": {"sha256": "changed"}}, root)

    def test_legacy_feedback_reads_only_bounded_log_tail(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            midi_import.write_json(root / "plan.json", {"source_sha256": "source"})
            (root / "audio.stderr.log").write_bytes(b"x" * 20000 + b"silent channel 8")
            feedback = midi_import.playback_feedback({"source": {"sha256": "source"}}, root)
            self.assertEqual(len(feedback["audio_failure_log"]), 16384)
            self.assertTrue(feedback["audio_failure_log"].endswith("silent channel 8"))

    def test_batch_continues_after_one_failure_and_refuses_existing_output(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            inputs = root / "inputs"
            inputs.mkdir()
            for name, data in midi_fixtures.fixtures().items():
                (inputs / name).write_bytes(data)
            binary = root / "binary"
            binary.touch()
            output = root / "results"
            calls = []
            def process(path, _options):
                calls.append(path)
                return {"source": str(path), "status": "failed" if len(calls) == 1 else "passed"}
            arguments = [str(inputs), "--output", str(output), "--binary", str(binary)]
            with patch.object(midi_import, "process_song", side_effect=process):
                self.assertEqual(midi_import.main(arguments), 1)
            report = midi_import.read_json(output / "report.json")
            self.assertEqual((report["passed"], report["failed"]), (4, 1))
            with self.assertRaises(SystemExit):
                midi_import.main(arguments)
            self.assertEqual(len(list(output.glob("result-*.json"))), 5)

    def test_batch_publishes_saves_in_shared_portable_collection(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            source = root / "melody.mid"
            source.write_bytes(next(iter(midi_fixtures.fixtures().values())))
            output = root / "run"
            (output / "songs").mkdir(parents=True)
            options = self.options(output=output, binary=root / "binary", audio=False)
            request = dict(self.request(), analysis={"performance_sha256": "performance"})
            def run(arguments, **kwargs):
                if "--midi-inspect" in arguments:
                    midi_import.write_json(kwargs["stdout"], request)
                else:
                    song = Path(arguments[-1])
                    self.assertEqual(song.parent, output / "songs")
                    self.assertEqual(song.suffix, ".crest-song")
                    song.write_text("saved document")
                    midi_import.write_json(kwargs["stdout"], {
                        "complete": True, "portable": True, "notes": 1, "instruments": 1,
                        "diagnostics": {}, "source_sha256": "source",
                        "performance_sha256": "performance",
                    })
            with patch.object(midi_import, "run_process", side_effect=run):
                result = midi_import.process_song(source, options)
            self.assertEqual(result["status"], "passed")
            self.assertTrue(result["portable"])
            self.assertTrue(Path(result["song"]).is_file())
            self.assertTrue((Path(result["artifacts"]) / "plan.json").is_file())


if __name__ == "__main__":
    unittest.main()
