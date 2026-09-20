#!/usr/bin/env python3
"""Stage/build/replay one untimed owner census; never emit acceptance metrics."""
import argparse
import fcntl
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib

HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[3]
sys.path.insert(0,str(HERE.parent))
import runner
import stage
from validate import aggregate, validate_capture
from receipt import validate_receipt, validate_receipt_status
from s04_common import command, strict_json_loads
from s07_benchmark import cargo_configuration_paths, cargo_executable, native_environment, release_configuration

DEFAULT_BASE=ROOT/'target/s07-bis/a0b-candidate'
BASE_SHA='124956f668540814b95e6f677bdeae98bb2cad4f7586d3cb87f869e5c5af118f'


def tool_files():
    own=[p for p in HERE.rglob('*') if p.is_file() and p.suffix in {'.py','.rs','.toml','.lock'}]
    helpers=[HERE.parent/'runner.py',*[ROOT/'scripts'/name for name in ('s04_common.py','s07_benchmark.py','s07_benchmark_child.py','s07_benchmark_inputs.py','s07_benchmark_graph.py','s07_benchmark_measure.py','s07_benchmark_stats.py','s06_process.py','s06_protocol.py','s07_binder.py')]]
    return {str(p.relative_to(ROOT)):runner.digest(p) for p in sorted(set(own+helpers))}


def registry_entries(path):
    return {(r['name'],r['version'],r['source']):r['checksum'] for r in tomllib.loads(path.read_text())['package'] if 'source' in r}


def validate_build(directory,sha):
    if runner.digest(directory/'manifest.json')!=sha: raise ValueError('build manifest changed')
    record=strict_json_loads((directory/'manifest.json').read_bytes())
    if record.get('kind')!='s07_bis_untimed_owner_census_build' or record.get('diagnostic_only') is not True:
        raise ValueError('wrong diagnostic build')
    if runner.inventory(directory)!=record['inventory']: raise ValueError('build inventory drift')
    if any(p.is_symlink() or p.stat().st_mode & 0o222 for p in (directory,*directory.rglob('*'))):
        raise ValueError('build must be immutable')
    artifact=directory/record['artifact']['path']
    if runner.digest(artifact)!=record['artifact']['sha256'] or not os.access(artifact,os.X_OK):
        raise ValueError('diagnostic artifact changed')
    return record


