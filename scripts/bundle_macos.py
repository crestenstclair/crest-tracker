#!/usr/bin/env python3
"""Build and verify a relocatable local macOS app from the paired checkouts."""
import argparse
import hashlib
import json
import plistlib
import shutil
import subprocess
import tempfile
from pathlib import Path
from develop import ROOT, local_cargo


def resource_entries(source, target):
    """Expand one resource mapping, rejecting escaping paths and LFS pointers."""
    if target.is_absolute() or '..' in target.parts:
        raise ValueError(f'Resource destination escapes the package: {target}')
    directory = source.is_dir()
    candidates = sorted(source.rglob('*')) if directory else [source]
    for item in candidates:
        if item.is_dir():
            continue
        destination = target / item.relative_to(source) if directory else target
        with item.open('rb') as stream:
            if stream.read(128).startswith(b'version https://git-lfs.github.com/spec/v1'):
                raise ValueError(f'Missing Git LFS content: {item}; materialize Synth resources first')
        yield destination, item


def resource_files(synth, tracker=ROOT):
    """Resolve the existing Synth resource map and Tracker examples exactly once."""
    config = json.loads((synth / 'tauri.conf.json').read_text())
    sources = [(synth / source, Path(target))
               for source, target in config['bundle']['resources'].items()]
    sources.append((tracker / 'assets', Path('tracker-examples')))
    files = {}
    for source, target in sources:
        mapped = dict(resource_entries(source, target))
        collisions = files.keys() & mapped.keys()
        if collisions:
            raise ValueError(f'Duplicate resource destination: {collisions}')
        files.update(mapped)
    if not files:
        raise ValueError('No package resources were resolved')
    return files


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verify_resources(bundle, files):
    """Reject absent, altered, extra or externally linked package resources."""
    root = bundle / 'Contents' / 'Resources'
    actual = {item.relative_to(root) for item in root.rglob('*') if not item.is_dir()}
    if actual != set(files):
        raise ValueError(f'Package resource inventory differs: missing={set(files)-actual}, extra={actual-set(files)}')
    for relative, source in files.items():
        packaged = root / relative
        if not packaged.resolve().is_relative_to(root.resolve()):
            raise ValueError(f'Package resource points outside the bundle: {relative}')
        if digest(packaged) != digest(source):
            raise ValueError(f'Package resource differs from its source: {relative}')


def verify_bundle(bundle, files):
    verify_resources(bundle, files)
    contents = bundle / 'Contents'
    info = plistlib.loads((contents / 'Info.plist').read_bytes())
    name = info['CFBundleExecutable']
    if Path(name).name != name:
        raise ValueError('Bundle executable must be a filename')
    executable = contents / 'MacOS' / name
    dependencies = subprocess.run(['otool', '-L', str(executable)], check=True,
                                  capture_output=True, text=True).stdout
    # This package statically links Synth and its native dependencies. A newly
    # introduced non-system dylib needs explicit packaging before admission.
    libraries = [line.strip().split(' (', 1)[0]
                 for line in dependencies.splitlines() if line.startswith('\t')]
    # otool can return success with no dependency rows for a script. This app
    # requires a native executable linked to macOS, not a launcher wrapper.
    if not libraries:
        raise ValueError('Bundle executable has no Mach-O dynamic-library evidence')
    for library in libraries:
        if not library.startswith(('/System/Library/', '/usr/lib/')):
            raise ValueError(f'Unpackaged dynamic dependency: {library}')
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(bundle)], check=True)
    print(f'Verified {len(files)} resource files, system-only dylibs and signature: {bundle}')


def assemble_bundle(bundle, executable, files):
    contents = bundle / 'Contents'
    (contents / 'MacOS').mkdir(parents=True)
    resources = contents / 'Resources'
    resources.mkdir()
    shutil.copy2(executable, contents / 'MacOS' / 'crest-tracker-app')
    for relative, source in files.items():
        target = resources / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
    info = {'CFBundleIdentifier': 'dev.crest.tracker', 'CFBundleName': 'Crest Tracker',
            'CFBundleDisplayName': 'Crest Tracker', 'CFBundleExecutable': 'crest-tracker-app',
            'CFBundlePackageType': 'APPL', 'CFBundleShortVersionString': '0.1.0',
            'CFBundleVersion': '1', 'NSHighResolutionCapable': True,
            'LSMinimumSystemVersion': '11.0'}
    (contents / 'Info.plist').write_bytes(plistlib.dumps(info))
    subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(bundle)], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--synth-path', type=Path, default=ROOT.parent / 'crest-synth')
    parser.add_argument('--release', action='store_true')
    parser.add_argument('--verify-only', type=Path, help='Verify an existing or relocated bundle without rebuilding')
    args = parser.parse_args()
    synth = args.synth_path.resolve()
    files = resource_files(synth)
    if args.verify_only:
        verify_bundle(args.verify_only.resolve(), files)
        return
    subprocess.run(local_cargo(synth, 'build', '-p', 'crest-tracker-app', *(['--release'] if args.release else [])), cwd=ROOT, check=True)
    output = ROOT / 'target' / ('release' if args.release else 'debug')
    bundle = output / 'Crest Tracker.app'
    # A fresh staging tree cannot retain removed resources or leave a partially
    # overwritten app. The previous generated bundle survives any build failure.
    with tempfile.TemporaryDirectory(prefix='.bundle-', dir=output) as temporary:
        staging = Path(temporary) / bundle.name
        assemble_bundle(staging, output / 'crest-tracker-app', files)
        verify_bundle(staging, files)
        previous = Path(temporary) / 'previous.app'
        if bundle.exists():
            bundle.rename(previous)
        try:
            staging.rename(bundle)
        except OSError:
            if previous.exists():
                previous.rename(bundle)
            raise
    print(bundle)


if __name__ == '__main__':
    main()
