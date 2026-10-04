#!/usr/bin/env python3
"""Run selected *unchanged* native client tests against Go and the Rust private server.
The overlay replaces the client/server connection only; no baseline/evidence ledger.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
TESTS = '^(TestInitializeCodeActionKinds|TestProjectInfoConfiguredProject|TestProjectInfoInferredProject|TestProgressNotificationsEndToEnd|TestL2InferredOptionsAndDocumentSync)$'

def main():
    go = shutil.which('go')
    env = {**os.environ, 'GOTOOLCHAIN': 'local', 'GOWORK': 'off', 'GOCACHE': str(ROOT / 'target/go-build')}
    expected = tomllib.loads((ROOT / 'data/s04/toolchains.toml').read_text())['go']
    if not go or subprocess.check_output([go, 'env', 'GOVERSION'], env=env, text=True).strip() != expected:
        raise SystemExit(f'put {expected} on PATH')
    upstream = ROOT / 'upstream/tsc'
    source = upstream / 'internal/testutil/lsptestutil/lspclient.go'
    native = source.read_text()
    patched = native.replace('"io"', '"io"\n"os"', 1).replace('Server       *lsp.Server', 'Server       interface { InitComplete() <-chan struct{}; SetCompilerOptionsForInferredProjects(context.Context, *core.CompilerOptions) }', 1)
    anchor = 'func NewLSPClient(t *testing.T, serverOpts lsp.ServerOptions, onServerRequest ServerRequestHandler) (*LSPClient, func() error) {'
    assert patched.count(anchor) == 1
    patched = patched.replace(anchor, anchor + '\nif os.Getenv("TSR_LSP_SERVER") != "" {return newRustClient(t,serverOpts,onServerRequest)}', 1)
    with tempfile.TemporaryDirectory(prefix='tsr-l2-client-') as tmp:
        stage = Path(tmp)
        (stage / 'lspclient.go').write_text(patched)
        overlay = stage / 'overlay.json'
        overlay.write_text(json.dumps({'Replace': {str(source): str(stage / 'lspclient.go'), str(source.parent / 'rust_client.go'): str(HERE / 'rust_client.go'), str(upstream / 'internal/lsp/l2_options_sync_test.go'): str(HERE / 'options_sync_test.go')}}))
        binary = ROOT / 'target/phase5/lsp-client-tests'
        binary.parent.mkdir(exist_ok=True)
        subprocess.run([go, 'test', '-overlay', str(overlay), '-c', '-o', str(binary), './internal/lsp'], cwd=upstream, env=env, check=True, timeout=300)
        for name, extra in [('Go', {}), ('Rust', {'TSR_LSP_SERVER': str(ROOT / 'target/debug/phase5_testserver')})]:
            print(name, flush=True)
            subprocess.run([str(binary), '-test.run', TESTS, '-test.timeout', '60s', '-test.v'], cwd=upstream, env={**env, **extra}, check=True, timeout=75)
if __name__ == '__main__':
    main()
