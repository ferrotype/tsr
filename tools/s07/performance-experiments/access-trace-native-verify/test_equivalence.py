"""Execute the original Python fixture corpus against both decoders unchanged."""
import copy
import gzip
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(HERE))
import run


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


fixtures = load('native_equivalence_original_fixtures', HERE.with_name('access-trace') / 'test_verify.py')
python = fixtures.verify
BINARY = Path(os.environ.get('NATIVE_VERIFY_BINARY', ROOT / 'target/s07-bis-native-verify/release/tsr_s07_access_trace_native_verify')).resolve()
OUTPUT = Path(os.environ.get('NATIVE_VERIFY_TEST_OUTPUT', ROOT / f'target/s07-bis-native-verify/equivalence-{time.time_ns()}'))
OUTPUT.mkdir(parents=True, exist_ok=False)


class Both:
    def __init__(self):
        self.documents = {}
        self.case = 0

    def __getattr__(self, name):
        return getattr(python, name)

    def directory(self):
        self.case += 1
        path = OUTPUT / f'case-{self.case:04d}'
        path.mkdir()
        return path

    def native(self, compressed, documents, config, compressed_limit):
        directory = self.directory()
        trace = directory / 'trace.gz'
        trace.write_bytes(compressed)
        paths = []
        for index, document in enumerate(documents):
            path = directory / f'input-registry-{index}.json'
            run.write_json(path, document)
            paths.append(path)
        try:
            result = run.verify_gzip(BINARY, run.digest(BINARY), trace, paths, directory / 'verification',
                                     config=config, compressed_limit=compressed_limit,
                                     trace_sha256=run.digest(trace))
        except ValueError:
            stderr = directory / 'verification/child.stderr'
            if stderr.is_file() and b'panicked at' in stderr.read_bytes():
                raise AssertionError('native malformed input panicked: ' + str(directory))
            raise
        return result

    def registry(self, documents):
        documents = list(documents)
        error, events = None, None
        try:
            events = python.registry(documents)
        except python.InvalidTrace as caught:
            error = caught
        native_error = None
        try:
            self.native(gzip.compress(fixtures.encoded(fixtures.fixture_records(0))), documents,
                        {'allow_empty': True}, run.DEFAULT_COMPRESSED)
        except ValueError as caught:
            native_error = caught
        if (error is None) != (native_error is None):
            raise AssertionError(f'registry acceptance differs: Python={error}, native={native_error}')
        if error:
            raise error
        self.documents[id(events)] = copy.deepcopy(documents)
        return events

    def verify(self, source, events, **options):
        chunks = []
        while chunk := source.read(python.READ_CHUNK):
            chunks.append(chunk)
        compressed = b''.join(chunks)
        expected, error = None, None
        try:
            expected = python.verify(io.BytesIO(compressed), events, **options)
        except python.InvalidTrace as caught:
            error = caught
        limits = options.get('limits', python.Limits())
        config = {'max_payload': limits.payload_bytes, 'max_block': limits.block_bytes,
                  'max_record': limits.record_bytes, 'max_files': limits.files,
                  'max_active_nodes': limits.active_nodes, 'allow_empty': options.get('allow_empty', False),
                  'progress_records': options.get('progress_records', 1_000_000),
                  'expected': options.get('expected') or {}}
        actual, native_error = None, None
        try:
            actual = self.native(compressed, self.documents[id(events)], config, limits.compressed_bytes)
        except ValueError as caught:
            native_error = caught
        if (error is None) != (native_error is None):
            raise AssertionError(f'trace acceptance differs: Python={error}, native={native_error}; case={self.case}')
        if error:
            raise error
        if actual != expected:
            different = [key for key in set(actual) | set(expected) if actual.get(key) != expected.get(key)]
            raise AssertionError(f'complete reports differ in {different}; case={self.case}')
        return actual

    def verify_file(self, path, registries, max_payload=run.DEFAULT_PAYLOAD, max_compressed=run.DEFAULT_COMPRESSED, **options):
        documents = [python.strict_json(Path(value).read_bytes()) if isinstance(value, (str, Path)) else value
                     for value in registries]
        limits = python.Limits(payload_bytes=max_payload, compressed_bytes=max_compressed,
                               files=options.pop('max_files', python.Limits.files),
                               active_nodes=options.pop('max_active_nodes', python.Limits.active_nodes))
        with Path(path).open('rb') as source:
            return self.verify(source, self.registry(documents), limits=limits, **options)


