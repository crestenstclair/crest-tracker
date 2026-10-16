"""Package failures must be caught before replacing a runnable app."""
import json
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from bundle_macos import resource_files, verify_bundle, verify_resources


class BundleResourceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.synth = self.root / 'synth'
        self.synth.mkdir()
        self.tracker = self.root / 'tracker'
        (self.tracker / 'assets').mkdir(parents=True)
        (self.tracker / 'assets' / 'example.crest-song').write_text('example')
        (self.synth / 'sound.sf2').write_bytes(b'RIFF complete resource')
        self.configure({'sound.sf2': 'sf2/HiDef.sf2'})

    def configure(self, resources):
        (self.synth / 'tauri.conf.json').write_text(json.dumps({'bundle': {'resources': resources}}))

    def package(self):
        files = resource_files(self.synth, self.tracker)
        bundle = self.root / 'relocated' / 'Crest Tracker.app'
        for relative, source in files.items():
            destination = bundle / 'Contents/Resources' / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(source.read_bytes())
        return bundle, files

    def test_relocated_resources_match_sources_and_reject_corruption(self):
        bundle, files = self.package()
        verify_resources(bundle, files)
        asset = bundle / 'Contents/Resources/sf2/HiDef.sf2'
        asset.write_bytes(b'altered')
        with self.assertRaisesRegex(ValueError, 'differs from its source'):
            verify_resources(bundle, files)
        asset.unlink()
        with self.assertRaisesRegex(ValueError, 'inventory differs'):
            verify_resources(bundle, files)

    def test_stale_files_and_external_links_reject(self):
        bundle, files = self.package()
        stale = bundle / 'Contents/Resources/stale.wav'
        stale.write_bytes(b'old')
        with self.assertRaisesRegex(ValueError, 'inventory differs'):
            verify_resources(bundle, files)
        stale.unlink()
        asset = bundle / 'Contents/Resources/sf2/HiDef.sf2'
        asset.unlink()
        asset.symlink_to(self.synth / 'sound.sf2')
        with self.assertRaisesRegex(ValueError, 'outside the bundle'):
            verify_resources(bundle, files)

    def test_missing_lfs_content_rejects_before_build(self):
        (self.synth / 'sound.sf2').write_bytes(b'version https://git-lfs.github.com/spec/v1\noid sha256:missing\n')
        with self.assertRaisesRegex(ValueError, 'Missing Git LFS content'):
            resource_files(self.synth, self.tracker)

    def test_escaping_and_colliding_destinations_reject(self):
        for destination in ('../escaped', '/absolute'):
            self.configure({'sound.sf2': destination})
            with self.assertRaisesRegex(ValueError, 'escapes the package'):
                resource_files(self.synth, self.tracker)
        self.configure({'sound.sf2': 'tracker-examples/example.crest-song'})
        with self.assertRaisesRegex(ValueError, 'Duplicate resource destination'):
            resource_files(self.synth, self.tracker)

    def test_non_native_executable_cannot_pass_an_empty_otool_result(self):
        bundle, files = self.package()
        (bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleExecutable': 'launcher',
        }))
        for output in ('', 'launcher: is not an object file\n'):
            with self.subTest(output=output), patch('bundle_macos.subprocess.run') as run:
                run.return_value = subprocess.CompletedProcess([], 0, output, '')
                with self.assertRaisesRegex(ValueError, 'no Mach-O'):
                    verify_bundle(bundle, files)
                run.assert_called_once()
