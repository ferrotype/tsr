"""API development orchestration with fake clients; no compiler or Node runs."""
import io
import json
from pathlib import Path
import signal
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools.phase7.api import pairs  # noqa: E402


def observation(samples):
    return {'method': 'development concurrent', 'node': 'v24.20.0', 'mode': 'sync', 'task': 'location',
            'iterations': len(samples), 'samples_ns': samples,
            'work': {'count': 10160, 'iterations': len(samples), 'sha256': 'a' * 64},
            'lifecycle': {'spawned': 1, 'reaped': 1, 'peak': 1, 'remaining': 0}}


class ApiPairTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory())).resolve()
        self.binaries = {name: self.root / name for name in ('control', 'candidate')}
        for name, path in self.binaries.items():
            path.write_text(name)
        self.results = {'control': observation([1, 2, 100]), 'candidate': observation([3, 100, 200])}
        self.children, self.calls, self.events = {}, [], []
        self.done, self.failure, self.exit_status = False, None, {}
        self.enterContext(mock.patch.object(pairs.subprocess, 'Popen', side_effect=self.spawn))
        self.kill = self.enterContext(mock.patch.object(pairs.os, 'killpg'))
        self.enterContext(mock.patch.object(pairs, 'receive', side_effect=self.receive))

    def spawn(self, command, **kwargs):
        name = Path(command[5]).name
        self.calls.append((command, kwargs))
        if self.failure == 'spawn' and name == 'candidate':
            raise OSError('cannot spawn candidate')
        child = SimpleNamespace(pid=100 + len(self.children), stdout=io.BytesIO(),
                                stdin=mock.Mock(wraps=io.BytesIO()), command=command, options=kwargs)
        child.input = []

        def send(data):
            if data == b'collect\n':
                self.assertTrue(self.done, 'collection must wait for both timed tasks')
            child.input.append(data)
            self.events.append((name, data))

        def wait(timeout=None):
            if timeout is not None:
                self.assertTrue(all(c.input == [b'run\n', b'collect\n'] for c in self.children.values()))
                pairs.write(Path(command[-1]), self.results[name])
            return self.exit_status.get(name, 0)

        child.stdin.write.side_effect = send
        child.wait = mock.Mock(side_effect=wait)
        self.children[name] = child
        return child

    def receive(self, children, expected, timeout):
        self.assertEqual(set(children), {'control', 'candidate'})
        self.assertEqual(timeout, 17)
        if expected == 'READY':
            self.assertTrue(all(not child.input for child in children.values()))
        else:
            self.assertEqual(expected, 'DONE')
            self.assertTrue(all(child.input == [b'run\n'] for child in children.values()))
            self.done = True
        self.events.append(('receive', expected))
        if self.failure == expected:
            raise TimeoutError(f'waiting for {expected}')

    def run_pair(self, output, index=0):
        return pairs.run_pair('/pinned/node', self.binaries, 'sync', 'location', 3, output, index, 17)

    def assert_cleaned(self):
        self.assertEqual(self.kill.call_count, len(self.children))
        for child in self.children.values():
            self.kill.assert_any_call(child.pid, signal.SIGKILL)
            child.stdin.close.assert_called_once()
            self.assertTrue(child.stdout.closed)
            self.assertTrue(child.options['stderr'].closed)
            child.wait.assert_called_with()

    def test_both_barriers_commands_alternating_order_and_median_ratio(self):
        report = self.run_pair(self.root / 'pair', index=1)
        self.assertEqual(report['submission_order'], ['candidate', 'control'])
        self.assertEqual(self.events, [('receive', 'READY'), ('candidate', b'run\n'), ('control', b'run\n'),
                                       ('receive', 'DONE'), ('candidate', b'collect\n'), ('control', b'collect\n')])
        self.assertEqual(report['median_ns'], {'control': 2, 'candidate': 100})
        self.assertEqual(report['ratio'], 50)
        self.assertEqual(report['work'], self.results['control']['work'])
        self.assertGreaterEqual(report['release_skew_ms'], 0)
        for command, options in self.calls:
            self.assertEqual(command[:4], ['/pinned/node', '--conditions', '@typescript/source', '--experimental-import-meta-resolve'])
            self.assertEqual(command[6:9], ['sync', 'location', '3'])
            self.assertTrue(options['start_new_session'])
            self.assertEqual(options['cwd'], ROOT / 'upstream')
        self.assertEqual(json.loads((self.root / 'pair/commands.json').read_text()), report['commands'])
        self.assert_cleaned()

    def test_setup_and_timed_failures_kill_and_reap_every_started_client(self):
        for failure in ('spawn', 'READY', 'DONE', 'exit'):
            with self.subTest(failure=failure):
                self.children, self.calls, self.events = {}, [], []
                self.done, self.failure = False, failure
                self.kill.reset_mock()
                self.exit_status = {'candidate': 2} if failure == 'exit' else {}
                with self.assertRaises((OSError, ValueError)):
                    self.run_pair(self.root / failure)
                self.assert_cleaned()
                if failure in ('spawn', 'READY'):
                    self.assertFalse(any(child.input for child in self.children.values()))
                if failure == 'DONE':
                    self.assertTrue(all(child.input == [b'run\n'] for child in self.children.values()))

    def test_wrong_answer_and_input_counts_are_rejected_with_raw_results_retained(self):
        for key, value in (('sha256', 'b' * 64), ('count', 10159)):
            with self.subTest(key=key):
                self.results['candidate'] = observation([3, 100, 200])
                self.results['candidate']['work'][key] = value
                output = self.root / key
                with self.assertRaisesRegex(ValueError, 'input schedule or symbol answers differ'):
                    self.run_pair(output)
                self.assertTrue((output / 'control.json').is_file())
                self.assertTrue((output / 'candidate.json').is_file())

    def test_matching_but_invalid_work_or_lifecycle_cannot_authenticate_measurement(self):
        changes = {
            'empty-work': lambda r: r['work'].update(count=0),
            'boolean-count': lambda r: r['work'].update(count=True),
            'invalid-digest': lambda r: r['work'].update(sha256='not a digest'),
            'omitted-work': lambda r: r['work'].update(iterations=2),
            'wrong-task': lambda r: r.update(task='position'),
            'wrong-mode': lambda r: r.update(mode='async'),
            'profile': lambda r: r.update(method='profile-only'),
            'leak': lambda r: r['lifecycle'].update(remaining=1),
            'overlap': lambda r: r['lifecycle'].update(peak=2),
            'missing-sample': lambda r: r['samples_ns'].pop(),
            'zero-sample': lambda r: r['samples_ns'].__setitem__(0, 0),
            'boolean-sample': lambda r: r['samples_ns'].__setitem__(0, True),
        }
        for name, change in changes.items():
            with self.subTest(name=name):
                self.results = {role: observation([1, 2, 3]) for role in self.binaries}
                for result in self.results.values():
                    change(result)
                with self.assertRaisesRegex(ValueError, 'invalid'):
                    self.run_pair(self.root / name)