def build(output,baseline,base_sha):
    reference=runner.validate_bundle(baseline,base_sha)
    runner.validate_inputs(baseline,reference['expected_work'])
    output.mkdir(parents=True,exist_ok=False)
    (output/'artifacts').mkdir()
    source=output/'source'
    shutil.copytree(baseline/'source',source)
    for p in source.rglob('*'): p.chmod(0o755 if p.is_dir() else 0o644)
    source.chmod(0o755)
    changed=stage.apply(source)
    crate=source/'tools/s07/performance-experiments/owner-census'
    before_tools=tool_files()
    for name,sha in before_tools.items():
        runner.copy_file(ROOT/name,output/'tool-snapshot'/name)
        if runner.digest(output/'tool-snapshot'/name)!=sha: raise ValueError('tool changed while snapshotting')
    for name in ('Cargo.toml','Cargo.lock','src/main.rs','src/observe.rs'):
        runner.copy_file(HERE/name,crate/name)
    registry=registry_entries(source/'Cargo.lock')
    if any(registry.get(k)!=v for k,v in registry_entries(crate/'Cargo.lock').items()):
        raise ValueError('standalone dependency not in frozen lock')
    env=native_environment()
    env['CARGO_TARGET_DIR']=str(ROOT/'target/s07-bis-owner-census-native')
    stable=tomllib.loads((source/'rust-toolchain.toml').read_text())['toolchain']['channel']
    rustc=command(['rustc','+'+stable,'-vV'],cwd=crate,env=env).decode()
    host=next(line.removeprefix('host: ') for line in rustc.splitlines() if line.startswith('host: '))
    config=lambda:{str(p):runner.digest(p) if p.is_file() else None for p in cargo_configuration_paths(env,crate)}
    before_config=config(); before_source=runner.inventory(source)
    # Semantic tests run in the same additive source copy, before the full capture.
    test_args=['cargo','+'+stable,'test','--offline','--locked','--manifest-path',str(crate/'Cargo.toml'),'--bin','tsr_s07_bis_owner_census','--','--nocapture']
    tested=subprocess.run(test_args,cwd=crate,env=env,capture_output=True)
    (output/'test.stdout').write_bytes(tested.stdout); (output/'test.stderr').write_bytes(tested.stderr)
    if tested.returncode: raise ValueError('owner census contract tests failed; inspect build test.stderr')
    argv=['cargo','+'+stable,'build','--release','--offline','--locked','--manifest-path',str(crate/'Cargo.toml'),'--bin','tsr_s07_bis_owner_census','--target',host,'--message-format=json-render-diagnostics',*release_configuration(env,crate)]
    built=subprocess.run(argv,cwd=crate,env=env,capture_output=True)
    (output/'cargo-messages.ndjson').write_bytes(built.stdout); (output/'cargo.stderr').write_bytes(built.stderr)
    if built.returncode: raise ValueError('owner census build failed; inspect cargo.stderr')
    binary=cargo_executable(built.stdout,crate/'Cargo.toml','tsr_s07_bis_owner_census','bin',[])
    ast_artifacts=[strict_json_loads(line) for line in built.stdout.splitlines() if line]
    if not any(r.get('reason')=='compiler-artifact' and r.get('target',{}).get('name')=='tsr_ast' and r.get('features')==['owner-census'] for r in ast_artifacts):
        raise ValueError('observer feature not active on actual AST artifact')
    shutil.copyfile(binary,output/'artifacts/owner-census')
    runner.runtime_libraries(output/'artifacts/owner-census',output/'runtime-libraries.txt')
    runner.copy_file(baseline/'inputs.json',output/'inputs.json')
    if before_config!=config() or before_source!=runner.inventory(source) or before_tools!=tool_files():
        raise ValueError('source/tools/configuration changed during build')
    manifest={'version':1,'kind':'s07_bis_untimed_owner_census_build','diagnostic_only':True,
        'baseline_manifest_sha256':base_sha,'baseline_source_fingerprint':reference['source_fingerprint'],
        'baseline_revision':reference['revision'],'expected_work':reference['expected_work'],
        'rustc':rustc,'profile':'release with explicit baseline profile; owner-census access-only feature',
        'artifact':{'path':'artifacts/owner-census','sha256':runner.digest(output/'artifacts/owner-census')},
        'tool_inputs':before_tools,'configuration':before_config,'staged_source_inventory':before_source,
        'patch_sha256':runner.digest(source/'owner-census.patch'),'patched_files':changed,
        'test_command':test_args,'build_command':argv}
    return runner.seal(output,manifest)


def capture(build_dir,build_sha,output):
    manifest=validate_build(build_dir,build_sha)
    runner.validate_inputs(build_dir,manifest['expected_work'])
    output.mkdir(parents=True,exist_ok=False)
    before_tools=tool_files()
    for name,sha in before_tools.items():
        runner.copy_file(ROOT/name,output/'tool-snapshot'/name)
        if runner.digest(output/'tool-snapshot'/name)!=sha: raise ValueError('capture tool changed while copying')
    raw=output/'owners.ndjson'
    argv=[str(build_dir/manifest['artifact']['path']),str(build_dir/'inputs.json'),str(raw)]
    captured=subprocess.run(argv,cwd=ROOT,env=native_environment(),capture_output=True)
    (output/'child.stdout').write_bytes(captured.stdout);(output/'child.stderr').write_bytes(captured.stderr)
    receipt={'version':1,'build_manifest_sha256':build_sha,'command':argv,
        'child_returncode':captured.returncode,'raw_sha256':runner.digest(raw) if raw.is_file() else None,
        'stdout_sha256':runner.digest(output/'child.stdout'),'stderr_sha256':runner.digest(output/'child.stderr'),
        'capture_tool_inputs':before_tools}
    runner.write_json(output/'capture-receipt.json',receipt)
    if captured.returncode: raise ValueError('owner census child failed; exit receipt and diagnostics retained')
    if before_tools!=tool_files(): raise ValueError('capture tools changed')
    return finalize(output,build_dir,build_sha)


