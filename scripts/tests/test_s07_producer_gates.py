"""Named S07 contract gates must observe their complete measured obligations."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s07_producers import GRAPH_CONTRACT_TESTS, binder_contract_metrics, program
import s07_program_helpers as helpers
import s06_utilities as utilities


class ProgramPreflightTests(unittest.TestCase):
    def test_committed_source_check_inventory_matches_the_runner(self):
        manifest = json.loads((helpers.ROOT / 'data/s07/program-helper-tests.json').read_bytes())
        self.assertEqual(manifest['checks'], helpers.CHECKS)

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'data/s07').mkdir(parents=True)
        (self.root / 'data/upstream.json').write_text(json.dumps({'pin': 'test-pin'}))
        self.manifest = {'schema': 1, 'upstream_pin': 'test-pin', 'checks': helpers.CHECKS,
                         'groups': [{'package': 'tsr_compiler', 'target': 'helpers',
                                     'prefix': 'named::', 'tests': ['named::first', 'named::second']}]}
        self.binary = str(self.root / 'helper-tests')
        self.build = json.dumps({'reason': 'compiler-artifact', 'profile': {'test': True},
                                 'executable': self.binary,
                                 'manifest_path': str(self.root / 'crates/tsr_compiler/Cargo.toml'),
                                 'target': {'kind': ['test'], 'name': 'helpers'}}).encode()
        self.valid_inventory = b'named::first: test\nnamed::second: test\n'

    def write_manifest(self):
        raw = json.dumps(self.manifest).encode()
        (self.root / 'data/s07/program-helper-tests.json').write_bytes(raw)
        return raw

    def test_valid_manifest_and_binary_inventory_pass_real_preflight(self):
        raw = self.write_manifest()
        with patch.object(helpers, 'ROOT', self.root), \
                patch.object(helpers, 'setup', side_effect=[self.build, self.valid_inventory]) as setup:
            prepared = helpers.preflight(self.root / 'capture')
        self.assertEqual(prepared, (raw, self.manifest, {('tsr_compiler', 'helpers'): self.binary}))
        self.assertEqual(setup.call_count, 2)
        self.assertEqual(setup.call_args_list[1].args[0], [self.binary, '--list', '--format=terse'])

    def test_builds_only_manifest_targets_with_separate_build_budget(self):
        self.manifest['groups'] += [
            {'package': 'tsr_compiler', 'target': 'lib', 'prefix': 'compiler::', 'tests': ['compiler::check']},
            {'package': 'tsr_module', 'target': 'lib', 'prefix': 'module::', 'tests': ['module::check']},
            {'package': 'tsr_module', 'target': 'lib', 'prefix': 'trace::', 'tests': ['trace::check']},
        ]
        self.write_manifest()
        def run(args, **kwargs):
            if args[0] == 'cargo':
                package = args[args.index('--package') + 1]
                targets = sorted({g['target'] for g in self.manifest['groups'] if g['package'] == package})
                rows = [{'reason': 'compiler-artifact', 'profile': {'test': True},
                         'executable': f'/{package}-{target}',
                         'manifest_path': str(self.root / 'crates' / package / 'Cargo.toml'),
                         'target': {'kind': ['lib'] if target == 'lib' else ['test'],
                                    'name': package if target == 'lib' else target}}
                        for target in targets]
                output = b'\n'.join(json.dumps(row).encode() for row in rows)
            else:
                output = ''.join(name + ': test\n' for g in self.manifest['groups']
                                 if args[0] == f"/{g['package']}-{g['target']}" for name in g['tests']).encode()
            return subprocess.CompletedProcess(args, 0, output, b'')
        # Exercise setup/invoke as well as preflight so the budget must actually
        # reach subprocess.run, rather than merely appear at a mocked setup call.
        with patch.object(helpers, 'ROOT', self.root), \
                patch.object(utilities.subprocess, 'run', side_effect=run) as invoked:
            _, _, binaries = helpers.preflight(self.root / 'capture')
        base = ['cargo', 'test', '--locked', '--release', '--no-run', '--message-format=json', '--package']
        self.assertEqual([call.args[0] for call in invoked.call_args_list[:2]], [
            base + ['tsr_compiler', '--test', 'helpers', '--lib'],
            base + ['tsr_module', '--lib'],
        ])
        self.assertEqual([call.kwargs['timeout'] for call in invoked.call_args_list],
                         [1800, 1800, 300, 300, 300])
        self.assertEqual(set(binaries), {('tsr_compiler', 'helpers'), ('tsr_compiler', 'lib'), ('tsr_module', 'lib')})

    def test_build_timeout_stops_before_subset_capture(self):
        self.write_manifest()
        with patch('s07_program_compare.input_fingerprints', return_value={}), \
                patch('s07_producers.ROOT', self.root), patch.object(helpers, 'ROOT', self.root), \
                patch.object(utilities.subprocess, 'run', side_effect=subprocess.TimeoutExpired(['cargo'], 1800)), \
                patch('s07_producers.prepare_subset') as subset:
            with self.assertRaises(subprocess.TimeoutExpired):
                program()
        subset.assert_not_called()

    def test_inventory_drift_stops_before_subset_or_source_capture(self):
        self.write_manifest()
        for inventory in (b'named::first: test\n',
                          self.valid_inventory + b'named::unexpected: test\n',
                          self.valid_inventory + b'named::first: test\n'):
            with self.subTest(inventory=inventory), \
                    patch('s07_program_compare.input_fingerprints', return_value={}), \
                    patch('s07_producers.ROOT', self.root), \
                    patch.object(helpers, 'ROOT', self.root), \
                    patch.object(helpers, 'setup', side_effect=[self.build, inventory]) as setup, \
                    patch('s07_producers.prepare_subset', side_effect=AssertionError('subset reached')) as subset, \
                    patch('s07_producers.command', side_effect=AssertionError('source capture reached')) as command:
                with self.assertRaisesRegex(ValueError, 'named test inventory drift'):
                    program()
                self.assertEqual(setup.call_count, 2)
                subset.assert_not_called()
                command.assert_not_called()

    def test_manifest_drift_stops_before_build_or_capture(self):
        self.manifest['upstream_pin'] = 'another-pin'
        self.write_manifest()
        with patch('s07_program_compare.input_fingerprints', return_value={}), \
                patch('s07_producers.ROOT', self.root), \
                patch.object(helpers, 'ROOT', self.root), \
                patch.object(helpers, 'setup') as setup, \
                patch('s07_producers.prepare_subset', side_effect=AssertionError('subset reached')) as subset, \
                patch('s07_producers.command', side_effect=AssertionError('source capture reached')) as command:
            with self.assertRaisesRegex(ValueError, 'helper checker inventory changed'):
                program()
            setup.assert_not_called()
            subset.assert_not_called()
            command.assert_not_called()

    def test_helper_source_check_failure_stops_before_expensive_capture(self):
        self.write_manifest()
        def setup(args, *unused, **kwargs):
            if args[0] == 'cargo':
                return self.build
            if args[0] == self.binary:
                return self.valid_inventory
            raise ValueError('stale helper source manifest')
        with patch('s07_program_compare.input_fingerprints', return_value={}), \
                patch('s07_producers.ROOT', self.root), patch.object(helpers, 'ROOT', self.root), \
                patch.object(helpers, 'setup', side_effect=setup), \
                patch('s07_producers.prepare_subset') as subset, \
                patch('s07_producers.command') as command:
            with self.assertRaisesRegex(ValueError, 'stale helper source manifest'):
                program()
            subset.assert_not_called()
            command.assert_not_called()


class ProgramResumeTests(unittest.TestCase):
    def test_current_stages_reuse_without_capture_and_corruption_is_not_hidden(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'target/s07-program-reports').mkdir(parents=True)
            loader_report = dict(loader_graph_parity=True, config_parity=True,
                                 metrics={'subset_loads': True}, required_variants=2, passed_variants=2)
            verification = dict(metrics={'option_verification': True}, passed_variants=2)
            with patch('s07_producers.ROOT', root), \
                    patch('s07_program_compare.input_fingerprints', side_effect=dict), \
                    patch('s07_program_helpers.preflight', return_value='prepared'), \
                    patch('s07_program_helpers.measure', return_value={'pass': True, 'tests': 1}), \
                    patch('s07_producers.prepare_subset', return_value=(root/'sources', root/'native', root/'requests')), \
                    patch('s07_subset_freeze.freeze'), \
                    patch('s07_program_replay.replay_loader', return_value=loader_report) as replay_loader, \
                    patch('s07_program_replay.replay_verification', return_value=verification), \
                    patch('s07_program_compare.check_subset') as loader_capture, \
                    patch('s07_verify_compare.capture') as verification_capture, \
                    patch('s07_producers.command') as command, \
                    patch('sys.stderr'):
                self.assertTrue(program()['metrics']['subset_loads'])
                loader_capture.assert_not_called()
                verification_capture.assert_not_called()
                command.assert_not_called()
                for name in ('.cargo/config.toml', 'crates/tsr_bundled/bundled/libs/lib.d.ts'):
                    path = root / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text('before')
                    def change_build_input(*args):
                        path.write_text('after')
                        return loader_report
                    replay_loader.side_effect = change_build_input
                    with self.subTest(changed_input=name):
                        result = program()
                        self.assertFalse(result['metrics']['source_stable'])
                        self.assertFalse(result['metrics']['subset_loads'])
                replay_loader.side_effect = ValueError('corrupt stored output')
                with self.assertRaisesRegex(ValueError, 'corrupt stored output'):
                    program()
                loader_capture.assert_not_called()
                verification_capture.assert_not_called()
                command.assert_not_called()


class BinderGateTests(unittest.TestCase):
    def setUp(self):
        self.manifest = {'groups': [{'name': 'resolver', 'tests': ['name_resolver', 'reference_resolver']}]}
        self.documents = {'binder-cases.json': ['a', 'b'],
                          'binder-supplemental.json': {'requests': [{'id': 'edge'}, {'id': 'alias'}]}}
        self.report = {'tests': {'a': True, 'b': True}, 'supplemental': {'requests': 2, 'passed': 2}}
        self.helpers = {'results': [{'group': 'resolver', 'test': name, 'pass': True}
                                    for name in self.manifest['groups'][0]['tests']]}
        self.protocol = {'passed': sorted(GRAPH_CONTRACT_TESTS), 'tests': len(GRAPH_CONTRACT_TESTS)}

    def metrics(self):
        with patch('s07_helpers.load_manifest', return_value=(self.manifest, 'hash')):
            return binder_contract_metrics(self.documents, self.report, self.helpers, self.protocol)

    def test_complete_measurements_pass_separate_gates(self):
        self.assertEqual(self.metrics(), {'resolvers': True, 'graph_contracts': True})

    def test_missing_reordered_duplicate_or_failed_resolver_cannot_pass(self):
        original = copy.deepcopy(self.helpers['results'])
        for rows in ([], original[:1], original[::-1], original + original[:1],
                     [{**row, 'pass': False} for row in original]):
            with self.subTest(rows=rows):
                self.helpers['results'] = rows
                self.assertFalse(self.metrics()['resolvers'])
                self.assertTrue(self.metrics()['graph_contracts'])

    def test_failed_or_missing_primary_graph_is_not_hidden_by_supplemental_success(self):
        for rows in ({'a': True}, {'a': True, 'b': False}, {'a': True, 'b': True, 'extra': True}):
            self.report['tests'] = rows
            self.assertFalse(self.metrics()['graph_contracts'])

    def test_supplemental_mismatch_or_reduced_denominator_fails_graph_gate(self):
        for supplemental in ({'requests': 2, 'passed': 1}, {'requests': 1, 'passed': 1}):
            self.report['supplemental'] = supplemental
            self.assertFalse(self.metrics()['graph_contracts'])

    def test_removed_graph_obligation_fails_even_when_protocol_count_is_adjusted(self):
        self.protocol['passed'].remove('deleted flow edge')
        self.protocol['tests'] -= 1
        self.assertFalse(self.metrics()['graph_contracts'])

    def test_duplicate_graph_obligation_and_empty_protocol_cannot_pass(self):
        self.protocol['passed'].append(self.protocol['passed'][0])
        self.protocol['tests'] += 1
        self.assertFalse(self.metrics()['graph_contracts'])
        self.protocol = {'passed': [], 'tests': 0}
        self.assertFalse(self.metrics()['graph_contracts'])


if __name__ == '__main__':
    unittest.main()
