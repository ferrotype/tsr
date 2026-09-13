#!/usr/bin/env python3
"""Capture and compare named P2 real-program observations, separate from P0/E2."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from s04 import go_environment, same_json_value, verified_upstream
from s04_common import strict_json_loads
from s08_oracle import ROOT, canonical, digest

SPEC = 'tools/s08/p2/requests.json'
DRIVER = 'tools/s08/p2/oracle_test.go'
IDENTITY_BRIDGE = 'tools/s08/p2/identity_bridge_test.go'
MODES = ['single-checker', 'repeated', 'separate-checker', 'concurrent']
PHASES = ['config', 'program', 'syntactic', 'bind', 'semantic', 'global', 'combined']
DIAGNOSTIC_FIELDS = {'file', 'pos', 'end', 'code', 'category', 'key_hex', 'text_hex', 'source_hex',
                     'args', 'chain', 'related', 'unnecessary', 'deprecated', 'skipped_on_no_emit'}


def fields(value, expected):
    if not isinstance(value, dict) or set(value) != set(expected.split()):
        raise ValueError('P2 protocol fields differ: ' + expected)


def specification(spec):
    shape=dict(spec)
    mode=shape.pop('diagnostic_mode', 'checker')
    if mode not in ('checker', 'program'):raise ValueError('invalid diagnostic mode')
    fields(shape, 'version scope options programs merge')
    options = ({'target':'ESNext','module':'ESNext','strict':True,'noLib':True},
               {'target':'ESNext','module':'ESNext','strict':True,'noLib':False,'skipLibCheck':True})
    selected_options = dict(spec['options'])
    for name in ('noUncheckedIndexedAccess', 'isolatedModules', 'verbatimModuleSyntax', 'allowUnreachableCode', 'allowJs', 'checkJs', 'noImplicitOverride', 'allowSyntheticDefaultImports', 'esModuleInterop', 'resolveJsonModule', 'declaration', 'isolatedDeclarations', 'stripInternal', 'useDefineForClassFields', 'importHelpers', 'noEmit'):
        if name in selected_options and type(selected_options.pop(name)) is not bool:
            raise ValueError(f'{name} must be boolean')
    if selected_options.get('target') not in ('ES5', 'ES2015', 'ES2016', 'ES2017', 'ES2018', 'ES2019', 'ES2020', 'ES2021', 'ES2022', 'ES2023', 'ES2024', 'ES2025', 'ESNext'):
        raise ValueError('unknown target')
    if selected_options.get('module') not in ('None', 'CommonJS', 'AMD', 'UMD', 'System', 'ES2015', 'ES2020', 'ES2022', 'ESNext', 'Node16', 'Node18', 'Node20', 'NodeNext', 'Preserve'):
        raise ValueError('unknown module format')
    selected_options['target'] = 'ESNext'
    selected_options['module'] = 'ESNext'
    if type(spec['version']) is not int or spec['version'] != 1 or not any(same_json_value(selected_options, value) for value in options):
        raise ValueError('capability capture requires explicit strict options and declared library checking policy')
    if not spec['programs'] or len({p['id'] for p in spec['programs']}) != len(spec['programs']):
        raise ValueError('empty or duplicate P2 programs')
    for program in spec['programs']:
        fields(program, 'id files roots queries')
        if (not program['roots'] or len(set(program['roots'])) != len(program['roots'])
                or any(path not in program['files'] for path in program['roots'])):
            raise ValueError('P2 root inventory differs from files')
        for path, source in program['files'].items():
            if not path.startswith('/') or '..' in Path(path).parts or not isinstance(source, str):
                raise ValueError('invalid P2 source file')
        if not program['queries'] or len({q['id'] for q in program['queries']}) != len(program['queries']):
            raise ValueError('empty or duplicate P2 queries')
        for query in program['queries']:
            fields(query, 'id file declaration target operation')
            if (query['file'] not in program['files'] or not query['declaration']
                    or query['target'] not in ('name','annotation','annotation_name','initializer')
                    or query['operation'] not in ('type_at_location','declared_type','declared_type_summary')
                    or (query['operation'] in ('declared_type','declared_type_summary') and query['target'] != 'name')):
                raise ValueError('invalid P2 query selector')
    merge = spec['merge']
    fields(merge, 'id files shared symbol modes single_roots independent_roots')
    if (merge['modes'] != MODES or merge['shared'] != '/base.ts' or merge['symbol'] != 'Shared'
            or merge['single_roots'] != ['/base.ts','/a.ts','/b.ts']
            or merge['independent_roots'] != {'A':['/base.ts','/a.ts'],'B':['/base.ts','/b.ts']}
            or set(merge['files']) != set(merge['single_roots'])):
        raise ValueError('P2 requires all four actual shared-source merge modes')
    return spec


def diagnostic_payload(values):
    if not isinstance(values, list):
        raise ValueError('diagnostic payload must be a list')
    for value in values:
        if not isinstance(value, dict) or set(value) != DIAGNOSTIC_FIELDS:
            raise ValueError('incomplete structured diagnostic')
        if value['file'] is not None and not isinstance(value['file'], str):
            raise ValueError('invalid diagnostic file')
        for key in ('pos','end','code','category'):
            if type(value[key]) is not int:
                raise ValueError('invalid diagnostic numeric field')
        for key in ('key_hex','text_hex','source_hex'):
            if not isinstance(value[key], str):
                raise ValueError('invalid diagnostic text')
            bytes.fromhex(value[key])
        for key in ('unnecessary','deprecated','skipped_on_no_emit'):
            if type(value[key]) is not bool:
                raise ValueError('invalid diagnostic boolean field')
        diagnostic_payload(value['chain']); diagnostic_payload(value['related'])


def operation_state(value, path, missing, allow_unsupported):
    state = value.get('state')
    if state == 'unsupported':
        if not allow_unsupported or set(value) - {'id','mode','state','operation','reason'}:
            raise ValueError('unsupported operation has invalid payload: ' + path)
        if not isinstance(value.get('operation'), str) or not value['operation'] or not isinstance(value.get('reason'), str) or not value['reason']:
            raise ValueError('unsupported operation lacks named operation/reason')
        missing.append({'path':path,'operation':value['operation'],'reason':value['reason']})
        return False
    if state != 'executed':
        raise ValueError('P2 execution failed or unclassified: ' + path)
    return True


def validate(spec, observed, *, allow_unsupported=False, require_rust_ownership=False):
    specification(spec)
    if observed.get('version') != 1 or observed.get('request_sha256') != digest(canonical(spec)+b'\n'):
        raise ValueError('P2 observation request identity differs')
    if [p['id'] for p in observed['programs']] != [p['id'] for p in spec['programs']]:
        raise ValueError('P2 program inventory differs')
    missing = []
    def diagnostics(value, path, declaration=False):
        if not operation_state(value,path,missing,allow_unsupported):
            return
        fields(value, 'state config program syntactic bind semantic global combined baseline' + (' declaration' if declaration else ''))
        incomplete = False
        for phase in (PHASES[:-1] + (['declaration'] if declaration else []) + ['combined']):
            payload = value[phase]
            if isinstance(payload, list):
                diagnostic_payload(payload)
                if phase == 'combined' and incomplete:
                    raise ValueError('combined diagnostics require every completed phase')
            else:
                if operation_state(payload,path+'/'+phase,missing,allow_unsupported):
                    raise ValueError('diagnostic phase must be a list or named Unsupported')
                incomplete = True
        baseline = value['baseline']
        if baseline.get('state') == 'unsupported':
            operation_state(baseline,path+'/baseline',missing,allow_unsupported)
        elif incomplete:
            raise ValueError('error baseline requires every completed diagnostic phase')
        elif baseline == {'state':'no_content'}:
            if value['combined']:raise ValueError('diagnostics cannot lose their error baseline')
        elif set(baseline) == {'state','text_hex'} and baseline['state'] == 'content':
            bytes.fromhex(baseline['text_hex'])
        else:raise ValueError('invalid P2 error baseline outcome')
    for request, program in zip(spec['programs'],observed['programs'],strict=True):
        path = request['id']
        if not operation_state(program,path,missing,allow_unsupported):continue
        fields(program,'id state queries diagnostics')
        if [q['id'] for q in program['queries']] != [q['id'] for q in request['queries']]:
            raise ValueError('P2 query inventory differs')
        for requested, query in zip(request['queries'], program['queries'], strict=True):
            if operation_state(query,path+'/'+query['id'],missing,allow_unsupported):
                fields(query,'id state node symbol type')
                type_fields = 'flags object_flags display_hex in_alias_display_hex'
                if requested['operation'] != 'declared_type_summary': type_fields += ' properties'
                fields(query['type'], type_fields)
                for key in ('display_hex','in_alias_display_hex'):bytes.fromhex(query['type'][key])
        diagnostics(program['diagnostics'],path+'/diagnostics',spec['options'].get('declaration',False))
    if [m['mode'] for m in observed['merges']] != MODES:
        raise ValueError('P2 merge execution modes differ')
    for merge in observed['merges']:
        path='merge/'+merge['mode']
        if not operation_state(merge,path,missing,allow_unsupported):continue
        if (merge['shared_bound_file_identity'] is not True or merge['base_unchanged'] is not True
                or not same_json_value(merge['base_before'],merge['base_after'])):
            raise ValueError('shared binding identity or immutability failed')
        single=merge['mode'] in ('single-checker','repeated')
        expected = 1 if merge['mode']=='single-checker' else 6 if merge['mode']=='repeated' else 5
        if len(merge['observations']) != expected:
            raise ValueError('merge mode did not execute its required sequence')
        if not single:
            if merge['different_merged_symbols'] is not True or merge['different_types'] is not True:
                raise ValueError('independent checker identities were collapsed')
            if [o['checker'] for o in merge['observations']] != ['A','B','B','A','B'] or merge['observations'][-1].get('after_release_A') is not True:
                raise ValueError('reverse order/release sequence was omitted')
        for index, observation in enumerate(merge['observations']):
            if observation['merged_is_bound_base'] is not False:
                raise ValueError('checker merge reused the shared source symbol')
            if (single and index>0) or (not single and index>=2):
                if observation.get('same_symbol_as_previous') is not True or observation.get('same_type_as_previous') is not True:
                    raise ValueError('repeat lost native checker identity')
        for owner,value in merge['diagnostics'].items():diagnostics(value,path+'/'+owner+'/diagnostics')
    ownership = observed.get('rust_ownership')
    if require_rust_ownership and ownership is None:
        raise ValueError('Rust retained-result ownership checks are missing')
    if ownership is not None:
        fields(ownership,'scope programs')
        if [p['id'] for p in ownership['programs']] != [p['id'] for p in spec['programs']]:
            raise ValueError('Rust retained-result ownership inventory differs')
        for value in ownership['programs']:
            if operation_state(value,'rust_ownership/'+value['id'],missing,allow_unsupported):
                fields(value,'id state program_survives_retained_result retained_display_unchanged program_released_after_result_drop')
                if any(value[key] is not True for key in ('program_survives_retained_result','retained_display_unchanged','program_released_after_result_drop')):
                    raise ValueError('Rust retained-result lifetime check failed')
    return missing


def capture(directory, spec_path=SPEC):
    spec_path=str((ROOT/spec_path).resolve().relative_to(ROOT))
    directory=Path(directory).resolve();directory.mkdir(parents=True,exist_ok=False)
    upstream=verified_upstream();env=go_environment()
    spec=specification(strict_json_loads((ROOT/spec_path).read_bytes()));raw=canonical(spec)+b'\n'
    (directory/'requests.json').write_bytes(raw)
    driver=directory/'s08_p2_test.go';driver.write_bytes((ROOT/DRIVER).read_bytes())
    bridge=directory/'s08_p2_identity_bridge_test.go';bridge.write_bytes((ROOT/IDENTITY_BRIDGE).read_bytes())
    replacements={str(upstream/'tsc/internal/checker/s08_p2_test.go'):str(driver),
                  str(upstream/'tsc/internal/checker/s08_p2_identity_bridge_test.go'):str(bridge)}
    (directory/'overlay.json').write_bytes(canonical({'Replace':replacements})+b'\n')
    source_names=(spec_path,DRIVER,IDENTITY_BRIDGE,'scripts/s08_p2.py','scripts/s04.py','scripts/s04_common.py','scripts/s04_runtime.py','scripts/s08_oracle.py','data/s04/toolchains.toml','data/upstream.json')
    sources={name:digest((ROOT/name).read_bytes()) for name in source_names}
    for name in sources:
        snapshot=directory/'source-snapshot'/name;snapshot.parent.mkdir(parents=True,exist_ok=True);snapshot.write_bytes((ROOT/name).read_bytes())
    env.update(S08_P2_REQUESTS=str(directory/'requests.json'),S08_P2_OUTPUT=str(directory/'observations.json'))
    repo_flag='-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath='+str(directory/'unmatched-prefix')
    command=['go','test','-mod=readonly','-trimpath',repo_flag,'-overlay',str(directory/'overlay.json'),'./internal/checker','-run','^TestS08P2$','-count=1','-timeout=2m']
    (directory/'command.json').write_bytes(canonical(command)+b'\n')
    with (directory/'go.stdout').open('wb') as stdout,(directory/'go.stderr').open('wb') as stderr:
        completed=subprocess.run(command,cwd=upstream/'tsc',env=env,stdout=stdout,stderr=stderr,timeout=150,check=False)
    if completed.returncode:raise ValueError(f'P2 native capture failed; logs retained in {directory}')
    observed=strict_json_loads((directory/'observations.json').read_bytes());validate(spec,observed)
    verified_upstream()
    if any(digest((ROOT/name).read_bytes())!=value for name,value in sources.items()):raise ValueError('P2 source changed during observation')
    report={'version':1,'scope':spec['scope'],
            'pin':strict_json_loads((ROOT/'data/upstream.json').read_bytes())['pin'],'sources':sources,
            'request_sha256':digest(raw),'observation_sha256':digest((directory/'observations.json').read_bytes()),
            'runtime':{k:observed[k] for k in ('go','goos','goarch')},'programs':len(spec['programs']),
            'queries':sum(len(p['queries']) for p in spec['programs']),'merge_modes':MODES,
            'diagnostic_counts':{p['id']:{phase:len(p['diagnostics'][phase]) for phase in PHASES} for p in observed['programs']}}
    (directory/'report.json').write_bytes(canonical(report)+b'\n')
    print(json.dumps({k:v for k,v in report.items() if k!='sources'},sort_keys=True))
    return observed


def compare(directory, actual_path, output):
    directory=Path(directory);report=strict_json_loads((directory/'report.json').read_bytes())
    for name,expected in report['sources'].items():
        if digest((directory/'source-snapshot'/name).read_bytes())!=expected:raise ValueError('P2 capture source snapshot changed')
    for name,key in (('requests.json','request_sha256'),('observations.json','observation_sha256')):
        if digest((directory/name).read_bytes())!=report[key]:raise ValueError('P2 native capture bytes changed')
    spec=strict_json_loads((directory/'requests.json').read_bytes());native=strict_json_loads((directory/'observations.json').read_bytes());validate(spec,native)
    actual=strict_json_loads(Path(actual_path).read_bytes());missing=validate(spec,actual,allow_unsupported=True,require_rust_ownership=True)
    mismatches=[]
    for section,key in (('programs','id'),('merges','mode')):
        for expected,result in zip(native[section],actual[section],strict=True):
            if not same_json_value(expected,result):mismatches.append({'section':section,'id':expected[key]})
    result={'version':1,'matched':not missing and not mismatches,'unsupported_operations':missing,'mismatches':mismatches,
            'native_observation_sha256':report['observation_sha256'],'actual_sha256':digest(Path(actual_path).read_bytes()),
            'scope':spec['scope']}
    with Path(output).open('xb') as destination:destination.write(canonical(result)+b'\n')
    print(json.dumps(result,sort_keys=True))
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__);sub=parser.add_subparsers(dest='operation',required=True)
    native=sub.add_parser('capture');native.add_argument('--output',type=Path,required=True)
    native.add_argument('--spec',default=SPEC,help='repository-relative named checkpoint request file')
    check=sub.add_parser('compare');check.add_argument('--native',type=Path,required=True);check.add_argument('--actual',type=Path,required=True);check.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    try:
        if args.operation=='capture':capture(args.output,args.spec)
        elif not compare(args.native,args.actual,args.output)['matched']:return 1
    except (OSError,ValueError,TypeError,KeyError,subprocess.TimeoutExpired) as error:
        print(f'S08 P2 failed: {error}',file=sys.stderr);return 1
    return 0


if __name__=='__main__':raise SystemExit(main())
