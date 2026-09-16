"""E2 scoring. A complete denominator is mandatory; failures cannot be waived."""
from collections import Counter
from datetime import date
from fnmatch import fnmatchcase
import re

from s08_p4 import canonical
from s08_oracle import digest
import s08_p5_corpus as corpus

METRICS = ('types_parity', 'errors_parity', 'type_to_string_parity')


def display_queries(value):
    if not isinstance(value, dict) or value.get('state') != 'executed':
        return None
    if set(value) != {'state', 'queries'} or not isinstance(value['queries'], list):
        raise ValueError('malformed public TypeToString observation')
    for query in value['queries']:
        if set(query) != {'operation', 'file', 'kind', 'pos', 'end', 'text_hex'}:
            raise ValueError('incomplete public TypeToString query')
        if query['operation'] != 'TypeToString' or not isinstance(query['file'], str):
            raise ValueError('wrong public display operation')
        if any(type(query[k]) is not int for k in ('kind', 'pos', 'end')):
            raise ValueError('invalid display query position/kind')
        corpus.hex_bytes(query['text_hex'])
    return value['queries']


def display_coverage(queries, walked):
    """Observe every nonabsent GetTypeAtLocation result, including duplicates."""
    expected = [{k: q[k] for k in ('file', 'kind', 'pos', 'end')}
                for q in walked if q['operation'] == 'GetTypeAtLocation' and not q.get('absent', False)]
    actual = [{k: q[k] for k in ('file', 'kind', 'pos', 'end')} for q in queries]
    if actual != expected:
        raise ValueError('public TypeToString schedule does not cover the actual type queries')


def inventory(requests, frozen, *, partial=False):
    ids = [r['id'] for r in requests]
    all_ids = [r['id'] for r in frozen]
    selected = set(ids)
    if not ids or len(set(ids)) != len(ids):
        raise ValueError('empty or duplicate E2 inventory')
    if ids != ([i for i in all_ids if i in selected] if partial else all_ids):
        raise ValueError('E2 requires the complete frozen ordered inventory')
    indexed = {r['id']: r for r in frozen}
    for request in requests:
        wanted = indexed[request['id']]
        for key in ('acceptance_tier', 'diagnostic_phases', 'type_baseline_requested'):
            if request[key] != wanted[key]: raise ValueError('E2 frozen request changed: ' + key)
        if digest(canonical(request['loading']) + b'\n') != wanted['loading_request_sha256']:
            raise ValueError('E2 loading input changed')
        if request.get('public_type_strings') is not True or request.get('error_baseline_requested') is not True:
            raise ValueError('E2 requires error baselines and public TypeToString observations')


def approvals(ledger, pin, ids):
    if set(ledger) != {'divergence'} or not isinstance(ledger['divergence'], list):
        raise ValueError('invalid divergence ledger')
    result, seen = {}, set()
    for entry in ledger['divergence']:
        required = {'id', 'title', 'scope', 'kind', 'rationale', 'approved_by', 'approved_on', 'upstream_pin', 'observations'}
        if set(entry) != required or entry['id'] in seen:
            raise ValueError('divergence requires unique identity, approval and exact observation witnesses')
        seen.add(entry['id'])
        if any(not isinstance(entry[k], str) or not entry[k].strip() for k in required - {'scope', 'observations'}):
            raise ValueError('empty divergence metadata')
        date.fromisoformat(entry['approved_on'])
        if entry['upstream_pin'] != pin or entry['kind'] not in ('ordering', 'message', 'position', 'emit', 'other'):
            raise ValueError('wrong divergence pin/kind')
        if not isinstance(entry['scope'], list) or not entry['scope'] or any(not isinstance(s, str) or not s for s in entry['scope']):
            raise ValueError('invalid divergence scope')
        if not isinstance(entry['observations'], list) or not entry['observations']:
            raise ValueError('a scope glob alone cannot approve an output change')
        for witness in entry['observations']:
            if set(witness) != {'variant_id', 'metric', 'native_sha256', 'rust_sha256'}:
                raise ValueError('incomplete divergence witness')
            key = witness['variant_id'], witness['metric']
            if key[0] not in ids or key[1] not in METRICS or key in result or not any(fnmatchcase(key[0], s) for s in entry['scope']):
                raise ValueError('unknown, duplicated or out-of-scope divergence witness')
            if any(not isinstance(witness[k], str) or re.fullmatch('[0-9a-f]{64}', witness[k]) is None for k in ('native_sha256', 'rust_sha256')):
                raise ValueError('invalid divergence output digest')
            result[key] = dict(witness, approval=entry['id'])
    return result


