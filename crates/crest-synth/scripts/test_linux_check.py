"""Verify that the Linux gate does not accidentally select narrower UI coverage."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCOPED_WITNESSES = tuple(
    f"CREST_WEBVIEW_{name}_WITNESS"
    for name in (
        "PAGE_NAVIGATION", "OPTION", "EMPTY_PATCH", "CONTROLLER",
        "DETAIL", "SAMPLE", "PERFORMANCE",
    )
)


class LinuxCheckCoverageTests(unittest.TestCase):
    def test_soundfont_evidence_is_confined_to_its_own_journey(self):
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            calls = temporary / "calls.jsonl"
            stub = (
                f"#!{sys.executable}\n"
                "import json, os, sys\n"
                "with open(os.environ['CREST_CHECK_CALLS'], 'a') as output:\n"
                "    output.write(json.dumps({'args': sys.argv[1:], "
                "'soundfont': os.environ.get('CREST_SOUNDFONT_EVIDENCE_DIR'), "
                f"'scopes': [key for key in {SCOPED_WITNESSES!r} "
                "if os.environ.get(key) == '1'], "
                "'detail': os.environ.get('CREST_WEBVIEW_DETAIL_WITNESS')}) + '\\n')\n"
            )
            # Exercise the shell's actual command environments without starting
            # Cargo or recursively invoking unittest discovery.
            for executable in ("cargo", "python3"):
                path = temporary / executable
                path.write_text(stub)
                path.chmod(0o755)
            environment = dict(os.environ)
            environment.update({key: "1" for key in SCOPED_WITNESSES})
            environment.update(
                PATH=f"{temporary}{os.pathsep}{environment['PATH']}",
                CREST_CHECK_CALLS=str(calls),
                CREST_SOUNDFONT_EVIDENCE_DIR="/tmp/operator-soundfont-evidence",
            )
            subprocess.run(
                ["bash", str(root / "scripts/linux/check.sh")],
                env=environment,
                cwd=temporary,
                check=True,
            )
            invocations = [json.loads(line) for line in calls.read_text().splitlines()]

        general = [call for call in invocations if "--all-targets" in call["args"]]
        self.assertTrue(general)
        self.assertTrue(all(call["soundfont"] is None for call in general))
        self.assertTrue(all(not call["scopes"] for call in general))
        dedicated = [call for call in invocations if call["soundfont"] is not None]
        self.assertEqual(
            [call["args"] for call in dedicated],
            [
                ["test", "--locked", "--test", "soundfont_file_loading"],
                ["test", "--locked", "--test", "webview_projection_shell"],
            ],
        )
        self.assertTrue(all(
            call["soundfont"] == "/tmp/operator-soundfont-evidence"
            for call in dedicated
        ))
        self.assertEqual(dedicated[-1]["detail"], "1")
        other_ui = [
            call for call in invocations
            if "webview_projection_shell" in call["args"]
            and call["soundfont"] is None
        ]
        self.assertTrue(other_ui, "other scoped UI journeys must remain separate")
        self.assertEqual(
            [call["scopes"] for call in other_ui],
            [[SCOPED_WITNESSES[index]] for index in range(4)]
            + [[SCOPED_WITNESSES[4], SCOPED_WITNESSES[5]]],
        )
        self.assertEqual(dedicated[-1]["scopes"], [SCOPED_WITNESSES[4]])


if __name__ == "__main__":
    unittest.main()
