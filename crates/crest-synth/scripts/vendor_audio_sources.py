#!/usr/bin/env python3
"""Fetch the pinned upstream DSP subsets. Normal builds never use the network."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path, PurePosixPath
import hashlib
import io
import json
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1] / 'vendor' / 'audio'
PINS = json.loads((ROOT / 'sources.json').read_text())
AIRWINDOWS = ('PurestEcho Doublelay TapeDelay2 Reverb kPlateD StereoChorus Vibrato '
              'AutoPan ButterComp2 SoftGate Baxandall2 Parametric Biquad2 Capacitor2 '
              'Density2 ToTape6 DeRez3').split()
MDA = ('JX10 Piano EPiano Leslie Dynamics DeEsser DubDelay TalkBox').split()


def read(url):
    return urllib.request.urlopen(url, timeout=120).read()


def selected(name, path):
    if "files" in PINS[name]:
        return path in PINS[name]["files"]
    p = PurePosixPath(path)
    if PINS[name].get("full"):
        return p.parts[0] not in (".git", ".github", "test", "tests", "bench", "benchmarks", "doc", "docs", "demos")
    if any(part.lower() in ('license', 'license.txt', 'license.mit', 'copying', 'copying.txt') for part in p.parts):
        return True
    if len(p.parts) == 1 and p.suffix.lower() in ('.md', '.txt'):
        return True
    if name == 'mutable':
        return p.parts[0] in ('plaits', 'rings', 'elements', 'tides2', 'peaks', 'clouds', 'warps') and (p.suffix in ('.h', '.cc') and ('drivers' not in p.parts or p.name == 'debug_pin.h'))
    if name == 'stmlib':
        return p.suffix in ('.h', '.cc') and not any(x in p.parts for x in ('third_party', 'stm', 'system', 'bootloader'))
    if name == 'daisysp':
        return path.startswith('Source/') and p.suffix in ('.h', '.cpp')
    if name == 'stk':
        return p.parts[0] in ('include', 'src', 'rawwaves')
    if name == 'mda-deesser':
        return p.name in ('mdaDeEsserProcessor.h', 'mdaDeEsserProcessor.cpp', 'mdaDeEsserController.cpp')
    if name == 'mda':
        return any(p.name.lower().startswith('mda' + engine.lower()) for engine in MDA)
    if name == 'msfa':
        return path.startswith('app/src/main/jni/') and p.suffix in ('.h', '.cc')
    if name == 'nam':
        if path == 'example_models/lstm.nam': return True
        return p.parts[0] in ('NAM', 'Dependencies') and p.suffix in ('.h', '.hpp', '.cpp')
    if name in ('fftconvolver', 'stretch', 'linear'):
        return len(p.parts) == 1 and p.suffix in ('.h', '.hpp', '.cpp')
    if name == 'r8brain':
        return p.suffix in ('.h', '.hpp', '.cpp', '.inc') and p.parts[0] not in ('other', 'bench')
    if name == 'sfizz':
        return p.parts[0] not in ('.github', 'tests', 'benchmarks', 'clients', 'demos', 'docs', 'scripts')
    return False


def fetch(name):
    pin = PINS[name]
    dest = ROOT / pin.get("destination", name)
    dest.mkdir(parents=True, exist_ok=True)
    base = f"https://raw.githubusercontent.com/{pin['repository']}/{pin['revision']}"
    if name == 'airwindows':
        files = ['LICENSE'] + [f'plugins/WinVST/{effect}/{effect}{suffix}' for effect in AIRWINDOWS for suffix in ('.h', '.cpp', 'Proc.cpp')]
        for path in files:
            target = dest / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(read(base + '/' + path))
    else:
        archive = read(f"https://codeload.github.com/{pin['repository']}/tar.gz/{pin['revision']}")
        with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
            for entry in tar:
                path = '/'.join(PurePosixPath(entry.name).parts[1:])
                if not entry.isfile() or not path or '..' in PurePosixPath(path).parts or not selected(name, path):
                    continue
                target = dest / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(tar.extractfile(entry).read())
    records = [f'{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.relative_to(dest)}' for p in sorted(dest.rglob('*')) if p.is_file() and p.name not in ('SHA256SUMS', 'PROVENANCE.md')]
    (dest / 'SHA256SUMS').write_text('\n'.join(records) + '\n')
    (dest / 'PROVENANCE.md').write_text(f"# Upstream DSP source\n\nSource: https://github.com/{pin['repository']}\n\nRevision: `{pin['revision']}`\n\nFetched by `scripts/vendor_audio_sources.py`. Upstream source and license notices\nare retained; local integration lives under `native/audio`. This directory\ncontains the selected DSP subset, not an installed plugin or its editor.\n")
    return name, len(records)


if __name__ == '__main__':
    import sys
    names = sys.argv[1:] or list(PINS)
    with ThreadPoolExecutor(max_workers=4) as pool:
        for name, count in pool.map(fetch, names):
            print(f'{name}: {count} files', flush=True)
