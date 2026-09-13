"""The inventory must not turn an interrupted or shortened run into completion."""
import copy
import contextlib
import io
from types import SimpleNamespace
from unittest.mock import patch
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s08_p4 as p4


class InventoryProtocol(unittest.TestCase):
    def setUp(self):
        self.request = {'id': 'case#configuration=0', 'acceptance_tier': 'acceptance',
                        'diagnostic_phases': ['config', 'program', 'syntactic', 'semantic', 'global', 'declaration'],
                        'type_baseline_requested': True, 'loading': {}}
        self.row = {'version': 1, 'id': self.request['id'], 'acceptance_tier': 'acceptance',
                    'load': {'state': 'executed', 'graph': {'ID': self.request['id'], 'Files': [{'Name': '/file.ts'}]}},
                    'bind_diagnostics': {'state': 'executed', 'diagnostics': []},
                    'phases': {k: {'state': 'executed', 'diagnostics': []} for k in self.request['diagnostic_phases']},
                    'type_symbol_baselines': {'state': 'not_implemented', 'reason': 'native walker pending'}}
        self.row['phases']['declaration'] = {'state': 'not_implemented', 'reason': 'emit resolver pending'}
        self.row['phases']['semantic'] = {'state': 'failed', 'files': [{'file_hex': b'/file.ts'.hex(),
               'result': {'state': 'failed', 'class': 'unsupported', 'reason': 'function body'}}]}

    def test_frozen_loading_serialization_preserves_path_precedence(self):
        from s07_subset import json_bytes
        import json
        loading = {'options': {'paths': {'z*': ['first/*'], 'a*': ['second/*']}},
                   'config_raw': {'compilerOptions': {'paths': {'b*': ['b'], 'a*': ['a']}}}}
        self.assertEqual(p4.canonical(loading) + b'\n', json_bytes(loading))
        wrapped = {'loading': loading, 'id': 'ordered-paths'}
        decoded = json.loads(p4.canonical(wrapped))
        self.assertEqual(list(decoded['loading']['options']['paths']), ['z*', 'a*'])
        self.assertEqual(list(decoded['loading']['config_raw']['compilerOptions']['paths']), ['b*', 'a*'])

    def test_preserves_unimplemented_declaration_even_without_diagnostics(self):
        p4.validate_row(self.request, self.row)
        report = p4.summarize([self.request], [self.row])
        self.assertEqual(report['tiers']['acceptance']['all_requested_diagnostic_phases_completed'], 0)
        reasons = {r['reason'] for r in report['tiers']['acceptance']['failure_buckets']}
        self.assertEqual(reasons, {'emit resolver pending', 'function body', 'native walker pending'})

    def test_rejects_dropped_phase_and_changed_tier(self):
        row = copy.deepcopy(self.row); del row['phases']['declaration']
        with self.assertRaisesRegex(ValueError, 'phase'): p4.validate_row(self.request, row)
        row = copy.deepcopy(self.row); row['acceptance_tier'] = 'informational'
        with self.assertRaisesRegex(ValueError, 'tier'): p4.validate_row(self.request, row)

    def test_rejects_missing_source_or_success_hiding_failure(self):
        row = copy.deepcopy(self.row); row['phases']['semantic']['files'] = []
        with self.assertRaisesRegex(ValueError, 'file inventory'): p4.validate_row(self.request, row)
        row = copy.deepcopy(self.row); row['phases']['semantic']['state'] = 'executed'
        with self.assertRaisesRegex(ValueError, 'hides a failed'): p4.validate_row(self.request, row)

    def test_informational_failures_do_not_change_acceptance_counts(self):
        request = copy.deepcopy(self.request); request['acceptance_tier'] = 'informational'
        row = p4.fatal(request, 'panic', 'retained native-like panic payload')
        report = p4.summarize([request], [p4.validate_row(request, row)])
        self.assertEqual(report['tiers']['acceptance']['variants'], 0)
        self.assertEqual(report['tiers']['informational']['failure_buckets'][0]['reason'], 'retained native-like panic payload')

    def test_requested_baseline_may_not_be_disabled(self):
        row = copy.deepcopy(self.row); row['type_symbol_baselines'] = {'state': 'not_requested'}
        with self.assertRaisesRegex(ValueError, 'silently dropped'): p4.validate_row(self.request, row)

    def test_replay_requires_exact_binary_input_and_raw_artifacts(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); case = root / 'cases/00000'; case.mkdir(parents=True)
            requests = [self.request]
            metadata = {'requests_sha256': p4.digest(p4.canonical(requests) + b'\n')}
            p4.write_new(root / 'requests.json', requests); p4.write_new(root / 'capture.json', metadata)
            _, rows, report = p4.replay(root, True)
            self.assertFalse(report['complete']); self.assertEqual(rows, [])
            with self.assertRaisesRegex(ValueError, 'incomplete'): p4.replay(root)
            (case / 'stderr').write_bytes(b'panic details')
            result = {'request_sha256': p4.digest(p4.canonical(self.request) + b'\n'),
                      'capture_sha256': p4.digest(p4.canonical(metadata) + b'\n'),
                      'artifacts': {'stderr': p4.digest(b'panic details')}, 'row': self.row}
            p4.write_new(case / 'result.json', result)
            self.assertTrue(p4.replay(root)[2]['complete'])
            (case / 'stderr').write_bytes(b'changed reason')
            with self.assertRaisesRegex(ValueError, 'artifact changed'): p4.replay(root)

    def test_real_process_exit_timeout_and_resume_retain_raw_failures(self):
        for source, timeout, expected in [
            ("import sys; print('details', file=sys.stderr); sys.exit(7)", 3, 'process_exit'),
            ("import time; time.sleep(2)", 0.1, 'timeout'),
            ("import sys; open(sys.argv[2], 'w').write('{}')", 3, 'harness_protocol'),
        ]:
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                binary = root / 'adapter'
                binary.write_text('#!/usr/bin/env python3\n' + source + '\n')
                binary.chmod(0o755)
                build = {'version': 1, 'sources': {}, 'binary': str(binary),
                         'binary_sha256': p4.digest(binary.read_bytes())}
                p4.write_new(root / 'build.json', build)
                manifest = root / 'manifest.json'; manifest.write_text('{}')
                args = SimpleNamespace(loading_requests=None, tier='acceptance', case=[],
                    build_record=root / 'build.json', output=root / 'capture', timeout=timeout, resume=False)
                with patch.object(p4, 'inventory', return_value=[self.request]), patch.object(p4, 'sources', return_value={}), patch.object(p4, 'MANIFEST', manifest), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                    p4.run(args)
                    result = p4.read(args.output / 'cases/00000/result.json')
                    self.assertEqual(result['row']['fatal']['class'], expected)
                    self.assertIn('stderr', result['artifacts'])
                    original = (args.output / 'cases/00000/result.json').read_bytes()
                    args.resume = True
                    p4.run(args)
                    self.assertEqual((args.output / 'cases/00000/result.json').read_bytes(), original)
                    args.timeout += 1
                    with self.assertRaisesRegex(ValueError, 'identical'): p4.run(args)

    def test_replay_refuses_gap_followed_by_completion(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); (root / 'cases/00001').mkdir(parents=True)
            requests = [self.request, {**self.request, 'id': 'second'}]
            p4.write_new(root / 'requests.json', requests)
            p4.write_new(root / 'capture.json', {'requests_sha256': p4.digest(p4.canonical(requests) + b'\n')})
            p4.write_new(root / 'cases/00001/result.json', {})
            with self.assertRaisesRegex(ValueError, 'noncontiguous'): p4.replay(root, True)

    def test_resume_uses_verified_snapshot_after_workspace_edits(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / 'adapter'
            binary.write_text('#!/usr/bin/env python3\nimport sys;sys.exit(7)\n')
            binary.chmod(0o755)
            snapshot = root / 'source-snapshot'; snapshot.mkdir()
            (snapshot / 'checker.rs').write_bytes(b'original')
            sources = {'checker.rs': p4.digest(b'original')}
            build = {'version': 1, 'sources': sources, 'source_snapshot': str(snapshot),
                     'binary': str(binary), 'binary_sha256': p4.digest(binary.read_bytes())}
            p4.write_new(root / 'build.json', build)
            manifest = root / 'manifest.json'; manifest.write_text('{}')
            args = SimpleNamespace(loading_requests=None, tier='acceptance', case=[],
                build_record=root / 'build.json', output=root / 'capture', timeout=3, resume=False)
            with patch.object(p4, 'inventory', return_value=[self.request]), patch.object(p4, 'sources', return_value=sources) as current, patch.object(p4, 'MANIFEST', manifest), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                p4.run(args)
                original = (args.output / 'cases/00000/result.json').read_bytes()
                current.return_value = {'checker.rs': p4.digest(b'new implementation')}
                args.resume = True
                p4.run(args)
                self.assertEqual((args.output / 'cases/00000/result.json').read_bytes(), original)
                (args.output / 'source-snapshot/checker.rs').write_bytes(b'tampered')
                with self.assertRaisesRegex(ValueError, 'source snapshot differs'): p4.run(args)
                args.resume = False
                with self.assertRaisesRegex(ValueError, 'source tree differs'): p4.run(args)

    def test_source_snapshot_rejects_changed_missing_and_extra_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            snapshot = Path(tmp)
            source = snapshot / 'checker.rs'
            source.write_bytes(b'original')
            expected = {'checker.rs': p4.digest(b'original')}
            p4.verify_source_snapshot(snapshot, expected)
            for mutation in ('changed', 'missing', 'extra'):
                source.write_bytes(b'original')
                extra = snapshot / 'unexpected.rs'
                if extra.exists(): extra.unlink()
                if mutation == 'changed': source.write_bytes(b'changed')
                elif mutation == 'missing': source.unlink()
                else: extra.write_bytes(b'extra')
                with self.assertRaisesRegex(ValueError, 'source snapshot differs'):
                    p4.verify_source_snapshot(snapshot, expected)


if __name__ == '__main__': unittest.main()
