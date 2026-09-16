"""Read-only authentication and aggregation for P7 measurement captures.

Metrics are derived from every raw row, never trusted from a cached report.
No child processes are launched by these validators.
"""
import hashlib
import re
from pathlib import Path

from s04_common import strict_json_loads
from s08_oracle import canonical, digest


def file_digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def authenticated(path, expected):
    raw = Path(path).read_bytes()
    if digest(raw) != expected:
        raise ValueError(f'measurement artifact changed: {path}')
    return raw


def number(value, name, *, signed=False):
    if type(value) is not int or (not signed and value < 0):
        raise ValueError(f'invalid measurement counter: {name}')
    return value


def build_record(directory, capture, current_sources, method_path):
    if capture.get('version') != 2:
        raise ValueError('measurement capture needs authenticated P7 protocol 2')
    if capture['method_sha256'] != file_digest(method_path):
        raise ValueError('measurement method changed')
    build = strict_json_loads(authenticated(directory / 'build.json', capture['build_sha256']))
    if digest(canonical(build['sources'])) != capture['sources_sha256'] or build['sources_sha256'] != capture['sources_sha256']:
        raise ValueError('measurement source inventory differs')
    if build['sources'] != current_sources:
        raise ValueError('measurement sources changed; capture is stale')
    for binary in build['binaries'].values():
        if file_digest(binary['path']) != binary['sha256']:
            raise ValueError('measurement executable changed')
    return build


def roster(capture, plan, modes, runtime_key):
    count = capture['samples_per_runtime']
    if type(count) is not int or not 1 <= count <= len(plan['sampling']['measured_order']):
        raise ValueError('invalid measurement sample count')
    if not capture['smoke'] and count != plan['sampling']['measured_samples_per_runtime']:
        raise ValueError('measurement lacks the frozen sample count')
    expected = []
    for mode in modes:
        expected.extend((mode, r.lower(), 'warmup', True) for r in plan['sampling']['warmup_order'])
        for index, pair in enumerate(plan['sampling']['measured_order'][:count]):
            expected.extend((mode, r.lower(), f'sample-{index}', False) for r in pair)
    actual = [(r['mode'], r[runtime_key], r['label'], r['warmup']) for r in capture['runs']]
    if actual != expected:
        raise ValueError('missing, extra, duplicated or reordered measurement samples')


# Type families whose bytes sum to the footprint numerator (data/s08/type-footprint.json).
TYPE_FAMILIES = ('type_records', 'intrinsic', 'literal', 'unique_es_symbol', 'anonymous', 'evolving_arrays', 'reference',
                 'interface', 'tuple', 'union', 'intersection', 'type_parameter', 'template_literal', 'mapped',
                 'reverse_mapped', 'instantiation_expression', 'index', 'indexed_access', 'string_mapping',
                 'substitution', 'conditional', 'alias', 'type_lists', 'type_caches')


def census_invariants(census, inventory, variant):
    """The accounting rules every executed census row must satisfy (type-footprint.json):
    a complete type-family inventory that is the same on every row of the sample,
    non-negative counts and bytes, the family sums, and a consistent type triple."""
    families = census.get('families')
    if not isinstance(families, dict):
        raise ValueError(f'{variant}: census families missing')
    for name in TYPE_FAMILIES:
        if name not in families:
            raise ValueError(f'{variant}: census type family {name} missing')
    for name, family in families.items():
        if not isinstance(family, dict) or number(family.get('count'), name) < 0 or number(family.get('bytes'), name) < 0:
            raise ValueError(f'{variant}: census family {name} has a negative or missing count or bytes')
    if inventory is not None and set(families) != inventory:
        raise ValueError(f'{variant}: census family inventory differs within the sample')
    if number(census['type_storage_bytes'], 'type storage') != sum(families[n]['bytes'] for n in TYPE_FAMILIES):
        raise ValueError(f'{variant}: census type_storage_bytes is not the type-family sum')
    if number(census['checker_bytes'], 'checker bytes') != sum(f['bytes'] for f in families.values()):
        raise ValueError(f'{variant}: census checker_bytes is not the family sum')
    types = census['types']
    created, reachable = number(types['created'], 'created'), number(types['reachable'], 'reachable')
    if reachable > created or number(types.get('unreachable_occupied'), 'unreachable_occupied') != created - reachable:
        raise ValueError(f'{variant}: census reachable/created/unreachable_occupied triple is inconsistent')
    return set(families)


def checker_rows(rows, ids, mode):
    if [row['id'] for row in rows] != ids or len(set(ids)) != len(ids):
        raise ValueError('measurement rows differ from the frozen ordered inventory')
    totals = {'variants': len(ids), 'executed': len(ids), 'failed': 0, 'interval_ns': 0,
              'phases_ns': {p: 0 for p in ('init', 'check', 'display')},
              'allocation': {'requested_bytes': 0, 'retained_bytes': 0},
              'census': {k: 0 for k in ('type_storage_bytes', 'checker_bytes', 'types_reachable', 'types_created', 'unavailable', 'failed')}}
    outputs, actions = hashlib.sha256(), hashlib.sha256()
    inventory = None
    for row in rows:
        if row['outcome'] != 'executed':
            raise ValueError(f"measurement work failed: {row['id']}")
        if not re.fullmatch('[0-9a-f]{64}', row['output_sha256']):
            raise ValueError('invalid output digest')
        for key, value in row['actions'].items():
            number(value, key)
        outputs.update(row['output_sha256'].encode() + b'\n')
        actions.update(canonical(row['actions']) + b'\n')
        interval = number(row['interval_ns'], 'interval_ns')
        if interval == 0:
            raise ValueError('checker interval must be positive')
        totals['interval_ns'] += interval
        if mode == 'phase':
            for name in totals['phases_ns']:
                totals['phases_ns'][name] += number(row['phases_ns'][name], name)
        if mode != 'alloc':
            continue
        allocation = row['allocation']
        totals['allocation']['requested_bytes'] += number(allocation['requested_bytes'], 'requested bytes')
        delta = number(allocation['live_at_checkpoint'], 'live checkpoint') - number(allocation['live_before_interval'], 'live before')
        totals['allocation']['retained_bytes'] += delta
        number(allocation['live_after_release'], 'live after release')
        census = row['checkpoint']['census']
        if census.get('state') == 'failed':
            totals['census']['failed'] += 1
            continue
        unavailable = census['unavailable']
        if not isinstance(unavailable, list) or any(not isinstance(s, str) or not s for s in unavailable):
            raise ValueError('invalid unavailable census families')
        inventory = census_invariants(census, inventory, row['id'])
        totals['census']['unavailable'] += len(unavailable)
        for name in ('type_storage_bytes', 'checker_bytes'):
            totals['census'][name] += number(census[name], name)
        for name in ('reachable', 'created'):
            totals['census']['types_' + name] += number(census['types'][name], name)
    totals['outputs_sha256'] = outputs.hexdigest()
    totals['actions_sha256'] = actions.hexdigest()
    return totals


def check_totals(actual, expected, prefix='totals'):
    """Compare only independently reconstructable counters; runtime metadata stays raw."""
    for key, value in expected.items():
        name = prefix + '.' + key
        if key not in actual:
            raise ValueError(f'missing {name}')
        if isinstance(value, dict):
            check_totals(actual[key], value, name)
        elif type(actual[key]) is not type(value) or actual[key] != value:
            raise ValueError(f'{name} differs from raw observations')
