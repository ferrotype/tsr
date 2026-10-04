#!/usr/bin/env python3
"""Native code-lens lists, resolution commands, edits and settings refreshes."""
import difflib
import json
import tempfile
from pathlib import Path
from interop import Peer, ROOT

SOURCES=[
    '/*😀*/ export function f(){} function g(){f();} export const v=1; const local=2; g();',
    'interface I {m():void;p:number} abstract class A implements I {abstract m():void;p=1;} class B extends A {m(){}} const i:I=new B(); i.m();',
    'export class C {constructor(){} p=1; private q=2; #r=3; get value(){return this.p;} set value(x:number){this.p=x;} m(){}} const c=new C();c.m(); c.value;',
    'function f(x:number):number; function f(x:string):string; function f(x:any){return x;} f(1); f("x"); enum E{A,B} E.A; type T={p:number;m():void;};',
    'import {f} from "./other"; f(); export function g(){f();} g();',
]
SETTINGS=[{}, {'referencesCodeLensEnabled':True}, {'referencesCodeLensEnabled':True,'referencesCodeLensShowOnAllFunctions':True,'implementationsCodeLensEnabled':True,'implementationsCodeLensShowOnInterfaceMethods':True,'implementationsCodeLensShowOnAllClassMethods':True}, {'referencesCodeLens':{'enabled':False},'implementationsCodeLens':{'enabled':True,'showOnInterfaceMethods':True}}, {}]
def run(binary,root,encoding):
    peer=Peer([str(binary),'--lsp','--stdio'],root)
    try:
        peer.request('initialize',{'processId':None,'rootUri':root.as_uri(),'initializationOptions':{'codeLensShowLocationsCommandName':'editor.showReferences'},'capabilities':{'general':{'positionEncodings':[encoding]},'workspace':{'configuration':True,'codeLens':{'refreshSupport':True}}}})
        peer.send('initialized',{});uri=(root/'main.ts').as_uri();rows=[];version=1
        peer.send('textDocument/didOpen',{'textDocument':{'uri':uri,'languageId':'typescript','version':version,'text':''}})
        for setting in SETTINGS:
            peer.send('workspace/didChangeConfiguration',{'settings':{'js/ts':setting}})
            for index,text in enumerate(SOURCES):
                version+=1;peer.send('textDocument/didChange',{'textDocument':{'uri':uri,'version':version},'contentChanges':[{'text':text}]})
                response=peer.exchange('textDocument/codeLens',{'textDocument':{'uri':uri}});response.pop('id',None);rows.append((index,'lenses',response))
                for lens in response.get('result') or []:
                    response=peer.exchange('codeLens/resolve',lens);response.pop('id',None);rows.append((index,'resolved',response))
            rows.append(('refreshes',peer.server_requests.count('workspace/codeLens/refresh')))
        peer.request('shutdown');peer.send('exit');return rows
    finally:peer.close()
def main():
    with tempfile.TemporaryDirectory(prefix='tsr-code-lens-') as folder:
        root=Path(folder).resolve();(root/'main.ts').write_text('');(root/'other.ts').write_text('export function f(){}')
        (root/'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"strict":true,"module":"esnext"},"files":["main.ts","other.ts"]}')
        for encoding in ('utf-8','utf-16'):
            go=run(ROOT/'target/phase5/go-lsp',root,encoding);rust=run(ROOT/'target/debug/tsrust',root,encoding)
            if go!=rust:
                out=ROOT/'target/phase5/code-lens-diff';out.mkdir(exist_ok=True)
                for name,rows in [('Go',go),('Rust',rust)]: (out/f'{name}.json').write_text(json.dumps(rows,indent=2,ensure_ascii=False))
                print(''.join(list(difflib.unified_diff(json.dumps(go,indent=2).splitlines(True),json.dumps(rust,indent=2).splitlines(True),fromfile='Go',tofile='Rust'))[:300]))
                raise SystemExit(f'code lenses differ ({encoding}); {len(go)}/{len(rust)} responses')
            print(f'{len(go)} code-lens responses and refresh counts match Go ({encoding})',flush=True)
if __name__=='__main__':main()
