"""Separate allocation-capture controls; no compiler builds or benchmarks."""
import copy
import gzip
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.phase7.budget import allocation, measure, probe
from scripts.tests.test_phase7_budget import OBSERVATION, GO_CHECKPOINT
from scripts.tests.test_phase7_budget_measure import Archive


class AllocationControls(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.archive = Archive()
        self.reference_dir = self.root / 'ordinary'
        self.reference_dir.mkdir()
        self.reference_path = self.reference_dir / 'measurement.json'
        self.reference_path.write_bytes(b'{}')
        self.observations = {engine: copy.deepcopy(OBSERVATION) for engine in ('go', 'rust')}
        self.binding = {'selection': {'scenario': 'xstate', 'mode': 'check'},
                        'descriptor': {'work': {'check': {'files': 1, 'diagnostics': 0}}}}
        self.identities = self.make_identities()
        ordinary_build = self.root / 'ordinary-rust-build.json'
        probe.write(ordinary_build, {'sources': {'compiled.rs': 'same-source'}})
        (self.reference_dir / 'rust-build.json').write_bytes(ordinary_build.read_bytes())
        self.reference = {'pin': self.identities['rust']['pin'], 'scenario_acceptance_binding': self.binding,
                          'artifacts': {'go': self.identities['go'],
                                        'rust': {'manifest': probe.identity(ordinary_build)}}}
        self.calls = []
        self.reference_patch = patch.object(allocation, 'load_ordinary_reference', side_effect=self.load_reference)
        self.reference_patch.start()
        self.capture_patch = patch.object(probe, 'capture', side_effect=self.fake_capture)
        self.capture = self.capture_patch.start()
        self.addCleanup(self.reference_patch.stop)
        self.addCleanup(self.capture_patch.stop)

    def load_reference(self, *args):
        return self.reference, self.archive, self.observations, probe.identity(self.reference_path)

    def make_identities(self):
        pin = probe.decode((measure.ROOT / 'data/upstream.json').read_bytes())['pin']
        result = {}
        for engine in ('go', 'rust'):
            binary = self.root / engine
            binary.write_bytes(engine.encode())
            asset = probe.identity(binary)
            build = self.root / (engine + '-build.json')
            if engine == 'go':
                sources = {'data/upstream.json': probe.identity(measure.ROOT / 'data/upstream.json')['sha256']}
                for name in ('operation.go', 'stream.go'):
                    relative = 'tools/phase7/budget/go/body/' + name
                    sources[relative] = probe.identity(measure.ROOT / relative)['sha256']
                manifest = dict(mode='phase7-budget', measurement_contract=2, performance_eligible=True,
                                source_guard=True, build_exit=0, sources=sources, pin=pin,
                                go_gc_policy=measure.GO_GC_POLICY, artifacts={'budget-go': asset})
                probe.write(build, manifest)
                result[engine] = measure.build_identity('native', engine, binary, build)
            else:
                manifest = dict(mode='phase7-budget', measurement_contract=2, performance_eligible=False,
                                source_guard=True, source_changes_during_build=[], build_exit=0,
                                sources={'compiled.rs': 'same-source'}, pin=pin,
                                allocation_instrumented=True, allocator=measure.ALLOCATOR,
                                profile={'name': 'release', 'panic': 'unwind', 'lto': 'fat', 'codegen_units': 1},
                                build_command=['cargo', 'build', '--release', '--manifest-path', 'tools/phase7/budget/rust/Cargo.toml',
                                               '--features', 'allocation-metrics'], size=asset['bytes'], sha256=asset['sha256'])
                probe.write(build, manifest)
                result[engine] = allocation.instrumented_identity(binary, build)
        return result

    def fake_capture(self, binary, input_value, output, timeout, backend, *, preparation, protocol_version):
        self.assertEqual(protocol_version, 2)
        self.assertEqual([request['command'] for request, _ in preparation], ['prepare_begin', 'prepare_end'])
        self.calls.append(backend)
        output.mkdir()
        checkpoint = dict(GO_CHECKPOINT, heap_alloc='100', total_alloc='1000') if backend == 'go' else dict(
            authority='rust_storage_owners_not_heap_bytes', allocator=measure.ALLOCATOR,
            allocation_instrumented='true', tracked_owners='5', tracked_allocations='10',
            live_requested_bytes='60', total_requested_bytes='800', peak_requested_bytes='90',
            allocation_events='100', deallocation_events='50')
        observation = copy.deepcopy(OBSERVATION)
        header = {key: value for key, value in observation.items() if key not in ('sources', 'writes')}
        header.update(source_count='1', write_count='0')
        def response(kind, value=None):
            return probe.encoded({'version': 2, 'id': '0', 'kind': kind, 'value': value})
        observed = response('observation_begin', header) + response('observation_source', observation['sources'][0])
        with gzip.open(output / 'responses.ndjson.gz', 'wb') as archive:
            for kind in ('preparing', 'prepared', 'executed'):
                archive.write(response(kind))
            archive.write(response('checkpoint', checkpoint))
            archive.write(observed)
            archive.write(response('released'))
        (output / 'stderr').write_bytes(b'')
        commands = [{'command': name, 'round_trip_ns': 999999}
                    for name in ('prepare_begin', 'prepare_end', 'execute', 'checkpoint',
                                 'observe_begin', 'observe_source', 'release')]
        result = dict(complete=True, exit=0, binary=probe.identity(binary), checkpoint=checkpoint,
                      commands=commands, observation=observation)
        persisted = dict(result)
        persisted.pop('observation')
        persisted['observation_record'] = dict(format='rows-v2', file='responses.ndjson.gz', row=4, count=2,
                                               responses_sha256=hashlib.sha256(observed).hexdigest())
        probe.write(output / 'record.json', persisted)
        return result

    def run_capture(self):
        return allocation.run(self.reference_dir, self.identities, self.root / 'output', 10)

    def read_capture(self):
        return allocation.read_capture(self.root / 'output', 'xstate', 'check', ordinary_reference=self.reference_dir)

    def test_seven_fresh_alternating_pairs_separate_authorities_and_readonly_replay(self):
        report = self.run_capture()
        self.assertTrue(report['complete'], report['failures'])
        self.assertTrue(report['allocation_eligible'])
        self.assertFalse(report['performance_acceptance'])
        self.assertEqual(self.calls, ['go', 'rust', 'rust', 'go'] * 3 + ['go', 'rust'])
        self.assertEqual(report['summary']['retained_bytes']['ratio'], 0.6)
        self.assertEqual(report['summary']['total_allocation_traffic_bytes']['ratio'], 0.8)
        self.assertNotIn('elapsed_rpc_ns', report)
        self.assertNotIn('peak_rss', report)
        row = self.read_capture()
        self.assertEqual(row['ratio'], 0.6)
        self.assertEqual(row['samples'], {'go': [100] * 7, 'rust': [60] * 7})
        self.assertIn('rounded GC heap', row['authorities']['go'])
        self.assertIn('requested layout', row['authorities']['rust'])
        self.assertEqual(len(self.calls), 14, 'readonly replay must not execute children')

    def test_corrupt_observation_or_missing_owner_metrics_retains_failed_capture(self):
        def bad(*args, **kwargs):
            result = self.fake_capture(*args, **kwargs)
            if kwargs['backend'] == 'rust':
                result['observation']['sources'][0]['text'] = [50]
            return result
        self.capture.side_effect = bad
        report = self.run_capture()
        self.assertFalse(report['complete'])
        self.assertIn('sources', report['failures'][0])
        self.assertTrue((self.root / 'output/pair-00-rust/record.json').exists())
        self.assertNotIn('summary', report)
        checkpoint = self.fake_capture(self.root / 'rust', {}, self.root / 'metrics', 10, 'rust',
                                       preparation=self.archive.preparation(), protocol_version=2)['checkpoint']
        for key in ('live_requested_bytes', 'tracked_owners', 'tracked_allocations'):
            invalid = dict(checkpoint, **{key: '0'})
            with self.subTest(key=key), self.assertRaises(ValueError):
                allocation.metrics(invalid, 'rust')
        with self.assertRaises(ValueError):
            allocation.metrics(dict(checkpoint, allocation_instrumented='false'), 'rust')

    def test_reference_source_mismatch_or_ordinary_rust_artifact_is_rejected(self):
        self.identities['rust']['sources'] = {'compiled.rs': 'different'}
        with self.assertRaisesRegex(ValueError, 'source/pin'):
            self.run_capture()
        manifest = probe.decode((self.root / 'rust-build.json').read_bytes())
        manifest['allocation_instrumented'] = False
        probe.write(self.root / 'rust-build.json', manifest)
        with self.assertRaisesRegex(ValueError, 'instrumented mimalloc'):
            allocation.instrumented_identity(self.root / 'rust', self.root / 'rust-build.json')

    def test_readonly_replay_rejects_metric_tampering_and_missing_pairs(self):
        report = self.run_capture()
        original = copy.deepcopy(report)
        report['pairs'][0]['rust']['metrics']['retained_bytes'] = 1
        probe.write(self.root / 'output/allocation.json', report)
        with self.assertRaisesRegex(ValueError, 'raw checkpoint'):
            self.read_capture()
        original['pairs'].pop()
        probe.write(self.root / 'output/allocation.json', original)
        with self.assertRaisesRegex(ValueError, 'seven complete'):
            self.read_capture()

    def test_readonly_replay_uses_saved_go_contract_without_current_source_discovery(self):
        self.assertTrue(self.run_capture()['complete'])
        with patch.object(measure, 'build_identity', side_effect=AssertionError('current source discovery')):
            self.assertEqual(self.read_capture()['ratio'], 0.6)

    def test_source_pairing_uses_archived_ordinary_rust_manifest(self):
        # Both capture and replay must survive deletion of the producer's
        # original metadata after the ordinary capture archived its exact bytes.
        Path(self.reference['artifacts']['rust']['manifest']['path']).unlink()
        self.assertTrue(self.run_capture()['complete'])
        self.assertEqual(self.read_capture()['ratio'], 0.6)
        (self.reference_dir / 'rust-build.json').write_text('{}')
        with self.assertRaisesRegex(ValueError, 'archived ordinary Rust'):
            self.read_capture()

    def test_checking_work_cannot_shrink_with_unchanged_owned_bytes(self):
        def bad(*args, **kwargs):
            result = self.fake_capture(*args, **kwargs)
            if kwargs['backend'] == 'rust':
                result['observation']['types'] = '1'
            return result
        self.capture.side_effect = bad
        report = self.run_capture()
        self.assertFalse(report['complete'])
        self.assertIn('checking work', report['failures'][0])

    def test_busy_ordinary_reference_is_authenticated_without_timing_qualification(self):
        self.reference_patch.stop()
        report = {'scenario_acceptance_binding': self.binding,
                  'archive': {'metadata': 'metadata', 'contents': 'contents'}}
        probe.write(self.reference_path, report)
        with patch.object(measure, 'read_capture', return_value={'metadata': {'measurement_qualified': False}}) as read, \
             patch.object(allocation.stream, 'Archive', return_value=self.archive), \
             patch.object(probe, 'read_observation', return_value=OBSERVATION):
            allocation.load_ordinary_reference(self.reference_dir)
        read.assert_called_once_with(self.reference_dir, 'xstate', 'check', 'native', require_qualification=False)

    def test_output_must_stay_outside_frozen_archives_and_source_workload(self):
        protected = self.root / 'protected'
        protected.mkdir()
        for kind in ('inventory', 'contents', 'workload'):
            self.archive.inventory = None
            self.archive.contents = None
            self.archive.input.pop('current_directory', None)
            if kind == 'workload':
                self.archive.input['current_directory'] = list(bytes(str(protected), 'utf-8'))
            else:
                setattr(self.archive, kind, protected)
            with self.subTest(kind=kind), self.assertRaisesRegex(ValueError, 'outside the source'):
                allocation.run(self.reference_dir, self.identities, protected / kind, 10)
        self.assertFalse(self.calls)


if __name__ == '__main__':
    unittest.main()
