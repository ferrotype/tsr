"""Failure-oriented tests for the program acceptance boundary."""
import copy
import unittest
from s04_common import strict_json_loads
from s07_program import validate_requests
from s07_program_compare import compare, compare_config, differences


class ProgramComparisonTests(unittest.TestCase):
    def setUp(self):
        self.requests = [{'id': 'a'}, {'id': 'b'}]
        self.oracle = [{'ID': 'a', 'Files': [], 'Trace': None}, {'ID': 'b', 'Files': [], 'Trace': None}]

    def test_identity_failures_are_not_shorter_passing_runs(self):
        for actual in [self.oracle[:1], self.oracle[::-1], self.oracle + [self.oracle[0]], [], [{'ID': 'c'}, self.oracle[1]]]:
            with self.subTest(actual=actual), self.assertRaises(ValueError):
                compare(self.requests, self.oracle, actual)

    def test_duplicate_source_request_is_rejected(self):
        with self.assertRaises(ValueError):
            compare([{'id': 'a'}, {'id': 'a'}], self.oracle, self.oracle)

    def test_identical_panics_never_certify_loading(self):
        rows = [{'ID': 'a', 'Panic': 'unrecognized assertion'}, {'ID': 'b'}]
        result = compare(self.requests, rows, rows)
        self.assertFalse(result[0]['passed'])
        self.assertEqual(result[0]['failure'], 'source_failure')
        self.assertEqual(result[0]['go_panic'], 'unrecognized assertion')

    def test_typed_unsupported_failure_is_retained(self):
        actual = copy.deepcopy(self.oracle)
        actual[0] = {'ID': 'a', 'Error': 'Unsupported("source operation")'}
        result = compare(self.requests, self.oracle, actual)
        self.assertEqual(result[0]['rust_error'], 'Unsupported("source operation")')
        self.assertFalse(result[0]['passed'])

    def test_nil_empty_extra_fields_and_numeric_boolean_are_differences(self):
        for change in ({'Trace': []}, {'Files': None}, {'Extra': False}):
            actual = copy.deepcopy(self.oracle)
            actual[0].update(change)
            self.assertFalse(compare(self.requests, self.oracle, actual)[0]['passed'])
        self.assertEqual(differences(False, 0), ['/'])

    def test_nested_source_difference_has_precise_pointer(self):
        self.assertEqual(differences({'Files': [{'Meta': {'PackageJsonType': 'module'}}]},
                                     {'Files': [{'Meta': {'PackageJsonType': ''}}]}),
                         ['/Files/0/Meta/PackageJsonType'])

    def test_wire_duplicate_keys_and_nonfinite_are_protocol_failures(self):
        for wire in ('{"ID":"a","ID":"b"}', '[NaN]', '[1e999]'):
            with self.subTest(wire=wire), self.assertRaises(ValueError):
                strict_json_loads(wire)

    def test_request_source_bytes_are_not_coerced(self):
        request = dict(id='a', cwd='/', case_sensitive=True, roots=['/a.ts'], files={'/a.ts': 'ff'}, options={})
        self.assertEqual(validate_requests([request]), ['a'])
        for bad in ('xyz', 'f', True):
            altered = copy.deepcopy(request)
            altered['files']['/a.ts'] = bad
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                validate_requests([altered])

    def test_config_passed_flags_are_not_observations(self):
        with self.assertRaises(ValueError):
            compare_config(['a'], [{'id':'a'}], [{'id':'a','passed':True}], [{'id':'a','passed':True}])

    def test_config_compares_every_output_field(self):
        source = {'id':'a','options':{},'root_file_names':[], 'config_raw':None,
                  'config_diagnostics':[], 'option_diagnostics':[], 'compile_on_save':None}
        for key, value in [('options',{'strict':False}), ('root_file_names',['/a.ts']),
                           ('config_raw',{}), ('config_diagnostics',[{}]),
                           ('option_diagnostics',[{}]), ('compile_on_save',False)]:
            actual = {**source,key:value}
            with self.subTest(key=key):
                rows = compare_config(['a'],[{'id':'a'}],[source],[actual])
                self.assertFalse(rows[0]['passed'])

    def test_tagged_options_and_raw_config_observe_source_property_order(self):
        ordered = {'kind': 'object', 'entries': [
            {'key': '6669727374', 'value': {'kind': 'null'}},
            {'key': '6f74686572', 'value': {'kind': 'null'}}]}
        source = {'id': 'a', 'options': ordered, 'root_file_names': [], 'config_raw': ordered,
                  'config_diagnostics': [], 'option_diagnostics': [], 'compile_on_save': None}
        for field in ('options', 'config_raw'):
            actual = copy.deepcopy(source)
            actual[field]['entries'].reverse()
            self.assertFalse(compare_config(['a'], [{'id': 'a'}], [source], [actual])[0]['passed'])


if __name__ == '__main__':
    unittest.main()
