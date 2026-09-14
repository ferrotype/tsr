#!/usr/bin/env python3
"""Focused native P5 display observations, not an E2 acceptance producer."""
import argparse
from pathlib import Path

from s08_oracle import ROOT, canonical, digest, run_overlay, strict_json_loads

REQUESTS = ROOT / 'tools/s08/p5/display-requests.json'
DRIVER = ROOT / 'tools/s08/p5/display_oracle_test.go'


def validate(request, result):
    if request['version'] != 1 or result.get('version') != 1:
        raise ValueError('unknown display contract version')
    if [p['id'] for p in result['programs']] != [p['id'] for p in request['programs']]:
        raise ValueError('missing, extra or reordered program')
    ids = set()
    count = 0
    for spec, observed in zip(request['programs'], result['programs'], strict=True):
        if spec['id'] in ids:
            raise ValueError('duplicate program identity')
        ids.add(spec['id'])
        if spec.get('module', 'esnext') not in ('esnext', 'node16', 'nodenext'):
            raise ValueError('invalid display module option')
        files, encoded = spec.get('files'), spec.get('file_bytes', {})
        if not isinstance(files, dict) or not isinstance(encoded, dict) or files.keys() & encoded.keys():
            raise ValueError('invalid or overlapping source encodings')
        if any(not isinstance(path, str) or not isinstance(value, str) for path, value in files.items()):
            raise ValueError('invalid text source')
        for path, value in encoded.items():
            if not isinstance(path, str) or not isinstance(value, str) or bytes.fromhex(value).hex() != value:
                raise ValueError('invalid canonical source hex')
        if [q['id'] for q in observed['queries']] != [q['id'] for q in spec['queries']]:
            raise ValueError('missing, extra or reordered display query')
        query_ids = set()
        for q, row in zip(spec['queries'], observed['queries'], strict=True):
            if q['id'] in query_ids:
                raise ValueError('duplicate query identity')
            query_ids.add(q['id'])
            if ('enclosing_declaration' in q
                    and (not isinstance(q['enclosing_declaration'], str)
                         or not q['enclosing_declaration'] or q.get('context') not in ('declaration', 'source'))):
                raise ValueError('invalid enclosing declaration override')
            if row.get('state') == 'absent':
                if q['operation'] == 'type_string' or set(row) != {'id', 'state'}:
                    raise ValueError('invalid absent display result')
            elif row.get('state') == 'content':
                fields = {'id', 'state', 'text_hex'}
                if q['operation'] == 'type_node':
                    fields.add('kind')
                    if type(row.get('kind')) is not int:
                        raise ValueError('type node lacks kind')
                if set(row) != fields or not isinstance(row.get('text_hex'), str):
                    raise ValueError('invalid display payload')
                if bytes.fromhex(row['text_hex']).hex() != row['text_hex']:
                    raise ValueError('noncanonical display bytes')
            else:
                raise ValueError('unclassified display result')
            count += 1
    return count


def capture(output):
    request = strict_json_loads(REQUESTS.read_bytes())
    names = [REQUESTS, DRIVER, Path(__file__), ROOT / 'scripts/s08_oracle.py']
    sources = {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in names}
    observed = run_overlay(output, 'checker', DRIVER.read_text(), request, 'TestS08P5Display')
    count = validate(request, observed)
    if sources != {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in names}:
        raise ValueError('capture inputs changed while Go ran')
    (output / 'sources.json').write_bytes(canonical(sources) + b'\n')
    print(f'Captured {count} native display requests in {output}')


def compare(native, actual):
    request = strict_json_loads((native / 'requests.json').read_bytes())
    expected = strict_json_loads((native / 'observations.json').read_bytes())
    observed = strict_json_loads(actual.read_bytes())
    if expected['request_sha256'] != digest(canonical(request) + b'\n'):
        raise ValueError('native request fingerprint differs')
    if observed.get('request_sha256') != expected['request_sha256']:
        raise ValueError('Rust did not observe the captured request bytes')
    provenance = strict_json_loads((native / 'provenance.json').read_bytes())
    if provenance['output_sha256'] != digest((native / 'observations.json').read_bytes()):
        raise ValueError('native observation fingerprint differs')
    validate(request, expected)
    count = validate(request, observed)
    if observed['programs'] != expected['programs']:
        mismatches = [q['id'] for p, r in zip(expected['programs'], observed['programs'], strict=True)
                      for q, s in zip(p['queries'], r['queries'], strict=True) if q != s]
        raise ValueError('display differs: ' + ', '.join(mismatches))
    print(f'{count}/{count} exact display results match pinned Go')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    run = commands.add_parser('capture')
    run.add_argument('--output', type=Path, required=True)
    check = commands.add_parser('compare')
    check.add_argument('--native', type=Path, required=True)
    check.add_argument('--actual', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'capture':
        capture(args.output.resolve())
    else:
        compare(args.native, args.actual)


if __name__ == '__main__':
    main()
