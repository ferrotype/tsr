#!/usr/bin/env python3
"""Verify and replay one manifested CLI/scanner/parser fuzz case without modifying its archive.

All executable bytes, options, input files and observations stay explicit.
A resolved attempt means only that the archived failure did not occur on that
attempt; it does not certify a fix for nondeterministic behavior.
"""
from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
from pathlib import Path
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.phase7.fuzz import run as fuzz  # noqa: E402


def read_json(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f'duplicate JSON key {key!r} in {path}')
            result[key] = value
        return result
    def invalid_constant(value):
        raise ValueError(f'non-finite JSON value {value!r} in {path}')
    return json.loads(path.read_bytes(), object_pairs_hook=unique, parse_constant=invalid_constant)


def verify(case, candidate_rust=None):
    """run.json is the trust anchor; check the per-case manifest and all bytes.

    This is integrity checking, not authenticity or a new compiler-build claim.
    Legacy archives without a creation-time manifest are deliberately refused.
    """
    if case.name not in ('original', 'minimized'):
        raise ValueError('case must name an original/ or minimized/ archive directory')
    run_file = case.parents[1] / 'run.json'
    metadata = read_json(run_file)
    findings = [item for item in metadata['failures'] if item['directory'] == case.parent.name]
    if len(findings) != 1:
        raise ValueError('case must belong to exactly one finding in run.json')
    expected = findings[0].get('manifests', {}).get(case.name)
    manifest_path = case / 'manifest.json'
    if not expected or not manifest_path.is_file():
        raise ValueError('legacy/unmanifested archive: no creation-time integrity record')
    if fuzz.sha256(manifest_path.read_bytes()) != expected:
        raise ValueError('case manifest hash differs from run.json')
    manifest = read_json(manifest_path)
    if type(manifest['schema']) is not int or manifest['schema'] != 1:
        raise ValueError('unsupported case manifest schema')
    for path in case.rglob('*'):
        if path.is_symlink():
            raise ValueError(f'archive contains a symlink: {path}')
    files = fuzz.tree_files(case)
    del files['manifest.json']
    if files != manifest['files']:
        raise ValueError('archived file set/bytes differ from the case manifest')
    target = manifest.get('target', 'cli')
    if target != metadata.get('target', 'cli') or target not in ('cli', 'scanner', 'parser'):
        raise ValueError('case target differs from run.json or is unsupported')
    expected_inputs = (['globals.d.ts', 'main.ts', 'tsconfig.json'] if target == 'cli'
                       else ['main.ts', 'request.ndjson'])
    if manifest['inputs'] != expected_inputs:
        raise ValueError('unexpected input inventory for the named fuzz target')
    if target != 'cli':
        from tools.phase7.fuzz import protocol
        if manifest.get('protocol_limits') != metadata.get('protocol_limits') or manifest.get('protocol_limits') != protocol.LIMITS:
            raise ValueError('saved protocol limits differ from the named target')
        protocol.validate_input(target, {name: (case / name).read_bytes() for name in manifest['inputs']})
        if manifest['arguments'] not in ([], protocol.arguments_for(target)):
            raise ValueError('unexpected named protocol target arguments')
    for field in ('arguments', 'binaries', 'case_seconds', 'pin'):
        if manifest[field] != metadata[field]:
            raise ValueError(f'case manifest {field} differs from run.json')
    if manifest['memory_requested_bytes'] != metadata['memory']['requested_bytes']:
        raise ValueError('case memory allowance differs from run.json')
    if (not isinstance(manifest['arguments'], list)
            or not all(isinstance(item, str) for item in manifest['arguments'])):
        raise ValueError('recorded compiler arguments must be strings')
    timeout = manifest['case_seconds']
    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not math.isfinite(timeout) or timeout <= 0:
        raise ValueError('invalid saved per-case timeout')
    byte_limit = manifest['memory_requested_bytes']
    if type(byte_limit) is not int or byte_limit <= 0 or byte_limit % (1024 * 1024):
        raise ValueError('invalid saved memory limit')
    if set(manifest['binaries']) != {'go', 'rust'}:
        raise ValueError('expected exactly the saved Go/Rust pair')
    binaries = {}
    for runtime, recorded in manifest['binaries'].items():
        binary = Path(recorded['path'])
        if runtime == 'rust' and candidate_rust is not None:
            binary = candidate_rust.resolve()
        if not binary.is_absolute() or not binary.is_file() or not os.access(binary, os.X_OK):
            raise ValueError(f'{runtime} recorded executable is unavailable: {binary}')
        if not (runtime == 'rust' and candidate_rust is not None) and fuzz.identity(binary) != recorded:
            raise ValueError(f'{runtime} executable identity differs from the archive')
        binaries[runtime] = binary
    if binaries['go'].resolve() == binaries['rust'].resolve():
        raise ValueError('recorded Go/Rust executables must be distinct')
    saved = read_json(case / 'result.json')
    if (not isinstance(saved['failures'], list) or not saved['failures']
            or not all(isinstance(item, str) for item in saved['failures'])):
        raise ValueError('archive must contain explicit finding categories')
    if saved['failures'] != findings[0]['failures']:
        raise ValueError('archived failure categories differ from run.json')
    if set(saved['observations']) != {'go', 'rust'}:
        raise ValueError('archived observations must contain exactly Go/Rust')
    for observed in saved['observations'].values():
        valid_exit = type(observed['exit']) is int or (observed['exit'] is None
                     and isinstance(observed.get('execution_error'), str))
        if not valid_exit or type(observed['timeout']) is not bool:
            raise ValueError('invalid saved termination observation')
        for channel in ('stdout_hex', 'stderr_hex'):
            bytes.fromhex(observed[channel])
        if not isinstance(observed['emitted'], dict):
            raise ValueError('invalid saved emitted-file observation')
    return metadata, manifest, binaries, saved


