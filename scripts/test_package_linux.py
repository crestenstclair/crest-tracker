"""Reject incomplete or unlaunchable Linux packages before publishing them."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from package_linux import verify_executable


class LinuxExecutableTests(unittest.TestCase):
    def test_invalid_executable_rejects_before_running_ldd(self):
        # A launcher script, truncated ELF, big-endian ELF and ARM64 ELF cannot
        # be presented as the shipping Linux x86_64 binary.
        valid = bytearray(64)
        valid[:6] = b'\x7fELF\x02\x01'
        valid[18:20] = (62).to_bytes(2, 'little')
        big_endian = valid.copy()
        big_endian[5] = 2
        arm = valid.copy()
        arm[18:20] = (183).to_bytes(2, 'little')
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / 'crest-tracker-app'
            for content in (b'#!/bin/sh\n', valid[:20], big_endian, arm):
                executable.write_bytes(content)
                with self.subTest(content=content), patch('package_linux.subprocess.run') as run:
                    with self.assertRaisesRegex(ValueError, 'native Linux x86_64 ELF'):
                        verify_executable(executable)
                    run.assert_not_called()

    def test_unresolved_dependency_rejects_even_when_ldd_succeeds(self):
        header = bytearray(64)
        header[:6] = b'\x7fELF\x02\x01'
        header[18:20] = (62).to_bytes(2, 'little')
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / 'crest-tracker-app'
            executable.write_bytes(header)
            with patch('package_linux.subprocess.run') as run:
                run.return_value = subprocess.CompletedProcess(
                    [], 0, 'libwebkit2gtk-4.1.so.0 => not found\n', '')
                with self.assertRaisesRegex(ValueError, 'Missing Linux runtime dependency'):
                    verify_executable(executable)
