#!/usr/bin/env python3
"""Observe the native walker and compare focused P5 bytes/query identities."""
import argparse
import copy
from pathlib import Path

from s08_baselines import overlay_sources
from s08_p5_oracle import run_overlay
from s08_oracle import ROOT, canonical, digest, strict_json_loads, verified_upstream

REQUESTS = ROOT / 'tools/s08/p5/walker-requests.json'
DRIVER = ROOT / 'tools/s08/p5/walker_oracle_test.go'


def validate(request, result):
    if request['version'] != 1 or request['scope'] != 'native-walker-focused' or result.get('version') != 1:
        raise ValueError('unknown walker contract version')
    ids = [c['id'] for c in request['cases']]
    if len(set(ids)) != len(ids) or [c['id'] for c in result['cases']] != ids:
        raise ValueError('missing, duplicate, extra or reordered case')
    for spec, row in zip(request['cases'], result['cases'], strict=True):
        if row.get('state') != 'executed':
            raise ValueError(f"walker failed for {spec['id']}: {row}")
        if set(row) != {'id', 'state', 'types', 'symbols', 'queries'}:
            raise ValueError('unexpected walker result fields')
        for key in ('types', 'symbols'):
            item = row[key]
            if (item.get('state') == 'disabled') != (not spec['enabled']):
                raise ValueError('baseline enablement changed')
            if item.get('state') == 'content':
                if set(item) != {'state', 'text_hex'} or bytes.fromhex(item['text_hex']).hex() != item['text_hex']:
                    raise ValueError('invalid baseline bytes')
            elif item.get('state') not in ('no_content', 'disabled') or set(item) != {'state'}:
                raise ValueError('unclassified baseline state')
        if not isinstance(row['queries'], list) or (not spec['enabled'] and row['queries']):
            raise ValueError('disabled baseline made queries')
        symbols = False
        for q in row['queries']:
            fields = {'operation', 'file', 'kind', 'pos', 'end'}
            operation = q.get('operation')
            if operation == 'GetTypeAtLocation':
                fields |= {'result_flags', 'type_id'}
            elif operation == 'TypeToTypeNode':
                fields |= {'type_id', 'flags', 'internal_flags'}
            elif operation == 'GetSymbolAtLocation':
                symbols = True
                if 'absent' in q:
                    fields.add('absent')
                    if q['absent'] is not True:
                        raise ValueError('invalid absent query')
            elif operation == 'SymbolToStringEx':
                symbols = True
                fields.add('flags')
            else:
                raise ValueError('unknown query operation')
            if symbols and operation in ('GetTypeAtLocation', 'TypeToTypeNode'):
                raise ValueError('type queries occurred after symbol walk')
            if set(q) != fields or q['file'] not in {f['name'] for f in spec['files']}:
                raise ValueError('invalid query fields or file')
            if any(type(q[k]) is not int for k in fields - {'operation', 'file', 'absent'}):
                raise ValueError('noninteger query field')
            if q.get('type_id', 1) <= 0:
                raise ValueError('invalid type identity')
    return len(ids)


def normalized(row):
    # Type IDs are runtime-local. Preserve repeated/ distinct identity within
    # each file's native checker, never compare numeric IDs across runtimes or
    # infer that checker instances for two source files are the same universe.
    row = copy.deepcopy(row)
    identities = {}
    for q in row['queries']:
        if 'type_id' in q:
            table = identities.setdefault(q['file'], {})
            q['type_id'] = table.setdefault(q['type_id'], len(table) + 1)
    return row


def capture(output):
    paths = [REQUESTS, DRIVER, Path(__file__), ROOT / 'scripts/s08_oracle.py', ROOT / 'scripts/s08_p5_oracle.py', ROOT / 'scripts/s08_baselines.py',
             ROOT / 'tools/s08/oracle/baselines_bridge.go', ROOT / 'tools/s08/oracle/diagnostics_observer.go']
    sources = {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in paths}
    extras = overlay_sources(verified_upstream())
    del extras['testrunner/s08_baselines_test.go']
    request = strict_json_loads(REQUESTS.read_bytes())
    observed = run_overlay(output, 'testutil/tsbaseline', DRIVER.read_text(), request, 'TestS08P5Walker', extra_sources=extras)
    if sources != {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in paths}:
        raise ValueError('capture sources changed during observation')
    (output / 'sources.json').write_bytes(canonical(sources) + b'\n')
    count = validate(request, observed)
    print(f'Captured {count} native walker cases in {output}')


def compare(native, actual):
    request = strict_json_loads((native / 'requests.json').read_bytes())
    expected = strict_json_loads((native / 'observations.json').read_bytes())
    observed = strict_json_loads(actual.read_bytes())
    fingerprint = digest((native / 'requests.json').read_bytes())
    if expected['request_sha256'] != fingerprint or observed.get('request_sha256') != fingerprint:
        raise ValueError('request fingerprint differs')
    provenance = strict_json_loads((native / 'provenance.json').read_bytes())
    if provenance['output_sha256'] != digest((native / 'observations.json').read_bytes()):
        raise ValueError('native output fingerprint differs')
    validate(request, expected)
    count = validate(request, observed)
    mismatches = [a['id'] for a, b in zip(expected['cases'], observed['cases'], strict=True) if normalized(a) != normalized(b)]
    if mismatches:
        raise ValueError('walker bytes or query schedule differ: ' + ', '.join(mismatches))
    print(f'{count}/{count} exact native walker results and query schedules match')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    capture_parser = sub.add_parser('capture')
    capture_parser.add_argument('--output', type=Path, required=True)
    compare_parser = sub.add_parser('compare')
    compare_parser.add_argument('--native', type=Path, required=True)
    compare_parser.add_argument('--actual', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'capture':
        capture(args.output.resolve())
    else:
        compare(args.native, args.actual)


if __name__ == '__main__':
    main()
