#!/usr/bin/env python3
"""The pin's JS API benchmarks against the Go and Rust servers
(docs/PHASE7-plan.md, R0 item 4).

    capture.py run --output DIR [--rounds 3]

The benchmarks are packages/typescript's test/sync/api.bench.ts and
test/async/api.bench.ts, run unchanged by capture.mjs, which records tinybench's
per-task results. The pinned client spawns its server through `getExePath()`,
`upstream/built/local/tsc` in a repository checkout (the jsapi suite's
placement, tools/phase6/jsapi/runner.py); each run points it at tsgo or tsrust
(tools/phase7/bench/capture.py build). Rounds alternate the runtimes.

tinybench keeps no raw samples unless asked, and the pinned files do not ask:
each run gives each task its median latency (`p50`) over its iterations, and a
task's ratio is the median of Rust's round medians over Go's.

Tasks named `TS - ...` run the legacy JS compiler and `materialize ...` tasks
decode an already transferred buffer in the client; both are client controls,
recorded without a ratio, as is test/nodelist.bench.ts, which starts no server.
The other tasks are server-dependent: spawn, project load, transfer and queries,
each its own ratio. Both runtimes must run the same tasks without an error,
and the task names embed the identifier count each server's transferred AST
yields, so a server that transfers a different tree runs differently named
tasks and the capture fails.

The memory pass runs each file once more per runtime with its own
singleIteration option, with `getExePath()` pointing at server_rss.py, which
records every spawned server's own peak RSS; a mode's peak is the largest.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import shutil
from statistics import median
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / 'scripts'))
from tools.phase6.jsapi import runner as jsapi  # noqa: E402
from tools.phase7.bench import common  # noqa: E402
from s07_benchmark_measure import host_info, reject_concurrent_builds  # noqa: E402

HERE = Path(__file__).resolve().parent
PACKAGE = ROOT / 'upstream/packages/typescript'
FILES = {'sync': 'test/sync/api.bench.ts', 'async': 'test/async/api.bench.ts'}
CONTROL_FILE = 'test/nodelist.bench.ts'
CONTROL_PREFIXES = ('TS - ', 'materialize ')
FORMAT = 1
ROUNDS = 3
TIMEOUT_SECONDS = 1800


def key(name):
    """A task's ratio name without its identifier count:
    `getSymbolAtPosition - 2894 identifiers (batched)` -> `getsymbolatposition_identifiers_batched`."""
    return re.sub(r'[^a-z0-9]+', '_', re.sub(r'\b\d+ ', '', name).lower()).strip('_')


def is_control(name):
    return name.startswith(CONTROL_PREFIXES)


def node_command(node, bench, out, single=False):
    return [node, '--conditions', '@typescript/source', '--experimental-import-meta-resolve',
            str(HERE / 'capture.mjs'), str(PACKAGE / bench), str(out), *(['single'] if single else [])]


def run_file(node, bench, out, env=None, single=False):
    result = subprocess.run(node_command(node, bench, out, single), cwd=PACKAGE, env=env or os.environ,
                            capture_output=True, text=True, timeout=TIMEOUT_SECONDS)
    if result.returncode != 0 or not Path(out).is_file():
        raise ValueError(f'{bench} exited {result.returncode}: {result.stderr[-2000:]}')
    benches = json.loads(Path(out).read_text())['benches']
    if len(benches) != 1:
        raise ValueError(f'{bench}: expected one bench, captured {len(benches)}')
    tasks = benches[0]['tasks']
    failed = [task['name'] for task in tasks
              if not task['result'] or task['result'].get('state', 'completed') != 'completed'
              or task['result'].get('error')]
    if failed:
        raise ValueError(f'{bench}: tasks did not complete: {failed}')
    return tasks


def latency(task):
    """A run's median latency of a task (ms) and its iteration count."""
    statistics = task['result'].get('latency') or {}
    middle, count = statistics.get('p50'), statistics.get('samplesCount')
    if type(middle) not in (int, float) or not middle > 0 or type(count) is not int or count < 1:
        raise ValueError(f"{task['name']}: no latency median")
    return middle, count


def prerequisites():
    node, version = jsapi.pinned_node()
    if not (ROOT / 'upstream/node_modules/tinybench').is_dir():
        raise ValueError('run `npm ci --ignore-scripts` in upstream/ first: the benchmarks import tinybench')
    return node, version


