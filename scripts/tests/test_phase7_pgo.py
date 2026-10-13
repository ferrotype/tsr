"""Mocked PGO producer controls; no Rust build, training or benchmark runs."""
import copy
from contextlib import ExitStack
import json
from pathlib import Path
import subprocess
import signal
import tempfile
import unittest
from unittest import mock

from tools.phase7.bench import common, pgo


def messages(executable):
    artifact = {'reason': 'compiler-artifact', 'manifest_path': str(pgo.ROOT / 'crates/tsrust/Cargo.toml'),
                'target': {'name': 'tsrust', 'kind': ['bin'], 'crate_types': ['bin']},
                'features': [], 'filenames': [str(executable)], 'executable': str(executable),
                'profile': {'opt_level': '3', 'debuginfo': 0, 'debug_assertions': False,
                            'overflow_checks': False, 'test': False}}
    return [artifact, {'reason': 'build-finished', 'success': True}]


class InputTests(unittest.TestCase):
    def test_source_identity_reenumerates_new_files_and_hashes_bundled_bytes(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(pgo, 'ROOT', Path(directory)):
            root = Path(directory)
            source = root / 'crates/example/src/a.rs'
            source.parent.mkdir(parents=True)
            source.write_text('first')
            bundled = root / 'crates/tsr_bundled/bundled/libs/lib.d.ts'
            bundled.parent.mkdir(parents=True)
            bundled.write_text('library')
            helper = root / 'scripts/s07_benchmark.py'
            helper.parent.mkdir()
            helper.write_text('helper')
            original = pgo.source_identity()
            self.assertIn('scripts/s07_benchmark.py', original)
            source.with_name('new.rs').write_text('new, even if not tracked')
            self.assertNotEqual(original, pgo.source_identity())
            source.with_name('new.rs').unlink()
            bundled.write_text('LIBRARY')
            self.assertNotEqual(original, pgo.source_identity())
            bundled.write_text('library')
            source.unlink()
            self.assertNotEqual(original, pgo.source_identity())

    def test_environment_rejects_compiler_profile_overrides_but_preserves_homes(self):
        for key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
                    'CARGO_TARGET_X_RUSTFLAGS', 'CARGO_PROFILE_RELEASE_LTO', 'CARGO_INCREMENTAL'):
            with self.subTest(key=key), mock.patch.dict(pgo.os.environ, {key: 'override'}, clear=True):
                with self.assertRaisesRegex(ValueError, key):
                    pgo.environment()
        with mock.patch.dict(pgo.os.environ, {'CARGO_HOME': '/caller/cargo', 'RUSTUP_HOME': '/caller/rustup'}, clear=True):
            self.assertEqual(pgo.environment()['CARGO_HOME'], '/caller/cargo')
            self.assertEqual(pgo.environment()['RUSTUP_HOME'], '/caller/rustup')

    def test_preflight_checks_file_bytes_and_resolved_options(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'a.ts'
            source.write_text('const a = 1;')
            listed = [str(source)]
            options = '{"compilerOptions":{}}\n'
            descriptor = {'name': 'fixture', 'project': '.', 'modes': {'check': ['--noEmit']},
                          'work': {'check': {'listed': 1, 'inputs_sha256': common.inputs_digest(root, listed),
                                             'options_sha256': common.sha256(options.encode())}}}
            probes = [{'exit': 0, 'stdout': '\n'.join(listed) + '\n', 'stderr': ''},
                      {'exit': 0, 'stdout': options, 'stderr': ''}]
            with mock.patch.object(common, 'require_unchanged'), mock.patch.object(common, 'checkout', return_value=root), \
                    mock.patch.object(pgo, 'execute', side_effect=copy.deepcopy(probes)) as execute:
                before = pgo.preflight(root / 'binary', descriptor, 2, {}, root / 'probe')
                self.assertEqual(before['listed_paths'], listed)
                self.assertIn('--listFilesOnly', execute.call_args_list[0].args[0])
                self.assertIn('--showConfig', execute.call_args_list[1].args[0])
            source.write_text('const a = 2;')
            with mock.patch.object(common, 'require_unchanged'), mock.patch.object(common, 'checkout', return_value=root), \
                    mock.patch.object(pgo, 'execute', return_value=probes[0]) as execute:
                with self.assertRaisesRegex(ValueError, 'inputs differ'):
                    pgo.preflight(root / 'binary', descriptor, 2, {}, root / 'probe')
                self.assertEqual(execute.call_count, 1)
            source.write_text('const a = 1;')
            probes[1]['stdout'] = '{"compilerOptions":{"strict":true}}\n'
            with mock.patch.object(common, 'require_unchanged'), mock.patch.object(common, 'checkout', return_value=root), \
                    mock.patch.object(pgo, 'execute', side_effect=probes):
                with self.assertRaisesRegex(ValueError, 'options differ'):
                    pgo.preflight(root / 'binary', descriptor, 2, {}, root / 'probe')

    def test_timeout_keeps_partial_output_and_failed_record(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory) / 'failure'
            error = subprocess.TimeoutExpired(['compiler'], 7, output=b'partial stdout', stderr=b'partial stderr')
            process = mock.Mock(pid=123)
            process.communicate.side_effect = [error, (b'partial stdout', b'partial stderr')]
            with mock.patch.object(pgo.os, 'name', 'posix'), \
                    mock.patch.object(pgo.subprocess, 'Popen', return_value=process) as spawn, \
                    mock.patch.object(pgo.os, 'killpg') as kill, self.assertRaises(subprocess.TimeoutExpired):
                pgo.execute(['compiler'], directory, {}, prefix, timeout=7)
            self.assertTrue(spawn.call_args.kwargs['start_new_session'])
            kill.assert_called_once_with(123, signal.SIGKILL)
            self.assertEqual(process.communicate.call_args_list, [mock.call(timeout=7), mock.call()])
            self.assertEqual(prefix.with_suffix('.stdout').read_text(), 'partial stdout')
            self.assertEqual(prefix.with_suffix('.stderr').read_text(), 'partial stderr')
            record = json.loads(prefix.with_suffix('.json').read_text())
            self.assertIsNone(record['exit'])
            self.assertIn('timed out', record['error'])

    def test_interruption_kills_group_drains_output_and_records_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory) / 'interrupted'
            process = mock.Mock(pid=456)
            events = []
            def communicate(**kwargs):
                events.append('wait' if kwargs else 'drain')
                if kwargs:
                    raise KeyboardInterrupt()
                return 'stdout after interrupt', 'stderr after interrupt'
            process.communicate.side_effect = communicate
            with mock.patch.object(pgo.os, 'name', 'posix'), \
                    mock.patch.object(pgo.subprocess, 'Popen', return_value=process), \
                    mock.patch.object(pgo.os, 'killpg', side_effect=lambda *args: events.append('kill')), \
                    self.assertRaises(KeyboardInterrupt):
                pgo.execute(['cargo'], directory, {}, prefix)
            self.assertEqual(events, ['wait', 'kill', 'drain'])
            self.assertEqual(prefix.with_suffix('.stdout').read_text(), 'stdout after interrupt')
            self.assertEqual(prefix.with_suffix('.stderr').read_text(), 'stderr after interrupt')
            record = json.loads(prefix.with_suffix('.json').read_text())
            self.assertIsNone(record['exit'])
            self.assertEqual(record['error'], 'KeyboardInterrupt')

    def test_artifact_requires_finished_release_cli_and_uses_reported_path(self):
        rows = messages(Path('/actual/redirected/tsrust'))
        self.assertEqual(pgo.artifact('\n'.join(map(json.dumps, rows)))['executable'], '/actual/redirected/tsrust')
        invalid = [rows[:-1], [rows[0], {'reason': 'build-finished', 'success': False}],
                   [rows[0], rows[0], rows[1]]]
        for change in ({'features': ['instrumentation']}, {'profile': {**rows[0]['profile'], 'test': True}},
                       {'executable': 'relative/tsrust'}):
            invalid.append([{**rows[0], **change}, rows[1]])
        for case in invalid:
            with self.subTest(case=case), self.assertRaises(ValueError):
                pgo.artifact('\n'.join(map(json.dumps, case)))


