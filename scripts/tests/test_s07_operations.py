"""Reject incomplete, stale and self-consistently reduced operation inventories."""
import copy
from contextlib import redirect_stderr, redirect_stdout
import io
import json
import re
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s07_operation_validation import ROOT, validate_document
import s07_operations
from s07_subset import json_bytes, sha256


class OperationMatrixTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = json.loads((ROOT / 'data/s07/operations.json').read_bytes())

    def test_pinned_document_has_a_complete_rooted_static_closure(self):
        result = validate_document(self.document, regenerate=False)
        self.assertEqual(len(result), 6)
        self.assertEqual(sum(row['required_generated_functions'] for row in result), 2)

    def test_committed_rust_mapping_lines_still_name_the_source_marker(self):
        files = {}
        for identifier, function in self.document['functions'].items():
            for mapping in function['rust_mappings']:
                path = mapping['file']
                if path not in files:
                    files[path] = (ROOT / path).read_text().splitlines()
                with self.subTest(identifier=identifier, path=path):
                    line = mapping['line']
                    self.assertGreaterEqual(line, 1)
                    self.assertLessEqual(line, len(files[path]))
                    markers = re.findall(r'\bport:\s+(tsc/\S+)', files[path][line - 1])
                    self.assertIn(identifier, markers,
                                  'source formatting moved an anchor: regenerate the operation inventory')

    def test_committed_subset_review_covers_current_operation_inventory(self):
        # A valid regenerated inventory can still invalidate the accepted subset
        # through its fingerprint. Catch that before either producer runs Go.
        directory = ROOT / 'data/s07'
        rule = json.loads((directory / 'subset-rule.json').read_bytes())
        review = json.loads((directory / 'subset-review.json').read_bytes())
        self.assertEqual(rule['operation_matrix']['sha256'],
                         sha256((directory / 'operations.json').read_bytes()),
                         'operation anchors changed: review and refreeze the dependent subset rule')
        self.assertEqual(rule.pop('review_sha256'), sha256(json_bytes(review)))
        self.assertEqual(rule['state'], 'frozen')
        for name, expected in rule['provenance']['syntax']['producer_inputs'].items():
            self.assertEqual(expected, sha256((ROOT / name).read_bytes()),
                             'syntax observation input changed: ' + name)
        rule['state'] = 'candidate_pending_dependency_closure_and_review'
        self.assertEqual(review['candidate_sha256']['subset-rule.json'], sha256(json_bytes(rule)))
        for name in ('subset.json', 'checker-obligations.json'):
            self.assertEqual(review['candidate_sha256'][name], sha256((directory / name).read_bytes()), name)

    def test_embedded_inventory_progress_does_not_corrupt_producer_json(self):
        stdout, stderr = io.StringIO(), io.StringIO()
        package = b'{"ImportPath":"github.com/microsoft/TypeScript/tsc/internal/compiler","GoFiles":[]}'
        with patch('s07_operations.upstream_pin', return_value=(self.document['upstream_pin'], 'go1.27.1')):
            with patch('s07_operations.command', side_effect=[package, RuntimeError('stop after progress')]):
                with redirect_stdout(stdout), redirect_stderr(stderr):
                    with self.assertRaisesRegex(RuntimeError, 'stop after progress'):
                        s07_operations.create()
        self.assertEqual(stdout.getvalue(), '')
        self.assertEqual(stderr.getvalue(), 'source package compiler\n')

    def reject(self, mutate):
        altered = copy.deepcopy(self.document)
        mutate(altered)
        with self.assertRaises(ValueError):
            validate_document(altered, regenerate=False)

    def test_required_operation_cannot_be_missing_or_reordered(self):
        self.reject(lambda d: d['operations'].pop())
        self.reject(lambda d: d['operations'].reverse())

    def test_source_anchor_kind_and_fingerprint_are_checked(self):
        identifier = next(iter(self.document['functions']))
        for key, value in [('start_line', 0), ('kind', 'generated'), ('source_sha256', '0' * 64)]:
            self.reject(lambda d, key=key, value=value: d['functions'][identifier].update({key: value}))
        self.reject(lambda d: d['generator_inputs'].pop('scripts/s07_operations.py'))

    def test_generated_functions_cannot_inflate_source_denominator(self):
        def move(d):
            op = next(o for o in d['operations'] if o['required_generated_functions'])
            value = op['required_generated_functions'].pop()
            op['required_source_functions'] = sorted(op['required_source_functions'] + [value])
        self.reject(move)

    def test_named_comparator_callbacks_are_required_source_dependencies(self):
        operation = next(o for o in self.document['operations'] if o['id'] == 'program.option_verification')
        for name in ('CompareDiagnostics', 'EqualDiagnostics', 'EqualDiagnosticsNoRelatedInfo'):
            self.assertIn('tsc/internal/ast/diagnostic.go:' + name, operation['required_source_functions'])
        self.assertTrue(any(c.get('reference') and c.get('callee') == 'tsc/internal/ast/diagnostic.go:CompareDiagnostics'
                            for c in self.document['transitive_calls']))
        self.assertIn('tsc/internal/compiler/emitter.go:sourceFileMayBeEmitted', operation['required_source_functions'])

    def test_unknown_static_callee_and_outside_caller_are_rejected(self):
        self.reject(lambda d: d['transitive_calls'][0].update(callee='tsc/internal/fake.go:missing'))
        self.reject(lambda d: d['transitive_calls'][0].update(line=0))
        self.reject(lambda d: d.update(unresolved_calls=[{'expression': 'unknown()'}]))

    def test_missing_exclusion_reason_and_undefined_operation_are_rejected(self):
        self.reject(lambda d: d['operations'][0]['excluded_source_functions'][0].update(reason=''))
        self.reject(lambda d: d['dynamic_calls'][0].update(operation='unknown'))

    def test_external_label_cannot_hide_unknown_project_function(self):
        def change(d):
            d['external_boundaries'][0].update(callee=None, interface=False,
                                              package='github.com/microsoft/TypeScript/tsc/internal/core')
        self.reject(change)

    def test_regeneration_detects_self_consistent_omitted_callback(self):
        altered = copy.deepcopy(self.document)
        altered['dynamic_calls'].pop()
        # Static closure is unchanged; regenerating the pinned source is what
        # detects this reduction rather than trusting internal consistency.
        validate_document(altered, regenerate=False)
        with patch('s07_operation_validation.create', return_value=self.document):
            with self.assertRaisesRegex(ValueError, 'differs from current pinned source'):
                validate_document(altered)


if __name__ == '__main__':
    unittest.main()