def capture(output, rounds=ROUNDS, log=sys.stdout):
    if output.exists() and any(output.iterdir()):
        raise ValueError(f'{output} is not empty; a capture is never overwritten')
    output.mkdir(parents=True, exist_ok=True)
    node, version = prerequisites()
    binaries = common.binaries()
    host = host_info(minimum_cpus=1)
    reject_concurrent_builds()
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(['git', 'status', '--porcelain', '--', 'crates', 'Cargo.toml', 'Cargo.lock'],
                                         cwd=ROOT, text=True).strip())
    report = {'format': FORMAT, 'revision': head, 'dirty': dirty,
              'pin': json.loads((ROOT / 'data/upstream.json').read_text())['pin'], 'node': version,
              'started': datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'), 'host': host,
              'load_before': [round(value, 2) for value in os.getloadavg()],
              'binaries': {runtime: common.identity(path) for runtime, path in binaries.items()},
              'runs': [], 'memory': {}, 'controls': {}, 'complete': False}
    path = output / 'capture.json'
    try:
        for mode, bench in FILES.items():
            for index in range(rounds):
                for runtime in (('go', 'rust') if index % 2 == 0 else ('rust', 'go')):
                    jsapi._place_binary(binaries[runtime])
                    tasks = run_file(node, bench, output / f'{mode}-{index}-{runtime}.json')
                    report['runs'].append({'mode': mode, 'round': index, 'runtime': runtime, 'tasks': tasks})
                    print(f'api {mode} round {index} {runtime}: {len(tasks)} tasks', file=log, flush=True)
                    path.write_text(json.dumps(report) + '\n')
        server = HERE / 'server_rss.py'
        for mode, bench in FILES.items():
            report['memory'][mode] = {}
            for runtime in ('go', 'rust'):
                with tempfile.NamedTemporaryFile('r', suffix='.rss') as rss:
                    env = {**os.environ, 'PHASE7_API_SERVER': str(binaries[runtime]), 'PHASE7_API_RSS_LOG': rss.name}
                    jsapi._place_binary(server)
                    run_file(node, bench, output / f'{mode}-memory-{runtime}.json', env, single=True)
                    values = [int(line) for line in Path(rss.name).read_text().split()]
                if not values:
                    raise ValueError(f'{mode} {runtime}: the memory pass spawned no server')
                report['memory'][mode][runtime] = {'peak_rss_bytes': max(values), 'server_peaks': values}
                print(f'api {mode} memory {runtime}: {max(values) / 2**20:.0f} MB over {len(values)} servers',
                      file=log, flush=True)
        report['controls']['nodelist'] = run_file(node, CONTROL_FILE, output / 'nodelist.json')
        validate(report)
        report['complete'] = True
    finally:
        report['finished'] = datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
        report['load_after'] = [round(value, 2) for value in os.getloadavg()]
        path.write_text(json.dumps(report) + '\n')
    return report


def validate(report):
    """Both runtimes ran the same tasks in every round of a mode."""
    for mode in FILES:
        names = {(run['runtime'], run['round']): [task['name'] for task in run['tasks']]
                 for run in report['runs'] if run['mode'] == mode}
        if len(set(map(tuple, names.values()))) != 1:
            raise ValueError(f'{mode}: the runtimes ran different tasks (a server transferred a different tree?): '
                             + json.dumps({f'{runtime} {index}': value for (runtime, index), value in names.items()}))


def read_capture(path):
    """The `api` workload's measurement: elapsed_<mode>_<task> per
    server-dependent task and peak_rss_<mode>, Rust over Go."""
    path = Path(path)
    report = json.loads((path / 'capture.json' if path.is_dir() else path).read_text())
    if report.get('format') != FORMAT or report.get('complete') is not True:
        raise ValueError('a run needs a complete capture')
    if report.get('dirty'):
        raise ValueError('the capture measured a tsrust built from uncommitted sources')
    validate(report)
    ratios, samples, controls, pooled_counts = {}, {'go': {}, 'rust': {}}, {}, {}
    for mode in FILES:
        runs = [run for run in report['runs'] if run['mode'] == mode]
        for name in [task['name'] for task in runs[0]['tasks']]:
            rounds, counts = {'go': [], 'rust': []}, {'go': 0, 'rust': 0}
            for run in runs:
                task = next(task for task in run['tasks'] if task['name'] == name)
                middle, count = latency(task)
                rounds[run['runtime']].append(middle)
                counts[run['runtime']] += count
            if is_control(name):
                controls[f'{mode}: {name}'] = {runtime: median(values) for runtime, values in rounds.items()}
                continue
            ratio = f'elapsed_{mode}_{key(name)}'
            if ratio in ratios:
                raise ValueError(f'{mode}: two tasks map to {ratio}')
            ratios[ratio] = median(rounds['rust']) / median(rounds['go'])
            pooled_counts[ratio] = counts
            for runtime in ('go', 'rust'):
                samples[runtime][ratio] = rounds[runtime]
        memory = report['memory'][mode]
        ratios[f'peak_rss_{mode}'] = memory['rust']['peak_rss_bytes'] / memory['go']['peak_rss_bytes']
        for runtime in ('go', 'rust'):
            samples[runtime][f'peak_rss_{mode}'] = memory[runtime]['server_peaks']
    controls['nodelist'] = {task['name']: latency(task)[0] for task in report['controls']['nodelist']}
    metadata = {'node': report['node'], 'pin': report['pin'], 'binaries': report['binaries'],
                'controls_median_ms': controls, 'load_average': {'before': report['load_before'], 'after': report['load_after']},
                'rounds': max(run['round'] for run in report['runs']) + 1, 'iterations': pooled_counts,
                'elapsed': "each round's tinybench median latency of the task in ms (samples); the ratio "
                           "compares the medians of the rounds",
                'peak_rss': "the memory pass's servers, each its own peak RSS; the ratio compares the largest"}
    return {'ratios': ratios, 'samples': samples, 'metadata': metadata, 'host': report['host'],
            'revision': report['revision'], 'recorded_at': report['finished']}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('command', choices=('run',))
    parser.add_argument('--output', type=Path, required=True, help='an empty capture directory')
    parser.add_argument('--rounds', type=int, default=ROUNDS)
    args = parser.parse_args()
    try:
        report = capture(args.output, args.rounds)
        print(f"api capture {'complete' if report['complete'] else 'INCOMPLETE'}: {args.output / 'capture.json'}")
    except (OSError, ValueError, subprocess.SubprocessError, RuntimeError) as error:
        sys.exit(f'capture.py {args.command}: {error}')


if __name__ == '__main__':
    main()
