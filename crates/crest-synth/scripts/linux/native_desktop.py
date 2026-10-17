"""Native XTest input, AT-SPI observation and PulseAudio capture for Linux witnesses."""
import array
import math
import subprocess
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

