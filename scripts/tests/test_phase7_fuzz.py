"""Real fake-child controls for the first bounded Phase 7 CLI fuzzer."""
from __future__ import annotations

import json
import os
from pathlib import Path
import random
import signal
import sys
import tempfile
import time
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools.phase7.fuzz import protocol, replay, run as fuzz  # noqa: E402

FAKE = r'''
import hashlib, json, os, pathlib, signal, subprocess, sys, time
mode = MODE
if mode in ('switch', 'alternating'):
    control = pathlib.Path(CONTROL_FILE)
    previous = mode
    mode = control.read_text()
    if previous == 'alternating':
        control.write_text('same' if mode == 'missing' else 'missing')
root = pathlib.Path.cwd()
if pathlib.Path(BANNER_FILE).exists():
    print(pathlib.Path(BANNER_FILE).read_text())
assert sys.argv[1:] == ['--project', 'tsconfig.json', '--pretty', 'true', '--checkers', '1']
source = (root / 'main.ts').read_bytes()
config = json.loads((root / 'tsconfig.json').read_text())
assert config['compilerOptions']['noEmitOnError'] is False
assert not (root / 'out').exists(), 'stale output from another compiler'
if mode == 'timeout':
    child_code = 'import pathlib,time\np=pathlib.Path(' + repr(HEARTBEAT_FILE) + ')\nwhile True:\n p.write_text(str(time.monotonic_ns())); time.sleep(0.02)'
    descendant = subprocess.Popen([sys.executable, '-c', child_code])
    while not pathlib.Path(HEARTBEAT_FILE).exists():
        time.sleep(0.01)
    pathlib.Path(PID_FILE).write_text(str(descendant.pid))
    time.sleep(60)
if mode == 'crash':
    os.kill(os.getpid(), signal.SIGKILL)
if mode == 'diagnostic' or (mode == 'conditional' and b'TRIGGER' in source):
    print(str(root / 'main.ts') + '(1,1): error TS2322: Changed diagnostic.')
    print('  The complete related message is preserved.')
    sys.exit(2)
if mode in ('related-a', 'related-b'):
    print('main.ts(1,1): error TS1: Same primary diagnostic.')
    print('Related diagnostic ' + mode)
    sys.exit(2)
if mode == 'stderr':
    print('unexpected compiler warning', file=sys.stderr)
if mode != 'missing':
    out = root / 'out'
    out.mkdir()
    (out / 'main.js').write_bytes(b'different' if mode == 'emit' else source)
    (out / 'main.d.ts').write_bytes((root / 'globals.d.ts').read_bytes())
if mode == 'path':
    print(str(root / 'main.ts') + '(1,1): error TS1: Same diagnostic.\r')
    sys.exit(2)
'''


