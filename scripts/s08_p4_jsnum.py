#!/usr/bin/env python3
"""Compare P4 exponentiation against the actual pinned Go on this architecture."""
import argparse
from pathlib import Path
import struct
import subprocess

from s08_oracle import ROOT, canonical, run_overlay


def bits(value):
    return struct.unpack('>Q', struct.pack('>d', value))[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--actual', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    # The exact 2**63 endpoint enters Go's integer path after MaxInt64 rounds
    # up to float64. Run both signs and odd/even powers on every native runner.
    pairs = [(2.0**63, 3.0), (2.0**63, 4.0), (-2.0**63, 3.0),
             (float.fromhex('0x1.fffffffffffffp+62'), 3.0), (3.0, 34.0),
             (3.0, 200.0), (13.0, 255.0), (0.0, 0.0), (-0.0, 3.0),
             (-0.0, -3.0), (2.0, 1023.0), (-1.0, 0.5)]
    request = {'pairs': [[bits(base), bits(exponent)] for base, exponent in pairs]}
    observed = run_overlay(args.output, 'jsnum',
                           (ROOT / 'tools/s08/p4/jsnum_test.go').read_text(),
                           request, 'TestS08P4NumberArithmetic')
    raw = ''.join(f'{base} {exponent}\n' for base, exponent in request['pairs']).encode()
    actual = subprocess.run([str(args.actual.resolve())], input=raw, capture_output=True, check=True)
    (args.output / 'rust.stdout').write_bytes(actual.stdout)
    (args.output / 'rust.stderr').write_bytes(actual.stderr)
    values = [int(line) for line in actual.stdout.splitlines()]
    # NaN payload/sign is not a value contract; all finite/infinite/zero results
    # compare exact bits, including native architecture-dependent endpoint.
    def equal(a, b):
        nan = lambda x: x & 0x7ff0000000000000 == 0x7ff0000000000000 and x & 0xfffffffffffff != 0
        return a == b or nan(a) and nan(b)
    matched = len(values) == len(observed['power_bits']) and all(
        equal(a, b) for a, b in zip(values, observed['power_bits']))
    result = {'matched': matched, 'cases': len(pairs), 'goarch': observed['goarch'],
              'goos': observed['goos'], 'go': observed['go']}
    (args.output / 'comparison.json').write_bytes(canonical(result) + b'\n')
    print(canonical(result).decode())
    return 0 if matched else 1


if __name__ == '__main__':
    raise SystemExit(main())
