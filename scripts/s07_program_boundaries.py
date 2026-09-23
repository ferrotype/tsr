#!/usr/bin/env python3
"""Four direct native loader boundaries, separate from the original 48 observations."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil

from s04_common import command, strict_json_loads
from s06_build import ROOT, oracle_export
from s07_program import validate_requests


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.check and args.output:
        raise ValueError('--check and --output are mutually exclusive')
    request_path = ROOT / 'data/s07/program-boundary-requests.json'
    requests = request_path.read_bytes()
    identities = validate_requests(strict_json_loads(requests))
    adapters = [ROOT / 'tools/s07/program/export_test.go',
                ROOT / 'tools/s07/program/boundaries/export_test.go']
    adapter_hashes = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                      for path in adapters}
    with oracle_export() as (checkout, env, pin):
        for index, adapter in enumerate(adapters):
            shutil.copyfile(adapter, checkout / f'tsc/internal/compiler/s07_boundary_{index}_test.go')
        output = checkout / 'observations.json'
        env.update(S07_BOUNDARY_REQUESTS=str(request_path), S07_BOUNDARY_OUTPUT=str(output))
        repo = f'-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={checkout}/s07-unmatched-prefix'
        command(['go', 'test', '-trimpath', '-mod=readonly', repo, './internal/compiler',
                 '-run', '^TestS07ProgramBoundaries$', '-count=1'], cwd=checkout / 'tsc', env=env)
        data = output.read_bytes()
        rows = strict_json_loads(data)
        if [row['ID'] for row in rows] != identities or any(row.get('Panic') for row in rows):
            raise ValueError('missing, extra, reordered or panicking native loader boundary')
        manifest = {
            'upstream_pin': pin,
            'requests_sha256': hashlib.sha256(requests).hexdigest(),
            'observations_sha256': hashlib.sha256(data).hexdigest(),
            'adapters': adapter_hashes,
            'source_sha256': {str(path.relative_to(checkout)): hashlib.sha256(path.read_bytes()).hexdigest()
                              for path in sorted((checkout / 'tsc/internal/compiler').glob('*.go'))
                              if not path.name.endswith('_test.go')},
            'rows': len(rows),
        }
    if request_path.read_bytes() != requests or any(
            hashlib.sha256((ROOT / path).read_bytes()).hexdigest() != digest
            for path, digest in adapter_hashes.items()):
        raise ValueError('loader boundary requests or adapters changed during capture')
    manifest_bytes = (json.dumps(manifest, sort_keys=True, indent=2) + '\n').encode()
    if args.output:
        args.output.write_bytes(data)
        args.output.with_suffix('.manifest.json').write_bytes(manifest_bytes)
    else:
        for name, raw in {'observations': data, 'manifest': manifest_bytes}.items():
            path = ROOT / f'data/s07/program-boundary-{name}.json'
            if args.check:
                if path.read_bytes() != raw:
                    raise ValueError(f'pinned loader boundary {name} changed')
            else:
                path.write_bytes(raw)
    print(f'{len(rows)} direct Go loader boundary rows')


if __name__ == '__main__':
    main()