def compare_display(native, row):
    if 'fatal' in row or row['type_symbol_baselines']['state'] != 'executed':
        return {'state': 'failed'}
    expected = display_queries(native.get('public_type_strings'))
    actual = display_queries(row['type_symbol_baselines'].get('public_type_strings'))
    if expected is None or actual is None: return {'state': 'failed'}
    display_coverage(expected, native['queries'])
    display_coverage(actual, row['type_symbol_baselines']['queries'])
    return {'state': 'match' if expected == actual else 'different',
            'native_queries': len(expected), 'rust_queries': len(actual)}


def grade(requests, rows, native, ledger, pin, *, partial=False, approval_ids=None):
    if [r['id'] for r in rows] != [r['id'] for r in requests] or [r['id'] for r in native] != [r['id'] for r in requests]:
        raise ValueError('E2 missing, duplicate or reordered observations')
    approved = approvals(ledger, pin, approval_ids if approval_ids is not None else {r['id'] for r in requests})
    used, results = set(), []
    for request, row, go in zip(requests, rows, native, strict=True):
        corpus.validate_row(request, row)
        corpus.native_metadata(request, go)
        checks = {}
        if go['state'] != 'executed':
            checks = {m: {'state': 'native_unavailable'} for m in METRICS}
        else:
            checks['types_parity'] = corpus.compare_row(go, row)
            checks['errors_parity'] = corpus.compare_errors(go, row)
            checks['type_to_string_parity'] = ({'state': 'disabled'} if checks['types_parity']['state'] == 'disabled'
                                               else compare_display(go, row))
        for metric, check in checks.items():
            check['accepted'] = check['state'] in ('match', 'disabled')
            if check['state'] != 'different': continue
            # Hash exactly the compared domains. Keep synthetic numeric type IDs
            # out of the witness just as compare_row keeps them out of parity.
            if metric == 'types_parity':
                from s08_queries import action
                def domain(value):
                    files = {}
                    queries = [action(q, files) for q in value['queries']]
                    return {k: value[k] for k in ('types', 'symbols')} | {'queries': queries, 'files': files}
                left, right = domain(go), domain(row['type_symbol_baselines'])
            elif metric == 'errors_parity':
                left = {k: go[k] for k in ('error_pre_diagnostics', 'error_post_diagnostics', 'error_diagnostics', 'error_render_inputs', 'error_pretty', 'errors')}
                right = row['error_baseline']
            else:
                left, right = go['public_type_strings'], row['type_symbol_baselines']['public_type_strings']
            check.update(native_sha256=digest(canonical(left)), rust_sha256=digest(canonical(right)))
            key = request['id'], metric
            witness = approved.get(key)
            if witness and all(witness[k] == check[k] for k in ('native_sha256', 'rust_sha256')):
                check.update(accepted=True, approval=witness['approval'])
                used.add(key)
        results.append({'id': request['id'], 'acceptance_tier': request['acceptance_tier'], 'checks': checks})
    acceptance = [r for r in results if r['acceptance_tier'] == 'acceptance']
    acceptance_ids = {r['id'] for r in acceptance}
    metrics = {}
    if not partial:
        if not acceptance: raise ValueError('empty acceptance denominator')
        metrics = {m: sum(r['checks'][m]['accepted'] for r in acceptance) / len(acceptance) for m in METRICS}
        unused_acceptance = {key for key in set(approved) - used if key[0] in acceptance_ids}
        metrics['divergences_approved'] = (not unused_acceptance and all(
            c['accepted'] for r in acceptance for c in r['checks'].values()))
    return {'version': 1, 'partial': partial, 'metrics': metrics, 'rows': results,
            'counts': {tier: {m: dict(Counter(r['checks'][m]['state'] for r in results if r['acceptance_tier'] == tier)) for m in METRICS}
                       for tier in ('acceptance', 'informational')},
            'unused_approvals': [list(k) for k in sorted(set(approved) - used)]}
