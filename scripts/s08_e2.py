#!/usr/bin/env python3
"""E2 acceptance over authenticated captures; never launches a corpus implicitly.

capture: preflight native inputs and runtime obligations, then resumable Rust.
verify: replay raw outputs, require current sources and the full frozen inventory.
producer: retain S07 subset evidence; add correctness metrics when capture exists.
"""
import argparse
import math
from pathlib import Path
import sys
import tomllib

import s08_e2_contract as contract
import s08_e2_obligations as obligations
import s08_p4 as p4
import s08_p5_corpus as corpus
from s08_oracle import ROOT, digest

DEFAULT = ROOT / 'target/s08/e2'


def sources():
    result = corpus.sources()
    for pattern in ('scripts/s08_e2*.py', 'scripts/s08_contracts.py', 'scripts/s08_p3_comparators.py',
                    'scripts/s06_utilities.py', 'scripts/s04.py', 'scripts/s07_acceptance.py',
                    'tools/s08/oracle/**', 'tools/s08/p3c/order-inputs.json',
                    'data/s08/e2-obligations.json', 'data/s08/baseline-requests.json',
                    'data/s08/supplemental-report.json', 'data/s08/supplemental-observations.json.xz',
                    'data/s07/subset*.json', 'data/s07/e2-*.json', 'rust-toolchain*', '.cargo/**'):
        for path in ROOT.glob(pattern):
            if path.is_file(): result[str(path.relative_to(ROOT))] = digest(path.read_bytes())
    return result


def frozen():
    manifest = p4.read(p4.MANIFEST)
    if manifest['pin'] != p4.read(ROOT / 'data/upstream.json')['pin']:
        raise ValueError('E2 denominator pin changed')
    counts = contract.Counter(r['acceptance_tier'] for r in manifest['requests'])
    if dict(counts) != {'acceptance': 9369, 'informational': 1359}:
        raise ValueError('E2 denominator changed; owner review required')
    return manifest['requests']


def native_current(report):
    if report.get('public_type_strings') is not True:
        raise ValueError('native capture lacks --public-type-strings; recapture with --walker-inputs --error-inputs --public-type-strings')
    required = {'scripts/s08_baselines.py', 'tools/s08/oracle/baselines_bridge.go', 'tools/s08/oracle/baselines_test.go'}
    if not required <= report['source_inputs'].keys(): raise ValueError('native adapter source fingerprint incomplete')
    for name, expected in report['source_inputs'].items():
        if Path(name).is_absolute() or '..' in Path(name).parts or expected != digest((ROOT / name).read_bytes()):
            raise ValueError('native observation adapter changed: ' + name)


def preflight(native, loading, *, partial=False):
    native_current(p4.read(native / 'report.json'))
    pairs = corpus.prepare(native, loading)
    requests = [dict(r, public_type_strings=True) for r, _ in pairs]
    contract.inventory(requests, frozen(), partial=partial)
    for request, go in pairs:
        if go['state'] == 'executed' and request['type_baseline_requested']:
            values = contract.display_queries(go.get('public_type_strings'))
            if values is None: raise ValueError('native public TypeToString did not execute: ' + request['id'])
            contract.display_coverage(values, go['queries'])
    obligations.inputs()
    return requests


def verify(directory, *, partial=False):
    captured = directory / 'corpus'
    metadata = p4.read(captured / 'capture.json')
    if metadata['build']['sources'] != sources():
        raise ValueError('E2 corpus has stale sources; keep it for diagnosis and capture current code')
    native_current(p4.read(captured / 'native-report.json'))
    requests, rows, _ = corpus.replay(captured)
    contract.inventory(requests, frozen(), partial=partial)
    native = [p4.strict_json_loads(line) for line in (captured / 'native-observations.ndjson').read_bytes().splitlines()]
    ledger_raw = (ROOT / 'data/divergences.toml').read_bytes()
    ledger = tomllib.loads(ledger_raw.decode())
    report = contract.grade(requests, rows, native, ledger, p4.read(ROOT / 'data/upstream.json')['pin'],
                            partial=partial, approval_ids={r['id'] for r in frozen()})
    measured = obligations.replay(directory / 'obligations', sources)
    report['obligations'] = measured
    if not partial: report['metrics'].update(measured['metrics'])
    report['capture_sha256'] = digest((captured / 'capture.json').read_bytes())
    report['obligations_sha256'] = digest((directory / 'obligations/capture.json').read_bytes())
    report['divergences_sha256'] = digest(ledger_raw)
    p4.atomic(directory / ('smoke-report.json' if partial else 'verified.json'), report)
    return report


def producer():
    from s07_producers import e2 as subset
    correctness = verify(DEFAULT)['metrics'] if (DEFAULT / 'corpus').exists() else None
    result = subset()
    if correctness is not None:
        result['metrics'].update(correctness)
    else:
        print('E2 correctness unavailable: no target/s08/e2/corpus capture. See docs/S08-E2.md; subset evidence only.', file=sys.stderr)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('producer', 'obligations', 'preflight', 'capture', 'verify'))
    parser.add_argument('--output', type=Path, default=DEFAULT)
    parser.add_argument('--native', type=Path)
    parser.add_argument('--loading-requests', type=Path, default=ROOT / 'target/s07-subset/review/loading-requests.candidate.json')
    parser.add_argument('--timeout', type=float, default=60)
    parser.add_argument('--resume', action='store_true')
    parser.add_argument('--smoke', action='store_true', help='allow a partial inventory; never emit acceptance metrics')
    args = parser.parse_args()
    directory = args.output.resolve()
    if not math.isfinite(args.timeout) or args.timeout <= 0: parser.error('--timeout must be positive and finite')
    if args.command == 'producer': print(p4.canonical(producer()).decode())
    elif args.command == 'obligations':
        print(p4.canonical(obligations.capture(directory / 'obligations', sources)['metrics']).decode())
    elif args.command in ('preflight', 'capture'):
        if args.native is None: parser.error('--native required')
        requests = preflight(args.native.resolve(), args.loading_requests.resolve(), partial=args.smoke)
        if args.command == 'preflight':
            print(p4.canonical({'requests': len(requests), 'partial': args.smoke}).decode())
            return
        if (directory / 'obligations').exists(): obligations.replay(directory / 'obligations', sources)
        else: obligations.capture(directory / 'obligations', sources)
        corpus.run(args.native.resolve(), args.loading_requests.resolve(), directory / 'corpus', args.timeout,
                   args.resume, public_type_strings=True, source_fn=sources)
        print(p4.canonical(verify(directory, partial=args.smoke)['metrics']).decode())
    else:
        print(p4.canonical(verify(directory, partial=args.smoke)['metrics']).decode())


if __name__ == '__main__':
    try: main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print('E2 evidence unavailable: ' + str(error), file=sys.stderr)
        raise SystemExit(1) from error
