#!/usr/bin/env python3
"""Capture pinned Go SortFunc comparison traces, or check both frozen outputs."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import sys
import tomllib

from render import ROOT, render

sys.path.insert(0, str(ROOT / "scripts"))
from s04 import go_environment


def run():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    env = go_environment()
    pin = tomllib.loads((ROOT / 'data/s04/toolchains.toml').read_text())['go']
    actual = subprocess.check_output(['go', 'env', 'GOVERSION'], env=env, text=True).strip()
    if actual != pin:
        raise SystemExit(f'Go sort trace capture requires {pin}; found {actual}')
    goroot = Path(subprocess.check_output(['go', 'env', 'GOROOT'], env=env, text=True).strip())
    source = Path(__file__).resolve().parent
    with tempfile.TemporaryDirectory(prefix='s06-go-sort-') as temporary:
        temp = Path(temporary)
        overlay = temp / 'overlay.json'
        overlay.write_text(json.dumps({'Replace': {
            str(goroot / 'src/slices/s06_trace_test.go'): str(source / 'trace_test.go'),
            str(goroot / 'src/slices/s06_access.go'): str(source / 'access.go'),
        }}))
        output = temp / 'traces.json'
        subprocess.run(
            ['go', 'test', '-overlay', str(overlay), 'slices', '-run', 'TestS06NodeIndexSortTrace', '-count=1'],
            env={**env, 'S06_SORT_TRACE_OUTPUT': str(output)}, check=True,
        )
        data = json.loads(output.read_text())
    if data['go'] != pin:
        raise SystemExit('Go sort trace compiler differs from the required pin')
    data['source_sha256'] = {
        name: hashlib.sha256((goroot / name).read_bytes()).hexdigest()
        for name in ['src/slices/sort.go', 'src/slices/zsortanyfunc.go']
    }
    outputs = {
        ROOT / 'data/s06/node-index-sort-traces.json': json.dumps(data, separators=(',', ':')) + '\n',
        ROOT / 'crates/tsr_core/src/go_sort/fixtures.rs': render(data),
    }
    for path, content in outputs.items():
        if args.check:
            if not path.is_file() or path.read_text() != content:
                raise SystemExit(f'Go sort trace drift: {path.relative_to(ROOT)}')
        else:
            path.write_text(content)
    count = sum(len(case['trace']) for case in data['cases'])
    print(f'{len(data["cases"])} pinned Go sort fixtures; {count} exact comparison observations')


if __name__ == '__main__':
    try:
        run()
    except (OSError, subprocess.SubprocessError, ValueError, KeyError) as error:
        raise SystemExit(f'Go sort trace capture failed: {error}') from None
