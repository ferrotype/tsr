"""Bounded test2json supervision for retained Phase 5 test workers."""
from __future__ import annotations

import json
import os
from pathlib import Path
import queue
import re
import signal
import subprocess
import threading
import time
from urllib.parse import quote


class HarnessError(RuntimeError):
    """The event producer did not establish an attributable test outcome."""


class Events:
    def __init__(self, tests, suite):
        self.tests = set(tests)
        self.suite = suite
        self.active = None
        self.active_leaf = None
        self.parallel_waiters = set()
        self.running = {}
        self.elapsed = {test: 0.0 for test in tests}
        self.rows = {test: {} for test in tests}
        self.completed = set()
        self.seen = set()
        self.output = {}
        self.partial_output = {}
        self.package_failed = False
        self.last_terminal = None

    def parent(self, name):
        if not isinstance(name, str) or not name:
            raise HarnessError('missing test identity')
        parent = name.split('/', 1)[0]
        if parent not in self.tests:
            raise HarnessError(f'unrequested test identity: {name}')
        return parent

    def stop(self, parent, now):
        started = self.running.pop(parent, None)
        if started is not None:
            self.elapsed[parent] += now - started
        if self.active == parent:
            self.active = None
            self.active_leaf = None

    def row(self, parent, suffix, state, reason='', detail=''):
        if not isinstance(state, str) or state not in {'pass', 'fail', 'skip'}:
            raise HarnessError(f'invalid outcome: {state}')
        if not isinstance(reason, str) or not isinstance(detail, str):
            raise HarnessError('invalid reason or detail')
        identity = f'{self.suite}/{parent}' + suffix
        if identity in self.rows[parent]:
            raise HarnessError(f'duplicate result identity: {identity}')
        row = {'id': identity, 'parent': f'{self.suite}/{parent}', 'state': state}
        if reason:
            row['reason'] = reason
        if detail:
            row['detail'] = detail
        self.rows[parent][identity] = row

    def attributable_exit(self, returncode):
        """test2json synthesizes a named FAIL when the test process panics."""
        return bool(returncode and self.active is None and self.last_terminal
                    and self.last_terminal[1] == 'fail')

    def baseline_lines(self, name, output):
        """Discard ordinary logical lines while retaining bounded control lines."""
        prefix = 'TSR_BASELINE '
        pending = self.partial_output.get(name, '')
        parts = output.split('\n')
        for index, part in enumerate(parts):
            complete = index < len(parts) - 1
            if pending is not None:
                # Until the prefix is established retain at most its length.
                probe = pending + part[:max(0, len(prefix) - len(pending))]
                if not (prefix.startswith(probe) or probe.startswith(prefix)):
                    pending = None
                else:
                    pending += part
                    if len(pending) > 1048576:
                        raise HarnessError('baseline control line exceeds one MiB')
            if complete:
                if pending is not None and pending.startswith(prefix):
                    yield pending
                pending = ''
        self.partial_output[name] = pending

    def feed(self, event, now):
        if not isinstance(event, dict) or not isinstance(event.get('Action'), str):
            raise HarnessError('malformed test2json event')
        action = event['Action']
        name = event.get('Test')
        if action == 'output':
            output = event.get('Output')
            if not isinstance(output, str):
                raise HarnessError('malformed output event')
            if name is not None:
                self.parent(name)
                self.output[name] = (self.output.get(name, '') + output)[-65536:]
            for line in self.baseline_lines(name, output):
                try:
                    baseline = json.loads(line[len('TSR_BASELINE '):])
                    test, path, state = baseline['test'], baseline['path'], baseline['state']
                except (ValueError, KeyError, TypeError) as error:
                    raise HarnessError('malformed baseline event') from error
                parent = self.parent(test)
                if not isinstance(path, str) or not path or test not in self.seen or parent in self.completed:
                    raise HarnessError('invalid baseline identity or lifecycle')
                # Unframed stdout inherits test2json's current test, which can
                # lag a scheduler handoff. The reporter's explicit test owns it.
                suffix = '/baseline/' + quote(test, safe='') + '/' + quote(path, safe='')
                self.row(parent, suffix, state, baseline.get('reason', ''), baseline.get('detail', ''))
            return None
        if name is None:
            if action == 'fail':
                self.package_failed = True
            elif action not in {'start', 'pass'}:
                raise HarnessError(f'unattributed event: {action}')
            return None
        parent = self.parent(name)
        if action not in {'run', 'pause', 'cont', 'pass', 'fail', 'skip'}:
            raise HarnessError(f'unknown test event: {action}')
        if parent in self.completed:
            raise HarnessError(f'event after terminal parent: {name}')
        if action == 'run':
            if name in self.seen:
                raise HarnessError(f'duplicate run: {name}')
            self.seen.add(name)
        elif name not in self.seen:
            raise HarnessError(f'event before run: {name}')
        if action in {'run', 'cont'}:
            self.last_terminal = None
            if self.active is not None and self.active != parent:
                # testing releases a parallel parent's slot while it waits for
                # its parallel children, without issuing another parent pause.
                # CONT also precedes a departing test's delayed PASS output: it
                # is the slot handoff authority with -test.parallel=1.
                if action != 'cont' and self.active_leaf not in self.parallel_waiters:
                    raise HarnessError('concurrent active test leaves')
                self.stop(self.active, now)
            if name == parent and parent in self.running:
                raise HarnessError('duplicate active parent')
            if parent not in self.running:
                self.running[parent] = now
            self.active = parent
            self.active_leaf = name
        elif action == 'pause':
            if name == parent:
                if parent not in self.running:
                    raise HarnessError('pause without active parent')
                self.stop(parent, now)
            else:
                self.parallel_waiters.add(name.rsplit('/', 1)[0])
                if self.active_leaf == name:
                    self.active_leaf = name.rsplit('/', 1)[0]
        elif action in {'pass', 'fail', 'skip'}:
            if name == parent:
                self.stop(parent, now)
                self.row(parent, '', action, self.output.get(name, '').strip() if action == 'skip' else '', self.output.get(name, '') if action == 'fail' else '')
                self.completed.add(parent)
                self.last_terminal = (parent, action)
                return parent
            self.row(parent, '/subtest/' + quote(name[len(parent)+1:], safe=''), action, self.output.get(name, '').strip() if action == 'skip' else '', self.output.get(name, '') if action == 'fail' else '')
            if self.active_leaf == name:
                self.active_leaf = name.rsplit('/', 1)[0]
        return None


