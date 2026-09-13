#!/usr/bin/env python3
"""Capture/replay native diagnostic formatting and error-baseline bytes."""
import argparse
from pathlib import Path
from s08_p5_oracle import run_overlay
from s08_oracle import ROOT, canonical, digest, strict_json_loads

REQUESTS = ROOT / 'tools/s08/p5/error-requests.json'
DRIVER = ROOT / 'tools/s08/p5/error_oracle_test.go'


def validate(request, result):
    if request.get('version') != 1 or request.get('scope') != 'diagnostic-writer-focused' or result.get('version') != 1:
        raise ValueError('unknown diagnostic writer contract')
    ids = [c['id'] for c in request['cases']]
    if len(set(ids)) != len(ids) or [c['id'] for c in result['cases']] != ids:
        raise ValueError('missing, duplicate, extra or reordered case')
    def check_hex(value):
        if not isinstance(value, str) or bytes.fromhex(value).hex() != value:
            raise ValueError('noncanonical output bytes')
    for case, row in zip(request['cases'], result['cases'], strict=True):
        if row.get('state') != 'executed' or set(row) != {'id', 'state', 'plain_hex', 'pretty_hex', 'summary_hex', 'errors_plain', 'errors_pretty'}:
            raise ValueError(f"diagnostic writer failed for {case['id']}: {row}")
        for key in ('plain_hex', 'pretty_hex', 'summary_hex'):
            check_hex(row[key])
        for key in ('errors_plain', 'errors_pretty'):
            item = row[key]
            if case['diagnostics']:
                if item.get('state') != 'content' or set(item) != {'state', 'text_hex'}:
                    raise ValueError('diagnostics lost their baseline')
                check_hex(item['text_hex'])
            elif item != {'state': 'no_content'}:
                raise ValueError('no-content baseline state changed')
    return len(ids)


def capture(output):
    paths = [REQUESTS, DRIVER, Path(__file__), ROOT / 'scripts/s08_p5_oracle.py', ROOT / 'scripts/s08_oracle.py']
    sources = {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in paths}
    request = strict_json_loads(REQUESTS.read_bytes())
    result = run_overlay(output, 'testutil/tsbaseline', DRIVER.read_text(), request, 'TestS08P5Errors')
    if sources != {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in paths}:
        raise ValueError('capture source changed during observation')
    (output / 'sources.json').write_bytes(canonical(sources) + b'\n')
    print(f'Captured {validate(request, result)} native diagnostic writer cases in {output}')


def compare(native, actual):
    request_bytes = (native / 'requests.json').read_bytes()
    request = strict_json_loads(request_bytes)
    expected_bytes = (native / 'observations.json').read_bytes()
    expected = strict_json_loads(expected_bytes)
    actual = strict_json_loads(actual.read_bytes())
    provenance = strict_json_loads((native / 'provenance.json').read_bytes())
    fingerprint = digest(request_bytes)
    if expected.get('request_sha256') != fingerprint or actual.get('request_sha256') != fingerprint or provenance['request_sha256'] != fingerprint:
        raise ValueError('request fingerprint differs')
    if provenance['output_sha256'] != digest(expected_bytes):
        raise ValueError('native observation fingerprint differs')
    validate(request, expected)
    count = validate(request, actual)
    mismatches = [a['id'] for a, b in zip(expected['cases'], actual['cases'], strict=True) if a != b]
    if mismatches:
        raise ValueError('diagnostic bytes differ: ' + ', '.join(mismatches))
    print(f'{count}/{count} native diagnostic writer and error-baseline results match exactly')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    cap = sub.add_parser('capture')
    cap.add_argument('--output', type=Path, required=True)
    cmp = sub.add_parser('compare')
    cmp.add_argument('--native', type=Path, required=True)
    cmp.add_argument('--actual', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'capture': capture(args.output.resolve())
    else: compare(args.native, args.actual)


if __name__ == '__main__':
    main()
