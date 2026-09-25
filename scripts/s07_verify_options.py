#!/usr/bin/env python3
"""Direct observations of original Program.verifyCompilerOptions, without checking/emitting."""
import argparse,copy,hashlib,json,shutil
from pathlib import Path
from s04_common import command,strict_json_loads
from s06_build import ROOT,oracle_export

def fixtures():
    options=[{}, {'baseUrl':'/base'},{'outFile':'/out.js'},{'target':1},{'module':2},{'module':3},{'module':4},{'moduleResolution':1},{'alwaysStrict':False},{'esModuleInterop':False},{'allowSyntheticDefaultImports':False},{'moduleResolution':2},{'downlevelIteration':False},{'downlevelIteration':True},
    {'strictPropertyInitialization':True,'strictNullChecks':False},{'exactOptionalPropertyTypes':True,'strictNullChecks':False},{'isolatedDeclarations':True,'allowJs':True},{'inlineSourceMap':True,'sourceMap':True,'mapRoot':'/map'},{'composite':True,'declaration':False,'incremental':False},{'incremental':True},
    {'paths':{'**':None,'*':[],'x':['bare','a**','./x']}},{'inlineSources':True,'sourceRoot':'/source'},{'mapRoot':'/maps'},{'declarationDir':'/types'},{'declarationMap':True},{'lib':[]},{'isolatedModules':True,'preserveConstEnums':False},{'verbatimModuleSyntax':True,'preserveConstEnums':False},
    {'checkJs':True,'allowJs':False},{'emitDeclarationOnly':True},{'emitDecoratorMetadata':True},{'jsxFactory':'A..B','jsx':4,'reactNamespace':'bad-name'},{'reactNamespace':'bad-name'},{'jsxFragmentFactory':'A..B','jsx':5},{'reactNamespace':'A','jsx':4},{'jsxImportSource':'pkg','jsx':3},
    {'allowImportingTsExtensions':True,'noEmit':False},{'moduleResolution':1,'resolvePackageJsonExports':True,'resolvePackageJsonImports':True,'customConditions':[]},{'moduleResolution':100,'module':2},{'module':100,'moduleResolution':2},{'module':101,'moduleResolution':2},{'module':102,'moduleResolution':2},{'module':199,'moduleResolution':2},{'module':1,'moduleResolution':3},{'module':1,'moduleResolution':99},
    {'outDir':'/dist','configFilePath':'/tsconfig.json','noEmit':False},{'rootDir':'/other'},{'composite':True},{'allowJs':True,'noEmit':False},{'allowJs':True,'noEmit':False,'sourceMap':True,'declaration':True,'declarationMap':True},{'incremental':True,'tsBuildInfoFile':'/src/a.ts','noEmit':False}]
    rows=[]
    for index,extra in enumerate(options):
        for syntax in [False,True]:
            opts={'noLib':True,'noEmit':True,'target':99,**extra}
            row=dict(id=f'options/{index:02d}/{"syntax" if syntax else "command"}',cwd='/',case_sensitive=True,roots=['/src/a.ts','/src/a.js'],files={'/src/a.ts':b'export let value=1;'.hex(),'/src/a.js':b'let other=2;'.hex()},options=opts)
            if syntax: row.update(config_name='/tsconfig.json',config_text=json.dumps({'compilerOptions':opts},indent=2).encode().hex())
            rows.append(row)
    row=copy.deepcopy(rows[0]);row.update(id='options/duplicate-property-order',options={'noLib':True,'strictPropertyInitialization':True,'strictNullChecks':False},config_name='/tsconfig.json',config_text=b'{"compilerOptions":{"strictNullChecks":false,"strictPropertyInitialization":true,"strictNullChecks":true}}'.hex());rows.append(row)
    # Valid JSX factories (the entity-name parse succeeds) and package maps
    # under the resolutions that support them.
    extra=[('jsx-factories-valid',{'jsx':3,'jsxFactory':'React.createElement','jsxFragmentFactory':'React.Fragment'}),
           ('package-maps-node16',{'module':100,'moduleResolution':3,'resolvePackageJsonExports':True}),
           ('package-maps-nodenext',{'module':199,'moduleResolution':99,'customConditions':['c']}),
           ('package-maps-bundler',{'module':99,'moduleResolution':100,'resolvePackageJsonImports':True})]
    for name,extra_options in extra:
        for syntax in [False,True]:
            opts={'noLib':True,'noEmit':True,'target':99,**extra_options}
            row=dict(id=f'options/extra/{name}/{"syntax" if syntax else "command"}',cwd='/',case_sensitive=True,roots=['/src/a.ts','/src/a.js'],files={'/src/a.ts':b'export let value=1;'.hex(),'/src/a.js':b'let other=2;'.hex()},options=opts)
            if syntax: row.update(config_name='/tsconfig.json',config_text=json.dumps({'compilerOptions':opts},indent=2).encode().hex())
            rows.append(row)
    # Output paths on a case-insensitive host (an output that overwrites an
    # input spelled in another case; two outputs differing only in case), and
    # a TypeScript dependency found in node_modules, which is not emitted.
    files=lambda values:{name:text.encode().hex() for name,text in values.items()}
    rows.append(dict(id='outputs/case-insensitive-overwrite',cwd='/',case_sensitive=False,roots=['/src/x.ts','/src/X.js'],files=files({'/src/x.ts':'export let value=1;','/src/X.js':'let other=2;'}),options={'noLib':True,'noEmit':False,'target':99,'allowJs':True}))
    rows.append(dict(id='outputs/case-insensitive-collision',cwd='/',case_sensitive=False,roots=['/src/a.ts','/src/A.tsx'],files=files({'/src/a.ts':'export let value=1;','/src/A.tsx':'export let other=2;'}),options={'noLib':True,'noEmit':False,'target':99,'jsx':3}))
    rows.append(dict(id='outputs/external-library-not-emitted',cwd='/src/app',case_sensitive=True,roots=['/src/app/main.ts'],files=files({'/src/app/main.ts':'import "pkg";','/src/node_modules/pkg/index.ts':'export const p = 1;'}),options={'noLib':True,'noEmit':False,'target':99,'outDir':'/out','rootDir':'/src/app'}))
    rows.append(dict(id='outputs/external-library-control',cwd='/src/app',case_sensitive=True,roots=['/src/app/main.ts'],files=files({'/src/app/main.ts':'import "../pkg";','/src/pkg/index.ts':'export const p = 1;'}),options={'noLib':True,'noEmit':False,'target':99,'outDir':'/out','rootDir':'/src/app'}))
    return rows

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--check',action='store_true');parser.add_argument('--requests',type=Path);parser.add_argument('--output',type=Path);args=parser.parse_args()
    if bool(args.requests)!=bool(args.output):raise ValueError('requests/output must be specified together')
    requests=args.requests.read_bytes() if args.requests else (json.dumps(fixtures(),sort_keys=True,indent=2)+'\n').encode()
    adapters=[ROOT/'tools/s07/verify-options/export_test.go',ROOT/'tools/s07/program/export_test.go']
    with oracle_export() as (checkout,env,pin):
        request_path=checkout/'verify-requests.json';request_path.write_bytes(requests);output=checkout/'verify-observed.json'
        for index,adapter in enumerate(adapters):shutil.copyfile(adapter,checkout/f'tsc/internal/compiler/s07_verify_{index}_test.go')
        env.update(S07_VERIFY_REQUESTS=str(request_path),S07_VERIFY_OUTPUT=str(output))
        repo=f'-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={checkout}/s07-unmatched-prefix'
        command(['go','test','-trimpath','-mod=readonly',repo,'./internal/compiler','-run','^TestS07VerifyOptions$','-count=1'],cwd=checkout/'tsc',env=env)
        data=output.read_bytes()
        if [row['id'] for row in strict_json_loads(data)]!=[row['id'] for row in strict_json_loads(requests)]:raise ValueError('missing, extra or reordered verification observation')
        manifest=dict(pin=pin,requests_sha256=hashlib.sha256(requests).hexdigest(),observations_sha256=hashlib.sha256(data).hexdigest(),adapters={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in adapters},source_sha256=hashlib.sha256((checkout/'tsc/internal/compiler/program.go').read_bytes()).hexdigest())
    if args.output:
        args.output.write_bytes(data);args.output.with_suffix('.manifest.json').write_text(json.dumps(manifest,sort_keys=True,indent=2)+'\n');return
    for name,raw in {'verify-options-requests.json':requests,'verify-options-observations.json':data,'verify-options-manifest.json':(json.dumps(manifest,sort_keys=True,indent=2)+'\n').encode()}.items():
        path=ROOT/'data/s07'/name
        if args.check:
            if path.read_bytes()!=raw:raise ValueError('pinned option verification observations drift: '+name)
        else:path.write_bytes(raw)
    print(f'{len(fixtures())} direct Go option-verification requests')
if __name__=='__main__':main()
