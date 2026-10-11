#!/usr/bin/env python3
"""Bounded concurrent batched-symbol development screens, not API acceptance.

Explicit tsserverPath selects each saved executable; no shared symlink changes.
Fresh clients perform the pinned task's setup and core.ts checker warmup. Both
must be READY before timing, and DONE before work hashing, output or teardown.
Use capture.py for serial acceptance; build and profile outside these screens.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import selectors
import signal
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.phase5.latency.capture import identity as fixture_identity  # noqa: E402
from tools.phase6.jsapi.runner import pinned_node  # noqa: E402
from tools.phase7.bench.common import identity  # noqa: E402

HERE = Path(__file__).resolve().parent


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def receive(children, expected, timeout):
    deadline, pending = time.monotonic() + timeout, dict.fromkeys(children, b'')
    with selectors.DefaultSelector() as selector:
        for name, child in children.items():
            selector.register(child.stdout, selectors.EVENT_READ, name)
        while pending:
            ready = selector.select(max(0, deadline - time.monotonic()))
            if not ready:
                raise TimeoutError(f'waiting for {expected}: {list(pending)}')
            for key, _ in ready:
                name = key.data
                data = os.read(key.fd, 4096)
                if not data:
                    raise ValueError(f'{name} exited before {expected}')
                pending[name] += data
                if b'\n' in pending[name]:
                    if pending[name] != (expected + '\n').encode():
                        raise ValueError(f'{name}: unexpected handshake {pending[name]!r}')
                    del pending[name]
                    selector.unregister(key.fileobj)


def run_pair(node, binaries, mode, task, iterations, output, index, timeout):
    output.mkdir()
    children, logs, commands = {}, {}, {}
    order = ['control', 'candidate'] if index % 2 == 0 else ['candidate', 'control']
    try:
        for name in order:
            commands[name] = [node, '--conditions', '@typescript/source', '--experimental-import-meta-resolve',
                              str(HERE / 'development.mjs'), str(binaries[name]), mode, task,
                              str(iterations), str(output / f'{name}.json')]
            logs[name] = (output / f'{name}.stderr').open('wb')
            children[name] = subprocess.Popen(commands[name], cwd=ROOT / 'upstream', stdin=subprocess.PIPE,
                                              stdout=subprocess.PIPE, stderr=logs[name], start_new_session=True)
        write(output / 'commands.json', commands)
        receive(children, 'READY', timeout)
        released = {}
        for name in order:
            released[name] = time.monotonic_ns()
            children[name].stdin.write(b'run\n')
            children[name].stdin.flush()
        receive(children, 'DONE', timeout)
        for child in children.values():
            child.stdin.write(b'collect\n')
            child.stdin.flush()
        for name, child in children.items():
            if child.wait(timeout=timeout) != 0:
                raise ValueError(f'{name} failed; see {output / (name + ".stderr")}')
        results = {name: json.loads((output / f'{name}.json').read_text()) for name in order}
        for name, result in results.items():
            life, samples, work = result['lifecycle'], result['samples_ns'], result['work']
            if (result['method'] != 'development concurrent' or result['mode'] != mode or result['task'] != task
                    or result['iterations'] != iterations or work['iterations'] != iterations
                    or type(work['count']) is not int or work['count'] < 1
                    or not re.fullmatch('[0-9a-f]{64}', str(work['sha256']))):
                raise ValueError(f'{name}: invalid workload identity or work observation')
            if (life['remaining'] or life['spawned'] != 1 or life['reaped'] != 1 or life['peak'] != 1
                    or len(samples) != iterations or any(type(value) is not int or value <= 0 for value in samples)):
                raise ValueError(f'{name}: invalid lifecycle or samples')
        if results['control']['node'] != results['candidate']['node']:
            raise ValueError('candidate/control Node versions differ')
        if results['control']['work'] != results['candidate']['work']:
            raise ValueError('candidate/control input schedule or symbol answers differ')
        medians = {name: statistics.median(result['samples_ns']) for name, result in results.items()}
        return {'index': index, 'submission_order': order, 'commands': commands, 'work': results['control']['work'],
                'release_skew_ms': abs(released['candidate'] - released['control']) / 1e6,
                'median_ns': medians, 'ratio': medians['candidate'] / medians['control']}
    finally:
        for child in children.values():
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
            child.stdin.close()
            child.stdout.close()
        for log in logs.values():
            log.close()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('control', 'candidate', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--mode', choices=('sync', 'async'), required=True)
    parser.add_argument('--task', choices=('location', 'position'), required=True)
    parser.add_argument('--pairs', type=int, default=3)
    parser.add_argument('--iterations', type=int, default=5)
    parser.add_argument('--timeout', type=int, default=120)
    args = parser.parse_args(argv)
    if min(args.pairs, args.iterations, args.timeout) < 1:
        parser.error('pairs, iterations and timeout must be positive')
    node, version = pinned_node()
    binaries = {name: getattr(args, name).resolve() for name in ('control', 'candidate')}
    identities = {name: identity(path) for name, path in binaries.items()}
    fixture = ROOT / 'upstream/tsc/testdata/fixtures/compiler'
    fixture_sha = fixture_identity(fixture)
    output = args.output.resolve()
    output.mkdir(parents=True)
    report = {'method': 'development concurrent pairs; no profiler; not acceptance', 'complete': False,
              'node': version, 'binaries': identities, 'fixture_sha256': fixture_sha,
              'host': {'os': platform.system().lower(), 'architecture': platform.machine(),
                       'cpu_capacity': os.cpu_count(), 'hostname': platform.node()},
              'environment': {key: os.environ.get(key) for key in ('GOGC', 'GOMAXPROCS')},
              'mode': args.mode, 'task': args.task, 'iterations': args.iterations, 'requested_pairs': args.pairs,
              'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'driver_sha256': hashlib.sha256((HERE / 'development.mjs').read_bytes()).hexdigest(),
              'started': datetime.now(timezone.utc).isoformat(), 'pairs': []}
    try:
        for index in range(args.pairs):
            pair = run_pair(node, binaries, args.mode, args.task, args.iterations,
                            output / f'pair-{index:02d}', index, args.timeout)
            report['pairs'].append(pair)
            write(output / 'summary.json', report)
        if fixture_identity(fixture) != fixture_sha or {name: identity(path) for name, path in binaries.items()} != identities:
            raise ValueError('fixture or executable changed during the screen')
        report.update(complete=True, median_ratio=statistics.median(pair['ratio'] for pair in report['pairs']))
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        report['finished'] = datetime.now(timezone.utc).isoformat()
        write(output / 'summary.json', report)
    print(json.dumps({'median_ratio': report['median_ratio'], 'output': str(output)}))


if __name__ == '__main__':
    main()
