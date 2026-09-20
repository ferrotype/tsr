import copy
from contextlib import redirect_stderr
import io
import json
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s08_ownership as ownership

ROOT = Path(__file__).resolve().parents[2]


class CheckerMergeOwnership(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.spec, cls.native = ownership.fixture(ROOT)

    def observed(self):
        result = copy.deepcopy(self.native)
        result['rust_ownership'] = {'scope': 'Rust lifetime checks', 'programs': [{
            'id': ownership.PROGRAM, 'state': 'executed',
            'program_survives_retained_result': True, 'retained_display_unchanged': True,
            'program_released_after_result_drop': True}]}
        return result

    def test_projection_keeps_every_merge_action_and_native_pin(self):
        self.assertEqual([p['id'] for p in self.spec['programs']], [ownership.PROGRAM])
        self.assertEqual([m['mode'] for m in self.native['merges']], ownership.p2.MODES)
        self.assertEqual([len(m['observations']) for m in self.native['merges']], [1, 6, 5, 5])
        ownership.validate(self.spec, self.native, self.observed())

    def test_shared_mutation_aliasing_release_and_payload_drift_fail(self):
        for mutate in (
            lambda a: a['merges'].pop(),
            lambda a: a['merges'].append(copy.deepcopy(a['merges'][0])),
            lambda a: a['merges'].reverse(),
            lambda a: a['merges'][2].update(state='unsupported', operation='merge', reason='missing'),
            lambda a: a['merges'][2].update(shared_bound_file_identity=False),
            lambda a: a['merges'][2].update(base_unchanged=False),
            lambda a: a['merges'][2].update(different_merged_symbols=False),
            lambda a: a['merges'][2].update(different_types=False),
            lambda a: a['merges'][2]['observations'][-1].update(after_release_A=False),
            lambda a: a['merges'][2]['observations'][2].update(same_symbol_as_previous=False),
            lambda a: a['merges'][2]['observations'][0]['type'].update(display_hex='77726f6e67'),
            lambda a: a['rust_ownership']['programs'][0].update(program_released_after_result_drop=False),
        ):
            actual = self.observed()
            mutate(actual)
            with self.assertRaises((ValueError, KeyError)):
                ownership.validate(self.spec, self.native, actual)

    def test_modes_are_required_and_failures_do_not_certify_other_criteria(self):
        modes = dict.fromkeys(ownership.MODES, True)
        for changed in ({}, {**modes, 'miri': 1}, {**modes, 'extra': True}):
            with self.assertRaises(ValueError):
                ownership.publish_metrics({'metrics': {}}, changed)
        for failed in ownership.MODES:
            report = {'metrics': {'miri': True, 'address_sanitizer': True}}
            ownership.publish_metrics(report, {**modes, failed: False})
            self.assertIs(report['metrics']['independent_checker_merges'], False)
            self.assertIs(report['metrics']['independent_checker_merges_' + failed], False)
            if failed in ('miri', 'address_sanitizer'):
                self.assertIs(report['metrics'][failed], False)
            self.assertNotIn('checker_result_retention', report['metrics'])
            self.assertNotIn('builder_cache_retention', report['metrics'])

    def test_missing_output_and_native_mismatch_are_measured_failures(self):
        for kind in ('missing', 'mismatch', 'process-failure'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp, redirect_stderr(io.StringIO()):
                directory = Path(temp)
                def invoke(root, args, env):
                    if kind == 'process-failure':
                        raise RuntimeError('child failed')
                    if kind == 'mismatch':
                        actual = self.observed()
                        actual['merges'][2]['observations'][0]['type']['display_hex'] = '77726f6e67'
                        Path(args[-1]).write_text(json.dumps(actual))
                    return b''
                self.assertFalse(ownership.measure(ROOT, invoke, ['cargo'], [], {},
                    self.spec, self.native, directory, 'debug'))

    def test_preexisting_output_cannot_satisfy_an_unexecuted_child(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            (directory / 'debug.json').write_text(json.dumps(self.observed()))
            with self.assertRaisesRegex(ValueError, 'already exists'):
                ownership.measure(ROOT, lambda *args: b'', ['cargo'], [], {},
                    self.spec, self.native, directory, 'debug')

    def test_all_modes_execute_real_example_with_memory_checks(self):
        calls = []
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'data/s04').mkdir(parents=True)
            (root / 'data/s04/toolchains.toml').write_bytes((ROOT / 'data/s04/toolchains.toml').read_bytes())
            (root / 'Cargo.toml').write_bytes((ROOT / 'Cargo.toml').read_bytes())
            def invoke(root, args, env):
                if args == ['rustc', '-Vv']:
                    return b'rustc test\nhost: aarch64-apple-darwin\n'
                if 'setup' in args:
                    return b''
                calls.append((args, env))
                self.assertEqual(args[args.index('--example') + 1], 'p2_checker')
                self.assertEqual(json.loads(Path(args[-2]).read_text()), self.spec)
                if 'miri' in args:
                    raise RuntimeError('instrumented failure')
                Path(args[-1]).write_text(json.dumps(self.observed()))
                return b''
            with patch.object(ownership, 'fixture', return_value=(self.spec, self.native)), redirect_stderr(io.StringIO()):
                outcomes = ownership.merges(root, invoke)
        self.assertEqual(len(calls), 4)
        self.assertEqual(outcomes, {'debug': True, 'release': True, 'miri': False, 'address_sanitizer': True})
        self.assertIn('--release', calls[1][0])
        self.assertIn('-Zmiri-strict-provenance', calls[2][1]['MIRIFLAGS'])
        self.assertNotIn('-Zmiri-disable-validation', calls[2][1]['MIRIFLAGS'])
        self.assertIn('-Zbuild-std', calls[3][0])
        self.assertEqual(calls[3][1]['RUSTFLAGS'], '-Zsanitizer=address')

    def test_e3_composition_preserves_existing_results_and_early_failure(self):
        base = {'metrics': {'miri': True, 'address_sanitizer': True, 'shared_bound_file': True},
                'tests': {'id_exhaustion': 'pass'}}
        with patch.object(ownership.ownership, 'run', return_value=copy.deepcopy(base)), \
                patch.object(ownership, 'merges', return_value=dict.fromkeys(ownership.MODES, True)):
            report = ownership.run(ROOT)
        self.assertTrue(report['metrics']['independent_checker_merges'])
        self.assertEqual(report['tests'], base['tests'])
        early = {'metrics': {'wrong_owner_rejected': False}, 'tests': {'wrong_owner_rejected': 'fail'}}
        with patch.object(ownership.ownership, 'run', return_value=early), \
                patch.object(ownership, 'merges') as merges:
            self.assertEqual(ownership.run(ROOT), early)
            merges.assert_not_called()

    def test_e3_fingerprints_cover_checker_and_native_fixture(self):
        spec = tomllib.loads((ROOT / 'status/runs.toml').read_text())['e3']
        self.assertEqual(spec['command'], ['python3', 'scripts/s08_ownership.py'])
        for name in ('scripts/s08_ownership.py', 'scripts/s08_p2.py', 'scripts/s08_oracle.py',
                     'crates/tsr_checker/**', 'crates/tsr_printer/**', 'crates/tsr_nodebuilder/**',
                     'crates/tsr_transformers/**', ownership.ARCHIVE, ownership.RECORD, 'data/upstream.json'):
            self.assertIn(name, spec['sources'])
        self.assertIn("'run.e3.independent_checker_merges == true'",
                      (ROOT / '.github/workflows/status.yml').read_text())


if __name__ == '__main__':
    unittest.main()
