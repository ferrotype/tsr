"""Sample and targeted runs stay reproducible and cannot certify acceptance."""
import contextlib
import copy
import io
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import phase2_compare as compare  # noqa: E402
import phase2_corpus as corpus  # noqa: E402
import phase2_inventory as inventory  # noqa: E402
import phase2_native as native  # noqa: E402
import phase2_producers as producers  # noqa: E402
import phase2_blockers as blockers  # noqa: E402
from phase2_fixtures import build_capture, load, CONTROL, PANIC, MAPPER  # noqa: E402
from s08_oracle import digest  # noqa: E402


class Selection(unittest.TestCase):
    def test_frozen_sample_is_not_a_prefix(self):
        rows = inventory.executed()
        selected = corpus.select_rows(rows, sample=True)
        self.assertEqual(len(selected), 300)
        self.assertEqual(selected, [row for row in rows if row['sample']])
        self.assertNotEqual(selected, rows[:300])
        extra = next(row['id'] for row in rows if not row['sample'])
        self.assertEqual(corpus.select_rows(rows, sample=True, cases=[extra, selected[-1]['id']]),
                         [row for row in rows if row['sample'] or row['id'] == extra])
        self.assertEqual(corpus.select_rows(rows, cases=[rows[-1]['id'], rows[0]['id']]), [rows[0], rows[-1]])

    def test_invalid_selection_is_rejected(self):
        rows = [{'id': 'a', 'sample': True}, {'id': 'b', 'sample': False}]
        for kwargs in ({'cases': ['a', 'a']}, {'cases': ['unknown']}, {'limit': 0}, {'limit': -1},
                       {'limit': True}, {'sample': True, 'limit': 1}, {'cases': ['a'], 'limit': 1},
                       {'sample': 'true'}, {'cases': 'a'}):
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                corpus.select_rows(rows, **kwargs)
        informational = next(row['id'] for row in inventory.read()['rows'] if row['tier'] == 'informational')
        with self.assertRaisesRegex(ValueError, 'non-executed'):
            corpus.select_rows(inventory.executed(), cases=[informational])

    def test_partial_metadata_cannot_claim_full(self):
        with self.assertRaisesRegex(ValueError, 'disagrees'):
            corpus.capture_selection({'selection': corpus.selection(sample=True), 'partial': False})
        with self.assertRaisesRegex(ValueError, 'authenticated selection'):
            corpus.capture_selection({'partial': True})
        self.assertEqual(corpus.capture_selection({'partial': False}), corpus.selection())


