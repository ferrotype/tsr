#!/usr/bin/env python3
"""Focused L3 responses against the pinned native service (no corpus/capture).

Build target/debug/tsrust and target/phase5/go-lsp first (interop.py builds Go).
The same live files and ordered requests are sent to each server. Responses
are compared in full, including nulls, ordering, and negotiated coordinates.
"""
import difflib
import json
from pathlib import Path
import tempfile

from interop import Peer, ROOT


SOURCES = [
    ('main.ts', '/*😀*/ const x = "hello";\nfunction f(\n a: number,\n b: string\n) {\n // one\n // two\n return {\n  a, b\n };\n}\n// #region name\nconst y = [\n 1, 2\n];\n// #endregion\n'),
    ('main.ts', 'type M<T> = { -readonly [K in keyof T]?: T[K]; };\nconst t = `a${x}b${f(x)}c`;'),
    ('main.tsx', 'const x = <><A.B first={1}>\n <div>😀</div>\n</A.B></>;'),
    ('main.tsx', 'const x = <A>text</B>; const y = <div>\n</div>; const z = < ></ >;'),
    ('main.ts', 'import {\n a, b\n} from "./dep";\nimport c from "./other";\n/** comment\n * // #region not a region\n */\nif (x) {\n f(\n  a, b\n );\n} else if (y) {\n while (z) {\n  z--;\n }\n}\n'),
    ('main.ts', 'namespace A.B { export class C<T> { static readonly value = 1; constructor(public x: T) {} get v() { return this.x; } method(p: T) { const local = p; return local; } } }\ninterface I { p: string; call(n: number): void }\ntype Alias = I; enum E { First, Second }\nconst fn = (x: number) => x; const {a, b: renamed} = {a: 1, b: "b"};\n'),
    ('main.ts', 'import { Point, make, Base } from "./dep";\nconst p: Point = { x: 1 }; const { x } = p; const short: Point = {x};\nclass Derived extends Base { override run() { return make(1); } }\nconst c = new Derived(); c.run();\nouter: for (;;) { if (p.x) break outer; continue outer; }\nfunction f(x: number) { switch(x) { case 1: return x; default: return 0; } }\n'),
]

TOKEN_TYPES = ['namespace', 'class', 'enum', 'interface', 'struct', 'typeParameter', 'type', 'parameter', 'variable', 'property', 'enumMember', 'decorator', 'event', 'function', 'method', 'macro', 'label', 'comment', 'string', 'keyword', 'number', 'regexp', 'operator']
TOKEN_MODIFIERS = ['declaration', 'definition', 'readonly', 'static', 'deprecated', 'abstract', 'async', 'modification', 'documentation', 'defaultLibrary', 'local']


def position(text, offset, encoding):
    before = text[:offset]
    line = before.count('\n')
    suffix = before.rsplit('\n', 1)[-1]
    character = len(suffix.encode('utf-8')) if encoding == 'utf-8' else len(suffix.encode('utf-16-le')) // 2
    return {'line': line, 'character': character}


def run(binary, root, encoding, line_only):
    peer = Peer([str(binary), '--lsp', '--stdio'], root)
    try:
        peer.request('initialize', {'processId': None, 'rootUri': root.as_uri(), 'capabilities': {
            'general': {'positionEncodings': [encoding]},
            'textDocument': {'foldingRange': {'lineFoldingOnly': line_only, 'foldingRange': {'collapsedText': True}},
                'documentSymbol': {'hierarchicalDocumentSymbolSupport': not line_only},
                'definition': {'linkSupport': not line_only}, 'typeDefinition': {'linkSupport': not line_only},
                'semanticTokens': {'requests': {'full': True, 'range': True}, 'tokenTypes': TOKEN_TYPES, 'tokenModifiers': TOKEN_MODIFIERS, 'formats': ['relative']}},
            'workspace': {'configuration': True}}})
        peer.send('initialized', {})
        results = []
        for index, (name, text) in enumerate(SOURCES):
            uri = (root / name).as_uri()
            if index in (0, 2):
                peer.send('textDocument/didOpen', {'textDocument': {'uri': uri, 'languageId': 'typescriptreact' if name.endswith('tsx') else 'typescript', 'version': 1, 'text': text}})
            else:
                peer.send('textDocument/didChange', {'textDocument': {'uri': uri, 'version': index+1}, 'contentChanges': [{'text': text}]})
            document = {'textDocument': {'uri': uri}}
            results.append(('folding', index, peer.request('textDocument/foldingRange', document)))
            positions = [position(text, i, encoding) for i in range(len(text)+1)]
            results.append(('selection', index, peer.request('textDocument/selectionRange', {**document, 'positions': positions})))
            results.append(('symbols', index, peer.request('textDocument/documentSymbol', document)))
            results.append(('semantic', index, peer.request('textDocument/semanticTokens/full', document)))
            results.append(('semantic-range', index, peer.request('textDocument/semanticTokens/range', {**document, 'range': {'start': positions[len(positions)//3], 'end': positions[2*len(positions)//3]}})))
            import re
            for match in re.finditer(r'[A-Za-z_$][\w$]*', text):
                for method in ['definition', 'typeDefinition']:
                    p = position(text, match.start(), encoding)
                    results.append((method, index, p, peer.request('textDocument/' + method, {**document, 'position': p})))
            if name.endswith('tsx'):
                for p in positions:
                    results.append(('linked', index, p, peer.request('textDocument/linkedEditingRange', {**document, 'position': p})))
        for query in ['', 'm', 'M', 'AB', 'cb']:
            results.append(('workspace', query, peer.request('workspace/symbol', {'query': query})))
        peer.request('shutdown')
        peer.send('exit')
        return results
    finally:
        peer.close()


def main():
    with tempfile.TemporaryDirectory(prefix='tsr-l3-') as directory:
        root = Path(directory).resolve()
        (root / 'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"jsx":"preserve"},"files":["main.ts","main.tsx"]}')
        for name in ['main.ts', 'main.tsx']:
            (root / name).write_text('')
        (root / 'dep.ts').write_text('export interface Point { x: number; }\nexport function make(x: string): Point;\nexport function make(x: number): Point;\nexport function make(x: unknown): Point { return {x: 1}; }\nexport class Base { run(): Point { return {x: 1}; } }')
        for encoding in ['utf-8', 'utf-16']:
            for line_only in [False, True]:
                expected = run(ROOT / 'target/phase5/go-lsp', root, encoding, line_only)
                actual = run(ROOT / 'target/debug/tsrust', root, encoding, line_only)
                if actual != expected:
                    output = ROOT / 'target/phase5/l3-diff'
                    output.mkdir(exist_ok=True)
                    for label, rows in [('Go', expected), ('Rust', actual)]:
                        (output / f'{label}.json').write_text(json.dumps(rows, indent=2, ensure_ascii=False))
                    diff = difflib.unified_diff(json.dumps(expected, indent=2).splitlines(True), json.dumps(actual, indent=2).splitlines(True), fromfile='Go', tofile='Rust')
                    print(''.join(diff)[:12000])
                    raise SystemExit(f'L3 differs ({encoding}, lineOnly={line_only}); full responses in {output}')
                print(f'{len(actual)} L3 responses match Go ({encoding}, lineOnly={line_only})', flush=True)


if __name__ == '__main__':
    main()
