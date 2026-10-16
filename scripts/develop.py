#!/usr/bin/env python3
"""Develop against a sibling Synth checkout; verify and pin published revisions.

Requires Python 3.11+. Local Cargo commands may update Cargo.lock. pin-synth
requires a tracked, clean Cargo.lock. It never stages, commits, pushes, or changes
branches. On failure it restores only its own manifest edit; any Cargo.lock
changes remain available for inspection.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
SYNTH_URL = "https://github.com/crestenstclair/crest-synth.git"
DEFAULT_TESTS = ["midi_device_contracts", "scheduled_midi_rendering", "source_channel_expression", "phase7_callback_safety"]


def run(arguments, directory, capture=False):
    return subprocess.run(arguments, cwd=directory, check=True, text=True,
                          capture_output=capture).stdout


def local_cargo(synth, *arguments):
    override = f"patch.{json.dumps(SYNTH_URL)}.crest-synth.path={json.dumps(str(synth))}"
    # Cargo external subcommands must receive the override themselves. A global
    # --config before `clippy` is not forwarded by current Cargo versions.
    if arguments and arguments[0] == "clippy":
        return ["cargo", "clippy", "--config", override, *arguments[1:]]
    return ["cargo", "--config", override, *arguments]


def pair_status(root, synth, diff=False):
    for directory in (root, synth):
        print(f"\n{directory}", flush=True)
        run(["git", "status", "--short"], directory)
        run(["git", "rev-parse", "HEAD"], directory)
        if diff:
            run(["git", "diff", "--stat", "HEAD"], directory)
            run(["git", "diff", "HEAD"], directory)


def reject_local_overrides(root):
    """Reject Cargo source overrides from manifests, ancestors, or Cargo home."""
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    config_dirs = [directory / ".cargo" for directory in (root, *root.parents)]
    config_dirs.append(cargo_home)
    files = [root / "Cargo.toml"]
    files.extend(directory / "config.toml" for directory in config_dirs)
    files.extend(directory / "config" for directory in config_dirs)
    for path in set(files):
        if path.is_file():
            config = tomllib.loads(path.read_text())
            if any(config.get(key) for key in ("patch", "replace", "paths")):
                raise ValueError(f"Pinned checks forbid source overrides: {path}")
    if any(key.startswith(("CARGO_PATCH_", "CARGO_REPLACE_", "CARGO_PATHS")) for key in os.environ):
        raise ValueError("Pinned checks forbid source override environment variables")


def check_pinned(root):
    reject_local_overrides(root)
    run(["cargo", "test", "--locked", "-p", "crest-tracker"], root)
    run(["cargo", "check", "--locked", "-p", "crest-tracker-app"], root)


def published_revision(revision):
    """Fetch into disposable storage to prove an exact remote commit exists."""
    if not re.fullmatch(r"[0-9a-fA-F]{40}", revision):
        raise ValueError("Synth revision must be a full 40-character commit SHA")
    with tempfile.TemporaryDirectory(prefix="crest-synth-pin-") as directory:
        run(["git", "init", "--bare", "--quiet"], directory)
        run(["git", "fetch", "--quiet", "--depth=1", SYNTH_URL, revision], directory)
        fetched = run(["git", "rev-parse", "FETCH_HEAD^{commit}"], directory, True).strip()
        if fetched.lower() != revision.lower():
            raise ValueError("The remote did not return the requested Synth commit")
    return revision.lower()


def replace_revision(manifest, revision):
    dependency = tomllib.loads(manifest)["workspace"]["dependencies"]["crest-synth"]
    if dependency.get("git") != SYNTH_URL or "rev" not in dependency:
        raise ValueError("Expected a revision-pinned crest-synth workspace dependency")
    # The workspace owns this single inline dependency; preserve all other bytes.
    pattern = r'(?m)^(crest-synth\s*=\s*\{[^\n]*\brev\s*=\s*")[^"]+("[^\n]*\})'
    updated, count = re.subn(pattern, lambda match: match[1] + revision + match[2], manifest)
    if count != 1:
        raise ValueError("Expected exactly one inline crest-synth revision field")
    return updated


def pin_synth(root, synth, revision=None):
    reject_local_overrides(root)
    run(["git", "ls-files", "--error-unmatch", "Cargo.lock"], root, True)
    if run(["git", "status", "--porcelain", "--", "Cargo.lock"], root, True).strip():
        raise ValueError("Commit or preserve Cargo.lock changes before pin-synth")
    if revision is None:
        if run(["git", "status", "--porcelain"], synth, True).strip():
            raise ValueError("Commit Synth changes before pinning its HEAD")
        revision = run(["git", "rev-parse", "HEAD"], synth, True).strip()
    revision = published_revision(revision)
    path = root / "Cargo.toml"
    before = path.read_text()
    after = replace_revision(before, revision)
    path.write_text(after)
    try:
        run(["cargo", "update", "-p", "crest-synth", "--precise", revision], root)
        check_pinned(root)
    except BaseException:
        if path.read_text() == after:
            path.write_text(before)
        print("Pin failed. Inspect Cargo.lock; no lockfile changes were reverted.", file=sys.stderr)
        raise
    print(f"Pinned Synth {revision}. Review Cargo.toml and Cargo.lock together.")


def main(argv=None, root=ROOT):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--synth-path", type=Path, default=root.parent / "crest-synth")
    parser.add_argument("command", choices=["dev", "synth", "check-pair", "status-pair", "diff-pair",
                                           "pin-synth", "check-pinned", "lint"])
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    options = parser.parse_args(argv)
    synth = options.synth_path.expanduser().resolve()
    arguments = options.arguments
    if arguments[:1] == ["--"]:
        arguments = arguments[1:]
    command = options.command
    if command in ("status-pair", "diff-pair", "check-pinned", "lint") and arguments:
        parser.error(f"{command} accepts no extra arguments")
    if command == "pin-synth" and len(arguments) > 1:
        parser.error("pin-synth accepts at most one revision")
    if command == "check-pair" and any(not re.fullmatch(r"[A-Za-z0-9_-]+", target)
                                        for target in arguments):
        parser.error("Synth test targets must contain only letters, digits, underscores, or hyphens")
    if command not in ("check-pinned", "pin-synth") and not (synth / "Cargo.toml").is_file():
        parser.error(f"Synth checkout is missing: {synth}")
    if command == "dev":
        run(local_cargo(synth, "run", "-p", "crest-tracker-app", "--", *arguments), root)
    elif command == "synth":
        run(["cargo", "run", "--release", "--bin", "crest-synth", "--", *arguments], synth)
    elif command == "lint":
        run(local_cargo(synth, "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"), root)
    elif command == "check-pair":
        pair_status(root, synth)
        run(local_cargo(synth, "test", "--workspace"), root)
        for target in arguments or DEFAULT_TESTS:
            run(["cargo", "test", "--test", target], synth)
    elif command in ("status-pair", "diff-pair"):
        pair_status(root, synth, diff=command == "diff-pair")
    elif command == "pin-synth":
        pin_synth(root, synth, arguments[0] if arguments else None)
    else:
        check_pinned(root)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"develop: {error}", file=sys.stderr)
        sys.exit(1)
