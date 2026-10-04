#!/usr/bin/env python3
"""L4 development comparisons: real servers, lists, resolve and applied edits.

Like pinned fourslash, sort completion lists by sortText/name (insensitive,
then sensitive), retaining response order for ties. Everything else is exact.
"""
import difflib
import json
import tempfile
from pathlib import Path
from interop import Peer, ROOT
from read_only import position

CASES = [
    '/*😀*/ const alpha = 1; let beta = "b"; function call(p: number) { const local = p; /*cursor*/ }',
    'const item = { alpha: 1, beta: "b", method(x: number) {} }; item./*cursor*/',
    'interface Item { required: string; optional?: number; method(): void } declare const item: Item; item.op/*cursor*/tional',
    'interface Item { "not an id": number; normal: string } declare const item: Item; item./*cursor*/',
    'interface Item { name: string } declare const item: Item | undefined; item./*cursor*/',
    'interface Item { name: string } declare const item: Item | undefined; item?./*cursor*/',
    'namespace Space { export const value = 1; export interface Shape {} } Space./*cursor*/',
    'namespace Space { export const value = 1; export interface Shape {} } let value: Space./*cursor*/',
    'type Existing = string; let value: /*cursor*/',
    '/** Local description. */ const local = 1; loc/*cursor*/',
    'const current = /*cursor*/',
    'function call(a = /*cursor*/, b: number) {}',
    'const /*cursor*/',
    'function /*cursor*/',
    '// no completion /*cursor*/',
    '/*cursor*/',
    'interface Options { required: string; optional?: number; done: boolean } const opt: Options = { done: true, /*cursor*/ };',
    'interface Options { required: string; optional?: number } const opt: Options = { /*cursor*/ };',
    'const value = { one: 1, two: 2 }; const { /*cursor*/ } = value;',
    'class Empty { /*cursor*/ }',
    'interface Empty { /*cursor*/ }',
    'class Derived extends Base { /*cursor*/ } class Base { value: number; method(): void {} private hidden: string }',
    'class Empty { constructor(/*cursor*/) {} }',
    'import { /*cursor*/ } from "./dep";',
    'import { alpha, /*cursor*/ } from "./dep";',
    'export { /*cursor*/ } from "./dep";',
    'let value = 1 as /*cursor*/',
    'function func<T = /*cursor*/, Later = unknown>() {}',
    'type T<K extends /*cursor*/> = K;',
    'declare const state: "one" | "two"; switch(state) { case "one": break; case /*cursor*/ }',
    'declare const state: "one" | "two"; switch(state) { /*cursor*/ }',
    'enum State {One, Two}; declare const state: State; switch(state) { case State.One: break; case /*cursor*/ }',
    'interface Promise<T> {then(onfulfilled: (value: T) => unknown): Promise<unknown>} declare const promised: Promise<{value: number}>; async function f() {promised./*cursor*/}',
    'enum State {One, Two}; declare const state: State; switch(state) { case State.One: break; case State./*cursor*/ }',
    'declare const state: -1 | 0 | 2n | "one"; switch(state) { case -1: break; case 0: break; /*cursor*/ }',
    'interface Promise<T> {then(onfulfilled: (value: T) => unknown): Promise<unknown>} declare const promised: Promise<{"x-y": number}>; async function f() {const previous=1\n promised./*cursor*/}',
    'interface Promise<T> {then(onfulfilled: (value: T) => unknown): Promise<unknown>} declare const promised: Promise<{value: number}> | undefined; async function f() { promised./*cursor*/}',
    'class ClassName {} new /*cursor*/',
    'function accept<T extends {one: number; two?: string}>(arg: T): T {return arg} accept({/*cursor*/});',
    'import type { Default } from "./dep"; new De/*cursor*/',

    'const object = { alpha: 1, optional: true }; object["/*cursor*/"]',
    'let state: "ready" | "done" = "/*cursor*/";',
    'function choose(state: "ready" | "done"): void {} choose("/*cursor*/")',
    'type Obj = { alpha: number; method(): void }; type Prop = Obj["/*cursor*/"]',
    '/**/*cursor*/\nfunction greet(name: string, count: number) {}',
    '/**/*cursor*/ */\nfunction greet(name: string, count: number) {}',
    '/**/*cursor*/\nclass Example {}',
    '/**\n * @/*cursor*/\n */\nfunction greet(name: string) {}',
    '/**\n * @param /*cursor*/\n */\nfunction greet(name: string, count: number) {}',
    'let state: \"ready\" | \"done\" = /*cursor*/;',
    'function choose(value: 1 | 2 | 3): void {} choose(/*cursor*/)',
    'function choose(value: 1 | 2 | 3): void {} choose(true ? 1 : /*cursor*/)',
    'let values: (\"one\" | \"two\")[] = [/*cursor*/]',
    'let values: (\"one\" | \"two\")[] = [\"one\", /*cursor*/]',
    'import { alpha } from "./dep"; met/*cursor*/',
    'import * as deps from "./dep"; met/*cursor*/',
    'import type { Shape } from "./dep"; met/*cursor*/',
    'import Default from "./dep"; met/*cursor*/',
    'outer: while(true) { inner: while (true) { break /*cursor*/ } }',
    'outer: while(true) { continue ou/*cursor*/ter; }',
    'import { alpha } from "./de/*cursor*/p";',
    'export * from ".//*cursor*/";',
    '/// <reference path="./' + '/*cursor*/' + '" />',
    'type Imported = import(".//*cursor*/");',
    'declare const state: \"one\" | \"two\"; state === /*cursor*/',


]

