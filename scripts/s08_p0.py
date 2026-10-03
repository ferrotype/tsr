#!/usr/bin/env python3
"""Prepare, freeze and verify S08 P0 contracts. Does not emit sprint pass metrics."""
import argparse
import json
import lzma
from pathlib import Path
import subprocess
import sys
import tomllib

from s04 import verified_upstream
from s04_common import strict_json_loads
from s08_audit import prepare as audit
from s08_contract_oracle import prepare as observe, requests, SPEC, TEXT, RESIDUALS
from s08_contracts import validate, compare
from s08_inventory import prepare as inventory
from s08_oracle import ROOT, canonical, digest
from s08_queries import prepare as queries, extract_capture

DATA = ROOT / 'data/s08'
METHODS = ('type-footprint.json', 'checker-workload.json', 'relater-fixtures.json', 'ownership-fixtures.json')
OUTPUTS = ('dependency-closure.json.xz', 'dependency-closure-report.json', 'dependency-audit.json',
           'query-contract.json', 'supplemental-observations.json.xz', 'supplemental-report.json')


def read(path):
    path = Path(path)
    raw = path.read_bytes()
    return strict_json_loads(lzma.decompress(raw) if path.suffix == '.xz' else raw)


def write(path, value):
    raw = canonical(value) + b'\n'
    Path(path).write_bytes(lzma.compress(raw, preset=6) if str(path).endswith('.xz') else raw)


def validate_sources(sources, base=ROOT):
    for name, expected in sources.items():
        if digest((base / name).read_bytes()) != expected:
            raise ValueError('contract source drift: ' + name)


def validate_methods():
    partition = read(ROOT / 'data/s07/e2-acceptance.json')
    count = sum(v['tier'] == 'acceptance' for v in partition['variants'])
    for name in METHODS:
        value = read(DATA / name)
        if value['pin'] != read(ROOT / 'data/upstream.json')['pin']:
            raise ValueError('methodology pin mismatch')
        if 'inputs' in value:
            validate_sources(value['inputs'])
            if value['acceptance_variants'] != count:
                raise ValueError('methodology denominator mismatch')
    relater = read(DATA / 'relater-fixtures.json')
    if relater['request_sha256'] != digest((ROOT / relater['request_source']).read_bytes()):
        raise ValueError('relater request source drift')
    for name in ('checker-workload.json', 'relater-fixtures.json'):
        sampling = read(DATA / name)['sampling']
        if sampling['measured_samples_per_runtime'] != 7 or sampling['warmups_per_runtime'] != 1:
            raise ValueError('sampling protocol changed without a reviewed freeze')
    # The perf thresholds remain authoritative; a method edit cannot move a gate.
    thresholds = tomllib.loads((ROOT / 'status/perf/thresholds.toml').read_text())
    if thresholds.get('checker', {}).get('type_footprint') != read(DATA / 'type-footprint.json')['threshold']['maximum']:
        raise ValueError('type footprint methodology disagrees with status/perf/thresholds.toml')
    owners = read(DATA / 'ownership-fixtures.json')['cases']
    if len({r['id'] for r in owners}) != len(owners) or not owners:
        raise ValueError('duplicate or empty ownership fixture inventory')
    if any(not r['setup'] or not r['expected'] or r['status'] != 'pending_production_fixture' for r in owners):
        raise ValueError('ownership design contract cannot claim execution')


def validate_capture(directory):
    directory = Path(directory)
    closure = read(directory / 'dependency-closure.json.xz')
    closure_report = read(directory / 'dependency-closure-report.json')
    validate_sources(closure_report['sources'])
    for key, value in {'functions':len(closure['functions']),'calls':len(closure['calls']),'unresolved_calls':len(closure['unresolved']),'interfaces':{k:len(v) for k,v in closure['interfaces'].items()}}.items():
        if closure_report[key]!=value:raise ValueError('forged typed inventory count: '+key)
    if closure_report['decoded_sha256'] != digest(canonical(closure) + b'\n'):
        raise ValueError('typed closure digest mismatch')
    if canonical(audit(closure)) != canonical(read(directory / 'dependency-audit.json')):
        raise ValueError('obligation or callback audit drift')
    report = read(directory / 'supplemental-report.json')
    validate_sources(report['sources'])
    supplemental_raw = lzma.decompress((directory / 'supplemental-observations.json.xz').read_bytes())
    if report['frozen_observation_sha256'] != digest(supplemental_raw):
        raise ValueError('supplemental output digest mismatch')
    supplemental = strict_json_loads(supplemental_raw)
    spec = read(ROOT / SPEC)
    validate(spec, requests(spec), read(ROOT / TEXT), read(ROOT / RESIDUALS), supplemental)
    if report['cases']!=len(supplemental['rows']) or report['relation_actions']!=sum(len(g['actions']) for r in supplemental['rows'] for g in r['groups']):
        raise ValueError('forged supplemental capture count')
    validate_sources(read(directory / 'query-contract.json')['sources'])
    upstream = verified_upstream()
    validate_sources(closure_report['native_sources'], upstream / 'tsc')
    validate_sources(report['native_sources'], upstream / 'tsc')
    validate_methods()
    return closure, supplemental


