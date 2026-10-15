"""Operation-runner evidence checks with fake children; no compiler runs."""
import copy
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest
from unittest import mock

from tools.phase7.bench import operations


def report(backend='routed', mode='observe', experiment='argument'):
    result = {'format': 1, 'experiment': experiment, 'backend': backend, 'mode': mode, 'complete': True,
              'workload': {'files': [{'path': '/fixture/a.ts', 'sha256': 'a' * 64, 'options_sha256': 'e' * 64}],
                           'options_sha256': 'e' * 64,
                           'configurations': [{'path': '/fixture/tsconfig.json', 'sha256': 'f' * 64}],
                           'diagnostics_sha256': 'b' * 64, 'diagnostics_count': 2,
                           'types': 100, 'signatures': 25, 'instantiations': 10}}
    if experiment == 'argument':
        result['operations'] = {'calls': 12, 'outermost_calls': 4, 'errors': 2,
                                'location_errors': 0, 'cleanup_errors': 0, 'elapsed_ns': 100 if backend == 'routed' else 80}
        if mode == 'observe':
            result['operations'].update(trace_checksum=123, kinds={'-1': 12},
                                        modes={'0': 4}, depths={'0': 4}, inferential_calls=2,
                                        samples=[['a.ts', 12], ['a.ts', 27]])
    else:
        result['operations'] = {'requests': 8, 'total_seen': 10, 'truncated': False,
                                'request_sha256': 'c' * 64, 'result_sha256': 'd' * 64, 'errors': 0,
                                'local_requests': 8 if backend == 'scoped' else 0,
                                'routed_requests': 8 if backend == 'routed' else 0,
                                'node_reads': 90, 'list_reads': 10, 'semantic_calls': 5, 'elapsed_ns': 100}
    return result


class ValidationTests(unittest.TestCase):
    def test_observation_and_timing_compare_only_agreed_fields(self):
        observed = report()
        timed = report('scoped', 'time')
        self.assertEqual(operations.comparable(observed), operations.comparable(timed))
        self.assertNotIn('trace_checksum', operations.comparable(timed)['operations'])
        for backend in operations.BACKENDS:
            flow = report(backend, 'time', 'flow')
            operations.validate(flow, 'flow', backend, 'time')
        self.assertEqual(operations.comparable(report('routed', 'time', 'flow')),
                         operations.comparable(report('scoped', 'time', 'flow')))

    def test_matching_invalid_reports_cannot_validate_work(self):
        changes = {
            'wrong-mode': lambda r: r.update(mode='time'),
            'incomplete': lambda r: r.update(complete=False),
            'bad-format': lambda r: r.update(format=True),
            'no-files': lambda r: r['workload'].update(files=[]),
            'duplicate-file': lambda r: r['workload']['files'].append(r['workload']['files'][0]),
            'bad-hash': lambda r: r['workload'].update(diagnostics_sha256='bad'),
            'bad-options': lambda r: r['workload'].update(options_sha256='bad'),
            'missing-file-options': lambda r: r['workload']['files'][0].pop('options_sha256'),
            'bad-file-options': lambda r: r['workload']['files'][0].update(options_sha256=1),
            'no-configurations': lambda r: r['workload'].update(configurations=[]),
            'bad-configuration': lambda r: r['workload']['configurations'][0].update(sha256='bad'),
            'missing-config-path': lambda r: r['workload']['configurations'][0].pop('path'),
            'duplicate-config': lambda r: r['workload']['configurations'].append(r['workload']['configurations'][0]),
            'no-types': lambda r: r['workload'].update(types=0),
            'bool-count': lambda r: r['operations'].update(calls=True),
            'zero-coverage': lambda r: r['operations'].update(calls=0),
            'bad-location': lambda r: r['operations'].update(location_errors=1),
            'bad-cleanup': lambda r: r['operations'].update(cleanup_errors=1),
            'no-trace': lambda r: r['operations'].pop('trace_checksum'),
            'empty-samples': lambda r: r['operations'].update(samples=[]),
            'empty-kinds': lambda r: r['operations'].update(kinds={}),
            'bad-kind-count': lambda r: r['operations'].update(kinds={'-1': 11}),
        }
        for name, change in changes.items():
            with self.subTest(name=name):
                item = report()
                change(item)
                with self.assertRaises(ValueError):
                    operations.validate(item, 'argument', 'routed', 'observe')
        for elapsed in (0, -1, True, 1.5):
            item = report(mode='time')
            item['operations']['elapsed_ns'] = elapsed
            with self.subTest(elapsed=elapsed), self.assertRaisesRegex(ValueError, 'elapsed_ns'):
                operations.validate(item, 'argument', 'routed', 'time')

    def test_scoped_flow_must_use_local_backend_and_errors_fail(self):
        for changes in ({'local_requests': 0, 'routed_requests': 8}, {'errors': 1},
                        {'local_requests': 7}, {'node_reads': 0}, {'result_sha256': 'bad'},
                        {'truncated': 0}, {'total_seen': 7}):
            item = report('scoped', 'time', 'flow')
            item['operations'].update(changes)
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                operations.validate(item, 'flow', 'scoped', 'time')

    def test_nested_flow_entries_and_additional_timed_fields_are_compared(self):
        routed, scoped = report('routed', 'time', 'flow'), report('scoped', 'time', 'flow')
        routed['operations'].update(routed_requests=12, types_after_check=100)
        scoped['operations'].update(local_requests=9, routed_requests=3, types_after_check=100)
        for item in (routed, scoped):
            operations.validate(item, 'flow', item['backend'], 'time')
        self.assertEqual(operations.comparable(routed), operations.comparable(scoped))
        scoped['operations']['local_requests'] = 10
        self.assertNotEqual(operations.comparable(routed), operations.comparable(scoped))
        scoped['operations']['local_requests'] = 9
        scoped['operations']['types_after_check'] = 101
        self.assertNotEqual(operations.comparable(routed), operations.comparable(scoped))

    def test_missing_required_fields_raise_validation_errors(self):
        for experiment in ('argument', 'flow'):
            for mode in ('observe', 'time'):
                original = report('routed', mode, experiment)
                for section in ('workload', 'operations'):
                    for key in original[section]:
                        item = copy.deepcopy(original)
                        item[section].pop(key)
                        with self.subTest(experiment=experiment, mode=mode, section=section, key=key):
                            with self.assertRaises(ValueError):
                                operations.validate(item, experiment, 'routed', mode)


class CollectionTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory())).resolve()
        self.finished = set()
        self.results = {backend: report(backend, 'time') for backend in operations.BACKENDS}
        self.exit_codes = {}
        self.lock = threading.Lock()

    def execute(self, command, cwd, timeout):
        self.assertEqual(command[:3], ['/saved/binary', '/fixture/tsconfig.json', 'argument'])
        self.assertEqual(cwd, Path('/fixture'))
        self.assertEqual(timeout, 17)
        backend = command[3]
        with self.lock:
            self.finished.add(backend)
        return {'exit': self.exit_codes.get(backend, 0), 'stdout': json.dumps(self.results[backend]),
                'stderr': f'{backend} log', 'launch_ns': 100 if backend == 'routed' else 101}

    def collect(self, index=0, mode='time'):
        with mock.patch.object(operations, 'execute', side_effect=self.execute):
            return operations.collect(Path('/saved/binary'), Path('/fixture/tsconfig.json'), 'argument',
                                      mode, self.root, index, 17, report() if mode == 'time' else None)

    def test_waits_for_both_children_before_validation_and_alternates_order(self):
        validate = operations.validate

        def after_both(*args):
            self.assertEqual(self.finished, set(operations.BACKENDS))
            for backend in operations.BACKENDS:
                self.assertTrue((self.root / f'time-1-{backend}.stdout').is_file())
            return validate(*args)

        with mock.patch.object(operations, 'validate', side_effect=after_both):
            result = self.collect(1)
        self.assertEqual(result['submission_order'], ['scoped', 'routed'])
        self.assertEqual(result['ratio'], 0.8)
        self.assertEqual((self.root / 'time-1-scoped.stderr').read_text(), 'scoped log')

    def test_observe_rejects_reordered_trace_with_both_reports_preserved(self):
        self.results = {backend: report(backend) for backend in operations.BACKENDS}
        self.results['scoped']['operations']['samples'].reverse()
        with self.assertRaisesRegex(ValueError, 'ordered operation'):
            self.collect(mode='observe')
        for backend in operations.BACKENDS:
            self.assertIn('report', json.loads((self.root / f'observe-0-{backend}.json').read_text()))

    def test_additional_observed_fields_are_compared(self):
        self.results = {backend: report(backend) for backend in operations.BACKENDS}
        self.results['routed']['operations']['before_after'] = [1, 2]
        self.results['scoped']['operations']['before_after'] = [1, 3]
        with self.assertRaisesRegex(ValueError, 'ordered operation'):
            self.collect(mode='observe')

    def test_every_pair_rejects_work_count_output_and_child_failures(self):
        changes = {
            'workload': lambda r: r['workload'].update(diagnostics_count=3),
            'count': lambda r: r['operations'].update(calls=13),
            'exit': lambda r: self.exit_codes.update(scoped=2),
            'error-json': lambda r: r.update(complete=False, error='failed'),
        }
        for name, change in changes.items():
            with self.subTest(name=name):
                self.results = {backend: report(backend, 'time') for backend in operations.BACKENDS}
                self.exit_codes = {}
                change(self.results['scoped'])
                with self.assertRaises(ValueError):
                    self.collect()
                for backend in operations.BACKENDS:
                    self.assertTrue((self.root / f'time-0-{backend}.stdout').is_file())

    def test_timed_pair_cannot_change_options_even_when_backends_agree(self):
        changes = {
            'options': lambda r: r['workload'].update(options_sha256='1' * 64),
            'file-options': lambda r: r['workload']['files'][0].update(options_sha256='2' * 64),
            'configuration': lambda r: r['workload']['configurations'][0].update(sha256='3' * 64),
        }
        for name, change in changes.items():
            with self.subTest(name=name):
                self.results = {backend: report(backend, 'time') for backend in operations.BACKENDS}
                for result in self.results.values():
                    change(result)
                with self.assertRaisesRegex(ValueError, 'differ from observation'):
                    self.collect()

    def test_malformed_stdout_and_spawn_failure_keep_both_logs(self):
        raw = {'routed': {'exit': 0, 'stdout': 'not json', 'stderr': 'parse log'},
               'scoped': {'exit': None, 'stdout': '', 'stderr': '', 'error': 'cannot spawn'}}
        with mock.patch.object(operations, 'execute', side_effect=lambda cmd, *_: raw[cmd[3]]):
            with self.assertRaisesRegex(ValueError, 'cannot spawn'):
                operations.collect(Path('/saved/binary'), Path('/fixture/tsconfig.json'), 'argument',
                                   'time', self.root, 0, 17, report())
        self.assertEqual((self.root / 'time-0-routed.stdout').read_text(), 'not json')
        self.assertIn('cannot spawn', (self.root / 'time-0-scoped.json').read_text())


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory())).resolve()
        self.binary = self.root / 'binary'
        self.binary.write_text('saved executable')
        self.project = self.root / 'tsconfig.json'
        self.project.write_text('{}')
        self.enterContext(mock.patch.object(operations, 'source_identity', return_value={'revision': 'e' * 40}))

    def test_failed_observation_preserves_incomplete_summary_and_stops_timing(self):
        output = self.root / 'failed'
        with mock.patch.object(operations, 'collect', side_effect=ValueError('incompatible')) as collect:
            with self.assertRaisesRegex(ValueError, 'incompatible'):
                operations.run(self.binary, self.project, 'argument', output)
        self.assertEqual(collect.call_count, 1)
        summary = json.loads((output / 'summary.json').read_text())
        self.assertFalse(summary['complete'])
        self.assertEqual(summary['error'], 'incompatible')
        self.assertEqual(summary['pairs'], [])
        self.assertIn('sha256', summary['binary'])

    def test_pair_history_manifest_metadata_and_existing_output_refusal(self):
        output = self.root / 'run'
        manifest = self.root / 'build.json'
        manifest.write_text('{"flags":"caller supplied"}')
        results = [report(), {'ratio': 0.8, 'elapsed_ns': {'routed': 100, 'scoped': 80}},
                   {'ratio': 1.0, 'elapsed_ns': {'routed': 120, 'scoped': 120}}]
        with mock.patch.object(operations, 'collect', side_effect=copy.deepcopy(results)) as collect:
            result = operations.run(self.binary, self.project, 'argument', output, pairs=2, build_manifest=manifest)
        self.assertEqual(collect.call_count, 3)
        self.assertTrue(result['complete'])
        self.assertEqual(result['median_ratio'], 0.9)
        self.assertEqual(result['build_manifest']['metadata']['flags'], 'caller supplied')
        original = (output / 'summary.json').read_bytes()
        with self.assertRaises(FileExistsError):
            operations.run(self.binary, self.project, 'argument', output)
        self.assertEqual((output / 'summary.json').read_bytes(), original)
        with self.assertRaisesRegex(ValueError, 'outside the checkout'):
            operations.run(self.binary, self.project, 'argument', operations.ROOT / 'target/new-benchmark')

    def test_child_timeout_kills_reaps_and_keeps_partial_output(self):
        child = mock.Mock(pid=123, returncode=-9)
        child.communicate.side_effect = [subprocess.TimeoutExpired(['binary'], 17), ('partial json', 'timeout log')]
        with mock.patch.object(operations.subprocess, 'Popen', return_value=child) as spawn, \
                mock.patch.object(operations.os, 'killpg') as kill:
            result = operations.execute(['binary'], self.root, 17)
        kill.assert_called_once_with(123, operations.signal.SIGKILL)
        self.assertTrue(spawn.call_args.kwargs['start_new_session'])
        self.assertTrue(result['timed_out'])
        self.assertEqual(result['stdout'], 'partial json')
        self.assertEqual(result['stderr'], 'timeout log')
        self.assertEqual(child.communicate.call_count, 2)


if __name__ == '__main__':
    unittest.main()
