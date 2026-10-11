#!/usr/bin/env python3
"""Bounded concurrent LSP candidate/control screens using the Phase 5 driver.

This is development evidence, not an acceptance capture or a new LSP producer.
Each pair runs the unchanged Phase 5 request schedule in separate Python clients
and fresh server processes against two isolated copies of the same fixture. Its
existing normalization and exact traffic/response comparator gate every reported
candidate/control ratio.
Build and profile separately. Checkpoint acceptance still uses the serial
tools/phase5/latency/capture.py runner and its twenty-pair/extension rules.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import multiprocessing
from multiprocessing.connection import wait
import os
from pathlib import Path
import platform
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.phase5.latency import capture as latency  # noqa: E402

METRICS = latency.METRICS
WORKER_TIMEOUT = 240


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')


def binary_identity(path):
    if not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError(f'not an executable file: {path}')
    return {'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


def run_worker(command, cwd, scenario, output, launch, settled):
    """One interpreter per client; transfer large results through files, not a Queue."""
    run = {'command': command, 'cwd': str(cwd), 'client_pid': os.getpid()}
    did_settle = False

    def on_settled():
        nonlocal did_settle
        if did_settle:
            return
        did_settle = True
        try:
            settled.wait(timeout=WORKER_TIMEOUT)
        except threading.BrokenBarrierError:
            # The parent aborts this barrier if a worker exits or misses its
            # deadline. The pair is invalid, but keep the surviving evidence.
            pass

    try:
        if os.name == 'posix':
            os.setsid()  # Let the parent clean up this client's server on a crash.
        output.mkdir(parents=True, exist_ok=True)
        launch.wait(timeout=10)
        run['launch_ns'] = time.monotonic_ns()
        values, transcript = latency.run_runtime(command, cwd, scenario, output, on_settled=on_settled)
        run['measurements_ns'] = values
        write_json(output / 'transcript.json', transcript)
    except BaseException as error:
        launch.abort()
        # Popen/setup can fail before run_runtime reaches its settlement hook.
        on_settled()
        run['error'] = f'{type(error).__name__}: {error}'
    finally:
        output.mkdir(parents=True, exist_ok=True)
        write_json(output / 'run.json', run)


def stop_worker(process):
    """Bound termination, including a server left behind by a crashed client."""
    if os.name == 'posix':
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    elif process.is_alive():
        process.terminate()
        process.join(timeout=1)
    if process.is_alive():
        process.kill()
    process.join(timeout=5)


def run_workers(order, commands, roots, scenario, output):
    context = multiprocessing.get_context('spawn')
    launch, settled = context.Barrier(2), context.Barrier(2)
    workers, errors, stopped = {}, {}, set()
    try:
        for name in order:
            worker = context.Process(target=run_worker,
                                     args=(commands[name], roots[name], scenario, output / name, launch, settled))
            try:
                worker.start()
            except Exception as error:
                errors[name] = f'client worker launch failed: {error}'
                return errors
            workers[name] = worker
        pending = set(workers)
        deadline = time.monotonic() + WORKER_TIMEOUT
        while pending:
            ready = wait([workers[name].sentinel for name in pending], timeout=max(0, deadline - time.monotonic()))
            if not ready:
                errors.update({name: 'client worker deadline expired' for name in pending})
                break
            for name in list(pending):
                worker = workers[name]
                if worker.sentinel in ready:
                    worker.join(timeout=1)
                    pending.remove(name)
                    if worker.exitcode != 0:
                        errors[name] = f'client worker exited with status {worker.exitcode}'
                    elif not (output / name / 'run.json').is_file():
                        errors[name] = 'client worker exited without a result'
                    if name in errors:
                        stop_worker(worker)
                        stopped.add(name)
                    # Also release a peer when a worker exits without a result.
                    # On success both already passed the settlement barrier.
                    launch.abort()
                    settled.abort()
    finally:
        launch.abort()
        settled.abort()
        for name, worker in workers.items():
            if name not in stopped and (worker.is_alive() or name in errors):
                stop_worker(worker)
            worker.close()
    return errors


def run_pair(fixture, scenario, commands, output, index):
    output.mkdir(parents=True)
    order = ['control', 'candidate'] if index % 2 == 0 else ['candidate', 'control']
    pair = {'index': index, 'submission_order': order, 'matched': False,
            'measurements_ns': {}, 'runs': {}, 'errors': []}
    try:
        with tempfile.TemporaryDirectory(prefix='tsr-lsp-pair-') as directory:
            roots = {name: Path(directory).resolve() / name / 'project' for name in commands}
            for cwd in roots.values():
                shutil.copytree(fixture, cwd, symlinks=True)
            worker_errors = run_workers(order, commands, roots, scenario, output)
            # Both clients have joined before reading large transcripts/comparing.
            transcripts = {}
            for name in order:
                artifact = output / name
                artifact.mkdir(exist_ok=True)
                run = {'command': commands[name], 'cwd': str(roots[name])}
                try:
                    if (artifact / 'run.json').is_file():
                        run = latency.replay.strict_json((artifact / 'run.json').read_text())
                    if name in worker_errors:
                        raise ValueError(worker_errors[name])
                    if 'error' in run:
                        raise ValueError(run['error'])
                    values = run['measurements_ns']
                    transcript = latency.replay.strict_json((artifact / 'transcript.json').read_text())
                    if set(values) != set(METRICS) or any(type(values[key]) is not int or values[key] <= 0 for key in METRICS):
                        raise ValueError('expected all five positive integer latency measurements')
                    pair['measurements_ns'][name] = values
                    transcripts[name] = transcript
                except Exception as error:
                    run['error'] = str(error)
                    pair['errors'].append(f'{name}: {error}')
                pair['runs'][name] = run
                write_json(artifact / 'run.json', run)
            if not pair['errors']:
                pair['matched'] = latency.replay.compare(transcripts['control'], transcripts['candidate'], output / 'mismatch')
                if not pair['matched']:
                    pair['errors'].append('candidate/control traffic or responses differ')
                else:
                    pair['ratios'] = {metric: pair['measurements_ns']['candidate'][metric] / pair['measurements_ns']['control'][metric]
                                      for metric in METRICS}
                    pair['launch_skew_ms'] = abs(pair['runs']['candidate']['launch_ns'] - pair['runs']['control']['launch_ns']) / 1e6
    except Exception as error:
        pair['errors'].append(str(error))
    finally:
        write_json(output / 'pair.json', pair)
    return pair


def screen(fixture, scenario, control, candidate, output, *, pairs=10):
    if type(pairs) is not int or pairs < 1:
        raise ValueError('pair count must be a positive integer')
    fixture, output = Path(fixture).resolve(), Path(output).resolve()
    if not fixture.is_dir():
        raise ValueError(f'fixture is not a directory: {fixture}')
    if output.is_relative_to(fixture):
        raise ValueError('output must be outside the fixture')
    latency.validate_scenario(scenario, fixture)
    binaries = {'control': Path(control).resolve(), 'candidate': Path(candidate).resolve()}
    identities = {name: binary_identity(path) for name, path in binaries.items()}
    fixture_sha256 = latency.identity(fixture)
    commands = {name: [str(path), '--lsp', '--stdio'] for name, path in binaries.items()}
    report = {
        'format': 1, 'method': 'development concurrent pairs; independent spawned Python clients; alternating submission order; no profiler',
        'acceptance': False, 'producer': 'tools/phase5/latency/capture.py:run_runtime',
        'comparison_rules': [latency.COMPARISON_RULE],
        'binaries': identities, 'fixture': str(fixture), 'fixture_sha256': fixture_sha256,
        'scenario': scenario, 'scenario_sha256': hashlib.sha256(json.dumps(scenario, sort_keys=True).encode()).hexdigest(),
        'first_diagnostics_mode': latency.diagnostic_mode(scenario),
        'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'host': {'os': platform.system().lower(), 'architecture': platform.machine(),
                 'cpu_capacity': os.cpu_count(), 'hostname': platform.node()},
        'environment': {key: os.environ.get(key) for key in ('GOGC', 'GOMAXPROCS')},
        'started': datetime.now(timezone.utc).isoformat(), 'requested_pairs': pairs,
        'complete': False, 'pairs': [],
    }
    output.mkdir(parents=True)  # Never overwrite an earlier screen.
    summary = output / 'summary.json'
    write_json(summary, report)
    try:
        for index in range(pairs):
            pair = run_pair(fixture, scenario, commands, output / f'pair-{index:02d}', index)
            report['pairs'].append(pair)
            write_json(summary, report)
            if not pair['matched']:
                raise ValueError(f'pair {index} invalid: ' + '; '.join(pair['errors']))
            print(json.dumps({'pair': index, 'ratios': pair['ratios']}), flush=True)
        if latency.identity(fixture) != fixture_sha256:
            raise ValueError('fixture changed during the screen')
        if {name: binary_identity(path) for name, path in binaries.items()} != identities:
            raise ValueError('executable changed during the screen')
        report['median_ratios'] = {metric: statistics.median(pair['ratios'][metric] for pair in report['pairs'])
                                   for metric in METRICS}
        report['complete'] = True
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        report['finished'] = datetime.now(timezone.utc).isoformat()
        write_json(summary, report)
    print(report['median_ratios'], flush=True)
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--control', type=Path, required=True, help='saved control executable')
    parser.add_argument('--candidate', type=Path, required=True, help='saved candidate executable')
    parser.add_argument('--fixture', type=Path, required=True, help='prepared Phase 5 fixture')
    parser.add_argument('--scenario', type=Path, required=True, help='Phase 5 latency scenario JSON')
    parser.add_argument('--pairs', type=int, default=10)
    parser.add_argument('--output', type=Path, required=True, help='new result directory')
    args = parser.parse_args(argv)
    try:
        screen(args.fixture, latency.replay.strict_json(args.scenario.read_text()), args.control,
               args.candidate, args.output, pairs=args.pairs)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    main()
