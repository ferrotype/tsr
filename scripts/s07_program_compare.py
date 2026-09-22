#!/usr/bin/env python3
"""Strict comparison of every independently selected S07 program-loader request.

A successful native process alone is not parity. Every observation field is
compared, including nil/empty distinctions, callback traces and diagnostics.
"""
import hashlib
import json
import os
import shutil
from pathlib import Path
import sys

from s04_common import command, strict_json_loads
from s04_ownership import instrumentation_environment

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def identities(rows, field, label):
    if not isinstance(rows, list) or not rows:
        raise ValueError(label + ' must be a nonempty array')
    ids = []
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get(field), str) or not row[field]:
            raise ValueError(label + ' has an invalid identity')
        ids.append(row[field])
    if len(ids) != len(set(ids)):
        raise ValueError(label + ' has duplicate identities')
    return ids


def differences(expected, actual, path='', ordered=False):
    """Retain precise mismatch paths without repeating potentially large sources."""
    if type(expected) is not type(actual):
        return [path or '/']
    if isinstance(expected, dict):
        result = []
        if ordered and list(expected) != list(actual):
            result.append(path + '/$property_order')
        for key in sorted(expected.keys() | actual.keys()):
            child = path + '/' + key.replace('~', '~0').replace('/', '~1')
            if key not in expected or key not in actual:
                result.append(child)
            else:
                result.extend(differences(expected[key], actual[key], child,
                                          ordered or key == 'config_raw' or (path == '/options' and key == 'paths')))
        return result
    if isinstance(expected, list):
        result = []
        if len(expected) != len(actual):
            result.append(path + '/length')
        for index, (left, right) in enumerate(zip(expected, actual)):
            result.extend(differences(left, right, path + '/' + str(index), ordered))
        return result
    return [] if expected == actual else [path or '/']


def compare(requests, expected, actual):
    ids = identities(requests, 'id', 'requests')
    if identities(expected, 'ID', 'Go observations') != ids:
        raise ValueError('missing, extra, or reordered Go observations')
    if identities(actual, 'ID', 'Rust observations') != ids:
        raise ValueError('missing, extra, or reordered Rust observations')
    results = []
    for identity, left, right in zip(ids, expected, actual):
        failure = None
        if 'Panic' in left or 'Error' in left:
            # A source panic retained for diagnosis is not an eligible successful
            # loader row, even when Rust happens to panic with the same text.
            failure = 'source_failure'
        elif 'Panic' in right or 'Error' in right:
            failure = 'rust_failure'
        paths = differences(left, right)
        if paths and failure is None:
            failure = 'mismatch'
        row = {'id': identity, 'passed': failure is None}
        if failure:
            row.update(failure=failure, differing_paths=paths)
            for label, value in [('go', left), ('rust', right)]:
                for field in ('Error', 'Panic'):
                    if field in value:
                        row[label + '_' + field.lower()] = value[field]
        results.append(row)
    return results


CONFIG_FIELDS = {'id', 'options', 'root_file_names', 'config_raw',
                 'config_diagnostics', 'option_diagnostics', 'compile_on_save'}


def config_provenance_inputs():
    """Registered production closure; a producer cannot select fewer inputs."""
    go = {'tools/s07/config/export_test.go', 'tools/s07/config/host_test.go',
          'tools/s07/config/fixture_export_test.go', 'tools/s07/subset/options_bridge.go',
          'scripts/s07_config.py', 'scripts/s07_program_compare.py',
          'scripts/s06_oracle/export_test.go', 'scripts/s06_oracle/metadata_bridge.go',
          'scripts/s06_build.py', 'scripts/s04.py', 'scripts/s04_common.py',
          'data/upstream.json', 'data/s04/toolchains.toml', 'data/s06/corpus.json'}
    rust = {'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'scripts/s07_config.py',
            'scripts/s07_program_compare.py', 'tools/s07/config/rust_observation.rs',
            'scripts/s04_ownership.py',
            'data/upstream.json', 'data/s04/toolchains.toml'}
    crates = ('tsr_compiler', 'tsr_module', 'tsr_semver', 'tsr_tsoptions', 'tsr_vfs',
              'tsr_bundled', 'tsr_tspath', 'tsr_core', 'tsr_parser', 'tsr_scanner',
              'tsr_ast', 'tsr_arena', 'tsr_binder', 'tsr_jsstring', 'tsr_jsnum', 'tsr_diagnostics', 'tsr_json', 'tsr_locale', 'tsr_glob')
    for crate in crates:
        directory = ROOT / 'crates' / crate
        rust.add(str((directory / 'Cargo.toml').relative_to(ROOT)))
        rust.update(str(p.relative_to(ROOT)) for p in directory.rglob('*.rs'))
    return {'go_inputs': go, 'rust_inputs': rust}


