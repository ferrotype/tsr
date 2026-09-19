#!/usr/bin/env python3
"""Build S10's parser-only wasm and real --api executable from the Git pin."""
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

from s04 import go_environment, verified_upstream
from s10_corpus import ROOT, file_digest, sources


def build():
    before = sources()
    upstream = verified_upstream()
    env = go_environment()
    pin = json.loads((ROOT / 'data/upstream.json').read_text())['pin']
    output = ROOT / 'target/s10/go'
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='export-', dir=output) as temporary:
        checkout = Path(temporary)
        archive = subprocess.check_output(['git', 'archive', pin, 'tsc/go.mod', 'tsc/go.sum',
                                           'tsc/internal', 'tsc/cmd/tsc'], cwd=upstream)
        with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
            stream.extractall(checkout, filter='data')
        entry = checkout / 'tsc/cmd/s10-parser'
        entry.mkdir()
        shutil.copy2(ROOT / 'tools/s10/go-parser/main.go', entry / 'main.go')
        commands = [
            (['go', 'build', '-trimpath', '-mod=readonly', '-ldflags=-s -w', '-o',
              str(output / 'parser.wasm'), './cmd/s10-parser'], dict(env, GOOS='js', GOARCH='wasm')),
            (['go', 'build', '-trimpath', '-mod=readonly', '-o', str(output / 'tsgo'), './cmd/tsc'], env),
        ]
        for command, environment in commands:
            subprocess.run(command, cwd=checkout / 'tsc', env=environment, check=True)
    goroot = Path(subprocess.check_output(['go', 'env', 'GOROOT'], env=env, text=True).strip())
    shutil.copy2(goroot / 'lib/wasm/wasm_exec.js', output / 'wasm_exec.cjs')
    verified_upstream()
    if sources() != before:
        raise ValueError('sources changed during Go build')
    record = {'pin': pin, 'sources': before, 'go': subprocess.check_output(['go', 'version'], env=env, text=True).strip(),
              'commands': [command for command, _ in commands], 'toolchain_local': True,
              'artifacts': {p.name: {'sha256': file_digest(p), 'bytes': p.stat().st_size}
                            for p in (output / 'parser.wasm', output / 'tsgo', output / 'wasm_exec.cjs')}}
    (output / 'build.json').write_text(json.dumps(record, indent=2) + '\n')
    print(output)


if __name__ == '__main__':
    build()
