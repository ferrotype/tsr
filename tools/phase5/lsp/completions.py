#!/usr/bin/env python3
"""L4 development comparisons: real servers, lists, resolve and applied edits.

Like pinned fourslash, sort completion lists by sortText/name (insensitive,
then sensitive), retaining response order for ties. Everything else is exact.
"""
import argparse
import difflib
import fnmatch
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
    'declare const state: \"a$\" | \"brace}\"; switch(state) { /*cursor*/ }',
    'import { enumValue } from \"./dep\"; switch(enumValue) { /*cursor*/ }',
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
    'import type Default from "./dep"; new De/*cursor*/',
    'import { type Default } from "./dep"; new De/*cursor*/',
    'import { alpha as zebra } from "./dep"; met/*cursor*/',
    'import {\n    alpha,\n} from "./dep"; met/*cursor*/',
    'import {\n    alpha // keep comment\n} from "./dep"; met/*cursor*/',
    'import Default, { alpha } from "./dep"; met/*cursor*/',
    'import {} from "./dep"; met/*cursor*/',
    '"use strict";\nmet/*cursor*/',
    '#!/usr/bin/env node\n// license\nmet/*cursor*/',
    'import { alpha } from "./dep"; let x: Sh/*cursor*/',
    'class C { #private = 1; method() { this./*cursor*/ } }',
    'const known = 1; function f() { this./*cursor*/ }',


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


