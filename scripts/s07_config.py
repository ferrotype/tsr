#!/usr/bin/env python3
"""Fresh unchanged Go config interpretation over explicit byte-preserving requests."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
from s04_common import command, strict_json_loads
from s06_build import ROOT, oracle_export
from s04_ownership import instrumentation_environment

def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def publish_direct(output, data, manifest, write=False):
    artifacts = {
        output: data,
        output.with_suffix('.manifest.json'): (json.dumps(manifest,sort_keys=True,separators=(',',':'))+'\n').encode(),
    }
    changed = [path for path, content in artifacts.items() if not path.exists() or path.read_bytes() != content]
    if changed and not write:
        raise ValueError('direct config fixture or provenance drift: ' + ', '.join(str(path) for path in changed))
    if write:
        output.parent.mkdir(parents=True, exist_ok=True)
        for path, content in artifacts.items():
            path.write_bytes(content)

def capture(requests,output,write=False):
    request_bytes=requests.read_bytes();request_hash=hashlib.sha256(request_bytes).hexdigest()
    inputs=provenance()['go_inputs']
    rows=strict_json_loads(request_bytes);ids=request_ids(rows,'direct config requests')
    for row in rows:
        for value in [row['text_hex'],*row['files'].values()]:
            canonical_hex(value)
    with oracle_export() as (checkout,env,pin):
        package=checkout/'tsc/internal/testrunner'
        sources={str(path.relative_to(checkout)):sha(path) for path in sorted(package.glob('*.go'))}
        for name in ['export_test.go','host_test.go']:shutil.copyfile(ROOT/'tools/s07/config'/name,package/('s07_'+name))
        result=checkout/'config.json';env.update(S07_CONFIG_REQUESTS=str(requests),S07_CONFIG_OUTPUT=str(result))
        repo_path=f'-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={checkout}/s07-unmatched-prefix'
        command(['go','test','-trimpath','-mod=readonly',repo_path,'./internal/testrunner','-run','^TestS07Config$','-count=1'],cwd=checkout/'tsc',env=env)
        data=result.read_bytes()
        if request_ids(strict_json_loads(data),'direct source outputs')!=ids:raise ValueError('missing/extra/reordered source rows')
        if inputs!=provenance()['go_inputs'] or sha(requests)!=request_hash:raise ValueError('source/request inputs changed during direct config observation')
    manifest={'pin':pin,'requests_sha256':request_hash,'observations_sha256':hashlib.sha256(data).hexdigest(),'source_sha256':sources,'inputs_sha256':inputs}
    publish_direct(output,data,manifest,write)
    print(f'{len(ids)} direct Go config observations')

def observed_value(value):
    kind=value['kind']
    if kind=='null' and set(value)=={'kind'}:return None
    if kind in ('boolean','integer') and set(value)=={'kind','value'}:
        if type(value['value']) is not (bool if kind=='boolean' else int):raise ValueError('invalid normalized option scalar')
        return value['value']
    if kind=='string' and set(value)=={'kind','hex'}:return canonical_hex(value['hex']).decode('utf-8')
    if kind=='array' and set(value)=={'kind','nil','values'}:
        if type(value['nil']) is not bool or not isinstance(value['values'],list) or (value['nil'] and value['values']):raise ValueError('invalid normalized option array')
        return None if value['nil'] else [observed_value(child) for child in value['values']]
    if kind=='object' and set(value)=={'kind','entries'}:
        result={}
        if not isinstance(value['entries'],list):raise ValueError('invalid normalized option object')
        for child in value['entries']:
            if set(child)!={'key','value'}:raise ValueError('invalid normalized option entry')
            key=canonical_hex(child['key']).decode('utf-8')
            if key in result:raise ValueError('duplicate normalized option key')
            result[key]=observed_value(child['value'])
        return result
    raise ValueError('unexpected normalized compiler option kind')

def provenance():
    from s07_program_compare import config_provenance_inputs
    return {kind:{name:sha(ROOT/name) for name in sorted(paths)} for kind,paths in config_provenance_inputs().items()}
def config_inputs():return provenance()['rust_inputs']

def request_ids(rows, label):
    from s07_program_compare import identities
    return identities(rows, 'id', label)

def canonical_hex(value):
    if not isinstance(value, str) or len(value) % 2 or any(c not in '0123456789abcdef' for c in value):
        raise ValueError('invalid request hex')
    return bytes.fromhex(value)

def selectors_for(loading, cases):
    selectors = []
    for identity in request_ids(loading, 'loading requests'):
        try:
            case, index = identity.rsplit('#configuration=', 1)
        except ValueError as error:
            raise ValueError('loading ID outside frozen effective variants') from error
        if (case not in cases or not index.isascii() or not index.isdecimal()
                or str(int(index)) != index or int(index) >= len(cases[case]['configurations'])):
            raise ValueError('loading ID outside frozen effective variants')
        selectors.append({'id': identity, 'path': cases[case]['path'], 'configuration': int(index)})
    return selectors

def validate_fixture_requests(requests, selectors, go_rows, loading, cases, upstream):
    """Bind fresh source preprocessing to the frozen physical and loader inputs."""
    ids = request_ids(loading, 'loading requests')
    for rows, label in ((selectors, 'selectors'), (requests, 'source requests'), (go_rows, 'source outputs')):
        if request_ids(rows, label) != ids:
            raise ValueError('missing/extra/reordered config source IDs')
    for request, selector, observed, load in zip(requests, selectors, go_rows, loading, strict=True):
        case = cases[selector['id'].rsplit('#configuration=', 1)[0]]
        index = selector['configuration']
        physical = (upstream / case['path']).read_bytes()
        if (request['path'] != case['path'] or request['raw_sha256'] != case['raw_sha256']
                or canonical_hex(request['physical_hex']) != physical
                or hashlib.sha256(physical).hexdigest() != case['raw_sha256']):
            raise ValueError('physical config request changed')
        if (request['settings'] != case['configurations'][index] or type(request['configuration']) is not int or request['configuration'] != index
                or request['symlinks'] != case['symlinks'] or request['loaded_sha256'] != case['loaded_sha256']):
            raise ValueError('source preprocessing changed from S06 boundary')
        if (request['cwd'] != case['variants'][index]['current_directory']
                or request['config_cwd'] != (case['current_directory'] or '/.src')
                or request['case_sensitive'] is not True
                or type(request['run_external_code']) is not bool
                or request['run_external_code'] != (case['global_options'].get('runexternalcode') == 'true')):
            raise ValueError('source fixture host boundary changed')
        if len(request['units']) != len(case['units']):
            raise ValueError('source unit inventory changed')
        for unit, frozen_unit in zip(request['units'], case['units'], strict=True):
            text = canonical_hex(unit['text_hex'])
            if (unit['name'] != frozen_unit['name'] or unit['file_options'] != frozen_unit['file_options']
                    or unit['script_kind'] != frozen_unit['script_kind']
                    or len(text) != frozen_unit['extracted_bytes']
                    or hashlib.sha256(text).hexdigest() != frozen_unit['extracted_sha256']):
                raise ValueError('source unit transport changed')
        options = observed_value(observed['options'])
        if (options != load['options']
                or list((options.get('paths') or {}).keys()) != list((load['options'].get('paths') or {}).keys())
                or [canonical_hex(v).decode('utf-8') for v in observed['root_file_names']] != load['roots']):
            raise ValueError('fresh config options/roots differ from loading request boundary: ' + request['id'])

def rust_binary():
    env = instrumentation_environment(os.environ.copy(), ROOT)
    for key in list(env):
        if key.startswith('CARGO_PROFILE_') or key == 'CARGO_BUILD_TARGET':
            env.pop(key, None)
    output = command(['cargo', 'build', '--locked', '--offline', '-p', 'tsr_compiler',
                      '--example', 's07_config', '--message-format=json'], cwd=ROOT, env=env)
    records = [strict_json_loads(line) for line in output.splitlines() if line.strip()]
    binaries = [row['executable'] for row in records
                if row.get('reason') == 'compiler-artifact'
                and row.get('target', {}).get('name') == 's07_config' and row.get('executable')]
    if len(binaries) != 1:
        raise ValueError('Cargo did not return exactly one config probe binary')
    return Path(binaries[0]), env

def full(loading_path,output):
    from s04 import verified_upstream
    loading_bytes=loading_path.read_bytes()
    loading_hash=hashlib.sha256(loading_bytes).hexdigest()
    loading=strict_json_loads(loading_bytes);ids=request_ids(loading,'loading requests')
    frozen=strict_json_loads((ROOT/'data/s06/corpus.json').read_bytes());cases={case['id']:case for case in frozen['cases'] if case['kind']=='case'}
    selectors=selectors_for(loading,cases)
    destination=output.parent;destination.mkdir(parents=True,exist_ok=True)
    selector_path=destination/(output.stem+'.selectors.json');source_path=destination/(output.stem+'.requests.json');go_path=destination/(output.stem+'.go.json');rust_path=destination/(output.stem+'.rust.json')
    selector_path.write_text(json.dumps(selectors,separators=(',',':'))+'\n')
    go_inputs=provenance()['go_inputs']
    with oracle_export() as(checkout,env,pin):
        for name in ['export_test.go','host_test.go','fixture_export_test.go']:shutil.copyfile(ROOT/'tools/s07/config'/name,checkout/'tsc/internal/testrunner'/('s07_config_'+name))
        shutil.copyfile(ROOT/'tools/s07/subset/options_bridge.go',checkout/'tsc/internal/testutil/harnessutil/s07_subset_options_bridge.go')
        # The bridge mentions lazy testLibFolderMap; no library content is read
        # by this config/options operation and no checker or loader is started.
        env.update(S07_CONFIG_SELECTORS=str(selector_path),S07_CONFIG_SOURCE_REQUESTS=str(source_path),S07_CONFIG_OUTPUT=str(go_path),S06_EXTRACT_ONLY='1')
        repo_path=f'-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={checkout}/s07-unmatched-prefix'
        command(['go','test','-trimpath','-mod=readonly',repo_path,'./internal/testrunner','-run','^TestS07ConfigFixtures$','-count=1','-timeout=15m'],cwd=checkout/'tsc',env=env)
    if go_inputs!=provenance()['go_inputs']:raise ValueError('config source adapter changed during capture')
    requests=strict_json_loads(source_path.read_bytes());go_rows=strict_json_loads(go_path.read_bytes())
    source_hash=sha(source_path);go_hash=sha(go_path)
    validate_fixture_requests(requests,selectors,go_rows,loading,cases,verified_upstream())
    rust_inputs=config_inputs()
    binary,env=rust_binary();binary_hash=sha(binary)
    command([str(binary),str(source_path),str(rust_path)],cwd=ROOT,env=env)
    rust_rows=strict_json_loads(rust_path.read_bytes())
    if request_ids(rust_rows,'Rust config outputs')!=ids:raise ValueError('missing/extra/reordered Rust config IDs')
    if config_inputs()!=rust_inputs or sha(binary)!=binary_hash:raise ValueError('Rust config inputs/binary changed during observation')
    if go_inputs!=provenance()['go_inputs'] or sha(loading_path)!=loading_hash:raise ValueError('source or loading inputs changed during config observation')
    if sha(source_path)!=source_hash or sha(go_path)!=go_hash:raise ValueError('source observations changed during Rust config observation')
    from s07_program_compare import compare_config
    differences=[row for row in compare_config(ids,requests,go_rows,rust_rows) if not row['passed']]
    artifact=lambda path:{'path':str(path),'sha256':sha(path)}
    manifest={'schema':1,'operation':'config_options','upstream_pin':pin,'loading_requests_sha256':loading_hash,'source_requests':artifact(source_path),'go_observations':artifact(go_path),'rust_observations':artifact(rust_path),'go_inputs':go_inputs,'rust_inputs':rust_inputs,'rust_binary':artifact(binary)}
    output.write_text(json.dumps(manifest,sort_keys=True,separators=(',',':'))+'\n')
    output.with_suffix('.differences.json').write_text(json.dumps(differences,separators=(',',':'))+'\n')
    print(f'{len(ids)} complete source/current-Rust config rows; {len(differences)} mismatches')

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--requests',type=Path,default=ROOT/'tools/s07/config/requests.json');parser.add_argument('--loading-requests',type=Path);parser.add_argument('--output',type=Path)
    mode=parser.add_mutually_exclusive_group();mode.add_argument('--check',action='store_true',help='check frozen direct fixtures without writing (default)');mode.add_argument('--write',action='store_true',help='explicitly replace reviewed direct fixtures')
    args=parser.parse_args()
    if args.loading_requests and (args.check or args.write):parser.error('direct fixture mode does not apply to full loading requests')
    output=args.output or (ROOT/'target/s07-config/full.evidence.json' if args.loading_requests else ROOT/'data/s07/config-observations.json')
    if args.loading_requests:full(args.loading_requests.resolve(),output.resolve())
    else:capture(args.requests.resolve(),output.resolve(),args.write)
if __name__=='__main__':main()