def _kill(process):
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    except PermissionError:
        # macOS can return EPERM for the vanished process group during exit.
        # Accept only an already exiting/reaped producer, never a live worker.
        try:
            process.wait(timeout=0.1)
        except subprocess.TimeoutExpired:
            raise
    process.wait()


def run_batch(command_prefix, tests, *, cwd, env, timeout, local, on_complete=None, suite='fourslash'):
    """Return (rows, elapsed seconds by explicit parent id), publishing completed parents."""
    tests = list(tests)
    if len(set(tests)) != len(tests) or any(not re.fullmatch(r'Test[^/\s]+', test) for test in tests):
        raise HarnessError('invalid or duplicate requested test identities')
    if timeout <= 0:
        raise ValueError('timeout must be positive')
    local = Path(local)
    local.mkdir(parents=True, exist_ok=True)
    remaining = list(tests)
    rows, elapsed = [], {}
    unidentified = 0
    attempt = 0
    while remaining:
        attempt += 1
        regex = '^(' + '|'.join(re.escape(test) for test in remaining) + ')$'
        command = list(command_prefix) + ['-test.run=' + regex, '-test.parallel=1', '-test.timeout=0', '-test.v=test2json']
        events = Events(remaining, suite)
        frames = queue.Queue(maxsize=256)
        stopping = threading.Event()
        with (local / f'worker-{attempt}.stderr').open('wb') as errors:
            process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=errors, start_new_session=True)
            def enqueue(value):
                while not stopping.is_set():
                    try:
                        frames.put(value, timeout=0.05)
                        return
                    except queue.Full:
                        continue
            def read():
                try:
                    with (local / f'worker-{attempt}.jsonl').open('wb') as raw:
                        while not stopping.is_set():
                            line = process.stdout.readline(1048577)
                            if not line:
                                break
                            raw.write(line)
                            if len(line) > 1048576:
                                enqueue(HarnessError('test2json frame exceeds one MiB'))
                                break
                            enqueue(line)
                except Exception as error:
                    enqueue(error)
                finally:
                    enqueue(None)
            reader = threading.Thread(target=read, daemon=True)
            reader.start()
            failure = None
            attributed_exit = False
            launched = time.monotonic()
            try:
                while True:
                    now = time.monotonic()
                    active = events.active
                    used = events.elapsed.get(active, 0) + (now - events.running[active] if active else 0)
                    if active and used >= timeout:
                        failure = 'test deadline exceeded'
                        break
                    if not active and now - launched >= timeout:
                        failure = 'worker stalled without an active test'
                        break
                    try:
                        line = frames.get(timeout=min(0.05, timeout))
                    except queue.Empty:
                        continue
                    if isinstance(line, Exception):
                        raise HarnessError(f'event reader failed: {line}') from line
                    if line is None:
                        try:
                            process.wait(timeout=1)
                        except subprocess.TimeoutExpired as error:
                            raise HarnessError('worker closed stdout without exiting') from error
                        if events.package_failed and not events.completed.symmetric_difference(events.tests) and all(events.rows[parent].get(f'{suite}/{parent}', {}).get('state') != 'fail' for parent in events.tests):
                            raise HarnessError('package failed without an attributable failed test')
                        attributed_exit = events.attributable_exit(process.returncode)
                        if events.completed == events.tests:
                            if process.returncode and not events.package_failed and not attributed_exit:
                                raise HarnessError('worker exited unsuccessfully after completed tests')
                            break
                        failure = f'worker exited before completion (status {process.returncode})'
                        break
                    try:
                        event = json.loads(line)
                    except (ValueError, UnicodeDecodeError) as error:
                        raise HarnessError('malformed test2json JSON') from error
                    previous_active = events.active
                    parent = events.feed(event, time.monotonic())
                    if events.active != previous_active:
                        launched = time.monotonic()
                    if parent:
                        finished = list(events.rows[parent].values())
                        rows.extend(finished)
                        elapsed[f'{suite}/{parent}'] = events.elapsed[parent]
                        remaining.remove(parent)
                        if on_complete:
                            on_complete(finished)
                        launched = time.monotonic()
                if failure:
                    active = events.active
                    if active:
                        events.stop(active, time.monotonic())
                        events.row(active, '', 'fail', failure)
                        finished = list(events.rows[active].values())
                        rows.extend(finished)
                        elapsed[f'{suite}/{active}'] = events.elapsed[active]
                        remaining.remove(active)
                        unidentified = 0
                        if on_complete:
                            on_complete(finished)
                    elif attributed_exit:
                        # The terminal failure was already published. Restart
                        # unfinished (including paused) tests without double rows.
                        unidentified = 0
                    else:
                        unidentified += 1
                        if unidentified >= 2:
                            raise HarnessError('two worker failures without an active test: ' + failure)
            finally:
                stopping.set()
                _kill(process)
                reader.join(timeout=1)
                process.stdout.close()
    return rows, elapsed
