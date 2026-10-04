#!/usr/bin/env python3
"""Bounded #93 regressions against the pinned Go executable and both Rust entries.
Run interop.py first to build the pinned command; no capture or expectation edits.
"""
import json
from pathlib import Path
import subprocess
import tempfile

from interop import Peer, ROOT


def flags():
    commands = [[str(ROOT / 'target/phase5/go-lsp'), '--lsp'],
                [str(ROOT / 'target/debug/tsrust'), '--lsp']]
    cases = [['--help'], ['---stdio'], ['-=bad'], ['--wat'], ['--stdio=maybe'],
             ['--clientProcessId'], ['--clientProcessId=not-a-pid'],
             ['--clientProcessId=0x10'], ['--clientProcessId=0X_Ff'],
             ['--clientProcessId=08'], ['--clientProcessId=-0b10'],
             ['--clientProcessId=0_7'], ['--clientProcessId=1__2'],
             ['--clientProcessId=9223372036854775807'],
             ['--clientProcessId=9223372036854775808'],
             ['--clientProcessId=-9223372036854775808'],
             ['--clientProcessId=-9223372036854775809'],
             ['--stdio=false'], ['--stdio=false', '--', '--unknown']]
    for case in cases:
        results = [subprocess.run(command + case, capture_output=True, timeout=5) for command in commands]
        left, right = [(r.returncode, r.stdout, r.stderr) for r in results]
        assert left == right, (case, left, right)
    print(f'{len(cases)} CLI cases: exit codes and complete stdout/stderr match Go', flush=True)


def malformed():
    results = []
    for binary in [ROOT / 'target/phase5/go-lsp', ROOT / 'target/debug/tsrust']:
        peer = Peer([str(binary), '--lsp', '--stdio'], ROOT)
        try:
            data = b'{"jsonrpc":"2.0","id":1,"method":"initialize","params":]}'
            peer.process.stdin.write(f'Content-Length: {len(data)}\r\n\r\n'.encode() + data)
            peer.process.stdin.flush()
            response = peer.queue.get(timeout=10)
            assert not isinstance(response, Exception), response
            results.append(response)
        finally:
            peer.close()
    assert results[0] == results[1], results
    print('Malformed JSON: complete InvalidRequest response matches Go', flush=True)


def run(command, cwd, files, private=False):
    peer = Peer(command, cwd)
    try:
        if private:
            peer.request('test/initialize', {'version': 3, 'caseSensitive': True, 'base': files, 'symlinks': {}, 'callbacks': [], 'plugins': [], 'options': {}, 'project': {'currentDirectory': str(cwd), 'defaultLibraryPath': str(cwd), 'positionEncoding': 'utf-16'}})
        query = {'textDocument': {'uri': (cwd / 'main.ts').as_uri()}}
        peer.request('initialize', {'processId': None, 'rootUri': cwd.as_uri(), 'capabilities': {'workspace': {'configuration': True, 'didChangeWatchedFiles': {'dynamicRegistration': True}, 'diagnostics': {'refreshSupport': True}}}})
        peer.send('exit') # Still awaiting initialized: this must not close either entry.
        results = [peer.exchange('custom/projectInfo', query)['error']]
        peer.send('initialized', {})
        while True:
            pending = peer.queue.get(timeout=20)
            assert not isinstance(pending, Exception), pending
            if pending.get('method') == 'workspace/configuration':
                break
            peer.respond(pending)
        # Dispatch is blocked on this reverse request. Cancellation before the
        # project-info request starts must do nothing, even though its ID is reserved.
        peer.id += 1
        peer.write({'id': peer.id, 'method': 'custom/projectInfo', 'params': query})
        peer.send('$/cancelRequest', {'id': peer.id})
        peer.respond(pending)
        queued = peer.await_response()
        assert 'error' not in queued, queued
        results.append(queued['result'])
        for method, params in [('custom/setLogVerbosity', {'verbosity': 7}),
                               ('unknown/request', None), ('shutdown', {})]:
            results.append(peer.exchange(method, params)['error'])
        peer.send('unknown/notification')
        for file in ['unknown.vue', 'absent.vue']:
            response = peer.exchange('textDocument/diagnostic', {'textDocument': {'uri': (cwd / file).as_uri()}})
            results.append({k: response[k] for k in ['result', 'error'] if k in response})
        for name, expected in [('stable.json', 'stable.json'), ('../bad.json', 'tsconfig.json')]:
            peer.send('workspace/didChangeConfiguration', {'settings': {'js/ts': {'customConfigFileName': name, 'unstable': {'customConfigFileName': 'unstable.json'}}}})
            result = peer.request('custom/projectInfo', query)
            assert result['configFilePath'] == str(cwd / expected), result
            results.append(result)
        peer.drain(0.8)
        start = peer.server_requests.count('workspace/diagnostic/refresh')
        peer.send('workspace/didChangeWatchedFiles', {'changes': [{'uri': (cwd / 'readme.md').as_uri(), 'type': 2}]})
        peer.drain(0.8)
        assert peer.server_requests.count('workspace/diagnostic/refresh') == start
        peer.send('workspace/didChangeWatchedFiles', {'changes': [{'uri': (cwd / 'dependency.ts').as_uri(), 'type': 2}, {'uri': (cwd / 'second.ts').as_uri(), 'type': 3}]})
        peer.drain(1.0)
        refreshes = peer.server_requests.count('workspace/diagnostic/refresh') - start
        assert refreshes == 1, refreshes
        results.append({'watchRefreshes': refreshes})
        peer.request('shutdown')
        peer.send('exit')
        peer.process.stdin.close()
        assert peer.process.wait(timeout=10) == 0, peer.process.stderr.read().decode()
        return results
    finally:
        peer.close()


def main():
    flags()
    malformed()
    with tempfile.TemporaryDirectory(prefix='tsr-l2-review-') as directory:
        cwd = Path(directory).resolve()
        config = '{"compilerOptions":{"strict":true,"noLib":true},"files":["main.ts"]}'
        files = {str(cwd / name): text for name, text in [
            ('tsconfig.json', config), ('stable.json', config), ('unstable.json', config),
            ('main.ts', 'const x = 1;'), ('unknown.vue', 'text')]}
        for name, text in files.items():
            Path(name).write_text(text)
        expected = run([str(ROOT / 'target/phase5/go-lsp'), '--lsp', '--stdio'], cwd, files)
        for label, command, private in [
            ('native Rust', [str(ROOT / 'target/debug/tsrust'), '--lsp', '--stdio'], False),
            ('private Rust', [str(ROOT / 'target/debug/phase5_testserver'), '--stdio'], True)]:
            actual = run(command, cwd, files, private)
            assert actual == expected, json.dumps({'Go': expected, label: actual}, indent=2)
            print(f'{label}: complete errors, queued cancellation, unknown-file fallback, config precedence and watched refresh match Go', flush=True)


if __name__ == '__main__':
    main()
