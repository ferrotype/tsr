#!/usr/bin/env python3
"""Capture/replay the frozen E2 schedule through external Rust or bare wasm.

Never rebuild implicitly. Validate all inputs before launching a child. Preserve
raw requests/results and native authority; smoke runs never emit gate metrics.
"""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import queue
import shutil
import subprocess
import threading
import tomllib

import s08_e2 as e2
import s08_e2_contract as contract
import s08_p4 as p4
import s08_p5_corpus as corpus
from s08_oracle import ROOT


def file_digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def sources():
    values = e2.sources()
    for pattern in ('tools/s10/**/*', 'scripts/s10*.py', 'status/experiments.toml',
                    'rust-toolchain.toml', 'status/runs.toml', 'data/divergences.toml', 'data/s07/vscode*.json',
                    'data/workloads.toml', 'data/s04/toolchains.toml', 'scripts/s07_benchmark_stats.py',
                    'scripts/s04_ownership.py', 'scripts/s04_runtime.py', 'scripts/s06_ownership.py'):
        for path in ROOT.glob(pattern):
            if path.is_file() and '__pycache__' not in path.parts:
                values[str(path.relative_to(ROOT))] = file_digest(path)
    return values


def node_flags():
    return p4.read(ROOT / 'tools/s10/toolchains.json')['node_flags']


def node_command(*args):
    return ['node', *node_flags(), *args]


def node_version():
    actual = subprocess.check_output(['node', '--version'], text=True).strip()
    expected = 'v' + p4.read(ROOT / 'upstream/package.json')['volta']['node']
    if actual != expected:
        raise ValueError(f'S10 requires pinned Node {expected}, found {actual}')
    return actual


def preflight(source):
    metadata = p4.read(source / 'capture.json')
    if file_digest(source / 'requests.json') != metadata['requests_sha256']:
        raise ValueError('source requests changed')
    if file_digest(source / 'native-report.json') != metadata['native_report_sha256']:
        raise ValueError('native report changed')
    native = p4.read(source / 'native-report.json')
    e2.native_current(native)
    if file_digest(source / 'native-observations.ndjson') != native['observation_sha256']:
        raise ValueError('native output digest mismatch')
    requests = p4.read(source / 'requests.json')
    contract.inventory(requests, e2.frozen())
    # Validate native request correspondence before work, not after the corpus.
    with (source / 'native-observations.ndjson').open() as stream:
        for request in requests:
            row = p4.strict_json_loads(stream.readline())
            if row['id'] != request['id']:
                raise ValueError('native inventory differs')
            corpus.native_metadata(request, row)
        if stream.read(1):
            raise ValueError('native inventory has extra rows')
    return requests


class Child:
    def __init__(self, command, stderr):
        self.process = subprocess.Popen(command, cwd=ROOT, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=stderr)
        self.lines = queue.Queue()
        def read():
            try:
                for line in self.process.stdout:
                    self.lines.put(line)
            finally:
                self.lines.put(None)
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def observe(self, request, timeout):
        self.process.stdin.write(p4.canonical(request) + b'\n')
        self.process.stdin.flush()
        line = self.lines.get(timeout=timeout)
        if line is None:
            raise ValueError('child exited before returning the observation')
        return corpus.validate_row(request, p4.strict_json_loads(line))

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process.stdin.close()
        self.reader.join(timeout=5)
        self.process.stdout.close()


