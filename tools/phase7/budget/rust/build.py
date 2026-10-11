#!/usr/bin/env python3
"""Build a reproducible public native budget consumer into a durable directory.

The ordinary executable is for elapsed/RSS/size. Allocation instrumentation
wraps the same allocator in a separate artifact and cannot qualify those rows.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
MANIFEST = 'tools/phase7/budget/rust/Cargo.toml'
sys.path.insert(0, str(ROOT / 'scripts'))
from s10_build import build_environment  # noqa: E402


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_snapshot(environment):
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--locked', '--offline', '--format-version=1',
        '--manifest-path', MANIFEST], cwd=ROOT, env=environment))
    files = {HERE / 'Cargo.lock', HERE / 'Cargo.toml', Path(__file__),
             ROOT / 'Cargo.toml', ROOT / 'Cargo.lock', ROOT / 'rust-toolchain.toml',
             ROOT / 'data/upstream.json', ROOT / 'scripts/s10_build.py', ROOT / 'scripts/s04_ownership.py'}
    for package in metadata['packages']:
        if package['source'] is not None:
            continue
        directory = Path(package['manifest_path']).parent
        files.add(directory / 'Cargo.toml')
        files.update(path for child in ('src', 'bundled')
                     for path in (directory / child).rglob('*') if path.is_file())
        if (directory / 'build.rs').is_file():
            files.add(directory / 'build.rs')
    files.update(path for path in (ROOT / '.cargo').rglob('*') if path.is_file())
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(files)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--allocation-metrics', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() or any(part.casefold() == 'target' for part in output.parts):
        parser.error('artifacts require a new directory outside every target ancestor')
    environment = build_environment()
    subprocess.run(['cargo', 'fetch', '--locked', '--manifest-path', MANIFEST],
                   cwd=ROOT, env=environment, check=True)
    before = source_snapshot(environment)
    command = ['cargo', 'build', '--offline', '--locked', '--release', '--manifest-path', MANIFEST,
               '--target-dir', 'target', '--message-format=json-render-diagnostics']
    if args.allocation_metrics:
        command += ['--features', 'allocation-metrics']
    output.mkdir(parents=True)
    with (output / 'build.log').open('wb') as log:
        result = subprocess.run(command, cwd=ROOT, env=environment, stdout=subprocess.PIPE, stderr=log)
    (output / 'cargo-messages.ndjson').write_bytes(result.stdout)
    result.check_returncode()
    artifacts = [row for line in result.stdout.splitlines()
                 if (row := json.loads(line)).get('reason') == 'compiler-artifact'
                 and row['target']['name'] == 'phase7_rust_budget' and row.get('executable')]
    if len(artifacts) != 1:
        raise ValueError('Cargo did not identify exactly one native budget executable')
    executable = output / 'phase7_rust_budget'
    shutil.copy2(artifacts[0]['executable'], executable)
    after = source_snapshot(environment)
    if before != after:
        raise ValueError('budget consumer sources changed during build; retained artifact is not qualified')
    record = {'version': 1, 'mode': 'phase7-budget', 'measurement_contract': 2,
              'performance_eligible': not args.allocation_metrics,
              'source_guard': True, 'source_changes_during_build': [], 'sources': before,
              'allocation_instrumented': args.allocation_metrics, 'allocator': 'mimalloc-0.1.48',
              'profile': {'name': 'release', 'panic': 'unwind', 'lto': 'fat', 'codegen_units': 1},
              'pin': json.loads((ROOT / 'data/upstream.json').read_bytes())['pin'],
              'versions': {name: subprocess.check_output([name, '--version'], cwd=ROOT,
                                                         env=environment, text=True).strip()
                           for name in ('rustc', 'cargo')},
              'build_command': command, 'build_exit': 0,
              'executable': str(executable), 'size': executable.stat().st_size,
              'sha256': digest(executable)}
    (output / 'build.json').write_text(json.dumps(record, indent=2) + '\n')
    print(output)


if __name__ == '__main__':
    main()
