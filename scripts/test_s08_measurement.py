"""P7 capture/replay protocol tests; no builds, benchmark processes or corpus runs."""
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
    def setUp(self):
        self.loading = {
            'options': {'paths': {'@interface/*': ['src/interface/*'], '@blah': ['blah'], '@humbug/*': ['*/generated']}},
            'config_raw': {'files': ['main.ts'], 'compilerOptions': {'paths': {'z/*': ['z/*'], 'a/*': ['a/*']}}},
        }
        self.request = {
            'id': 'case', 'acceptance_tier': 'acceptance', 'loading': self.loading,
            'diagnostic_phases': ['config'], 'type_baseline_requested': False,
            'public_type_strings': True, 'error_baseline_requested': True,
        }
        self.frozen = {**self.request, 'loading_request_sha256': digest(request_bytes(self.loading) + b'\n')}
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        corpus = self.root / 'corpus'
        corpus.mkdir()
        (corpus / 'requests.json').write_bytes(request_bytes([self.request]) + b'\n')
        (self.root / 'data/s07').mkdir(parents=True)
        (self.root / 'data/s08').mkdir(parents=True)
        (self.root / 'data/s07/subset.json').write_text('{}')
        (self.root / 'data/s08/baseline-requests.json').write_bytes(canonical({'requests': [self.frozen]}))
        plan = copy.deepcopy(checker.method())
        plan['acceptance_variants'] = 1
        self.enterContext(patch.object(checker, 'ROOT', self.root))
        self.enterContext(patch.object(checker, 'CORPUS', corpus))
        self.enterContext(patch.object(checker, 'frozen_ids', return_value=['case']))
        self.enterContext(patch.object(checker, 'method', return_value=plan))
        self.enterContext(patch.object(checker.s08_baselines, 'requests_from_subset',
                                       return_value=(None, [{'id': 'case'}])))

    def test_written_requests_preserve_semantic_map_order(self):
        for smoke in (None, 1):
            with self.subTest(smoke=smoke):
                result = checker.prepare_requests(self.root, smoke)
                raw = (self.root / 'rust-requests.json').read_bytes()
                written = checker.strict_json_loads(raw)
                # Use the real replay contract after serialization, not
                # dictionary equality (which ignores map order).
                checker.inventory(written, [self.frozen], partial=True)
                actual = written[0]['loading']
                self.assertEqual(list(actual['options']['paths']), ['@interface/*', '@blah', '@humbug/*'])
                self.assertEqual(list(actual['config_raw']), ['files', 'compilerOptions'])
                self.assertEqual(list(actual['config_raw']['compilerOptions']['paths']), ['z/*', 'a/*'])
                self.assertEqual(raw, request_bytes([self.request]) + b'\n')
                self.assertEqual(result['rust_requests_sha256'], digest(raw))

    def test_reordered_written_requests_fail_before_build_or_sample(self):
        # Reintroduce the actual writer bug; the in-memory inventory is valid.
        # Capture must re-read the serialized inputs through the real verifier.
        with patch.object(checker, 'request_canonical', canonical), \
                patch.object(checker, 'build', side_effect=AssertionError('build reached before request validation')) as build, \
                patch.object(checker, 'sample') as sample:
            with self.assertRaisesRegex(ValueError, 'E2 loading input changed'):
                checker.capture(self.root)
            build.assert_not_called()
            sample.assert_not_called()

    def test_bad_sample_count_fails_before_build_or_sample(self):
        for count in (0, -1, 8, True):
            with self.subTest(count=count), \
                    patch.object(checker, 'build', side_effect=AssertionError('build reached before sample count validation')) as build, \
                    patch.object(checker, 'sample') as sample:
                with self.assertRaisesRegex(ValueError, 'sample count'):
                    checker.capture(self.root, count, smoke=1)
                build.assert_not_called()
                sample.assert_not_called()

    def test_changed_native_requests_fail_before_build_or_sample(self):
        def changed_native(value):
            if value == [{'id': 'case'}]:
                value = [{'id': 'case', 'settings': {'module': 'amd'}}]
            return request_bytes(value)

        with patch.object(checker, 'request_canonical', side_effect=changed_native), \
                patch.object(checker, 'build', side_effect=AssertionError('build reached before request validation')) as build, \
                patch.object(checker, 'sample') as sample:
            with self.assertRaisesRegex(ValueError, 'native measurement requests changed'):
                checker.capture(self.root)
            build.assert_not_called()
            sample.assert_not_called()


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
        observer_sources = self.root / 'tools/s08/oracle/families'
        observer_sources.mkdir(parents=True)
        observer_hashes = {}
        for name in checker.OBSERVER_SOURCES:
            raw = ('fixture ' + name).encode()
            (observer_sources / name).write_bytes(raw)
            (self.root / 'runtime-overlay' / name).write_bytes(raw)
            observer_hashes[name] = digest(raw)
        build['binaries']['go-alloc'].update(runtime_observer='requested-allocation-provenance-v1',
                                            sdk_sha256=measurement.file_digest(sdk),
                                            observer_sources_sha256=observer_hashes)
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
                'checkpoint': {'census': CheckerCapture.census()}}

    @staticmethod
    def census():
        families = {name: {'count': 0, 'bytes': 0} for name in measurement.TYPE_FAMILIES}
        families['type_records'] = {'count': 2, 'bytes': 16}
        families['symbols'] = {'count': 1, 'bytes': 16}
        return {'type_storage_bytes': 16, 'checker_bytes': 32, 'families': families,
                'types': {'reachable': 1, 'created': 2, 'unreachable_occupied': 1}, 'unavailable': []}

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

    def run_capture(self):
        # Replace expensive execution, not validation. Each sample supplies real
        # authenticated files, just as sample() does after a child exits.
        (self.root / 'capture.json').unlink(missing_ok=True)
        build = checker.strict_json_loads((self.root / 'build.json').read_bytes())
        self.sample_calls = []

        def sample(directory, build_report, runtime, mode, label):
            self.sample_calls.append((runtime, mode, label))
            return next(copy.deepcopy(run) for run in self.capture['runs']
                        if (run['runtime'], run['mode'], run['label']) == (runtime, mode, label))

        with patch.object(checker, 'prepare_requests', return_value=self.capture['requests']), \
                patch.object(checker, 'build', return_value=build), \
                patch.object(checker, 'host_info', return_value={}), \
                patch.object(checker, 'sample', side_effect=sample):
            return checker.capture(self.root)

    def test_capture_preserves_the_full_sample_order_and_replays(self):
        result = self.run_capture()
        expected = [(run['runtime'], run['mode'], run['label']) for run in self.capture['runs']]
        self.assertEqual(self.sample_calls, expected)
        self.assertEqual(len(result['runs']), 48)
        self.assertFalse((self.root / 'capture-failure.json').exists())
        checker.verify_capture(self.root, result)

    def test_warmup_output_mismatch_stops_before_measured_samples(self):
        run = self.capture['runs'][1]
        row = self.row()
        row['output_sha256'] = 'b' * 64
        self.write_sample(run, [row])
        with self.assertRaisesRegex(ValueError, 'output or action schedule.*case'):
            self.run_capture()
        self.assertEqual(self.sample_calls, [('go', 'normal', 'warmup'), ('rust', 'normal', 'warmup')])
        failure = checker.strict_json_loads((self.root / 'capture-failure.json').read_bytes())
        self.assertEqual(failure['failure']['label'], 'warmup')
        self.assertEqual(len(failure['runs']), 2)
        self.assertTrue((self.root / 'samples/normal/rust-warmup.rows.ndjson').exists())
        self.assertFalse((self.root / 'capture.json').exists())
        with patch.object(checker, 'build') as build:
            with self.assertRaisesRegex(ValueError, 'capture already exists'):
                checker.capture(self.root)
            build.assert_not_called()

    def test_warmup_action_mismatch_stops_before_measured_samples(self):
        run = self.capture['runs'][1]
        row = self.row()
        row['actions']['GetTypeAtLocation'] += 1
        self.write_sample(run, [row])
        with self.assertRaisesRegex(ValueError, 'output or action schedule'):
            self.run_capture()
        self.assertEqual(len(self.sample_calls), 2)

    def test_each_mode_checks_its_first_warmup_against_the_same_work(self):
        run = next(r for r in self.capture['runs'] if r['mode'] == 'phase' and r['warmup'])
        row = self.row()
        row['output_sha256'] = 'b' * 64
        self.write_sample(run, [row])
        with self.assertRaisesRegex(ValueError, 'output or action schedule'):
            self.run_capture()
        self.assertEqual(len(self.sample_calls), 17)
        self.assertEqual(self.sample_calls[-1], ('go', 'phase', 'warmup'))

    def test_measured_sample_is_checked_before_the_next_child(self):
        run = self.capture['runs'][2]
        row = self.row()
        row['output_sha256'] = 'b' * 64
        self.write_sample(run, [row])
        with self.assertRaisesRegex(ValueError, 'output or action schedule'):
            self.run_capture()
        self.assertEqual(len(self.sample_calls), 3)
        self.assertEqual(self.sample_calls[-1], ('go', 'normal', 'sample-0'))

    def test_first_sample_totals_are_verified_before_the_next_child(self):
        run = self.capture['runs'][0]
        run['totals']['interval_ns'] += 1
        path = self.root / 'samples/normal/go-warmup.summary.json'
        path.write_bytes(canonical(run['totals']))
        run['artifacts'][path.name] = measurement.file_digest(path)
        with self.assertRaisesRegex(ValueError, 'interval_ns differs'):
            self.run_capture()
        self.assertEqual(self.sample_calls, [('go', 'normal', 'warmup')])

    def test_child_request_identity_is_verified_before_the_next_child(self):
        run = self.capture['runs'][0]
        run['totals']['request_sha256'] = 'b' * 64
        path = self.root / 'samples/normal/go-warmup.summary.json'
        path.write_bytes(canonical(run['totals']))
        run['artifacts'][path.name] = measurement.file_digest(path)
        with self.assertRaisesRegex(ValueError, 'child loaded different requests'):
            self.run_capture()
        self.assertEqual(self.sample_calls, [('go', 'normal', 'warmup')])

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

    def test_zero_rust_requested_bytes_is_unavailable_not_one(self):
        for run in self.capture['runs']:
            if run['mode'] == 'alloc' and run['runtime'] == 'rust':
                row = self.row()
                row['allocation']['requested_bytes'] = 0
                self.write_sample(run, [row])
        self.save()
        report = checker.report(self.root)
        self.assertNotIn('allocated_bytes_ratio', report['metrics'])
        self.assertIn('instrumentation inactive', report['unavailable']['allocated_bytes_ratio'])

    def test_negative_retained_delta_is_reported_signed(self):
        for run in self.capture['runs']:
            if run['mode'] == 'alloc' and run['runtime'] == 'rust':
                row = self.row()
                row['allocation']['live_at_checkpoint'] = 5
                self.write_sample(run, [row])
        self.save()
        report = checker.report(self.root)
        summary = report['modes']['alloc']['retained_bytes_ratio']
        self.assertTrue(summary['signed'])
        self.assertEqual(summary['rust_median'], -5)
        self.assertNotIn('retained_bytes_ratio', report['unavailable'])

    def test_busy_host_is_flagged_and_disclosed(self):
        self.assertFalse(checker.report(self.root)['host_busy'])
        run = next(r for r in self.capture['runs'] if r['mode'] == 'normal' and not r['warmup'])
        run['load_average'] = [checker.HOST_BUSY_LOAD + 1, 1.0, 1.0]
        self.save()
        report = checker.report(self.root)
        self.assertTrue(report['host_busy'])
        self.assertTrue(report['modes']['normal']['host_load']['busy'])
        self.assertIn('normal', report['modes'])

    def test_runtime_observer_sdk_identity_is_authenticated(self):
        (self.root / 'runtime-overlay/sdk.json').write_text('{"changed": true}')
        with self.assertRaisesRegex(ValueError, 'artifact changed'):
            checker.report(self.root)

    def test_changed_footprint_policy_requires_fresh_capture(self):
        self.footprint.write_bytes(canonical({'threshold': {'maximum': 0.85}}))
        with self.assertRaisesRegex(ValueError, 'footprint contract differs'):
            checker.report(self.root)

    def test_runtime_observer_sources_are_authenticated_independently_of_globs(self):
        # sources() is fixed at {} by this fixture: neither edited copy may
        # escape detection even if the broad fingerprint misses these files.
        for directory in ('runtime-overlay', 'tools/s08/oracle/families'):
            for name in checker.OBSERVER_SOURCES:
                with self.subTest(directory=directory, name=name):
                    path = self.root / directory / name
                    original = path.read_bytes()
                    path.write_bytes(original + b'changed')
                    try:
                        with self.assertRaisesRegex(ValueError, 'artifact changed'):
                            checker.report(self.root)
                    finally:
                        path.write_bytes(original)

    def test_incomplete_observer_source_inventory_is_rejected(self):
        path = self.root / 'build.json'
        original = measurement.strict_json_loads(path.read_bytes())
        for hashes in (None, {}, {'runtime_allocations.go': 'a' * 64}):
            with self.subTest(hashes=hashes):
                build = copy.deepcopy(original)
                build['binaries']['go-alloc']['observer_sources_sha256'] = hashes
                path.write_bytes(canonical(build))
                self.capture['build_sha256'] = measurement.file_digest(path)
                self.save()
                with self.assertRaisesRegex(ValueError, 'observer source inventory'):
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


