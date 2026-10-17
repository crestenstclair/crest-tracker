#!/usr/bin/env python3
"""Exercise a relocated Linux Tracker through native UI and monitored Sample audio."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import shutil
import sys
import tarfile
import tempfile
import time

from develop import ROOT

sys.path.insert(0, str(ROOT / 'crates/crest-synth/scripts/linux'))
import pyatspi
from gi.repository import GLib
from native_desktop import command, key, record, windows


def descendants(node):
    # AT-SPI can return a vacant child slot while WebKit replaces a subtree.
    if node is None:
        return
    yield node
    for child in node:
        yield from descendants(child)


def normalized(value):
    # WebKitGTK exposes CSS text-transform in accessibility names.
    return ''.join(value.lstrip('› ').split()).casefold()


def describe(node):
    label = f'{node.getRoleName()}: {node.name}'
    if node.getRole() == pyatspi.ROLE_ENTRY:
        label += f' value={node.queryText().getText(0, -1)!r}'
    return label


def expected_sample_import(original):
    """Opening freezes this fixture's verified sample into its hashed library path.

    Only the active reference and its manifest alias change. This expectation
    comes from the source WAV, not from the document produced by reopening.
    """
    source = (ROOT / 'crates/crest-synth/assets/sample-test.wav').read_bytes()
    digest = hashlib.sha256(source).hexdigest()
    reference = {'kind': 'sample', 'locator': 'Test Tone.wav'}
    assert original['resources'] == [{'reference': reference, 'sha256': digest,
                                     'bytes': len(source), 'file': None, 'installed': None}]
    asset = original['synth']['patches'][0]['instrument']['assetReferences'][0]
    assert asset == {'parameterId': 'sample.asset', 'reference': reference}
    imported = {'kind': 'sample', 'locator': f'Imported/{digest}.wav'}
    expected = copy.deepcopy(original)
    expected['resources'][0]['reference'] = imported
    expected['resources'][0]['installed'] = imported
    expected['synth']['patches'][0]['instrument']['assetReferences'][0]['reference'] = imported
    return expected


class Desktop:
    """Observe the owned app; all activation and text input goes through XTest."""
    def __init__(self, process, evidence):
        self.process = process
        self.evidence = evidence

    def frame(self):
        # Deliver AT-SPI cache invalidations before walking WebKit's current tree.
        # The application has its own process and clock; this only pumps observation.
        context = GLib.MainContext.default()
        while context.pending():
            context.iteration(False)
        return next((window for pid, window in windows()
                     if pid == self.process.pid and window.getRole() == pyatspi.ROLE_FRAME), None)

    def find(self, label, role=pyatspi.ROLE_PUSH_BUTTON):
        frame = self.frame()
        if frame is None:
            return None
        return next((node for node in descendants(frame)
                     if node.getRole() == role
                     and normalized(node.name).startswith(normalized(label))), None)

    def wait(self, predicate, label):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise AssertionError(f'App exited {self.process.returncode}: {label}')
            try:
                result = predicate()
            except GLib.Error as error:
                # WebKit can retire an accessibility object during a tree read.
                # Retry observation/reveal only; never repeat input activation.
                # A real process exit remains an immediate failure above.
                if error.domain != 'atspi_error' or not (error.code == 0 or
                        error.code == 1 and error.message.startswith('No such interface')):
                    raise
                result = None
            if result:
                print(f'Verified: {label}', flush=True)
                return result
            time.sleep(.1)
        self.snapshot('timeout')
        raise AssertionError(f'Timed out: {label}')

    def button_bounds(self, label):
        button = self.find(label)
        if button is None:
            return None
        button.queryComponent().scrollTo(pyatspi.SCROLL_ANYWHERE)
        time.sleep(.1)
        # A render during scrolling can retire the previous accessible object.
        button = self.find(label)
        if button is None:
            return None
        bounds = button.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
        return bounds if bounds[2] > 0 and bounds[3] > 0 else None

    def click(self, label):
        x, y, width, height = self.wait(lambda: self.button_bounds(label), label)
        command('xdotool', 'mousemove', '--sync', str(x + width // 2), str(y + height // 2),
                'click', '1')

    def status(self, label):
        frame = self.frame()
        # The GTK document title includes the actual host status. WebKitGTK's
        # accessibility tree omits the non-interactive workspace status span.
        return frame is not None and normalized(frame.name).endswith(normalized('— ' + label))

    def enter_path(self, label, field, path):
        self.click(label)
        self.wait(lambda: (entry := self.find(field, pyatspi.ROLE_ENTRY)) is not None
                  and entry.queryComponent().grabFocus(), f'{field} editor focused')
        key('ctrl+a')
        command('xdotool', 'type', '--clearmodifiers', '--delay', '5', str(path))
        self.wait(lambda: (entry := self.find(field, pyatspi.ROLE_ENTRY)) is not None
                  and entry.queryText().getText(0, -1) == str(path), 'exact typed path')
        key('Return')

    def panel(self):
        self.click('MENU')
        self.click('Performance clips')

    def snapshot(self, label):
        subprocess.run(['import', '-window', 'root', str(self.evidence / f'{label}.png')],
                       check=False, timeout=10)
        try:
            frame = self.frame()
            report = '' if frame is None else '\n'.join(
                describe(node) for node in descendants(frame))
        except GLib.Error as error:
            report = f'Accessibility snapshot unavailable: {error}'
        (self.evidence / f'{label}-accessibility.txt').write_text(report)


def exercise(desktop, root):
    desktop.wait(desktop.frame, 'shipping app window')
    desktop.panel()
    desktop.click('Open expressive Sample example')
    desktop.wait(lambda: desktop.status('OPENED'), 'Sample document prepared')
    desktop.panel()
    desktop.wait(lambda: desktop.find('Launch clip 1'), 'expressive Sample example loaded')
    desktop.click('Launch clip 1')
    desktop.wait(lambda: desktop.find('Stop clip 1'), 'clip looping')
    assert record(root, 'sample-loop')[1] > 1e-5, 'Sample clip must reach native audio output'
    desktop.snapshot('playing')
    desktop.click('Stop clip 1')
    desktop.wait(lambda: desktop.find('Launch clip 1'), 'clip stopped')
    time.sleep(1)
    assert all(value == 0 for value in record(root, 'stopped')[0]), 'Stop must release audio'

    path = root / 'expressive.crest-song'
    key('ctrl+shift+s')
    desktop.enter_path('Save to filename', 'Save to filename', path)
    desktop.wait(lambda: path.exists() and desktop.status('SAVED'), 'saved project')
    original = json.loads(path.read_text())
    shutil.copy2(path, root / 'before-reopen.crest-song')
    clip = original['performance']['session']['slots'][0]['clip']
    fixture = json.loads((ROOT / 'assets/performance-phrase.json').read_text())['slots'][0]['clip']
    assert clip['events'] == fixture['events'], 'Save must retain every source note and X/Y/Z event'
    assert clip['length'] == fixture['length'], 'Save must retain exact free-time loop length'
    assert original['performance']['session']['version'] == 10

    # New clears the document before reopening; this cannot pass by continuing
    # to play the original in-memory engine after a failed Open.
    key('ctrl+n')
    desktop.wait(lambda: desktop.status('OPENED'), 'new project opened')
    desktop.panel()
    assert desktop.find('Launch clip 1') is None, 'New must clear the performance bank'
    key('ctrl+o')
    desktop.enter_path('Open path…', 'Open Song path', path)
    desktop.wait(lambda: desktop.status('OPENED'), 'saved project reopened')
    desktop.panel()
    desktop.wait(lambda: desktop.find('Launch clip 1'), 'reopened recorded clip')
    desktop.click('Launch clip 1')
    desktop.wait(lambda: desktop.find('Stop clip 1'), 'reopened clip looping')
    assert record(root, 'reopened-loop')[1] > 1e-5, 'Reopened clip must remain audible'
    desktop.click('Stop clip 1')
    desktop.wait(lambda: desktop.find('Launch clip 1'), 'reopened clip stopped')
    changed = path.stat().st_mtime_ns
    key('ctrl+s')
    desktop.wait(lambda: path.stat().st_mtime_ns != changed and desktop.status('SAVED'),
                 'reopened project saved')
    assert json.loads(path.read_text()) == expected_sample_import(original), \
        'Save/reopen may only relink the verified sample; all other project data must remain exact'
    desktop.snapshot('reopened')
    key('ctrl+w')
    assert desktop.process.wait(timeout=15) == 0, 'Owned shutdown must succeed'


def smoke(binary, root):
    environment = os.environ | {'XDG_CONFIG_HOME': str(root / 'config'),
                                'XDG_DATA_HOME': str(root / 'data')}
    with (root / 'application.log').open('w') as log:
        app = subprocess.Popen([str(binary)], cwd=root, env=environment, stdout=log, stderr=log)
        desktop = Desktop(app, root)
        try:
            exercise(desktop, root)
            (root / 'result.json').write_text(json.dumps({
                'passed': True, 'scope': 'linux_virtual_desktop_sample_clip',
                'physical_qualification': False, 'live_midi_clock_qualification': False,
            }, indent=2) + '\n')
        finally:
            if app.poll() is None:
                try:
                    desktop.snapshot('failure')
                finally:
                    app.terminate()
                    try:
                        app.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        app.kill()
                        app.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    args = parser.parse_args()
    if os.environ.get('CREST_LINUX_TEST_DESKTOP') != '1':
        parser.error('Run through crates/crest-synth/scripts/linux/with-desktop.sh')
    evidence = ROOT / 'target/linux-desktop-evidence'
    evidence.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='performance-', dir=evidence))
    print(f'Linux desktop evidence: {root}', flush=True)
    with tempfile.TemporaryDirectory(prefix='crest relocated desktop ') as temporary:
        with tarfile.open(args.archive) as archive:
            archive.extractall(temporary, filter='data')
        binaries = list(Path(temporary).glob('*/bin/crest-tracker-app'))
        assert len(binaries) == 1, 'Archive must contain exactly one shipping app'
        shutil.copy2(binaries[0].parent.parent / 'build-info.json', root / 'package-build-info.json')
        smoke(binaries[0], root)


if __name__ == '__main__':
    main()
