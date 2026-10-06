#!/usr/bin/env python3
"""Build ordinary CLIs once and compare the bounded replay matrix in CI."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from tools.phase5.harness import runner
from tools.phase5.replay import replay

SESSIONS = ('checkjs', 'monorepo', 'references')
ENCODINGS = ('utf-8', 'utf-16')


def prepare(output):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    go = runner._pinned_go(output)
    subprocess.run([go, 'build', '-o', str(output / 'native-lsp'), './cmd/tsc'],
                   cwd=ROOT / 'upstream/tsc', env=runner._go_env(output), check=True)
    result = subprocess.run(
        ['cargo', 'build', '--release', '--locked', '-p', 'tsrust', '--bin', 'tsrust', '--message-format=json'],
        cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True)
    artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    executables = [item['executable'] for item in artifacts
                   if item.get('reason') == 'compiler-artifact'
                   and item.get('target', {}).get('name') == 'tsrust' and item.get('executable')]
    if len(executables) != 1:
        raise RuntimeError(f'expected one ordinary tsrust artifact, got {executables}')
    shutil.copy2(executables[0], output / 'rust-lsp')


def run_matrix(binaries, output):
    binaries, output = Path(binaries).resolve(), Path(output).resolve()
    commands = {}
    for runtime in ('native', 'rust'):
        executable = binaries / f'{runtime}-lsp'
        if not executable.is_file():
            raise ValueError(f'missing ordinary CLI: {executable}')
        executable.chmod(executable.stat().st_mode | 0o111)
        commands[runtime] = [str(executable), '--lsp', '--stdio']
    output.mkdir(parents=True, exist_ok=True)
    rows = []
    for session in SESSIONS:
        for encoding in ENCODINGS:
            case = output / session / encoding
            transcripts, errors = {}, {}
            for runtime in ('native', 'rust'):
                local = case / runtime
                local.mkdir(parents=True, exist_ok=True)
                try:
                    transcripts[runtime] = replay.run(
                        replay.HOME / 'sessions' / f'{session}.jsonl', commands[runtime], local, encoding)
                except Exception as error:
                    errors[runtime] = f'{type(error).__name__}: {error}'
                    (local / 'error.txt').write_text(errors[runtime] + '\n')
            matched = not errors and replay.compare(
                transcripts['native'], transcripts['rust'], case / 'mismatch')
            row = dict(session=session, encoding=encoding, matched=matched, errors=errors)
            rows.append(row)
            print(json.dumps(row), flush=True)
    (output / 'summary.json').write_text(json.dumps(rows, indent=2) + '\n')
    return all(row['matched'] for row in rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    build = sub.add_parser('prepare')
    build.add_argument('--output', type=Path, required=True)
    run = sub.add_parser('run')
    run.add_argument('--binaries', type=Path, required=True)
    run.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.action == 'prepare':
        prepare(args.output)
    elif not run_matrix(args.binaries, args.output):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
