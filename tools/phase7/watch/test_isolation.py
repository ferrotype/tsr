#!/usr/bin/env python3
"""Isolation controls; --probe additionally runs a short actual native watch."""
import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('watch_runner', HERE / 'run.py')
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)
PROBE = None


class Isolation(unittest.TestCase):
    def test_native_watch_encoded_sources(self):
        if PROBE is None:
            self.skipTest('supply --probe for live encoded-source watch checks')
        for encoding, bom in [('utf-8', b'\xef\xbb\xbf'),
                              ('utf-16-le', b'\xff\xfe'), ('utf-16-be', b'\xfe\xff')]:
            with self.subTest(encoding=encoding), tempfile.TemporaryDirectory(prefix='phase7-watch-encoding-') as temporary:
                root = Path(temporary)
                source = root / 'source'
                shutil.copytree(HERE / 'fixtures/project', source)
                path = source / 'index.ts'
                original = bom + path.read_text().encode(encoding)
                path.write_bytes(original)
                output = root / 'capture'
                result = subprocess.run(
                    [sys.executable, str(HERE / 'run.py'), 'run', '--project', str(source),
                     '--edit-file', 'index.ts', '--output', str(output), '--probe', str(PROBE),
                     '--cycles', '4', '--warmup', '2', '--timeout', '30'],
                    capture_output=True, text=True, timeout=180)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr +
                                 '\n' + (output / 'compiler.log').read_text())
                self.assertEqual(path.read_bytes(), original)
                record = json.loads((output / 'watch.json').read_text())
                self.assertEqual(record['status'], 'pass')
                self.assertEqual(record['source_encoding'], encoding)
                self.assertIn('testing-mode', record['memory_scope'])
                copied = (output / 'project/index.ts').read_bytes()
                self.assertEqual(copied, RUNNER.edited_source(original, 4))

    def test_external_link_refused_before_copy(self):
        with tempfile.TemporaryDirectory(prefix='phase7-watch-isolation-') as temporary:
            root = Path(temporary)
            source = root / 'source'
            source.mkdir()
            external = root / 'external.ts'
            external.write_text('export const sentinel = 1;\n')
            (source / 'escape.ts').symlink_to(external)
            with self.assertRaisesRegex(ValueError, 'external symlink'):
                RUNNER.inventory(source)
            self.assertEqual(external.read_text(), 'export const sentinel = 1;\n')

    def test_copied_internal_links_never_edit_original(self):
        modes = ['bytes', 'clone'] if sys.platform == 'darwin' else ['bytes']
        for mode in modes:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory(prefix='phase7-watch-isolation-') as temporary:
                root = Path(temporary)
                source = root / 'source'
                source.mkdir()
                original = source / 'index.ts'
                original.write_text('export const sentinel = 1;\n')
                (source / 'absolute.ts').symlink_to(original)
                (source / 'relative.ts').symlink_to('index.ts')
                (source / 'dangling.ts').symlink_to('unbuilt/index.ts')
                before = RUNNER.inventory(source)
                copied = root / 'copy'
                RUNNER.isolated_copy(source, copied, before, mode)
                self.assertNotEqual(original.stat().st_ino, (copied / 'index.ts').stat().st_ino)
                self.assertFalse((copied / 'dangling.ts').exists())
                self.assertTrue((copied / 'dangling.ts').is_symlink())
                (copied / 'absolute.ts').write_text('changed only in copy\n')
                self.assertEqual((copied / 'relative.ts').read_text(), 'changed only in copy\n')
                self.assertEqual(RUNNER.inventory(source), before)

    def test_native_watch_cannot_emit_to_configured_external_paths(self):
        if PROBE is None:
            self.skipTest('supply --probe for live native-watch output isolation')
        with tempfile.TemporaryDirectory(prefix='phase7-watch-live-isolation-') as temporary:
            root = Path(temporary)
            source = root / 'source'
            shutil.copytree(HERE / 'fixtures/project', source)
            external = root / 'external-output'
            external.mkdir()
            (external / 'sentinel').write_text('unchanged\n')
            config = json.loads((source / 'tsconfig.json').read_text())
            config['compilerOptions'].update({
                'noEmit': False, 'incremental': True, 'composite': True,
                'outDir': str(external),
                'tsBuildInfoFile': str(external / 'unsafe.tsbuildinfo'),
            })
            (source / 'tsconfig.json').write_text(json.dumps(config))
            before = RUNNER.inventory(source)
            external_before = RUNNER.inventory(external)
            output = root / 'capture'
            command = [sys.executable, str(HERE / 'run.py'), 'run',
                       '--project', str(source), '--edit-file', 'index.ts',
                       '--output', str(output), '--probe', str(PROBE),
                       '--cycles', '4', '--warmup', '2', '--timeout', '30']
            result = subprocess.run(command, capture_output=True, text=True, timeout=180)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr +
                             '\n' + (output / 'compiler.log').read_text())
            self.assertEqual(RUNNER.inventory(source), before)
            self.assertEqual(RUNNER.inventory(external), external_before)
            self.assertEqual(json.loads((output / 'watch.json').read_text())['status'], 'pass')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--probe', type=Path)
    arguments = parser.parse_args()
    PROBE = arguments.probe.resolve(strict=True) if arguments.probe else None
    unittest.main(argv=[sys.argv[0]])
