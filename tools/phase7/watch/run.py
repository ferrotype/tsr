#!/usr/bin/env python3
"""Copy an isolated project and observe production-command native watch cycles."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

def digest(path):
    result = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            result.update(chunk)
    return result.hexdigest()


def inventory(root):
    """Do not follow links, import .git, or permit a copied link to escape."""
    root = root.resolve(strict=True)
    result = []
    for directory, dirs, files in os.walk(root, followlinks=False):
        dirs[:] = sorted(name for name in dirs if name != '.git')
        for name in sorted(set(dirs + files) - {'.git'}):
            path = Path(directory) / name
            relative = str(path.relative_to(root))
            if path.is_symlink():
                # Provisioned pnpm workspaces can contain intentionally dangling
                # links to unbuilt packages. Preserve them only inside the copy.
                target = path.resolve(strict=False)
                if not target.is_relative_to(root):
                    raise ValueError(f'external symlink cannot be isolated: {relative}')
                result.append({'path': relative, 'link': str(target.relative_to(root)),
                               'dangling': not target.exists()})
            elif path.is_file():
                result.append({'path': relative, 'sha256': digest(path), 'bytes': path.stat().st_size})
            elif not path.is_dir():
                raise ValueError(f'special filesystem entry cannot be copied: {relative}')
    return sorted(result, key=lambda item: item['path'])


def clone_file(source, destination):
    """Require the macOS clonefile operation; never silently hardlink/fallback."""
    if sys.platform != 'darwin':
        raise ValueError('clone copy mode requires macOS/APFS')
    import ctypes
    native = ctypes.CDLL(None, use_errno=True)
    clone = native.clonefile
    clone.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_int]
    clone.restype = ctypes.c_int
    if clone(os.fsencode(source), os.fsencode(destination), 0) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code), str(source))
    shutil.copystat(source, destination)
    return destination


def isolated_copy(source, destination, entries, mode='bytes'):
    shutil.copytree(source, destination, symlinks=True,
                    ignore=shutil.ignore_patterns('.git'),
                    copy_function=clone_file if mode == 'clone' else shutil.copy2)
    # Remap every symlink, including absolute links that originally stayed
    # inside the provisioned project. No hardlinks to source files are created.
    for entry in entries:
        if 'link' in entry:
            path = destination / entry['path']
            path.unlink()
            path.symlink_to(os.path.relpath(destination / entry['link'], path.parent))
    if inventory(destination) != entries:
        raise ValueError('isolated copy differs from the source inventory')


def sample_os(pid):
    rss = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(pid)], text=True, timeout=15).strip()) * 1024
    if sys.platform.startswith('linux'):
        descriptors = len(list(Path(f'/proc/{pid}/fd').iterdir()))
        method = '/proc/PID/fd'
    elif sys.platform == 'darwin':
        raw = subprocess.check_output(['lsof', '-nP', '-a', '-p', str(pid), '-Ff'], text=True, timeout=15)
        descriptors = sum(line[1:].isdigit() for line in raw.splitlines() if line.startswith('f'))
        method = 'lsof numeric file descriptors'
    else:
        raise ValueError('descriptor sampling supports Linux and macOS only')
    return {'rss_bytes': rss, 'descriptor_count': descriptors,
            'descriptor_method': method, 'rss_method': 'ps rss KiB',
            'monotonic_seconds': time.monotonic()}


class Reports:
    def __init__(self, path, process, timeout):
        self.path, self.process, self.timeout = path, process, timeout
        self.offset = 0

    def next(self, phase):
        deadline = time.monotonic() + self.timeout
        while time.monotonic() < deadline:
            if self.path.exists():
                with self.path.open('rb') as source:
                    source.seek(self.offset)
                    line = source.readline()
                if line.endswith(b'\n'):
                    self.offset += len(line)
                    value = json.loads(line)
                    if value.get('phase') != phase:
                        raise ValueError(f'expected {phase} report, got {value}')
                    value.update(sample_os(self.process.pid))
                    return value
            if self.process.poll() is not None:
                raise ValueError(f'probe exited {self.process.returncode} before {phase}; inspect compiler.log')
            time.sleep(0.05)
        raise TimeoutError(f'probe did not report {phase} in {self.timeout} seconds')


def send(process, command):
    process.stdin.write((command + '\n').encode())
    process.stdin.flush()


def trend(samples):
    values = [sample['rss_bytes'] for sample in samples]
    indices = list(range(len(values)))
    centre = (len(values) - 1) / 2
    denominator = sum((index - centre) ** 2 for index in indices)
    mean = sum(values) / len(values)
    slope = (sum((index - centre) * (value - mean) for index, value in zip(indices, values))
             / denominator if denominator else 0)
    return {'first_bytes': values[0], 'last_bytes': values[-1], 'maximum_bytes': max(values),
            'least_squares_bytes_per_cycle': slope, 'leak_gate': False}


def capture(args):
    project = args.project.resolve(strict=True)
    output = args.output.resolve()
    if not project.is_dir() or output.exists() or output.is_relative_to(project) or 'target' in output.parts:
        raise ValueError('output must be new, outside the project and every target directory')
    if args.cycles <= args.warmup or args.warmup < 2 or args.cycles > 1000 or args.timeout <= 0:
        raise ValueError('require 2 <= warmup < cycles <= 1000 and a positive cycle timeout')
    config_relative, edit_relative = args.config, args.edit_file
    for relative in (config_relative, edit_relative):
        if relative.is_absolute() or '..' in relative.parts:
            raise ValueError('config and edit-file must be paths inside project')
        path = project / relative
        if not path.is_file() or path.is_symlink() or not path.resolve().is_relative_to(project):
            raise ValueError(f'expected a regular contained file: {relative}')
    if edit_relative.suffix not in ('.ts', '.tsx', '.mts', '.cts'):
        raise ValueError('edit-file must be an included TypeScript source')
    before = inventory(project)
    output.mkdir(parents=True)
    copied = output / 'project'
    isolated_copy(project, copied, before, args.copy_mode)
    edit = copied / edit_relative
    # An ancestor may have been a contained directory symlink. Edit only a path
    # whose fully resolved copy remains isolated, never an original inode.
    if not edit.resolve(strict=True).is_relative_to(copied):
        raise ValueError('copied edit path escaped isolation')
    original = edit.read_bytes()
    edit.chmod(edit.stat().st_mode | 0o200)

    def apply(generation):
        text = original + f'\n// phase7-watch-equivalent-edit: {generation:08d}\n'.encode()
        edit.write_bytes(text)
        return hashlib.sha256(text).hexdigest()

    metadata = {'schema': 1, 'status': 'prepared', 'original_project': str(project),
                'isolated_project': str(copied), 'config': str(config_relative),
                'edited_file': str(edit_relative), 'cycles': args.cycles,
                'warmup_cycles': args.warmup, 'timeout_seconds_per_phase': args.timeout,
                'descriptor_plateau_slack': args.descriptor_slack,
                'copy_mode': args.copy_mode,
                'expected_program_files': args.expected_program_files,
                'scope': 'production command/compiler/OsSystem/native backend; controlled cycle scheduler via passive testing hooks',
                'source_inventory': before, 'samples': [],
                'rss_is_not_a_leak_or_teardown_gate': True}
    (output / 'prepared.json').write_text(json.dumps(metadata, indent=2) + '\n')
    apply(0)
    if args.action == 'prepare':
        print(output / 'prepared.json')
        return 0
    probe = args.probe.resolve(strict=True)
    metadata['probe'] = {'path': str(probe), 'sha256': digest(probe)}
    process = None
    try:
        with (output / 'compiler.log').open('xb') as log:
            process = subprocess.Popen([str(probe), str(copied / config_relative), str(edit),
                                        str(output / 'probe.jsonl'), str(args.timeout)],
                                       cwd=copied, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT)
            reports = Reports(output / 'probe.jsonl', process, args.timeout)
            metadata['baseline'] = reports.next('baseline')
            send(process, 'start')
            ready = reports.next('ready')
            if ready.get('native_error'):
                raise ValueError(f'native backend error: {ready["native_error"]}')
            if ready['source_sha256'] != digest(edit) or ready['program_files'] == 0:
                raise ValueError('initial actual compiler source observation differs')
            if args.expected_program_files is not None and ready['program_files'] != args.expected_program_files:
                raise ValueError(f'expected {args.expected_program_files} program files, observed {ready["program_files"]}')
            metadata['ready'] = ready
            for cycle in range(1, args.cycles + 1):
                expected = apply(cycle)
                send(process, 'cycle')
                sample = reports.next('cycle')
                sample['cycle'] = cycle
                metadata['samples'].append(sample)
                if sample.get('native_error'):
                    raise ValueError(f'native backend error: {sample["native_error"]}')
                if sample['source_sha256'] != expected or sample['superseded_file_released'] is not True:
                    raise ValueError(f'cycle {cycle} failed new-source/old-root checks')
                if sample['program_files'] != ready['program_files']:
                    raise ValueError(f'cycle {cycle} changed project file count')
            send(process, 'close')
            closed = reports.next('closed')
            metadata['closed'] = closed
            send(process, 'finish')
            process.stdin.close()
            if process.wait(timeout=args.timeout) != 0:
                raise ValueError(f'probe failed with exit {process.returncode}')
        steady = metadata['samples'][args.warmup:]
        counts = {(value['owners'], value['storage_allocations']) for value in steady}
        descriptors = [value['descriptor_count'] for value in steady]
        metadata['gates'] = {
            'tracked_owners_and_storage_plateau': len(counts) == 1,
            'superseded_roots_released': all(value['superseded_file_released'] for value in metadata['samples']),
            'tracked_teardown': closed['owners'] == 0 and closed['storage_allocations'] == 0 and closed['last_file_released'],
            'descriptor_plateau': max(descriptors) - min(descriptors) <= args.descriptor_slack,
        }
        metadata['rss_trend_after_warmup'] = trend(steady)
        metadata['descriptor_after_close_minus_before_watch'] = closed['descriptor_count'] - metadata['baseline']['descriptor_count']
        metadata['original_project_unchanged'] = inventory(project) == before
        if not metadata['original_project_unchanged']:
            raise ValueError('source project changed during capture; result cannot attest source stability')
        if not all(metadata['gates'].values()):
            raise ValueError(f'resource gate failed: {metadata["gates"]}')
        metadata['status'] = 'pass'
        return 0
    except BaseException as error:
        metadata['status'] = 'failed'
        metadata['error'] = f'{type(error).__name__}: {error}'
        raise
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        (output / 'watch.json').write_text(json.dumps(metadata, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'run'])
    parser.add_argument('--project', type=Path, required=True, help='entire isolated-copy source root, including workspace dependencies')
    parser.add_argument('--config', type=Path, default=Path('tsconfig.json'))
    parser.add_argument('--edit-file', type=Path, required=True, help='included TS source relative to project')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--probe', type=Path, help='prebuilt phase7-live-watch executable (run only)')
    parser.add_argument('--copy-mode', choices=['bytes', 'clone'], default='bytes',
                        help='clone requires macOS clonefile; no fallback or hardlinks')
    parser.add_argument('--cycles', type=int, default=50)
    parser.add_argument('--warmup', type=int, default=5)
    parser.add_argument('--timeout', type=int, default=120)
    parser.add_argument('--descriptor-slack', type=int, default=2)
    parser.add_argument('--expected-program-files', type=int,
                        help='optional frozen workload roster count; mismatch refuses capture')
    args = parser.parse_args()
    if args.action == 'run' and args.probe is None:
        parser.error('run requires a prebuilt --probe; builds are deliberately separate')
    if args.descriptor_slack < 0:
        parser.error('descriptor-slack must be nonnegative')
    return capture(args)


if __name__ == '__main__':
    raise SystemExit(main())
