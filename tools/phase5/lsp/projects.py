#!/usr/bin/env python3
"""Focused L6 project-tree requests against pinned Go, using fresh servers.

No corpus, benchmark, capture archive, npm install, or network access. Both
servers read identical live files. The test opens only the declaration owner;
its two consumers must be discovered through the unopened solution tree.
"""
import argparse
import difflib
import json
from pathlib import Path
import tempfile
import subprocess
from interop import ROOT
from completions import ConfiguredPeer
from read_only import position

SOURCES = {
    'a/a.ts': '/*😀*/ export function shared() { return 1; }\nexport interface Service { run(): void; }\n',
    'b/b.ts': 'import {shared, Service} from "../a/a";\nexport function callerB() { shared(); }\nexport class B implements Service { run() {} }\n',
    'c/c.ts': 'import {shared, Service} from "../a/a";\nexport function callerC() { shared(); }\nexport class C implements Service { run() {} }\n',
}
SOURCES['a/a.ts'] += 'export declare function overloaded(x: string): void;\nexport const separator = 0;\nexport declare function overloaded(x: number): void;\n'
SOURCES['b/b.ts'] += 'import {overloaded} from "../a/a";\nexport function overloadedB() { overloaded("x"); overloaded(1); }\n'
SOURCES['c/c.ts'] += 'import {overloaded} from "../a/a";\nexport function overloadedC() { overloaded(2); }\n'



def run(binary, root, encoding, method):
    peer = ConfiguredPeer([str(binary), '--lsp', '--stdio'], root, {'tsserver': {'automaticTypeAcquisition': {'enabled': False}}, 'referencesCodeLensEnabled': True, 'implementationsCodeLensEnabled': True})
    try:
        peer.request('initialize', {'processId': None, 'rootUri': root.as_uri(), 'initializationOptions': {'codeLensShowLocationsCommandName': 'editor.showReferences'}, 'capabilities': {
            'general': {'positionEncodings': [encoding]},
            'textDocument': {'implementation': {'linkSupport': True}},
            'workspace': {'configuration': True, 'workspaceEdit': {'documentChanges': True, 'resourceOperations': ['rename']}}
        }})
        peer.send('initialized', {})
        opened = 'b/b.ts' if method == 'moduleRename' else 'a/a.ts'
        text = SOURCES[opened]
        uri = (root/opened).as_uri()
        peer.send('textDocument/didOpen', {'textDocument': {'uri': uri, 'languageId': 'typescript', 'version': 1, 'text': text}})
        params = {'textDocument': {'uri': uri}, 'position': position(text, text.index('shared'), encoding)}
        if method == 'textDocument/references':
            params['context'] = {'includeDeclaration': True}
        elif method == 'textDocument/rename':
            params['newName'] = 'renamed'
        elif method == 'moduleRename':
            params['position'] = position(text, text.index('../a/a')+4, encoding)
            params['newName'] = 'renamed'
        elif method == 'textDocument/implementation':
            params['position'] = position(text, text.index('Service'), encoding)
        elif method in ('callHierarchy/incomingCalls', 'overloadedIncoming'):
            if method == 'overloadedIncoming':
                params['position'] = position(text, text.index('overloaded'), encoding)
            items = peer.request('textDocument/prepareCallHierarchy', params)
            assert items, items
            if method == 'overloadedIncoming':
                assert len(items) == 2, items
            params = {'item': items[0]}
        elif method == 'workspace/willRenameFiles':
            params = {'files': [{'oldUri': uri, 'newUri': (root/'a/new.ts').as_uri()}]}
        if method in ('referenceLens', 'implementationLens'):
            lenses = peer.request('textDocument/codeLens', {'textDocument': {'uri': uri}})
            kind = 'references' if method == 'referenceLens' else 'implementations'
            # shared() and Service are the first exported declaration of each kind.
            lens = next(lens for lens in lenses if lens['data']['kind'] == kind)
            result = peer.request('codeLens/resolve', lens)
        else:
            result = peer.request('textDocument/rename' if method == 'moduleRename' else 'callHierarchy/incomingCalls' if method == 'overloadedIncoming' else method, params)
        peer.request('shutdown'); peer.send('exit')
        return result
    finally:
        peer.close()


