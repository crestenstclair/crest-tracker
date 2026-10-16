#!/usr/bin/env python3
"""Exercise the shipping app through native keys, dialogs and monitored audio.

Run inside with-desktop.sh, with or without --wayland. AT-SPI observes real
window titles/focus; XTest drives the isolated X11 desktop or Weston's seat.
"""

import argparse
import array
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import time

import pyatspi


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def key(chord):
    command("xdotool", "key", "--clearmodifiers", chord)
    time.sleep(.25)


def windows():
    for application in pyatspi.Registry.getDesktop(0):
        for window in application:
            yield application.get_process_id(), window


def visible_dialog():
    return next((window for _, window in windows()
                 if window.getRole() in (pyatspi.ROLE_DIALOG, pyatspi.ROLE_ALERT,
                                         pyatspi.ROLE_FILE_CHOOSER)
                 and window.getState().contains(pyatspi.STATE_SHOWING)), None)


def record(root, label):
    path = root / f"{label}.f32"
    with path.open("wb") as output:
        capture = subprocess.Popen([
            "parec", "--device=crest_test_sink.monitor", "--format=float32le",
            "--rate=48000", "--channels=2", "--raw"], stdout=output)
        try:
            time.sleep(2)
        finally:
            capture.terminate()
            capture.wait(timeout=5)
    samples = array.array("f", path.read_bytes())
    assert samples and all(math.isfinite(value) for value in samples)
    rms = math.sqrt(sum(value * value for value in samples) / len(samples))
    print(f"{label}: {len(samples) // 2} frames, RMS {rms}", flush=True)
    return samples, rms


def smoke(binary, root):
    path = root / "session.crest"
    assert not path.exists(), "Evidence directory must be fresh"
    environment = dict(os.environ, XDG_CONFIG_HOME=str(root / "config"))
    if environment.get("GDK_BACKEND") == "wayland":
        environment.pop("DISPLAY", None)  # No X11 fallback for the app.
    with (root / "application.log").open("w") as log:
        app = subprocess.Popen([str(binary)], env=environment, stdout=log, stderr=log)

        owned_window = None

        def main_window():
            nonlocal owned_window
            # GTK/AT-SPI can temporarily omit the frame from the application
            # children while a separate-process modal prompt owns focus.
            owned_window = next((window for pid, window in windows()
                                 if pid == app.pid and window.getRole() == pyatspi.ROLE_FRAME),
                                owned_window)
            return owned_window

        def title():
            window = main_window()
            return window.name if window is not None else ""

        def main_active():
            window = main_window()
            return window is not None and window.getState().contains(pyatspi.STATE_ACTIVE)

        def wait_for(predicate, label):
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                assert app.poll() is None, f"App exited {app.returncode}: {label}"
                value = predicate()
                if value:
                    print(f"Verified: {label}", flush=True)
                    return value
                time.sleep(.1)
            subprocess.run(["import", "-window", "root", str(root / "timeout.png")], check=False)
            print([(pid, window.name, window.getRoleName(), window.getState().getStates())
                   for pid, window in windows()], flush=True)
            raise AssertionError(f"Timed out: {label}; main window: {title()}")

        def wait_for_main():
            # A closed dialog can precede the compositor's keyboard-enter
            # event. Wait for real focus, including Wayland close animations.
            wait_for(lambda: not visible_dialog() and main_active(), "main keyboard focus")

        def wait_for_dialog():
            wait_for(lambda: (window := visible_dialog()) is not None
                     and window.getState().contains(pyatspi.STATE_ACTIVE), "active native dialog")

        try:
            wait_for(lambda: "READY" in title(), "ready document")
            wait_for_main()
            assert record(root, "playing")[1] > 1e-5, "Startup must be audible"
            key("t")
            time.sleep(1)
            assert all(value == 0 for value in record(root, "stopped")[0]), "T must stop exactly"

            key("ctrl+shift+s")
            wait_for_dialog()
            key("ctrl+a")
            command("xdotool", "type", "--clearmodifiers", str(path))
            key("Return")
            wait_for(lambda: path.exists() and path.stem in title(), "saved document")
            wait_for_main()
            original = path.read_bytes()
            json.loads(original)

            key("ctrl+n")
            wait_for(lambda: "Untitled" in title() and "READY" in title(), "new document")
            key("ctrl+o")
            wait_for_dialog()
            key("ctrl+l")
            command("xdotool", "type", "--clearmodifiers", str(path))
            key("Return")
            time.sleep(.5)
            if visible_dialog():
                key("Return")  # GTK can first accept the location entry.
            wait_for(lambda: path.stem in title() and "READY" in title(), "opened document")
            wait_for_main()
            saved_at = path.stat().st_mtime_ns
            key("ctrl+s")
            wait_for(lambda: path.stat().st_mtime_ns != saved_at and "READY" in title(),
                     "round-trip save")
            assert path.read_bytes() == original, "Save/Open must preserve the exact session"

            key("Return")
            key("s")
            command("xdotool", "keydown", "k", "key", "Right", "keyup", "k")
            wait_for(lambda: "*" in title(), "dirty Root Note edit")
            key("ctrl+n")
            # Zenity's GTK4 accessibility backend does not report ACTIVE on
            # its modal alert. Prove actual keyboard delivery by cancelling it.
            wait_for(lambda: (window := visible_dialog()) is not None
                     and window.name == "Unsaved Crest Synth session", "unsaved guard")
            key("Escape")
            wait_for_main()
            assert "*" in title(), "Cancel must preserve the dirty document"
            assert path.read_bytes() == original, "Cancel must preserve the saved bytes"
            key("ctrl+s")
            wait_for(lambda: "*" not in title() and "READY" in title(), "save edited document")
            assert path.read_bytes() != original, "Root Note edit must persist"
            key("ctrl+w")
            assert app.wait(timeout=10) == 0, "Owned shutdown must succeed"
            print("PASS: audio, exact stop, Save As/New/Open, exact round trip, edit, "
                  "unsaved Cancel, Save, owned close", flush=True)
        finally:
            if app.poll() is None:
                app.terminate()
                try:
                    app.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    app.kill()
                    app.wait(timeout=3)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    if os.environ.get("CREST_LINUX_TEST_DESKTOP") != "1":
        parser.error("Run through scripts/linux/with-desktop.sh")
    # The developer container is disposable. Keep failure logs and captures in
    # its mounted build cache so they remain inspectable after container exit.
    evidence_root = Path(os.environ.get("CARGO_TARGET_DIR", "target")) / "linux-evidence"
    evidence_root.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix="session-", dir=evidence_root)).resolve()
    print(f"Session smoke evidence: {evidence}", flush=True)
    smoke(args.binary.resolve(), evidence)
