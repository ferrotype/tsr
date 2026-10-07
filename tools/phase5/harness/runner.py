#!/usr/bin/env python3
"""Prepare and supervise the pinned Phase 5 native test adapter."""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib

if __package__:
    from . import inventory, overlay, supervisor
else:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import inventory
    import overlay
    import supervisor

ROOT = Path(__file__).resolve().parents[3]
PACKAGES = {'fourslash': 'fourslash/tests', 'lsp': 'lsp'}
LSP_TESTS = {
    'TestCompletionAfterFileClose', 'TestCompletionWithConcurrentFileClose',
    'TestCompletionForUnopenedFile', 'TestAutoImportCompletionForUnopenedFile',
    'TestCompletionSnapshotFreezing', 'TestSetContentMapperContributionsBeforeDidOpen',
    'TestProgressNotificationsEndToEnd', 'TestInitializeCodeActionKinds',
    'TestProjectInfoConfiguredProject', 'TestProjectInfoInferredProject',
    'TestReferencesAfterAncestorProjectConfigDeletion1', 'TestSemanticTokensCRLF',
    'TestSemanticTokensDefaultLibraryCaseInsensitive',
}


def _go_env(stage):
    return {**os.environ, 'GOTOOLCHAIN': 'local', 'GOWORK': 'off',
            'GOFLAGS': '-mod=readonly', 'GOCACHE': os.environ.get('GOCACHE', str(ROOT / 'target/go-build'))}


def _pinned_go(stage):
    expected = tomllib.loads((ROOT / 'data/s04/toolchains.toml').read_text())['go']
    candidate = shutil.which('go')
    candidates = [candidate] if candidate else []
    candidates.append(str(Path.home() / '.local/share/mise/installs/go' / expected.removeprefix('go') / 'bin/go'))
    for candidate in dict.fromkeys(candidates):
        if not Path(candidate).is_file():
            continue
        version = subprocess.check_output([candidate, 'env', 'GOVERSION'], env=_go_env(stage), text=True).strip()
        if version == expected:
            return str(Path(candidate).resolve())
    raise RuntimeError(f'put the pinned {expected} on PATH (automatic Go downloads are disabled)')


def _source_selection(go, suite, stage):
    env = _go_env(stage)
    package = subprocess.run(
        [go, 'list', '-json', './internal/' + PACKAGES[suite]],
        cwd=ROOT / 'upstream/tsc', env=env, check=True,
        stdout=subprocess.PIPE, text=True)
    info = json.loads(package.stdout)
    host = subprocess.run([go, 'env', '-json', 'GOOS', 'GOARCH'], env=env,
                          check=True, stdout=subprocess.PIPE, text=True)
    host = json.loads(host.stdout)
    files = info.get('TestGoFiles', []) + info.get('XTestGoFiles', [])
    if not files:
        raise RuntimeError('go list selected no test source files')
    sources = sorted(str((Path(info['Dir']) / filename).resolve().relative_to(ROOT))
                     for filename in files)
    return dict(compiled_sources=sources, goos=host['GOOS'], goarch=host['GOARCH'])


def prepare(suite, stage, release=True, *, prebuilt_server=None, prebuilt_test2json=None):
    """Build once outside test deadlines; return serializable absolute paths."""
    if suite not in PACKAGES:
        raise ValueError(f'unknown suite: {suite}')
    stage = Path(stage).resolve()
    stage.mkdir(parents=True, exist_ok=True)
    go = _pinned_go(stage)
    patch = overlay.create(stage / 'overlay')
    binary = stage / f'{suite}-tests'
    test2json = stage / 'test2json'
    upstream = ROOT / 'upstream/tsc'
    env = _go_env(stage)
    selection = _source_selection(go, suite, stage)
    subprocess.run([go, 'test', '-overlay', str(patch), '-c', '-o', str(binary),
                    './internal/' + PACKAGES[suite]], cwd=upstream, env=env, check=True)
    if prebuilt_test2json is None:
        subprocess.run([go, 'build', '-o', str(test2json), 'cmd/test2json'], cwd=upstream, env=env, check=True)
    else:
        test2json = Path(prebuilt_test2json).resolve()
        if not test2json.is_file():
            raise ValueError('prebuilt test2json is missing')
    if prebuilt_server is None:
        command = ['cargo', 'build', '--locked', '-p', 'tsr_testhost', '--bin', 'phase5_testserver', '--message-format=json']
        if release:
            command.append('--release')
        result = subprocess.run(command, cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True)
        artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        servers = [item['executable'] for item in artifacts
                   if item.get('reason') == 'compiler-artifact'
                   and item.get('target', {}).get('name') == 'phase5_testserver'
                   and item.get('executable')]
        if len(servers) != 1:
            raise RuntimeError(f'expected one private server artifact, got {servers}')
        server = Path(servers[0]).resolve()
    else:
        server = Path(prebuilt_server).resolve()
        if not server.is_file():
            raise ValueError('prebuilt private server is missing')
    prepared = dict(suite=suite, stage=str(stage), go=go, binary=str(binary),
                    test2json=str(test2json), server=str(server), cwd=str(upstream))
    prepared.update(selection)
    # Validate the compiled roster before saving reusable preparation metadata.
    list_variants(suite, prepared)
    (stage / 'prepared.json').write_text(json.dumps(prepared, indent=2) + '\n')
    return prepared


