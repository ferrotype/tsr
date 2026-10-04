#!/usr/bin/env python3
"""Compare prepare and both follow-up requests, including edits and imported calls."""
import difflib
import json
import re
import tempfile
from pathlib import Path
from interop import Peer, ROOT
from read_only import position

SOURCES = [
    '/*😀*/ function f(x:number){return x;} function g(){return f(1);} g(); const a=()=>f(2); let b=()=>f(3); b(); a();',
    'class A {constructor(){f();} static {f();} x=f(); m(){return f();} get p(){return f();} set p(v:number){f();}} function f(){return 1;} const a=new A(); a.m(); a.p; a.p=2;',
    'function f(x:string):string; function f(x:number):number; function f(x:any){return x;} function g(){return f(1);} f("x");',
    'namespace N {export function f(){return 1;} export const arrow=()=>f();} N.f(); N.arrow();',
    'const o={m(){f();}, get p(){f();return 1;},set p(v:number){f();}}; function f(){} o.m(); o.p; o.p=1;',
    'import {f as imported,C} from "./other"; export function f(){imported();const c=new C(); c.m();} f();',
    'export default function(){f();} function f(){} const C=class {m(){f();}};const c=new C();c.m();',
    'function f(){return 1;} function g(){ (f)(); f`x${f()}`; ((()=>f()))(); } g();',
]
OTHER = 'export function f(){return 1;} export class C {m(){return f();}}'

def run(binary, root, encoding):
    peer=Peer([str(binary),'--lsp','--stdio'],root)
    try:
        peer.request('initialize',{'processId':None,'rootUri':root.as_uri(),'capabilities':{'general':{'positionEncodings':[encoding]}}})
        peer.send('initialized',{});uri=(root/'main.ts').as_uri();rows=[]
        for index,text in enumerate(SOURCES):
            if index==0:peer.send('textDocument/didOpen',{'textDocument':{'uri':uri,'languageId':'typescript','version':1,'text':text}})
            else:peer.send('textDocument/didChange',{'textDocument':{'uri':uri,'version':index+1},'contentChanges':[{'text':text}]})
            for match in re.finditer(r'[A-Za-z_$][\w$]*',text):
                offset=match.start();params={'textDocument':{'uri':uri},'position':position(text,offset,encoding)}
                response=peer.exchange('textDocument/prepareCallHierarchy',params);response.pop('id',None)
                rows.append((index,offset,'prepare',response))
                for item in response.get('result') or []:
                    for method in ['incomingCalls','outgoingCalls']:
                        response=peer.exchange('callHierarchy/'+method,{'item':item});response.pop('id',None)
                        rows.append((index,offset,method,response))
        peer.request('shutdown');peer.send('exit');return rows
    finally:peer.close()

def main():
    with tempfile.TemporaryDirectory(prefix='tsr-call-hierarchy-') as folder:
        root=Path(folder).resolve();(root/'main.ts').write_text('');(root/'other.ts').write_text(OTHER)
        (root/'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"strict":true,"module":"esnext"},"files":["main.ts","other.ts"]}')
        for encoding in ['utf-8','utf-16']:
            go=run(ROOT/'target/phase5/go-lsp',root,encoding);rust=run(ROOT/'target/debug/tsrust',root,encoding)
            if go!=rust:
                out=ROOT/'target/phase5/call-hierarchy-diff';out.mkdir(exist_ok=True)
                for name,rows in [('Go',go),('Rust',rust)]: (out/f'{name}.json').write_text(json.dumps(rows,indent=2,ensure_ascii=False))
                print(''.join(list(difflib.unified_diff(json.dumps(go,indent=2).splitlines(True),json.dumps(rust,indent=2).splitlines(True),fromfile='Go',tofile='Rust'))[:350]))
                raise SystemExit(f'call hierarchy differs ({encoding}); {len(go)}/{len(rust)} responses')
            print(f'{len(go)} call hierarchy responses match Go ({encoding})',flush=True)
if __name__=='__main__':main()
