#!/usr/bin/env python3
"""Focused references and implementation comparisons; real files and live edits."""
import difflib
import json
import re
import tempfile
from pathlib import Path
from interop import Peer, ROOT
from read_only import position

SOURCES = [
    '/*😀*/ const value=1; function f(value:number){return value;} const o={value}; o.value; value;',
    'interface I {m():void;p:number} class A implements I {p=1;m(){}} class B extends A {m(){super.m();}} const x:I=new B(); x.m(); x.p;',
    'interface I {m():void;} const a:I={m(){}}; const b:I={m(){}}; declare const i:I; i.m();',
    'import {value as renamed, f, C} from "./other"; renamed; f(renamed); const c=new C(); c.m();',
    'import {value,f} from "./middle"; value; f(value); export {value as publicValue};',
    'const value=1; export {value}; const o={value}; let {value:other}=o; other; const {value:v}=o; v;',
    'namespace N {export const x=1;} N.x; interface X {p:number} const o:X={p:1}; o.p; const {p}=o; p;',
    'class C {constructor(){} static make(){return new this();} m(){return this;} } class D extends C {constructor(){super();} m(){return super.m();}} new C();',
    'let value:string|number; const text="literal"; const other="literal"; type T="literal"; let t:T="literal"; let q:string="literal";',
    'import C, {default as Alias} from "./default"; new C(); new Alias();',
    'function f(n:number){if(n) return 1; else if(n<0) throw new Error(); else return 2;} for(;;){switch(1){case 1:break;default:continue;}}',
    'async function f(){await x(); try {throw x;} catch(e){throw e;} finally{x();} return 0;} function* g(){yield 1;yield 2;}',
    'class A {public x=1; protected y=2; get p(){return 1;} set p(v:number){} constructor(public z:number){} }',
    'outer: for(;;){if(true)break outer; for(;;){continue outer;}} function f(this:{x:number}){return this.x;} import.meta;',
]
OTHER='export const value=1; export function f(x:number){return x;} export class C {m(){return value;}}'
MIDDLE='export {value,f} from "./other"; export * from "./other";'

def run(binary,root,encoding,links):
    peer=Peer([str(binary),'--lsp','--stdio'],root)
    try:
        peer.request('initialize',{'processId':None,'rootUri':root.as_uri(),'capabilities':{'_vs_supportsVisualStudioExtensions':links,
            'general':{'positionEncodings':[encoding]},'textDocument':{'implementation':{'linkSupport':links}}}})
        peer.send('initialized',{})
        uri=(root/'main.ts').as_uri();rows=[]
        for index,text in enumerate(SOURCES):
            if index==0:peer.send('textDocument/didOpen',{'textDocument':{'uri':uri,'languageId':'typescript','version':1,'text':text}})
            else:peer.send('textDocument/didChange',{'textDocument':{'uri':uri,'version':index+1},'contentChanges':[{'text':text}]})
            for match in re.finditer(r'[A-Za-z_$][\w$]*',text):
                offset=match.start();params={'textDocument':{'uri':uri},'position':position(text,offset,encoding)}
                for include in [True,False]:
                    response=peer.exchange('textDocument/references',{**params,'context':{'includeDeclaration':include}});response.pop('id',None)
                    rows.append((index,offset,'references',include,response))
                response=peer.exchange('textDocument/implementation',params);response.pop('id',None)
                rows.append((index,offset,'implementation',None,response))
                for method,extra in [('textDocument/_vs_references',{'context':{'includeDeclaration':False}}),('textDocument/documentHighlight',{}),('custom/textDocument/multiDocumentHighlight',{'filesToSearch':[uri,(root/'other.ts').as_uri(),uri]})]:
                    response=peer.exchange(method,{**params,**extra});response.pop('id',None);rows.append((index,offset,method,None,response))
        # Search from the defining file after several imports were edited in and out.
        for name,text in [('other.ts',OTHER),('middle.ts',MIDDLE)]:
            for match in re.finditer(r'\b(value|f|C|m)\b',text):
                params={'textDocument':{'uri':(root/name).as_uri()},'position':position(text,match.start(),encoding),'context':{'includeDeclaration':True}}
                response=peer.exchange('textDocument/references',params);response.pop('id',None);rows.append((name,match.start(),'references',True,response))
        peer.request('shutdown');peer.send('exit');return rows
    finally:peer.close()

def main():
    with tempfile.TemporaryDirectory(prefix='tsr-references-') as folder:
        root=Path(folder).resolve();(root/'main.ts').write_text('');(root/'other.ts').write_text(OTHER);(root/'middle.ts').write_text(MIDDLE)
        (root/'default.ts').write_text('export default class C {m(){}}');
        (root/'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"strict":true,"module":"esnext"},"files":["main.ts","other.ts","middle.ts","default.ts"]}')
        for encoding in ('utf-8','utf-16'):
            for links in (False,True):
                go=run(ROOT/'target/phase5/go-lsp',root,encoding,links);rust=run(ROOT/'target/debug/tsrust',root,encoding,links)
                if go!=rust:
                    out=ROOT/'target/phase5/references-diff';out.mkdir(exist_ok=True)
                    for name,rows in [('Go',go),('Rust',rust)]: (out/f'{name}.json').write_text(json.dumps(rows,indent=2,ensure_ascii=False))
                    diffs=[(a,b) for a,b in zip(go,rust,strict=True) if a!=b]
                    print(f'{len(diffs)} differing of {len(go)} responses; first differences:')
                    for a,b in diffs[:8]:
                        print(''.join(difflib.unified_diff(json.dumps(a,indent=2).splitlines(True),json.dumps(b,indent=2).splitlines(True),fromfile='Go',tofile='Rust')))
                    raise SystemExit(f'references differ ({encoding}, links={links})')
                print(f'{len(go)} responses match Go ({encoding}, links={links})',flush=True)
if __name__=='__main__':main()