def verify_config_inputs(document):
    for family, required in config_provenance_inputs().items():
        entries = document.get(family)
        if not isinstance(entries, dict) or set(entries) != required:
            raise ValueError('incomplete or unregistered config production input set: ' + family)
        for source, wanted in entries.items():
            if digest(ROOT / source) != wanted:
                raise ValueError('stale config production input: ' + source)


def compare_config(ids, requests, expected, actual):
    for rows, label in ((requests, 'source config requests'),
                        (expected, 'Go config observations'), (actual, 'Rust config observations')):
        if identities(rows, 'id', label) != ids:
            raise ValueError(label + ' denominator mismatch')
    result = []
    for identity, left, right in zip(ids, expected, actual):
        for row in (left, right):
            if set(row) != CONFIG_FIELDS:
                raise ValueError('config evidence requires actual complete observations, not passed flags')
        paths = differences(left, right)
        result.append({'id':identity, 'passed':not paths, 'differing_paths':paths})
    return result


def config_observations(path, loader_requests, ids):
    path = Path(path).resolve()
    document = strict_json_loads(path.read_bytes())
    pin = strict_json_loads((ROOT/'data/upstream.json').read_bytes())['pin']
    if (document.get('schema') != 1 or document.get('operation') != 'config_options'
            or document.get('upstream_pin') != pin
            or document.get('loading_requests_sha256') != digest(loader_requests)):
        raise ValueError('config evidence has incompatible provenance or loader requests')
    def artifact(key):
        entry = document.get(key)
        if not isinstance(entry, dict) or set(entry) != {'path','sha256'}:
            raise ValueError('missing config artifact: '+key)
        actual_path = path.parent/entry['path']
        if digest(actual_path) != entry['sha256']:
            raise ValueError('stale config artifact: '+key)
        return actual_path
    verify_config_inputs(document)
    artifact('rust_binary')
    records = [strict_json_loads(artifact(key).read_bytes())
               for key in ('source_requests','go_observations','rust_observations')]
    return compare_config(ids, *records)


def validate_oracle_manifest(requests, oracle):
    manifest_path = oracle.with_suffix('.manifest.json')
    if oracle.name == 'program-observations.json':
        manifest_path = oracle.with_name('program-manifest.json')
    manifest = strict_json_loads(manifest_path.read_bytes())
    pin = strict_json_loads((ROOT / 'data/upstream.json').read_bytes())['pin']
    wanted = {'upstream_pin': pin, 'requests_sha256': digest(requests),
              'observations_sha256': digest(oracle),
              'adapter_sha256': digest(ROOT / 'tools/s07/program/export_test.go')}
    for key, value in wanted.items():
        if manifest.get(key) != value:
            raise ValueError('stale or incompatible Go loader manifest: ' + key)
    return manifest_path, manifest


def rust_binary():
    env = instrumentation_environment(os.environ.copy(), ROOT)
    for key in list(env):
        if key.startswith('CARGO_PROFILE_') or key == 'CARGO_BUILD_TARGET':
            env.pop(key, None)
    output = command(['cargo', 'build', '--locked', '--release', '-p', 'tsr_compiler',
                      '--example', 'program_probe', '--message-format=json'], cwd=ROOT, env=env)
    records = [strict_json_loads(line) for line in output.splitlines() if line.strip()]
    binaries = [row['executable'] for row in records
                if row.get('reason') == 'compiler-artifact' and
                row.get('target', {}).get('name') == 'program_probe' and row.get('executable')]
    if len(binaries) != 1:
        raise ValueError('Cargo did not return exactly one program probe binary')
    return Path(binaries[0]), env