CONTEXT_CASES = [
    'type T = {one: number; two: string}; type Key = T[("one" | "/*cursor*/")];',
    'type T<K extends "one" | "two"> = K; type Key = T<"one" | "/*cursor*/">;',
    'declare const object: {"١word": number; "²word": number; "123word": number}; object./*cursor*/',
    'export const first = 1; const second = 2; export { /*cursor*/ };',
    'const first = 1; const second = 2; export { first, /*cursor*/ };',

    'const item = { one: 1, two: 2 }; const { one, /*cursor*/ } = item;',
    'const item = { one: 1, two: 2 }; const { renamed: /*cursor*/ } = item;',
    'interface Item { one: number; two?: string }; const item: Item = { "/*cursor*/": 1 };',
    'interface Item { one: number; two?: string }; const item: Item = { one: 1, "/*cursor*/": 2 };',
    'declare const item: { "a-b": number; "c d": string; plain: boolean }; "/*cursor*/" in item;',
    'function choose<T extends "first" | "second">(value: T) {} choose("/*cursor*/");',
    'function choose(value: "first"): void; function choose(value: "second"): void; function choose(value: string) {} choose("/*cursor*/");',
    'type Key = "first" | "second"; let value: Key = `/*cursor*/`;',
    'type T = { one: number; two: string }; type Key = T["one" | "/*cursor*/"];',
    'class Base { protected value = 1; private hidden = 2; method() {} } class Derived extends Base { method() { super./*cursor*/ } }',
    'interface Shape { value: number; method(): void } class Derived implements Shape { /*cursor*/ }',
    'interface Shape { value: number; method(): void } class Derived implements Shape { override /*cursor*/ }',
    'interface Shape { method(): void } class Base { value = 1 } class Derived extends Base implements Shape { override /*cursor*/ }',
    'interface Shape { "a-b": number; optional?: number }; const item: Shape = { /*cursor*/ };',
    'type Shape = {kind:"one"; a:number} | {kind:"two"; b:string}; const item: Shape = {kind:"one", /*cursor*/};',
    'interface Item { one: number; two: number }; const other = { one: 1 }; const item: Item = {...other, /*cursor*/};',
    'declare const object: { "a-b": number; default: number; 123: string }; object./*cursor*/',
    'const αlpha = 1; α/*cursor*/',
    'function f<T, U extends T = /*cursor*/>() {}',
    'import { alpha as renamed } from "./dep"; export { /*cursor*/ };',
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

def expand_snippet(text):
    # The emitted L4 snippets use tab stops, defaults, and escaped text. Refuse
    # unhandled snippet syntax rather than accidentally testing the raw wire.
    def part(at, nested=False):
        out = []
        while at < len(text):
            ch = text[at]
            at += 1
            if ch == "\\":
                assert at < len(text) and text[at] in "\\$}", text
                out.append(text[at]); at += 1
            elif ch == "$":
                braced = at < len(text) and text[at] == "{"
                if braced: at += 1
                start = at
                while at < len(text) and text[at].isdigit(): at += 1
                assert at > start, text
                if braced:
                    if text[at] == ":":
                        value, at = part(at + 1, True); out.append(value)
                    else:
                        assert text[at] == "}", text
                        at += 1
            elif ch == "}" and nested:
                return "".join(out), at
            else: out.append(ch)
        assert not nested, text
        return "".join(out), at
    return part(0)[0]

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
    if item.get('insertTextFormat') == 2:
        primary['newText'] = expand_snippet(primary['newText'])
    edits = [primary, *item.get('additionalTextEdits', [])]
    converted = [(offset_at(text, e['range']['start'], encoding), offset_at(text, e['range']['end'], encoding), e['newText']) for e in edits]
    ordered = sorted(converted, key=lambda e: (e[0], e[1]))
    assert all(a[1] <= b[0] for a, b in zip(ordered, ordered[1:])), ordered
    for start, end, insert in reversed(ordered):
        text = text[:start] + insert + text[end:]
    return text

class ConfiguredPeer(Peer):
    def __init__(self, command, cwd, config):
        self.config = config or {}
        self.watchers = {}
        super().__init__(command, cwd)

    def respond(self, value):
        if value.get('method') == 'workspace/configuration' and 'id' in value:
            self.server_requests.append(value['method'])
            result = [self.config if item['section'] in ['typescript', 'javascript'] else {} for item in value['params']['items']]
            self.write({'id': value['id'], 'result': result})
        else:
            if value.get('method') == 'client/registerCapability':
                for registration in value['params']['registrations']:
                    if registration.get('method') == 'workspace/didChangeWatchedFiles':
                        self.watchers[registration['id']] = registration['registerOptions']['watchers']
            elif value.get('method') == 'client/unregisterCapability':
                for registration in value['params']['unregisterations']:
                    self.watchers.pop(registration['id'], None)
            super().respond(value)


def run(binary, root, encoding, rich, cases=CASES, filename="main.ts", auto_insert=False, config=None):
    peer = ConfiguredPeer([str(binary), '--lsp', '--stdio'], root, config)
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
                if item.get('data',{}).get('autoImport') or item.get('data',{}).get('source') in ['ObjectLiteralMethodSnippet/', 'TypeOnlyAlias/'] or item['label'].startswith(('Pkg', 'case ')) or item['label'].rstrip('?') in ['alpha', 'local', 'name', 'optional', 'method', 'Shape', 'Existing', 'string', 'title', 'onClick', 'greet', 'Default', 'Shared', 'PkgDefault']:
                    resolved = peer.request('completionItem/resolve', item)
                    rows.append(['resolve', index, item['label'], resolved])
                    if item.get('data', {}).get('autoImport'):
                        assert item.get('data', {}).get('isImportStatementCompletion') or 'additionalTextEdits' in resolved, resolved
                        rows.append(['apply', index, item['label'], apply_completion(text, offset, encoding, resolved, result['result'].get('itemDefaults', {}))])
                    elif item.get('insertTextFormat') == 2 or item.get('insertText') and (config or {}).get('suggest', {}).get('classMemberSnippets', {}).get('enabled'):
                        rows.append(['apply-snippet', index, item['label'], apply_completion(text, offset, encoding, resolved, result['result'].get('itemDefaults', {}))])
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
    'const { alpha } = require("./dep"); met/*cursor*/',
    'const { alpha: renamed } = require("./dep"); met/*cursor*/',
    'const { alpha, } = require("./dep"); met/*cursor*/',
    'const {\n    alpha,\n} = require("./dep"); met/*cursor*/',
    'const dependency = require("./dep"); met/*cursor*/',
    '/** @type {Sh/*cursor*/} */ const value = 1;',
    '/** @import {Shape} from "./dep" */\n/** @type {Ot/*cursor*/} */ const value = 1;',

    'const known=1; function f(){ this./*cursor*/ }',
    '// @ts-check\nconst known=1; function f(){ this./*cursor*/ }',
    'const someName=1; const obj={ alpha:1 }; obj./*cursor*/',
    'const typed = 1; const other = { "quoted":1, "not-an-identifier":1 }; unknown./*cursor*/',
    'const known=1; ident/*cursor*/',

    '/**\n * @param /*cursor*/\n */\nfunction greet(name, count) {}',
    '/**\n * @param /*cursor*/\n */\nfunction greet(name="hi", count=1, ...rest) {}',
    '/**\n * @param /*cursor*/\n */\nfunction greet({one, two: renamed, child: {name}}) {}',
    '/**/*cursor*/\nfunction greet(name="hi", ...rest) {return name}',
]