def _check_prepared(suite, prepared):
    if suite not in PACKAGES or prepared.get('suite') != suite:
        raise ValueError('prepared suite does not match requested suite')
    for key in ('binary', 'test2json', 'server'):
        if not Path(prepared[key]).is_file():
            raise ValueError(f'prepared {key} is missing: {prepared[key]}')


def list_variants(suite, prepared):
    """Compiled lists establish membership; source inventory detects drift."""
    _check_prepared(suite, prepared)
    result = subprocess.run([prepared['binary'], '-test.list=^Test'], cwd=prepared['cwd'],
                            env=_runtime_env(prepared, None, native=True), check=True,
                            stdout=subprocess.PIPE, text=True)
    names = [line.strip() for line in result.stdout.splitlines() if re.fullmatch(r'Test\w+', line.strip())]
    if len(names) != len(set(names)):
        raise RuntimeError('compiled roster contains duplicate tests')
    sources = set(prepared.get('compiled_sources', []))
    if not sources:
        raise ValueError('prepared metadata lacks go-list source selection; prepare again')
    expected = {row['name'] for row in inventory.collect(ROOT)
                if row['package'] == PACKAGES[suite] and row['source'].rsplit(':', 1)[0] in sources}
    if set(names) != expected:
        raise RuntimeError(f'compiled/source roster disagreement: missing={sorted(expected-set(names))}, extra={sorted(set(names)-expected)}')
    if suite == 'lsp':
        if not LSP_TESTS <= set(names):
            raise RuntimeError('compiled LSP roster lacks a routed client test')
        names = [name for name in names if name in LSP_TESTS]
    return [f'{suite}/{name}' for name in sorted(names)]


def _runtime_env(prepared, local, native):
    env = _go_env(prepared['stage'])
    # An inherited transport or fault selector must not contaminate native runs.
    for key in ('TSR_LSP_SERVER', 'TSR_FAULT', 'TSR_BASELINE_LOCAL', 'TSGO_BASELINE_TRACKING_DIR', 'TSR_UPSTREAM_ROOT'):
        env.pop(key, None)
    env['TSR_UPSTREAM_ROOT'] = prepared['cwd']
    if not native:
        env['TSR_LSP_SERVER'] = prepared['server']
    if local is not None:
        env['TSR_BASELINE_LOCAL'] = str(Path(local) / 'baselines')
        env['TSGO_BASELINE_TRACKING_DIR'] = str(Path(local) / 'tracking')
    return env


def run_batch(suite, prepared, variants, local, timeout, native=False, on_complete=None):
    """Run exact suite ids, returning (result rows, parent timing seconds)."""
    _check_prepared(suite, prepared)
    available = set(list_variants(suite, prepared))
    variants = [variant['id'] if isinstance(variant, dict) else variant for variant in variants]
    if len(set(variants)) != len(variants) or any(variant not in available for variant in variants):
        raise ValueError('batch contains duplicate or unrouted test ids')
    local = Path(local).resolve()
    for name in ('baselines', 'tracking', 'events'):
        (local / name).mkdir(parents=True, exist_ok=True)
    command = [prepared['test2json'], '-t', '-p', 'internal/' + PACKAGES[suite], prepared['binary']]
    groups = [[variant] for variant in variants] if suite == 'lsp' else [variants]
    rows, timings = [], {}
    for index, group in enumerate(groups):
        events = local / 'events' / str(index) if suite == 'lsp' else local / 'events'
        result, measured = supervisor.run_batch(
            command, [variant.split('/', 1)[1] for variant in group],
            cwd=prepared['cwd'], env=_runtime_env(prepared, local, native),
            timeout=timeout, local=events, on_complete=on_complete, suite=suite)
        rows.extend(result)
        timings.update(measured)
    return rows, timings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('prepare', 'list', 'batch', 'run'))
    parser.add_argument('suite', choices=tuple(PACKAGES))
    parser.add_argument('--prepared', type=Path, required=True, help='reusable preparation directory')
    parser.add_argument('--debug', action='store_true', help='build debug Rust server instead of release')
    parser.add_argument('--ids', type=Path, help='newline-delimited suite ids for batch')
    parser.add_argument('--id', help='single suite id for run')
    parser.add_argument('--local', type=Path, help='isolated result/baseline directory')
    parser.add_argument('--timeout', type=float, default=120)
    parser.add_argument('--native', action='store_true')
    parser.add_argument('--timings', type=Path)
    args = parser.parse_args()
    if args.command == 'prepare':
        print(json.dumps(prepare(args.suite, args.prepared, release=not args.debug)))
        return
    prepared = json.loads((args.prepared / 'prepared.json').read_text())
    if args.command == 'list':
        for identity in list_variants(args.suite, prepared):
            print(identity)
        return
    if args.local is None:
        parser.error('run and batch require --local')
    if args.command == 'run':
        if not args.id:
            parser.error('run requires --id')
        variants = [args.id]
    else:
        if args.ids is None:
            parser.error('batch requires --ids')
        variants = [line.strip() for line in args.ids.read_text().splitlines() if line.strip()]
    def publish(rows):
        for row in rows:
            print(json.dumps(row), flush=True)
    rows, timings = run_batch(args.suite, prepared, variants, args.local, args.timeout,
                              native=args.native, on_complete=publish)
    if args.timings:
        args.timings.write_text(json.dumps(timings, indent=2) + '\n')
    if any(row['state'] == 'fail' for row in rows):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
