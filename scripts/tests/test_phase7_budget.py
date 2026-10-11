"""Bounded protocol and work-equivalence controls; no compiler builds."""
import copy
import gzip
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.phase7.budget import probe


OBSERVATION = {'outcome': 'compiled', 'roots': [[47, 97]],
               'sources': [{'name': [47, 97], 'path': [47, 97], 'text': [49],
                            'default_library': False, 'node_count': '10', 'identifier_count': '1'}],
               'options_json': list(b'{"paths":{"z":["z"],"a":["a"]}}'),
               'checkers': '1', 'types': '20', 'nodes': '10', 'identifiers': '1',
               'diagnostics': [], 'writes': [], 'emitted_files': [],
               'emit_skipped': None, 'emit_diagnostics': []}
GO_CHECKPOINT = dict.fromkeys('heap_alloc heap_inuse heap_sys stack_inuse stack_sys sys total_alloc mallocs frees num_gc'.split(), '100')
DIAGNOSTIC = {'file': 0, 'pos': '0', 'end': '1', 'code': 2304, 'category': 1,
              'message_key': [], 'message_text': [97], 'message_args': [],
              'localized': [97], 'chain': [], 'related': [], 'source': [],
              'unnecessary': False, 'deprecated': False, 'skipped_on_no_emit': False,
              'repopulate': None}


