"""Measurement scheduling/eligibility controls; no compiler or benchmark runs."""
import copy
import hashlib
import gzip
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.phase7.budget import measure, probe, snapshot, stream
from scripts.tests.test_phase7_budget import OBSERVATION, GO_CHECKPOINT


class Archive:
    def __init__(self):
        self.input = {'operation': 'check'}
        self.input_bytes = b'{"operation":"check"}\n'
        self.identity = {'input_sha256': hashlib.sha256(self.input_bytes).hexdigest()}
        self.checks = 0
        self.changed = False
        self.content_rows = []
        self.contents = None

    def unchanged(self):
        self.checks += 1
        if self.changed:
            raise ValueError('snapshot archive changed during replay')

    def preparation(self):
        self.unchanged()
        yield ({'command': 'prepare_begin'}, 'preparing')
        yield ({'command': 'prepare_end'}, 'prepared')


def captured(backend, elapsed=100):
    observation = copy.deepcopy(OBSERVATION)
    observation['sources'][0]['text'] = b'1'
    observation['types'] = '30' if backend.startswith('rust') else '20'
    if backend == 'go':
        checkpoint = dict(GO_CHECKPOINT,
                          process_peak_rss_before_observation_bytes='1000',
                          process_peak_rss_authority=measure.PEAK_AUTHORITY)
    elif backend == 'rust':
        checkpoint = dict(authority='rust_storage_owners_not_heap_bytes',
                          allocation_instrumented='false', allocator=measure.ALLOCATOR,
                          tracked_owners='3', tracked_allocations='4',
                          process_peak_rss_before_observation_bytes='900',
                          process_peak_rss_authority=measure.PEAK_AUTHORITY)
    else:
        checkpoint = dict(authority='go-wasm-node' if backend == 'go-wasm' else 'production-wasm-node',
                          linear_memory_bytes='1000', process_rss_bytes='800', v8_heap_used_bytes='50',
                          external_bytes='10', array_buffer_bytes='5', process_peak_rss_bytes='1000',
                          gc_policy=measure.GC_POLICY)
        if backend == 'go-wasm':
            checkpoint['go_runtime'] = GO_CHECKPOINT
    return {'complete': True, 'exit': 0, 'observation': observation, 'checkpoint': checkpoint,
            'retained_process_rss_before_observation': 800, 'entire_process_peak_rss_bytes': 2000,
            'commands': [{'command': name, 'round_trip_ns': elapsed if name == 'execute' else 10**9}
                         for name in ('prepare_begin', 'prepare_end', 'execute', 'checkpoint',
                                      'observe_begin', 'observe_source', 'release')]}


