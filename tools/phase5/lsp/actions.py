#!/usr/bin/env python3
"""Focused L5 code-action differential: exact edits and applied documents."""
import argparse
import difflib
import json
import tempfile
from pathlib import Path
from completions import ConfiguredPeer
from read_only import position
from edits import apply
from interop import ROOT

ORGANIZE = [
    'import {z, a as local, a} from "./dep";\nimport D from "./dep";\na; z; local; D;\n',
    '// header\nimport {z, /* comment */ a, unused} from "./dep";\n// middle\nimport {a as local} from "./dep";\nz; a; local;\n',
    'import {unused} from "./dep";\nconst x = 1;\n',
    'import "z";\nimport "A";\nimport "a";\n\nimport "./b";\nimport "./a";\n',
    'import {type Shape, z, a} from "./dep";\nimport type {Shape as S} from "./dep";\nconst x: Shape = {}; const y: S = {}; z; a;\n',
    'import * as ns from "./dep";\nimport D from "./dep";\nns.a; D;\n',
    'import D from "./dep";\nimport E from "./dep";\nD; E;\n',
    'import {z,\n a,\n} from "./dep"\n z; a;\n',
    '/*😀*/ import {z, a} from "./dep";\r\n z; a;\r\n',
    'export {z} from "./dep";\nexport {a} from "./dep";\nexport * from "./dep";\nexport * from "./dep";\n',
    'export {z, a} from "./dep";\nconst x=1;\nexport {x};\nexport {z} from "./dep";',
    'declare module "pkg" { import {z, a} from "./dep"; export {z, a}; }\n',
    'import "x10";\nimport "x2";\nimport "x01";\nimport "X1";\nimport "é";\nimport "e";\nimport "É";\nimport "가";\nimport "가";\n',
    'import {a} from "./dep" with {type: "json"};\nimport {z} from "./dep" with {type: "json"};\na; z;\n',
]
KINDS = ['source.organizeImports.ts', 'source.removeUnusedImports.ts', 'source.sortImports.ts']

def run(binary, root, encoding, config, cases=ORGANIZE, kinds=KINDS):
    peer = ConfiguredPeer([str(binary), '--lsp', '--stdio'], root, config)
    result = []
    try:
        peer.request('initialize', {'processId':None,'rootUri':root.as_uri(),'capabilities':{
            'general':{'positionEncodings':[encoding]},'workspace':{'configuration':True}},
            'initializationOptions':{'disablePushDiagnostics':True}})
        peer.send('initialized', {})
        uri = (root/'main.ts').as_uri()
        for index, text in enumerate(cases):
            peer.send('textDocument/didOpen', {'textDocument':{'uri':uri,'languageId':'typescript','version':1,'text':text}})
            for kind in kinds:
                response = peer.request('textDocument/codeAction', {'textDocument':{'uri':uri},
                    'range':{'start':position(text,0,encoding),'end':position(text,len(text),encoding)},
                    'context':{'diagnostics':[], 'only':[kind]}})
                applied = [{name:apply(text if name == uri else Path(name.removeprefix('file://')).read_text(), edits, encoding)
                            for name, edits in action.get('edit',{}).get('changes',{}).items()} for action in response or []]
                result.append([index, kind, response, applied])
            peer.send('textDocument/didClose', {'textDocument':{'uri':uri}})
        peer.request('shutdown'); peer.send('exit')
    finally:
        peer.close()
    return result

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rust',type=Path,default=ROOT/'target/debug/tsrust')
    parser.add_argument('--go',type=Path,default=ROOT/'target/phase5/go-lsp')
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='tsr-l5-actions-') as directory:
        root=Path(directory).resolve()
        (root/'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"module":"esnext"},"files":["main.ts","dep.ts"]}')
        (root/'main.ts').write_text('')
        (root/'dep.ts').write_text('export const a=1, z=2, unused=3; export default class D{}; export interface Shape {}')
        count=0
        for encoding in ['utf-8','utf-16']:
            for config in [{}, {'organizeImportsSort':'ordinal'}, {'organizeImportsSort':'natural'},
                           {'organizeImportsSort':'naturalIgnoreCase'}, {'organizeImportsCollation':'unicode','organizeImportsNumericCollation':True},
                           {'organizeImportsCollation':'unicode','organizeImportsAccentCollation':False,'organizeImportsCaseFirst':'upper'},
                           {'organizeImportsTypeOrder':'first'},
                           {'preferences':{'organizeImports':{'sort':'ordinalIgnoreCase','typeOrder':'inline'}}},
                           {'preferences':{'organizeImports':{'unicodeCollation':'unicode','caseSensitivity':'caseSensitive','numericCollation':True,'accentCollation':False,'caseFirst':'upper'}}},
                           {'unstable':{'organizeImportsSort':'naturalIgnoreCase'}}]:
                go=run(args.go,root,encoding,config)
                rust=run(args.rust,root,encoding,config)
                if go != rust:
                    for (g,r) in zip(go,rust):
                        if g != r:
                            print(encoding,config, 'case',g[0])
                            print('\n'.join(difflib.unified_diff(json.dumps(g,indent=2,ensure_ascii=False).splitlines(),json.dumps(r,indent=2,ensure_ascii=False).splitlines(),fromfile='Go',tofile='Rust')))
                    raise SystemExit(1)
                count+=len(go)
        print(f'{count} organize-import native comparisons passed')
if __name__=='__main__': main()
