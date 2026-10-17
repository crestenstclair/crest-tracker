#!/usr/bin/env python3
"""Build and verify a relocatable local macOS app from the workspace."""
import argparse
import plistlib
import shutil
import subprocess
import tempfile
from pathlib import Path
from develop import ROOT, local_cargo
from bundle_resources import resource_files, verify_resource_files


def verify_resources(bundle, files):
    verify_resource_files(bundle / 'Contents' / 'Resources', files)


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
    parser.add_argument('--synth-path', type=Path, default=ROOT / 'crates' / 'crest-synth')
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
