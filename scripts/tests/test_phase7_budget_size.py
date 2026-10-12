"""Production size identity and readonly recomputation controls; no builds."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tools.phase7.budget import measure, probe, size


class SizeControls(unittest.TestCase):
    def fixture(self, root, runtime='native'):
        bound = {}
        for engine, count in (('go', 20), ('rust', 28)):
            directory = root / engine
            directory.mkdir()
            body = directory / ('budget-go' if engine == 'go' else 'phase7_rust_budget')
            if runtime == 'wasm':
                body = directory / ('budget-go.wasm' if engine == 'go' else 'checker_bg.wasm')
            body.write_bytes(b'a' * count)
            pin = directory / 'data/upstream.json'
            pin.parent.mkdir(); pin.write_text('{"pin":"' + 'a' * 40 + '"}')
            build = directory / 'build.json'
            manifest = {'mode': 'phase7-budget', 'measurement_contract': 2,
                        'revision': 'a' * 40,
                        'performance_eligible': True, 'source_guard': True, 'build_exit': 0,
                        'sources': {'data/upstream.json': probe.identity(pin)['sha256']},
                        'pin': 'a' * 40, 'go_gc_policy': measure.GO_GC_POLICY,
                        'commands': [['go', 'build', '-ldflags=-s -w', './cmd/phase7-budget/native']],
                        'build_command': ['cargo', 'build', '--manifest-path', 'tools/phase7/budget/rust/Cargo.toml'],
                        'allocation_instrumented': False, 'allocator': measure.ALLOCATOR,
                        'source_changes_during_build': [],
                        'profile': {'name': 'release', 'panic': 'unwind', 'lto': 'fat', 'codegen_units': 1},
                        'size': count, 'sha256': probe.identity(body)['sha256'],
                        'artifacts': {body.name: probe.identity(body)}}
            if runtime == 'wasm':
                glue = directory / ('wasm_exec.cjs' if engine == 'go' else 'bindings.mjs')
                glue.write_bytes(b'g' * 3)
                manifest['artifacts'][glue.name] = probe.identity(glue)
                if engine == 'rust':
                    manifest.update(mode='checker', command=['cargo', 'build', '-p', 'tsr_wasm', '--features', 'checker', '--release'])
            build.write_text(json.dumps(manifest))
            files = [probe.identity(build), probe.identity(body), probe.identity(pin)]
            if runtime == 'wasm':
                files.append(probe.identity(glue))
            helpers = [probe.identity(size.ROOT / relative) for relative in size.JS_ADAPTERS[engine]] if runtime == 'wasm' else []
            files += helpers
            bound[engine] = {'manifest': files[0], 'runtime': runtime, 'engine': engine,
                             'revision': manifest['revision'], 'helpers': helpers,
                             'production_eligible': True, 'files': files, 'executable': str(body),
                             'eligibility_reasons': []}
        return bound

    def capture(self, root, bound, runtime='native'):
        artifacts = {engine: Path(bound[engine]['executable']) if runtime == 'native' else
                     Path(bound[engine]['manifest']['path']).parent for engine in ('go', 'rust')}
        # Native fixtures exercise real unchanged/hash validation; wasm mocks
        # only the independent Node executable-resolution guard.
        guard = measure.unchanged if runtime == 'native' else lambda identity: None
        def strip_copy(command, **kwargs):
            self.assertEqual(command[1:-1], ['--strip-all'])
            destination = Path(command[-1])
            data = destination.read_bytes()
            destination.write_bytes(data[:len(data) // 2])
            return subprocess.CompletedProcess(command, 0, b'', b'')
        with patch.object(measure, 'build_identity', side_effect=lambda runtime, engine, *_: bound[engine]), \
             patch.object(measure, 'host_info', return_value={'os': 'test'}), \
             patch.object(measure, 'unchanged', side_effect=guard), \
             patch.object(size.sys, 'platform', 'linux'), \
             patch.object(size.shutil, 'which', return_value='/usr/bin/true'), \
             patch.object(size.subprocess, 'run', side_effect=strip_copy):
            return size.capture(runtime, artifacts['go'], artifacts['rust'],
                                bound['go']['manifest']['path'], bound['rust']['manifest']['path'], root / 'capture')

    def test_native_ratio_recomputes_body_and_retains_manifests(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name).resolve(); bound = self.fixture(root)
            result = self.capture(root, bound)
            self.assertEqual(result['ratios'], {'artifact_bytes': 1.4})
            self.assertEqual(result['revision'], 'a' * 40)
            self.assertEqual(result['samples']['go']['artifact_bytes'], [10])
            self.assertEqual(result['metadata']['artifact_bytes']['rust'], {'body': 14, 'glue': 0, 'total': 14})
            for engine, count in (('go', 20), ('rust', 28)):
                self.assertEqual(Path(bound[engine]['executable']).read_bytes(), b'a' * count)
                normalized = result['metadata']['native_stripping']['artifacts'][engine]
                self.assertEqual(normalized['source'], probe.identity(bound[engine]['executable']))
                self.assertEqual(normalized['output']['bytes'], count // 2)
            with patch.object(size.subprocess, 'run', side_effect=AssertionError('readonly replay')):
                self.assertEqual(size.read_capture(root / 'capture', 'native')['ratios']['artifact_bytes'], 1.4)
            self.assertEqual((root / 'capture/go-build.json').read_bytes(), Path(bound['go']['manifest']['path']).read_bytes())
            Path(bound['rust']['executable']).write_bytes(b'changed')
            with patch.object(measure, 'build_identity', side_effect=lambda runtime, engine, *_: bound[engine]):
                with self.assertRaisesRegex(ValueError, 'changed|differs'):
                    size.read_capture(root / 'capture', 'native')

    def test_readonly_reader_ignores_later_helper_and_source_changes(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name).resolve(); bound = self.fixture(root, 'wasm')
            self.capture(root, bound, 'wasm')
            # These captured pin/source identities stay as provenance only.
            for engine in ('go', 'rust'):
                Path(next(row['path'] for row in bound[engine]['files'] if row['path'].endswith('/data/upstream.json'))).unlink()
            with patch.object(measure, 'build_identity', side_effect=AssertionError('must not inspect current helpers')), \
                 patch.object(measure.subprocess, 'check_output', side_effect=AssertionError('must not spawn Node')):
                self.assertEqual(size.read_capture(root / 'capture', 'wasm')['ratios']['artifact_bytes'], 1.4)

    def test_wasm_glue_is_separate_from_body_ratio(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name).resolve(); result = self.capture(root, self.fixture(root, 'wasm'), 'wasm')
            self.assertEqual(result['ratios']['artifact_bytes'], 1.4)
            for engine, body in (('go', 20), ('rust', 28)):
                glue = 3 + sum((size.ROOT / relative).stat().st_size for relative in size.JS_ADAPTERS[engine])
                self.assertEqual(result['metadata']['artifact_bytes'][engine], {'body': body, 'glue': glue, 'total': body + glue})
            archived = root / 'capture/helpers' / size.JS_ADAPTERS['rust'][0]
            archived.write_bytes(b'tampered adapter')
            with self.assertRaisesRegex(ValueError, 'deployment JS adapter differs'):
                size.read_capture(root / 'capture', 'wasm')

    def test_saved_metadata_and_size_tampering_are_rejected(self):
        for member in ('go-build.json', 'size.json'):
            with self.subTest(member=member), tempfile.TemporaryDirectory() as name:
                root = Path(name).resolve(); bound = self.fixture(root); self.capture(root, bound)
                path = root / 'capture' / member
                if member == 'size.json':
                    report = probe.decode(path.read_bytes()); report['artifact_bytes']['rust']['body'] = 1
                    probe.write(path, report)
                else:
                    path.write_text('{}')
                with patch.object(measure, 'build_identity', side_effect=lambda runtime, engine, *_: bound[engine]):
                    with self.assertRaisesRegex(ValueError, 'metadata changed|saved size'):
                        size.read_capture(root / 'capture', 'native')

    def test_missing_mismatched_or_tampered_stripping_is_rejected(self):
        for mutation in ('missing', 'legacy', 'flags', 'command', 'source', 'output'):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as name:
                root = Path(name).resolve(); bound = self.fixture(root); self.capture(root, bound)
                path = root / 'capture/size.json'
                report = probe.decode(path.read_bytes())
                row = report['native_stripping']['artifacts']['rust']
                if mutation == 'missing':
                    del report['native_stripping']
                elif mutation == 'legacy':
                    report['version'] = 1
                elif mutation == 'flags':
                    report['native_stripping']['flags'] = ['--strip-debug']
                elif mutation == 'command':
                    row['command'].remove('--strip-all')
                elif mutation == 'source':
                    row['source']['sha256'] = '0' * 64
                else:
                    Path(row['output']['path']).write_bytes(b'corrupted')
                probe.write(path, report)
                with self.assertRaisesRegex(ValueError, 'stripping|artifact differs'):
                    size.read_capture(root / 'capture', 'native')

    def test_strip_failure_propagates_and_preserves_originals(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name).resolve(); bound = self.fixture(root)
            output = root / 'capture'; output.mkdir()
            with patch.object(size.sys, 'platform', 'linux'), \
                 patch.object(size.shutil, 'which', return_value='/usr/bin/true'), \
                 patch.object(size.subprocess, 'run', return_value=subprocess.CompletedProcess('strip', 1, b'', b'strip failure')):
                with self.assertRaisesRegex(ValueError, 'strip exited 1: strip failure'):
                    size.strip_native(bound, output)
            self.assertEqual(Path(bound['go']['executable']).read_bytes(), b'a' * 20)
            self.assertEqual(Path(bound['rust']['executable']).read_bytes(), b'a' * 28)
            self.assertFalse((output / 'size.json').exists())

    def test_strip_tool_replacement_aborts_before_measuring_the_other_runtime(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name).resolve(); bound = self.fixture(root)
            output = root / 'capture'; output.mkdir()
            tool = root / 'llvm-strip'; tool.write_bytes(b'original tool')
            def replace_tool(command, **kwargs):
                tool.write_bytes(b'replaced tool')
                return subprocess.CompletedProcess(command, 0, b'', b'')
            with patch.object(size.subprocess, 'run', side_effect=replace_tool) as run:
                with self.assertRaisesRegex(ValueError, 'strip tool changed'):
                    size.strip_native(bound, output, tool)
            self.assertEqual(run.call_count, 1)

    def test_output_rejects_existing_target_and_symlink_parent(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name).resolve(); target = root / 'TaRgEt'; target.mkdir()
            alias = root / 'alias'; alias.symlink_to(target, target_is_directory=True)
            for path in (root, target / 'new', alias / 'new'):
                with self.assertRaises(ValueError):
                    size.new_output(path)

    def test_prototype_and_unstripped_native_are_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name).resolve(); bound = self.fixture(root)
            paths = [bound[e]['executable'] for e in ('go', 'rust')] + [bound[e]['manifest']['path'] for e in ('go', 'rust')]
            with patch.object(measure, 'build_identity', side_effect=lambda runtime, engine, *_: bound[engine]):
                bound['go']['production_eligible'] = False
                with self.assertRaisesRegex(ValueError, 'eligible'):
                    size.identities('native', *paths)
                bound['go']['production_eligible'] = True
                Path(paths[2]).write_text('{"commands":[]}')
                with self.assertRaisesRegex(ValueError, 'stripped'):
                    size.identities('native', *paths)

    def test_test_compiler_wasm_manifest_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name).resolve(); build = root / 'build.json'
            build.write_text('{"mode":"portable-compiler-wasm"}')
            with self.assertRaisesRegex(ValueError, 'production tsr_wasm'):
                measure.build_identity('wasm', 'rust', root, build)


if __name__ == '__main__':
    unittest.main()
