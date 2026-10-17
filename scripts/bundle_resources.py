"""One built-in resource inventory for native Tracker packages."""
import hashlib
import json
from pathlib import Path
from develop import ROOT


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


def verify_resource_files(root, files):
    """Reject absent, altered, extra or externally linked package resources."""
    actual = {item.relative_to(root) for item in root.rglob('*') if not item.is_dir()}
    if actual != set(files):
        raise ValueError(f'Package resource inventory differs: missing={set(files)-actual}, extra={actual-set(files)}')
    for relative, source in files.items():
        packaged = root / relative
        if not packaged.resolve().is_relative_to(root.resolve()):
            raise ValueError(f'Package resource points outside the bundle: {relative}')
        if digest(packaged) != digest(source):
            raise ValueError(f'Package resource differs from its source: {relative}')


