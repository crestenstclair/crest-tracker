"""Fast workflow tests. No Cargo, network, or real repository mutations."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import develop

REVISION = "a" * 40
MANIFEST = ('[workspace.dependencies]\ncrest-synth = { git = "' + develop.SYNTH_URL +
            '", rev = "' + "b" * 40 + '", features = [] }\n')


class WorkflowTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve() / "tracker"
        self.synth = Path(self.temporary.name).resolve() / 'synth "quoted" space'
        self.root.mkdir()
        self.synth.mkdir()
        (self.root / "Cargo.toml").write_text(MANIFEST)
        (self.synth / "Cargo.toml").touch()
        patch.dict(os.environ, {"CARGO_HOME": str(self.root / "cargo-home")}).start()
        self.runner = patch.object(develop, "run", return_value="").start()
        self.addCleanup(patch.stopall)

    def commands(self):
        return [call.args[0] for call in self.runner.call_args_list]

    def execute(self, command, *arguments):
        develop.main(["--synth-path", str(self.synth), command, *arguments], self.root)

    def test_dev_quotes_path_and_forwards_application_arguments(self):
        self.execute("dev", "--", "--song", "one two.crest")
        command = self.commands()[0]
        self.assertEqual(command[2], 'patch.' + json.dumps(develop.SYNTH_URL) +
                         '.crest-synth.path=' + json.dumps(str(self.synth)))
        self.assertEqual(command[3:], ["run", "-p", "crest-tracker-app", "--",
                                      "--song", "one two.crest"])
        self.assertEqual(self.runner.call_args.args[1], self.root)

    def test_lint_forwards_override_to_external_cargo_subcommand(self):
        self.execute("lint")
        command = self.commands()[0]
        self.assertEqual(command[:3], ["cargo", "clippy", "--config"])
        self.assertIn(json.dumps(str(self.synth)), command[3])
        self.assertEqual(command[4:], ["--workspace", "--all-targets", "--", "-D", "warnings"])

    def test_synth_preserves_original_diagnostic_arguments_and_checkout(self):
        self.execute("synth", "--", "--full-instrument-effect-demo")
        self.assertEqual(self.commands(), [["cargo", "run", "--release", "--bin", "crest-synth", "--", "--full-instrument-effect-demo"]])
        self.assertEqual(self.runner.call_args.args[1], self.synth)

    def test_check_pair_runs_both_packages_and_focused_synth_tests(self):
        self.execute("check-pair")
        commands = self.commands()
        self.assertIn(["test", "--workspace"], [item[3:] for item in commands])
        self.assertEqual(commands[-len(develop.DEFAULT_TESTS):], [["cargo", "test", "--test", target]
                                        for target in develop.DEFAULT_TESTS])
        self.assertEqual(self.runner.call_args.args[1], self.synth)

    def test_diff_pair_uses_read_only_git_commands(self):
        self.execute("diff-pair")
        self.assertEqual(len(self.commands()), 8)
        self.assertTrue(all(command[:2] in (["git", "status"], ["git", "rev-parse"],
                                          ["git", "diff"]) for command in self.commands()))

    def test_pinned_checks_never_add_override(self):
        self.execute("check-pinned")
        self.assertTrue(all("--locked" in command and "--config" not in command
                            for command in self.commands()))

    def test_ancestor_override_blocks_pinned_check(self):
        config = self.root.parent / ".cargo"
        config.mkdir()
        (config / "config.toml").write_text('[patch.crates-io]\nsomething = {path="local"}\n')
        with self.assertRaisesRegex(ValueError, "forbid source overrides"):
            self.execute("check-pinned")
        self.runner.assert_not_called()

    def test_pin_refuses_dirty_lock_without_mutation(self):
        self.runner.side_effect = ["Cargo.lock", " M Cargo.lock"]
        with self.assertRaisesRegex(ValueError, "Cargo.lock changes"):
            self.execute("pin-synth", REVISION)
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)

    def test_pin_failure_restores_only_own_manifest_edit(self):
        patch.object(develop, "published_revision", return_value=REVISION).start()
        self.runner.side_effect = ["Cargo.lock", "", subprocess.CalledProcessError(1, "cargo")]
        with self.assertRaises(subprocess.CalledProcessError):
            self.execute("pin-synth", REVISION)
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)

    def test_revision_edit_preserves_other_manifest_content(self):
        self.assertEqual(develop.replace_revision(MANIFEST, REVISION),
                         MANIFEST.replace("b" * 40, REVISION))

    def test_pin_failure_preserves_concurrent_manifest_edit(self):
        patch.object(develop, "published_revision", return_value=REVISION).start()
        path = self.root / "Cargo.toml"

        def fail_update(arguments, directory, capture=False):
            if arguments[0] == "cargo":
                path.write_text("concurrent edit")
                raise subprocess.CalledProcessError(1, arguments)
            return ""

        self.runner.side_effect = fail_update
        with self.assertRaises(subprocess.CalledProcessError):
            self.execute("pin-synth", REVISION)
        self.assertEqual(path.read_text(), "concurrent edit")

    def test_head_pin_refuses_dirty_synth(self):
        self.runner.side_effect = ["Cargo.lock", "", " M src/lib.rs"]
        with self.assertRaisesRegex(ValueError, "Commit Synth changes"):
            self.execute("pin-synth")
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)

    def test_remote_probe_fetches_only_into_disposable_storage(self):
        self.runner.side_effect = ["", "", REVISION]
        self.assertEqual(develop.published_revision(REVISION), REVISION)
        self.assertEqual(self.commands()[1], ["git", "fetch", "--quiet", "--depth=1",
                                              develop.SYNTH_URL, REVISION])
        directories = {call.args[1] for call in self.runner.call_args_list}
        self.assertEqual(len(directories), 1)
        self.assertFalse(Path(directories.pop()).exists())

    def test_remote_revision_validation_precedes_subprocess(self):
        with self.assertRaises(ValueError):
            develop.published_revision("main")
        self.runner.assert_not_called()


if __name__ == "__main__":
    unittest.main()
