"""S07 resume authenticates artifacts and recomputes the complete stage result."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s07_program_compare as loader
import s07_program_replay as replay
import s07_verify_compare as verification
import s07_config as config_capture


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value))


class ProgramReplayTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for name in ('data/upstream.json', 'source.rs', 'tools/s07/program/export_test.go',
                     'tools/s07/verify-options/export_test.go', 'tsc/internal/compiler/program.go', 'scripts/s06_oracle/metadata_bridge.go', 'built-probe'):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('source')
        write(self.root / 'data/upstream.json', {'pin': 'pin'})
        self.requests = self.root / 'requests.json'
        self.rows = [dict(id=name, cwd='/', case_sensitive=True, roots=[], files={}, options={}) for name in ('a', 'b')]
        write(self.requests, self.rows)
        self.go = self.root / 'loader-go.json'
        write(self.go, [dict(ID=row['id'], Files=[]) for row in self.rows])
        self.verify_go = self.root / 'verification-go.json'
        write(self.verify_go, [dict(id=row['id'], diagnostics=[], includes=[], blocked=[]) for row in self.rows])
        write(self.go.with_suffix('.manifest.json'), dict(upstream_pin='pin', requests_sha256=loader.digest(self.requests),
              observations_sha256=loader.digest(self.go), adapter_sha256=loader.digest(self.root / 'tools/s07/program/export_test.go'),
              rows=2, source_sha256={'tsc/internal/compiler/program.go': loader.digest(self.root / 'tsc/internal/compiler/program.go'),
                                    'tsc/internal/compiler/s06_metadata_bridge.go': loader.digest(self.root / 'scripts/s06_oracle/metadata_bridge.go')}))
        write(self.verify_go.with_suffix('.manifest.json'), dict(pin='pin', requests_sha256=loader.digest(self.requests),
              observations_sha256=loader.digest(self.verify_go),
              adapters={name: loader.digest(self.root / name) for name in ('tools/s07/program/export_test.go', 'tools/s07/verify-options/export_test.go')},
              source_sha256=loader.digest(self.root / 'tsc/internal/compiler/program.go')))
        for module in (loader, verification):
            self.enterContext(patch.object(module, 'ROOT', self.root))
            self.enterContext(patch.object(module, 'input_fingerprints', side_effect=lambda: {'source.rs': loader.digest(self.root / 'source.rs')}))
            self.enterContext(patch.object(module, 'rust_binary', return_value=(self.root / 'built-probe', {})))
        self.enterContext(patch.object(replay, 'verified_upstream', return_value=self.root))
        self.enterContext(patch.object(verification, 'verified_upstream', return_value=self.root))
        self.enterContext(patch.object(loader, 'config_provenance_inputs', return_value={'go_inputs': {'source.rs'}, 'rust_inputs': {'source.rs'}}))
        self.config = self.root / 'config.json'
        config = dict(schema=1, operation='config_options', upstream_pin='pin', loading_requests_sha256=loader.digest(self.requests),
                      go_inputs={'source.rs': loader.digest(self.root / 'source.rs')}, rust_inputs={'source.rs': loader.digest(self.root / 'source.rs')})
        config_rows = [dict(id=row['id'], options={}, root_file_names=[], config_raw={}, config_diagnostics=[], option_diagnostics=[], compile_on_save=False) for row in self.rows]
        for name, value in (('source_requests', self.rows), ('go_observations', config_rows), ('rust_observations', config_rows), ('rust_binary', 'binary')):
            path = self.root / (name + '.json')
            write(path, value)
            config[name] = dict(path=path.name, sha256=loader.digest(path))
        write(self.config, config)
        self.enterContext(patch.object(config_capture, 'rust_binary', return_value=(self.root / 'rust_binary.json', {})))
        self.output = self.root / 'loader.json'
        self.verify_output = self.root / 'verification'
        def observed(args, **_):
            native = self.verify_go if '--verify-options' in args else self.go
            Path(args[2]).write_text(''.join(json.dumps(row) + '\n' for row in json.loads(native.read_bytes())))
        # Produce fixtures with the unchanged capture implementations, but replace
        # only the process execution. Replay must reconstruct their exact reports.
        with patch.object(loader, 'command', side_effect=observed), patch.object(verification, 'command', side_effect=observed):
            self.loader_report = loader.check_subset(self.requests, self.go, self.output, self.config)
            self.verify_report = verification.capture(self.requests, self.verify_go, self.verify_output)

    def replay_loader(self):
        return replay.replay_loader(self.requests, self.go, self.output, self.config)

    def replay_verification(self):
        return replay.replay_verification(self.requests, self.verify_go, self.verify_output)

    def test_current_complete_capture_replays_without_observation_processes(self):
        with patch.object(loader, 'command', side_effect=AssertionError('process')), patch.object(verification, 'command', side_effect=AssertionError('process')):
            self.assertEqual(self.replay_loader(), self.loader_report)
            self.assertEqual(self.replay_verification(), self.verify_report)
            self.assertEqual(len(replay.verification_oracle_current(self.requests, self.verify_go)), 2)

    def test_build_inputs_missing_from_old_closure_cannot_reuse_changed_binary(self):
        for name in ('.cargo/config.toml', 'crates/tsr_bundled/bundled/libs/lib.d.ts'):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('changed build input')
            old_binary = (self.root / 'built-probe').read_bytes()
            # Model Cargo rebuilding after an input the old capture did not hash.
            (self.root / 'built-probe').write_text(name)
            for method in (self.replay_loader, self.replay_verification):
                with self.subTest(input=name, method=method.__name__), self.assertRaisesRegex(replay.StaleCapture, 'executable'):
                    method()
            (self.root / 'built-probe').write_bytes(old_binary)

    def test_rebuilt_mutable_config_binary_is_stale_not_corrupt(self):
        (self.root / 'rust_binary.json').write_text('rebuilt config executable')
        with self.assertRaisesRegex(replay.StaleCapture, 'config executable'):
            replay.config_current(self.config, self.requests)
        # A retry must still permit ordinary recapture, even though the shared
        # Cargo output was already overwritten by the first rebuild.
        with self.assertRaisesRegex(replay.StaleCapture, 'config executable'):
            replay.config_current(self.config, self.requests)

    def test_changed_sources_and_requests_are_stale(self):
        for path in (self.root / 'source.rs', self.requests, self.root / 'data/upstream.json'):
            original = path.read_bytes()
            replacement = 'new source' if path.name == 'source.rs' else json.dumps(self.rows[::-1] if path == self.requests else {'pin': 'new-pin'})
            path.write_text(replacement)
            for method in (self.replay_loader, self.replay_verification):
                with self.subTest(path=path.name, method=method.__name__), self.assertRaises(replay.StaleCapture):
                    method()
            path.write_bytes(original)

    def test_loader_overlay_changes_require_recapture(self):
        bridge = self.root / 'scripts/s06_oracle/metadata_bridge.go'
        bridge.write_text('changed overlay')
        with self.assertRaisesRegex(replay.StaleCapture, 'native loader sources'):
            self.replay_loader()

    def test_native_source_and_adapter_changes_are_stale(self):
        for name in ('tsc/internal/compiler/program.go', 'tools/s07/program/export_test.go'):
            path = self.root / name
            original = path.read_bytes()
            path.write_text('new source')
            for method in (self.replay_loader, self.replay_verification):
                with self.subTest(name=name, method=method.__name__), self.assertRaises(replay.StaleCapture):
                    method()
            path.write_bytes(original)

    def test_tampered_raw_bytes_and_binaries_are_corrupt_not_stale(self):
        for path, method in ((self.output.with_suffix('.rust.jsonl'), self.replay_loader),
                             (self.output.with_suffix('.program-probe'), self.replay_loader),
                             (self.verify_output / 'observations.jsonl', self.replay_verification),
                             (self.verify_output / 'program-probe', self.replay_verification)):
            original = path.read_bytes()
            path.write_bytes(original + b'changed')
            with self.subTest(path=path), self.assertRaisesRegex(ValueError, 'artifact hash mismatch') as caught:
                method()
            self.assertNotIsInstance(caught.exception, replay.StaleCapture)
            path.write_bytes(original)

    def test_forged_success_counts_and_rows_are_rejected(self):
        for report, path, method in ((self.loader_report, self.output, self.replay_loader),
                                     (self.verify_report, self.verify_output / 'report.json', self.replay_verification)):
            for field, value in (('rows', []), ('required_variants', 0), ('passed_variants', 1), ('metrics', {})):
                changed = copy.deepcopy(report)
                changed[field] = value
                write(path, changed)
                with self.subTest(method=method.__name__, field=field), self.assertRaisesRegex(ValueError, 'rows or metrics'):
                    method()
            write(path, report)

    def test_complete_failed_observation_remains_false(self):
        actual = self.verify_output / 'observations.jsonl'
        rows = json.loads(self.verify_go.read_bytes())
        rows[0]['diagnostics'] = ['failure']
        actual.write_text(''.join(json.dumps(row) + '\n' for row in rows))
        report = copy.deepcopy(self.verify_report)
        report['rust_sha256'] = loader.digest(actual)
        report['rows'] = verification.compare(self.rows, json.loads(self.verify_go.read_bytes()), rows)
        report['passed_variants'] = 1
        report['metrics']['option_verification'] = False
        write(self.verify_output / 'report.json', report)
        self.assertFalse(self.replay_verification()['metrics']['option_verification'])

    def test_empty_native_inventory_cannot_reuse_success(self):
        write(self.verify_go, [])
        manifest_path = self.verify_go.with_suffix('.manifest.json')
        manifest = json.loads(manifest_path.read_bytes())
        manifest['observations_sha256'] = loader.digest(self.verify_go)
        write(manifest_path, manifest)
        with self.assertRaisesRegex(ValueError, 'nonempty array'):
            replay.verification_oracle_current(self.requests, self.verify_go)

    def test_unstable_capture_and_stale_config_require_recapture(self):
        for report, path, field, value, method in ((self.loader_report, self.output, 'source_changed_during_capture', True, self.replay_loader),
                                                  (self.verify_report, self.verify_output / 'report.json', 'source_stable', False, self.replay_verification)):
            changed = {**report, field: value}
            write(path, changed)
            with self.assertRaises(replay.StaleCapture):
                method()
            write(path, report)
        with patch.object(loader, 'config_provenance_inputs', return_value={'go_inputs': {'source.rs', 'built-probe'}, 'rust_inputs': {'source.rs'}}):
            with self.assertRaises(replay.StaleCapture):
                self.replay_loader()


if __name__ == '__main__':
    unittest.main()
