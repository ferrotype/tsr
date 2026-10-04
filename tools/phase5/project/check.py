#!/usr/bin/env python3
"""Small L1 comparison using the pinned state writer, not a new formatter.

The first native run proves the JSON projection renders exactly like direct
snapshot access. A second run feeds the Rust endpoint's projections to that
same writer. No captures, acceptance metrics or freshness registries are made.
"""
import difflib
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading
import time
import tomllib

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent


def reader(process, frames):
    try:
        while True:
            headers = {}
            while (line := process.stdout.readline()) != b'\r\n':
                if not line:
                    raise EOFError('private endpoint closed stdout')
                key, value = line.split(b':', 1)
                headers[key] = value.strip()
            length = int(headers[b'Content-Length'])
            data = process.stdout.read(length)
            if len(data) != length:
                raise EOFError('partial frame')
            frames.put(json.loads(data))
    except Exception as error:
        frames.put(error)


def rust_states(fixture, repetitions=1):
    process = subprocess.Popen([str(ROOT / 'target/debug/phase5_testserver'), '--stdio'],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    frames = queue.Queue()
    thread = threading.Thread(target=reader, args=(process, frames), daemon=True)
    thread.start()
    next_id = 0
    def send(method, params, request=False):
        nonlocal next_id
        message = {'jsonrpc': '2.0', 'method': method, 'params': params}
        if request:
            next_id += 1
            message['id'] = next_id
        body = json.dumps(message, ensure_ascii=False).encode()
        process.stdin.write(f'Content-Length: {len(body)}\r\n\r\n'.encode() + body)
        process.stdin.flush()
        if not request:
            return None
        while True:
            result = frames.get(timeout=30)
            if isinstance(result, Exception):
                raise result
            if result.get('method') == 'testhost/failure':
                raise RuntimeError(result)
            if result.get('id') == next_id and 'method' not in result:
                if 'error' in result:
                    raise RuntimeError(result)
                return result['result']
    try:
        result = []
        for test in range(repetitions):
            if test:
                send('test/reset', {}, True)
            send('test/initialize', {'version': 3, 'caseSensitive': False, 'base': fixture['files'],
                 'symlinks': {}, 'callbacks': [], 'plugins': [], 'options': {},
                 'project': {'currentDirectory': '/', 'defaultLibraryPath': '/', 'positionEncoding': 'utf-16'}}, True)
            for action in fixture['actions']:
                kind = action['kind']
                uri = 'file://' + action.get('file', '')
                if kind == 'open':
                    send('textDocument/didOpen', {'textDocument': {'uri': uri, 'languageId': 'typescript', 'version': action['version'], 'text': action['text']}})
                elif kind == 'change':
                    send('textDocument/didChange', {'textDocument': {'uri': uri, 'version': action['version']}, 'contentChanges': [{'text': action['text']}]})
                elif kind == 'close':
                    send('textDocument/didClose', {'textDocument': {'uri': uri}})
                elif kind == 'options':
                    send('test/setOptions', {'options': action['options']}, True)
                elif kind == 'state':
                    result.append(send('test/projectState', {}, True))
                else:
                    raise ValueError(f'unknown action {kind}')
        send('test/shutdown', {}, True)
        process.stdin.close()
        if process.wait(timeout=10):
            raise RuntimeError(process.stderr.read().decode())
        return result
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        thread.join(timeout=2)


def main():
    go = shutil.which('go')
    expected = tomllib.loads((ROOT / 'data/s04/toolchains.toml').read_text())['go']
    env = {**os.environ, 'GOTOOLCHAIN': 'local', 'GOWORK': 'off', 'GOCACHE': str(ROOT / 'target/go-build')}
    if not go or subprocess.check_output([go, 'env', 'GOVERSION'], env=env, text=True).strip() != expected:
        raise SystemExit(f'put {expected} on PATH')
    pin = json.loads((ROOT / 'data/upstream.json').read_text())['pin']
    upstream = ROOT / 'upstream'
    subprocess.run(['git', '-C', str(upstream), 'diff', '--exit-code', pin, '--', 'tsc'], check=True, stdout=subprocess.DEVNULL)
    original = subprocess.check_output(['git', '-C', str(upstream), 'show', f'{pin}:tsc/internal/fourslash/statebaseline.go'], text=True)
    methods = original[original.index('func (f *FourslashTest) printProjectsDiff'):]
    methods = methods.replace('(f *FourslashTest)', '(f *projectionWriter)').replace('snapshot *project.Snapshot', 'snapshot *wireSnapshot').replace('*compiler.Program', '*wireProgram').replace('map[string]projectInfo', 'map[string]*wireProgram')
    fixture = json.loads((HERE / 'cases.json').read_text())
    target = ROOT / 'target/phase5-project'
    target.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='tsr-project-projection-') as tmp:
        stage = Path(tmp)
        probe = stage / 'projection_test.go'
        probe.write_text((HERE / 'projection_test.go').read_text() + '\n' + methods)
        overlay = stage / 'overlay.json'
        overlay.write_text(json.dumps({'Replace': {str(upstream / 'tsc/internal/fourslash/rust_projection_test.go'): str(probe)}}))
        native = target / 'native-tests'
        subprocess.run([go, 'test', '-overlay', str(overlay), '-c', '-o', str(native), './internal/fourslash'], cwd=upstream / 'tsc', env=env, check=True, timeout=300)
        env.update(TSR_PROJECT_CASES=str(HERE / 'cases.json'), TSR_PROJECT_OUTPUT=str(stage / 'go.json'))
        subprocess.run([str(native), '-test.run', '^TestRustProjectStateNative$', '-test.timeout', '60s'], cwd=upstream / 'tsc', env=env, check=True, timeout=75)
        go_result = json.loads((stage / 'go.json').read_text())
        subprocess.run(['cargo', 'build', '-p', 'tsr_testhost', '--bin', 'phase5_testserver'], cwd=ROOT, check=True, timeout=300)
        started = time.monotonic()
        rust = rust_states(fixture)
        batch = rust_states(fixture, repetitions=2)
        if batch != rust * 2:
            raise SystemExit('fresh-process and retained-cache batch state sequences differ')
        elapsed = time.monotonic() - started
        (stage / 'rust.json').write_text(json.dumps(rust))
        env.update(TSR_PROJECT_RENDER=str(stage / 'rust.json'), TSR_PROJECT_OUTPUT=str(stage / 'rendered.json'))
        subprocess.run([str(native), '-test.run', '^TestRustProjectStateRender$', '-test.timeout', '30s'], cwd=upstream / 'tsc', env=env, check=True, timeout=40)
        rendered = json.loads((stage / 'rendered.json').read_text())
        if rendered != go_result['baselines']:
            for i, (a, b) in enumerate(zip(go_result['baselines'], rendered, strict=True)):
                if a != b:
                    print(f'State {i}:\n' + ''.join(difflib.unified_diff(a.splitlines(True), b.splitlines(True), fromfile='Go', tofile='Rust')))
            raise SystemExit('project state baselines differ')
        print(f'{len(rust)} project states match the original Go writer byte for byte; native projection round-trip matches too')
        print(f'Fresh-process and two-test retained-cache batch agree ({elapsed:.3f}s for these small endpoint runs; builds excluded)')


if __name__ == '__main__':
    main()
