#!/usr/bin/env python3
"""Check the codegen resolver and codec unit fixture against pinned Go.

--update refreshes only codec-expected.json, a small unit-test fixture. This is
not a suite acceptance capture or an evidence/freshness registry.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--update', action='store_true')
    args = parser.parse_args()
    go = shutil.which('go')
    expected_version = tomllib.loads((ROOT / 'data/s04/toolchains.toml').read_text())['go']
    env = {**os.environ, 'GOTOOLCHAIN': 'local', 'GOWORK': 'off', 'GOCACHE': str(ROOT / 'target/go-build')}
    if not go or subprocess.check_output([go, 'env', 'GOVERSION'], env=env, text=True).strip() != expected_version:
        parser.error(f'put {expected_version} on PATH')
    upstream = ROOT / 'upstream'
    pin = json.loads((ROOT / 'data/upstream.json').read_text())['pin']
    if subprocess.run(['git', '-C', str(upstream), 'diff', '--quiet', pin, '--', 'tsc'], check=False).returncode:
        parser.error('restore pinned upstream/tsc before running the native comparison')
    package = 'tsc/internal/lsp/lsproto'
    with tempfile.TemporaryDirectory(prefix='tsr-lsproto-') as temporary:
        stage = Path(temporary)
        generated = stage / 'regenerated.go'
        subprocess.run(['node', str(HERE / 'export.mjs'), str(stage / 'model.json'), str(generated)], cwd=ROOT, check=True)
        # Overlay the probe without editing the canonical submodule.
        replacements = {}
        files = subprocess.check_output(['git', '-C', str(upstream), 'ls-tree', '--name-only', pin, package + '/'], text=True).splitlines()
        for name in files:
            if not name.endswith('.go'):
                continue
            path = stage / Path(name).name
            path.write_bytes(subprocess.check_output(['git', '-C', str(upstream), 'show', f'{pin}:{name}']))
            replacements[str(upstream / name)] = str(path)
        replacements[str(upstream / package / 'rust_codec_probe_test.go')] = str(HERE / 'probe_test.go')
        overlay = stage / 'overlay.json'
        output = stage / 'result.json'
        params_output = stage / 'params-result.json'
        env.update(TSR_CODEC_CASES=str(HERE / 'codec-cases.json'), TSR_CODEC_RESULT=str(output),
                   TSR_PARAMS_CASES=str(HERE / 'params-cases.json'), TSR_PARAMS_RESULT=str(params_output),
                   TSR_PINNED_GO=str(stage / 'lsp_generated.go.pinned'), TSR_GENERATED_GO=str(generated))
        pinned = subprocess.check_output(['git', '-C', str(upstream), 'show', f'{pin}:{package}/lsp_generated.go'])
        Path(env['TSR_PINNED_GO']).write_bytes(pinned)
        replacements[str(upstream / package / 'lsp_generated.go')] = env['TSR_PINNED_GO']
        overlay.write_text(json.dumps({'Replace': replacements}))
        subprocess.run([go, 'test', '-overlay', str(overlay), './internal/lsp/lsproto',
                        '-run', '^TestRust(CodecMatrix|ParamsMatrix|ResolverMatchesPinnedGo)$', '-count=1'],
                       cwd=upstream / 'tsc', env=env, check=True, timeout=180)
        observed = json.loads(output.read_text())
        expected = HERE / 'codec-expected.json'
        if args.update:
            expected.write_text(json.dumps(observed, indent=2, sort_keys=True, ensure_ascii=False) + '\n')
        elif observed != json.loads(expected.read_text()):
            raise SystemExit('codec fixture differs from pinned Go; inspect before updating')
        params = json.loads(params_output.read_text())
        # The pinned JSON library deliberately chooses "cannot" or "unable to"
        # once per process (errors.go:errorModalVerb). Normalize only that
        # documented prefix; preserve every other byte of the response message.
        for response in params.values():
            error = response.get('error')
            if error and error['message'].startswith('InvalidParams: json: unable to '):
                error['message'] = error['message'].replace(
                    'InvalidParams: json: unable to ', 'InvalidParams: json: cannot ', 1)
        expected_params = HERE / 'params-expected.json'
        if args.update:
            expected_params.write_text(json.dumps(params, indent=2, sort_keys=True, ensure_ascii=False) + '\n')
        elif params != json.loads(expected_params.read_text()):
            raise SystemExit('parameter response fixture differs from pinned Go; inspect before updating')
        print(f'{len(params)} complete parameter responses checked')
        print(f'Pinned resolver matches; {sum(map(len, observed.values()))} codec operations checked')


if __name__ == '__main__':
    main()
