#!/usr/bin/env python3
"""Run the pinned `packages/typescript` test suites against a binary.

The pinned client resolves the server it spawns through `getExePath()`, which
in a repository checkout is `upstream/built/local/tsc`; the pin ignores
`/built`. Placing a binary there runs the untouched suites against it, with the
pinned Go binary for the native run and `tsrust` for ours. One variant is one
test file; one row is one test case, with the file as its explicit parent.
"""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
PACKAGE = ROOT / 'upstream/packages/typescript'
REPORTER = HERE / 'reporter.mjs'
SUITE = 'jsapi'


class HarnessError(RuntimeError):
    """The producer did not establish an attributable outcome."""


def pinned_node():
    """The Node the pin's `volta` entry names must be the one on PATH."""
    pin = json.loads((ROOT / 'upstream/package.json').read_text())['volta']['node']
    node = shutil.which('node')
    if not node:
        raise RuntimeError(f'put node {pin} on PATH')
    version = subprocess.check_output([node, '--version'], text=True).strip()
    if version != f'v{pin}':
        raise RuntimeError(f'the pinned client runs on node {pin}; found {version}')
    return node, pin


def test_files():
    files = sorted(path.relative_to(PACKAGE).as_posix()
                   for path in (PACKAGE / 'test').rglob('*.test.ts'))
    if not files:
        raise RuntimeError('the pinned client has no test files; initialize upstream')
    return files


def prepare(stage, binaries=None):
    """Record the Node, the binaries and the roster once; builds happen outside deadlines."""
    stage = Path(stage).resolve()
    stage.mkdir(parents=True, exist_ok=True)
    node, pin = pinned_node()
    if binaries is None:
        sys.path.insert(0, str(ROOT / 'scripts'))
        import phase5_replay_ci
        binaries = stage / 'binaries'
        phase5_replay_ci.prepare(binaries)
    binaries = Path(binaries).resolve()
    prepared = dict(suite=SUITE, stage=str(stage), node=node, node_version=pin,
                    native=str(binaries / 'native-lsp'), rust=str(binaries / 'rust-lsp'),
                    package=str(PACKAGE), cwd=str(ROOT / 'upstream'),
                    compiled_sources=test_files(),
                    goos=platform.system().lower(), goarch=platform.machine())
    _check_prepared(prepared)
    (stage / 'prepared.json').write_text(json.dumps(prepared, indent=2) + '\n')
    return prepared


def _check_prepared(prepared):
    if prepared.get('suite') != SUITE:
        raise ValueError('prepared suite does not match jsapi')
    for key in ('native', 'rust', 'node'):
        if not Path(prepared[key]).is_file():
            raise ValueError(f'prepared {key} is missing: {prepared[key]}')
    if not Path(prepared['package']).is_dir():
        raise ValueError('the pinned client package is missing; initialize upstream')
    # The two api.test.ts files import the bench module, which imports the
    # root workspace's tinybench and typescript: the pin's lockfile must be
    # installed (`npm ci --ignore-scripts` in upstream/; node_modules is ignored).
    if not (ROOT / 'upstream/node_modules/tinybench').is_dir():
        raise ValueError("run `npm ci --ignore-scripts` in upstream/ first: the client tests import tinybench")


def list_variants(suite, prepared):
    _check_prepared(prepared)
    current = test_files()
    if current != prepared['compiled_sources']:
        raise RuntimeError('the pinned test roster changed since preparation; prepare again')
    return [f'{SUITE}/{name}' for name in current]


def _place_binary(binary, link=None):
    """`getExePath()`'s repository path; a symlink the pin ignores."""
    link = Path(link) if link else ROOT / 'upstream/built/local/tsc'
    link.parent.mkdir(parents=True, exist_ok=True)
    if link.is_symlink() or link.exists():
        link.unlink()
    os.symlink(binary, link)


def _row(parent, suffix, state, reason='', detail=''):
    row = {'id': f'{SUITE}/{parent}' + suffix, 'parent': f'{SUITE}/{parent}', 'state': state}
    if reason:
        row['reason'] = reason[:2000]
    if detail:
        row['detail'] = detail[:8000]
    return row


