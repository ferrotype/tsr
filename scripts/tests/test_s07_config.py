"""Counterexamples for binding config observations to their real source inputs."""
import copy
import contextlib
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s07_config as config


def string(value):
    return {'hex': value.encode().hex(), 'kind': 'string'}


def object_value(entries):
    return {'entries': [{'key': key.encode().hex(), 'value': value}
                        for key, value in entries], 'kind': 'object'}


class ConfigProducerTests(unittest.TestCase):
    def test_glob_dependency_is_in_both_config_and_program_fingerprints(self):
        import s07_program_compare as program
        path = "crates/tsr_glob/src/lib.rs"
        self.assertIn(path, program.config_provenance_inputs()["rust_inputs"])
        self.assertIn(path, program.input_fingerprints())

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.upstream = Path(self.directory.name)
        physical = b'// @target: esnext\nlet value = 1;\n'
        text = b'let value = 1;\n'
        (self.upstream / 'input.ts').write_bytes(physical)
        self.identity = 'case:input.ts#configuration=0'
        self.case = {
            'path': 'input.ts', 'raw_sha256': hashlib.sha256(physical).hexdigest(),
            'loaded_sha256': hashlib.sha256(physical).hexdigest(),
            'configurations': [{'target': 'esnext'}], 'symlinks': {},
            'current_directory': '', 'global_options': {},
            'variants': [{'current_directory': '/.src'}],
            'units': [{'name': 'input.ts', 'file_options': {}, 'script_kind': 3,
                       'extracted_bytes': len(text), 'extracted_sha256': hashlib.sha256(text).hexdigest()}],
        }
        self.cases = {'case:input.ts': self.case}
        self.loading = [{'id': self.identity, 'options': {'target': 99}, 'roots': ['/.src/input.ts']}]
        self.selectors = config.selectors_for(self.loading, self.cases)
        self.requests = [{
            'id': self.identity, 'path': 'input.ts', 'raw_sha256': self.case['raw_sha256'],
            'physical_hex': physical.hex(), 'loaded_sha256': self.case['loaded_sha256'],
            'configuration': 0, 'settings': {'target': 'esnext'}, 'symlinks': {},
            'cwd': '/.src', 'config_cwd': '/.src', 'case_sensitive': True, 'run_external_code': False,
            'units': [{'name': 'input.ts', 'file_options': {}, 'script_kind': 3, 'text_hex': text.hex()}],
        }]
        self.go = [{'id': self.identity, 'options': object_value([
            ('target', {'kind': 'integer', 'value': 99})]), 'root_file_names': ['/.src/input.ts'.encode().hex()]}]

    def validate(self):
        config.validate_fixture_requests(self.requests, self.selectors, self.go,
                                         self.loading, self.cases, self.upstream)

    def test_accepts_exact_physical_transport_and_options(self):
        self.validate()

    def test_rejects_each_changed_source_boundary(self):
        mutations = {
            'physical_hex': '00', 'raw_sha256': '0' * 64, 'loaded_sha256': '0' * 64,
            'settings': {}, 'configuration': 1, 'path': 'else.ts', 'symlinks': {'x': 'y'},
            'cwd': '/changed', 'config_cwd': '/changed', 'run_external_code': True,
            'case_sensitive': False, 'units': [],
        }
        original = copy.deepcopy(self.requests[0])
        for key, value in mutations.items():
            with self.subTest(field=key):
                self.requests[0] = {**original, key: value}
                with self.assertRaises(ValueError):
                    self.validate()

    def test_rejects_changed_physical_file_even_when_request_is_unchanged(self):
        (self.upstream / 'input.ts').write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'physical'):
            self.validate()

    def test_rejects_each_changed_unit_field(self):
        original = copy.deepcopy(self.requests[0]['units'][0])
        for key, value in {'name': 'other.ts', 'file_options': {'x': 'y'},
                           'script_kind': 4, 'text_hex': '00'}.items():
            with self.subTest(field=key):
                self.requests[0]['units'][0] = {**original, key: value}
                with self.assertRaisesRegex(ValueError, 'transport'):
                    self.validate()

    def test_rejects_options_or_roots_unrelated_to_loader(self):
        self.loading[0]['options']['target'] = 1
        with self.assertRaisesRegex(ValueError, 'loading request boundary'):
            self.validate()
        self.loading[0]['options']['target'] = 99
        self.loading[0]['roots'].reverse()
        self.loading[0]['roots'].append('/.src/other.ts')
        with self.assertRaisesRegex(ValueError, 'loading request boundary'):
            self.validate()

    def test_rejects_paths_order_drift(self):
        self.loading[0]['options']['paths'] = {'second': ['b'], 'first': ['a']}
        array = lambda value: {'kind': 'array', 'nil': False, 'values': [string(value)]}
        self.go[0]['options']['entries'].append({
            'key': b'paths'.hex(), 'value': object_value([('first', array('a')), ('second', array('b'))])})
        with self.assertRaisesRegex(ValueError, 'loading request boundary'):
            self.validate()

    def test_rejects_missing_extra_or_duplicate_rows(self):
        for changed in ([], self.requests * 2, [{**self.requests[0], 'id': 'other'}]):
            with self.subTest(rows=len(changed)):
                with self.assertRaises(ValueError):
                    config.validate_fixture_requests(changed, self.selectors, self.go,
                                                     self.loading, self.cases, self.upstream)

    def test_rejects_ids_outside_exact_frozen_variants(self):
        for identity in ('case:input.ts#configuration=1', 'case:input.ts#configuration=00',
                         'case:input.ts#configuration=٠', 'missing#configuration=0', 'case:input.ts'):
            with self.subTest(identity=identity):
                with self.assertRaises(ValueError):
                    config.selectors_for([{'id': identity}], self.cases)

    def test_rejects_noncanonical_hex(self):
        for value in ('A0', 'a', 'a0 ', 10):
            with self.subTest(value=value):
                with self.assertRaises(ValueError):
                    config.canonical_hex(value)

    def test_cargo_report_selects_actual_binary_and_preserves_caller_homes(self):
        record = {'reason': 'compiler-artifact', 'target': {'name': 's07_config'},
                  'executable': '/custom/target/s07_config'}
        environment = {'PATH': os.environ['PATH'], 'CARGO_HOME': '/registry', 'RUSTUP_HOME': '/rustup',
                       'CARGO_TARGET_DIR': '/custom/target', 'CARGO_NET_OFFLINE': 'true',
                       'RUSTFLAGS': '--cfg injected', 'RUSTC_WRAPPER': 'fake', 'CARGO_BUILD_TARGET': 'wrong',
                       'CARGO_PROFILE_DEV_DEBUG_ASSERTIONS': 'false'}
        with patch.dict(os.environ, environment, clear=True), patch.object(config, 'command', return_value=json.dumps(record).encode()) as command:
            binary, env = config.rust_binary()
        self.assertEqual(binary, Path(record['executable']))
        for key in ('CARGO_HOME', 'RUSTUP_HOME', 'CARGO_TARGET_DIR', 'CARGO_NET_OFFLINE'):
            self.assertEqual(env[key], environment[key])
        for key in ('RUSTFLAGS', 'RUSTC_WRAPPER', 'CARGO_BUILD_TARGET', 'CARGO_PROFILE_DEV_DEBUG_ASSERTIONS'):
            self.assertNotIn(key, env)
        self.assertIn('--locked', command.call_args.args[0])

    def test_rejects_missing_or_duplicate_cargo_artifact(self):
        record = {'reason': 'compiler-artifact', 'target': {'name': 's07_config'}, 'executable': '/binary'}
        for output in (b'', (json.dumps(record) + '\n' + json.dumps(record)).encode()):
            with patch.object(config, 'command', return_value=output):
                with self.assertRaisesRegex(ValueError, 'exactly one'):
                    config.rust_binary()

    def full_capture(self, *, mutate=None, mismatch=False):
        """Exercise publication and failure flow with only external processes replaced."""
        loading_path = self.upstream / 'loading.json'
        loading_path.write_text(json.dumps(self.loading))
        frozen_path = self.upstream / 'data/s06/corpus.json'
        frozen_path.parent.mkdir(parents=True)
        frozen_path.write_text(json.dumps({'cases': [{**self.case, 'id': 'case:input.ts', 'kind': 'case'}]}))
        binary = self.upstream / 'native-observer'
        binary.write_bytes(b'native')
        wrapper = self.upstream / 'evidence.json'
        go_rows = [{**self.go[0], 'config_raw': {'kind': 'null'}, 'config_diagnostics': [],
                    'option_diagnostics': [], 'compile_on_save': None}]
        fingerprints = {'go_inputs': {'adapter': 'source'}, 'rust_inputs': {'library': 'current'}}

        def run(args, *, cwd, env):
            if args[0] == 'go':
                Path(env['S07_CONFIG_SOURCE_REQUESTS']).write_text(json.dumps(self.requests))
                Path(env['S07_CONFIG_OUTPUT']).write_text(json.dumps(go_rows))
            else:
                actual = copy.deepcopy(go_rows)
                if mismatch:
                    actual[0]['root_file_names'] = []
                Path(args[2]).write_text(json.dumps(actual))
                if mutate:
                    mutate(fingerprints, loading_path, binary)
            return b''

        with (patch.object(config, 'ROOT', self.upstream),
              patch.object(config, 'oracle_export', return_value=contextlib.nullcontext((self.upstream, {}, 'pin'))),
              patch.object(config, 'provenance', side_effect=lambda: copy.deepcopy(fingerprints)),
              patch.object(config, 'rust_binary', return_value=(binary, {})),
              patch.object(config, 'command', side_effect=run),
              patch.object(config.shutil, 'copyfile'),
              patch('s04.verified_upstream', return_value=self.upstream)):
            config.full(loading_path, wrapper)
        return wrapper

    def test_complete_mismatch_publishes_real_outputs_without_infrastructure_failure(self):
        wrapper = self.full_capture(mismatch=True)
        manifest = json.loads(wrapper.read_text())
        actual = json.loads(Path(manifest['rust_observations']['path']).read_text())
        self.assertEqual(actual[0]['root_file_names'], [])
        differences = json.loads(wrapper.with_suffix('.differences.json').read_text())
        self.assertEqual(differences[0]['id'], self.identity)
        self.assertFalse(differences[0]['passed'])

    def test_changed_production_inputs_cannot_publish_wrapper(self):
        def mutate(fingerprints, loading_path, binary):
            fingerprints['rust_inputs']['library'] = 'changed'
        with self.assertRaisesRegex(ValueError, 'Rust config inputs/binary changed'):
            self.full_capture(mutate=mutate)
        self.assertFalse((self.upstream / 'evidence.json').exists())

    def test_changed_native_executable_cannot_publish_wrapper(self):
        with self.assertRaisesRegex(ValueError, 'Rust config inputs/binary changed'):
            self.full_capture(mutate=lambda inputs, path, binary: binary.write_bytes(b'changed'))
        self.assertFalse((self.upstream / 'evidence.json').exists())

    def test_changed_loading_input_cannot_publish_wrapper(self):
        with self.assertRaisesRegex(ValueError, 'loading inputs changed'):
            self.full_capture(mutate=lambda inputs, path, binary: path.write_bytes(b'[]'))
        self.assertFalse((self.upstream / 'evidence.json').exists())

    def test_rejects_malformed_normalized_options(self):
        invalid = [
            {'kind': 'integer', 'value': True}, {'kind': 'boolean', 'value': 1},
            {'kind': 'array', 'nil': True, 'values': [string('x')]},
            {'kind': 'object', 'entries': [{'key': '70', 'value': string('x')}] * 2},
            {'kind': 'string', 'hex': 'A0'}, {'kind': 'null', 'value': None},
        ]
        for value in invalid:
            with self.subTest(value=value):
                with self.assertRaises(ValueError):
                    config.observed_value(value)

    def test_direct_fixture_check_never_rewrites_changed_observations_or_provenance(self):
        output = self.upstream / 'frozen.json'
        manifest = {'pin': 'source', 'observations_sha256': 'observed'}
        config.publish_direct(output, b'original', manifest, write=True)
        before = {path: path.read_bytes() for path in (output, output.with_suffix('.manifest.json'))}
        config.publish_direct(output, b'original', manifest)
        for data, metadata in ((b'changed', manifest), (b'original', {**manifest, 'pin': 'changed'})):
            with self.assertRaisesRegex(ValueError, 'fixture or provenance drift'):
                config.publish_direct(output, data, metadata)
            self.assertEqual({path: path.read_bytes() for path in before}, before)

    def test_direct_fixture_check_rejects_missing_outputs(self):
        output = self.upstream / 'missing.json'
        with self.assertRaisesRegex(ValueError, 'fixture or provenance drift'):
            config.publish_direct(output, b'new', {})
        self.assertFalse(output.exists())
        self.assertFalse(output.with_suffix('.manifest.json').exists())


if __name__ == '__main__':
    unittest.main()
