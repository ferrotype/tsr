"""Named S07 contract gates must observe their complete measured obligations."""
import copy
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s07_producers import GRAPH_CONTRACT_TESTS, binder_contract_metrics, program


class ProgramPreflightTests(unittest.TestCase):
    def test_inventory_drift_stops_before_subset_or_source_capture(self):
        with patch('s07_program_compare.input_fingerprints', return_value={}), \
                patch('s07_program_helpers.preflight', side_effect=ValueError('inventory drift')), \
                patch('s07_producers.prepare_subset') as subset, \
                patch('s07_producers.command') as command:
            with self.assertRaisesRegex(ValueError, 'inventory drift'):
                program()
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
