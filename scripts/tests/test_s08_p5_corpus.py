"""P5 cannot turn missing diagnostics, emit work or capture rows into parity."""
import contextlib
import copy
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s08_p5_corpus as p5


class CorpusContract(unittest.TestCase):
    def setUp(self):
        self.request = {'id': 'fixture#configuration=0', 'acceptance_tier': 'acceptance',
                        'diagnostic_phases': ['config', 'program', 'syntactic', 'semantic', 'global'],
                        'type_baseline_requested': False, 'error_baseline_requested': True, 'loading': {},
                        'error_inputs': [{'name_hex': '2f612e7473', 'content_hex': 'ff'}]}
        self.diagnostic = {'file_hex': '2f612e7473', 'pos': 0, 'end': 1, 'code': 2322, 'category': 1,
                           'key_hex': '61', 'text_hex': '', 'source_hex': '', 'args_hex': ['ff'],
                           'chain': [], 'related': [], 'unnecessary': False, 'deprecated': False,
                           'skipped_on_no_emit': False}
        ds = [self.diagnostic]
        self.native = {'id': self.request['id'], 'state': 'executed', 'acceptance_tier': 'acceptance',
                       'types': {'state': 'disabled'}, 'symbols': {'state': 'disabled'}, 'queries': [],
                       'error_pre_diagnostics': ds, 'error_post_diagnostics': ds, 'error_diagnostics': ds,
                       'error_inputs': self.request['error_inputs'], 'error_render_inputs': self.request['error_inputs'],
                       'error_pretty': False, 'errors': {'state': 'content', 'text_hex': 'ff'}}
        self.row = {'version': 1, 'id': self.request['id'], 'acceptance_tier': 'acceptance',
                    'load': {'state': 'executed', 'graph': {'ID': self.request['id'], 'Files': []}},
                    'bind_diagnostics': {'state': 'executed', 'diagnostics': []},
                    'phases': {p: {'state': 'executed', 'diagnostics': []} for p in self.request['diagnostic_phases']},
                    'type_symbol_baselines': {'state': 'not_requested'},
                    'error_baseline': {'state': 'executed', 'diagnostics': ds, 'baseline': self.native['errors'],
                                       'emit': 'not_executed', 'pretty': False, 'inputs': self.native['error_inputs']}}

    def test_errors_are_compared_when_type_baselines_are_disabled(self):
        p5.native_metadata(self.request, self.native)
        p5.validate_row(self.request, self.row)
        result = p5.summarize([self.request], [self.row], [self.native])
        self.assertEqual(result['rows'][0]['state'], 'match')
        self.assertEqual(result['rows'][0]['type_symbols']['state'], 'disabled')
        self.assertEqual(result['rows'][0]['errors']['emit'], 'not_executed')

    def test_each_error_dimension_is_compared_without_normalizing_bytes(self):
        for key in ('diagnostics', 'related', 'chain', 'inputs', 'pretty', 'bytes'):
            row = copy.deepcopy(self.row)
            actual = row['error_baseline']
            if key == 'diagnostics': actual['diagnostics'][0]['args_hex'] = ['efbfbd']
            elif key in ('related', 'chain'): actual['diagnostics'][0][key] = [copy.deepcopy(self.diagnostic)]
            elif key == 'inputs': actual['inputs'].reverse(); actual['inputs'].append(actual['inputs'][0])
            elif key == 'pretty': actual['pretty'] = True
            else: actual['baseline']['text_hex'] = 'efbfbd'
            with self.subTest(key=key):
                self.assertEqual(p5.compare_errors(self.native, row)['state'], 'different')

    def test_pre_post_difference_is_visible_even_when_rust_matches_post(self):
        native = copy.deepcopy(self.native)
        native['error_pre_diagnostics'] = []
        differences = p5.compare_errors(native, self.row)['differences']
        self.assertEqual(differences, ['native_pre_post_diagnostics', 'pre_diagnostics'])

    def test_cannot_lose_errors_drop_requests_or_claim_emit(self):
        for mutation in ('missing', 'disabled', 'empty', 'emit', 'phase', 'request'):
            request, row = copy.deepcopy(self.request), copy.deepcopy(self.row)
            if mutation == 'missing': row.pop('error_baseline')
            elif mutation == 'disabled': row['error_baseline'] = {'state': 'not_requested'}
            elif mutation == 'empty': row['error_baseline']['baseline'] = {'state': 'no_content'}
            elif mutation == 'emit': row['error_baseline']['emit'] = 'executed'
            elif mutation == 'phase': row['phases']['global'] = {'state': 'failed', 'class': 'unsupported', 'reason': 'reason'}
            else: request.pop('error_baseline_requested')
            with self.subTest(mutation=mutation), self.assertRaises(ValueError): p5.validate_row(request, row)

    def test_input_selection_preserves_duplicates_and_source_order(self):
        native = copy.deepcopy(self.native)
        a, b = native['error_inputs'][0], {'name_hex': '62', 'content_hex': ''}
        native['error_inputs'] = [a, b, a]
        native['error_render_inputs'] = [a, a]
        p5.native_metadata(self.request, native)
        native['error_render_inputs'] = [b, b]
        with self.assertRaisesRegex(ValueError, 'subsequence'): p5.native_metadata(self.request, native)
        native['error_render_inputs'] = [{'name_hex': 'FF', 'content_hex': ''}]
        with self.assertRaisesRegex(ValueError, 'hex'): p5.native_metadata(self.request, native)

    def test_resume_and_replay_authenticate_binary_sources_native_and_raw_results(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); native = root / 'native'; native.mkdir()
            p5.write_new(native / 'observations.ndjson', self.native)
            p5.write_new(native / 'report.json', {'observation_sha256': p5.digest((native / 'observations.ndjson').read_bytes())})
            binary = root / 'adapter'
            binary.write_text('#!/usr/bin/env python3\nimport sys\nfrom pathlib import Path\nPath(sys.argv[2]).write_bytes(' + repr(p5.canonical(self.row)) + ')\n')
            binary.chmod(0o755)
            def build(directory, **_):
                directory.mkdir()
                snapshot = directory / 'source-snapshot'; snapshot.mkdir()
                return {'sources': {}, 'source_snapshot': str(snapshot), 'binary': str(binary),
                        'binary_sha256': p5.digest(binary.read_bytes())}
            output = root / 'output'
            with patch.object(p5, 'prepare', return_value=[(self.request, self.native)]), patch.object(p5.p4, 'build', side_effect=build), patch.object(p5, 'sources', return_value={}) as current, contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                p5.run(native, None, output, 5)
                result = output / 'cases/00000/result.json'
                original = result.read_bytes()
                current.return_value = {'changed.rs': 'changed'}
                p5.run(native, None, output, 5, resume=True)
                self.assertEqual(result.read_bytes(), original)
                self.assertFalse(p5.read(output / 'report.json')['source_stable'])
                self.assertEqual(p5.replay(output)[2]['counts_by_tier']['acceptance'], {'match': 1})
                with self.assertRaisesRegex(ValueError, 'identical'): p5.run(native, None, output, 6, resume=True)
                altered = p5.read(result)
                altered['row']['error_baseline']['pretty'] = True
                result.write_bytes(p5.canonical(altered))
                with self.assertRaisesRegex(ValueError, 'raw observation'): p5.replay(output)
                result.write_bytes(original)
                for path in (output / 'native-observations.ndjson', output / 'executable', output / 'cases/00000/observation.json'):
                    raw = path.read_bytes(); path.write_bytes(raw + b' ')
                    with self.assertRaises(ValueError): p5.replay(output)
                    path.write_bytes(raw)
                (output / 'source-snapshot/extra.rs').write_bytes(b'extra')
                with self.assertRaisesRegex(ValueError, 'snapshot'): p5.replay(output)


if __name__ == '__main__': unittest.main()
