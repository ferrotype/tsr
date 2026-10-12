"""Watch edits must preserve file encoding while observing decoded compiler text."""
import importlib.util
from pathlib import Path
import unittest

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


if __name__ == '__main__':
    unittest.main()