def canonical(response, method):
    # Go's cross-project worker map has no inter-project enumeration order.
    # Preserve all payloads and edit ordering; canonicalize only that map's
    # collection order (IDs in VS references are deliberately not covered here).
    # Incoming callers are explicitly sorted per declaration by Go, then
    # merged in declaration order. That order is part of this comparison.
    if isinstance(response, list) and method not in ('callHierarchy/incomingCalls', 'overloadedIncoming'):
        return sorted(response, key=lambda item: json.dumps(item, sort_keys=True))
    if isinstance(response, dict) and response.get('command', {}).get('arguments'):
        arguments = response['command']['arguments']
        return {**response, 'command': {**response['command'], 'arguments': [*arguments[:2], sorted(arguments[2], key=lambda item: json.dumps(item, sort_keys=True))]}}
    if isinstance(response, dict) and 'documentChanges' in response:
        return {**response, 'documentChanges': sorted(response['documentChanges'], key=lambda item: json.dumps(item, sort_keys=True))}
    return response


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rust', type=Path)
    parser.add_argument('--method', action='append')
    parser.add_argument('--encoding', action='append', choices=['utf-8', 'utf-16'])
    parser.add_argument('--go', type=Path, default=ROOT/'target/phase5/go-lsp')
    args = parser.parse_args()
    if args.rust is None:
        built = subprocess.run(['cargo', 'build', '-p', 'tsrust', '--bin', 'tsrust', '--message-format=json'], cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True)
        artifacts = [json.loads(line) for line in built.stdout.splitlines()]
        binaries = [item['executable'] for item in artifacts if item.get('reason') == 'compiler-artifact' and item.get('target', {}).get('name') == 'tsrust' and item.get('executable')]
        if len(binaries) != 1:
            raise RuntimeError(f'expected one server artifact, got {binaries}')
        args.rust = Path(binaries[0])
    with tempfile.TemporaryDirectory(prefix='tsr-l6-projects-') as directory:
        root = Path(directory).resolve()
        (root/'tsconfig.json').write_text(json.dumps({'files': [], 'references': [{'path': './'+name} for name in ['a', 'b', 'c']]}))
        for name, text in SOURCES.items():
            (root/name).parent.mkdir(parents=True, exist_ok=True)
            (root/name).write_text(text)
            project = name[0]
            (root/project/'tsconfig.json').write_text(json.dumps({'compilerOptions': {'composite': True, 'noLib': True}, 'files': [project+'.ts'], **({'references': [{'path': '../a'}]} if project != 'a' else {})}))
        for encoding in args.encoding or ['utf-8', 'utf-16']:
            for method in args.method or ['textDocument/references', 'textDocument/rename', 'textDocument/implementation', 'callHierarchy/incomingCalls', 'workspace/willRenameFiles', 'moduleRename', 'referenceLens', 'implementationLens', 'overloadedIncoming']:
                native = canonical(run(args.go, root, encoding, method), method)
                rust = canonical(run(args.rust, root, encoding, method), method)
                if native != rust:
                    output = ROOT/'target/phase5/l6-projects-diff'
                    output.mkdir(parents=True, exist_ok=True)
                    for label, data in [('go', native), ('rust', rust)]:
                        (output/f'{label}.json').write_text(json.dumps(data, indent=2))
                    print(''.join(difflib.unified_diff(json.dumps(native, indent=2).splitlines(True), json.dumps(rust, indent=2).splitlines(True), fromfile='Go', tofile='Rust')))
                    raise SystemExit(f'{method} differs ({encoding}); {output}')
                print(f'{method} matches pinned Go ({encoding})', flush=True)

if __name__ == '__main__':
    main()
