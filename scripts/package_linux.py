#!/usr/bin/env python3
"""Build and exercise a relocatable Linux x86_64 Tracker development archive."""
import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

from bundle_resources import digest, resource_files, verify_resource_files
from develop import ROOT


EXECUTABLE = 'crest-tracker-app'


def verify_executable(path):
    """Reject the wrong architecture and missing runtime libraries before packing."""
    with path.open('rb') as stream:
        header = stream.read(64)
    if (len(header) != 64 or header[:6] != b'\x7fELF\x02\x01'
            or int.from_bytes(header[18:20], 'little') != 62):
        raise ValueError('Package requires a native Linux x86_64 ELF executable')
    result = subprocess.run(['ldd', str(path)], check=True, capture_output=True, text=True)
    if 'not found' in result.stdout:
        raise ValueError(f'Missing Linux runtime dependency: {result.stdout}')
    return result.stdout


def inventory(executable, directory):
    """The shipping entry point loads the packaged SoundFont and engine registries."""
    environment = os.environ | {'XDG_CONFIG_HOME': str(directory / 'config')}
    return subprocess.run([str(executable), '--synth-inventory'], cwd=directory,
                          env=environment, capture_output=True, text=True, timeout=90)


def verify_relocated(package, files, working_directory):
    resources = package / 'share' / EXECUTABLE
    verify_resource_files(resources, files)
    executable = package / 'bin' / EXECUTABLE
    verify_executable(executable)
    result = inventory(executable, working_directory)
    if result.returncode:
        raise ValueError(f'Relocated production inventory failed: {result.stderr}')
    value = json.loads(result.stdout)
    if (value.get('scope') != 'synth_production_capability_inventory'
            or value.get('registration_validated') is not True
            or not value.get('instruments') or not value.get('effects')):
        raise ValueError('Relocated package lacks a validated production inventory')
    # The checkout still contains its asset. Removing only the packaged copy
    # must fail instead of silently satisfying the load from that checkout.
    asset = resources / 'sf2' / 'HiDef.sf2'
    hidden = working_directory / 'withheld-soundfont.sf2'
    asset.rename(hidden)
    try:
        if inventory(executable, working_directory).returncode == 0:
            raise ValueError('Missing packaged SoundFont silently fell back to checkout')
    finally:
        hidden.rename(asset)
    verify_resource_files(resources, files)
    return value


def assemble(package, files, commit, dirty):
    binary = package / 'bin' / EXECUTABLE
    binary.parent.mkdir(parents=True)
    shutil.copy2(ROOT / 'target' / 'release' / EXECUTABLE, binary)
    for relative, source in files.items():
        target = package / 'share' / EXECUTABLE / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
    (package / 'runtime-libraries.txt').write_text(verify_executable(binary))
    manifest = tomllib.loads((ROOT / 'Cargo.toml').read_text())
    info = {'commit': commit, 'dirty': dirty, 'platform': 'linux-x86_64',
            'library_rev': manifest['workspace']['dependencies']['crest-performance-core']['rev'],
            'development_candidate': True, 'build_system': platform.platform()}
    (package / 'build-info.json').write_text(json.dumps(info, indent=2) + '\n')
    (package / 'README.txt').write_text(
        'Crest Tracker development package — Linux x86_64\n\n'
        'Keep bin/ and share/ together. Run ./bin/crest-tracker-app from a normal desktop.\n'
        'Built and tested on Ubuntu 24.04; system GTK3, WebKitGTK 4.1, ALSA, DBus,\n'
        'PulseAudio and SDL platform libraries remain runtime dependencies.\n'
        'See runtime-libraries.txt for the build system dependency inventory.\n'
        'The embedded Synth, pinned performance library, examples, SoundFont and notices\n'
        'are included. No source checkout is needed for packaged built-in assets.\n\n'
        'This archive is not Steam Deck or physical-hardware qualification. Linux live\n'
        'performance MIDI/audio clock correlation remains unsupported; ordinary clip\n'
        'playback uses the native frame clock. Native inventory smoke is not UI,\n'
        'audio-device, MIDI-device or timing acceptance.\n')
    paths = sorted(path for path in package.rglob('*') if path.is_file())
    (package / 'SHA256SUMS').write_text(''.join(
        f'{digest(path)}  {path.relative_to(package)}\n' for path in paths))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        raise SystemExit('Run this packager on Linux x86_64')
    files = resource_files(ROOT / 'crates' / 'crest-synth')
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT))
    if dirty:
        raise ValueError('Package requires a clean committed source tree')
    subprocess.run(['cargo', 'build', '--locked', '--release', '-p', 'crest-tracker-app'], cwd=ROOT, check=True)
    output = ROOT / 'target' / 'linux-candidate'
    output.mkdir(parents=True, exist_ok=True)
    name = f'crest-tracker-linux-x86_64-{commit[:12]}'
    with tempfile.TemporaryDirectory(prefix='crest-package-') as temporary:
        root = Path(temporary)
        package = root / name
        assemble(package, files, commit, dirty)
        archive = root / (name + '.tar.gz')
        with tarfile.open(archive, 'w:gz') as bundle:
            bundle.add(package, arcname=name)
        extracted = root / 'relocated directory'
        extracted.mkdir()
        with tarfile.open(archive) as bundle:
            bundle.extractall(extracted, filter='data')
        restored = extracted / name
        subprocess.run(['sha256sum', '--quiet', '-c', 'SHA256SUMS'], cwd=restored, check=True)
        value = verify_relocated(restored, files, root)
        subprocess.run(['sha256sum', '--quiet', '-c', 'SHA256SUMS'], cwd=restored, check=True)
        current = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
        if current != commit or subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT):
            raise ValueError('Source changed during package build; no archive published')
        shutil.copy2(archive, output / archive.name)
        (output / (archive.name + '.sha256')).write_text(f'{digest(archive)}  {archive.name}\n')
        (output / 'inventory.json').write_text(json.dumps(value, indent=2) + '\n')
        print(f'Verified relocated package and missing-asset rejection: {output / archive.name}')


if __name__ == '__main__':
    main()
