"""Bounded CI orchestration checks; these never execute replay or build a CLI."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('replay_ci', ROOT / 'scripts/phase5_replay_ci.py')
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


class ReplayCITests(unittest.TestCase):
    def binaries(self, directory):
        for runtime in ('native', 'rust'):
            (directory / f'{runtime}-lsp').write_text('mock executable')
        return directory

    def expected(self, directory):
        root = directory / 'expected'
        for session in ci.SESSIONS:
            (root / session).mkdir(parents=True)
            for encoding in ci.ENCODINGS:
                (root / session / f'{encoding}.json').write_text('{"response": 1}')
        return root

    def test_all_sessions_and_encodings_compare_ordinary_servers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(ci.replay, 'run', return_value={'response': 1}) as run:
                self.assertTrue(ci.run_matrix(self.binaries(root), root / 'results', self.expected(root)))
            self.assertEqual(run.call_count, 12)
            self.assertEqual({call.args[0].stem for call in run.call_args_list}, set(ci.SESSIONS))
            self.assertEqual({call.args[3] for call in run.call_args_list}, set(ci.ENCODINGS))
            self.assertTrue(all(call.args[1][1:] == ['--lsp', '--stdio'] for call in run.call_args_list))
            self.assertEqual(len(json.loads((root / 'results/summary.json').read_text())), 6)

    def test_prepare_uses_pinned_native_cli_and_cargo_reported_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / 'custom-target/tsrust'
            executable.parent.mkdir()
            executable.write_text('ordinary Rust CLI')
            artifact = {'reason': 'compiler-artifact', 'target': {'name': 'tsrust'}, 'executable': str(executable)}
            with patch.object(ci.runner, '_pinned_go', return_value='/pinned/go'):
                with patch.object(ci.subprocess, 'run', return_value=SimpleNamespace(stdout=json.dumps(artifact))) as run:
                    ci.prepare(root / 'binaries')
            self.assertEqual(run.call_args_list[0].args[0][-1], './cmd/tsc')
            self.assertEqual(run.call_args_list[0].args[0][0], '/pinned/go')
            self.assertNotIn('-overlay', run.call_args_list[0].args[0])
            self.assertIn('--release', run.call_args_list[1].args[0])
            self.assertIn('--locked', run.call_args_list[1].args[0])
            self.assertEqual((root / 'binaries/rust-lsp').read_text(), 'ordinary Rust CLI')

    def test_mismatch_and_capture_failure_fail_without_omitting_later_cases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            observations = [RuntimeError('capture failed'), {'response': 1}, {'response': 1}, {'response': 2}] + [{'response': 1}] * 8
            with patch.object(ci.replay, 'run', side_effect=observations):
                self.assertFalse(ci.run_matrix(self.binaries(root), root / 'results', self.expected(root)))
            rows = json.loads((root / 'results/summary.json').read_text())
            self.assertEqual(len(rows), 6)
            self.assertFalse(rows[0]['matched'])
            self.assertFalse(rows[1]['matched'])
            self.assertTrue(rows[-1]['matched'])
            self.assertTrue((root / 'results/checkjs/utf-8/native/error.txt').is_file())
            self.assertTrue((root / 'results/checkjs/utf-16/mismatch/actual.json').is_file())

    def test_changed_committed_response_fails_even_when_servers_agree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            expected = root / 'expected'
            for session in ci.SESSIONS:
                (expected / session).mkdir(parents=True)
                for encoding in ci.ENCODINGS:
                    source = ci.EXPECTED / session / f'{encoding}.json'
                    (expected / session / f'{encoding}.json').write_text(source.read_text())
            changed = json.loads((expected / 'checkjs/utf-8.json').read_text())
            changed['responses'][0]['message']['result']['capabilities']['hoverProvider'] = False
            (expected / 'checkjs/utf-8.json').write_text(json.dumps(changed))
            def original(session, command, output, encoding):
                return ci.replay.strict_json((ci.EXPECTED / session.stem / f'{encoding}.json').read_text())
            with patch.object(ci.replay, 'run', side_effect=original):
                self.assertFalse(ci.run_matrix(self.binaries(root), root / 'results', expected))
            rows = json.loads((root / 'results/summary.json').read_text())
            self.assertTrue(rows[0]['live_matched'])
            self.assertEqual(rows[0]['expected_matched'], {'native': False, 'rust': False})
            self.assertFalse(rows[0]['matched'])
            self.assertTrue(all(row['matched'] for row in rows[1:]))
            for runtime in ('native', 'rust'):
                mismatch = root / f'results/checkjs/utf-8/expected-mismatch/{runtime}'
                self.assertEqual(json.loads((mismatch / 'expected.json').read_text()), changed)
                actual = json.loads((mismatch / 'actual.json').read_text())
                self.assertTrue(actual['responses'][0]['message']['result']['capabilities']['hoverProvider'])

    def test_missing_or_malformed_expected_response_fails_and_retains_remaining_cases(self):
        for invalid in (None, '{"response": 1, "response": 1}'):
            with self.subTest(invalid=invalid), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                expected = self.expected(root)
                target = expected / 'checkjs/utf-8.json'
                if invalid is None:
                    target.unlink()
                else:
                    target.write_text(invalid)
                with patch.object(ci.replay, 'run', return_value={'response': 1}) as run:
                    self.assertFalse(ci.run_matrix(self.binaries(root), root / 'results', expected))
                rows = json.loads((root / 'results/summary.json').read_text())
                self.assertEqual(run.call_count, 12)
                self.assertIn('expected', rows[0]['errors'])
                self.assertFalse(rows[0]['matched'])
                self.assertTrue(all(row['matched'] for row in rows[1:]))

    def test_workflow_runs_replay_only_in_lsp_shard_and_keeps_failure_artifact(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertEqual(workflow.count('phase5_replay_ci.py prepare'), 1)
        self.assertEqual(workflow.count('phase5_replay_ci.py run'), 1)
        section = workflow.split('  phase5-parity:', 1)[1].split('  parity-check:', 1)[0]
        self.assertEqual(section.count("!cancelled() && matrix.suite == 'lsp'"), 3)
        self.assertIn('name: phase5-replay-results', section)


if __name__ == '__main__':
    unittest.main()
