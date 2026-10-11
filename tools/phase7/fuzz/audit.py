#!/usr/bin/env python3
"""Write a deterministic lexical Unsupported / not available review inventory.

This scans Rust source lines under crates, including comments, tests and generated
files. It does not classify reachable behavior, approvals, scope or completeness.
The output is review input, never a pass/fail metric.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[3]
PATTERN = re.compile(r'\bUnsupported\b|not available')


def inventory(root):
    sources = root / 'crates'
    if not sources.is_dir():
        raise ValueError(f'no crates source directory: {sources}')
    entries, skipped = [], []
    source_hash = hashlib.sha256()
    files = 0
    for path in sorted(sources.rglob('*.rs')):
        if path.is_symlink() or not path.is_file():
            skipped.append(path.relative_to(root).as_posix())
            continue
        relative = path.relative_to(root).as_posix()
        data = path.read_bytes()
        source_hash.update(relative.encode() + b'\0' + hashlib.sha256(data).digest())
        files += 1
        for number, line in enumerate(data.decode('utf-8').splitlines(), 1):
            matches = [match.group() for match in PATTERN.finditer(line)]
            if matches:
                entries.append({'file': relative, 'line': number, 'text': line, 'matches': matches})
    return {'schema': 1, 'scope': 'all regular crates/**/*.rs files; lexical line matches',
            'pattern': PATTERN.pattern, 'files_scanned': files,
            'source_sha256': source_hash.hexdigest(), 'skipped': skipped,
            'interpretation': 'Review inventory only. Includes comments, tests and generated sources. '
                              'No reachability, semantic classification, approval, scope decision, '
                              'coverage or passing metric is inferred. Other refusal wording and '
                              'multiline phrases are outside this lexical search.',
            'entries': entries}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT, help='repository containing crates/')
    parser.add_argument('--output', type=Path, required=True, help='new JSON file outside target')
    args = parser.parse_args(argv)
    output = args.output.resolve()
    if 'target' in output.parts or output.exists():
        parser.error('output must be a new file outside target')
    result = inventory(args.root.resolve())
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open('x') as destination:
        json.dump(result, destination, indent=2)
        destination.write('\n')
    print(json.dumps({'output': str(output), 'matched_lines': len(result['entries']),
                      'interpretation': 'lexical review inventory, not a pass metric'}))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
