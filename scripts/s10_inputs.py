#!/usr/bin/env python3
"""Authenticate the existing S07 source corpus for the E7 parse-only interval."""
import argparse
import json
from pathlib import Path

from s10_corpus import ROOT, file_digest


def prepare(output, limit=0):
    manifest = json.loads((ROOT / 'data/s07/vscode-files.json').read_text())
    options = json.loads((ROOT / 'data/s07/vscode-parse-options.json').read_text())['files']
    if len(manifest['files']) != 13094 or len(options) != len(manifest['files']):
        raise ValueError('S10 parser workload differs from frozen S07 inventory')
    checkout = ROOT.parent / '.ts-rust-workloads' / ('vscode-' + manifest['commit'])
    files = []
    for raw, config in zip(manifest['files'], options, strict=True):
        if config['filename'] != '/vscode/' + raw['path'] or config['path'] != config['filename']:
            raise ValueError('parser options do not match source inventory')
        path = checkout / raw['path']
        if not path.is_file() or file_digest(path) != raw['sha256']:
            raise ValueError('missing/changed source; provision S07 workload: ' + str(path))
        files.append(dict(config, local=str(path), sha256=raw['sha256'], bytes=raw['bytes']))
    if limit:
        files = files[::max(1, len(files) // limit)][:limit]
    result = {'version': 1, 'partial': bool(limit), 'files': files,
              'manifest_sha256': file_digest(ROOT / 'data/s07/vscode-files.json'),
              'options_sha256': file_digest(ROOT / 'data/s07/vscode-parse-options.json')}
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, separators=(',', ':')) + '\n')
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--limit', type=int, default=0)
    args = parser.parse_args()
    if args.limit < 0:
        parser.error('limit cannot be negative')
    print(len(prepare(args.output, args.limit)['files']))
