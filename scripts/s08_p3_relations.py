"""Validate the complete frozen action inventory without treating P4/P5 gaps as passes."""
import argparse
import itertools
import json
from pathlib import Path
from s08_oracle import canonical, digest, strict_json_loads
from s04 import same_json_value
from s08_contracts import diagnostics, fields, state


def compare(request_raw, native_raw, policy, actual):
    requests, native = strict_json_loads(request_raw), strict_json_loads(native_raw)
    request_hash, native_hash = digest(request_raw), digest(native_raw)
    fields(policy, 'version request_sha256 native_sha256 pending_cases pending_protocol')
    if type(policy['version']) is not int or policy['version'] != 1 or type(actual['version']) is not int or actual['version'] != 1:
        raise ValueError('unsupported relation replay version')
    if policy['request_sha256'] != request_hash or actual['request_sha256'] != request_hash or native['request_sha256'] != request_hash or policy['native_sha256'] != native_hash:
        raise ValueError('relation replay provenance mismatch')
    ids = [row['id'] for row in requests]
    if len(set(ids)) != len(ids) or [row['id'] for row in native['rows']] != ids or [row['id'] for row in actual['rows']] != ids:
        raise ValueError('relation request/observation inventory mismatch')
    if set(policy['pending_cases']) - set(ids):
        raise ValueError('unused pending relation case')
    if policy['pending_protocol'] != ['post_action_display', 'post_action_union_ordering', 'final_state', 'source_and_global_diagnostics']:
        raise ValueError('review required for a changed protocol boundary')
    executed = pending = 0
    pending_rows = []
    for request, expected, observed in zip(requests, native['rows'], actual['rows'], strict=True):
        groups = [(mode,list(actions)) for mode,actions in itertools.groupby(request['actions'],key=lambda a:a['mode'])]
        if len(groups) != len(expected['groups']) or len(groups) != len(observed['groups']):
            raise ValueError('missing relation mode')
        for (mode, actions), go, rust in zip(groups, expected['groups'], observed['groups'], strict=True):
            fields(rust, 'mode observations')
            if rust['mode'] != mode or [a['action'] for a in go['actions']] != actions:
                raise ValueError('relation mode/action drift')
            result = rust['observations']
            if request['id'] in policy['pending_cases']:
                boundary = policy['pending_cases'][request['id']]
                fields(boundary, 'checkpoint operation reason')
                if boundary['checkpoint'] != 'P4' or not boundary['operation']:
                    raise ValueError('pending case lacks a concrete P4 home')
                if result != {'state':'pending','reason':boundary['reason']}:
                    raise ValueError('pending case changed; update its implementation/record')
                pending += len(actions)
                pending_rows.append({'id':request['id'],'mode':mode,**boundary})
                continue
            fields(result, 'before_lookup actions state display_state')
            if result['state'] != 'observed' or result['display_state'] != 'pending':
                raise ValueError('required P3 relation case did not execute')
            state(result['before_lookup'])
            if not same_json_value(result['before_lookup'],go['before_lookup']):
                raise ValueError('relation initialization counters differ')
            if len(result['actions']) != len(actions):
                raise ValueError('missing relation action')
            for action, wanted, found in zip(actions, go['actions'], result['actions'], strict=True):
                fields(found, 'action result ternary_calls before after diagnostics')
                if not same_json_value(found['action'],action):
                    raise ValueError('reordered relation action')
                state(found['before']); state(found['after']); diagnostics(found['diagnostics'])
                if type(found['result']) is not bool or type(found['ternary_calls']) is not list or any(type(v) is not int or v not in (-1,0,1,2,3) for v in found['ternary_calls']):
                    raise ValueError('invalid relation result')
                for key in ('result','ternary_calls','before','after','diagnostics'):
                    if not same_json_value(found[key],wanted[key]):
                        raise ValueError(f"relation mismatch {request['id']}/{mode}/{action['source']}->{action['target']}: {key}")
                executed += 1
    return {'version':1,'scoped_actions_match':True,'full_p0_contract_match':False,
            'executed_actions':executed,'pending_actions':pending,'total_actions':executed+pending,
            'pending_cases':pending_rows,'pending_protocol':policy['pending_protocol'],
            'request_sha256':request_hash,'native_sha256':native_hash,
            'scope':'Exact relation results, ternary calls, state and diagnostics; not the full P0/P7/E2 contract'}


if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('requests','native','policy','actual','output'):
        parser.add_argument('--'+name,type=Path,required=True)
    args=parser.parse_args()
    result=compare(args.requests.read_bytes(),args.native.read_bytes(),strict_json_loads(args.policy.read_bytes()),strict_json_loads(args.actual.read_bytes()))
    result['actual_sha256']=digest(args.actual.read_bytes())
    args.output.write_bytes(canonical(result)+b'\n')
    print(json.dumps({k:v for k,v in result.items() if k!='pending_cases'}))
