#!/usr/bin/env python3
"""Capture the external embedding consumer's exact ownership tests in four modes."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess

from s04_ownership import instrumentation_environment
from s04_runtime import cache_home, load_toolchains
from s06_ownership import validate_output as validate_named_output
from s10_corpus import ROOT, file_digest, sources as corpus_sources
import s08_p4 as p4


def sources():
    """Inputs compiled by the native consumer and its ownership runner.

    The broader historical S10 corpus closure includes browsers, the Go oracle,
    measurement scripts and documentation; none runs in this native Rust lane.
    Keep source/assets/manifests for its Rust closure and its actual runner.
    """
    runner = {
        'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
        'tools/s10/lifetime-cases.json', 'scripts/s10_lifetime.py',
        'scripts/s10_corpus.py', 'scripts/s04_ownership.py',
        'scripts/s04_runtime.py', 'scripts/s06_ownership.py',
        'scripts/s08_p4.py', 'data/s04/toolchains.toml',
    }
    def used(name):
        path = Path(name)
        if name in runner or name.startswith('.cargo/'):
            return True
        if name.startswith('crates/'):
            # Include embedded assets as well as Rust and build manifests.
            return '/src/' in name or '/bundled/' in name or path.name in {'Cargo.toml', 'build.rs'}
        if name.startswith('tools/s10/rust-consumer/'):
            return path.suffix == '.rs' or path.name in {'Cargo.toml', 'Cargo.lock'}
        return name.startswith('tools/phase7/portable/') and path.suffix == '.rs'
    return {name: digest for name, digest in corpus_sources().items() if used(name)}


def source_cases(root, value):
    """Inventory this integration target's plain root #[test] functions.

    Refuse other test declaration forms instead of silently dropping them.
    Cargo output independently confirms which exact names actually execute.
    """
    path = root / Path(value['consumer']).parent / 'tests' / (value['test'] + '.rs')
    text = path.read_text()
    cases = re.findall(r'(?m)^#\[test\]\s*\nfn ([a-z][a-z0-9_]*)\s*\(', text)
    if not cases or len(re.findall(r'(?m)^\s*#\[test\]', text)) != len(cases):
        raise ValueError('unsupported/empty external-consumer test declarations: ' + str(path))
    if len(cases) != len(set(cases)):
        raise ValueError('duplicate external-consumer test declarations')
    return sorted(cases)


def manifest(root=ROOT):
    value = p4.read(root / 'tools/s10/lifetime-cases.json')
    cases = value['cases']
    if (not isinstance(cases, list) or not cases
            or any(not isinstance(c, str) or re.fullmatch(r'[a-z][a-z0-9_]*', c) is None for c in cases)
            or cases != sorted(set(cases))):
        raise ValueError('invalid external-consumer lifetime inventory')
    if value['modes'] != ['debug', 'release', 'miri', 'address_sanitizer']:
        raise ValueError('all four ownership modes are required')
    actual = source_cases(root, value)
    if cases != actual:
        missing = sorted(set(actual) - set(cases))
        stale = sorted(set(cases) - set(actual))
        raise ValueError(f'lifetime inventory differs from source; missing: {missing}; stale: {stale}')
    for target, supporting in value.get('supporting_cases', {}).items():
        actual = source_cases(root, dict(value, test=target))
        if not supporting or supporting != sorted(set(supporting)) or not set(supporting) <= set(actual):
            raise ValueError('invalid supporting lifetime inventory: ' + target)
    return value


def validate_output(output, cases, mode):
    validate_named_output(output, cases, mode, scope='external embedding consumer')
    if not re.search(rb'^test result: ok\..*; 0 filtered out;', output, re.MULTILINE):
        raise ValueError('external embedding consumer ' + mode + ': filtered tests are not a complete target')


def output_directory(directory):
    directory = directory.resolve()
    if 'target' in directory.parts:
        raise ValueError('lifetime output must be outside target')
    return directory


def capture(directory):
    directory = output_directory(directory)
    if directory.exists():
        raise ValueError('lifetime capture output must be a new directory')
    spec = manifest()
    before = sources()
    directory.mkdir(parents=True, exist_ok=False)
    base = instrumentation_environment(dict(os.environ, CARGO_TERM_COLOR='never'), ROOT)
    nightly = load_toolchains(ROOT)['nightly']
    rustc = subprocess.check_output(['rustc', '-Vv'], env=base, text=True)
    host = next(row.removeprefix('host: ') for row in rustc.splitlines() if row.startswith('host: '))
    miri = dict(base, MIRI_SYSROOT=str(cache_home(ROOT, base) / 'miri' / nightly / host),
                CARGO_TARGET_DIR=str(ROOT / 'target/s04-miri'), MIRIFLAGS='-Zmiri-strict-provenance', RUSTFLAGS='')
    asan = dict(base, CARGO_TARGET_DIR=str(ROOT / 'target/s04-asan'), RUSTFLAGS='-Zsanitizer=address',
                ASAN_OPTIONS='detect_leaks=0' if 'apple' in host else 'detect_leaks=1')
    commands = {
        'debug': (['cargo', 'test'], ['--target-dir', 'target'], base),
        'release': (['cargo', 'test'], ['--release', '--target-dir', 'target'], base),
        'miri': (['cargo', '+' + nightly, 'miri', 'test'], ['--target', host], miri),
        'address_sanitizer': (['cargo', '+' + nightly, 'test'], ['-Zbuild-std', '--target', host], asan),
    }
    outcomes = {}
    for mode, (prefix, options, env) in commands.items():
        if mode == 'miri':
            with (directory / 'miri-setup.log').open('wb') as log:
                subprocess.run(['cargo', '+' + nightly, 'miri', 'setup', '--target', host], cwd=ROOT,
                               env=miri, stdout=log, stderr=subprocess.STDOUT, check=True)
        command = [*prefix, '--locked', '--manifest-path', spec['consumer'], '--test', spec['test'],
                   *options, '--', '--test-threads=1']
        print('ownership: ' + mode, flush=True)
        path = directory / (mode + '.log')
        with path.open('wb') as log:
            result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
        outcomes[mode] = {'command': command, 'exit_code': result.returncode, 'log_sha256': file_digest(path)}
        supporting = {}
        for target, cases in spec.get('supporting_cases', {}).items():
            supporting[target] = {}
            for case in cases:
                command = [*prefix, '--locked', '--manifest-path', spec['consumer'], '--test', target,
                           *options, '--', case, '--exact', '--test-threads=1']
                path = directory / (mode + '.' + target + '.' + case + '.log')
                with path.open('wb') as log:
                    result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
                supporting[target][case] = {'command': command, 'exit_code': result.returncode,
                                            'log_sha256': file_digest(path)}
        outcomes[mode]['supporting'] = supporting
        p4.atomic(directory / 'progress.json', outcomes)
    record = {'version': 1, 'sources': before, 'source_stable': before == sources(), 'rustc': rustc,
              'nightly': nightly, 'host': host, 'spec': spec, 'modes': outcomes}
    p4.write_new(directory / 'capture.json', record)
    return verify(directory)


def verify(directory):
    directory = output_directory(directory)
    record = p4.read(directory / 'capture.json')
    spec = manifest()
    if record['sources'] != sources() or not record['source_stable'] or record['spec'] != spec:
        raise ValueError('stale or changed lifetime capture')
    if set(record['modes']) != set(spec['modes']):
        raise ValueError('incomplete ownership mode inventory')
    outcomes = {}
    for mode, row in record['modes'].items():
        path = directory / (mode + '.log')
        if file_digest(path) != row['log_sha256']:
            raise ValueError('ownership log changed: ' + mode)
        try:
            validate_output(path.read_bytes(), spec['cases'], mode)
            outcomes[mode] = row['exit_code'] == 0
            supporting = row.get('supporting', {})
            if set(supporting) != set(spec.get('supporting_cases', {})):
                raise ValueError('incomplete supporting target inventory')
            for target, cases in spec.get('supporting_cases', {}).items():
                if set(supporting[target]) != set(cases):
                    raise ValueError('incomplete supporting case inventory')
                for case in cases:
                    receipt = supporting[target][case]
                    log = directory / (mode + '.' + target + '.' + case + '.log')
                    if file_digest(log) != receipt['log_sha256']:
                        raise ValueError('supporting ownership log changed')
                    validate_named_output(log.read_bytes(), [case], mode,
                                          scope='external embedding supporting consumer')
                    outcomes[mode] &= receipt['exit_code'] == 0
        except ValueError:
            outcomes[mode] = False
    return {'metrics': {'lifetime_checks': all(outcomes.values())}, 'modes': outcomes,
            'cases': spec['cases'], 'capture_sha256': file_digest(directory / 'capture.json')}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['capture', 'verify'])
    parser.add_argument('--output', type=Path, required=True, help='capture/receipt directory outside target; new for capture')
    args = parser.parse_args(argv)
    try:
        report = (capture if args.action == 'capture' else verify)(args.output)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps(report, indent=2))
    return 0 if report['metrics']['lifetime_checks'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
