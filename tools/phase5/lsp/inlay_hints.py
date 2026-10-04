#!/usr/bin/env python3
"""Compare complete inlay labels, links, settings and edit responses with Go."""
import difflib
import json
import tempfile
from pathlib import Path
from interop import Peer, ROOT
from read_only import position

SOURCES = [
    '/*😀*/ let x=1; let text="hello"; const v=1; const y=x+1; let number=1; enum E { A,B=4,C }',
    'function f(a:number,b:string,...values:boolean[]){return a;} f(1,"😀",true,false); const a=1; f(a,"",true); f(/*a*/ 1, "");',
    'interface Array<T>{length:number;[n:number]:T} declare function f(...args:[a:string,b:number]):void; f("",1); declare const xs:[string,number]; f(...xs);',
    'declare function use(cb:(a:number,b:string)=>number):void; use((a,b)=>a); use(function(a,b){return a;}); const f=(a=1)=>a; const g=a=>a;',
    'class C {x=1; value!:string; method(){return this.x;} get count(){return 1;} } let c=new C(); let C=new C();',
    'import {Box, make} from "./other"; let box=make(); let copy=box; let pair=[box,make()] as const; function identity(){return box;}',
    'declare let x:number|string; function pred(x:number|string){return typeof x==="string";} function inferred(){return x;} let o={x:1,y:"s"};',
    'interface Array<T>{length:number;[n:number]:T} let tuples:[a:number,b?:string,...rest:boolean[]]; let mapped: {[K in "a"|"b"]: number}; let copy=mapped; let tuple=tuples;',
    'import {Box} from \'./other\'; let a= null as "x"|"y"; let t=null as `a${string}`; let f=null as <T extends Box>(x:T)=>T;',
    'declare function f(a:number,b:string):void; f(1,"unfinished',
]
RAW = {
    'includeInlayParameterNameHints':'all',
    'includeInlayParameterNameHintsWhenArgumentMatchesName':True,
    'includeInlayFunctionParameterTypeHints':True,
    'includeInlayVariableTypeHints':True,
    'includeInlayVariableTypeHintsWhenTypeMatchesName':True,
    'includeInlayPropertyDeclarationTypeHints':True,
    'includeInlayFunctionLikeReturnTypeHints':True,
    'includeInlayEnumMemberValueHints':True,
}
NESTED = {'inlayHints':{
    'parameterNames':{'enabled':'literals','suppressWhenArgumentMatchesName':True},
    'parameterTypes':{'enabled':True},'variableTypes':{'enabled':True,'suppressWhenTypeMatchesName':True},
    'propertyDeclarationTypes':{'enabled':True},'functionLikeReturnTypes':{'enabled':True},'enumMemberValues':{'enabled':True}}}

def run(binary, root, encoding):
    peer=Peer([str(binary),'--lsp','--stdio'],root)
    try:
        peer.request('initialize',{'processId':None,'rootUri':root.as_uri(),'capabilities':{
            'general':{'positionEncodings':[encoding]},'workspace':{'configuration':True,'inlayHint':{'refreshSupport':True}}}})
        peer.send('initialized',{})
        uri=(root/'main.ts').as_uri()
        peer.send('textDocument/didOpen',{'textDocument':{'uri':uri,'languageId':'typescript','version':1,'text':''}})
        rows=[]
        version=1
        for setting in [{}, RAW, {**RAW, 'quotePreference':'single'}, {**NESTED,'unstable':RAW}, {**RAW,'quotePreference':'double'}, {'includeInlayVariableTypeHints':False}, {}]:
            peer.send('workspace/didChangeConfiguration',{'settings':{'js/ts':setting}})
            for i,text in enumerate(SOURCES):
                version+=1
                peer.send('textDocument/didChange',{'textDocument':{'uri':uri,'version':version},'contentChanges':[{'text':text}]})
                for start,end in [(0,len(text)),(len(text)//3,len(text)//2)]:
                    params={'textDocument':{'uri':uri},'range':{'start':position(text,start,encoding),'end':position(text,end,encoding)}}
                    rows.append((i,start,end,peer.request('textDocument/inlayHint',params)))
            rows.append(('refreshes',peer.server_requests.count('workspace/inlayHint/refresh')))
        peer.request('shutdown');peer.send('exit')
        rows.append(('refreshes',peer.server_requests.count('workspace/inlayHint/refresh')))
        return rows
    finally:peer.close()

def main():
    with tempfile.TemporaryDirectory(prefix='tsr-inlay-') as folder:
        root=Path(folder).resolve()
        (root/'main.ts').write_text('')
        (root/'other.ts').write_text('export class Box { value=1; } export function make(){return new Box();}')
        (root/'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"strict":true},"files":["main.ts","other.ts"]}')
        for encoding in ('utf-8','utf-16'):
            go=run(ROOT/'target/phase5/go-lsp',root,encoding)
            rust=run(ROOT/'target/debug/tsrust',root,encoding)
            if go!=rust:
                out=ROOT/'target/phase5/inlay-diff';out.mkdir(exist_ok=True)
                for name,rows in [('Go',go),('Rust',rust)]:
                    (out/f'{name}.json').write_text(json.dumps(rows,indent=2,ensure_ascii=False))
                print(''.join(difflib.unified_diff(json.dumps(go,indent=2).splitlines(True),json.dumps(rust,indent=2).splitlines(True),fromfile='Go',tofile='Rust'))[:16000])
                raise SystemExit(f'inlay hints differ ({encoding})')
            print(f'{len(rust)-1} inlay responses and refreshes match Go ({encoding})',flush=True)
if __name__=='__main__':main()
