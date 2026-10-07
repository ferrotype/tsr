#!/usr/bin/env python3
"""Run selected *unchanged* native client tests against Go and the Rust private server.
The overlay replaces the client/server connection only; no baseline/evidence ledger.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import sys
import tomllib
ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / 'tools/phase5/harness'))
import overlay as harness_overlay
TESTS = '^(TestInitializeCodeActionKinds|TestProjectInfoConfiguredProject|TestProjectInfoInferredProject|TestProgressNotificationsEndToEnd|TestL2InferredOptionsAndDocumentSync)$'

def build_rust_server():
    result = subprocess.run(
        ['cargo', 'build', '-p', 'tsr_testhost', '--bin', 'phase5_testserver', '--message-format=json'],
        cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True,
    )
    artifacts = [json.loads(line) for line in result.stdout.splitlines()]
    binaries = [item['executable'] for item in artifacts
                if item.get('reason') == 'compiler-artifact'
                and item.get('target', {}).get('name') == 'phase5_testserver'
                and item.get('executable')]
    if len(binaries) != 1:
        raise RuntimeError(f'expected one private server artifact, got {binaries}')
    return binaries[0]

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tests', default=TESTS)
    args = parser.parse_args()
    go = shutil.which('go')
    env = {**os.environ, 'GOTOOLCHAIN': 'local', 'GOWORK': 'off', 'GOCACHE': str(ROOT / 'target/go-build')}
    expected = tomllib.loads((ROOT / 'data/s04/toolchains.toml').read_text())['go']
    if not go or subprocess.check_output([go, 'env', 'GOVERSION'], env=env, text=True).strip() != expected:
        raise SystemExit(f'put {expected} on PATH')
    server = build_rust_server()
    upstream = ROOT / 'upstream/tsc'
    with tempfile.TemporaryDirectory(prefix='tsr-l2-client-') as tmp:
        overlay = harness_overlay.create(Path(tmp), include_l2_fixture=True)
        binary = ROOT / 'target/phase5/lsp-client-tests'
        binary.parent.mkdir(exist_ok=True)
        subprocess.run([go, 'test', '-overlay', str(overlay), '-c', '-o', str(binary), './internal/lsp'], cwd=upstream, env=env, check=True, timeout=300)
        for name, extra in [('Go', {}), ('Rust', {'TSR_LSP_SERVER': server})]:
            print(name, flush=True)
            subprocess.run([str(binary), '-test.run', args.tests, '-test.timeout', '60s', '-test.v'], cwd=upstream, env={**env, **extra}, check=True, timeout=75)
if __name__ == '__main__':
    main()
