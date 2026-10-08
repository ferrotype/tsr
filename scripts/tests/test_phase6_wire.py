"""The Phase 6 wire probe: a silent server does not block the capture."""
import os
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools.phase6.wire import capture  # noqa: E402


class Probe(unittest.TestCase):
    def test_a_silent_server_is_recorded_and_killed_within_the_deadline(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / 'silent'
            # Reads its input and never answers.
            binary.write_text('#!/bin/sh\nexec sleep 30\n')
            binary.chmod(0o755)
            record = capture.run_script(binary, 'sync', Path(tmp), timeout=0.5)
        self.assertEqual(record['steps'][0]['name'], 'initialize')
        self.assertIn('no reply within', record['steps'][0]['error'])
        self.assertEqual(len(record['steps']), 1)
        self.assertIsNotNone(record['exit'])


if __name__ == '__main__':
    unittest.main()
