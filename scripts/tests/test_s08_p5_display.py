import contextlib
import copy
import io
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s08_p5_display import ROOT, canonical, compare, strict_json_loads, validate


class DisplayProtocol(unittest.TestCase):
    def setUp(self):
        self.native = ROOT / 'data/s08/p5/display'
        self.request = strict_json_loads((self.native / 'requests.json').read_bytes())
        self.observed = strict_json_loads((self.native / 'observations.json').read_bytes())

    def test_native_inventory_has_valid_states(self):
        self.assertEqual(validate(self.request, self.observed), 178)

    def test_unknown_or_mistyped_module_options_fail(self):
        for bad in ('commonjs', '', 199, None, True):
            request = copy.deepcopy(self.request)
            request['programs'][0]['module'] = bad
            with self.subTest(module=bad), self.assertRaises(ValueError):
                validate(request, self.observed)

    def test_missing_reordered_or_duplicate_queries_fail(self):
        for mutation in ('missing', 'reordered', 'duplicate'):
            value = copy.deepcopy(self.observed)
            queries = value['programs'][0]['queries']
            if mutation == 'missing':
                queries.pop()
            elif mutation == 'reordered':
                queries.reverse()
            else:
                queries[1] = copy.deepcopy(queries[0])
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                validate(self.request, value)

    def test_absent_cannot_masquerade_as_empty_content(self):
        self.observed['programs'][0]['queries'][0] = {'id': 'hex-context-free', 'state': 'absent'}
        with self.assertRaises(ValueError):
            validate(self.request, self.observed)

    def test_comparison_rejects_changed_bytes_or_request(self):
        for mutation in ('bytes', 'request'):
            value = copy.deepcopy(self.observed)
            if mutation == 'bytes':
                value['programs'][0]['queries'][1]['text_hex'] += '20'
            else:
                value['request_sha256'] = '0' * 64
            with tempfile.TemporaryDirectory() as temporary:
                actual = Path(temporary) / 'actual.json'
                actual.write_bytes(canonical(value))
                with self.subTest(mutation=mutation), self.assertRaises(ValueError), contextlib.redirect_stdout(io.StringIO()):
                    compare(self.native, actual)

    def test_raw_source_encoding_rejects_overlap_and_malformed_hex(self):
        for bad in ('ff0', 'not hex', 'FF', 'ff 00', None):
            request = copy.deepcopy(self.request)
            next(p for p in request['programs'] if p['id'] == 'lossless-source-and-regenerated-literals')['file_bytes']['/main.ts'] = bad
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                validate(request, self.observed)
        request = copy.deepcopy(self.request)
        next(p for p in request['programs'] if p['id'] == 'lossless-source-and-regenerated-literals')['files']['/main.ts'] = 'different source'
        with self.assertRaises(ValueError):
            validate(request, self.observed)