def input_fingerprints():
    crates = ('tsr_arena', 'tsr_ast', 'tsr_core', 'tsr_diagnostics', 'tsr_json', 'tsr_locale', 'tsr_glob', 'tsr_jsstring',
              'tsr_jsnum', 'tsr_scanner', 'tsr_parser', 'tsr_binder', 'tsr_tspath',
              'tsr_vfs', 'tsr_bundled', 'tsr_tsoptions', 'tsr_semver', 'tsr_module', 'tsr_compiler')
    paths=[ROOT/'Cargo.lock',ROOT/'Cargo.toml',ROOT/'rust-toolchain.toml',
           ROOT/'tools/s07/program/rust_observation.rs',Path(__file__).resolve(),
           ROOT/'scripts/s07_program.py']
    paths += [path for crate in crates for path in (ROOT/'crates'/crate).rglob('*')
              if path.is_file() and path.suffix in {'.rs', '.toml'}]
    return {str(path.relative_to(ROOT)):digest(path) for path in sorted(paths)}


def check_subset(requests, oracle, output, config_evidence=None):
    requests, oracle, output = map(lambda path: Path(path).resolve(), (requests, oracle, output))
    output.parent.mkdir(parents=True, exist_ok=True)
    from s07_program import validate_requests
    rows = strict_json_loads(requests.read_bytes())
    validate_requests(rows)
    expected = strict_json_loads(oracle.read_bytes())
    ids = identities(rows, 'id', 'requests')
    manifest_path, manifest = validate_oracle_manifest(requests, oracle)
    if manifest.get('rows') != len(ids):
        raise ValueError('Go loader manifest denominator mismatch')
    # Check the oracle's denominator before invoking a costly native process.
    if identities(expected, 'ID', 'Go observations') != ids:
        raise ValueError('missing, extra, or reordered Go observations')
    inputs=input_fingerprints()
    binary, env = rust_binary()
    frozen_binary=output.with_suffix('.program-probe')
    shutil.copyfile(binary,frozen_binary)
    frozen_binary.chmod(0o755)
    binary=frozen_binary
    actual_path = output.with_suffix('.rust.jsonl')
    command([str(binary), str(requests), str(actual_path)], cwd=ROOT, env=env)
    actual = [strict_json_loads(line) for line in actual_path.read_bytes().splitlines() if line.strip()]
    comparisons = compare(rows, expected, actual)
    loader_passed = all(row['passed'] for row in comparisons)
    source_changed=inputs!=input_fingerprints()
    config_passed = False
    config_hash = None
    config_comparisons = None
    if config_evidence is not None:
        config_evidence = Path(config_evidence).resolve()
        config_hash = digest(config_evidence)
        config_comparisons = config_observations(config_evidence, requests, ids)
        config_passed = all(row['passed'] for row in config_comparisons)
    report = {
        'schema': 1, 'operation': 'program_loader', 'upstream_pin': manifest['upstream_pin'],
        'requests_sha256': digest(requests), 'oracle_sha256': digest(oracle),
        'oracle_manifest_sha256': digest(manifest_path), 'rust_observations_sha256': digest(actual_path),
        'binary_sha256': digest(binary), 'config_evidence_sha256': config_hash,
        'required_variants': len(ids), 'passed_variants': sum(row['passed'] for row in comparisons),
        'loader_graph_parity': loader_passed, 'config_parity': config_passed,
        'source_changed_during_capture':source_changed,
        'metrics': {'subset_loads': loader_passed and config_passed and not source_changed}, 'rows': comparisons,
        'inputs': inputs,
        'config_rows': config_comparisons,
    }
    output.write_text(json.dumps(report, sort_keys=True, indent=2) + '\n')
    return report


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--requests', type=Path, required=True)
    parser.add_argument('--oracle', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--config-evidence', type=Path)
    args = parser.parse_args()
    report = check_subset(args.requests, args.oracle, args.output, args.config_evidence)
    print(json.dumps({key: report[key] for key in ('required_variants', 'passed_variants', 'loader_graph_parity', 'config_parity', 'metrics')}))
    sys.exit(0 if report['metrics']['subset_loads'] else 1)
