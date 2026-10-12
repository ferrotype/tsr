"""Current external lifetime roster and retained-output controls; no Cargo runs."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s10_lifetime as lifetime
import s10_corpus as corpus


def output(cases):
    return ('running ' + str(len(cases)) + ' tests\n'
            + ''.join('test ' + case + ' ... ok\n' for case in cases)
            + f'test result: ok. {len(cases)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n').encode()


class LifetimeControls(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.spec = {'version': 1, 'consumer': 'consumer/Cargo.toml', 'test': 'lifetime',
                     'modes': ['debug', 'release', 'miri', 'address_sanitizer'],
                     'cases': ['emit_retains_sources', 'load_releases_storage']}
        (self.root / 'tools/s10').mkdir(parents=True)
        (self.root / 'consumer/tests').mkdir(parents=True)
        self.source = self.root / 'consumer/tests/lifetime.rs'
        self.source.write_text('#[test]\nfn load_releases_storage() {}\n\n#[test]\nfn emit_retains_sources() {}\n')
        self.save_spec()

    def save_spec(self):
        (self.root / 'tools/s10/lifetime-cases.json').write_text(json.dumps(self.spec))

    def test_current_manifest_names_all_sixteen_plain_integration_tests(self):
        spec = lifetime.manifest()
        self.assertEqual(spec['cases'], [
            'callback_order_and_inline_maps_are_observed_independently_of_output_files',
            'compiler_diagnostics_and_queries_share_the_emit_pool',
            'compiler_request_cancellation_preserves_native_emit_and_checker_contracts',
            'compiler_scope_panic_retires_both_checker_roots',
            'concurrent_first_queries_share_a_checker_identity',
            'emit_refuses_a_target_file_from_another_program',
            'emitter_records_and_write_metadata_retain_their_source_owners',
            'malformed_source_keeps_original_bytes_and_resolvable_diagnostics',
            'native_query_unwind_preserves_payload_and_invalidates_the_session',
            'public_ownership_retained_results_and_generated_syntax_keep_exact_owners',
            'public_ownership_shared_file_survives_concurrent_snapshot_edit',
            'reentry_fails_and_retirement_invalidates_retained_queries_and_emit',
            'repeated_load_check_query_emit_drop_releases_tracked_storage',
            'retained_type_survives_session_drop_and_rejects_a_foreign_session',
            'retirement_before_initialization_refuses_a_query',
            'transpile_results_retain_sources_and_follow_pinned_cancellation',
        ])
        self.assertTrue(all('::' not in case for case in spec['cases']))

    def test_supporting_inventory_rejects_unlisted_source_case(self):
        self.spec['supporting_cases'] = {'services': ['mapped_sources']}
        (self.root / 'consumer/tests/services.rs').write_text('#[test]\nfn different_case() {}\n')
        self.save_spec()
        with self.assertRaisesRegex(ValueError, 'supporting lifetime inventory'):
            lifetime.manifest(self.root)

    def test_source_identity_excludes_nested_build_outputs_but_keeps_consumer_code(self):
        source = self.root / 'tools/s10/consumer/src/main.rs'
        source.parent.mkdir(parents=True)
        source.write_text('fn main() {}')
        output = self.root / 'tools/s10/consumer/target/debug/build/generated.rs'
        output.parent.mkdir(parents=True)
        output.write_bytes(b'build output')
        with patch.object(corpus, 'ROOT', self.root), \
             patch.object(corpus, 'source_patterns', return_value=['tools/s10/**/*']):
            before = corpus.sources()
            self.assertIn('tools/s10/consumer/src/main.rs', before)
            self.assertFalse(any('target' in Path(name).parts for name in before))
            output.write_bytes(b'different build output')
            self.assertEqual(corpus.sources(), before)
            source.write_text('fn main() { panic!(); }')
            self.assertNotEqual(corpus.sources(), before)

    def test_lifetime_inputs_exclude_browser_and_docs_but_include_shared_rust(self):
        inputs = {name: 'old' for name in (
            'crates/tsr_embed/src/session.rs', 'crates/tsr_embed/README.md',
            'crates/tsr_bundled/bundled/libs/lib.d.ts', 'crates/tsr_embed/Cargo.toml',
            'tools/s10/rust-consumer/tests/lifetime.rs', 'tools/s10/rust-consumer/Cargo.lock',
            'tools/phase7/portable/compiler_consumer.rs', 'tools/phase7/portable/browser/run.mjs',
            'tools/phase7/portable/wasm-compiler/build.py', 'scripts/s10_lifetime.py',
        )}
        with patch.object(lifetime, 'corpus_sources', return_value=inputs):
            before = lifetime.sources()
            self.assertEqual(len(before), 7)
            for name in inputs.keys() - before.keys():
                inputs[name] = 'unrelated'
            self.assertEqual(lifetime.sources(), before)
            inputs['tools/phase7/portable/compiler_consumer.rs'] = 'changed'
            self.assertNotEqual(lifetime.sources(), before)

    def test_missing_case_is_named_even_when_remaining_inventory_is_valid(self):
        self.spec['cases'].remove('emit_retains_sources')
        self.save_spec()
        with self.assertRaisesRegex(ValueError, r"missing: \['emit_retains_sources'\]"):
            lifetime.manifest(self.root)

    def test_added_or_renamed_source_tests_cannot_silently_escape_the_roster(self):
        self.source.write_text(self.source.read_text() + '#[test]\nfn new_owner_contract() {}\n')
        with self.assertRaisesRegex(ValueError, 'new_owner_contract'):
            lifetime.manifest(self.root)
        self.source.write_text(self.source.read_text().replace('load_releases_storage', 'replacement_contract'))
        with self.assertRaises(ValueError) as failure:
            lifetime.manifest(self.root)
        self.assertIn('replacement_contract', str(failure.exception))
        self.assertIn('load_releases_storage', str(failure.exception))

    def test_unsupported_declarations_and_empty_or_duplicate_manifests_fail_closed(self):
        self.source.write_text('#[test]\nasync fn async_case() {}\n')
        with self.assertRaisesRegex(ValueError, 'unsupported/empty'):
            lifetime.manifest(self.root)
        for cases in ([], ['lifetime::old_case'], ['one_case', 'one_case']):
            self.spec['cases'] = cases
            self.save_spec()
            with self.assertRaisesRegex(ValueError, 'inventory'):
                lifetime.manifest(self.root)

    def test_output_requires_each_exact_case_once_and_no_ignored_filtered_or_empty_run(self):
        cases = self.spec['cases']
        complete = output(cases)
        row = b'test emit_retains_sources ... ok\n'
        mutations = [complete.replace(row, b''), complete + row,
                     complete.replace(b'emit_retains_sources ... ok', b'emit_retains_sources ... ignored'),
                     complete.replace(b'emit_retains_sources ... ok', b'emit_retains_sources ... FAILED'),
                     complete.replace(b'0 filtered out', b'1 filtered out'), output([])]
        with contextlib.redirect_stderr(io.StringIO()):
            lifetime.validate_output(complete, cases, 'debug')
            for changed in mutations:
                with self.subTest(changed=changed), self.assertRaises(ValueError):
                    lifetime.validate_output(changed, cases, 'debug')
            with self.assertRaisesRegex(ValueError, 'missing: emit_retains_sources'):
                lifetime.validate_output(complete.replace(row, b''), cases, 'release')

    def test_existing_capture_and_target_output_refused_before_tools_run(self):
        existing = self.root / 'existing'
        existing.mkdir()
        marker = existing / 'unchanged'
        marker.write_bytes(b'original')
        with patch.object(lifetime.subprocess, 'check_output') as invoke:
            with self.assertRaisesRegex(ValueError, 'new directory'):
                lifetime.capture(existing)
            with self.assertRaisesRegex(ValueError, 'outside target'):
                lifetime.capture(self.root / 'target/new')
            invoke.assert_not_called()
        self.assertEqual(marker.read_bytes(), b'original')

    def receipt(self):
        directory = self.root / 'receipt'
        directory.mkdir()
        rows = {}
        for mode in self.spec['modes']:
            log = directory / (mode + '.log')
            log.write_bytes(output(self.spec['cases']))
            rows[mode] = {'exit_code': 0, 'log_sha256': lifetime.file_digest(log)}
        record = {'sources': {'fixture.rs': 'fixture-digest'}, 'source_stable': True,
                  'spec': self.spec, 'modes': rows}
        (directory / 'capture.json').write_text(json.dumps(record))
        return directory, record

    def test_verify_rejects_missing_mode_and_does_not_turn_empty_logs_into_a_pass(self):
        directory, record = self.receipt()
        with patch.object(lifetime, 'manifest', return_value=self.spec), \
             patch.object(lifetime, 'sources', return_value=record['sources']), \
             contextlib.redirect_stderr(io.StringIO()):
            self.assertTrue(lifetime.verify(directory)['metrics']['lifetime_checks'])
            log = directory / 'miri.log'
            log.write_bytes(output([]))
            record['modes']['miri']['log_sha256'] = lifetime.file_digest(log)
            (directory / 'capture.json').write_text(json.dumps(record))
            report = lifetime.verify(directory)
            self.assertFalse(report['metrics']['lifetime_checks'])
            self.assertFalse(report['modes']['miri'])
            record['modes'] = {}
            (directory / 'capture.json').write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, 'incomplete ownership mode inventory'):
                lifetime.verify(directory)

    def test_supporting_case_missing_log_or_failed_execution_cannot_pass(self):
        self.spec['supporting_cases'] = {'services': ['mapped_sources']}
        directory, record = self.receipt()
        with patch.object(lifetime, 'manifest', return_value=self.spec), \
             patch.object(lifetime, 'sources', return_value=record['sources']):
            self.assertFalse(lifetime.verify(directory)['metrics']['lifetime_checks'])
            for mode, row in record['modes'].items():
                log = directory / (mode + '.services.mapped_sources.log')
                log.write_bytes(output(['mapped_sources']))
                row['supporting'] = {'services': {'mapped_sources': {
                    'exit_code': 0, 'log_sha256': lifetime.file_digest(log)}}}
            (directory / 'capture.json').write_text(json.dumps(record))
            self.assertTrue(lifetime.verify(directory)['metrics']['lifetime_checks'])
            record['modes']['miri']['supporting']['services']['mapped_sources']['exit_code'] = 1
            (directory / 'capture.json').write_text(json.dumps(record))
            self.assertFalse(lifetime.verify(directory)['metrics']['lifetime_checks'])

    def test_cli_requires_output_and_failed_measured_checks_exit_nonzero(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            lifetime.main(['capture'])
        with patch.object(lifetime, 'verify', return_value={'metrics': {'lifetime_checks': False}}), \
             contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(lifetime.main(['verify', '--output', str(self.root / 'receipt')]), 1)


if __name__ == '__main__':
    unittest.main()
