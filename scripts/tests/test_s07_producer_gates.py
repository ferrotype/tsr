"""Named S07 contract gates must observe their complete measured obligations."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s07_producers import GRAPH_CONTRACT_TESTS, binder_contract_metrics, program
import s07_program_helpers as helpers


class ProgramPreflightTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'data/s07').mkdir(parents=True)
        (self.root / 'data/upstream.json').write_text(json.dumps({'pin': 'test-pin'}))
        self.manifest = {'schema': 1, 'upstream_pin': 'test-pin', 'checks': helpers.CHECKS,
                         'groups': [{'package': 'ts_compiler', 'target': 'helpers',
                                     'prefix': 'named::', 'tests': ['named::first', 'named::second']}]}
        self.binary = str(self.root / 'helper-tests')
        self.build = json.dumps({'reason': 'compiler-artifact', 'profile': {'test': True},
                                 'executable': self.binary,
                                 'manifest_path': str(self.root / 'crates/ts_compiler/Cargo.toml'),
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
        self.assertEqual(prepared, (raw, self.manifest, {('ts_compiler', 'helpers'): self.binary}))
        self.assertEqual(setup.call_count, 2)
        self.assertEqual(setup.call_args_list[1].args[0], [self.binary, '--list', '--format=terse'])

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
