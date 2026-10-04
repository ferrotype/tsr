#!/usr/bin/env python3
"""L5 fixes against pinned Go: identical diagnostics, edits and applied text."""
import argparse
import difflib
import json
import tempfile
from pathlib import Path
from completions import ConfiguredPeer
from edits import apply
from interop import ROOT
from read_only import position

# Each request uses the native diagnostic verbatim, including its span and text.
# That separates editing fidelity from the checker's diagnostic production.
CASES = [
    ('imports', 'a; z; new D(); let x: Shape;\n', {}),
    ('existing', 'import { a } from "./dep";\nz; a;\n', {}),
    ('namespace', 'import * as dep from "./dep";\na; z;\n', {}),
    ('promote-specifier', 'import { type D, type Shape } from "./dep";\nnew D();\n', {}),
    ('promote-clause', 'import type {D, Shape} from "./dep";\nnew D();\n', {'verbatimModuleSyntax':True}),
    ('promote-default', 'import type D from "./dep";\nnew D();\n', {}),
    ('class-properties', 'interface I {x: string; optional?: number; method(a: number): boolean}\nclass C implements I {}\n', {}),
    ('class-constructor', 'interface I { x: number; foo(): string }\nclass C implements I { constructor() {} }\n', {}),
    ('class-overloads', 'interface I { foo(a: number): string; foo(a: string, b?: boolean): number; }\nclass C implements I {}\n', {}),
    ('class-index', 'interface I { [key: string]: number; x: number }\nclass C implements I {}\n', {}),
    ('class-many', 'interface I {x: string} interface J {y: number}\nclass C implements I,J {}\nclass D implements I {}\n', {}),
    ('class-imports', 'import {I} from "./dep";\nclass C implements I {}\n', {}),
    ('class-optional-method', 'interface I { foo?(): void; "a-b": number }\nclass C implements I {}\n', {}),
    ('isolated-variable', 'export const value = make();\nfunction make() { return {field: 1}; }\n', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-functions', 'export function f() { return {a: 1}; }\nexport const fn = x => x;\n', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-default', 'const value = 1; export default value + 1;\n', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-destructure', 'const value = {a:1,b:"x"}; export const {a,b} = value;\n', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-literal', 'const value=1; export const obj = {value};\nexport const array = [1, "x"];\n', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-spreads', 'const value={a:1}; export const obj={...value,b:2};\nconst arr=[1,2]; export const values=[...arr,3];\n', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-class', 'const make = () => class {x=1};\nexport class C extends make() {}\n', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-expando', 'export function f() {}\nf.prop=1;\n', {'isolatedDeclarations':True,'declaration':True}),
    ('class-mapped', 'type Shape = { [P in "x"|"y"]: number }; class C implements Shape {}', {}),
    ('class-accessors', 'interface Shape { get x(): number; set x(value: number); constructor(): void } class C implements Shape {}', {}),
    ('class-generic', 'interface Shape { method<T extends {x: number} = {x: number}>(value: T): T } class C implements Shape {}', {}),
    ('class-existing', 'class Base {public x=1; protected y=2; private z=3} interface Shape {x:number; y:number; z:number} class C extends Base implements Shape {z=1}', {}),
    ('isolated-generic', 'interface Box<T=string>{value:T}; const make=():Box => ({value:"x"}); export const value=make();', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-predicate', 'export function isString(value: unknown) {return typeof value === "string"}', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-params', 'export const foo = (value={a:1}) => value; export function fn(value=[1]) {return value;}', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-destructure-default', 'const object={a:1,b:{c:"x"}}; export const {a=0,b:{c}}=object, last=1;', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-destructure-array', 'const array=[1,2,3]; export const [,a,...rest]=array;', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-property', 'const local=()=>1; export const object={a:local(), b:[1,2],c:{d:local()}};', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-imports', 'import {makeResult} from "./dep"; export const value=makeResult();', {'isolatedDeclarations':True,'declaration':True}),
    ('isolated-unique', 'declare const unique: unique symbol; export const value = unique;', {'isolatedDeclarations':True,'declaration':True}),
    ('jsx-imports', 'const el=<Widget />;', {'jsx':'react'}),
    ('jsx-casefold', 'const el=<WIDGET />;', {'jsx':'react'}),
    ('js-imports', '/** @type {Shape} */ let value; new D();', {'allowJs':True,'checkJs':True}),
    ('js-implements', '/** @implements {import("./dep").I} */ class C {}', {'allowJs':True,'checkJs':True}),
    ('crlf', '/*😀*/ interface I {x: number; f(): void}\r\nclass C implements I {}\r\n', {}),
]
DEPENDENCY='export const a=1,z=2; export class D{}; export default D; export interface Shape {}\nexport interface Result{value:number} export interface I { first: Result; second: Result; foo(): Result } export const makeResult=():Result=>({value:1}); export const Widget=()=>null;'

def run(binary, root, encoding, text, native=None, config=None, filename="main.ts", locale="en"):

    peer=ConfiguredPeer([str(binary),'--lsp','--stdio'],root,config or {})
    uri=(root/filename).as_uri()
    result=[]
    try:
        peer.request('initialize',{'processId':None,'rootUri':root.as_uri(),'locale':locale,'capabilities':{
            'general':{'positionEncodings':[encoding]},'workspace':{'configuration':True},
            'textDocument':{'diagnostic':{'relatedDocumentSupport':True}}},
            'initializationOptions':{'disablePushDiagnostics':True}})
        peer.send('initialized',{})
        peer.send('textDocument/didOpen',{'textDocument':{'uri':uri,'languageId':'javascript' if filename.endswith('.js') else 'typescript','version':1,'text':text}})
        if native is None:
            report=peer.request('textDocument/diagnostic',{'textDocument':{'uri':uri}})
            native=report.get('items',[])
        schedules=[([d],['quickfix']) for d in native]
        schedules.extend([(native,['quickfix']),([],['source.fixAll.ts'])])
        for diagnostics,only in schedules:
            response=peer.request('textDocument/codeAction',{'textDocument':{'uri':uri},
                'range':{'start':position(text,0,encoding),'end':position(text,len(text),encoding)},
                'context':{'diagnostics':diagnostics,'only':only}})
            # Go iterates the fix-all provider map; its order is immaterial.
            response=sorted(response or [],key=lambda a:(a['title'],json.dumps(a,sort_keys=True)))
            applied=[{name:apply(text if name==uri else Path(name.removeprefix('file://')).read_text(),edits,encoding)
                      for name,edits in a.get('edit',{}).get('changes',{}).items()} for a in response]
            result.append([diagnostics,only,response,applied])
        peer.request('shutdown');peer.send('exit')
    finally: peer.close()
    return native,result

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--rust',type=Path,default=ROOT/'target/debug/tsrust');p.add_argument('--go',type=Path,default=ROOT/'target/phase5/go-lsp')
    p.add_argument('--config',type=json.loads,default={});p.add_argument('--locale',default='en');p.add_argument('--case',action='append');p.add_argument('--encoding',action='append');args=p.parse_args()
    count=0;failures=0
    with tempfile.TemporaryDirectory(prefix='tsr-l5-fixes-') as directory:
        root=Path(directory).resolve();(root/'dep.ts').write_text(DEPENDENCY);(root/'main.ts').write_text('')
        for name,text,options in CASES:
            if args.case and name not in args.case:continue
            filename='main.tsx' if name.startswith('jsx-') else 'main.js' if name.startswith('js-') else 'main.ts'
            (root/filename).write_text('')
            files=[filename,'dep.ts']
            if name=='jsx-casefold':
                (root/'lower.ts').write_text('export default function widget() { return null; }')
                files.append('lower.ts')
            (root/'tsconfig.json').write_text(json.dumps({'compilerOptions':{'noLib':True,'module':'esnext','strict':True,**options},'files':files}))
            for encoding in args.encoding or ['utf-8','utf-16']:
                native,go=run(args.go,root,encoding,text,filename=filename,config=args.config,locale=args.locale)
                try: _,rust=run(args.rust,root,encoding,text,native,filename=filename,config=args.config,locale=args.locale)
                except Exception as error:
                    print(name,encoding,repr(error),flush=True);failures+=1;continue
                if go!=rust:
                    failures+=1;print(name,encoding,flush=True)
                    print('\n'.join(difflib.unified_diff(json.dumps(go,indent=2,ensure_ascii=False).splitlines(),json.dumps(rust,indent=2,ensure_ascii=False).splitlines(),fromfile='Go',tofile='Rust')),flush=True)
                count+=len(go)
    print(f'{count} native quick-fix requests, {failures} differing cases',flush=True)
    if failures:raise SystemExit(1)
if __name__=='__main__':main()
