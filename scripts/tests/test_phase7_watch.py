"""Watch edits must preserve file encoding while observing decoded compiler text."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from tools.phase7.watch import production

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('phase7_watch', ROOT / 'tools/phase7/watch/run.py')
WATCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(WATCH)


class WatchEncodingTests(unittest.TestCase):
    def test_edits_preserve_bom_encoding_and_source_bytes(self):
        source = 'export const text = "é😀";\r\n'
        suffix = '\n// phase7-watch-equivalent-edit: 00000003\n'
        for encoding, bom in [('utf-8', b''), ('utf-8', b'\xef\xbb\xbf'),
                              ('utf-16-le', b'\xff\xfe'), ('utf-16-be', b'\xfe\xff')]:
            with self.subTest(encoding=encoding, bom=bom):
                original = bom + source.encode(encoding)
                edited = WATCH.edited_source(original, 3)
                self.assertEqual(edited, original + suffix.encode(encoding))
                self.assertEqual(WATCH.compiler_text(edited), (source + suffix).encode())
                self.assertEqual(len(edited), len(WATCH.edited_source(original, 4)))

    def test_invalid_utf8_is_not_lossily_decoded(self):
        original = b'// raw bytes: \xff\xed\xa0\x80\n'
        edited = WATCH.edited_source(original, 1)
        self.assertTrue(edited.startswith(original))
        self.assertEqual(WATCH.compiler_text(edited), edited)
        self.assertEqual(WATCH.compiler_text(b'\xef\xbb\xbf' + edited), edited)

    def test_utf16_replaces_surrogates_and_preserves_ignored_final_byte(self):
        # Pinned decoder replaces an unpaired surrogate and discards odd bytes.
        for bom, payload in [(b'\xff\xfe', b'\x00\xd8A\x00'),
                             (b'\xfe\xff', b'\xd8\x00\x00A')]:
            with self.subTest(bom=bom):
                original = bom + payload + b'\xab'
                edited = WATCH.edited_source(original, 2)
                self.assertTrue(edited.startswith(bom + payload))
                self.assertEqual(edited[-1:], b'\xab')
                self.assertEqual(WATCH.compiler_text(original), '�A'.encode())
                self.assertEqual(WATCH.compiler_text(edited),
                                 '�A\n// phase7-watch-equivalent-edit: 00000002\n'.encode())


class ProductionWatchTests(unittest.TestCase):
    def test_diagnostic_witness_preserves_encoding_and_has_constant_width(self):
        for encoding, bom in [('utf-8', b''), ('utf-8', b'\xef\xbb\xbf'),
                              ('utf-16-le', b'\xff\xfe'), ('utf-16-be', b'\xfe\xff')]:
            with self.subTest(encoding=encoding):
                original = bom + 'export const x = "é";'.encode(encoding)
                raw, token = production.edited_source(original, 1)
                next_raw, next_token = production.edited_source(original, 2)
                self.assertEqual(len(raw), len(next_raw))
                self.assertNotEqual(token, next_token)
                self.assertEqual(WATCH.compiler_text(raw), f'export const x = "é";\n{token};\n'.encode())
                if encoding.startswith('utf-16'):
                    odd, _ = production.edited_source(original + b'\xaa', 1)
                    self.assertEqual(odd, raw + b'\xaa')

    def test_completion_requires_this_edits_diagnostic_and_keeps_roster(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'compiler.log'
            root = Path(directory).resolve()
            for name in ('main.ts', 'globals.d.ts'):
                (root / name).touch()
            log.write_bytes(b"main.ts(5,1): error TS2304: Cannot find name '__phase7_watch_missing_00000001'.\n"
                            + f'{root}/main.ts\n{root}/globals.d.ts\n'.encode() +
                            b'1:00:00 - Found 1 error. Watching for file changes.\n')
            process = Mock()
            process.poll.return_value = None
            reader = production.Completions(log, process, 1, root)
            with patch.object(production.shared, 'sample_os', return_value={'rss_bytes': 10}):
                sample = reader.next('__phase7_watch_missing_00000001')
                self.assertEqual(sample['program_files'], 2)
                self.assertEqual(sample['diagnostics'], 1)
                self.assertEqual(sample['log_end'], log.stat().st_size)
                with log.open('ab') as stream:
                    stream.write(b'Found 1 error. Watching for file changes.\n')
                with self.assertRaisesRegex(ValueError, 'did not check'):
                    reader.next('__phase7_watch_missing_00000002')

    def test_roster_rejects_an_external_source_even_when_cycle_completes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'copy'
            root.mkdir()
            outside = Path(directory).resolve() / 'original.ts'
            outside.touch()
            log = root / 'compiler.log'
            log.write_text(f"main.ts(1,1): error TS2304: Cannot find name 'missing'.\n"
                           f'{outside}\nFound 1 error. Watching for file changes.\n')
            with self.assertRaisesRegex(ValueError, 'source escaped'):
                production.Completions(log, Mock(), 1, root).next('missing')

    def test_an_exited_process_is_not_a_watch_measurement(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'compiler.log'
            log.write_bytes(b'invalid command line\n')
            process = Mock(returncode=2)
            process.poll.return_value = 2
            with self.assertRaisesRegex(ValueError, 'watch exited 2'):
                production.Completions(log, process, 1, Path('/fixture')).next('missing')


if __name__ == '__main__':
    unittest.main()
