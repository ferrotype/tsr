import contextlib
import copy
import io
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s08_p5_walker import ROOT, canonical, compare, normalized, strict_json_loads, validate
from s08_p5_corpus import compare_row


class WalkerContract(unittest.TestCase):
    def setUp(self):
        self.native = ROOT / 'data/s08/p5/walker'
        self.request = strict_json_loads((self.native / 'requests.json').read_bytes())
        self.observed = strict_json_loads((self.native / 'observations.json').read_bytes())

    def test_native_inventory_and_empty_states(self):
        self.assertEqual(validate(self.request, self.observed), 24)
        rows = self.observed['cases']
        self.assertEqual(rows[0]['types'], {'state': 'disabled'})
        self.assertEqual(rows[1]['types'], {'state': 'no_content'})
        self.assertEqual(rows[2]['types']['state'], 'content')

    def assert_comparison_fails(self, value):
        with tempfile.TemporaryDirectory() as directory:
            actual = Path(directory) / 'actual.json'
            actual.write_bytes(canonical(value))
            with self.assertRaises(ValueError), contextlib.redirect_stdout(io.StringIO()):
                compare(self.native, actual)

    def test_missing_reordered_or_duplicate_cases_fail(self):
        for mutation in ('missing', 'reordered', 'duplicate'):
            value = copy.deepcopy(self.observed)
            if mutation == 'missing': value['cases'].pop()
            elif mutation == 'reordered': value['cases'].reverse()
            else: value['cases'][1] = copy.deepcopy(value['cases'][0])
            with self.subTest(mutation=mutation): self.assert_comparison_fails(value)

    def test_bytes_query_order_flags_and_absence_are_compared(self):
        for mutation in ('bytes', 'order', 'missing', 'flags', 'absent', 'empty', 'request'):
            value = copy.deepcopy(self.observed)
            case = value['cases'][3]
            if mutation == 'bytes': case['types']['text_hex'] += '20'
            elif mutation == 'order': case['queries'][0], case['queries'][1] = case['queries'][1], case['queries'][0]
            elif mutation == 'missing': case['queries'].pop()
            elif mutation == 'flags': next(q for q in case['queries'] if q['operation'] == 'TypeToTypeNode')['flags'] ^= 1
            elif mutation == 'absent': next(q for q in case['queries'] if q.get('absent')).pop('absent')
            elif mutation == 'empty': case['types'] = {'state': 'content', 'text_hex': ''}
            else: value['request_sha256'] = '0' * 64
            with self.subTest(mutation=mutation): self.assert_comparison_fails(value)

    def test_runtime_ids_normalize_but_repeated_identity_does_not_disappear(self):
        original = self.observed['cases'][4]
        renamed = copy.deepcopy(original)
        for q in renamed['queries']:
            if 'type_id' in q: q['type_id'] += 1000
        self.assertEqual(normalized(original), normalized(renamed))
        typed = [q for q in renamed['queries'] if 'type_id' in q]
        typed[-1]['type_id'] += 1000
        self.assertNotEqual(normalized(original), normalized(renamed))

    def test_failed_walker_cannot_pass_as_no_content(self):
        value = copy.deepcopy(self.observed)
        value['cases'][3]['state'] = 'failed'
        self.assert_comparison_fails(value)
        row = {'type_symbol_baselines': {'state': 'failed', 'class': 'walker_error', 'reason': 'unsupported'}}
        self.assertEqual(compare_row(self.observed['cases'][3], row)['state'], 'failed')
