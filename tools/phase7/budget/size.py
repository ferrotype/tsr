#!/usr/bin/env python3
"""Capture production consumer body sizes and declare runtime glue separately.

No build or compiler execution occurs. Native capture strips private copies of
both executables with the same tool and policy; originals remain untouched.
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
import shutil
import subprocess
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
STRIP_FLAGS = ['--strip-all']
JS_ADAPTERS = {
    'go': ('tools/phase7/budget/go/wasm.mjs',),
    'rust': ('tools/phase7/budget/production-wasm.mjs',
             'tools/s10/wasm/instance.mjs', 'tools/s10/wasm/imports.mjs'),
}


def strip_native(bound, output, strip_tool=None):
    """Normalize both linked artifacts without replacing the timed executables."""
    # Apple's strip rejects the Go linker's indirect symbol entries. Use LLVM
    # for both files, including on Darwin, and record the exact tool identity.
    tool = strip_tool or shutil.which('llvm-strip')
    if tool is None:
        raise ValueError('native size capture requires --strip-tool /path/to/llvm-strip')
    tool = probe.identity(Path(tool).resolve(strict=True))
    flags = STRIP_FLAGS
    record = {'policy': 'llvm-strip-all-v1', 'platform': sys.platform,
              'tool': tool, 'flags': flags, 'artifacts': {}}
    for engine in ('go', 'rust'):
        source = Path(bound[engine]['executable'])
        destination = output / (engine + '-stripped')
        original = probe.identity(source)
        shutil.copy2(source, destination)
        if probe.identity(destination)['sha256'] != original['sha256']:
            raise ValueError('native executable changed while copying')
        command = [tool['path'], *flags, str(destination)]
        if probe.identity(tool['path']) != tool:
            raise ValueError('strip tool changed during capture')
        result = subprocess.run(command, capture_output=True, check=False, timeout=60)
        if probe.identity(tool['path']) != tool:
            raise ValueError('strip tool changed during capture')
        if result.returncode:
            raise ValueError(f'{engine} strip exited {result.returncode}: '
                             + result.stderr.decode('utf-8', errors='replace'))
        record['artifacts'][engine] = {
            'source': original, 'output': probe.identity(destination),
            'command': command, 'exit': result.returncode,
            'stdout': result.stdout.decode('utf-8', errors='replace'),
            'stderr': result.stderr.decode('utf-8', errors='replace'),
        }
    return record


def stripped_counts(directory, bound, record):
    """Verify the saved normalization; replay never invokes strip or a compiler."""
    if (not isinstance(record, dict) or record.get('policy') != 'llvm-strip-all-v1' or
            record.get('flags') != STRIP_FLAGS):
        raise ValueError('native size requires matching stripping on both artifacts')
    result = {}
    for engine in ('go', 'rust'):
        row = record['artifacts'][engine]
        original = next(item for item in bound[engine]['files']
                        if item['path'] == bound[engine]['executable'])
        path = directory / (engine + '-stripped')
        if (row['source'] != original or row['output']['path'] != str(path) or
                type(row.get('exit')) is not int or row['exit'] != 0 or
                row['command'] != [record['tool']['path'], *record['flags'], str(path)]):
            raise ValueError('native stripping provenance differs from captured artifacts')
        actual = measure.bound_asset(path, row['output'])
        body = measure.positive(actual['bytes'], 'stripped body bytes')
        result[engine] = {'body': body, 'glue': 0, 'total': body}
    return result


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
            for relative in JS_ADAPTERS[engine]:
                path = str(ROOT / relative)
                rows = [item for item in identity.get('helpers', []) if item['path'] == path]
                if len(rows) != 1 or rows[0] not in identity['files']:
                    raise ValueError('missing or ambiguous deployment JS adapter: ' + relative)
                glue += rows[0]['bytes']
        measure.positive(body, 'body bytes')
        result[engine] = {'body': body, 'glue': glue, 'total': body + glue}
    return result


def capture(runtime, go, rust, go_build, rust_build, output, strip_tool=None):
    output = new_output(output)
    bound = identities(runtime, go, rust, go_build, rust_build)
    for identity in bound.values():
        measure.unchanged(identity)
    output.mkdir(parents=True)
    if runtime == 'wasm':
        measure.archive_helpers(output, [], bound)
    saved = {}
    for engine in ('go', 'rust'):
        path = output / (engine + '-build.json')
        path.write_bytes(Path(bound[engine]['manifest']['path']).read_bytes())
        saved[engine] = probe.identity(path)
        if saved[engine]['sha256'] != bound[engine]['manifest']['sha256']:
            raise ValueError('build metadata changed during archive')
    normalization = strip_native(bound, output, strip_tool) if runtime == 'native' else None
    counts = (stripped_counts(output, bound, normalization) if runtime == 'native'
              else byte_counts(runtime, bound))
    report = {'version': 2,
              'runtime': runtime, 'recorded_at': datetime.now(timezone.utc).isoformat(),
              'revision': measure.paired_revision(bound),
              'production_eligible': True, 'scope': SCOPE, 'artifacts': bound,
              'saved_manifests': saved, 'artifact_bytes': counts,
              'host': measure.host_info()}
    if normalization is not None:
        report['native_stripping'] = normalization
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
    if report.get('version') == 1:
        raise ValueError('legacy size capture lacks matched stripping or complete JS accounting; recapture sizes')
    if (type(report.get('version')) is not int or report['version'] != 2 or
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
        if bound[engine].get('revision') != measure.artifact_revision(manifest):
            raise ValueError('captured artifact revision differs from its build manifest')
        if bound[engine].get('dirty') != manifest.get('dirty'):
            raise ValueError('captured artifact dirty provenance differs from its build manifest')
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
            for relative in JS_ADAPTERS[engine]:
                rows = [item for item in bound[engine].get('helpers', []) if item['path'] == str(ROOT / relative)]
                if len(rows) != 1:
                    raise ValueError('missing deployment JS adapter: ' + relative)
                actual = probe.identity(measure.captured_file(directory, 'helpers/' + relative))
                if any(actual[key] != rows[0][key] for key in ('bytes', 'sha256')):
                    raise ValueError('archived deployment JS adapter differs: ' + relative)
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
    if report.get('revision') != measure.paired_revision(bound):
        raise ValueError('captured revision differs from the built artifacts')
    counts = (stripped_counts(directory, bound, report.get('native_stripping')) if runtime == 'native'
              else byte_counts(runtime, bound))
    if counts != report['artifact_bytes']:
        raise ValueError('saved size differs from artifact bytes')
    return {'ratios': {'artifact_bytes': counts['rust']['body'] / counts['go']['body']},
            'samples': {engine: {'artifact_bytes': [counts[engine]['body']]} for engine in ('go', 'rust')},
            'host': report['host'], 'recorded_at': report['recorded_at'], 'revision': report.get('revision'),
            'metadata': {'runtime': runtime, 'scope': SCOPE, 'artifact_bytes': counts,
                         'artifacts': bound, 'saved_manifests': report['saved_manifests'],
                         **({'native_stripping': report['native_stripping']} if runtime == 'native' else {}),
                         'production_eligible': True, 'eligibility_reasons': []}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', choices=('native', 'wasm'), required=True)
    parser.add_argument('--strip-tool', type=Path, help='LLVM strip executable for native sizes')
    for name in ('go', 'rust', 'go-build', 'rust-build', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    output = new_output(args.output)
    try:
        result = capture(args.runtime, args.go, args.rust, args.go_build, args.rust_build, output, args.strip_tool)
    except Exception as error:
        # Only this invocation can have created the previously absent output.
        if output.is_dir():
            probe.write(output / 'failure.json', {'complete': False, 'phase': 'size-capture',
                        'error': str(error), 'recorded_at': datetime.now(timezone.utc).isoformat()})
        raise
    print(result['ratios'])


if __name__ == '__main__':
    main()