def key(item):
    # Fixture labels use ASCII case mappings; exact ties remain stable.
    sort = item.get('sortText', '')
    label = item['label']
    return sort.lower(), sort, label.lower(), label

def offset_at(text, position, encoding):
    lines = text.splitlines(keepends=True)
    line = position['line']
    prefix = ''.join(lines[:line])
    tail = lines[line] if line < len(lines) else ''
    codec, width = ('utf-8', 1) if encoding == 'utf-8' else ('utf-16-le', 2)
    return len(prefix) + len(tail.encode(codec)[:position['character'] * width].decode(codec))

def apply_completion(text, offset, encoding, item, defaults):
    edit = item.get('textEdit')
    if edit:
        primary = {'range': edit.get('range', edit.get('replace')), 'newText': edit['newText']}
    else:
        replace = defaults.get('editRange')
        if replace:
            replace = replace.get('replace', replace)
        else:
            start = offset
            while start and (text[start-1].isalnum() or text[start-1] in '_$'):
                start -= 1
            replace = {'start': position(text, start, encoding), 'end': position(text, offset, encoding)}
        primary = {'range': replace, 'newText': item.get('insertText', item['label'])}
    edits = [primary, *item.get('additionalTextEdits', [])]
    converted = [(offset_at(text, e['range']['start'], encoding), offset_at(text, e['range']['end'], encoding), e['newText']) for e in edits]
    ordered = sorted(converted, key=lambda e: (e[0], e[1]))
    assert all(a[1] <= b[0] for a, b in zip(ordered, ordered[1:])), ordered
    for start, end, insert in reversed(ordered):
        text = text[:start] + insert + text[end:]
    return text

