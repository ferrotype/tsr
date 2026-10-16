#!/usr/bin/env python3
"""Rust candidate/control screens using the R0 work validator.

Supply two saved release executables and a provisioned scenario cache. This
does not build, provision, profile, or replace the serial Go/Rust acceptance
runner. By default each pair starts at a barrier; submission order alternates,
while the OS determines actual launch order. With --serial, children run one
at a time in alternating order. Hashing starts after both children finish.
Type counts must agree by default. An algorithmic optimization that avoids
constructing types may instead declare exact counts for both executables and
explain the difference; all other work checks still apply to every sample.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import json
import math
from pathlib import Path
import shutil
import statistics
import sys
import threading
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.phase7.bench import build as ordinary_build, common, pgo  # noqa: E402

METRICS = ('wall_ns', 'peak_rss_bytes', 'user_ns')
WORK_COUNTERS = ('Files', 'Lines', 'Identifiers', 'Symbols', 'Types', 'Instantiations')


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def run_pair(binaries, descriptor, mode, checkers, output, index, timeout, *, expected_types=None,
             serial=False, exact_work=False):
    cwd = common.checkout(descriptor)
    directories = {}
    try:
        for name in binaries:
            directories[name] = common.emit_directory() if mode == 'emit' else None
        arguments = {name: common.arguments(descriptor, mode, checkers, directories[name]) for name in binaries}
        barrier = None if serial else threading.Barrier(2)

        def child(name):
            if barrier is not None:
                barrier.wait(timeout=10)
            launch = time.monotonic_ns()
            return launch, common.run(binaries[name], cwd, arguments[name], timeout=timeout)

        order = ['control', 'candidate'] if index % 2 == 0 else ['candidate', 'control']
        with concurrent.futures.ThreadPoolExecutor(1 if serial else 2) as pool:
            futures = {name: pool.submit(child, name) for name in order}
        # Exiting the pool waits for BOTH children. No output IO or hashing
        # below competes with either child's measured interval.
        observations, failures = {}, []
        for name, future in futures.items():
            path = output / f'{index}-{name}'
            try:
                launch, result = future.result()
            except Exception as error:
                failures.append(f'{name}: {error}')
                write_json(path.with_suffix('.json'), {'error': str(error)})
                continue
            path.with_suffix('.stdout').write_text(result['stdout'])
            path.with_suffix('.stderr').write_text(result['stderr'])
            raw = {key: value for key, value in result.items() if key not in ('stdout', 'stderr')}
            raw.update(launch_ns=launch, command=[str(binaries[name]), *arguments[name]], cwd=str(cwd))
            # Save both raw results before parsing either, including malformed
            # output from a compiler failure.
            write_json(path.with_suffix('.json'), raw)
            observations[name] = (raw, result)
        for name, (raw, result) in observations.items():
            try:
                raw['facts'] = common.observe(result, cwd, directories[name])
                issues = common.problems(raw['facts'], descriptor['work'][mode], mode)
                if expected_types is not None and raw['facts']['types'] != expected_types[name]:
                    issues.append(f"type count {raw['facts']['types']} differs from expected {expected_types[name]}")
            except ValueError as error:
                issues = [str(error)]
            if result['stderr']:
                issues.append('wrote to stderr: ' + result['stderr'][:200])
            for metric in METRICS:
                if raw[metric] <= 0:
                    issues.append(f'{metric} must be positive, got {raw[metric]}')
            raw['issues'] = issues
            write_json(output / f'{index}-{name}.json', raw)
            failures += [f'{name}: {issue}' for issue in issues]
        if failures:
            raise ValueError(f'pair {index} invalid: ' + '; '.join(failures))
        measured = {name: raw for name, (raw, _) in observations.items()}
        candidate, control = measured['candidate'], measured['control']
        if expected_types is None and candidate['facts']['types'] != control['facts']['types']:
            raise ValueError(f'pair {index}: candidate/control type counts differ')
        statistics_by_side = {name: raw['facts']['statistics'] for name, raw in measured.items()}
        if exact_work:
            for key in WORK_COUNTERS:
                values = [statistics_by_side[name].get(key) for name in ('control', 'candidate')]
                if values[0] is None or values[0] != values[1]:
                    raise ValueError(f'pair {index}: work counter {key} differs or is missing: {values}')
        return {
            'pair': index, 'submission_order': order,
            'launch_skew_ms': abs(candidate['launch_ns'] - control['launch_ns']) / 1e6,
            'ratios': {metric: candidate[metric] / control[metric] for metric in METRICS},
            'seconds': {name: raw['wall_ns'] / 1e9 for name, raw in measured.items()},
            'check_seconds': {name: raw['facts']['check_seconds'] for name, raw in measured.items()},
            'emit_seconds': {name: raw['facts']['emit_seconds'] for name, raw in measured.items()},
            'types': {name: raw['facts']['types'] for name, raw in measured.items()},
            'statistics': statistics_by_side,
        }
    finally:
        for directory in directories.values():
            if directory is not None:
                shutil.rmtree(directory)


def read_build(path, binary):
    path = Path(path).resolve()
    manifest = json.loads(path.read_text())
    if (manifest.get('complete') is not True or manifest.get('source_stable') is not True
            or manifest.get('exit') != 0 or manifest.get('pgo') is not False):
        raise ValueError(f'{path}: require a successful, source-stable ordinary build')
    for key in ('sources', 'toolchain', 'release_configuration', 'cargo_configuration', 'binary'):
        if key not in manifest:
            raise ValueError(f'{path}: missing {key}')
    identity = common.identity(binary)
    if any(identity[key] != manifest['binary'].get(key) for key in ('sha256', 'bytes')):
        raise ValueError(f'{path}: executable differs from the recorded build')
    ordinary_build.validate_features(manifest.get('features'))
    ordinary_build.validate_features(manifest.get('cargo_artifact', {}).get('features', []), resolved=True)
    # Older archived builds have explicit requested/binary features only.
    # New helpers also record dependency features from Cargo's actual artifacts.
    if 'workspace_features' in manifest:
        if not isinstance(manifest['workspace_features'], dict) or not manifest['workspace_features']:
            raise ValueError(f'{path}: missing resolved workspace feature inventory')
        for features in manifest['workspace_features'].values():
            ordinary_build.validate_features(features, resolved=True)
    return {'path': str(path), 'identity': common.identity(path), 'manifest': manifest}


def diagnostic_summary(samples):
    summary = {}
    for key in ('Config time', 'Parse time', 'Bind time', 'Check time', 'Emit time', 'Total time', 'Memory allocs'):
        if not all(key in sample['statistics'][side] for sample in samples for side in ('control', 'candidate')):
            continue
        def value(sample, side):
            raw = sample['statistics'][side][key]
            number = int(raw) if key == 'Memory allocs' else float(raw.removesuffix('s'))
            if number < 0 or not math.isfinite(number):
                raise ValueError(f'invalid {key}: {raw}')
            return number
        control = [value(sample, 'control') for sample in samples]
        candidate = [value(sample, 'candidate') for sample in samples]
        ratios = [b / a for a, b in zip(control, candidate)] if all(control) else []
        scale = 1 if key == 'Memory allocs' else 1000
        summary[key] = {'paired_median_ratio': statistics.median(ratios) if ratios else None,
                        'ratio_range': [min(ratios), max(ratios)] if ratios else None,
                        'paired_median_saved': statistics.median((a - b) * scale for a, b in zip(control, candidate)),
                        'saved_unit': 'calls' if key == 'Memory allocs' else 'ms'}
    return summary


def screen(descriptor, control, candidate, output, *, mode, checkers=2, pairs=3, timeout=240,
           control_types=None, candidate_types=None, type_count_reason=None, serial=False,
           control_build=None, candidate_build=None):
    if pairs < 1 or timeout <= 0:
        raise ValueError('pair count and timeout must be positive')
    if mode not in common.MODES or checkers not in common.CHECKERS:
        raise ValueError('unsupported mode or checker count')
    expected_types = None
    if any(value is not None for value in (control_types, candidate_types, type_count_reason)):
        if any(type(value) is not int or value <= 0 for value in (control_types, candidate_types)):
            raise ValueError('control-types and candidate-types must both be positive integers')
        if not isinstance(type_count_reason, str) or not type_count_reason.strip():
            raise ValueError('explicit type counts require a nonempty type-count-reason')
        expected_types = {'control': control_types, 'candidate': candidate_types}
    manifest_mode = control_build is not None or candidate_build is not None
    if manifest_mode and (control_build is None or candidate_build is None):
        raise ValueError('control-build and candidate-build must be supplied together')
    if manifest_mode and (mode != 'check' or expected_types is not None):
        raise ValueError('build-verified screens require check mode and equal work counts')
    if not common.provisioned(descriptor):
        raise ValueError(f"{descriptor['name']}: provision the scenario with scripts/phase7_scenarios.py first")
    common.require_unchanged(descriptor)
    binaries = {'control': Path(control).resolve(), 'candidate': Path(candidate).resolve()}
    record = {
        'scenario': descriptor['name'], 'mode': mode, 'checkers': checkers,
        'method': ('serial pairs; alternating execution order; no profiler' if serial else
                   'concurrent pairs; alternating submission order; no profiler'),
        'binaries': {name: common.identity(path) for name, path in binaries.items()},
        'cache': str(common.CACHE.resolve()), 'timeout_seconds': timeout,
        'requested_pairs': pairs, 'complete': False, 'pairs': [],
        'type_count_policy': 'explicit' if expected_types is not None else 'equal',
    }
    if expected_types is not None:
        record['type_count_expectations'] = expected_types
        record['type_count_reason'] = type_count_reason.strip()
    output = Path(output).resolve()
    output.mkdir(parents=True)  # Refuse to overwrite an earlier screen.
    summary = output / 'summary.json'
    write_json(summary, record)
    try:
        if manifest_mode:
            env = pgo.environment()
            builds = {name: read_build(path, binaries[name])
                      for name, path in [('control', control_build), ('candidate', candidate_build)]}
            record['builds'] = builds
            left, right = (builds[name]['manifest'] for name in ('control', 'candidate'))
            for key in ('release_configuration', 'cargo_configuration'):
                if left[key] != right[key]:
                    raise ValueError(f'build {key} differs')
            for key in ('channel', 'host', 'rustc'):
                if left['toolchain'].get(key) is None or left['toolchain'].get(key) != right['toolchain'].get(key):
                    raise ValueError(f'build toolchain {key} differs')
            record['source_changes'] = [
                {'path': path, 'control_sha256': left['sources'].get(path), 'candidate_sha256': right['sources'].get(path)}
                for path in sorted(left['sources'].keys() | right['sources'].keys())
                if left['sources'].get(path) != right['sources'].get(path)]
            def verify_builds():
                if pgo.source_identity() != right['sources']:
                    raise ValueError('current source differs from candidate build')
                if pgo.benchmark.cargo_configuration(env) != right['cargo_configuration']:
                    raise ValueError('current Cargo configuration differs from candidate build')
                for name, build in builds.items():
                    if common.identity(Path(build['path'])) != build['identity']:
                        raise ValueError(f'{name} build manifest changed during screen')
            verify_builds()
            record['preflight'] = {name: pgo.preflight(binary, descriptor, checkers, env, output / f'before-{name}')
                                   for name, binary in binaries.items()}
            if record['preflight']['control'] != record['preflight']['candidate']:
                raise ValueError('control/candidate inputs or options differ')
            write_json(summary, record)
        for index in range(pairs):
            pair = run_pair(binaries, descriptor, mode, checkers, output, index, timeout,
                            expected_types=expected_types, serial=serial, exact_work=manifest_mode)
            record['pairs'].append(pair)
            write_json(summary, record)
            print(json.dumps(pair), flush=True)
        common.require_unchanged(descriptor)
        if manifest_mode:
            for name, binary in binaries.items():
                after = pgo.preflight(binary, descriptor, checkers, env, output / f'after-{name}')
                if after != record['preflight'][name]:
                    raise ValueError(f'{name} inputs or options changed during screen')
            verify_builds()
        if {name: common.identity(path) for name, path in binaries.items()} != record['binaries']:
            raise ValueError('executable changed during screen')
        record['diagnostic_summary'] = diagnostic_summary(record['pairs'])
    except Exception as error:
        record['error'] = str(error)
        write_json(summary, record)
        raise
    record['median_ratios'] = {
        metric: statistics.median(pair['ratios'][metric] for pair in record['pairs']) for metric in METRICS
    }
    record['complete'] = True
    write_json(summary, record)
    print(record['median_ratios'], flush=True)
    return record


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scenario', choices=common.NAMES, default='vscode')
    parser.add_argument('--mode', choices=common.MODES, required=True)
    parser.add_argument('--checkers', type=int, choices=common.CHECKERS, default=2)
    parser.add_argument('--pairs', type=int, default=3)
    parser.add_argument('--serial', action='store_true', help='run each pair one child at a time in alternating order')
    parser.add_argument('--timeout', type=int, default=240, help='seconds per compiler process')
    parser.add_argument('--control', type=Path, required=True, help='saved control release executable')
    parser.add_argument('--candidate', type=Path, required=True, help='saved candidate release executable')
    parser.add_argument('--control-build', type=Path, help='ordinary control build.json; requires candidate-build')
    parser.add_argument('--candidate-build', type=Path, help='ordinary candidate build.json; enables source/input verification')
    parser.add_argument('--control-types', type=int, help='exact control type count; requires candidate count and reason')
    parser.add_argument('--candidate-types', type=int, help='exact candidate type count; requires control count and reason')
    parser.add_argument('--type-count-reason', help='why an algorithmic optimization changes type construction')
    parser.add_argument('--cache', type=Path, default=common.CACHE, help='provisioned Phase 7 scenario cache')
    parser.add_argument('--output', type=Path, required=True, help='new result directory (must not exist)')
    args = parser.parse_args(argv)
    previous_cache = common.CACHE
    common.CACHE = args.cache.resolve()
    try:
        screen(common.load(args.scenario), args.control, args.candidate, args.output,
               mode=args.mode, checkers=args.checkers, pairs=args.pairs, timeout=args.timeout,
               control_types=args.control_types, candidate_types=args.candidate_types,
               type_count_reason=args.type_count_reason, serial=args.serial,
               control_build=args.control_build, candidate_build=args.candidate_build)
    except (ValueError, OSError) as error:
        parser.exit(1, f'{error}\n')
    finally:
        common.CACHE = previous_cache


if __name__ == '__main__':
    main()
