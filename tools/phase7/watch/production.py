#!/usr/bin/env python3
"""Measure ordinary CLI --watch RSS on an isolated project, without test hooks."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.phase7.watch import run as shared

COMPLETE = re.compile(rb'Found (\d+) errors?\. Watching for file changes\.\r?\n')


def edited_source(original, generation):
    # A fixed-width, deliberately unresolved name witnesses that this exact edit
    # reached the checker. Comments alone cannot establish this without hooks.
    token = f'__phase7_watch_missing_{generation:08d}'
    suffix = f'\n{token};\n'.encode(shared.source_encoding(original))
    split = len(original)
    if shared.source_encoding(original) != 'utf-8' and split % 2:
        split -= 1
    return original[:split] + suffix + original[split:], token


class Completions:
    def __init__(self, log, process, timeout, root):
        self.log, self.process, self.timeout = log, process, timeout
        self.root = root.resolve()
        self.offset = 0

    def next(self, token):
        deadline = time.monotonic() + self.timeout
        while time.monotonic() < deadline:
            with self.log.open('rb') as stream:
                stream.seek(self.offset)
                pending = stream.read()
            match = COMPLETE.search(pending)
            if match:
                block = pending[:match.end()]
                start = self.offset
                self.offset += len(block)
                witness = f"error TS2304: Cannot find name '{token}'.".encode()
                if witness not in block:
                    raise ValueError(f'completed watch cycle did not check {token}; inspect compiler.log')
                files = [line.decode('utf-8') for line in block.splitlines()
                         if line.startswith((b'/', b'bundled:///libs/')) and line.endswith(
                             (b'.ts', b'.tsx', b'.mts', b'.cts', b'.js', b'.jsx', b'.mjs', b'.cjs', b'.json'))]
                for file in files:
                    if not file.startswith('bundled:///libs/') and not Path(file).resolve(strict=True).is_relative_to(self.root):
                        raise ValueError(f'compiler source escaped isolated copy: {file}')
                return {'diagnostics': int(match[1]), 'source_witness': token,
                        'log_start': start, 'log_end': self.offset,
                        'program_files': len(files),
                        'file_roster_sha256': hashlib.sha256(json.dumps(files).encode()).hexdigest(),
                        **shared.sample_os(self.process.pid)}
            if self.process.poll() is not None:
                raise ValueError(f'watch exited {self.process.returncode} before completion; inspect compiler.log')
            time.sleep(0.05)
        raise TimeoutError(f'no completed watch cycle for {token} in {self.timeout}s')


def capture(args):
    project, compiler = args.project.resolve(strict=True), args.compiler.resolve(strict=True)
    output = args.output.resolve()
    if output.exists() or output.is_relative_to(project) or 'target' in output.parts:
        raise ValueError('output must be new, outside the project and target')
    if not 2 <= args.warmup < args.cycles <= 1000 or args.timeout <= 0:
        raise ValueError('require 2 <= warmup < cycles <= 1000 and positive timeout')
    for relative in (args.config, args.edit_file):
        file = project / relative
        if relative.is_absolute() or '..' in relative.parts or not file.is_file() or file.is_symlink() or not file.resolve().is_relative_to(project):
            raise ValueError(f'expected a regular contained project file: {relative}')
    if args.edit_file.suffix not in ('.ts', '.tsx', '.mts', '.cts'):
        raise ValueError('edit-file must be an included TypeScript source')
    before = shared.inventory(project)
    output.mkdir(parents=True)
    copied = output / 'project'
    shared.isolated_copy(project, copied, before, args.copy_mode)
    edit = copied / args.edit_file
    original = edit.read_bytes()
    edit.chmod(edit.stat().st_mode | 0o200)
    command = [str(compiler), '-p', str(copied / args.config), '--watch', '--pretty', 'false',
               '--preserveWatchOutput', '--noEmit', '--incremental', 'false', '--composite', 'false',
               '--runExternalCode', 'false', '--noCheck', 'false', '--listFiles', '--locale', 'en-us']
    report = {'schema': 1, 'status': 'running', 'command': command,
              'compiler_sha256': shared.digest(compiler), 'source_inventory': before,
              'original_project': str(project), 'edited_file': str(args.edit_file),
              'copy_mode': args.copy_mode, 'warmup_cycles': args.warmup, 'cycles': args.cycles,
              'memory_scope': 'ordinary production CLI --watch; no testing hooks or retained probe snapshots',
              'source_witness': 'fixed-width unique unresolved name; one TS2304 per completed generation',
              'rss_is_not_a_leak_or_teardown_gate': True, 'samples': []}
    process = None
    try:
        with (output / 'compiler.log').open('xb') as log:
            raw, token = edited_source(original, 0)
            edit.write_bytes(raw)
            process = subprocess.Popen(command, cwd=copied, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            completions = Completions(output / 'compiler.log', process, args.timeout, copied)
            report['ready'] = ready = completions.next(token)
            if not ready['program_files'] or (args.expected_program_files is not None and ready['program_files'] != args.expected_program_files):
                raise ValueError(f'initial file roster count differs: {ready["program_files"]}')
            for cycle in range(1, args.cycles + 1):
                raw, token = edited_source(original, cycle)
                edit.write_bytes(raw)
                sample = completions.next(token)
                sample.update(cycle=cycle, edited_file_sha256=hashlib.sha256(raw).hexdigest())
                report['samples'].append(sample)
                if any(sample[key] != ready[key] for key in ('diagnostics', 'program_files', 'file_roster_sha256')):
                    raise ValueError(f'cycle {cycle} changed diagnostics count or file roster')
            steady = report['samples'][args.warmup:]
            report['rss_trend_after_warmup'] = shared.trend(steady)
            descriptors = [row['descriptor_count'] for row in steady]
            report['descriptor_spread_after_warmup'] = max(descriptors) - min(descriptors)
            os.killpg(process.pid, signal.SIGINT)
            report['exit_code'] = process.wait(timeout=args.timeout)
            if report['exit_code'] not in (0, 130, -signal.SIGINT):
                raise ValueError(f'watch failed on shutdown: {report["exit_code"]}')
        report['original_project_unchanged'] = shared.inventory(project) == before
        if not report['original_project_unchanged'] or shared.digest(compiler) != report['compiler_sha256']:
            raise ValueError('original project or compiler changed during capture')
        report['status'] = 'complete'
        return 0
    except BaseException as error:
        report.update(status='failed', error=f'{type(error).__name__}: {error}')
        raise
    finally:
        if process is not None:
            # The group may outlive its leader; never leave descendants behind.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=10)
        report['process_reaped'] = process is not None and process.poll() is not None
        (output / 'watch.json').write_text(json.dumps(report, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', type=Path, required=True)
    parser.add_argument('--config', type=Path, default=Path('tsconfig.json'))
    parser.add_argument('--edit-file', type=Path, required=True)
    parser.add_argument('--compiler', type=Path, required=True, help='prebuilt ordinary release tsrust')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cycles', type=int, default=50)
    parser.add_argument('--warmup', type=int, default=5)
    parser.add_argument('--timeout', type=float, default=120)
    parser.add_argument('--copy-mode', choices=['bytes', 'clone'], default='bytes')
    parser.add_argument('--expected-program-files', type=int)
    return capture(parser.parse_args())


if __name__ == '__main__':
    raise SystemExit(main())