OBSERVATION_FIELDS = ('exit', 'timeout', 'stdout_hex', 'stderr_hex', 'emitted',
                      'execution_error', 'runner_error', 'compiler_failure')


def observation_differences(saved, current):
    """Name changed observations; both archived and current bytes stay saved."""
    return {runtime: [field for field in OBSERVATION_FIELDS
                      if current.observations[runtime].get(field) != saved['observations'][runtime].get(field)]
            for runtime in ('go', 'rust')
            if any(current.observations[runtime].get(field) != saved['observations'][runtime].get(field)
                   for field in OBSERVATION_FIELDS)}


def classify(saved, current):
    if any(item.get('runner_error') or (item.get('execution_error')
            and item.get('compiler_failure') != 'output-limit')
           for item in current.observations.values()):
        return 'execution-failure'
    if not current.failures:
        return 'resolved'
    if current.failures == saved['failures'] and not observation_differences(saved, current):
        return 'reproduced'
    return 'execution-failure'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', type=Path, required=True, help='saved finding original/ or minimized/ directory')
    parser.add_argument('--output', type=Path, required=True, help='new result directory outside target')
    parser.add_argument('--attempts', type=int, default=1, help='one to ten fresh paired attempts')
    parser.add_argument('--budget-seconds', type=fuzz.positive, default=30)
    parser.add_argument('--run-sha256', help='optional independent SHA-256 anchor for run.json')
    parser.add_argument('--candidate-rust', type=Path,
                        help='explicit fixed Rust executable; original archive and pinned Go remain verified')
    args = parser.parse_args(argv)
    if os.name != 'posix':
        parser.error('process-group cleanup requires a POSIX host')
    if not 1 <= args.attempts <= 10:
        parser.error('attempts must be between one and ten')
    case, output = args.case.resolve(), args.output.resolve()
    if 'target' in output.parts or output.exists():
        parser.error('output must be a new directory outside target')
    output.mkdir(parents=True)
    report = {'schema': 1, 'case': str(case), 'attempts': [],
              'meaning': 'resolved means agreement on this attempt only; nondeterminism is not waived'}
    started = time.monotonic()
    try:
        run_path = case.parents[1] / 'run.json'
        run_hash = fuzz.sha256(run_path.read_bytes())
        report['run_sha256'] = run_hash
        if args.run_sha256 and run_hash != args.run_sha256:
            raise ValueError('run.json hash differs from the independent anchor')
        metadata, manifest, binaries, saved = verify(case, args.candidate_rust)
        report['target'] = manifest.get('target', 'cli')
        report['manifest_sha256'] = fuzz.sha256((case / 'manifest.json').read_bytes())
        report['original_failures'] = saved['failures']
        report['original_binaries'] = manifest['binaries']
        report['executed_binaries'] = {runtime: fuzz.identity(binary) for runtime, binary in binaries.items()}
        report['mode'] = 'candidate-rust' if args.candidate_rust is not None else 'exact-saved-pair'
        report['reproduction_meaning'] = ('archived observations reproduced against the explicit Rust candidate; '
                                          'not an original Rust binary replay'
                                          if args.candidate_rust is not None else 'original saved binary pair')
        report['commands'] = {runtime: [str(binary), *manifest['arguments']]
                              for runtime, binary in binaries.items()}
        memory = fuzz.memory_policy(manifest['memory_requested_bytes'] // (1024 * 1024))
        report['memory'] = memory
        # Platform enforcement is observed anew, never inherited as an assurance.
        context = {**metadata, 'memory': memory, 'binaries': report['executed_binaries']}
        inputs = {name: (case / name).read_bytes() for name in manifest['inputs']}
        deadline = started + args.budget_seconds
        for index in range(args.attempts):
            if report['target'] == 'cli':
                result = fuzz.compare_case(inputs['main.ts'], binaries, manifest['case_seconds'],
                                           memory, deadline, files=inputs, arguments=manifest['arguments'])
            else:
                from tools.phase7.fuzz import protocol
                result = protocol.compare_case(report['target'], inputs['main.ts'], binaries,
                                               manifest['case_seconds'], memory, deadline,
                                               files=inputs, arguments=manifest['arguments'])
            status = classify(saved, result)
            directory = output / f'attempt-{index:02d}'
            fuzz.save_case(directory, inputs['main.ts'], result, files=inputs, context=context)
            report['attempts'].append({'directory': directory.name, 'status': status,
                                       'failures': result.failures,
                                       'changed_observations': observation_differences(saved, result)})
        statuses = {item['status'] for item in report['attempts']}
        report['status'] = ('execution-failure' if 'execution-failure' in statuses
                            else 'reproduced' if 'reproduced' in statuses else 'resolved')
    except (OSError, ValueError, KeyError, TypeError, TimeoutError, IndexError, subprocess.SubprocessError) as error:
        report['status'] = 'execution-failure'
        report['error'] = f'{type(error).__name__}: {error}'
    finally:
        report['wall_seconds'] = time.monotonic() - started
        (output / 'replay.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'status': report['status'], 'attempts': report['attempts'], 'output': str(output)}))
    return {'resolved': 0, 'reproduced': 1, 'execution-failure': 2}[report['status']]


if __name__ == '__main__':
    raise SystemExit(main())
