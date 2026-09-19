import contextlib
import copy
import io
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s08_p5_errors import ROOT, canonical, compare, strict_json_loads, validate


class DiagnosticWriterContract(unittest.TestCase):
    def setUp(self):
        self.native = ROOT / 'data/s08/p5/errors'
        self.request = strict_json_loads((self.native / 'requests.json').read_bytes())
        self.result = strict_json_loads((self.native / 'observations.json').read_bytes())

    def test_complete_native_fixture(self):
        self.assertEqual(validate(self.request, self.result), 24)

    def test_missing_reordered_and_failed_cases_are_rejected(self):
        for mutation in ('missing', 'reorder', 'duplicate', 'failure'):
            row = copy.deepcopy(self.result)
            if mutation == 'missing': row['cases'].pop()
            elif mutation == 'reorder': row['cases'].reverse()
            elif mutation == 'duplicate': row['cases'][1] = row['cases'][0]
            else: row['cases'][0]['state'] = 'failed'
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                validate(self.request, row)

    def test_each_output_column_and_request_hash_are_compared(self):
        for key in ('plain_hex', 'pretty_hex', 'summary_hex', 'errors_plain', 'errors_pretty', 'request_sha256'):
            row = copy.deepcopy(self.result)
            case = row['cases'][2]
            if key == 'request_sha256': row[key] = '0' * 64
            elif key.startswith('errors_'): case[key]['text_hex'] += '20'
            else: case[key] += '20'
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / 'actual.json'
                path.write_bytes(canonical(row))
                with self.subTest(column=key), self.assertRaises(ValueError), contextlib.redirect_stdout(io.StringIO()):
                    compare(self.native, path)

    def test_empty_content_cannot_replace_no_content(self):
        row = copy.deepcopy(self.result)
        row['cases'][0]['errors_plain'] = {'state': 'content', 'text_hex': ''}
        with self.assertRaises(ValueError): validate(self.request, row)