class PipelineTests(unittest.TestCase):
    def setUp(self):
        stack = self.enterContext(ExitStack())
        self.root = Path(stack.enter_context(tempfile.TemporaryDirectory())).resolve()
        self.binary = self.root / 'cargo-selected-cli'
        self.binary.write_bytes(b'exact artifact')
        self.events, self.build_envs = [], []
        self.failure = None
        self.identity = {'source.rs': 'unchanged'}
        self.inputs = {'frozen_inputs_sha256': 'inputs', 'content_sha256': 'bytes',
                       'options_sha256': 'options', 'listed_paths': ['/fixture/a.ts']}
        stack.enter_context(mock.patch.object(pgo, 'environment', return_value={}))
        stack.enter_context(mock.patch.object(pgo, 'toolchain', return_value={
            'channel': 'pinned', 'host': 'native-host', 'profdata': 'llvm-profdata'}))
        stack.enter_context(mock.patch.object(pgo, 'source_identity', side_effect=lambda: dict(self.identity)))
        stack.enter_context(mock.patch.object(pgo.benchmark, 'cargo_configuration', return_value={}))
        stack.enter_context(mock.patch.object(pgo.benchmark, 'release_configuration',
                                             return_value=['--config', 'profile.release.lto="fat"']))
        stack.enter_context(mock.patch.object(pgo.subprocess, 'check_output', return_value='revision\n'))
        stack.enter_context(mock.patch.object(common, 'load', side_effect=lambda name: {'name': name, 'work': {'check': {}}}))
        stack.enter_context(mock.patch.object(common, 'provisioned', return_value=True))
        stack.enter_context(mock.patch.object(common, 'checkout', return_value=Path('/fixture')))
        stack.enter_context(mock.patch.object(common, 'arguments', return_value=['--noEmit', '--listFiles']))
        stack.enter_context(mock.patch.object(common, 'require_unchanged'))
        stack.enter_context(mock.patch.object(common, 'observe', return_value={'listed_paths': ['/fixture/a.ts'], 'types': 1234}))
        stack.enter_context(mock.patch.object(common, 'problems', side_effect=lambda *args: ['changed work'] if self.failure == 'training' else []))
        stack.enter_context(mock.patch.object(pgo, 'preflight', side_effect=self.preflight))
        stack.enter_context(mock.patch.object(pgo, 'execute', side_effect=self.execute))

    def preflight(self, binary, descriptor, checkers, env, prefix):
        after = prefix.name.endswith('-after')
        self.events.append('after' if after else 'before')
        if self.failure == 'preflight' or after and self.failure == 'options-after':
            raise ValueError('training options differ')
        return copy.deepcopy(self.inputs)

    def execute(self, command, cwd, env, prefix, timeout=1800):
        result = {'command': list(map(str, command)), 'exit': 0, 'stdout': '', 'stderr': ''}
        if command[0] == 'cargo':
            name = 'optimized' if '-Cprofile-use=' in env['CARGO_ENCODED_RUSTFLAGS'] else 'instrumented'
            self.events.append(name)
            self.build_envs.append(env)
            self.assertEqual(command[command.index('--target') + 1], 'native-host')
            self.assertIn('profile.release.lto="fat"', command)
            result['stdout'] = '\n'.join(map(json.dumps, messages(self.binary)))
            if self.failure == name:
                result['exit'] = 1
            if self.failure == 'source-added':
                self.identity['new.rs'] = 'new source'
        elif command[0] == 'llvm-profdata':
            self.events.append('merge')
            Path(command[command.index('-o') + 1]).write_bytes(b'merged profile')
            if self.failure == 'merge':
                result['exit'] = 1
        else:
            self.events.append('train')
            profile = Path(env['LLVM_PROFILE_FILE'].replace('%m', 'module').replace('%p', '123'))
            profile.write_bytes(b'training profile')
        return result

    def test_default_training_sequence_and_saved_artifacts(self):
        previous = common.CACHE
        output = self.root / 'experiment with spaces'
        report = pgo.run(output, self.root / 'cache')
        self.assertTrue(report['complete'])
        self.assertEqual(self.events, ['instrumented', 'before', 'train', 'after', 'before', 'train', 'after',
                                       'before', 'train', 'after', 'merge', 'optimized'])
        self.assertEqual([(row['scenario'], row['checkers']) for row in report['training']], list(pgo.TRAINING))
        self.assertEqual([row['facts']['types'] for row in report['training']], [1234] * 3)
        self.assertEqual((output / 'optimized/tsrust').read_bytes(), b'exact artifact')
        self.assertEqual(report['builds'][0]['artifact']['sha256'], common.sha256(b'exact artifact'))
        self.assertEqual(self.build_envs[0]['CARGO_ENCODED_RUSTFLAGS'], '-Cprofile-generate=' + str(output / 'raw'))
        self.assertEqual(len(self.build_envs[1]['CARGO_ENCODED_RUSTFLAGS'].split('\x1f')), 2)
        self.assertEqual(common.CACHE, previous)

    def test_failures_stop_later_stages_and_preserve_incomplete_manifest(self):
        for failure in ('instrumented', 'preflight', 'training', 'options-after', 'merge', 'optimized', 'source-added'):
            with self.subTest(failure=failure):
                self.events.clear()
                self.failure = failure
                self.identity = {'source.rs': 'unchanged'}
                output = self.root / failure
                previous = common.CACHE
                with self.assertRaises(ValueError):
                    pgo.run(output, self.root / 'cache')
                report = json.loads((output / 'manifest.json').read_text())
                self.assertFalse(report['complete'])
                self.assertIn('error', report)
                if failure != 'optimized':
                    self.assertNotIn('optimized', self.events)
                if failure in ('instrumented', 'preflight', 'source-added'):
                    self.assertNotIn('train', self.events)
                self.assertEqual(common.CACHE, previous)

    def test_existing_output_and_target_directory_do_not_start_build(self):
        for output in (self.root, self.root / 'target/experiment'):
            with self.subTest(output=output), self.assertRaises((ValueError, FileExistsError)):
                pgo.run(output, self.root / 'cache')
        self.assertEqual(self.events, [])
