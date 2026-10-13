#!/usr/bin/env python3
"""Separate wasm startup diagnostics; no project work or budget acceptance.

load_processor_ns includes artifact reads/hash checks, bindings, wasm module
compilation/instantiation and runtime initialization performed by loadProcessor.
It excludes the frontend's ES-module import. launch_to_ready_ns includes Node
startup, frontend import, the loader and ready-response transport. Neither is
a pure module compilation measurement or a compiler operation measurement.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import math
import os
from pathlib import Path
import selectors
import signal
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.phase7.budget import measure, probe  # noqa: E402

PAIRS = 7
MAX_STDOUT = 64 * 1024
CHILD = Path(__file__).with_name('startup-child.mjs')
METRICS = ('load_processor_ns', 'launch_to_ready_ns')


def parse_ready(stdout, engine):
    if not stdout.endswith(b'\n') or stdout.count(b'\n') != 1 or len(stdout) > MAX_STDOUT:
        raise ValueError('startup requires exactly one bounded ready response')
    row = probe.decode(stdout)
    probe.fields(row, 'version kind engine load_processor_ns', 'startup ready')
    if type(row['version']) is not int or row['version'] != 1 or row['kind'] != 'startup_ready' or row['engine'] != engine:
        raise ValueError('startup response version/kind/engine differs')
    probe.decimal(row['load_processor_ns'], 'loader duration', positive=True)
    return int(row['load_processor_ns'])


def capture(identity, directory, timeout, *, child=CHILD):
    """One fresh process; retain logs even on failure or timeout."""
    directory.mkdir()
    engine = identity['engine']
    artifact_key = 'TSR_GO_WASM_ARTIFACT_DIR' if engine == 'go' else 'TSR_WASM_ARTIFACT_DIR'
    command = [identity['node']['path'], *identity['node_arguments'], str(child), engine,
               identity['environment'][artifact_key]]
    environment = dict(os.environ)
    for key, value in identity['environment'].items():
        if value is None:
            environment.pop(key, None)
        else:
            environment[key] = value
    row = {'command': command, 'complete': False, 'exit': None}
    process = None
    stdout = bytearray()
    ready_elapsed = None
    try:
        with (directory / 'stdout').open('wb') as output, (directory / 'stderr').open('wb') as error:
            begin = time.perf_counter_ns()
            process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                       stderr=error, env=environment, start_new_session=True)
            deadline = time.monotonic() + timeout
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while selector.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError('startup child exceeded its timeout')
                    for key, _ in selector.select(remaining):
                        chunk = os.read(key.fd, 4096)
                        received = time.perf_counter_ns()
                        if not chunk:
                            selector.unregister(key.fileobj)
                            continue
                        stdout.extend(chunk)
                        if ready_elapsed is None and b'\n' in stdout:
                            ready_elapsed = received - begin
                        output.write(chunk)
                        output.flush()
                        if len(stdout) > MAX_STDOUT:
                            raise ValueError('startup stdout exceeds its bound')
            row['exit'] = process.wait(timeout=max(0.001, deadline - time.monotonic()))
        if row['exit'] != 0:
            raise ValueError('startup child exited unsuccessfully')
        loader = parse_ready(bytes(stdout), engine)
        if ready_elapsed is None or not 0 < loader <= ready_elapsed:
            raise ValueError('startup loader interval exceeds launch-to-ready interval')
        row.update(complete=True, load_processor_ns=loader, launch_to_ready_ns=ready_elapsed)
    except Exception as error:
        row['failure'] = str(error)
    finally:
        if process is not None:
            try:
                # A child may exit while a descendant still holds stdout open.
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except OSError as error:
                row['cleanup_error'] = str(error)
                row.update(complete=False, failure=row.get('failure', 'startup child cleanup failed'))
                try:
                    if process.poll() is None:
                        process.kill()
                except OSError as direct_error:
                    row['cleanup_error'] += '; direct child: ' + str(direct_error)
            try:
                row['exit'] = process.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired) as error:
                row['cleanup_error'] = row.get('cleanup_error', '') + '; wait: ' + str(error)
                row.update(complete=False, failure=row.get('failure', 'startup child cleanup failed'))
            finally:
                process.stdout.close()
        row['raw_files'] = [dict(probe.identity(directory / name), file=name) for name in ('stdout', 'stderr')]
        probe.write(directory / 'record.json', row)
    return row


def summaries(pairs):
    result = {}
    for metric in METRICS:
        samples = {engine: [pair[engine][metric] for pair in pairs] for engine in ('go', 'rust')}
        medians = {engine: statistics.median(values) for engine, values in samples.items()}
        result[metric] = {'samples': samples, 'median_ns': medians,
                          'rust_over_go': medians['rust'] / medians['go'],
                          'relative_mad': {engine: statistics.median(abs(value - medians[engine]) for value in values) / medians[engine]
                                           for engine, values in samples.items()}}
    return result


def run(identities, output, timeout):
    if type(timeout) not in (int, float) or not math.isfinite(timeout) or timeout <= 0:
        raise ValueError('startup requires a finite positive timeout')
    if output.exists() or any(part.casefold() == 'target' for part in output.resolve().parts):
        raise ValueError('startup requires a new directory outside target')
    if identities['go']['node'] != identities['rust']['node'] or identities['go']['node_arguments'] != identities['rust']['node_arguments']:
        raise ValueError('startup pairs require the same pinned Node executable and flags')
    for identity in identities.values():
        artifact_root = Path(identity['manifest']['path']).resolve().parent
        if any(part.casefold() == 'target' for part in artifact_root.parts):
            raise ValueError('startup requires retained artifacts outside target')
        if output.resolve().is_relative_to(artifact_root):
            raise ValueError('startup output must be outside the immutable artifact directory')
    output.mkdir(parents=True)
    controllers = [probe.identity(path) for path in (Path(__file__), CHILD, Path(measure.__file__), Path(probe.__file__))]
    helpers = measure.archive_helpers(output, controllers, identities)
    for engine, identity in identities.items():
        (output / (engine + '-build.json')).write_bytes(Path(identity['manifest']['path']).read_bytes())
    report = {'schema': 1, 'kind': 'wasm-startup-diagnostic', 'complete': False,
              'performance_eligible': False, 'budget_threshold': None,
              'started_utc': datetime.now(timezone.utc).isoformat(),
              'scope': __doc__, 'artifacts': identities, 'controllers': controllers,
              'helper_manifest': 'helper-manifest.json', 'warmup': {}, 'pairs': [], 'failures': []}

    def save():
        probe.write(output / 'startup.json', report)

    def guard():
        for identity in identities.values():
            measure.unchanged(identity)
        for item in controllers:
            if probe.identity(item['path']) != item:
                raise ValueError('startup controller changed during capture')
        for item in helpers:
            actual = probe.identity(output / item['archived'])
            if any(actual[key] != item[key] for key in ('bytes', 'sha256')):
                raise ValueError('archived startup helper changed')

    def sample(engine, label):
        guard()
        row = capture(identities[engine], output / label, timeout)
        guard()
        if not row['complete']:
            raise ValueError(label + ': ' + row['failure'])
        return dict(row, record=label + '/record.json')

    save()
    try:
        for engine in ('go', 'rust'):
            report['warmup'][engine] = sample(engine, 'warmup-' + engine)
            save()
        for index in range(PAIRS):
            order = ['go', 'rust'] if index % 2 == 0 else ['rust', 'go']
            pair = {'index': index, 'order': order}
            report['pairs'].append(pair)
            for engine in order:
                pair[engine] = sample(engine, f'pair-{index:02d}-{engine}')
                save()
        report.update(complete=True, summary=summaries(report['pairs']),
                      finished_utc=datetime.now(timezone.utc).isoformat())
    except Exception as error:
        report['failures'].append(str(error))
    finally:
        save()
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--go-artifacts', type=Path, required=True)
    parser.add_argument('--rust-artifacts', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=180)
    args = parser.parse_args()
    try:
        identities = {engine: measure.build_identity('wasm', engine, directory, directory / 'build.json')
                      for engine, directory in [('go', args.go_artifacts), ('rust', args.rust_artifacts)]}
        report = run(identities, args.output.resolve(), args.timeout)
        print(args.output.resolve() / 'startup.json')
        return 0 if report['complete'] else 2
    except Exception as error:
        print(error, file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
