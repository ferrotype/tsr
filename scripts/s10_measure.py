#!/usr/bin/env python3
"""Authenticated S10 parser/socket measurements and portable-host checks.

No implicit builds. Seven samples are a screen; use 21 for a final capture if
uncertainty spans the gate. Every sample is retained, including warmups.
"""
import argparse
import json
import os
import platform
from datetime import datetime, timezone
from pathlib import Path
import shutil
import subprocess
import tomllib

import s08_p4 as p4
from s07_benchmark_stats import ratio_summary
from s10_corpus import ROOT, file_digest, node_version, node_command, node_flags, sources
from s10_inputs import prepare


def threshold(experiment, criterion):
    ledger = tomllib.loads((ROOT / 'status/experiments.toml').read_text())
    return next(c['threshold'] for c in ledger[experiment]['criteria'] if c['id'] == criterion)


def build_files(kind, before):
    paths = set()
    modes = ['parser'] if kind == 'parser' else ['checker'] if kind == 'portable' else []
    for mode in modes:
        name = f'target/s10/{mode}/build.json'
        build = p4.read(ROOT / name)
        if build['sources'] != before or build['mode'] != mode:
            raise ValueError('stale wasm build: ' + mode)
        paths.add(name)
        for path, item in build['artifacts'].items():
            if file_digest(ROOT / path) != item['sha256']:
                raise ValueError('changed wasm artifact: ' + path)
            paths.add(path)
    if kind != 'portable':
        name = 'target/s10/go/build.json'
        build = p4.read(ROOT / name)
        if build['sources'] != before:
            raise ValueError('stale Go build')
        paths.add(name)
        for path, item in build['artifacts'].items():
            name = 'target/s10/go/' + path
            if file_digest(ROOT / name) != item['sha256']:
                raise ValueError('changed Go artifact: ' + name)
            paths.add(name)
    if kind == 'node':
        build = p4.read(ROOT / 'target/s10/node/build.json')
        if build['sources'] != before or file_digest(ROOT / 'target/s10/node/ts_node.node') != build['binary_sha256']:
            raise ValueError('stale or changed Node adapter')
        paths |= {'target/s10/node/build.json', 'target/s10/node/ts_node.node'}
    if kind in ('portable', 'node'):
        paths.add('target/release/examples/parser')
    for pattern in ('tools/s10/wasm/*.mjs', 'tools/s10/node/*.mjs', 'tools/s10/parser/*'):
        paths.update(str(p.relative_to(ROOT)) for p in ROOT.glob(pattern) if p.is_file())
    return sorted(paths)


def capture(args):
    node = node_version()
    before = sources()
    paths = build_files(args.kind, before)
    # Inventory/hash validation precedes all timing and output-directory creation.
    inputs = None
    if args.kind == 'parser':
        inputs = ROOT / 'target/s10/parser-inputs.json'
        prepare(inputs, args.limit)
    directory = args.output
    directory.mkdir(parents=True, exist_ok=False)
    bundle = directory / 'artifacts'
    for name in paths:
        target = bundle / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, target)
    if inputs:
        shutil.copy2(inputs, directory / 'inputs.json')
        command = node_command('tools/s10/wasm/parser-bench.mjs', str((directory / 'inputs.json').resolve()),
                               'target/s10/parser', str(args.samples))
    elif args.kind == 'node':
        command = node_command('tools/s10/node/bench.mjs', str(args.samples), str(args.iterations))
    else:
        command = node_command('tools/s10/wasm/test.mjs', 'target/s10/checker')
    record = {'version': 1, 'kind': args.kind, 'partial': bool(args.limit), 'sources': before,
              'node': node, 'node_flags': node_flags(), 'command': command, 'samples': args.samples, 'iterations': args.iterations,
              'host': {'platform': platform.platform(), 'processor': platform.processor(), 'cpus': os.cpu_count()},
              'started_at': datetime.now(timezone.utc).isoformat(),
              'inputs_sha256': file_digest(directory / 'inputs.json') if inputs else None,
              'artifacts': {name: file_digest(bundle / name) for name in paths}}
    p4.write_new(directory / 'capture.json', record)
    if args.kind == 'node':
        with (directory / 'adapter-tests.json').open('wb') as log:
            subprocess.run(node_command('tools/s10/node/test.mjs'), cwd=bundle, stdout=log, check=True)
    with (directory / 'raw.json').open('wb') as output, (directory / 'stderr').open('wb') as stderr:
        process = subprocess.run(command, cwd=bundle, stdout=output, stderr=stderr)
    p4.write_new(directory / 'completed.json', {'exit_code': process.returncode,
                 'source_stable': sources() == before,
                 'adapter_tests_sha256': file_digest(directory / 'adapter-tests.json') if args.kind == 'node' else None,
                 'finished_at': datetime.now(timezone.utc).isoformat(), 'raw_sha256': file_digest(directory / 'raw.json')})
    return verify(directory)


def summarize(raw, kind, samples):
    rows = raw['samples']
    if len(rows) != samples or samples not in (7, 14, 21):
        raise ValueError('requires a complete 7, 14 or 21 sample inventory')
    for index, row in enumerate(rows):
        expected = ['rust', 'go'] if (index % 2 == 0) == (kind == 'parser') else ['go', 'rust']
        if row['index'] != index or row['order'] != expected or set(row['elapsed_ns']) != {'rust', 'go'}:
            raise ValueError('timing order/inventory mismatch')
    limit = 1 / threshold('E7', 'parse_throughput') if kind == 'parser' else threshold('E8', 'node_parse_latency')
    return ratio_summary([r['elapsed_ns']['go'] for r in rows], [r['elapsed_ns']['rust'] for r in rows],
                         timing=True, threshold=limit)


