"""Focused adapter tests, with no Go/Cargo build or corpus execution."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tools.phase5.harness import runner


class RunnerTests(unittest.TestCase):
    def prepared(self, directory, suite='lsp'):
        root = Path(directory)
        for name in ('binary', 'test2json', 'server'):
            (root / name).touch()
        return dict(suite=suite, stage=str(root), cwd=str(root), compiled_sources=['selected.go'], goos='darwin', goarch='arm64',
                    **{name: str(root / name) for name in ('binary', 'test2json', 'server')})

    def test_compiled_lsp_list_filters_direct_and_fixture_dependent_tests(self):
        names = sorted(runner.LSP_TESTS | {'TestReplay', 'TestServerShutdownNoDeadlock'})
        rows = [dict(package='lsp', name=name, source='selected.go:1') for name in names]
        rows.append(dict(package='lsp', name='TestPlatformExcluded', source='excluded_js_test.go:1'))
        with tempfile.TemporaryDirectory() as directory:
            prepared = self.prepared(directory)
            with patch.object(runner.inventory, 'collect', return_value=rows), patch.object(
                    runner.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '\n'.join(names))):
                ids = runner.list_variants('lsp', prepared)
            self.assertEqual(set(ids), {'lsp/' + name for name in runner.LSP_TESTS})
            with patch.object(runner.inventory, 'collect', return_value=rows), patch.object(
                    runner.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, 'TestReplay')):
                with self.assertRaisesRegex(RuntimeError, 'roster disagreement'):
                    runner.list_variants('lsp', prepared)

    def test_native_environment_cannot_select_rust_and_batch_paths_are_isolated(self):
        with tempfile.TemporaryDirectory() as directory:
            prepared = self.prepared(directory)
            local = Path(directory) / 'results'
            identity = 'lsp/TestInitializeCodeActionKinds'
            with patch.dict(os.environ, {'TSR_LSP_SERVER': '/wrong/server', 'TSR_FAULT': 'hover'}), patch.object(
                    runner, 'list_variants', return_value=[identity]), patch.object(
                    runner.supervisor, 'run_batch', return_value=([], {})) as supervise:
                runner.run_batch('lsp', prepared, [identity], local, 3, native=True)
            args, kwargs = supervise.call_args
            self.assertEqual(args[1], ['TestInitializeCodeActionKinds'])
            self.assertNotIn('TSR_LSP_SERVER', kwargs['env'])
            self.assertNotIn('TSR_FAULT', kwargs['env'])
            self.assertEqual(kwargs['env']['TSGO_BASELINE_TRACKING_DIR'], str(local.resolve() / 'tracking'))
            self.assertTrue((local / 'baselines').is_dir())
            with patch.object(runner, 'list_variants', return_value=[identity]):
                with self.assertRaises(ValueError):
                    runner.run_batch('lsp', prepared, [identity, identity], local, 3)
            second = 'lsp/TestProjectInfoInferredProject'
            with patch.object(runner, 'list_variants', return_value=[identity, second]), patch.object(
                    runner.supervisor, 'run_batch', return_value=([], {})) as fresh:
                runner.run_batch('lsp', prepared, [identity, second], local, 3)
            self.assertEqual(fresh.call_count, 2)
            self.assertNotEqual(fresh.call_args_list[0].kwargs['local'], fresh.call_args_list[1].kwargs['local'])
            self.assertEqual(fresh.call_args_list[0].kwargs['env']['TSR_LSP_SERVER'], prepared['server'])

    def test_go_list_records_both_test_packages_and_host(self):
        package = dict(Dir=str(runner.ROOT / 'upstream/tsc/internal/lsp'),
                       TestGoFiles=['internal_test.go'], XTestGoFiles=['external_test.go'])
        results = [subprocess.CompletedProcess([], 0, json.dumps(package)),
                   subprocess.CompletedProcess([], 0, json.dumps(dict(GOOS='darwin', GOARCH='arm64')))]
        with patch.object(runner.subprocess, 'run', side_effect=results):
            selected = runner._source_selection('/pinned/go', 'lsp', '/tmp/stage')
        self.assertEqual(selected['compiled_sources'], [
            'upstream/tsc/internal/lsp/external_test.go',
            'upstream/tsc/internal/lsp/internal_test.go'])
        self.assertEqual((selected['goos'], selected['goarch']), ('darwin', 'arm64'))

    def test_prepare_uses_cargo_artifact_path_and_builds_without_test_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            server = Path(directory) / 'custom-target/release/phase5_testserver'
            artifact = {'reason': 'compiler-artifact', 'target': {'name': 'phase5_testserver'}, 'executable': str(server)}
            with patch.object(runner, '_pinned_go', return_value='/pinned/go'), patch.object(
                    runner, '_source_selection', return_value=dict(compiled_sources=['selected.go'], goos='darwin', goarch='arm64')), patch.object(
                    runner.overlay, 'create', return_value=Path(directory) / 'overlay.json'), patch.object(
                    runner.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, json.dumps(artifact))) as run, patch.object(
                    runner, 'list_variants', return_value=[]):
                prepared = runner.prepare('fourslash', directory)
            self.assertEqual(prepared['server'], str(server.resolve()))
            self.assertEqual(len(run.call_args_list), 3)
            self.assertTrue(all('timeout' not in call.kwargs for call in run.call_args_list))
            self.assertIn('--release', run.call_args_list[-1].args[0])
            self.assertEqual(json.loads((Path(directory) / 'prepared.json').read_text()), prepared)


if __name__ == '__main__':
    unittest.main()
