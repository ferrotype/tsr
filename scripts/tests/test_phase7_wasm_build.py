"""Compiler wasm freshness resolves its complete locked dependency graph."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('phase7_wasm_build', ROOT / 'tools/phase7/portable/wasm-compiler/build.py')
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


class WasmBuildTests(unittest.TestCase):
    def test_prefetch_includes_inactive_platforms_and_propagates_failure(self):
        with mock.patch.object(BUILD.subprocess, 'run') as run:
            BUILD.prefetch_registry()
        run.assert_called_once_with(
            ['cargo', 'fetch', '--locked', '--manifest-path', str(BUILD.HERE / 'Cargo.toml')],
            cwd=ROOT, check=True)
        # No target filter: windows-link must be available even on Linux.
        with mock.patch.object(BUILD.subprocess, 'run', side_effect=subprocess.CalledProcessError(101, ['cargo', 'fetch'])):
            with self.assertRaises(subprocess.CalledProcessError):
                BUILD.prefetch_registry()

    def test_snapshots_remain_offline_and_detect_local_compiler_changes(self):
        with tempfile.TemporaryDirectory(prefix='phase7-freshness-', dir=ROOT) as temporary:
            package = Path(temporary)
            (package / 'src').mkdir()
            (package / 'Cargo.toml').write_text('[package]\nname="freshness-witness"\n')
            source = package / 'src/lib.rs'
            source.write_text('pub fn observed() -> u8 { 1 }\n')
            metadata = json.dumps({'packages': [
                {'source': None, 'manifest_path': str(package / 'Cargo.toml')},
                {'source': 'registry+example', 'manifest_path': '/unused/windows-link/Cargo.toml'},
            ]})
            with mock.patch.object(BUILD.subprocess, 'check_output', return_value=metadata) as query:
                before = BUILD.source_snapshot()
                source.write_text('pub fn observed() -> u8 { 2 }\n')
                after = BUILD.source_snapshot()
            self.assertNotEqual(before[str(source.relative_to(ROOT))], after[str(source.relative_to(ROOT))])
            self.assertEqual(before.keys(), after.keys())
            for call in query.call_args_list:
                self.assertEqual(call.args[0], ['cargo', 'metadata', '--locked', '--offline', '--format-version=1',
                                               '--manifest-path', str(BUILD.HERE / 'Cargo.toml')])


if __name__ == '__main__':
    unittest.main()
