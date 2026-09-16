"""P7 evidence replay tests; no builds, benchmark processes or corpus runs."""
import copy
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import s08_checkerbench as checker
import s08_measurement as measurement
import s08_relater as relater
from s08_oracle import canonical, digest
from s08_p4 import canonical as request_bytes


class CheckerRequests(unittest.TestCase):
    def test_written_requests_preserve_semantic_map_order(self):
        loading = {
            'options': {'paths': {'@interface/*': ['src/interface/*'], '@blah': ['blah'], '@humbug/*': ['*/generated']}},
            'config_raw': {'files': ['main.ts'], 'compilerOptions': {'paths': {'z/*': ['z/*'], 'a/*': ['a/*']}}},
        }
        request = {
            'id': 'case', 'acceptance_tier': 'acceptance', 'loading': loading,
            'diagnostic_phases': ['config'], 'type_baseline_requested': False,
            'public_type_strings': True, 'error_baseline_requested': True,
        }
        frozen = {**request, 'loading_request_sha256': digest(request_bytes(loading) + b'\n')}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            corpus = root / 'corpus'
            corpus.mkdir()
            (corpus / 'requests.json').write_bytes(request_bytes([request]) + b'\n')
            (root / 'data/s07').mkdir(parents=True)
            (root / 'data/s08').mkdir(parents=True)
            (root / 'data/s07/subset.json').write_text('{}')
            (root / 'data/s08/baseline-requests.json').write_bytes(canonical({'requests': [frozen]}))
            with patch.object(checker, 'ROOT', root), patch.object(checker, 'CORPUS', corpus), \
                    patch.object(checker, 'frozen_ids', return_value=['case']), \
                    patch.object(checker, 'method', return_value={'acceptance_variants': 1}), \
                    patch.object(checker.s08_baselines, 'requests_from_subset', return_value=(None, [{'id': 'case'}])):
                for smoke in (None, 1):
                    with self.subTest(smoke=smoke):
                        result = checker.prepare_requests(root, smoke)
                        raw = (root / 'rust-requests.json').read_bytes()
                        written = checker.strict_json_loads(raw)
                        # Use the real replay contract after serialization, not
                        # dictionary equality (which ignores map order).
                        checker.inventory(written, [frozen], partial=True)
                        actual = written[0]['loading']
                        self.assertEqual(list(actual['options']['paths']), ['@interface/*', '@blah', '@humbug/*'])
                        self.assertEqual(list(actual['config_raw']), ['files', 'compilerOptions'])
                        self.assertEqual(list(actual['config_raw']['compilerOptions']['paths']), ['z/*', 'a/*'])
                        self.assertEqual(raw, request_bytes([request]) + b'\n')
                        self.assertEqual(result['rust_requests_sha256'], digest(raw))