@unittest.skipUnless(os.name == 'posix', 'POSIX process-group controls')
class DifferentialControls(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.pid_file = self.root / 'descendant.pid'
        self.heartbeat_file = self.root / 'descendant.heartbeat'
        self.control_file = self.root / 'fake-control'
        self.banner_file = self.root / 'common-banner'
        self.memory = fuzz.memory_policy(1024)

    def executable(self, name, mode):
        path = self.root / name
        interpreter = sys.executable
        if mode == 'missing-interpreter':
            link = self.root / 'fake-interpreter'
            link.symlink_to(sys.executable)
            interpreter, mode = str(link), 'missing'
        script = '#!' + interpreter + '\n' + FAKE.replace('MODE', repr(mode)).replace('PID_FILE', repr(str(self.pid_file))).replace('HEARTBEAT_FILE', repr(str(self.heartbeat_file))).replace('CONTROL_FILE', repr(str(self.control_file))).replace('BANNER_FILE', repr(str(self.banner_file)))
        path.write_text(script)
        path.chmod(0o755)
        return path

    def pair(self, mode='same', source=b'export const value = 1;\n', timeout=6, go_mode='same'):
        binaries = {'go': self.executable('tsgo', go_mode), 'rust': self.executable('tsrust', mode)}
        return fuzz.compare_case(source, binaries, timeout, self.memory)

    def test_identical_input_options_and_fresh_outputs_match(self):
        result = self.pair()
        self.assertEqual(result.failures, [])
        self.assertEqual(result.artifacts['rust']['emitted']['main.js'], b'export const value = 1;\n')
        self.assertEqual(set(result.observations['rust']['emitted']), {'main.js', 'main.d.ts'})

    def test_complete_diagnostics_include_message_changes_and_stderr(self):
        result = self.pair('diagnostic')
        self.assertIn('stdout_hex', result.failures)
        self.assertIn(b'The complete related message', bytes.fromhex(result.observations['rust']['stdout_hex']))
        self.assertIn('stderr_hex', self.pair('stderr').failures)
        self.assertEqual(self.pair('related-a', go_mode='related-b').failures, ['stdout_hex'])
        # Absolute per-case directories and CRLF may differ; all messages remain.
        self.assertEqual(self.pair('path', go_mode='path').failures, [])

    def test_missing_outputs_and_changed_bytes_fail(self):
        for control in ('missing', 'emit'):
            with self.subTest(control=control):
                result = self.pair(control)
                self.assertIn('emitted', result.failures)
                self.assertFalse(any('crash' in reason for reason in result.failures))

    def test_even_identical_crashes_are_findings(self):
        result = self.pair('crash', go_mode='crash')
        self.assertEqual(result.failures, ['go-crash', 'rust-crash'])
        self.assertEqual(result.observations['rust']['exit'], -signal.SIGKILL)

    def test_timeout_kills_descendants_and_is_preserved(self):
        started = time.monotonic()
        result = self.pair('timeout', timeout=2)
        self.assertLess(time.monotonic() - started, 3)
        self.assertIn('rust-timeout', result.failures)
        self.assertTrue(result.observations['rust']['timeout'])
        self.assertGreater(int(self.pid_file.read_text()), 0)
        # The descendant was running before timeout; its heartbeat must stop.
        heartbeat = self.heartbeat_file.read_bytes()
        self.assertTrue(heartbeat)
        time.sleep(0.15)
        self.assertEqual(self.heartbeat_file.read_bytes(), heartbeat)

    def test_launch_and_limit_setup_errors_are_runner_errors_without_findings(self):
        source = self.root / 'input.ts'
        source.write_bytes(b'let x = 1;')
        go = self.executable('go', 'same')
        rust = self.root / 'rust'
        rust.write_text('#!/does/not/exist\n')
        rust.chmod(0o755)
        for target in ('cli', 'scanner', 'parser'):
            output = self.root / ('launch-' + target)
            status = fuzz.main(['--rust', str(rust), '--go', str(go), '--target', target,
                                '--output', str(output), '--replay', str(source),
                                '--budget-seconds', '20', '--case-seconds', '6'])
            self.assertEqual(status, 2)
            metadata = json.loads((output / 'run.json').read_text())
            self.assertEqual(metadata['stop_reason'], 'runner-error')
            self.assertEqual(metadata['executions'], 0)
            self.assertEqual(metadata['failures'], [])
            self.assertEqual(list(output.glob('failure-*')), [])
            self.assertIn('FileNotFoundError', metadata['runner_errors'][0]['errors']['rust'])
        rust = self.executable('rust', 'same')
        with mock.patch.object(fuzz, '_child_limits', side_effect=OSError('limit setup refused')):
            result = fuzz.compare_case(source.read_bytes(), {'go': go, 'rust': rust}, 6,
                                       {**self.memory, 'enforced': True})
        self.assertEqual(set(fuzz.runner_errors(result)), {'go', 'rust'})
        with self.assertRaisesRegex(ValueError, 'cannot be minimized'):
            fuzz.minimize(source.read_bytes(), result, lambda _: result, 2, time.monotonic() + 5)

    def test_minimization_runner_failure_keeps_original_finding_and_raw_error(self):
        binaries = {'go': self.executable('go', 'same'), 'rust': self.executable('rust', 'missing')}
        source = self.root / 'input.ts'
        source.write_bytes(b'TRIGGER padding')
        original = fuzz.compare_case(source.read_bytes(), binaries, 6, self.memory)
        failed = fuzz.Pair({'go': {'exit': None, 'timeout': False, 'runner_error': 'setup failed',
                                   'execution_error': 'setup failed'}},
                           {'go': {'stdout': b'raw', 'stderr': b'error', 'emitted': {}}},
                           ['go-execution-failure'])
        output = self.root / 'minimizer-error'
        with mock.patch.object(fuzz, 'compare_case', side_effect=[original, original, failed]):
            status = fuzz.main(['--rust', str(binaries['rust']), '--go', str(binaries['go']),
                                '--output', str(output), '--replay', str(source), '--minimize-trials', '2'])
        self.assertEqual(status, 2)
        metadata = json.loads((output / 'run.json').read_text())
        self.assertEqual((metadata['executions'], len(metadata['failures']), metadata['minimize_executions']), (1, 1, 2))
        self.assertEqual(metadata['failures'][0]['minimize_trials'], 2)
        self.assertEqual(set(metadata['failures'][0]['manifests']), {'original'})
        self.assertTrue((output / 'failure-000000/original/manifest.json').is_file())
        self.assertFalse((output / 'failure-000000/minimized').exists())
        self.assertEqual((output / 'runner-error-minimization/go/stdout').read_bytes(), b'raw')

    def test_minimizer_retains_a_reproducing_trigger(self):
        binaries = {'go': self.executable('tsgo', 'same'),
                    'rust': self.executable('tsrust', 'conditional')}
        evaluate = lambda source: fuzz.compare_case(source, binaries, 6, self.memory)
        source = b'// unrelated padding\nTRIGGER\n// more unrelated padding\n'
        original = evaluate(source)
        self.assertEqual(original.failures, ['exit', 'stdout_hex', 'emitted'])
        minimal, result, trials = fuzz.minimize(source, original, evaluate, 40, time.monotonic() + 20)
        self.assertLess(len(minimal), len(source))
        self.assertIn(b'TRIGGER', minimal)
        self.assertEqual(result.failures, original.failures)
        self.assertEqual(evaluate(minimal).failures, original.failures)
        self.assertLessEqual(trials, 40)
        self.assertEqual(evaluate(b'').failures, [])

    def test_run_saves_original_minimized_raw_bytes_and_binary_identities(self):
        rust, go = self.executable('tsrust', 'missing'), self.executable('tsgo', 'same')
        source = self.root / 'input.ts'
        source.write_bytes(b'export const value = 1;\xff\n')
        output = self.root / 'findings'
        status = fuzz.main(['--rust', str(rust), '--go', str(go), '--output', str(output),
                            '--replay', str(source), '--budget-seconds', '20', '--case-seconds', '6',
                            '--minimize-trials', '2'])
        self.assertEqual(status, 1)
        metadata = json.loads((output / 'run.json').read_text())
        self.assertEqual(metadata['executions'], 1)
        self.assertEqual(metadata['binaries']['rust']['path'], str(rust.resolve()))
        self.assertEqual(len(metadata['binaries']['rust']['sha256']), 64)
        original = output / 'failure-000000/original'
        self.assertEqual((original / 'main.ts').read_bytes(), source.read_bytes())
        self.assertEqual((original / 'go/out/main.js').read_bytes(), source.read_bytes())
        reduced = output / 'failure-000000/minimized'
        self.assertIn('emitted', json.loads((reduced / 'result.json').read_text())['failures'])
        self.assertFalse((reduced / 'rust/out/main.js').exists())
        replay.verify(reduced)

    def archive(self, mode='missing'):
        binaries = {'go': self.executable('tsgo', 'same'), 'rust': self.executable('tsrust', mode)}
        source = b'export const archived: number = 7;\n'
        # Ordinary controls should not accidentally archive Python startup
        # contention as the failure they were meant to exercise. Intentional
        # timeout controls retain their own short bound.
        case_seconds = 2 if mode == 'timeout' else 6
        result = fuzz.compare_case(source, binaries, case_seconds, self.memory)
        expected = (['rust-crash', 'exit', 'emitted'] if mode == 'crash'
                    else ['rust-timeout', 'exit', 'emitted'] if mode == 'timeout'
                    else ['emitted'])
        self.assertEqual(result.failures, expected, json.dumps(result.observations, indent=2))
        metadata = {'schema': 1, 'arguments': list(fuzz.ARGS), 'case_seconds': case_seconds,
                    'memory': self.memory, 'pin': 'fake-pin',
                    'binaries': {key: fuzz.identity(path) for key, path in binaries.items()}}
        archive = self.root / 'archive'
        finding = archive / 'failure-000000'
        hashes = {variant: fuzz.save_case(finding / variant, source, result, context=metadata)
                  for variant in ('original', 'minimized')}
        metadata['failures'] = [{'directory': finding.name, 'failures': result.failures, 'manifests': hashes}]
        (archive / 'run.json').write_text(json.dumps(metadata) + '\n')
        return finding / 'minimized'

    def replay(self, case, name='replayed', attempts=1):
        output = self.root / name
        status = replay.main(['--case', str(case), '--output', str(output), '--attempts', str(attempts)])
        return status, json.loads((output / 'replay.json').read_text()), output

    def test_manifested_replay_reproduces_and_keeps_original_unchanged(self):
        case = self.archive()
        before = fuzz.tree_files(self.root / 'archive')
        # Replay must use archived bytes/arguments even if generator defaults change.
        with mock.patch.object(fuzz, 'ARGS', ['--noEmit']), mock.patch.object(fuzz, 'CONFIG', {}):
            status, report, output = self.replay(case)
        self.assertEqual(status, 1, json.dumps(report, indent=2))
        self.assertEqual(report['status'], 'reproduced')
        self.assertEqual((output / 'attempt-00/main.ts').read_bytes(), (case / 'main.ts').read_bytes())
        self.assertEqual((output / 'attempt-00/tsconfig.json').read_bytes(), (case / 'tsconfig.json').read_bytes())
        self.assertEqual(fuzz.tree_files(self.root / 'archive'), before)
        saved_output = fuzz.tree_files(output)
        with self.assertRaises(SystemExit):
            self.replay(case)
        self.assertEqual(fuzz.tree_files(output), saved_output)
        self.assertEqual(fuzz.tree_files(self.root / 'archive'), before)

    def test_replay_reproduces_a_recorded_crash(self):
        case = self.archive('crash')
        status, report, output = self.replay(case)
        self.assertEqual((status, report['status']), (1, 'reproduced'))
        observed = json.loads((output / 'attempt-00/result.json').read_text())
        self.assertIn('rust-crash', observed['failures'])
        self.assertEqual(observed['observations']['rust']['exit'], -signal.SIGKILL)

    def test_same_failure_category_with_changed_common_diagnostics_is_visible(self):
        case = self.archive()
        before = fuzz.tree_files(self.root / 'archive')
        self.banner_file.write_text('a new diagnostic from both children')
        status, report, output = self.replay(case, 'changed-common-output')
        self.assertEqual((status, report['status']), (2, 'execution-failure'))
        attempt = report['attempts'][0]
        self.assertEqual(attempt['failures'], ['emitted'])
        self.assertEqual(attempt['changed_observations'],
                         {'go': ['stdout_hex'], 'rust': ['stdout_hex']})
        observed = json.loads((output / 'attempt-00/result.json').read_text())
        self.assertIn(b'a new diagnostic', bytes.fromhex(observed['observations']['go']['stdout_hex']))
        self.assertEqual(fuzz.tree_files(self.root / 'archive'), before)

    def test_replay_reproduces_a_recorded_timeout(self):
        case = self.archive('timeout')
        status, report, output = self.replay(case)
        self.assertEqual((status, report['status']), (1, 'reproduced'))
        observed = json.loads((output / 'attempt-00/result.json').read_text())
        self.assertIn('rust-timeout', observed['failures'])
        self.assertTrue(observed['observations']['rust']['timeout'])

    def test_replay_refuses_manifest_input_output_and_executable_tampering(self):
        case = self.archive()
        original = fuzz.tree_files(self.root / 'archive')
        targets = [case / 'manifest.json', case / 'main.ts', case / 'tsconfig.json',
                   case / 'go/out/main.js', case / 'rust/stdout', self.root / 'tsrust']
        for index, target in enumerate(targets):
            with self.subTest(target=target.name):
                saved = target.read_bytes()
                target.write_bytes(saved + b' ')
                status, report, output = self.replay(case, name=f'tampered-{index}')
                self.assertEqual(status, 2)
                self.assertEqual(report['status'], 'execution-failure')
                self.assertEqual(report['attempts'], [])
                self.assertIn('differ', report['error'])
                self.assertFalse((output / 'attempt-00').exists())
                target.write_bytes(saved)
        self.assertEqual(fuzz.tree_files(self.root / 'archive'), original)

    def test_replay_reports_resolution_and_new_timeout_separately(self):
        self.control_file.write_text('missing')
        case = self.archive('switch')
        before = fuzz.tree_files(self.root / 'archive')
        self.control_file.write_text('same')
        status, report, _ = self.replay(case, 'resolved')
        self.assertEqual((status, report['status']), (0, 'resolved'))
        self.control_file.write_text('timeout')
        status, report, output = self.replay(case, 'timed-out')
        self.assertEqual((status, report['status']), (2, 'execution-failure'))
        self.assertIn('rust-timeout', report['attempts'][0]['failures'])
        self.assertTrue((output / 'attempt-00/result.json').is_file())
        self.assertEqual(fuzz.tree_files(self.root / 'archive'), before)

    def test_replay_preserves_first_child_outputs_when_second_cannot_launch(self):
        case = self.archive('missing-interpreter')
        (self.root / 'fake-interpreter').unlink()
        status, report, output = self.replay(case)
        self.assertEqual((status, report['status']), (2, 'execution-failure'))
        self.assertEqual((output / 'attempt-00/go/out/main.js').read_bytes(), (case / 'main.ts').read_bytes())
        saved = json.loads((output / 'attempt-00/result.json').read_text())
        self.assertIn('rust-execution-failure', saved['failures'])
        self.assertIn('FileNotFoundError', saved['observations']['rust']['execution_error'])

    def test_replay_keeps_alternating_outcomes_visible(self):
        self.control_file.write_text('missing')
        case = self.archive('alternating')
        status, report, output = self.replay(case, attempts=2)
        self.assertEqual(status, 1)
        self.assertEqual([item['status'] for item in report['attempts']], ['resolved', 'reproduced'])
        first = json.loads((output / 'attempt-00/result.json').read_text())
        second = json.loads((output / 'attempt-01/result.json').read_text())
        self.assertEqual(first['failures'], [])
        self.assertEqual(second['failures'], ['emitted'])

    def test_replay_refuses_unmanifested_archives_and_wrong_independent_anchor(self):
        case = self.archive()
        output = self.root / 'wrong-anchor'
        status = replay.main(['--case', str(case), '--output', str(output), '--run-sha256', '0' * 64])
        self.assertEqual(status, 2)
        report = json.loads((output / 'replay.json').read_text())
        self.assertIn('independent anchor', report['error'])
        (case / 'manifest.json').unlink()
        status, report, _ = self.replay(case, 'legacy')
        self.assertEqual(status, 2)
        self.assertIn('unmanifested', report['error'])

    def test_explicit_candidate_records_both_identities_and_keeps_go_exact(self):
        case = self.archive()
        before = fuzz.tree_files(self.root / 'archive')
        original = fuzz.identity(self.root / 'tsrust')
        # Replacing a saved Rust path is allowed only with the explicit candidate option.
        candidate = self.executable('tsrust', 'same')
        output = self.root / 'candidate'
        status = replay.main(['--case', str(case), '--output', str(output), '--candidate-rust', str(candidate)])
        report = json.loads((output / 'replay.json').read_text())
        self.assertEqual((status, report['status'], report['mode']), (0, 'resolved', 'candidate-rust'))
        self.assertEqual(report['original_binaries']['rust'], original)
        self.assertEqual(report['executed_binaries']['rust'], fuzz.identity(candidate.resolve()))
        self.assertIn('not an original Rust binary replay', report['reproduction_meaning'])
        self.assertEqual(fuzz.tree_files(self.root / 'archive'), before)
        candidate_archive = json.loads((output / 'attempt-00/manifest.json').read_text())
        self.assertEqual(candidate_archive['binaries']['rust'], fuzz.identity(candidate.resolve()))
        self.executable('tsgo', 'emit')
        rejected = self.root / 'changed-go'
        status = replay.main(['--case', str(case), '--output', str(rejected), '--candidate-rust', str(candidate)])
        self.assertEqual(status, 2)
        report = json.loads((rejected / 'replay.json').read_text())
        self.assertIn('go executable identity differs', report['error'])
        self.assertEqual(report['attempts'], [])

    def test_insufficient_budget_starts_no_partial_pair(self):
        binaries = {'go': self.root / 'nonexistent-go', 'rust': self.root / 'nonexistent-rust'}
        with self.assertRaises(TimeoutError):
            fuzz.compare_case(b'x', binaries, 2, self.memory, time.monotonic() + 0.1)


class GenerationControls(unittest.TestCase):
    def test_seed_reproduces_generation_and_mutation(self):
        def sequence(seed):
            rng = random.Random(seed)
            return [(fuzz.generated(rng), fuzz.mutate(b'const value: number = 1;\n', rng)) for _ in range(8)]
        self.assertEqual(sequence(42), sequence(42))
        self.assertNotEqual(sequence(42), sequence(43))

    def test_memory_claim_matches_enforcement_platform(self):
        policy = fuzz.memory_policy(128)
        self.assertEqual(policy['enforced'], sys.platform.startswith('linux'))
        if not policy['enforced']:
            self.assertIsNone(policy['mechanism'])
            self.assertIn('unsupported', policy['note'])



SCANNER_FAKE = r"""
import json, pathlib, sys, time
mode = MODE
assert sys.argv[1:] == []
raw = sys.stdin.buffer.read()
root = pathlib.Path.cwd()
assert raw == (root / 'request.ndjson').read_bytes()
request = json.loads(raw)
assert bytes.fromhex(request['source_hex']) == (root / 'main.ts').read_bytes()
assert request['actions'] == [{'op': 'scan_all'}]
assert request['target'] == 99 and request['variant'] == 0 and request['skip_trivia'] is True
assert request['decode_source'] is False
assert not (root / 'out').exists()
if mode == 'timeout':
    time.sleep(60)
if mode == 'large':
    sys.stdout.buffer.write(b'x' * 4096)
    sys.stdout.buffer.flush()
    sys.exit(0)
identity = request['id']
value = {'kind': 1, 'token': 1, 'full_start': 0, 'start': 0, 'end': 0,
         'text_hex': '', 'value_hex': '', 'flags': 0, 'range': [0, 0],
         'predicates': [False] * 7, 'directives': []}
if mode == 'value':
    value['value_hex'] = '61'
diagnostics = []
if mode == 'diagnostic':
    diagnostics = [{'code': 1127, 'category': 1, 'key': 'Invalid_character',
                    'start': 0, 'length': 1, 'args': [{'kind': 'string', 'hex': 'ff'}]}]
observation = {'event': 'observation', 'id': identity, 'action': 0, 'ordinal': 0,
               'status': 'ok', 'value': value, 'diagnostics': diagnostics}
if mode == 'panic':
    observation['status'] = 'panic'
    observation['value'] = {'message': 'unhandled scanner failure', 'class': 'unexpected', 'input_hex': None}
records = [{'event': 'begin', 'id': identity, 'version': 1}, observation,
           {'event': 'end', 'id': identity, 'observations': 1, 'completed_actions': 1}]
if mode == 'missing-end':
    records.pop()
if mode == 'missing-eof':
    observation['value']['kind'] = 80
if mode == 'trailing':
    records.append(records[-1])
for record in records:
    print(json.dumps(record))
"""


@unittest.skipUnless(os.name == 'posix', 'POSIX process-group controls')
class ScannerControls(unittest.TestCase):
    setUp = DifferentialControls.setUp
    replay = DifferentialControls.replay

    def executable(self, name, mode):
        path = self.root / name
        path.write_text('#!' + sys.executable + '\n' + SCANNER_FAKE.replace('MODE', repr(mode)))
        path.chmod(0o755)
        return path

    def pair(self, mode='same', go_mode='same', timeout=6, source=b'const a = 1;\xff'):
        binaries = {'go': self.executable('scanner-go', go_mode),
                    'rust': self.executable('scanner-rust', mode)}
        return protocol.compare_case('scanner', source, binaries, timeout, self.memory)

    def test_scanner_exact_request_schedule_and_invalid_utf8_are_preserved(self):
        self.assertEqual(self.pair().failures, [])
        files = protocol.input_files('scanner', b'\xff')
        self.assertEqual(protocol.validate_input('scanner', files)['source_hex'], 'ff')
        with self.assertRaisesRegex(ValueError, 'differs'):
            protocol.validate_input('scanner', {**files, 'main.ts': b'changed'})

    def test_scanner_full_token_values_and_diagnostics_are_compared(self):
        for mode in ('value', 'diagnostic'):
            with self.subTest(mode=mode):
                result = self.pair(mode)
                self.assertEqual(result.failures, ['stdout_hex'])
        self.assertIn(b'Invalid_character', self.pair('diagnostic').artifacts['rust']['stdout'])

    def test_identically_incomplete_or_trailing_streams_cannot_pass(self):
        for mode in ('missing-end', 'missing-eof', 'trailing'):
            with self.subTest(mode=mode):
                result = self.pair(mode, go_mode=mode)
                self.assertEqual(result.failures, ['go-protocol-failure', 'rust-protocol-failure'])
                self.assertTrue(result.observations['go']['protocol_error'])

    def test_equal_unexpected_scanner_panics_are_findings(self):
        result = self.pair('panic', go_mode='panic')
        self.assertEqual(result.failures, ['go-library-failure', 'rust-library-failure'])
        self.assertIn(b'unhandled scanner failure', result.artifacts['rust']['stdout'])

    def test_scanner_timeout_and_capture_limits_fail_closed(self):
        result = self.pair('timeout', timeout=2)
        self.assertIn('rust-timeout', result.failures)
        with mock.patch.object(protocol, 'MAX_OUTPUT', 1024):
            result = self.pair('large')
        self.assertIn('rust-execution-failure', result.failures)
        self.assertLessEqual(len(result.artifacts['rust']['stdout']), 1024)
        self.assertIn('retained prefix', result.observations['rust']['execution_error'])
        saved = {'failures': result.failures, 'observations': result.observations}
        self.assertEqual(replay.classify(saved, result), 'reproduced')
        result.observations['rust'].pop('compiler_failure')
        self.assertEqual(replay.classify(saved, result), 'execution-failure')

    def test_scanner_archive_replay_retains_manifest_request_and_original(self):
        source = self.root / 'source.ts'
        source.write_bytes(b'let x = 0;\xff')
        binaries = {'go': self.executable('scanner-go', 'same'),
                    'rust': self.executable('scanner-rust', 'diagnostic')}
        archive = self.root / 'archive'
        status = fuzz.main(['--target', 'scanner', '--rust', str(binaries['rust']),
                            '--go', str(binaries['go']), '--output', str(archive),
                            '--replay', str(source), '--case-seconds', '6',
                            '--budget-seconds', '20', '--minimize-trials', '2'])
        self.assertEqual(status, 1)
        case = archive / 'failure-000000/minimized'
        metadata, manifest, _, _ = replay.verify(case)
        self.assertEqual(manifest['target'], 'scanner')
        self.assertEqual(manifest['inputs'], ['main.ts', 'request.ndjson'])
        self.assertEqual(manifest['arguments'], [])
        self.assertEqual(metadata['protocol_limits'], protocol.LIMITS)
        before = fuzz.tree_files(archive)
        status, report, output = self.replay(case)
        self.assertEqual((status, report['status'], report['target']), (1, 'reproduced', 'scanner'))
        self.assertEqual((output / 'attempt-00/request.ndjson').read_bytes(), (case / 'request.ndjson').read_bytes())
        self.assertEqual(fuzz.tree_files(archive), before)
        request_path = case / 'request.ndjson'
        request_path.write_bytes(request_path.read_bytes() + b' ')
        status, report, _ = self.replay(case, 'tampered-scanner')
        self.assertEqual(status, 2)
        self.assertEqual(report['attempts'], [])

    def test_explicit_scanner_replay_refuses_oversized_source_without_output(self):
        source = self.root / 'large.ts'
        source.write_bytes(b'a' * (protocol.MAX_SOURCE + 1))
        go, rust = self.executable('scanner-go', 'same'), self.executable('scanner-rust', 'same')
        output = self.root / 'oversized'
        with self.assertRaises(SystemExit):
            fuzz.main(['--target', 'scanner', '--rust', str(rust), '--go', str(go),
                       '--output', str(output), '--replay', str(source)])
        self.assertFalse(output.exists())


PARSER_FAKE = r"""
import json, pathlib, sys
mode = MODE
assert sys.argv[1:] == ['--fuzz-related']
raw = sys.stdin.buffer.read()
root = pathlib.Path.cwd()
assert raw == (root / 'request.ndjson').read_bytes()
request = json.loads(raw)
source = (root / 'main.ts').read_bytes()
assert bytes.fromhex(request['source_hex']) == source
assert request['version'] == 1 and request['id'] == 'r5/parser'
assert request['op'] == 'parse' and request['primary'] is None
assert request['filename'] == request['path'] == '/s06/fixture.ts'
assert request['script_kind'] == 3 and request['jsx'] is False and request['force'] is False
assert request['operations'] == ['parse', 'node_index_before', 'encode_source_file', 'node_index_after']
assert not (root / 'out').exists()
records = [{'tag': 'begin', 'op': 'parse'}]
sequence = 0
stages = 0
def observe(stage, kind, value):
    global sequence
    records.append({'tag': 'observation', 'seq': sequence, 'stage': stage, 'kind': kind, 'value': value})
    sequence += 1
def finish(stage, outcome='ok', message=b''):
    global stages
    records.append({'tag': 'stage', 'stage': stage, 'outcome': outcome, 'message_hex': message.hex()})
    stages += 1
if mode == 'panic':
    finish('parse', 'panic', b'unexpected parser failure')
else:
    observe('parse', 'source_file', {'kind': 307, 'pos': 0, 'end': len(source), 'flags': 0,
            'node_count': 1, 'text_count': 0, 'identifier_count': 0, 'script_kind': 3,
            'language_variant': 0, 'declaration_file': False, 'hash': '0' * 32})
    observe('parse', 'diagnostic', {'collection': 'parse', 'index': 0, 'code': 1127,
            'category': 1, 'pos': 0, 'end': 1, 'key_hex': b'Invalid_character'.hex(),
            'text_hex': (b'changed diagnostic' if mode == 'diagnostic' else b'').hex(),
            'args_hex': [b'\xff'.hex()], 'file_hex': None, 'related': [
                {'code': 1005, 'category': 1, 'pos': 0, 'end': 0, 'file_hex': None,
                 'key_hex': '61', 'text_hex': '62' if mode == 'related' else '61',
                 'args_hex': [], 'related': False if mode == 'malformed-related' else []}]})
    if mode != 'missing-stage':
        finish('parse')
    for stage in ('node_index_before', 'encode_source_file', 'node_index_after'):
        if stage == 'encode_source_file':
            observe(stage, 'bytes', {'length': 1})
            if mode != 'missing-chunk':
                observe(stage, 'chunk', {'offset': 0, 'hex': '42' if mode == 'encoded' else '41'})
        else:
            identity = ({'independent_order_equal': True, 'cache_reused': True}
                        if stage == 'node_index_before' else {'before_reused': True, 'encoded_reused': True})
            observe(stage, 'identity', identity)
            observe(stage, 'table', {'length': 2})
            observe(stage, 'node', {'index': 0, 'node': None})
            if mode != 'missing-node':
                observe(stage, 'node', {'index': 1, 'node': {'kind': 307, 'pos': 0,
                        'end': len(source), 'flags': 0, 'parent': 1 if mode == 'index' else 0, 'lookup': 1}})
            observe(stage, 'absent_lookup', {'index': 0})
        finish(stage)
records.append({'tag': 'end', 'observations': sequence, 'stages': stages})
if mode == 'missing-end':
    records.pop()
if mode == 'trailing':
    records.append(records[-1])
for record in records:
    record.update(id=request['id'], version=1)
if mode.startswith('nonobject-'):
    _, kind, position = mode.split('-')
    value = {'null': None, 'array': [], 'string': 'bad', 'number': 3, 'boolean': False}[kind]
    records[{'begin': 0, 'middle': 1, 'end': -1}[position]] = value
wire = b'\n'.join(json.dumps(record).encode() for record in records) + b'\n'
if mode == 'truncated':
    wire = wire[:-1]
sys.stdout.buffer.write(wire)
"""


@unittest.skipUnless(os.name == 'posix', 'POSIX process-group controls')
class ParserControls(unittest.TestCase):
    setUp = DifferentialControls.setUp
    replay = DifferentialControls.replay

    def executable(self, name, mode):
        path = self.root / name
        path.write_text('#!' + sys.executable + '\n' + PARSER_FAKE.replace('MODE', repr(mode)))
        path.chmod(0o755)
        return path

    def pair(self, mode='same', go_mode='same', source=b'let x = 0;\xff'):
        binaries = {'go': self.executable('parser-go', go_mode),
                    'rust': self.executable('parser-rust', mode)}
        return protocol.compare_case('parser', source, binaries, 6, self.memory)

    def test_parser_exact_options_and_complete_ordered_records_match(self):
        result = self.pair()
        self.assertEqual(result.failures, [])
        frames = [json.loads(line) for line in result.artifacts['rust']['stdout'].splitlines()]
        self.assertEqual([frame['stage'] for frame in frames if frame['tag'] == 'stage'],
                         ['parse', 'node_index_before', 'encode_source_file', 'node_index_after'])
        diagnostic = next(frame['value'] for frame in frames if frame.get('kind') == 'diagnostic')
        self.assertEqual(diagnostic['args_hex'], ['ff'])
        self.assertEqual(sum(frame.get('kind') == 'node' for frame in frames), 4)
        self.assertEqual(next(frame['value'] for frame in frames if frame.get('kind') == 'chunk'),
                         {'offset': 0, 'hex': '41'})

    def test_parser_diagnostic_index_and_encoded_byte_changes_are_findings(self):
        for mode in ('diagnostic', 'related', 'index', 'encoded'):
            with self.subTest(mode=mode):
                result = self.pair(mode)
                self.assertEqual(result.failures, ['stdout_hex'])
                self.assertNotEqual(result.artifacts['go']['stdout'], result.artifacts['rust']['stdout'])

    def test_identical_missing_or_truncated_parser_frames_cannot_pass(self):
        for mode in ('missing-end', 'missing-stage', 'missing-node', 'missing-chunk', 'truncated', 'trailing', 'malformed-related'):
            with self.subTest(mode=mode):
                result = self.pair(mode, go_mode=mode)
                self.assertEqual(result.failures, ['go-protocol-failure', 'rust-protocol-failure'])
                self.assertTrue(result.observations['rust']['protocol_error'])

    def test_nonobject_parser_frames_are_retained_protocol_findings(self):
        for kind in ('null', 'array', 'string', 'number', 'boolean'):
            for position in ('begin', 'middle', 'end'):
                mode = f'nonobject-{kind}-{position}'
                with self.subTest(kind=kind, position=position):
                    result = self.pair(mode, go_mode=mode)
                    self.assertEqual(result.failures, ['go-protocol-failure', 'rust-protocol-failure'])
                    self.assertIn('frame must be an object', result.observations['rust']['protocol_error'])
                    self.assertEqual(bytes.fromhex(result.observations['rust']['stdout_hex']),
                                     result.artifacts['rust']['stdout'])
        source = self.root / 'source.ts'
        source.write_bytes(b'let x = 0;')
        binaries = {runtime: self.executable(runtime, 'nonobject-null-begin') for runtime in ('go', 'rust')}
        output = self.root / 'nonobject-run'
        status = fuzz.main(['--target', 'parser', '--rust', str(binaries['rust']), '--go', str(binaries['go']),
                            '--output', str(output), '--replay', str(source), '--minimize-trials', '0'])
        self.assertEqual(status, 1)
        metadata = json.loads((output / 'run.json').read_text())
        self.assertEqual(metadata['failures'][0]['failures'], ['go-protocol-failure', 'rust-protocol-failure'])
        self.assertTrue((output / 'failure-000000/original/rust/stdout').read_bytes().startswith(b'null\n'))

    def test_equal_parser_panics_are_findings(self):
        result = self.pair('panic', go_mode='panic')
        self.assertEqual(result.failures, ['go-library-failure', 'rust-library-failure'])
        self.assertIn(b'unexpected parser failure'.hex().encode(), result.artifacts['rust']['stdout'])

    def test_parser_archive_replay_keeps_target_request_and_original(self):
        source = self.root / 'source.ts'
        source.write_bytes(b'let x = 0;\xff')
        binaries = {'go': self.executable('parser-go', 'same'),
                    'rust': self.executable('parser-rust', 'diagnostic')}
        archive = self.root / 'archive'
        status = fuzz.main(['--target', 'parser', '--rust', str(binaries['rust']),
                            '--go', str(binaries['go']), '--output', str(archive),
                            '--replay', str(source), '--case-seconds', '6',
                            '--budget-seconds', '20', '--minimize-trials', '2'])
        self.assertEqual(status, 1)
        case = archive / 'failure-000000/minimized'
        metadata, manifest, _, _ = replay.verify(case)
        self.assertEqual(manifest['target'], 'parser')
        self.assertEqual(manifest['inputs'], ['main.ts', 'request.ndjson'])
        self.assertEqual(manifest['arguments'], ['--fuzz-related'])
        self.assertEqual(metadata['protocol_limits'], protocol.LIMITS)
        before = fuzz.tree_files(archive)
        status, report, output = self.replay(case)
        self.assertEqual((status, report['status'], report['target']), (1, 'reproduced', 'parser'))
        self.assertEqual((output / 'attempt-00/request.ndjson').read_bytes(), (case / 'request.ndjson').read_bytes())
        self.assertEqual(fuzz.tree_files(archive), before)
        metadata['target'] = 'scanner'
        (archive / 'run.json').write_text(json.dumps(metadata) + '\n')
        status, report, _ = self.replay(case, 'wrong-target')
        self.assertEqual(status, 2)
        self.assertEqual(report['attempts'], [])
        self.assertIn('target differs', report['error'])


if __name__ == '__main__':
    unittest.main()
