"""Concurrent LSP screen orchestration; fake runs execute no compiler work."""
import copy
import json
import multiprocessing
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools.phase7.lsp import pairs  # noqa: E402
from scripts.tests.test_phase5_latency import SERVER  # noqa: E402


def scenario():
    uri = '@PROJECT_ROOT_URI@/main.ts'

    def query(character):
        return {'textDocument': {'uri': uri}, 'position': {'line': 0, 'character': character}}

    requests = {name: query(2) for name in pairs.METRICS[1:]}
    requests['references']['context'] = {'includeDeclaration': True}
    requests['rename']['newName'] = 'renamed'
    return {'encoding': 'utf-16', 'open': {'path': 'main.ts', 'languageId': 'typescript', 'version': 1},
            'warmup_hover': query(1), 'edit': {'textDocument': {'uri': uri, 'version': 2},
            'contentChanges': [{'range': {'start': {'line': 0, 'character': 0},
                                         'end': {'line': 0, 'character': 0}}, 'text': ' '}]},
            'requests': requests}


def settlement_probe(command, cwd, selected, output, launch, settled, ready, release, postprocessed):
    """Spawn-safe probe around the production worker's settlement callback."""
    def run(command, cwd, selected, output, *, on_settled):
        if command[0] == 'control':
            if not release.wait(timeout=5):
                raise TimeoutError('test did not release control')
        else:
            ready.set()
        on_settled()
        if command[0] == 'candidate':
            postprocessed.set()
        return dict.fromkeys(pairs.METRICS, 100), {}

    with mock.patch.object(pairs.latency, 'run_runtime', side_effect=run):
        pairs.run_worker(command, cwd, selected, output, launch, settled)


class ConcurrentLspPairTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory())).resolve()
        self.fixture = self.root / 'fixture'
        self.fixture.mkdir()
        (self.fixture / 'main.ts').write_text('const x = 1;')
        (self.fixture / 'alias.ts').symlink_to('main.ts')
        self.control, self.candidate = self.root / 'control', self.root / 'candidate'
        for binary in (self.control, self.candidate):
            binary.write_text(binary.name)
            binary.chmod(0o700)
        self.enterContext(mock.patch.object(pairs.subprocess, 'check_output', return_value='revision\n'))
        self.enterContext(mock.patch('builtins.print'))

    def screen(self, output, count=1):
        return pairs.screen(self.fixture, scenario(), self.control, self.candidate, output, pairs=count)

    def result(self, cwd, output, value=100, change=None):
        output.mkdir(parents=True, exist_ok=True)
        uri = (cwd / 'main.ts').as_uri()
        raw = {
            'encoding': 'utf-16',
            'responses': [{'method': 'textDocument/hover', 'message': {
                'jsonrpc': '2.0', 'id': 1, 'result': {'uri': uri, 'text': 'answer'}}}],
            'executed': [{'jsonrpc': '2.0', 'id': 1, 'method': 'textDocument/hover',
                          'params': {'textDocument': {'uri': uri}}}],
            'traffic': [{'jsonrpc': '2.0', 'method': 'window/logMessage',
                         'params': {'type': 4, 'message': str(cwd / 'main.ts')}},
                        {'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics',
                         'params': {'uri': uri, 'version': 1, 'diagnostics': []}}],
            'received': [], 'wire_frames': [],
        }
        if change:
            change(raw)
        pairs.write_json(output / 'raw.json', raw)
        (output / 'stderr.log').write_text('')
        transcript = pairs.latency.normalize_completion_order(pairs.latency.replay.normalize(raw, cwd))
        return {metric: value * (index + 1) for index, metric in enumerate(pairs.METRICS)}, transcript

    def worker_runs(self, run):
        """Exercise parent summaries/comparison with deterministic, file-backed results."""
        def workers(order, commands, roots, selected, output):
            for name in order:
                artifact = output / name
                artifact.mkdir(parents=True, exist_ok=True)
                record = {'command': commands[name], 'cwd': str(roots[name]), 'launch_ns': 100}
                try:
                    values, transcript = run(commands[name], roots[name], selected, artifact,
                                             on_settled=lambda: None)
                    record['measurements_ns'] = values
                    pairs.write_json(artifact / 'transcript.json', transcript)
                except Exception as error:
                    record['error'] = str(error)
                pairs.write_json(artifact / 'run.json', record)
            return {}
        return mock.patch.object(pairs, 'run_workers', side_effect=workers)

    def test_isolated_roots_and_medians_of_paired_ratios(self):
        counts, calls = {'control': 0, 'candidate': 0}, []
        timings = {'control': [1, 2, 100], 'candidate': [3, 100, 200]}

        def run(command, cwd, selected, output, *, on_settled):
            name = Path(command[0]).name
            index = counts[name]
            counts[name] += 1
            calls.append((command, cwd, copy.deepcopy(selected)))
            self.assertEqual((cwd / 'main.ts').read_text(), 'const x = 1;')
            self.assertTrue((cwd / 'alias.ts').is_symlink())
            self.assertEqual((cwd / 'alias.ts').resolve(), cwd / 'main.ts')
            (cwd / 'main.ts').write_text(name)
            return self.result(cwd, output, timings[name][index])

        output = self.root / 'screen'
        with self.worker_runs(run):
            report = self.screen(output, count=3)
        self.assertTrue(report['complete'])
        self.assertFalse(report['acceptance'])
        self.assertIn('independent spawned Python clients', report['method'])
        self.assertEqual(report['median_ratios'], dict.fromkeys(pairs.METRICS, 3.0))
        self.assertEqual([item['submission_order'] for item in report['pairs']],
                         [['control', 'candidate'], ['candidate', 'control'], ['control', 'candidate']])
        self.assertEqual(counts, {'control': 3, 'candidate': 3})
        self.assertEqual(report['binaries']['candidate'], pairs.binary_identity(self.candidate))
        self.assertEqual(report['fixture_sha256'], pairs.latency.identity(self.fixture))
        roots = [cwd for _, cwd, _ in calls]
        self.assertEqual(len(set(roots)), 6)
        self.assertTrue(all(not cwd.exists() for cwd in roots))
        self.assertEqual((self.fixture / 'main.ts').read_text(), 'const x = 1;')
        for command, _, selected in calls:
            self.assertEqual(command[1:], ['--lsp', '--stdio'])
            self.assertEqual(selected, scenario())
        for index in range(3):
            for name in ('control', 'candidate'):
                artifact = output / f'pair-{index:02d}' / name
                for filename in ('raw.json', 'stderr.log', 'transcript.json', 'run.json'):
                    self.assertTrue((artifact / filename).is_file())

    def test_response_and_all_traffic_differences_reject_both_samples(self):
        cases = {
            'response': lambda raw: raw['responses'][0]['message']['result'].update(text='wrong'),
            'notification': lambda raw: raw['traffic'][0]['params'].update(message='different'),
            'diagnostics': lambda raw: raw['traffic'][1]['params'].update(version=2),
            'executed': lambda raw: raw['executed'][0].update(method='textDocument/references'),
            'omission': lambda raw: raw['executed'].clear(),
            'server-request': lambda raw: raw['traffic'].append({
                'jsonrpc': '2.0', 'id': 5, 'method': 'workspace/configuration', 'params': {}}),
        }
        for name, change in cases.items():
            with self.subTest(name=name):
                output = self.root / name

                def run(command, cwd, selected, artifact, *, on_settled):
                    on_settled()
                    return self.result(cwd, artifact, change=change if Path(command[0]) == self.candidate else None)

                with self.worker_runs(run), \
                        self.assertRaisesRegex(ValueError, 'traffic or responses differ'):
                    self.screen(output)
                report = json.loads((output / 'summary.json').read_text())
                self.assertFalse(report['complete'])
                self.assertNotIn('median_ratios', report)
                self.assertNotIn('ratios', report['pairs'][0])
                for label in ('expected', 'actual'):
                    self.assertTrue((output / 'pair-00/mismatch' / f'{label}.json').is_file())
                for role in ('control', 'candidate'):
                    self.assertTrue((output / 'pair-00' / role / 'raw.json').is_file())

    def test_failure_preserves_both_runs_and_prior_pairs_but_no_overall_ratio(self):
        counts = {'control': 0, 'candidate': 0}

        def run(command, cwd, selected, output, *, on_settled):
            name = Path(command[0]).name
            counts[name] += 1
            on_settled()
            result = self.result(cwd, output)
            if name == 'candidate' and counts[name] == 2:
                raise TimeoutError('response deadline expired')
            return result

        output = self.root / 'failed'
        with self.worker_runs(run), \
                self.assertRaisesRegex(ValueError, 'response deadline expired'):
            self.screen(output, count=3)
        report = json.loads((output / 'summary.json').read_text())
        self.assertEqual(counts, {'control': 2, 'candidate': 2})
        self.assertFalse(report['complete'])
        self.assertNotIn('median_ratios', report)
        self.assertEqual(len(report['pairs']), 2)
        self.assertTrue(report['pairs'][0]['matched'])
        self.assertFalse(report['pairs'][1]['matched'])
        self.assertNotIn('ratios', report['pairs'][1])
        for role in ('control', 'candidate'):
            artifact = output / 'pair-01' / role
            self.assertTrue((artifact / 'raw.json').is_file())
            self.assertTrue((artifact / 'run.json').is_file())

    def test_spawned_clients_overlap_and_return_large_results_through_files(self):
        # Both servers must exist concurrently. A sequential runner fails their
        # rendezvous; PID checks distinguish processes from two client threads.
        server = self.root / 'server.py'
        prefix = """import os, time
from pathlib import Path
cwd = Path.cwd()
assert (cwd / 'main.ts').read_text() == 'const x = 1;'
assert (cwd / 'alias.ts').is_symlink()
ready = cwd.parent / 'ready'
ready.write_text(str(os.getppid()))
other = cwd.parent.parent / ('candidate' if cwd.parent.name == 'control' else 'control') / 'ready'
deadline = time.monotonic() + 5
while not other.exists():
    if time.monotonic() >= deadline: raise RuntimeError('peer never launched')
    time.sleep(0.01)
import sys
print(os.getppid(), file=sys.stderr, flush=True)
"""
        server.write_text(prefix + SERVER.replace(
            '        emit({"id":message["id"],"result":result})',
            '        if message["method"] == "textDocument/references": result = [{"value": "x" * 100}] * 1000\n'
            '        emit({"id":message["id"],"result":result})'))
        output = self.root / 'real-processes'
        commands = {name: [sys.executable, str(server)] for name in ('control', 'candidate')}
        pair = pairs.run_pair(self.fixture, scenario(), commands, output, 0)
        self.assertTrue(pair['matched'], pair['errors'])
        pids = {run['client_pid'] for run in pair['runs'].values()}
        self.assertEqual(len(pids), 2)
        self.assertNotIn(os.getpid(), pids)
        for name, run in pair['runs'].items():
            artifact = output / name
            self.assertEqual(int((artifact / 'stderr.log').read_text()), run['client_pid'])
            self.assertGreater((artifact / 'transcript.json').stat().st_size, 100_000)
            self.assertFalse(Path(run['cwd']).exists())
        self.assertEqual((self.fixture / 'main.ts').read_text(), 'const x = 1;')

    def test_failure_before_settled_hook_releases_peer_and_retains_its_evidence(self):
        server = self.root / 'server.py'
        server.write_text(SERVER)
        commands = {'control': [sys.executable, str(server)], 'candidate': [str(self.root / 'missing')]}
        output = self.root / 'launch-failure'
        pair = pairs.run_pair(self.fixture, scenario(), commands, output, 0)
        self.assertEqual(len(pair['errors']), 1, pair['errors'])
        self.assertIn('candidate: FileNotFoundError:', pair['errors'][0])
        self.assertEqual(set(pair['measurements_ns']['control']), set(pairs.METRICS))
        self.assertTrue((output / 'control/raw.json').is_file())
        self.assertFalse(pair['matched'])
        self.assertNotIn('ratios', pair)

    def test_spawned_worker_waits_for_both_clients_before_postprocessing(self):
        context = multiprocessing.get_context('spawn')
        launch, settled = context.Barrier(2), context.Barrier(2)
        ready, release, postprocessed = context.Event(), context.Event(), context.Event()
        output = self.root / 'settlement'
        workers = [context.Process(target=settlement_probe, args=(
            [name], self.fixture, scenario(), output / name, launch, settled, ready, release, postprocessed))
            for name in ('control', 'candidate')]
        try:
            for worker in workers:
                worker.start()
            self.assertTrue(ready.wait(timeout=5))
            self.assertFalse(postprocessed.wait(timeout=0.1))
            self.assertFalse((output / 'candidate/transcript.json').exists())
            self.assertFalse((output / 'candidate/run.json').exists())
            release.set()
            for worker in workers:
                worker.join(timeout=5)
                self.assertEqual(worker.exitcode, 0)
            self.assertTrue(postprocessed.is_set())
            for name in ('control', 'candidate'):
                self.assertNotIn('error', json.loads((output / name / 'run.json').read_text()))
        finally:
            release.set()
            launch.abort()
            settled.abort()
            for worker in workers:
                if worker.pid is not None:
                    if worker.is_alive():
                        pairs.stop_worker(worker)
                    worker.close()

    @unittest.skipUnless(os.name == 'posix', 'process-group cleanup uses POSIX sessions')
    def test_crashed_client_releases_peer_and_kills_orphaned_server(self):
        import fcntl

        server = self.root / 'server.py'
        server.write_text(SERVER)
        crash = self.root / 'crash.py'
        lease = self.root / 'server.lease'
        crash.write_text("import fcntl, os, signal, sys, time\n"
                         f"lease = open({str(lease)!r}, 'w')\n"
                         "fcntl.flock(lease, fcntl.LOCK_EX)\n"
                         "print(os.getpid(), file=sys.stderr, flush=True)\n"
                         "os.kill(os.getppid(), signal.SIGKILL)\ntime.sleep(60)\n")
        commands = {'control': [sys.executable, str(server)], 'candidate': [sys.executable, str(crash)]}
        output = self.root / 'crash'
        pair = pairs.run_pair(self.fixture, scenario(), commands, output, 0)
        self.assertFalse(pair['matched'])
        self.assertTrue(any('client worker exited with status' in error for error in pair['errors']), pair)
        self.assertTrue((output / 'control/raw.json').is_file())
        self.assertGreater(int((output / 'candidate/stderr.log').read_text()), 0)
        # A kernel-held lease is released on exit, including before an orphan
        # zombie is reaped. This cannot pass because a process probe was denied.
        with lease.open('r') as handle:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_worker_deadline_rejects_pair_and_joins_clients(self):
        server = self.root / 'hang.py'
        server.write_text('import time\ntime.sleep(60)\n')
        commands = {name: [sys.executable, str(server)] for name in ('control', 'candidate')}
        output = self.root / 'timeout'
        with mock.patch.object(pairs, 'WORKER_TIMEOUT', 0.5):
            pair = pairs.run_pair(self.fixture, scenario(), commands, output, 0)
        self.assertFalse(pair['matched'])
        self.assertEqual(len(pair['errors']), 2, pair)
        self.assertTrue(all('client worker deadline expired' in error for error in pair['errors']), pair)
        self.assertFalse(multiprocessing.active_children())

    def test_missing_zero_or_boolean_measurement_is_rejected(self):
        for value in (None, 0, True):
            with self.subTest(value=value):
                output = self.root / f'invalid-{value}'

                def run(command, cwd, selected, artifact, *, on_settled):
                    on_settled()
                    measurements, transcript = self.result(cwd, artifact)
                    if Path(command[0]) == self.candidate:
                        if value is None:
                            measurements.pop('hover')
                        else:
                            measurements['hover'] = value
                    return measurements, transcript

                with self.worker_runs(run), \
                        self.assertRaisesRegex(ValueError, 'positive integer latency'):
                    self.screen(output)
                self.assertNotIn('median_ratios', json.loads((output / 'summary.json').read_text()))

    def test_changed_fixture_or_binary_leaves_incomplete_summary(self):
        for target in (self.fixture / 'main.ts', self.candidate):
            with self.subTest(target=target):
                original = target.read_bytes()

                def run(command, cwd, selected, artifact, *, on_settled):
                    on_settled()
                    result = self.result(cwd, artifact)
                    if Path(command[0]) == self.candidate:
                        target.write_text('changed during the screen')
                    return result

                output = self.root / f'changed-{target.name}'
                with self.worker_runs(run), \
                        self.assertRaisesRegex(ValueError, 'changed during the screen'):
                    self.screen(output)
                report = json.loads((output / 'summary.json').read_text())
                self.assertFalse(report['complete'])
                self.assertNotIn('median_ratios', report)
                target.write_bytes(original)

    def test_invalid_count_existing_output_and_nested_output_start_no_servers(self):
        with mock.patch.object(pairs, 'run_workers') as run:
            for count in (0, -1, True):
                with self.assertRaisesRegex(ValueError, 'positive integer'):
                    self.screen(self.root / 'zero', count=count)
            with self.assertRaises(FileExistsError):
                self.screen(self.root)
            with self.assertRaisesRegex(ValueError, 'outside the fixture'):
                self.screen(self.fixture / 'output')
        run.assert_not_called()

    def test_cli_selects_explicit_inputs_and_defaults_to_ten_pairs(self):
        path = self.root / 'scenario.json'
        pairs.write_json(path, scenario())
        output = self.root / 'cli-output'
        with mock.patch.object(pairs, 'screen') as screen:
            pairs.main(['--control', str(self.control), '--candidate', str(self.candidate),
                        '--fixture', str(self.fixture), '--scenario', str(path), '--output', str(output)])
        screen.assert_called_once_with(self.fixture, scenario(), self.control, self.candidate, output, pairs=10)


if __name__ == '__main__':
    unittest.main()