class CheckerCapture(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.plan = copy.deepcopy(checker.method())
        self.plan['acceptance_variants'] = 1
        self.method = self.root / 'method.json'
        self.method.write_bytes(canonical(self.plan))
        self.footprint = self.root / 'footprint.json'
        self.footprint.write_text('{}')
        for name in ('data/s08', 'data/s07'):
            (self.root / name).mkdir(parents=True)
        self.requests = [{'id': 'case'}]
        (self.root / 'data/s08/baseline-requests.json').write_bytes(canonical({'requests': self.requests}))
        (self.root / 'data/s07/subset.json').write_text('{}')
        for name in ('rust', 'go'):
            (self.root / f'{name}-requests.json').write_bytes(canonical(self.requests))
        self.binary = self.root / 'executable'
        self.binary.write_bytes(b'fixture executable identity')
        build = {'sources': {}, 'sources_sha256': digest(canonical({})), 'binaries': {
            name: {'path': str(self.binary), 'sha256': measurement.file_digest(self.binary)}
            for name in ('go', 'go-alloc', 'rust-normal', 'rust-phase', 'rust-alloc')}}
        (self.root / 'runtime-overlay').mkdir()
        sdk = self.root / 'runtime-overlay/sdk.json'
        sdk.write_text('{}')
        build['binaries']['go-alloc'].update(runtime_observer='requested-allocation-provenance-v1', sdk_sha256=measurement.file_digest(sdk))
        (self.root / 'build.json').write_bytes(canonical(build))
        self.capture = {
            'version': 2, 'pin': self.plan['pin'], 'host': {}, 'smoke': None,
            'samples_per_runtime': 7, 'sources_sha256': build['sources_sha256'],
            'build_sha256': measurement.file_digest(self.root / 'build.json'),
            'method_sha256': measurement.file_digest(self.method),
            'footprint_sha256': measurement.file_digest(self.footprint),
            'requests': {'variants': 1, 'smoke': None, 'ids_sha256': digest(canonical(['case'])),
                         **{name + '_requests_sha256': measurement.file_digest(self.root / f'{name}-requests.json')
                            for name in ('rust', 'go')}}, 'runs': []}
        for mode in checker.MODES:
            for label, warmup, pair in [('warmup', True, self.plan['sampling']['warmup_order']),
                                        *[(f'sample-{i}', False, pair) for i, pair in enumerate(self.plan['sampling']['measured_order'])]]:
                for runtime in pair:
                    run = {'runtime': runtime.lower(), 'mode': mode, 'label': label, 'warmup': warmup,
                           'process': {'returncode': 0, 'process_ns': 200, 'peak_rss_bytes': 1000}}
                    self.write_sample(run, [self.row()])
                    self.capture['runs'].append(run)
        for name, value in [('ROOT', self.root), ('METHOD', self.method), ('FOOTPRINT', self.footprint)]:
            context = patch.object(checker, name, value)
            context.start()
            self.addCleanup(context.stop)
        for name, value in [('method', self.plan), ('sources', {}), ('frozen_ids', ['case']),
                            ('inventory', None)]:
            context = patch.object(checker, name, return_value=value)
            context.start()
            self.addCleanup(context.stop)
        context = patch.object(checker.s08_baselines, 'requests_from_subset', return_value=(None, self.requests))
        context.start()
        self.addCleanup(context.stop)
        self.save()

    @staticmethod
    def row():
        return {'id': 'case', 'outcome': 'executed', 'output_sha256': 'a' * 64,
                'actions': {'GetTypeAtLocation': 1, 'diagnostics': 0}, 'interval_ns': 100,
                'phases_ns': {'init': 40, 'check': 40, 'display': 20},
                'allocation': {'requested_bytes': 40, 'live_before_interval': 10,
                               'live_at_checkpoint': 30, 'live_after_release': 10},
                'checkpoint': {'census': {'type_storage_bytes': 16, 'checker_bytes': 32,
                                         'types': {'reachable': 1, 'created': 2}, 'unavailable': []}}}

    def write_sample(self, run, rows):
        sample = self.root / 'samples' / run['mode']
        sample.mkdir(parents=True, exist_ok=True)
        prefix = f"{run['runtime']}-{run['label']}"
        row_path = sample / (prefix + '.rows.ndjson')
        row_path.write_bytes(b''.join(canonical(r) + b'\n' for r in rows))
        totals = measurement.checker_rows(rows, ['case'], run['mode'])
        totals.update(version=2, mode=run['mode'], request_sha256=self.capture['requests'][run['runtime'] + '_requests_sha256'])
        run['totals'] = totals
        run['rows_sha256'] = measurement.file_digest(row_path)
        (sample / (prefix + '.stdout')).write_bytes(canonical(totals) if run['runtime'] == 'rust' else b'PASS\n')
        (sample / (prefix + '.stdout.stderr')).write_bytes(b'')
        paths = [row_path, sample / (prefix + '.stdout'), sample / (prefix + '.stdout.stderr')]
        if run['runtime'] == 'go':
            path = sample / (prefix + '.summary.json')
            path.write_bytes(canonical(totals))
            paths.append(path)
        run['artifacts'] = {path.name: measurement.file_digest(path) for path in paths}

    def save(self):
        (self.root / 'capture.json').write_bytes(canonical(self.capture))

    def test_valid_capture_replays_and_ignores_cached_report(self):
        (self.root / 'report.json').write_text('{"metrics":{"elapsed_ratio":0.01}}')
        report = checker.current_report(self.root)
        self.assertEqual(report['metrics']['elapsed_ratio'], 1)
        self.assertEqual(report['metrics']['type_footprint_ratio'], 1)

    def test_raw_authentication_including_non_normal_and_warmup(self):
        for mode, label in [('phase', 'warmup'), ('alloc', 'sample-6')]:
            with self.subTest(mode=mode, label=label):
                path = self.root / 'samples' / mode / f'rust-{label}.rows.ndjson'
                raw = path.read_bytes()
                path.write_bytes(raw + b' ')
                with self.assertRaisesRegex(ValueError, 'artifact changed'):
                    checker.report(self.root)
                path.write_bytes(raw)

    def test_missing_duplicate_and_reordered_samples(self):
        original = copy.deepcopy(self.capture['runs'])
        for runs in [original[:-1], original + [original[-1]], list(reversed(original))]:
            self.capture['runs'] = runs
            self.save()
            with self.assertRaisesRegex(ValueError, 'samples'):
                checker.report(self.root)

    def test_forged_inventory_total_is_rejected(self):
        self.capture['requests']['variants'] = 9369
        self.save()
        with self.assertRaisesRegex(ValueError, 'count/identity'):
            checker.report(self.root)

    def test_rehashed_rows_do_not_bypass_output_parity(self):
        run = self.capture['runs'][-1]
        row = self.row()
        row['output_sha256'] = 'b' * 64
        self.write_sample(run, [row])
        self.save()
        with self.assertRaisesRegex(ValueError, 'output or action schedule'):
            checker.report(self.root)

    def test_rehashed_totals_are_reconstructed(self):
        run = self.capture['runs'][1]
        run['totals']['interval_ns'] += 1
        name = 'rust-warmup.stdout'
        path = self.root / 'samples/normal' / name
        path.write_bytes(canonical(run['totals']))
        run['artifacts'][name] = measurement.file_digest(path)
        self.save()
        with self.assertRaisesRegex(ValueError, 'interval_ns differs'):
            checker.report(self.root)

    def test_census_failure_or_unknown_family_withholds_footprint(self):
        run = next(r for r in self.capture['runs'] if r['mode'] == 'alloc' and not r['warmup'])
        for census in [{'state': 'failed', 'reason': 'unhandled type'},
                       {**self.row()['checkpoint']['census'], 'unavailable': ['semantic_roots']}]:
            row = self.row()
            row['checkpoint']['census'] = census
            self.write_sample(run, [row])
            self.save()
            report = checker.report(self.root)
            self.assertNotIn('type_footprint_ratio', report['metrics'])
            self.assertIn('type_footprint_ratio', report['unavailable'])

    def test_runtime_observer_sdk_identity_is_authenticated(self):
        (self.root / 'runtime-overlay/sdk.json').write_text('{"changed": true}')
        with self.assertRaisesRegex(ValueError, 'artifact changed'):
            checker.report(self.root)

    def test_missing_allocation_binary_is_rejected(self):
        path = self.root / 'build.json'
        build = measurement.strict_json_loads(path.read_bytes())
        del build['binaries']['go-alloc']
        path.write_bytes(canonical(build))
        self.capture['build_sha256'] = measurement.file_digest(path)
        self.save()
        with self.assertRaisesRegex(ValueError, 'executable inventory'):
            checker.report(self.root)

    def test_changed_executable_is_stale(self):
        self.binary.write_text('replacement executable')
        with self.assertRaisesRegex(ValueError, 'executable changed'):
            checker.report(self.root)

    def test_invalid_capture_cannot_use_cached_metrics(self):
        (self.root / 'report.json').write_text('{"metrics":{"elapsed_ratio":0.01}}')
        self.capture['runs'].pop()
        self.save()
        self.assertIsNone(checker.current_report(self.root))


class RelaterContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        _, cls.requests, cls.observations, _ = relater.frozen()

    def diagnostic_group(self):
        for row in self.observations['rows']:
            for group in row['groups']:
                if any(a['diagnostics'] for a in group['actions']):
                    found = copy.deepcopy(group)
                    found['state'] = 'executed'
                    found['after_lookup'] = copy.deepcopy(group['actions'][0]['before'])
                    return group, found, [a['action'] for a in group['actions']]
        self.fail('frozen diagnostic witness is missing')

    def test_equal_diagnostic_count_is_not_payload_parity(self):
        group, found, actions = self.diagnostic_group()
        target = next(a for a in found['actions'] if a['diagnostics'])
        target['diagnostics'][0]['text_hex'] = '77726f6e67'
        errors = relater.compare_group('reference', group, actions, found)
        self.assertTrue(any('diagnostic payload' in e for e in errors))
        self.assertFalse(relater.compare_group('reference', group, actions, found, behavior_only=True))

    def test_complete_behavior_does_not_certify_pre_resolved_reference(self):
        group, found, actions = self.diagnostic_group()
        self.assertFalse(relater.compare_group('id', group, actions, found))
        self.assertFalse(relater.compare_group('reference', group, actions, found, behavior_only=True))
        self.assertEqual(relater.compare_group('reference', group, actions, found),
                         ['reference setup pre-resolves the production graph; lazy protocol unavailable'])

    def test_duplicate_or_missing_checker_rows_rejected(self):
        for rows in [[], [CheckerCapture.row(), CheckerCapture.row()]]:
            with self.assertRaisesRegex(ValueError, 'inventory'):
                measurement.checker_rows(rows, ['case'], 'normal')

    def test_signed_retained_delta_preserved(self):
        row = CheckerCapture.row()
        row['allocation']['live_at_checkpoint'] = 5
        self.assertEqual(measurement.checker_rows([row], ['case'], 'alloc')['allocation']['retained_bytes'], -5)

    def test_matching_inventory_still_has_no_comparison_ratios(self):
        # Even an amended inventory on which all booleans agree cannot certify
        # equivalent lazy work. This checks reporting, independently of coverage.
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            totals = {'groups_executed': 85, 'groups_unsupported': 0, 'setup_ns': 100,
                      'relation_ns': 20, 'setup_requested_bytes': 40,
                      'relation_requested_bytes': 10, 'retained_bytes': 20}
            capture = {'pin': 'pin', 'smoke': False, 'sources_sha256': digest(canonical({})),
                       'runs': [{'mode': mode, 'implementation': impl, 'warmup': False, 'totals': totals}
                                for mode in relater.MODES for impl in relater.IMPLEMENTATIONS for _ in range(7)]}
            (root / 'capture.json').write_bytes(canonical(capture))
            parity = {'parity': 0, 'implementations': {
                'id': {'parity': 1}, 'reference': {'parity': 0, 'unsupported': 0, 'behavior_agreement': 1}}}
            with patch.object(relater, 'verify_capture', return_value=parity), patch.object(relater, 'sources', return_value={}):
                result = relater.report(root)
            self.assertEqual(result['metrics']['reference_behavior_agreement'], 1)
            self.assertEqual(result['metrics']['parity_reference'], 0)
            for key in ('elapsed_ratio', 'throughput_ratio', 'allocated_bytes_ratio', 'retained_bytes_ratio'):
                self.assertNotIn(key, result['metrics'])
            for mode in result['modes'].values():
                self.assertIn('unavailable', mode['relation_ns_ratio'])

    def test_relater_totals_are_reconstructed(self):
        group = {'mode': 'assignable', 'state': 'executed', 'setup_ns': 12, 'relation_ns': 3,
                 'allocation': {'setup_requested_bytes': 10, 'relation_requested_bytes': 2,
                                'live_before': 20, 'live_after': 18, 'live_after_release': 10}}
        observed = {'rows': [{'id': 'case', 'state': 'executed', 'groups': [group]}],
                    'totals': {'setup_ns': 12, 'relation_ns': 3, 'setup_requested_bytes': 10,
                               'relation_requested_bytes': 2, 'retained_bytes': -2,
                               'groups_executed': 1, 'groups_unsupported': 0}}
        requests = [{'id': 'case', 'actions': [{'mode': 'assignable'}]}]
        self.assertEqual(relater.observed_totals(observed, requests)['retained_bytes'], -2)
        observed['totals']['groups_executed'] = 105
        with self.assertRaisesRegex(ValueError, 'groups_executed differs'):
            relater.observed_totals(observed, requests)


if __name__ == '__main__':
    unittest.main()