class HandshakeTests(unittest.TestCase):
    def call_receive(self, reads, events):
        streams = {name: object() for name in ('control', 'candidate')}
        children = {name: SimpleNamespace(stdout=stream) for name, stream in streams.items()}
        keys = {name: SimpleNamespace(data=name, fd=index, fileobj=stream)
                for index, (name, stream) in enumerate(streams.items())}
        with mock.patch.object(pairs.selectors, 'DefaultSelector') as create, \
                mock.patch.object(pairs.os, 'read', side_effect=reads):
            selector = create.return_value.__enter__.return_value
            selector.select.side_effect = [[(keys[name], 1) for name in names] for names in events]
            pairs.receive(children, 'READY', 10)
            return selector

    def test_fragmented_readiness_waits_for_both_clients(self):
        selector = self.call_receive([b'RE', b'READY\n', b'ADY\n'], [('control',), ('candidate',), ('control',)])
        self.assertEqual(selector.select.call_count, 3)
        self.assertEqual(selector.unregister.call_count, 2)

    def test_timeout_eof_and_unexpected_messages_fail(self):
        cases = [([], [()], TimeoutError), ([b''], [('control',)], ValueError),
                 ([b'DONE\n'], [('candidate',)], ValueError), ([b'READY\nextra'], [('control',)], ValueError)]
        for reads, events, error in cases:
            with self.subTest(reads=reads), self.assertRaises(error):
                self.call_receive(reads, events)


class ScreenTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory())).resolve()
        self.binaries = {name: self.root / name for name in ('control', 'candidate')}
        for name, path in self.binaries.items():
            path.write_text(name)
        self.enterContext(mock.patch.object(pairs, 'pinned_node', return_value=('/pinned/node', '24.20.0')))
        self.fixture = self.enterContext(mock.patch.object(pairs, 'fixture_identity', return_value='a' * 64))
        self.enterContext(mock.patch.object(pairs.subprocess, 'check_output', return_value='revision\n'))
        self.enterContext(mock.patch('builtins.print'))

    def cli(self, output, **options):
        args = ['--control', str(self.binaries['control']), '--candidate', str(self.binaries['candidate']),
                '--mode', 'async', '--task', 'position', '--output', str(output)]
        for key, value in options.items():
            args += ['--' + key, str(value)]
        pairs.main(args)

    def test_summary_scores_paired_ratios_and_records_input_identities(self):
        output = self.root / 'screen'
        with mock.patch.object(pairs, 'run_pair', side_effect=[{'ratio': r} for r in (3, 50, 2)]) as run:
            self.cli(output)
        report = json.loads((output / 'summary.json').read_text())
        self.assertTrue(report['complete'])
        self.assertEqual(report['median_ratio'], 3)
        self.assertEqual(report['binaries']['control'], pairs.identity(self.binaries['control']))
        self.assertEqual(report['fixture_sha256'], 'a' * 64)
        self.assertEqual((report['iterations'], report['requested_pairs']), (5, 3))
        self.assertEqual(run.call_count, 3)
        self.assertEqual(run.call_args.args[:5], ('/pinned/node', self.binaries, 'async', 'position', 5))

    def test_failed_pair_keeps_prior_pairs_without_overall_ratio(self):
        output = self.root / 'failure'
        with mock.patch.object(pairs, 'run_pair', side_effect=[{'ratio': 2}, ValueError('wrong answer')]) as run, \
                self.assertRaisesRegex(ValueError, 'wrong answer'):
            self.cli(output)
        report = json.loads((output / 'summary.json').read_text())
        self.assertFalse(report['complete'])
        self.assertEqual(report['pairs'], [{'ratio': 2}])
        self.assertNotIn('median_ratio', report)
        self.assertEqual(run.call_count, 2)

    def test_changed_fixture_leaves_incomplete_result(self):
        self.fixture.side_effect = ['a' * 64, 'b' * 64]
        output = self.root / 'changed'
        with mock.patch.object(pairs, 'run_pair', return_value={'ratio': 2}), \
                self.assertRaisesRegex(ValueError, 'fixture or executable changed'):
            self.cli(output, pairs=1)
        report = json.loads((output / 'summary.json').read_text())
        self.assertFalse(report['complete'])
        self.assertNotIn('median_ratio', report)

    def test_invalid_bounds_or_existing_output_never_launch_clients(self):
        with mock.patch.object(pairs, 'run_pair') as run:
            for option in ('pairs', 'iterations', 'timeout'):
                with self.subTest(option=option), self.assertRaises(SystemExit):
                    self.cli(self.root / option, **{option: 0})
            with self.assertRaises(FileExistsError):
                self.cli(self.root)
        run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
