#!/usr/bin/env python3
"""Focused L5 file-move comparisons, including edits applied to their sources."""
import argparse
import difflib
import json
import tempfile
from pathlib import Path
from completions import ConfiguredPeer
from edits import apply
from interop import ROOT
from read_only import position

CASES = [
    ({'main.ts': 'import {x} from "./dep"; x;', 'dep.ts': 'export const x=1'}, 'dep.ts', 'renamed.ts', {}),
    ({'main.ts': 'import {x} from "./dep"; x;', 'dep.ts': 'export const x=1'}, 'main.ts', 'nested/main.ts', {}),
    ({'main.ts': 'import {x} from "./dir"; x;', 'dir/index.ts': 'export const x=1'}, 'dir', 'other', {}),
    ({'main.ts': 'import {x} from "./dir/index"; x;', 'dir/index.ts': 'export const x=1'}, 'dir', 'other', {}),
    ({'main.ts': 'import {x} from "./dep.js"; x;', 'dep.ts': 'export const x=1'}, 'dep.ts', 'renamed.ts', {}),
    ({'main.ts': 'import {x} from "./dep.ts"; x;', 'dep.ts': 'export const x=1'}, 'dep.ts', 'renamed.ts', {'allowImportingTsExtensions': True}),
    ({'main.ts': '/*😀*/\r\nimport {x} from "./dep"; x;\r\n', 'dep.ts': 'export const x=1'}, 'dep.ts', 'renamed.ts', {}),
    ({'main.ts': '/// <reference path="./dep.ts"/>\nconst y=x;', 'dep.ts': 'const x=1'}, 'dep.ts', 'renamed.ts', {}),
    ({'main.ts': 'import {x} from "./missing"; x;'}, 'missing.ts', 'renamed.ts', {}),
    ({'main.ts': 'import {x} from "./missing"; x;'}, 'missing', 'renamed', {}),
    ({'main.ts': 'import {x} from "@app/dep"; x;', 'src/dep.ts': 'export const x=1'}, 'src/dep.ts', 'src/renamed.ts', {'baseUrl': '.', 'paths': {'@app/*': ['src/*']}}),
    ({'main.ts': 'import {x} from "./a.css"; x;', 'a.d.css.ts': 'export const x: number;', 'a.css': '.x {}'}, 'a.d.css.ts', 'b.d.css.ts', {'allowArbitraryExtensions': True}),
    ({'main.ts': 'import {x} from "./dep"; x;', 'dep.ts': 'export const x=1'}, 'dep.ts', 'renamed.ts', {'outDir': 'dep.ts', 'typeRoots': ['dep.ts']}),
    ({'main.ts':'import {x} from "./src/dep"; x;', 'src/dep.ts':'export const x=1', 'tsconfig.json':'{"compilerOptions":{"noLib":true},"include":["main.ts", "src/**/*.ts"]}'}, 'src/dep.ts', 'other/dep.ts', {}),
    ({'main.ts':'import {x} from "./src/dep"; x;', 'src/dep.ts':'export const x=1', 'tsconfig.json':'{\n  "compilerOptions":{"noLib":true},\n  "include":[\n    "main.ts",\n    "src/**/*.ts" // keep this comment\n  ]\n}'}, 'src/dep.ts', 'other/dep.ts', {}),

]

PREFERENCE_CASES = [
    ({'main.ts': 'import {x} from "@app/dep"; x;', 'src/dep.ts': 'export const x=1'},
     'src/dep.ts', 'src/renamed.ts', {'baseUrl': '.', 'paths': {'@app/*': ['src/*']}},
     {'preferences': {'autoImportSpecifierExcludeRegexes': ['^@app/']}}, {}),
    ({'main.ts': 'import {x} from "<root>/src/dep"; x;', 'src/dep.ts': 'export const x=1'},
     'src/dep.ts', 'src/renamed.ts', {'baseUrl': '.', 'paths': {'@app/*': ['src/*']}}, {}, {}),
    ({'main.ts': 'import {x} from "./b/src/lib/index"; import "b"; x;',
      'b/src/lib/index.ts': 'export const x=1', 'b/package.json': '{"name":"b","main":"./src/lib/index.ts"}'},
     'b/src/lib/index.ts', 'b/src/other/index.ts', {}, {}, {'node_modules/b': 'b'}),
    ({'main.ts': 'import {x} from "./b/src/y.js"; import "b"; x;',
      'b/src/y.ts': 'export const x=1', 'b/package.json': '{"name":"b","main":"./src/y.ts"}'},
     'b/src/y.ts', 'b/src/z.ts', {}, {'preferences': {'importModuleSpecifierEnding': 'minimal'}}, {'node_modules/b': 'b'}),
    *[({'main.ts': 'export {};', old: 'export type T = import("./dep.ts").T;',
        'dep.ts': 'export interface T {x:number}', 'package.json': '{"type":"module"}'},
       old, new, {'module': 'nodenext', 'moduleResolution': 'nodenext'}, {}, {})
      for old, new in [('entry.d.ts', 'entry.ts'), ('entry.ts', 'entry.d.ts')]],
]

