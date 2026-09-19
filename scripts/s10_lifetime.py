#!/usr/bin/env python3
"""Capture the external embedding consumer's exact ownership tests in four modes."""
import argparse
import json
import os
from pathlib import Path
import subprocess

from s04_ownership import instrumentation_environment
from s04_runtime import cache_home, load_toolchains
from s06_ownership import validate_output
from s10_corpus import ROOT, file_digest, sources
import s08_p4 as p4


def manifest():
    value = p4.read(ROOT / 'tools/s10/lifetime-cases.json')
    cases = value['cases']
    if not cases or len(set(cases)) != len(cases) or any(not c.startswith('lifetime::') for c in cases):
        raise ValueError('invalid external-consumer lifetime inventory')
    if value['modes'] != ['debug', 'release', 'miri', 'address_sanitizer']:
        raise ValueError('all four ownership modes are required')
    return value


def capture(directory):
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
        p4.atomic(directory / 'progress.json', outcomes)
    record = {'version': 1, 'sources': before, 'source_stable': before == sources(), 'rustc': rustc,
              'nightly': nightly, 'host': host, 'spec': spec, 'modes': outcomes}
    p4.write_new(directory / 'capture.json', record)
    return verify(directory)


def verify(directory):
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
            validate_output(path.read_bytes(), spec['cases'], mode, scope='external embedding consumer')
            outcomes[mode] = row['exit_code'] == 0
        except ValueError:
            outcomes[mode] = False
    return {'metrics': {'lifetime_checks': all(outcomes.values())}, 'modes': outcomes,
            'cases': spec['cases'], 'capture_sha256': file_digest(directory / 'capture.json')}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['capture', 'verify'])
    parser.add_argument('--output', type=Path, default=ROOT / 'target/s10/lifetime')
    args = parser.parse_args()
    print(json.dumps((capture if args.action == 'capture' else verify)(args.output), indent=2))
