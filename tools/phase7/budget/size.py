#!/usr/bin/env python3
"""Capture production consumer body sizes and declare runtime glue separately.

No build or compiler execution occurs. The Go native binary must be stripped;
Rust must be the ordinary public embedding build. Rust wasm must be production
checker wasm, never the portable compiler acceptance artifact. Surfaces share
parse/config/load/check/emit, but Rust wasm also exports transpile and other
public value operations; this is a production-surface comparison, not proof of
identical exports. The threshold ratio uses body bytes, with glue and totals
reported independently.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.phase7.budget import measure, probe  # noqa: E402

SCOPE = {
    'common': 'production parse/config/load/check/optional emit; one checker; builtin libraries',
    'native': 'standalone public tsr_embed consumer versus pinned Go production consumer',
    'wasm': 'public production tsr_wasm checker versus pinned Go production consumer',
    'extra_rust_wasm_surface': 'single-file module/declaration transpile and public AST/value operations',
    'excluded': 'test-only compiler acceptance wasm, test walker, allocator instrumentation',
    'ratio': 'Rust body bytes / Go body bytes; runtime glue reported separately',
}


def new_output(path):
    path = Path(path).resolve()
    if any(part.lower() == 'target' for part in path.parts) or path.exists():
        raise ValueError('size output must be new and outside target')
    return path


def identities(runtime, go, rust, go_build, rust_build):
    if runtime not in ('native', 'wasm'):
        raise ValueError('unknown size runtime')
    for path in (go, rust, go_build, rust_build):
        if any(part.lower() == 'target' for part in Path(path).resolve(strict=True).parts):
            raise ValueError('size artifacts and manifests must be durable outside target')
    result = {engine: measure.build_identity(runtime, engine, Path(artifact), Path(build))
              for engine, artifact, build in (('go', go, go_build), ('rust', rust, rust_build))}
    if not all(value['production_eligible'] for value in result.values()):
        raise ValueError('size capture requires eligible production builds')
    if runtime == 'native':
        manifest = probe.decode(Path(go_build).read_bytes())
        if not any(isinstance(command, list) and command[:2] == ['go', 'build'] and
                   '-ldflags=-s -w' in command and './cmd/phase7-budget/native' in command
                   for command in manifest.get('commands', [])):
            raise ValueError('native Go size requires the stripped production build')
    return result


def byte_counts(runtime, bound):
    result = {}
    for engine in ('go', 'rust'):
        identity = bound[engine]
        if runtime == 'native':
            body = next(item for item in identity['files'] if item['path'] == identity['executable'])['bytes']
            glue = 0
        else:
            body_name, glue_name = ('budget-go.wasm', 'wasm_exec.cjs') if engine == 'go' else ('checker_bg.wasm', 'bindings.mjs')
            def size(name):
                rows = [item for item in identity['files'] if Path(item['path']).name == name]
                if len(rows) != 1:
                    raise ValueError('missing or ambiguous size asset: ' + name)
                return rows[0]['bytes']
            body, glue = size(body_name), size(glue_name)
        measure.positive(body, 'body bytes')
        result[engine] = {'body': body, 'glue': glue, 'total': body + glue}
    return result


def capture(runtime, go, rust, go_build, rust_build, output):
    output = new_output(output)
    bound = identities(runtime, go, rust, go_build, rust_build)
    for identity in bound.values():
        measure.unchanged(identity)
    output.mkdir(parents=True)
    saved = {}
    for engine in ('go', 'rust'):
        path = output / (engine + '-build.json')
        path.write_bytes(Path(bound[engine]['manifest']['path']).read_bytes())
        saved[engine] = probe.identity(path)
        if saved[engine]['sha256'] != bound[engine]['manifest']['sha256']:
            raise ValueError('build metadata changed during archive')
    report = {'version': 1, 'runtime': runtime, 'recorded_at': datetime.now(timezone.utc).isoformat(),
              'production_eligible': True, 'scope': SCOPE, 'artifacts': bound,
              'saved_manifests': saved, 'artifact_bytes': byte_counts(runtime, bound),
              'host': measure.host_info()}
    for identity in bound.values():
        measure.unchanged(identity)
    probe.write(output / 'size.json', report)
    return read_capture(output, runtime)


def read_capture(directory, runtime):
    """Authenticate immutable manifests/assets and recompute, never build/run."""
    if runtime not in ('native', 'wasm'):
        raise ValueError('unknown size runtime')
    directory = Path(directory).resolve(strict=True)
    report = probe.decode((directory / 'size.json').read_bytes())
    if (type(report.get('version')) is not int or report['version'] != 1 or
            report.get('runtime') != runtime or report.get('production_eligible') is not True or
            report.get('scope') != SCOPE):
        raise ValueError('invalid production size capture')
    bound = report['artifacts']
    # The archived manifests authenticate body/glue bytes. Capture-time helper
    # and source identities remain provenance; future helper edits cannot make
    # a historical size observation stale. No Node or compiler is run here.
    for engine in ('go', 'rust'):
        identity = probe.identity(directory / (engine + '-build.json'))
        saved = report['saved_manifests'][engine]
        if identity != saved or identity['sha256'] != bound[engine]['manifest']['sha256']:
            raise ValueError('saved build metadata changed')
        manifest = probe.decode((directory / (engine + '-build.json')).read_bytes())
        if (bound[engine].get('runtime') != runtime or bound[engine].get('engine') != engine or
                bound[engine].get('production_eligible') is not True or bound[engine].get('eligibility_reasons') != []):
            raise ValueError('invalid captured size eligibility')
        if engine == 'go' or runtime == 'native':
            measure.require_budget_manifest(manifest, engine)
        if engine == 'go':
            pin_rows = [item for item in bound[engine]['files'] if item['path'].endswith('/data/upstream.json')]
            if (len(pin_rows) != 1 or manifest.get('sources', {}).get('data/upstream.json') != pin_rows[0]['sha256']
                    or not isinstance(manifest.get('pin'), str) or len(manifest['pin']) != 40):
                raise ValueError('Go size manifest lacks its captured pinned source identity')
        if runtime == 'native':
            if engine == 'go':
                if not any(isinstance(command, list) and command[:2] == ['go', 'build'] and
                           '-ldflags=-s -w' in command and './cmd/phase7-budget/native' in command
                           for command in manifest.get('commands', [])):
                    raise ValueError('native Go size requires the stripped production build')
                expected = measure.artifact_entry(manifest, 'budget-go')
            else:
                command = manifest.get('build_command')
                if (not isinstance(command, list) or command[:2] != ['cargo', 'build'] or
                        not measure.command_pair(command, '--manifest-path', 'tools/phase7/budget/rust/Cargo.toml') or
                        manifest.get('allocation_instrumented') is not False or manifest.get('allocator') != measure.ALLOCATOR or
                        manifest.get('source_changes_during_build') != [] or
                        manifest.get('profile') != {'name': 'release', 'panic': 'unwind', 'lto': 'fat', 'codegen_units': 1} or
                        type(manifest.get('profile', {}).get('codegen_units')) is not int):
                    raise ValueError('native Rust size requires the ordinary public embedding release')
                expected = {'bytes': manifest.get('size'), 'sha256': manifest.get('sha256')}
            actual = measure.bound_asset(bound[engine]['executable'], expected)
            if actual not in bound[engine]['files']:
                raise ValueError('native size asset differs from captured identity')
        else:
            if engine == 'rust':
                command = manifest.get('command')
                if (manifest.get('mode') != 'checker' or not measure.command_pair(command, '-p', 'tsr_wasm') or
                        not measure.command_pair(command, '--features', 'checker') or '--release' not in command or
                        not manifest.get('sources')):
                    raise ValueError('wasm size requires production tsr_wasm checker')
            names = ('budget-go.wasm', 'wasm_exec.cjs') if engine == 'go' else ('checker_bg.wasm', 'bindings.mjs')
            for name in names:
                rows = [item for item in bound[engine]['files'] if Path(item['path']).name == name]
                if len(rows) != 1:
                    raise ValueError('missing or ambiguous size asset: ' + name)
                actual = measure.bound_asset(rows[0]['path'], measure.artifact_entry(manifest, name))
                if actual != rows[0]:
                    raise ValueError('wasm size asset differs from captured identity')
    counts = byte_counts(runtime, bound)
    if counts != report['artifact_bytes']:
        raise ValueError('saved size differs from artifact bytes')
    return {'ratios': {'artifact_bytes': counts['rust']['body'] / counts['go']['body']},
            'samples': {engine: {'artifact_bytes': [counts[engine]['body']]} for engine in ('go', 'rust')},
            'host': report['host'], 'recorded_at': report['recorded_at'],
            'metadata': {'runtime': runtime, 'scope': SCOPE, 'artifact_bytes': counts,
                         'artifacts': bound, 'saved_manifests': report['saved_manifests'],
                         'production_eligible': True, 'eligibility_reasons': []}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', choices=('native', 'wasm'), required=True)
    for name in ('go', 'rust', 'go-build', 'rust-build', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    output = new_output(args.output)
    try:
        result = capture(args.runtime, args.go, args.rust, args.go_build, args.rust_build, output)
    except Exception as error:
        # Only this invocation can have created the previously absent output.
        if output.is_dir():
            probe.write(output / 'failure.json', {'complete': False, 'phase': 'size-capture',
                        'error': str(error), 'recorded_at': datetime.now(timezone.utc).isoformat()})
        raise
    print(result['ratios'])


if __name__ == '__main__':
    main()
