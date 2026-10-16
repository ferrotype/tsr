#!/usr/bin/env python3
"""Compare checker operation backends in saved experimental binaries.

Observe both backends first. Timed pairs use fresh processes released from a
launch barrier, with alternating submission order. No parsing or hashing runs
until both children finish. The timer is operations.elapsed_ns, not process
wall time; simultaneous pairs are a development screen, not throughput data.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import statistics
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[3]
BACKENDS = ('routed', 'scoped')
ARGUMENT_BACKENDS = (*BACKENDS, 'typed', 'state', 'combined')
SHA = re.compile(r'^[0-9a-f]{64}$')
ARGUMENT_COUNTS = ('calls', 'outermost_calls', 'errors', 'location_errors', 'cleanup_errors')


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def identity(path):
    return {'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'bytes': path.stat().st_size}


def source_identity():
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    status = subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True)
    return {'revision': revision, 'dirty': bool(status), 'status': status,
            'meaning': 'checkout at runner start; not a claim about binary build inputs'}


def participants(binary, experiment, *, baseline_binary=None,
                 baseline_backend='routed', candidate_backend='scoped'):
    """Keep process identity distinct even when both executables use one backend."""
    if experiment not in ('argument', 'flow'):
        raise ValueError('unknown experiment')
    allowed = ARGUMENT_BACKENDS if experiment == 'argument' else BACKENDS
    if baseline_backend not in allowed or candidate_backend not in allowed:
        raise ValueError(f'unsupported {experiment} backend')
    labels = ((baseline_backend, candidate_backend) if baseline_backend != candidate_backend
              else ('baseline', 'candidate'))
    return {
        labels[0]: {'role': 'baseline', 'backend': baseline_backend,
                    'binary': Path(baseline_binary if baseline_binary is not None else binary)},
        labels[1]: {'role': 'candidate', 'backend': candidate_backend, 'binary': Path(binary)},
    }


def execute(command, cwd, timeout):
    """Reap each child, keeping partial output when it fails or times out."""
    result = {'command': command, 'cwd': str(cwd), 'launch_ns': time.monotonic_ns()}
    try:
        child = subprocess.Popen(command, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 text=True, errors='replace', start_new_session=True)
    except OSError as error:
        return {**result, 'exit': None, 'stdout': '', 'stderr': '', 'error': str(error),
                'finish_ns': time.monotonic_ns()}
    try:
        stdout, stderr = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        result['error'] = f'child timed out after {timeout}s'
        result['timed_out'] = True
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        stdout, stderr = child.communicate()
    result.update(exit=child.returncode, stdout=stdout, stderr=stderr, finish_ns=time.monotonic_ns())
    return result


def count(value, name, *, positive=False):
    if type(value) is not int or value < (1 if positive else 0):
        raise ValueError(f'{name} must be a {"positive" if positive else "nonnegative"} integer')


def digest(value, name):
    if not isinstance(value, str) or not SHA.fullmatch(value):
        raise ValueError(f'{name} must be a sha256')


def validate(data, experiment, backend, mode):
    if not isinstance(data, dict) or type(data.get('format')) is not int or data['format'] != 1:
        raise ValueError('invalid operation report format')
    for key, expected in [('experiment', experiment), ('backend', backend), ('mode', mode), ('complete', True)]:
        if data.get(key) != expected or key == 'complete' and data.get(key) is not True:
            raise ValueError(f'invalid {key}: expected {expected!r}')
    if 'error' in data or 'error_json' in data:
        raise ValueError('child reported an error')
    for key in ('setup_ns', 'semantic_ns'):
        if key in data:
            count(data[key], key)
    workload = data.get('workload')
    if not isinstance(workload, dict):
        raise ValueError('missing workload')
    files = workload.get('files')
    if not isinstance(files, list) or not files:
        raise ValueError('workload files must be a nonempty ordered list')
    paths = []
    for item in files:
        if not isinstance(item, dict) or not isinstance(item.get('path'), str) or not item['path']:
            raise ValueError('invalid workload file path')
        paths.append(item['path'])
        digest(item.get('sha256'), 'file sha256')
        digest(item.get('options_sha256'), 'file options_sha256')
    if len(paths) != len(set(paths)):
        raise ValueError('duplicate workload file path')
    digest(workload.get('options_sha256'), 'workload.options_sha256')
    configurations = workload.get('configurations')
    if not isinstance(configurations, list) or not configurations:
        raise ValueError('workload configurations must be a nonempty ordered list')
    config_paths = []
    for configuration in configurations:
        if not isinstance(configuration, dict) or not isinstance(configuration.get('path'), str) or not configuration['path']:
            raise ValueError('invalid configuration path')
        config_paths.append(configuration['path'])
        digest(configuration.get('sha256'), 'configuration sha256')
    if len(config_paths) != len(set(config_paths)):
        raise ValueError('duplicate configuration path')
    digest(workload.get('diagnostics_sha256'), 'diagnostics_sha256')
    for key in ['diagnostics_count', 'types', 'signatures', 'instantiations']:
        count(workload.get(key), f'workload.{key}', positive=key == 'types')
    ops = data.get('operations')
    if not isinstance(ops, dict):
        raise ValueError('missing operations')
    count(ops.get('elapsed_ns'), 'elapsed_ns', positive=mode == 'time')
    if experiment == 'argument':
        for key in ARGUMENT_COUNTS:
            count(ops.get(key), key, positive=key in ('calls', 'outermost_calls'))
        if ops['outermost_calls'] > ops['calls'] or ops['location_errors'] or ops['cleanup_errors']:
            raise ValueError('invalid argument coverage, location_errors, or cleanup_errors')
        if mode == 'observe':
            checksum = ops.get('trace_checksum')
            count(checksum, 'trace_checksum')
            count(ops.get('inferential_calls'), 'inferential_calls')
            for key in ('modes', 'depths'):
                if not isinstance(ops.get(key), dict) or not ops[key]:
                    raise ValueError(f'missing observed {key}')
            kinds, samples = ops.get('kinds'), ops.get('samples')
            if not isinstance(kinds, dict) or not kinds:
                raise ValueError('observed kinds must be a nonempty count map')
            for kind, occurrences in kinds.items():
                if not re.fullmatch(r'-?[0-9]+', kind):
                    raise ValueError('observed kind must be a signed integer key')
                count(occurrences, 'kind count', positive=True)
            if sum(kinds.values()) != ops['calls']:
                raise ValueError('observed kind counts differ from calls')
            if not isinstance(samples, list) or not samples or len(samples) > ops['calls']:
                raise ValueError('observed samples must cover a nonempty subset of calls')
    else:
        for key in ['requests', 'total_seen', 'errors', 'local_requests', 'routed_requests',
                    'node_reads', 'list_reads', 'semantic_calls']:
            count(ops.get(key), key, positive=key in ('requests', 'total_seen', 'node_reads'))
        if type(ops.get('truncated')) is not bool:
            raise ValueError('truncated must be a boolean')
        if ops['total_seen'] < ops['requests']:
            raise ValueError('total_seen cannot be less than captured requests')
        if 'types_after_check' in ops:
            count(ops['types_after_check'], 'types_after_check', positive=True)
        for key in ['request_sha256', 'result_sha256']:
            digest(ops.get(key), key)
        if ops['errors']:
            raise ValueError('flow errors must be zero')
        if ops['local_requests'] + ops['routed_requests'] < ops['requests']:
            raise ValueError('flow backend request coverage is below captured requests')
        if backend == 'routed' and ops['local_requests']:
            raise ValueError('routed flow must not use local_requests')
        if backend == 'scoped' and ops['local_requests'] == 0:
            raise ValueError('scoped flow must have local_requests coverage')
    return data


def comparable(data, *, observed=False):
    """Exclude timers and backend-routing counters, never ordered outputs."""
    excluded = {'elapsed_ns', 'local_requests', 'routed_requests'}
    if not observed:
        excluded.update(('trace_checksum', 'kinds', 'samples', 'modes', 'depths', 'inferential_calls'))
    result = {'workload': data['workload'],
              'operations': {key: value for key, value in data['operations'].items() if key not in excluded}}
    if data['experiment'] == 'flow':
        # Semantic callbacks can reenter the kernel; these counts include them.
        result['kernel_entries'] = data['operations']['local_requests'] + data['operations']['routed_requests']
    return result


def collect(binary, project, experiment, mode, output, index, timeout, baseline=None, *,
            baseline_binary=None, baseline_backend='routed', candidate_backend='scoped'):
    sides = participants(binary, experiment, baseline_binary=baseline_binary,
                         baseline_backend=baseline_backend, candidate_backend=candidate_backend)
    baseline_label, candidate_label = sides
    order = list(sides if index % 2 == 0 else reversed(sides))
    barrier = threading.Barrier(2) if mode == 'time' else None

    def child(label):
        side = sides[label]
        if barrier is not None:
            barrier.wait(timeout=10)
        return execute([str(side['binary']), str(project), experiment, side['backend'], mode],
                       project.parent, timeout)

    with concurrent.futures.ThreadPoolExecutor(2 if barrier else 1) as pool:
        futures = {label: pool.submit(child, label) for label in order}
    # Both processes have ended before any parsing, validation, or output IO.
    raw, reports, failures = {}, {}, []
    for label, future in futures.items():
        try:
            result = future.result()
        except Exception as error:
            result = {'exit': None, 'stdout': '', 'stderr': '', 'error': str(error)}
        result.update(role=sides[label]['role'], backend=sides[label]['backend'])
        raw[label] = result
        prefix = output / f'{mode}-{index}-{label}'
        prefix.with_suffix('.stdout').write_text(result['stdout'])
        prefix.with_suffix('.stderr').write_text(result['stderr'])
        write_json(prefix.with_suffix('.json'), {k: v for k, v in result.items() if k not in ('stdout', 'stderr')})
    for label, result in raw.items():
        issues = []
        if result.get('error'):
            issues.append(result['error'])
        if result['exit'] != 0:
            issues.append(f"child exited {result['exit']}")
        if mode == 'time' and result['stderr']:
            issues.append('timed child wrote to stderr: ' + result['stderr'][:200])
        try:
            data = json.loads(result['stdout'])
            result['report'] = data
            reports[label] = validate(data, experiment, sides[label]['backend'], mode)
            if baseline is not None and comparable(data) != comparable(baseline):
                issues.append('workload or operation counts/outputs differ from observation')
        except (ValueError, TypeError) as error:
            issues.append(str(error))
        result['issues'] = issues
        write_json(output / f'{mode}-{index}-{label}.json',
                   {k: v for k, v in result.items() if k not in ('stdout', 'stderr')})
        failures += [f'{label}: {issue}' for issue in issues]
    if len(reports) == 2 and comparable(reports[baseline_label], observed=mode == 'observe') != comparable(
            reports[candidate_label], observed=mode == 'observe'):
        failures.append('baseline/candidate workload or ordered operation counts/outputs differ')
    if failures:
        raise ValueError('; '.join(failures))
    if mode == 'observe':
        return reports[baseline_label]
    elapsed = {label: report['operations']['elapsed_ns'] for label, report in reports.items()}
    result = {'pair': index, 'submission_order': order, 'elapsed_ns': elapsed,
              'roles': {'baseline': baseline_label, 'candidate': candidate_label},
              'ratio': elapsed[candidate_label] / elapsed[baseline_label],
              'saved_ns': elapsed[baseline_label] - elapsed[candidate_label],
              'launch_skew_ms': abs(raw[candidate_label]['launch_ns'] - raw[baseline_label]['launch_ns']) / 1e6}
    # Useful attribution, but neither interval is substituted for the operation timer.
    for key in ('setup_ns', 'semantic_ns'):
        if all(key in report for report in reports.values()):
            result[key] = {label: report[key] for label, report in reports.items()}
    return result


def run(binary, project, experiment, output, *, pairs=5, timeout=600, build_manifest=None,
        baseline_binary=None, baseline_backend='routed', candidate_backend='scoped',
        baseline_build_manifest=None):
    if type(pairs) is not int or pairs < 1 or not math.isfinite(timeout) or timeout <= 0:
        raise ValueError('pairs and timeout must be positive')
    binary, project, output = [Path(path).resolve() for path in (binary, project, output)]
    baseline_binary = Path(baseline_binary).resolve() if baseline_binary is not None else binary
    sides = participants(binary, experiment, baseline_binary=baseline_binary,
                         baseline_backend=baseline_backend, candidate_backend=candidate_backend)
    if output.is_relative_to(ROOT):
        raise ValueError('output must be outside the checkout, including target directories')
    if not binary.is_file() or not baseline_binary.is_file() or not project.is_file():
        raise ValueError('both binaries and project must be existing files')
    output.mkdir(parents=True)  # Never replace an earlier result, even a failed one.
    record = {'format': 1, 'experiment': experiment, 'project': str(project), 'requested_pairs': pairs,
              'timeout_seconds': timeout, 'complete': False, 'pairs': [],
              'roles': {side['role']: label for label, side in sides.items()},
              'backends': {side['role']: side['backend'] for side in sides.values()},
              'method': 'observe compatibility, then concurrent fresh-process operation timer pairs; alternating submission order',
              'metric': 'operations.elapsed_ns; not process wall time or throughput'}
    summary = output / 'summary.json'
    write_json(summary, record)
    try:
        record['binary'] = identity(binary)
        record['baseline_binary'] = identity(baseline_binary)
        record['source'] = source_identity()
        for key, supplied in [('build_manifest', build_manifest),
                              ('baseline_build_manifest', baseline_build_manifest)]:
            if supplied is not None:
                manifest = Path(supplied).resolve()
                metadata = json.loads(manifest.read_text())
                record[key] = {'identity': identity(manifest), 'metadata': metadata,
                               'meaning': 'supplied metadata only; build flags are not verified by this runner'}
                if not isinstance(metadata, dict):
                    raise ValueError(f'{key} must contain an object')
                expected = record['baseline_binary' if key == 'baseline_build_manifest' else 'binary']['sha256']
                if 'binary_sha256' in metadata and metadata['binary_sha256'] != expected:
                    raise ValueError(f'{key} binary_sha256 differs from the selected binary')
        write_json(summary, record)
        selected = {'baseline_binary': baseline_binary, 'baseline_backend': baseline_backend,
                    'candidate_backend': candidate_backend}
        baseline = collect(binary, project, experiment, 'observe', output, 0, timeout, **selected)
        record['observation_compatible'] = True
        write_json(summary, record)
        for index in range(pairs):
            pair = collect(binary, project, experiment, 'time', output, index, timeout, baseline, **selected)
            record['pairs'].append(pair)
            write_json(summary, record)
            print(json.dumps(pair), flush=True)
        if identity(binary) != record['binary']:
            raise ValueError('candidate binary changed during the experiment')
        if identity(baseline_binary) != record['baseline_binary']:
            raise ValueError('baseline binary changed during the experiment')
        record['median_ratio'] = statistics.median(pair['ratio'] for pair in record['pairs'])
        record['median_elapsed_ns'] = {backend: statistics.median(pair['elapsed_ns'][backend] for pair in record['pairs'])
                                       for backend in sides}
        baseline_label, candidate_label = sides
        record['median_saved_ns'] = statistics.median(
            pair['elapsed_ns'][baseline_label] - pair['elapsed_ns'][candidate_label] for pair in record['pairs'])
        for key in ('setup_ns', 'semantic_ns'):
            if all(key in pair for pair in record['pairs']):
                record[f'median_{key}'] = {
                    label: statistics.median(pair[key][label] for pair in record['pairs']) for label in sides}
        record['complete'] = True
    except Exception as error:
        record['error'] = str(error)
        write_json(summary, record)
        raise
    write_json(summary, record)
    return record


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    command = commands.add_parser('run')
    command.add_argument('--binary', type=Path, required=True)
    command.add_argument('--baseline-binary', type=Path, help='saved control executable; defaults to --binary')
    command.add_argument('--baseline-backend', choices=ARGUMENT_BACKENDS, default='routed')
    command.add_argument('--candidate-backend', choices=ARGUMENT_BACKENDS, default='scoped')
    command.add_argument('--project', type=Path, required=True)
    command.add_argument('--experiment', choices=('argument', 'flow'), required=True)
    command.add_argument('--pairs', type=int, default=5)
    command.add_argument('--timeout', type=float, default=600, help='seconds per child')
    command.add_argument('--output', type=Path, required=True, help='new directory outside the checkout')
    command.add_argument('--build-manifest', type=Path, help='archive supplied build metadata; flags are not verified')
    command.add_argument('--baseline-build-manifest', type=Path, help='metadata for the saved control executable')
    args = parser.parse_args(argv)
    try:
        run(args.binary, args.project, args.experiment, args.output, pairs=args.pairs,
            timeout=args.timeout, build_manifest=args.build_manifest,
            baseline_binary=args.baseline_binary, baseline_backend=args.baseline_backend,
            candidate_backend=args.candidate_backend, baseline_build_manifest=args.baseline_build_manifest)
    except (OSError, ValueError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    main()
