#!/usr/bin/env python3
"""Untimed, matched checker operation counts. Never performance evidence.

Go uses an overlay. Rust sources are temporarily instrumented in the shared
checkout during the build, with backups written before the first change and
hash-checked restoration even on failure. Do not edit or build that checkout
concurrently. Executables, raw counts and restoration records stay outside target.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.phase7.bench import common, pgo, work_counts_go, work_counts_rust  # noqa: E402


def restore(directory, root=ROOT):
    """Never overwrite an intervening edit, including one to a new helper."""
    record = json.loads((directory / 'restore.json').read_text())
    conflicts = []
    for row in record:
        path = root / row['path']
        current = common.sha256(path.read_bytes()) if path.exists() else None
        if current == row['before_sha256']:
            continue
        if current != row['after_sha256']:
            conflicts.append(row['path'])
            continue
        if row['before_sha256'] is None:
            path.unlink()
        else:
            backup = directory / 'original' / row['path']
            if common.sha256(backup.read_bytes()) != row['before_sha256']:
                conflicts.append(row['path'] + ' (backup changed)')
                continue
            path.write_bytes(backup.read_bytes())
    if conflicts:
        raise ValueError('restoration refused intervening changes: ' + ', '.join(conflicts))


@contextmanager
def temporary_sources(edits, directory, root=ROOT):
    directory.mkdir(parents=True)  # Never replace the only recovery copy.
    record = []
    for relative, content in edits.items():
        path = root / relative
        if not path.resolve().is_relative_to(root.resolve()):
            raise ValueError('edit escapes source root: ' + relative)
        before = path.read_bytes() if path.exists() else None
        for name, data in (('original', before), ('instrumented', content.encode())):
            if data is not None:
                backup = directory / name / relative
                backup.parent.mkdir(parents=True, exist_ok=True)
                backup.write_bytes(data)
        record.append({'path': relative, 'before_sha256': common.sha256(before) if before is not None else None,
                       'after_sha256': common.sha256(content.encode())})
    pgo.write(directory / 'restore.json', record)
    try:
        for row in record:
            path = root / row['path']
            current = common.sha256(path.read_bytes()) if path.exists() else None
            if current != row['before_sha256']:
                raise ValueError('source changed before instrumentation: ' + row['path'])
            path.write_text(edits[row['path']])
        yield
    finally:
        restore(directory, root)


def fresh_directory(path):
    path = Path(path).resolve()
    if 'target' in path.parts:
        raise ValueError('diagnostic output must stay outside target')
    path.mkdir(parents=True)
    return path


def build(output, go, relation_origins=False, allow_dirty=False):
    output = fresh_directory(output)
    env = pgo.environment()
    if env.get('TSR_WORK_COUNTS'):
        raise ValueError('unset TSR_WORK_COUNTS before building')
    # Changes to other files are harmless; only refuse dirty build inputs.
    dirty = subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=all', '--',
                                     'crates', 'Cargo.toml', 'Cargo.lock', '.cargo', 'rust-toolchain.toml'],
                                    cwd=ROOT, text=True)
    untracked = '\n'.join(line for line in dirty.splitlines() if line.startswith('?? '))
    if untracked:
        raise ValueError('untracked build inputs cannot be preserved in source.patch; '
                         'track them before instrumenting: ' + untracked)
    if dirty and not allow_dirty:
        raise ValueError('commit or preserve source edits before instrumenting: ' + dirty)
    source = pgo.source_identity()
    pin = subprocess.check_output(['git', 'rev-parse', 'HEAD:upstream'], cwd=ROOT, text=True).strip()
    upstream = ROOT / 'upstream/tsc'
    actual = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=upstream, text=True).strip()
    upstream_dirty = subprocess.check_output(['git', 'status', '--porcelain'], cwd=upstream, text=True)
    if actual != pin or upstream_dirty:
        raise ValueError('upstream must be the clean gitlink')
    go_env = {**env, 'GOTOOLCHAIN': 'local', 'GOWORK': 'off', 'GOFLAGS': '-mod=readonly', 'CGO_ENABLED': '1'}
    go_version = subprocess.check_output([go, 'version'], env=go_env, text=True).strip()
    # Toolchain authority lives in S04's pin, not the optional inventory field.
    import tomllib
    versions = tomllib.loads((ROOT / 'data/s04/toolchains.toml').read_text())
    expected_go = versions['go']
    if ('go' + expected_go.removeprefix('go')) not in go_version.split():
        raise ValueError('unexpected Go toolchain: ' + go_version)
    manifest = {'format': 1, 'purpose': 'untimed operation counts', 'complete': False,
                'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                'pin': pin, 'sources': source, 'go_version': go_version,
                'counter_order': list(work_counts_go.COUNTERS), 'relation_origins': relation_origins,
                'source_status': dirty}
    patch = subprocess.check_output(['git', 'diff', '--binary', 'HEAD', '--', 'crates', 'Cargo.toml', 'Cargo.lock',
                                     '.cargo', 'rust-toolchain.toml'], cwd=ROOT)
    (output / 'source.patch').write_bytes(patch)
    manifest['source_patch_sha256'] = common.sha256(patch)
    pgo.write(output / 'build.json', manifest)
    overlay = {}
    for relative, content in work_counts_go.edits(upstream, relation_origins=relation_origins).items():
        backing = output / 'go-overlay' / relative
        backing.parent.mkdir(parents=True, exist_ok=True)
        backing.write_text(content)
        overlay[str(upstream / relative)] = str(backing)
    pgo.write(output / 'go-overlay.json', {'Replace': overlay})
    result = pgo.execute([go, 'build', '-trimpath', '-mod=readonly', '-overlay', str(output / 'go-overlay.json'),
                          '-o', str(output / 'tsgo'), './cmd/tsc'], upstream, go_env, output / 'go-build')
    if result['exit']:
        raise ValueError('Go diagnostic build failed; see go-build.stderr')
    channel, host = pgo.benchmark.rust_native_toolchain(env, ROOT)
    command = ['cargo', '+' + channel, 'build', '--locked', '--release', '--target', host,
               '-p', 'tsrust', '--bin', 'tsrust', '--message-format=json-render-diagnostics',
               *pgo.benchmark.release_configuration(env, ROOT)]
    manifest['rustc'] = subprocess.check_output(['rustc', '+' + channel, '-vV'], text=True)
    with temporary_sources(work_counts_rust.edits(ROOT, relation_origins=relation_origins), output / 'rust-overlay'):
        result = pgo.execute(command, ROOT, env, output / 'rust-build')
        if result['exit']:
            raise ValueError('Rust diagnostic build failed; see rust-build.stderr')
        selected = pgo.artifact(result['stdout'])
        shutil.copy2(selected['executable'], output / 'tsrust')
        manifest['cargo_artifact'] = selected
    if source != pgo.source_identity():
        raise ValueError('source identity changed after restoration')
    manifest.update(complete=True, binaries={name: common.identity(output / name) for name in ('tsgo', 'tsrust')},
                    instrumentation={str(path.relative_to(output)): common.sha256(path.read_bytes())
                                     for folder in ('go-overlay', 'rust-overlay/instrumented')
                                     for path in sorted((output / folder).rglob('*')) if path.is_file()})
    pgo.write(output / 'build.json', manifest)
    return manifest


def capture(build_directory, output, cache, scenarios):
    previous = common.CACHE
    try:
        return _capture(build_directory, output, cache, scenarios)
    finally:
        common.CACHE = previous


def _capture(build_directory, output, cache, scenarios):
    output = fresh_directory(output)
    build_directory = Path(build_directory).resolve()
    manifest = json.loads((build_directory / 'build.json').read_text())
    if not manifest['complete']:
        raise ValueError('diagnostic build is incomplete')
    common.CACHE = Path(cache).resolve()
    env = pgo.environment()
    env.pop('TSR_WORK_COUNTS', None)
    report = {'format': 1, 'purpose': 'untimed operation counts; timings are not benchmark results',
              'build': common.identity(build_directory / 'build.json'), 'complete': False, 'scenarios': {}}
    pgo.write(output / 'report.json', report)
    for name in scenarios:
        descriptor = common.load(name)
        root = common.checkout(descriptor)
        rows = {}
        report['scenarios'][name] = rows
        for runtime, executable in (('go', 'tsgo'), ('rust', 'tsrust')):
            binary = build_directory / executable
            if common.identity(binary) != manifest['binaries'][executable]:
                raise ValueError('diagnostic executable changed')
            prefix = output / (name + '-' + runtime)
            before = pgo.preflight(binary, descriptor, 1, env, prefix.with_name(prefix.name + '-before'))
            counts_path = prefix.with_suffix('.counts.json')
            result = pgo.execute([str(binary), *common.arguments(descriptor, 'check', 1)], root,
                                 {**env, 'TSR_WORK_COUNTS': str(counts_path)}, prefix, timeout=600)
            facts = common.observe(result, root)
            issues = common.problems(facts, descriptor['work']['check'], 'check', runtime)
            if result['stderr']:
                issues.append('unexpected stderr')
            counts = json.loads(counts_path.read_text())
            if counts.get('format') != 1 or set(counts.get('counters', {})) != set(manifest['counter_order']):
                issues.append('counter inventory differs')
            if any(type(value) is not int or value < 0 for value in counts['counters'].values()):
                issues.append('invalid counter value')
            if 'type_created' in counts['counters'] and counts['counters']['type_created'] != facts['types']:
                issues.append('type creation counter differs from compiler diagnostics')
            after = pgo.preflight(binary, descriptor, 1, env, prefix.with_name(prefix.name + '-after'))
            if before != after or facts['listed_paths'] != before['listed_paths']:
                issues.append('inputs, options or file order changed')
            rows[runtime] = {'facts': facts, 'counters': counts['counters'], 'inputs': before, 'issues': issues}
            if manifest.get('relation_origins'):
                origins = counts.get('relation_origins', {})
                if any(type(value) is not int or value < 0 for value in origins.values()) or sum(origins.values()) != counts['counters']['relation_request']:
                    issues.append('relation origins do not account for every request')
                rows[runtime]['relation_origins'] = origins
            pgo.write(output / 'report.json', report)
            if issues:
                raise ValueError(name + ' ' + runtime + ': ' + '; '.join(issues))
        if rows['go']['inputs'] != rows['rust']['inputs']:
            raise ValueError(name + ': runtime inputs differ')
        rows['comparison'] = {key: {'go': rows['go']['counters'][key], 'rust': rows['rust']['counters'][key],
                                    'delta': rows['rust']['counters'][key] - rows['go']['counters'][key]}
                              for key in manifest['counter_order']}
    report['complete'] = True
    pgo.write(output / 'report.json', report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    make = commands.add_parser('build')
    make.add_argument('--output', required=True)
    make.add_argument('--go', required=True)
    make.add_argument('--relation-origins', action='store_true')
    make.add_argument('--allow-dirty', action='store_true',
                      help='archive and preserve tracked uncommitted build inputs; untracked inputs are rejected')
    run = commands.add_parser('capture')
    run.add_argument('--build', required=True)
    run.add_argument('--output', required=True)
    run.add_argument('--cache', required=True)
    run.add_argument('--scenario', choices=common.NAMES, action='append', required=True)
    recover = commands.add_parser('restore')
    recover.add_argument('directory', type=Path)
    args = parser.parse_args()
    if args.command == 'build':
        build(args.output, args.go, args.relation_origins, args.allow_dirty)
    elif args.command == 'capture':
        capture(args.build, args.output, args.cache, args.scenario)
    else:
        restore(args.directory)


if __name__ == '__main__':
    main()
