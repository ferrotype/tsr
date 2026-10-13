"""Frozen, bounded preparation controls; no compiler build or live workload."""
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    'phase7_stream', Path(__file__).resolve().parents[2] / 'tools/phase7/budget/stream.py')
stream = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(stream)
snapshot = stream.snapshot


class StreamTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.root = self.base / 'source'
        self.root.mkdir()
        self.file = self.root / 'input.ts'
        self.raw = b'x' * (stream.CHUNK + 3)
        self.file.write_bytes(self.raw)
        (self.root / 'unread').write_bytes(b'known but not supplied')
        (self.root / 'empty').mkdir()
        (self.root / 'alias').symlink_to('input.ts')
        self.inventory, self.contents = self.base / 'inventory', self.base / 'contents'
        snapshot.inventory(self.root, self.inventory)
        log = self.base / 'reads.ndjson'
        read = {'version': 1, 'kind': 'read', 'path_b64': snapshot.b64(os.fsencode(self.file)),
                'canonical_path_b64': snapshot.b64(os.fsencode(self.file)), 'found': True,
                'raw_size': str(len(self.raw)), 'sha256': hashlib.sha256(self.raw).hexdigest()}
        log.write_bytes(snapshot.encode(read) + snapshot.encode(
            {'version': 1, 'kind': 'complete', 'read_events': '1'}))
        snapshot.freeze(self.inventory, [log], self.contents)
        self.input = self.base / 'input.json'
        self.input.write_bytes(snapshot.encode({'current_directory': list(os.fsencode(self.root)),
            'case_sensitive': True, 'entries': [], 'operation': 'check',
            'configuration': {'config_path': list(os.fsencode(self.root / 'tsconfig.json')),
                              'arguments': []}}))

    def archive(self):
        return stream.Archive(self.inventory, self.contents, self.input)

    def test_bounded_frames_reassemble_raw_bytes_and_keep_unread_metadata(self):
        archive = self.archive()
        with patch.object(stream, 'MAX_METADATA', 512):
            commands = list(archive.preparation())
        first, kind = commands[0]
        self.assertEqual(kind, 'preparing')
        self.assertEqual(first['expected_entries'], '5')
        self.assertEqual(first['expected_files'], '2')
        self.assertEqual(first['expected_contents'], '1')
        self.assertEqual(first['input']['entries'], [])
        metadata, chunks = [], []
        for request, _ in commands:
            self.assertLessEqual(len(stream.probe.encoded(request)), stream.MAX_FRAME)
            self.assertEqual(request['version'], 2)
            if request['command'] == 'prepare_metadata':
                self.assertLessEqual(len(stream.probe.encoded(request['entries'])) - 1, 512)
                metadata.extend(request['entries'])
            if request['command'] == 'content_chunk':
                self.assertEqual(int(request['offset']), sum(map(len, chunks)))
                decoded = base64.b64decode(request['bytes_b64'], validate=True)
                self.assertLessEqual(len(decoded), stream.CHUNK)
                chunks.append(decoded)
        self.assertEqual(len(chunks), 2)
        self.assertEqual(b''.join(chunks), self.raw)
        self.assertEqual(len(metadata), 5)
        self.assertEqual(commands[-1][0]['command'], 'prepare_end')

    def test_replay_uses_archived_contents_without_reopening_original_files(self):
        archive = self.archive()
        self.file.write_bytes(b'changed after freeze')
        commands = list(archive.preparation())
        chunks = [base64.b64decode(request['bytes_b64']) for request, _ in commands
                  if request['command'] == 'content_chunk']
        self.assertEqual(b''.join(chunks), self.raw)

    def test_blob_tampering_fails_before_start_and_during_stream(self):
        archive = self.archive()
        blob = next((self.contents / 'blobs').iterdir())
        changed = b'y' + self.raw[1:]
        blob.write_bytes(changed)
        with self.assertRaisesRegex(ValueError, 'digest differs'):
            self.archive()
        with self.assertRaisesRegex(ValueError, 'changed during streaming'):
            list(archive.preparation())

    def test_receipt_counts_and_scope_are_not_trusted(self):
        receipt = self.contents / 'complete.json'
        value = json.loads(receipt.read_bytes())
        value['files'] += 1
        receipt.write_bytes(snapshot.encode(value))
        with self.assertRaisesRegex(ValueError, 'counts differ'):
            self.archive()
        value['files'] -= 1
        receipt.write_bytes(snapshot.encode(value))
        request = json.loads(self.input.read_bytes())
        request['configuration']['config_path'] = list(b'/outside/tsconfig.json')
        self.input.write_bytes(snapshot.encode(request))
        with self.assertRaisesRegex(ValueError, 'inside the archived scope'):
            self.archive()

    def test_changed_archive_is_rejected_before_completion(self):
        archive = self.archive()
        preparation = archive.preparation()
        next(preparation)
        with (self.inventory / 'metadata.ndjson').open('ab') as out:
            out.write(b'{}\n')
        with self.assertRaisesRegex(ValueError, 'changed during replay'):
            list(preparation)

    def test_special_or_symlink_blobs_cannot_block_the_controller(self):
        pipe = self.base / 'pipe'
        os.mkfifo(pipe)
        with self.assertRaisesRegex(ValueError, 'not a regular file'):
            stream.open_blob(pipe)
        alias = self.base / 'alias'
        alias.symlink_to(self.file)
        with self.assertRaises(OSError):
            stream.open_blob(alias)


if __name__ == '__main__':
    unittest.main()
