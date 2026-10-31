#!/usr/bin/env python3
"""Save one ordinary release tsrust executable and its exact build inputs.

This builds neither Go nor a benchmark workload. Run pairs.py separately with
the saved executable and build.json. Output must be a new directory outside
target; experimental features are explicit and no PGO flags are injected.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.phase7.bench import common, pgo  # noqa: E402

# Deliberately closed: new experiments must opt in here after checking that
# they do not instrument the measured compiler. Retain historical selectors
# so their saved build manifests remain comparable after promotion/removal.
ORDINARY_FEATURES = frozenset(('contextual-owner-scope', 'compact-node-read'))


def validate_features(features, *, resolved=False):
    if not isinstance(features, (list, tuple)) or not all(isinstance(feature, str) for feature in features):
        raise ValueError('Cargo features must be a list of names')
    allowed = ORDINARY_FEATURES | {'default'} if resolved else ORDINARY_FEATURES
    unknown = sorted(set(features) - allowed)
    if unknown:
        raise ValueError(f'non-ordinary or instrumented Cargo features: {unknown}')


def workspace_features(artifacts):
    resolved = {}
    for row in artifacts:
        if row.get('reason') != 'compiler-artifact' or not row.get('manifest_path'):
            continue
        manifest = Path(row['manifest_path']).resolve()
        if manifest.is_relative_to(ROOT):
            key = str(manifest.relative_to(ROOT))
            resolved[key] = sorted(set(resolved.get(key, ())) | set(row.get('features', ())))
    if not resolved:
        raise ValueError('Cargo reported no resolved workspace artifact features')
    for features in resolved.values():
        validate_features(features, resolved=True)
    return resolved


def build(output, features=()):
    output = Path(output).resolve()
    if 'target' in output.parts:
        raise ValueError('save the executable outside target')
    output.mkdir(parents=True)
    record = {'complete': False, 'pgo': False, 'features': list(features)}
    def save():
        pgo.write(output / 'build.json', record)
    save()
    try:
        validate_features(features)
        env = pgo.environment()
        release = pgo.benchmark.release_configuration(env, ROOT)
        channel, host = pgo.benchmark.rust_native_toolchain(env, ROOT)
        toolchain = {'channel': channel, 'host': host,
                     'rustc': subprocess.check_output(['rustc', '+' + channel, '-vV'], cwd=ROOT, env=env, text=True)}
        sources = pgo.source_identity()
        configuration = pgo.benchmark.cargo_configuration(env)
        command = ['cargo', '+' + channel, 'build', '--release', '--locked', '--target', host,
                   '-p', 'tsrust', '--bin', 'tsrust', '--message-format=json-render-diagnostics', *release]
        if features:
            command += ['--features', ','.join(features)]
        record.update(revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                      sources=sources, cargo_configuration=configuration, release_configuration=release,
                      toolchain=toolchain, command=command, build_tool=common.identity(Path(__file__)))
        save()
        result = pgo.execute(command, ROOT, env, output / 'cargo')
        record['exit'] = result['exit']
        if result['exit'] != 0:
            raise ValueError(f"ordinary compiler build exited {result['exit']}")
        if sources != pgo.source_identity() or configuration != pgo.benchmark.cargo_configuration(env):
            raise ValueError('source or Cargo configuration changed during the build')
        if common.identity(Path(__file__)) != record['build_tool']:
            raise ValueError('build helper changed during the build')
        artifacts = [json.loads(line) for line in result['stdout'].splitlines() if line.startswith('{')]
        record['workspace_features'] = workspace_features(artifacts)
        artifacts = [row for row in artifacts if row.get('reason') == 'compiler-artifact'
                     and row.get('target', {}).get('name') == 'tsrust'
                     and row['target'].get('kind') == ['bin'] and row.get('executable')]
        if len(artifacts) != 1:
            raise ValueError('expected exactly one tsrust executable artifact')
        artifact = artifacts[0]
        shutil.copy2(artifact['executable'], output / 'tsrust')
        record.update(complete=True, source_stable=True, cargo_artifact=artifact,
                      binary=common.identity(output / 'tsrust'))
        save()
        return record
    except BaseException as error:
        record['error'] = str(error) or type(error).__name__
        save()
        raise


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='new archive directory outside target')
    parser.add_argument('--features', nargs='*', default=[], help='explicit Cargo features for the candidate')
    args = parser.parse_args(argv)
    try:
        record = build(args.output, args.features)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, str(error) + '\n')
    print(json.dumps({'complete': record['complete'], 'binary': record['binary'], 'features': record['features']}))


if __name__ == '__main__':
    main()
