#!/usr/bin/env python3
"""Supplemental binder depth: actual small-stack growth plus original Go graphs."""
import argparse
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from s04_common import command, strict_json_loads
from s04_ownership import instrumentation_environment
from s06_protocol import canonical
from s07_binder import Process, build_oracle, compare, rust_binary, validate_request

ROOT = Path(__file__).resolve().parents[1]
CASES = ROOT / 'data/s07/binder-depth-cases.json'


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def inputs():
    paths = [CASES, Path(__file__), ROOT/'Cargo.lock', ROOT/'Cargo.toml', ROOT/'rust-toolchain.toml']
    for name in ('tsr_binder','tsr_parser','tsr_scanner','tsr_ast','tsr_arena','tsr_core',
                 'tsr_diagnostics','tsr_jsstring','tsr_jsnum','tsr_unicode'):
        crate = ROOT/'crates'/name
        if crate.is_dir():
            paths += list(crate.rglob('*.rs')) + [crate/'Cargo.toml']
    paths += list((ROOT/'scripts/s07_oracle').glob('*.go'))
    paths += [ROOT/'scripts'/name for name in ('s07_binder.py','s06_protocol.py','s06_process.py','s05_protocol.py','s06_build.py','s04_common.py','s04_ownership.py')]
    return {str(path.relative_to(ROOT)):digest(path) for path in sorted(paths)}


def native_rows(output, inventory):
    observed = []
    for line in output.splitlines():
        if b'S07_BINDER_DEPTH:' in line:
            value = strict_json_loads(line.split(b'S07_BINDER_DEPTH:', 1)[1])
            if not isinstance(value, list):
                raise ValueError('native depth record is not an array')
            observed.extend(value)
    expected = inventory['small_stack'] + inventory['constructed']
    by_id = {}
    for row in observed:
        if not isinstance(row, dict) or not isinstance(row.get('id'), str) or row['id'] in by_id:
            raise ValueError('invalid or duplicate native depth identity')
        by_id[row['id']] = row
    if set(by_id) != {row['id'] for row in expected}:
        raise ValueError('missing or extra native depth scenarios')
    for spec in expected:
        row = by_id[spec['id']]
        required = {'id', 'guard_entries', 'actual_segment_growths'}
        if 'source_hex' in spec:
            required |= {'binary_nodes', 'max_binary_frames'}
        if spec.get('terminal_failure'):
            required.add('terminal_failure')
        if set(row) != required:
            raise ValueError('native depth record fields changed: '+spec['id'])
        for field in required-{'id','terminal_failure'}:
            if type(row[field]) is not int or row[field] < 0:
                raise ValueError('invalid depth counter: '+field)
        # The unwind probe aborts immediately after its first real stack growth.
        # Its entry count depends on the native frame size, not completed depth;
        # still require the guard, actual growth and terminal failure together.
        injected_unwind = spec.get('terminal_failure') is True
        minimum_entries = 0 if injected_unwind else 500
        if row['guard_entries'] <= minimum_entries:
            raise ValueError('native depth guard was not exercised')
        if (spec.get('require_growth') or injected_unwind) and row['actual_segment_growths'] <= 0:
            raise ValueError('actual segment growth was not observed')
        if spec.get('require_binary') and (row['binary_nodes'] < 20000 or row['max_binary_frames'] <= 20000):
            raise ValueError('binary continuation stack was not exercised')
        if spec.get('terminal_failure') and row['terminal_failure'] is not True:
            raise ValueError('panic did not publish terminal failure')
    return [by_id[row['id']] for row in expected]


def native_capture(output, env, inventory):
    cargo = command(['cargo','test','--locked','--release','-p','tsr_binder','--lib','--no-run','--message-format=json'],cwd=ROOT,env=env)
    artifacts = [strict_json_loads(line) for line in cargo.splitlines() if line.strip()]
    binaries = [row['executable'] for row in artifacts if row.get('reason') == 'compiler-artifact' and row.get('profile',{}).get('test') and row.get('target',{}).get('name') == 'tsr_binder' and row.get('executable')]
    if len(binaries) != 1:
        raise ValueError('Cargo did not identify one binder test executable')
    binary = output/'depth-tests'
    shutil.copy2(binaries[0],binary)
    result = subprocess.run([str(binary),'binder_depth','--nocapture','--test-threads=1'],cwd=ROOT,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120,check=False)
    (output/'native.log').write_bytes(result.stdout)
    rows = native_rows(result.stdout,inventory)
    if result.returncode:
        raise ValueError('native binder depth process failed; see native.log')
    return {'rows':rows,'binary_sha256':digest(binary),'log_sha256':digest(output/'native.log')}


def capture(output):
    output = Path(output).resolve(); output.mkdir(parents=True,exist_ok=True)
    inventory = strict_json_loads(CASES.read_bytes())
    requests = inventory['graph_requests']
    ids = [request['id'] for request in requests]
    if not requests or len(ids) != len(set(ids)):
        raise ValueError('invalid frozen depth request identities')
    for request in requests:
        validate_request(request)
        if request['primary'] is not None:
            raise ValueError('supplemental depth request cannot be primary')
    before = inputs()
    report = {'schema':1,'scope':'supplemental-binder-depth-only','inputs':before,'requests_sha256':digest(CASES),'graph_rows':[],'metrics':{'binder_depth':False}}
    try:
        env = instrumentation_environment(os.environ.copy(),ROOT)
        report['native'] = native_capture(output,env,inventory)
        oracle, pin = build_oracle()
        rust = rust_binary()
        report['upstream_pin'] = pin
        binaries = {}
        for name, source in [('oracle',oracle),('rust',rust)]:
            binary = output/('depth-'+name)
            shutil.copy2(source,binary); binaries[name] = binary
        report['binaries'] = {name:digest(path) for name,path in binaries.items()}
        with ExitStack() as stack:
            processes = {}
            for name,binary in binaries.items():
                process = Process([str(binary)],output/(name+'.stderr'),env=env,deadline=120)
                stack.callback(process.close); processes[name] = process
            for request in requests:
                for process in processes.values(): process.send(request)
                result = compare(request,**processes)
                successful = all(stage['outcome'] == 'ok' for stages in result['stages'].values() for stage in stages)
                result['passed'] = result['equal'] and successful
                report['graph_rows'].append(result)
                print(request['id'], 'passed' if result['passed'] else result['first_difference'], file=sys.stderr, flush=True)
        report['source_changed_during_capture'] = before != inputs()
        report['metrics']['binder_depth'] = (not report['source_changed_during_capture'] and len(report['graph_rows']) == len(requests) and all(row['passed'] for row in report['graph_rows']))
    except (ValueError,RuntimeError,OSError,subprocess.SubprocessError,TimeoutError) as error:
        report['failure'] = str(error)
    (output/'report.json').write_bytes(canonical(report)+b'\n')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,default=ROOT/'target/s07-binder-depth')
    args = parser.parse_args()
    result = capture(args.output)
    print(json.dumps({'metrics':result['metrics'],'failure':result.get('failure'),'graph_rows':len(result['graph_rows'])},sort_keys=True))
    raise SystemExit(0 if result['metrics']['binder_depth'] else 1)