def assemble(directory, inventory_dir, query_dir, supplemental_dir):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=False)
    closure = read(Path(inventory_dir) / 'dependency-closure.json.xz')
    write(directory / 'dependency-closure.json.xz', closure)
    write(directory / 'dependency-closure-report.json', read(Path(inventory_dir) / 'report.json'))
    write(directory / 'dependency-audit.json', audit(closure))
    write(directory / 'query-contract.json', read(Path(query_dir) / 'query-contract.json'))
    supplemental = read(Path(supplemental_dir) / 'observations.json')
    write(directory / 'supplemental-observations.json.xz', supplemental)
    report = read(Path(supplemental_dir) / 'report.json')
    report['frozen_observation_sha256'] = digest(canonical(supplemental) + b'\n')
    write(directory / 'supplemental-report.json', report)
    validate_capture(directory)


def prepare(directory, capture=None):
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    inventory(directory / 'inventory')
    observe(directory / 'supplemental')
    if capture is None:
        capture = extract_capture(directory / 'archived-inputs')
    queries(capture, directory / 'queries')
    assemble(directory / 'candidate', directory / 'inventory', directory / 'queries', directory / 'supplemental')


def source_inventory():
    paths = sorted({*ROOT.glob('scripts/s08*.py'), *ROOT.glob('tools/s08/inventory/*'),
                    *ROOT.glob('tools/s08/contracts/*'), *ROOT.glob('tools/s08/oracle/contracts/*')})
    return {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in paths if p.is_file()}


def summary_counts(directory):
    directory=Path(directory)
    supplemental=read(directory/'supplemental-observations.json.xz')
    query=read(directory/'query-contract.json')
    return dict(acceptance_variants=query['acceptance_variants'],query_operations=sum(query['query_operations'].values()),
                obligations=len(read(directory/'dependency-audit.json')['obligations']),
                relation_cases=len(supplemental['rows']),relation_actions=sum(len(g['actions']) for r in supplemental['rows'] for g in r['groups']),
                residual_cases=len(supplemental['supplemental']['residuals']),text_cases=len(supplemental['supplemental']['text']))


def freeze(directory):
    closure, supplemental = validate_capture(directory)
    for name in OUTPUTS:
        (DATA / name).write_bytes((Path(directory) / name).read_bytes())
    manifest = dict(version=1, pin=read(ROOT / 'data/upstream.json')['pin'],
                    scope='P0 contracts frozen; all Rust semantic parity, census, benchmark and reference-relater results remain pending',
                    artifacts={name: digest((DATA / name).read_bytes()) for name in (*OUTPUTS, *METHODS)},
                    sources=source_inventory(), counts=summary_counts(DATA))
    write(DATA / 'p0-contract.json', manifest)
    print(json.dumps(manifest['counts'], sort_keys=True))


def check(directory, native=False, capture=None):
    manifest = read(DATA / 'p0-contract.json')
    validate_sources(manifest['artifacts'], DATA)
    validate_sources(manifest['sources'])
    if manifest['sources'] != source_inventory():
        raise ValueError('P0 source inventory changed')
    closure, supplemental = validate_capture(DATA)
    if canonical(manifest['counts'])!=canonical(summary_counts(DATA)):
        raise ValueError('forged P0 summary counts')
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    if capture is None:
        capture = extract_capture(directory / 'archived-inputs')
    queries(capture, directory / 'queries')
    if read(directory / 'queries/query-contract.json') != read(DATA / 'query-contract.json'):
        raise ValueError('native query projection drift')
    if native:
        actual_closure, _ = inventory(directory / 'inventory')
        if canonical(actual_closure) != canonical(closure):
            raise ValueError('native typed dependency closure drift')
        actual, _ = observe(directory / 'supplemental')
        compare(supplemental, actual)
    print(json.dumps(dict(verified=True, native_regenerated=native, **manifest['counts']), sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=('prepare', 'freeze', 'check'))
    parser.add_argument('--output', type=Path, help='new review directory for prepare/check')
    parser.add_argument('--candidate', type=Path, help='reviewed candidate directory for freeze')
    parser.add_argument('--capture', type=Path, help='optional existing final capture; still authenticated against the archive')
    parser.add_argument('--native', action='store_true', help='check: also regenerate the native dependency and supplemental observations')
    args = parser.parse_args()
    if args.operation == 'freeze':
        if args.candidate is None:
            parser.error('freeze requires --candidate')
        freeze(args.candidate)
    elif args.output is None:
        parser.error('prepare/check requires --output')
    elif args.operation == 'prepare':
        prepare(args.output, args.capture)
    else:
        check(args.output, args.native, args.capture)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
        print(f'S08 P0 failed: {error}', file=sys.stderr)
        raise SystemExit(1) from error