def finalize(output,build_dir,build_sha,recovery_note=None):
    if (output/'manifest.json').exists(): raise ValueError('capture already sealed')
    manifest=validate_build(build_dir,build_sha)
    receipt_path=output/'capture-receipt.json'
    if receipt_path.exists():
        validate_receipt_status(strict_json_loads(receipt_path.read_bytes()))
    child=strict_json_loads((output/'child.stdout').read_bytes())
    raw=output/'owners.ndjson'
    rows=validate_capture(raw.read_bytes(),child,manifest['expected_work'])
    if not receipt_path.exists():
        if not recovery_note: raise ValueError('missing invocation receipt; explicit recovery note required')
        snapshots=output/'tool-snapshot'
        captured_tools={str(p.relative_to(snapshots)):runner.digest(p) for p in snapshots.rglob('*') if p.is_file()}
        if not captured_tools: raise ValueError('missing original capture tool snapshots')
        receipt={'version':1,'build_manifest_sha256':build_sha,'child_returncode':None,
            'raw_sha256':runner.digest(raw),'stdout_sha256':runner.digest(output/'child.stdout'),
            'stderr_sha256':runner.digest(output/'child.stderr'),'capture_tool_inputs':captured_tools,
            'recovery_note':recovery_note}
        runner.write_json(receipt_path,receipt)
    receipt=strict_json_loads(receipt_path.read_bytes())
    validate_receipt(receipt, build_sha, raw.read_bytes(),
                     (output/'child.stdout').read_bytes(), (output/'child.stderr').read_bytes())
    for name,sha in receipt['capture_tool_inputs'].items():
        if runner.digest(output/'tool-snapshot'/name)!=sha: raise ValueError('capture tool snapshot changed')
    finalizer_tools=tool_files()
    for name,sha in finalizer_tools.items():
        runner.copy_file(ROOT/name,output/'finalizer-tools'/name)
        if runner.digest(output/'finalizer-tools'/name)!=sha: raise ValueError('finalizer tool changed')
    report={'version':1,'kind':'s07_bis_untimed_owner_census','diagnostic_only':True,
        'tool_inputs':receipt['capture_tool_inputs'],'finalizer_tool_inputs':finalizer_tools,
        'build_manifest_sha256':build_sha,'baseline_manifest_sha256':manifest['baseline_manifest_sha256'],
        'raw_sha256':runner.digest(raw),'raw_bytes':raw.stat().st_size,'child':child,
        'totals':aggregate(rows),'receipt':receipt,'host':{'platform':sys.platform,'machine':os.uname().machine},
        'limitations':['No timing, allocation traffic or RSS measurement.','Physical core and binding records include records unreachable from AST edges; lazy graphs are observed only by reserved/initialized counts.',
        'Identifier suffix eligibility is an endpoint byte comparison, not a proof about later range mutation; unique fallback counts deduplicate selected bytes per file only.',
        'Node backing order is core auxiliary allocation/completion order, not parser Vec construction capacity or nested-list start order.',
        'HashMap capacity is public guaranteed-entry capacity; hash bucket/control and allocator layouts are not measured.']}
    return runner.seal(output,report)


def replay(directory,build_dir,build_sha):
    manifest=validate_build(build_dir,build_sha)
    report=strict_json_loads((directory/'manifest.json').read_bytes())
    if runner.inventory(directory)!=report['inventory'] or report['build_manifest_sha256']!=build_sha:
        raise ValueError('capture inventory/provenance changed')
    child=strict_json_loads((directory/'child.stdout').read_bytes())
    raw=(directory/'owners.ndjson').read_bytes()
    validate_receipt(report['receipt'], build_sha, raw,
                     (directory/'child.stdout').read_bytes(), (directory/'child.stderr').read_bytes(),
                     report['tool_inputs'])
    rows=validate_capture(raw,child,manifest['expected_work'])
    if child!=report['child'] or aggregate(rows)!=report['totals'] or runner.sha(raw)!=report['raw_sha256']:
        raise ValueError('capture replay differs')
    return {'files':len(rows),'raw_sha256':report['raw_sha256'],'totals':report['totals']}


def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('command',choices=['build','capture','replay','finalize'])
    p.add_argument('--baseline',type=Path,default=DEFAULT_BASE);p.add_argument('--baseline-sha',default=BASE_SHA)
    p.add_argument('--recovery-note');p.add_argument('--build',type=Path);p.add_argument('--build-sha');p.add_argument('--output',type=Path,required=True)
    a=p.parse_args()
    if a.command=='replay': print(json.dumps(replay(a.output.resolve(),a.build.resolve(),a.build_sha),indent=2));return
    if a.command=='finalize': print(json.dumps({'manifest_sha256':finalize(a.output.resolve(),a.build.resolve(),a.build_sha,a.recovery_note)}));return
    # Coordinate with the existing timing runner. This work makes no timing claim
    # but must not contaminate any simultaneously requested timing screen.
    with (runner.CACHE/'s07-benchmark/measurement.lock').open('a+') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        value=build(a.output.resolve(),a.baseline.resolve(),a.baseline_sha) if a.command=='build' else capture(a.build.resolve(),a.build_sha,a.output.resolve())
    print(json.dumps({'manifest_sha256':value}))

if __name__=='__main__': main()
