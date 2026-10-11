#!/usr/bin/env python3
"""Bounded, deterministic differential fuzzing against explicit saved executables.

Targets observe CLI diagnostics/emit or S05/S06 library records and termination.
It is blind mutation, not coverage-guided instrumentation.
No build or compiler fallback occurs here. See README.md for replay and limits.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import math
import os
from pathlib import Path
import random
import re
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.phase7.bench.common import identity, sha256, tree_files  # noqa: E402

CONFIG = {'compilerOptions': {'noLib': True, 'types': [], 'strict': True,
          'target': 'es2020', 'module': 'commonjs', 'declaration': True,
          'noEmitOnError': False, 'outDir': 'out', 'newLine': 'lf',
          'skipLibCheck': True}, 'files': ['globals.d.ts', 'main.ts']}
GLOBALS = b'''interface Array<T> { length: number; [n: number]: T; }
interface Boolean {} interface Function {} interface CallableFunction {}
interface NewableFunction {} interface IArguments {} interface Number {}
interface Object {} interface RegExp {} interface String {}
'''
ARGS = ['--project', 'tsconfig.json', '--pretty', 'false', '--checkers', '1']


@dataclass
class Pair:
    observations: dict
    artifacts: dict
    failures: list[str]


def memory_policy(megabytes):
    # macOS accepts RLIMIT_AS but does not reliably enforce it. Never claim it.
    supported = sys.platform.startswith('linux')
    return {'requested_bytes': megabytes * 1024 * 1024,
            'enforced': supported, 'mechanism': 'RLIMIT_AS' if supported else None,
            'note': 'per-process address space, not aggregate descendant RSS' if supported
            else 'memory enforcement unsupported on this platform; informational only'}


def _child_limits(memory, output_limit):
    import resource
    if memory['enforced']:
        byte_limit = memory['requested_bytes']
        resource.setrlimit(resource.RLIMIT_AS, (byte_limit, byte_limit))
    if output_limit is not None:
        # Both redirected channels are regular files. Keep one excess byte so
        # truncation is distinguishable from a complete stream at the limit.
        resource.setrlimit(resource.RLIMIT_FSIZE, (output_limit + 1, output_limit + 1))


def _kill_group(child):
    # Also called on a normal exit: descendants must not outlive their case.
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def run_child(binary, cwd, timeout, memory, arguments=None, *, stdin_bytes=None, max_output_bytes=None):
    """Redirect to files, so a child holding pipes cannot evade the timeout."""
    started = time.monotonic()
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr, tempfile.TemporaryFile() as stdin:
        if stdin_bytes is not None:
            stdin.write(stdin_bytes)
            stdin.seek(0)
        try:
            child = subprocess.Popen([str(binary), *(ARGS if arguments is None else arguments)],
                                     cwd=cwd, stdin=stdin if stdin_bytes is not None else subprocess.DEVNULL,
                                     stdout=stdout, stderr=stderr, start_new_session=True,
                                     preexec_fn=(lambda: _child_limits(memory, max_output_bytes))
                                     if memory['enforced'] or max_output_bytes is not None else None)
        except (OSError, subprocess.SubprocessError) as error:
            return {'exit': None, 'timeout': False, 'wall_seconds': time.monotonic() - started,
                    'execution_error': f'{type(error).__name__}: {error}'}, b'', b''
        timed_out = False
        try:
            child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
        finally:
            _kill_group(child)
            child.wait()
        stdout.seek(0)
        stderr.seek(0)
        result = {'exit': child.returncode, 'timeout': timed_out,
                  'wall_seconds': time.monotonic() - started}
        channels = [stream.read() if max_output_bytes is None else stream.read(max_output_bytes + 1)
                    for stream in (stdout, stderr)]
        if max_output_bytes is not None and any(len(data) > max_output_bytes for data in channels):
            result['execution_error'] = f'captured channel exceeds {max_output_bytes} bytes; retained prefix only'
            channels = [data[:max_output_bytes] for data in channels]
        return result, *channels


def normalize(data, directory):
    """Keep all diagnostic bytes/order, replacing only case roots and CRLF."""
    return data.replace(os.fsencode(directory), b'@CASE@').replace(b'\r\n', b'\n')


def input_files(source):
    return {'main.ts': source, 'globals.d.ts': GLOBALS,
            'tsconfig.json': (json.dumps(CONFIG, sort_keys=True, indent=2) + '\n').encode()}


def compare_case(source, binaries, timeout, memory, deadline=math.inf, *, files=None, arguments=None):
    if deadline - time.monotonic() < timeout:
        raise TimeoutError('insufficient budget for a complete paired case')
    observations, artifacts = {}, {}
    files = input_files(source) if files is None else files
    case_deadline = min(deadline, time.monotonic() + timeout)
    with tempfile.TemporaryDirectory(prefix='phase7-fuzz-') as temporary:
        for runtime in ('go', 'rust'):
            directory = (Path(temporary) / runtime).resolve()
            directory.mkdir()
            for name, data in files.items():
                (directory / name).write_bytes(data)
            # Both compilers get the same half of the total per-case allowance.
            remaining = min(timeout / 2, case_deadline - time.monotonic())
            if remaining <= 0:
                raise TimeoutError('run budget exhausted before paired compilation')
            result, stdout, stderr = run_child(binaries[runtime], directory, remaining, memory, arguments)
            emitted = {path.relative_to(directory / 'out').as_posix(): path.read_bytes()
                       for path in sorted((directory / 'out').rglob('*')) if path.is_file()}
            result.update(stdout_hex=normalize(stdout, directory).hex(),
                          stderr_hex=normalize(stderr, directory).hex(),
                          emitted=tree_files(directory / 'out'))
            observations[runtime] = result
            artifacts[runtime] = {'stdout': stdout, 'stderr': stderr, 'emitted': emitted}
    failures = []
    for runtime, result in observations.items():
        if result.get('execution_error'):
            failures.append(runtime + '-execution-failure')
        elif result['timeout']:
            failures.append(runtime + '-timeout')
        elif result['exit'] not in (0, 1, 2):
            failures.append(runtime + '-crash')
    for field in ('exit', 'stdout_hex', 'stderr_hex'):
        if observations['go'][field] != observations['rust'][field]:
            failures.append(field)
    if artifacts['go']['emitted'] != artifacts['rust']['emitted']:
        failures.append('emitted')
    return Pair(observations, artifacts, failures)


def generated(rng):
    number = rng.randrange(-5, 6)
    type_name = rng.choice(['number', 'string', 'number | string'])
    value = rng.choice([str(number), json.dumps('text' + str(number)), 'undefined'])
    statements = [f'export const value: {type_name} = {value};',
                  'export function id<T>(x: T): T { return x; }',
                  'export interface Box<T> { value: T; }',
                  'export class Item { constructor(public value: number) {} }',
                  'export const pair: [number, string] = [1, "two"];',
                  'export type Choice = { kind: "a"; x: number } | { kind: "b"; x: string };',
                  'export function choose(x: Choice) { return x.kind === "a" ? x.x + 1 : x.x; }']
    return ('\n'.join(statements[:1] + rng.sample(statements[1:], rng.randrange(1, 7))) + '\n').encode()


def mutate(source, rng):
    """One byte or token mutation; malformed UTF-8 and syntax are intentional."""
    if not source:
        return generated(rng)
    if rng.randrange(2):
        tokens = list(re.finditer(rb'[A-Za-z_$][A-Za-z_0-9$]*|\d+|[^\s]', source))
        if tokens:
            token = rng.choice(tokens)
            replacement = rng.choice([b'', b'number', b'string', b'undefined', b'?', b'{', b'0'])
            return source[:token.start()] + replacement + source[token.end():]
    start = rng.randrange(len(source))
    end = min(len(source), start + rng.randrange(1, 9))
    return source[:start] + rng.choice([b'', b'\xff', b'\n', b'"', b'/*', b'}', b'\x00']) + source[end:]


def failure_signature(result):
    return (result.failures, [(item['exit'], item['timeout'], item.get('execution_error'))
                              for item in result.observations.values()])


def minimize(source, original, evaluate, max_trials, deadline):
    """Bounded chunk deletion, retaining the exact observed failure categories.

    Every accepted candidate has been compiled by both children. The original
    result remains saved even if the limit prevents further reduction.
    """
    best, result, trials, chunks = source, original, 0, 2
    while best and trials < max_trials and time.monotonic() < deadline:
        width = max(1, math.ceil(len(best) / chunks))
        reduced = False
        for start in range(0, len(best), width):
            if trials >= max_trials or time.monotonic() >= deadline:
                break
            candidate = best[:start] + best[start + width:]
            try:
                observed = evaluate(candidate)
            except TimeoutError:
                return best, result, trials
            trials += 1
            if failure_signature(observed) == failure_signature(original):
                best, result, reduced = candidate, observed, True
                chunks = max(2, chunks - 1)
                break
        if not reduced:
            if width == 1:
                break
            chunks = min(len(best), chunks * 2)
    return best, result, trials


def save_case(directory, source, result, *, files=None, context=None):
    directory.mkdir(parents=True)
    files = input_files(source) if files is None else files
    for name, data in files.items():
        (directory / name).write_bytes(data)
    (directory / 'result.json').write_text(json.dumps({
        'failures': result.failures, 'observations': result.observations}, indent=2) + '\n')
    for runtime, artifact in result.artifacts.items():
        destination = directory / runtime
        destination.mkdir()
        (destination / 'stdout').write_bytes(artifact['stdout'])
        (destination / 'stderr').write_bytes(artifact['stderr'])
        for name, data in artifact['emitted'].items():
            path = destination / 'out' / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    if context is not None:
        manifest = {'schema': 1, 'inputs': sorted(files), 'arguments': context['arguments'],
                    'target': context.get('target', 'cli'),
                    'binaries': context['binaries'], 'case_seconds': context['case_seconds'],
                    'memory_requested_bytes': context['memory']['requested_bytes'],
                    'pin': context['pin'], 'files': tree_files(directory)}
        if 'protocol_limits' in context:
            manifest['protocol_limits'] = context['protocol_limits']
        (directory / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        return sha256((directory / 'manifest.json').read_bytes())
    return None


def positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError('must be finite and positive')
    return number


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rust', type=Path, required=True, help='saved tsrust executable')
    parser.add_argument('--go', type=Path, required=True, help='saved pinned tsgo executable')
    parser.add_argument('--target', choices=('cli', 'scanner', 'parser'), default='cli',
                        help='exact executable protocol: CLI, S05 scanner or parse-only S06 adapters')
    parser.add_argument('--output', type=Path, required=True, help='new persistent directory outside target')
    parser.add_argument('--seed', type=int, default=1)
    parser.add_argument('--iterations', type=int, default=100)
    parser.add_argument('--budget-seconds', type=positive, default=60)
    parser.add_argument('--case-seconds', type=positive, default=5)
    parser.add_argument('--memory-mb', type=int, default=1024)
    parser.add_argument('--minimize-trials', type=int, default=30)
    parser.add_argument('--max-failures', type=int, default=10)
    parser.add_argument('--corpus', type=Path, default=Path(__file__).with_name('corpus'))
    parser.add_argument('--replay', type=Path, help='replay one saved main.ts instead of generation')
    args = parser.parse_args(argv)
    if os.name != 'posix':
        parser.error('process-group cleanup requires a POSIX host')
    if min(args.iterations, args.memory_mb, args.max_failures) <= 0 or args.minimize_trials < 0:
        parser.error('iterations, memory-mb and max-failures must be positive; minimize-trials nonnegative')
    binaries = {'rust': args.rust.resolve(), 'go': args.go.resolve()}
    if binaries['rust'] == binaries['go']:
        parser.error('Rust and Go must be distinct saved executables')
    for binary in binaries.values():
        if not binary.is_file() or not os.access(binary, os.X_OK):
            parser.error(f'not an executable file: {binary}')
    output = args.output.resolve()
    if 'target' in output.parts or output.exists():
        parser.error('output must be a new directory outside target')
    corpus = [(str(path), path.read_bytes()) for path in sorted(args.corpus.glob('*.ts'))]
    if not corpus and not args.replay:
        parser.error('corpus must contain at least one .ts seed')
    memory = memory_policy(args.memory_mb)
    protocol = None
    if args.target != 'cli':
        from tools.phase7.fuzz import protocol
    metadata = {'schema': 1, 'seed': args.seed, 'iterations_requested': args.iterations,
                'budget_seconds': args.budget_seconds, 'case_seconds': args.case_seconds,
                'target': args.target, 'arguments': ARGS if protocol is None else [],
                'binaries': {key: identity(path) for key, path in binaries.items()},
                'pin': json.loads((ROOT / 'data/upstream.json').read_text())['pin'],
                'memory': memory, 'coverage': 'not collected (blind byte/token mutation)',
                'corpus': [{'path': name, 'sha256': sha256(data)} for name, data in corpus],
                'executions': 0, 'failures': [], 'minimize_executions': 0, 'stop_reason': 'iterations'}
    if protocol is not None:
        metadata['protocol_limits'] = protocol.LIMITS
        if args.replay and args.replay.stat().st_size > protocol.MAX_SOURCE:
            parser.error(f'protocol replay source exceeds {protocol.MAX_SOURCE} bytes')
    output.mkdir(parents=True)
    rng = random.Random(args.seed)
    started = time.monotonic()
    deadline = started + args.budget_seconds
    files_for = input_files if protocol is None else lambda source: protocol.input_files(args.target, source)
    evaluate = (lambda source: compare_case(source, binaries, args.case_seconds, memory, deadline)
                if protocol is None else protocol.compare_case(args.target, source, binaries,
                                                               args.case_seconds, memory, deadline))
    try:
        with (output / 'cases.ndjson').open('w') as log:
            for index in range(1 if args.replay else args.iterations):
                if time.monotonic() >= deadline:
                    metadata['stop_reason'] = 'budget'
                    break
                if args.replay:
                    origin, source = str(args.replay), args.replay.read_bytes()
                elif index % 2 == 0:
                    origin, source = 'generated', generated(rng)
                else:
                    origin, seed_source = rng.choice(corpus)
                    source = mutate(seed_source, rng)
                if protocol is not None:
                    source = source[:protocol.MAX_SOURCE]
                try:
                    result = evaluate(source)
                except TimeoutError:
                    metadata['stop_reason'] = 'budget'
                    break
                metadata['executions'] += 1
                row = {'index': index, 'origin': origin, 'input_sha256': sha256(source),
                       'failures': result.failures, 'observations': result.observations}
                log.write(json.dumps(row) + '\n')
                log.flush()
                if result.failures:
                    finding = output / f'failure-{index:06d}'
                    original_manifest = save_case(finding / 'original', source, result,
                                                  files=files_for(source), context=metadata)
                    minimal, reduced, trials = minimize(source, result, evaluate,
                                                        args.minimize_trials, deadline)
                    minimized_manifest = save_case(finding / 'minimized', minimal, reduced,
                                                   files=files_for(minimal), context=metadata)
                    metadata['minimize_executions'] += trials
                    metadata['failures'].append({'index': index, 'directory': finding.name,
                                                 'failures': result.failures, 'minimize_trials': trials,
                                                 'manifests': {'original': original_manifest,
                                                               'minimized': minimized_manifest}})
                    if len(metadata['failures']) >= args.max_failures:
                        metadata['stop_reason'] = 'max-failures'
                        break
    finally:
        metadata['wall_seconds'] = time.monotonic() - started
        metadata['budget_exhausted'] = time.monotonic() >= deadline
        (output / 'run.json').write_text(json.dumps(metadata, indent=2) + '\n')
    print(json.dumps({'executions': metadata['executions'], 'failures': len(metadata['failures']),
                      'output': str(output), 'memory': memory}))
    return 1 if metadata['failures'] else (0 if metadata['executions'] else 2)


if __name__ == '__main__':
    raise SystemExit(main())
