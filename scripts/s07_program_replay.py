"""Reuse complete S07 program stages only after replaying their recorded evidence."""
from pathlib import Path

from s04 import verified_upstream
from s04_common import strict_json_loads
import s07_program_compare as loader
from s07_program import validate_requests
import s07_verify_compare as verification


class StaleCapture(ValueError):
    """An intact stage no longer describes the current requests or sources."""


def document(path):
    value = strict_json_loads(Path(path).read_bytes())
    if not isinstance(value, dict):
        raise ValueError(f'program capture is not an object: {path}')
    return value


def authenticate(report, key, path):
    if report.get(key) != loader.digest(path):
        raise ValueError(f'program capture artifact hash mismatch: {key}: {path}')


def current(actual, expected, label):
    if actual != expected:
        raise StaleCapture(f'program capture is stale: {label}')


def requests_at(path):
    rows = strict_json_loads(Path(path).read_bytes())
    validate_requests(rows)
    return rows


def pin():
    return document(loader.ROOT / 'data/upstream.json')['pin']


def current_binary(build, expected, label):
    # Older stage closures predate build-configuration and embedded-data inputs.
    # Do not invent historic hashes for them: Cargo must produce the identical
    # executable now before its old observations can describe the current build.
    binary, _ = build()
    current(loader.digest(binary), expected, label + ' executable')


def config_current(path, requests):
    """Check artifact integrity before distinguishing source drift from corruption."""
    path = Path(path)
    report = document(path)
    if report.get('schema') != 1 or report.get('operation') != 'config_options':
        raise ValueError('incompatible program config evidence')
    for key in ('rust_binary', 'source_requests', 'go_observations', 'rust_observations'):
        entry = report.get(key)
        if not isinstance(entry, dict) or set(entry) != {'path', 'sha256'}:
            raise ValueError('missing program config artifact: ' + key)
        if key != 'rust_binary':
            authenticate(entry, 'sha256', path.parent / entry['path'])
    current(report.get('upstream_pin'), pin(), 'config pin')
    current(report.get('loading_requests_sha256'), loader.digest(requests), 'config requests')
    for key, names in loader.config_provenance_inputs().items():
        current(report.get(key), {name: loader.digest(loader.ROOT / name) for name in names}, key)
    from s07_config import rust_binary
    # Config records the mutable Cargo output, unlike the two copied program
    # probes. A rebuilt output is stale evidence, not corrupted capture bytes.
    current_binary(rust_binary, report['rust_binary']['sha256'], 'config')
    return loader.config_observations(path, requests, loader.identities(requests_at(requests), 'id', 'requests'))


def verification_oracle_current(requests, oracle):
    oracle = Path(oracle)
    manifest = document(oracle.with_suffix('.manifest.json'))
    if set(manifest) != {'pin', 'requests_sha256', 'observations_sha256', 'adapters', 'source_sha256'}:
        raise ValueError('incompatible native option-verifier manifest fields')
    authenticate(manifest, 'observations_sha256', oracle)
    wanted = dict(pin=pin(), requests_sha256=loader.digest(requests),
                  observations_sha256=loader.digest(oracle),
                  adapters={name: loader.digest(loader.ROOT / name) for name in
                            ('tools/s07/program/export_test.go', 'tools/s07/verify-options/export_test.go')},
                  source_sha256=loader.digest(verified_upstream() / 'tsc/internal/compiler/program.go'))
    current(manifest, wanted, 'native option-verifier manifest')
    rows = requests_at(requests)
    expected = strict_json_loads(oracle.read_bytes())
    # This also validates every native observation's field shape.
    verification.compare(rows, expected, expected)
    return expected


def stable_inputs(report, requests, *, source_stable):
    if type(source_stable) is not bool:
        raise ValueError('program capture lacks a boolean source-stability observation')
    current(source_stable, True, 'sources changed during capture')
    current(report.get('requests_sha256'), loader.digest(requests), 'requests')
    current(report.get('inputs'), loader.input_fingerprints(), 'production inputs')


def require_reconstructed(report, reconstructed):
    if loader.differences(report, reconstructed):
        raise ValueError('program capture rows or metrics disagree with raw observation replay')
    return report


