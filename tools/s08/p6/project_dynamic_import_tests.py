#!/usr/bin/env python3
"""Project focused regression expectations from authenticated native P2 captures.

This projection checks top-level type text and complete semantic diagnostics.
It does not replace the full P2 comparison of nested properties and signatures.
"""
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'scripts'))
import s08_p2 as p2


def project(directories):
    cases = []
    identities = set()
    for directory in directories:
        report = p2.strict_json_loads((directory / 'report.json').read_bytes())
        for name, expected in report['sources'].items():
            if p2.digest((directory / 'source-snapshot' / name).read_bytes()) != expected:
                raise ValueError('native source snapshot differs: ' + name)
        for name, key in (('requests.json', 'request_sha256'),
                          ('observations.json', 'observation_sha256')):
            if p2.digest((directory / name).read_bytes()) != report[key]:
                raise ValueError('native capture fingerprint differs: ' + name)
        spec = p2.strict_json_loads((directory / 'requests.json').read_bytes())
        native = p2.strict_json_loads((directory / 'observations.json').read_bytes())
        p2.specification(spec)
        p2.validate(spec, native)
        for request, observed in zip(spec['programs'], native['programs'], strict=True):
            if request['id'] in identities:
                raise ValueError('duplicate fixture identity: ' + request['id'])
            identities.add(request['id'])
            cases.append({
                'options': spec['options'], 'program': request,
                'native_capture': directory.name,
                'request_sha256': report['request_sha256'],
                'types': [{'id': query['id'], 'display_hex': query['type']['display_hex']}
                          for query in observed['queries']],
                'semantic': observed['diagnostics']['semantic'],
            })
    return {'version': 1,
            'scope': 'Native top-level public type displays and complete semantic diagnostics; '
                     'nested properties and signatures remain in the separate full comparison',
            'cases': cases}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('captures', type=Path, nargs='+')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    raw = (json.dumps(project(args.captures), indent=2, sort_keys=True) + '\n').encode()
    if args.check:
        if args.output.read_bytes() != raw:
            raise ValueError('committed dynamic-import expectations differ from native projection')
    else:
        args.output.write_bytes(raw)
