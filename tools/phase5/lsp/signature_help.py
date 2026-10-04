#!/usr/bin/env python3
"""Focused native signature-help comparisons, with live edits between requests."""
import difflib
import json
import tempfile
from pathlib import Path
from interop import Peer, ROOT
from read_only import position

SOURCES = [
    'function f(a: number, b?: string): number {return a;} f(1, "😀"); f(); f(1,);',
    'function f(a:string): string; function f(a:number,b:boolean):number; function f(a:any,b?:any){return a;} f(1, true);',
    'function f<T extends {x:number}, U = string>(a:T, b:U):U {return b;} f<{x:number}, string>({x:1}, ""); f<',
    'interface Pair<A,B=string>{a:A;b:B} type P=Pair<number,string>; let x:Pair<number,',
    'declare function f(cb:(a:number,b:string)=>number):void; f((a,b)=>a); f(function(a,b){return a;});',
    'interface Array<T>{length:number;[n:number]:T} declare function f(...args:[a:string,b:number]):void; f("",1); declare const xs:[string,number]; f(...xs,);',
    'interface Array<T>{length:number;[n:number]:T} declare function f(...args:[prefix:string,...middle:number[],suffix:boolean]):void; f("",1,2,true);',
    'declare function tag(strings:any, a:number,b:string):void; tag`x${1}y${"😀"}z`; tag`plain`;',
    '/** A function. {@link f}\n * @param a first\n * @param b second\n */ function f(a:number,b:string):boolean{return true;} f(1,"");',
    'declare function f(cb:((x:number)=>string)|undefined):void; f((x)=>String(x));',
    'declare function f(a:number):void; f(/* here */ 1); f("unfinished',
]

def run(binary, root, encoding, vs):
    peer=Peer([str(binary),'--lsp','--stdio'],root)
    try:
        peer.request('initialize',{'processId':None,'rootUri':root.as_uri(),'capabilities':{
            'general':{'positionEncodings':[encoding]},'_vs_supportsVisualStudioExtensions':vs,
            'textDocument':{'signatureHelp':{'contextSupport':True,'signatureInformation':{
                'documentationFormat':['plaintext' if vs else 'markdown'],
                'activeParameterSupport':vs,'noActiveParameterSupport':vs}}}}})
        peer.send('initialized',{})
        uri=(root/'main.ts').as_uri()
        rows=[]
        for index,text in enumerate(SOURCES):
            if index==0:
                peer.send('textDocument/didOpen',{'textDocument':{'uri':uri,'languageId':'typescript','version':1,'text':text}})
            else:
                peer.send('textDocument/didChange',{'textDocument':{'uri':uri,'version':index+1},'contentChanges':[{'text':text}]})
            for offset in range(len(text)+1):
                params={'textDocument':{'uri':uri},'position':position(text,offset,encoding)}
                rows.append((index,offset,'invoked',peer.request('textDocument/signatureHelp',{**params,'context':{'triggerKind':1,'isRetrigger':False}})))
                if offset and text[offset-1] in '(<,':
                    for retrigger in (False,True):
                        rows.append((index,offset,'retrigger' if retrigger else 'typed',peer.request('textDocument/signatureHelp',{**params,'context':{'triggerKind':2,'triggerCharacter':text[offset-1],'isRetrigger':retrigger}})))
        peer.request('shutdown'); peer.send('exit')
        return rows
    finally:peer.close()

def main():
    with tempfile.TemporaryDirectory(prefix='tsr-signature-') as folder:
        root=Path(folder).resolve()
        (root/'main.ts').write_text('')
        (root/'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"strict":true},"files":["main.ts"]}')
        for encoding in ('utf-8','utf-16'):
            for vs in (False,True):
                go=run(ROOT/'target/phase5/go-lsp',root,encoding,vs)
                rust=run(ROOT/'target/debug/tsrust',root,encoding,vs)
                if go!=rust:
                    out=ROOT/'target/phase5/signature-diff';out.mkdir(exist_ok=True)
                    for name,rows in [('Go',go),('Rust',rust)]:
                        (out/f'{name}.json').write_text(json.dumps(rows,indent=2,ensure_ascii=False))
                    print(''.join(difflib.unified_diff(json.dumps(go,indent=2).splitlines(True),json.dumps(rust,indent=2).splitlines(True),fromfile='Go',tofile='Rust'))[:16000])
                    raise SystemExit(f'signature help differs ({encoding}, VS={vs})')
                print(f'{len(rust)} signature responses match Go ({encoding}, VS={vs})',flush=True)
if __name__=='__main__':main()