def run(binary, root, encoding, rich, cases=CASES, filename="main.ts", auto_insert=False):
    peer = Peer([str(binary), '--lsp', '--stdio'], root)
    try:
        peer.request('initialize', {'processId': None, 'rootUri': root.as_uri(), 'capabilities': {
            'general': {'positionEncodings': [encoding]},
            'textDocument': {'completion': {'completionItem': {
                'snippetSupport': rich, 'commitCharactersSupport': rich,
                'insertReplaceSupport': rich, 'labelDetailsSupport': rich,
                'documentationFormat': ['markdown' if rich else 'plaintext']},
                'completionList': {'itemDefaults': ['commitCharacters', 'editRange'] if rich else []}}},
            'workspace': {'configuration': True}}})
        peer.send('initialized', {})
        uri = (root / filename).as_uri()
        rows = []
        for index, case in enumerate(cases):
            offset = case.index('/*cursor*/')
            text = case.replace('/*cursor*/', '')
            if index == 0:
                peer.send('textDocument/didOpen', {'textDocument': {'uri': uri, 'languageId': 'javascript' if filename.endswith('.js') else 'typescriptreact' if filename.endswith('.tsx') else 'typescript', 'version': 1, 'text': text}})
            else:
                peer.send('textDocument/didChange', {'textDocument': {'uri': uri, 'version': index + 1}, 'contentChanges': [{'text': text}]})
            if auto_insert:
                result = peer.exchange('textDocument/_vs_onAutoInsert', {'_vs_textDocument': {'uri': uri}, '_vs_position': position(text, offset, encoding), '_vs_ch': '>'})
            else:
                result = peer.exchange('textDocument/completion', {'textDocument': {'uri': uri}, 'position': position(text, offset, encoding)})
            result.pop('id', None)
            if isinstance(result.get('result'), dict) and 'items' in result['result']:
                result['result']['items'].sort(key=key)
            rows.append(['list', index, result])
            for item in (result.get('result') or {}).get('items', []):
                if item['label'].startswith(('Pkg', 'case ')) or item['label'].rstrip('?') in ['alpha', 'local', 'name', 'optional', 'method', 'Shape', 'Existing', 'string', 'title', 'onClick', 'greet']:
                    resolved = peer.request('completionItem/resolve', item)
                    rows.append(['resolve', index, item['label'], resolved])
                    if item.get('data', {}).get('autoImport'):
                        assert 'additionalTextEdits' in resolved, resolved
                        rows.append(['apply', index, item['label'], apply_completion(text, offset, encoding, resolved, result['result'].get('itemDefaults', {}))])
        peer.request('shutdown')
        peer.send('exit')
        return rows
    finally:
        peer.close()

JSX_HEADER = 'declare namespace JSX { interface Element {} interface IntrinsicElements { div: { title?: string; visible?: boolean; count: number; onClick?: () => void }; "custom-tag": { name: string }; span: {} } }\n'
JSX_CASES = [JSX_HEADER + text for text in [
    'const element = </*cursor*/',
    'const element = <d/*cursor*/',
    'const element = <div /*cursor*//>',
    'const element = <div count={1} /*cursor*//>',
    'const spread = {count: 1}; const element = <div {...spread} /*cursor*//>',
    'const element = <div ti/*cursor*/tle="x" />',
    'const element = <div></ /*cursor*/',
    'const element = <div></ /*cursor*/ >',
    'function Component(props: {name: string; count?: number}) {return <div count={1}/>}; const element = <Component /*cursor*//>',
]]
AUTO_CASES = ['const element = <div>/*cursor*/', 'const element = <div>/*cursor*/</div>', 'const element = <>/*cursor*/', 'const element = <div><div>/*cursor*/</div>', 'const element = <Widget.Child>/*cursor*/', 'const element = <dollar$>/*cursor*/', 'const x = 1 >/*cursor*/ 0']
JS_CASES = [
    '/**\n * @param /*cursor*/\n */\nfunction greet(name, count) {}',
    '/**\n * @param /*cursor*/\n */\nfunction greet(name="hi", count=1, ...rest) {}',
    '/**\n * @param /*cursor*/\n */\nfunction greet({one, two: renamed, child: {name}}) {}',
    '/**/*cursor*/\nfunction greet(name="hi", ...rest) {return name}',
]

PACKAGE_CASES = [
    'Pkg/*cursor*/',
    'let value: Pkg/*cursor*/',
    'import { PkgValue } from "sample"; Pkg/*cursor*/',
    'import type { PkgType } from "sample"; Pkg/*cursor*/',
    'import value from "conditional//*cursor*/";',
    'import value from "conditional/features//*cursor*/";',
    'import value from "#internal//*cursor*/";',
    'import value from "alias//*cursor*/";',
    'import value from "ext//*cursor*/";',
    'import value from "pre/*cursor*/";',
    'import value from "legacy//*cursor*/";',
]