def _run_file(prepared, name, local, timeout, binary):
    """One test file: node's per-file process under the reporter."""
    _place_binary(binary, prepared.get('exe_link'))
    command = [prepared['node'], '--conditions', '@typescript/source', '--test',
               f'--test-reporter={REPORTER}', '--test-reporter-destination=stdout',
               '--test-concurrency=1', str(Path(prepared['package']) / name)]
    stderr_path = local / (name.replace('/', '_') + '.stderr')
    with stderr_path.open('wb') as errors:
        process = subprocess.Popen(command, cwd=prepared['package'], stdout=subprocess.PIPE,
                                   stderr=errors, start_new_session=True, env=os.environ)
        lines = []
        def read():
            for line in process.stdout:
                lines.append(line)
        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        reader.join(timeout)
        timed_out = reader.is_alive()
        if timed_out:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        process.wait()
        reader.join(5)
        process.stdout.close()
    rows, path, started, finished = [], {}, {}, set()
    counts = {}
    file_failure = None
    output = []
    for raw in lines:
        try:
            event = json.loads(raw)
        except json.JSONDecodeError:
            continue
        kind = event.get('type')
        if kind == 'output':
            # The file's own stdout and stderr arrive as events; keep a tail
            # for the failure detail.
            output.append(event.get('message') or '')
            output = output[-200:]
            continue
        if kind not in ('test:start', 'test:pass', 'test:fail'):
            continue
        nesting = event.get('nesting')
        test_name = event.get('name')
        if not isinstance(nesting, int) or not isinstance(test_name, str):
            raise HarnessError(f'{name}: malformed reporter event {raw!r}')
        # A file whose process dies is reported by node as one test named
        # after the file; that is the file's failure, not a case.
        if nesting == 0 and test_name == event.get('file'):
            if kind == 'test:fail':
                file_failure = event.get('error') or 'test file failed'
            continue
        if kind == 'test:start':
            path[nesting] = test_name
            for deeper in [level for level in path if level > nesting]:
                del path[deeper]
            full = ' > '.join(path[level] for level in sorted(path))
            counts[full] = counts.get(full, 0) + 1
            if counts[full] > 1:
                full += f' #{counts[full]}'
            started[(nesting, test_name)] = full
            continue
        if event.get('kind') == 'suite':
            started.pop((nesting, test_name), None)
            continue
        full = started.pop((nesting, test_name), None)
        if full is None:
            raise HarnessError(f'{name}: terminal event before start: {test_name!r}')
        suffix = '/' + full.replace('/', '∕')
        if kind == 'test:fail':
            rows.append(_row(name, suffix, 'fail', 'assertion failed', event.get('error') or ''))
        elif event.get('skip') is not None or event.get('todo') is not None:
            rows.append(_row(name, suffix, 'skip', event.get('skip') or event.get('todo') or 'skipped'))
        else:
            rows.append(_row(name, suffix, 'pass'))
        finished.add(full)
    unfinished = [full for (nesting, _), full in started.items() if full not in finished]
    tail = (''.join(output) + stderr_path.read_text(errors='replace'))[-4000:]
    if timed_out:
        rows.append(_row(name, '', 'fail', f'deadline: {timeout} s', tail))
    elif file_failure is not None:
        rows.append(_row(name, '', 'fail', 'test file failed', file_failure + '\n' + tail))
    elif not rows and process.returncode:
        rows.append(_row(name, '', 'fail', f'worker exited {process.returncode} without results', tail))
    elif unfinished:
        for full in unfinished:
            rows.append(_row(name, '/' + full.replace('/', '∕'), 'fail', 'no terminal event', tail))
        rows.append(_row(name, '', 'fail', f'worker exited {process.returncode} with unfinished tests', tail))
    else:
        rows.append(_row(name, '', 'pass'))
    return rows


def run_batch(suite, prepared, variants, local, timeout, native=False, on_complete=None):
    """Rows for exact suite ids, publishing each file's rows as it finishes."""
    _check_prepared(prepared)
    available = set(list_variants(suite, prepared))
    names = [variant['id'] if isinstance(variant, dict) else variant for variant in variants]
    if len(set(names)) != len(names) or any(name not in available for name in names):
        raise ValueError('batch contains duplicate or unrouted test ids')
    local = Path(local).resolve()
    local.mkdir(parents=True, exist_ok=True)
    binary = prepared['native' if native else 'rust']
    rows, timings = [], {}
    for name in names:
        started = time.monotonic()
        file_rows = _run_file(prepared, name.split('/', 1)[1], local, timeout, binary)
        timings[name] = time.monotonic() - started
        rows.extend(file_rows)
        if on_complete:
            on_complete(file_rows)
    return rows, timings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('prepare', 'list', 'run'))
    parser.add_argument('--prepared', type=Path, required=True)
    parser.add_argument('--binaries', type=Path, help='a directory holding native-lsp and rust-lsp')
    parser.add_argument('--id', action='append', default=[])
    parser.add_argument('--local', type=Path)
    parser.add_argument('--timeout', type=float, default=600)
    parser.add_argument('--native', action='store_true')
    args = parser.parse_args()
    if args.command == 'prepare':
        print(json.dumps(prepare(args.prepared, args.binaries)))
        return
    prepared = json.loads((args.prepared / 'prepared.json').read_text())
    if args.command == 'list':
        print('\n'.join(list_variants(SUITE, prepared)))
        return
    if args.local is None:
        parser.error('run requires --local')
    rows, _ = run_batch(SUITE, prepared, args.id or list_variants(SUITE, prepared), args.local,
                        args.timeout, native=args.native,
                        on_complete=lambda rows: [print(json.dumps(row), flush=True) for row in rows])
    if any(row['state'] == 'fail' for row in rows):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
