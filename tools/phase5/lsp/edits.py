#!/usr/bin/env python3
"""L5 focused native comparison of edits and the source obtained by applying them.

No benchmark, corpus capture, or replacement for the L7 fourslash runner.
"""
import argparse
import difflib
import json
import tempfile
from pathlib import Path
from completions import ConfiguredPeer, offset_at
from interop import ROOT
from read_only import position

FORMAT_CASES = [
    'const value={a:1,b:2};\nfunction f(x:number){return x+1;}\n',
    '/*😀*/ const value={a:1};\r\nif(value){\r\nvalue.a ++;\r\n}\r\n',
    'const value={a:1,b:2};   \n',
    'const broken = {x: , y:2};\n',
    'const jsx=<div><span x={1+2}/></div>;\n',
    '// ;/*type*/\nconst x=1;',
    'function f(){/*type*/',
    'function f(){return 1;}/*type*/',
    'const value= 1;/*type*/',
    'function f(){\n/*type*/}',
    '/* comment ;/*type*/ still comment */\nconst x = 1;',
]

RENAME_CASES = [
    '/*😀*/ const /*rename*/value = 1; const object = {value}; object.value;',
    'const value = 1; const object = {/*rename*/value}; object.value;',
    'const value = 1; const object = {value}; object./*rename*/value;',
    'const object = {value: 1}; const {/*rename*/value} = object; value;',
    'const object = {/*rename*/value: 1}; const {value} = object; value;',
    'interface Shape { /*rename*/value: number }; const value = 1; const object: Shape = {value}; object.value;',
    'const object = {value: 1}; const {value: /*rename*/local} = object; local;',
    'class Example { #/*rename*/private = 1; method(){ return this.#private; } }',
    '/*rename*/outer: while(true){ break outer; }',
    'class /*rename*/Example { static self(){ return this; } } new Example();',
    'interface Base { /*rename*/value: number }; class Derived implements Base {value = 1;} new Derived().value;',
    'const object = { /*rename*/123: true }; object[123];',
    'declare const state: "/*rename*/ready" | "done"; if(state === "ready") {}',
    'import { /*rename*/value } from "./dep"; value; export { value };',
    'import {value} from "./dep"; /*rename*/value; export { value };',
    'import { /*rename*/value as local } from "./dep"; local;',
    'import { value as /*rename*/local } from "./dep"; local;',
    'const value = 1; export { /*rename*/value }; value;',
    'const /*rename*/value = 1; export { value }; value;',
    'export { /*rename*/value } from "./dep";',
    'export { value as /*rename*/local } from "./dep";',
    'import { /*rename*/default as Example } from "./dep";',
    'const value = /*rename*/123;',
    'const /*rename*/😀 = 1;',
]

def renames(binary, root, encoding, config):
    peer = ConfiguredPeer([str(binary), '--lsp', '--stdio'], root, config)
    result = []
    try:
        peer.request('initialize', {'processId':None, 'rootUri':root.as_uri(), 'capabilities':{
            'general':{'positionEncodings':[encoding]}, 'workspace':{'configuration':True}}})
        peer.send('initialized', {})
        uri = (root/'main.tsx').as_uri()
        for index, marked in enumerate(RENAME_CASES):
            at = marked.index('/*rename*/')
            text = marked.replace('/*rename*/', '')
            peer.send('textDocument/didOpen', {'textDocument':{'uri':uri, 'languageId':'typescriptreact', 'version':1, 'text':text}})
            params = {'textDocument':{'uri':uri}, 'position':position(text,at,encoding)}
            prepared = peer.exchange('textDocument/prepareRename', params)
            prepared.pop('id', None)
            renamed = peer.request('textDocument/rename', {**params, 'newName':'changed'})
            applied = {name:apply(text if name == uri else (root/'dep.ts').read_text(), edits, encoding) for name, edits in (renamed or {}).get('changes', {}).items()}
            result.append([index, prepared, renamed, applied])
            peer.send('textDocument/didClose', {'textDocument':{'uri':uri}})
        peer.request('shutdown'); peer.send('exit')
    finally:
        peer.close()
    return result

