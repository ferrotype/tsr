#!/usr/bin/env python3
"""Native source/definition navigation, package forwarding and declaration maps."""
import base64
import difflib
import json
import re
import tempfile
from pathlib import Path
from interop import Peer, ROOT
from read_only import position

SOURCES = [
    '/*😀*/ import {foo, Box, shared} from "pkg"; foo(); const b = new Box(); b.value; shared;',
    'import Default from "pkg"; const value = new Default(); value.value;',
    'import {renamed} from "forward"; renamed();',
    'import {mapped} from "./mapped"; mapped();',
    'import {inline} from "./inline"; inline();',
    'import {strange} from "./strange"; strange();',
    '/// <reference path="./mapped.d.ts"/>\nconst x = 1;',
    'import type {OnlyType} from "pkg"; const x:OnlyType = {p:1}; x.p;',
    'const obj={p:1}; obj.p; function f(){return obj;} f();',
    'import * as ns from "pkg"; ns.foo();',
    'import {broken} from "./broken"; broken();',
    'import {chain} from "./chain"; chain();',
]
FILES={
    'main.ts':'',
    'node_modules/pkg/package.json':json.dumps({'name':'pkg','types':'index.d.ts','main':'runtime.js'}),
    'node_modules/pkg/index.d.ts':'export declare function foo(): number; export declare class Box {value:number;} export default Box; export declare const shared:number; export interface OnlyType{p:number}',
    'node_modules/pkg/runtime.js':'export function foo(){return 1;} export class Box{value=1;} export default Box; export const shared=2;',
    'node_modules/forward/package.json':json.dumps({'name':'forward','types':'index.d.ts','main':'entry.js'}),
    'node_modules/forward/index.d.ts':'export declare function renamed(): number;',
    'node_modules/forward/entry.js':'export * from "./more.js";',
    'node_modules/forward/more.js':'export function renamed(){return 1;} export * from "./entry.js";',
    'src/mapped.ts':'/*😀*/ export function mapped() {return 1;}\n',
    'src/inline.ts':'export function inline() {return 2;}\n',
    'src/original.weird':'export function strange() {return 3;}\n',
    'src/chain.ts':'export function chain() {return 4;}\n',
    'broken.d.ts':'export declare function broken(): number;\n//# sourceMappingURL=broken.d.ts.map',
    'broken.d.ts.map':'{not valid json',
}
def vlq(value):
    chars='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
    value=(-value*2+1) if value<0 else value*2
    result=''
    while value>31: result+=chars[32+(value&31)];value >>= 5
    return result+chars[value]
def add_map(files,name,target,inline=False):
    text=f'export declare function {name}(): number;\n'
    original=files[target];start=text.index(name);target_start=len(original[:original.index(name)].encode('utf-16-le'))//2
    mappings='AAAA,'+','.join([vlq(start)+'AA'+vlq(target_start),vlq(len(name))+'AA'+vlq(len(name))])
    data={'version':3,'file':f'{name}.d.ts','sources':[target],'names':[],'mappings':mappings}
    if inline:
        url='data:application/json;base64,'+base64.b64encode(json.dumps(data).encode()).decode()
    else:
        url=f'{name}.d.ts.map';files[url]=json.dumps(data)
    files[f'{name}.d.ts']=text+'//# sourceMappingURL='+url
add_map(FILES,'mapped','src/mapped.ts')
add_map(FILES,'inline','src/inline.ts',True)
add_map(FILES,'strange','src/original.weird')
add_map(FILES,'chain','src/chain.ts')
# A second declaration map exercises map chaining before loading original syntax.
FILES['chain-middle.d.ts']=FILES['chain.d.ts']
FILES['chain-middle.d.ts.map']=FILES['chain.d.ts.map'].replace('chain.d.ts','chain-middle.d.ts')
FILES['chain-middle.d.ts']=FILES['chain-middle.d.ts'].replace('chain.d.ts.map','chain-middle.d.ts.map')
FILES['chain.d.ts.map']=json.dumps({'version':3,'file':'chain.d.ts','sources':['chain-middle.d.ts'],'names':[],'mappings':'AAAA,wBAAwB,KAAK'})

def run(binary,root,encoding,links):
    peer=Peer([str(binary),'--lsp','--stdio'],root)
    try:
        peer.request('initialize',{'processId':None,'rootUri':root.as_uri(),'capabilities':{'general':{'positionEncodings':[encoding]},'workspace':{'configuration':True},'textDocument':{'definition':{'linkSupport':links}}}})
        peer.send('initialized',{});uri=(root/'main.ts').as_uri();rows=[]
        for index,text in enumerate(SOURCES):
            if index==0:peer.send('textDocument/didOpen',{'textDocument':{'uri':uri,'languageId':'typescript','version':1,'text':text}})
            else:peer.send('textDocument/didChange',{'textDocument':{'uri':uri,'version':index+1},'contentChanges':[{'text':text}]})
            for match in re.finditer(r'[A-Za-z_$][\w$]*',text):
                params={'textDocument':{'uri':uri},'position':position(text,match.start(),encoding)}
                for method in ('textDocument/definition','custom/textDocument/sourceDefinition'):
                    response=peer.exchange(method,params);response.pop('id',None);rows.append((index,match.start(),method,response))
        peer.send('workspace/didChangeConfiguration',{'settings':{'js/ts':{'preferGoToSourceDefinition':True}}})
        for match in re.finditer(r'\bchain\b',SOURCES[-1]):
            params={'textDocument':{'uri':uri},'position':position(SOURCES[-1],match.start(),encoding)}
            response=peer.exchange('textDocument/definition',params);response.pop('id',None);rows.append(('preference',match.start(),response))
        peer.request('shutdown');peer.send('exit');return rows
    finally:peer.close()
def main():
    with tempfile.TemporaryDirectory(prefix='tsr-source-definition-') as folder:
        root=Path(folder).resolve()
        for name,text in FILES.items():file=root/name;file.parent.mkdir(parents=True,exist_ok=True);file.write_text(text)
        (root/'tsconfig.json').write_text('{"compilerOptions":{"noLib":true,"strict":true,"module":"nodenext"},"files":["main.ts"]}')
        for encoding in ('utf-8','utf-16'):
            for links in (False,True):
                go=run(ROOT/'target/phase5/go-lsp',root,encoding,links);rust=run(ROOT/'target/debug/tsrust',root,encoding,links)
                if go!=rust:
                    out=ROOT/'target/phase5/source-definition-diff';out.mkdir(exist_ok=True)
                    for name,rows in [('Go',go),('Rust',rust)]: (out/f'{name}.json').write_text(json.dumps(rows,indent=2,ensure_ascii=False))
                    diffs=[(a,b) for a,b in zip(go,rust,strict=True) if a!=b]
                    print(f'{len(diffs)} differing of {len(go)} responses; first differences:')
                    for a,b in diffs[:8]:print(''.join(difflib.unified_diff(json.dumps(a,indent=2).splitlines(True),json.dumps(b,indent=2).splitlines(True),fromfile='Go',tofile='Rust')))
                    raise SystemExit(f'source definitions differ ({encoding}, links={links})')
                print(f'{len(go)} source/definition responses match Go ({encoding}, links={links})',flush=True)
if __name__=='__main__':main()
