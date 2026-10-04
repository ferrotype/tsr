#!/usr/bin/env python3
"""Small real-process L2 check. No corpus, capture archive or acceptance metric.
Build debug tsrust and phase5_testserver first; Go is built from the pin here.
"""
import importlib.util
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading
import tomllib
ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('project_check', ROOT / 'tools/phase5/project/check.py')
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)

class Peer:

    def __init__(self, command, cwd):
        self.process = subprocess.Popen(command, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.queue = queue.Queue()
        self.thread = threading.Thread(target=shared.reader, args=(self.process, self.queue), daemon=True)
        self.thread.start()
        self.id = 0

    def write(self, message):
        body = json.dumps({'jsonrpc': '2.0', **message}, ensure_ascii=False).encode()
        self.process.stdin.write(f'Content-Length: {len(body)}\r\n\r\n'.encode() + body)
        self.process.stdin.flush()

    def send(self, method, params=None):
        self.write({'method': method, **({'params': params} if params is not None else {})})

    def request(self, method, params=None):
        self.id += 1
        self.write({'id': self.id, 'method': method, **({'params': params} if params is not None else {})})
        while True:
            value = self.queue.get(timeout=20)
            if isinstance(value, Exception):
                raise value
            if value.get('id') == self.id and 'method' not in value:
                if 'error' in value:
                    raise AssertionError(value)
                return value['result']
            if 'method' in value and 'id' in value:
                result = [{}, {}, {}, {}] if value['method'] == 'workspace/configuration' else None
                self.write({'id': value['id'], 'result': result})

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=5)
        self.thread.join(timeout=2)

def run(command, cwd, files, encoding, private=False):
    peer = Peer(command, cwd)
    try:
        if private:
            peer.request('test/initialize', {'version': 3, 'caseSensitive': True, 'base': files, 'symlinks': {}, 'callbacks': [], 'plugins': [], 'options': {}, 'project': {'currentDirectory': str(cwd), 'defaultLibraryPath': str(cwd), 'positionEncoding': 'utf-16'}})
        result = []
        result.append(peer.request('initialize', {'processId': None, 'rootUri': cwd.as_uri(), 'capabilities': {'general': {'positionEncodings': [encoding]}, 'textDocument': {'diagnostic': {'relatedInformation': True, 'tagSupport': {'valueSet': [1, 2]}}}, 'workspace': {'configuration': True, 'didChangeWatchedFiles': {'dynamicRegistration': True}, 'diagnostics': {'refreshSupport': True}}}}))
        peer.send('initialized', {})
        uri = (cwd / 'main.ts').as_uri()
        text = '/*😀*/ const x: number = "bad";'
        peer.send('textDocument/didOpen', {'textDocument': {'uri': uri, 'languageId': 'typescript', 'version': 1, 'text': text}})
        query = {'textDocument': {'uri': uri}}
        result.append(peer.request('textDocument/diagnostic', query))
        result.append(peer.request('custom/projectInfo', query))
        start = text.index('"bad"')
        units = lambda s: len(s.encode('utf-8')) if encoding == 'utf-8' else len(s.encode('utf-16-le')) // 2
        peer.send('textDocument/didChange', {'textDocument': {'uri': uri, 'version': 2}, 'contentChanges': [{'range': {'start': {'line': 0, 'character': units(text[:start])}, 'end': {'line': 0, 'character': units(text[:start + 5])}}, 'text': '1'}]})
        result.append(peer.request('textDocument/diagnostic', query))
        assert result[-1]['items'] == [], result[-1]
        peer.send('workspace/didChangeConfiguration', {'settings': {'js/ts': {'validate': {'enabled': False}}}})
        peer.send('textDocument/didChange', {'textDocument': {'uri': uri, 'version': 3}, 'contentChanges': [{'text': text}]})
        result.append(peer.request('textDocument/diagnostic', query))
        assert result[-1]['items'] == [], result[-1]
        peer.send('workspace/didChangeConfiguration', {'settings': {'js/ts': {'validate': {'enabled': True}}}})
        result.append(peer.request('textDocument/diagnostic', query))
        assert result[-1]['items'], result[-1]
        result.append(peer.request('shutdown'))
        peer.send('exit')
        peer.process.stdin.close()
        assert peer.process.wait(timeout=10) == 0, peer.process.stderr.read().decode()
        return result
    finally:
        peer.close()

def main():
    go = shutil.which('go')
    env = {**os.environ, 'GOTOOLCHAIN': 'local', 'GOWORK': 'off', 'GOCACHE': str(ROOT / 'target/go-build')}
    assert subprocess.check_output([go, 'env', 'GOVERSION'], env=env, text=True).strip() == tomllib.loads((ROOT / 'data/s04/toolchains.toml').read_text())['go']
    native = ROOT / 'target/phase5/go-lsp'
    native.parent.mkdir(exist_ok=True)
    subprocess.run([go, 'build', '-o', str(native), './cmd/tsc'], cwd=ROOT / 'upstream/tsc', env=env, check=True, timeout=300)
    with tempfile.TemporaryDirectory(prefix='tsr-l2-interop-') as directory:
        cwd = Path(directory).resolve()
        files = {str(cwd / 'tsconfig.json'): '{"compilerOptions":{"strict":true,"noLib":true},"files":["main.ts"]}', str(cwd / 'main.ts'): 'const x: number = 1;'}
        for name, text in files.items():
            Path(name).write_text(text)
        for encoding in ['utf-8', 'utf-16']:
            expected = run([str(native), '--lsp', '--stdio'], cwd, files, encoding)
            for label, command, private in [('native Rust', [str(ROOT / 'target/debug/tsrust'), '--lsp', '--stdio'], False), ('private Rust', [str(ROOT / 'target/debug/phase5_testserver'), '--stdio'], True)]:
                actual = run(command, cwd, files, encoding, private)
                if actual != expected:
                    import difflib
                    print(''.join(difflib.unified_diff(json.dumps(expected, indent=2, ensure_ascii=False).splitlines(True), json.dumps(actual, indent=2, ensure_ascii=False).splitlines(True), fromfile='Go', tofile=label)))
                    raise SystemExit(f'{label} differs ({encoding})')
                print(f'{label}: complete initialize/diagnostic/project-info/edit/settings/shutdown responses match Go ({encoding})', flush=True)
if __name__ == '__main__':
    main()