def replay_loader(requests, oracle, output, config):
    requests, oracle, output, config = map(Path, (requests, oracle, output, config))
    report = document(output)
    if report.get('schema') != 1 or report.get('operation') != 'program_loader':
        raise ValueError('incompatible program loader capture')
    manifest_path = (oracle.with_name('program-manifest.json') if oracle.name == 'program-observations.json'
                     else oracle.with_suffix('.manifest.json'))
    raw = output.with_suffix('.rust.jsonl')
    binary = output.with_suffix('.program-probe')
    for key, path in (('oracle_sha256', oracle), ('oracle_manifest_sha256', manifest_path),
                      ('rust_observations_sha256', raw), ('binary_sha256', binary),
                      ('config_evidence_sha256', config)):
        authenticate(report, key, path)
    changed = report.get('source_changed_during_capture')
    if type(changed) is not bool:
        raise ValueError('program loader capture lacks source-stability observation')
    stable_inputs(report, requests, source_stable=not changed)
    current(report.get('upstream_pin'), pin(), 'loader pin')
    manifest = document(manifest_path)
    authenticate(manifest, 'observations_sha256', oracle)
    for key, wanted in (('upstream_pin', pin()), ('requests_sha256', loader.digest(requests)),
                        ('adapter_sha256', loader.digest(loader.ROOT / 'tools/s07/program/export_test.go'))):
        current(manifest.get(key), wanted, 'native loader ' + key)
    _, manifest = loader.validate_oracle_manifest(requests, oracle)
    native_root = verified_upstream()
    sources = {str(path.relative_to(native_root)): loader.digest(path)
               for path in sorted((native_root / 'tsc/internal/compiler').glob('*.go'))
               if not path.name.endswith('_test.go')}
    if not sources:
        raise ValueError('missing pinned program loader sources')
    # oracle_export installs this non-test bridge before s07_program records
    # the compiler source map. Authenticate the declared overlay as well as
    # the pin; do not drop extra manifest entries or invent historic hashes.
    sources['tsc/internal/compiler/s06_metadata_bridge.go'] = loader.digest(
        loader.ROOT / 'scripts/s06_oracle/metadata_bridge.go')
    current(manifest.get('source_sha256'), sources, 'native loader sources')
    current_binary(loader.rust_binary, report['binary_sha256'], 'loader')
    rows = requests_at(requests)
    ids = loader.identities(rows, 'id', 'requests')
    if manifest.get('rows') != len(ids):
        raise ValueError('Go loader manifest denominator mismatch')
    comparisons = loader.compare(rows, strict_json_loads(oracle.read_bytes()),
                                 [strict_json_loads(line) for line in raw.read_bytes().splitlines() if line.strip()])
    configs = config_current(config, requests)
    loader_passed = all(row['passed'] for row in comparisons)
    config_passed = all(row['passed'] for row in configs)
    reconstructed = dict(schema=1, operation='program_loader', upstream_pin=pin(),
                         requests_sha256=loader.digest(requests), oracle_sha256=loader.digest(oracle),
                         oracle_manifest_sha256=loader.digest(manifest_path),
                         rust_observations_sha256=loader.digest(raw), binary_sha256=loader.digest(binary),
                         config_evidence_sha256=loader.digest(config), required_variants=len(ids),
                         passed_variants=sum(row['passed'] for row in comparisons),
                         loader_graph_parity=loader_passed, config_parity=config_passed,
                         source_changed_during_capture=False,
                         metrics={'subset_loads': loader_passed and config_passed}, rows=comparisons,
                         inputs=loader.input_fingerprints(), config_rows=configs)
    return require_reconstructed(report, reconstructed)


def replay_verification(requests, oracle, output):
    requests, oracle, output = map(Path, (requests, oracle, output))
    report = document(output / 'report.json')
    if report.get('schema') != 1 or report.get('operation') != 'verify_compiler_options':
        raise ValueError('incompatible option-verifier capture')
    manifest = oracle.with_suffix('.manifest.json')
    raw, binary = output / 'observations.jsonl', output / 'program-probe'
    for key, path in (('oracle_sha256', oracle), ('oracle_manifest_sha256', manifest),
                      ('rust_sha256', raw), ('binary_sha256', binary)):
        authenticate(report, key, path)
    stable_inputs(report, requests, source_stable=report.get('source_stable'))
    current(report.get('pin'), pin(), 'option-verifier pin')
    rows = requests_at(requests)
    expected = verification_oracle_current(requests, oracle)
    current_binary(loader.rust_binary, report['binary_sha256'], 'option verifier')
    comparisons = verification.compare(rows, expected,
                                       [strict_json_loads(line) for line in raw.read_bytes().splitlines()])
    reconstructed = dict(schema=1, operation='verify_compiler_options', pin=pin(),
                         requests_sha256=loader.digest(requests), oracle_sha256=loader.digest(oracle),
                         oracle_manifest_sha256=loader.digest(manifest), rust_sha256=loader.digest(raw),
                         binary_sha256=loader.digest(binary), inputs=loader.input_fingerprints(), source_stable=True,
                         required_variants=len(rows), passed_variants=sum(row['passed'] for row in comparisons),
                         rows=comparisons, metrics={'option_verification': all(row['passed'] for row in comparisons)})
    return require_reconstructed(report, reconstructed)