class Reports(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix='phase2-report-'))
        self.addCleanup(shutil.rmtree, self.directory)
        self.fixture = load()
        self.records = self.fixture['records']
        by_id = {row['id']: row for row in inventory.executed()}
        self.inventory = [copy.deepcopy(by_id[vid]) for vid in self.records]
        # A tiny substitute inventory with a non-prefix sample, using real observations.
        for row in self.inventory:
            row['sample'] = row['id'] == PANIC
        self.native_rows = [self.records[row['id']]['native'] for row in self.inventory]
        self.native_report = {'pin': self.fixture['provenance']['pin'],
                              'observation_sha256': self.fixture['provenance']['native_observation_sha256']}
        self.addCleanup(patch.stopall)
        patch.object(native, 'load_capture', return_value=(self.directory, self.native_report, self.native_rows)).start()
        patch.object(native, 'current').start()
        patch.object(inventory, 'executed', return_value=self.inventory).start()
        patch.object(corpus, 'sources', return_value={}).start()
        (self.directory / 'verified.json').write_text(json.dumps(self.native_report))
        (self.directory / 'report.json').write_text(json.dumps(self.native_report))

    def build(self, selected):
        if (self.directory / 'cases').exists():
            shutil.rmtree(self.directory / 'cases')
        rows = corpus.select_rows(self.inventory, **selected)
        self.metadata = build_capture(self.directory, [row['id'] for row in rows], selection=selected)
        return rows

    def report(self, **kwargs):
        with contextlib.redirect_stdout(io.StringIO()):
            return compare.report(self.directory, self.directory, **kwargs)

    def test_request_builder_uses_sample_and_targets_in_inventory_order(self):
        selected = corpus.selection(sample=True, cases=[CONTROL])
        expected = self.build(selected)
        with patch.object(inventory, 'read', return_value={'rows': self.inventory}), patch.object(
                corpus, 'loading_requests', return_value={vid: record['request']['loading'] for vid, record in self.records.items()}):
            _, requests, observations = corpus.requests(self.directory, **selected)
        self.assertEqual([row['id'] for row in requests], [row['id'] for row in expected])
        self.assertEqual([row['id'] for row in observations], [row['id'] for row in expected])
        for request in requests:
            self.assertEqual(request, self.records[request['id']]['request'])

    def test_run_and_resume_bind_the_selected_requests_and_raw_outputs(self):
        output = self.directory / 'sample-run'
        selected = corpus.selection(sample=True, cases=[CONTROL])

        def build(directory, **kwargs):
            directory.mkdir()
            snapshot = directory / 'snapshot'
            snapshot.mkdir()
            binary = directory / 'binary'
            binary.write_bytes(b'fixture only')
            return {'sources': {}, 'source_snapshot': str(snapshot), 'binary': str(binary),
                    'binary_sha256': digest(binary.read_bytes())}

        def execute(binary, case_dir, request, timeout, **kwargs):
            corpus.p4.begin_case(case_dir, request)
            record = self.records[request['id']]
            for name, raw in record['artifacts'].items():
                (case_dir / name).write_bytes(bytes.fromhex(raw))
            return kwargs['validator'](request, copy.deepcopy(record['envelope']['row']))

        with (patch.object(inventory, 'read', return_value={'rows': self.inventory}),
              patch.object(corpus, 'loading_requests', return_value={vid: r['request']['loading'] for vid, r in self.records.items()}),
              patch.object(corpus.p4, 'build', side_effect=build) as compiled,
              patch.object(corpus.p4, 'execute_case', side_effect=execute) as executed,
              contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO())):
            result = corpus.run(self.directory, output, 1, 60, **selected)
            again = corpus.run(self.directory, output, 1, 60, resume=True, **selected)
        self.assertEqual(compiled.call_count, 1)
        self.assertEqual(executed.call_count, 2)  # no resumed row was re-executed
        self.assertEqual(result, again)
        self.assertTrue(result['summary']['partial'])
        self.assertEqual(result['selection'], selected)
        self.assertEqual([r['id'] for r in corpus.p4.read(output / 'requests.json')], [CONTROL, PANIC])

    def test_partial_report_uses_only_selected_native_rows(self):
        selected = corpus.selection(sample=True, cases=[CONTROL, MAPPER])
        expected = self.build(selected)
        result = self.report()
        self.assertTrue(result['partial'])
        self.assertEqual(result['selection'], selected)
        self.assertEqual(result['executed'], 3)
        self.assertEqual([r['id'] for r in result['rows']], [r['id'] for r in expected])
        for row in result['rows']:
            self.assertNotIn('harness_error', row)
            self.assertEqual(set(row['outcomes']), set(compare.DOMAINS))
            self.assertTrue(set(row['outcomes'].values()) <= set(compare.CATEGORIES))
            if any(o not in ('match', 'disabled') for o in row['outcomes'].values()):
                self.assertIsInstance(row['bucket'], str)
                self.assertIsInstance(row['owner'], str)
            for detail in row.get('details', {}).values():
                if detail['category'] == 'unsupported':
                    self.assertTrue(detail['operation'])
                    self.assertNotEqual(detail['operation'], 'P5 native baseline walker/display schedule')

    def test_full_report_still_covers_every_inventory_row(self):
        self.build(corpus.selection())
        result = self.report()
        self.assertFalse(result['partial'])
        self.assertEqual(result['executed'], len(self.inventory))

    def test_partial_report_cannot_be_recorded(self):
        for selected in (corpus.selection(sample=True), corpus.selection(cases=[CONTROL]), corpus.selection(limit=2)):
            with self.subTest(selected=selected):
                self.build(selected)
                record = self.directory / 'must-not-be-created.json'
                with patch.object(compare, 'RECORD', record), self.assertRaisesRegex(ValueError, 'informational'):
                    self.report(record=True)
                self.assertFalse(record.exists())

    def test_partial_report_cannot_replace_the_acceptance_blocker_register(self):
        self.build(corpus.selection(sample=True))
        comparison = self.report()
        with self.assertRaisesRegex(ValueError, 'requires a full comparison'):
            blockers.build(self.directory, self.directory, record=True)
        self.assertFalse(blockers.complete({'entries': []}, comparison))

    def test_full_record_requires_current_sources(self):
        self.build(corpus.selection())
        with self.assertRaisesRegex(ValueError, 'current sources'):
            self.report(record=True)

    def test_inconsistent_selection_is_rejected_at_comparison(self):
        # IDs are authenticated against the recorded selection, not just shared
        # by the Rust request and response; rebinding their hashes cannot help.
        build_capture(self.directory, [CONTROL], selection=corpus.selection(sample=True))
        with self.assertRaisesRegex(ValueError, 'missing, extra or reordered'):
            self.report()

    def test_different_native_capture_or_inventory_is_rejected(self):
        self.build(corpus.selection(cases=[CONTROL]))
        for target in ('inventory', 'native'):
            metadata = copy.deepcopy(self.metadata)
            if target == 'inventory':
                metadata['inventory_sha256'] = '0' * 64
            else:
                metadata['native']['observation_sha256'] = '0' * 64
            corpus.p4.atomic(self.directory / 'capture.json', metadata)
            path = self.directory / 'cases/00000/result.json'
            envelope = corpus.p4.read(path)
            envelope['capture_sha256'] = digest(corpus.p4.canonical(metadata) + b'\n')
            corpus.p4.atomic(path, envelope)
            with self.subTest(target=target), self.assertRaisesRegex(ValueError, 'different inventory or native'):
                self.report()

    def test_resume_rejects_changed_selection_even_for_same_requests(self):
        selected = corpus.selection(cases=[CONTROL])
        self.build(selected)
        self.metadata['native'] = {'directory': str(self.directory),
                                   'report_sha256': digest((self.directory / 'report.json').read_bytes()),
                                   'observation_sha256': self.native_report['observation_sha256']}
        corpus.p4.atomic(self.directory / 'capture.json', self.metadata)
        requests = corpus.p4.read(self.directory / 'requests.json')
        with patch.object(corpus, 'requests', return_value=(self.native_report, requests, [])), patch.object(corpus.p4, 'build') as build:
            with self.assertRaisesRegex(ValueError, 'identical native capture'):
                corpus.run(self.directory, self.directory, 1, 60, resume=True, sample=True)
            build.assert_not_called()

    def test_producer_withholds_metrics_for_partial_or_stale_runs(self):
        # Selecting every ID explicitly still means an informational targeted
        # run, even when its rows, requests and current sources match a full run.
        for selected in (corpus.selection(cases=[r['id'] for r in self.inventory]), corpus.selection()):
            with self.subTest(selected=selected):
                self.build(selected)
                doc = {'counts': {'executed': len(self.inventory)}}
                review = {'reference_disagreements': [], 'input_mismatches': [], 'states': {'executed': len(self.inventory)}}
                provenance = self.directory / 'provenance.json'
                provenance.write_text(json.dumps(review))
                requests = corpus.p4.read(self.directory / 'requests.json')
                with (patch.object(inventory, 'read', return_value=doc),
                      patch.object(inventory, 'build', return_value=doc),
                      patch.object(inventory, 'render', return_value=inventory.INVENTORY.read_bytes()),
                      patch.object(native, 'review', return_value=review),
                      patch.object(native, 'PROVENANCE', provenance),
                      patch.object(corpus, 'sources', return_value=self.metadata['build']['sources'] if selected['cases'] else {}),
                      patch.object(corpus, 'requests', return_value=(self.native_report, requests, [])),
                      patch.object(compare, 'report') as report,
                      contextlib.redirect_stderr(io.StringIO())):
                    result = producers.checker(self.directory, self.directory)['metrics']
                self.assertTrue(result['native_verified'])
                self.assertFalse(result['harness_valid'])
                self.assertNotIn('errors_parity', result)
                self.assertNotIn('display_parity', result)
                report.assert_not_called()


if __name__ == '__main__':
    unittest.main()
