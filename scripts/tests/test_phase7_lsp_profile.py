"""The repeated LSP profiling workload, without starting a profiler or server."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools.phase7.lsp import profile  # noqa: E402


def completions():
    return {
        'isIncomplete': False,
        'itemDefaults': {'commitCharacters': ['.', '('], 'data': {'version': 2}},
        'items': [
            {'label': 'beta', 'sortText': '2', 'data': {'id': 7, 'tags': [2, 1]},
             'additionalTextEdits': [{'newText': 'second'}, {'newText': 'first'}]},
            {'label': 'alpha', 'sortText': '1', 'detail': 'complete detail'},
        ],
    }


class LspProfileTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory())).resolve()
        self.fixture = self.root / 'fixture'
        self.fixture.mkdir()
        (self.fixture / 'main.ts').write_text('const x = 1;')
        self.binary = self.root / 'server'
        self.binary.write_bytes(b'not executed')
        # Scenario validation belongs to capture's tests; keep this fixture
        # focused on the wrapper that profile installs around measured_request.
        self.enterContext(mock.patch.object(profile.capture, 'validate_scenario'))
        self.calls = []

    def invoke(self, output, results, *, metric='completion', other_request=False,
               runtime_error=None):
        pending = iter(results)
        params = {'textDocument': {'uri': 'file:///main.ts'},
                  'position': {'line': 0, 'character': 2}}
        calls = self.calls

        class FakePeer:
            def measured_request(self, method, requested):
                calls.append((method, copy.deepcopy(requested)))
                result = next(pending)
                if isinstance(result, Exception):
                    raise result
                return {'jsonrpc': '2.0', 'id': len(calls), 'result': result}, len(calls) * 11

        def run_runtime(command, cwd, scenario, artifact):
            self.assertEqual(command[0:2], ['samply', 'record'])
            self.assertEqual(command[-3:], [str(self.binary), '--lsp', '--stdio'])
            self.assertEqual((cwd / 'main.ts').read_text(), 'const x = 1;')
            if runtime_error:
                raise runtime_error
            peer = profile.capture.Peer()
            if other_request:
                other, elapsed = peer.measured_request('textDocument/hover', params)
                self.assertEqual(other['result'], {'contents': ['untouched', 'order']})
                self.assertEqual(elapsed, 11)
            method = profile.capture.METHODS[metric]
            response, elapsed = peer.measured_request(method, params)
            self.assertEqual(elapsed, 22 if other_request else 11)
            self.assertEqual(response['id'], 2 if other_request else 1)
            return {}, {'responses': [{'method': method, 'message': response}]}

        with mock.patch.object(profile.capture, 'Peer', FakePeer), \
                mock.patch.object(profile.capture, 'run_runtime', side_effect=run_runtime):
            try:
                return profile.profile(self.binary, self.fixture, {'fixed': True}, metric, 2, output)
            finally:
                self.assertIs(profile.capture.Peer, FakePeer)

    def test_repeats_only_selected_method_and_preserves_completion_payload(self):
        original = completions()
        reordered = copy.deepcopy(original)
        reordered['items'].reverse()
        original_before, reordered_before = copy.deepcopy((original, reordered))
        output = self.root / 'success'
        report = self.invoke(output, [{'contents': ['untouched', 'order']},
                                      original, reordered, original], other_request=True)
        self.assertTrue(report['complete'])
        self.assertEqual(report['repeats'], 2)
        self.assertIsInstance(report['repeated_interval_ns'], int)
        self.assertEqual([method for method, _ in self.calls],
                         ['textDocument/hover'] + ['textDocument/completion'] * 3)
        self.assertTrue(all(params == self.calls[0][1] for _, params in self.calls))
        self.assertEqual(original, original_before)
        self.assertEqual(reordered, reordered_before)
        transcript = json.loads((output / 'transcript.json').read_text())
        self.assertEqual(transcript['responses'][0]['message'],
                         {'jsonrpc': '2.0', 'id': 2, 'result': original_before})
        self.assertEqual(json.loads((output / 'profile.json').read_text()), report)

    def test_changed_completion_fields_and_nested_array_order_fail(self):
        mutations = {
            'payload': lambda value: value['items'][0]['data'].update(id=8),
            'item field': lambda value: value['items'][1].pop('detail'),
            'list metadata': lambda value: value.update(isIncomplete=True),
            'defaults': lambda value: value['itemDefaults']['data'].update(version=3),
            'nested order': lambda value: value['items'][0]['additionalTextEdits'].reverse(),
            'duplicate': lambda value: value['items'].append(copy.deepcopy(value['items'][0])),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name):
                self.calls.clear()
                original = completions()
                changed = copy.deepcopy(original)
                mutate(changed)
                output = self.root / name
                with self.assertRaisesRegex(ValueError, 'textDocument/completion: repeated response changed'):
                    self.invoke(output, [original, changed, original])
                self.assertEqual(len(self.calls), 2, 'stop at the changed repeat')
                self.assert_incomplete(output)

    def test_noncompletion_arrays_are_compared_in_order(self):
        original = [{'uri': 'file:///b.ts', 'range': {'start': 1, 'end': 2}},
                    {'uri': 'file:///a.ts', 'range': {'start': 3, 'end': 4}}]
        with self.assertRaisesRegex(ValueError, 'textDocument/references: repeated response changed'):
            self.invoke(self.root / 'references', [original, original[::-1]], metric='references')
        self.assert_incomplete(self.root / 'references')

    def test_peer_is_restored_after_runtime_and_repeat_failures(self):
        for name, results, error in [
            ('runtime', [], RuntimeError('runtime failed')),
            ('repeat', [completions(), TimeoutError('repeat timed out')], None),
        ]:
            with self.subTest(name=name):
                with self.assertRaisesRegex((RuntimeError, TimeoutError), 'runtime failed|repeat timed out'):
                    self.invoke(self.root / name, results, runtime_error=error)
                self.assert_incomplete(self.root / name)

    def assert_incomplete(self, output):
        record = json.loads((output / 'profile.json').read_text())
        self.assertFalse(record['complete'])
        self.assertNotIn('repeated_interval_ns', record)
        self.assertFalse((output / 'transcript.json').exists())


if __name__ == '__main__':
    unittest.main()