class MeasurementControls(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.archive = Archive()
        self.calls = []
        self.identities = {name: self.native_identity(name) for name in ('go', 'rust')}
        self.start_patch('host_info', return_value={'cpu_capacity': 1})
        self.start_patch('reject_concurrent_builds')
        self.cpu = self.start_patch('other_cpu', return_value={'percent': 0, 'busiest': []})
        self.capture = patch.object(probe, 'capture', side_effect=self.fake_capture).start()
        self.addCleanup(patch.stopall)

    def start_patch(self, name, **kwargs):
        item = patch.object(measure, name, **kwargs)
        result = item.start()
        self.addCleanup(item.stop)
        return result

    def native_identity(self, engine):
        binary = self.root / engine
        binary.write_bytes(engine.encode())
        digest = probe.identity(binary)
        if engine == 'go':
            sources = {'tools/phase7/budget/go/body/' + name:
                       probe.identity(measure.ROOT / 'tools/phase7/budget/go/body' / name)['sha256']
                       for name in ('operation.go', 'stream.go')}
            sources['data/upstream.json'] = probe.identity(measure.ROOT / 'data/upstream.json')['sha256']
            manifest = dict(mode='phase7-budget-prototype', performance_eligible=False,
                            pin=probe.decode((measure.ROOT / 'data/upstream.json').read_bytes())['pin'],
                            sources=sources, artifacts={'budget-go': digest})
        else:
            manifest = dict(profile={'name': 'release', 'panic': 'unwind', 'lto': 'fat', 'codegen_units': 1},
                            allocator=measure.ALLOCATOR, allocation_instrumented=False,
                            source_guard=True, source_changes_during_build=[], build_exit=0,
                            build_command=['cargo', 'build', '--manifest-path', 'tools/phase7/budget/rust/Cargo.toml'],
                            executable=str(binary), size=digest['bytes'], sha256=digest['sha256'])
        build = self.root / (engine + '.json')
        manifest['revision'] = 'a' * 40
        probe.write(build, manifest)
        return measure.build_identity('native', engine, binary, build)

    def fake_capture(self, binary, input_value, output, timeout, backend, *, preparation, protocol_version):
        self.assertEqual(protocol_version, 2)
        prepared = [request['command'] for request, _ in preparation]
        self.assertEqual((prepared[0], prepared[-1]), ('prepare_begin', 'prepare_end'))
        self.assertNotIn('execute', prepared)
        self.calls.append((backend, output.name))
        output.mkdir()
        result = captured(backend, 100 if backend.startswith('go') else 50)
        observation = result['observation']
        if 'current_directory' in input_value:
            name = input_value['current_directory'] + list(b'/a.ts')
            observation['roots'] = [name]
            observation['sources'][0].update(name=name, path=name)
            count = getattr(self, 'expected_source_files', 1)
            observation['sources'] = [dict(observation['sources'][0], name=name if i == 0 else input_value['current_directory'] + list(f'/dependency-{i}.ts'.encode()),
                                           path=name if i == 0 else input_value['current_directory'] + list(f'/dependency-{i}.ts'.encode()))
                                      for i in range(count)]
            observation.update(nodes=str(10 * count), identifiers=str(count))
        header = {key: value for key, value in observation.items() if key not in ('sources', 'writes')}
        header.update(source_count=str(len(observation['sources'])), write_count='0')
        def response(kind, value=None):
            return probe.encoded({'version': 2, 'id': '0', 'kind': kind, 'value': value})
        observed = response('observation_begin', header) + b''.join(
            response('observation_source', dict(source, text=list(source['text']))) for source in observation['sources'])
        with gzip.open(output / 'responses.ndjson.gz', 'wb') as archive:
            for kind in ('preparing', 'prepared', 'executed'):
                archive.write(response(kind))
            archive.write(response('checkpoint', result['checkpoint']))
            archive.write(observed)
            archive.write(response('released'))
        (output / 'stderr').write_bytes(b'')
        result['commands'] = result['commands'][:-2] + [dict(command='observe_source', round_trip_ns=1)
                              for _ in observation['sources']] + [result['commands'][-1]]
        result['binary'] = probe.identity(binary)
        persisted = dict(result)
        persisted.pop('observation')
        persisted['observation_record'] = {'format': 'rows-v2', 'file': 'responses.ndjson.gz', 'row': 4,
                                          'count': 1 + len(observation['sources']), 'responses_sha256': hashlib.sha256(observed).hexdigest()}
        probe.write(output / 'record.json', persisted)
        return result

    def run_capture(self, **kwargs):
        return measure.run(self.archive, self.identities, self.root / 'output', 10, **kwargs)

    def test_output_cannot_mutate_workload_or_frozen_archives(self):
        workload = self.root / 'workload'
        inventory = self.root / 'inventory'
        contents = self.root / 'contents'
        for path in (workload, inventory, contents):
            path.mkdir()
        self.archive.input['current_directory'] = list(bytes(workload))
        self.archive.inventory = inventory
        self.archive.contents = contents
        alias = self.root / 'workload-alias'
        alias.symlink_to(workload, target_is_directory=True)
        for parent in (workload, inventory, contents, alias):
            output = parent / 'new-capture'
            with self.subTest(parent=parent), self.assertRaisesRegex(ValueError, 'outside the source workload'):
                measure.run(self.archive, self.identities, output, 10)
            self.assertFalse(output.exists())
        self.assertEqual(self.calls, [])
        measure.require_output(self.archive, self.root / 'safe-new-capture')

    def test_serial_alternation_warmup_discarded_rpc_only_and_prototypes_ineligible(self):
        result = self.run_capture()
        self.assertTrue(result['complete'], result['failures'])
        self.assertFalse(result['qualifies'])
        self.assertFalse(result['performance_acceptance'])
        self.assertEqual(len(result['pairs']), 7)
        self.assertEqual([name for name, _ in self.calls[:6]], ['go', 'rust', 'go', 'rust', 'rust', 'go'])
        self.assertEqual(result['ratios']['elapsed_rpc_ns']['go_median'], 100)
        self.assertEqual(result['ratios']['elapsed_rpc_ns']['rust_median'], 50)
        self.assertEqual(result['ratios']['elapsed_rpc_ns']['samples_per_runtime'], 7)
        self.assertEqual(result['ratios']['checkpoint_process_peak_rss_bytes']['go_median'], 1000)
        self.assertEqual(result['ratios']['entire_lifecycle_peak_rss_bytes']['go_median'], 2000)
        self.assertEqual(len(list((self.root / 'output').glob('*/record.json'))), 16)
        self.assertTrue(all(not pair['go']['qualifies'] for pair in result['pairs']))
        self.assertGreaterEqual(self.archive.checks, 16 * 3)
        # Full owned bytes stay in raw probe archives, not duplicated in summaries.
        disk = probe.decode((self.root / 'output/measurement.json').read_bytes())
        self.assertNotIn('observation', disk['pairs'][0]['go'])
        helper_manifest = probe.decode((self.root / 'output/helper-manifest.json').read_bytes())
        self.assertIn('measure.py', [Path(item['path']).name for item in helper_manifest])
        for item in helper_manifest:
            self.assertEqual(probe.identity(self.root / 'output' / item['archived'])['sha256'], item['sha256'])
        self.assertIsNone(result['startup_metric'])
        self.assertIsNone(result['scenario_acceptance_binding'])
        self.assertIn('performance_eligible=false', ' '.join(disk['eligibility_reasons']))

    def test_fixed_s07_extension_only_once_and_no_busy_row_replacement(self):
        real = measure.ratio_summary

        def extend(go, rust, **kwargs):
            result = real(go, rust, **kwargs)
            if kwargs.get('timing'):
                result['needs_more'] = len(go) == 7
            return result

        self.cpu.return_value = {'percent': measure.BUSY_OTHER_CPU + 1, 'busiest': [['busy', 201]]}
        with patch.object(measure, 'ratio_summary', side_effect=extend):
            result = self.run_capture()
        self.assertTrue(result['complete'], result['failures'])
        self.assertEqual(len(result['pairs']), 14)
        self.assertEqual(len(self.calls), 30)
        self.assertEqual(result['idle_pairs'], 0)
        self.assertEqual(result['valid_pairs'], 14)
        self.assertTrue(result['busy'])
        self.assertTrue(result['sampling']['ratios_include_busy_rows'])
        self.assertEqual(result['ratios']['elapsed_rpc_ns']['samples_per_runtime'], 14)

    def test_one_byte_difference_fails_with_raw_pair_and_partial_report_retained(self):
        def different(*args, **kwargs):
            result = self.fake_capture(*args, **kwargs)
            if args[2].name == 'pair-00-rust':
                result['observation']['sources'][0]['text'] = b'2'
            return result

        self.capture.side_effect = different
        result = self.run_capture()
        self.assertFalse(result['complete'])
        self.assertIn('sources', result['failures'][0])
        self.assertEqual(len(result['pairs']), 1)
        self.assertTrue((self.root / 'output/pair-00-rust/record.json').exists())
        self.assertNotIn('ratios', result)
        self.assertFalse(result['pairs'][0]['rust']['valid'])

    def test_archive_input_and_binary_mutation_fail_closed(self):
        for mutation in ('archive', 'input', 'binary'):
            with self.subTest(mutation=mutation):
                output = self.root / mutation
                self.archive.changed = False
                input_path = self.root / 'input.json'
                input_path.write_bytes(self.archive.input_bytes)

                def mutate(*args, **kwargs):
                    result = self.fake_capture(*args, **kwargs)
                    if mutation == 'archive':
                        self.archive.changed = True
                    elif mutation == 'input':
                        input_path.write_bytes(b'{}')
                    else:
                        (self.root / 'rust').write_bytes(b'changed')
                    return result

                self.capture.side_effect = mutate
                result = measure.run(self.archive, self.identities, output, 10, input_path=input_path)
                self.assertFalse(result['complete'])
                self.assertIn('changed', result['failures'][0])
                self.assertEqual(len(result['pairs']), 0)
                if mutation == 'binary':
                    (self.root / 'rust').write_bytes(b'rust')

    def test_failed_child_retains_unavailable_row_and_previous_pairs(self):
        def fail(*args, **kwargs):
            if args[2].name == 'pair-02-go':
                args[2].mkdir()
                probe.write(args[2] / 'record.json', {'complete': False, 'error': 'consumer exited 137'})
                raise ValueError('consumer exited 137')
            return self.fake_capture(*args, **kwargs)

        self.capture.side_effect = fail
        result = self.run_capture()
        self.assertFalse(result['complete'])
        self.assertEqual(result['status'], 'unavailable')
        self.assertEqual(result['valid_pairs'], 2)
        self.assertEqual(result['pairs'][2]['go']['status'], 'unavailable')
        self.assertFalse(result['pairs'][2]['go']['qualifies'])
        self.assertTrue((self.root / 'output/pair-02-go/record.json').exists())
        self.assertNotIn('ratios', result)

    def test_frozen_blob_change_during_last_sample_is_detected(self):
        self.archive.contents = self.root / 'contents'
        blobdir = self.archive.contents / 'blobs'
        blobdir.mkdir(parents=True)
        sha = hashlib.sha256(b'original').hexdigest()
        blob = blobdir / sha
        blob.write_bytes(b'original')
        self.archive.content_rows = [{'sha256': sha, 'raw_size': '8'}]

        def mutate(*args, **kwargs):
            result = self.fake_capture(*args, **kwargs)
            if args[2].name == 'pair-06-rust':
                blob.write_bytes(b'modified')  # Same size, metadata/receipt unchanged.
            return result

        self.capture.side_effect = mutate
        result = self.run_capture()
        self.assertFalse(result['complete'])
        self.assertEqual(len(result['pairs']), 7)
        self.assertIn('frozen content changed bytes', result['failures'][0])
        self.assertNotIn('ratios', result)

    def test_concurrent_build_rejected_without_timing_child(self):
        with patch.object(measure, 'reject_concurrent_builds', side_effect=ValueError('compiler active')):
            result = self.run_capture()
        self.assertFalse(result['complete'])
        self.assertEqual(self.calls, [])
        self.assertIn('compiler active', result['failures'])

    def test_modern_checkpoint_and_defined_work_required(self):
        cases = []
        old = captured('go'); old['checkpoint'] = GO_CHECKPOINT
        cases.append(('go', old))
        unknown = captured('rust'); del unknown['checkpoint']['allocator']
        cases.append(('rust', unknown))
        gc = captured('rust-wasm'); gc['checkpoint']['gc_policy'] = 'unknown'
        cases.append(('rust-wasm', gc))
        old_node = captured('go-wasm'); del old_node['checkpoint']['gc_policy']; del old_node['checkpoint']['process_peak_rss_bytes']
        cases.append(('go-wasm', old_node))
        zero = captured('go'); zero['retained_process_rss_before_observation'] = 0
        cases.append(('go', zero))
        no_work = captured('go'); no_work['observation']['types'] = '0'
        cases.append(('go', no_work))
        parallel = captured('go'); parallel['observation']['checkers'] = '2'
        cases.append(('go', parallel))
        repeated = captured('go'); repeated['commands'].insert(3, {'command': 'execute', 'round_trip_ns': 1})
        cases.append(('go', repeated))
        bad_peak = captured('go'); bad_peak['entire_process_peak_rss_bytes'] = 500
        cases.append(('go', bad_peak))
        for backend, record in cases:
            with self.subTest(backend=backend, checkpoint=record['checkpoint']):
                with self.assertRaises(ValueError):
                    measure.sample_metrics(record, backend, 'check')

    def test_complete_instrumented_checkpoint_reaches_timing_guard(self):
        record = captured('rust')
        record['checkpoint'].update(allocation_instrumented='true', **dict.fromkeys(
            'live_requested_bytes total_requested_bytes peak_requested_bytes allocation_events deallocation_events '
            'live_usable_bytes total_usable_bytes peak_usable_bytes'.split(), '1'))
        probe.validate_checkpoint(record['checkpoint'], 'rust')
        with self.assertRaisesRegex(ValueError, 'native timing requires uninstrumented known mimalloc'):
            measure.sample_metrics(record, 'rust', 'check')

    def test_artifact_revisions_are_validated_and_paired(self):
        self.assertEqual(measure.paired_revision(self.identities), 'a' * 40)
        self.assertIsNone(measure.artifact_revision({}))
        for value in ('HEAD', 'A' * 40, True, 'a' * 39):
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, 'revision'):
                measure.artifact_revision({'revision': value})
        changed = copy.deepcopy(self.identities)
        changed['rust']['revision'] = 'b' * 40
        self.assertEqual(measure.paired_revision(changed), 'b' * 40)
        changed['rust']['revision'] = None
        self.assertIsNone(measure.paired_revision(changed))
        self.assertEqual(measure.sample_metrics(captured('go-wasm'), 'go-wasm', 'check')['checkpoint_process_peak_rss_bytes'], 1000)

    def test_native_manifest_kind_gc_contract_hash_and_instrumentation(self):
        for engine, field, value in [('go', 'mode', 'corpus'), ('go', 'performance_eligible', True),
                                     ('go', 'sources', {}), ('go', 'pin', '0' * 40), ('rust', 'allocation_instrumented', True),
                                     ('rust', 'build_exit', False),
                                     ('rust', 'allocator', 'system'), ('rust', 'source_guard', False),
                                     ('rust', 'build_command', ['cargo', 'build', '-p', 'testrunner']),
                                     ('rust', 'sha256', '0' * 64)]:
            with self.subTest(engine=engine, field=field):
                build = self.root / (engine + '.json')
                original = build.read_bytes()
                value_manifest = probe.decode(original)
                value_manifest[field] = value
                probe.write(build, value_manifest)
                with self.assertRaises(ValueError):
                    measure.build_identity('native', engine, self.root / engine, build)
                build.write_bytes(original)

    def test_wasm_identity_binds_assets_node_and_helpers_and_rejects_wrong_kind(self):
        directory = self.root / 'wasm'
        directory.mkdir()
        artifacts = {}
        for name in ('checker_bg.wasm', 'bindings.mjs'):
            path = directory / name
            path.write_bytes(name.encode())
            artifacts[name] = probe.identity(path)
        manifest = {'mode': 'checker', 'command': ['cargo', 'build', '--release', '-p', 'tsr_wasm', '--features', 'checker'],
                    'sources': {'compiler': '0' * 64}, 'artifacts': artifacts}
        build = directory / 'build.json'
        probe.write(build, manifest)
        node = self.root / 'node'; node.write_bytes(b'node')
        with patch.object(measure.shutil, 'which', return_value=str(node)), patch.object(measure.subprocess, 'check_output', return_value='v24.20.0\n'):
            result = measure.build_identity('wasm', 'rust', directory, build)
            self.assertEqual(result['environment']['PATH'], os.pathsep.join((str(node.parent), '/usr/bin', '/bin')))
            measure.unchanged(result)
            self.assertIn('bindings.mjs', [Path(item['path']).name for item in result['files']])
            self.assertIn('instance.mjs', [Path(item['path']).name for item in result['files']])
            node.write_bytes(b'changed')
            with self.assertRaises(ValueError):
                measure.unchanged(result)
            node.write_bytes(b'node')
            (directory / 'checker_bg.wasm').write_bytes(b'wrong')
            with self.assertRaises(ValueError):
                measure.build_identity('wasm', 'rust', directory, build)
            manifest['mode'] = 'corpus'; probe.write(build, manifest)
            with self.assertRaises(ValueError):
                measure.build_identity('wasm', 'rust', directory, build)

    def production_fixture(self):
        descriptor = measure.common.load('xstate')
        self.expected_source_files = descriptor['work']['check']['files']
        project = self.root / ('xstate-' + descriptor['source']['commit'][:12])
        project.mkdir()
        (project / 'a.ts').write_bytes(b'1')
        (project / 'tsconfig.json').write_bytes(b'{"files":["a.ts"]}')
        probe.write(project / measure.common.MARKER, {'spec_sha256': measure.common.provision_spec(descriptor)})
        old_cache = measure.scenario_input.common.CACHE
        try:
            request, selection = measure.scenario_input.make('xstate', 'check', project)
        finally:
            measure.scenario_input.common.CACHE = old_cache
        input_path = self.root / 'input.json'; input_path.write_bytes(probe.encoded(request))
        selection_path = self.root / 'selection.json'; probe.write(selection_path, selection)
        inventory = self.root / 'inventory'; snapshot.inventory(project, inventory)
        contents = self.root / 'contents'; (contents / 'blobs').mkdir(parents=True)
        rows = []
        for name in ('a.ts', 'tsconfig.json'):
            path = project / name; data = path.read_bytes(); sha = hashlib.sha256(data).hexdigest()
            (contents / 'blobs' / sha).write_bytes(data)
            rows.append({'path_b64': snapshot.b64(os.fsencode(path)), 'raw_size': str(len(data)), 'sha256': sha})
        (contents / 'contents.ndjson').write_bytes(b''.join(snapshot.encode(row) for row in rows))
        probe.write(contents / 'complete.json', {'version': 1, 'kind': 'frozen_contents',
                    'metadata_sha256': snapshot.digest(inventory / 'metadata.ndjson'),
                    'contents_sha256': snapshot.digest(contents / 'contents.ndjson'),
                    'files': 2, 'raw_bytes': sum(int(row['raw_size']) for row in rows)})
        self.archive = stream.Archive(inventory, contents, input_path)
        binding = measure.bind_selection(self.archive, selection_path)
        for engine in ('go', 'rust'):
            build = self.root / (engine + '.json'); manifest = probe.decode(build.read_bytes())
            manifest.update(mode='phase7-budget', measurement_contract=2, performance_eligible=True,
                            source_guard=True, build_exit=0)
            if engine == 'go':
                manifest['go_gc_policy'] = measure.GO_GC_POLICY
            else:
                manifest['sources'] = {'reviewed-consumer': '0' * 64}
            probe.write(build, manifest)
            self.identities[engine] = measure.build_identity('native', engine, self.root / engine, build)
        return binding, input_path, project

    def test_bind_selection_restores_the_module_cache_on_success_and_failure(self):
        original_cache = measure.scenario_input.common.CACHE
        self.production_fixture()
        self.assertIs(measure.scenario_input.common.CACHE, original_cache)
        def fail(*args):
            measure.scenario_input.common.CACHE = self.root / 'temporary-cache'
            raise ValueError('selection failure')
        with patch.object(measure.scenario_input, 'make', side_effect=fail):
            with self.assertRaisesRegex(ValueError, 'selection failure'):
                measure.bind_selection(self.archive, self.root / 'selection.json')
        self.assertIs(measure.scenario_input.common.CACHE, original_cache)

    def test_production_capture_readonly_raw_replay_and_native_heap_stays_missing(self):
        binding, input_path, project = self.production_fixture()
        result = self.run_capture(selection=binding, input_path=input_path)
        self.assertTrue(result['complete'], result['failures'])
        self.assertTrue(result['qualifies'], result['eligibility_reasons'])
        # Replay cannot consult the original project or spawn a consumer.
        import shutil
        shutil.rmtree(project)
        with patch.object(probe, 'capture', side_effect=AssertionError('must not execute')):
            replay = measure.read_capture(self.root / 'output', 'xstate', 'check', 'native')
        self.assertEqual(replay['ratios'], {'elapsed': 0.5, 'peak_rss': 0.9})
        self.assertEqual(replay['revision'], 'a' * 40)
        self.assertNotIn('retained_rss', replay['ratios'])
        self.assertNotIn('retained_bytes', replay['ratios'])
        self.assertIn('retained_bytes', replay['metadata']['unavailable'])
        self.assertIsNone(replay['metadata']['startup'])
        with self.assertRaises(ValueError):
            measure.read_capture(self.root / 'output', 'vscode', 'check', 'native')
        output = self.root / 'output'; measurement = output / 'measurement.json'; original = measurement.read_bytes()
        data = probe.decode(original); data['warmup']['go']['other_cpu_before']['percent'] = 201; probe.write(measurement, data)
        with self.assertRaisesRegex(ValueError, 'busy host'):
            measure.read_capture(output, 'xstate', 'check', 'native')
        raw_only = measure.read_capture(output, 'xstate', 'check', 'native', require_qualification=False)
        self.assertFalse(raw_only['metadata']['measurement_qualified'])
        self.assertIn('busy host', ' '.join(raw_only['metadata']['eligibility_reasons']))
        measurement.write_bytes(original)
        data = probe.decode(original); data['pairs'][0]['go']['elapsed_rpc_ns'] = 1; probe.write(measurement, data)
        with self.assertRaisesRegex(ValueError, 'saved metric'):
            measure.read_capture(output, 'xstate', 'check', 'native')
        measurement.write_bytes(original)
        raw = output / 'pair-00-go/responses.ndjson.gz'; raw.write_bytes(raw.read_bytes() + b'changed')
        with self.assertRaisesRegex(ValueError, 'raw sample archive changed'):
            measure.read_capture(output, 'xstate', 'check', 'native')

    def test_fourteen_small_mad_straddling_pairs_remain_valid_but_do_not_qualify(self):
        binding, input_path, _ = self.production_fixture()
        rust_times = [980, 980, 990, 990, 1000, 1010, 1020,
                      990, 990, 990, 990, 1010, 1010, 1020]

        def uncertain(*args, **kwargs):
            result = self.fake_capture(*args, **kwargs)
            directory = args[2]
            if directory.name.startswith('pair-'):
                index = int(directory.name.split('-')[1])
                elapsed = 1000 if kwargs['backend'] == 'go' else rust_times[index]
                next(row for row in result['commands'] if row['command'] == 'execute')['round_trip_ns'] = elapsed
                persisted = probe.decode((directory / 'record.json').read_bytes())
                persisted['commands'] = result['commands']
                probe.write(directory / 'record.json', persisted)
            return result

        self.capture.side_effect = uncertain
        result = self.run_capture(selection=binding, input_path=input_path)
        self.assertTrue(result['complete'], result['failures'])
        self.assertEqual(len(result['pairs']), 14)
        self.assertEqual(result['valid_pairs'], 14)
        self.assertFalse(result['qualifies'])
        self.assertFalse(result['performance_acceptance'])
        elapsed = result['ratios']['elapsed_rpc_ns']
        self.assertEqual(elapsed['ratio'], 0.99)
        self.assertLess(elapsed['rust_relative_mad'], 0.05)
        self.assertLess(elapsed['bootstrap']['lower'], 1)
        self.assertGreater(elapsed['bootstrap']['upper'], 1)
        with self.assertRaisesRegex(ValueError, 'inconclusive'):
            measure.read_capture(self.root / 'output', 'xstate', 'check', 'native')
        replay = measure.read_capture(self.root / 'output', 'xstate', 'check', 'native', require_qualification=False)
        self.assertEqual(replay['ratios']['elapsed'], 0.99)
        self.assertFalse(replay['metadata']['measurement_qualified'])
        self.assertIn('inconclusive', ' '.join(replay['metadata']['eligibility_reasons']))

    def test_clear_threshold_miss_is_still_a_qualified_measurement(self):
        summary = measure.ratio_summary([100] * 14, [110] * 14, timing=True, accepted_counts=(7, 14))
        self.assertEqual(summary['ratio'], 1.1)
        self.assertEqual(measure.elapsed_qualification_reasons(summary), [])

    def test_wasm_production_capture_and_readonly_three_metric_adapter(self):
        binding, input_path, _ = self.production_fixture()
        node = self.root / 'node'; node.write_bytes(b'node')
        for engine in ('go', 'rust'):
            directory = self.root / ('wasm-' + engine); directory.mkdir()
            names = ('budget-go.wasm', 'wasm_exec.cjs') if engine == 'go' else ('checker_bg.wasm', 'bindings.mjs')
            artifacts = {}
            for name in names:
                path = directory / name; path.write_bytes(name.encode()); artifacts[name] = probe.identity(path)
            if engine == 'go':
                manifest = probe.decode((self.root / 'go.json').read_bytes()); manifest['artifacts'] = artifacts
            else:
                manifest = {'mode': 'checker', 'sources': {'reviewed-release': '0' * 64},
                            'command': ['cargo', 'build', '--release', '-p', 'tsr_wasm', '--features', 'checker'],
                            'artifacts': artifacts}
            build = directory / 'build.json'; probe.write(build, manifest)
            with patch.object(measure.shutil, 'which', return_value=str(node)), patch.object(measure.subprocess, 'check_output', return_value='v24.20.0\n'):
                self.identities[engine] = measure.build_identity('wasm', engine, directory, build)
        with patch.object(measure.shutil, 'which', return_value=str(node)):
            result = self.run_capture(selection=binding, input_path=input_path)
        self.assertTrue(result['qualifies'], result['failures'])
        replay = measure.read_capture(self.root / 'output', 'xstate', 'check', 'wasm')
        self.assertEqual(replay['ratios'], {'elapsed': 0.5, 'peak_rss': 1.0, 'retained_rss': 1.0})
        self.assertEqual(replay['metadata']['unavailable'], {})
        self.assertEqual(len(replay['samples']['rust']['retained_rss']), 7)

    def test_production_eligibility_not_a_threshold_pass_and_selection_cannot_relabel_input(self):
        binding, input_path, _ = self.production_fixture()
        self.capture.side_effect = self.fake_capture
        result = self.run_capture(selection=binding, input_path=input_path)
        self.assertTrue(result['qualifies'])
        self.assertGreater(result['ratios']['checkpoint_process_peak_rss_bytes']['ratio'], 0.70)
        work = captured('go')['observation']
        with self.assertRaisesRegex(ValueError, 'one-checker warm-up'):
            measure.checking_work(work, None, 100)
        self.assertEqual(measure.checking_work(work, None, 40)['types'], 20)
        wrong = copy.deepcopy(self.archive.input)
        wrong['configuration']['arguments'].append(list(b'--noCheck'))
        with self.assertRaisesRegex(ValueError, 'project/mode/options'):
            measure.validate_selection(binding['selection'], binding['descriptor'], wrong)
        for changes in ({'measurement_contract': True}, {'source_guard': False}, {'go_gc_policy': 'unknown'}):
            manifest = probe.decode((self.root / 'go.json').read_bytes()); manifest.update(changes)
            with self.assertRaises(ValueError):
                measure.require_budget_manifest(manifest, 'go')

    def test_descriptor_oracle_omission_policy_exact_bytes_order_and_only_go(self):
        reference = copy.deepcopy(OBSERVATION)
        reference.update(emit_skipped=False)
        reference['writes'] = [{'name': list(f'/phase7-output/mui-docs/emit/{i:03d}.js'.encode()), 'text': bytes([i])}
                               for i in range(100)]
        reference['emitted_files'] = [row['name'] for row in reference['writes']]
        selection = {'scenario': 'mui-docs', 'mode': 'emit'}
        count, sha = measure.emitted_union(reference, selection)
        binding = {'selection': selection, 'descriptor': {'oracle_emit_varies': True,
                    'work': {'emit': {'files': 1, 'diagnostics': 0, 'emitted': count, 'emitted_sha256': sha}}}}
        subset = copy.deepcopy(reference)
        subset['writes'].pop(50); subset['emitted_files'].pop(50)
        accepted = measure.work_comparison(reference, subset, 'go', binding)
        self.assertEqual(len(accepted['omitted_write_names']), 1)
        self.assertEqual(accepted['differences'], ['emitted_files', 'writes'])
        for engine, candidate, policy in [('rust', subset, binding), ('go', subset, None)]:
            with self.assertRaises(ValueError):
                measure.work_comparison(reference, candidate, engine, policy)
        changed = copy.deepcopy(subset); changed['writes'][0]['text'] = b'changed'
        reordered = copy.deepcopy(subset); reordered['writes'].reverse()
        omitted = copy.deepcopy(subset); omitted['writes'].pop(); omitted['emitted_files'].pop()
        nonemit = copy.deepcopy(subset); nonemit['nodes'] = '11'
        for candidate in (changed, reordered, omitted, nonemit):
            with self.assertRaises(ValueError):
                measure.work_comparison(reference, candidate, 'go', binding)
        wrong_union = copy.deepcopy(binding); wrong_union['descriptor']['work']['emit']['emitted_sha256'] = '0' * 64
        with self.assertRaises(ValueError):
            measure.work_comparison(reference, subset, 'go', wrong_union)

    def test_environment_restored_even_after_failure_and_timeout_finite(self):
        with patch.dict(measure.os.environ, {'TSR_GO_WASM_ARTIFACT_DIR': 'original'}):
            with self.assertRaises(ValueError):
                with measure.launch_environment({'TSR_GO_WASM_ARTIFACT_DIR': 'temporary'}):
                    raise ValueError('failure')
            self.assertEqual(measure.os.environ['TSR_GO_WASM_ARTIFACT_DIR'], 'original')
        for timeout in (0, -1, float('inf'), float('nan')):
            with self.assertRaises(ValueError):
                measure.run(self.archive, self.identities, self.root / 'unused', timeout)


if __name__ == '__main__':
    unittest.main()
