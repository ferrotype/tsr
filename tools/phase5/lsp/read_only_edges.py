#!/usr/bin/env python3
"""Read-only JS/JSX, implicit imports and suggestion diagnostics at the pin."""
import difflib
import json
import re
import tempfile
from pathlib import Path
from interop import Peer, ROOT
from read_only import position
FILES={
    'common.js':'/*😀*/ const item=1; exports.item=item; module.exports.fn=function fn(p){return p+item;};',
    'use.js':'const {item:renamed,fn}=require("./common"); fn(renamed); const ns=require("./common"); ns.item;',
    'docs.js':'/** @typedef {{value: number}} Shape */\n/** @param {Shape} obj */\nfunction f(obj){return obj.value;}\nconst shape={value:1}; f(shape);',
    'view.tsx':'import {Component} from "./component"; export const view=<Component value={1}>text</Component>;',
    'component.tsx':'export function Component(props:{value:number;children?:string}){return <div>{props.value}</div>;}',
    'plain.ts':'export async function plain(){return 1;}\nexport class Base {}\nexport class Derived extends Base {}',
    'suggest.ts':'/** @deprecated use newer */\nfunction old(value:number){return value;}\nfunction unused(value:string){const ignored=1; return old(1);}\nexport const result = old(2);',
    'node_modules/react/jsx-runtime.d.ts':'export namespace JSX {interface IntrinsicElements {div:any;} interface Element {}} export declare function jsx(type:any,props:any): JSX.Element; export declare function jsxs(type:any,props:any): JSX.Element;',
    'node_modules/tslib/tslib.d.ts':'export declare function __awaiter(...args:any[]):any; export declare function __extends(...args:any[]):any;',
    'node_modules/tslib/package.json':'{"name":"tslib","types":"tslib.d.ts"}',
}

def run(binary,root,encoding):
    peer=Peer([str(binary),'--lsp','--stdio'],root)
    try:
        peer.request('initialize',{'processId':None,'rootUri':root.as_uri(),'capabilities':{'general':{'positionEncodings':[encoding]},'textDocument':{'hover':{'contentFormat':['markdown']},'diagnostic':{'relatedInformation':True,'tagSupport':{'valueSet':[1,2]}}}}})
        peer.send('initialized',{});rows=[]
        for name,text in FILES.items():
            if name.endswith('.json'):continue
            uri=(root/name).as_uri()
            if not name.startswith('node_modules'):
                peer.send('textDocument/didOpen',{'textDocument':{'uri':uri,'languageId':'javascript' if name.endswith('.js') else 'typescriptreact' if name.endswith('.tsx') else 'typescript','version':1,'text':text}})
            response=peer.exchange('textDocument/diagnostic',{'textDocument':{'uri':uri}});response.pop('id',None);rows.append((name,0,'diagnostic',response))
            # Source-file position and every identifier/string token include
            # implicit JSX-runtime/tslib module references and CommonJS aliases.
            offsets=sorted({0,*[m.start() for m in re.finditer(r'[A-Za-z_$][\w$]*',text)]})
            for offset in offsets:
                params={'textDocument':{'uri':uri},'position':position(text,offset,encoding)}
                for method,extra in [('textDocument/references',{'context':{'includeDeclaration':True}}),('textDocument/documentHighlight',{}),('textDocument/hover',{}),('textDocument/definition',{})]:
                    response=peer.exchange(method,{**params,**extra});response.pop('id',None);rows.append((name,offset,method,response))
        peer.request('shutdown');peer.send('exit');return rows
    finally:peer.close()
def main():
    with tempfile.TemporaryDirectory(prefix='tsr-read-only-edges-') as folder:
        root=Path(folder).resolve()
        for name,text in FILES.items():file=root/name;file.parent.mkdir(parents=True,exist_ok=True);file.write_text(text)
        files=[n for n in FILES if not n.startswith('node_modules')]
        (root/'tsconfig.json').write_text(json.dumps({'compilerOptions':{'noLib':True,'strict':True,'module':'esnext','moduleResolution':'node','allowJs':True,'checkJs':True,'jsx':'react-jsx','importHelpers':True},'files':files}))
        for encoding in ('utf-8','utf-16'):
            go=run(ROOT/'target/phase5/go-lsp',root,encoding);rust=run(ROOT/'target/debug/tsrust',root,encoding)
            if go!=rust:
                out=ROOT/'target/phase5/read-only-edges-diff';out.mkdir(exist_ok=True)
                for name,rows in [('Go',go),('Rust',rust)]: (out/f'{name}.json').write_text(json.dumps(rows,indent=2,ensure_ascii=False))
                diffs=[(a,b) for a,b in zip(go,rust,strict=True) if a!=b]
                print(f'{len(diffs)} differing of {len(go)} responses; first differences:')
                for a,b in diffs[:8]:print(''.join(difflib.unified_diff(json.dumps(a,indent=2).splitlines(True),json.dumps(b,indent=2).splitlines(True),fromfile='Go',tofile='Rust')))
                raise SystemExit(f'read-only edges differ ({encoding})')
            print(f'{len(go)} JS/JSX/suggestion responses match Go ({encoding})',flush=True)
if __name__=='__main__':main()