both = Both()
fixtures.verify = both


class OriginalCorpus(fixtures.TraceVerifierTests):
    """Every original test method runs through the dual-backend adapter."""


class NativeBoundaryTests(unittest.TestCase):
    def test_compiled_field_bounds_exact_value_and_documented_wildcard_site(self):
        documents = [python.strict_json(path.read_bytes()) for path in fixtures.REGISTRIES]
        kind = next(event for event in documents[2]['events'] if event['id'] == 101)
        kind['fields']['b'].update(min=2, max=4, exact_value=3)
        lookup = next(event for event in documents[2]['events'] if event['id'] == 100)
        lookup.pop('sites')
        lookup['site_semantics'] = 'Fixture exercises any encoded u16 site.'
        events = both.registry(documents)
        rows = fixtures.fixture_records()
        rows[11] = fixtures.record(100, site=65535, domain=2, a=fixtures.NODE)
        self.assertTrue(both.verify(io.BytesIO(gzip.compress(fixtures.encoded(rows))), events)['complete'])
        rows[12] = fixtures.record(101, site=101, domain=2, a=fixtures.NODE, b=4)
        with self.assertRaises(python.InvalidTrace):
            both.verify(io.BytesIO(gzip.compress(fixtures.encoded(rows))), events)
        rows[0] = fixtures.record(1, a=2**64 - 1, b=3)
        with self.assertRaises(python.InvalidTrace):
            both.verify(io.BytesIO(gzip.compress(fixtures.encoded(rows))), events)

    def test_native_registry_rejects_duplicate_json_keys_and_nonfinite_literals(self):
        docs = [path.read_bytes() for path in fixtures.REGISTRIES]
        for changed in (docs[0].replace(b'"version": 1', b'"version": 1, "version": 1', 1), b'{"version":NaN}'):
            directory = both.directory()
            paths = []
            for index, raw in enumerate([changed, docs[1], docs[2]]):
                path = directory / f'registry-{index}.json'
                path.write_bytes(raw)
                paths.append(path)
            config = directory / 'config.json'
            run.write_json(config, {'allow_empty': True})
            argv = [str(BINARY), str(config), *map(str, paths)]
            with (directory / 'child.stdout').open('wb') as stdout, (directory / 'child.stderr').open('wb') as stderr:
                result = subprocess.run(argv, input=fixtures.encoded(fixtures.fixture_records(0)), stdout=stdout, stderr=stderr)
            run.write_json(directory / 'receipt.json', {'command': argv, 'returncode': result.returncode})
            self.assertEqual(result.returncode, 2)
            self.assertEqual((directory / 'child.stdout').read_bytes(), b'')

    def test_wrapper_keeps_failed_gzip_receipt_even_with_complete_native_payload(self):
        raw = fixtures.encoded(fixtures.fixture_records())
        compressed = gzip.compress(raw) + gzip.compress(b'')
        with self.assertRaises(ValueError):
            both.native(compressed, [python.strict_json(path.read_bytes()) for path in fixtures.REGISTRIES], {}, run.DEFAULT_COMPRESSED)
        receipt = python.strict_json((OUTPUT / f'case-{both.case:04d}/verification/receipt.json').read_bytes())
        self.assertFalse(receipt['accepted'])
        self.assertIsNotNone(receipt['wrapper_error'])


def tearDownModule():
    run.write_json(OUTPUT / 'corpus-receipt.json', {'version': 1, 'cases': both.case,
        'binary': str(BINARY), 'binary_sha256': run.digest(BINARY),
        'original_fixture_sha256': run.digest(HERE.with_name('access-trace') / 'test_verify.py'),
        'original_verifier_sha256': run.digest(run.REFERENCE),
        'adapter_sha256': run.digest(Path(__file__)),
        'scope': 'Original fixture acceptance and complete successful JSON report equivalence; unittest status is recorded by caller.'})


if __name__ == '__main__':
    unittest.main()