def apply(text, edits, encoding):
    ranges = [(offset_at(text, e['range']['start'], encoding),
               offset_at(text, e['range']['end'], encoding), e['newText']) for e in edits or []]
    ranges.sort(key=lambda e: (e[0], e[1]))
    end = 0
    for start, finish, _ in ranges:
        assert 0 <= end <= start <= finish <= len(text), ranges
        end = finish
    for start, end, replacement in reversed(ranges):
        text = text[:start] + replacement + text[end:]
    return text

def run(binary, root, encoding, config):
    peer = ConfiguredPeer([str(binary), '--lsp', '--stdio'], root, config)
    result = []
    try:
        peer.request('initialize', {'processId':None, 'rootUri':root.as_uri(), 'capabilities':{
            'general':{'positionEncodings':[encoding]}, 'workspace':{'configuration':True}}})
        peer.send('initialized', {})
        uri = (root / 'main.tsx').as_uri()
        for index, marked in enumerate(FORMAT_CASES):
            at = marked.find('/*type*/')
            text = marked.replace('/*type*/', '')
            peer.send('textDocument/didOpen', {'textDocument':{'uri':uri, 'languageId':'typescriptreact', 'version':1, 'text':text}})
            options = {'tabSize': 2, 'insertSpaces': True, 'trimTrailingWhitespace': True}
            requests = [('textDocument/formatting', {})]
            requests.append(('textDocument/rangeFormatting', {'range':{'start':position(text, 0, encoding), 'end':position(text, len(text), encoding)}}))
            if at >= 0:
                requests.append(('textDocument/onTypeFormatting', {'position':position(text, at, encoding), 'ch':text[at-1:at]}))
            for method, extra in requests:
                response = peer.request(method, {'textDocument':{'uri':uri}, 'options':options, **extra})
                result.append([index, method, response, apply(text, response, encoding)])
            peer.send('textDocument/didClose', {'textDocument':{'uri':uri}})
        peer.request('shutdown')
        peer.send('exit')
    finally:
        peer.close()
    return result

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rust', type=Path, default=ROOT/'target/debug/tsrust')
    parser.add_argument('--go', type=Path, default=ROOT/'target/phase5/go-lsp')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='tsr-l5-') as directory:
        root = Path(directory).resolve()
        (root/'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"jsx":"preserve"},"files":["main.tsx"]}')
        (root/'main.tsx').write_text('')
        (root/'dep.ts').write_text('export const value = 1; export default class Example {}')
        for encoding in ['utf-8', 'utf-16']:
            for config in [{}, {'format':{'enabled':False}}, {'format':{'newLineCharacter':'\r\n','insertSpaceAfterOpeningAndBeforeClosingNonemptyBraces':False}}]:
                go, rust = (run(binary, root, encoding, config) for binary in [args.go, args.rust])
                if go != rust:
                    print('\n'.join(difflib.unified_diff(json.dumps(go, indent=2,ensure_ascii=False).splitlines(), json.dumps(rust, indent=2,ensure_ascii=False).splitlines(), fromfile='Go',tofile='Rust')))
                    raise SystemExit(1)
                print(f'format {encoding} {config}: {len(go)} edits and applied-text comparisons pass')
            for config in [{}, {'preferences':{'useAliasesForRenames':False}}]:
                go, rust = (renames(binary, root, encoding, config) for binary in [args.go, args.rust])
                if go != rust:
                    print('rename', encoding, config)
                    print('\n'.join(difflib.unified_diff(json.dumps(go, indent=2,ensure_ascii=False).splitlines(), json.dumps(rust, indent=2,ensure_ascii=False).splitlines(), fromfile='Go',tofile='Rust')))
                    raise SystemExit(1)
                print(f'rename {encoding} {config}: {len(go)} prepare, edits and applied-text comparisons pass')

if __name__ == '__main__':
    main()
