#!/usr/bin/env python3
"""Shared paths for the in-repository Synth fork and Tracker tools."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
SYNTH = ROOT / "crates" / "crest-synth"


def local_cargo(synth, *arguments):
    """Reject accidental use of an external Synth checkout."""
    if Path(synth).resolve() != SYNTH.resolve():
        raise ValueError("Synth now lives in crates/crest-synth; edit and commit both packages together")
    return ["cargo", *arguments]