class CensusInvariants(unittest.TestCase):
    def rows(self, **census):
        row = CheckerCapture.row()
        row['checkpoint']['census'].update(census)
        return [row]

    def test_consistent_row_passes(self):
        measurement.checker_rows(self.rows(), ['case'], 'alloc')

    def test_family_sum_and_triple_are_enforced(self):
        with self.assertRaisesRegex(ValueError, 'type-family sum'):
            measurement.checker_rows(self.rows(type_storage_bytes=17), ['case'], 'alloc')
        with self.assertRaisesRegex(ValueError, 'family sum'):
            measurement.checker_rows(self.rows(checker_bytes=33), ['case'], 'alloc')
        with self.assertRaisesRegex(ValueError, 'triple'):
            measurement.checker_rows(self.rows(types={'reachable': 3, 'created': 2, 'unreachable_occupied': 0}), ['case'], 'alloc')
        with self.assertRaisesRegex(ValueError, 'triple'):
            measurement.checker_rows(self.rows(types={'reachable': 1, 'created': 2, 'unreachable_occupied': 0}), ['case'], 'alloc')

    def test_incomplete_or_negative_inventory_is_rejected(self):
        families = dict(CheckerCapture.census()['families'])
        del families['alias']
        with self.assertRaisesRegex(ValueError, 'type family alias missing'):
            measurement.checker_rows(self.rows(families=families), ['case'], 'alloc')
        families = dict(CheckerCapture.census()['families'])
        families['symbols'] = {'count': 1, 'bytes': -16}
        with self.assertRaises(ValueError):
            measurement.checker_rows(self.rows(families=families), ['case'], 'alloc')

    def test_inventory_must_match_across_rows(self):
        first, second = CheckerCapture.row(), CheckerCapture.row()
        second['id'] = 'other'
        second['checkpoint']['census']['families']['extra'] = {'count': 0, 'bytes': 0}
        with self.assertRaisesRegex(ValueError, 'inventory differs'):
            measurement.checker_rows([first, second], ['case', 'other'], 'alloc')


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
        self.assertTrue(any('diagnostics' in e for e in errors))
        self.assertFalse(relater.compare_group('reference', group, actions, found, behavior_only=True))

    def test_complete_behavior_does_not_certify_pre_resolved_reference(self):
        group, found, actions = self.diagnostic_group()
        self.assertFalse(relater.compare_group('id', group, actions, found))
        self.assertFalse(relater.compare_group('reference', group, actions, found, behavior_only=True))
        self.assertEqual(relater.compare_group('reference', group, actions, found),
                         ['reference setup pre-resolves the production graph; lazy protocol unavailable'])

    def test_bound_program_still_requires_all_semantic_states(self):
        group, found, actions = self.diagnostic_group()
        found['source_mode'] = 'bound_program'
        self.assertFalse(relater.compare_group('reference', group, actions, found))
        for point in ('before', 'after'):
            for counter in ('types_created', 'signatures_created', 'instantiations'):
                with self.subTest(point=point, counter=counter):
                    changed = copy.deepcopy(found)
                    changed['actions'][0][point][counter] += 1
                    errors = relater.compare_group('reference', group, actions, changed)
                    self.assertIn(f'action 0: {point}', errors)

    def test_bound_program_lookup_and_first_action_must_agree(self):
        group, found, actions = self.diagnostic_group()
        found['source_mode'] = 'bound_program'
        found['after_lookup']['types_created'] += 1
        self.assertIn('after_lookup', relater.compare_group('reference', group, actions, found))

    def test_creation_transitions_compare_actual_counts_not_nonzero(self):
        group, found, _ = self.diagnostic_group()
        found['actions'][0]['after']['types_created'] += 1
        self.assertFalse(relater.transitions(group, found)[0]['agree'])

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
            parity = {'parity': 0, 'both_match_every_case': False, 'implementations': {
                'id': {'parity': 1}, 'reference': {'parity': 0, 'unsupported': 0, 'behavior_agreement': 1}}}
            with patch.object(relater, 'verify_capture', return_value=parity), patch.object(relater, 'sources', return_value={}):
                result = relater.report(root)
            self.assertEqual(result['metrics']['reference_behavior_agreement'], 1)
            self.assertEqual(result['metrics']['parity_reference'], 0)
            for key in ('elapsed_ratio', 'throughput_ratio', 'allocated_bytes_ratio', 'retained_bytes_ratio'):
                self.assertNotIn(key, result['metrics'])
            for mode in result['modes'].values():
                self.assertIn('unavailable', mode['relation_ns_ratio'])

    def test_failed_relater_parity_stops_before_measurement_children(self):
        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(relater, 'build', return_value={}), \
                 patch.object(relater, 'parity', return_value={'both_match_every_case': False}), \
                 patch.object(relater, 'run_child') as child:
                with self.assertRaisesRegex(ValueError, 'relater parity failed'):
                    relater.capture(Path(temporary))
                child.assert_not_called()

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
