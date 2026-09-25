#!/usr/bin/env python3
"""Observe actual pinned file-inclusion helpers on loaded and constructed reason edges."""
import argparse
import hashlib
import json
import shutil
from s04_common import command, strict_json_loads
from s06_build import ROOT, oracle_export


def fixtures():
    rows = []
    def add(name, files, roots, paths, **options):
        row = dict(id=name, cwd='/src', case_sensitive=True,
                   files={path: (text if isinstance(text, bytes) else text.encode()).hex() for path, text in files.items()},
                   roots=roots, options=dict(noLib=True, target=99, **options),
                   queries=[dict(Path=path) for path in paths])
        rows.append(row)
        return row
    add('singleton-import', {'/src/main.ts':'import /*space*/ "./dep";', '/src/dep.ts':'export const x=1;'}, ['/src/main.ts'], ['/src/dep.ts','/src/main.ts','','/missing.ts'])
    row=add('reason-identity', {'/src/main.ts':'import "./dep"; import "./dep";', '/src/dep.ts':'export const x=1;'}, ['/src/main.ts','/src/dep.ts'], ['/src/dep.ts'])
    row['queries'] += [dict(Path='/src/dep.ts',Identity=identity) for identity in ['same','distinct']]
    row['queries'].append(dict(Path='/src/dep.ts',Preferred=1))
    add('references', {'/src/main.ts':'/// <reference path="./dep.ts" />\n/// <reference path="./dep.ts" />\n', '/src/dep.ts':'let x=1;'}, ['/src/main.ts'], ['/src/dep.ts'])
    add('type-reference-package-zero', {'/src/main.ts':'/// <reference types="pkg" />\n', '/src/node_modules/@types/pkg/package.json':'{"name":"@types/pkg","version":"1.0.0","types":"index.d.ts"}', '/src/node_modules/@types/pkg/index.d.ts':'declare const x:number;'}, ['/src/main.ts'], ['/src/node_modules/@types/pkg/index.d.ts'], types=[])
    add('automatic-type-package', {'/src/main.ts':'let x=1;', '/src/node_modules/@types/pkg/package.json':'{"name":"@types/pkg","version":"1.0.0","types":"index.d.ts"}', '/src/node_modules/@types/pkg/index.d.ts':'declare const x:number;'}, ['/src/main.ts'], ['/src/node_modules/@types/pkg/index.d.ts'], types=['*'])
    row=add('module-augmentation', {'/src/main.ts':'export {}; declare module "./dep" { interface T {} }', '/src/dep.ts':'export interface T {}'}, ['/src/main.ts','/src/dep.ts'], ['/src/dep.ts'])
    row['queries'][0]['AugmentationFrom']='/src/main.ts'
    package={'/src/node_modules/tslib/package.json':'{"name":"tslib","version":"2.0.0","types":"index.d.ts"}', '/src/node_modules/tslib/index.d.ts':'export {};'}
    add('synthetic-helper', {'/src/main.ts':'export const x=1;', **package}, ['/src/main.ts'], ['/src/node_modules/tslib/index.d.ts'], importHelpers=True)
    add('synthetic-jsx', {'/src/main.tsx':'export const x=<div/>;', '/src/node_modules/react/jsx-runtime.d.ts':'export {};'}, ['/src/main.tsx'], ['/src/node_modules/react/jsx-runtime.d.ts'], jsx=4)
    for name,package in [('module','{"type":"module"}'),('commonjs','{"type":"commonjs"}'),('unknown','{"type":"other"}'),('absent-type','{}'),('missing',None)]:
        files={'/src/main.ts':'import "./dep.js";', '/src/dep.ts':'export const x=1;'}
        if package is not None: files['/src/package.json']=package
        add('format/'+name,files,['/src/main.ts'],['/src/dep.ts'],module=199,moduleResolution=99)
    add('invalid-trivia-bytes', {'/src/main.ts':b'import /*\xff*/ \"./dep\";', '/src/dep.ts':'export {};'}, ['/src/main.ts'], ['/src/dep.ts'])
    row=add('library-directive', {'/src/main.ts':'/// <reference lib="es5" />\n'}, ['/src/main.ts'], ['bundled:///libs/lib.es5.d.ts'],lib=[])
    row['options']['noLib']=False
    add('rootdir-import-location', {'/src/bound/main.ts':'import "../dep";', '/src/dep.ts':'export {};'}, ['/src/bound/main.ts'], ['/src/dep.ts'],rootDir='/src/bound')
    add('rootdir-root-global', {'/src/dep.ts':'export {};'}, ['/src/dep.ts'], ['/src/dep.ts'],rootDir='/other')
    add('composite-import-location', {'/src/main.ts':'import "./dep";', '/src/dep.ts':'export {};'}, ['/src/main.ts'], ['/src/dep.ts'],composite=True)
    row=add('package-redirect', {'/src/main.ts':'import "a"; import "b";', **{f'/src/node_modules/{name}/package.json':'{"name":"pkg","version":"1.0.0","types":"index.d.ts"}' for name in ['a','b']}, **{f'/src/node_modules/{name}/index.d.ts':'export {};' for name in ['a','b']}}, ['/src/main.ts'], ['/src/node_modules/a/index.d.ts','/src/node_modules/b/index.d.ts'])
    for target in ['ES5',['ES5']]:
        row=add('config/target-'+('array' if isinstance(target,list) else 'scalar'), {'/src/main.ts':'let x=1;'}, ['/src/main.ts'], ['bundled:///libs/lib.d.ts'])
        row['options'].update(noLib=False,target=1)
        row.update(config_name='/src/tsconfig.json',config_text=json.dumps({'compilerOptions':{'target':target}}).encode().hex(),specs=dict(Includes=['**/*'],BeforeIncludes=['**/*'],Default=True))
    row=add('config/types', {'/src/main.ts':'let x=1;', '/src/node_modules/@types/pkg/index.d.ts':'declare const x:number;'}, ['/src/main.ts'], ['/src/node_modules/@types/pkg/index.d.ts'],types=['pkg'])
    row.update(config_name='/src/tsconfig.json',config_text=b'{"compilerOptions":{"types":["pkg"]}}'.hex(),specs={})
    row=add('config/lib-array', {'/src/main.ts':'let x=1;'}, ['/src/main.ts'], ['bundled:///libs/lib.es5.d.ts'],lib=['lib.es5.d.ts'])
    row['options']['noLib']=False
    row.update(config_name='/src/tsconfig.json',config_text=b'{"compilerOptions":{"lib":["lib.es5.d.ts"]}}'.hex(),specs={})
    row=add('default-library', {'/src/main.ts':'let x=1;'}, ['/src/main.ts'], ['bundled:///libs/lib.d.ts'])
    row['options'].update(noLib=False,target=1)
    row=add('explicit-library', {'/src/main.ts':'let x=1;'}, ['/src/main.ts'], ['bundled:///libs/lib.es5.d.ts'],lib=['lib.es5.d.ts'])
    row['options']['noLib']=False
    for name,config,specs in [
        ('files',{'files':['./dep.ts']},dict(Files=['/src/dep.ts'],BeforeFiles=['./dep.ts'])),
        ('include',{'include':['./*.ts']},dict(Includes=['/src/*.ts'],BeforeIncludes=['./*.ts'])),
        ('default',{},dict(Includes=['**/*'],BeforeIncludes=['**/*'],Default=True)),
        ('substitution',{'files':['${configDir}/dep.ts']},dict(Files=['/src/dep.ts'],BeforeFiles=['${configDir}/dep.ts'])),
        ('duplicate-property',None,dict(Files=['/src/dep.ts'],BeforeFiles=['./dep.ts'])),
    ]:
        row=add('config/'+name,{'/src/main.ts':'import "./dep";', '/src/dep.ts':'export {};'},['/src/main.ts','/src/dep.ts'],['/src/dep.ts'])
        text=json.dumps(config) if config is not None else '{"files": ["other.ts"], "files": ["./dep.ts"]}'
        row.update(config_name='/src/tsconfig.json',config_text=text.encode().hex(),specs=specs)
    # File-name casing: two spellings of one path on a case-insensitive host,
    # the "already included" wording (a root after a referencing import), the
    # option that disables it, two paths differing only in case on a
    # case-sensitive host, duplicate root spellings whose two diagnostics
    # are equal (the collection keeps one), and two spellings of a root that
    # fails to load (missing, or JavaScript without allowJs), whose failure
    # is reported once, for the spelling collected first.
    row=add('casing/insensitive-import-variants', {'/src/main.ts':'import "./a"; import "./A";', '/src/a.ts':'export {};'}, ['/src/main.ts'], ['/src/a.ts'])
    row['case_sensitive']=False
    row=add('casing/root-after-import', {'/src/main.ts':'import "./a";', '/src/a.ts':'export {};'}, ['/src/main.ts','/src/A.ts'], ['/src/a.ts'])
    row['case_sensitive']=False
    row=add('casing/force-consistent-false', {'/src/main.ts':'import "./a"; import "./A";', '/src/a.ts':'export {};'}, ['/src/main.ts'], ['/src/a.ts'], forceConsistentCasingInFileNames=False)
    row['case_sensitive']=False
    add('casing/sensitive-distinct-paths', {'/src/a.ts':'export {};', '/src/A.ts':'export {};'}, ['/src/a.ts','/src/A.ts'], ['/src/a.ts','/src/A.ts'])
    row=add('casing/duplicate-root-variants', {'/src/a.ts':'export {};'}, ['/src/a.ts','/src/A.ts','/src/A.ts'], ['/src/a.ts'])
    row['case_sensitive']=False
    row=add('casing/missing-root-variants', {'/src/main.ts':'export {};'}, ['/src/main.ts','/src/missing.ts','/src/MISSING.ts'], ['/src/missing.ts'])
    row['case_sensitive']=False
    row=add('casing/js-root-variants', {'/src/main.ts':'export {};', '/src/a.js':'export {};'}, ['/src/main.ts','/src/a.js','/src/A.js'], ['/src/a.js'])
    row['case_sensitive']=False
    # Unknown `/// <reference lib>` names: a spelling suggestion, none, and a
    # file-name spelling that is lowercased and trimmed before the suggestion.
    for name,text in [('unknown-suggestion','/// <reference lib="es2015.promis" />\n'),('unknown-no-suggestion','/// <reference lib="nothing-like-this" />\n'),('unknown-file-name','/// <reference lib="LIB.ES2015.PROMIS.D.TS" />\n')]:
        row=add('lib-reference/'+name, {'/src/main.ts':text}, ['/src/main.ts'], ['/src/main.ts'], lib=[])
        row['options']['noLib']=False
    # One file's processing diagnostics arrive out of position order (type
    # references are resolved before library references); global diagnostics
    # arrive out of message order.
    row=add('diagnostics/file-unsorted', {'/src/main.ts':'/// <reference lib="zzz" />\n/// <reference types="missing" />\n'}, ['/src/main.ts'], ['/src/main.ts'], lib=[], types=[])
    row['options']['noLib']=False
    add('diagnostics/globals-unsorted', {'/src/z.ts':'export {};', '/src/a.ts':'export {};'}, ['/src/z.ts','/src/a.ts'], ['/src/z.ts'], rootDir='/other')
    add('diagnostics/duplicate-resolution-diagnostic', {'/src/main.ts':'import "self";', '/src/other.ts':'import "self";', '/src/package.json':'{"name":"self","exports":"./dist/main.js"}'}, ['/src/main.ts','/src/other.ts'], ['/src/main.ts'], outDir='dist')
    # Program accessors: configuration parsing diagnostics, source files by
    # name (relative, other casing, missing) and default-library membership.
    row=add('accessors/config-and-files', {'/src/main.ts':'import "./Dep";', '/src/dep.ts':'export {};'}, ['/src/main.ts'], ['/src/dep.ts'], lib=['lib.es5.d.ts'])
    row['options']['noLib']=False
    row['case_sensitive']=False
    row.update(config_name='/src/tsconfig.json',config_text=b'{"compilerOptions": {"lib": ["es5"],}, "files": [}'.hex(),specs={},
               source_file_queries=['/src/main.ts','main.ts','/SRC/DEP.TS','./dep.ts','/src/missing.ts','bundled:///libs/lib.es5.d.ts'])
    row=add('accessors/no-config', {'/src/main.ts':'export {};'}, ['/src/main.ts'], ['/src/main.ts'])
    row['source_file_queries']=['/src/main.ts','/src/Main.ts']
    return rows


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check',action='store_true')
    args=parser.parse_args()
    requests=(json.dumps(fixtures(),indent=2,sort_keys=True)+'\n').encode()
    merge_rows=[dict(id='empty',diagnostics=[]),dict(id='related-merge',diagnostics=[dict(Chain=0,Related=r) for r in [[2,1],[1,3],[],[2]]]),dict(id='chain-code-ties',diagnostics=[dict(Chain=i*13%7+1,Related=[]) for i in range(39)]),dict(id='mixed-ties',diagnostics=[dict(Chain=i*13%7,Related=[i%5,(i*3)%5]) for i in range(41)])]
    merge_requests=(json.dumps(merge_rows,indent=2,sort_keys=True)+'\n').encode()
    adapters={ROOT/'tools/s07/program/export_test.go':'tsc/internal/compiler/s07_program_test.go',
              ROOT/'tools/s07/include-reason/export_test.go':'tsc/internal/compiler/s07_include_test.go',
              ROOT/'tools/s07/include-reason/specs_bridge.go':'tsc/internal/tsoptions/s07_include_specs.go'}
    sources=['tsc/internal/compiler/'+name for name in ['fileInclude.go','includeprocessor.go','processingDiagnostic.go','filesparser.go','program.go']]+['tsc/internal/ast/diagnostic.go','tsc/internal/tsoptions/tsconfigparsing.go','tsc/internal/tsoptions/parsedcommandline.go']
    with oracle_export() as (checkout,env,pin):
        request_path=checkout/'include-requests.json';request_path.write_bytes(requests)
        output=checkout/'include-observed.json'
        for adapter,target in adapters.items():shutil.copyfile(adapter,checkout/target)
        merge_request=checkout/'merge-requests.json';merge_request.write_bytes(merge_requests)
        merge_output=checkout/'merge-observed.json'
        env.update(S07_INCLUDE_REQUESTS=str(request_path),S07_INCLUDE_OUTPUT=str(output),S07_INCLUDE_MERGE_REQUESTS=str(merge_request),S07_INCLUDE_MERGE_OUTPUT=str(merge_output))
        repo=f'-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={checkout}/s07-unmatched-prefix'
        command(['go','test','-trimpath','-mod=readonly',repo,'./internal/compiler','-run','^TestS07IncludeReasons$','-count=1'],cwd=checkout/'tsc',env=env)
        data=output.read_bytes()
        merge_data=merge_output.read_bytes()
        if [r['id'] for r in strict_json_loads(merge_data)] != [r['id'] for r in merge_rows]:raise ValueError('missing, extra or reordered merge observations')
        observed=strict_json_loads(data)
        if [r['id'] for r in observed] != [r['id'] for r in strict_json_loads(requests)]:raise ValueError('missing, extra or reordered inclusion observations')
        manifest=dict(pin=pin,requests_sha256=hashlib.sha256(requests).hexdigest(),observations_sha256=hashlib.sha256(data).hexdigest(),adapters={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in adapters},sources={p:hashlib.sha256((checkout/p).read_bytes()).hexdigest() for p in sources},rows=len(observed),queries=sum(len(r['queries']) for r in observed),scope='Actual pinned inclusion helpers; config matching metadata and duplicate reason identities are explicitly constructed, not config-parser evidence.')
        manifest.update(merge_requests_sha256=hashlib.sha256(merge_requests).hexdigest(),merge_observations_sha256=hashlib.sha256(merge_data).hexdigest(),merge_rows=len(merge_rows))
    for name,raw in {'include-reason-requests.json':requests,'include-reason-observations.json':data,'include-merge-requests.json':merge_requests,'include-merge-observations.json':merge_data,'include-reason-manifest.json':(json.dumps(manifest,indent=2,sort_keys=True)+'\n').encode()}.items():
        target=ROOT/'data/s07'/name
        if args.check:
            if target.read_bytes()!=raw:raise ValueError('pinned inclusion helper observations drift: '+name)
        else:target.write_bytes(raw)
    print(f"{manifest['rows']} direct Go inclusion requests / {manifest['queries']} queries")


if __name__=='__main__':main()