def capture(args):
    node = node_version()
    requests = preflight(args.source)
    if args.smoke:
        # Spread coverage across the entire frozen inventory, preserving order.
        requests = requests[::max(1, len(requests) // args.smoke)][:args.smoke]
    before = sources()
    build_path = ROOT / ('target/s10/rust-consumer-build.json' if args.runtime == 'rust'
                         else 'target/s10/corpus/build.json')
    build = p4.read(build_path)
    if build['sources'] != before:
        raise ValueError('embedding binary was built from different sources; run s10_build first')
    if args.runtime == 'rust':
        if file_digest(ROOT / 'target/release/s10_rust_consumer') != build['binary_sha256']:
            raise ValueError('external consumer executable changed')
    else:
        for name, item in build['artifacts'].items():
            if file_digest(ROOT / name) != item['sha256']:
                raise ValueError('wasm build artifact changed: ' + name)
    args.output.mkdir(parents=True, exist_ok=False)
    artifacts = args.output / 'artifacts'
    artifacts.mkdir()
    if args.runtime == 'rust':
        shutil.copy2(ROOT / 'target/release/s10_rust_consumer', artifacts / 'consumer')
        shutil.copy2(build_path, artifacts / 'build.json')
        command = [str((artifacts / 'consumer').resolve())]
    else:
        for name in ('bindings.mjs', 'corpus_bg.wasm', 'build.json'):
            shutil.copy2(ROOT / 'target/s10/corpus' / name, artifacts / name)
        for name in ('corpus.mjs', 'instance.mjs', 'imports.mjs'):
            shutil.copy2(ROOT / 'tools/s10/wasm' / name, artifacts / name)
        command = node_command(str((artifacts / 'corpus.mjs').resolve()), str(artifacts.resolve()))
    p4.write_new(args.output / 'requests.json', requests)
    # Reference the preserved native file with an authenticated digest; avoid
    # making a 562 MB copy for every development sample. Replay requires it.
    native = (args.source / 'native-observations.ndjson').resolve()
    record = {'version': 1, 'runtime': args.runtime, 'partial': bool(args.smoke),
              'sources': before, 'native': str(native), 'native_sha256': file_digest(native),
              'native_report': p4.read(args.source / 'native-report.json'),
              'requests_sha256': file_digest(args.output / 'requests.json'),
              'artifacts': {p.name: file_digest(p) for p in artifacts.iterdir()},
              'node': node, 'node_flags': node_flags(),
              'command': command, 'timeout': args.timeout}
    p4.write_new(args.output / 'capture.json', record)
    child = None
    try:
        with (args.output / 'stderr').open('wb') as stderr, (args.output / 'observations.ndjson').open('wb') as output:
            for index, request in enumerate(requests):
                if child is None:
                    child = Child(command, stderr)
                try:
                    row = child.observe(request, args.timeout)
                except (queue.Empty, ValueError, OSError, KeyError, TypeError) as error:
                    kind = 'timeout' if isinstance(error, queue.Empty) else 'child_protocol'
                    row = p4.fatal(request, kind, str(error) or f'exceeded {args.timeout} seconds')
                    child.close()
                    child = None
                output.write(p4.canonical(row) + b'\n')
                output.flush()
                if index % 100 == 0 or index + 1 == len(requests):
                    print(f'{args.runtime}: {index + 1}/{len(requests)}', flush=True)
                p4.atomic(args.output / 'progress.json', {'observed': index + 1, 'total': len(requests), 'last': request['id']})
    finally:
        if child is not None:
            child.close()
    p4.write_new(args.output / 'completed.json', {
        'observations_sha256': file_digest(args.output / 'observations.ndjson'),
        'source_stable': before == sources(),
    })
    return verify(args.output)


def verify(directory):
    record = p4.read(directory / 'capture.json')
    completed = p4.read(directory / 'completed.json')
    if record['node_flags'] != node_flags():
        raise ValueError('capture used a different engine stack configuration')
    if record['node'] != 'v' + p4.read(ROOT / 'upstream/package.json')['volta']['node']:
        raise ValueError('capture used a different Node version')
    if not completed['source_stable'] or sources() != record['sources']:
        raise ValueError('S10 capture has stale or changing sources')
    for name, expected in record['artifacts'].items():
        if Path(name).name != name or file_digest(directory / 'artifacts' / name) != expected:
            raise ValueError('S10 executable/glue changed')
    if file_digest(directory / 'requests.json') != record['requests_sha256']:
        raise ValueError('S10 request inventory changed')
    if file_digest(directory / 'observations.ndjson') != completed['observations_sha256']:
        raise ValueError('S10 raw observations changed')
    native = Path(record['native'])
    if file_digest(native) != record['native_sha256']:
        raise ValueError('S10 native authority changed')
    e2.native_current(record['native_report'])
    requests = p4.read(directory / 'requests.json')
    frozen = e2.frozen()
    contract.inventory(requests, frozen, partial=record['partial'])
    ledger = tomllib.loads((ROOT / 'data/divergences.toml').read_text())
    approval_ids = {r['id'] for r in frozen}
    pin = p4.read(ROOT / 'data/upstream.json')['pin']
    counts = defaultdict(lambda: defaultdict(Counter))
    passing = Counter()
    findings = []
    with native.open() as go_stream, (directory / 'observations.ndjson').open() as rust_stream:
        native_rows = (p4.strict_json_loads(line) for line in go_stream)
        for request in requests:
            go = next((r for r in native_rows if r['id'] == request['id']), None)
            if go is None:
                raise ValueError('native variant missing')
            row = p4.strict_json_loads(rust_stream.readline())
            result = contract.grade([request], [row], [go], ledger, pin,
                                    partial=True, approval_ids=approval_ids)['rows'][0]
            tier = request['acceptance_tier']
            for metric, check in result['checks'].items():
                counts[tier][metric][check['state']] += 1
            if all(check['accepted'] for check in result['checks'].values()):
                passing[tier] += 1
            else:
                findings.append(result)
        if rust_stream.read(1):
            raise ValueError('extra Rust observations')
    totals = Counter(request['acceptance_tier'] for request in requests)
    report = {'partial': record['partial'], 'runtime': record['runtime'],
              'counts': counts, 'passing_all_domains': passing, 'totals': totals,
              'findings': findings, 'metrics': {},
              'capture_sha256': file_digest(directory / 'capture.json')}
    if not record['partial']:
        metric = 'checker_parity' if record['runtime'] == 'wasm' else 'rust_consumer_parity'
        report['metrics'][metric] = passing['acceptance'] / totals['acceptance']
    p4.atomic(directory / 'verified.json', report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['capture', 'verify'])
    parser.add_argument('--runtime', choices=['rust', 'wasm'])
    parser.add_argument('--source', type=Path, default=ROOT / 'target/s08/e2/corpus')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--smoke', type=int, default=0)
    parser.add_argument('--timeout', type=float, default=120)
    args = parser.parse_args()
    if args.smoke < 0 or not 0 < args.timeout <= 3600:
        parser.error('invalid smoke count or timeout')
    if args.action == 'capture' and args.runtime is None:
        parser.error('capture requires --runtime')
    result = capture(args) if args.action == 'capture' else verify(args.output)
    print(json.dumps({k: v for k, v in result.items() if k != 'findings'}, indent=2))


if __name__ == '__main__':
    main()
