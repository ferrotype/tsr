"""Join the typed closure to explicit reviewed homes and immutable S07 obligations."""
from collections import Counter

from s04_common import strict_json_loads
from s08_inventory import sites
from s08_oracle import ROOT, canonical, digest

REVIEW = 'tools/s08/inventory/review.json'
IDENTITY_FIELDS = ('caller', 'file', 'line', 'column', 'kind', 'signature', 'value')


def site_identity(site):
    """Decoded source identity; dictionary indices and target resolution are not identity."""
    if any(key not in site for key in IDENTITY_FIELDS):
        raise ValueError('incomplete unresolved callback identity')
    return {key: site[key] for key in IDENTITY_FIELDS}


def reviewed_boundaries(review):
    approved = {}
    for row in review['unresolved_sites']:
        if (set(row) != {'identity', 'boundary'} or set(row['identity']) != set(IDENTITY_FIELDS)
                or row['boundary'] not in review['callback_boundaries']):
            raise ValueError('invalid reviewed callback boundary')
        key = canonical(site_identity(row['identity']))
        if key in approved:
            raise ValueError('duplicate reviewed callback identity')
        approved[key] = row['boundary']
    return approved


def boundary(site, approved=None):
    if approved is None:
        approved = reviewed_boundaries(strict_json_loads((ROOT / REVIEW).read_bytes()))
    key = canonical(site_identity(site))
    if key not in approved:
        raise ValueError('unreviewed unresolved callback: ' + str(site_identity(site)))
    return approved[key]


def prepare(closure):
    review = strict_json_loads((ROOT / REVIEW).read_bytes())
    subset = strict_json_loads((ROOT / 'data/s07/subset.json').read_bytes())
    partition = strict_json_loads((ROOT / 'data/s07/e2-acceptance.json').read_bytes())
    original = strict_json_loads((ROOT / 'data/s07/checker-obligations.json').read_bytes())
    tiers = {r['id']: r['tier'] for r in partition['variants']}
    libraries = {}
    for case in subset['cases']:
        for variant in case['variants']:
            if tiers.get(variant['id']) != 'acceptance':
                continue
            for index in variant['dependency_closure']:
                file = subset['file_observations'][index]
                libraries.setdefault((file['Name'], file['SHA256']), variant['id'])
    definitions = {r['id']: r for r in closure['functions']}
    selectors = {r['selector'] for r in closure['functions']}
    obligations = []
    fixture_ids = {r['id'] for r in strict_json_loads((ROOT / 'tools/s08/contracts/relations.json').read_bytes())['cases']}
    for row in original['obligations']:
        family = row['family']
        home, fixtures = review['families'][family]
        if not set(fixtures) <= fixture_ids:
            raise ValueError('unknown supplemental obligation fixture')
        selector = 'internal/checker::Checker.' + row['pinned_source_anchor']['function']
        if selector not in selectors:
            raise ValueError('obligation source anchor absent from typed closure: ' + selector)
        obligations.append(dict(id=row['id'], family=family, native_selector=selector,
                                home='crates/tsr_checker: ' + home,
                                original_witness_tier=tiers[row['input_witness']],
                                acceptance_library_witness=libraries.get((row['library'], row['source_sha256'])),
                                supplemental_fixtures=fixtures,
                                status='pending_semantic_port',
                                test_status='Go supplemental contract observed; Rust algorithm pending'))
    source_homes = []
    for file in sorted({r['file'] for r in definitions.values()}):
        if not file:
            home, status = 'enclosing generic/function wrapper', 'synthetic_SSA_body'
        elif file.startswith('internal/'):
            home, status = review['packages'][file.split('/')[1]]
        else:
            raise ValueError('unmapped project source: ' + file)
        source_homes.append(dict(file=file, home=home, status=status))
    unresolved = []
    approved = reviewed_boundaries(review)
    seen = set()
    for site in sites(closure):
        if site['targets']:
            continue
        category = boundary(site, approved)
        identity = canonical(site_identity(site))
        if identity in seen:
            raise ValueError('duplicate unresolved callback identity')
        seen.add(identity)
        unresolved.append(dict(site=site['index'], identity_sha256=digest(identity),
                               boundary=category, resolution=review['callback_boundaries'][category]))
    if seen != set(approved):
        raise ValueError('reviewed callback identity absent from unresolved closure')
    interfaces = []
    for interface, members in closure['interfaces'].items():
        for member in members:
            name = member['name']
            if name in review['project_reference_members']:
                status = 'guarded_project_reference_boundary'
                contract = 'Not selected by acceptance; reject unsupported real use explicitly, never supply a false empty answer.'
            elif name == 'BindSourceFiles':
                status = 'bound_input_precondition'
                contract = 'Complete bound roots before checker creation; checker does not mutate binder state.'
            elif interface.startswith('internal/checker::'):
                status = 'trait_declared_compiler_adapter_pending' if name in review['trait_members'] else 'trait_and_adapter_pending'
                contract = 'Port native host semantics over frozen program inputs; no default value stands in for an implementation.'
            elif interface.endswith('SymbolTracker'):
                status = 'trait_declared_implementation_pending'
                contract = 'Implement balanced fallback-node stack, symbol accessibility and diagnostic callbacks alongside the real node builder.'
            else:
                status = 'declaration_diagnostic_path_pending'
                contract = 'Audit actual declaration-transform use at P5. Required callbacks execute; output-only callbacks retain an explicit phase guard. No successful no-op stand-in.'
            implementations = sorted({r['selector'] for r in definitions.values() if r['selector'].endswith('.' + name)})
            interfaces.append(dict(interface=interface, member=name, source=member,
                                   same_named_native_methods=implementations, status=status, contract=contract))
    return dict(version=1, pin=subset['pin'], scope=review['scope'],
                sources={p: digest((ROOT / p).read_bytes()) for p in (REVIEW, 'scripts/s08_audit.py',
                    'data/s07/checker-obligations.json', 'data/s07/e2-acceptance.json',
                    'tools/s08/contracts/relations.json', 'crates/tsr_checker/src/host.rs')},
                closure_sha256=digest(canonical(closure) + b'\n'),
                counts=dict(obligations=len(obligations), families=dict(Counter(r['family'] for r in obligations)),
                            library_obligations_loaded_by_acceptance=sum(r['acceptance_library_witness'] is not None for r in obligations),
                            interface_members=len(interfaces), reviewed_static_boundaries=len(unresolved)),
                obligations=obligations, source_homes=source_homes, interfaces=interfaces, static_boundaries=unresolved,
                validation='Exact partition, all 675 immutable obligation IDs, source anchors, every expanded interface member and every unresolved static site. Matching method names are navigation aids, not proofs of dynamic dispatch.')