def verify(directory):
    record = p4.read(directory / 'capture.json')
    completed = p4.read(directory / 'completed.json')
    if record['sources'] != sources() or not completed['source_stable']:
        raise ValueError('stale or unstable S10 measurement sources')
    if record['node_flags'] != node_flags():
        raise ValueError('different engine stack configuration')
    if record['node'] != 'v' + p4.read(ROOT / 'upstream/package.json')['volta']['node']:
        raise ValueError('wrong Node version')
    for name, digest in record['artifacts'].items():
        if Path(name).is_absolute() or '..' in Path(name).parts or file_digest(directory / 'artifacts' / name) != digest:
            raise ValueError('changed or unsafe measurement artifact: ' + name)
    if completed['exit_code'] != 0 or file_digest(directory / 'raw.json') != completed['raw_sha256']:
        raise ValueError('failed or changed raw measurement; inspect stderr')
    raw = p4.read(directory / 'raw.json')
    kind = record['kind']
    if kind != 'portable' and raw['node'] != record['node']:
        raise ValueError('measurement Node version disagrees')
    metrics = {}
    stats = None
    if kind == 'portable':
        # JS driver authenticates exact import names and exercises the shipped
        # checker feature, disposal, independent instances and terminal traps.
        fixtures = p4.read(ROOT / 'tools/s10/parser/fixtures.json')
        if raw['mode'] != 'checker' or raw['fixtures'] != len(fixtures) or raw['ownership'] != 'passed':
            raise ValueError('incomplete portable-host observations')
        metrics['portable_host'] = True
    else:
        stats = summarize(raw, kind, record['samples'])
        if kind == 'parser':
            if file_digest(directory / 'inputs.json') != record['inputs_sha256']:
                raise ValueError('changed parser inventory')
            inputs = p4.read(directory / 'inputs.json')
            if bool(inputs['partial']) != record['partial'] or (not inputs['partial'] and len(inputs['files']) != 13094):
                raise ValueError('partial or incomplete parser inventory')
            for key, path in [('manifest_sha256', 'data/s07/vscode-files.json'),
                              ('options_sha256', 'data/s07/vscode-parse-options.json')]:
                if inputs[key] != file_digest(ROOT / path):
                    raise ValueError('stale parser workload')
            if raw['files'] != len(inputs['files']) or raw['source_bytes'] != sum(r['bytes'] for r in inputs['files']):
                raise ValueError('parser workload counts disagree')
            observed = raw['observations']
            if len(observed) != len(inputs['files']) or any(o['filename'] != r['filename'] for o, r in zip(observed, inputs['files'])):
                raise ValueError('parser parity inventory mismatch')
            rust = directory / 'artifacts/target/s10/parser/parser_bg.wasm'
            go = directory / 'artifacts/target/s10/go/parser.wasm'
            metrics['parser_artifact_size_ratio'] = rust.stat().st_size / go.stat().st_size
            metric, value = 'parse_throughput_ratio', 1 / stats['ratio']
        elif kind == 'node':
            tests = directory / 'adapter-tests.json'
            if file_digest(tests) != completed['adapter_tests_sha256']:
                raise ValueError('changed Node lifetime observations')
            test_report = p4.read(tests)
            if test_report != {'fixtures': 10, 'lifecycles': 4, 'retained_outputs': 40}:
                raise ValueError('incomplete Node lifetime observations')
            if (raw['bytes_per_file'] != 10240 or not raw['fresh_source_each_iteration']
                    or raw['iterations'] != record['iterations']
                    or raw['socket_sequence'] != ['updateTemporarySnapshot', 'getSourceFile', 'release']):
                raise ValueError('socket interval/workload differs')
            hashes = [h for row in [raw['warmup'], *raw['samples']] for h in row['sources']]
            if len(set(hashes)) != len(hashes):
                raise ValueError('socket capture reused source text')
            for row in [raw['warmup'], *raw['samples']]:
                if len(row['sources']) != raw['iterations'] or len(row['outputs']) != raw['iterations']:
                    raise ValueError('socket output comparison count differs')
            metric, value = 'node_parse_latency_ratio', stats['ratio']
        else:
            raise ValueError('unknown measurement kind')
        # A failing measurement remains a measured failure. A nominal pass
        # without the required confidence/stability withholds the metric.
        if stats['stable'] or stats['ratio'] > stats['bootstrap']['threshold']:
            metrics[metric] = value
    if record['partial']:
        metrics = {}
    report = {'metrics': metrics, 'statistics': stats, 'partial': record['partial'],
              'capture_sha256': file_digest(directory / 'capture.json')}
    p4.atomic(directory / 'verified.json', report)
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['capture', 'verify'])
    parser.add_argument('--kind', choices=['parser', 'node', 'portable'])
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--samples', type=int, choices=[7, 14, 21], default=7)
    parser.add_argument('--iterations', type=int, default=100)
    parser.add_argument('--limit', type=int, default=0)
    args = parser.parse_args()
    if args.limit < 0 or args.iterations < 1 or (args.action == 'capture' and not args.kind):
        parser.error('capture requires a kind, nonnegative limit and positive iterations')
    if args.limit and args.kind != 'parser':
        parser.error('only parser screens accept --limit')
    print(json.dumps(capture(args) if args.action == 'capture' else verify(args.output), indent=2))
