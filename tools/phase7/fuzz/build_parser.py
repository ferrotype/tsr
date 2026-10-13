#!/usr/bin/env python3
"""Build only the narrow R5 parser oracle from an untouched verified Go pin.

No corpus generation, fixture export, expectation freezing or upstream edits.
The required pinned Go toolchain/cache policy comes from the existing S04 helper.
"""
from __future__ import annotations

import argparse
import io
import json
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'scripts'))
from s04 import go_environment, verified_upstream  # noqa: E402
from s04_common import command  # noqa: E402
sys.path.insert(0, str(ROOT))
from tools.phase7.fuzz import run as fuzz  # noqa: E402


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path, help='new saved build directory outside target')
    args = parser.parse_args(argv)
    output = args.output.resolve()
    if output.exists() or 'target' in output.parts:
        parser.error('output must be a new directory outside target')
    upstream, env = verified_upstream(), go_environment()
    pin = json.loads((ROOT / 'data/upstream.json').read_bytes())['pin']
    source = Path(__file__).with_name('parser_oracle') / 'main.go'
    output.mkdir(parents=True)
    record = {'pin': pin, 'adapter_sha256': fuzz.sha256(source.read_bytes()), 'status': 'execution-failure'}
    try:
        with tempfile.TemporaryDirectory(prefix='pin-export-', dir=output) as temporary:
            checkout = Path(temporary)
            archive = command(['git', 'archive', pin, 'tsc/go.mod', 'tsc/go.sum', 'tsc/internal'], cwd=upstream)
            with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
                stream.extractall(checkout, filter='data')
            package = checkout / 'tsc/internal/r5parser'
            package.mkdir()
            shutil.copyfile(source, package / 'main.go')
            executable = output / 'parser-go'
            arguments = ['go', 'build', '-trimpath', '-mod=readonly', '-o', str(executable), './internal/r5parser']
            record['command'] = arguments
            command(arguments, cwd=checkout / 'tsc', env=env)
        verified_upstream()
        record.update(status='built', binary=fuzz.identity(executable))
    finally:
        (output / 'build.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