IMPORT_STATEMENT_CASES = [
    'import /*cursor*/',
    'import Sha/*cursor*/',
    'import type /*cursor*/',
    'import type Sha/*cursor*/',
    'import { /*cursor*/',
    'import { Sha/*cursor*/',
    'import { type /*cursor*/',
    'import { type Sha/*cursor*/',
    'import { alpha, /*cursor*/',
    'import { alpha } /*cursor*/',
    'import * as deps /*cursor*/',
    'export { alpha } /*cursor*/',
    'export * /*cursor*/',
    'import Sha/*cursor*/\ninterface Other {}',
    'import { Sha/*cursor*/\ninterface Other {}',
    '/** leading */\nimport Sha/*cursor*/',
]

CLASS_SNIPPET_CASES = [
    'class Base { value: number; protected method(arg: number): void {} } class Derived extends Base { /*cursor*/ }',
    'import { ImportedBase } from "./base"; class Derived extends ImportedBase { /*cursor*/ }',
    'import { ImportedBase } from "./base"; import { alpha } from "./dep"; class Derived extends ImportedBase { /*cursor*/ }',
    'interface Base { optional?: string; method(arg: string): number } class Derived implements Base { /*cursor*/ }',
    'abstract class Base { abstract method<T>(arg: T): T; abstract property: string } class Derived extends Base { /*cursor*/ }',
    'class Base { get name(): string { return "" } set name(value: string) {} } class Derived extends Base { /*cursor*/ }',
    'interface Base { method(x: number): string; method(x: string, y?: boolean): number } class Derived implements Base { /*cursor*/ }',
    'class Base { method(x: number): string; method(x: string): string; method(x: unknown): string { return "" } } class Derived extends Base { /*cursor*/ }',
    'class Base { protected method(arg: number): void {} } class Derived extends Base { public /*cursor*/ }',
    'class Base { protected method(arg: number): void {} } class Derived extends Base { public met/*cursor*/ }',
    'abstract class Base { abstract method(arg: number): void } abstract class Derived extends Base { abstract /*cursor*/ }',
    'class Base { static value = 1; method() {} } class Derived extends Base { static /*cursor*/ }',
    'interface Base { "not-an-identifier": string; "dollar$"(x: number): void } class Derived implements Base { /*cursor*/ }',
    'interface Base { method(x: number): void } class Derived implements Base { method(x: number) {} /*cursor*/ }',
    'import { ImportedBase } from "./base"; import type { Shape } from "./dep"; class Derived extends ImportedBase { /*cursor*/ }',
    'import { ImportedBase } from "./base"; import {\n    alpha,\n} from "./dep"; class Derived extends ImportedBase { /*cursor*/ }',
]