def package_fixture(root):
    root.mkdir()
    (root / 'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"module":"nodenext","paths":{"alias/*":["./src/*"],"ext/*":["./src/*.js"],"prefix*end":["./src/pre*.ts"]}},"files":["main.ts"]}')
    (root / 'main.ts').write_text('')
    (root / 'package.json').write_text('{"type":"module","dependencies":{"sample":"*","conditional":"*","legacy":"*"},"devDependencies":{"dev-only":"*"},"optionalDependencies":{"optional-only":"*"},"imports":{"#internal/*":"./src/*.js"}}')
    files = {
        'sample/package.json': '{"types":"index.d.ts"}',
        'sample/index.d.ts': 'export declare const PkgValue: number; export interface PkgType { value: number }; export declare class PkgClass {}',
        'conditional/package.json': '{"exports":{".":{"import":"./esm.d.ts","require":"./cjs.d.cts"},"./feature":"./feature.d.ts","./features/*":"./features/*.d.ts"}}',
        'conditional/esm.d.ts': 'export declare const PkgImport: number;',
        'conditional/cjs.d.cts': 'export declare const PkgRequire: number;',
        'conditional/feature.d.ts': 'export declare const PkgFeature: number;',
        'conditional/features/first.d.ts': 'export declare const PkgFirst: number;',
        'legacy/package.json': '{"types":"index.d.ts","typesVersions":{"*":{"*":["types/*"]}}}',
        'legacy/types/a.d.ts': 'export declare const PkgLegacy: number;',
        'dev-only/package.json': '{"types":"index.d.ts"}',
        'dev-only/index.d.ts': 'export declare const PkgDevOnly: number;',
        'optional-only/package.json': '{"types":"index.d.ts"}',
        'optional-only/index.d.ts': 'export declare const PkgOptionalOnly: number;',
        'not-dependency/package.json': '{"types":"index.d.ts"}',
        'not-dependency/index.d.ts': 'export declare const PkgHidden: number;',
    }
    (root / 'src' / 'nested').mkdir(parents=True)
    (root / 'src' / 'a.ts').write_text('export const value = 1;')
    (root / 'src' / 'prebuilt.ts').write_text('export const value = 1;')
    (root / 'src' / 'nested' / 'b.ts').write_text('export const value = 2;')
    for name, text in files.items():
        file = root / 'node_modules' / name
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)

def main():
    with tempfile.TemporaryDirectory(prefix='tsr-l4-') as directory:
        root = Path(directory).resolve()
        (root / 'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"strict":true,"allowJs":true,"jsx":"preserve"},"files":["main.ts","jsx.tsx","script.js"]}')
        (root / 'main.ts').write_text('')
        (root / 'jsx.tsx').write_text('')
        (root / 'script.js').write_text('')
        (root / 'dep.ts').write_text('export const alpha = 1; export interface Shape { a: number } export function method() {} export default class Default {}')
        package_root = root / 'packages'
        package_fixture(package_root)
        for encoding in ['utf-8', 'utf-16']:
            for rich in [False, True]:
                expected = run(ROOT / 'target/phase5/go-lsp', root, encoding, rich)
                actual = run(ROOT / 'target/debug/tsrust', root, encoding, rich)
                for cases, name, auto in [(JSX_CASES, 'jsx.tsx', False), (AUTO_CASES, 'jsx.tsx', True), (JS_CASES, 'script.js', False)]:
                    expected.extend(run(ROOT / 'target/phase5/go-lsp', root, encoding, rich, cases, name, auto))
                    actual.extend(run(ROOT / 'target/debug/tsrust', root, encoding, rich, cases, name, auto))
                expected.extend(run(ROOT / 'target/phase5/go-lsp', package_root, encoding, rich, PACKAGE_CASES))
                actual.extend(run(ROOT / 'target/debug/tsrust', package_root, encoding, rich, PACKAGE_CASES))
                if actual != expected:
                    output = ROOT / 'target/phase5/l4-diff'
                    output.mkdir(exist_ok=True)
                    for name, rows in [('Go', expected), ('Rust', actual)]:
                        (output / (name + '.json')).write_text(json.dumps(rows, indent=2, ensure_ascii=False))
                    print(''.join(difflib.unified_diff(json.dumps(expected, indent=2).splitlines(True), json.dumps(actual, indent=2).splitlines(True), fromfile='Go', tofile='Rust'))[:18000])
                    raise SystemExit(f'L4 differs ({encoding}, rich={rich}); full responses in {output}')
                print(f'{len(actual)} completion responses match Go ({encoding}, rich={rich})', flush=True)

if __name__ == '__main__':
    main()
