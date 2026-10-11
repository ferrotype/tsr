import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    'phase7_snapshot', Path(__file__).resolve().parents[2] / 'tools/phase7/budget/snapshot.py')
snapshot = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(snapshot)


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name).resolve()
        self.root = self.base / 'source'
        self.root.mkdir()
        (self.root / 'empty').mkdir()
        (self.root / 'a.bin').write_bytes(b'\xff\xfeA\0')
        (self.root / 'unread.unusual').write_bytes(b'not read by compiler')
        (self.root / 'alias').symlink_to('a.bin')
        (self.root / 'dangling').symlink_to('missing')
        self.inventory = self.base / 'inventory'

    def read_log(self, name, paths):
        path = self.base / name
        values = []
        for item in paths:
            raw = item.read_bytes()
            values.append({'version': 1, 'kind': 'read', 'path_b64': snapshot.b64(os.fsencode(item)),
                           'canonical_path_b64': snapshot.b64(os.fsencode(item.resolve())),
                           'found': True, 'sha256': hashlib.sha256(raw).hexdigest(), 'raw_size': str(len(raw))})
        values.append({'version': 1, 'kind': 'complete', 'read_events': str(len(values))})
        path.write_bytes(b''.join(snapshot.encode(value) for value in values))
        return path

    def test_metadata_keeps_unread_files_empty_directories_links_and_raw_contents(self):
        snapshot.inventory(self.root, self.inventory)
        _, _, entries = snapshot.load_inventory(self.inventory)
        self.assertEqual(len(entries), 6)
        self.assertEqual(entries[os.fsencode(self.root / 'unread.unusual')]['kind'], 'physical_file')
        self.assertEqual(entries[os.fsencode(self.root / 'empty')]['kind'], 'directory')
        self.assertEqual(snapshot.unb64(entries[os.fsencode(self.root / 'dangling')]['target_b64']), b'missing')
        go = self.read_log('go.ndjson', [self.root / 'alias'])
        rust = self.read_log('rust.ndjson', [self.root / 'a.bin'])
        output = self.base / 'contents'
        result = snapshot.freeze(self.inventory, [go, rust], output)
        self.assertEqual(result['files'], 1)
        blobs = list((output / 'blobs').iterdir())
        self.assertEqual(len(blobs), 1)
        self.assertEqual(blobs[0].read_bytes(), b'\xff\xfeA\0')
        self.assertEqual(len(list(snapshot.rows(output / 'contents.ndjson'))), 1)

    def test_changes_after_discovery_fail_without_complete_receipt(self):
        snapshot.inventory(self.root, self.inventory)
        log = self.read_log('reads.ndjson', [self.root / 'a.bin'])
        (self.root / 'unread.unusual').write_bytes(b'changed even though unread')
        with self.assertRaisesRegex(ValueError, 'changed since inventory'):
            snapshot.freeze(self.inventory, [log], self.base / 'contents')
        self.assertFalse((self.base / 'contents/complete.json').exists())

    def test_incomplete_and_inconsistent_logs_cannot_supply_contents(self):
        snapshot.inventory(self.root, self.inventory)
        _, _, entries = snapshot.load_inventory(self.inventory)
        log = self.read_log('reads.ndjson', [self.root / 'a.bin'])
        values = list(snapshot.rows(log))
        log.write_bytes(snapshot.encode(values[0]))
        with self.assertRaisesRegex(ValueError, 'did not complete'):
            snapshot.discovered_reads([log], entries)
        changed = dict(values[0], sha256='0' * 64)
        log.write_bytes(snapshot.encode(values[0]) + snapshot.encode(changed)
                        + snapshot.encode({'version': 1, 'kind': 'complete', 'read_events': '2'}))
        with self.assertRaisesRegex(ValueError, 'changing file content'):
            snapshot.discovered_reads([log], entries)

    def test_external_symlink_is_rejected_and_replaced_directory_is_not_followed(self):
        (self.root / 'outside').symlink_to(self.base)
        with self.assertRaisesRegex(ValueError, 'leaves snapshot scope'):
            snapshot.inventory(self.root, self.inventory)
        (self.root / 'empty').rmdir()
        (self.root / 'empty').symlink_to(self.base)
        with self.assertRaises(OSError):
            snapshot.open_physical(os.fsencode(self.root / 'empty/anything'), os.fsencode(self.root))

    def test_metadata_tampering_is_rejected(self):
        snapshot.inventory(self.root, self.inventory)
        with (self.inventory / 'metadata.ndjson').open('ab') as out:
            out.write(b'{}\n')
        with self.assertRaisesRegex(ValueError, 'differs from inventory'):
            snapshot.load_inventory(self.inventory)

    def test_external_dangling_link_stays_missing_and_target_creation_invalidates(self):
        target = self.base / 'absent'
        (self.root / 'container-link').symlink_to(target)
        snapshot.inventory(self.root, self.inventory)
        root, record, entries = snapshot.load_inventory(self.inventory)
        row = entries[os.fsencode(self.root / 'container-link')]
        self.assertEqual(snapshot.unb64(row['target_b64']), os.fsencode(target))
        snapshot.verify_unchanged(root, record)
        target.write_bytes(b'newly exists outside scope')
        with self.assertRaisesRegex(ValueError, 'leaves snapshot scope'):
            snapshot.verify_unchanged(root, record)


if __name__ == '__main__':
    unittest.main()