SNIPPET_CASES = [
    'interface Target { method(arg: number, optional?: string): void }; const value: Target = { /*cursor*/ };',
    'interface Target { property: ((x: number) => void) | undefined; other: ((a: number) => void) | ((b: string) => number) }; const value: Target = { /*cursor*/ };',
    'interface Target { "not-a-method"(dollar$: string): void }; const value: Target = { /*cursor*/ };',
    'interface Target { generic<T>(arg: T): T; optional?: (first?: number, ...rest: string[]) => void }; const value: Target = { /*cursor*/ };',
]

PACKAGE_CASES = [
    'import {PkgDevOnly} from "dev-only"; Pkg/*cursor*/',
    '/// <reference path="./node_modules/ambient-provider/globals.d.ts" />\nimport {provided} from "declared-only"; Pkg/*cursor*/',

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
    (root / 'package.json').write_text('{"type":"module","dependencies":{"sample":"*","conditional":"*","legacy":"*","typed":"*","fallback":"*","symlinked":"*","require-package":"*"},"devDependencies":{"dev-only":"*"},"optionalDependencies":{"optional-only":"*"},"imports":{"#internal/*":"./src/*.js"}}')
    files = {
        'typed/package.json': '{"types":"index.d.ts"}',
        'typed/index.d.ts': 'export declare const PkgOwnTypes: number;',
        '@types/typed/index.d.ts': 'export declare const PkgWrongFallback: number;',
        'fallback/package.json': '{"main":"index.js"}',
        'fallback/index.js': 'exports.PkgJavaScript = 1;',
        '@types/fallback/deep.d.ts': 'export declare const PkgTypeFallback: number;',
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
        'require-package/package.json': '{"types":"index.d.ts"}',
        'require-package/index.d.ts': 'declare function PkgFactory(): void; export = PkgFactory;',
        'ambient-provider/package.json': '{"types":"index.d.ts"}',
        'ambient-provider/index.d.ts': 'export declare const PkgAmbient: number;',
        'ambient-provider/globals.d.ts': 'declare module "declared-only" {export const provided: number}',
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

    store = root / '.store'
    store.mkdir()
    (store / 'package.json').write_text('{"types":"index.d.ts"}')
    (store / 'index.d.ts').write_text('export { PkgShared } from "shared"; export declare const PkgLinked: number;')
    shared = root / '.shared'
    shared.mkdir()
    (shared / 'index.d.ts').write_text('export declare const PkgShared: number;')
    (store / 'node_modules').mkdir()
    (store / 'node_modules' / 'shared').symlink_to(shared, target_is_directory=True)
    (root / 'node_modules' / 'symlinked').symlink_to(store, target_is_directory=True)

def run_invalidation(binary, root, encoding, config_replacement=False):
    root.mkdir(exist_ok=True)
    (root / 'tsconfig.json').write_text('{"compilerOptions":{"noLib":true},"files":["main.ts","dep.ts"]}')
    (root / 'main.ts').write_text('Cha')
    (root / 'dep.ts').write_text('export const ChangedOne = 1;')
    peer = ConfiguredPeer([str(binary), '--lsp', '--stdio'], root, {})
    rows = []
    main_uri, dep_uri = ((root / name).as_uri() for name in ['main.ts', 'dep.ts'])
    def query(expected):
        response = peer.request('textDocument/completion', {'textDocument': {'uri': main_uri}, 'position': {'line': 0, 'character': 3}})
        response['items'].sort(key=key)
        labels = {item['label'] for item in response['items']}
        if expected is not None:
            assert {name for name in labels if name.startswith('Changed')} == expected, (expected, labels)
        rows.append(['invalidation', len(rows), response])
    try:
        peer.request('initialize', {'processId': None, 'rootUri': root.as_uri(), 'capabilities': {
            'general': {'positionEncodings': [encoding]},
            'workspace': {'configuration': True, 'didChangeWatchedFiles': {'dynamicRegistration': True}}}})
        peer.send('initialized', {})
        peer.send('textDocument/didOpen', {'textDocument': {'uri': main_uri, 'languageId': 'typescript', 'version': 1, 'text': 'Cha'}})
        query({'ChangedOne'})
        peer.send('textDocument/didChange', {'textDocument': {'uri': main_uri, 'version': 2}, 'contentChanges': [{'text': 'Cha // same file edit'}]})
        query({'ChangedOne'})
        peer.send('textDocument/didOpen', {'textDocument': {'uri': dep_uri, 'languageId': 'typescript', 'version': 1, 'text': 'export const ChangedOne = 1;'}})
        peer.send('textDocument/didChange', {'textDocument': {'uri': dep_uri, 'version': 2}, 'contentChanges': [{'text': 'export const ChangedTwo = 2;'}]})
        query({'ChangedTwo'})
        peer.config = {'preferences': {'autoImportFileExcludePatterns': ['**/dep.ts']}}
        peer.send('workspace/didChangeConfiguration', {'settings': {'js/ts': peer.config}})
        query(set())
        peer.config = {}
        peer.send('workspace/didChangeConfiguration', {'settings': {'js/ts': peer.config}})
        query({'ChangedTwo'})
        (root / 'dep.ts').write_text('export const ChangedDisk = 3;')
        peer.send('textDocument/didClose', {'textDocument': {'uri': dep_uri}})
        peer.send('workspace/didChangeWatchedFiles', {'changes': [{'uri': dep_uri, 'type': 2}]})
        query({'ChangedDisk'})
        (root / 'tsconfig.json').write_text('{"compilerOptions":{"noLib":true},"files":["main.ts"]}')
        peer.send('workspace/didChangeWatchedFiles', {'changes': [{'uri': (root / 'tsconfig.json').as_uri(), 'type': 2}]})
        # Native retains its project bucket on removal alone. The additional
        # replacement sequence is an explicit, unresolved L6 integration probe.
        query({'ChangedDisk'})
        if config_replacement:
            (root / 'extra.ts').write_text('export const ChangedExtra = 4;')
            (root / 'tsconfig.json').write_text('{"compilerOptions":{"noLib":true},"files":["main.ts","extra.ts"]}')
            peer.send('workspace/didChangeWatchedFiles', {'changes': [
                {'uri': (root / 'extra.ts').as_uri(), 'type': 1},
                {'uri': (root / 'tsconfig.json').as_uri(), 'type': 2},
            ]})
            query(None)
        peer.request('shutdown'); peer.send('exit')
        return rows
    finally:
        peer.close()


def run_package_invalidation(binary, root, encoding):
    root.mkdir(exist_ok=True)
    dep = root / 'node_modules/pkg'
    dep.mkdir(parents=True, exist_ok=True)
    (root / 'tsconfig.json').write_text('{"compilerOptions":{"noLib":true},"files":["main.ts"]}')
    (root / 'main.ts').write_text('Changed')
    (root / 'package.json').write_text('{"dependencies":{"pkg":"*"}}')
    (dep / 'package.json').write_text('{"types":"index.d.ts"}')
    (dep / 'index.d.ts').write_text('export declare const ChangedBefore: number;')
    peer = ConfiguredPeer([str(binary), '--lsp', '--stdio'], root, {})
    rows = []
    def query(expected):
        response = peer.request('textDocument/completion', {'textDocument': {'uri': (root / 'main.ts').as_uri()}, 'position': {'line': 0, 'character': 7}})
        response['items'].sort(key=key)
        assert {i['label'] for i in response['items'] if i['label'].startswith('Changed')} == expected, response
        rows.append(['package-invalidation', len(rows), response])
    def watch(file, kind):
        peer.send('workspace/didChangeWatchedFiles', {'changes': [{'uri': file.as_uri(), 'type': kind}]})
    try:
        peer.request('initialize', {'processId': None, 'rootUri': root.as_uri(), 'capabilities': {
            'general': {'positionEncodings': [encoding]},
            'workspace': {'configuration': True, 'didChangeWatchedFiles': {'dynamicRegistration': True}}}})
        peer.send('initialized', {})
        peer.send('textDocument/didOpen', {'textDocument': {'uri': (root / 'main.ts').as_uri(), 'languageId': 'typescript', 'version': 1, 'text': 'Changed'}})
        query({'ChangedBefore'})
        peer.drain(0.05)
        # Verify that a real editor can send the next event: do not merely
        # inject a change for a path the server never subscribed to.
        patterns = [w['globPattern'] for group in peer.watchers.values() for w in group]
        assert any(isinstance(p, str) and fnmatch.fnmatchcase(str(dep / 'index.d.ts'), p) for p in patterns), patterns
        (dep / 'index.d.ts').write_text('export declare const ChangedAfter: number;')
        watch(dep / 'index.d.ts', 2)
        query({'ChangedAfter'})
        (dep / 'index.d.ts').unlink()
        watch(dep / 'index.d.ts', 3)
        query(set())
        (dep / 'index.d.ts').write_text('export declare const ChangedRecreated: number;')
        watch(dep / 'index.d.ts', 1)
        query({'ChangedRecreated'})
        (dep / 'next.d.ts').write_text('export declare const ChangedEntry: number;')
        (dep / 'package.json').write_text('{"types":"next.d.ts"}')
        watch(dep / 'package.json', 2)
        query({'ChangedEntry'})
        peer.request('shutdown'); peer.send('exit')
        return rows
    finally:
        peer.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config-replacement', action='store_true', help='Run the unresolved L6 root-replacement sequence; differences exit nonzero')
    args = parser.parse_args()
    if args.config_replacement:
        with tempfile.TemporaryDirectory(prefix='tsr-l4-config-') as directory:
            root = Path(directory).resolve()
            expected = run_invalidation(ROOT / 'target/phase5/go-lsp', root, 'utf-16', True)
            actual = run_invalidation(ROOT / 'target/debug/tsrust', root, 'utf-16', True)
            output = ROOT / 'target/phase5/l4-config-replacement'
            output.mkdir(exist_ok=True)
            for name, rows in [('Go', expected), ('Rust', actual)]:
                (output / (name + '.json')).write_text(json.dumps(rows, indent=2, ensure_ascii=False))
            if expected != actual:
                raise SystemExit(f'L6 config replacement differs; full responses in {output}')
            print('Config replacement responses match Go')
        return
    with tempfile.TemporaryDirectory(prefix='tsr-l4-') as directory:
        root = Path(directory).resolve()
        (root / 'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"strict":true,"allowJs":true,"jsx":"preserve"},"files":["main.ts","jsx.tsx","script.js"]}')
        (root / 'main.ts').write_text('')
        (root / 'jsx.tsx').write_text('')
        (root / 'script.js').write_text('')
        (root / 'base.ts').write_text('import { Shape, Other, Default } from "./dep"; export interface ImportedBase { method(arg: Shape): Other; property: Default }')
        (root / 'dep.ts').write_text('export const alpha = 1; export interface Shape { a: number } export function method() {} export interface Other { b: string } export class Default {} export default Default; export enum State {One, Two}; export declare const enumValue: State;')
        package_root = root / 'packages'
        package_fixture(package_root)
        for encoding in ['utf-8', 'utf-16']:
            for rich in [False, True]:
                expected = run(ROOT / 'target/phase5/go-lsp', root, encoding, rich)
                actual = run(ROOT / 'target/debug/tsrust', root, encoding, rich)
                for cases, name, auto in [(CONTEXT_CASES, 'main.ts', False), (JSX_CASES, 'jsx.tsx', False), (AUTO_CASES, 'jsx.tsx', True), (JS_CASES, 'script.js', False)]:
                    expected.extend(run(ROOT / 'target/phase5/go-lsp', root, encoding, rich, cases, name, auto))
                    actual.extend(run(ROOT / 'target/debug/tsrust', root, encoding, rich, cases, name, auto))
                expected.extend(run(ROOT / 'target/phase5/go-lsp', package_root, encoding, rich, PACKAGE_CASES))
                actual.extend(run(ROOT / 'target/debug/tsrust', package_root, encoding, rich, PACKAGE_CASES))
                for binary, rows in [('go-lsp', expected), ('../debug/tsrust', actual)]:
                    rows.extend(run(ROOT / 'target/phase5' / binary, root, encoding, rich, IMPORT_STATEMENT_CASES, config={'suggest': {'includeCompletionsForImportStatements': True}}))
                    rows.extend(run(ROOT / 'target/phase5' / binary, root, encoding, rich, SNIPPET_CASES, config={'suggest': {'objectLiteralMethodSnippets': {'enabled': True}}}))
                    rows.extend(run(ROOT / 'target/phase5' / binary, root, encoding, rich, CLASS_SNIPPET_CASES, config={'suggest': {'classMemberSnippets': {'enabled': True}}}))
                for formatting in [
                    {'indentSize': 2, 'tabSize': 2, 'semicolons': 'remove', 'newLineCharacter': '\r\n'},
                    {'convertTabsToSpaces': False, 'insertSpaceBeforeFunctionParenthesis': True, 'placeOpenBraceOnNewLineForFunctions': True, 'insertSpaceAfterCommaDelimiter': False, 'insertSpaceAfterOpeningAndBeforeClosingNonemptyBraces': False},
                ]:
                    config = {'format': formatting, 'suggest': {'classMemberSnippets': {'enabled': True}, 'objectLiteralMethodSnippets': {'enabled': True}}}
                    for binary, rows in [('go-lsp', expected), ('../debug/tsrust', actual)]:
                        rows.extend(run(ROOT / 'target/phase5' / binary, root, encoding, rich, SNIPPET_CASES[:1] + CLASS_SNIPPET_CASES[:2] + ['met/*cursor*/', 'import {} from \"./dep\"; met/*cursor*/'], config=config))
                for preference in ['single', 'double', 'auto']:
                    cases = ['declare const object: {"a-b": number; "123word": number}; object./*cursor*/', "const first = 'single'; declare const object: {\"a-b\": number}; object./*cursor*/"]
                    for binary, rows in [('go-lsp', expected), ('../debug/tsrust', actual)]:
                        rows.extend(run(ROOT / 'target/phase5' / binary, root, encoding, rich, cases, config={'preferences': {'quotePreference': preference}}))
                for preferences in [
                    {'importModuleSpecifier': 'relative', 'importModuleSpecifierEnding': 'js'},
                    {'importModuleSpecifier': 'non-relative', 'autoImportSpecifierExcludeRegexes': ['^sample$', '/^CONDITIONAL/i']},
                    {'autoImportFileExcludePatterns': ['**/node_modules/sample/**'], 'autoImportEntrypointDirectorySearch': True},
                ]:
                    for binary, rows in [('go-lsp', expected), ('../debug/tsrust', actual)]:
                        rows.extend(run(ROOT / 'target/phase5' / binary, package_root, encoding, rich, ['Pkg/*cursor*/'], config={'preferences': preferences}))
                expected.extend(run_invalidation(ROOT / 'target/phase5/go-lsp', root / 'invalidation', encoding))
                actual.extend(run_invalidation(ROOT / 'target/debug/tsrust', root / 'invalidation', encoding))
                expected.extend(run_package_invalidation(ROOT / 'target/phase5/go-lsp', root / 'package-invalidation', encoding))
                actual.extend(run_package_invalidation(ROOT / 'target/debug/tsrust', root / 'package-invalidation', encoding))
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
