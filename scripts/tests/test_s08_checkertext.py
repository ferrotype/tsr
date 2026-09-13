import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s08_checkertext import CASES, COVERAGE, REQUESTS, ROOT, compare, strict_json_loads, validate_inventory


class CheckerTextProtocol(unittest.TestCase):
    def setUp(self):
        self.request = strict_json_loads(REQUESTS.read_bytes())
        self.coverage = strict_json_loads(COVERAGE.read_bytes())
        self.cases = strict_json_loads(CASES.read_bytes())
        self.native = strict_json_loads((ROOT / 'data/s08/p5/text/observations.json').read_bytes())

    def test_all_frozen_queries_are_required(self):
        report, failures = compare(self.request, self.coverage, self.cases, self.native, self.native)
        self.assertEqual(report['metrics']['probes'], 64)
        self.assertEqual(report['metrics']['token_literal_bytes_probes'], 52)
        self.assertEqual(report['metrics']['helper_printer_semantics_probes'], 64)
        self.assertTrue(all(value == 'pass' for value in report['tests'].values()))
        self.assertEqual(failures, [])

    def test_a_measured_mismatch_fails_its_criterion_and_is_retained(self):
        actual = copy.deepcopy(self.native)
        actual['programs'][2]['queries'][0]['text_hex'] += '20'
        report, failures = compare(self.request, self.coverage, self.cases, self.native, actual)
        self.assertTrue(report['metrics']['token_literal_bytes'])
        self.assertFalse(report['metrics']['helper_printer_semantics'])
        self.assertEqual(report['metrics']['failed_probes'], 1)
        self.assertEqual(failures[0]['case'], 'source-annotation-and-regeneration')
        self.assertNotEqual(failures[0]['native'], failures[0]['rust'])

    def test_absent_on_both_sides_is_not_literal_evidence(self):
        native = copy.deepcopy(self.native)
        row = native['programs'][0]['queries'][1]
        native['programs'][0]['queries'][1] = {'id': row['id'], 'state': 'absent'}
        report, failures = compare(self.request, self.coverage, self.cases, native, native)
        self.assertFalse(report['metrics']['token_literal_bytes'])
        self.assertFalse(report['metrics']['helper_printer_semantics'])
        self.assertEqual(len(failures), 1)

    def test_selection_and_results_cannot_shrink_or_change_identity(self):
        for mutation in ('digest', 'group', 'queries', 'criterion', 'all'):
            coverage = copy.deepcopy(self.coverage)
            if mutation == 'digest': coverage['request_sha256'] = '0' * 64
            elif mutation == 'group': coverage['groups'].pop()
            elif mutation == 'queries': coverage['groups'][0]['queries'].pop()
            elif mutation == 'criterion': coverage['groups'][0]['criteria'] = ['unknown']
            else:
                for group in coverage['groups']: group['criteria'] = ['helper_printer_semantics']
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                validate_inventory(self.request, coverage, self.cases)
        actual = copy.deepcopy(self.native)
        actual['programs'][0]['queries'].pop()
        with self.assertRaises(ValueError):
            compare(self.request, self.coverage, self.cases, self.native, actual)
        actual = copy.deepcopy(self.native)
        actual['request_sha256'] = '0' * 64
        with self.assertRaises(ValueError):
            compare(self.request, self.coverage, self.cases, self.native, actual)