class BudgetControls(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def binary(self, behavior):
        path = self.root / 'child'
        path.write_text('''#!/usr/bin/env python3
import json, sys, time
observation = json.loads(%r)
checkpoint = json.loads(%r)
behavior = %r
for line in sys.stdin:
    request = json.loads(line)
    command = request['command']
    if behavior == 'timeout' and command == 'execute': time.sleep(30)
    if behavior == 'eof' and command == 'execute': sys.exit(0)
    kind = {'prepare':'prepared','execute':'executed','checkpoint':'checkpoint','observe':'observation','release':'released',
            'observe_begin':'observation_begin','observe_source':'observation_source','observe_write':'observation_write'}[command]
    value = observation if command == 'observe' else checkpoint if command == 'checkpoint' else None
    if command == 'observe_begin':
        value = {key: item for key, item in observation.items() if key not in ('sources', 'writes')}
        value.update(source_count=str(len(observation['sources'])), write_count=str(len(observation['writes'])))
        if behavior == 'bad_count': value['source_count'] = '01'
        if behavior == 'missing_row': value['source_count'] = '2'
    if command in ('observe_source', 'observe_write'):
        rows = observation['sources' if command == 'observe_source' else 'writes']
        if int(request['index']) >= len(rows): sys.exit(0)
        value = rows[int(request['index'])]
        if behavior == 'bad_row': value['text'] = [True]
        if behavior == 'wrong_row_kind': kind = 'observation_write'
    if behavior == 'empty' and command == 'observe': value['sources'] = []
    if behavior == 'malformed_source' and command == 'observe': value['sources'][0]['text'] = [True]
    if behavior == 'bad_checkpoint' and command == 'checkpoint': value = None
    if behavior == 'bad_ack' and command == 'prepare': value = {}
    if behavior == 'bad_envelope' and command == 'prepare': print('null', flush=True); continue
    row = {'version':request['version'], 'id':request['id'], 'kind':kind, 'value':value}
    if behavior == 'identity' and command == 'execute': row['id'] = '1'
    if behavior == 'failure' and command == 'execute': row['kind'] = 'failure'; row['value'] = 'actual missing capability'
    print(json.dumps(row), flush=True)
    if behavior == 'extra' and command == 'release': print(json.dumps(row), flush=True)
''' % (json.dumps(OBSERVATION), json.dumps(GO_CHECKPOINT), behavior))
        path.chmod(0o755)
        return path

    def test_checkpoint_precedes_observation_and_counts_are_not_timing_acceptance(self):
        with patch.object(probe, 'resident', return_value=12345):
            result = probe.capture(self.binary('normal'), {'case': 'tiny'}, self.root / 'result', 3)
        self.assertTrue(result['complete'])
        self.assertFalse(result['performance_acceptance'])
        self.assertEqual([row['command'] for row in result['commands']],
                         ['prepare', 'execute', 'checkpoint', 'observe', 'release'])
        self.assertEqual(result['retained_process_rss_before_observation'], 12345)
        self.assertGreater(result['entire_process_peak_rss_bytes'], 0)
        persisted = json.loads((self.root / 'result/record.json').read_bytes())
        self.assertNotIn('observation', persisted)
        pointer = persisted['observation_record']
        response = (self.root / 'result' / pointer['file']).read_bytes()
        self.assertEqual(probe.decode(response)['value'], result['observation'])
        self.assertEqual(probe.hashlib.sha256(response).hexdigest(), pointer['response_sha256'])
        self.assertEqual(probe.read_observation(self.root / 'result'), result['observation'])
        (self.root / 'result' / pointer['file']).write_bytes(response.replace(b'compiled', b'different'))
        with self.assertRaisesRegex(ValueError, 'response changed'):
            probe.read_observation(self.root / 'result')

    def test_failures_preserve_commands_and_cannot_report_completed_work(self):
        for kind in ('eof', 'identity', 'failure', 'extra', 'empty', 'malformed_source',
                     'bad_checkpoint', 'bad_ack', 'bad_envelope'):
            with self.subTest(kind=kind), patch.object(probe, 'resident', return_value=1):
                directory = self.root / kind
                with self.assertRaises((ValueError, EOFError)):
                    probe.capture(self.binary(kind), {}, directory, 3)
                record = json.loads((directory / 'record.json').read_bytes())
                self.assertFalse(record['complete'])
                self.assertIn('error', record)
                self.assertTrue(record['commands'])

    def test_timeout_kills_child_and_preserves_partial_record(self):
        with self.assertRaises(TimeoutError):
            probe.capture(self.binary('timeout'), {}, self.root / 'timeout', 0.5)
        record = json.loads((self.root / 'timeout/record.json').read_bytes())
        self.assertIn('execute', record['error'])
        self.assertFalse(record['complete'])

    def test_cleanup_falls_back_to_direct_child_when_group_signal_is_refused(self):
        with patch.object(probe.os, 'killpg', side_effect=PermissionError('group gone')):
            with self.assertRaises(TimeoutError):
                probe.capture(self.binary('timeout'), {}, self.root / 'cleanup', 0.5)
        record = json.loads((self.root / 'cleanup/record.json').read_bytes())
        self.assertIn('execute', record['error'])
        self.assertNotIn('cleanup_error', record)

    def test_protocol_preserves_order_and_rejects_duplicates_or_non_json_numbers(self):
        value = {'paths': {'z': ['z'], 'a': ['a']}}
        self.assertEqual(list(probe.decode(probe.encoded(value))['paths']), ['z', 'a'])
        for raw in (b'{"a":1,"a":2}', b'{"a":NaN}', b'{"a":Infinity}',
                    b'{"a":1e400}', b'{"a":[-1e400]}'):
            with self.assertRaises(ValueError):
                probe.decode(raw)
        self.assertEqual(probe.decode(b'{"a":1.25}'), {'a': 1.25})

    def test_comparison_keeps_ordered_paths_diagnostics_and_emit_bytes(self):
        same = copy.deepcopy(OBSERVATION)
        same['types'] = '23'
        self.assertEqual(probe.comparison(OBSERVATION, same), [])
        for key, value in [('options_json', list(b'{"paths":{"a":["a"],"z":["z"]}}')),
                           ('diagnostics', [{'code': 2304}]), ('writes', [{'text': [50]}]),
                           ('sources', [{'text': [50]}])]:
            changed = copy.deepcopy(OBSERVATION)
            changed[key] = value
            self.assertEqual(probe.comparison(OBSERVATION, changed), ['options' if key == 'options_json' else key])

    def test_empty_work_zero_types_wrong_checker_count_never_pass(self):
        for key, value in [('sources', []), ('roots', []), ('checkers', '2'),
                           ('types', '0'), ('types', '-1'), ('types', '020'),
                           ('types', '²'), ('outcome', 'config_failure'), ('nodes', '0')]:
            changed = copy.deepcopy(OBSERVATION)
            changed[key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                probe.validate_observation(changed)

    def test_options_keep_json_kinds_and_only_paths_object_order(self):
        def compare(left, right):
            go, rust = copy.deepcopy(OBSERVATION), copy.deepcopy(OBSERVATION)
            go['options_json'], rust['options_json'] = list(left), list(right)
            probe.validate_observation(go)
            probe.validate_observation(rust)
            return probe.comparison(go, rust)

        for left, right in [(b'{"strict":true}', b'{"strict":1}'),
                            (b'{"nested":[false]}', b'{"nested":[0]}'),
                            (b'{"paths":{"a":["x"]}}', b'{"paths":[["a",["x"]]]}')]:
            with self.subTest(left=left, right=right):
                self.assertEqual(compare(left, right), ['options'])
        self.assertEqual(compare(b'{"a":true,"b":{"z":1,"y":2}}',
                                 b'{"b":{"y":2,"z":1},"a":true}'), [])

    def test_observation_schema_bytes_indices_and_recursive_diagnostics_are_checked(self):
        valid = copy.deepcopy(OBSERVATION)
        valid['diagnostics'] = [copy.deepcopy(DIAGNOSTIC)]
        valid['diagnostics'][0]['related'] = [copy.deepcopy(DIAGNOSTIC)]
        probe.validate_observation(valid, 'check')
        mutations = [
            lambda value: value.update(extra=0),
            lambda value: value.pop('identifiers'),
            lambda value: value.update(identifiers='-1'),
            lambda value: value.update(sources='not an array'),
            lambda value: value.update(roots=[None]),
            lambda value: value.update(options_json=list(b'[]')),
            lambda value: value.update(options_json=list(b'{"target":1e400}')),
            lambda value: value['sources'][0].update(text=[None]),
            lambda value: value['sources'][0].update(default_library=1),
            lambda value: value['diagnostics'][0].update(file=True),
            lambda value: value['diagnostics'][0].update(file=1),
            lambda value: value['diagnostics'][0].update(pos='-0'),
            lambda value: value['diagnostics'][0].update(code=True),
            lambda value: value['diagnostics'][0].update(category=4),
            lambda value: value['diagnostics'][0].update(message_args=[[256]]),
            lambda value: value['diagnostics'][0]['related'][0].update(chain=None),
            lambda value: value['diagnostics'][0].update(repopulate={}),
            lambda value: value.update(writes=[{'name': [], 'text': []}]),
            lambda value: value.update(emitted_files=['not bytes']),
            lambda value: value.update(emit_skipped=0),
        ]
        for mutate in mutations:
            changed = copy.deepcopy(valid)
            mutate(changed)
            with self.assertRaises(ValueError):
                probe.validate_observation(changed)
        with self.assertRaises(ValueError):
            probe.validate_observation(valid, 'check_emit')
        valid['emit_skipped'] = True
        probe.validate_observation(valid, 'check_emit')
        with self.assertRaises(ValueError):
            probe.validate_observation(valid, 'check')

    def test_checkpoint_authorities_and_metric_types_are_not_interchangeable(self):
        probe.validate_checkpoint(GO_CHECKPOINT, 'go')
        rust = {'authority': 'rust_storage_owners_not_heap_bytes',
                'allocation_instrumented': 'false', 'tracked_owners': '2', 'tracked_allocations': '3'}
        probe.validate_checkpoint(rust, 'rust')
        probe.validate_checkpoint(dict(rust, allocator='mimalloc-0.1.48'), 'rust')
        peak = {'process_peak_rss_before_observation_bytes': '200',
                'process_peak_rss_authority': 'getrusage(RUSAGE_SELF).ru_maxrss'}
        probe.validate_checkpoint(dict(rust, **peak), 'rust')
        probe.validate_checkpoint(dict(GO_CHECKPOINT, **peak), 'go')
        for backend, checkpoint in [('rust', GO_CHECKPOINT), ('go', rust),
                                    ('go', dict(GO_CHECKPOINT, heap_alloc='01')),
                                    ('go', dict(GO_CHECKPOINT, mallocs=True)),
                                    ('rust', dict(rust, authority='heap_bytes')),
                                    ('rust', dict(rust, allocator='unknown')),
                                    ('rust', dict(rust, process_peak_rss_before_observation_bytes='200')),
                                    ('go', dict(GO_CHECKPOINT, **dict(peak, process_peak_rss_before_observation_bytes='0'))),
                                    ('rust', dict(rust, allocation_instrumented='true'))]:
            with self.assertRaises(ValueError):
                probe.validate_checkpoint(checkpoint, backend)

    def test_paged_observation_compares_complete_bytes_and_authenticates_archive(self):
        directory = self.root / 'paged'
        with patch.object(probe, 'resident', return_value=1):
            result = probe.capture(self.binary('normal'), {}, directory, 3, protocol_version=2)
        self.assertTrue(result['complete'])
        self.assertEqual(probe.comparison(OBSERVATION, result['observation']), [])
        self.assertIsInstance(result['observation']['sources'][0]['text'], bytes)
        self.assertEqual([row['command'] for row in result['commands']],
                         ['prepare', 'execute', 'checkpoint', 'observe_begin', 'observe_source', 'release'])
        self.assertEqual(probe.read_observation(directory), result['observation'])
        archive = directory / 'responses.ndjson.gz'
        raw = gzip.decompress(archive.read_bytes())
        archive.write_bytes(gzip.compress(raw.replace(b'"text": [49]', b'"text": [50]')))
        self.assertNotEqual(gzip.decompress(archive.read_bytes()), raw)
        with self.assertRaisesRegex(ValueError, 'responses changed'):
            probe.read_observation(directory)

    def test_invalid_or_missing_paged_rows_never_complete(self):
        for behavior in ('bad_count', 'missing_row', 'bad_row', 'wrong_row_kind'):
            with self.subTest(behavior=behavior), patch.object(probe, 'resident', return_value=1):
                directory = self.root / behavior
                with self.assertRaises((ValueError, EOFError)):
                    probe.capture(self.binary(behavior), {}, directory, 3, protocol_version=2)
                self.assertFalse(probe.decode((directory / 'record.json').read_bytes())['complete'])

    def test_wasm_capacity_process_and_runtime_metrics_keep_their_own_authority(self):
        memory = dict.fromkeys('linear_memory_bytes process_rss_bytes v8_heap_used_bytes external_bytes array_buffer_bytes'.split(), '100')
        rust = dict(memory, authority='production-wasm-node')
        go = dict(memory, authority='go-wasm-node', go_runtime=GO_CHECKPOINT)
        probe.validate_checkpoint(rust, 'rust-wasm')
        probe.validate_checkpoint(go, 'go-wasm')
        collection = {'process_peak_rss_bytes': '300', 'gc_policy': 'v8_after_prepare_and_at_checkpoint'}
        probe.validate_checkpoint(dict(rust, **collection), 'rust-wasm')
        probe.validate_checkpoint(dict(go, **collection), 'go-wasm')
        for value, backend in [(rust, 'rust'), (go, 'go'), (rust, 'go-wasm'), (go, 'rust-wasm'),
                               (dict(rust, linear_memory_bytes='0'), 'rust-wasm'),
                               (dict(rust, gc_policy=collection['gc_policy']), 'rust-wasm'),
                               (dict(go, **dict(collection, process_peak_rss_bytes='0')), 'go-wasm'),
                               (dict(go, go_runtime={}), 'go-wasm')]:
            with self.assertRaises(ValueError):
                probe.validate_checkpoint(value, backend)


if __name__ == '__main__':
    unittest.main()
