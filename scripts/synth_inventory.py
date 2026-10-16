#!/usr/bin/env python3
"""Capture every production Synth instrument/effect schema through Tracker's dependency."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from develop import ROOT, local_cargo
from workload import repository, utc, write_artifact


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--synth-path", type=Path, default=ROOT.parent / "crest-synth")
    parser.add_argument("--output", type=Path, default=ROOT / "target/workload/synth-inventory.json")
    options = parser.parse_args()
    synth = options.synth_path.expanduser().resolve()
    sources = {"tracker": repository(ROOT), "synth": repository(synth)}
    arguments = local_cargo(synth, "run", "--quiet", "--release", "-p", "crest-tracker-app",
                            "--", "--synth-inventory")
    result = subprocess.run(arguments, cwd=ROOT, text=True, capture_output=True, check=True)
    inventory = json.loads(result.stdout)
    if (inventory.get("scope") != "synth_production_capability_inventory"
            or inventory.get("registration_validated") is not True
            or not inventory.get("instruments") or not inventory.get("effects")):
        raise ValueError("Missing production capability inventory")
    after = {"tracker": repository(ROOT), "synth": repository(synth)}
    changed = sources != after
    artifact = {"captured_utc": utc(), "command": arguments, "repositories_before": sources,
                "repositories_after": after, "source_changed_during_run": changed,
                "status": "failed" if changed else "passed", "inventory": inventory}
    write_artifact(options.output, artifact)
    print(f"Synth inventory: {len(inventory['instruments'])} instruments, {len(inventory['effects'])} effects; {options.output}")
    if changed:
        print("Source changed during capture; repeat against stable sources.", file=sys.stderr)
    return 1 if changed else 0


if __name__ == "__main__":
    sys.exit(main())