def run(binary, root, files, old, new, encoding, document_changes, will_rename, config=None):
    peer = ConfiguredPeer([str(binary), '--lsp', '--stdio'], root, config or {})
    try:
        peer.request('initialize', {'processId': None, 'rootUri': root.as_uri(), 'capabilities': {
            'general': {'positionEncodings': [encoding]}, 'workspace': {'configuration': True,
            'workspaceEdit': {'documentChanges': document_changes, 'resourceOperations': ['rename']},
            'fileOperations': {'willRename': will_rename}}}})
        peer.send('initialized', {})
        uri = (root/'main.ts').as_uri()
        text = files['main.ts']
        peer.send('textDocument/didOpen', {'textDocument': {'uri': uri, 'languageId': 'typescript', 'version': 1, 'text': text}})
        response = peer.request('workspace/willRenameFiles', {'files': [{'oldUri': (root/old).as_uri(), 'newUri': (root/new).as_uri()}]})
        result = [normalize(response, files, root, encoding)]
        if '"./dep"' in text and document_changes:
            params = {'textDocument': {'uri': uri}, 'position': position(text, text.index('"./dep"')+4, encoding)}
            result.append(peer.request('textDocument/prepareRename', params))
            result.append(normalize(peer.request('textDocument/rename', {**params, 'newName': 'renamed'}), files, root, encoding))
        peer.request('shutdown'); peer.send('exit')
        return result
    finally:
        peer.close()

def normalize(response, files, root, encoding):
    if response is None: return None
    response = json.loads(json.dumps(response))
    documents = response.get('documentChanges', [])
    # Go returns a map of file edits; inter-file order is not an edit-order guarantee.
    documents.sort(key=lambda d: (d.get('textDocument', {}).get('uri', ''), d.get('oldUri', '')))
    changes = dict(response.get('changes', {}))
    for document in documents:
        if 'textDocument' in document:
            changes.setdefault(document['textDocument']['uri'], []).extend(document['edits'])
    applied = {uri: apply(files[str(Path(uri.removeprefix('file://')).relative_to(root))], edits, encoding) for uri, edits in changes.items()}
    return {'response': response, 'applied': applied}

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--rust', type=Path, default=ROOT/'target/debug/tsrust')
    p.add_argument('--go', type=Path, default=ROOT/'target/phase5/go-lsp')
    args = p.parse_args()
    count = 0
    cases = [(*case, {}, {}) for case in CASES] + PREFERENCE_CASES
    for index, (files, old, new, options, config, links) in enumerate(cases):
        with tempfile.TemporaryDirectory(prefix='tsr-l5-file-') as directory:
            root = Path(directory).resolve()
            files = {name: text.replace('<root>', str(root)) for name, text in files.items()}
            files['tsconfig.json'] = files.get('tsconfig.json') or json.dumps({'compilerOptions': {'noLib': True, **options}, 'files': ['main.ts', *[f for f in files if f != 'main.ts' and f.endswith('.ts')]]}, indent=2)
            for name, text in files.items():
                (root/name).parent.mkdir(parents=True, exist_ok=True)
                (root/name).write_text(text)
            for name, target in links.items():
                (root/name).parent.mkdir(parents=True, exist_ok=True)
                (root/name).symlink_to(root/target, target_is_directory=True)
            for encoding in ['utf-8', 'utf-16']:
                for document_changes, will_rename in [(False, False), (True, False), (True, True)]:
                    go, rust = [run(b, root, files, old, new, encoding, document_changes, will_rename, config) for b in [args.go, args.rust]]
                    if go != rust:
                        print(index, old, new, encoding, document_changes, will_rename)
                        print('\n'.join(difflib.unified_diff(json.dumps(go, indent=2).splitlines(), json.dumps(rust, indent=2).splitlines(), fromfile='Go', tofile='Rust')))
                        raise SystemExit(1)
                    count += 1
    print(f'{count} file move/config/import edits and applied outputs match Go')

if __name__ == '__main__': main()
